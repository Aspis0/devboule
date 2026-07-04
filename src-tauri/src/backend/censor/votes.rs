//! k-sample self-consistency voting for the Censor local-LLM review path.
//!
//! When the user configures more than one sample (`n_samples > 1`), Censor asks the
//! local model to review the SAME file several times (each at a non-zero temperature so
//! the samples differ) and keeps only the smells the model reports CONSISTENTLY. This
//! module is the PURE, IO-free core of that: it clusters the per-sample findings by
//! proximity, counts how many DISTINCT samples agree on each cluster, and splits the
//! clusters into "confirmed" (enough votes to block) and "suspects" (fewer votes — worth
//! a second look by the verifier role, but flagged as unverified).
//!
//! Nothing here talks to a model or the network — it operates on already-parsed
//! [`RawFinding`]s produced by `gemma::parse_gemma` / `gemma::parse_censor_v2`, so it is
//! trivially unit-testable and deterministic.

use super::gemma::MAX_GEMMA_FINDINGS;
use super::runners::RawFinding;
use super::schema::Severity;

/// Line key used for a file-level finding (`RawFinding.line == None`). Deliberately far
/// below any real 1-based source line so file-level findings only ever cluster with each
/// OTHER under the step-3 proximity tolerance. (The step-3b drifted-assertion merge is
/// line-blind, so a file-level cluster CAN later merge with a numbered one when the text
/// matches — the representative then prefers a member that carries a line, see
/// [`finalize_cluster`].) The diff arithmetic uses `saturating_sub` so the huge gap
/// between this sentinel and a real line never overflows.
const FILE_LEVEL_LINE: i64 = i64::MIN;

/// Tunable knobs for the voting pass. Resolved from `CensorLocalAi` (see
/// `CensorLocalAi::review_params`); the [`Default`] is the LEGACY single-sample behavior
/// (one sample, one vote confirms, tolerance ±2) so a config that never opts in votes
/// exactly like the pre-voting engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoteParams {
    /// How many times the model reviews the file. `1` = legacy single pass (no voting).
    pub n_samples: u8,
    /// A cluster with at least this many DISTINCT-sample votes is CONFIRMED (blocking).
    pub min_votes_block: u8,
    /// A cluster with at least this many votes (but fewer than [`min_votes_block`]) is a
    /// SUSPECT: surfaced for the verifier role, flagged `[unverified …]`.
    pub min_votes_verify: u8,
    /// Two findings join the same cluster when their line numbers differ by at most this
    /// many lines (the model rarely pins the SAME smell to the exact same line across
    /// samples, so a small tolerance keeps agreeing reports together).
    pub line_tolerance: i64,
}

impl Default for VoteParams {
    fn default() -> Self {
        Self {
            n_samples: 1,
            min_votes_block: 1,
            min_votes_verify: 1,
            line_tolerance: 2,
        }
    }
}

/// One clustered smell plus how many distinct samples agreed on it. `cluster_lines`
/// carries the line key of every finding folded into the cluster (a file-level finding
/// contributes [`FILE_LEVEL_LINE`]); it exists mostly for observability / tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VotedFinding {
    /// The representative finding: the highest-severity member of the cluster (ties
    /// resolved to the first encountered in clustering order).
    pub finding: RawFinding,
    /// Number of DISTINCT samples that reported a finding in this cluster.
    pub votes: u8,
    /// The line key of each finding in the cluster (sorted ascending).
    pub cluster_lines: Vec<i64>,
}

/// Minimum word-set Jaccard similarity between two findings' TITLES for the drifted-
/// assertion merge (see [`merge_drifted_assertions`]). Tuned on the live-e2e failure it
/// fixes: "Error handling in clean_workflow_name" vs "Potential error handling issue in
/// `clean_workflow_name`" scores ≈0.67; the distinct "Magic number in overhead/token/
/// context-window calculation" style nits score ≈0.43 and stay separate.
const TITLE_MERGE_JACCARD: f64 = 0.6;

/// Minimum word-set Jaccard between two findings' BODIES for the same merge. Looser than
/// the title bar — bodies paraphrase more (the live pair scores ≈0.45) — but still blocks
/// merging same-title findings whose rationales describe different things.
const BODY_MERGE_JACCARD: f64 = 0.4;

/// Higher = more severe. Used to pick a cluster's representative finding.
fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::High => 2,
        Severity::Medium => 1,
        Severity::Low => 0,
    }
}

/// The clustering key for a finding: its 1-based line, or [`FILE_LEVEL_LINE`] when the
/// finding is file-level (no line).
fn line_key(f: &RawFinding) -> i64 {
    f.line.map(|l| l as i64).unwrap_or(FILE_LEVEL_LINE)
}

/// Cluster the findings from all samples by line proximity and count distinct-sample
/// votes per cluster. PURE.
///
/// Algorithm:
///   1. collect every `(line_key, sample_idx, finding)` across all samples;
///   2. sort by line key (stable — preserves per-sample encounter order within equal
///      lines, so the tie-break below is deterministic);
///   3. greedy proximity clustering ANCHORED to the cluster's FIRST line: a finding joins
///      the CURRENT cluster when its line minus the cluster's FIRST line is
///      `<= line_tolerance`, otherwise it starts a new cluster. Anchoring to the first line
///      (not the last) prevents single-linkage chaining — e.g. `10,12,14,16,18` with
///      `tol=2` no longer drifts into one giant cluster;
///   4. each cluster's `votes` = the number of DISTINCT sample indices in it (two
///      findings from the SAME sample landing in one cluster count as ONE vote);
///   5. the representative finding is the PLURALITY one — the normalized title reported by
///      the most distinct samples (see [`finalize_cluster`]) — so one hallucinated outlier
///      cannot hijack a majority's vote count.
pub fn cluster_and_vote(samples: Vec<Vec<RawFinding>>, p: &VoteParams) -> Vec<VotedFinding> {
    // 1. Collect every finding tagged with its line key and originating sample index.
    let mut items: Vec<(i64, usize, RawFinding)> = Vec::new();
    for (idx, sample) in samples.into_iter().enumerate() {
        for f in sample {
            let key = line_key(&f);
            items.push((key, idx, f));
        }
    }
    if items.is_empty() {
        return Vec::new();
    }
    // 2. Stable sort by line key so equal-line findings keep their per-sample encounter
    //    order (makes the representative tie-break deterministic).
    items.sort_by_key(|(key, _, _)| *key);

    // 3. Greedy proximity clustering, ANCHORED to the cluster's FIRST line: a finding joins
    //    while its line is within tolerance of the FIRST line of the current cluster (not
    //    the last), which stops single-linkage chaining. `saturating_sub` guards the huge
    //    gap between the file-level sentinel and a real line from overflowing.
    let mut clusters: Vec<Vec<(i64, usize, RawFinding)>> = Vec::new();
    let mut cluster: Vec<(i64, usize, RawFinding)> = Vec::new();
    let mut first_line = 0i64;
    for (key, idx, f) in items {
        // Start a new cluster when the current one is non-empty AND this finding is beyond
        // the proximity tolerance of the cluster's FIRST line.
        if !cluster.is_empty() && key.saturating_sub(first_line) > p.line_tolerance {
            clusters.push(std::mem::take(&mut cluster));
        }
        if cluster.is_empty() {
            first_line = key;
        }
        cluster.push((key, idx, f));
    }
    if !cluster.is_empty() {
        clusters.push(cluster);
    }

    // 3b. Merge DRIFTED assertions: the model cannot count lines reliably on long files
    //     (±20 on a 250-line file), so the SAME logical finding from two samples can land
    //     beyond `line_tolerance` and fragment into two 1-vote clusters. Re-join such
    //     fragments by assertion identity instead of line proximity.
    merge_drifted_assertions(&mut clusters);

    clusters.iter().map(|c| finalize_cluster(c)).collect()
}

/// The precomputed text-identity of one finding, built ONCE per finding before the merge
/// loop (the pairwise scan would otherwise re-tokenize the same strings for every
/// comparison in every fixpoint pass).
struct AssertionKey {
    title_words: Vec<String>,
    body_words: Vec<String>,
    /// Code-identifier tokens from title+body: words containing `_`, `.`, `(` or `::`
    /// after edge-trimming (the shapes Rust identifiers/paths/calls take in finding
    /// text, e.g. "clean_workflow_name", "path.starts_with(root", "std::fs").
    idents: Vec<String>,
}

fn assertion_key(f: &RawFinding) -> AssertionKey {
    let title_words = word_set(&f.title);
    let body_words = word_set(&f.body);
    let mut idents: Vec<String> = title_words
        .iter()
        .chain(body_words.iter())
        .filter(|w| w.contains('_') || w.contains('.') || w.contains('(') || w.contains("::"))
        .cloned()
        .collect();
    idents.sort();
    idents.dedup();
    AssertionKey {
        title_words,
        body_words,
        idents,
    }
}

/// Merge clusters that carry the SAME assertion from DIFFERENT samples, regardless of
/// line distance. PURE. Two clusters merge only when ALL of these hold for some
/// cross-pair of members:
///   1. the clusters' sample sets are DISJOINT — the same sample reporting a similar
///      smell at two far-apart lines means two genuine sites (one sample never reports
///      one bug twice), while different samples at far-apart lines is the line-drift
///      signature;
///   2. title word-set Jaccard ≥ [`TITLE_MERGE_JACCARD`] AND body word-set Jaccard
///      ≥ [`BODY_MERGE_JACCARD`];
///   3. the pair SHARES a code-identifier token (see [`AssertionKey::idents`]) — the
///      anchor that separates "same assertion, drifted line" (drifts quote the same
///      symbols) from "two different findings phrased in the same generic boilerplate"
///      ("missing null check before use" vs "… before access" crosses both Jaccard bars
///      on shared stop-words alone; requiring a shared identifier kills that fabricated
///      merge, and a pair with NO identifiers at all has no assertion identity to anchor
///      on, so it never merges).
///
/// Runs to FIXPOINT (a merged cluster may then match a third fragment). Two documented
/// limits, both deliberate:
///   - a noise pattern the model repeats VERBATIM (same identifiers) at scattered lines
///     will merge and can cross the confirm threshold — consistent with what votes
///     measure (assertion consistency, not truth); the alternative (no merge) silently
///     demotes real bugs to 1-vote suspects whenever the model misjudges line numbers;
///   - when one sample holds TWO findings that both match another sample's single
///     finding, the greedy scan pairs the FIRST eligible one (line-sort order), not a
///     best-match: with the identifier anchor required, that only happens when one
///     sample asserted the same symbols twice, where either pairing is defensible.
fn merge_drifted_assertions(clusters: &mut Vec<Vec<(i64, usize, RawFinding)>>) {
    // Precompute keys once, in lockstep with `clusters`; merges below keep the two
    // structures aligned (same append/remove operations on both).
    let mut keys: Vec<Vec<AssertionKey>> = clusters
        .iter()
        .map(|c| c.iter().map(|(_, _, f)| assertion_key(f)).collect())
        .collect();
    loop {
        let mut merged_any = false;
        let mut i = 0;
        while i < clusters.len() {
            let mut j = i + 1;
            while j < clusters.len() {
                if clusters_share_assertion(&clusters[i], &clusters[j], &keys[i], &keys[j]) {
                    let mut absorbed = clusters.remove(j);
                    clusters[i].append(&mut absorbed);
                    let mut absorbed_keys = keys.remove(j);
                    keys[i].append(&mut absorbed_keys);
                    merged_any = true;
                    // Do not advance j: removal shifted the next candidate into j.
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        if !merged_any {
            return;
        }
    }
}

/// The pairwise merge test of [`merge_drifted_assertions`]: disjoint sample sets AND at
/// least one cross-pair agreeing on title/body Jaccard AND sharing an identifier token.
fn clusters_share_assertion(
    a: &[(i64, usize, RawFinding)],
    b: &[(i64, usize, RawFinding)],
    a_keys: &[AssertionKey],
    b_keys: &[AssertionKey],
) -> bool {
    let disjoint = a
        .iter()
        .all(|(_, ia, _)| b.iter().all(|(_, ib, _)| ia != ib));
    if !disjoint {
        return false;
    }
    a_keys.iter().any(|ka| {
        b_keys.iter().any(|kb| {
            !ka.idents.is_empty()
                && ka.idents.iter().any(|w| kb.idents.binary_search(w).is_ok())
                && jaccard(&ka.title_words, &kb.title_words) >= TITLE_MERGE_JACCARD
                && jaccard(&ka.body_words, &kb.body_words) >= BODY_MERGE_JACCARD
        })
    })
}

/// Deduplicated, lowercased word set of a text, with non-alphanumeric edges stripped per
/// word (so "`clean_workflow_name`" and "clean_workflow_name" match, and pure-punctuation
/// tokens like "&&" drop out). Sorted so [`jaccard`] can intersect via binary search.
fn word_set(text: &str) -> Vec<String> {
    let mut words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    words.sort();
    words.dedup();
    words
}

/// Jaccard similarity of two sorted, deduplicated word sets. Empty-vs-anything = 0.0 (an
/// empty title/body carries no assertion identity — never merge on it).
fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.iter().filter(|w| b.binary_search(w).is_ok()).count();
    let union = a.len() + b.len() - inter;
    inter as f64 / union as f64
}

/// Normalize a title for plurality matching: trim, collapse internal whitespace runs to a
/// single space, and lowercase — so "Null  Deref" and "null deref" match.
fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Reduce one accumulated cluster into a [`VotedFinding`]: count distinct sample indices
/// (the cluster's vote count) and pick the representative by PLURALITY of normalized title.
///
/// Representative selection: group the cluster's members by normalized title, and for each
/// group count the DISTINCT samples that reported that title. The group with the most
/// distinct-sample support wins; ties are broken by higher severity, then by first
/// encountered (clustering order). This means one hallucinated high-severity outlier cannot
/// out-rank an 8-sample majority just because it is more severe — the majority's text is
/// surfaced, carrying the whole cluster's vote count.
fn finalize_cluster(cluster: &[(i64, usize, RawFinding)]) -> VotedFinding {
    // Distinct-sample vote count: two findings from the SAME sample count once.
    let mut seen: Vec<usize> = Vec::new();
    for (_, idx, _) in cluster {
        if !seen.contains(idx) {
            seen.push(*idx);
        }
    }
    let votes = seen.len().min(u8::MAX as usize) as u8;

    // Plurality-by-normalized-title groups, kept in first-encounter order.
    struct Group<'a> {
        norm: String,
        samples: Vec<usize>,
        rep: &'a RawFinding,
    }
    let mut groups: Vec<Group> = Vec::new();
    for (_, idx, f) in cluster {
        let norm = normalize_title(&f.title);
        if let Some(g) = groups.iter_mut().find(|g| g.norm == norm) {
            if !g.samples.contains(idx) {
                g.samples.push(*idx);
            }
            // Within a title group the representative is the highest-severity member,
            // first-encountered on a tie — EXCEPT that a member carrying a line number
            // beats an equal-severity file-level one: after the drifted-assertion merge a
            // file-level fragment can share a cluster with a pinpointed one, and the
            // output must not lose the line a sample actually provided.
            let rank = severity_rank(f.severity);
            let rep_rank = severity_rank(g.rep.severity);
            if rank > rep_rank || (rank == rep_rank && g.rep.line.is_none() && f.line.is_some()) {
                g.rep = f;
            }
        } else {
            groups.push(Group {
                norm,
                samples: vec![*idx],
                rep: f,
            });
        }
    }
    // Winner: most distinct samples; tie → higher severity of the group's rep; tie → first
    // encountered (we only replace on strictly-better, so the earliest group wins full ties).
    let mut best = &groups[0];
    for g in &groups[1..] {
        let more_votes = g.samples.len() > best.samples.len();
        let equal_votes = g.samples.len() == best.samples.len();
        let higher_sev = severity_rank(g.rep.severity) > severity_rank(best.rep.severity);
        if more_votes || (equal_votes && higher_sev) {
            best = g;
        }
    }
    let rep = best.rep.clone();

    // Sorted explicitly: after the drifted-assertion merge a cluster's members are no
    // longer guaranteed to be in ascending line order.
    let mut cluster_lines: Vec<i64> = cluster.iter().map(|(key, _, _)| *key).collect();
    cluster_lines.sort_unstable();
    VotedFinding {
        finding: rep,
        votes,
        cluster_lines,
    }
}

/// Split voted clusters into `(confirmed, suspects)`:
///   - CONFIRMED: `votes >= min_votes_block` (returned verbatim);
///   - SUSPECTS: `min_votes_verify <= votes < min_votes_block` — body PREFIXED with an
///     `[unverified <votes>/<n_samples> votes] ` marker so the verifier role reading them
///     over the `censor_findings` MCP tool sees they are unconfirmed;
///   - everything below `min_votes_verify` is DROPPED.
///
/// The combined `confirmed + suspects` count is capped at [`MAX_GEMMA_FINDINGS`], with
/// confirmed findings taking priority (suspects fill only the remaining budget). Before the
/// cap the clusters are sorted by strongest agreement first — `(votes desc, severity desc)`
/// — so when the cap bites it keeps the most-agreed-upon / most-severe findings rather than
/// whichever happened to sort earliest by line.
pub fn split_by_threshold(
    mut voted: Vec<VotedFinding>,
    p: &VoteParams,
) -> (Vec<RawFinding>, Vec<RawFinding>) {
    // Strongest first: more votes, then higher severity. Stable, so equal keys keep their
    // (line-sorted) order. This makes the MAX cap keep the best clusters.
    voted.sort_by(|a, b| {
        b.votes
            .cmp(&a.votes)
            .then_with(|| severity_rank(b.finding.severity).cmp(&severity_rank(a.finding.severity)))
    });

    let mut confirmed: Vec<RawFinding> = Vec::new();
    let mut suspects: Vec<RawFinding> = Vec::new();
    for v in voted {
        if v.votes >= p.min_votes_block {
            confirmed.push(v.finding);
        } else if v.votes >= p.min_votes_verify {
            // Flag the suspect so the verifier role sees it is unconfirmed. The numerator
            // is the ACTUAL vote count (not a hardcoded 1) so a 2/5 suspect reads
            // correctly; the denominator is the configured sample count. Re-cap at
            // BODY_CAP AFTER prepending so a body already at the cap can't overflow it.
            let mut f = v.finding;
            let marker = format!("[unverified {}/{} votes] ", v.votes, p.n_samples);
            f.body = super::runners::cap(&format!("{marker}{}", f.body), super::gemma::BODY_CAP);
            suspects.push(f);
        }
        // votes < min_votes_verify → dropped (not enough agreement to surface at all).
    }
    // Cap the COMBINED total at MAX_GEMMA_FINDINGS, confirmed taking priority so a flood
    // of low-vote suspects can never crowd out a confirmed blocker.
    if confirmed.len() >= MAX_GEMMA_FINDINGS {
        confirmed.truncate(MAX_GEMMA_FINDINGS);
        suspects.clear();
    } else {
        let remaining = MAX_GEMMA_FINDINGS - confirmed.len();
        suspects.truncate(remaining);
    }
    (confirmed, suspects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::Category;

    fn finding(line: Option<u32>, severity: Severity, title: &str) -> RawFinding {
        RawFinding {
            file: "src/lib.rs".to_string(),
            line,
            severity,
            category: Category::Correctness,
            source: "gemma".to_string(),
            title: title.to_string(),
            body: format!("body of {title}"),
        }
    }

    fn params(n: u8, block: u8, verify: u8, tol: i64) -> VoteParams {
        VoteParams {
            n_samples: n,
            min_votes_block: block,
            min_votes_verify: verify,
            line_tolerance: tol,
        }
    }

    #[test]
    fn clusters_nearby_lines_across_samples_within_tolerance() {
        // Three samples each flag ~the same smell at lines 10, 11, 12 (within ±2) plus
        // one sample adds an unrelated smell far away at line 40.
        let samples = vec![
            vec![finding(Some(10), Severity::High, "a")],
            vec![finding(Some(11), Severity::Medium, "b")],
            vec![
                finding(Some(12), Severity::Low, "c"),
                finding(Some(40), Severity::Medium, "far"),
            ],
        ];
        let voted = cluster_and_vote(samples, &params(3, 2, 1, 2));
        assert_eq!(voted.len(), 2, "one tight cluster + one lone far finding");
        // First cluster (lines 10..12): 3 distinct samples agree.
        assert_eq!(voted[0].votes, 3);
        assert_eq!(voted[0].cluster_lines, vec![10, 11, 12]);
        // Representative = highest severity in the cluster (High @ line 10).
        assert_eq!(voted[0].finding.severity, Severity::High);
        assert_eq!(voted[0].finding.line, Some(10));
        // Second cluster: the lone far finding, 1 vote.
        assert_eq!(voted[1].votes, 1);
        assert_eq!(voted[1].cluster_lines, vec![40]);
    }

    #[test]
    fn gap_larger_than_tolerance_starts_new_cluster() {
        // Lines 10 and 13 are 3 apart; with tolerance 2 they must NOT merge.
        let samples = vec![
            vec![finding(Some(10), Severity::Medium, "a")],
            vec![finding(Some(13), Severity::Medium, "b")],
        ];
        let voted = cluster_and_vote(samples, &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 2);
        assert_eq!(voted[0].votes, 1);
        assert_eq!(voted[1].votes, 1);
    }

    #[test]
    fn two_findings_from_same_sample_in_one_cluster_count_as_one_vote() {
        // A single sample emits TWO findings at adjacent lines (10, 11). They cluster
        // together but come from the SAME sample → votes must be 1, not 2.
        let samples = vec![vec![
            finding(Some(10), Severity::Medium, "a"),
            finding(Some(11), Severity::High, "b"),
        ]];
        let voted = cluster_and_vote(samples, &params(1, 1, 1, 2));
        assert_eq!(voted.len(), 1);
        assert_eq!(voted[0].votes, 1, "same-sample duplicates are ONE vote");
        assert_eq!(voted[0].cluster_lines, vec![10, 11]);
        // Representative = the High one even though it was second.
        assert_eq!(voted[0].finding.severity, Severity::High);
    }

    #[test]
    fn split_confirms_suspects_and_drops_below_verify() {
        // Craft three clusters with 3, 1 and 0 (unreachable) votes. Block>=2, verify>=1.
        let voted = vec![
            VotedFinding {
                finding: finding(Some(5), Severity::High, "confirmed"),
                votes: 3,
                cluster_lines: vec![5, 5, 6],
            },
            VotedFinding {
                finding: finding(Some(20), Severity::Medium, "suspect"),
                votes: 1,
                cluster_lines: vec![20],
            },
        ];
        let (confirmed, suspects) = split_by_threshold(voted, &params(3, 2, 1, 2));
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].title, "confirmed");
        // The confirmed finding body is UNTOUCHED (no marker).
        assert_eq!(confirmed[0].body, "body of confirmed");
        assert_eq!(suspects.len(), 1);
        assert_eq!(suspects[0].title, "suspect");
        // The suspect body is prefixed with the unverified marker (votes/n).
        assert!(
            suspects[0].body.starts_with("[unverified 1/3 votes] "),
            "got: {}",
            suspects[0].body
        );
        assert!(suspects[0].body.ends_with("body of suspect"));
    }

    #[test]
    fn split_drops_clusters_below_verify_threshold() {
        // verify=2: a single-vote cluster is neither confirmed nor a suspect → dropped.
        let voted = vec![VotedFinding {
            finding: finding(Some(5), Severity::Medium, "lonely"),
            votes: 1,
            cluster_lines: vec![5],
        }];
        let (confirmed, suspects) = split_by_threshold(voted, &params(3, 3, 2, 2));
        assert!(confirmed.is_empty());
        assert!(suspects.is_empty());
    }

    #[test]
    fn empty_input_yields_no_clusters_and_empty_split() {
        let voted = cluster_and_vote(vec![], &params(3, 2, 1, 2));
        assert!(voted.is_empty());
        let voted2 = cluster_and_vote(vec![vec![], vec![], vec![]], &params(3, 2, 1, 2));
        assert!(voted2.is_empty(), "all-empty samples produce no clusters");
        let (c, s) = split_by_threshold(vec![], &params(3, 2, 1, 2));
        assert!(c.is_empty() && s.is_empty());
    }

    #[test]
    fn single_sample_n1_behaves_like_passthrough() {
        // n=1, block=1, verify=1: every finding is its own 1-vote cluster and every
        // cluster clears the block threshold → all confirmed, none suspect, no markers.
        let samples = vec![vec![
            finding(Some(3), Severity::High, "x"),
            finding(Some(30), Severity::Low, "y"),
        ]];
        let voted = cluster_and_vote(samples, &params(1, 1, 1, 2));
        assert_eq!(voted.len(), 2);
        assert!(voted.iter().all(|v| v.votes == 1));
        let (confirmed, suspects) = split_by_threshold(voted, &params(1, 1, 1, 2));
        assert_eq!(confirmed.len(), 2);
        assert!(suspects.is_empty());
        // Bodies untouched (passthrough).
        assert_eq!(confirmed[0].body, "body of x");
    }

    #[test]
    fn file_level_findings_cluster_together_not_with_numbered_lines() {
        // Two file-level findings (line None) from two samples must vote together and NOT
        // merge with a numbered-line finding near line 1.
        let samples = vec![
            vec![finding(None, Severity::Medium, "file-a")],
            vec![finding(None, Severity::High, "file-b")],
            vec![finding(Some(1), Severity::Low, "line1")],
        ];
        let voted = cluster_and_vote(samples, &params(3, 2, 1, 2));
        assert_eq!(voted.len(), 2, "file-level cluster + numbered-line cluster");
        // The file-level cluster sorts first (sentinel is far below any real line).
        assert_eq!(voted[0].votes, 2);
        assert_eq!(voted[0].finding.line, None);
        assert_eq!(voted[0].finding.severity, Severity::High);
        assert_eq!(voted[1].votes, 1);
        assert_eq!(voted[1].finding.line, Some(1));
    }

    #[test]
    fn total_confirmed_plus_suspects_capped_at_max() {
        // Build MAX+5 single-vote clusters; with block=1 all would confirm, but the cap
        // trims the total to MAX_GEMMA_FINDINGS.
        let voted: Vec<VotedFinding> = (0..(MAX_GEMMA_FINDINGS as u32 + 5))
            .map(|i| VotedFinding {
                finding: finding(Some(i + 1), Severity::Medium, "f"),
                votes: 1,
                cluster_lines: vec![(i + 1) as i64],
            })
            .collect();
        let (confirmed, suspects) = split_by_threshold(voted, &params(1, 1, 1, 2));
        assert_eq!(confirmed.len(), MAX_GEMMA_FINDINGS);
        assert!(suspects.is_empty());
    }

    // ---- review fix: plurality representative (issue 2) ----

    #[test]
    fn representative_is_plurality_title_not_a_lone_high_severity_outlier() {
        // 8 samples agree on a Medium "null deref" at ~line 5; ONE sample (the 9th) reports
        // a High "SQL injection" in the same cluster. The representative must be the
        // 8-sample-majority null-deref, NOT the lone high-severity outlier.
        let mut samples: Vec<Vec<RawFinding>> = (0..8)
            .map(|i| vec![finding(Some(5 + (i % 2)), Severity::Medium, "null deref")])
            .collect();
        samples.push(vec![finding(Some(6), Severity::High, "SQL injection")]);
        let voted = cluster_and_vote(samples, &params(9, 5, 1, 2));
        assert_eq!(voted.len(), 1, "all land in one cluster");
        assert_eq!(voted[0].votes, 9, "9 distinct samples voted");
        assert_eq!(
            normalize_title(&voted[0].finding.title),
            "null deref",
            "plurality title wins over the lone high-severity outlier"
        );
        assert_eq!(voted[0].finding.severity, Severity::Medium);
    }

    #[test]
    fn representative_tie_breaks_by_severity_then_first_encountered() {
        // Two distinct titles, one sample each (tie on votes) → higher severity wins.
        let samples = vec![
            vec![finding(Some(3), Severity::Low, "typo")],
            vec![finding(Some(3), Severity::High, "use after free")],
        ];
        let voted = cluster_and_vote(samples, &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 1);
        assert_eq!(normalize_title(&voted[0].finding.title), "use after free");
    }

    // ---- review fix: first-line anchoring, no single-linkage chaining (issue 3) ----

    #[test]
    fn tolerance_anchors_to_first_line_no_chaining() {
        // Lines 10, 12, 14 with tol=2: single-linkage would chain all three (each within 2
        // of the previous). Anchored to the cluster's FIRST line (10), only 10 and 12 join;
        // 14 (4 beyond 10) starts a new cluster.
        let samples = vec![
            vec![finding(Some(10), Severity::Medium, "a")],
            vec![finding(Some(12), Severity::Medium, "b")],
            vec![finding(Some(14), Severity::Medium, "c")],
        ];
        let voted = cluster_and_vote(samples, &params(3, 2, 1, 2));
        assert_eq!(
            voted.len(),
            2,
            "{{10,12}} and {{14}}, not one chained cluster"
        );
        assert_eq!(voted[0].cluster_lines, vec![10, 12]);
        assert_eq!(voted[1].cluster_lines, vec![14]);
    }

    // ---- review fix: cap keeps strongest clusters (issue 5) ----

    #[test]
    fn cap_keeps_highest_vote_confirmed_findings() {
        // MAX 2-vote "minor" clusters + 2 nine-vote "critical" clusters = MAX+2 confirmed.
        // The cap must KEEP both criticals (sorted votes-desc first), dropping two minors.
        let mut voted: Vec<VotedFinding> = (0..MAX_GEMMA_FINDINGS as u32)
            .map(|i| VotedFinding {
                finding: finding(Some(i + 1), Severity::Medium, "minor"),
                votes: 2,
                cluster_lines: vec![(i + 1) as i64],
            })
            .collect();
        voted.push(VotedFinding {
            finding: finding(Some(900), Severity::Medium, "critical"),
            votes: 9,
            cluster_lines: vec![900],
        });
        voted.push(VotedFinding {
            finding: finding(Some(901), Severity::Medium, "critical"),
            votes: 9,
            cluster_lines: vec![901],
        });
        let (confirmed, _) = split_by_threshold(voted, &params(9, 2, 1, 2));
        assert_eq!(confirmed.len(), MAX_GEMMA_FINDINGS);
        let criticals = confirmed.iter().filter(|f| f.title == "critical").count();
        assert_eq!(criticals, 2, "both high-vote criticals survive the cap");
    }

    // ---- drifted-assertion merge (live-e2e 2026-07-03: planted `&&` bug fragmented) ----

    fn finding_with_body(
        line: Option<u32>,
        severity: Severity,
        title: &str,
        body: &str,
    ) -> RawFinding {
        let mut f = finding(line, severity, title);
        f.body = body.to_string();
        f
    }

    /// The EXACT live failure: two samples nailed the same planted `&&`-for-`||` bug with
    /// near-identical text but line estimates 27 apart (107 vs 134; real line 125). Line
    /// clustering fragments them into two 1-vote suspects; the merge must re-join them
    /// into ONE 2-vote cluster.
    #[test]
    fn drifted_same_assertion_from_disjoint_samples_merges() {
        let a = finding_with_body(
            Some(107),
            Severity::Medium,
            "Error handling in clean_workflow_name",
            "The condition `trimmed.is_empty() && trimmed.chars().count() > WORKFLOW_NAME_MAX_CHARS` is logically incorrect and should be reviewed.",
        );
        let b = finding_with_body(
            Some(134),
            Severity::Medium,
            "Potential error handling issue in `clean_workflow_name`",
            "The condition `trimmed.is_empty() && trimmed.chars().count() > WORKFLOW_NAME_MAX_CHARS` is logically incorrect. It should check if the length exceeds the maximum allowed characters.",
        );
        let voted = cluster_and_vote(vec![vec![a], vec![b], vec![]], &params(3, 2, 1, 2));
        assert_eq!(voted.len(), 1, "the two drifted fragments merge");
        assert_eq!(voted[0].votes, 2, "…and carry both samples' votes");
        assert_eq!(voted[0].cluster_lines, vec![107, 134]);
    }

    #[test]
    fn same_sample_far_findings_never_merge_even_if_identical() {
        // ONE sample reports the identical smell at two far-apart sites: two genuine
        // occurrences, NOT line drift. Sample sets are not disjoint → no merge.
        let samples = vec![vec![
            finding_with_body(Some(10), Severity::Medium, "unnecessary clone", "clone of x"),
            finding_with_body(Some(200), Severity::Medium, "unnecessary clone", "clone of x"),
        ]];
        let voted = cluster_and_vote(samples, &params(1, 1, 1, 2));
        assert_eq!(voted.len(), 2, "two sites from one sample stay separate");
    }

    #[test]
    fn dissimilar_assertions_do_not_merge_across_far_lines() {
        // Disjoint samples, far lines, but genuinely different findings (the live "magic
        // number in …" nits): titles share words yet stay under the similarity bars.
        let samples = vec![
            vec![finding_with_body(
                Some(78),
                Severity::Low,
                "Magic number in overhead calculation",
                "The minimum overhead of 5000 tokens is a magic number and should be a named constant.",
            )],
            vec![finding_with_body(
                Some(120),
                Severity::Low,
                "Magic number in token estimation",
                "The division by 4 to estimate tokens is a magic number and should be extracted.",
            )],
        ];
        let voted = cluster_and_vote(samples, &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 2, "different nits stay separate clusters");
    }

    #[test]
    fn similar_title_but_different_body_does_not_merge() {
        // Same short title from two samples, but the rationales describe DIFFERENT things
        // → the body bar must block the merge.
        let samples = vec![
            vec![finding_with_body(
                Some(10),
                Severity::Low,
                "typo",
                "misspelled recieve in the parser error message",
            )],
            vec![finding_with_body(
                Some(300),
                Severity::Low,
                "typo",
                "wrong article used in the scheduler doc comment header",
            )],
        ];
        let voted = cluster_and_vote(samples, &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 2, "same title, different assertion — no merge");
    }

    /// Reviewer BLOCKER repro (2026-07-03): two DISTINCT bugs phrased in near-identical
    /// generic boilerplate cross BOTH Jaccard bars on shared stop-words alone (title
    /// ≈0.67, body ≈0.78). The identifier anchor must block the fabricated 2-vote merge —
    /// which would both false-confirm and bury the second site's line entirely.
    #[test]
    fn generic_boilerplate_without_shared_identifier_never_merges() {
        let a = finding_with_body(
            Some(12),
            Severity::Medium,
            "Missing null check before use",
            "This could cause a crash if the pointer is not checked before it is used in the function.",
        );
        let b = finding_with_body(
            Some(900),
            Severity::Medium,
            "Missing null check before access",
            "This could cause a crash if the index is not checked before it is accessed in the function.",
        );
        let voted = cluster_and_vote(vec![vec![a], vec![b]], &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 2, "no shared identifier — no merge");
        assert!(voted.iter().all(|v| v.votes == 1));
    }

    /// Reviewer WARNING repro (2026-07-03): identical assertion (sharing an identifier)
    /// reported file-level by one sample and pinpointed at line 88 by another. They merge
    /// — and the representative must carry the pinpointed line, not the file-level None.
    #[test]
    fn merged_file_level_and_numbered_representative_keeps_the_line() {
        let a = finding_with_body(
            None,
            Severity::Medium,
            "Ignored error from flush_buffers()",
            "The result of flush_buffers() is discarded so a failed flush is silent.",
        );
        let b = finding_with_body(
            Some(88),
            Severity::Medium,
            "Ignored error from flush_buffers()",
            "The result of flush_buffers() is discarded so a failed flush is silent.",
        );
        let voted = cluster_and_vote(vec![vec![a], vec![b]], &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 1, "identical assertion merges across file-level/numbered");
        assert_eq!(voted[0].votes, 2);
        assert_eq!(
            voted[0].finding.line,
            Some(88),
            "representative keeps the pinpointed line"
        );
    }

    #[test]
    fn merge_runs_to_fixpoint_across_three_fragments() {
        // Three samples, same assertion, wildly scattered lines: all three fragments must
        // fold into ONE 3-vote cluster (A+B first, then the merged cluster absorbs C).
        let mk = |line: u32| {
            finding_with_body(
                Some(line),
                Severity::Medium,
                "inverted containment check",
                "the check path.starts_with(root) is inverted and skips in-project files",
            )
        };
        let samples = vec![vec![mk(10)], vec![mk(50)], vec![mk(90)]];
        let voted = cluster_and_vote(samples, &params(3, 2, 1, 2));
        assert_eq!(voted.len(), 1, "all three drifted fragments merge");
        assert_eq!(voted[0].votes, 3);
        assert_eq!(voted[0].cluster_lines, vec![10, 50, 90]);
    }

    #[test]
    fn overlapping_sample_sets_block_the_merge() {
        // Cluster {s0,s1} and cluster {s1}: sample 1 appears in both → the second is a
        // separate site reported by an already-counted sample, not drift. No merge.
        let mk = |line: u32| {
            finding_with_body(
                Some(line),
                Severity::Medium,
                "ignored write error",
                "the result of write_all is discarded so a short write is silent",
            )
        };
        let samples = vec![vec![mk(10)], vec![mk(11), mk(400)]];
        let voted = cluster_and_vote(samples, &params(2, 2, 1, 2));
        assert_eq!(voted.len(), 2, "shared sample forbids the merge");
        // The near pair clusters by line proximity (2 votes); the far one stays 1 vote.
        assert_eq!(voted[0].votes, 2);
        assert_eq!(voted[1].votes, 1);
    }

    // ---- review fix: suspect body re-capped after marker (issue 6) ----

    #[test]
    fn suspect_body_is_recapped_at_body_cap_after_marker() {
        use crate::backend::censor::gemma::BODY_CAP;
        // A body already at BODY_CAP; prepending the marker would overflow it without a re-cap.
        let mut f = finding(Some(5), Severity::Medium, "smell");
        f.body = "x".repeat(BODY_CAP);
        let voted = vec![VotedFinding {
            finding: f,
            votes: 1,
            cluster_lines: vec![5],
        }];
        let (_, suspects) = split_by_threshold(voted, &params(3, 2, 1, 2));
        assert_eq!(suspects.len(), 1);
        assert!(suspects[0].body.starts_with("[unverified 1/3 votes] "));
        // `cap` bounds to BODY_CAP chars + a single ellipsis on overflow (same contract as
        // parse_gemma). Without the re-cap the body would be marker(23) + BODY_CAP chars.
        assert!(
            suspects[0].body.chars().count() <= BODY_CAP + 1,
            "prefixed body re-capped to BODY_CAP (+ellipsis), got {}",
            suspects[0].body.chars().count()
        );
    }
}
