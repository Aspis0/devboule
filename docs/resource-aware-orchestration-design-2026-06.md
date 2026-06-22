# Resource-aware orchestration + capability-tiered agentic coders — DESIGN

> Status: DESIGN, owner-driven (2026-06-18). UNTRACKED (docs/), do NOT commit.
> GPU-free design. Supersedes the one-shot emit-edits-only paradigm for capable local models.
> Feasibility probed & GREEN (see §2). Build is incremental (§12); nothing here is built yet.

---

## 0. Problem (why we are changing it)

Today every LOCAL mini-coder (oMLX / Ollama / Api / AppleFm) is **one-shot**:
- `backend_can_write_file = matches!(backend.kind, Codex)` (`mini_coder_executor.rs:3510`) — only the **Codex** backend writes files itself. The local HTTP models never touch disk.
- The app **front-loads** file content into the prompt (`MAX_PROMPT_FILE_BYTES = 32*1024`, `:3480`, truncates files >32KB — e.g. `ProjectWorkspace.tsx` at 39KB gets cut), the model emits a `{edits:[{path,oldString,newString}]}` JSON capped at `OMLX_MAX_TOKENS_DEFAULT = 6144` output tokens (`:131`), the app validates + applies + runs Censor, and on reject the **orchestrator re-spawns** the model with feedback.
- `agenticIterative` for local models is only a bigger ROUND budget, **not** a tool-using loop.

Result: the minis are dumb one-shot edit-emitters; ALL autonomy/iteration lives in the orchestrator loop = the **"too many round-trips"**. The 32K/6144 caps are arbitrary latency guards from a past session, wrong for 262K-context models, and they actively truncate real work.

**The pivot:** capable (>20B) local models run a REAL agent loop (tool-calling: read/edit/run, sandboxed to scope) → they work AUTONOMOUSLY: the main coder gives a high-level task, the mini reads what it needs on demand (no front-load → no 32K cap), edits, self-iterates, reports done; Censor still gates after. Weak (<20B) models keep the current emit-edits scheme. Layered on top: the **app is a resource broker** that budgets RAM/compute across multiple local backends + cloud, and recommends a hardware-appropriate config.

---

## 1. The model (owner-confirmed)

Three layers + a capability tier:

- **L0 — App = resource broker.** Detects the machine (OS, GPU, unified vs discrete memory, RAM/VRAM size), aggregates live memory across ALL local backends + Oracle, proposes a **hardware-tiered recommended config** (which role runs where), and HARD-gates every local spawn against the global budget. All override-able in Settings.
- **L1 — Placement + budget delegation.** App decides local-vs-cloud per role/main-coder under the global budget, and tells each main coder its RAM allowance.
- **L2 — Main coder = sub-broker.** Spends its allowance spawning minis within the cap.
- **Capability tier:** local model **>20B** → **agentic** (writes files itself via tools, sandboxed, Censor after); **<20B** → current **emit-edits** scheme. Thresholds + budgets in Settings, user-overridable.

---

## 2. Ground truth (probed 2026-06-18, M1 Max 64GB, GPU free) — all GREEN

- **oMLX tool-calling WORKS natively.** Qwen3.6-35B-A3B (no-think) and gemma-4-26B-A4B both returned `finish_reason=tool_calls` with one clean `read_file({"path":...})` (valid JSON args), and both chose to READ before editing. → No ReAct text-parsing needed.
- **oMLX = engine POOL, multi-resident.** `GET :8000/health` → `engine_pool:{model_count, loaded_count, final_ceiling, current_model_memory}`. On 64GB: `final_ceiling`≈**51.8 GiB** (oMLX self-reserves ~12GB), `current_model_memory`≈**38.6 GiB** with `loaded_count:2`. A→B→A→B all fast, zero reload → both stay resident; oMLX evicts itself under the ceiling. (`/status`,`/metrics`,`/admin/*` = 404; only `/health`.)
- **Ollama introspection WORKS** (`:11434`, v0.30.8). `/api/tags` → installed models WITH size+parameter_size+quantization (qwen3:30b-a3b 18.56GB/30.5B/Q4_K_M, gemma3:12b 8.15GB, Devstral-Small-2-24B Q6 20.22GB [coder], Nemotron-3-Nano-4B Q8 4.23GB/Q4 2.84GB). `/api/ps` → loaded models w/ size/size_vram/`expires_at` (0 at idle).
- **MLX footprint = WIRED memory, not RSS** (~38.6GB for 2 mid 4-bit models; omlx-server RSS stayed flat at 21GB). Ollama gives footprint upfront via `/api/tags`; oMLX `/v1/models` omits size (measure via `/health` delta).
- **Different memory idioms:** oMLX = resident pool + fixed ceiling + LRU evict; Ollama = lazy-load + TTL unload (`expires_at`), 0 resident at idle.
- **Caveat (NOT solved by any probe): resident ≠ compute-free.** ONE GPU → concurrent decodes serialize (~20-34 tok/s ceiling for ONE active model; concurrents split it). `/health` doesn't report compute. The broker needs a COMPUTE-concurrency cap on top of the memory pool.

---

## 3. Hardware detection & memory regimes

The broker must classify the machine, because the budgeting math differs:

| Regime | Example | Memory model | Footprint check |
|---|---|---|---|
| **Unified** | Mac (M-series), some new NVIDIA | RAM == VRAM, one pool | model + KV vs total RAM budget |
| **Discrete GPU** | Windows + RTX 4050m 6GB | VRAM separate from RAM | model + KV **must fit VRAM** or it's unusable on GPU (CPU offload = slow fallback) |
| **CPU-only** | no GPU | RAM, slow | small models only / cloud-first |

Detection inputs:
- **OS / arch:** `std::env::consts::OS`; macOS `sysctl hw.memsize` / `system_profiler SPHardwareDataType` / `SPDisplaysDataType`; Windows `nvidia-smi --query-gpu=memory.total` / WMI; Linux `nvidia-smi` / `/proc/meminfo`.
- **GPU type + VRAM:** unified (Apple/`hw.memsize`) vs discrete (`nvidia-smi` VRAM).
- **Backends present:** is oMLX up (`:8000/health`)? Ollama up (`:11434/api/version`)? (ports configurable.)
- **Per-model footprint:** Ollama `/api/tags` (size/params/quant upfront); oMLX = estimate (params × bytes/quant) + KV reserve, refine by `/health` delta on first load.
- **KV cache reserve:** budget for a REALISTIC context (e.g. 16-32K), NOT the 262K max — KV can exceed weights at long context.

---

## 4. L0 — Resource broker (the app)

### 4a. Global budget aggregation (multi-backend — the app owns it, NOT oMLX)
`global_used = oMLX(/health current_model_memory) + Σ Ollama(/api/ps size) + Oracle footprint + app/OS headroom`.
`global_free = global_ceiling − global_used`, where `global_ceiling` is a Settings value defaulting to ~(total RAM/VRAM − OS/app reserve). **Why the app and not oMLX:** oMLX `/health` knows only its own pool; if Ollama also loads models the two independently can blow past total RAM. The app polls BOTH and is the single global accountant.

### 4b. Hard spawn-gate
An LLM cannot be trusted to do RAM arithmetic. Every local spawn (main or mini) is gated by the app against live `global_free` + the per-role/per-main budget. The app controls DEMAND even if it can't reconfigure a backend's internal ceiling. Over-budget → refuse / queue / suggest cloud.

### 4c. Compute-concurrency cap
Separate from memory: a small cap on concurrent ACTIVE local decodes (default ~1-2 on a single GPU) so resident-but-serialized models don't all crawl. Configurable; this is what makes "1 local + 1 cloud" the right call on a Mac.

### 4d. Hardware-tiered RECOMMENDED config (the "be smart about my machine" requirement)
The broker proposes a role→placement default by tier (user can override everything):

| Tier | Example | Recommended local | Cloud |
|---|---|---|---|
| **Low** | Win + 4050m 6GB discrete | **Oracle** (+ small Censor ≤~4B if it fits VRAM) | main coder, mini coder, (Censor if no fit) |
| **Mid** | 16-32GB unified / 12-16GB VRAM | Oracle + Censor + maybe 1 small local mini | main coder, heavy minis |
| **High** | 64GB+ unified (M1 Max) | Oracle + main coder + minis (>20B agentic) | optional / overflow |

Roles ranked by "must-be-capable": **main coder > mini coder > Censor > Oracle**. Under pressure the broker pushes the heaviest roles to cloud first and keeps the light ones (Oracle, then Censor) local.

---

## 5. Config model — role × backend matrix (the "handle every combination" requirement)

### 5a. Roles & placements
- **Roles:** Oracle (retrieval/context), Main coder, Mini coder, Censor. (Verifier if distinct from Censor.)
- **Placements per role:** `oMLX-local` | `Ollama-local` | `cloud-Claude` | `cloud-GLM` (extensible).
- Each role is **independently assignable**. The owner's example (main=cloud, censor=cloud, mini=local) is a valid cell of the matrix; so are dozens of others.

### 5b. Model registry (user-curated)
- Auto-discover installed: oMLX `/v1/models`, Ollama `/api/tags`.
- User curates the exposed list per backend, tagging each: role-eligibility, capability tier (size + a **`tool_calling_reliable`** flag defaulting by the size threshold), and footprint.
- The **main coder is given this curated list** and CHOOSES per task + its remaining budget.

### 5c. Validation across ALL combinations (be careful — many permutations)
For any chosen matrix the app validates:
- local role → its model+KV must FIT the backend's budget on THIS hardware (discrete: VRAM; unified: RAM share). Over → warn/block + suggest cloud or a smaller model.
- cloud role → required key present (vault), else block; routes cost to the existing cost-ledger.
- `mini=local` while `main=cloud` is fine (a cloud main coder can still spawn local minis — the app brokers the mini's RAM).
- mixed local backends (some oMLX, some Ollama) must both be accounted in `global_used`.
- compute-cap respected across the whole matrix.
Surface a clear per-cell status (OK / warn / blocked + reason) in Settings.

---

## 6. L1 — Placement + budget delegation (app → main coder)
- The app resolves the matrix → concrete placements, then tells each main coder: its RAM allowance + the curated model list it may spawn from. Guidance to the LLM, **enforced** by the §4b gate.
- When the global budget can't host all requested local main coders (e.g. 2 projects, 64GB), the app applies the tiering: keep the highest-priority project's main coder local, route the rest to cloud (the owner's "1 local + 1 Claude").

## 7. L2 — Main coder sub-broker
- Within its allowance the main coder spawns minis (count × footprint ≤ allowance), choosing models from the curated list by task difficulty (and thinking-on only for hard-semantic tasks per `local-mini-coder-policy`).
- Every `spawn_mini_coder` still passes through the app's hard gate (§4b) + compute cap (§4c).

## 8. Capability tier (execution mode by model)
- **>20B (or `tool_calling_reliable`)** → **agentic loop** (§9): writes files itself, sandboxed, Censor after.
- **<20B** → current **emit-edits** (app applies). Unchanged.
- Selection: by the registry's tier flag, default by the Settings size threshold, per-model override. Hooks into the existing `WriteMode` (EmitEdits/AgenticIterative) + `MiniWriteBehavior` (Safe/Auto/AgenticAllowed) gate (`mini_coder.rs`) — promote AgenticIterative from a round-budget knob to a REAL tool loop for HTTP backends, not just Codex.

## 9. Agentic loop spec (for capable local models)
- **OSS to steal (researched 2026-06-18, all Apache-2.0/MIT):**
  - **Goose** (block/goose) — **Rust** 64% + TS, Apache-2.0, Cargo workspace `/crates`, MCP-native, 15+ providers incl. Ollama/OpenRouter. SAME stack as us → the structural reference for an in-process Rust agent loop (plan→select-tool→execute→evaluate→loop). 44.7k★, now under Linux Foundation AAIF.
  - **Qwen-Agent** (Alibaba, Python, Apache-2.0) — function-calling loop purpose-built for Qwen models (our minis are Qwen) → steal its Qwen tool-call handling/quirks.
  - **mini-swe-agent** (Princeton, MIT, ~100 lines, bash-loop, linear history) — the minimalism reference (how little a loop needs).
  - **smolagents** (HF, Apache-2.0, ~1000 lines, ReAct) — reference only; "code-as-action" makes the sandbox riskier, avoid that style.
  - (OpenCode = full TS terminal app, UX reference not a lib.)
- **DECISION (anti-over-engineering):** do NOT take Goose as a heavy dependency. **STEAL THE PATTERN** of its Rust loop + Qwen-Agent's Qwen tool-call handling + mini-swe-agent's minimalism, and write a **minimal loop in the `devboule-coder` crate** that REUSES what we already have (MCP via aspis_mcp.py, the scope-allowlist sandbox, Censor, the mini executor). We only lack the loop itself (tool-call→dispatch→feed result→repeat until done) wired to the oMLX/Ollama tool-calling protocol (confirmed working §2). Adopting Goose whole would drag in its provider/extension system that we already have in another form.
- **Tool set (sandboxed):** `read_file`, `edit_file`/`write_file`, `list_dir`/`grep`, `run` (tests/build), maybe `oracle_context`. No network, no destructive.
- **Sandbox = TOOL GATE, not edit validator.** Reuse the existing scope-allowlist so every tool call is confined to the directive's scope (the same allowlist that today validates emitted edits becomes the gate on read/write/run paths). This is the "improve the existing sandbox" the owner noted.
- **No token cap on local** (per `local-mini-coder-policy`); replace the 32K/6144 caps with a runaway guard (round budget + wall-clock), not a content/output truncation.
- **Round budget + Censor:** the mini self-iterates to "done", then Censor gates; on reject, feedback goes back into the loop (still bounded). The orchestrator no longer drives byte-level re-spawns.

## 10. Settings UI
- Hardware budget: global RAM/VRAM ceiling + compute-concurrency cap (with the detected default + "recommended config" apply button).
- Size thresholds: the >20B agentic line + per-model `tool_calling_reliable` override.
- Model registry: curated installed list per backend (discover + tag role/tier/footprint).
- Role × backend matrix (§5) with live validation status per cell.

## 11. Hooks into existing code
- `MiniWriteBehavior` + `WriteMode` gate (`mini_coder.rs`), `mini_coder_executor.rs` launch arms (per backend), the scope-allowlist sandbox, the cost-ledger (`backend/cost.rs`), the vault (cloud keys), `/health` (oMLX) + Ollama API as live-state sources. Settings store (config.json) for the matrix + budgets.

## 12. Incremental build sequence (each step: implement → on-disk verify → 1 hostile reviewer → fix; whole diff → max-recall)
1. **Hardware + backend detection** (read-only): OS/GPU/memory regime; probe oMLX `/health` + Ollama `/api/tags|ps`; per-model footprint table. Pure read, no behavior change.
2. **Global budget accountant** (app): aggregate §4a, expose to UI; no gating yet (observe-only).
3. **Model registry + Settings** (discover + curate + role×backend matrix + validation §5).
4. **Hard spawn-gate + compute cap** (§4b/4c): enforce on local spawns.
5. **Recommended-config tiering** (§4d): propose defaults by hardware.
6. **Agentic loop for capable local models** (§9): the big one — reuse OSS loop, tool gate via sandbox, Censor integration; behind the §8 tier so weak models keep emit-edits.
7. **Retire the 32K/6144 caps** for the agentic path; keep a runaway guard.
8. **Placement/delegation polish** (L1/L2 multi-project local-vs-cloud).

Reuse-vs-new: detection/budget/registry/matrix = NEW (app layer). Agentic loop = reuse OSS + existing sandbox/Censor. Tier selection = extend existing WriteMode gate.

## 13. Open questions / risks
- Per-model oMLX footprint (measure `/health` delta on load) — needed for accurate pre-spawn fit checks.
- Oracle's own footprint (measure) — it's the always-local baseline.
- Compute-cap value tuning (1 vs 2 concurrent local decodes) — measure tok/s degradation.
- Can the app influence oMLX's internal `final_ceiling` (config/env) or only control demand? (Demand-gating is enough; ceiling-tuning is a bonus.)
- Discrete-VRAM fit on Windows: CPU-offload fallback (slow) vs refuse — owner preference.
- ~~Which OSS agent loop to adopt~~ → RESOLVED (§9): steal patterns (Goose/Qwen-Agent/mini-swe-agent), write a minimal loop in `devboule-coder` reusing our MCP/sandbox/Censor.
- Coder-model candidate: **Devstral-Small-2-24B** (already in the user's Ollama, ~20GB Q6, 68% SWE-bench Verified, runs on 32GB) — strong local CODER for the registry vs the Qwen/gemma generalists. Tag it coder-role in §5b.

---
*Design 2026-06-18. Next: owner review → start §12 step 1 (detection, read-only, GPU-free).*
