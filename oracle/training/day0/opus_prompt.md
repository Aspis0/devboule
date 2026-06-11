# Task: Day 0 Pipeline — Qwen3.6-27B Pruning + Recovery Training

## Context

I'm building **Devboule**, a local-first AI coding agent orchestration platform (Tauri/Rust backend, React frontend, Apache 2.0). The system uses **Claude Code** as the main orchestrator, which delegates to a local **mini coder** model for implementation work. Claude Code then reviews and corrects errors via a component called **Censor**.

I have an **Apprenticeship Pipeline** already built that:
- Intercepts Censor's corrections and logs `(prompt, rejected_code, chosen_code)` pairs (**Scribe**)
- Clusters similar errors using cosine similarity (**cluster.py**)
- Runs nightly ORPO LoRA training when a cluster hits 30 pairs (**night_run.py**)
- Benchmarks before/after training with Pass@1 on 50 gold standard tasks (**night_run.py**)
- Promotes or rolls back the new LoRA adapter (**night_run.py**)

The pipeline is already working. What I need now is the **Day 0 setup** that prepares the base model before the nightly loop starts.

---

## Hardware & Environment

- **Machine**: Apple M1 Max, 64GB unified memory, macOS
- **Inference/training framework**: `mlx-lm` (native Apple Silicon, NOT PyTorch for training)
- **Pruning**: PyTorch with MPS backend (only for Day 0 surgery, then convert to MLX)
- **Base model**: `Qwen/Qwen3.6-27B` — dense (all 27B params active), Apache 2.0, released April 2026
- **Python**: 3.11+

---

## Existing Apprentice Pipeline (do not modify these files)

### `config.py`
```python
from pathlib import Path

BASE  = Path(__file__).parent
DATA  = BASE / "data"
LOGS  = BASE / "logs"
ADP   = BASE / "adapters"

MODEL_PATH  = "/path/to/qwen3-27b-pruned-mlx"  # after Day 0
EMBED_MODEL = "Qwen/Qwen3-Embedding-0.6B"

SIM_THRESHOLD   = 0.85
CLUSTER_TRIGGER = 30
DECAY_DAYS      = 14
VICTORY_STREAK  = 3

TRAIN_ITERS = 200
LORA_RANK   = 8
BATCH_SIZE  = 4

GOLD_SAMPLE     = 50
REG_TOLERANCE   = 0.05
SANDBOX_TIMEOUT = 10
```

### `scribe.py`
```python
"""
scribe.py — L'unica cosa che chiami da Censor.
"""
import json, uuid
from datetime import datetime
from pathlib import Path
from config import DATA, EMBED_MODEL

DATA.mkdir(parents=True, exist_ok=True)
PAIRS = DATA / "pairs.jsonl"
_embed_model = None

def _embed(text: str) -> list:
    global _embed_model
    if _embed_model is None:
        from sentence_transformers import SentenceTransformer
        _embed_model = SentenceTransformer(EMBED_MODEL)
    return _embed_model.encode(text, normalize_embeddings=True).tolist()

def log_pair(prompt: str, rejected: str, chosen: str, test_code: str | None = None) -> str:
    import cluster
    pid = str(uuid.uuid4())
    emb = _embed(f"{prompt}\n{rejected}")
    cid = cluster.assign(pid, emb)
    entry = {
        "id": pid, "ts": datetime.utcnow().isoformat(),
        "prompt": prompt, "rejected": rejected, "chosen": chosen,
        "test_code": test_code, "embedding": emb, "cluster": cid,
    }
    with open(PAIRS, "a") as f:
        f.write(json.dumps(entry) + "\n")
    print(f"[Scribe] {pid[:8]} → cluster {cid[:8]}")
    return pid
```

### `cluster.py`
```python
"""
cluster.py — Cluster state in clusters.json. No database, no extra deps.
"""
import json, uuid, numpy as np
from datetime import datetime, timedelta
from pathlib import Path
from config import DATA, SIM_THRESHOLD, CLUSTER_TRIGGER, DECAY_DAYS, VICTORY_STREAK

STATE  = DATA / "clusters.json"
REPLAY = DATA / "replay.jsonl"

def _load(): return json.loads(STATE.read_text()) if STATE.exists() else {}
def _save(s): STATE.write_text(json.dumps(s, indent=2))
def _sim(a, b):
    a, b = np.array(a), np.array(b)
    d = np.linalg.norm(a) * np.linalg.norm(b)
    return float(np.dot(a, b) / d) if d > 0 else 0.0

def assign(pair_id: str, embedding: list) -> str:
    s = _load()
    best_id, best_sim = None, 0.0
    for cid, c in s.items():
        if c["status"] != "active": continue
        sim = _sim(embedding, c["centroid"])
        if sim > best_sim: best_sim, best_id = sim, cid
    if best_sim >= SIM_THRESHOLD and best_id:
        c = s[best_id]; n = len(c["pairs"])
        c["centroid"] = [(c["centroid"][i]*n + embedding[i])/(n+1) for i in range(len(embedding))]
        c["pairs"].append(pair_id); c["last_seen"] = datetime.utcnow().isoformat()
    else:
        best_id = str(uuid.uuid4())
        s[best_id] = {"status": "active", "centroid": embedding, "pairs": [pair_id],
                      "created": datetime.utcnow().isoformat(), "last_seen": datetime.utcnow().isoformat(), "streak": 0}
    _save(s); return best_id

def ready():
    s = _load()
    return [cid for cid, c in s.items() if c["status"] == "active" and len(c["pairs"]) >= CLUSTER_TRIGGER]

def decay():
    s = _load(); cutoff = datetime.utcnow() - timedelta(days=DECAY_DAYS)
    for cid, c in list(s.items()):
        if c["status"] != "active": continue
        if datetime.fromisoformat(c["last_seen"]) < cutoff: _historicize(cid, s)
    _save(s)

def update_streak(cluster_ids: list, passed: bool):
    s = _load()
    for cid in cluster_ids:
        if cid not in s: continue
        if passed:
            s[cid]["streak"] = s[cid].get("streak", 0) + 1
            if s[cid]["streak"] >= VICTORY_STREAK: _historicize(cid, s)
        else: s[cid]["streak"] = 0
    _save(s)

def _pairs_for(cid):
    pf = DATA / "pairs.jsonl"
    if not pf.exists(): return []
    return [json.loads(l) for l in pf.read_text().splitlines() if l and json.loads(l).get("cluster") == cid]

def _historicize(cid, state):
    c = state[cid]; pairs = _pairs_for(cid)
    if pairs:
        ranked = sorted(pairs, key=lambda p: -_sim(p["embedding"], c["centroid"]))
        with open(REPLAY, "a") as f:
            for p in ranked[:3]:
                f.write(json.dumps({"prompt": p["prompt"], "chosen": p["chosen"], "rejected": p["rejected"]}) + "\n")
    c["status"] = "historicized"
```

### `night_run.py`
```python
"""
night_run.py — Nightly orchestrator. Runs at 2 AM via cron.
Prepare data → mlx-lm ORPO training → before/after benchmark → promote or rollback.
"""
import ast, json, random, shutil, subprocess, tempfile
from datetime import datetime
from pathlib import Path
import cluster
from config import ADP, BATCH_SIZE, DATA, GOLD_SAMPLE, LOGS, LORA_RANK, MODEL_PATH, REG_TOLERANCE, SANDBOX_TIMEOUT, TRAIN_ITERS

LOGS.mkdir(parents=True, exist_ok=True); ADP.mkdir(parents=True, exist_ok=True)
_LOG = LOGS / f"night_{datetime.now().strftime('%Y%m%d_%H%M')}.log"
_model_cache = {}

def log(msg):
    line = f"[{datetime.now().strftime('%H:%M:%S')}] {msg}"
    print(line)
    with open(_LOG, "a") as f: f.write(line + "\n")

def _load_pairs(cluster_ids):
    pf = DATA / "pairs.jsonl"
    if not pf.exists(): return []
    return [{"prompt": p["prompt"], "chosen": p["chosen"], "rejected": p["rejected"]}
            for line in pf.read_text().splitlines()
            if line and (p := json.loads(line)).get("cluster") in cluster_ids]

def prepare(cluster_ids):
    train_dir = DATA / "train"; train_dir.mkdir(exist_ok=True)
    pairs = _load_pairs(cluster_ids)
    replay = []
    rf = DATA / "replay.jsonl"
    if rf.exists():
        all_r = [json.loads(l) for l in rf.read_text().splitlines() if l]
        replay = random.sample(all_r, min(len(pairs), len(all_r), 100))
    combined = pairs + replay; random.shuffle(combined)
    (train_dir / "data.jsonl").write_text("\n".join(json.dumps(p) for p in combined))
    log(f"Data: {len(pairs)} cluster + {len(replay)} replay = {len(combined)} total")
    return train_dir

def train(train_dir):
    new_adapter = ADP / "new"
    if new_adapter.exists(): shutil.rmtree(new_adapter)
    cmd = ["mlx_lm.lora", "--model", MODEL_PATH, "--train", "--train-mode", "orpo",
           "--data", str(train_dir), "--iters", str(TRAIN_ITERS), "--batch-size", str(BATCH_SIZE),
           "--lora-rank", str(LORA_RANK), "--adapter-path", str(new_adapter)]
    log(f"Training: {TRAIN_ITERS} iters, rank {LORA_RANK}")
    subprocess.run(cmd, check=True)
    return new_adapter

def _run_code(code, test):
    script = code + "\n" + test
    try: ast.parse(script)
    except SyntaxError: return False
    with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as f:
        f.write(script); fname = f.name
    try:
        r = subprocess.run(["python", fname], capture_output=True, timeout=SANDBOX_TIMEOUT)
        return r.returncode == 0
    except subprocess.TimeoutExpired: return False
    finally: Path(fname).unlink(missing_ok=True)

def _generate(prompt, adapter):
    key = str(adapter)
    if key not in _model_cache:
        from mlx_lm import load
        _model_cache[key] = load(MODEL_PATH, adapter_path=str(adapter) if adapter else None)
    model, tok = _model_cache[key]
    from mlx_lm import generate
    return generate(model, tok, prompt=prompt, max_tokens=512, verbose=False)

def _pass_at_1(tasks, adapter):
    if not tasks: return 1.0
    return sum(1 for t in tasks if _run_code(_generate(t["prompt"], adapter), t.get("test_code","pass"))) / len(tasks)

def _sample_gold():
    gf = DATA / "gold_standard.jsonl"
    if not gf.exists(): return []
    all_tasks = [json.loads(l) for l in gf.read_text().splitlines() if l]
    return random.sample(all_tasks, min(GOLD_SAMPLE, len(all_tasks)))

def _sample_cluster_tasks(cluster_ids, n=20):
    pf = DATA / "pairs.jsonl"
    if not pf.exists(): return []
    tasks = []
    for line in pf.read_text().splitlines():
        if not line: continue
        p = json.loads(line)
        if p.get("cluster") in cluster_ids:
            tasks.append({"prompt": p["prompt"], "test_code": p.get("test_code") or "pass"})
    return random.sample(tasks, min(n, len(tasks)))

def run_benchmark(cluster_ids, new_adapter):
    gold_tasks = _sample_gold(); cluster_tasks = _sample_cluster_tasks(cluster_ids)
    if not gold_tasks: log("No gold standard. Auto-promoting."); return True
    current = (ADP / "active") if (ADP / "active").exists() else None
    log("── PRE-training benchmark ──")
    pre_gold = _pass_at_1(gold_tasks, current)
    pre_cluster = _pass_at_1(cluster_tasks, current) if cluster_tasks else None
    log(f"  Gold: {pre_gold:.2%}  |  Cluster: {pre_cluster:.2%}" if pre_cluster else f"  Gold: {pre_gold:.2%}")
    log("── POST-training benchmark ──")
    post_gold = _pass_at_1(gold_tasks, new_adapter)
    post_cluster = _pass_at_1(cluster_tasks, new_adapter) if cluster_tasks else None
    d_gold = post_gold - pre_gold; d_clus = (post_cluster - pre_cluster) if pre_cluster else None
    log(f"  Gold: {post_gold:.2%} (Δ {d_gold:+.2%})  |  Cluster: {post_cluster:.2%} (Δ {d_clus:+.2%})" if d_clus else f"  Gold: {post_gold:.2%} (Δ {d_gold:+.2%})")
    if pre_gold - post_gold > REG_TOLERANCE:
        log(f"❌ REGRESSION: -{pre_gold-post_gold:.2%} > limit {REG_TOLERANCE:.2%}"); return False
    log("✅ Benchmark OK"); return True

def promote(new_adapter):
    active = ADP / "active"; backup = ADP / "backup"
    if active.exists():
        if backup.exists(): shutil.rmtree(backup)
        shutil.copytree(active, backup)
    shutil.copytree(new_adapter, active); log("✅ Promoted")

def main():
    log("═══ Night Run Start ═══")
    cluster.decay()
    ready_clusters = cluster.ready()
    if not ready_clusters: log("No clusters ready. Done."); return
    log(f"Ready: {[c[:8] for c in ready_clusters]}")
    train_dir = prepare(ready_clusters); new_adapter = train(train_dir)
    if run_benchmark(ready_clusters, new_adapter):
        promote(new_adapter); cluster.update_streak(ready_clusters, passed=True)
    else:
        log("Keeping old adapter."); cluster.update_streak(ready_clusters, passed=False)
    log("═══ Night Run Done ═══")

if __name__ == "__main__": main()
```

---

## What I Need: The Day 0 Pipeline

Build **4 Python scripts** that run once to prepare the model before the nightly loop starts. No web UI, no complex frameworks, no async. Just Python scripts that work.

### Script 1: `day0_vocab_prune.py`

Prune non-coding tokens from `Qwen/Qwen3.6-27B` using PyTorch (MPS backend on M1 Mac).

**Algorithm:**
1. Load the model in PyTorch (float16 or bfloat16, use MPS if available, else CPU)
2. Load a small code corpus to identify code-relevant tokens — use the first 50K rows of `coseal/Magicoder-Evol-Instruct-110K-sft` from HuggingFace (the `output` field contains Python code)
3. Tokenize the entire corpus, count frequency per token ID
4. Build the keep-set: all token IDs with frequency > 0 in the code corpus, PLUS all special tokens (bos, eos, pad, etc.), PLUS all ASCII printable character tokens (IDs corresponding to single bytes 32–126)
5. Remove rows NOT in the keep-set from:
   - `model.model.embed_tokens.weight` (input embeddings)
   - `model.lm_head.weight` (output projection)
6. Remap token IDs: build `old_id → new_id` mapping, update the tokenizer vocabulary accordingly
7. Save the pruned model to `./qwen3-27b-pruned-hf/` in HuggingFace safetensors format
8. Save the updated tokenizer to the same directory

Print: original vocab size, new vocab size, reduction percentage.

**Important:** Use `torch.device("mps")` if available, else `"cpu"`. Do NOT use CUDA. Load in bfloat16 to fit in 64GB RAM.

---

### Script 2: `day0_layer_prune.py`

Apply ShortGPT-style layer pruning to `./qwen3-27b-pruned-hf/` (output of Script 1).

**Algorithm (Block Influence scoring):**
1. Load the pruned model in PyTorch bfloat16 on MPS/CPU
2. Load 200 random samples from `coseal/Magicoder-Evol-Instruct-110K-sft` as calibration data
3. For each transformer layer `i`, compute Block Influence (BI):
   - Hook the input and output of layer `i`
   - Run the calibration samples through the model
   - BI[i] = mean over samples of `1 - cosine_similarity(layer_input, layer_output)`
   - Layers with **low BI** are most redundant (output ≈ input = identity)
4. Print a table: layer index → BI score (sorted ascending)
5. Remove the `N_LAYERS_TO_REMOVE` layers with lowest BI (default: 4)
   - This means removing them from `model.model.layers` list
6. Update `model.config.num_hidden_layers` accordingly
7. Save to `./qwen3-27b-pruned-hf/` (overwrite in place, or to `./qwen3-27b-pruned-v2-hf/`)

`N_LAYERS_TO_REMOVE = 4` as a top-level constant, easy to change.

---

### Script 3: `day0_prepare_data.py`

Download and format both training datasets from HuggingFace, ready for `mlx-lm-lora`.

**Dataset A — SFT recovery (`coseal/Magicoder-Evol-Instruct-110K-sft`):**
- Load from HuggingFace datasets
- Each row has `instruction` and `output` fields
- Format for mlx-lm SFT (chat template format):
  ```json
  {"messages": [{"role": "user", "content": "<instruction>"}, {"role": "assistant", "content": "<output>"}]}
  ```
- Save to `./data/day0_sft/train.jsonl` (all 110K rows)
- Save a 500-row validation split to `./data/day0_sft/valid.jsonl`

**Dataset B — ORPO warm-up (`coseal/CodeUltraFeedback_binarized`):**
- Load from HuggingFace datasets
- Each row has `prompt`, `chosen`, `rejected` fields
- Format for mlx-lm ORPO (already the right structure):
  ```json
  {"prompt": "...", "chosen": "...", "rejected": "..."}
  ```
- Save to `./data/day0_orpo/train.jsonl` (all rows, ~9K)
- Save a 200-row validation split to `./data/day0_orpo/valid.jsonl`

Print row counts for each split when done.

---

### Script 4: `day0_run.sh`

Shell script that orchestrates all Day 0 steps in order. Each step logs its start/end time. If any step fails, the script stops immediately (`set -e`).

**Steps in order:**
1. Run `python day0_vocab_prune.py`
2. Run `python day0_layer_prune.py`
3. Convert pruned HuggingFace model to MLX format:
   ```bash
   python -m mlx_lm.convert \
     --hf-path ./qwen3-27b-pruned-v2-hf \
     --mlx-path ./qwen3-27b-pruned-mlx \
     --dtype bfloat16
   ```
4. Run `python day0_prepare_data.py`
5. SFT recovery training (mlx-lm-lora, SFT mode):
   ```bash
   mlx_lm.lora \
     --model ./qwen3-27b-pruned-mlx \
     --train \
     --train-mode sft \
     --data ./data/day0_sft \
     --iters 500 \
     --batch-size 4 \
     --lora-rank 8 \
     --adapter-path ./adapters/day0_sft
   ```
6. Fuse SFT adapter into model:
   ```bash
   mlx_lm.fuse \
     --model ./qwen3-27b-pruned-mlx \
     --adapter-path ./adapters/day0_sft \
     --save-path ./qwen3-27b-day0-mlx
   ```
7. ORPO warm-up training (mlx-lm-lora, ORPO mode):
   ```bash
   mlx_lm.lora \
     --model ./qwen3-27b-day0-mlx \
     --train \
     --train-mode orpo \
     --data ./data/day0_orpo \
     --iters 300 \
     --batch-size 4 \
     --lora-rank 8 \
     --adapter-path ./adapters/day0_orpo
   ```
8. Copy ORPO adapter as the initial `active` adapter for the nightly pipeline:
   ```bash
   cp -r ./adapters/day0_orpo ./adapters/active
   ```
9. Update `MODEL_PATH` in `config.py` to `./qwen3-27b-day0-mlx`
10. Print: "Day 0 complete. Model ready at ./qwen3-27b-day0-mlx. Nightly pipeline can start."

---

## Hard Constraints

- **No CUDA.** MPS (Apple Silicon Metal) or CPU only in PyTorch scripts.
- **No PyTorch for training.** Training is always via `mlx_lm.lora` subprocess calls.
- **No over-engineering.** No classes, no async, no config files for Day 0 scripts. Top-level constants for anything that might need tweaking. Each script is self-contained.
- **HuggingFace datasets** loaded via `from datasets import load_dataset`. Cache locally.
- **Model loading** in PyTorch: always `torch.bfloat16` to fit 64GB RAM.
- **Error messages must be clear.** If something fails (e.g. MPS not available, dataset not found), print a clear message and exit with code 1.
- **No LLM-as-a-Judge** anywhere. Validation is always execution-based (compiler/interpreter).
- Scripts must be runnable as-is with standard Python packages: `torch`, `transformers`, `datasets`, `safetensors`, `numpy`. Nothing exotic.

---

## Deliverables

Four files:
1. `day0_vocab_prune.py`
2. `day0_layer_prune.py`
3. `day0_prepare_data.py`
4. `day0_run.sh`

Each file complete, self-contained, immediately runnable. No placeholders, no TODOs. Real code.
