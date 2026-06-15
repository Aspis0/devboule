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
//! PRODUCT GENERALITY: the API is keyed on [`FileLang`]. Wired grammars: Rust,
//! TS/JS (the TSX grammar — broadest of the TS family, parses TS/JS/JSX/TSX),
//! Python, Go, C/C++, HTML, and Kotlin. `FileLang::Other` degrades gracefully —
//! [`extract_items`] returns an empty `Vec` and [`parse_file`] yields an empty
//! identifier set, so symbol grounding is disabled for it (unknown != contradicted)
//! while the universal line-range grounding still applies (it only needs the line
//! count). tree-sitter is not OS-specific, so there is NO `cfg` gating here.
//!
//! NO GRAMMAR (lint-runner-only quick wins): `FileLang::Shell`/`Yaml`/`Sql`/
//! `Dockerfile`/`GithubActions`/`Css` are deliberately NOT given a tree-sitter
//! grammar — they exist only to route the shellcheck/yamllint/sqlfluff/hadolint/
//! actionlint/stylelint lint runners. They degrade EXACTLY like
//! `FileLang::Other`: [`extract_items`] returns an empty `Vec` and [`parse_file`]
//! yields an empty identifier set, so symbol grounding stays DISABLED for them
//! (unknown != contradicted) while the universal line-range grounding still applies.
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
/// TS/JS (`Ts`): parsed with `tree-sitter-typescript`'s TSX grammar; returns the
/// top-level `function_declaration`, `generator_function_declaration`,
/// `class_declaration`, `abstract_class_declaration`, `interface_declaration`,
/// `type_alias_declaration`, `enum_declaration`, and `lexical_declaration` /
/// `variable_declaration` whose initializer is an arrow/function (named by the bound
/// variable), UNWRAPPING `export_statement` to its inner declaration.
///
/// Python (`Py`): parsed with `tree-sitter-python`; returns the top-level
/// `function_definition` and `class_definition`, UNWRAPPING `decorated_definition`.
///
/// Go (`Go`): parsed with `tree-sitter-go`; returns the top-level
/// `function_declaration`, `method_declaration` (Go methods ARE top-level — they sit
/// directly under `source_file`, unlike class methods in TS/Python), `type_declaration`
/// (named by its inner `type_spec`'s `name` — covers struct/interface/alias),
/// `const_declaration`, and `var_declaration`. Names come from the `name` field where
/// the grammar exposes one (a `const`/`var` block has no single declaration-level name,
/// so it is left `None`).
///
/// C/C++ (`Cpp`): parsed with `tree-sitter-cpp` (a superset grammar that parses both C
/// and C++); returns the top-level `function_definition`, `declaration` ONLY when it
/// declares a function prototype (a plain variable declaration is skipped), and the
/// `class_specifier` / `struct_specifier` / `enum_specifier` / `union_specifier` /
/// `namespace_definition` type/namespace units — UNWRAPPING a `template_declaration` to
/// its inner declaration. C++ declarators are deeply nested, so a function name is the
/// leaf identifier found by descending through the declarator chain (best-effort —
/// `None` if it can't be resolved); a class/struct/enum/union/namespace is named by its
/// `name`/`type_identifier` field.
///
/// HTML (`Html`): parsed with `tree-sitter-html`; returns the top-level `element`
/// children of the `document` root. HTML's "review unit" is WEAK (a document is a tree
/// of nested elements, not a list of named declarations), so items are best-effort:
/// each top-level element is named by its `tag_name` (e.g. `html`, `body`). The
/// load-bearing job for HTML is identifier collection for grounding — the names a
/// finding would cite: every `tag_name` plus the VALUES of the name/reference attributes
/// `id`/`class`/`name`/`for`/`href`/`src`/`action` (see [`collect_html_identifiers`]).
///
/// Kotlin (`Kotlin`): parsed with `tree-sitter-kotlin-ng`; returns the top-level
/// `function_declaration`, `class_declaration`, `object_declaration`, and
/// `property_declaration` units (best-effort — node-kind names per the grammar's
/// `node-types.json`), named by the `identifier` found under the declaration.
///
/// `Shell`/`Yaml`/`Sql`/`Other`: no grammar wired — returns an empty `Vec` (the
/// shell/YAML/SQL langs are lint-runner-only quick wins; see the module header).
/// This NEVER panics (a grammar/parse failure also yields an empty `Vec`).
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
        FileLang::Ts => {
            let (items, identifiers) = parse_ts(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        FileLang::Py => {
            let (items, identifiers) = parse_py(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        FileLang::Go => {
            let (items, identifiers) = parse_go(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        FileLang::Cpp => {
            let (items, identifiers) = parse_cpp(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        FileLang::Html => {
            let (items, identifiers) = parse_html(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        FileLang::Kotlin => {
            let (items, identifiers) = parse_kotlin(source);
            ParsedFile {
                total_lines,
                items,
                identifiers,
            }
        }
        // No grammar wired for these (lint-runner-only quick wins) — they degrade
        // exactly like `Other`: empty items + empty identifier set (symbol grounding
        // disabled), with the real `total_lines` so line-range grounding still works.
        FileLang::Shell
        | FileLang::Yaml
        | FileLang::Sql
        | FileLang::Dockerfile
        | FileLang::GithubActions
        | FileLang::Css
        | FileLang::Other => ParsedFile {
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

// ===========================================================================
// TS/JS (TSX grammar)
// ===========================================================================

/// The TS/JS top-level item node kinds we treat as review units, as direct children
/// of the `program` root (or the inner declaration of an `export_statement`, which we
/// unwrap). The first group is named by a `name` field; the two variable-binding kinds
/// (`lexical_declaration` for `let`/`const`, `variable_declaration` for `var`) are
/// special-cased in [`ts_top_level_item`] because a top-level `const foo = () => {}`
/// is a function in everything but grammar shape, and its name lives on the
/// `variable_declarator`, not on the declaration node.
const TS_ITEM_KINDS: [&str; 7] = [
    "function_declaration",
    "generator_function_declaration",
    "class_declaration",
    "abstract_class_declaration",
    "interface_declaration",
    "type_alias_declaration",
    "enum_declaration",
];

/// Build a [`ReviewItem`] from a top-level TS/JS node IF it is a review unit; else
/// `None`. The node passed in is the (already export-unwrapped) declaration. Lines and
/// names come from the SAME node we report (so the line range covers the declaration,
/// not the `export` keyword); see [`ts_top_level_item`]'s caller for the unwrap. The
/// row math SATURATES (`+1` on `u32::MAX` would wrap) — pathological for real source,
/// but a wrapped tiny line number would corrupt grounding, so we clamp.
fn ts_review_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    let kind = node.kind();
    let name = if TS_ITEM_KINDS.contains(&kind) {
        // The declared identifier lives in the `name` field for every one of these.
        node.child_by_field_name("name")
            .and_then(|n| n.utf8_text(bytes).ok())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
    } else if kind == "lexical_declaration" || kind == "variable_declaration" {
        // ONLY a binding whose initializer is an arrow/function is an item — a plain
        // `const X = 1;` is data, not a review unit. Name = the bound variable.
        ts_function_binding_name(node, bytes)?
    } else {
        return None;
    };
    Some(ReviewItem {
        kind: kind.to_string(),
        name,
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// For a `lexical_declaration`/`variable_declaration`, return `Some(Some(name))` when
/// its FIRST `variable_declarator`'s `value` is an arrow or function expression (so the
/// declaration is a function bound to a name), where `name` is the declarator's `name`
/// field text; `Some(None)` if it is a function binding with no readable name; and
/// `None` (filtering the whole declaration out as a review unit) when no declarator
/// binds a function. We look at the first declarator: a multi-binding `const a = () =>
/// {}, b = 1;` is rare and the first binding decides the unit's identity.
fn ts_function_binding_name(
    node: &tree_sitter::Node,
    bytes: &[u8],
) -> Option<Option<String>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let value = match child.child_by_field_name("value") {
            Some(v) => v,
            None => continue,
        };
        if !matches!(
            value.kind(),
            "arrow_function" | "function_expression" | "function" | "generator_function"
        ) {
            continue;
        }
        let name = child
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(bytes).ok())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string());
        return Some(name);
    }
    None
}

/// The TS/JS leaf node kinds that count as "an identifier present in the file" for
/// symbol grounding. `identifier` covers variables/functions/calls/imports;
/// `property_identifier` covers object keys, class members and method names;
/// `type_identifier` covers classes/interfaces/type-aliases/enums and type
/// references; `shorthand_property_identifier` covers `{ x }` object shorthand and its
/// pattern form. Same conservative rule as Rust: collect every name a finding would
/// plausibly cite so symbol grounding never false-drops a real finding.
fn is_ts_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "property_identifier"
            | "type_identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
    )
}

/// Parse TS/JS `source` into `(items, identifiers)` — the SINGLE TS parse routine.
/// Top-level items are the direct children of `program`, with `export_statement`
/// UNWRAPPED to its inner `declaration` (so `export function f` reports the
/// `function_declaration`, named `f`, with the export's line range collapsed onto the
/// declaration). The identifier set is every identifier-like leaf in the whole tree
/// (see [`is_ts_identifier_kind`]). A parse failure yields empty + empty (symbol
/// grounding then disabled — fail-open, never a false drop).
fn parse_ts(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_with(source, tree_sitter_typescript::LANGUAGE_TSX.into()) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        // Unwrap `export ...` / `export default ...` to the declaration it carries; a
        // re-export with no declaration (`export { x }`, `export * from ...`) has no
        // `declaration` field, so it is skipped (not a review unit).
        let decl = if child.kind() == "export_statement" {
            match child.child_by_field_name("declaration") {
                Some(d) => d,
                None => continue,
            }
        } else {
            child
        };
        if let Some(item) = ts_review_item(&decl, bytes) {
            items.push(item);
        }
    }

    let identifiers = collect_identifiers(root, bytes, is_ts_identifier_kind);
    (items, identifiers)
}

// ===========================================================================
// Python
// ===========================================================================

/// The Python top-level item node kinds we treat as review units, as direct children
/// of the `module` root (or the inner `definition` of a `decorated_definition`, which
/// we unwrap). Both are named by a `name` field.
const PY_ITEM_KINDS: [&str; 2] = ["function_definition", "class_definition"];

/// Build a [`ReviewItem`] from a (already decorator-unwrapped) top-level Python node
/// IF it is a review unit; else `None`. Name comes from the `name` field. Row math
/// SATURATES (see [`ts_review_item`]).
fn py_review_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    let kind = node.kind();
    if !PY_ITEM_KINDS.contains(&kind) {
        return None;
    }
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string());
    Some(ReviewItem {
        kind: kind.to_string(),
        name,
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// Parse Python `source` into `(items, identifiers)` — the SINGLE Python parse routine.
/// Top-level items are the direct children of `module`, with `decorated_definition`
/// UNWRAPPED to its inner `definition` (so `@deco\ndef f` reports the
/// `function_definition`, named `f`; the line range is taken from that inner node, i.e.
/// the `def`/`class` line through the body). The identifier set is every `identifier`
/// leaf in the whole tree. A parse failure yields empty + empty (fail-open).
fn parse_py(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_with(source, tree_sitter_python::LANGUAGE.into()) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        // Unwrap `@deco\ndef/class` to the definition it decorates.
        let def = if child.kind() == "decorated_definition" {
            match child.child_by_field_name("definition") {
                Some(d) => d,
                None => continue,
            }
        } else {
            child
        };
        if let Some(item) = py_review_item(&def, bytes) {
            items.push(item);
        }
    }

    let identifiers = collect_identifiers(root, bytes, |k| k == "identifier");
    (items, identifiers)
}

// ===========================================================================
// Go
// ===========================================================================

/// The Go top-level item node kinds we treat as review units, as direct children of
/// the `source_file` root. `function_declaration` (free functions) and
/// `method_declaration` (methods — in Go these are TOP-LEVEL, declared on a receiver
/// directly under the root, NOT nested inside their type) are named by a `name`
/// field. `type_declaration` (which wraps one or more `type_spec`s — struct,
/// interface, or alias) has NO `name` field of its own; it is named by its FIRST
/// inner `type_spec`'s `name` (see [`go_item_name`]). `const_declaration` /
/// `var_declaration` wrap `const_spec`/`var_spec`(s) and likewise carry no single
/// declaration-level `name` (a `const ( ... )` block binds many), so they are review
/// units with no name (`None`).
const GO_ITEM_KINDS: [&str; 5] = [
    "function_declaration",
    "method_declaration",
    "type_declaration",
    "const_declaration",
    "var_declaration",
];

/// Build a [`ReviewItem`] from a direct child of the `source_file` root IF it is one
/// of [`GO_ITEM_KINDS`]; else `None`. Row math SATURATES (see [`ts_review_item`]).
fn go_top_level_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    let kind = node.kind();
    if !GO_ITEM_KINDS.contains(&kind) {
        return None;
    }
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    Some(ReviewItem {
        kind: kind.to_string(),
        name: go_item_name(node, bytes),
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// Pull the name from a top-level Go item node, where the grammar exposes one:
///   - `function_declaration` / `method_declaration` — the `name` field holds the
///     declared identifier (the function name, or the method name; the receiver is a
///     separate field we deliberately ignore for the display name).
///   - `type_declaration` — has NO `name` field; it wraps `type_spec`(s). We name it
///     by the FIRST `type_spec`'s `name` field (`type Point struct{}` → `Point`,
///     covering struct/interface/alias). A grouped `type ( A …; B … )` block reports
///     the first spec's name as the unit's display hint.
///   - `const_declaration` / `var_declaration` — no single declaration-level name (a
///     block binds many specs), so `None`.
/// Anything else / a missing field → `None` (never guess).
fn go_item_name(node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    let field = if node.kind() == "type_declaration" {
        // The declaration has no `name`; descend to the first `type_spec` and take its
        // `name` field (the new type's identifier — NOT the underlying type).
        let mut cursor = node.walk();
        let spec = node
            .children(&mut cursor)
            .find(|c| c.kind() == "type_spec" || c.kind() == "type_alias")?;
        spec.child_by_field_name("name")
    } else {
        node.child_by_field_name("name")
    };
    let child = field?;
    let text = child.utf8_text(bytes).ok()?;
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// The Go leaf node kinds that count as "an identifier present in the file" for symbol
/// grounding. `identifier` covers funcs/consts/vars/locals/calls; `type_identifier`
/// covers type names (structs/interfaces/aliases and type references);
/// `field_identifier` covers struct fields and method names; `package_identifier`
/// covers the package qualifier in a selector (`fmt` in `fmt.Println`) — a finding
/// would plausibly cite any of these. Same conservative rule as the other languages:
/// collect every name a finding might cite so symbol grounding never false-drops a
/// real Go finding.
fn is_go_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "type_identifier" | "field_identifier" | "package_identifier"
    )
}

/// Parse Go `source` into `(items, identifiers)` — the SINGLE Go parse routine. Top-
/// level items are the direct children of `source_file` (see [`GO_ITEM_KINDS`]); the
/// identifier set is every identifier-like leaf in the whole tree (see
/// [`is_go_identifier_kind`]). A parse failure yields empty + empty (symbol grounding
/// then disabled — fail-open, never a false drop).
fn parse_go(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_with(source, tree_sitter_go::LANGUAGE.into()) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        if let Some(item) = go_top_level_item(&child, bytes) {
            items.push(item);
        }
    }

    let identifiers = collect_identifiers(root, bytes, is_go_identifier_kind);
    (items, identifiers)
}

// ===========================================================================
// C / C++
// ===========================================================================

/// The C/C++ top-level item node kinds we treat as review units, as direct children
/// of the `translation_unit` root (a `template_declaration` is UNWRAPPED to its inner
/// declaration first — see [`cpp_unwrap_template`]). `function_definition` is a
/// defined function/method; `declaration` is included ONLY when it declares a function
/// prototype (a plain variable declaration is NOT a review unit — see
/// [`cpp_declaration_is_function`]); `class_specifier` / `struct_specifier` /
/// `enum_specifier` / `union_specifier` are the aggregate type units; and
/// `namespace_definition` is a namespace. Names come from the relevant declarator/name
/// field (see [`cpp_item_name`]).
const CPP_ITEM_KINDS: [&str; 7] = [
    "function_definition",
    "declaration",
    "class_specifier",
    "struct_specifier",
    "enum_specifier",
    "union_specifier",
    "namespace_definition",
];

/// The C/C++ declarator-wrapper kinds that nest around the leaf name. C++ declarators
/// are recursive — `int *const f()` parses as `pointer_declarator` →
/// `function_declarator` → `identifier` — so to find the declared name we descend
/// through every wrapper toward the leaf. Listed exhaustively so an unrecognized
/// wrapper stops the descent (best-effort, never guesses past a node we don't know).
const CPP_DECLARATOR_WRAPPERS: [&str; 6] = [
    "pointer_declarator",
    "reference_declarator",
    "function_declarator",
    "parenthesized_declarator",
    "array_declarator",
    "init_declarator",
];

/// The C/C++ leaf node kinds that name a declared entity (the name a function/method
/// declarator ultimately resolves to). `identifier` is the common case; `field_identifier`
/// is an out-of-line member name; `qualified_identifier` is `Klass::method` (we take its
/// text verbatim as the display hint); `destructor_name` / `operator_name` cover `~T()`
/// and `operator==`.
const CPP_NAME_LEAVES: [&str; 5] = [
    "identifier",
    "field_identifier",
    "qualified_identifier",
    "destructor_name",
    "operator_name",
];

/// If `node` is a `template_declaration`, return its inner declaration child (the
/// thing the template wraps — a `function_definition`, `class_specifier`, etc.);
/// otherwise return `node` unchanged. A `template_declaration`'s last meaningful child
/// is the templated entity (the `template<...>` parameter list precedes it); we pick
/// the FIRST child whose kind is one of [`CPP_ITEM_KINDS`]. If none is found (a bare
/// template forward-declaration), the node is returned unchanged and simply won't
/// match an item kind. Cloned `Node` is `Copy`, so this is cheap.
fn cpp_unwrap_template<'a>(node: tree_sitter::Node<'a>) -> tree_sitter::Node<'a> {
    if node.kind() != "template_declaration" {
        return node;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if CPP_ITEM_KINDS.contains(&child.kind()) {
            return child;
        }
    }
    node
}

/// Does this `declaration` node declare a FUNCTION (a prototype), as opposed to a plain
/// variable? A function prototype's declarator subtree contains a `function_declarator`
/// (`int f(int);` → `declaration` → `function_declarator`); a variable declaration does
/// not (`int x;` → `declaration` → `identifier`). We scan the declaration's descendants
/// for a `function_declarator`, but DO NOT descend into a nested `function_definition` /
/// nested aggregate body (there is none at this point — a top-level `declaration` is a
/// prototype, not a definition). Bounded DFS, never recurses unbounded.
fn cpp_declaration_is_function(node: &tree_sitter::Node) -> bool {
    let mut stack = vec![*node];
    while let Some(n) = stack.pop() {
        if n.kind() == "function_declarator" {
            return true;
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    false
}

/// Build a [`ReviewItem`] from a direct child of the `translation_unit` root (after
/// template-unwrapping) IF it is one of [`CPP_ITEM_KINDS`]; else `None`. A bare
/// `declaration` that is NOT a function prototype (a plain variable) is skipped. Row
/// math SATURATES (see [`ts_review_item`]).
fn cpp_top_level_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    let node = cpp_unwrap_template(*node);
    let kind = node.kind();
    if !CPP_ITEM_KINDS.contains(&kind) {
        return None;
    }
    // A top-level `declaration` is a review unit ONLY when it is a function prototype;
    // a plain variable declaration is intentionally skipped (don't crash, just drop).
    if kind == "declaration" && !cpp_declaration_is_function(&node) {
        return None;
    }
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    Some(ReviewItem {
        // Report the UNWRAPPED kind so a `template<...> class C {}` is a
        // `class_specifier` (the reviewed unit), not a `template_declaration`.
        kind: kind.to_string(),
        name: cpp_item_name(&node, bytes),
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// Pull the display name from a top-level C/C++ item node, best-effort:
///   - `class_specifier` / `struct_specifier` / `enum_specifier` / `union_specifier` —
///     the `name` field (a `type_identifier`, or a `qualified_identifier`/`template_type`
///     for a specialized/qualified definition). An anonymous aggregate has no `name` →
///     `None`.
///   - `namespace_definition` — the `name` field (`namespace_identifier` or a
///     `nested_namespace_specifier`); an anonymous namespace has none → `None`.
///   - `function_definition` / `declaration` (function prototype) — there is NO single
///     `name` field; the name is the leaf identifier at the bottom of the `declarator`
///     chain (descended via [`cpp_declarator_leaf_name`]).
/// Anything we can't resolve → `None` (never guess).
fn cpp_item_name(node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "class_specifier" | "struct_specifier" | "enum_specifier" | "union_specifier"
        | "namespace_definition" => {
            let name = node.child_by_field_name("name")?;
            let text = name.utf8_text(bytes).ok()?;
            (!text.is_empty()).then(|| text.to_string())
        }
        "function_definition" | "declaration" => {
            let declarator = node.child_by_field_name("declarator")?;
            cpp_declarator_leaf_name(declarator, bytes)
        }
        _ => None,
    }
}

/// Descend a C/C++ `declarator` subtree toward the leaf name, returning that name's
/// text. At each step: if the node IS a name leaf ([`CPP_NAME_LEAVES`]) return its
/// text; if it is a known wrapper ([`CPP_DECLARATOR_WRAPPERS`]) follow its own
/// `declarator` field one level down and repeat; otherwise stop (`None`). The loop is
/// bounded by a small depth cap (pathological nesting can't spin). Best-effort: a name
/// we can't resolve yields `None`, never a guess or a panic.
fn cpp_declarator_leaf_name(mut node: tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    // C++ declarators don't nest deeply in practice; cap to guard against an adversarial
    // or malformed tree without an unbounded loop.
    for _ in 0..64 {
        let kind = node.kind();
        if CPP_NAME_LEAVES.contains(&kind) {
            let text = node.utf8_text(bytes).ok()?;
            return (!text.is_empty()).then(|| text.to_string());
        }
        if CPP_DECLARATOR_WRAPPERS.contains(&kind) {
            node = node.child_by_field_name("declarator")?;
            continue;
        }
        return None;
    }
    None
}

/// The C/C++ leaf node kinds that count as "an identifier present in the file" for
/// symbol grounding. `identifier` covers functions/vars/calls; `type_identifier` covers
/// class/struct/enum/typedef names and type references; `field_identifier` covers struct
/// members and method names; `namespace_identifier` covers a namespace name/qualifier;
/// `destructor_name` / `operator_name` cover `~T` and `operator==`. Same conservative
/// rule as the other languages: OVER-collect every name a finding might cite so symbol
/// grounding never false-drops a real C/C++ finding.
fn is_cpp_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "namespace_identifier"
            | "destructor_name"
            | "operator_name"
    )
}

/// Parse C/C++ `source` into `(items, identifiers)` — the SINGLE C/C++ parse routine.
/// Top-level items are the direct children of `translation_unit` (template-unwrapped;
/// see [`CPP_ITEM_KINDS`]); the identifier set is every identifier-like leaf in the
/// whole tree (see [`is_cpp_identifier_kind`]). A parse failure yields empty + empty
/// (symbol grounding then disabled — fail-open, never a false drop).
fn parse_cpp(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_with(source, tree_sitter_cpp::LANGUAGE.into()) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        if let Some(item) = cpp_top_level_item(&child, bytes) {
            items.push(item);
        }
    }

    let identifiers = collect_identifiers(root, bytes, is_cpp_identifier_kind);
    (items, identifiers)
}

// ===========================================================================
// HTML
// ===========================================================================

/// The HTML attributes whose VALUE is a name a finding would cite, so we collect those
/// values into the identifier set:
///   - `id` / `name` — element identifiers a CSS/JS/a11y finding references.
///   - `class` — one or more space-separated class names; a `class="a b c"` value
///     contributes EACH whitespace-separated token (CSS class names are space-delimited
///     within the attribute).
///   - `for` / `href` / `src` / `action` — REFERENCE attributes: a finding can cite the
///     TARGET of a broken `<label for="email">`, a dangling `href="#missing"`, a bad
///     `src`/`action` (e.g. an a11y "label points to a non-existent id" or a "broken
///     anchor" finding). Without these, the cited target (`email`, `missing`) would be in
///     no `id`/`name` either and the real finding would be FALSE-DROPPED. Over-collecting
///     these is the conservative, stated principle. Their values are tokenized into
///     identifier-like runs (see [`collect_html_attribute`]) so `href="#missing"` yields
///     `missing` and `action="/api/login"` yields `api`/`login` — the bare names a
///     finding cites — rather than a URL/fragment that would never match a cited symbol.
/// Lowercased compare so `ID`/`Class`/`HREF` match.
const HTML_NAME_ATTRS: [&str; 7] = ["id", "class", "name", "for", "href", "src", "action"];

/// The subset of [`HTML_NAME_ATTRS`] whose value is a REFERENCE (a URL, an anchor, an id
/// reference) rather than a bare identifier. For these we collect the identifier-like
/// TOKENS of the value (stripping `#`, `/`, `.`, query punctuation, …) so the bare names
/// a finding cites land in the set; `id`/`name` keep their whole value, `class` is
/// whitespace-split (handled separately).
const HTML_REFERENCE_ATTRS: [&str; 4] = ["for", "href", "src", "action"];

/// Build a [`ReviewItem`] from a direct `element` child of the `document` root; `None`
/// for non-element children (text, comments, doctype). HTML's review unit is WEAK — a
/// top-level element is the best available unit — so this is best-effort: the element is
/// named by its `tag_name` (the opening tag's name) where one is present. Row math
/// SATURATES (see [`ts_review_item`]).
fn html_top_level_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    if node.kind() != "element" {
        return None;
    }
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    Some(ReviewItem {
        kind: "element".to_string(),
        name: html_element_tag_name(node, bytes),
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// The tag name of an `element` node: descend to its `start_tag` (or a
/// `self_closing_tag`) child and take that tag's `tag_name` leaf. `None` if the grammar
/// doesn't expose one (a fragment/erroneous parse). Never guesses.
fn html_element_tag_name(node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let tag = node
        .children(&mut cursor)
        .find(|c| c.kind() == "start_tag" || c.kind() == "self_closing_tag")?;
    let mut tcursor = tag.walk();
    let name = tag.children(&mut tcursor).find(|c| c.kind() == "tag_name")?;
    let text = name.utf8_text(bytes).ok()?;
    (!text.is_empty()).then(|| text.to_string())
}

/// Strip the surrounding quotes from a `quoted_attribute_value` node's text, returning
/// the inner value. The grammar wraps a quoted value as `quoted_attribute_value` with an
/// inner `attribute_value`; we prefer that inner node's text, falling back to trimming
/// the literal `"`/`'` quotes off the node text (an EMPTY `id=""` yields `None`).
fn html_attr_value_text(value_node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    // Prefer the inner `attribute_value` (the unquoted content) when present.
    let mut cursor = value_node.walk();
    if let Some(inner) = value_node
        .children(&mut cursor)
        .find(|c| c.kind() == "attribute_value")
    {
        let text = inner.utf8_text(bytes).ok()?;
        return (!text.is_empty()).then(|| text.to_string());
    }
    // An unquoted `attribute_value` node, or a quoted node we trim the quotes off of.
    let raw = value_node.utf8_text(bytes).ok()?;
    let trimmed = raw.trim_matches(|c| c == '"' || c == '\'');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Collect the HTML names a finding would cite into the identifier set, by a pre-order
/// DFS over an explicit stack (never recurses unbounded — mirrors [`collect_identifiers`]).
/// We collect:
///   - EVERY `tag_name` (`div`, `span`, … — element names a finding references); and
///   - the VALUE of each name/reference attribute (see [`HTML_NAME_ATTRS`]) — `id`/`name`
///     whole, `class` split on whitespace, and the reference attributes `for`/`href`/
///     `src`/`action` tokenized to their bare names (see [`collect_html_attribute`]). An
///     attribute's name is its `attribute_name` child; its value is the following
///     `quoted_attribute_value`/`attribute_value`.
/// Same conservative rule as the other languages: OVER-collect every name a finding
/// might cite so symbol grounding never false-drops a real HTML finding. A parse failure
/// yields an empty set (grounding then disabled — fail-open).
fn collect_html_identifiers(root: tree_sitter::Node, bytes: &[u8]) -> HashSet<String> {
    let mut identifiers = HashSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "tag_name" => {
                if let Ok(text) = node.utf8_text(bytes) {
                    if !text.is_empty() {
                        identifiers.insert(text.to_string());
                    }
                }
            }
            "attribute" => {
                collect_html_attribute(&node, bytes, &mut identifiers);
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    identifiers
}

/// From one `attribute` node, if its name is one of [`HTML_NAME_ATTRS`], collect its
/// value into `identifiers`:
///   - `class` — split on whitespace into its individual class names.
///   - `for`/`href`/`src`/`action` ([`HTML_REFERENCE_ATTRS`]) — tokenized into
///     identifier-like runs (so `#missing` → `missing`, `/api/login` → `api`/`login`),
///     since these carry a REFERENCE (anchor/URL/id-ref), not a bare name.
///   - `id`/`name` — the whole (trimmed) value, the bare identifier itself.
/// The attribute's name is the `attribute_name` child; the value is the
/// `quoted_attribute_value` (or bare `attribute_value`) child. A nameless / valueless
/// attribute contributes nothing.
fn collect_html_attribute(
    node: &tree_sitter::Node,
    bytes: &[u8],
    identifiers: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();
    let name = children
        .iter()
        .find(|c| c.kind() == "attribute_name")
        .and_then(|c| c.utf8_text(bytes).ok())
        .map(|s| s.to_ascii_lowercase());
    let Some(name) = name else { return };
    if !HTML_NAME_ATTRS.contains(&name.as_str()) {
        return;
    }
    let Some(value_node) = children
        .iter()
        .find(|c| c.kind() == "quoted_attribute_value" || c.kind() == "attribute_value")
    else {
        return;
    };
    let Some(value) = html_attr_value_text(value_node, bytes) else {
        return;
    };
    if name == "class" {
        // CSS class names are space-separated within the attribute value.
        for token in value.split_whitespace() {
            if !token.is_empty() {
                identifiers.insert(token.to_string());
            }
        }
    } else if HTML_REFERENCE_ATTRS.contains(&name.as_str()) {
        // A reference value (`#missing`, `/api/login`, `email`) carries the cited target
        // wrapped in URL/anchor punctuation. Collect its identifier-like tokens so a
        // finding citing the bare target (`missing`, `login`, `email`) is grounded.
        for token in ident_tokens(&value) {
            identifiers.insert(token.to_string());
        }
    } else {
        // `id` / `name`: the whole value IS the bare identifier.
        identifiers.insert(value);
    }
}

/// Parse HTML `source` into `(items, identifiers)` — the SINGLE HTML parse routine. Items
/// are the top-level `element` children of `document` (best-effort, named by `tag_name`;
/// see [`html_top_level_item`]); the identifier set is every `tag_name` plus the values of
/// `id`/`class`/`name` attributes (see [`collect_html_identifiers`]). A parse failure
/// yields empty + empty (symbol grounding then disabled — fail-open, never a false drop).
fn parse_html(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_with(source, tree_sitter_html::LANGUAGE.into()) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        if let Some(item) = html_top_level_item(&child, bytes) {
            items.push(item);
        }
    }

    let identifiers = collect_html_identifiers(root, bytes);
    (items, identifiers)
}

// ===========================================================================
// Kotlin
// ===========================================================================

/// The Kotlin top-level item node kinds we treat as review units, as direct children of
/// the `source_file` root (per the `tree-sitter-kotlin-ng` grammar's `node-types.json`):
/// `function_declaration` (top-level `fun`), `class_declaration` (`class`/`interface`),
/// `object_declaration` (`object`), and `property_declaration` (top-level `val`/`var`).
/// Names come from the declaration's `identifier` (see [`kotlin_item_name`]).
const KOTLIN_ITEM_KINDS: [&str; 4] = [
    "function_declaration",
    "class_declaration",
    "object_declaration",
    "property_declaration",
];

/// Build a [`ReviewItem`] from a direct child of the `source_file` root IF it is one of
/// [`KOTLIN_ITEM_KINDS`]; else `None`. Row math SATURATES (see [`ts_review_item`]).
fn kotlin_top_level_item(node: &tree_sitter::Node, bytes: &[u8]) -> Option<ReviewItem> {
    let kind = node.kind();
    if !KOTLIN_ITEM_KINDS.contains(&kind) {
        return None;
    }
    let to_line = |row: usize| -> u32 { u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1) };
    Some(ReviewItem {
        kind: kind.to_string(),
        name: kotlin_item_name(node, bytes),
        start_line: to_line(node.start_position().row),
        end_line: to_line(node.end_position().row),
    })
}

/// Pull the display name from a top-level Kotlin item node, best-effort. The grammar
/// names a declaration via an `identifier` leaf (the sole identifier leaf kind in
/// `tree-sitter-kotlin-ng` 1.1.0 — see [`is_kotlin_identifier_kind`]). For `fun`/`class`/
/// `object` the name leaf is a DIRECT child of the declaration; for a
/// `property_declaration` (`val`/`var`) the grammar nests the name one level deeper
/// inside a `variable_declaration` child (`property_declaration → variable_declaration →
/// identifier`), so we ALSO descend into a `variable_declaration` to find it. We take the
/// FIRST name leaf found in source order. A destructuring `multi_variable_declaration`,
/// or any declaration we can't resolve, yields `None` (never guess).
fn kotlin_item_name(node: &tree_sitter::Node, bytes: &[u8]) -> Option<String> {
    // Prefer the grammar's `name`/`identifier` field where one is exposed.
    if let Some(child) = node.child_by_field_name("name") {
        if let Ok(text) = child.utf8_text(bytes) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // Direct name leaf (fun/class/object).
        if is_kotlin_identifier_kind(child.kind()) {
            if let Ok(text) = child.utf8_text(bytes) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        // A `property_declaration` wraps the name in a `variable_declaration`; descend
        // one level to its first identifier leaf.
        if child.kind() == "variable_declaration" {
            let mut vcursor = child.walk();
            for grandchild in child.children(&mut vcursor) {
                if is_kotlin_identifier_kind(grandchild.kind()) {
                    if let Ok(text) = grandchild.utf8_text(bytes) {
                        if !text.is_empty() {
                            return Some(text.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// The Kotlin leaf node kind that counts as "an identifier present in the file" for
/// symbol grounding. In `tree-sitter-kotlin-ng` 1.1.0, `identifier` is the SOLE
/// identifier leaf kind — its `node-types.json` has no `simple_identifier` or
/// `type_identifier` node (those exist in OTHER Kotlin grammars, not this fork), so
/// matching them would be dead code. A composite `qualified_identifier` (`a.b.c`) is a
/// PARENT of `identifier` leaves, not a leaf itself, so the walk reaches each bare
/// `identifier` underneath it. Same conservative rule as the other languages: collect
/// every name a finding might cite so symbol grounding never false-drops a real Kotlin
/// finding.
fn is_kotlin_identifier_kind(kind: &str) -> bool {
    kind == "identifier"
}

/// Parse Kotlin `source` into `(items, identifiers)` — the SINGLE Kotlin parse routine.
/// Top-level items are the direct children of `source_file` (see [`KOTLIN_ITEM_KINDS`]);
/// the identifier set is every identifier-like leaf in the whole tree (see
/// [`is_kotlin_identifier_kind`]). A parse failure yields empty + empty (symbol grounding
/// then disabled — fail-open, never a false drop).
fn parse_kotlin(source: &str) -> (Vec<ReviewItem>, HashSet<String>) {
    let tree = match parse_with(source, tree_sitter_kotlin_ng::LANGUAGE.into()) {
        Some(t) => t,
        None => return (Vec::new(), HashSet::new()),
    };
    let root = tree.root_node();
    let bytes = source.as_bytes();

    let mut items = Vec::new();
    let mut top_cursor = root.walk();
    for child in root.children(&mut top_cursor) {
        if let Some(item) = kotlin_top_level_item(&child, bytes) {
            items.push(item);
        }
    }

    let identifiers = collect_identifiers(root, bytes, is_kotlin_identifier_kind);
    (items, identifiers)
}

// ===========================================================================
// Shared grammar helpers
// ===========================================================================

/// Build a fresh parser for `language` and parse `source`. Returns `None` (never
/// panics) if the language can't be set (an ABI mismatch would surface here) or the
/// parse yields nothing. Parser construction is cheap (microseconds) and a fresh
/// parser avoids any shared-state / thread concerns — same rationale as the Rust path.
fn parse_with(source: &str, language: tree_sitter::Language) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).ok()?;
    parser.parse(source, None)
}

/// Collect every identifier-like leaf in the tree rooted at `root`, by a pre-order DFS
/// over an explicit stack (never recurses unbounded on a deep tree — mirrors the Rust
/// walk in [`parse_rust`]). `is_ident_kind` decides which node kinds count for the
/// language. Empty-text nodes are skipped. This is the language-agnostic body shared by
/// the TS and Python paths; Rust keeps its own walk because it also folds in `lifetime`
/// names with bespoke `'`-stripping.
fn collect_identifiers(
    root: tree_sitter::Node,
    bytes: &[u8],
    is_ident_kind: impl Fn(&str) -> bool,
) -> HashSet<String> {
    let mut identifiers = HashSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if is_ident_kind(node.kind()) {
            if let Ok(text) = node.utf8_text(bytes) {
                if !text.is_empty() {
                    identifiers.insert(text.to_string());
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    identifiers
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
    fn extract_items_other_lang_is_empty() {
        // `Other` has no grammar — graceful empty, never a panic. (Ts/Py now DO
        // produce items; see the dedicated TS/Python tests below.)
        assert!(extract_items("anything", FileLang::Other).is_empty());
        assert!(extract_items(SNIPPET, FileLang::Other).is_empty());
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
        // `Other`: no grammar → no identifiers → symbol grounding DISABLED (unknown
        // != contradicted), but line-range grounding still applies. (Ts/Py now HAVE
        // grammars, so this invariant is exercised on the grammar-less `Other`.)
        let src = "line one\nline two\n"; // 2 content lines
        let parsed = parse_file(src, FileLang::Other);
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
    fn grounding_lint_only_langs_have_no_grammar_but_keep_line_range() {
        // Shell/Yaml/Sql/Dockerfile/GithubActions/Css are lint-runner-only quick wins with
        // NO tree-sitter grammar: they behave exactly like `Other` — empty items + empty
        // identifiers (symbol grounding DISABLED), with line-range grounding still active.
        let src = "first\nsecond\nthird\n"; // 3 content lines
        for lang in [
            FileLang::Shell,
            FileLang::Yaml,
            FileLang::Sql,
            FileLang::Dockerfile,
            FileLang::GithubActions,
            FileLang::Css,
        ] {
            let parsed = parse_file(src, lang);
            assert!(parsed.items.is_empty(), "{lang:?} should yield no items");
            assert!(
                parsed.identifiers.is_empty(),
                "{lang:?} should yield no identifiers (no grammar)"
            );
            assert_eq!(parsed.total_lines, 3, "{lang:?} line count");
            // Symbol grounding disabled → an invented symbol is KEPT (unknown != contradicted).
            assert_eq!(
                grounds(&parsed, Some(2), Some("invented_name")),
                Grounding::Kept,
                "{lang:?} must not drop on an unknown symbol (no grammar)"
            );
            // Line-range grounding still works: out-of-EOF dropped, in-range kept.
            assert_eq!(
                grounds(&parsed, Some(4), None),
                Grounding::DroppedLineOutOfFile,
                "{lang:?} must drop a line past EOF"
            );
            assert_eq!(
                grounds(&parsed, Some(3), None),
                Grounding::Kept,
                "{lang:?} must keep an in-range line"
            );
        }
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

    // =======================================================================
    // TS/JS (TSX grammar)
    // =======================================================================

    /// A TS snippet exercising every top-level review-unit kind plus a nested method
    /// (which must NOT surface as a top-level item). Line numbers (1-based) annotated.
    const TS_SNIPPET: &str = "\
function foo() {
  return 1;
}

class Bar {
  method() {
    return 2;
  }
}

export function baz() {}

const qux = () => {
  return 3;
};

interface I {
  x: number;
}

type T = string | number;
";

    #[test]
    fn extract_items_ts_kinds_lines_and_names() {
        let items = extract_items(TS_SNIPPET, FileLang::Ts);

        // The top-level units: foo, Bar, baz, qux, I, T — exactly six. The nested
        // `method` inside `Bar` is NOT a top-level item.
        assert_eq!(items.len(), 6, "items: {items:?}");

        let foo = item(&items, "function_declaration");
        assert_eq!(foo.name.as_deref(), Some("foo"));
        // `function foo() {` is line 1; closing `}` is line 3.
        assert_eq!(foo.start_line, 1);
        assert_eq!(foo.end_line, 3);

        let bar = item(&items, "class_declaration");
        assert_eq!(bar.name.as_deref(), Some("Bar"));
        // `class Bar {` is line 5; closing `}` is line 9.
        assert_eq!(bar.start_line, 5);
        assert_eq!(bar.end_line, 9);

        // `export function baz()` is unwrapped: the item is the `function_declaration`,
        // named `baz`. The line range is the declaration's (line 11), NOT the export
        // keyword on a separate line — here they coincide on line 11.
        let baz = items
            .iter()
            .find(|i| i.name.as_deref() == Some("baz"))
            .expect("baz function present");
        assert_eq!(baz.kind, "function_declaration");
        assert_eq!(baz.start_line, 11);
        assert_eq!(baz.end_line, 11);

        // `const qux = () => {}` — an arrow bound to a name is a review unit, named by
        // the binding. Its kind is the declaration node `lexical_declaration`.
        let qux = item(&items, "lexical_declaration");
        assert_eq!(qux.name.as_deref(), Some("qux"));
        // `const qux = () => {` is line 13; closing `};` is line 15.
        assert_eq!(qux.start_line, 13);
        assert_eq!(qux.end_line, 15);

        let iface = item(&items, "interface_declaration");
        assert_eq!(iface.name.as_deref(), Some("I"));
        assert_eq!(iface.start_line, 17);
        assert_eq!(iface.end_line, 19);

        let ty = item(&items, "type_alias_declaration");
        assert_eq!(ty.name.as_deref(), Some("T"));
        assert_eq!(ty.start_line, 21);
        assert_eq!(ty.end_line, 21);

        // The class method is NOT a separate top-level item: no item is named `method`.
        assert!(
            !items.iter().any(|i| i.name.as_deref() == Some("method")),
            "nested method leaked as a top-level item: {items:?}"
        );
    }

    #[test]
    fn extract_items_ts_covers_more_kinds() {
        // generator/abstract-class/enum and a non-function `const` (which is NOT an
        // item — a plain data binding is not a review unit).
        let src = "\
function* gen() { yield 1; }
abstract class Shape {}
enum Color { Red, Green }
const PI = 3.14;
var legacy = function () {};
";
        let items = extract_items(src, FileLang::Ts);
        let kinds: HashSet<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        for expected in [
            "generator_function_declaration",
            "abstract_class_declaration",
            "enum_declaration",
        ] {
            assert!(kinds.contains(expected), "missing {expected} in {kinds:?}");
        }
        assert_eq!(
            item(&items, "generator_function_declaration").name.as_deref(),
            Some("gen")
        );
        assert_eq!(
            item(&items, "abstract_class_declaration").name.as_deref(),
            Some("Shape")
        );
        assert_eq!(item(&items, "enum_declaration").name.as_deref(), Some("Color"));
        // `var legacy = function () {}` IS a function binding → an item named `legacy`.
        let legacy = item(&items, "variable_declaration");
        assert_eq!(legacy.name.as_deref(), Some("legacy"));
        // `const PI = 3.14` is NOT a function binding → NOT a review unit.
        assert!(
            !items.iter().any(|i| i.name.as_deref() == Some("PI")),
            "non-function const leaked as an item: {items:?}"
        );
    }

    #[test]
    fn extract_items_ts_empty_and_malformed_do_not_panic() {
        assert!(extract_items("", FileLang::Ts).is_empty());
        let _ = extract_items("function broken( {", FileLang::Ts);
        let _ = extract_items("}}}{{{ not ts 流", FileLang::Ts);
    }

    #[test]
    fn grounding_ts_keeps_present_symbol_drops_invented() {
        let parsed = parse_file(TS_SNIPPET, FileLang::Ts);
        // Symbol grounding is now active for TS (identifiers populated).
        assert!(!parsed.identifiers.is_empty());
        assert!(parsed.identifiers.contains("foo"));
        // `Bar` (type_identifier) and `method` (property_identifier) are collected.
        assert!(parsed.identifiers.contains("Bar"));
        assert!(parsed.identifiers.contains("method"));
        // A finding citing a present symbol → kept.
        assert_eq!(grounds(&parsed, Some(1), Some("foo")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("Bar.method")), Grounding::Kept);
        // An invented symbol with the grammar present → dropped.
        assert_eq!(
            grounds(&parsed, Some(1), Some("totallyInvented")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn grounding_ts_line_checks() {
        let parsed = parse_file(TS_SNIPPET, FileLang::Ts);
        // A line past EOF is dropped.
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        // A valid in-file line BETWEEN items (the blank line 4) with no symbol → kept
        // (in the file but outside an item is NOT a contradiction → no over-drop).
        assert_eq!(grounds(&parsed, Some(4), None), Grounding::Kept);
    }

    // =======================================================================
    // Python
    // =======================================================================

    /// A Python snippet: a top-level function, a class with a method (the method must
    /// NOT be top-level), and a DECORATED function (which must be captured, unwrapped).
    const PY_SNIPPET: &str = "\
def foo():
    return 1


class Bar:
    def method(self):
        return 2


@deco
def decorated():
    return 3
";

    #[test]
    fn extract_items_py_kinds_lines_and_names() {
        let items = extract_items(PY_SNIPPET, FileLang::Py);

        // Three top-level units: foo, Bar, decorated. The class method is NOT one.
        assert_eq!(items.len(), 3, "items: {items:?}");

        let foo = item(&items, "function_definition");
        assert_eq!(foo.name.as_deref(), Some("foo"));
        // `def foo():` is line 1; body `return 1` is line 2.
        assert_eq!(foo.start_line, 1);
        assert_eq!(foo.end_line, 2);

        let bar = item(&items, "class_definition");
        assert_eq!(bar.name.as_deref(), Some("Bar"));
        // `class Bar:` is line 5; the method body `return 2` ends on line 7.
        assert_eq!(bar.start_line, 5);
        assert_eq!(bar.end_line, 7);

        // `@deco\ndef decorated():` — the `decorated_definition` is unwrapped to the
        // inner `function_definition`, named `decorated`. The line range is the inner
        // node's: `def decorated():` is line 11 through body line 12 (the `@deco`
        // line 10 belongs to the decorator wrapper, not the unwrapped definition).
        let decorated = items
            .iter()
            .find(|i| i.name.as_deref() == Some("decorated"))
            .expect("decorated function present");
        assert_eq!(decorated.kind, "function_definition");
        assert_eq!(decorated.start_line, 11);
        assert_eq!(decorated.end_line, 12);

        // The class method is NOT a separate top-level item.
        assert!(
            !items.iter().any(|i| i.name.as_deref() == Some("method")),
            "nested method leaked as a top-level item: {items:?}"
        );
    }

    #[test]
    fn extract_items_py_empty_and_malformed_do_not_panic() {
        assert!(extract_items("", FileLang::Py).is_empty());
        let _ = extract_items("def broken(:", FileLang::Py);
        let _ = extract_items("@@@ not python 流", FileLang::Py);
    }

    #[test]
    fn grounding_py_keeps_present_symbol_drops_invented() {
        let parsed = parse_file(PY_SNIPPET, FileLang::Py);
        // Symbol grounding now active for Python.
        assert!(!parsed.identifiers.is_empty());
        assert!(parsed.identifiers.contains("foo"));
        assert!(parsed.identifiers.contains("Bar"));
        assert!(parsed.identifiers.contains("method"));
        assert_eq!(grounds(&parsed, Some(1), Some("foo")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("Bar.method")), Grounding::Kept);
        assert_eq!(
            grounds(&parsed, Some(1), Some("totally_invented")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn grounding_py_line_checks() {
        let parsed = parse_file(PY_SNIPPET, FileLang::Py);
        // A line past EOF is dropped.
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        // A valid in-file blank line BETWEEN items (line 3) with no symbol → kept.
        assert_eq!(grounds(&parsed, Some(3), None), Grounding::Kept);
    }

    // =======================================================================
    // Go
    // =======================================================================

    /// A Go snippet: a package clause, an import, a free function, a struct type, and
    /// a METHOD on that type (the method is TOP-LEVEL in Go — declared on a receiver
    /// directly under `source_file`, NOT nested inside the struct). The method USES the
    /// imported package (`fmt.Println`) so the grammar emits a `package_identifier`
    /// node for `fmt` — an import path alone is a string literal, not an identifier, so
    /// only a USE site exercises package-identifier collection. Line numbers (1-based)
    /// annotated for the assertions.
    const GO_SNIPPET: &str = "\
package main

import \"fmt\"

func F() int {
\treturn 1
}

type T struct {
\tx int
}

func (t T) M() int {
\tfmt.Println(t.x)
\treturn t.x
}
";

    #[test]
    fn extract_items_go_kinds_lines_and_names() {
        let items = extract_items(GO_SNIPPET, FileLang::Go);

        // Three top-level units: the func F, the type T, and the method M. The
        // package clause and the import are NOT review units.
        assert_eq!(items.len(), 3, "items: {items:?}");

        let f = item(&items, "function_declaration");
        assert_eq!(f.name.as_deref(), Some("F"));
        // `func F() int {` is line 5; closing `}` is line 7 (1-based inclusive).
        assert_eq!(f.start_line, 5);
        assert_eq!(f.end_line, 7);

        // `type T struct{}` — the declaration has no `name`; named by its inner
        // `type_spec`'s `name`. `type T struct {` is line 9; closing `}` is line 11.
        let t = item(&items, "type_declaration");
        assert_eq!(t.name.as_deref(), Some("T"));
        assert_eq!(t.start_line, 9);
        assert_eq!(t.end_line, 11);

        // The method M is TOP-LEVEL in Go (unlike a class method). `func (t T) M()
        // int {` is line 13; closing `}` is line 16 (the body now has two stmts).
        let m = item(&items, "method_declaration");
        assert_eq!(m.name.as_deref(), Some("M"));
        assert_eq!(m.start_line, 13);
        assert_eq!(m.end_line, 16);
    }

    #[test]
    fn extract_items_go_covers_const_var_and_interface() {
        // A const block, a var, an interface type (named via its type_spec), and a
        // type alias — exercising the remaining GO_ITEM_KINDS + the type_spec naming.
        let src = "\
package p

const K = 1

var greeting = \"hi\"

type Shape interface {
\tArea() float64
}

type Id = uint64
";
        let items = extract_items(src, FileLang::Go);
        let kinds: HashSet<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        for expected in [
            "const_declaration",
            "var_declaration",
            "type_declaration",
        ] {
            assert!(kinds.contains(expected), "missing {expected} in {kinds:?}");
        }
        // The interface type is named by its type_spec (`Shape`); the alias by its
        // (`Id`). Both are `type_declaration`s, so collect their names.
        let type_names: HashSet<Option<&str>> = items
            .iter()
            .filter(|i| i.kind == "type_declaration")
            .map(|i| i.name.as_deref())
            .collect();
        assert!(type_names.contains(&Some("Shape")), "names: {type_names:?}");
        assert!(type_names.contains(&Some("Id")), "names: {type_names:?}");
        // A single-binding `const K = 1` / `var greeting` block has no single
        // declaration-level name → None (a review unit with no display name).
        let consts: Vec<_> = items.iter().filter(|i| i.kind == "const_declaration").collect();
        assert_eq!(consts.len(), 1);
        assert_eq!(consts[0].name, None);
    }

    #[test]
    fn extract_items_go_empty_and_malformed_do_not_panic() {
        assert!(extract_items("", FileLang::Go).is_empty());
        let _ = extract_items("func broken( {", FileLang::Go);
        let _ = extract_items("}}}{{{ not go 流", FileLang::Go);
    }

    #[test]
    fn grounding_go_keeps_present_symbol_drops_invented() {
        let parsed = parse_file(GO_SNIPPET, FileLang::Go);
        // Symbol grounding is active for Go (identifiers populated).
        assert!(!parsed.identifiers.is_empty());
        // `F` (identifier), `T` (type_identifier), `M` (field_identifier as a method
        // name), `x` (field_identifier), `fmt` (package_identifier) are collected.
        assert!(parsed.identifiers.contains("F"));
        assert!(parsed.identifiers.contains("T"));
        assert!(parsed.identifiers.contains("M"));
        assert!(parsed.identifiers.contains("x"));
        assert!(
            parsed.identifiers.contains("fmt"),
            "package_identifier 'fmt' missing: {:?}",
            parsed.identifiers
        );
        // A finding citing a present symbol → kept (a qualified path with a known
        // component is kept too).
        assert_eq!(grounds(&parsed, Some(5), Some("F")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("T.M")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("fmt.Println")), Grounding::Kept);
        // A wholly invented symbol with the grammar present → dropped.
        assert_eq!(
            grounds(&parsed, Some(5), Some("totallyInvented")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn grounding_go_line_checks() {
        let parsed = parse_file(GO_SNIPPET, FileLang::Go);
        // A finding past EOF is dropped.
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        // The import line (3) is in the file but OUTSIDE any item → kept (no over-drop).
        assert_eq!(grounds(&parsed, Some(3), None), Grounding::Kept);
    }

    // =======================================================================
    // C / C++
    // =======================================================================

    /// A C/C++ snippet exercising every top-level unit kind: a defined free function
    /// (`function_definition`), a `class` with an inline method declaration (the method
    /// `m` is NESTED inside the class body — NOT top-level), a `struct`, a `namespace`,
    /// and a `template`d class (which must be UNWRAPPED to its inner `class_specifier`).
    /// Line numbers (1-based) annotated for the assertions.
    const CPP_SNIPPET: &str = "\
void f() {}

class C {
    void m();
};

struct S {};

namespace n {
    int g();
}

template <class T>
class TC {};
";

    #[test]
    fn extract_items_cpp_kinds_lines_and_names() {
        let items = extract_items(CPP_SNIPPET, FileLang::Cpp);

        // Five top-level units: the func f, the class C, the struct S, the namespace n,
        // and the templated class TC (template unwrapped). The inline method `m`, the
        // namespace member `g`, and the `template<...>` wrapper are NOT separate units.
        assert_eq!(items.len(), 5, "items: {items:?}");

        // `void f() {}` is line 1 (start == end, single line).
        let f = item(&items, "function_definition");
        assert_eq!(f.name.as_deref(), Some("f"));
        assert_eq!(f.start_line, 1);
        assert_eq!(f.end_line, 1);

        // `class C {` is line 3; closing `};` is line 5 (1-based inclusive).
        let c = item(&items, "class_specifier");
        assert_eq!(c.name.as_deref(), Some("C"));
        assert_eq!(c.start_line, 3);
        assert_eq!(c.end_line, 5);

        // `struct S {};` is line 7 (single line).
        let s = item(&items, "struct_specifier");
        assert_eq!(s.name.as_deref(), Some("S"));
        assert_eq!(s.start_line, 7);
        assert_eq!(s.end_line, 7);

        // `namespace n {` is line 9; closing `}` is line 11.
        let ns = item(&items, "namespace_definition");
        assert_eq!(ns.name.as_deref(), Some("n"));
        assert_eq!(ns.start_line, 9);
        assert_eq!(ns.end_line, 11);

        // The templated class is UNWRAPPED: it reports as a `class_specifier` named
        // `TC` on the inner-class line (14), NOT a `template_declaration`. There are
        // two `class_specifier`s now (C and TC), so locate TC by name.
        let tc = items
            .iter()
            .find(|i| i.kind == "class_specifier" && i.name.as_deref() == Some("TC"))
            .unwrap_or_else(|| panic!("expected TC class_specifier in {items:?}"));
        assert_eq!(tc.start_line, 14);
        assert_eq!(tc.end_line, 14);
        // No `template_declaration` survives as a unit (it was unwrapped).
        assert!(
            !items.iter().any(|i| i.kind == "template_declaration"),
            "template should be unwrapped: {items:?}"
        );
    }

    #[test]
    fn extract_items_cpp_function_prototype_yes_variable_no() {
        // A top-level function PROTOTYPE is a review unit; a plain top-level VARIABLE
        // declaration is NOT (same `declaration` node kind, distinguished by the
        // presence of a function_declarator).
        let src = "\
int proto(int a);
int globalVar = 3;
enum Color { Red, Green };
union U { int i; float f; };
";
        let items = extract_items(src, FileLang::Cpp);
        let kinds: HashSet<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        // The prototype (a `declaration` with a function_declarator) is kept and named.
        let proto = item(&items, "declaration");
        assert_eq!(proto.name.as_deref(), Some("proto"));
        assert_eq!(proto.start_line, 1);
        // The enum and union are units; the plain variable declaration is NOT.
        assert!(kinds.contains("enum_specifier"), "kinds: {kinds:?}");
        assert!(kinds.contains("union_specifier"), "kinds: {kinds:?}");
        let enum_item = item(&items, "enum_specifier");
        assert_eq!(enum_item.name.as_deref(), Some("Color"));
        let union_item = item(&items, "union_specifier");
        assert_eq!(union_item.name.as_deref(), Some("U"));
        // Exactly one `declaration` survives (the prototype) — `globalVar` is dropped.
        let decls: Vec<_> = items.iter().filter(|i| i.kind == "declaration").collect();
        assert_eq!(decls.len(), 1, "only the prototype is a unit: {items:?}");
    }

    #[test]
    fn extract_items_cpp_empty_and_malformed_do_not_panic() {
        assert!(extract_items("", FileLang::Cpp).is_empty());
        let _ = extract_items("void broken( {", FileLang::Cpp);
        let _ = extract_items("}}}{{{ not c++ 流", FileLang::Cpp);
        // A C file (the .c/.h family shares FileLang::Cpp) parses with the same grammar.
        let _ = extract_items("int main(void) { return 0; }", FileLang::Cpp);
    }

    #[test]
    fn grounding_cpp_keeps_present_symbol_drops_invented() {
        let parsed = parse_file(CPP_SNIPPET, FileLang::Cpp);
        // Symbol grounding is active for C/C++ (identifiers populated).
        assert!(!parsed.identifiers.is_empty());
        // `f` (identifier), `C`/`S`/`TC` (type_identifier), `m` (field_identifier as a
        // method name), `n` (namespace_identifier) are collected.
        assert!(parsed.identifiers.contains("f"));
        assert!(parsed.identifiers.contains("C"));
        assert!(parsed.identifiers.contains("S"));
        assert!(parsed.identifiers.contains("TC"));
        assert!(parsed.identifiers.contains("m"));
        assert!(
            parsed.identifiers.contains("n"),
            "namespace_identifier 'n' missing: {:?}",
            parsed.identifiers
        );
        // A finding citing a present symbol → kept (a qualified path with a known
        // component is kept too).
        assert_eq!(grounds(&parsed, Some(1), Some("f")), Grounding::Kept);
        assert_eq!(grounds(&parsed, None, Some("C::m")), Grounding::Kept);
        // A wholly invented symbol with the grammar present → dropped.
        assert_eq!(
            grounds(&parsed, Some(1), Some("totallyInvented")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn grounding_cpp_line_checks() {
        let parsed = parse_file(CPP_SNIPPET, FileLang::Cpp);
        // A finding past EOF is dropped.
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        // The blank line (2) is in the file but OUTSIDE any item → kept (no over-drop).
        assert_eq!(grounds(&parsed, Some(2), None), Grounding::Kept);
    }

    // =======================================================================
    // HTML
    // =======================================================================

    /// An HTML snippet: a `<div>` with `id` + `class`, a nested `<span>` with a `name`
    /// attribute, and a multi-class element. The identifier set must contain the tag
    /// names (`div`, `span`, `p`) and the attribute VALUES a finding would cite
    /// (`main`, the class tokens `x`/`y`/`z`, the `name` value `field1`). Line numbers
    /// (1-based) annotated for the assertions.
    const HTML_SNIPPET: &str = "\
<div id=\"main\" class=\"x y\">
  <span name=\"field1\">hi</span>
</div>
<p class=\"z\">bye</p>
";

    #[test]
    fn extract_items_html_top_level_elements_and_tag_names() {
        let items = extract_items(HTML_SNIPPET, FileLang::Html);
        // Two TOP-LEVEL elements: the `div` (lines 1-3) and the `p` (line 4). The nested
        // `<span>` is NOT a top-level unit. HTML's review unit is weak/best-effort.
        assert_eq!(items.len(), 2, "items: {items:?}");
        let div = items
            .iter()
            .find(|i| i.name.as_deref() == Some("div"))
            .unwrap_or_else(|| panic!("expected a div element in {items:?}"));
        assert_eq!(div.kind, "element");
        assert_eq!(div.start_line, 1);
        assert_eq!(div.end_line, 3);
        let p = items
            .iter()
            .find(|i| i.name.as_deref() == Some("p"))
            .unwrap_or_else(|| panic!("expected a p element in {items:?}"));
        assert_eq!(p.start_line, 4);
        assert_eq!(p.end_line, 4);
    }

    #[test]
    fn extract_html_identifiers_collects_tags_ids_classes_names() {
        // The spec's grounding contract: `<div id="main" class="x">` → `div`, `main`,
        // `x` collected (here also `span`/`p` tags, `y`/`z` classes, `field1` name).
        let parsed = parse_file(HTML_SNIPPET, FileLang::Html);
        assert!(!parsed.identifiers.is_empty());
        // Tag names.
        for tag in ["div", "span", "p"] {
            assert!(
                parsed.identifiers.contains(tag),
                "tag '{tag}' missing: {:?}",
                parsed.identifiers
            );
        }
        // `id` value.
        assert!(parsed.identifiers.contains("main"), "id value missing");
        // `class` values — each space-separated token is its own identifier.
        for class in ["x", "y", "z"] {
            assert!(
                parsed.identifiers.contains(class),
                "class '{class}' missing: {:?}",
                parsed.identifiers
            );
        }
        // `name` value.
        assert!(parsed.identifiers.contains("field1"), "name value missing");
    }

    #[test]
    fn grounding_html_keeps_present_drops_invented_and_line_checks() {
        let parsed = parse_file(HTML_SNIPPET, FileLang::Html);
        // A finding citing a present tag/id/class → kept.
        assert_eq!(grounds(&parsed, Some(1), Some("main")), Grounding::Kept);
        assert_eq!(grounds(&parsed, Some(1), Some("div")), Grounding::Kept);
        // A wholly invented symbol with the grammar present → dropped.
        assert_eq!(
            grounds(&parsed, Some(1), Some("totallyInvented")),
            Grounding::DroppedUnknownSymbol
        );
        // Line-range grounding works for HTML: past EOF → dropped; in-file → kept.
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        assert_eq!(grounds(&parsed, Some(2), None), Grounding::Kept);
    }

    #[test]
    fn extract_html_collects_reference_attr_targets_and_grounds_them() {
        // A broken `<label for="email">` and a dangling `<a href="#missing">`: the cited
        // TARGETS (`email`, `missing`) are not also `id`s in this snippet, so without
        // collecting `for`/`href` values a finding citing them would be FALSE-DROPPED.
        let src = "\
<label for=\"email\">Email</label>
<a href=\"#missing\">link</a>
<form action=\"/api/login\"></form>
<img src=\"logo.png\">
";
        let parsed = parse_file(src, FileLang::Html);
        // The reference targets land in the identifier set (tokenized: `#missing` →
        // `missing`, `/api/login` → `api`/`login`, `logo.png` → `logo`/`png`).
        for ident in ["email", "missing", "api", "login", "logo"] {
            assert!(
                parsed.identifiers.contains(ident),
                "reference target '{ident}' missing: {:?}",
                parsed.identifiers
            );
        }
        // A finding citing a `for`/`href` target is KEPT (no longer a false drop).
        assert_eq!(grounds(&parsed, Some(1), Some("email")), Grounding::Kept);
        assert_eq!(grounds(&parsed, Some(2), Some("missing")), Grounding::Kept);
        // A wholly invented target with the grammar present is still dropped.
        assert_eq!(
            grounds(&parsed, Some(1), Some("nonexistentTarget")),
            Grounding::DroppedUnknownSymbol
        );
    }

    #[test]
    fn extract_items_html_empty_and_malformed_do_not_panic() {
        assert!(extract_items("", FileLang::Html).is_empty());
        let _ = extract_items("<div><span>", FileLang::Html);
        let _ = extract_items("}}}<<< not html 流", FileLang::Html);
    }

    // =======================================================================
    // Kotlin
    // =======================================================================

    /// A Kotlin snippet: a top-level `fun`, a `class`, an `object`, and a top-level
    /// `val` property — one of each [`KOTLIN_ITEM_KINDS`]. Line numbers (1-based)
    /// annotated for the assertions.
    const KOTLIN_SNIPPET: &str = "\
fun f(): Int {
    return 1
}

class C {
    fun m(): Int = 2
}

object O {
    val k = 3
}

val greeting = \"hi\"
";

    #[test]
    fn extract_items_kotlin_top_level_kinds_and_names() {
        let items = extract_items(KOTLIN_SNIPPET, FileLang::Kotlin);
        // Four top-level units: fun f, class C, object O, val greeting. The nested
        // method `m` and the nested `val k` are NOT top-level.
        assert_eq!(items.len(), 4, "items: {items:?}");

        let kinds: HashSet<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        for expected in [
            "function_declaration",
            "class_declaration",
            "object_declaration",
            "property_declaration",
        ] {
            assert!(kinds.contains(expected), "missing {expected} in {kinds:?}");
        }

        // Names: `f` (fun), `C` (class), `O` (object), `greeting` (val).
        let f = item(&items, "function_declaration");
        assert_eq!(f.name.as_deref(), Some("f"));
        assert_eq!(f.start_line, 1);
        let c = item(&items, "class_declaration");
        assert_eq!(c.name.as_deref(), Some("C"));
        let o = item(&items, "object_declaration");
        assert_eq!(o.name.as_deref(), Some("O"));
        let v = item(&items, "property_declaration");
        assert_eq!(v.name.as_deref(), Some("greeting"));
    }

    #[test]
    fn grounding_kotlin_keeps_present_drops_invented_and_line_checks() {
        let parsed = parse_file(KOTLIN_SNIPPET, FileLang::Kotlin);
        // Symbol grounding is active for Kotlin (identifiers populated).
        assert!(!parsed.identifiers.is_empty());
        for name in ["f", "C", "O", "greeting"] {
            assert!(
                parsed.identifiers.contains(name),
                "identifier '{name}' missing: {:?}",
                parsed.identifiers
            );
        }
        // A finding citing a present symbol → kept.
        assert_eq!(grounds(&parsed, Some(1), Some("f")), Grounding::Kept);
        // A wholly invented symbol with the grammar present → dropped.
        assert_eq!(
            grounds(&parsed, Some(1), Some("totallyInvented")),
            Grounding::DroppedUnknownSymbol
        );
        // Line-range grounding works for Kotlin.
        assert_eq!(
            grounds(&parsed, Some(parsed.total_lines + 1), None),
            Grounding::DroppedLineOutOfFile
        );
        assert_eq!(grounds(&parsed, Some(4), None), Grounding::Kept);
    }

    #[test]
    fn extract_items_kotlin_empty_and_malformed_do_not_panic() {
        assert!(extract_items("", FileLang::Kotlin).is_empty());
        let _ = extract_items("fun broken( {", FileLang::Kotlin);
        let _ = extract_items("}}}{{{ not kotlin 流", FileLang::Kotlin);
    }
}
