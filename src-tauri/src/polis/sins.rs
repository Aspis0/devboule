//! Polis Map — the Augure's URBAN SIN detectors (pure Rust, no Oracle).
//!
//! Implements the auto-detectable sins from the doc's table:
//!   - Hardcoded secret-like value                -> `inferno`
//!   - Cyclic import (cycle in the road graph)    -> `fire`
//!   - > 3 TODO/FIXME/HACK comments               -> `smoke`
//!   - Exported-but-never-imported symbol         -> `smoke`
//!   - env var used in code but absent from `.env.example` -> `fire`
//!
//! CRITICAL REDACTION GUARANTEE: when a secret is detected, the `UrbanSin`
//! description MUST NOT contain the secret value. We report only
//! `"Hardcoded secret-like value at line N"`. The matched bytes are never
//! copied into any output string. See `detect_secrets` + its tests.
//!
//! DEFERRED (require git / Scaleway / external state — left as seams):
//!   - Secret in committed `.env`            (needs git log)
//!   - Stale file / old pinned dependency    (needs git log / dates)
//!   - Scaleway endpoint without IAM header  (provider-specific)
//!   - IAM key expiry within 30 days         (Scaleway API)

use crate::polis::augure::DetectedSin;
use crate::polis::model::{purpose, severity, Building, Road, UrbanSin};
use crate::polis::scanner::{RoadGraph, ScannedFile};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

// =========================================================================
// P4.1 — Threshold policy constants (all pure, doc-commented)
// =========================================================================

/// Cyclomatic complexity at or above which a per-item sin is emitted: >= 15 → smoke.
const COMPLEXITY_SMOKE: u32 = 15;
/// CC >= 25 → fire.
const COMPLEXITY_FIRE: u32 = 25;
/// CC >= 40 → inferno.
const COMPLEXITY_INFERNO: u32 = 40;
/// Max per-file complexity sins emitted (the 3 worst items).  Prevents sin spam
/// on a file full of switch/match arms.
const COMPLEXITY_MAX_PER_FILE: usize = 3;

/// A file is a "god-file" when BOTH its LOC and its fan-in exceed the project
/// P95 of the respective metric.  Severity fire by default.
/// When EITHER exceeds P99 → inferno.
/// Skipped entirely when the project has fewer than this many files in the
/// import graph (percentiles are meaningless on tiny sets).
const GOD_FILE_MIN_FILES: usize = 20;

/// A src file (not a test) triggers the test-gap sin when it has at least this
/// many exported symbols with ZERO membership in `test_refs` AND the project
/// has at least one test file.
const TEST_GAP_MIN_UNREF_SYMBOLS: usize = 3;

/// Clone pair with >= this many tokens raises a fire sin (otherwise smoke).
const CLONE_FIRE_TOKENS: u32 = 100;

/// Result of a full sin sweep.
pub struct SinReport {
    /// file_id -> sins attributable to that building.
    pub by_file: HashMap<String, Vec<DetectedSin>>,
    /// Sins not tied to a single building (currently unused; cycles are
    /// attributed per-file so the offending buildings burn).
    pub city_wide: Vec<DetectedSin>,
}

/// Detect the CONTENT-based sins for a single file while its body is still in
/// memory (called from the scanner walk, which then drops the body). These sins
/// are emitted with `file_id: None`; the stable file_id is stamped in later by
/// `detect_graph_sins` once the building shells exist. Keeping detection here is
/// what lets the scanner avoid retaining every file's full content.
///
/// `env_example` is the (optional) set of keys declared in `.env.example`.
pub fn detect_content_sins(content: &str, env_example: Option<&HashSet<String>>) -> Vec<DetectedSin> {
    let mut sins = Vec::new();
    // 1) Hardcoded secret -> inferno (REDACTED — never includes the value).
    sins.extend(detect_secrets(content, ""));
    // 2) TODO/FIXME/HACK > 3 -> smoke.
    if let Some(sin) = detect_todo_smoke(content, "") {
        sins.push(sin);
    }
    // 3) env var used but absent from .env.example -> fire.
    if let Some(allowed) = env_example {
        let code_only = crate::polis::scanner::strip_comments(content);
        sins.extend(detect_missing_env(&code_only, allowed, ""));
    }
    // Detected before the file_id is known; stamp later.
    for s in &mut sins {
        s.sin.file_id = None;
    }
    sins
}

/// Add the GRAPH-derived sins (cycles, orphan-export) and merge in the
/// already-detected per-file content sins. Keys everything by file_id and fills
/// in each sin's `file_id`. Pure: no file IO, no content needed.

/// Build Tarjan SCCs from the combined road edge set (ast + regex).
/// Thin adapter: maps Road edges to graph::ImportEdge and delegates to the
/// production iterative implementation in `crate::backend::graph::tarjan_scc`.
fn tarjan_scc_from_roads(roads: &[Road]) -> Vec<Vec<String>> {
    use crate::backend::graph;
    // Filter to IMPORT roads only — clone/semantic/infrastructure roads are
    // visual-only and must not fabricate dep-cycle SCCs (B1 fix).
    let edges: Vec<graph::ImportEdge> = roads
        .iter()
        .filter(|r| r.road_type == crate::polis::model::road_type::IMPORT)
        .map(|r| graph::ImportEdge {
            from: r.from.clone(),
            to: r.to.clone(),
            weight: 1,
        })
        .collect();
    graph::tarjan_scc(&edges)
}

pub fn detect_graph_sins(
    scanned: &[ScannedFile],
    buildings: &[Building],
    graph: &RoadGraph,
    roads: &[Road],
    import_graph: Option<&crate::backend::graph::ImportGraph>,
) -> SinReport {
    let mut by_file: HashMap<String, Vec<DetectedSin>> = HashMap::new();
    let city_wide: Vec<DetectedSin> = Vec::new();

    // file_path -> file_id, and file_path -> purpose (buildings carry both).
    let id_by_path: HashMap<&str, &str> = buildings
        .iter()
        .map(|b| (b.file_path.as_str(), b.file_id.as_str()))
        .collect();
    let purpose_by_id: HashMap<&str, &str> = buildings
        .iter()
        .map(|b| (b.file_id.as_str(), b.purpose.as_str()))
        .collect();

    // Merge content sins (detected during the scan) keyed by file_id, stamping
    // the now-known file_id and a stable sin_id onto each.
    for f in scanned {
        let Some(&file_id) = id_by_path.get(f.rel_path.as_str()) else {
            continue;
        };
        if f.content_sins.is_empty() {
            continue;
        }
        let entry = by_file.entry(file_id.to_string()).or_default();
        for ds in &f.content_sins {
            let mut sin = ds.clone();
            sin.sin.file_id = Some(file_id.to_string());
            sin.sin.sin_id = stamp_sin_id(&sin.sin.sin_id, file_id);
            entry.push(sin);
        }
    }

    // P2.2 — Cyclic import via Tarjan SCC over the COMBINED road edge set
    // (ast + regex fallback edges).  Each SCC of size >= 2 produces ONE
    // DetectedSin per member file.  Evidence string lists cycle MEMBER PATHS
    // (not file_ids) in sorted order.  Severity: 2-member SCC → fire, >=3 → inferno.
    let sccs = tarjan_scc_from_roads(roads);

    // Build file_id -> file_path map from buildings for evidence paths.
    let id_to_path: HashMap<&str, &str> = buildings
        .iter()
        .map(|b| (b.file_id.as_str(), b.file_path.as_str()))
        .collect();

    for scc in &sccs {
        if scc.len() < 2 {
            continue;
        }
        let severity = if scc.len() >= 3 {
            severity::INFERNO.to_string()
        } else {
            severity::FIRE.to_string()
        };
        // Translate file_ids to paths, sort by path for readable evidence.
        let mut members: Vec<&str> = scc
            .iter()
            .filter_map(|id| id_to_path.get(id.as_str()).copied())
            .collect();
        members.sort();
        let evidence = if members.len() <= 5 {
            format!("cycle: {}", members.join(" -> "))
        } else {
            format!("cycle: {} -> ...", members[..5].join(" -> "))
        };
        // Emit sin per file_id (not path) — the sin attaches to the building.
        for file_id in scc {
            by_file.entry(file_id.clone()).or_default().push(DetectedSin {
                sin: UrbanSin {
                    sin_id: deterministic_sin_id("cycle", file_id),
                    severity: severity.clone(),
                    description: "Cyclic import detected in the road graph".to_string(),
                    auto_detectable: true,
                    file_id: Some(file_id.clone()),
                },
                rule_id: "dep-cycle",
                evidence: evidence.clone(),
                line: None,
            });
        }
    }

    // =====================================================================
    // P4.1 — dead-export UPGRADE (AST fan-in beats name-heuristic)
    // =====================================================================
    //
    // The existing dead-export check fires when a file has exported symbols
    // but zero incoming roads.  With AST data available, we UPGRADE it:
    //   - If the file IS in the import graph AND has fan-in > 0, the AST
    //     evidence beats the regex heuristic → SUPPRESS the sin.
    //   - If fan-in == 0 (or not in the graph), keep the original sin.
    //
    // Fan-in per file = count of import edges whose `to` == file's rel_path.
    // The import graph's edges use rel_paths, not file_ids.
    // B1 fix: compute import targets from IMPORT-only roads (clone/semantic/
    // infrastructure roads are visual-only and must not suppress dead-export).
    let imported_targets: HashSet<&str> = roads
        .iter()
        .filter(|r| r.road_type == crate::polis::model::road_type::IMPORT)
        .map(|r| r.to.as_str())
        .collect();

    // Build a fan-in map from AST edges: rel_path -> count of distinct importers.
    let ast_fan_in: HashMap<&str, usize> = if let Some(ig) = import_graph {
        let mut m: HashMap<&str, usize> = HashMap::new();
        for e in &ig.edges {
            *m.entry(e.to.as_str()).or_insert(0) += 1;
        }
        m
    } else {
        HashMap::new()
    };
    // Quick lookup: is rel_path an AST-covered file?
    let ast_files: HashSet<&str> = import_graph
        .map(|ig| ig.files.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    for f in scanned {
        let Some(&file_id) = id_by_path.get(f.rel_path.as_str()) else {
            continue;
        };
        let is_entry_point = purpose_by_id.get(file_id).copied() == Some(purpose::LIGHTHOUSE);
        if is_entry_point {
            continue;
        }
        if !f.has_exported_symbol {
            continue;
        }
        // Original heuristic: no incoming regex roads → dead export.
        let no_regex_imports = !imported_targets.contains(file_id);

        // UPGRADE: if the file is AST-covered, fan-in > 0 means the AST saw
        // real importers → the regex heuristic's silence is a false negative
        // (the import uses a path alias / re-export / etc.).  SUPPRESS the sin.
        // If fan-in == 0, the AST confirms the heuristic → KEEP the sin.
        let ast_fi = ast_files
            .contains(f.rel_path.as_str())
            .then(|| ast_fan_in.get(f.rel_path.as_str()).copied().unwrap_or(0));

        let should_fire = match ast_fi {
            Some(fi) if fi > 0 => {
                // AST says this file HAS importers → suppress the name-heuristic sin.
                false
            }
            Some(_fi) => {
                // AST confirms zero fan-in → keep the sin (stronger evidence).
                true
            }
            None => {
                // Not AST-covered → original heuristic alone.
                no_regex_imports
            }
        };

        if should_fire {
            by_file
                .entry(file_id.to_string())
                .or_default()
                .push(DetectedSin {
                    sin: UrbanSin {
                        sin_id: deterministic_sin_id("orphan-export", file_id),
                        severity: severity::SMOKE.to_string(),
                        description: "Exported symbol with no incoming imports detected".to_string(),
                        auto_detectable: true,
                        file_id: Some(file_id.to_string()),
                    },
                    rule_id: "dead-export",
                    evidence: "no incoming imports detected".to_string(),
                    line: None,
                });
        }
    }

    // =====================================================================
    // P4.1 — COMPLEXITY (per-item cyclomatic complexity)
    // =====================================================================
    if let Some(ig) = import_graph {
        // Map rel_path -> file_id for attribution.
        let fid_by_path: HashMap<&str, &str> = buildings
            .iter()
            .map(|b| (b.file_path.as_str(), b.file_id.as_str()))
            .collect();

        for m in &ig.metrics {
            let Some(&file_id) = fid_by_path.get(m.rel_path.as_str()) else {
                continue;
            };
            // Collect all offending items: (complexity, severity, item).
            let mut offenders: Vec<(u32, &str, &crate::backend::graph::ItemMetric)> = Vec::new();
            for it in &m.items {
                // Container aggregates (impl/class) never trip — their per-method
                // breakdown is expanded into separate function entries by
                // graph.rs's FileMetrics construction; a stray container entry
                // (older cache, hand-built fixture) must not sin on the blended
                // number the design doc forbids.
                if matches!(
                    it.kind.as_str(),
                    "impl_item" | "class_declaration" | "abstract_class_declaration"
                ) {
                    continue;
                }
                if it.complexity < COMPLEXITY_SMOKE {
                    continue;
                }
                let sev = if it.complexity >= COMPLEXITY_INFERNO {
                    severity::INFERNO
                } else if it.complexity >= COMPLEXITY_FIRE {
                    severity::FIRE
                } else {
                    severity::SMOKE
                };
                offenders.push((it.complexity, sev, it));
            }
            if offenders.is_empty() {
                continue;
            }
            // Take the 3 worst (highest cc), then sort by severity desc (inferno
            // first) so the worst items appear first in the sin list.
            offenders.sort_by(|a, b| b.0.cmp(&a.0));
            offenders.truncate(COMPLEXITY_MAX_PER_FILE);
            // Optional: resort by severity so inferno/fire/smoke order is nicer.
            offenders.sort_by(|a, b| {
                severity_rank(b.1).cmp(&severity_rank(a.1))
                    .then_with(|| a.2.line.cmp(&b.2.line))
            });

            for (cc, sev, it) in &offenders {
                let fn_label = it.name.as_deref().unwrap_or("<unnamed>");
                let threshold = if *sev == severity::INFERNO {
                    COMPLEXITY_INFERNO
                } else if *sev == severity::FIRE {
                    COMPLEXITY_FIRE
                } else {
                    COMPLEXITY_SMOKE
                };
                let evidence = format!(
                    "fn {fn_label} exceeds the cyclomatic threshold"
                );
                by_file.entry(file_id.to_string()).or_default().push(DetectedSin {
                    sin: UrbanSin {
                        sin_id: deterministic_sin_id(
                            &format!("complexity-{}-{}", it.line, cc),
                            file_id,
                        ),
                        severity: sev.to_string(),
                        description: format!(
                            "High cyclomatic complexity in {fn_label} (cc {cc}, threshold {threshold})"
                        ),
                        auto_detectable: true,
                        file_id: Some(file_id.to_string()),
                    },
                    rule_id: "complexity",
                    evidence,
                    line: Some(it.line),
                });
            }
        }
    }

    // =====================================================================
    // P4.1 — GOD-FILE (above P95 of both LOC and fan-in)
    // =====================================================================
    if let Some(ig) = import_graph {
        let n = ig.metrics.len();
        if n >= GOD_FILE_MIN_FILES {
            // Compute P95 and P99 of LOC.
            let mut locs: Vec<u32> = ig.metrics.iter().map(|m| m.loc).collect();
            locs.sort();
            let p95_loc = percentile(&locs, 0.95);
            let p99_loc = percentile(&locs, 0.99);

            // Compute P95 and P99 of fan-in.
            let mut fan_ins: Vec<usize> = ig.metrics.iter().map(|m| {
                ast_fan_in.get(m.rel_path.as_str()).copied().unwrap_or(0)
            }).collect();
            fan_ins.sort();
            let p95_fan = percentile_usize(&fan_ins, 0.95);
            let p99_fan = percentile_usize(&fan_ins, 0.99);

            let fid_by_path: HashMap<&str, &str> = buildings
                .iter()
                .map(|b| (b.file_path.as_str(), b.file_id.as_str()))
                .collect();

            for m in &ig.metrics {
                let Some(&file_id) = fid_by_path.get(m.rel_path.as_str()) else {
                    continue;
                };
                let fi = ast_fan_in.get(m.rel_path.as_str()).copied().unwrap_or(0);
                let loc = m.loc;

                // Must exceed BOTH P95 thresholds.
                if loc as usize <= p95_loc || fi <= p95_fan {
                    continue;
                }
                let severity = if (loc as usize) > p99_loc || fi > p99_fan {
                    severity::INFERNO
                } else {
                    severity::FIRE
                };
                let evidence = "file size and fan-in both above project P95".to_string();
                by_file.entry(file_id.to_string()).or_default().push(DetectedSin {
                    sin: UrbanSin {
                        sin_id: deterministic_sin_id("god-file", file_id),
                        severity: severity.to_string(),
                        description: format!(
                            "God file: {loc} LOC (P95 {p95_loc}) with {fi} importers (P95 {p95_fan}) — above P95 on both axes"
                        ),
                        auto_detectable: true,
                        file_id: Some(file_id.to_string()),
                    },
                    rule_id: "god-file",
                    evidence,
                    line: None,
                });
            }
        }
    }

    // =====================================================================
    // P4.1 — TEST-GAP (exported symbols unreferenced by any test)
    // =====================================================================
    if let Some(ig) = import_graph {
        // Only fire when the project has at least one test file.
        let has_test_file = ig.metrics.iter().any(|m| {
            crate::backend::graph::is_test_path(&m.rel_path)
        });
        if has_test_file {
            let fid_by_path: HashMap<&str, &str> = buildings
                .iter()
                .map(|b| (b.file_path.as_str(), b.file_id.as_str()))
                .collect();

            for m in &ig.metrics {
                // Only src files (not tests themselves).
                if crate::backend::graph::is_test_path(&m.rel_path) {
                    continue;
                }
                let Some(&file_id) = fid_by_path.get(m.rel_path.as_str()) else {
                    continue;
                };
                // Exported symbols with zero membership in test_refs.
                let unreferenced: Vec<&String> = m.exported.iter()
                    .filter(|sym| !ig.test_refs.contains(sym.as_str()))
                    .collect();
                if unreferenced.len() < TEST_GAP_MIN_UNREF_SYMBOLS {
                    continue;
                }
                let first_few: Vec<&str> = unreferenced.iter()
                    .take(3)
                    .map(|s| s.as_str())
                    .collect();
                let evidence = "exported symbols unreferenced by any test".to_string();
                by_file.entry(file_id.to_string()).or_default().push(DetectedSin {
                    sin: UrbanSin {
                        sin_id: deterministic_sin_id("test-gap", file_id),
                        severity: severity::SMOKE.to_string(),
                        description: format!(
                            "{} exported symbols not covered by any test (e.g. {})",
                            unreferenced.len(),
                            first_few.join(", ")
                        ),
                        auto_detectable: true,
                        file_id: Some(file_id.to_string()),
                    },
                    rule_id: "test-gap",
                    evidence,
                    line: None,
                });
            }
        }
    }

    // =====================================================================
    // P4.2 — CLONE sin (per file in each detected clone pair)
    // =====================================================================
    if let Some(ig) = import_graph {
        let fid_by_path: HashMap<&str, &str> = buildings
            .iter()
            .map(|b| (b.file_path.as_str(), b.file_id.as_str()))
            .collect();

        for cp in &ig.clones {
            // File A sin.
            if let Some(&fid_a) = fid_by_path.get(cp.a.as_str()) {
                let sev = if cp.tokens >= CLONE_FIRE_TOKENS {
                    severity::FIRE
                } else {
                    severity::SMOKE
                };
                let evidence = format!(
                    "duplicated block shared with {other}",
                    other = cp.b,
                );
                by_file.entry(fid_a.to_string()).or_default().push(DetectedSin {
                    sin: UrbanSin {
                        sin_id: deterministic_sin_id(
                            &format!("clone-{}", &cp.b.replace('/', "-")),
                            fid_a,
                        ),
                        severity: sev.to_string(),
                        description: format!(
                            "~{tokens} tokens duplicated with {} (from line {line})",
                            cp.b,
                            tokens = cp.tokens,
                            line = cp.a_line,
                        ),
                        auto_detectable: true,
                        file_id: Some(fid_a.to_string()),
                    },
                    rule_id: "clone",
                    evidence,
                    line: Some(cp.a_line),
                });
            }

            // File B sin.
            if let Some(&fid_b) = fid_by_path.get(cp.b.as_str()) {
                let sev = if cp.tokens >= CLONE_FIRE_TOKENS {
                    severity::FIRE
                } else {
                    severity::SMOKE
                };
                let evidence = format!(
                    "duplicated block shared with {other}",
                    other = cp.a,
                );
                by_file.entry(fid_b.to_string()).or_default().push(DetectedSin {
                    sin: UrbanSin {
                        sin_id: deterministic_sin_id(
                            &format!("clone-{}", &cp.a.replace("/", "-")),
                            fid_b,
                        ),
                        severity: sev.to_string(),
                        description: format!(
                            "~{tokens} tokens duplicated with {} (from line {line})",
                            cp.a,
                            tokens = cp.tokens,
                            line = cp.b_line,
                        ),
                        auto_detectable: true,
                        file_id: Some(fid_b.to_string()),
                    },
                    rule_id: "clone",
                    evidence,
                    line: Some(cp.b_line),
                });
            }
        }
    }

    SinReport { by_file, city_wide }
}

/// Numeric rank of a severity string for sorting (inferno > fire > smoke).
fn severity_rank(s: &str) -> u8 {
    match s {
        "inferno" => 3,
        "fire" => 2,
        "smoke" => 1,
        _ => 0,
    }
}

/// The value at the given percentile (0.0–1.0) of a sorted slice.
/// Returns the floor: index = ceil(p * len) - 1, clamped.
fn percentile(sorted: &[u32], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (p * sorted.len() as f64).ceil() as usize;
    let idx = idx.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx] as usize
}

fn percentile_usize(sorted: &[usize], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (p * sorted.len() as f64).ceil() as usize;
    let idx = idx.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

// ---------------------------------------------------------------------------
// Secret detection (REDACTED output)
// ---------------------------------------------------------------------------

/// Detect hardcoded secret-like values. Returns one `inferno` sin per offending
/// LINE. The description ONLY references the line number — never the matched
/// value, so the secret is not leaked into the CityState / sidebar.
pub fn detect_secrets(content: &str, file_id: &str) -> Vec<DetectedSin> {
    let mut out = Vec::new();
    let mut flagged_lines: HashSet<usize> = HashSet::new();

    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if flagged_lines.contains(&line_no) {
            continue;
        }
        if line_looks_secret(line) {
            flagged_lines.insert(line_no);
            out.push(DetectedSin {
                sin: UrbanSin {
                    sin_id: deterministic_sin_id(&format!("secret-{line_no}"), file_id),
                    severity: severity::INFERNO.to_string(),
                    description: format!("Hardcoded secret-like value at line {line_no}"),
                    auto_detectable: true,
                    file_id: Some(file_id.to_string()),
                },
                rule_id: "secret",
                evidence: format!("secret at line {line_no}"),
                line: Some(line_no as u32),
            });
        }
    }
    out
}

/// Placeholder substituted for any secret-like token we strip from free-form text.
pub const SECRET_REDACTION_PLACEHOLDER: &str = "[REDACTED]";

/// FIX 4 (privacy, defense-in-depth) — scrub secret-like tokens out of FREE-FORM
/// text (e.g. an Oracle dossier/blurb that could ECHO a secret from the indexed
/// code) BEFORE it is persisted to `.aspis-meta.json` or returned to the UI.
///
/// Reuses the SAME detection the scanner's `detect_secrets` uses
/// (`line_looks_secret` per line), but instead of only flagging the line it replaces
/// the matched secret SPAN with `SECRET_REDACTION_PLACEHOLDER`, preserving the rest
/// of the prose. Operates line by line (the detection is line-scoped); a line with
/// no secret is returned byte-for-byte. The original line terminators are preserved
/// faithfully (trailing newline kept; `\r\n` handled by `split_inclusive`).
///
/// Conservative by construction: it can only remove a span the existing secret
/// heuristic already recognizes as a hardcoded secret, so it never mangles ordinary
/// prose. A token the heuristic misses is not redacted here — the scanner's own
/// `inferno` sin would still flag the source file, but for the dossier text we strip
/// every pattern the heuristic knows. Never returns the secret value.
pub fn redact_secret_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for chunk in text.split_inclusive('\n') {
        // Separate the line body from its terminator so detection runs on the body.
        let (body, terminator) = match chunk.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (chunk, ""),
        };
        let (line, cr) = match body.strip_suffix('\r') {
            Some(b) => (b, "\r"),
            None => (body, ""),
        };
        if line_looks_secret(line) {
            out.push_str(&redact_secret_spans_in_line(line));
        } else {
            out.push_str(line);
        }
        out.push_str(cr);
        out.push_str(terminator);
    }
    out
}

/// Replace each secret SPAN in one line with the redaction placeholder, mirroring
/// the four branches of `line_looks_secret`. Only invoked on lines already known to
/// contain a secret. Redacts from the marker through the end of the opaque token
/// run, leaving the surrounding text (and the secret-y NAME, which is not itself a
/// secret) intact.
fn redact_secret_spans_in_line(line: &str) -> String {
    let mut result = line.to_string();

    // Prefix-marker tokens: redact the marker + its trailing token run as one span.
    // Loop until no more occurrences (a line could hold several).
    for (marker, min_run) in [("sk-", 16usize), ("AKIA", 12), ("Bearer ", 12)] {
        loop {
            let Some(pos) = result.find(marker) else {
                break;
            };
            let after = &result[pos + marker.len()..];
            let run = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .count();
            if run < min_run {
                // Not a secret occurrence of this marker; stop scanning this marker
                // (avoid an infinite loop — there is no qualifying span to remove).
                break;
            }
            let end = pos + marker.len() + run;
            result.replace_range(pos..end, SECRET_REDACTION_PLACEHOLDER);
        }
    }

    // Secret-like assignment value: redact the opaque VALUE token (not the name).
    if let Some((name, value)) = secret_assignment(&result) {
        if name_is_secret_like(&name) && value_is_opaque(&value) {
            if let Some(vpos) = result.find(&value) {
                result.replace_range(vpos..vpos + value.len(), SECRET_REDACTION_PLACEHOLDER);
            }
        }
    }

    result
}

/// Heuristic: does this line contain a secret-like token? We test for the
/// known prefixes/markers and for a long opaque value assigned to a
/// secret-looking name. We deliberately DO NOT capture the matched substring.
fn line_looks_secret(line: &str) -> bool {
    // Skip obvious comments to reduce false positives on documentation, but a
    // secret in a comment is still a secret, so we only skip lines that look
    // like pure prose markers — actually, keep it simple and scan everything.
    let l = line;

    // OpenAI-style key prefix: `sk-` followed by >= 16 token chars.
    if let Some(pos) = l.find("sk-") {
        let after = &l[pos + 3..];
        if token_run(after) >= 16 {
            return true;
        }
    }

    // AWS access key id.
    if let Some(pos) = l.find("AKIA") {
        let after = &l[pos + 4..];
        if token_run(after) >= 12 {
            return true;
        }
    }

    // `Bearer <token>` literal in source.
    if let Some(pos) = l.find("Bearer ") {
        let after = &l[pos + "Bearer ".len()..];
        if token_run(after) >= 12 {
            return true;
        }
    }

    // `api_key = "..."` / `apikey: '...'` / `secret = ...` with a non-trivial
    // value (long-ish). Name must look secret-y AND value must be opaque.
    if let Some((name, value)) = secret_assignment(l) {
        if name_is_secret_like(&name) && value_is_opaque(&value) {
            return true;
        }
    }

    false
}

/// Length of the leading run of token characters (alnum, `-`, `_`).
fn token_run(s: &str) -> usize {
    s.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .count()
}

/// Parse a `name <op> value` assignment where op is `=` or `:`. Returns the
/// trimmed name (left of op) and the unquoted value token.
fn secret_assignment(line: &str) -> Option<(String, String)> {
    // Find first `=` or `:` that is not part of `==`, `:=`, `::`.
    let bytes = line.as_bytes();
    let mut op_idx = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'=' {
            // skip `==`, `=>`, `<=`, `>=`, `!=`
            let prev = if i > 0 { bytes[i - 1] } else { 0 };
            let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
            if next != b'='
                && prev != b'='
                && next != b'>'
                && prev != b'!'
                && prev != b'<'
                && prev != b'>'
            {
                op_idx = Some(i);
                break;
            }
        } else if c == b':' {
            let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
            let prev = if i > 0 { bytes[i - 1] } else { 0 };
            if next != b':' && prev != b':' && next != b'=' {
                op_idx = Some(i);
                break;
            }
        }
        i += 1;
    }
    let op = op_idx?;
    let name = line[..op].trim();
    // Take the rightmost identifier-ish token of the name (handles `const X`).
    let name_tok = name
        .rsplit(|c: char| c.is_whitespace() || c == '.')
        .next()
        .unwrap_or(name)
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    let value_raw = line[op + 1..].trim();
    let value = unquote_first(value_raw);
    if name_tok.is_empty() || value.is_empty() {
        return None;
    }
    Some((name_tok.to_string(), value))
}

/// Pull the first quoted string, or the leading token if unquoted.
fn unquote_first(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    if let Some(rest) = s.strip_prefix('\'') {
        return rest.split('\'').next().unwrap_or("").to_string();
    }
    if let Some(rest) = s.strip_prefix('`') {
        return rest.split('`').next().unwrap_or("").to_string();
    }
    // Unquoted: take token up to whitespace / comment / semicolon.
    s.split(|c: char| c.is_whitespace() || c == ';' || c == ',')
        .next()
        .unwrap_or("")
        .to_string()
}

fn name_is_secret_like(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "api_key",
        "apikey",
        "api-key",
        "secret",
        "secretkey",
        "secret_key",
        "token",
        "password",
        "passwd",
        "access_key",
        "accesskey",
        "private_key",
        "auth",
        "credential",
    ];
    MARKERS.iter().any(|m| n.contains(m))
}

/// A value is "opaque" if it's long and looks like base64/hex/random — not a
/// short word, not an obvious placeholder.
fn value_is_opaque(value: &str) -> bool {
    let v = value.trim();
    if v.len() < 16 {
        return false;
    }
    // Obvious non-secret placeholders / references.
    let lower = v.to_ascii_lowercase();
    if lower.starts_with("process.env")
        || lower.starts_with("import.meta")
        || lower.starts_with("env.")
        || lower.contains("${")
        || lower.contains("your-")
        || lower.contains("xxxx")
        || lower.contains("changeme")
        || lower.contains("example")
        || lower.contains("placeholder")
    {
        return false;
    }
    // Must be mostly token chars (base64/hex-ish).
    let token_chars = v
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric()
                || *c == '+'
                || *c == '/'
                || *c == '='
                || *c == '-'
                || *c == '_'
        })
        .count();
    if token_chars < v.len().saturating_mul(9) / 10 {
        return false;
    }
    // Require some entropy signal:
    //   - digit + letter mix (typical of generated keys), OR
    //   - a long pure-hex string, OR
    //   - a long MIXED-CASE token with no digits (e.g. a base64/random token
    //     like `AbCdEfGhIjKlMnOpQrStUvWx`). Without this branch a digit-less
    //     opaque token slips through. We require both upper- AND lower-case and
    //     a longer minimum length to avoid flagging ordinary CamelCase words.
    let has_digit = v.chars().any(|c| c.is_ascii_digit());
    let has_alpha = v.chars().any(|c| c.is_ascii_alphabetic());
    let has_upper = v.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = v.chars().any(|c| c.is_ascii_lowercase());
    let is_hex = v.len() >= 32 && v.chars().all(|c| c.is_ascii_hexdigit());
    // Digit-less mixed-case opaque token: require >= 20 chars and both cases.
    // A single English word (even long, like "internationalization") is all one
    // case after the first letter, so it won't trip both `has_upper`+`has_lower`
    // unless it is genuinely mixed-case like a token. We also exclude dotted
    // references (e.g. `config.auth.defaultToken`): an opaque base64/random
    // token has no `.` separators, whereas a code reference does.
    let is_mixed_case_token =
        v.len() >= 20 && has_upper && has_lower && !v.contains(' ') && !v.contains('.');
    (has_digit && has_alpha) || is_hex || is_mixed_case_token
}

// ---------------------------------------------------------------------------
// TODO / FIXME / HACK smoke
// ---------------------------------------------------------------------------

/// > 3 TODO/FIXME/HACK markers -> a single `smoke` sin.
pub fn detect_todo_smoke(content: &str, file_id: &str) -> Option<DetectedSin> {
    let mut count = 0usize;
    for line in content.lines() {
        let upper = line.to_ascii_uppercase();
        if upper.contains("TODO") || upper.contains("FIXME") || upper.contains("HACK") {
            count += 1;
        }
    }
    if count > 3 {
        let evidence = format!("{count} todo/fixme/hack markers");
        Some(DetectedSin {
            sin: UrbanSin {
                sin_id: deterministic_sin_id("todo", file_id),
                severity: severity::SMOKE.to_string(),
                description: format!("{count} TODO/FIXME/HACK comments accumulated"),
                auto_detectable: true,
                file_id: Some(file_id.to_string()),
            },
            rule_id: "todo-density",
            evidence,
            line: None,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Missing env var
// ---------------------------------------------------------------------------

/// Load the keys declared in `.env.example` (if present). `None` means the file
/// is absent — in that case the env-var sin is skipped entirely. Loaded once by
/// the scanner and threaded into per-file content-sin detection.
pub fn load_env_example(root: &Path) -> Option<HashSet<String>> {
    let text = std::fs::read_to_string(root.join(".env.example")).ok()?;
    let mut keys = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, _)) = line.split_once('=') {
            keys.insert(k.trim().to_string());
        } else {
            keys.insert(line.to_string());
        }
    }
    Some(keys)
}

/// env var used in code (`process.env.X` / `std::env::var("X")`) but absent from
/// `.env.example` -> one `fire` sin per missing var.
pub fn detect_missing_env(
    content: &str,
    allowed: &HashSet<String>,
    file_id: &str,
) -> Vec<DetectedSin> {
    let mut missing: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for var in extract_env_vars(content) {
        if !allowed.contains(&var) && seen.insert(var.clone()) {
            missing.push(var);
        }
    }
    missing.sort();
    missing
        .into_iter()
        .map(|var| {
            let evidence = format!("env var {var} not in example");
            DetectedSin {
                sin: UrbanSin {
                    sin_id: deterministic_sin_id(&format!("env-{var}"), file_id),
                    severity: severity::FIRE.to_string(),
                    description: format!("Env var `{var}` used in code but missing from .env.example"),
                    auto_detectable: true,
                    file_id: Some(file_id.to_string()),
                },
                rule_id: "env-missing",
                evidence,
                line: None,
            }
        })
        .collect()
}

/// Extract referenced env var names from `process.env.X`, `process.env['X']`,
/// `import.meta.env.X`, and `std::env::var("X")`.
pub fn extract_env_vars(content: &str) -> Vec<String> {
    let mut out = Vec::new();

    let mut push_dotted = |hay: &str, marker: &str| {
        let mut start = 0;
        while let Some(rel) = hay[start..].find(marker) {
            let p = start + rel + marker.len();
            let name: String = hay[p..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
            start = p;
        }
    };
    push_dotted(content, "process.env.");
    push_dotted(content, "import.meta.env.");

    // process.env['X'] / process.env["X"]
    for marker in ["process.env[", "import.meta.env["] {
        let mut start = 0;
        while let Some(rel) = content[start..].find(marker) {
            let p = start + rel + marker.len();
            if let Some(name) = quoted_token(&content[p..]) {
                out.push(name);
            }
            start = p;
        }
    }

    // std::env::var("X") / env::var("X")
    for marker in ["env::var(", "env::var_os("] {
        let mut start = 0;
        while let Some(rel) = content[start..].find(marker) {
            let p = start + rel + marker.len();
            if let Some(name) = quoted_token(&content[p..]) {
                out.push(name);
            }
            start = p;
        }
    }

    let mut seen = HashSet::new();
    out.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

fn quoted_token(s: &str) -> Option<String> {
    let s = s.trim_start();
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let q = bytes[0];
    if q != b'"' && q != b'\'' {
        return None;
    }
    let rest = &s[1..];
    let end = rest.find(q as char)?;
    Some(rest[..end].to_string())
}

// ---------------------------------------------------------------------------
// Exported symbol detection (simplified)
// ---------------------------------------------------------------------------

/// `true` if the file declares at least one exported / public symbol. Computed
/// by the scanner during the walk (the orphan-export sin then needs only this
/// bool, not the retained file body).
pub fn has_exported_symbol(content: &str) -> bool {
    content.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with("export ")
            || t.starts_with("export default")
            || t.starts_with("pub fn ")
            || t.starts_with("pub struct ")
            || t.starts_with("pub enum ")
            || t.starts_with("pub const ")
            || t.starts_with("pub trait ")
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic-ish sin id. We use a UUID v4 seeded by nothing (random) only
/// where uniqueness matters and determinism does not; here ids include the kind
/// and file so they're stable enough for UI keys while still unique per run.
fn deterministic_sin_id(kind: &str, file_id: &str) -> String {
    // Stable across runs for the same (kind, file): use a namespaced format.
    // (We avoid a random UUID so the same sin keeps the same id between scans,
    // which the frontend uses for animation continuity.)
    let short = file_id.split('-').next().unwrap_or(file_id);
    format!("sin-{kind}-{short}")
}

/// Stamp the now-known `file_id` onto a content-sin id that was generated during
/// the scan with an empty file_id placeholder (so it ended in a trailing `-`).
/// Idempotent-ish: appends the file's short id to the placeholder. Keeps the
/// kind+line portion so the id stays stable across scans for the same finding.
fn stamp_sin_id(placeholder: &str, file_id: &str) -> String {
    let short = file_id.split('-').next().unwrap_or(file_id);
    format!("{placeholder}{short}")
}

/// Reserved for callers that want a guaranteed-unique id.
#[allow(dead_code)]
fn random_sin_id() -> String {
    format!("sin-{}", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polis::model::Coords;
    use crate::polis::model::{
        building_status, purpose, purpose_source, road_style, road_type, visual_tier, Road,
    };

    fn mk_building(id: &str, path: &str) -> Building {
        mk_building_with_purpose(id, path, purpose::HOUSE)
    }

    fn mk_building_with_purpose(id: &str, path: &str, purpose: &str) -> Building {
        Building {
            file_id: id.into(),
            file_path: path.into(),
            district_id: "d".into(),
            purpose: purpose.into(),
            purpose_source: purpose_source::DEFAULT.into(),
            feature_id: String::new(),
            feature_source: String::new(),
            provider: None,
            lines_of_code: 10,
            visual_tier: visual_tier::KALYBE.into(),
            coords: Coords::new(0.0, 0.0),
            status: building_status::NORMAL.into(),
            label: path.into(),
            description: String::new(),
            last_modified: String::new(),
            agent_present: None,
            suspect_of_card_id: None,
            kanban_card_id: None,
            untracked_change: None,
            sins: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Build a `ScannedFile` the way the scanner does: content-based sins are
    /// detected up front from `content` (which is then dropped), and the
    /// exported-symbol bool is precomputed. No `content` field is retained.
    fn mk_scanned(rel: &str, content: &str) -> ScannedFile {
        let content_hash = {
            let mut h = sha2::Sha256::new();
            use sha2::Digest;
            h.update(content.as_bytes());
            hex::encode(h.finalize())
        };
        ScannedFile {
            rel_path: rel.into(),
            abs_path: Path::new(rel).to_path_buf(),
            lines_of_code: count_nonempty_lines(content),
            raw_imports: Vec::new(),
            head: String::new(),
            has_exported_symbol: has_exported_symbol(content),
            content_sins: detect_content_sins(content, None),
            content_hash,
            scan_note: None,
        }
    }

    fn count_nonempty_lines(content: &str) -> u32 {
        content.lines().count() as u32
    }

    // ---- SECRET DETECTION + REDACTION GUARANTEE ----
    #[test]
    fn detects_openai_key_and_does_not_leak_value() {
        let secret = "sk-abc123DEF456ghi789JKL012";
        let content = format!("const key = \"{secret}\";\n");
        let sins = detect_secrets(&content, "fid");
        assert_eq!(sins.len(), 1);
        assert_eq!(sins[0].sin.severity, severity::INFERNO);
        // CRITICAL: the secret value must NOT appear in the description.
        assert!(
            !sins[0].sin.description.contains(secret),
            "secret value leaked into description: {}",
            sins[0].sin.description
        );
        assert!(!sins[0].sin.description.contains("sk-abc"));
        assert!(sins[0].sin.description.contains("line 1"));
        // Also ensure the redaction holds when serialized to JSON.
        let json = serde_json::to_string(&sins[0]).unwrap();
        assert!(!json.contains(secret), "secret leaked through JSON");
    }

    #[test]
    fn detects_bearer_aws_and_api_key_assignment() {
        let bearer = "Authorization: Bearer aZ09bQ12cX34dV56\n";
        assert_eq!(detect_secrets(bearer, "f").len(), 1);

        let aws = "const id = 'AKIAIOSFODNN7EXAMPLE9';\n";
        assert_eq!(detect_secrets(aws, "f").len(), 1);

        let api = "api_key = \"9f8e7d6c5b4a3f2e1d0c9b8a7\"\n";
        let sins = detect_secrets(api, "f");
        assert_eq!(sins.len(), 1);
        assert!(!sins[0].sin.description.contains("9f8e7d6c"));
    }

    #[test]
    fn does_not_flag_env_references_or_placeholders() {
        // env reference, not a literal secret.
        assert!(detect_secrets("const key = process.env.API_KEY;\n", "f").is_empty());
        assert!(
            detect_secrets("const apiKey = \"your-api-key-here-placeholder\";\n", "f").is_empty()
        );
        // short value -> not opaque.
        assert!(detect_secrets("password = \"short\"\n", "f").is_empty());
        // normal config, no secret-y name.
        assert!(detect_secrets("const url = \"https://example.com/path\";\n", "f").is_empty());
    }

    // ---- TODO smoke ----
    #[test]
    fn todo_smoke_only_fires_above_three() {
        let three = "// TODO a\n// FIXME b\n// HACK c\n";
        assert!(detect_todo_smoke(three, "f").is_none());
        let four = "// TODO a\n// FIXME b\n// HACK c\n// TODO d\n";
        let sin = detect_todo_smoke(four, "f").unwrap();
        assert_eq!(sin.sin.severity, severity::SMOKE);
        assert!(sin.sin.description.contains('4'));
    }

    // ---- missing env ----
    #[test]
    fn missing_env_fires_for_vars_absent_from_example() {
        let allowed: HashSet<String> = ["KNOWN_VAR".to_string()].into_iter().collect();
        let content =
            "const a = process.env.KNOWN_VAR;\nconst b = process.env.SECRET_TOKEN_X;\nstd::env::var(\"RUST_VAR\");\n";
        let sins = detect_missing_env(content, &allowed, "f");
        let descs: Vec<&str> = sins.iter().map(|s| s.sin.description.as_str()).collect();
        // KNOWN_VAR is allowed -> no sin.
        assert!(!descs.iter().any(|d| d.contains("KNOWN_VAR")));
        // The two unknown vars -> fire sins.
        assert!(descs.iter().any(|d| d.contains("SECRET_TOKEN_X")));
        assert!(descs.iter().any(|d| d.contains("RUST_VAR")));
        assert!(sins.iter().all(|s| s.sin.severity == severity::FIRE));
    }

    #[test]
    fn extract_env_vars_handles_all_forms() {
        let content = r#"
process.env.A_VAR;
process.env['B_VAR'];
import.meta.env.C_VAR;
std::env::var("D_VAR");
env::var_os("E_VAR");
"#;
        let vars = extract_env_vars(content);
        for v in ["A_VAR", "B_VAR", "C_VAR", "D_VAR", "E_VAR"] {
            assert!(vars.contains(&v.to_string()), "missing {v} in {vars:?}");
        }
    }

    // ---- digit-less opaque token (issue #8) ----
    #[test]
    fn detects_digitless_mixed_case_token_without_leaking() {
        // A long base64/token with NO digits but mixed case must now be flagged.
        let secret = "AbCdEfGhIjKlMnOpQrStUvWx";
        let content = format!("api_key = \"{secret}\"\n");
        let sins = detect_secrets(&content, "fid");
        assert_eq!(sins.len(), 1, "digit-less opaque token should be detected");
        assert_eq!(sins[0].sin.severity, severity::INFERNO);
        // Redaction must still hold.
        assert!(!sins[0].sin.description.contains(secret));
        let json = serde_json::to_string(&sins[0]).unwrap();
        assert!(!json.contains(secret), "secret leaked through JSON");
    }

    #[test]
    fn does_not_flag_ordinary_long_identifier_value() {
        // A long single-case English word is not mixed-case -> not flagged.
        assert!(detect_secrets("secret = \"internationalization\"\n", "f").is_empty());
        // A normal dotted reference is not opaque.
        assert!(detect_secrets("token = config.auth.defaultToken\n", "f").is_empty());
    }

    // FIX 4: free-form dossier/blurb text that ECHOES a secret must come back with
    // the secret value REDACTED before it is persisted or returned.
    #[test]
    fn redact_secret_tokens_scrubs_sk_bearer_and_api_key_from_prose() {
        let sk = "sk-abc123DEF456ghi789JKL012";
        let bearer = "aZ09bQ12cX34dV56";
        let api = "9f8e7d6c5b4a3f2e1d0c9b8a7";

        let dossier = format!(
            "This worker authenticates with the key {sk} and sends \
             Authorization: Bearer {bearer} on each call.\n\
             It also reads api_key = \"{api}\" from a hardcoded constant.\n\
             The RNA-seq pipeline is orchestrated here."
        );

        let scrubbed = redact_secret_tokens(&dossier);

        // NONE of the secret values survive.
        assert!(!scrubbed.contains(sk), "sk- token leaked: {scrubbed}");
        assert!(
            !scrubbed.contains(bearer),
            "Bearer token leaked: {scrubbed}"
        );
        assert!(!scrubbed.contains(api), "api_key value leaked: {scrubbed}");
        // The placeholder is present and the surrounding prose is preserved.
        assert!(scrubbed.contains(SECRET_REDACTION_PLACEHOLDER));
        assert!(scrubbed.contains("This worker authenticates"));
        assert!(scrubbed.contains("The RNA-seq pipeline is orchestrated here."));
        // Newlines preserved.
        assert_eq!(scrubbed.lines().count(), 3);
    }

    #[test]
    fn redact_secret_tokens_leaves_clean_prose_untouched() {
        let clean = "This module orchestrates the RNA-seq quantification flow.\n\
                     It reads its key from process.env.API_KEY at runtime.\n";
        assert_eq!(
            redact_secret_tokens(clean),
            clean,
            "clean prose must be byte-identical"
        );
    }

    // ---- cyclic import sin (via detect_graph_sins) ----
    #[test]
    fn cyclic_import_produces_fire_on_each_node() {
        let buildings = vec![mk_building("a", "a.ts"), mk_building("b", "b.ts")];
        let roads = vec![
            Road {
                road_id: "r1".into(),
                from: "a".into(),
                to: "b".into(),
                road_type: "import".into(),
                style: "lastricata".into(),
                weight: 1,
                path: None,
                provenance: None,
            },
            Road {
                road_id: "r2".into(),
                from: "b".into(),
                to: "a".into(),
                road_type: "import".into(),
                style: "lastricata".into(),
                weight: 1,
                path: None,
                provenance: None,
            },
        ];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("a.ts", "import './b';\n"),
            mk_scanned("b.ts", "import './a';\n"),
        ];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let a_sins = report.by_file.get("a").cloned().unwrap_or_default();
        assert!(a_sins
            .iter()
            .any(|s| s.sin.severity == severity::FIRE && s.sin.description.contains("Cyclic")));
    }

    // ---- exported-but-never-imported smoke ----
    #[test]
    fn orphan_export_produces_smoke() {
        // building "lib" exports a symbol; no road points to it -> smoke.
        let buildings = vec![mk_building("lib", "lib.ts"), mk_building("user", "user.ts")];
        // user imports nothing that resolves to lib -> lib has 0 incoming edges.
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("lib.ts", "export const orphan = 1;\n"),
            mk_scanned("user.ts", "const x = 1;\n"),
        ];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let lib_sins = report.by_file.get("lib").cloned().unwrap_or_default();
        assert!(lib_sins
            .iter()
            .any(|s| s.sin.severity == severity::SMOKE && s.sin.description.contains("Exported")));
    }

    // ---- lighthouse entry points are NOT orphan-export false positives (issue #12) ----
    #[test]
    fn lighthouse_entry_point_is_not_flagged_as_orphan_export() {
        // An entry point exports a symbol and legitimately has 0 incoming edges.
        // It must NOT trip the orphan-export sin.
        let buildings = vec![mk_building_with_purpose(
            "main",
            "src/main.tsx",
            purpose::LIGHTHOUSE,
        )];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned(
            "src/main.tsx",
            "export default function App() {}\n",
        )];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let main_sins = report.by_file.get("main").cloned().unwrap_or_default();
        assert!(
            !main_sins.iter().any(|s| s.sin.description.contains("Exported")),
            "lighthouse entry point must not be flagged as a dead export"
        );
    }

    // ---- commented-out env reference does not fire (issue #9) ----
    #[test]
    fn commented_out_env_reference_does_not_fire() {
        let allowed: HashSet<String> = HashSet::new();
        // A live env ref fires; a commented-out one must not (content-stripped).
        let live = detect_content_sins("const a = process.env.LIVE_VAR;\n", Some(&allowed));
        assert!(live.iter().any(|s| s.sin.description.contains("LIVE_VAR")));

        let commented =
            detect_content_sins("// const a = process.env.GHOST_VAR;\n", Some(&allowed));
        assert!(
            !commented
                .iter()
                .any(|s| s.sin.description.contains("GHOST_VAR")),
            "commented-out env reference must not raise a phantom fire"
        );
    }

    // ---- content sins carry the file_id after graph phase ----
    #[test]
    fn content_sins_are_keyed_and_stamped_by_file_id() {
        let buildings = vec![mk_building("fid-secret", "s.ts")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        // A real secret in the content -> content sin detected during the scan.
        let scanned = vec![mk_scanned(
            "s.ts",
            "api_key = \"AbCdEfGhIjKlMnOpQrStUvWx\"\n",
        )];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let s = report
            .by_file
            .get("fid-secret")
            .cloned()
            .unwrap_or_default();
        assert!(s
            .iter()
            .any(|sin| sin.sin.severity == severity::INFERNO
                && sin.sin.file_id.as_deref() == Some("fid-secret")));
    }

    // =========================================================================
    // P2.2 -- Tarjan SCC dep-cycle sin tests
    // =========================================================================

    #[test]
    fn dep_cycle_scc_of_two_produces_fire_with_evidence_string() {
        let buildings = vec![mk_building("a", "a.ts"), mk_building("b", "b.ts")];
        let roads = vec![
            Road {
                road_id: "r1".into(),
                from: "a".into(),
                to: "b".into(),
                road_type: "import".into(),
                style: "lastricata".into(),
                weight: 1,
                path: None,
                provenance: Some("ast".into()),
            },
            Road {
                road_id: "r2".into(),
                from: "b".into(),
                to: "a".into(),
                road_type: "import".into(),
                style: "lastricata".into(),
                weight: 1,
                path: None,
                provenance: Some("ast".into()),
            },
        ];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("a.ts", "import './b';\n"),
            mk_scanned("b.ts", "import './a';\n"),
        ];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let a_sins = report.by_file.get("a").cloned().unwrap_or_default();
        let cycle_sin = a_sins.iter().find(|s| s.rule_id == "dep-cycle").expect("dep-cycle sin expected");
        assert_eq!(cycle_sin.sin.severity, severity::FIRE, "2-member SCC must be fire");
        assert!(cycle_sin.evidence.starts_with("cycle: "), "evidence must describe the cycle");
        // Evidence should show file PATHS, not file_ids.
        assert!(cycle_sin.evidence.contains("a.ts") && cycle_sin.evidence.contains("b.ts"),
            "evidence must mention both cycle member paths: {}", cycle_sin.evidence);
        // file_ids (a, b) should NOT leak into evidence.
        assert!(!cycle_sin.evidence.contains("cycle: a -> b"),
            "evidence must use paths not ids: {}", cycle_sin.evidence);
    }

    #[test]
    fn dep_cycle_scc_of_three_produces_inferno() {
        let buildings = vec![
            mk_building("a", "a.ts"),
            mk_building("b", "b.ts"),
            mk_building("c", "c.ts"),
        ];
        let roads = vec![
            Road {
                road_id: "r1".into(), from: "a".into(), to: "b".into(),
                road_type: "import".into(), style: "lastricata".into(),
                weight: 1, path: None, provenance: Some("ast".into()),
            },
            Road {
                road_id: "r2".into(), from: "b".into(), to: "c".into(),
                road_type: "import".into(), style: "lastricata".into(),
                weight: 1, path: None, provenance: Some("ast".into()),
            },
            Road {
                road_id: "r3".into(), from: "c".into(), to: "a".into(),
                road_type: "import".into(), style: "lastricata".into(),
                weight: 1, path: None, provenance: Some("ast".into()),
            },
        ];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("a.ts", "import './b';\n"),
            mk_scanned("b.ts", "import './c';\n"),
            mk_scanned("c.ts", "import './a';\n"),
        ];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let a_sins = report.by_file.get("a").cloned().unwrap_or_default();
        let cycle_sin = a_sins.iter().find(|s| s.rule_id == "dep-cycle").expect("dep-cycle sin expected");
        assert_eq!(cycle_sin.sin.severity, severity::INFERNO, ">=3 SCC must be inferno");
    }

    #[test]
    fn dep_cycle_large_scc_evidence_truncated() {
        // Build a 6-member cycle -- evidence must truncate to 5 members + " -> ...".
        let ids: Vec<String> = (0..6).map(|i| {
            let ch = char::from_u32(97 + i as u32).unwrap();
            ch.to_string()
        }).collect();
        let mut buildings = Vec::new();
        let mut roads = Vec::new();
        let mut scanned = Vec::new();
        for i in 0..6 {
            let id = &ids[i];
            let path = format!("{}.ts", id);
            buildings.push(mk_building(id, &path));
            scanned.push(mk_scanned(&path, &format!("import './{}';\n", ids[(i+1)%6])));
        }
        for i in 0..6 {
            roads.push(Road {
                road_id: format!("r{}", i),
                from: ids[i].clone(),
                to: ids[(i+1)%6].clone(),
                road_type: "import".into(),
                style: "lastricata".into(),
                weight: 1,
                path: None,
                provenance: Some("ast".into()),
            });
        }
        let graph = RoadGraph::build(&buildings, &roads);
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        // Every member gets a dep-cycle sin.
        let a_sins = report.by_file.get("a").cloned().unwrap_or_default();
        let cycle_sin = a_sins.iter().find(|s| s.rule_id == "dep-cycle").expect("dep-cycle sin expected");
        // Evidence for 6-member cycle: first 5 sorted, then " -> ..."
        assert!(cycle_sin.evidence.ends_with(" -> ..."),
            "large SCC evidence must be truncated: {}", cycle_sin.evidence);
        let prefix = cycle_sin.evidence.strip_suffix(" -> ...").unwrap();
        let members = prefix.strip_prefix("cycle: ").unwrap();
        assert_eq!(members.split(" -> ").count(), 5, "must list exactly 5 members");
        // Evidence should use file paths (.ts), not file_ids (single letters).
        assert!(cycle_sin.evidence.contains(".ts"),
            "evidence must contain paths, got: {}", cycle_sin.evidence);
    }



    #[test]
    fn clone_road_does_not_fabricate_dep_cycle() {
        // B1 regression: import A→B + clone road B→A must produce NO dep-cycle sin.
        // Without the B1 filter, tarjan_scc_from_roads sees both edges → 2-node SCC.
        let buildings = vec![mk_building("a", "a.ts"), mk_building("b", "b.ts")];
        let roads = vec![
            // Real import: A imports B.
            Road {
                road_id: "r-import".into(),
                from: "a".into(),
                to: "b".into(),
                road_type: road_type::IMPORT.to_string(),
                style: road_style::LASTRICATA.to_string(),
                weight: 1,
                path: None,
                provenance: Some("ast".into()),
            },
            // Clone road: B↔A (opposite direction).  Must NOT create a cycle.
            Road {
                road_id: "r-clone".into(),
                from: "b".into(),
                to: "a".into(),
                road_type: road_type::CLONE.to_string(),
                style: road_style::TERRA_BATTUTA.to_string(),
                weight: 1,
                path: None,
                provenance: Some("ast".into()),
            },
        ];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("a.ts", "import './b';\n"),
            mk_scanned("b.ts", "export const x = 1;\n"),
        ];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, None);
        let all_sins: Vec<_> = report.by_file.values().flatten().collect();
        assert!(
            all_sins.iter().all(|s| s.rule_id != "dep-cycle"),
            "clone road must not fabricate a dep-cycle sin, got {:?}",
            all_sins.iter().filter(|s| s.rule_id == "dep-cycle").collect::<Vec<_>>()
        );
    }

    // =========================================================================
    // P4.1 — complexity sin thresholds + 3-worst cap
    // =========================================================================

    #[test]
    fn complexity_items_fire_at_thresholds() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let buildings = vec![mk_building("fid", "src/lib.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/lib.rs", "pub fn flat() {}")];

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/lib.rs".to_string(),
                loc: 60,
                items: vec![
                    // cc 15 → smoke threshold exactly
                    ItemMetric { name: Some("smoker".to_string()), line: 1, complexity: 15, kind: "function_item".to_string() },
                    // cc 25 → fire
                    ItemMetric { name: Some("burner".to_string()), line: 20, complexity: 25, kind: "function_item".to_string() },
                    // cc 40 → inferno
                    ItemMetric { name: Some("inferno_fn".to_string()), line: 30, complexity: 40, kind: "function_item".to_string() },
                ],
                exported: vec![],
            }],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let sins = report.by_file.get("fid").cloned().unwrap_or_default();

        let smoke_sin = sins.iter().find(|s| s.rule_id == "complexity" && s.sin.severity == severity::SMOKE);
        let fire_sin = sins.iter().find(|s| s.rule_id == "complexity" && s.sin.severity == severity::FIRE);
        let inferno_sin = sins.iter().find(|s| s.rule_id == "complexity" && s.sin.severity == severity::INFERNO);

        assert!(smoke_sin.is_some(), "cc 15 must produce smoke");
        assert!(fire_sin.is_some(), "cc 25 must produce fire");
        assert!(inferno_sin.is_some(), "cc 40 must produce inferno");
    }

    #[test]
    fn complexity_capped_at_three_worst() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let buildings = vec![mk_building("fid", "src/lib.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/lib.rs", "pub fn flat() {}")];

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/lib.rs".to_string(),
                loc: 100,
                items: (0..10).map(|i| ItemMetric {
                    name: Some(format!("fn{i}")),
                    line: i * 10 + 1,
                    complexity: 16 + i, // 16, 17, ..., 25
                    kind: "function_item".to_string(),
                }).collect(),
                exported: vec![],
            }],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let sins = report.by_file.get("fid").cloned().unwrap_or_default();
        let complexity_sins: Vec<_> = sins.iter().filter(|s| s.rule_id == "complexity").collect();
        assert_eq!(complexity_sins.len(), 3, "must cap at 3 worst, got {}", complexity_sins.len());
    }

    #[test]
    fn complexity_skips_below_threshold() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let buildings = vec![mk_building("fid", "src/lib.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/lib.rs", "pub fn flat() {}")];

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/lib.rs".to_string(),
                loc: 10,
                items: vec![
                    ItemMetric { name: Some("low".to_string()), line: 1, complexity: 14, kind: "function_item".to_string() },
                ],
                exported: vec![],
            }],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let sins = report.by_file.get("fid").cloned().unwrap_or_default();
        let complexity_sins: Vec<_> = sins.iter().filter(|s| s.rule_id == "complexity").collect();
        assert!(complexity_sins.is_empty(), "cc 14 < 15 → no sin");
    }

    // =========================================================================
    // P4.1 — god-file (P95 of loc AND fan-in)
    // =========================================================================

    #[test]
    fn god_file_fires_when_above_p95_both_axes() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric, ImportEdge};
        use std::collections::BTreeSet;

        // Build 25 synthetic files — one is a clear outlier in BOTH loc and fan-in.
        let n = 25usize;
        let mut metrics = Vec::new();
        let mut edges = Vec::new();
        for i in 0..n {
            let path = format!("src/file{i}.rs");
            let loc = if i == 0 { 5000 } else { 50 + i as u32 }; // file0 is the god
            metrics.push(FileMetrics {
                rel_path: path.clone(),
                loc,
                items: vec![ItemMetric {
                    name: Some(format!("fn{i}")),
                    line: 1,
                    complexity: 1,
                    kind: "function_item".to_string(),
                }],
                exported: vec![format!("Fn{i}")],
            });
            // Give file0 lots of inbound edges (high fan-in).
            if i > 0 {
                edges.push(ImportEdge {
                    from: format!("src/file{i}.rs"),
                    to: "src/file0.rs".to_string(),
                    weight: 1,
                });
            }
        }
        let all_paths: BTreeSet<String> = metrics.iter().map(|m| m.rel_path.clone()).collect();

        let ig = ImportGraph {
            edges,
            capped: false,
            files: all_paths,
            metrics,
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let mut buildings = Vec::new();
        let mut scanned = Vec::new();
        for i in 0..n {
            let id = format!("fid{i}");
            let path = format!("src/file{i}.rs");
            buildings.push(mk_building(&id, &path));
            scanned.push(mk_scanned(&path, &format!("pub fn fn{i}() {{}}")));
        }
        let roads: Vec<Road> = vec![];
        let road_graph = RoadGraph::build(&buildings, &roads);

        let report = detect_graph_sins(&scanned, &buildings, &road_graph, &roads, Some(&ig));
        let god_sin = report.by_file.get("fid0").and_then(|sins| {
            sins.iter().find(|s| s.rule_id == "god-file")
        });
        assert!(god_sin.is_some(), "file0 must be a god-file: high LOC + high fan-in");
        // B2: evidence is stable, description carries numbers.
        assert_eq!(god_sin.unwrap().evidence, "file size and fan-in both above project P95");
        assert!(god_sin.unwrap().sin.description.contains("5000"), "description must carry LOC");
        // file1 (no fan-in, normal LOC) must NOT be god-file.
        let others: Vec<_> = report.by_file.iter()
            .filter(|(id, sins)| *id != "fid0" && sins.iter().any(|s| s.rule_id == "god-file"))
            .collect();
        assert!(others.is_empty(), "only the outlier is a god-file, got {:?}", others);
    }

    #[test]
    fn god_file_skipped_when_below_min_files() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/a.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/a.rs".to_string(),
                loc: 10_000,
                items: vec![],
                exported: vec![],
            }],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let buildings = vec![mk_building("fid", "src/a.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/a.rs", "pub fn huge() {}")];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let any_god = report.by_file.values().flatten().any(|s| s.rule_id == "god-file");
        assert!(!any_god, "< 20 files → god-file skipped");
    }

    // =========================================================================
    // P4.1 — test-gap (exported symbols unreferenced by tests)
    // =========================================================================

    #[test]
    fn test_gap_fires_when_unreferenced_symbols_exist() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string(), "tests/test.rs".to_string()].into_iter().collect(),
            metrics: vec![
                FileMetrics {
                    rel_path: "src/lib.rs".to_string(),
                    loc: 20,
                    items: vec![],
                    exported: vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()],
                },
                FileMetrics {
                    rel_path: "tests/test.rs".to_string(),
                    loc: 10,
                    items: vec![],
                    exported: vec![],
                },
            ],
            // test_refs is empty → none of the exported symbols are referenced by tests
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let buildings = vec![
            mk_building("fid-lib", "src/lib.rs"),
            mk_building("fid-test", "tests/test.rs"),
        ];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("src/lib.rs", "pub struct Alpha; pub struct Beta; pub struct Gamma;"),
            mk_scanned("tests/test.rs", "// no test references"),
        ];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let lib_sins = report.by_file.get("fid-lib").cloned().unwrap_or_default();
        let gap = lib_sins.iter().find(|s| s.rule_id == "test-gap");
        assert!(gap.is_some(), "3 unreferenced exported symbols → test-gap sin");
        // B2: evidence is stable; description carries the measured values.
        assert!(gap.unwrap().sin.description.contains("Alpha"), "description must name the symbols");
        assert!(!gap.unwrap().evidence.contains("Alpha"), "evidence must NOT carry symbol names");
    }

    #[test]
    fn test_gap_skips_when_fewer_than_three_unreferenced() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string(), "tests/test.rs".to_string()].into_iter().collect(),
            metrics: vec![
                FileMetrics {
                    rel_path: "src/lib.rs".to_string(),
                    loc: 10,
                    items: vec![],
                    exported: vec!["Only".to_string(), "Two".to_string()],
                },
                FileMetrics {
                    rel_path: "tests/test.rs".to_string(),
                    loc: 5,
                    items: vec![],
                    exported: vec![],
                },
            ],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let buildings = vec![
            mk_building("fid-lib", "src/lib.rs"),
            mk_building("fid-test", "tests/test.rs"),
        ];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("src/lib.rs", "pub struct Only; pub struct Two;"),
            mk_scanned("tests/test.rs", "// empty"),
        ];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let lib_sins = report.by_file.get("fid-lib").cloned().unwrap_or_default();
        assert!(lib_sins.iter().all(|s| s.rule_id != "test-gap"), "< 3 unreferenced → skip");
    }

    #[test]
    fn test_gap_skips_when_no_test_files() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/lib.rs".to_string(),
                loc: 10,
                items: vec![],
                exported: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            }],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let buildings = vec![mk_building("fid", "src/lib.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/lib.rs", "pub struct A; pub struct B; pub struct C;")];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let sins = report.by_file.get("fid").cloned().unwrap_or_default();
        assert!(sins.iter().all(|s| s.rule_id != "test-gap"), "no test files → skip");
    }

    // =========================================================================
    // P4.1 — dead-export upgrade (AST evidence suppresses name-heuristic)
    // =========================================================================

    #[test]
    fn dead_export_suppressed_when_ast_fan_in_positive() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric, ImportEdge};
        use std::collections::BTreeSet;

        // AST says src/lib.ts has fan-in 1 → the name-heuristic dead-export sin
        // (from zero regex roads) must be SUPPRESSED.
        let ig = ImportGraph {
            edges: vec![ImportEdge {
                from: "src/consumer.ts".to_string(),
                to: "src/lib.ts".to_string(),
                weight: 1,
            }],
            capped: false,
            files: ["src/lib.ts".to_string(), "src/consumer.ts".to_string()]
                .into_iter().collect(),
            metrics: vec![
                FileMetrics {
                    rel_path: "src/lib.ts".to_string(),
                    loc: 5,
                    items: vec![],
                    exported: vec!["Helper".to_string()],
                },
                FileMetrics {
                    rel_path: "src/consumer.ts".to_string(),
                    loc: 5,
                    items: vec![],
                    exported: vec![],
                },
            ],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let buildings = vec![
            mk_building("fid-lib", "src/lib.ts"),
            mk_building("fid-consumer", "src/consumer.ts"),
        ];
        // NO regex roads → old heuristic would fire dead-export.
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("src/lib.ts", "export const Helper = 1;"),
            mk_scanned("src/consumer.ts", "import { Helper } from './lib';"),
        ];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let lib_sins = report.by_file.get("fid-lib").cloned().unwrap_or_default();
        assert!(
            lib_sins.iter().all(|s| s.rule_id != "dead-export"),
            "AST fan-in > 0 must suppress dead-export sin"
        );
    }

    #[test]
    fn dead_export_kept_when_ast_fan_in_zero() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        // AST says fan-in == 0 AND regex says no imports → keep the sin.
        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/orphan.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/orphan.rs".to_string(),
                loc: 5,
                items: vec![],
                exported: vec!["OrphanFn".to_string()],
            }],
            test_refs: BTreeSet::new(),
            clones: Vec::new(),
        };

        let buildings = vec![mk_building("fid", "src/orphan.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/orphan.rs", "pub fn orphan_fn() {}")];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let sins = report.by_file.get("fid").cloned().unwrap_or_default();
        assert!(
            sins.iter().any(|s| s.rule_id == "dead-export"),
            "AST fan-in == 0 + regex zero → dead-export must fire"
        );
    }


    // =========================================================================
    // P4.2 — clone sin (both files get a sin, cross-referencing evidence)
    // =========================================================================

    #[test]
    fn clone_pair_produces_two_sins_with_cross_references() {
        use crate::backend::graph::{ImportGraph, ClonePair, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/a.rs".to_string(), "src/b.rs".to_string()].into_iter().collect(),
            metrics: vec![
                FileMetrics { rel_path: "src/a.rs".to_string(), loc: 10, items: vec![], exported: vec![] },
                FileMetrics { rel_path: "src/b.rs".to_string(), loc: 10, items: vec![], exported: vec![] },
            ],
            test_refs: BTreeSet::new(),
            clones: vec![ClonePair {
                a: "src/a.rs".to_string(),
                b: "src/b.rs".to_string(),
                a_line: 5,
                b_line: 12,
                tokens: 80,
            }],
        };

        let buildings = vec![
            mk_building("fid-a", "src/a.rs"),
            mk_building("fid-b", "src/b.rs"),
        ];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![
            mk_scanned("src/a.rs", "fn a() {}"),
            mk_scanned("src/b.rs", "fn b() {}"),
        ];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));

        let a_sins = report.by_file.get("fid-a").cloned().unwrap_or_default();
        let a_clone = a_sins.iter().find(|s| s.rule_id == "clone").expect("a must have clone sin");
        assert!(a_clone.evidence.contains("src/b.rs"), "evidence must reference other file");
        // B2: evidence is stable — measured numbers live in description only.
        assert!(a_clone.sin.description.contains("80"), "description must carry token count");
        assert!(!a_clone.evidence.contains("80 tokens"), "evidence must NOT carry token count");
        assert_eq!(a_clone.sin.severity, severity::SMOKE, "80 tokens → smoke");

        let b_sins = report.by_file.get("fid-b").cloned().unwrap_or_default();
        let b_clone = b_sins.iter().find(|s| s.rule_id == "clone").expect("b must have clone sin");
        assert!(b_clone.evidence.contains("src/a.rs"), "evidence must reference other file");
    }

    #[test]
    fn clone_pair_with_100_tokens_is_fire() {
        use crate::backend::graph::{ImportGraph, ClonePair, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["x.rs".to_string(), "y.rs".to_string()].into_iter().collect(),
            metrics: vec![
                FileMetrics { rel_path: "x.rs".to_string(), loc: 10, items: vec![], exported: vec![] },
                FileMetrics { rel_path: "y.rs".to_string(), loc: 10, items: vec![], exported: vec![] },
            ],
            test_refs: BTreeSet::new(),
            clones: vec![ClonePair {
                a: "x.rs".to_string(), b: "y.rs".to_string(),
                a_line: 1, b_line: 1, tokens: 100,
            }],
        };
        let buildings = vec![mk_building("fx", "x.rs"), mk_building("fy", "y.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("x.rs", "fn x() {}"), mk_scanned("y.rs", "fn y() {}")];
        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let x_sins = report.by_file.get("fx").cloned().unwrap_or_default();
        let clone = x_sins.iter().find(|s| s.rule_id == "clone").expect("clone sin expected");
        assert_eq!(clone.sin.severity, severity::FIRE, "100 tokens → fire");
    }


    // =========================================================================
    // M1 — container complexity: impl/class aggregates don't trip; per-method do
    // =========================================================================

    #[test]
    fn container_aggregate_does_not_trip_but_heavy_method_does() {
        use crate::backend::graph::{ImportGraph, FileMetrics, ItemMetric};
        use std::collections::BTreeSet;

        // An impl with 10 trivial methods (cc=1 each) and one heavy method (cc=27).
        // Before M1 fix: the aggregate impl cc trips.  After: only the heavy method.
        let ig = ImportGraph {
            edges: vec![],
            capped: false,
            files: ["src/lib.rs".to_string()].into_iter().collect(),
            metrics: vec![FileMetrics {
                rel_path: "src/lib.rs".to_string(),
                loc: 200,
                items: vec![
                    // The impl container itself — aggregate cc high but child_complexities
                    // are expanded per-method.
                    ItemMetric { name: Some("Counter".to_string()), line: 1, complexity: 30, kind: "impl_item".to_string() },
                    // Per-method entries: 10 trivial + 1 heavy.
                    ItemMetric { name: Some("Counter::triv1".to_string()), line: 2, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv2".to_string()), line: 3, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv3".to_string()), line: 4, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv4".to_string()), line: 5, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv5".to_string()), line: 6, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv6".to_string()), line: 7, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv7".to_string()), line: 8, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv8".to_string()), line: 9, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv9".to_string()), line: 10, complexity: 1, kind: "function_item".to_string() },
                    ItemMetric { name: Some("Counter::triv10".to_string()), line: 11, complexity: 1, kind: "function_item".to_string() },
                    // This one should trip.
                    ItemMetric { name: Some("Counter::heavy_method".to_string()), line: 12, complexity: 27, kind: "function_item".to_string() },
                ],
                exported: vec![],
            }],
            test_refs: BTreeSet::new(),
            clones: vec![],
        };

        let buildings = vec![mk_building("fid", "src/lib.rs")];
        let roads: Vec<Road> = vec![];
        let graph = RoadGraph::build(&buildings, &roads);
        let scanned = vec![mk_scanned("src/lib.rs", "pub struct Counter;")];

        let report = detect_graph_sins(&scanned, &buildings, &graph, &roads, Some(&ig));
        let sins = report.by_file.get("fid").cloned().unwrap_or_default();
        let complexity_sins: Vec<_> = sins.iter().filter(|s| s.rule_id == "complexity").collect();

        // Only the heavy method should trip — not the impl aggregate and not the trivial ones.
        assert_eq!(complexity_sins.len(), 1, "only heavy method must trip, got {:?}",
            complexity_sins.iter().map(|s| &s.sin.description).collect::<Vec<_>>());
        let only = &complexity_sins[0];
        assert!(only.sin.description.contains("Counter::heavy_method"),
            "sin must name the heavy method, got: {}", only.sin.description);
        assert_eq!(only.line, Some(12), "sin line must be the method's line");
    }

}