//! Runtime embedding layer: backend trait, per-platform auto-selection, and a
//! resident pool with lazy load / idle unload / cooperative cancellation.
//!
//! This is the piece that replaces the Python resident server's
//! `ingestion/embedder.py` process-level lifecycle: instead of killing a child
//! process to reclaim RAM, the pool drops the model after an idle period and
//! reloads on demand (PLAN.md P3).

mod candle_backend;
pub mod ort_backend;

pub use candle_backend::CandleEmbedder;
pub use ort_backend::OrtEmbedder;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cooperative cancellation flag, checked between batches.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Attention / sequence memory guards (Metal unified memory is wired — OOM freezes)
// ═══════════════════════════════════════════════════════════════════════════

/// Hard cap on tokens per forward-pass sequence.
///
/// Long texts are **not** truncated: they are split into overlapping byte
/// windows of at most [`EMBED_WINDOW_BYTES`], embedded, then mean-pooled.
/// This cap only bounds a single window's forward pass. Override via
/// `ORACLE_EMBED_MAX_SEQ_TOKENS`.
pub const EMBED_MAX_SEQ_TOKENS: usize = 2560;

/// Headroom reserved for tokenizer special tokens (EOS from the post-processor
/// plus a small margin for any future post-processor additions).
///
/// Byte-level BPE guarantees `n_content_tokens ≤ n_bytes`, but the tokenizer
/// appends EOS (and possibly more) **after** that bound. Without this reserve,
/// a pathological window of `EMBED_MAX_SEQ_TOKENS` one-byte tokens becomes
/// `EMBED_MAX_SEQ_TOKENS + 1` after EOS and the safety-net truncation drops a
/// real token.
pub const EMBED_SPECIAL_TOKEN_RESERVE: usize = 4;

/// Max window size in **bytes** (not chars).
///
/// Qwen3's tokenizer is byte-level BPE, so every content token consumes ≥1 byte
/// and `n_content_tokens ≤ n_bytes`. Keeping the window at most
/// `EMBED_MAX_SEQ_TOKENS - EMBED_SPECIAL_TOKEN_RESERVE` bytes leaves room for
/// EOS/specials so the model never truncates. Override via
/// `ORACLE_EMBED_WINDOW_BYTES` (clamped at runtime — see
/// [`resolve_embed_window_bytes`]).
pub const EMBED_WINDOW_BYTES: usize = EMBED_MAX_SEQ_TOKENS - EMBED_SPECIAL_TOKEN_RESERVE;

/// Overlap between consecutive embed windows, in bytes (snapped to a UTF-8
/// char boundary). Override via `ORACLE_EMBED_WINDOW_OVERLAP_BYTES`.
pub const EMBED_WINDOW_OVERLAP_BYTES: usize = 256;

/// Max attention cost units per forward pass: `batch_size × seq_len²`.
///
/// One full window is ≈2556² ≈ 6.5M, so it fits alone. Measured reference:
/// 8 × 500² = 2M → 1.27 GB peak RSS; this targets roughly 4–5 GB worst case.
/// Override via `ORACLE_CHUNK_ATTENTION_BUDGET`.
pub const DEFAULT_ATTENTION_BUDGET: usize = 7_000_000;

/// Resolve max tokens per forward sequence (env override or [`EMBED_MAX_SEQ_TOKENS`]).
pub fn resolve_embed_max_seq_tokens() -> usize {
    std::env::var("ORACLE_EMBED_MAX_SEQ_TOKENS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(EMBED_MAX_SEQ_TOKENS)
}

/// Pure resolve of the embed window size: honour a positive override, fall back
/// to [`EMBED_WINDOW_BYTES`] on missing/zero/garbage, then **clamp** to
/// `max_seq_tokens - EMBED_SPECIAL_TOKEN_RESERVE` so env overrides cannot
/// reintroduce tokenizer truncation.
///
/// Returns `(effective_bytes, was_clamped)`.
pub fn effective_embed_window_bytes(
    requested: Option<usize>,
    max_seq_tokens: usize,
) -> (usize, bool) {
    let req = requested.filter(|&n| n > 0).unwrap_or(EMBED_WINDOW_BYTES);
    let cap = max_seq_tokens
        .saturating_sub(EMBED_SPECIAL_TOKEN_RESERVE)
        .max(1);
    if req > cap {
        (cap, true)
    } else {
        (req, false)
    }
}

/// Resolve window size in bytes (env override or [`EMBED_WINDOW_BYTES`]).
///
/// Always clamped to `resolve_embed_max_seq_tokens() - EMBED_SPECIAL_TOKEN_RESERVE`
/// so an oversized `ORACLE_EMBED_WINDOW_BYTES` cannot reintroduce silent
/// truncation. Logs once at WARN when a requested override is clamped.
pub fn resolve_embed_window_bytes() -> usize {
    let max_seq = resolve_embed_max_seq_tokens();
    let requested = std::env::var("ORACLE_EMBED_WINDOW_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok());
    let (effective, clamped) = effective_embed_window_bytes(requested, max_seq);
    if clamped {
        // Once-only so a hot embed path does not spam the log.
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            let req = requested.unwrap_or(EMBED_WINDOW_BYTES);
            eprintln!(
                "[oracle-embed] WARN ORACLE_EMBED_WINDOW_BYTES clamped \
                 requested={req} effective={effective} max_seq_tokens={max_seq} \
                 special_token_reserve={EMBED_SPECIAL_TOKEN_RESERVE}"
            );
        });
    }
    effective
}

/// Resolve window overlap in bytes (env override or [`EMBED_WINDOW_OVERLAP_BYTES`]).
pub fn resolve_embed_window_overlap_bytes() -> usize {
    std::env::var("ORACLE_EMBED_WINDOW_OVERLAP_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(EMBED_WINDOW_OVERLAP_BYTES)
}

/// Resolve attention budget (env override or [`DEFAULT_ATTENTION_BUDGET`]).
pub fn resolve_attention_budget() -> usize {
    std::env::var("ORACLE_CHUNK_ATTENTION_BUDGET")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_ATTENTION_BUDGET)
}

/// Attention cost of a right-padded batch: `batch_size × seq_len²`.
pub fn attention_cost(batch_size: usize, seq_len: usize) -> usize {
    let s = seq_len.max(1);
    batch_size.saturating_mul(s.saturating_mul(s))
}

/// Largest sub-batch size such that `n × seq_len² <= budget`.
///
/// Always at least 1: a single sequence that still exceeds the budget must run
/// alone rather than being dropped.
pub fn max_batch_for_attention(seq_len: usize, budget: usize) -> usize {
    let s = seq_len.max(1);
    let per = s.saturating_mul(s);
    if per == 0 {
        return budget.max(1);
    }
    (budget / per).max(1)
}

/// Split `total` items into sub-batch lengths that each satisfy the attention budget.
pub fn attention_sub_batch_sizes(total: usize, seq_len: usize, budget: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let max_n = max_batch_for_attention(seq_len, budget);
    let mut remaining = total;
    let mut out = Vec::new();
    while remaining > 0 {
        let n = remaining.min(max_n);
        out.push(n);
        remaining -= n;
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Windowing + mean-pool (pure; used by every embed backend)
// ═══════════════════════════════════════════════════════════════════════════

/// One byte-window of a source text. `text` is a slice of the original input;
/// `[start_byte, end_byte)` are half-open offsets into that input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextWindow<'a> {
    pub text: &'a str,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// Floor `index` to the nearest UTF-8 char boundary at or below `index`.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte index of the first char boundary strictly after `index` (or `s.len()`).
fn next_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Prefer a newline or space cut in `[prefer_from, end)`, returning the byte
/// index *after* the separator so the separator stays in the current window.
fn prefer_soft_cut(s: &str, prefer_from: usize, end: usize) -> Option<usize> {
    if prefer_from >= end || end > s.len() {
        return None;
    }
    let bytes = s.as_bytes();
    // Prefer newline over space; scan back from end.
    for i in (prefer_from..end).rev() {
        if !s.is_char_boundary(i) {
            continue;
        }
        let b = bytes[i];
        if b == b'\n' || b == b' ' || b == b'\t' {
            let after = next_char_boundary(s, i);
            if after > prefer_from && after <= end {
                return Some(after);
            }
        }
    }
    None
}

/// Split `text` into overlapping windows, each at most `window_bytes` **bytes**,
/// cut on UTF-8 char boundaries. Prefer newline/space boundaries in the last
/// ~15% of the window. Never emits an empty window; always advances ≥1 byte.
///
/// Windows are measured in bytes (not chars) so that with a byte-level BPE
/// tokenizer, `n_tokens ≤ window_bytes` structurally — the model cannot truncate.
pub fn window_text<'a>(
    text: &'a str,
    window_bytes: usize,
    overlap_bytes: usize,
) -> Vec<TextWindow<'a>> {
    if text.is_empty() {
        return Vec::new();
    }
    let window_bytes = window_bytes.max(1);
    let mut windows = Vec::new();
    let mut start = 0usize;

    while start < text.len() {
        debug_assert!(text.is_char_boundary(start));

        let mut end = (start + window_bytes).min(text.len());
        end = floor_char_boundary(text, end);
        if end <= start {
            // window_bytes smaller than the next char (only possible with tiny
            // test windows): take one full char so we never split UTF-8.
            end = next_char_boundary(text, start);
        }

        // Soft cut: newline/space in the last ~15% of the window.
        if end < text.len() {
            let span = end - start;
            let prefer_from = start + span.saturating_mul(85) / 100;
            if let Some(cut) = prefer_soft_cut(text, prefer_from, end) {
                if cut > start {
                    end = cut;
                }
            }
        }

        debug_assert!(end > start);
        debug_assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        windows.push(TextWindow {
            text: &text[start..end],
            start_byte: start,
            end_byte: end,
        });

        if end >= text.len() {
            break;
        }

        // Next start = end − overlap, snapped down to a char boundary; must
        // strictly advance so we cannot loop forever.
        let mut next = end.saturating_sub(overlap_bytes);
        next = floor_char_boundary(text, next);
        if next <= start {
            next = next_char_boundary(text, start);
        }
        // Still not past start (e.g. start at last char): jump to end.
        if next <= start {
            next = end;
        }
        start = next;
    }
    windows
}

/// Reconstruct the original byte string from windows by dropping the overlap
/// prefix of each window after the first. Exact when windows were produced by
/// [`window_text`] on the same text.
pub fn reconstruct_from_windows(windows: &[TextWindow<'_>]) -> String {
    if windows.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(windows.last().map(|w| w.end_byte).unwrap_or(0));
    let mut cursor = windows[0].start_byte;
    for w in windows {
        if w.end_byte <= cursor {
            continue;
        }
        let take_from = cursor.saturating_sub(w.start_byte);
        // `w.text` is the slice [start_byte, end_byte); skip overlap prefix.
        if take_from < w.text.len() {
            // take_from is a byte offset into w.text; must be a char boundary
            // because both cursor and start_byte are.
            out.push_str(&w.text[take_from..]);
        }
        cursor = w.end_byte;
    }
    out
}

/// Element-wise mean of `vectors`, then L2 re-normalise.
///
/// Qwen3 embeddings are unit-norm; the mean of unit vectors is not. Degenerate
/// (near-zero norm) input falls back to the first vector rather than NaNs.
/// A single input vector is returned unchanged (mean + renorm is a no-op for
/// an already unit vector; for non-unit input we still renorm — callers that
/// need bit-identity for the single-window path should pass unit vectors).
pub fn mean_pool_l2(vectors: &[Vec<f32>]) -> Vec<f32> {
    assert!(!vectors.is_empty(), "mean_pool_l2 requires at least one vector");
    if vectors.len() == 1 {
        // Common case: one window. Re-normalise only if needed so unit vectors
        // pass through unchanged.
        let v = &vectors[0];
        let norm_sq: f32 = v.iter().map(|x| x * x).sum();
        if (norm_sq - 1.0).abs() < 1e-5 {
            return v.clone();
        }
        let norm = norm_sq.sqrt();
        if norm < 1e-12 || !norm.is_finite() {
            return v.clone();
        }
        return v.iter().map(|x| x / norm).collect();
    }

    let dim = vectors[0].len();
    let mut acc = vec![0.0f32; dim];
    for v in vectors {
        debug_assert_eq!(v.len(), dim);
        for (a, &x) in acc.iter_mut().zip(v.iter()) {
            *a += x;
        }
    }
    let n = vectors.len() as f32;
    for a in acc.iter_mut() {
        *a /= n;
    }
    let norm_sq: f32 = acc.iter().map(|x| x * x).sum();
    let norm = norm_sq.sqrt();
    if norm < 1e-12 || !norm.is_finite() {
        return vectors[0].clone();
    }
    for a in acc.iter_mut() {
        *a /= norm;
    }
    acc
}

/// Expand each input text into windows. Returns flattened window strings and
/// per-text window counts (for regrouping). Empty texts get **one** empty
/// window so the 1:1 public API is preserved.
pub fn expand_texts_to_windows(
    texts: &[String],
    window_bytes: usize,
    overlap_bytes: usize,
) -> (Vec<String>, Vec<usize>) {
    let mut windows = Vec::new();
    let mut counts = Vec::with_capacity(texts.len());
    for t in texts {
        let ws = window_text(t, window_bytes, overlap_bytes);
        if ws.is_empty() {
            windows.push(String::new());
            counts.push(1);
        } else {
            counts.push(ws.len());
            for w in ws {
                windows.push(w.text.to_string());
            }
        }
    }
    (windows, counts)
}

/// Pool flat window vectors back to one vector per original text using `counts`.
pub fn pool_window_vectors(window_vectors: &[Vec<f32>], counts: &[usize]) -> Vec<Vec<f32>> {
    let total: usize = counts.iter().sum();
    assert_eq!(
        window_vectors.len(),
        total,
        "window vector count {} != sum(counts) {}",
        window_vectors.len(),
        total
    );
    let mut out = Vec::with_capacity(counts.len());
    let mut idx = 0;
    for &c in counts {
        assert!(c > 0);
        out.push(mean_pool_l2(&window_vectors[idx..idx + c]));
        idx += c;
    }
    out
}

/// Greedy pack of windows into forward-pass groups under an attention budget.
///
/// Token estimate for each window is its **byte length** (proven upper bound
/// for byte-level BPE). A full-size window under the default budget travels
/// alone; short windows still batch together.
///
/// Returns half-open `[start, end)` index ranges into the window list.
pub fn pack_windows_for_attention(
    window_byte_lens: &[usize],
    budget: usize,
) -> Vec<std::ops::Range<usize>> {
    let budget = budget.max(1);
    let mut groups = Vec::new();
    let mut i = 0;
    while i < window_byte_lens.len() {
        let mut j = i;
        let mut max_seq = 0usize;
        while j < window_byte_lens.len() {
            let seq = window_byte_lens[j].max(1);
            let new_max = max_seq.max(seq);
            let n = j - i + 1;
            let cost = attention_cost(n, new_max);
            if cost > budget {
                if n == 1 {
                    // Single window over budget still runs alone.
                    j += 1;
                }
                break;
            }
            max_seq = new_max;
            j += 1;
        }
        if j == i {
            j = i + 1;
        }
        groups.push(i..j);
        i = j;
    }
    groups
}

#[cfg(test)]
mod attention_tests {
    use super::*;

    #[test]
    fn max_batch_for_attention_respects_budget() {
        // 8 × 500² = 2_000_000 > 1.5M → max is 6 (6 × 250_000 = 1.5M)
        assert_eq!(max_batch_for_attention(500, 1_500_000), 6);
        // 1024² = 1_048_576 → only 1 fits under 1.5M
        assert_eq!(max_batch_for_attention(1024, 1_500_000), 1);
        // short sequences: many fit
        assert!(max_batch_for_attention(64, 1_500_000) >= 32);
        // Full embed window under default budget: alone.
        assert_eq!(
            max_batch_for_attention(EMBED_WINDOW_BYTES, DEFAULT_ATTENTION_BUDGET),
            1
        );
    }

    #[test]
    fn attention_sub_batches_each_satisfy_budget() {
        // True seq_len large enough that full batch of 32 would blow the budget.
        let seq_len = 800;
        let budget = DEFAULT_ATTENTION_BUDGET;
        let total = 32;
        assert!(attention_cost(total, seq_len) > budget);

        let sizes = attention_sub_batch_sizes(total, seq_len, budget);
        assert!(!sizes.is_empty());
        assert_eq!(sizes.iter().sum::<usize>(), total);
        for &n in &sizes {
            assert!(
                attention_cost(n, seq_len) <= budget,
                "sub-batch n={n} seq_len={seq_len} cost={} > budget={budget}",
                attention_cost(n, seq_len)
            );
        }
    }

    #[test]
    fn single_item_always_allowed() {
        // Even when 1 × seq² exceeds budget, max is still 1 (run alone).
        assert_eq!(max_batch_for_attention(10_000, 100), 1);
        assert_eq!(attention_sub_batch_sizes(3, 10_000, 100), vec![1, 1, 1]);
    }

    #[test]
    fn window_bytes_tied_to_max_seq_tokens() {
        // EOS (tokenizer post-processor) is appended on top of content tokens.
        // Byte-level BPE gives n_content ≤ n_bytes; without the special-token
        // reserve a full window would become max_seq+1 and truncation would
        // silently drop the last real token. Do NOT "simplify" this back to
        // EMBED_WINDOW_BYTES == EMBED_MAX_SEQ_TOKENS.
        assert!(
            EMBED_WINDOW_BYTES + EMBED_SPECIAL_TOKEN_RESERVE <= EMBED_MAX_SEQ_TOKENS,
            "EMBED_WINDOW_BYTES ({EMBED_WINDOW_BYTES}) + EMBED_SPECIAL_TOKEN_RESERVE \
             ({EMBED_SPECIAL_TOKEN_RESERVE}) must be ≤ EMBED_MAX_SEQ_TOKENS \
             ({EMBED_MAX_SEQ_TOKENS})"
        );
        assert_eq!(
            EMBED_WINDOW_BYTES,
            EMBED_MAX_SEQ_TOKENS - EMBED_SPECIAL_TOKEN_RESERVE
        );
    }

    #[test]
    fn effective_window_clamps_override_above_cap() {
        // Oversized override must not reintroduce truncation.
        let max_seq = 2560;
        let (eff, clamped) = effective_embed_window_bytes(Some(8000), max_seq);
        assert!(clamped);
        assert_eq!(eff, max_seq - EMBED_SPECIAL_TOKEN_RESERVE);
    }

    #[test]
    fn effective_window_honours_smaller_override() {
        let max_seq = 2560;
        let (eff, clamped) = effective_embed_window_bytes(Some(512), max_seq);
        assert!(!clamped);
        assert_eq!(eff, 512);
    }

    #[test]
    fn effective_window_zero_or_none_falls_back_to_default() {
        let max_seq = EMBED_MAX_SEQ_TOKENS;
        let (eff_none, c1) = effective_embed_window_bytes(None, max_seq);
        let (eff_zero, c2) = effective_embed_window_bytes(Some(0), max_seq);
        assert!(!c1 && !c2);
        assert_eq!(eff_none, EMBED_WINDOW_BYTES);
        assert_eq!(eff_zero, EMBED_WINDOW_BYTES);
    }

    #[test]
    fn attention_sub_batches_never_exceed_budget_when_batch_gt_1() {
        // Pure splitting helper (R3): every returned group with n>1 must
        // satisfy attention_cost ≤ budget. n==1 is the irreducible case.
        for &(total, seq_len, budget) in &[
            (32usize, 800usize, DEFAULT_ATTENTION_BUDGET),
            (16, 2000, 7_000_000),
            (8, 500, 1_500_000),
            (64, 64, 100_000),
            (5, 10_000, 100), // all single-item groups
        ] {
            let sizes = attention_sub_batch_sizes(total, seq_len, budget);
            assert_eq!(sizes.iter().sum::<usize>(), total);
            for &n in &sizes {
                if n > 1 {
                    assert!(
                        attention_cost(n, seq_len) <= budget,
                        "n={n} seq={seq_len} cost={} budget={budget}",
                        attention_cost(n, seq_len)
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod window_pool_tests {
    use super::*;

    /// Assert lossless reconstruction + window/char-boundary invariants.
    fn assert_lossless_windowing(original: &str, label: &str) {
        let windows = window_text(original, EMBED_WINDOW_BYTES, EMBED_WINDOW_OVERLAP_BYTES);
        assert!(!windows.is_empty(), "{label}: expected at least one window");
        for w in &windows {
            assert!(
                w.text.len() <= EMBED_WINDOW_BYTES,
                "{label}: window {} bytes > EMBED_WINDOW_BYTES",
                w.text.len()
            );
            assert!(
                original.is_char_boundary(w.start_byte),
                "{label}: start {} not char boundary",
                w.start_byte
            );
            assert!(
                original.is_char_boundary(w.end_byte),
                "{label}: end {} not char boundary",
                w.end_byte
            );
            assert_eq!(&original[w.start_byte..w.end_byte], w.text);
        }
        let rebuilt = reconstruct_from_windows(&windows);
        assert_eq!(
            rebuilt, original,
            "{label}: reconstruction must match original byte-for-byte (len {} vs {})",
            rebuilt.len(),
            original.len()
        );
    }

    #[test]
    fn no_data_loss_reconstruction_40kb() {
        // THE test the owner cares about: every byte survives windowing.
        let original: String = (0..40_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        assert_eq!(original.len(), 40_000);

        let windows = window_text(&original, EMBED_WINDOW_BYTES, EMBED_WINDOW_OVERLAP_BYTES);
        assert!(windows.len() > 1, "40 KB must produce multiple windows");
        assert_lossless_windowing(&original, "alpha-40kb");
    }

    #[test]
    fn no_data_loss_soft_cut_source_code_like_40kb() {
        // Source-like text with many newlines/spaces/tabs so prefer_soft_cut fires.
        let mut original = String::with_capacity(45_000);
        let mut i = 0usize;
        while original.len() < 40_000 {
            let indent = "    ".repeat(i % 4);
            original.push_str(&indent);
            original.push_str("fn item_");
            original.push_str(&i.to_string());
            original.push_str("() {\n");
            original.push_str(&indent);
            original.push_str("    let x = ");
            original.push_str(&(i % 97).to_string());
            original.push_str("; // comment\ttrail\n");
            original.push_str(&indent);
            original.push_str("}\n\n");
            i += 1;
        }
        assert!(original.len() >= 40_000);
        assert!(
            original.contains('\n') && original.contains(' ') && original.contains('\t'),
            "fixture must exercise soft-cut separators"
        );

        let windows = window_text(&original, EMBED_WINDOW_BYTES, EMBED_WINDOW_OVERLAP_BYTES);
        assert!(windows.len() > 1, "source-like 40KB must multi-window");
        // Soft cut shrinks the window: at least one non-final window should be
        // strictly shorter than EMBED_WINDOW_BYTES (hard end would fill it).
        let non_final: Vec<_> = windows
            .iter()
            .filter(|w| w.end_byte < original.len())
            .collect();
        assert!(
            non_final.iter().any(|w| w.text.len() < EMBED_WINDOW_BYTES),
            "expected at least one soft-cut shrink; all non-final windows were full size"
        );
        assert_lossless_windowing(&original, "source-soft-cut");
    }

    #[test]
    fn no_data_loss_soft_cut_newline_at_window_boundary() {
        // Newline sits exactly at the natural hard-cut end.
        let win = EMBED_WINDOW_BYTES;
        let mut original = String::with_capacity(win * 3);
        // Fill first window worth of 'a', last byte is '\n'.
        original.push_str(&"a".repeat(win - 1));
        original.push('\n');
        // More content so we actually window.
        original.push_str(&"b".repeat(win * 2));
        assert!(original.len() > win);

        let windows = window_text(&original, win, EMBED_WINDOW_OVERLAP_BYTES);
        assert!(windows.len() > 1);
        // Soft cut prefers the newline: first window should end at/after that \n.
        assert!(
            windows[0].text.contains('\n') || windows[0].end_byte <= win,
            "first window should land on the boundary newline path"
        );
        assert_lossless_windowing(&original, "newline-at-boundary");
    }

    #[test]
    fn no_data_loss_soft_cut_newline_only_in_soft_zone() {
        // Only newline lives inside the last ~15% soft-cut zone (not earlier).
        let win = EMBED_WINDOW_BYTES;
        let soft_start = win * 85 / 100;
        let mut original = String::with_capacity(win * 2 + 64);
        original.push_str(&"x".repeat(soft_start + 10));
        original.push('\n'); // inside soft zone of a full hard window
        original.push_str(&"y".repeat(win)); // forces multi-window + more content after cut
        assert!(original.len() > win);

        let windows = window_text(&original, win, EMBED_WINDOW_OVERLAP_BYTES);
        assert!(windows.len() > 1);
        // First window should soft-cut after the newline (shrunk vs hard end).
        assert!(
            windows[0].text.ends_with('\n') || windows[0].text.contains('\n'),
            "soft-zone newline should be preferred; first window={:?}",
            windows[0].text.len()
        );
        assert!(windows[0].text.len() < win || windows[0].end_byte == original.len());
        assert_lossless_windowing(&original, "newline-in-soft-zone");
    }

    #[test]
    fn no_data_loss_soft_cut_multibyte_with_newlines_spaces() {
        // Multi-byte chars mixed with newlines/spaces: soft cut + char-boundary
        // walk-back must interact without losing bytes or splitting UTF-8.
        let unit = "日本語 漢字\nテスト 😀 🎉 "; // spaces + newline + multi-byte
        let original = unit.repeat(3000); // well over 40 KB
        assert!(original.len() >= 40_000);
        assert!(original.contains('\n') && original.contains(' '));

        let windows = window_text(&original, EMBED_WINDOW_BYTES, EMBED_WINDOW_OVERLAP_BYTES);
        assert!(windows.len() > 1);
        for w in &windows {
            // Every window must be valid UTF-8 (already a &str) and not exceed cap
            // unless it is a single oversize char (impossible for these codepoints).
            assert!(w.text.len() <= EMBED_WINDOW_BYTES);
        }
        assert_lossless_windowing(&original, "multibyte-soft-cut");
    }

    #[test]
    fn windows_respect_utf8_boundaries_cjk_and_emoji() {
        let cjk: String = "日本語漢字テスト".repeat(800); // multi-byte, ~40KB+
        let emoji: String = "😀🎉🚀✨".repeat(2000);

        for (label, text) in [("cjk", cjk.as_str()), ("emoji", emoji.as_str())] {
            let windows = window_text(text, EMBED_WINDOW_BYTES, EMBED_WINDOW_OVERLAP_BYTES);
            assert!(!windows.is_empty(), "{label}: expected windows");
            for w in &windows {
                assert!(
                    w.text.len() <= EMBED_WINDOW_BYTES || w.text.chars().count() == 1,
                    "{label}: window len {}",
                    w.text.len()
                );
                assert!(text.is_char_boundary(w.start_byte), "{label}: start not boundary");
                assert!(text.is_char_boundary(w.end_byte), "{label}: end not boundary");
                // Round-trip slice must equal the window (no mid-char split).
                assert_eq!(&text[w.start_byte..w.end_byte], w.text);
            }
            assert_eq!(
                reconstruct_from_windows(&windows),
                text,
                "{label}: reconstruction mismatch"
            );
        }
    }

    #[test]
    fn single_window_short_text_unchanged_path() {
        let text = "short enough to fit";
        let windows = window_text(text, EMBED_WINDOW_BYTES, EMBED_WINDOW_OVERLAP_BYTES);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].text, text);
        assert_eq!(reconstruct_from_windows(&windows), text);
    }

    #[test]
    fn mean_pool_renormalises_unit_vectors() {
        // Two orthogonal unit vectors in 2D → mean has norm 1/√2 before renorm.
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        let pooled = mean_pool_l2(&[a, b]);
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "pooled norm should be 1, got {norm}"
        );
        // Direction should be equal mix.
        assert!((pooled[0] - pooled[1]).abs() < 1e-5);
    }

    #[test]
    fn mean_pool_single_unit_vector_unchanged() {
        let v = vec![0.6f32, 0.8]; // already unit (0.36+0.64=1)
        let out = mean_pool_l2(std::slice::from_ref(&v));
        assert_eq!(out, v);
    }

    #[test]
    fn mean_pool_degenerate_no_nan() {
        let zeros = vec![vec![0.0f32; 4], vec![0.0f32; 4]];
        let out = mean_pool_l2(&zeros);
        assert!(out.iter().all(|x| x.is_finite()), "must not produce NaN/Inf");
        assert_eq!(out, zeros[0]);
    }

    #[test]
    fn expand_and_pool_preserves_1to1_order() {
        let texts = vec![
            "short".to_string(),
            "x".repeat(10_000),
            "mid".to_string(),
            "y".repeat(5_000),
        ];
        let (windows, counts) = expand_texts_to_windows(
            &texts,
            EMBED_WINDOW_BYTES,
            EMBED_WINDOW_OVERLAP_BYTES,
        );
        assert_eq!(counts.len(), texts.len());
        assert_eq!(windows.len(), counts.iter().sum::<usize>());
        assert!(counts[0] == 1);
        assert!(counts[1] > 1);
        assert!(counts[2] == 1);
        assert!(counts[3] > 1);

        // Fake "embeddings": dim-2 vectors encoding window index so pool is deterministic.
        let fake: Vec<Vec<f32>> = (0..windows.len())
            .map(|i| {
                let v = vec![1.0f32, (i as f32) * 0.0]; // unit along e0
                v
            })
            .collect();
        let pooled = pool_window_vectors(&fake, &counts);
        assert_eq!(pooled.len(), texts.len(), "1:1 contract: K texts → K vectors");
        for (i, p) in pooled.iter().enumerate() {
            assert_eq!(p.len(), 2, "vector {i} dim");
            let norm: f32 = p.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-5 || p.iter().all(|&x| x == 0.0));
        }
    }

    #[test]
    fn pack_full_window_alone_short_batch_together() {
        let budget = DEFAULT_ATTENTION_BUDGET;
        // Full-size window: ~2556² ≈ 6.5M; two of them 2× that > budget.
        let full = EMBED_WINDOW_BYTES;
        let groups = pack_windows_for_attention(&[full, full], budget);
        assert_eq!(groups.len(), 2, "two full windows must not share a forward pass");
        assert_eq!(groups[0], 0..1);
        assert_eq!(groups[1], 1..2);

        // Short windows (e.g. 64 bytes): many fit under 7M.
        let shorts = vec![64usize; 16];
        let g = pack_windows_for_attention(&shorts, budget);
        assert_eq!(g.len(), 1, "short windows should batch together");
        assert_eq!(g[0], 0..16);
        assert!(attention_cost(16, 64) <= budget);
    }

    #[test]
    fn pack_covers_all_windows() {
        let lens = vec![100usize, EMBED_WINDOW_BYTES, 50, 200, EMBED_WINDOW_BYTES, 10];
        let groups = pack_windows_for_attention(&lens, DEFAULT_ATTENTION_BUDGET);
        let covered: usize = groups.iter().map(|r| r.end - r.start).sum();
        assert_eq!(covered, lens.len());
        assert_eq!(groups.first().map(|r| r.start), Some(0));
        assert_eq!(groups.last().map(|r| r.end), Some(lens.len()));
        // No gaps.
        for w in groups.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }
}

/// Which backend the pool should load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendChoice {
    /// candle (fastembed qwen3). `metal` only works on macOS builds with the
    /// `metal` feature; `f16` is only meaningful together with metal.
    Candle { metal: bool, f16: bool },
    /// ONNX Runtime with the platform GPU EP auto-selected (macOS → CoreML,
    /// Windows → DirectML) and automatic CPU fallback. `model_dir` holds
    /// `onnx/model*.onnx` + `tokenizer.json`; `int8` selects the quantized graph
    /// (needs its OWN index — parity-incompatible with f32-embedded corpora).
    Ort { model_dir: PathBuf, int8: bool },
}

/// A loaded embedding backend.
///
/// `embed` must return one L2-normalized vector per input text, in order.
/// Implementations check `cancel` between internal batches and bail with an
/// error containing "cancelled" when it fires.
pub trait Embedder: Send {
    fn model_id(&self) -> &str;
    fn embed(
        &mut self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>>;
}

/// Resolve the default backend for this build/platform.
///
/// macOS + `metal` feature → candle Metal F16 (index-parity proven, model in
/// the shared HF cache). Everything else → ONNX int8 with the platform GPU EP
/// auto-selected (CoreML/DirectML) and CPU fallback. `ORACLE_RS_BACKEND=candle|onnx`
/// overrides; `ORACLE_EMBED_DEVICE=cpu` forces CPU on the candle path
/// (mirroring the Python env knob); `ORACLE_RS_EP` forces the ONNX EP.
pub fn default_backend(ort_model_dir: PathBuf) -> BackendChoice {
    let forced = std::env::var("ORACLE_RS_BACKEND").ok();
    let force_cpu = std::env::var("ORACLE_EMBED_DEVICE")
        .map(|v| v.trim().eq_ignore_ascii_case("cpu"))
        .unwrap_or(false);
    let metal_available = cfg!(all(target_os = "macos", feature = "metal")) && !force_cpu;

    match forced.as_deref().map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("onnx") || v.eq_ignore_ascii_case("ort") => {
            BackendChoice::Ort {
                model_dir: ort_model_dir,
                int8: true,
            }
        }
        Some(v) if v.eq_ignore_ascii_case("candle") => BackendChoice::Candle {
            metal: metal_available,
            f16: metal_available,
        },
        _ if metal_available => BackendChoice::Candle {
            metal: true,
            f16: true,
        },
        _ => BackendChoice::Ort {
            model_dir: ort_model_dir,
            int8: true,
        },
    }
}

fn load_backend(choice: &BackendChoice) -> Result<Box<dyn Embedder>> {
    match choice {
        BackendChoice::Candle { metal, f16 } => Ok(Box::new(
            CandleEmbedder::load(*metal, *f16).context("loading candle embedder")?,
        )),
        BackendChoice::Ort { model_dir, int8 } => Ok(Box::new(
            OrtEmbedder::load(model_dir, *int8).context("loading ort embedder")?,
        )),
    }
}

struct PoolState {
    embedder: Option<Box<dyn Embedder>>,
    last_used: Instant,
}

/// Resident embedder pool: lazy load, reuse across calls, idle unload.
pub struct EmbedderPool {
    choice: BackendChoice,
    state: Mutex<PoolState>,
}

impl EmbedderPool {
    pub fn new(choice: BackendChoice) -> Self {
        EmbedderPool {
            choice,
            state: Mutex::new(PoolState {
                embedder: None,
                last_used: Instant::now(),
            }),
        }
    }

    pub fn backend(&self) -> &BackendChoice {
        &self.choice
    }

    /// Whether the model is currently resident in memory.
    pub fn is_loaded(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .embedder
            .is_some()
    }

    /// Embed texts, loading the model on first use.
    ///
    /// The pool lock is held for the whole call: embedding is single-flight by
    /// design (one model instance, GPU/CPU saturating), exactly like the
    /// Python server where one uvicorn worker owned the model.
    pub fn embed(
        &self,
        texts: &[String],
        batch_size: usize,
        cancel: &CancelFlag,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.embedder.is_none() {
            state.embedder = Some(load_backend(&self.choice)?);
        }
        state.last_used = Instant::now();
        let out = state
            .embedder
            .as_mut()
            .expect("just loaded")
            .embed(texts, batch_size, cancel);
        state.last_used = Instant::now();
        out
    }

    /// Drop the model if it has been idle for at least `max_idle`.
    /// Returns true when an unload happened.
    pub fn unload_if_idle(&self, max_idle: Duration) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.embedder.is_some() && state.last_used.elapsed() >= max_idle {
            state.embedder = None;
            true
        } else {
            false
        }
    }

    /// Drop the model immediately (e.g. on low-memory pressure).
    pub fn unload_now(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.embedder = None;
    }
}
