//! EmbedderPool lifecycle smoke (operator-run: needs the local ONNX model).

use oracle_core::embed::{BackendChoice, CancelFlag, EmbedderPool};
use std::path::PathBuf;
use std::time::Duration;

fn spike_model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/qwen3-onnx")
}

#[test]
#[ignore]
fn pool_load_embed_unload_reload() {
    let pool = EmbedderPool::new(BackendChoice::Ort {
        model_dir: spike_model_dir(),
        int8: false,
    });
    assert!(!pool.is_loaded());

    let texts = vec![
        "fn main() { println!(\"hello\"); }".to_string(),
        "The Oracle indexes the workspace incrementally.".to_string(),
        "SELECT id FROM file_chunks ORDER BY chunk_index".to_string(),
    ];
    let cancel = CancelFlag::new();
    let vectors = pool.embed(&texts, 8, &cancel).unwrap();
    assert_eq!(vectors.len(), 3);
    assert_eq!(vectors[0].len(), 1024);
    for v in &vectors {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "not L2-normalized: {norm}");
    }
    assert!(pool.is_loaded());

    // Not idle long enough -> stays resident; zero idle -> unloads.
    assert!(!pool.unload_if_idle(Duration::from_secs(3600)));
    assert!(pool.is_loaded());
    assert!(pool.unload_if_idle(Duration::ZERO));
    assert!(!pool.is_loaded());

    // Reload on demand still works after an unload.
    let again = pool.embed(&texts[..1], 8, &cancel).unwrap();
    assert_eq!(again.len(), 1);

    // Pre-cancelled flag fails fast with a "cancelled" error.
    let cancelled = CancelFlag::new();
    cancelled.cancel();
    let err = pool.embed(&texts, 1, &cancelled).unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err}");
}
