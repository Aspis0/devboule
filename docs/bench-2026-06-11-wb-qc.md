# Mini-benchmark live — 2026-06-11 (oMLX on M1 Max)

Real task from **aspis-biovision** (Western Blot QC). Goal: smoke + speed benchmark of the local models + the Gemma E4B Censor, one model at a time (unloaded between each to stay under 64 GB).

## Task (prompt sent)
> In the aspis-biovision Western Blot QC module, write a small TypeScript function `blotQcStatus(flags: string[]): 'usable' | 'review' | 'reject'`. Rules: `'reject'` if flags include `'saturated_membrane'`; `'review'` if flags include `'high_background'` or `'uneven_background'`; else `'usable'`. Only the typed function, no explanation.

## Output (IDENTICAL across all 3 generators — task was deterministic)
```typescript
function blotQcStatus(flags: string[]): 'usable' | 'review' | 'reject' {
  if (flags.includes('saturated_membrane')) {
    return 'reject';
  }
  if (flags.includes('high_background') || flags.includes('uneven_background')) {
    return 'review';
  }
  return 'usable';
}
```

## Results
| Model | Correct? | Speed | Resident RAM |
|---|---|---|---|
| Gemma 12B (`gemma-4-12B-it-qat-4bit`) | ✅ | 14.3 tok/s | 11.5 GB |
| Qwen 27B dense (`Qwen3.6-27B-OptiQ-4bit`) | ✅ | 10.4 tok/s | 21.0 GB |
| **Qwen 35B MoE** (`Qwen3.6-35B-A3B-4bit-DWQ`) | ✅ | **36.2 tok/s** 🏆 | 21.7 GB |

## Censor (Gemma E4B) verdict
Reviewed the generated function as a strict tier-2 reviewer:
> **No issues**

(1.4 s, 7.1 GB resident.) ⚠️ Caveat: the code was clean, so this only proves E4B passes correct code — it does NOT yet prove its bug-catch rate. A real Censor test needs a deliberately buggy input (planned in master-plan P9).

## Key takeaways
- **MoE is 3.5× faster than the 27B dense** (36 vs 10 tok/s), same quality here → empirically confirms the [[mini-coder-local-hardware-decision]] prediction. Real tension to weigh: MoE = best for mini INFERENCE (many one-shot calls); dense = chosen for ORPO TRAINABILITY. Option: mini = MoE for speed, ORPO on the dense only if MoE-training proves too hard.
- Task too simple to differentiate QUALITY (all identical) — only speed. A harder, real benchmark set is master-plan P15.
- Confirmed config: Qwen needs `chat_template_kwargs:{enable_thinking:false}`; oMLX `POST /v1/models/{id}/unload` keeps RAM to one model at a time.

Raw per-model outputs were also written to `/tmp/bench_*.txt` during the run (ephemeral — cleared on reboot; this file is the persistent record).
