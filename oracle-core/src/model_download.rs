//! Downloads the Qwen3-Embedding-0.6B ONNX bundle for the `ort` backend.
//!
//! This is the Rust engine's replacement for the Python venv + pip + warmup
//! flow: instead of installing a Python runtime that pulls the model into the
//! HF cache, we fetch the ONNX export directly into the oracle-data tree at the
//! layout `OrtEmbedder::load` expects:
//!   <oracle_data_root>/models/qwen3-onnx/onnx/model.onnx        (fp32 graph)
//!   <oracle_data_root>/models/qwen3-onnx/onnx/model.onnx_data   (fp32 weights)
//!   <oracle_data_root>/models/qwen3-onnx/tokenizer.json
//!
//! fp32 is the parity-proven bundle (cosine 0.9998 vs the Python stack, index
//! reusable). int8 is a smaller, single-file graph but parity-INCOMPATIBLE, so
//! it is a separate opt-in bundle that must own its own index.
//!
//! Downloads stream to a `.part` file and are atomically renamed on success, so
//! an interrupted download never leaves a truncated file that looks complete.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::embed::ort_backend::OrtEmbedder;

/// HuggingFace resolve base for the onnx-community Qwen3 export.
const HF_BASE: &str =
    "https://huggingface.co/onnx-community/Qwen3-Embedding-0.6B-ONNX/resolve/main";

/// Repo-relative files for the parity-proven fp32 bundle.
const FP32_FILES: &[&str] = &["onnx/model.onnx", "onnx/model.onnx_data", "tokenizer.json"];

/// Repo-relative files for the int8 bundle (single graph, no external data).
const INT8_FILES: &[&str] = &["onnx/model_int8.onnx", "tokenizer.json"];

/// Progress for a single file within the bundle.
#[derive(Debug, Clone)]
pub struct FileProgress {
    /// Repo-relative path currently transferring (e.g. `onnx/model.onnx_data`).
    pub file: String,
    /// 1-based index of this file in the bundle.
    pub index: usize,
    /// Total files in the bundle.
    pub total_files: usize,
    /// Bytes written so far for this file.
    pub bytes_done: u64,
    /// Total bytes for this file (from Content-Length), or `None` if unknown.
    pub bytes_total: Option<u64>,
}

fn bundle_files(int8: bool) -> &'static [&'static str] {
    if int8 {
        INT8_FILES
    } else {
        FP32_FILES
    }
}

/// The on-disk model directory for the given oracle-data root.
pub fn model_dir(oracle_data_root: &Path) -> PathBuf {
    OrtEmbedder::default_model_dir(oracle_data_root)
}

/// True when every required bundle file exists (and is non-trivially sized)
/// directly under `model_dir` (the resolved qwen3-onnx dir, NOT the data root).
///
/// This is the building block behind [`model_present`]; callers that have
/// already resolved an explicit model directory (e.g. `ORACLE_MODEL_DIR`)
/// should check *that* path rather than recomputing the default layout from a
/// data root, which would inspect the wrong location.
pub fn model_present_at(model_dir: &Path, int8: bool) -> bool {
    bundle_files(int8).iter().all(|rel| {
        let p = model_dir.join(rel);
        std::fs::metadata(&p).map(|m| m.len() > 1024).unwrap_or(false)
    })
}

/// True when every file of the requested bundle is present AND above a minimal
/// plausible size. UI-status only — never use this to SKIP `ensure_qwen3_onnx`
/// (that path does its own Content-Length verification); a planted 1-byte file
/// must not read as "installed" enough to bypass the download.
///
/// Equivalent to `model_present_at(&model_dir(root), int8)`.
pub fn model_present(oracle_data_root: &Path, int8: bool) -> bool {
    model_present_at(&model_dir(oracle_data_root), int8)
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        // Large weights on a slow link: no overall timeout, but a generous
        // connect timeout so a dead host fails fast instead of hanging forever.
        .connect_timeout(Duration::from_secs(30))
        // Allow cross-host redirects (HF resolve URLs legitimately redirect to
        // a CDN) but refuse any non-HTTPS hop — HTTPS→HTTP downgrade would
        // enable MITM model injection / cleartext model delivery.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 10 {
                return attempt.error("too many redirects");
            }
            if attempt.url().scheme() != "https" {
                return attempt.error("refusing non-https redirect for model download");
            }
            attempt.follow()
        }))
        .build()
        .context("building HTTP client for model download")
}

/// Remote size via a HEAD request, or `None` when the server omits it.
fn remote_len(client: &reqwest::blocking::Client, url: &str) -> Option<u64> {
    let resp = client.head(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.content_length()
}

/// Stream one file to `dest`, calling `progress` as bytes arrive. Writes to a
/// sibling `.part` file and renames on success (atomic within the same dir).
fn download_file(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    bytes_total: Option<u64>,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let part = dest.with_extension(match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.part"),
        None => "part".to_string(),
    });

    let mut resp = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} -> HTTP {}", resp.status());
    }

    let mut file = std::fs::File::create(&part)
        .with_context(|| format!("creating {}", part.display()))?;
    let mut buf = vec![0u8; 1 << 20]; // 1 MiB
    let mut done: u64 = 0;
    loop {
        let n = resp.read(&mut buf).context("reading download stream")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("writing model file")?;
        done += n as u64;
        progress(done);
    }
    file.flush().ok();
    drop(file);

    if let Some(expected) = bytes_total {
        let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if got != expected {
            let _ = std::fs::remove_file(&part);
            bail!(
                "size mismatch for {}: got {got} bytes, expected {expected}",
                dest.display()
            );
        }
    }

    std::fs::rename(&part, dest)
        .with_context(|| format!("finalizing {}", dest.display()))?;
    Ok(())
}

/// Ensure the requested ONNX bundle is present under `oracle_data_root`,
/// downloading any missing/mismatched file. Returns the model directory to hand
/// to `BackendChoice::Ort { model_dir, .. }`.
///
/// A file already at its full remote size is skipped, so re-running after a
/// completed install is a cheap set of HEAD requests. `progress` is invoked per
/// received chunk; pass a no-op closure to ignore it.
pub fn ensure_qwen3_onnx(
    oracle_data_root: &Path,
    int8: bool,
    mut progress: impl FnMut(FileProgress),
) -> Result<PathBuf> {
    let dir = model_dir(oracle_data_root);
    let files = bundle_files(int8);
    let client = http_client()?;

    for (i, rel) in files.iter().enumerate() {
        let url = format!("{HF_BASE}/{rel}");
        let dest = dir.join(rel);
        let bytes_total = remote_len(&client, &url);

        // Refuse to download without a Content-Length — HF always sends it,
        // so a missing value means an unexpected/untrusted server.  An unknown
        // remote length would bypass the exact-size guard and could allow an
        // unbounded write (e.g. a planted large payload).
        let bytes_total = match bytes_total {
            Some(len) => Some(len),
            None => bail!(
                "refusing model download for {rel}: server did not report a Content-Length"
            ),
        };

        // Skip if the local file already matches the remote size exactly.
        if let Some(expected) = bytes_total {
            if std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) == expected && expected > 0 {
                progress(FileProgress {
                    file: (*rel).to_string(),
                    index: i + 1,
                    total_files: files.len(),
                    bytes_done: expected,
                    bytes_total: Some(expected),
                });
                continue;
            }
        }

        let rel_owned = (*rel).to_string();
        let files_len = files.len();
        download_file(&client, &url, &dest, bytes_total, |done| {
            progress(FileProgress {
                file: rel_owned.clone(),
                index: i + 1,
                total_files: files_len,
                bytes_done: done,
                bytes_total,
            });
        })
        .with_context(|| format!("downloading {rel}"))?;
    }

    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_dir_layout_matches_ort_backend() {
        let root = Path::new("/tmp/oracle-data");
        assert_eq!(
            model_dir(root),
            root.join("models").join("qwen3-onnx"),
            "must match OrtEmbedder::default_model_dir so the backend finds it"
        );
    }

    #[test]
    fn fp32_bundle_lists_graph_weights_and_tokenizer() {
        assert_eq!(
            bundle_files(false),
            &["onnx/model.onnx", "onnx/model.onnx_data", "tokenizer.json"]
        );
        // int8 is a single graph file (no external _data) + tokenizer.
        assert_eq!(bundle_files(true), &["onnx/model_int8.onnx", "tokenizer.json"]);
    }

    #[test]
    fn model_present_false_on_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!model_present(tmp.path(), false));
    }

    #[test]
    fn model_present_true_when_all_files_large_enough() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = model_dir(tmp.path());
        // Files must exceed 1024 bytes to count as present (UI-status guard
        // against planted tiny files; see model_present doc).
        let payload = vec![0xABu8; 2048];
        for rel in FP32_FILES {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &payload).unwrap();
        }
        assert!(model_present(tmp.path(), false));
        // A zero-byte file must NOT count as present.
        std::fs::write(dir.join("tokenizer.json"), b"").unwrap();
        assert!(!model_present(tmp.path(), false));
    }
}
