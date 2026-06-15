//! Tree-sitter per-item extraction + finding GROUNDING for the Censor.
//!
//! This is the deterministic substrate of the Tier-A "DETERMINISTIC SANDWICH"
//! (master plan §"🛡️ DETERMINISTIC SANDWICH", Tier-A step (1); design doc §A).
//! Two jobs, both cheap and language-agnostic at the API boundary (keyed on
//! [`FileLang`]):
//!
//!   1. **Per-ITEM extraction** ([`extract_items`]) — split a source file into its
//!      top-level review units (Rust: `fn`/`impl`/`struct`/`enum`/`trait`/`mod`/
//!      `const`/`static`/`type`/macro-def) with EXACT 1-based inclusive line
//!      boundaries from the AST. The item is the universal review unit: it works
//!      for NEW files (no diff) AND edited files, keeps the scope small, and keeps
//!      the local reviewer's prefill short (design §A).
//!
//!   2. **Finding GROUNDING** ([`parse_file`] + [`grounds`] / [`ground_findings`])
//!      — the load-bearing anti-hallucination step. Drop ONLY an UNAMBIGUOUS AST
//!      contradiction (a finding that cites a line past EOF, or — when a grammar
//!      actually produced identifiers — a symbol that does not exist in the file).
//!      CONSERVATIVE by design: a false drop suppresses a REAL finding, which is
//!      worse than letting a hallucination slip to the next tier, so anything that
//!      is not a hard contradiction is KEPT.
//!
//! PRODUCT GENERALITY: the API is keyed on [`FileLang`]. Today only the Rust
//! grammar is wired (DISCIPLINE: one grammar at a time, per the plan). Other
//! languages degrade gracefully — [`extract_items`] returns an empty `Vec` and
//! [`parse_file`] yields an empty identifier set, so symbol grounding is disabled
//! for them (unknown != contradicted) while the universal line-range grounding
//! still applies (it only needs the line count). tree-sitter is not OS-specific,
//! so there is NO `cfg` gating here.
//!
//! DEAD-CODE NOTE: this module ships "dark". The public API is consumed by the
//! Censor merge in a LATER workstream (C4 wires grounding into the live reviewer
//! path; B uses item extraction for per-item review). The pure logic is fully
//! exercised by this module's tests, but the production callers don't exist yet,
//! so the API reads as unused until then. The allow is file-scoped (not crate-
//! wide) and removed when C4/B consume these APIs.
// TODO(C4/B): wire into the Censor merge — grounding into the post-LLM step
// (C4) and item extraction into the per-item review loop (B).
#![allow(dead_code)]

use super::detect::FileLang;
use std::collections::HashSet;

/// A top-level review unit extracted from a source file. Lines are 1-based and
/// INCLUSIVE (`start_line..=end_line`), matching the rest of the Censor (findings
/// carry 1-based lines; `RawFinding::line` / `gemma.rs`). `kind` is the tree-sitter
/// node kind (e.g. `"function_item"`, `"impl_item"`); `name` is the item's
/// identifier where one exists (an `impl` block / a trait impl may have none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    /// The grammar node kind (e.g. `"function_item"`, `"struct_item"`).
    pub kind: String,
    /// The item's name. An `impl` block has no declared name, so it is named by its
    /// implemented type (e.g. `Point` for `impl Display for Point`); see
    /// [`rust_item_name`]. `None` only when even that is absent.
    pub name: Option<String>,
    /// 1-based first line of the item (inclusive).
    pub start_line: u32,
    /// 1-based last line of the item (inclusive).
    pub end_line: u32,
}

/// The deterministic facts extracted from a source file: its line count, its
/// top-level review items, and the set of identifiers the grammar saw. Decoupled
/// from the finding struct ON PURPOSE so C4 can ground findings without this module
/// depending on `gemma.rs` (the shared-file coordination boundary).
///
/// `identifiers` is EMPTY when there is no grammar for the language. An empty set
/// means "we cannot vouch for any symbol", which DISABLES symbol grounding for that
/// file (unknown != contradicted) — see [`grounds`].
#[derive(Debug, Clone)]
pub struct ParsedFile {
    /// Number of lines in the source (0 for empty input). 1-based finding lines are
    /// valid in `1..=total_lines`.
    pub total_lines: u32,
    /// Top-level review units, in source order. Empty for languages without a
    /// grammar (or genuinely item-less files).
    pub items: Vec<ReviewItem>,
    /// Every identifier the grammar observed in the file. Used for symbol
    /// grounding. EMPTY ⇒ symbol grounding disabled for this file.
    pub identifiers: HashSet<String>,
}

/// The verdict of grounding one finding against a [`ParsedFile`]. `Kept` means the
/// AST does not contradict the finding (it survives to the next tier); the two
/// `Dropped*` variants are the only UNAMBIGUOUS structural contradictions we act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grounding {
    /// No AST contradiction — keep the finding.
    Kept,
    /// The finding cites a line `< 1` or `> total_lines` — it points past the end
    /// of the file (or before its start). A structural hallucination; drop it. This
    /// check is LANGUAGE-AGNOSTIC (only needs the line count).
    DroppedLineOutOfFile,
    /// The finding cites a symbol that the grammar parsed the file and did NOT find.
    /// Only possible when identifiers were actually extracted (a grammar exists);
    /// never fires on languages without a grammar.
    DroppedUnknownSymbol,
}

/// Extract the TOP-LEVEL review items from `source`, keyed on `lang`.
///
/// Rust: parsed with `tree-sitter-rust`; returns the top-level
/// `function_item`, `impl_item`, `struct_item`, `enum_item`, `trait_item`,
/// `mod_item`, `const_item`, `static_item`, `type_item`, and `macro_definition`
/// nodes (the universal review units — design §A). Lines are 1-based inclusive.
/// Names come from the item's declared identifier; an `impl` block (which has no
/// declared name) is named by its implemented type (see [`rust_item_name`]).
///
/// Other languages (`Ts`, `Py`, `Other`): return an empty `Vec` for now. The
/// routing is scaffolded so additional grammars slot in later — see the per-`lang`
/// match. This NEVER panics (a grammar/parse failure yields an empty `Vec`).
///
// TODO(C1 follow-up): add the JS/TS, Python, then C/C++/Kotlin/Go/HTML grammars.
///
/// Delegates to [`parse_file`] (the SINGLE parse path) and returns only its `items`,
/// so a file is parsed by exactly one routine. A caller that needs items AND grounding
/// (C4/B) should call [`parse_file`] once and use both fields rather than calling this
/// and [`parse_file`] separately.
pub fn extract_items(source: &str, lang: FileLang) -> Vec<ReviewItem> {
    parse_file(source, lang).items
}

/// Parse a file into the deterministic facts grounding needs: line count, items,
/// and the identifier set. Keyed on `lang`; graceful for languages without a
/// grammar (empty `items` + empty `identifiers`, but the real `total_lines`, so
/// line-range grounding still works). NEVER panics.
pub fn parse_file(source: &str, lang: FileLang) -> ParsedFile {
    let total_lines = count_lines(source);
    match lang {
        FileLang::Rust => {
            let (items, identifiers) = parse_rust(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        FileLang::Ts | FileLang::Py | FileLang::Other => ParsedFile {
            total_lines,
            items: Vec::new(),
            identifiers: HashSet::new(),
        },
    }
}

/// Ground ONE finding (by its `line` and/or `symbol`) against a [`ParsedFile`].
///
/// GROUNDING RULE — drop ONLY an UNAMBIGUOUS AST contradiction (false drops
/// suppress REAL findings, so be conservative):
///   - DROP [`Grounding::DroppedLineOutOfFile`] if `line == Some(n)` and
///     `n < 1 || n > total_lines` (cites a line past EOF — works for ALL
///     languages, only needs the line count).
///   - DROP [`Grounding::DroppedUnknownSymbol`] if `symbol == Some(s)`, the grammar
///     produced identifiers (`!parsed.identifiers.is_empty()`), AND `s` is not in
///     that set (cites a symbol that does not exist in the file). Only when
///     identifiers were actually extracted — never drop on a language with no
///     grammar (unknown != contradicted).
///   - KEEP otherwise — including a valid in-file line that falls BETWEEN items
///     (imports, blank lines, attributes): being inside the file but outside an
///     item is NOT a contradiction.
///
/// The line check runs FIRST: a finding can be doomed on BOTH counts, and an
/// out-of-file line is the more fundamental structural lie.
pub fn grounds(parsed: &ParsedFile, line: Option<u32>, symbol: Option<&str>) -> Grounding {
    // (1) Line-range grounding — language-agnostic, needs only the line count.
    if let Some(n) = line {
        if n < 1 || n > parsed.total_lines {
            return Grounding::DroppedLineOutOfFile;
        }
    }
    // (2) Symbol grounding — ONLY when a grammar actually produced identifiers.
    // No identifiers ⇒ no grammar ⇒ "unknown", which is NOT a contradiction.
    //
    // CONSERVATIVE matching (avoid OVER-DROP — the cardinal sin here): a reviewer
    // may cite a symbol as a QUALIFIED PATH or a generic/macro/lifetime shape
    // (`Point::origin`, `self.translate`, `Vec<T>`, `println!`, `'a`) while the
    // grammar collects each ident as a SEPARATE bare token. `symbol_grounded`
    // tokenizes the cited symbol into identifier-like runs and drops ONLY when NOT
    // EVEN ONE token exists in the file — the unambiguous contradiction (a wholly
    // invented name). If any token is a real identifier, KEEP it; a wrong path shape
    // is a SEMANTIC issue for a later tier, not a structural hallucination.
    if let Some(s) = symbol {
        if !parsed.identifiers.is_empty() && !symbol_grounded(&parsed.identifiers, s) {
            return Grounding::DroppedUnknownSymbol;
        }
    }
    Grounding::Kept
}

/// Is the cited `symbol` grounded in the file's `identifiers`? True (keep) if ANY
/// IDENTIFIER-LIKE token in the symbol is a known identifier.
///
/// The symbol is tokenized into identifier-like runs matching `[A-Za-z_][A-Za-z0-9_]*`
/// — i.e. we scan over every non-ident character. This naturally handles every path /
/// type shape a finding actually cites: `::` and `.` separators, generics and
/// turbofish (`Vec<T>`, `Foo::<Bar>`), macro bangs (`println!` → token `println`,
/// because the grammar stores the macro name WITHOUT the `!`), lifetimes (`'a` → token
/// `a`, matched against the lifetime names we collect into the set), commas, brackets,
/// parens, and whitespace. A leading raw-identifier prefix (`r#`) is stripped from each
/// token so `type` matches a declared `r#type` (symmetric with insertion-time
/// canonicalization).
///
/// Decision rule (CONSERVATIVE — a false drop suppresses a REAL finding, the cardinal
/// sin): KEEP if there are NO identifier-like tokens (nothing concrete to contradict);
/// otherwise DROP only when NONE of the tokens match — keep if ANY matches. So
/// `Point::nonexistent_method` is KEPT (a wrong method on a real type is SEMANTIC, for
/// a later tier), while a wholly-invented `Bogus::nope` DROPS.
fn symbol_grounded(identifiers: &HashSet<String>, symbol: &str) -> bool {
    let mut saw_token = false;
    for token in ident_tokens(symbol) {
        saw_token = true;
        if identifiers.contains(strip_raw_prefix(token)) {
            return true;
        }
    }
    // No identifier-like token (e.g. "", "::", "<>") ⇒ nothing concrete to contradict
    // ⇒ treat as grounded (do not drop). Otherwise we reached here with tokens that all
    // failed to match ⇒ not grounded.
    !saw_token
}

/// Scan `s` into its identifier-like tokens, each matching `[A-Za-z_][A-Za-z0-9_]*`.
/// Splits on every character that is not an ASCII ident char, so all path/type/macro/
/// lifetime punctuation (`:`, `.`, `<`, `>`, `!`, `'`, `,`, `()[]`, space, …) acts as a
/// separator. A run that starts with a digit (e.g. the `0` in `arr[0]`) is NOT an
/// identifier per the pattern and is filtered out, so a purely-numeric subscript can
/// never be treated as a concrete token to contradict. Returns borrowed slices into
/// `s` (no allocation).
fn ident_tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| {
            // Non-empty AND first byte is a non-digit ident-start (`[A-Za-z_]`); ASCII,
            // so byte indexing is safe.
            t.as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        })
}

/// Partition findings into `(kept, dropped)` by grounding each `(line, symbol)`
/// against `parsed`. The C4 caller hands a slice of `(line, symbol)` pairs (decoupled
/// from the finding struct so this module never depends on `gemma.rs`) and gets back
/// the indices/pairs that survived and the ones an AST contradiction dropped (each
/// dropped pair carries its [`Grounding`] reason for provenance/telemetry).
///
/// Generic over the caller's element type via a projection closure, so C4 can pass
/// `&[Finding]` (or any slice) without this module knowing the finding type: the
/// closure yields `(line, symbol)` for each element. Order is preserved within each
/// partition.
pub fn ground_findings<'a, T, F>(
    parsed: &ParsedFile,
    findings: &'a [T],
    project: F,
) -> (Vec<&'a T>, Vec<(&'a T, Grounding)>)
where
    F: Fn(&'a T) -> (Option<u32>, Option<&'a str>),
{
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for item in findings {
        let (line, symbol) = project(item);
        match grounds(parsed, line, symbol) {
            Grounding::Kept => kept.push(item),
            reason => dropped.push((item, reason)),
        }
    }
    (kept, dropped)
}

/// Count the lines in `source` for line-range grounding. Empty input ⇒ 0 (no valid
/// 1-based line). A trailing newline does NOT add a phantom final line: a finding's
/// line index can only reference content lines. `"a\nb"` and `"a\nb\n"` both have 2
/// lines. Uses `str::lines()`, which already treats a trailing `\n` this way and
/// handles both `\n` and `\r\n`.
fn count_lines(source: &str) -> u32 {
    if source.is_empty() {
        return 0;
    }
    // `lines()` yields no entry for a trailing newline, which is exactly what we
    // want: the count is the number of addressable content lines. Saturate rather
    // than truncate on the (unreachable for real source) >4B-line case: a wrapped
    // tiny count would OVER-DROP valid findings, the one failure mode we forbid.
    u32::try_from(source.lines().count()).unwrap_or(u32::MAX)
}

/// The Rust top-level item node kinds we treat as review units (design §A). These
/// are the kinds tree-sitter-rust emits as direct children of the `source_file`
/// root. Kept as a slice so the match in [`rust_item_name`] and the top-level
/// filter stay in sync.
const RUST_ITEM_KINDS: [&str; 10] = [
    "function_item",
    "impl_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "mod_item",
    "const_item",
    "static_item",
    "type_item",
    "macro_definition",
];

/// Build a [`ReviewItem`] from a direct child of the `source_file` root IF it is one
/// of [`RUST_ITEM_KINDS`]; otherwise `None`. Centralizes the kind filter, name
/// resolution, and the 0-based-row → 1-based-inclusive-line conversion for the single
/// parse walk ([`parse_rust`]). The row math SATURATES (`+1` on a `u32::MAX` row would
/// wrap): pathological for real source, but a wrapped tiny line number would corrupt
/// grounding, so we clamp.
fn rust_top_level_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    let kind = node.kind();
    if !RUST_ITEM_KINDS.contains(&kind) {
        return None;
    }
    // tree-sitter rows are 0-based usize; the Censor uses 1-based inclusive lines.
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    Some(ReviewItem {
        kind: kind.to_string(),
        name: rust_item_name(node, bytes),
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// Parse Rust `source` into `(items, identifiers)` for grounding — the SINGLE Rust
/// parse routine (both [`parse_file`] and, transitively, [`extract_items`] go through
/// here, so a file is parsed exactly once per call). The identifier set is EVERY
/// `identifier`/`type_identifier`/`field_identifier` leaf in the tree (whole-file, not
/// just top-level) PLUS every `lifetime` name (the alphanumeric part after `'`), so
/// symbol grounding can vouch for any name the source actually contains — a finding may
/// legitimately cite a local variable, a called function, a field, or a lifetime, not
/// only a top-level item. A parse failure yields empty `items` + empty `identifiers`
/// (symbol grounding then disabled — fail-open, never a false drop).
fn parse_rust(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_rust_tree(source) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    // Top-level items (same builder as `extract_rust_items` — single source of truth).
    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        if let Some(item) = rust_top_level_item(&child, bytes) {
            items.push(item);
        }
    }

    // All identifiers (whole-tree walk). Pre-order DFS via an explicit stack so we
    // never recurse unbounded on a deep tree. Identifiers are stored in their
    // RAW-PREFIX-STRIPPED canonical form (`r#type` → `type`) so a finding can cite
    // either form and match symmetrically; `symbol_grounded` strips the cited side
    // the same way, keeping every lookup O(1).
    let mut identifiers = HashSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if is_identifier_kind(kind) {
            if let Ok(text) = node.utf8_text(bytes) {
                if !text.is_empty() {
                    identifiers.insert(strip_raw_prefix(text).to_string());
                }
            }
        } else if kind == "lifetime" {
            // A `lifetime` node's text is `'name` (e.g. `'a`, `'static`). Store the
            // alphanumeric NAME (after the leading `'`) so a finding citing `'a`
            // tokenizes to `a` and matches a real lifetime — keeping the conservative
            // no-false-drop guarantee for lifetime citations.
            if let Ok(text) = node.utf8_text(bytes) {
                let name = text.trim_start_matches('\'');
                if !name.is_empty() {
                    identifiers.insert(name.to_string());
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    (items, identifiers)
}

/// Strip a Rust raw-identifier prefix (`r#`) so `r#type` and `type` compare equal.
/// Used on BOTH the stored identifiers and the cited symbol segments so the
/// canonicalization is symmetric.
fn strip_raw_prefix(ident: &str) -> &str {
    ident.strip_prefix("r#").unwrap_or(ident)
}

/// The leaf node kinds that count as "an identifier present in the file" for symbol
/// grounding. `identifier` covers fns/consts/statics/vars/calls; `type_identifier`
/// covers structs/enums/traits/type-aliases/types; `field_identifier` covers struct
/// fields and method names. Lifetime NAMES are collected separately in the tree walk
/// (the `lifetime` branch in [`parse_rust`]) so a finding citing `'a` is grounded.
/// Labels / primitive-type keywords are NOT identifiers a finding would meaningfully
/// cite, so they are excluded.
fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "type_identifier" | "field_identifier")
}

/// Pull the human-readable name from a top-level Rust item node, if the grammar
/// exposes one. Resolution is EXPLICIT per kind (verified against the
/// tree-sitter-rust 0.24.2 grammar — child fields differ subtly and a generic
/// `name`-then-`type` fallback is WRONG: for `type_item` the `type` field is the
/// alias's RIGHT-HAND side, e.g. `u64` for `type Id = u64;`, not the new name):
///   - `function_item` / `struct_item` / `enum_item` / `trait_item` / `mod_item` /
///     `const_item` / `static_item` / `macro_definition` / `type_item` — the `name`
///     field holds the declared identifier (for `type_item` that is the new alias
///     name, `Id`; its `type` field — the RHS — is deliberately NOT used).
///   - `impl_item` — has NO `name` field. We name it by its `type` field (the type
///     being implemented, e.g. `Point` for both `impl Point` and `impl Display for
///     Point`). Generic args are STRIPPED to the base name (`impl<T> Wrapper<T>` →
///     `Wrapper`, `impl Display for Vec<String>` → `Vec`) so the name is a stable
///     grouping key. That ties the review unit to the type it extends — useful for
///     grouping — even though tree-sitter calls the impl itself anonymous. A trait
///     impl additionally has a `trait` field; we keep the simpler type name rather
///     than synthesizing a composite, since `name` is a display hint, not an id.
/// Anything else (or a missing/empty field) → `None` rather than guessing.
fn rust_item_name(node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    let is_impl = node.kind() == "impl_item";
    // `impl_item` is the one kind whose name lives under `type`, not `name`.
    let field = if is_impl {
        node.child_by_field_name("type")
    } else {
        node.child_by_field_name("name")
    };
    let child = field?;
    let text = child.utf8_text(bytes).ok()?;
    // For an `impl`, the `type` field carries the FULL implemented type INCLUDING
    // generic args (`impl<T> Wrapper<T>` → `Wrapper<T>`; `impl Display for Vec<String>`
    // → `Vec<String>`). The name is a display/grouping hint, so strip from the first
    // `<` to the type's BASE name (`Wrapper`, `Vec`). Non-generic impls (`impl Point`)
    // have no `<`, so behavior is unchanged. Other kinds never carry generics in their
    // `name` field, so they pass through verbatim.
    let text = if is_impl {
        text.split('<').next().unwrap_or(text).trim()
    } else {
        text
    };
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Build a tree-sitter parser for Rust and parse `source`. Returns `None` (never
/// panics) if the language can't be set or the parse yields nothing. The grammar is
/// loaded per call — parser construction is cheap (microseconds) relative to a
/// review cadence, and a fresh parser avoids any shared-state / thread concerns.
fn parse_rust_tree(source: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    parser.set_language(&language).ok()?;
    parser.parse(source, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Rust snippet with a `use`, a `struct`, a `fn`, and an `impl` block — the
    /// canonical review-unit mix. Line numbers (1-based) are annotated for the
    /// assertions below.
    const SNIPPET: &str = "\
use std::collections::HashMap;

struct Point {
    x: i32,
    y: i32,
}

fn distance(a: &Point, b: &Point) -> i32 {
    let dx = a.x - b.x;
    dx
}

impl Point {
    fn origin() -> Point {
        Point { x: 0, y: 0 }
    }
}
";

    fn item<'a>(items: &'a [ReviewItem], kind: &str) -> &'a ReviewItem {
        items
            .iter()
            .find(|i| i.kind == kind)
            .unwrap_or_else(|| panic!("expected a {kind} in {items:?}"))
    }

    #[test]
    fn extract_items_rust_kinds_lines_and_names() {
        let items = extract_items(SNIPPET, FileLang::Rust);
        // Exactly three top-level items: struct, fn, impl (the `use` is NOT an item).
        assert_eq!(items.len(), 3, "items: {items:?}");

        let s = item(&items, "struct_item");
        assert_eq!(s.name.as_deref(), Some("Point"));
        // `struct Point {` is line 3; closing `}` is line 6 (1-based inclusive).
        assert_eq!(s.start_line, 3);
        assert_eq!(s.end_line, 6);

        let f = item(&items, "function_item");
        assert_eq!(f.name.as_deref(), Some("distance"));
        // `fn distance(...) {` is line 8; closing `}` is line 11.
        assert_eq!(f.start_line, 8);
        assert_eq!(f.end_line, 11);

        let im = item(&items, "impl_item");
        // An impl has no `name` field; we name it by its implemented `type` (`Point`)
        // so the review unit is tied to the type it extends.
        assert_eq!(im.name.as_deref(), Some("Point"));
        // `impl Point {` is line 13; closing `}` is line 17.
        assert_eq!(im.start_line, 13);
        assert_eq!(im.end_line, 17);
    }

    #[test]
    fn extract_items_impl_names_by_implemented_type() {
        // Both an inherent impl and a trait impl name by the implemented TYPE, not
        // the trait — the type is the grouping key for the review unit.
        let inherent = extract_items("impl Widget { fn new() {} }\n", FileLang::Rust);
        assert_eq!(inherent.len(), 1);
        assert_eq!(inherent[0].kind, "impl_item");
        assert_eq!(inherent[0].name.as_deref(), Some("Widget"));

        let trait_impl =
            extract_items("impl std::fmt::Display for Widget {}\n", FileLang::Rust);
        assert_eq!(trait_impl.len(), 1);
        assert_eq!(trait_impl[0].kind, "impl_item");
        assert_eq!(trait_impl[0].name.as_deref(), Some("Widget"));
    }

    #[test]
    fn extract_items_type_alias_names_lhs_not_rhs() {
        // Regression guard: `type Id = u64;` must name the alias `Id`, NOT the RHS
        // `u64` (the grammar's `type` field on a `type_item` is the right-hand side).
        let items = extract_items("type Id = u64;\n", FileLang::Rust);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "type_item");
        assert_eq!(items[0].name.as_deref(), Some("Id"));
    }

    #[test]
    fn extract_items_covers_more_kinds() {
        let src = "\
enum Color { Red, Green }
trait Shape { fn area(&self) -> f64; }
mod inner { pub const K: u32 = 1; }
const TOP: u32 = 9;
static GREETING: &str = \"hi\";
type Id = u64;
macro_rules! noop { () => {}; }
";
        let items = extract_items(src, FileLang::Rust);
        let kinds: HashSet<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        for expected in [
            "enum_item",
            "trait_item",
            "mod_item",
            "const_item",
            "static_item",
            "type_item",
            "macro_definition",
        ] {
            assert!(kinds.contains(expected), "missing {expected} in {kinds:?}");
        }
        // Names where the grammar exposes one.
        assert_eq!(item(&items, "enum_item").name.as_deref(), Some("Color"));
        assert_eq!(item(&items, "type_item").name.as_deref(), Some("Id"));
        assert_eq!(item(&items, "macro_definition").name.as_deref(), Some("noop"));
        // `const K` is NESTED in `mod inner` — only the top-level `const TOP` is an
        // item, so there is exactly one `const_item` and it is `TOP`.
        let consts: Vec<_> = items.iter().filter(|i| i.kind == "const_item").collect();
        assert_eq!(consts.len(), 1);
        assert_eq!(consts[0].name.as_deref(), Some("TOP"));
    }

    #[test]
    fn extract_items_empty_source_is_empty() {
        assert!(extract_items("", FileLang::Rust).is_empty());
    }

    #[test]
    fn extract_items_non_rust_is_empty() {
        // No grammar wired yet for these — graceful empty, never a panic.
        assert!(extract_items(SNIPPET, FileLang::Ts).is_empty());
        assert!(extract_items("def f(): pass", FileLang::Py).is_empty());
        assert!(extract_items("anything", FileLang::Other).is_empty());
    }

    #[test]
    fn extract_items_malformed_rust_does_not_panic() {
        // tree-sitter is error-tolerant; it still produces a tree. We only assert
        // we don't panic and we get a sane Vec.
        let _ = extract_items("fn broken( { let x =", FileLang::Rust);
        let _ = extract_items("}}}{{{ not rust at all 流", FileLang::Rust);
    }

    #[test]
    fn grounding_drops_line_past_eof() {
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        // SNIPPET has 17 content lines; a finding at line 999 is a structural lie.
        assert!(parsed.total_lines >= 17);
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        // Line 0 is also out of the 1-based range.
        assert_eq!(grounds(&parsed, Some(0), None), Grounding::DroppedLineOutOfFile);
    }

    #[test]
    fn grounding_keeps_valid_in_file_line() {
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        // A finding on the `fn distance` line (8) is in range → kept.
        assert_eq!(grounds(&parsed, Some(8), None), Grounding::Kept);
        // The last line is valid too.
        assert_eq!(grounds(&parsed, Some(parsed.total_lines), None), Grounding::Kept);
    }

    #[test]
    fn grounding_keeps_line_between_items() {
        // Line 1 is the `use` import — INSIDE the file but OUTSIDE any item. Being
        // between items is NOT a contradiction → kept (no over-drop).
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(grounds(&parsed, Some(1), None), Grounding::Kept);
        // Line 2 is blank — also in range, also kept.
        assert_eq!(grounds(&parsed, Some(2), None), Grounding::Kept);
    }

    #[test]
    fn grounding_keeps_present_symbol() {
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        // `Point`, `distance`, `origin`, the fields `x`/`y` are all in the file.
        assert!(parsed.identifiers.contains("Point"));
        assert_eq!(grounds(&parsed, Some(8), Some("distance")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("Point")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("x")), Grounding::Kept);
    }

    #[test]
    fn grounding_drops_absent_symbol() {
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        // `nonexistent_fn` is nowhere in the file, and the grammar DID produce
        // identifiers → unambiguous contradiction → drop.
        assert!(!parsed.identifiers.is_empty());
        assert_eq!(
            grounds(&parsed, Some(8), Some("nonexistent_fn")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn grounding_keeps_qualified_path_with_known_components() {
        // CONSERVATIVE: a reviewer may cite `Point::origin` while the grammar
        // collected `Point` and `origin` as SEPARATE identifiers. Both components
        // exist → NOT a contradiction → kept (this is the over-drop guard).
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert!(parsed.identifiers.contains("Point"));
        assert!(parsed.identifiers.contains("origin"));
        assert!(!parsed.identifiers.contains("Point::origin"));
        assert_eq!(grounds(&parsed, Some(14), Some("Point::origin")), Grounding::Kept);
        // A field access path too: `self.x` — `x` is a known field.
        assert_eq!(grounds(&parsed, Some(15), Some("self.x")), Grounding::Kept);
    }

    #[test]
    fn grounding_keeps_path_when_one_component_is_known() {
        // `Point::nonexistent_method` — `Point` exists, the method does not. We KEEP
        // it: a wrong method on a real type is a SEMANTIC issue for a later tier, not
        // a structural hallucination. Only a WHOLLY invented path is dropped.
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(
            grounds(&parsed, Some(14), Some("Point::nonexistent_method")),
            Grounding::Kept
        );
    }

    #[test]
    fn grounding_drops_fully_invented_path() {
        // No component exists anywhere in the file → unambiguous contradiction.
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(
            grounds(&parsed, Some(8), Some("Bogus::nope")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn grounding_raw_identifier_matches_plain_citation() {
        // A declared `r#type` is stored verbatim; a finding citing plain `type`
        // should still match (we strip the `r#` prefix on both sides).
        let src = "fn f() { let r#type = 1; let _ = r#type; }\n";
        let parsed = parse_file(src, FileLang::Rust);
        // Stored in canonical (stripped) form.
        assert!(parsed.identifiers.contains("type"));
        assert_eq!(grounds(&parsed, Some(1), Some("type")), Grounding::Kept);
        assert_eq!(grounds(&parsed, Some(1), Some("r#type")), Grounding::Kept);
    }

    #[test]
    fn grounding_empty_symbol_is_kept() {
        // An empty or separator-only symbol has nothing concrete to contradict.
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(grounds(&parsed, Some(8), Some("")), Grounding::Kept);
        assert_eq!(grounds(&parsed, Some(8), Some("::")), Grounding::Kept);
    }

    #[test]
    fn grounding_line_check_precedes_symbol_check() {
        // A finding that is BOTH out-of-file AND cites a bogus symbol reports the
        // line contradiction (the more fundamental structural lie).
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(
            grounds(&parsed, Some(99_999), Some("nonexistent_fn")),
            Grounding::DroppedLineOutOfFile
        );
    }

    #[test]
    fn grounding_keeps_when_no_line_and_no_symbol() {
        // A file-level finding (no line, no symbol) is never a structural
        // contradiction → always kept.
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(grounds(&parsed, None, None), Grounding::Kept);
    }

    #[test]
    fn grounding_no_grammar_keeps_unknown_symbol_but_still_checks_line() {
        // Python: no grammar wired → no identifiers → symbol grounding DISABLED
        // (unknown != contradicted), but line-range grounding still applies.
        let py = "def add(a, b):\n    return a + b\n"; // 2 content lines
        let parsed = parse_file(py, FileLang::Py);
        assert!(parsed.items.is_empty());
        assert!(parsed.identifiers.is_empty());
        // An unknown symbol is KEPT (we can't vouch, so we don't contradict).
        assert_eq!(grounds(&parsed, Some(1), Some("whatever")), Grounding::Kept);
        // But an out-of-file line is STILL dropped (only needs the line count).
        assert_eq!(parsed.total_lines, 2);
        assert_eq!(grounds(&parsed, Some(3), None), Grounding::DroppedLineOutOfFile);
        // An in-range line with no symbol → kept.
        assert_eq!(grounds(&parsed, Some(2), None), Grounding::Kept);
    }

    #[test]
    fn parse_file_empty_source_has_zero_lines() {
        let parsed = parse_file("", FileLang::Rust);
        assert_eq!(parsed.total_lines, 0);
        // Any positive line is therefore out of file.
        assert_eq!(grounds(&parsed, Some(1), None), Grounding::DroppedLineOutOfFile);
    }

    #[test]
    fn count_lines_ignores_trailing_newline() {
        // "a\nb" and "a\nb\n" both address 2 content lines.
        assert_eq!(parse_file("a\nb", FileLang::Other).total_lines, 2);
        assert_eq!(parse_file("a\nb\n", FileLang::Other).total_lines, 2);
        assert_eq!(parse_file("a", FileLang::Other).total_lines, 1);
    }

    #[test]
    fn ground_findings_partitions_kept_and_dropped() {
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        // (line, symbol) tuples standing in for the C4 caller's finding slice.
        let findings: Vec<(Option<u32>, Option<&str>)> = vec![
            (Some(8), Some("distance")),       // kept
            (Some(99_999), None),              // dropped: line out of file
            (Some(8), Some("nonexistent_fn")), // dropped: unknown symbol
            (None, None),                      // kept
            (Some(1), None),                   // kept: between-items line
        ];
        let (kept, dropped) =
            ground_findings(&parsed, &findings, |f| (f.0, f.1));
        assert_eq!(kept.len(), 3);
        assert_eq!(dropped.len(), 2);
        // Reasons carried through for provenance.
        let reasons: Vec<Grounding> = dropped.iter().map(|(_, r)| *r).collect();
        assert!(reasons.contains(&Grounding::DroppedLineOutOfFile));
        assert!(reasons.contains(&Grounding::DroppedUnknownSymbol));
    }

    #[test]
    fn extract_items_impl_generic_names_strip_type_args() {
        // `impl<T> Wrapper<T> {}` → base name `Wrapper`, NOT `Wrapper<T>` (the `type`
        // field carries generic args; we strip from the first `<`).
        let g = extract_items("impl<T> Wrapper<T> {}\n", FileLang::Rust);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].kind, "impl_item");
        assert_eq!(g[0].name.as_deref(), Some("Wrapper"));

        // A bounded generic trait impl names by the implemented TYPE's base.
        let tg = extract_items(
            "impl<T: Clone> Iterator for Wrapper<T> {}\n",
            FileLang::Rust,
        );
        assert_eq!(tg.len(), 1);
        assert_eq!(tg[0].kind, "impl_item");
        assert_eq!(tg[0].name.as_deref(), Some("Wrapper"));

        // A trait impl on a generic std type → the implemented type's base (`Vec`).
        let v = extract_items("impl Display for Vec<String> {}\n", FileLang::Rust);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name.as_deref(), Some("Vec"));

        // Regression: a non-generic impl is unchanged.
        let p = extract_items("impl Point {}\n", FileLang::Rust);
        assert_eq!(p[0].name.as_deref(), Some("Point"));
    }

    #[test]
    fn grounding_keeps_generic_symbol_with_known_components() {
        // `Vec<T>` cited as a symbol: tokens `Vec` and `T`. With both present the
        // finding is KEPT — the `<>` must not cause a false drop.
        let src = "fn f<T>() -> Vec<T> { Vec::new() }\n";
        let parsed = parse_file(src, FileLang::Rust);
        assert!(parsed.identifiers.contains("Vec"));
        assert!(parsed.identifiers.contains("T"));
        assert_eq!(grounds(&parsed, Some(1), Some("Vec<T>")), Grounding::Kept);
        // Turbofish shape too.
        assert_eq!(grounds(&parsed, Some(1), Some("Vec::<T>")), Grounding::Kept);
    }

    #[test]
    fn grounding_keeps_macro_symbol_without_bang() {
        // The grammar stores a macro invocation name WITHOUT the `!`. A finding citing
        // `println!` tokenizes to `println` and must match.
        let src = "fn f() { println!(\"hi\"); }\n";
        let parsed = parse_file(src, FileLang::Rust);
        assert!(parsed.identifiers.contains("println"));
        assert_eq!(grounds(&parsed, Some(1), Some("println!")), Grounding::Kept);
    }

    #[test]
    fn grounding_keeps_lifetime_symbol() {
        // A real lifetime `'a` is collected (name `a`); a finding citing `'a` is KEPT.
        let src = "fn f<'a>(x: &'a str) -> &'a str { x }\n";
        let parsed = parse_file(src, FileLang::Rust);
        assert!(
            parsed.identifiers.contains("a"),
            "lifetime name 'a' missing: {:?}",
            parsed.identifiers
        );
        assert_eq!(grounds(&parsed, Some(1), Some("'a")), Grounding::Kept);
    }

    #[test]
    fn grounding_keeps_real_type_wrong_method_drops_fully_invented() {
        // `Point::nonexistent_method` — `Point` exists, method does not → KEPT (a wrong
        // method on a real type is SEMANTIC). `Bogus::nope` — neither exists → DROPPED.
        let parsed = parse_file(SNIPPET, FileLang::Rust);
        assert_eq!(
            grounds(&parsed, Some(14), Some("Point::nonexistent_method")),
            Grounding::Kept
        );
        assert_eq!(
            grounds(&parsed, Some(8), Some("Bogus::nope")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn count_lines_crlf_last_line_kept_one_past_eof_dropped() {
        // A `\r\n`-terminated source: `lines()` handles CRLF, so the line count matches
        // the content lines. A finding on the LAST line is kept; one past EOF dropped.
        let src = "fn a() {}\r\nfn b() {}\r\n"; // 2 content lines
        let parsed = parse_file(src, FileLang::Rust);
        assert_eq!(parsed.total_lines, 2);
        assert_eq!(grounds(&parsed, Some(2), None), Grounding::Kept);
        assert_eq!(
            grounds(&parsed, Some(3), None),
            Grounding::DroppedLineOutOfFile
        );
    }

    #[test]
    fn extract_items_matches_parse_file_items_single_path() {
        // `extract_items` delegates to `parse_file` (one parse path): the items it
        // returns are identical to `parse_file(...).items`.
        let via_extract = extract_items(SNIPPET, FileLang::Rust);
        let via_parse = parse_file(SNIPPET, FileLang::Rust).items;
        assert_eq!(via_extract, via_parse);
    }
}
