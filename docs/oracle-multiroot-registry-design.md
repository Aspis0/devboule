# Oracle multi-root registry + per-project resolution — design spec

**Status:** implemented (2026-07-21 Grok goal). P1 project-manifest scope + P2 registry/`index_roots` + per-root discovery resolution + P3 union/`extra_roots` are in code with unit tests. Resident multi-process lazy spawn (design 2a full) is partial: one supervisor still owns the primary root; registry + per-root discovery files enable agents to target the correct server when present.
**Author intent (owner):** Devboule can have N projects open, but Oracle only knows ONE folder. Target: Oracle indexes *from* a folder (per-folder job stays), keeps a **registry of indexed roots**, resolves `oracle_context(project_id)` to **that project's own index**, and — later — can query across **multiple already-indexed roots** at once.

This supersedes finding **F45** (agent `oracle_context` returns 0 chunks on an attached external project): F45 is the symptom of the single-workspace assumption, not a standalone bug.

---

## Current state (verified in code)

**Single configured workspace:**
- `vault::OracleIndexPreferences.index_root: Option<String>` — exactly ONE root (`model.rs:120`).
- `oracle/commands.rs::current_oracle_index_root()` resolves that single pref (`:122`); both the operator query path and the resident-server supervisor use it (`:114`).
- One resident server, one discovery file `.oracle-server.json` (`oracle_service.rs:48`), one manifest served.

**But per-project indexes ALREADY exist on disk** — each index job writes into `<root>/oracle-data/`:
- `figlyph/oracle-data/chunk-index-manifest.json`
- `devboule-website/oracle-data/chunk-index-manifest.json`
- `devboule/oracle-data/chunk-index-manifest.json`

So multi-index is already a fact on disk; nothing coordinates it.

**The resolution is half-built already** (good news):
- `devboule-mcp/src/tools/oracle.rs::oracle_index_root_for_project()` (`:463`) ALREADY returns the **project's own `root_path`** when set (falls back to management root otherwise). So "which root does this project want" is solved.
- The gap: `oracle_allowed_file_ids()` (`:495`) loads the manifest from **`<management_root>/oracle-data/chunk-index-manifest.json`** (`:500-502`) and then filters by the project root → for an external project it loads the WRONG manifest → empty scope → 0 chunks (**F45**).
- Deeper gap: even with the right manifest, the **resident HTTP server only has ONE root's vectors loaded** (`current_oracle_index_root`), so a query for a project whose root ≠ the configured `index_root` hits a server that doesn't hold those vectors.

---

## Target architecture

Three layers, shippable in phases (small first).

### Layer 1 — Per-project manifest resolution (fixes F45; small)
`oracle_allowed_file_ids(project_id)` must load the manifest from the **project's own** `oracle-data/`, not always the management root.

- In `oracle.rs`: compute `root = oracle_index_root_for_project(projects_dir, project_id)` (already exists), then load `root/oracle-data/chunk-index-manifest.json` (not `management_root/oracle-data/...`). Keep the unscoped (`project_id=None`) case on the management root.
- This alone makes `oracle_context` return the project's files IF a server holding that root answers — which is Layer 2.

### Layer 2 — Registry of indexed roots + per-project server resolution
A small registry so N roots coexist and each project resolves to the server that holds its vectors.

- **Registry file** (e.g. `<management_root>/oracle-data/.oracle-roots.json`): `{ roots: [{ path, manifestPath, discovery: {baseUrl, authToken, pid} | null, lastIndexedAt, status }] }`. One entry per indexed root.
- **Preferences change**: `OracleIndexPreferences.index_root: Option<String>` → `index_roots: Vec<String>` (keep `index_root` as a deprecated single-value alias for back-compat migration; DEV file store F31 already round-trips the blob).
- **Server model** — pick one:
  - (a) **One resident server per root**, lazily spawned on first query, registry tracks each `discovery`. Simplest to reason about; more processes.
  - (b) **One server, multiple mounted roots**, routes a query to the right root's vectors by `root`/`project_id`. Fewer processes; needs the server to hold N Lance indexes.
  Recommend (a) first (reuses the existing single-root server unchanged, just N of them + a registry), migrate to (b) if process count bites.
- **Agent resolution**: `oracle_context(project_id)` → `root = project.root_path` → registry lookup → that root's server/token → query. The MCP tool already resolves the root; it just needs to hit the right server instead of the single global discovery.

### Layer 3 — Multi-root union query (the "use several indexed folders" future)
Once the registry exists, `oracle_context` can query **more than one** registered root and merge results — e.g. a project plus its dependency roots.

- Add optional `extra_roots: [path]` (or resolve from a per-project `dependsOnRoots` in the project frontmatter) to `oracle_context`.
- Fan out the retrieval to each root's server (Layer 2a) or each mounted index (Layer 2b), merge + re-rank by score, cap at `limit`.
- Security: every extra root must already be in the registry (indexed + approved) — never index-on-demand from a query. This preserves the `ASPIS_WORKSPACE_ROOT` / approved-root gate that B06/F06 established.

---

## Phasing / effort

| Phase | Scope | Effort | Unblocks |
|-------|-------|--------|----------|
| **P1** | Layer 1 manifest path fix in `oracle_allowed_file_ids` | small | F45 for projects whose root == the currently-served index root; correct-manifest everywhere |
| **P2** | Registry file + `index_roots: Vec` pref + per-root lazy server (2a) + per-project server resolution | medium | N projects each usable via Oracle regardless of which is the "active" workspace |
| **P3** | Union query across registered roots (`extra_roots` / `dependsOnRoots`) | medium | cross-root retrieval (project + dependency libs) |

P1 is a genuine quick win and closes F45 in the common case. P2 is the real fix for the owner's point ("N progetti, Oracle una cartella"). P3 is the stated future.

## Touchpoints (files)

- `src-tauri/src/backend/model.rs:120` — `OracleIndexPreferences` (single→multi root).
- `src-tauri/src/backend/vault.rs` — prefs read/save + F31 DEV file store + F39 migration (extend to the multi-root shape).
- `src-tauri/src/oracle/commands.rs:91,122` — `resolve_oracle_index_root` / `current_oracle_index_root` (per-root, registry-aware).
- `src-tauri/src/backend/oracle_service.rs:48,993` — discovery publish (per-root registry entry instead of single `.oracle-server.json`, or keep the file per-root).
- `devboule-mcp/src/tools/oracle.rs:463,495` — `oracle_index_root_for_project` (done), `oracle_allowed_file_ids` (P1 fix), `dispatch_oracle_context` (registry-aware server target for P2, fan-out for P3).

## Non-goals / keep
- Keep the per-folder index JOB as-is (it already writes `<root>/oracle-data/`).
- Keep the approved-root security gate (no query-time indexing of arbitrary paths).
- Keep single-root behavior working (a lone project == today's experience).
