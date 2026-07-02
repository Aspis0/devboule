//! Mini-coder EDIT APPLICATION — the single disk-writer of the emit-edits path.
//! Extracted VERBATIM from `mini_coder_executor.rs` (role-untangle Phase 2, pure
//! move): path normalization + the allowlist/traversal/symlink guards, the
//! exact/whitespace/fuzzy anchor-match tiers, and the two-pass (validate-all then
//! write-all) `apply_emitted_edits` / `apply_write_directive_edits`. The model
//! NEVER touches disk — this module applies its structured edits. The dense test
//! battery stays in `mini_coder_executor.rs` and exercises these items via the
//! wildcard re-export there (characterization guard for the move).

use std::path::Path;

use super::mini_coder::{self, MiniCoderDirective, MiniCoderOutcome, MiniCoderStatus};

/// P1 (KILL-WINDOW GAP): does a LIVE Stop win over the gate's `FailedWith` decision?
/// The `Escalate`/`StampTerminal` arms re-consult the live kill via [`live_kill_override`],
/// but the retry arm has no terminal outcome to override — so this guard answers the same
/// question (using the IDENTICAL "aborts" predicate: a flagged `kill_requested` OR a `stop`
/// sentinel that reached the live `steer_queue` out-of-band) for the retry arm. When true,
/// the arm aborts the chain (`aborted_by_human` + propagate) and spawns NO retry, instead of
/// parking the predecessor at `Failed` and launching a fresh `Pending` attempt that
/// would run despite the human's Stop. Pure (reads the live state under the caller's lock) so
/// the P1 race is directly unit-testable without an AppHandle, mirroring `live_kill_override`.
pub(crate) fn normalize_edit_rel(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// P4: hard cap on emitted edits per result (runaway-model-proof).
pub(crate) const MAX_MINI_EDITS: usize = 40;
/// P4: the plan's N<=10 cap on the ordered file-set allowlist of a write directive.
pub(crate) const MAX_MINI_ALLOWLIST_FILES: usize = 10;

/// Which tier of the apply cascade matched a non-CREATE edit. Recorded per applied
/// edit so the training-snapshot path can see when (and how confidently) the fuzzy
/// fallback "saved" an edit — signal for teaching the mini to emit cleaner anchors.
///
/// `Fuzzy` carries the winning window's similarity ratio (0..=1) so the flywheel can
/// distinguish a borderline 0.92 save from a near-exact 0.99 one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MatchTier {
    /// `old_string` occurred verbatim exactly once (the original contract).
    Exact,
    /// 0 exact matches, but exactly one span was whitespace-normalization-equal.
    Whitespace,
    /// Last resort: a line-aligned window whose difflib-style ratio cleared the bar.
    Fuzzy(f32),
}

impl MatchTier {
    /// Stable tag for diagnostics / training records. `fuzzy` carries the ratio
    /// rounded to 2 decimals so the string is compact yet preserves the signal.
    fn label(&self) -> String {
        match self {
            MatchTier::Exact => "exact".to_string(),
            MatchTier::Whitespace => "whitespace".to_string(),
            MatchTier::Fuzzy(ratio) => format!("fuzzy:{ratio:.2}"),
        }
    }
}

/// Minimum CHARACTER-level difflib ratio (similar::TextDiff::from_chars(...).ratio())
/// for the Tier-3 fuzzy fallback to even be considered. 0.92 mirrors Aider's near-miss
/// threshold — high enough that a window differing only in a few characters/whitespace
/// passes (e.g. a 75-char block with one changed char scores ~0.99), low enough that a
/// genuinely different block (different identifiers, reordered/rewritten lines) does
/// not (a structurally-unrelated block scores well under 0.5).
pub(crate) const FUZZY_MATCH_MIN_RATIO: f32 = 0.92;

/// Required separation between the best and second-best fuzzy window ratios. Without
/// it, two near-identical candidate blocks (e.g. two copies of the same helper that
/// each drifted slightly) could both clear the bar and we'd splice an arbitrary one —
/// silent corruption. Demanding a clear winner means "two plausible spots" => ERROR,
/// honoring the conservative "error rather than mis-apply" bias.
pub(crate) const FUZZY_MATCH_MIN_MARGIN: f32 = 0.05;

/// How far above/below the `old_string` line-count the Tier-3 window size is allowed
/// to flex. Whitespace/indent drift rarely changes the line COUNT, but a stray added
/// or removed blank line can; +/-1 covers that without exploding the search space.
pub(crate) const FUZZY_WINDOW_LINE_DELTA: usize = 1;

/// Above this file size the Tier-3 fuzzy fallback is SKIPPED entirely (the exact and
/// whitespace tiers still run). Tier 3 is O(windows x Myers-diff); on a large file with a
/// large mismatched `old_string` that can pin the Tauri backend thread for seconds — a
/// self-inflicted DoS. A large file needs a precise exact/whitespace anchor; we refuse to
/// guess across it. 256 KiB comfortably covers normal source files.
pub(crate) const FUZZY_MAX_FILE_BYTES: usize = 256 * 1024;

/// Per-diff wall-clock cap for a single Tier-3 character diff. `similar`'s Myers
/// implementation honors this deadline and approximates past it, so one pathological
/// window cannot run unbounded. 250 ms is far above any normal-size diff (microseconds)
/// yet bounds the worst case; the `.ratio()` metric is unchanged for normal inputs.
pub(crate) const FUZZY_DIFF_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Byte offset of the start of each line in `text`, plus a trailing sentinel equal to
/// `text.len()`. So line `k` (0-based) spans `starts[k]..starts[k+1]` INCLUDING its
/// own `\n`, and there are `starts.len() - 1` lines — matching `text.split('\n')`
/// semantics: a text ending in `\n` has a final EMPTY line (`"a\nbc\n"` -> offsets
/// `[0, 2, 5, 5]`, three lines `"a\n"`, `"bc\n"`, `""`). That phantom empty line is
/// harmless for the matchers (it normalizes to "" and scores ~0 against any non-empty
/// `old`). An empty `text` yields `[0, 0]` (one empty line).
pub(crate) fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts.push(text.len());
    starts
}

/// Whitespace-normalize a block for Tier-2 comparison WITHOUT crossing line
/// boundaries: per line, trim leading/trailing horizontal whitespace and collapse
/// internal runs of horizontal whitespace (spaces/tabs) to a single space; lines are
/// rejoined with `\n`. A single trailing `\n` (if any) is dropped FIRST, so a block
/// that ends in a newline and one that does not normalize identically — the splice
/// span (not this string) decides whether the trailing newline is consumed. This
/// neutralizes tabs-vs-spaces, trailing spaces, and indent drift while PRESERVING line
/// structure (a one-line block can never normalize-equal a two-line block). Only used
/// to LOCATE the span — the splice uses the original bytes, never this normalized form.
pub(crate) fn normalize_ws_block(block: &str) -> String {
    block
        .strip_suffix('\n')
        .unwrap_or(block)
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The line-count `old` spans and whether it carries a trailing `\n`. A trailing
/// newline does NOT add an extra (empty) line: `"a\nb"` and `"a\nb\n"` both span 2
/// lines; the flag drives whether the matched window's span consumes the last line's
/// own `\n`. An empty `old` is never passed here (CREATE is handled earlier).
pub(crate) fn old_block_shape(old: &str) -> (usize, bool) {
    let ends_nl = old.ends_with('\n');
    let body = old.strip_suffix('\n').unwrap_or(old);
    (body.split('\n').count(), ends_nl)
}

/// ORIGINAL byte span in `text` for the `win`-line window starting at line `first`,
/// given line-start offsets `starts` (sentinel-terminated, see [`line_start_offsets`]).
/// When `consume_trailing_nl` the span includes the last line's own `\n`
/// (`starts[first+win]`); otherwise it ends at that line's content (before its `\n`),
/// mirroring how an exact `old` without a trailing newline would match a whole-line
/// block yet leave the following `\n` in place. Caller guarantees `first+win <=
/// line_count`.
pub(crate) fn window_span(
    starts: &[usize],
    text: &str,
    first: usize,
    win: usize,
    consume_trailing_nl: bool,
) -> std::ops::Range<usize> {
    let start = starts[first];
    let raw_end = starts[first + win];
    if consume_trailing_nl {
        return start..raw_end;
    }
    // Trim exactly one trailing '\n' (the last line's terminator), if present.
    let end = if raw_end > start && text.as_bytes()[raw_end - 1] == b'\n' {
        raw_end - 1
    } else {
        raw_end
    };
    start..end
}

/// Tier 2: find the unique line-aligned span of `text` whose whitespace-normalized
/// form equals the whitespace-normalized `old`. Returns the ORIGINAL byte span
/// `start..end` (so the caller splices real bytes) only when EXACTLY ONE window
/// matches; `None` for zero or (ambiguity guard) more than one. `old` is assumed
/// non-empty (the CREATE branch is handled before any matching).
pub(crate) fn find_whitespace_span(text: &str, old: &str) -> Option<std::ops::Range<usize>> {
    let target = normalize_ws_block(old);
    // A whitespace-only `old` normalizes to "" — and so does the phantom trailing empty
    // line of a `\n`-terminated file. Without this guard Tier 2 would "match" that empty
    // line and return an EMPTY span at EOF, turning the splice into an INSERT of
    // `new_string` at end-of-file (silent corruption: the real target is left untouched).
    // An all-whitespace anchor is never a valid locator, so decline and let Tier 3 (which
    // also cannot confidently match) produce the correct "no confident match" error.
    if target.is_empty() {
        return None;
    }
    let (win, ends_nl) = old_block_shape(old);
    let starts = line_start_offsets(text);
    let line_count = starts.len() - 1;
    if win == 0 || win > line_count {
        return None;
    }
    let mut hit: Option<std::ops::Range<usize>> = None;
    for first in 0..=(line_count - win) {
        let span = window_span(&starts, text, first, win, ends_nl);
        if normalize_ws_block(&text[span.clone()]) == target {
            if hit.is_some() {
                // A second normalized match => ambiguous => Tier 2 declines (the
                // caller then tries Tier 3, which has its own ambiguity guard).
                return None;
            }
            hit = Some(span);
        }
    }
    hit
}

/// Half-open range overlap: `true` iff `a` and `b` share at least one byte. Two
/// line-aligned windows that start on the SAME line but use different window sizes
/// (`base-1`/`base`/`base+1`) overlap — they are the SAME physical region rescored, not
/// competing match locations, so they must not feed the Tier-3 ambiguity margin.
pub(crate) fn spans_overlap(a: &std::ops::Range<usize>, b: &std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Tier 3 (last resort): slide line-aligned windows of `old`'s line-count +/-
/// [`FUZZY_WINDOW_LINE_DELTA`] over `text`, scoring each with a difflib-style ratio
/// (`similar::TextDiff` at CHARACTER granularity, so a few differing characters in a
/// multi-line block still scores near 1.0; a line-granular ratio would top out at ~`1 -
/// 1/lines` for a single changed line and never clear the 0.92 bar). Each diff is bounded
/// by [`FUZZY_DIFF_TIMEOUT`], and the whole tier is SKIPPED for files larger than
/// [`FUZZY_MAX_FILE_BYTES`] (DoS guard). Returns the winning ORIGINAL byte span ONLY when
/// the best ratio is >= [`FUZZY_MATCH_MIN_RATIO`] AND it beats the best NON-OVERLAPPING
/// runner-up by >= [`FUZZY_MATCH_MIN_MARGIN`]; otherwise `None` (below threshold OR
/// ambiguous => the caller errors). The runner-up must be a DISJOINT region, not the same
/// region rescored at an adjacent window size. Conservative by construction: "no
/// confident, unambiguous winner" is always a refusal, never a guess.
pub(crate) fn find_fuzzy_span(text: &str, old: &str) -> Option<(std::ops::Range<usize>, f32)> {
    // DoS guard: never run the O(windows x Myers) fuzzy scan over a large file. Exact and
    // whitespace tiers already ran; a large file needs a precise anchor, not a guess.
    if text.len() > FUZZY_MAX_FILE_BYTES {
        return None;
    }
    let starts = line_start_offsets(text);
    let line_count = starts.len() - 1;
    let (base, ends_nl) = old_block_shape(old);
    if base == 0 || line_count == 0 {
        return None;
    }
    // Candidate window sizes: base, then +/-1 (clamped to >=1 and <= line_count).
    let lo = base.saturating_sub(FUZZY_WINDOW_LINE_DELTA).max(1);
    let hi = (base + FUZZY_WINDOW_LINE_DELTA).min(line_count);
    // Track best and second-best across ALL windows of ALL sizes. `second` only counts
    // a ratio whose SPAN does NOT OVERLAP the current best's span: an overlapping window
    // is the SAME physical region rescored at a different size (base / base+/-1 over the
    // same start line), so letting it feed the margin would make a single genuine match
    // defeat its own ambiguity guard. Only a disjoint region is a competing location.
    let mut best: Option<(f32, std::ops::Range<usize>)> = None;
    let mut second: Option<f32> = None;
    for win in lo..=hi {
        if win > line_count {
            continue;
        }
        for first in 0..=(line_count - win) {
            let span = window_span(&starts, text, first, win, ends_nl);
            let ratio = similar::TextDiff::configure()
                .timeout(FUZZY_DIFF_TIMEOUT)
                .diff_chars(&text[span.clone()], old)
                .ratio();
            match best.as_ref() {
                Some((best_ratio, best_span)) if *best_ratio >= ratio => {
                    if !spans_overlap(&span, best_span) && second.map(|s| ratio > s).unwrap_or(true)
                    {
                        second = Some(ratio);
                    }
                }
                _ => {
                    // New best. Demote the old best to second-best only if it is a
                    // NON-OVERLAPPING span (else it was the same region rescored — discard).
                    if let Some((old_ratio, old_span)) = best.replace((ratio, span.clone())) {
                        if !spans_overlap(&old_span, &span)
                            && second.map(|s| old_ratio > s).unwrap_or(true)
                        {
                            second = Some(old_ratio);
                        }
                    }
                }
            }
        }
    }
    let (best_ratio, best_span) = best?;
    if best_ratio < FUZZY_MATCH_MIN_RATIO {
        return None;
    }
    // Unambiguity: a clear winner over the runner-up DISTINCT window.
    if let Some(second_ratio) = second {
        if best_ratio - second_ratio < FUZZY_MATCH_MIN_MARGIN {
            return None;
        }
    }
    Some((best_span, best_ratio))
}

/// The apply cascade for a NON-empty `old_string`: Exact -> Whitespace -> Fuzzy.
/// Returns the ORIGINAL byte span to splice plus which tier won, or a structured
/// `Err` (the `i`/`rel` context is added by the caller). The exact tier preserves the
/// original exact-ONCE contract: >1 verbatim hits is an ambiguity ERROR that does NOT
/// fall through to fuzzy. Find-then-splice: the caller does `replace_range(span, new)`
/// on this span — never `replacen`, which would re-find literally and break Tiers 2/3.
pub(crate) fn locate_edit_span(text: &str, old: &str) -> Result<(std::ops::Range<usize>, MatchTier), String> {
    // Tier 1 — exact, verbatim, exactly once.
    let exact = text.matches(old).count();
    if exact == 1 {
        let start = text.find(old).expect("count==1 implies find");
        return Ok((start..start + old.len(), MatchTier::Exact));
    }
    if exact > 1 {
        // Ambiguous exact match: refuse rather than fuzzy-guess which one.
        return Err(format!("oldString matches {exact} times (need exactly 1)"));
    }
    // Tier 2 — whitespace-normalized, unambiguous.
    if let Some(span) = find_whitespace_span(text, old) {
        return Ok((span, MatchTier::Whitespace));
    }
    // Tier 3 — similarity ratio, confident + unambiguous.
    if let Some((span, ratio)) = find_fuzzy_span(text, old) {
        return Ok((span, MatchTier::Fuzzy(ratio)));
    }
    Err("oldString matches 0 times (no confident exact, whitespace, or fuzzy match)".to_string())
}

/// P4: validate and apply the mini's emitted edits inside `project_root`,
/// bounded by the directive's ordered file-set allowlist. The model NEVER
/// touches disk — this is the only writer, so every guard lives here:
///   - rel-path hygiene via `validate_rel_path` (rejects `..`, absolute, drive
///     prefixes, `-`-leading components);
///   - allowlist containment by EXACT byte match after `\` -> `/` normalization
///     (deliberate for APFS: a case-variant alias of an allowlisted file is
///     rejected on EVERY platform, so macOS and Linux CI agree);
///   - symlink escape: an existing target must canonicalize INSIDE the
///     canonical root; a created file's PARENT must already exist and
///     canonicalize inside the root (no implicit directory creation);
///   - exact-match anchors: a non-empty `old_string` must occur EXACTLY ONCE
///     in the file's CURRENT working text (prior edits of the same batch
///     included); an empty `old_string` means CREATE with `new_string` as the
///     full content, valid only when the file does not exist yet;
///   - per-call ATOMICITY against MODEL errors: pass 1 validates every edit
///     against an in-memory copy, so any validation failure -> Err with
///     NOTHING written. A pass-2 OS-level write error (disk full, perms) can
///     still leave earlier files flushed — that partial state is reported in
///     the Err and surfaces as a `failed` outcome for the coder to inspect.
/// `pre_write(rel)` runs once per touched file just before its flush so the
/// caller can snapshot the pre-image (training rail). Residual TOCTOU between
/// the passes is accepted: the threat model is the MODEL's output, not a
/// concurrent local attacker.
/// Return type of `apply_emitted_edits`: the ordered list of touched relative paths paired
/// with the per-file (old_content, new_content) captured during PASS 1. The two `Vec`s are
/// parallel — `applied[i]` is the path, `snapshots[i]` is its `(old, new)` content pair.
/// `old_content` is `""` for a file created by this batch (empty `old_string` edit).
#[derive(Debug)]
pub(crate) struct ApplyResult {
    /// Relative paths of the files that were actually written, in apply order.
    pub(crate) applied: Vec<String>,
    /// Per-file `(old_content, new_content)` parallel to `applied`.
    pub(crate) snapshots: Vec<(String, String)>,
    /// Observability for the fuzzy fallback: one `(rel, tier_label)` per NON-CREATE
    /// edit that matched, in edit order. `tier_label` is `exact` | `whitespace` |
    /// `fuzzy:<ratio>` (see [`MatchTier::label`]). CREATE edits (empty `old_string`)
    /// produce no entry — there is no anchor to match. The training-snapshot path
    /// reads this to learn when (and how confidently) fuzzy "saved" an edit.
    pub(crate) match_tiers: Vec<(String, String)>,
}

pub(crate) fn apply_emitted_edits(
    project_root: &Path,
    allowlist: &[String],
    edits: &[mini_coder::MiniEdit],
    mut pre_write: impl FnMut(&str),
) -> Result<ApplyResult, String> {
    if edits.is_empty() {
        return Ok(ApplyResult {
            applied: Vec::new(),
            snapshots: Vec::new(),
            match_tiers: Vec::new(),
        });
    }
    if edits.len() > MAX_MINI_EDITS {
        return Err(format!(
            "too many edits: {} (cap {MAX_MINI_EDITS})",
            edits.len()
        ));
    }
    if allowlist.is_empty() || allowlist.len() > MAX_MINI_ALLOWLIST_FILES {
        return Err(format!(
            "write directives need an allowlist of 1..={MAX_MINI_ALLOWLIST_FILES} files, got {}",
            allowlist.len()
        ));
    }
    let canon_root = std::fs::canonicalize(project_root)
        .map_err(|e| format!("project root does not canonicalize: {e}"))?;
    // P4 (review F1): BOTH sides go through the same lexical normalizer, or a
    // cosmetic variant on either side ("./src/a.rs" in the directive vs
    // "src/a.rs" emitted, or vice versa) silently fails the whole write.
    let allowed: std::collections::BTreeSet<String> =
        allowlist.iter().map(|f| normalize_edit_rel(f)).collect();

    // PASS 1 — validate in memory; nothing touches disk until every edit of
    // every file checks out.
    let mut contents: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    // Parallel map: old content captured at the moment a file is FIRST loaded (before any
    // edits). Empty string for a CREATE (the file did not exist before this batch).
    let mut old_contents: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    // Per-edit match-tier record (NON-CREATE edits only), in edit order — observability
    // for the fuzzy fallback (which tier saved the edit, at what ratio).
    let mut match_tiers: Vec<(String, String)> = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let rel = normalize_edit_rel(&edit.path);
        if rel.is_empty() {
            return Err(format!("edit {i}: empty path"));
        }
        super::censor::ledger::validate_rel_path(&rel).map_err(|e| format!("edit {i}: {e}"))?;
        if !allowed.contains(&rel) {
            return Err(format!("edit {i}: {rel} is not in the directive allowlist"));
        }
        let abs = canon_root.join(&rel);
        if !contents.contains_key(&rel) {
            if edit.old_string.is_empty() {
                // CREATE: must not exist (symlink_metadata also catches a
                // dangling symlink squatting on the name), parent must already
                // exist and resolve inside the root.
                if abs.symlink_metadata().is_ok() {
                    return Err(format!(
                        "edit {i}: {rel} already exists (empty oldString means create)"
                    ));
                }
                let parent = abs
                    .parent()
                    .ok_or_else(|| format!("edit {i}: {rel} has no parent directory"))?;
                let canon_parent = std::fs::canonicalize(parent)
                    .map_err(|_| format!("edit {i}: parent directory of {rel} does not exist"))?;
                if !canon_parent.starts_with(&canon_root) {
                    return Err(format!("edit {i}: {rel} escapes the project root"));
                }
                // CREATE: old content is empty (file did not exist).
                old_contents.insert(rel.clone(), String::new());
                contents.insert(rel.clone(), edit.new_string.clone());
                order.push(rel.clone());
                continue;
            }
            let canon_target = std::fs::canonicalize(&abs)
                .map_err(|_| format!("edit {i}: {rel} does not exist"))?;
            if !canon_target.starts_with(&canon_root) {
                return Err(format!("edit {i}: {rel} escapes the project root"));
            }
            let text = std::fs::read_to_string(&canon_target)
                .map_err(|e| format!("edit {i}: cannot read {rel}: {e}"))?;
            // Capture the pre-edit content before any mutations.
            old_contents.insert(rel.clone(), text.clone());
            contents.insert(rel.clone(), text);
            order.push(rel.clone());
        } else if edit.old_string.is_empty() {
            // A second empty-oldString edit on a file this batch already
            // created or loaded is always invalid.
            return Err(format!(
                "edit {i}: duplicate create for {rel} (empty oldString)"
            ));
        }
        let text = contents.get_mut(&rel).expect("inserted above");
        // Tiered match cascade (Exact -> Whitespace-normalized -> Similarity ratio).
        // Conservative by construction: an ambiguous exact match, or no confident +
        // unambiguous fuzzy/whitespace span, is an Err here — so PASS 2 never runs and
        // NOTHING is written (atomicity preserved). Find-then-splice on the located
        // ORIGINAL byte span via `replace_range` (NOT `replacen`, which would re-find
        // the literal `old_string` and break the whitespace/fuzzy tiers).
        let (span, tier) = locate_edit_span(text, &edit.old_string)
            .map_err(|e| format!("edit {i}: {e} in {rel}"))?;
        text.replace_range(span, &edit.new_string);
        match_tiers.push((rel.clone(), tier.label()));
    }

    // PASS 2 — flush, one write per touched file, pre-image hook first.
    for rel in &order {
        pre_write(rel);
        let abs = canon_root.join(rel);
        std::fs::write(&abs, contents[rel].as_bytes()).map_err(|e| format!("write {rel}: {e}"))?;
    }

    // Build parallel (old, new) snapshot vec in the same order as `order`.
    let snapshots: Vec<(String, String)> = order
        .iter()
        .map(|rel| {
            let old = old_contents.remove(rel).unwrap_or_default();
            let new = contents.remove(rel).unwrap_or_default();
            (old, new)
        })
        .collect();

    Ok(ApplyResult {
        applied: order,
        snapshots,
        match_tiers,
    })
}

/// P4: consume a finished mini's emitted edits. Returns the outcome to stamp:
///   - no edits -> unchanged;
///   - edits on a NON-write directive, or on a non-`done` outcome -> edits are
///     DROPPED (the model is untrusted; only a write directive's clean done may
///     touch disk) and the outcome passes through;
///   - write + done -> validate + apply via `apply_emitted_edits`; on success
///     `files_touched` becomes the APPLIED set (ground truth — the verdict gate
///     lints what actually changed, not what the model claims) and the edit
///     bodies are cleared; on failure the done converts to a synthesized
///     `failed` carrying the per-edit error (atomicity means nothing was
///     written, so there is no half-applied tree to lint).
/// Pre-images of every touched file land in the training blob store first.
///
/// Returns `(outcome, write_diffs)` where `write_diffs` is a per-file
/// `(path, Vec<DiffLine>)` list for the Activity Console — one entry per applied file,
/// in apply order. Empty on every non-apply path (no edits, non-write, failed apply).
pub(crate) fn apply_write_directive_edits(
    project_root: Option<&Path>,
    directive: &MiniCoderDirective,
    mut outcome: MiniCoderOutcome,
) -> (
    MiniCoderOutcome,
    Vec<(String, Vec<super::mini_activity::DiffLine>)>,
) {
    use super::mini_activity::build_file_diff;

    if outcome.edits.is_empty() {
        // P4 (review F6): a one-shot emit-edits write directive that emitted NO edits changed
        // NOTHING — zero the model-claimed files_touched, or the verdict gate would lint (and
        // spuriously retry on) files the mini never touched.
        // EXCEPTION: the AGENTIC path applies its edits via tools, so empty `edits` is EXPECTED
        // and its files_touched (set from the loop's own tracking) is real — keep it for Censor.
        if directive.write
            && directive.write_mode != mini_coder::WriteMode::AgenticIterative
            && outcome.status == MiniCoderStatus::Done
        {
            outcome.files_touched = Vec::new();
        }
        return (outcome, Vec::new());
    }
    if !directive.write || outcome.status != MiniCoderStatus::Done {
        outcome.edits = Vec::new();
        return (outcome, Vec::new());
    }
    let Some(root) = project_root else {
        return (
            MiniCoderOutcome::failed(
                "write directive finished without a resolvable project root".to_string(),
            ),
            Vec::new(),
        );
    };
    let edits = std::mem::take(&mut outcome.edits);
    // P7: keep the pre-image hashes — for a fix pass they ARE the previous
    // attempt's output, i.e. the "rejected" side of the ORPO pair.
    let mut preimages: Vec<(String, String)> = Vec::new();
    match apply_emitted_edits(root, &directive.files, &edits, |rel| {
        if let Some(hash) = crate::backend::training_export::snapshot_blob(root, &root.join(rel)) {
            preimages.push((rel.to_string(), hash));
        }
    }) {
        Ok(ApplyResult {
            applied,
            snapshots,
            match_tiers,
        }) => {
            crate::backend::training_export::record_write_preimages(
                root,
                directive,
                &preimages,
                &match_tiers,
            );
            // Build per-file diffs from the captured (old, new) content pairs.
            let write_diffs: Vec<(String, Vec<super::mini_activity::DiffLine>)> = applied
                .iter()
                .zip(snapshots.iter())
                .map(|(path, (old, new))| (path.clone(), build_file_diff(path, old, new)))
                .collect();
            outcome.files_touched = applied;
            (outcome, write_diffs)
        }
        Err(e) => (
            MiniCoderOutcome::failed(format!("emitted edits rejected: {e}")),
            Vec::new(),
        ),
    }
}
