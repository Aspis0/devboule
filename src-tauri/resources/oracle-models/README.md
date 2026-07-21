# Oracle embedder package (full vs lite)

Staging root for the Qwen3 Embedding 0.6B **ONNX int8** weights used by the
in-app Rust retrieval engine.

| Kind | Contents | First install |
|------|----------|---------------|
| **lite** (default) | `.bundle-kind` only | Downloads `model_int8.onnx` + `tokenizer.json` from HuggingFace (~600 MB) |
| **full** | + `qwen3-onnx/onnx/model_int8.onnx` and `qwen3-onnx/tokenizer.json` | Seeds from the app bundle (no download) |

## Staging

```bash
# Lite (default) — small package
bash scripts/stage-oracle-embedder.sh --lite

# Full — ships weights (requires local copy under oracle-data/models/qwen3-onnx
# or a HuggingFace download at stage time)
bash scripts/stage-oracle-embedder.sh --full
```

`tauri build` runs staging via `beforeBuildCommand`:

- **Default (safe):** always stages **lite** (`--lite`).
- **Full package:** set `DEVBOULE_BUNDLE_ORACLE_EMBEDDER=1` so beforeBuild
  passes `--full` instead. Only set this env when you intentionally want a
  full release (~600 MB extra in the app bundle).

```bash
# Full release recipe (explicit env on purpose)
DEVBOULE_BUNDLE_ORACLE_EMBEDDER=1 cargo tauri build
# or npm/tauri equivalent with the same env
```

Size integrity: the stage script pins expected byte sizes for
`model_int8.onnx` and `tokenizer.json`. A size mismatch fails the stage
closed (re-pin constants in `scripts/stage-oracle-embedder.sh` after an
intentional HF model update). Downloads write to `.part` then rename.

The large `qwen3-onnx/` tree is gitignored — never commit the ~600 MB weights.
