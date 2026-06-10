# Aspis — Infrastructure Costs

Living document tracking actual + projected monthly costs across the Aspis stack. Source of truth for "are we burning too much?" conversations.

Last updated: **2026-05-29**.
Pricing reference: **Scaleway June 1st 2026 schedule** (see §6 for the full pricing table).

Live audit source, 2026-05-29: Aspis Management vault credentials + Cloudflare REST + Scaleway REST. The app sees 8 Aspis Bio Cloudflare Workers after filtering 7 sibling workers, Cloudflare AI Search instance `aspis-bio-papers`, 5 Scaleway Serverless Containers, 5 Scaleway Object Storage buckets, and 0 currently running/stopped Scaleway Instance VMs after the lifecycle smoke cleanup.

---

## 1. TL;DR — Current monthly bill

| Component | Status | Monthly cost (EUR) |
|---|---|---|
| Cloudflare Workers (8 deployed — api/biovision/orasis/rnaseq-api/papers/oauth/mta-sts/resend-webhooks) | LIVE | ~$5 plan base (€4) + ~$1-2 Workers AI (paper RAG) |
| Cloudflare R2 (aspis-bio-papers 59.8MB / aspis-bio-rna-runs 0B today) | LIVE | $0 (well under 10 GB free) |
| Cloudflare AI Search (AutoRAG index for `aspis-bio-papers`) | LIVE, ingesting | ~$3 one-off + ~$0.30/mo queries — see §4.5 |
| Scaleway containers (orasis cellpose-cpu, fiji, features, ilastik) | LIVE, scale-to-zero | €0 free tier covers it today (see §3b) |
| Scaleway GPU L4 (orasis ephemeral) | READY, no active VM in 2026-05-29 audit | ~€10/month at modeled alpha usage (see §3b) |
| Scaleway RNA-seq compute (GP1-S per job) | READY, no active VM in 2026-05-29 audit | ~€0.48/job, currently low volume — see §3a |
| All AI inference (Scaleway + Infomaniak Gemma + Mistral) | LIVE | <€1/month today (see §5) |
| Scaleway Object Storage (orasis-raw, processed, previews) | LIVE | <€1 today |
| **Experiment Vault v1** (PG + S3 + LanceDB) | NOT YET DEPLOYED | **€7-15/mo alpha-beta, €25-30/mo at 1k users** (see §2) |
| **TOTAL today** | | **~€15-25 / month** |
| **TOTAL after Vault deploys (alpha 10 users)** | | **~€22-35 / month** |
| **TOTAL after Vault deploys (beta 100 users)** | | **~€28-40 / month** |
| **TOTAL after Vault deploys (1k users)** | | **~€80-120 / month** (Vault + AI co-scientist + RNA-seq jobs) |

---

## 2. Experiment Vault v1 — cost model

**Scope**: per-user persistent storage of experiment results (gel/WB/RNA-seq/Orasis cells/labbook notes). NOT raw inputs. Spec: `aspis-biovision/docs/experiment_vault_v1_spec.md`. Code lives in `aspis-lab/cloudflare/aspis-bio-api/src/vault/`.

### 2.1 Storage usage profile per user

Realistic active researcher (5% are power users):

- 10 gel results × 50 KB = 500 KB
- 10 WB results × 100 KB = 1 MB
- 5 RNA-seq DE reports × 2 MB = 10 MB
- 5 Orasis cell segmentations × 500 KB = 2.5 MB
- 20 lab book notes × 5 KB = 100 KB
- **Total: ~14 MB / month / active user**
- **Annual: ~170 MB / user**

User cap: **20 GB free per user** (generous — at typical usage, takes ~12 years to fill).

### 2.2 Storage class decision: Standard One Zone

| Option | Price | €/GB/month | Choice rationale |
|---|---|---|---|
| Standard Multi-AZ | €0.000022/GB/h | €0.0161 | Critical-availability data |
| **Standard One Zone** | **€0.000011/GB/h** | **€0.00803** | **Vault default — results are recomputable** |

50% savings. Adequate durability — if a zone is lost, the user can re-save from their browser/app.

### 2.3 Vault component costs (VERIFIED from Scaleway pricing pages)

| Component | Type | Cost |
|---|---|---|
| **Scaleway Object Storage (One Zone)** | Per-GB-hour | €0.00803 / GB / month |
| **Scaleway Serverless SQL Database** | Per-vCPU-second active (min 5-min window per burst); SCALE-TO-ZERO when idle | **€0.13572 / vCPU / h active** + **€0.199 / GB / month storage** + free backups |
| **LanceDB REST proxy on DEV1-S** | Always-on instance | €0.00898/h × 730 = **€6.55 / month** |
| **CF Worker requests** | Already paid | €0 marginal (existing Workers plan) |
| **Egress** | First 75 TB free | €0 realistic |

**Key insight on PG Serverless billing**: it truly scales to zero (no idle cost), but each burst of activity has a minimum 5-minute billed window. So the cost depends entirely on how requests cluster:
- Bursty traffic that fits in few 5-min windows → cheap
- Random small queries spread across the day → each one triggers a 5-min window → expensive
- Sustained traffic (>~10 h/day equivalent) → cheaper to use Managed PostgreSQL Cost Optimized fixed node (€0.0156/h ≈ €11.39/month always-on, smallest tier)

### 2.3.1 Save-time batching via CF Queue (cost mitigation) — implemented 2026-05-28

To collapse N random saves into 1 PG burst, `POST /v1/vault/save` is asynchronous on the PG side:
1. Synchronous: write blob to S3 → push message to CF Queue `aspis-bio-vault-saves` → return **202 Accepted** to the client.
2. CF triggers the consumer when EITHER 25 messages accumulate OR 30 seconds elapse since the first un-drained message (whichever fires first).
3. Consumer opens 1 PG connection, lands the whole batch in one burst window.

Latency contract: a save shows up in `GET /v1/vault/experiments` after **at most 30 seconds** — there is no "wait for 25 to fill" trap; single-message batches drain after 30s with the same code path.

Cost effect on PG compute (Serverless SQL DB):
- **Without queue** (old inline-PG flow): every save = 1 burst = €0.011 minimum. 5 users × 1 save/day × 30 = 150 bursts/mo = ~€1.70.
- **With queue**: same alpha load still 150 bursts because each save is isolated within a 30s window. But beta (100 users × 5 saves/day = 15k saves/mo) drops from ~€407/mo (naive) to ~€34/mo when clustering averages ~5 saves/window. **~12x cost reduction at beta scale.**

Failure path: 5 retries → DLQ `aspis-bio-vault-saves-dlq`. Blob stays on S3 (durable, recoverable). See `aspis-bio-api/src/vault/README.md` "Save queue" section for full architecture.

### 2.4 Total cost by user count (realistic distribution, updated post-queue)

Assumption: 5% of users are power users (1.2 GB stored), 95% typical (120 MB stored). PG estimates use the **batched-save model**: average ~3 saves per 30-second batch window at beta scale, single-message batches at alpha scale (low traffic = isolation, no clustering benefit).

| Users | S3 storage | PG compute (batched) | PG storage | LanceDB | **TOTAL/mo** | **€/user/mo** |
|---|---|---|---|---|---|---|
| 0 (no traffic) | €0 | €0 (consumer never invoked) | €0.01 (schema only) | €0 (free tier) | **€0.01** | — |
| **1 user, 1 save/day** | €0.01 | 30 batches × 5min × €0.136 ≈ **€0.34** | <€0.01 | €0 | **€0.35** | €0.35 |
| **5 users alpha, 1 save/day each** | €0.01 | 150 batches isolated ≈ **€1.70** | <€0.01 | €0 | **€1.71** | €0.34 |
| **10 users alpha, 2 saves/day clustered** | €0.01 | ~250 batches ≈ **€2.83** | <€0.01 | €0 | **€2.84** | €0.28 |
| 100 (beta) | €0.13 | ~3000 batches × ~3 msg = ~250h ≈ **€34** | €0.20 | €0 (free tier) | **€34.33** | €0.34 |
| 1,000 | €1.30 | sustained ~400h ≈ **€54** | €1.20 | €0-2 | **€56.50** | €0.057 |
| 10,000 | €13.00 | always-on 1.5 vCPU ≈ **€110** | €12 | €28.79 | **€163.79** | €0.016 |
| 100,000 | €130 | always-on 2 vCPU ≈ **€195** | €120 | €57.58 | **€502.58** | €0.005 |

**Alpha is effectively free**: even with 10 users saving daily, the bill is €2-3/month — dominated by minimum activity-window cost, not by traffic.

**Crossover point**: at 10k users sustained, switch from Serverless PG to **Managed PostgreSQL DB-PLAY2-NANO/PRO2-XXS** (€0.0561/h = €40.95/month) — fixed-cost wins above ~300h/month compute equivalent.

**Tunable**: queue's `max_batch_timeout` is 30s today (in `aspis-bio-api/wrangler.jsonc`). Lower to 5s for snappier list-after-save UX → ~6x more bursts → €1.70 → ~€10/mo at alpha. Trivial difference, defer until beta if needed.

### 2.5 Worst case — all users max the 20 GB cap

(Practically impossible at current usage patterns. Storage assumes One Zone, PG storage is separate and tiny.)

| Users | Storage S3 (20GB × N × €0.00803) | PG (same as §2.4) | LanceDB | Total infra | €/user/mo |
|---|---|---|---|---|---|
| 100 | €16.06 | €6.79 | €6.55 | **€29.40** | €0.29 |
| 1,000 | €160.60 | €20.36 | €6.55 | **€187.51** | €0.19 |
| 10,000 | €1,606 | €81.43 | €28.79 | **€1,716** | €0.17 |

Even with 10k users all maxed (impossible scenario): **<€0.20/user/month**. The 20 GB cap is safe to offer for free.

### 2.6 Scaling triggers (when to upgrade tiers)

| Signal | Action | Cost delta |
|---|---|---|
| >10k experiments/day | PG Serverless → PG Dedicated | +€10-15/month |
| >1k LanceDB queries/day | DEV1-S → BASIC3-X2C-4G | +€22/month |
| >75 TB egress/month | Investigate ingress vs CDN | TBD |
| >10k active users/day | Re-evaluate PG sizing | TBD |

---

## 3a. RNA-seq compute VMs — current state + per-organism roadmap

**Worker**: `aspis-lab/cloudflare/aspis-bio-rnaseq-api/`.

### 3a.1 Current behaviour (verified in `src/provider/scaleway.mjs:182`)

```javascript
const commercialType = cleanScalewayCommercialType(env?.RNA_SCALEWAY_INSTANCE_TYPE) || "GP1-XS";
const SCALEWAY_INSTANCE_ALLOWED_TYPES = new Set(["GP1-XS", "GP1-S"]);
```

`wrangler.jsonc`: `"RNA_SCALEWAY_INSTANCE_TYPE": "GP1-S"` (default).

**Truth**: every RNA-seq job today, regardless of organism, gets the **same GP1-S VM** (4 vCPU, 16 GB RAM). The allowlist only accepts GP1-XS or GP1-S.

There is **NO per-organism instance scaling** in the current code despite the difference in reference-genome sizes (`src/lib/config.mjs`):
- C. elegans: 4 GB reference
- Drosophila: 6 GB reference
- **Zebrafish: 12 GB reference** ← needs more RAM at runtime in practice

### 3a.2 Current cost per RNA-seq job

GP1-S new price (June 2026): **€0.1907/hour**.

Typical job wall time: 1.5-4 hours (depending on samples + organism). Average ~2.5 h.
- Cost per job: **€0.48 average**
- Limits: `DEFAULT_MAX_MONTHLY_JOBS = 10` per user (config.mjs:23)
- Per-user max RNA-seq compute: 10 jobs × €0.48 = **€4.80/user/month worst case**

At 100 active RNA-seq users (each at full quota): **€480/month** — significant cost driver.

### 3a.3 Work-amount scaling — proposed (NOT in code today)

**The right scaling factor is NOT organism alone — it's the actual job workload**: input FASTQ bytes, sample count, reference genome size, and whether the user requested heavy post-processing (deconvolution, custom DEG re-analysis). 40 GB of Drosophila FASTQs will OOM GP1-S the same way Zebrafish would.

Proposed selection logic (compute the **largest** of the three constraints, then pick the smallest VM that satisfies all three):

```
peak_memory_gb = max(
  2 × reference_bytes_gb,        // STAR/Salmon index in memory
  0.5 × total_fastq_bytes_gb,    // alignment working set
  4 GB                            // baseline
)
peak_vcpu = max(
  ceil(sample_count / 2),         // parallel alignment threads
  2
)
```

Then pick from:

| Tier | Instance | RAM | vCPU | New price/h | When |
|---|---|---|---|---|---|
| XS | **GP1-XS** | 8 GB | 2 | €0.0928 | Small jobs: <2 samples × <5 GB FASTQ AND C. elegans/worm |
| S | **GP1-S** (current default) | 16 GB | 4 | €0.1907 | Standard: 2-6 samples × <15 GB FASTQ AND Drosophila/small |
| M | **GP1-M** | 32 GB | 8 | €0.3835 | Heavy: any of: Zebrafish AND >2 samples, OR 6-12 samples, OR >15 GB FASTQ |
| L | **GP1-L** | 64 GB | 16 | €0.7742 | Big: >12 samples, OR >40 GB FASTQ, OR custom genome > 8 GB |
| XL | **GP1-XL** | 128 GB | 32 | €1.6738 | Opt-in only: large cohort / multi-org studies |

Worked examples (per-job cost at typical 2.5 h wall time):

| Job | Today (forced GP1-S, may OOM) | Proposed | Saving / sanity gain |
|---|---|---|---|
| 4× C. elegans, 8 GB FASTQ | €0.48 | GP1-XS €0.23 | **52% cheaper** |
| 6× Drosophila, 12 GB FASTQ | €0.48 | GP1-S €0.48 | same — happy path stays cheap |
| 4× Zebrafish, 20 GB FASTQ | **OOM crash** | GP1-M €0.96 | **completes instead of failing** |
| 8× Drosophila, 40 GB FASTQ | **OOM crash** | GP1-L €1.94 | **completes** |

Code change required (~2 hours, no audit risk):
- Extend `SCALEWAY_INSTANCE_ALLOWED_TYPES` to `{GP1-XS, GP1-S, GP1-M, GP1-L, GP1-XL}`
- Add `pickInstanceForJob(samples, totalFastqBytes, organismLabel)` helper in `src/lib/runtime.mjs`
- Plumb result through `startScalewayJobRun` (currently env-var only)
- Surface chosen tier in `job_views.mjs` so users see "running on GP1-M (32 GB)" in the UI
- Add cost preview in `preflight/validate.mjs`: "this job will cost ~€X.YZ"
- Audit cap: refuse XL tier unless `env.RNA_ALLOW_XL === "true"` (avoid surprise €5 jobs)

Cost impact at 100 RNA-seq users × 10 jobs/month, mix of 25% XS / 50% S / 20% M / 5% L:
- Today: 1000 × €0.48 = €480 (and N% fail with OOM)
- After: 250×€0.23 + 500×€0.48 + 200×€0.96 + 50×€1.94 = €57.50 + €240 + €192 + €97 = **€586.50** (zero OOM)

Roughly +22% spend BUT:
- Zero OOM failures (users don't lose 2 hours of compute)
- Small jobs cost 52% less (better unit economics)
- Bigger jobs become possible at all

**Filed as roadmap item — RNA-seq instance-tiering**. Implementation: half a day including UI surface + cost preview.

## 3b. Orasis containers + GPU

**Worker**: `aspis-lab/cloudflare/Orasis/`, containers in `aspis-biovision/Orasis/containers/`.

### 3b.1 Scaleway Serverless Containers (always live, scale-to-zero per request)

Per `aspis-biovision/Orasis/PROJECT.md` (and memory `orasis_scaleway_deployed_2026_05_19`), four containers are LIVE in the `aspis-biovision` namespace, fr-par:

| Container | Purpose | Mem allocation | Typical wall time/call |
|---|---|---|---|
| `orasis-cellpose-cpu` | CPSAM + cyto3 + nuclei + MyoFuse | ~2 GB | 30-180 s |
| `orasis-fiji-normalize` | Fiji image normalization | ~1 GB | 5-30 s |
| `orasis-features` | Phenotype + measurement features | ~1 GB | 5-15 s |
| `orasis-ilastik` | Ilastik pipelines | ~2 GB | 30-90 s |

**Pricing (June 2026, doubled)**: €0.000002/GB-s memory + €0.00001/vCPU-s.

Typical full Orasis pipeline (1 cell image):
- fiji 10s × 1 GB + 0.5 vCPU = €0.000025
- cellpose 60s × 2 GB + 1 vCPU = €0.00084
- features 10s × 1 GB + 0.5 vCPU = €0.000025
- **Total per image: ~€0.001**

**Free tier**: 400k GB-s memory + 200k vCPU-s/month COMBINED across all serverless containers + jobs. Translates to roughly **~10,000 typical Orasis pipeline runs/month free** at current sizing.

Below ~10,000 pipeline runs/month → **€0**. Above that, each additional run is ~€0.001.

### 3b.2 GPU L4 ephemeral (for cellpose-gpu)

Per memory `orasis_gpu_live_2026_05_28`: snapshot 6f923b3a, image+snapshot ready, cellpose-gpu serves cpsam via tunnel `gpu.aspis-bio.com`.

2026-05-29 live check from Aspis Management: no Scaleway Instance VMs are currently running or stopped in the Aspis Bio project after the smoke lifecycle cleanup. This section models the ephemeral GPU path; it is not evidence of an always-on VM.

| Resource | Pricing (June 2026) | Notes |
|---|---|---|
| **L4-1-24G** | €0.792/h | Ephemeral — terminate after each run per memory `scaleway_billing_terminate_not_stop` |
| Snapshot storage when L4 off | €0.000049/GB/h × 25 GB | €0.89/month idle |
| L4-2-24G upgrade | €1.578/h | For batch processing in M11 |

Usage pattern:
- Lifecycle gate (`src/gpu_lifecycle.ts`) boots L4 on first request, keeps alive ~5 min after last request, then terminates
- 10 GPU requests/day clustered into ~3 boot cycles × 8 min = 24 min/day = 12 h/month
- **L4 cost: 12 × €0.792 = €9.50/month** at current alpha usage
- Snapshot: €0.89/month
- **Total GPU: ~€10.40/month**

Scaling triggers:
- >100 GPU requests/day → consider always-on L4 (€578/month) only if usage justifies it
- >500 GPU requests/day → batch mode + L4-2 with parallel inference
---

## 4. Cloudflare resources — actual inventory + costs

Pulled live from `wrangler` against account `8a991014729a52a958cef2c5cbf0de50` (Gualtieriuser09).

### 4.1 Workers deployed (8 total in Aspis Bio scope)

| Worker | Purpose | Custom domain |
|---|---|---|
| `aspis-bio-api` | Auth, account, labbook, **Vault (NOT YET LIVE — VAULT_ENABLED=false)** | `api.aspis-bio.com` |
| `aspis-biovision-worker` | Gel/WB analysis, classify-lab-image, VLM routing | `biovision-api.aspis-bio.com` |
| `orasis-worker` | Cell segmentation orchestration, AI orchestrator | `orasis-api.aspis-bio.com` |
| `aspis-bio-rnaseq-api` | RNA-seq job dispatching + assist | `rnaseq-api.aspis-bio.com` |
| **`aspis-bio-papers`** | **Self-contained Paper RAG (embedding + retrieval, hosted on CF AutoRAG)** | `papers.aspis-bio.com` |
| `aspis-bio-oauth` | OAuth state broker (Dropbox + Google) | — |
| `aspis-bio-mta-sts` | Email security policy (MTA-STS for aspis-bio.com domain) | — |
| `aspis-bio-resend-webhooks` | Resend bounce/complaint webhook ingester | — |

**Workers Paid plan**: $5/month base, 10M requests included, $0.30 per additional 1M requests, $12.50/M CPU-ms after 30M included CPU-ms.

Current usage estimate (across all 8 workers): well under 10M requests/month at alpha. Plan covers it entirely. Marginal cost: $0 today.

### 4.2 R2 object storage (11 buckets total, 2 in Aspis Bio scope)

| Bucket | Location | Size | Objects | Used by |
|---|---|---|---|---|
| `aspis-bio-papers` | WEUR | **59.8 MB** | 2,094 | Paper RAG corpus (PDFs + extracted text) |
| `aspis-bio-rna-runs` | EEUR | 0 B | 0 | (a) Lab book attachments under `labbook/{sub_hash}/...` (when server sync enabled — never used today), (b) legacy RNA-seq "Tier 1" packaging under `rnaseq/users/{sub_hash}/...` (dead path; moderne flow goes through `aspis-bio-rnaseq-api` worker which writes to Scaleway) |

**Note on RNA-seq result storage**: counts + DEG + reports go to **Scaleway** Object Storage bucket `aspis-rnaseq-v0` (`fr-par`), configured via `aspis-bio-rnaseq-api/wrangler.jsonc:10` (`RNA_SCALEWAY_BUCKET`). The VM writes results under `users/{sub_hash}/jobs/{job_id}/artifacts/` directly via aws4fetch.

**Important**: the pipeline uses **Salmon (pseudo-alignment), NOT STAR** — so **no BAM files are produced**. Quantification is done via `quant.sf` (transcript-level counts, ~1-5 MB per sample). Plus the VM runs `cleanup_policy.js` after the run, which deletes `input/raw/`, `tmp/`, `work/` BEFORE uploading artifacts — so raw FASTQs never reach the bucket either. Only `artifacts/` is preserved + uploaded.

Realistic per-job output size: **80-150 MB** (NOT 15-30 GB as initially estimated — that figure assumed STAR/BAM which is not our pipeline):
- Salmon quant.sf × samples: ~20 MB
- DESeq2 report: ~10 MB
- MultiQC HTML: ~30-50 MB
- FastQC ZIPs × samples: ~30 MB
- Plots + logs + manifest: <5 MB

Retention declared in `src/lib/job_package.mjs:133` (30d artifacts, 7d raw input) is now **enforced server-side** by the lifecycle rules applied 2026-05-28 (see R1 below).

✅ **FIXED 2026-05-28**: 2 lifecycle rules applied via console — `user-jobs-30d` (prefix `users/`, 30d expiration) + `abort-incomplete-multipart-1d` (all, 1d). 30-day retention now enforced server-side. Orasis bucket lifecycle rules are also complete (see R1).

Other R2 buckets on the account (`aspis-dishes`, `aspis-music`, `aspis-icons`, `aspis-overture`, `aspis-news-images`, `aspis-generated-images`, `aspis-us`) belong to **separate sibling projects** — NOT Aspis Bio scope. Listed here just so we don't accidentally migrate or delete them while doing Aspis Bio infra work.

**R2 pricing (current, not on the June 2026 list since R2 is CF not Scaleway)**:
- Storage: **first 10 GB/month FREE**, then $0.015/GB/month
- Class A operations (write/list): first 1M FREE, then $4.50/M
- Class B operations (read): first 10M FREE, then $0.36/M
- Egress: **FREE** (the CF selling point vs S3)

**Current R2 cost: €0/month**. We are nowhere near the 10 GB free tier.

When RNA-seq runs start landing in `aspis-bio-rna-runs`: each run keeps a few GB of outputs (BAM, counts, MultiQC reports). At 100 jobs × 2 GB avg = 200 GB → ~$3/month at $0.015/GB. Still trivial.

### 4.3 KV namespaces (9 in Aspis Bio scope, 17 total on account)

| Namespace | ID | Bound to | Purpose |
|---|---|---|---|
| `ACCOUNT_PROFILE_KV` | bb37a7ea... | (TBD) | User profile metadata |
| `API_CACHE` | 42fa7593... | aspis-bio-api | API response cache |
| `ASPIS_BIO_OAUTH_STATE` | 641f645b... | aspis-bio-oauth | OAuth flow state |
| `aspis-biovision-worker-aspis_biovision_quota` | 26633337... | aspis-biovision-worker | Per-user biovision quota (KV fallback when DO unavailable) |
| `aspis-food-worker-FOOD_CACHE` | 04e3a395... | aspis-food-worker (separate project? verify) | Food worker cache — OUT OF SCOPE if not Aspis Bio |
| `ASPIS_RATE_LIMIT` | — | (TBD) | Global rate limit shared |
| `BIO_EMAIL_SUPPRESSION` | 03b4cd3f... | aspis-bio-api + resend-webhooks | Email bounce/complaint suppressions |
| `ORASIS_RUNS_META` | 286b742b... | orasis-worker | Run metadata cache |
| (preview namespaces for several of the above — dev/test only) | | | |

**KV pricing**:
- Storage: 1 GB FREE, then $0.50/GB/month
- Reads: 100k/day FREE, then $0.50/M
- Writes: 1k/day FREE, then $5.00/M

**Current KV cost: €0/month**. All workloads are within free tiers.

Watch: if Vault auto-suggest grouping triggers many KV reads per save, may consume free tier faster.

### 4.4 Durable Objects (across multiple workers)

Per wrangler.jsonc inspection:
- `aspis-bio-api`: `FREE_AI_QUOTA` (FreeAiQuotaGate), `RATE_LIMIT_GATE` (RateLimitGate)
- `aspis-biovision-worker`: `BIOVISION_RATE_LIMIT_GATE` (sqlite-backed)
- `aspis-bio-rnaseq-api`: rate-limit + job-lock DOs
- `orasis-worker`: `ORASIS_RATE_LIMIT_GATE`, `ORASIS_USAGE_GATE`, `ORASIS_BATCH_ORCHESTRATOR`, `ORASIS_GPU_GATE`

**DO pricing**:
- Requests: 1M/month FREE per DO, then $0.15/M
- Duration: 400k GB-s/month FREE, then $12.50/M GB-s
- Storage (SQLite-backed): $0.20/GB/month after 1 GB free

**Current DO cost: €0/month**. All within free tier at alpha volumes.

### 4.5 AI Search (AutoRAG) — Paper RAG self-contained

| Index | Namespace | Type | Source | Status | Created |
|---|---|---|---|---|---|
| `aspis-bio-papers` | default | r2 | `aspis-bio-papers` bucket | **waiting** (ingesting) | 2026-05-12 |

This is **CF's AutoRAG**: automatic embedding + vector index + retrieval, fed directly from an R2 bucket. The Paper RAG system uses it instead of running our own embedding pipeline.

The `aspis-bio-papers` worker:
- Receives queries from `aspis-bio-api` (via `PAPERS_RAG_URL` env)
- Forwards to AutoRAG `PAPER_SEARCH` binding
- Returns ranked passages + citations
- Also exposes `AI` binding for direct Workers AI calls (uses `@cf/zai-org/glm-4.7-flash` per wrangler.jsonc comments)

**AutoRAG pricing** (per CF current docs):
- Indexing: per source-byte ingested (~$0.10 per 1M tokens embedded, one-off per document)
- Queries: per search request + Workers AI tokens for the embedding model
- Storage: included in R2 bucket cost

Current state: **59.8 MB of papers (2,094 objects)** = roughly 30M tokens. One-off indexing cost: **~$3 total**, already paid (or being paid during the "waiting" sync now).

2026-05-29 Aspis Management live check: Cloudflare REST returns AI Search namespace `default`, AI Search instance `aspis-bio-papers`, and legacy AutoRAG jobs/files for `aspis-bio-papers`. The app provider console now lists the AI Search namespace/instance directly.

Per query: 1 embedding call (~$0.0001) + retrieval (negligible). At 100 queries/day: **~$0.30/month**.

**Decision: keep Paper RAG on CF AutoRAG**. Per the user (2026-05-28): "lo lascerei lì perché faceva tutto lui (embedding + retrieval)". It's self-contained, working, and the alternative (porting to LanceDB on Scaleway) would mean re-running embedding + losing the AutoRAG synchronization features. Cost is negligible.

### 4.6 Total Cloudflare bill today

| Line item | Cost |
|---|---|
| Workers Paid plan base | $5/month |
| R2 storage (59.8 MB) | $0 (under 10 GB free) |
| R2 operations | $0 (under free tier) |
| KV (all namespaces) | $0 (under free tier) |
| Durable Objects (all workers) | $0 (under free tier) |
| AI Search (paper indexing one-off) | ~$3 one-off + ~$0.30/month queries |
| Workers AI (paper RAG `glm-4.7-flash` calls) | ~$1-2/month |
| **Total Cloudflare today** | **~$5-8/month (€4-7)** |

### 4.7 Projection at 1,000 active users (vault live + papers used)

| Line item | Cost |
|---|---|
| Workers Paid plan base | $5/month |
| R2 storage (RNA-seq runs ~200 GB) | ~$3/month |
| Workers requests (~50M/month) | ~$12/month |
| KV reads (Vault auto-suggest grouping) | ~$1/month |
| AI Search queries (10 × today) | ~$3/month |
| Workers AI (paper RAG, more queries) | ~$10/month |
| **Total Cloudflare at 1k users** | **~$34/month (€30)** |

---

## 5. AI inference pricing — per-token rates

### 5.1 Provider matrix (only Scaleway + Infomaniak per PRIV-03)

Both are EU-resident, ZDR-compliant, GDPR-aligned. No OpenAI, Anthropic, GCP direct.

**Scaleway Generative APIs (Paris, serverless):**

| Model ID | Input €/1M | Output €/1M | Capabilities | Notes |
|---|---|---|---|---|
| **gemma-4-26b-a4b-it** | **€0.25** | **€0.50** | Chat, Vision | Used by biovision worker (VLM gel classification per memory `feedback_vlm_choice_gemma4`) |
| **gemma-3-27b-it** | €0.25 | €0.50 | Chat, Vision | Alternative to gemma-4 |
| mistral-small-3.2-24b-instruct-2506 | €0.15 | €0.35 | Chat, Vision | Default RNA-seq assist (per `aspis-bio-rnaseq-api/src/lib/config.mjs` `DEFAULT_ASSIST_MODEL`) |
| qwen3.6-35b-a3b | €0.25 | €1.50 | Chat, Vision | |
| mistral-medium-3.5-128b | €1.50 | €7.50 | Chat, Vision | Expensive — use only for hard tasks |
| qwen3-coder-30b-a3b-instruct | €0.20 | €0.80 | Chat, code | |
| pixtral-12b-2409 | €0.20 | €0.20 | Chat, Vision | Cheapest vision-capable |
| gpt-oss-120b | €0.15 | €0.60 | Chat | |
| qwen3-embedding-8b | €0.10 | Free | Embeddings | If we ever want non-deterministic embeddings |
| bge-multilingual-gemma2 | €0.10 | Free | Embeddings | EU-friendly embedding alternative |
| llama-3.3-70b-instruct | €0.90 | €0.90 | Chat | |
| whisper-large-v3 | €0.003/min audio | Free | Transcription | If we ever wire voice notes |

Free tier: **1M tokens + 60 min audio /month FREE**. Batch requests: **50% discount**.

**Infomaniak AI Tools (Geneva, OpenAI-compatible):**

| Model ID | Input CHF/1M (≈ EUR) | Output CHF/1M (≈ EUR) | Notes |
|---|---|---|---|
| **google/gemma-4-31B-it** | **CHF 0.20 (≈ €0.21)** | **CHF 0.40 (≈ €0.42)** | Used by Orasis orchestrator (`ai_orchestrate.ts::callGemma`); product id 108646 |

CHF→EUR conversion approx 1.05; verify on Infomaniak invoice. No public free tier confirmed.

### 5.2 Side-by-side comparison (Gemma class)

For an equivalent ~30B Gemma:
- **Scaleway gemma-4-26b-a4b**: €0.25 in / €0.50 out per million → most generous output:input
- **Infomaniak gemma-4-31B**: ≈€0.21 in / €0.42 out per million → slightly cheaper, slightly bigger model
- Infomaniak is ~15% cheaper at current rates. Scaleway has the free tier (1M tokens/month).

**Recommendation**: stay on **Infomaniak for the Orasis orchestrator** (already wired), use **Scaleway for biovision VLM gel calls** (already wired), keep both for redundancy.

### 5.3 Current usage + projection

**Today (no Vault co-scientist yet):**

| Worker | Model | Estimated monthly tokens | Cost |
|---|---|---|---|
| Orasis orchestrator (Infomaniak) | gemma-4-31B | ~150k tokens (5 calls/day × ~1k) | <€0.10/month |
| Biovision VLM (Scaleway) | gemma-4-26b | ~500k tokens (ad-hoc image analysis) | <€0.30/month + free tier covers it |
| RNA-seq assist (Scaleway) | mistral-small-3.2-24b | ~200k tokens | covered by free tier |
| **TOTAL AI today** | | | **<€1/month** |

**Projection after Vault F5 live + 100 active users:**

Per memory `experiment_vault_v1_f0_2026_05_28`, smoke 2 & 3 measured 4-5 tool round-trips per co-scientist query with ~3-4k tokens average per session (in 1500 in, ~700 out per round-trip).

- 100 users × 10 queries/day × 4k tokens (3.2k in + 0.8k out per session)
- Daily: 320k in + 80k out = 400k tokens
- Monthly: ~12M tokens (9.6M in + 2.4M out)
- Infomaniak gemma-4-31B: 9.6 × €0.21 + 2.4 × €0.42 = **~€3.02/month**

That's much cheaper than my earlier €30-50 estimate (which used wrong per-token rates). Per-user AI cost is essentially **€0.03/month at 100 users**.

**Projection at 1,000 users:** ~€30/month. **At 10,000 users:** ~€300/month.

Rate-limit lever: 50 tool calls/session enforced in `vault_tools_handler.ts`. If we want a hard monthly cap, add KV-tracked per-user counter (not currently in code — add when usage demands).

### 5.4 Cost-cutting levers

1. **Batch mode on Scaleway**: 50% discount. Useful for nightly re-indexing or bulk fingerprint computation if we ever need it.
2. **Embedding model swap**: if we ever drop deterministic fingerprints for some assays, `qwen3-embedding-8b` at €0.10/1M and free output is the cheapest EU embedding.
3. **Mistral-small for non-vision tasks**: €0.15/€0.35 vs Gemma €0.25/€0.50 → 40% saving on text-only AI tasks. Already used for RNA-seq assist.
4. **Cap per-user monthly tokens**: add hard limit in `vault_tools_handler.ts` rate-limit map (e.g. 100k tokens/user/month free, then refuse). Not implemented today.

---

## 6. Scaleway pricing reference — June 1st 2026

Full schedule for all products we use or might consider. Values in EUR.

### 6.1 Compute & instances

| Product | Hourly (new) | Monthly equiv (730h) | Notes |
|---|---|---|---|
| STARDUST1-S | €0.0006 | €0.44 | Tiny — testing only |
| DEV1-S | **€0.00898** | **€6.55** | **LanceDB proxy choice** |
| DEV1-M | €0.0202 | €14.75 | |
| DEV1-L | €0.04284 | €31.27 | |
| DEV1-XL | €0.06508 | €47.51 | |
| PLAY2-PICO | €0.01428 | €10.42 | |
| PLAY2-NANO | €0.02754 | €20.10 | |
| PLAY2-MICRO | €0.05508 | €40.21 | |
| BASIC3-X2C-4G | €0.03945 | €28.79 | LanceDB scale-up target |
| BASIC3-X2C-8G | €0.05923 | €43.24 | |
| BASIC3-X4C-8G | €0.079 | €57.67 | |
| BASIC3-X4C-16G | €0.11845 | €86.47 | |
| BASIC3-X8C-16G | €0.17768 | €129.71 | |
| BASIC3-X8C-32G | €0.2368 | €172.86 | |
| BASIC3-X16C-32G | €0.35525 | €259.33 | |
| BASIC3-X16C-64G | €0.47329 | €345.50 | |
| GP1-XS | €0.0928 | €67.74 | |
| GP1-S | €0.1907 | €139.21 | |
| GP1-M | €0.3835 | €279.96 | |
| GP1-L | €0.7742 | €565.17 | |
| GP1-XL | €1.6738 | €1,221.87 | |
| PRO2-XXS | €0.0561 | €40.95 | |
| PRO2-XS | €0.1122 | €81.91 | |
| PRO2-S | €0.2234 | €163.08 | |
| PRO2-M | €0.4468 | €326.16 | |
| PRO2-L | €0.8945 | €652.99 | |

### 6.2 GPU (on-demand, terminate after use)

| Product | Hourly (new) | Notes |
|---|---|---|
| **L4-1-24G** | **€0.792** | **Current Orasis GPU** |
| L4-2-24G | €1.578 | |
| L4-4-24G | €3.15 | |
| L4-8-24G | €6.30 | |
| L40S-1-48G | €1.47 | Bigger models |
| L40S-2-48G | €2.94 | |
| L40S-4-48G | €5.88 | |
| L40S-8-48G | €11.76 | |
| H100-1-80G | €2.868 | Training only |
| H100-2-80G | €5.736 | |
| H100-SXM-2-80G | €6.624 | |
| H100-SXM-4-80G | €12.774 | |
| H100-SXM-8-80G | €25.332 | Avoid unless training big |

### 6.3 Storage

| Product | Pricing (new) | Per GB / month | Notes |
|---|---|---|---|
| Compute Local Storage | €0.000049/GB/h | €0.0358 | |
| Compute Snapshot Local Storage | €0.000049/GB/h | €0.0358 | |
| Block Storage Snapshot | €0.000049/GB/h | €0.0358 | |
| Block Storage Volume Low Latency 5K | €0.00013/GB/h | €0.0949 | |
| Block Storage Volume SSD | €0.00013/GB/h | €0.0949 | |
| Distributed Data Lab — Volume disk | €0.000136/GB/h | €0.0993 | |
| BROKERNODE-STORAGE-SBS-5K | €0.000136/GB/h | €0.0993 | |
| SEARCHDB-STORAGE SBS 5K | €0.000136/GB/h | €0.0993 | |
| **Object Storage Multi-AZ** | **€0.000022/GB/h** | **€0.01606** | Critical data |
| **Object Storage One Zone** | **€0.000011/GB/h** | **€0.00803** | **Vault default** |

### 6.4 Serverless

| Product | Pricing (new June 2026) | Notes |
|---|---|---|
| Serverless Jobs — Memory | €0.000002/GB-s | Doubled from €0.000001 |
| Serverless Jobs — vCPU | €0.00001/vCPU-s | unchanged |
| Serverless Containers — Memory | €0.000002/GB-s | Doubled from €0.000001 |
| Serverless Containers — vCPU | €0.00001/vCPU-s | unchanged |
| Serverless Function — Provision | €0.000005/GB-s | 39% up |
| Serverless Function — Consumption | €0.000017/GB-s | 42% up |
| **Serverless SQL Database — Compute** | **€0.13572/vCPU/h active** | 4 GB RAM per vCPU; scale-to-zero; min 5-min billed window per burst |
| **Serverless SQL Database — Storage** | **€0.000272/GB/h** (€0.199/GB/month) | Daily backup 7-day retention FREE |
| **Free tier (Containers + Jobs combined, per month)** | 400k GB-s memory + 200k vCPU-s | Useful for LanceDB-as-container MVP |
| **Free tier (Functions, per month)** | 1M requests + 400k GB-s + provisioning | |

**Heads up**:
- Container/Jobs memory price **doubled June 2026**. Re-evaluate Orasis containers (cellpose-cpu / fiji / features / ilastik) — they may now be more expensive than a small always-on instance for steady traffic.
- Serverless SQL DB compute pricing **was not in the user-supplied schedule**; sourced from `scaleway.com/en/pricing/managed-databases/` directly. Verify before deploy.

### 6.5 Network & misc

| Product | Pricing (new) |
|---|---|
| Bare Metal Flexible IPv4 | €0.005/h (€3.65/month) |
| Zonal Flexible IPv4 | €0.005/h |
| VPC Public Gateway IP | €0.005/h |
| VPC Public Gateway M | €0.095/h (€69.35/month) |
| VPC Public Gateway S | €0.026/h (€18.98/month) |
| Load Balancer IP | €0.005/h |
| Load Balancer GP-M | €0.054/h (€39.42/month) |
| Load Balancer GP-S | €0.023/h (€16.79/month) |
| DNS zone for external domains | €0.007/h (€5.11/month) ⚠️ 7× increase from €0.001/h |
| Key Manager — Version Storage | €0.06/KEY/month |

**Heads up**: DNS zone external domains went 7× up. If we run more than a few zones, this stacks fast — keep DNS on Cloudflare where possible.

### 6.6 Dedibox (bare metal — not currently used)

| Product | Monthly (new) |
|---|---|
| Dedibox Core-10-XS | €134.99 |
| Dedibox Core-10-S | €229.99 |
| Dedibox Core-10-M | €349.99 |
| Dedibox Core-10-L | €449.99 |
| Dedibox Core-10-XL | €569.99 |
| Dedibox Core-10-XL-31T | €739.99 |
| Dedibox Core-10-XXL | €679.99 |

| Product | Hourly (new) | Monthly (new) |
|---|---|---|
| EM-I120E-NVMe | €0.37 | €134.99 |
| EM-I220E-NVMe | €0.63 | €229.99 |
| EM-I320E-NVMe | €0.959 | €349.99 |
| EM-I420E-NVMe | €1.233 | €449.99 |
| EM-I520E-NVMe | €1.562 | €569.99 |
| EM-I525E-NVMe | €2.027 | €739.99 |
| EM-I620E-NVMe | €1.863 | €679.99 |
| EM-L520E-NVMe | €1.233 | €449.99 |

---

## 7. Cost-watch alerts (manual today; automate when scale demands)

| Trigger | What to check | Action |
|---|---|---|
| Monthly Scaleway bill > €100 | Per-product breakdown in Scaleway console | Identify top 3 line items, justify or downgrade |
| L4 GPU bill > €50/month | Is `gpu_lifecycle.ts` terminating properly? | Inspect for orphan instances (memory: `scaleway_billing_terminate_not_stop`) |
| LanceDB DEV1-S CPU >80% sustained | Query rate must be high | Upgrade to BASIC3-X2C-4G (+€22/mo) |
| PG Serverless active-CU >1.5 avg | Vault saves spiking | Either accept or move to dedicated tier |
| Object Storage > 1 TB total | Power users above expectations | Re-evaluate per-user cap; consider charging beyond 20 GB |
| Egress > 50 TB/month | Approaching 75 TB free limit | Investigate which workload (probably GPU model pulls) |

---

## 8. Roadmap — infrastructure & cost cleanups

Tracked work, ordered by impact-per-hour. Each item has a clear deliverable and a "blocked by" gate.

### R1. Configure Scaleway bucket lifecycle rules — ✅ FULLY DONE 2026-05-28

All 4 Scaleway Object Storage buckets in project Aspis Bio (fr-par) now have lifecycle rules:

| Bucket | Rules | Effective policy |
|---|---|---|
| `aspis-rnaseq-v0` | `user-jobs-30d` (prefix `users/`, 30d) + `abort-incomplete-multipart-1d` (all, 1d) | RNA-seq outputs deleted after 30 days |
| `orasis-raw` | `inputs-5d` (prefix `inputs/`, 5d) + `uploads-5d` (prefix `uploads/`, 5d) + `abort-incomplete-multipart-1d` (all, 1d) | Raw cell images deleted after 5 days (covers both legacy + post-M11 prefixes) |
| `orasis-processed` | `processed-30d` (all objects, 30d) + `abort-incomplete-multipart-1d` (all, 1d) | Cell segmentation outputs deleted after 30 days |
| `orasis-previews` | `previews-30d` (all objects, 30d) + `abort-incomplete-multipart-1d` (all, 1d) | Thumbnails deleted after 30 days |

Storage growth now bounded. The `Object Storage > 1 TB total` cost-watch trigger in §7 stays as-is but is now essentially unreachable (steady-state projected at ~25 GB across all 4 buckets at 100 users/100 jobs per month per modality).

Pattern asymmetry: `aspis-rnaseq-v0` + `orasis-raw` use prefix scoping; `orasis-processed` + `orasis-previews` use bucket-wide "all objects". Functionally equivalent today because all 4 buckets contain only user data; documented for future reference.

**Original problem**: declared retention in `job_package.mjs:133` (30d artifacts, 7d raw input) was cosmetic until rules applied. This is now closed: live bucket lifecycle checks confirm retention and multipart-abort rules across RNA-seq and Orasis buckets. No pending console action remains for R1.

### R2. RNA-seq work-amount instance tiering — ✅ DONE 2026-05-28
Per-job picker shipped in `aspis-bio-rnaseq-api/src/lib/instance_tier.mjs` + wired in `src/provider/scaleway.mjs:430-447`. Allowlist expanded to 5 GP1 tiers. XL refusal gated by `RNA_ALLOW_XL` env. `instanceTier` surfaced in dispatch response. Plus disk hardening: bareDelete `with_volumes=all` fix, `powerOffScalewayInstance` helper (volume-preserving stop for failure recovery), `findOrphanScalewayVolumes` audit + admin endpoint `GET /api/v0/aspis-rna-seq/admin/orphan-volumes?hours=24`.

### R3. Lab Book attachments → Scaleway — ✅ DONE 2026-05-28
Migrated `labbookPutAttachment / Get / Head / Delete / DeletePrefix` in `aspis-bio-api/src/worker.mjs` from `env.RNA_RUN_BUCKET` (R2) to inline aws4fetch helpers against Scaleway `aspis-vault-eu` bucket using `VAULT_S3_*` env vars. Cascading user-delete also uses the new prefix delete with hard guards (key.startsWith(prefix) + safe-character regex) per audit finding. Zero data migration needed (R2 bucket was 0 B confirmed).
Status: code-complete + audit-clean. Blocked on Vault F1 deploy to start working (until VAULT_S3_BUCKET secret is set, helpers return null and routes 503 — same UX as pre-change).

### R4. Retire dead R2 path `rnaseq/users/*` in aspis-bio-api worker — ✅ DONE 2026-05-28
Deleted `persistTier1RunPackage` + `cleanupTier1RunPackage` + their callers. `aspis-bio-api` no longer writes Tier1 RNA-seq packages to R2. Audit confirmed zero leftover references to `tier1.storage` across the codebase.

### R5. RNA-seq packaging files → Scaleway — ✅ ALREADY DONE
Audit revealed this was a non-existent problem: `aspis-bio-rnaseq-api` (the active dispatch worker) ALREADY writes samplesheet+manifest to Scaleway `aspis-rnaseq-v0` via aws4fetch (see `src/provider/scaleway.mjs:407` providerPackageKey). The dead R4 path in `aspis-bio-api` was the only R2 RNA-seq write, and it's gone with R4.

### R9b. Data page wired to Vault backend — ✅ DONE 2026-05-28
Wired `aspis-lab/cloudflare/Aspis-bio-website/public/account/data/{data-items,app}.js` to fetch from `https://api.aspis-bio.com/v1/vault/experiments` using `AspisAccountShell.ensureSession()` for JWT. Mutates shared `DATA_ITEMS` array reference in place so view-data.js destructured local sees new contents. Fallback to MOCK_DATA_ITEMS with banner if vault returns 503 / 401 / network error. CSP-clean (no inline, no eval, no CDN). Cache-bust bumped to `20260528vault1` on all 5 versioned refs in `data.html`. Audit fixed 1 BLOCKER (XML key-injection in deletePrefix) + 1 HIGH (encodeURI → encodeURIComponent per-segment) + 1 HIGH (`STORAGE.usedGb` ReferenceError on `_byFamily`).

### R6. Vault F1 cloud apply — Scaleway PG + S3 bucket
**Goal**: turn on the Vault for real.
**What**: run `aspis-biovision/deploy/scaleway-vault/provision.sh` (after user echoes cost estimate per COST-03), apply migrations `0001..0007`, set wrangler secrets, deploy `aspis-bio-api`. Keep `VAULT_ENABLED=false` until smoke verified in prod.
**Effort**: 2 hours including verification. **Blocks**: explicit user "ok provision".
**Recurring cost**: ~€25-30/month at alpha.

### R7. Vault F4 deploy — LanceDB REST proxy
**Goal**: turn on similarity search.
**What**: deploy a Python FastAPI container on Scaleway Serverless Containers (or DEV1-S always-on) fronting LanceDB-on-S3. Implements `/insert /search /delete /delete_user` per the proxy contract in `aspis-bio-api/src/vault/README.md` §F4. Set `VAULT_VECTOR_URL` + `VAULT_VECTOR_TOKEN` secrets.
**Effort**: 1 day (write container + smoke against the existing `find_similar_experiments` tool). **Blocks**: R6.
**Recurring cost**: ~€6.55/month on DEV1-S, or ~€0-2 on Serverless Container with free tier.

### R8. Vault F5 wire — orchestrator integration deployed
**Goal**: AI co-scientist starts using the vault.
**What**: deploy Orasis worker with the wired tool dispatch loop (already coded). Set `ORASIS_TO_API_SECRET` shared secret on both workers via `wrangler secret put`. Flip `VAULT_ENABLED=true` for the test account only first; widen after one week of clean traffic.
**Effort**: 1 hour. **Blocks**: R6 + R7.
**Recurring cost**: ~€3/month at 100 users (Infomaniak Gemma tokens; see §5.3).

### R9. Vault F6 — Data page + Dashboard v2
**Goal**: user can see + drill into their saved experiments.
**What**: new pages under `Aspis-bio-website/public/account/data/` (list view + per-experiment detail). Dashboard v2 widget "your recent experiments + targets you've studied". Strict CSP (no inline scripts, no unpkg) — follow rna-seq.html template.
**Effort**: 2-3 days frontend. **Blocks**: R6.
**Recurring cost**: zero (static frontend).

### R10. Vault F7 — E2E mode + GDPR async delete-job
**Goal**: privacy-paranoid users opt into ciphertext-only sync; GDPR Art.17 with audit trail.
**What**:
- Implement client-side AES-GCM encryption of payload before save (key derived from user lock code, same pattern as Lab Book v2)
- Implement HMAC-on-targets so the server can answer `?target=CD45` lookups without seeing CD45 in cleartext
- Disable AI co-scientist features in E2E mode (UI greyed out with tooltip)
- Convert `DELETE /v1/vault/me` to async job: returns `202 + {job_id}`, persists job in PG, runs as worker `scheduled` cron, exposes `GET /v1/vault/jobs/:job_id` for status. Hard delete within 24h SLA.
- Add account-level toggle `/account/preferences/privacy_mode` (already exists in spec, needs UI).
**Effort**: 1 week. **Blocks**: R6 + user research on whether E2E is actually demanded.
**Recurring cost**: zero marginal (same infra, different code path).

### R11. (Optional) Migrate Paper RAG off CF AutoRAG
**Status**: **explicitly decided NO** by user on 2026-05-28. Keep Paper RAG on CF AutoRAG. Self-contained, ~$0.30/month queries, would lose AutoRAG auto-sync features if ported to LanceDB on Scaleway. Listed here only to record that the decision has been made and not revisit it.

---

## 9. History

- 2026-05-28: Document created. Vault v1 cost model added. Scaleway June 2026 pricing schedule embedded.
- 2026-05-28 (same day, follow-up): Replaced placeholder PG baseline (~€18) with VERIFIED Scaleway Serverless SQL DB pricing from official docs: €0.13572/vCPU/h active + €0.199/GB/month storage + free backups + scale-to-zero with 5-min minimum burst window. Vault total cost re-computed: ~€7/mo alpha, €13/mo beta, €28/mo at 1k users. Added free-tier note for serverless containers (400k GB-s + 200k vCPU-s/month combined Containers+Jobs).
- 2026-05-28 (third pass): Added §5 AI inference pricing matrix (Scaleway Generative APIs full model list + Infomaniak Gemma 4 31B). Added §3a RNA-seq compute VMs with verified current state (flat GP1-S for all organisms — NO per-organism scaling in code today) + proposed roadmap for organism-driven instance types. Added §3b Orasis containers + GPU L4 with realistic usage patterns. Updated TL;DR at 1k users from ~€45 to ~€80-120 to account for AI co-scientist tokens + RNA-seq job compute. Verified `aspis-bio-rnaseq-api/src/provider/scaleway.mjs:18` allowlist is locked to {GP1-XS, GP1-S}.
- 2026-05-28 (fourth pass): Revised §3a from per-organism to **per-job-workload** scaling (memory × samples × FASTQ size), per user feedback that "40GB Drosophila wouldn't fit GP1-S either". Added 5-tier (XS/S/M/L/XL) with concrete examples and zero-OOM guarantee. Expanded §4 from 4-line stub to full Cloudflare inventory pulled live via wrangler against account 8a991014729a52a958cef2c5cbf0de50: 8 workers (incl. oauth/mta-sts/resend-webhooks/papers), 2 Aspis Bio R2 buckets (aspis-bio-papers 59.8MB/2094 obj, aspis-bio-rna-runs 0B), 9 KV namespaces, multiple DOs, and CF AutoRAG `aspis-bio-papers` index (replaces our own embedding pipeline for paper retrieval). Decision logged: **keep Paper RAG on CF AutoRAG** (self-contained, ~$0.30/mo queries, would lose features if ported to LanceDB). Total CF bill today: ~$5-8/month, projects to ~$34/month at 1k users.
- 2026-05-28 (fifth pass): Corrected §4.2 — RNA-seq results do NOT land in R2, they go to Scaleway `aspis-rnaseq-v0` (fr-par) bucket via `RNA_SCALEWAY_BUCKET` env. The `aspis-bio-rna-runs` R2 bucket is for (a) lab book attachments under `labbook/{sub_hash}/...` when server sync enabled (today: nobody), (b) legacy "Tier 1" packaging path in `aspis-bio-api` (dead — modern flow uses `aspis-bio-rnaseq-api` → Scaleway). Verified RNA-seq retention in `job_package.mjs:133`: 30 days for artifacts, 7 days for raw FASTQ scratch — values are DECLARATIVE (passed to VM in manifest); need to confirm matching lifecycle rules on Scaleway bucket to actually enforce them (filed as R1 in §8 Roadmap). Added §8 Roadmap with 11 ordered items R1-R11 covering: bucket lifecycle verification (R1), RNA-seq instance tiering (R2), lab book → Scaleway migration (R3), dead-path cleanup (R4), packaging files → Scaleway (R5), Vault deploy phases F1+F4+F5+F6+F7 (R6-R10), and the explicit DECISION to keep Paper RAG on CF AutoRAG (R11, no-op). Each roadmap item has effort estimate, blockers, and cost impact.
- 2026-05-28 (sixth pass): **R1 audited** — user confirmed via Scaleway console that `aspis-rnaseq-v0` bucket has ZERO lifecycle rules. The declared 30/7-day retention in `job_package.mjs` is cosmetic; nothing actually deletes anything today. Updated R1 from "verify" to "configure (verified missing)" with conservative MVP policy (`users/` prefix 30d expiration + abort_incomplete_multipart 1d) ready to copy-paste into the console. Flagged the 3 Orasis buckets (`orasis-raw`, `orasis-processed`, `orasis-previews`) as likely having the same gap — to audit same way.
- 2026-05-28 (seventh pass): **R1 partially APPLIED** — user applied the 2 lifecycle rules to `aspis-rnaseq-v0` via Scaleway console: `user-jobs-30d` (prefix `users/`, 30d expire) + `abort-incomplete-multipart-1d` (all, 1d abort multipart). RNA-seq retention now enforced server-side. R1 status updated to "done for rnaseq, pending for orasis-*". Orasis buckets still need their own policies (per-bucket retention to define).
- 2026-05-28 (eighth pass): **MAJOR storage estimate correction** — user caught it: my initial "10-30 GB per RNA-seq job" assumed STAR-aligner output (BAMs). Actual pipeline (per `compute/tier1/README.md` + `cleanup_policy.js`) uses **Salmon pseudo-alignment** which produces NO BAM, plus the VM runs `cleanup_policy.js` after the run deleting `input/raw/`, `tmp/`, `work/` BEFORE upload. Realistic per-job output: 80-150 MB (quant.sf + DESeq2 + MultiQC + FastQC + plots). Steady-state at 100 jobs/month = ~10 GB on `aspis-rnaseq-v0` (NOT 2 TB) = **€0.08/month** (NOT €16). Total storage across ALL buckets including Vault at 100 users: **<€0.25/month** — effectively free. The cost-watch trigger "Object Storage > 1 TB total" in §7 stays as-is but is now ~100x less likely to fire.
- 2026-05-28 (ninth pass): **R1 FULLY COMPLETE** — user applied lifecycle rules on remaining 2 buckets (`orasis-processed` 30d, `orasis-previews` 30d, both bucket-wide + multipart abort). All 4 Aspis Bio Scaleway buckets now have retention enforced server-side. Project storage growth is bounded. Roadmap R1 closed; next candidates are R2 (RNA-seq instance tiering) or R6 (Vault F1 cloud provision) per user choice.
- 2026-05-28 (tenth pass): **Save-time batching via CF Queue implemented** (see §2.3.1 + `aspis-bio-api/src/vault/README.md` "Save queue"). `POST /v1/vault/save` is now S3-sync + queue-async: blob written immediately (durable), PG insert via consumer batched up to 25 messages / 30s (whichever fires first). Migration 0008 adds `queue_status` + `indexed_at` + `vector_indexed_at` to `assay_results`. Cost model in §2.4 updated: **alpha is effectively free** (€0.01-€2.84/mo at 10 users); beta drops from worst-case ~€407/mo to ~€34/mo thanks to batching (~12x reduction at 100 users × 5 saves/day). Latency contract: 30s max from save to row-visible-in-list. Audit fixed defense-in-depth `user_sub` validation in consumer + replay invariant doc. Deployed to api.aspis-bio.com Version `da9fe405`; CF Queues `aspis-bio-vault-saves` + `aspis-bio-vault-saves-dlq` created. Vault REMAINS inert (`VAULT_ENABLED=false`) until Scaleway PG + S3 provisioning. **For provisioning**: `aspis-vault-eu` bucket lifecycle must be bucket-wide (NO prefix) with NO expiration (vault data is user-persistent — user-driven delete only) + abort_incomplete_multipart 1d. Different from the per-bucket 30d policies on rnaseq-v0/orasis-*.
