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

/// Hard ceiling on a single downloaded file. The largest legitimate artifact
/// (the Qwen3 ONNX model) is well under this; anything bigger means a wrong
/// or hostile server, and we must not fill the disk. Restores the bound lost
/// when the Content-Length requirement was relaxed for HF's HEAD quirk.
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024; // 8 GiB

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

/// Effective per-file byte cap for a download. When the remote length IS
/// known (and non-zero) it is honored only up to the hard
/// [`MAX_DOWNLOAD_BYTES`] ceiling — an announced length above the ceiling is
/// clamped, so a hostile server announcing a huge Content-Length can never
/// raise the cap above the disk-safety bound. When the length is unknown
/// (HF's HEAD quirk) the cap is the hard [`MAX_DOWNLOAD_BYTES`] ceiling.
fn effective_download_cap(bytes_total: Option<u64>) -> u64 {
    bytes_total
        .filter(|&t| t > 0)
        .map(|t| t.min(MAX_DOWNLOAD_BYTES))
        .unwrap_or(MAX_DOWNLOAD_BYTES)
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
    // Some CDNs (HuggingFace) report Content-Length: 0 on HEAD requests for
    // redirect-to-CDN URLs. A 0 length is not a real model size — treat it as
    // "unknown" so the caller skips the exact-size guard instead of rejecting
    // a fully-downloaded file.
    resp.content_length().filter(|&len| len > 0)
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
    let cap = effective_download_cap(bytes_total);
    loop {
        let n = resp.read(&mut buf).context("reading download stream")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("writing model file")?;
        done += n as u64;
        if done > cap {
            let _ = std::fs::remove_file(&part);
            bail!(
                "download of {} exceeded the {} byte cap (got {done} bytes) — refusing to write more",
                dest.display(),
                cap
            );
        }
        progress(done);
    }
    file.flush().ok();
    drop(file);

    if let Some(expected) = bytes_total {
        if expected > 0 {
            let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if got != expected {
                let _ = std::fs::remove_file(&part);
                bail!(
                    "size mismatch for {}: got {got} bytes, expected {expected}",
                    dest.display()
                );
            }
        }
    }

    std::fs::rename(&part, dest)
        .with_context(|| format!("finalizing {}", dest.display()))?;
    Ok(())
}

/// Clear a broken model-dir symlink so downloads can create a real directory.
/// Real directories and working symlinks are left alone.
///
/// Public so the app installer can clear a dangling link before seeding a
/// bundled ONNX tree into the writable data dir.
pub fn clear_broken_model_dir_symlink(dir: &Path) -> Result<()> {
    let meta = match dir.symlink_metadata() {
        Ok(m) => m,
        Err(_) => return Ok(()), // nothing there
    };
    if meta.file_type().is_symlink() && !dir.exists() {
        std::fs::remove_file(dir).with_context(|| {
            format!("removing broken model-dir symlink {}", dir.display())
        })?;
    }
    Ok(())
}

/// Copy the int8 ONNX bundle files from `src_model_dir` into `dest_model_dir`.
///
/// Used by the full package installer to seed app-data from the read-only
/// bundle without re-downloading. Destination parents are created; a broken
/// dest-dir symlink is cleared first. Only the int8 file set is copied
/// (`onnx/model_int8.onnx` + `tokenizer.json`).
pub fn copy_int8_bundle(src_model_dir: &Path, dest_model_dir: &Path) -> Result<()> {
    if !model_present_at(src_model_dir, true) {
        bail!(
            "source int8 bundle incomplete at {}",
            src_model_dir.display()
        );
    }
    clear_broken_model_dir_symlink(dest_model_dir)?;
    for rel in INT8_FILES {
        let src = src_model_dir.join(rel);
        let dest = dest_model_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // Copy via a sibling `.part` then rename so a crash never leaves a
        // truncated file that `model_present` would treat as complete.
        let part = dest.with_extension(match dest.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("{ext}.part"),
            None => "part".to_string(),
        });
        std::fs::copy(&src, &part)
            .with_context(|| format!("copying {} → {}", src.display(), part.display()))?;
        std::fs::rename(&part, &dest)
            .with_context(|| format!("finalizing {}", dest.display()))?;
    }
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
    // A broken symlink at the model dir (common after Aspis→Devboule renames
    // that left `qwen3-onnx -> …/aspis-management/…`) makes create_dir_all fail
    // with "File exists" and the whole install never downloads. Clear it.
    clear_broken_model_dir_symlink(&dir)?;
    let files = bundle_files(int8);
    let client = http_client()?;

    for (i, rel) in files.iter().enumerate() {
        let url = format!("{HF_BASE}/{rel}");
        let dest = dir.join(rel);
        let bytes_total = remote_len(&client, &url);

        // Content-Length may be None when the CDN reports 0 on HEAD requests
        // (HuggingFace quirk). Proceed without the exact-size guard in that
        // case — the download reads to EOF and writes to a .part file, so an
        // unknown length is safe; we just can't verify the post-download size.

        // Skip if the local file already matches the remote size exactly.
        // (Only when we have a real, non-zero remote length.)
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
    fn clear_broken_model_dir_symlink_removes_dangling_link() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = model_dir(tmp.path());
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/nonexistent/oracle-qwen3-target", &dir).unwrap();
            assert!(dir.symlink_metadata().unwrap().file_type().is_symlink());
            assert!(!dir.exists());
            clear_broken_model_dir_symlink(&dir).unwrap();
            assert!(!dir.symlink_metadata().is_ok() || !dir.symlink_metadata().unwrap().file_type().is_symlink());
            // After clear, a real dir can be created.
            std::fs::create_dir_all(&dir).unwrap();
            assert!(dir.is_dir());
        }
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

    #[test]
    fn copy_int8_bundle_seeds_dest_from_complete_source() {
        let src_root = tempfile::tempdir().unwrap();
        let dest_root = tempfile::tempdir().unwrap();
        let src = model_dir(src_root.path());
        let dest = model_dir(dest_root.path());
        let payload = vec![0xCDu8; 2048];
        for rel in INT8_FILES {
            let p = src.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &payload).unwrap();
        }
        assert!(!model_present_at(&dest, true));
        copy_int8_bundle(&src, &dest).unwrap();
        assert!(model_present_at(&dest, true));
        for rel in INT8_FILES {
            assert_eq!(
                std::fs::read(dest.join(rel)).unwrap(),
                payload,
                "copied {rel} must match source bytes"
            );
        }
    }

    #[test]
    fn effective_download_cap_uses_remote_len_or_hard_ceiling() {
        assert_eq!(effective_download_cap(None), MAX_DOWNLOAD_BYTES);
        assert_eq!(effective_download_cap(Some(0)), MAX_DOWNLOAD_BYTES);
        assert_eq!(effective_download_cap(Some(1234)), 1234);
        assert_eq!(effective_download_cap(Some(u64::MAX)), MAX_DOWNLOAD_BYTES);
        assert!(effective_download_cap(Some(MAX_DOWNLOAD_BYTES + 1)) <= MAX_DOWNLOAD_BYTES);
    }

    #[test]
    fn copy_int8_bundle_rejects_incomplete_source() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        // Only tokenizer — missing model_int8.onnx.
        std::fs::write(src.path().join("tokenizer.json"), vec![0u8; 2048]).unwrap();
        let err = copy_int8_bundle(src.path(), dest.path()).unwrap_err();
        assert!(
            err.to_string().contains("incomplete"),
            "expected incomplete-source error, got: {err:#}"
        );
    }
}
