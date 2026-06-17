from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import logging
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import uuid
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from oracle.server.answerer import answer_from_context
from oracle.server.query_engine import lexical_chunk_context
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")

logging.getLogger("httpx").setLevel(logging.WARNING)

logger = logging.getLogger(__name__)

# Step 4a thin-client contract with the Rust supervisor (Step 4b).
# The resident HTTP Oracle server is discovered via these env vars (preferred,
# used by app-launched agents) or a discovery file written by the supervisor.
# SECURITY: the token carried here (env ASPIS_ORACLE_AUTH_TOKEN or the discovery
# file `authToken`) is the AGENT token (server-side ORACLE_AGENT_AUTH_TOKEN),
# which authorizes ONLY the /*-bounded scoped endpoints — never the unscoped
# /ask, /context, /index/*. The operator token (ORACLE_AUTH_TOKEN) is held only
# by the app/Rust UI path and is never published to agents.
ORACLE_HTTP_BASE_ENV = "ASPIS_ORACLE_HTTP_BASE"
ORACLE_HTTP_TOKEN_ENV = "ASPIS_ORACLE_AUTH_TOKEN"
ORACLE_DISCOVERY_FILENAME = ".oracle-server.json"

# Phase 11.2 STRUCTURE bridge. The read-only `project_structure` tool shells out to the
# Aspis Management app binary (`<app> structure --root <path>`) which REUSES the Rust
# tree-sitter structure builder (`src-tauri/.../backend/structure.rs`) — there is NO
# second parser in Python (zero duplication / no drift). The launch wiring sets the
# binary path in this env at every MCP launch site; absent ⇒ the tool fails closed with
# a clear error rather than guessing a path.
ASPIS_APP_BIN_ENV = "ASPIS_APP_BIN"
# Wall-clock cap on the bridge subprocess. The Rust builder is bounded (MAX_FILES /
# MAX_WALK_ENTRIES) so a normal repo finishes in well under a second; this is generous
# headroom for a huge tree on a cold disk. On elapse we kill the child and return a clean
# error result (never hang the server).
PROJECT_STRUCTURE_TIMEOUT_S = 60.0
# POST-HOC reject on the bridge's stdout size. NOTE: this is NOT a streaming memory guard
# — `_run_structure_bridge` uses `capture_output=True`, which buffers the ENTIRE stdout in
# memory BEFORE this length check runs, so a pathological graph is fully materialized first
# and only then rejected. That is acceptable because the bridge binary is the trusted app
# (`current_exe`, wired by the launch site) and the Rust side caps the graph tightly at the
# source (MAX_FILES / MAX_WALK_ENTRIES); this cap is a backstop sanity reject, not a true
# upstream memory bound. 16 MiB is far above any real spine+files payload yet bounded.
PROJECT_STRUCTURE_MAX_OUTPUT_BYTES = 16 * 1024 * 1024
# Per-(root, freshness-key) cache TTL. Repeated calls within a session reuse the parsed
# graph instead of re-walking the whole repo; the freshness key (newest mtime + file
# count under the root, cheaply sampled) invalidates the entry when the tree changes, and
# the TTL bounds staleness even if the freshness probe misses a same-second edit.
PROJECT_STRUCTURE_CACHE_TTL_S = 30.0
# Bound the cheap freshness probe so it never itself walks an unbounded tree (the build
# is bounded by the Rust side; the probe must be cheaper than the build it guards).
PROJECT_STRUCTURE_FRESHNESS_MAX_ENTRIES = 20_000


BLOCK_MARKER = "```aspis-project"
BLOCK_CLOSE = "```"
AGENTS_STATE_FILE = ".aspis-agents.json"
# Current on-disk schema version of `.aspis-agents.json`. v2 adds per-session
# `subagents` and `needsUser`. READ is tolerant of any version >= 1 (additive
# schema), so older v1 files load without migration; see normalize_agents_state.
AGENTS_STATE_VERSION = 2
VALID_PROJECT_STATUSES = {"active", "paused", "done", "archived"}
VALID_TASK_STATUSES = {"todo", "wip", "review", "blocked", "done"}
# Phase B role merge: spawn-time roles collapse to {coder, verifier}. The
# coder PLANS and CODES (and may spawn subagents).
# "mini" (P3) is the one-shot read-only sub-agent: oracle_context only, no
# mutation tools — enforced by ROLE_ALLOWED_TOOLS via require_registered_role.
# "orchestrator" (devboule-coder): the FIRST-CLASS planning+delegation main
# coder. The new Rust `devboule-coder` binary self-registers under this role.
# BEHAVIOR CHANGE (deliberate): "orchestrator" was previously an ALIAS that
# collapsed to "coder". It is now its OWN role and must NOT be normalized away —
# otherwise its narrower allowlist (no direct file-write/mutation tool; it
# delegates ALL writes to spawn_mini_coder) would never be reached. Its Kanban /
# project semantics are IDENTICAL to coder's (see CODER_LIKE_ROLES) so it never
# gains a verifier-only transition; it is strictly tighter-or-equal to coder.
VALID_ROLES = {"coder", "verifier", "mini", "orchestrator"}
# "orchestrator" intentionally REMOVED from the aliases below: as a first-class
# role it normalizes to itself, not to coder.
ROLE_ALIASES = {"architect": "coder", "code": "coder"}
# Roles that share the coder's project/Kanban transition + claim semantics. The
# orchestrator is the planning main coder, so it gets EXACTLY the coder's
# task-status powers (claim todo/wip/blocked, set todo/wip/review/blocked, reopen
# to todo) — never the verifier-only `done`. Used by validate_transition and
# project_claim_task so the orchestrator mirrors the coder there without widening
# any gate (tighter-or-equal to coder, never broader).
CODER_LIKE_ROLES = {"coder", "orchestrator"}
MAX_EVENTS = 300
# FIX 6: hard caps so a long-lived `.aspis-agents.json` cannot grow without bound
# (every register/heartbeat/claim appended forever). Enforced at the single
# normalize choke point (read + write). LIVE sessions and OPEN claims are NEVER
# dropped — only CLOSED sessions / terminal (done/expired) claims beyond the cap are
# pruned, oldest first.
MAX_SESSIONS = 200
MAX_CLAIMS = 500
# Mini-coder directive queue cap. MUST equal `MAX_DIRECTIVES` in
# src-tauri/src/backend/mini_coder.rs (the Rust executor is the co-writer). Beyond
# this the oldest TERMINAL directives are evicted first; an active directive
# (pending/launching/running) is never dropped. (CO-WRITER PARITY — change both.)
MAX_MINI_CODER_DIRECTIVES = 50
# Visual-check directive queue cap. Co-owned with the Rust executor; terminal
# entries are evicted oldest-first, pending/running entries are preserved.
MAX_VISUAL_CHECK_DIRECTIVES = 50
# Bounds for the `spawn_mini_coder` tool inputs + its bounded result poll.
MINI_CODER_MAX_TASK_LEN = 4000
MINI_CODER_MAX_FILES = 64
# ASYNC STEERING (a): bounds for the `steer_mini_coder` tool. CO-WRITER PARITY with the
# Rust `MAX_STEER_MESSAGE_LEN` / `MAX_STEER_QUEUE_LEN` in
# src-tauri/src/backend/mini_coder.rs — change BOTH together. A single mid-flight
# correction is capped so it cannot bloat the fix-pass prompt; the per-directive FIFO is
# bounded and a flooding append is REFUSED (never drops an already-queued correction).
MINI_CODER_MAX_STEER_LEN = 2000
MINI_CODER_MAX_STEER_QUEUE = 8
# The reserved steer message that maps to the kill path (case-insensitive, trimmed),
# mirroring Rust `STEER_STOP_SENTINEL`: a `stop` steer sets `killRequested` instead of
# queueing prose, so steering generalizes the Stop button rather than bypassing it.
MINI_CODER_STEER_STOP_SENTINEL = "stop"
# How a WRITE mini applies its changes. These wire strings MUST EXACTLY MATCH the
# Rust `WriteMode` serde representation (camelCase) in
# `src-tauri/src/backend/mini_coder.rs` so a directive this writer emits
# deserializes there. `emitEdits` is the default (and is OMITTED from the
# directive, NO-CHURN — see `dispatch_spawn_mini_coder`). PLUMBING ONLY: nothing
# branches on it yet (a later workstream reads it).
MINI_CODER_WRITE_MODE_DEFAULT = "emitEdits"
MINI_CODER_WRITE_MODES = ("emitEdits", "agenticIterative")
# Hard wall-clock cap on the blocking poll the tool does for the directive's
# `result`. The tool BLOCKS the coder's MCP thread by design (so the coder gets
# the terminal outcome synchronously), but it MUST always time out: on expiry it
# returns a synthesized `timeout` outcome. ~30 min covers the executor's WORST-CASE
# retry/escalation chain (up to 1 + 2 attempts, each bounded by the executor's own
# `DEFAULT_WALL_CLOCK_CAP_SECS` ~10 min) so the tool does not give up before a full
# escalation chain — and the executor's per-attempt kill+timeout — would land.
MINI_CODER_POLL_TIMEOUT_SECS = 1800.0
# Sleep between result re-reads. Bounded so the lock is taken briefly each pass and
# never held across the sleep (the executor co-writes the same file).
MINI_CODER_POLL_INTERVAL_SECS = 0.75
# Visual checks run a native capture + local VLM critique. They should be fast,
# but the MCP caller must always unblock.
VISUAL_CHECK_POLL_TIMEOUT_SECS = 120.0
VISUAL_CHECK_POLL_INTERVAL_SECS = 0.75
VISUAL_CHECK_MAX_FOCUS_CHARS = 500
VISUAL_CHECK_MAX_HTML_PATH_CHARS = 1024
# GH-P4: bound on the agent push-approval queue (mirrors `MAX_PUSH_REQUESTS` in the
# Rust `git_push.rs`). Only TERMINAL requests are evicted (oldest by createdAt).
MAX_GIT_PUSH_REQUESTS = 50
# GH-P4: hard wall-clock cap on the blocking poll `request_git_push` does for the
# human's verdict. The tool BLOCKS the agent's MCP thread by design (so the agent
# gets the approve/deny/push outcome synchronously), but it MUST always time out: on
# expiry it returns a synthesized `timeout` outcome and the agent STOPS (does not
# retry, does not raw-push). 10 min gives the human ample time to react.
GIT_PUSH_POLL_TIMEOUT_SECS = 600.0
# Sleep between verdict re-reads. Bounded so the lock is taken briefly each pass and
# never held across the sleep (the Rust approve/deny command co-writes the file).
GIT_PUSH_POLL_INTERVAL_SECS = 0.75
# Phase 1 — plan approval + reply-box. The `plan_submit`/`plan_status`/`ask_user`
# tools share the same file-only bridge + bounded-poll discipline as request_git_push
# (the Rust approve/reject/reply commands co-write `.aspis-agents.json`; there is no
# reverse-trigger). Constants co-owned with the Rust side — change both.
#   * MAX_PLAN_APPROVAL_REQUESTS: cap on the planApprovalRequests queue. Only TERMINAL
#     requests are evicted (oldest by createdAt); a pending_approval is NEVER dropped.
#   * PLAN_MAX_MARKDOWN_CHARS: hard cap on the submitted plan body; oversize is rejected
#     fast (before any artifact write) so a runaway plan can never fill the disk.
#   * PLAN_POLL_* / ASK_USER_POLL_*: same 10-min wall-clock cap + 0.75s re-read interval
#     as the git-push poll (the lock is taken briefly each pass, NEVER held across sleep).
MAX_PLAN_APPROVAL_REQUESTS = 20
PLAN_MAX_MARKDOWN_CHARS = 200_000
# Phase 11.5-B (Piece 1a): caps for `project_create_plan_tasks`. Mirror the Rust
# planner's MAX_TASKS / MAX_TASK_SCOPE (devboule-coder/src/planner.rs) so a plan the
# planner is allowed to emit is also allowed to be bulk-created on the Kanban — the
# two sides cannot drift into one accepting a plan the other rejects.
MAX_PLAN_TASKS = 40
MAX_PLAN_TASK_SCOPE = 3
PLAN_POLL_TIMEOUT_SECS = 600.0
PLAN_POLL_INTERVAL_SECS = 0.75
ASK_USER_POLL_TIMEOUT_SECS = 600.0
ASK_USER_POLL_INTERVAL_SECS = 0.75
# A claim is terminal (eligible for pruning) when its task is done OR its lease has
# expired; everything else (todo/wip/review/claimed/blocked/provider_action_*) is an
# OPEN claim that must be kept regardless of the cap.
TERMINAL_CLAIM_STATUSES = {"done"}
LEASELESS_CLAIM_WINDOW = timedelta(minutes=15)
LAUNCH_TOKEN_WINDOW = timedelta(hours=2)
SESSION_TOKEN_WINDOW = timedelta(hours=12)
ALLOW_UNMANAGED_PRIVILEGED_ENV = "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"
CF_API = "https://api.cloudflare.com/client/v4"
SCW_API = "https://api.scaleway.com"
SCW_TARGET_PROJECT_NAME = "aspis-bio"
CF_TARGET_ACCOUNT_NAME = "aspis-bio"
CF_ASPIS_BIO_WORKERS = {
    "aspis-bio-api",
    "aspis-biovision-worker",
    "orasis-worker",
    "aspis-bio-rnaseq-api",
    "aspis-bio-papers",
    "aspis-bio-oauth",
    "aspis-bio-mta-sts",
    "aspis-bio-resend-webhooks",
}
APP_VAULT_SERVICE = "Aspis Management"
APP_VAULT_ACCOUNTS = {
    "cloudflare_token": "provider:cloudflare",
    "cloudflare_account_id": "scope:cloudflare_account_id",
    "scaleway_token": "provider:scaleway",
    "scaleway_ai_token": "provider:scaleway_ai",
    "scaleway_project_id": "scope:scaleway_project_id",
    "scaleway_object_access_key": "aux:scaleway_object_access_key",
    "scaleway_object_secret_key": "aux:scaleway_object_secret_key",
    "infomaniak_token": "provider:infomaniak",
    "mistral_token": "provider:mistral",
}
CF_TOKEN_ENVS = ("ASPIS_CLOUDFLARE_API_TOKEN", "CLOUDFLARE_API_TOKEN")
CF_READONLY_TOKEN_ENVS = ("ASPIS_CLOUDFLARE_VERIFIER_TOKEN",)
CF_CODER_TOKEN_ENVS = ("ASPIS_CLOUDFLARE_CODER_WORKER_WRITE_TOKEN",)
CF_SECRET_ROTATOR_TOKEN_ENVS = ("ASPIS_CLOUDFLARE_SECRETS_ROTATOR_TOKEN",)
CF_ACCOUNT_ENVS = ("ASPIS_CLOUDFLARE_ACCOUNT_ID", "CLOUDFLARE_ACCOUNT_ID")
SCW_TOKEN_ENVS = ("ASPIS_SCALEWAY_API_TOKEN", "SCW_SECRET_KEY", "SCALEWAY_API_TOKEN")
SCW_AI_TOKEN_ENVS = ("ASPIS_SCALEWAY_AI_API_TOKEN", "SCALEWAY_AI_API_TOKEN")
SCW_PROJECT_ENVS = ("ASPIS_SCALEWAY_PROJECT_ID", "SCW_DEFAULT_PROJECT_ID")
SCW_OBJECT_ACCESS_KEY_ENVS = ("ASPIS_SCALEWAY_OBJECT_ACCESS_KEY", "SCW_ACCESS_KEY")
SCW_OBJECT_SECRET_KEY_ENVS = ("ASPIS_SCALEWAY_OBJECT_SECRET_KEY", "SCW_S3_SECRET_KEY")
INFOMANIAK_TOKEN_ENVS = ("ASPIS_INFOMANIAK_API_TOKEN", "INFOMANIAK_API_TOKEN")
MISTRAL_TOKEN_ENVS = ("ASPIS_MISTRAL_API_KEY", "MISTRAL_API_KEY")
SCW_ZONES = ("fr-par-1", "fr-par-2", "fr-par-3", "nl-ams-1", "nl-ams-2", "nl-ams-3", "pl-waw-1", "pl-waw-2", "pl-waw-3")
SCW_REGIONS = ("fr-par", "nl-ams", "pl-waw")
_MCP_ENGINE_CACHE: dict[str, Any] = {}
_MCP_INDEX_STATUS_CACHE: dict[str, tuple[float, dict[str, Any]]] = {}

# ANTI-DRIFT: the `contract` lists below MUST stay verbatim-identical to the
# contract strings in default_role_rules() in
# src-tauri/src/backend/agents.rs. If you change one, change both.
# INTENTIONAL BILINGUAL SPLIT (not drift): only `contract` is mirrored. `summary`
# and `forbidden` are Italian here because agents read them, while the Rust copies
# are English on purpose because they feed the fleet UI — do not "align" them.
ROLE_RULES = [
    {
        # Phase B merge: the coder now PLANS and CODES. It absorbs the former
        # orchestrator's planning/coordination mandate (assign work, open
        # blockers, reopen tasks to todo, create follow-ups) on top of its own
        # implementation duties. The final `done` is still verifier-only.
        "role": "coder",
        "summary": "Pianifica (/plan), lavora sul codice e usa Oracle; apre blocchi, riapre task a todo e segna wip/review/blocked, ma il done finale e solo verifier.",
        # PHASE E mandate (mirrored in Rust default_role_rules coder.censor): the
        # coder is Censor's per-step consumer. Kept as a dedicated `censor` field
        # (not in the verbatim-mirrored `contract`) because the mandate differs per
        # role; the Rust copy is English (UI), this one Italian (agents read it).
        "censor": [
            "A ogni confine di step chiama censor_findings(project_id, file=<file toccati>) per i file che hai modificato.",
            "Correggi i finding locali reali; chiudi i falsi positivi con censor_dispose(disposition=\"fp\").",
            "Raggruppa al confine di step: non e un'interruzione live, e una verifica batch prima di passare al passo successivo.",
        ],
        # Phase 1 plan-approval + reply-box mandate. Dedicated `plan` field (not in
        # the verbatim-mirrored `contract`) because the mandate differs per role; the
        # Rust copy is English (UI), this one Italian (agents read it). Prima di lavoro
        # multi-file il coder sottomette il piano e ASPETTA l'approvazione umana; su
        # rifiuto rivede e re-invia; usa ask_user per le domande bloccanti invece di
        # stallare nel terminale.
        "plan": [
            "Prima di lavoro multi-file invia il piano con plan_submit(project_id, title, plan_markdown) e ASPETTA l'approvazione umana: non iniziare l'implementazione prima di status=\"approved\".",
            "Se il piano viene rifiutato (status=\"rejected\") rivedilo seguendo la `note` del revisore e RE-INVIA con plan_submit; non procedere col piano bocciato.",
            "Se hai una domanda bloccante per l'umano usa ask_user(question) e attendi la risposta, invece di stallare o indovinare nel terminale.",
        ],
        # GH-P5 cooperative push mandate (mirrored in Rust default_role_rules
        # coder.push — bilingual by design, Italian here, English there). Gli
        # agenti committano liberamente ma NON fanno mai un `git push` grezzo:
        # l'ambiente di lancio dell'agente non ha credenziali git, quindi un push
        # grezzo fallisce subito; per pubblicare si passa dal tool MCP
        # request_git_push con approvazione umana.
        "push": [
            "Committa liberamente (git add -u / git commit) per salvare il lavoro.",
            "NON fare mai un `git push` grezzo: il tuo ambiente non ha credenziali git e fallira. Per pubblicare chiama il tool MCP `request_git_push`; un umano lo approva.",
            "Se la richiesta di push viene negata o va in timeout, FERMATI ed escala all'umano via needs_user (agent_heartbeat status=\"needs_user\"). NON riprovare, NON tentare un push grezzo, NON aggirare il gate.",
        ],
        "allowedTools": [
            "agent_register",
            "agent_heartbeat",
            "agent_state",
            "project_list",
            "project_get",
            "project_next_task",
            "project_claim_task",
            "project_update_status",
            "project_append_note",
            "project_create_followup",
            "project_create_plan_tasks",
            "provider_credentials_status",
            "cloudflare_list_workers",
            "cloudflare_rotate_worker_secret",
            "scaleway_list_resources",
            "scaleway_resource_action",
            "oracle_ask",
            "oracle_context",
            "project_structure",
            "censor_findings",
            "censor_dispose",
            "visual_check",
            "spawn_mini_coder",
            "steer_mini_coder",
            "request_git_push",
            "plan_submit",
            "plan_status",
            "ask_user",
        ],
        "forbidden": [
            "Non imposta done: serve verifier con evidenza.",
            "Non legge o stampa token. Usa solo token da env e scope Aspis Bio verificato.",
            "Delega a spawn_mini_coder solo sub-task economici e meccanici (boilerplate, bulk read->summary, edit semplici, docstring, test); pre-carica il contesto necessario; ragiona tu; RIVEDI l'output del mini come bozza prima di usarlo.",
            "Per un task di WRITE scegli `write_mode`: 'agenticIterative' SOLO per file in un linguaggio con copertura del gate deterministico in QUESTO progetto E con un modello mini abbastanza capace di iterare; altrimenti 'emitEdits' (default). Nel dubbio usa 'emitEdits'.",
            "Se spawn_mini_coder torna status='aborted_by_human' FERMA quel lavoro, NON riprovare il mini in silenzio, ed escala all'umano via needs_user (agent_heartbeat status=\"needs_user\").",
            "Se spawn_mini_coder torna status='escalated' (la catena di retry e' esaurita e Censor e' ancora sporco), rifai il file TU STESSO: il rail di training ha gia' catturato il fallimento, quindi NON rilanciare ciecamente il mini sullo stesso file.",
            "Prima di mettere un task in review: fai girare UN SOLO pass di review tuo (un subagente Sonnet) sui file che hai toccato, fixa i finding, POI sposta il task a review con una nota 'ready for final reviewer'. Il verdetto FINALE resta del verifier (il pass finale censorReview si lancia dall'app, NON parte da solo quando metti review), mai del tuo pass.",
            "Quando produci o revisioni un artifact HTML self-contained e serve feedback visuale, chiama visual_check(html_path, focus?) e tratta la critique come evidenza advisory.",
        ],
        "contract": [
            "Dichiara il modello (`model`) ad agent_register.",
            "Quando spawni o chiudi subagenti manda agent_heartbeat con `subagents=[{label, model, count, role?}]` aggiornato.",
            "Quando aspetti l'umano (domanda, permesso allow/deny, blocco) manda agent_heartbeat con status=\"needs_user\" e un message chiaro.",
        ],
    },
    {
        # ORCHESTRATOR (devboule-coder): the MAIN coder that PLANS + DELEGATES. The
        # new Rust `devboule-coder` binary self-registers under this role. It owns
        # the same Kanban/transition powers as the coder (CODER_LIKE_ROLES) but
        # holds NO direct file-write/mutation tool of its own: EVERY write is
        # delegated to spawn_mini_coder. Its allowlist is a STRICT SUBSET of the
        # coder's (tighter-or-equal, never broader) — no Censor dispose, no
        # provider/cloudflare/scaleway tool, no verifier-only transition.
        "role": "orchestrator",
        "summary": "Coder principale che PIANIFICA e DELEGA: capisce il progetto via oracle_ask/oracle_context, delega OGNI scrittura a spawn_mini_coder, gestisce il Kanban come un coder (claim, wip/review/blocked, riapri a todo) ma il done resta verifier; pubblica via request_git_push col gate umano.",
        # Plan-approval + reply-box mandate (same shape as the coder's). Prima di
        # lavoro multi-file l'orchestrator sottomette il piano e ASPETTA
        # l'approvazione umana — "mai full-auto non presidiato".
        "plan": [
            "Prima di lavoro multi-file invia il piano con plan_submit(project_id, title, plan_markdown) e ASPETTA l'approvazione umana: non iniziare la delega prima di status=\"approved\".",
            "Se il piano viene rifiutato (status=\"rejected\") rivedilo seguendo la `note` del revisore e RE-INVIA con plan_submit; non procedere col piano bocciato.",
            "Se hai una domanda bloccante per l'umano usa ask_user(question) e attendi la risposta, invece di stallare o indovinare nel terminale.",
        ],
        # Cooperative push mandate (identical to the coder's): commit freely, never
        # raw-push, publish only via the human-approved request_git_push gate.
        "push": [
            "Committa liberamente (git add -u / git commit) per salvare il lavoro.",
            "NON fare mai un `git push` grezzo: il tuo ambiente non ha credenziali git e fallira. Per pubblicare chiama il tool MCP `request_git_push`; un umano lo approva.",
            "Se la richiesta di push viene negata o va in timeout, FERMATI ed escala all'umano via ask_user. NON riprovare, NON tentare un push grezzo, NON aggirare il gate.",
        ],
        "allowedTools": [
            "agent_register",
            "agent_heartbeat",
            "agent_state",
            "project_list",
            "project_get",
            "project_next_task",
            "project_claim_task",
            "project_update_status",
            "project_append_note",
            "project_create_followup",
            "project_create_plan_tasks",
            "oracle_ask",
            "oracle_context",
            "project_structure",
            "spawn_mini_coder",
            "steer_mini_coder",
            "request_git_push",
            "plan_submit",
            "plan_status",
            "ask_user",
        ],
        "forbidden": [
            "Non scrive MAI file direttamente: NON hai alcun tool di scrittura/mutazione del filesystem. OGNI modifica al codice passa per spawn_mini_coder (tu pianifichi e riveli il contesto; il mini scrive).",
            "Per domande su progetto o codebase usa PRIMA oracle_ask / oracle_context (capacita di comprensione grounded): non indovinare ne leggere il filesystem a mano.",
            "Non imposta done: e verifier-only con evidenza. Tu puoi solo claim e wip/review/blocked (e riapertura a todo), esattamente come un coder.",
            "Ogni cambiamento passa per Censor + il Kanban + il gate umano: mai full-auto non presidiato. Quando un sotto-task e pronto, mettilo in review con una nota e lascia il verdetto finale al verifier.",
            "Se spawn_mini_coder torna status='aborted_by_human' FERMA quel lavoro, NON riprovare il mini in silenzio, ed escala all'umano via ask_user.",
            "Se spawn_mini_coder torna status='escalated' (la catena di retry e' esaurita e Censor e' ancora sporco), FERMATI ed escala all'umano via ask_user invece di rilanciare ciecamente lo stesso file.",
            "Non legge o stampa token o segreti. Usa solo token da env e scope Aspis Bio verificato; nessun provider OpenAI/Anthropic-API/GCP/AWS sui dati utente (solo Scaleway/Infomaniak EU, ZDR).",
        ],
        "contract": [
            "Dichiara il modello (`model`) ad agent_register.",
            "Quando spawni o chiudi subagenti (mini-coder) manda agent_heartbeat con `subagents=[{label, model, count, role?}]` aggiornato.",
            "Quando aspetti l'umano (domanda, permesso allow/deny, blocco) manda agent_heartbeat con status=\"needs_user\" e un message chiaro.",
        ],
    },
    {
        "role": "verifier",
        "summary": "Controlla task in review, output, test e rischi. Puo chiudere task o riaprirli come blocked.",
        # PHASE E mandate (mirrored in Rust default_role_rules verifier.censor): the
        # verifier is Censor's final authority over the residual ledger.
        "censor": [
            "Chiama censor_findings(project_id) per il ledger residuo; ignora i finding gia risolti.",
            "Concentrati su problemi cross-file, architetturali e di sicurezza multi-file che il modello piccolo non puo vedere.",
            "Adjudica: conferma i finding reali e chiudi i falsi positivi con censor_dispose (fp/wontfix/fixed).",
        ],
        "allowedTools": [
            "agent_register",
            "agent_heartbeat",
            "agent_state",
            "project_list",
            "project_get",
            "project_next_task",
            "project_claim_task",
            "project_update_status",
            "project_append_note",
            "provider_credentials_status",
            "cloudflare_list_workers",
            "scaleway_list_resources",
            "oracle_ask",
            "oracle_context",
            "project_structure",
            "censor_findings",
            "censor_dispose",
            "visual_check",
            "ask_user",
            "plan_status",
        ],
        "forbidden": [
            "Non modifica codice.",
            "Non modifica Cloudflare o Scaleway: solo read-only.",
            "Non marca done se il task non e in review, o senza evidence e confidence >= 0.70.",
            "Quando revisioni un artifact HTML self-contained, chiama visual_check(html_path, focus?) se il layout visuale puo influire sul verdetto; tratta la critique come evidenza advisory.",
        ],
        "contract": [
            "Dichiara il modello (`model`) ad agent_register.",
            "Quando spawni o chiudi subagenti manda agent_heartbeat con `subagents=[{label, model, count, role?}]` aggiornato.",
            "Quando aspetti l'umano (domanda, permesso allow/deny, blocco) manda agent_heartbeat con status=\"needs_user\" e un message chiaro.",
        ],
    },
    {
        "role": "mini",
        "summary": "Sub-agente one-shot in SOLA LETTURA: usa oracle_context per leggere il codebase e project_structure per la spina dorsale architetturale, nient'altro.",
        "allowedTools": [
            "agent_register",
            "oracle_context",
            "project_structure",
        ],
        "forbidden": [
            "Non modifica codice, task, Kanban, provider o findings: NESSUN tool di mutazione.",
            "Non spawna altri agenti, non manda agent_heartbeat (niente subagents, niente needs_user: il contatto umano e' del coder padre) e non chiama censor_*: e' una foglia one-shot.",
            "Non legge o stampa token o segreti.",
        ],
        "contract": [
            "Dichiara il modello (`model`) ad agent_register.",
            "Registrati con agent_register (role=\"mini\") prima di chiamare oracle_context / project_structure.",
        ],
    },
]

ROLE_ALLOWED_TOOLS = {
    rule["role"]: set(rule["allowedTools"])
    for rule in ROLE_RULES
}


TOOLS = [
    {
        "name": "agent_rules",
        "description": "Restituisce ruoli, responsabilita e divieti pratici per agenti Aspis.",
        "parameters": {},
    },
    {
        "name": "agent_state",
        "description": "Legge stato live di sessioni agenti, claim e ultimi eventi dopo registrazione.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "agent_register",
        "description": "Registra un agente CLI prima di leggere o aggiornare progetti.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "model": {"type": "string"},
            "client": {"type": "string", "default": ""},
            "message": {"type": "string"},
            "launch_token": {"type": "string", "default": ""},
        },
    },
    {
        "name": "agent_heartbeat",
        "description": "Aggiorna presenza live dell'agente nella dashboard.",
        "parameters": {
            "agent_id": {"type": "string"},
            "status": {"type": "string"},
            "message": {"type": "string"},
            "session_token": {"type": "string"},
            # OPTIONAL: file the agent is currently editing/working on. Lets Polis
            # place the agent on the EXACT building for that file instead of a
            # representative one. Absolute, project-relative, or scanned-folder-
            # relative; resolved to a building on the Rust/Polis side. Omit to
            # leave the previous value untouched (backward-compatible).
            "file_path": {"type": "string", "default": ""},
            # OPTIONAL: the agent's current subagent breakdown for the fleet UI.
            # A list of {label, model, count, role?}. Omit (or pass null) to leave
            # the stored value untouched; pass [] to clear it (no subagents now).
            "subagents": {"type": ["array", "null"]},
        },
    },
    {
        "name": "spawn_mini_coder",
        "description": (
            "Solo coder: delega un sotto-task economico a un mini-coder one-shot "
            "ospitato dall'app; blocca finche il mini termina e restituisce il "
            "risultato terminale. Per i task di WRITE scegli `write_mode`: "
            "'agenticIterative' (il mini corregge su piu round contro il gate "
            "deterministico) SOLO per file in un linguaggio con copertura del gate "
            "in questo progetto E con un modello mini abbastanza capace di iterare; "
            "altrimenti 'emitEdits' (default: una scrittura + una correzione)."
        ),
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "task": {"type": "string"},
            "files": {"type": "array", "items": {"type": "string"}},
            "backend": {"type": "string", "default": ""},
            "allow_oracle": {"type": "boolean", "default": False},
            "write": {"type": "boolean", "default": False},
            "write_mode": {
                "type": "string",
                "enum": list(MINI_CODER_WRITE_MODES),
                "default": MINI_CODER_WRITE_MODE_DEFAULT,
                "description": (
                    "How a write mini applies changes: 'emitEdits' (default; the "
                    "model returns a JSON edit list, the app applies it - use for "
                    "mechanical/well-scoped edits, uncovered languages, or a "
                    "small/weak local model) vs 'agenticIterative' (the model "
                    "iterates over multiple rounds against the deterministic gate - "
                    "use ONLY for files in a language with gate coverage in this "
                    "project AND when the local model is capable enough to iterate "
                    "usefully). Gated by language coverage; falls back to one-shot "
                    "when uncovered. Default to emitEdits when unsure."
                ),
            },
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "steer_mini_coder",
        "description": (
            "Solo coder/orchestrator: steer a RUNNING mini-coder you spawned by appending "
            "a mid-flight correction to its steer queue. The app folds queued corrections "
            "into the mini's NEXT fix-pass round (it takes effect at a round boundary, not "
            "mid-token), reusing the same channel as the Stop button. Send the message "
            "'stop' to ABORT the mini (it maps to the kill path). Pass the directiveId "
            "returned by spawn_mini_coder. Returns status=queued|stopped|queue_full|"
            "not_found|terminal."
        ),
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "directive_id": {"type": "string"},
            "message": {
                "type": "string",
                "description": (
                    "The mid-flight correction to fold into the mini's next round, or "
                    "'stop' to abort the mini (the stop sentinel maps to the kill path)."
                ),
            },
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "visual_check",
        "description": "Ask the app to render a self-contained HTML artifact, run a local visual critique, and return text feedback.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "html_path": {"type": "string"},
            "focus": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "request_git_push",
        "description": "Solo coder: RICHIEDE l'approvazione umana per un git push (puoi committare liberamente, ma il push lo approva l'umano). Blocca finche l'umano approva (e l'app esegue il push) o nega; su timeout FERMATI, non riprovare, non fare push diretto.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "project_id": {"type": "string"},
            "branch": {"type": "string", "default": ""},
            "remote": {"type": "string", "default": ""},
            "force": {"type": "boolean", "default": False},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "plan_submit",
        "description": "Solo coder: invia un piano di implementazione (markdown) per l'approvazione umana prima di lavoro multi-file; blocca finche l'umano approva o rifiuta. Su rifiuto rivedi e re-invia; su timeout fermati e non procedere senza approvazione.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "project_id": {"type": "string"},
            "title": {"type": "string"},
            "plan_markdown": {"type": "string"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "plan_status",
        "description": "Coder o verifier: legge lo stato corrente di un piano gia inviato (pending_approval/approved/rejected/timeout) dato il suo plan_id.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "plan_id": {"type": "string"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "ask_user",
        "description": "Coder o verifier: fai una domanda bloccante all'umano e attendi la risposta invece di stallare nel terminale; blocca finche arriva la risposta o scade il timeout.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "question": {"type": "string"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_list",
        "description": "Lista progetti Markdown locali leggibili dagli agenti.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_get",
        "description": "Legge un progetto con task, note, revision e path.",
        "parameters": {
            "project_id": {"type": "string"},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_next_task",
        "description": "Suggerisce il prossimo task non completato per un ruolo.",
        "parameters": {
            "project_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "agent_id": {"type": "string"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_claim_task",
        "description": "Crea un claim con lease sul task, visibile nella dashboard agenti.",
        "parameters": {
            "project_id": {"type": "string"},
            "task_id": {"type": "string"},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_update_status",
        "description": "Aggiorna status task/progetto con note ed evento auditabile.",
        "parameters": {
            "project_id": {"type": "string"},
            "task_id": {"type": "string"},
            "status": {"type": "string"},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "evidence": {"type": "string"},
            "confidence": {"type": "number"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_append_note",
        "description": "Aggiunge una nota strutturata al progetto.",
        "parameters": {
            "project_id": {"type": "string"},
            "text": {"type": "string"},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_create_followup",
        "description": "Crea un task TODO di follow-up senza chiudere quello corrente.",
        "parameters": {
            "project_id": {"type": "string"},
            "title": {"type": "string"},
            "reason": {"type": "string"},
            "category": {
                "type": "string",
                "enum": ["feature", "hardening", "bug", "other"],
                "default": "other",
            },
            "description": {"type": "string", "default": ""},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_create_plan_tasks",
        "description": (
            "Bulk-crea i task di un piano approvato sul Kanban del progetto come "
            "todo, taggati col planId. Alloca id T<n> freschi (nessuna collisione coi "
            "task manuali) e rimappa dependsOn dagli id interni del piano agli id "
            "allocati; valida che il DAG sia aciclico. Ritorna gli id allocati."
        ),
        "parameters": {
            "project_id": {"type": "string"},
            "plan_id": {"type": "string"},
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "title": {"type": "string"},
                        "scope": {"type": "array", "items": {"type": "string"}},
                        "acceptance": {"type": "string"},
                        "dependsOn": {"type": "array", "items": {"type": "string"}},
                    },
                    "required": ["id", "title"],
                },
            },
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "provider_credentials_status",
        "description": "Read-only: diagnostica quali credenziali provider/Oracle sono configurate senza esporre segreti.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "cloudflare_list_workers",
        "description": "Read-only: lista Workers nell'account Aspis Bio Cloudflare.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "account_id": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "cloudflare_rotate_worker_secret",
        "description": "Coder-only: ruota un secret di un Worker Cloudflare Aspis Bio.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "account_id": {"type": "string", "default": ""},
            "worker_name": {"type": "string"},
            "secret_name": {"type": "string"},
            "secret_value": {"type": "string"},
            "management_project_id": {"type": "string"},
            "task_id": {"type": "string"},
            "evidence": {"type": "string"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "scaleway_list_resources",
        "description": "Read-only: lista VM, funzioni e container nel progetto Scaleway Aspis Bio.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "project_id": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "scaleway_resource_action",
        "description": "Coder-only: start/stop/reboot/terminate VM o deploy serverless nel progetto Aspis Bio.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "resource_id": {"type": "string"},
            "action": {"type": "string"},
            "confirm_resource_name": {"type": "string", "default": ""},
            "project_id": {"type": "string", "default": ""},
            "scaleway_project_id": {"type": "string", "default": ""},
            "management_project_id": {"type": "string"},
            "task_id": {"type": "string"},
            "evidence": {"type": "string"},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "oracle_ask",
        "description": "Chiedi all'Oracle informazioni sull'architettura del progetto.",
        "parameters": {
            "query": {"type": "string"},
            "limit": {"type": "integer", "default": 5},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "project_id": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "oracle_context",
        "description": "Restituisce chunk testuali semanticamente rilevanti per agenti.",
        "parameters": {
            "query": {"type": "string"},
            "limit": {"type": "integer", "default": 8},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "project_id": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "project_structure",
        "description": "Read-only: i file architetturalmente centrali (la 'spina dorsale') del progetto + i conteggi riassuntivi, calcolati in modo deterministico (no-LLM, tree-sitter). Usalo PRIMA di oracle_ask per orientarti su quali file toccare.",
        "parameters": {
            "project_id": {"type": "string"},
            "full": {"type": "boolean", "default": False},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "censor_findings",
        "description": "Legge i finding APERTI di Censor (linter locali + Gemma) per un progetto; filtra per file con `file`.",
        "parameters": {
            "project_id": {"type": "string"},
            "file": {"type": "string", "default": ""},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "censor_dispose",
        "description": "Imposta la disposition di un finding Censor (open|fixed|fp|wontfix) e aggiunge una voce di provenance.",
        "parameters": {
            "project_id": {"type": "string"},
            "file": {"type": "string"},
            "id": {"type": "string"},
            "disposition": {"type": "string", "enum": sorted({"open", "fixed", "fp", "wontfix"})},
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
]


class McpError(ValueError):
    pass


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


# FIX 5: characters that render invisibly or override text direction. Stripped from
# every cleaned field BEFORE the whitespace collapse so a value cannot smuggle a
# right-to-left override / zero-width joiner / BOM into a notification toast or note
# (spoofing or hiding text). Covers: zero-width space/non-joiner/joiner, LRM/RLM,
# the LRE..RLO and LRI..PDI bidi controls + PDF, line/paragraph separators, and BOM.
_INVISIBLE_AND_BIDI_RE = re.compile(
    "["
    "​‌‍"  # ZWSP, ZWNJ, ZWJ
    "‎‏"  # LRM, RLM
    "‪-‮"  # LRE, RLE, PDF, LRO, RLO
    "⁦-⁩"  # LRI, RLI, FSI, PDI
    "  "  # line separator, paragraph separator
    "﻿"  # BOM / zero-width no-break space
    "]"
)


def strip_invisible_and_bidi(text: str) -> str:
    return _INVISIBLE_AND_BIDI_RE.sub("", text)


def clean_text(value: Any, label: str, limit: int = 4000) -> str:
    # FIX 5: remove invisible/bidi controls BEFORE collapsing whitespace so they
    # cannot survive into the stored value (and so a string made entirely of them
    # is correctly treated as empty -> required error).
    text = " ".join(strip_invisible_and_bidi(str(value or "")).split()).strip()
    if not text:
        raise McpError(f"{label} is required.")
    return text[:limit]


# FIX 3: agent ids feed signed notification toasts and session/claim ownership, so
# an unconstrained id (clean_text only collapsed whitespace, leaving spaces, emoji
# and arbitrary unicode) let a rogue local process register as e.g.
# "⚠️ Critical Security Alert" and sign phishing toasts. Mirror the Rust
# `validate_agent_id` allowlist EXACTLY: `[A-Za-z0-9._-]{1,64}`. Generated ids
# ("{role}-{millis}") conform with margin.
_AGENT_ID_RE = re.compile(r"[A-Za-z0-9._-]{1,64}")


def normalize_agent_id(value: Any) -> str:
    """Validate and return an agent id, rejecting anything outside the safe
    allowlist (mirrors Rust validate_agent_id). Applied at agent_register AND every
    handler that looks up a session/claim by agent_id, so a spoofed id can never be
    stored or matched."""
    agent_id = str(value or "").strip()
    if not agent_id:
        raise McpError("Agent id is required.")
    if not _AGENT_ID_RE.fullmatch(agent_id):
        raise McpError(
            "Agent id must use only letters, numbers, '.', '_' or '-' and be 1-64 characters."
        )
    return agent_id


def clean_description(value: Any) -> str | None:
    """Mirror of the Rust `clean_description`: trim + cap at 4000 chars but PRESERVE
    newlines (it is prose, not a single-line field, unlike `clean_text` which
    collapses all whitespace). Returns None when absent/blank so a missing
    description stays absent in the markdown — matching the Rust serde shape where
    `ProjectTask.description` is `Option<String>` (omitted when None)."""
    text = str(value or "").strip()
    if not text:
        return None
    return text[:4000]


VALID_TASK_CATEGORIES = ("feature", "hardening", "bug", "other")


def normalize_task_category(value: Any) -> str:
    """Mirror of the Rust normalize_task_category: feature|hardening|bug|other,
    trimmed + lowercased. Empty defaults to 'other' for agent-created cards."""
    category = str(value or "").strip().lower()
    if not category:
        return "other"
    if category not in VALID_TASK_CATEGORIES:
        raise McpError("Task category must be one of feature, hardening, bug, other.")
    return category


def normalize_project_id(value: str) -> str:
    project_id = str(value or "").strip().lower()
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{1,79}", project_id):
        raise McpError("Project id must use lowercase letters, numbers and hyphens.")
    return project_id


def validate_push_remote(value: Any) -> str | None:
    """FIX F9: validate a git push REMOTE name, mirroring the Rust
    `validate_push_remote` allowlist EXACTLY so the Python writer and the Rust push
    executor agree on what is acceptable. Without this parity, an agent could store a
    remote the Rust side rejects at push time — occupying a queue slot and ringing the
    bell for a request that can NEVER be approved successfully.

    Rules (verbatim from Rust): empty/blank -> None (the Rust side defaults it to
    `origin`); length <= 100; the FIRST char must be ASCII-alphanumeric (a leading
    `-` would be parsed by git as a flag); every char must be in
    `[A-Za-z0-9._-/]`. Raises McpError on violation so the agent fails fast at
    request time instead of at approval time."""
    raw = str(value or "").strip()
    if not raw:
        return None
    if len(raw) > 100:
        raise McpError("Remote name is too long.")
    if not raw[0].isascii() or not raw[0].isalnum():
        raise McpError("Remote name must start with a letter or digit.")
    if not re.fullmatch(r"[A-Za-z0-9._/-]+", raw):
        raise McpError("Remote name may only contain letters, digits, . _ - /")
    return raw


def normalize_task_id(value: str) -> str:
    task_id = str(value or "").strip()
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,39}", task_id):
        raise McpError("Task id is invalid.")
    return task_id


def normalize_current_file_path(value: Any) -> str | None:
    """Normalize the file an agent declares it is working on, for `currentFilePath`.

    The agent may pass an absolute, project-relative, or scanned-folder-relative
    path; we DO NOT resolve it against any root here (only Polis knows the
    scanned-folder root). We only canonicalize separators and bound the length.
    The Rust/Polis side does the building resolution (exact rel-path, then
    suffix, then basename). Backslashes are folded to forward slashes and a
    leading `./` is stripped so the stored value is stable across OSes. Returns
    None for an empty/blank input so an absent file leaves the session field
    untouched. Control chars or an over-long path raise, matching the other path
    validators. Unlike `clean_text`, internal spaces are preserved (paths may
    legitimately contain spaces, e.g. "Aspis Management/src/main.tsx").
    """
    text = str(value if value is not None else "").strip()
    if not text:
        return None
    if any(ord(ch) < 32 for ch in text):
        raise McpError("Current file path contains control characters.")
    if len(text) > 1024:
        raise McpError("Current file path exceeds 1024 characters.")
    normalized = text.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return normalized or None


def normalize_role(value: str) -> str:
    role = str(value or "").strip().lower()
    role = ROLE_ALIASES.get(role, role)
    if role not in VALID_ROLES:
        aliases = ", ".join(sorted(ROLE_ALIASES))
        raise McpError(
            f"Role must be one of: {', '.join(sorted(VALID_ROLES))}"
            + (f" (aliases: {aliases})." if aliases else ".")
        )
    return role


def coerce_role(value: str) -> str:
    # Non-raising role normalization for STORED data: maps valid roles + known
    # aliases to their canonical, and any UNKNOWN/garbage role to the safe "coder"
    # default so a corrupt stored role can never brick a session. Used when loading
    # state and when comparing stored vs incoming roles — distinct from
    # normalize_role(), which RAISES (it gates inbound tool args).
    role = str(value or "").strip().lower()
    role = ROLE_ALIASES.get(role, role)
    return role if role in VALID_ROLES else "coder"


def _roles_same_canonical(a: str, b: str) -> bool:
    # True when two role strings (alias or canonical) collapse to the same canonical
    # role. Used to decide whether a write would merely re-alias the stored role.
    return coerce_role(a) == coerce_role(b)


MODEL_MAX_LEN = 64
# Known model families. A reported model string that contains one of these
# keywords collapses to the family token so the fleet UI can aggregate
# model x role counts (e.g. "claude-opus-4-8" and "Claude Opus 4.8" both -> "opus").
MODEL_FAMILIES = ("opus", "sonnet", "haiku")


def normalize_model(value: Any) -> str:
    """Normalize a self-reported model string for fleet aggregation.

    None / non-str -> "" (blank, meaning "not reported"). Otherwise trim,
    lowercase and cap length. If the cleaned string contains a known family
    keyword it collapses to that family token (opus/sonnet/haiku); any other
    non-empty model is kept as-is (e.g. "deepseek-v3" stays "deepseek-v3").
    """
    if not isinstance(value, str):
        return ""
    cleaned = " ".join(value.split()).strip().lower()
    if not cleaned:
        return ""
    cleaned = cleaned[:MODEL_MAX_LEN]
    for family in MODEL_FAMILIES:
        if family in cleaned:
            return family
    return cleaned


# `needs_user`: the canonical "waiting on the human" status. Agents may report
# it under a few ergonomic aliases (`awaiting_user`, `blocked_on_user`); they all
# normalize to `needs_user` while the ORIGINAL alias is preserved as the
# needsUser.reason so the UI can still show what the agent literally said. Plain
# `blocked` is deliberately NOT in this set: it is a DISTINCT status with its own
# meaning in the claims/task subsystem and must never be folded into needs_user.
NEEDS_USER_STATUS = "needs_user"
NEEDS_USER_STATUS_ALIASES = {"awaiting_user", "blocked_on_user"}


def normalize_agent_status(value: Any) -> str:
    """Normalize a self-reported agent status: collapse whitespace, lowercase,
    and fold the needs_user aliases to the canonical `needs_user`. All other
    statuses (including the distinct `blocked`) pass through cleaned but
    unchanged. Returns "" for a blank/non-string input so the caller can apply
    its own default."""
    cleaned = " ".join(str(value or "").split()).strip().lower()
    if not cleaned:
        return ""
    if cleaned == NEEDS_USER_STATUS or cleaned in NEEDS_USER_STATUS_ALIASES:
        return NEEDS_USER_STATUS
    return cleaned


SUBAGENT_LABEL_MAX_LEN = 80
SUBAGENT_COUNT_MIN = 1
SUBAGENT_COUNT_MAX = 9999
SUBAGENTS_MAX = 32


def _coerce_subagent_count(value: Any) -> int | None:
    """Coerce a subagent count to an int in [1, 9999] or None when invalid.

    Accepts int, a clean float (integral value), or a clean numeric string.
    Out-of-range values are clamped; non-numeric / fractional values are
    rejected (return None) so the caller drops the entry.
    """
    if isinstance(value, bool):
        # bool is an int subclass; treat True/False as invalid counts.
        return None
    if isinstance(value, int):
        number = value
    elif isinstance(value, float):
        if not value.is_integer():
            return None
        number = int(value)
    elif isinstance(value, str):
        text = value.strip()
        if not re.fullmatch(r"-?\d+", text):
            return None
        number = int(text)
    else:
        return None
    if number < SUBAGENT_COUNT_MIN:
        return None
    return min(number, SUBAGENT_COUNT_MAX)


def normalize_subagents(value: Any) -> list[dict[str, Any]] | None:
    """Normalize a self-reported subagent breakdown.

    Returns None when `value` is not a list (meaning "not provided" — leave the
    stored value untouched). An empty list is a VALID value meaning "no
    subagents now" (clears the stored breakdown).

    Each entry must be a dict with a non-empty `label` (str, capped at 80) after
    stripping; `model` is normalized via normalize_model (may be ""); `count` is
    coerced to an int in [1, 9999] (defaults to 1 when absent); `role` is
    optional and normalized via the existing role rules (invalid -> None).
    Entries that fail validation (non-dict, empty label, bad count) are dropped;
    the resulting list is capped at 32 entries.
    """
    if not isinstance(value, list):
        return None
    result: list[dict[str, Any]] = []
    for entry in value:
        if not isinstance(entry, dict):
            continue
        label = " ".join(str(entry.get("label") or "").split()).strip()
        if not label:
            continue
        label = label[:SUBAGENT_LABEL_MAX_LEN]
        raw_count = entry.get("count", 1)
        if raw_count is None:
            raw_count = 1
        count = _coerce_subagent_count(raw_count)
        if count is None:
            continue
        role: str | None = None
        raw_role = entry.get("role")
        if raw_role is not None and str(raw_role).strip():
            try:
                role = normalize_role(str(raw_role))
            except McpError:
                role = None
        result.append(
            {
                "label": label,
                "model": normalize_model(entry.get("model")),
                "count": count,
                "role": role,
            }
        )
        if len(result) >= SUBAGENTS_MAX:
            break
    return result


def normalize_task_status(value: str) -> str:
    status = str(value or "").strip().lower()
    if status not in VALID_TASK_STATUSES:
        raise McpError("Task status must be todo, wip, review, blocked or done.")
    return status


def normalize_project_status(value: str) -> str:
    status = str(value or "").strip().lower()
    if status not in VALID_PROJECT_STATUSES:
        raise McpError("Project status must be active, paused, done or archived.")
    return status


def normalize_provider_name(value: str) -> str:
    return "-".join(part for part in re.split(r"[\s_-]+", str(value or "").strip().lower()) if part)


def validate_management_root(candidate: Path) -> Path:
    root = candidate.expanduser().resolve()
    if root.name == "src-tauri" and root.parent.joinpath("config.json").exists():
        root = root.parent.resolve()
    if not root.joinpath("config.json").is_file() or not root.joinpath("oracle", "server", "aspis_mcp.py").is_file():
        raise McpError(
            "Aspis MCP management root is invalid. Run from Aspis Management, pass --root, or set ASPIS_MANAGEMENT_ROOT."
        )
    root.joinpath("projects").mkdir(parents=True, exist_ok=True)
    return root


def approved_work_root_parents(management_root: Path | None = None) -> list[Path]:
    """Allowlisted parent directories under which a project rootPath may live.

    A project's configured rootPath must live under one of these (or equal one of
    them). This stops a project markdown from pointing Oracle at an arbitrary
    sensitive tree (e.g. C:\\Users\\gualt\\.ssh) just by setting root_path.
    """
    parents: list[Path] = []
    if management_root is not None:
        try:
            parents.append(management_root.expanduser().resolve())
        except OSError:
            pass
    for env_name in ("ASPIS_WORKSPACE_ROOT", "ASPIS_BIO_WORKSPACE_ROOT", "ASPIS_BIO_ROOT"):
        env_value = os.environ.get(env_name)
        if env_value and env_value.strip():
            try:
                parents.append(Path(env_value.strip()).expanduser().resolve())
            except OSError:
                continue
    # Default Aspis Bio workspace parent on this machine.
    default_workspace = Path.home() / "Desktop" / "aspis bio"
    parents.append(default_workspace.expanduser().resolve())
    # De-duplicate while preserving order.
    seen: set[str] = set()
    unique: list[Path] = []
    for parent in parents:
        key = str(parent).lower()
        if key not in seen:
            seen.add(key)
            unique.append(parent)
    return unique


def path_is_within(child: Path, parent: Path) -> bool:
    try:
        child.resolve().relative_to(parent)
        return True
    except (ValueError, OSError):
        return False


def validate_project_work_root(candidate: Path, management_root: Path | None = None) -> Path:
    root = candidate.expanduser().resolve()
    home = Path.home().resolve()
    broad_roots = {home}
    desktop = home / "Desktop"
    if desktop.exists():
        broad_roots.add(desktop.resolve())
    anchor = Path(root.anchor).resolve() if root.anchor else None
    if anchor is not None:
        broad_roots.add(anchor)
    if root in broad_roots:
        # PRIVACY: never echo the absolute path back to an agent (it leaks the OS
        # username + machine layout). Name only the basename + actionable phrase.
        raise McpError(
            f"Project working root '{root.name}' is too broad; "
            "point it at a specific project folder under the Aspis workspace."
        )
    lower = str(root).lower()
    if lower.endswith("\\windows") or "\\windows\\system32" in lower:
        raise McpError(
            f"Project working root '{root.name}' is unsafe (system directory); "
            "use a folder under the Aspis workspace."
        )
    # SECURITY (M2): constrain rootPath to an approved workspace parent so a
    # project markdown cannot index an arbitrary sensitive tree (e.g. ~/.ssh).
    parents = approved_work_root_parents(management_root)
    if not any(root == parent or path_is_within(root, parent) for parent in parents):
        raise McpError(
            "Project working root is outside the approved Aspis workspace roots. "
            "Place it under the management root or the configured Aspis Bio workspace."
        )
    return root


def resolve_root(root: str | Path | None = None) -> Path:
    if root:
        return validate_management_root(Path(root))
    if os.environ.get("ASPIS_MANAGEMENT_ROOT"):
        return validate_management_root(Path(os.environ["ASPIS_MANAGEMENT_ROOT"]))
    return validate_management_root(Path.cwd())


def resolve_projects_dir(root: str | Path | None = None, projects_dir: str | Path | None = None) -> Path:
    management_root = resolve_root(root)
    os.environ["ASPIS_MANAGEMENT_ROOT"] = str(management_root)
    env_dir = str(projects_dir or os.environ.get("ASPIS_PROJECTS_DIR") or "").strip()
    if env_dir and env_dir.strip():
        projects = Path(env_dir.strip()).expanduser().resolve()
    else:
        projects = management_root.joinpath("projects").resolve()
    projects.mkdir(parents=True, exist_ok=True)
    return projects


def management_root_from_projects_dir(projects_dir: Path) -> Path:
    parent = projects_dir.parent if projects_dir.name == "projects" else projects_dir
    try:
        return validate_management_root(parent)
    except McpError:
        pass
    env_root = os.environ.get("ASPIS_MANAGEMENT_ROOT")
    if env_root and env_root.strip():
        return validate_management_root(Path(env_root))
    return parent


def mcp_oracle_paths(projects_dir: Path) -> dict[str, Path]:
    root = management_root_from_projects_dir(projects_dir).resolve()
    oracle_dir = root / "oracle-data"
    return {
        "root": root,
        "sqlite": oracle_dir / "metadata.sqlite",
        "vectors": oracle_dir / "vectors.lancedb",
        "chunks": oracle_dir / "chunks.lancedb",
    }


def make_mcp_engine(projects_dir: Path):
    paths = mcp_oracle_paths(projects_dir)
    cache_key = str(paths["root"])
    engine = _MCP_ENGINE_CACHE.get(cache_key)
    if engine is None:
        from oracle.server.query_engine import QueryEngine

        engine = QueryEngine(
            SQLiteStore(paths["sqlite"]),
            LanceStore(paths["vectors"]),
            LanceStore(paths["chunks"]),
        )
        _MCP_ENGINE_CACHE[cache_key] = engine
    return engine


def oracle_index_root_for_args(projects_dir: Path, args: dict[str, Any]) -> Path:
    management_root = management_root_from_projects_dir(projects_dir).resolve()
    project_id = str(args.get("project_id") or "").strip()
    if project_id:
        project = load_project_locked(projects_dir, project_id)
        root_path = str(project["metadata"].get("rootPath") or "").strip()
        if root_path:
            return validate_project_work_root(Path(root_path), management_root)
    return management_root


def ensure_oracle_index_ready(projects_dir: Path, args: dict[str, Any]) -> dict[str, Any]:
    root = oracle_index_root_for_args(projects_dir, args)
    if not root.is_dir():
        # PRIVACY: basename only — the absolute path would leak the OS username.
        raise McpError(f"Oracle index root '{root.name}' does not exist on disk.")
    mcp_debug(projects_dir, f"oracle_index root={root}")
    cache_key = str(root)
    cached = _MCP_INDEX_STATUS_CACHE.get(cache_key)
    if cached and time.monotonic() - cached[0] < 15:
        mcp_debug(projects_dir, "oracle_index cache hit")
        status = cached[1]
    else:
        mcp_debug(projects_dir, "oracle_index import begin")
        from oracle.ingestion.chunk_index import (
            collect_text_files,
            file_needs_index,
            load_manifest,
            manifest_files_for_root,
            priority_rank,
        )

        mcp_debug(projects_dir, "oracle_index status begin")
        paths = mcp_oracle_paths(projects_dir)
        manifest_path = paths["root"] / "oracle-data" / "chunk-index-manifest.json"
        manifest = load_manifest(manifest_path)
        manifest_files = manifest_files_for_root(manifest, root, create=False)
        sqlite = SQLiteStore(paths["sqlite"])
        output_paths = {paths["sqlite"].resolve(), paths["chunks"].resolve(), manifest_path.resolve()}
        files = [path for path in collect_text_files(root) if path.resolve() not in output_paths]
        expected = {path.relative_to(root).as_posix() for path in files}
        indexed = set(manifest_files)
        pending = sorted(expected - indexed, key=lambda item: (priority_rank(item), item))
        stale = []
        for path in files:
            file_id = path.relative_to(root).as_posix()
            if file_id in indexed and file_needs_index(path, root, manifest_files, sqlite):
                stale.append(file_id)
        status = {
            # PRIVACY: store only the basename. This dict is forwarded into the
            # oracle_ask / oracle_context MCP responses; an absolute root would
            # leak the OS username + machine layout to the agent.
            "root": root.name,
            "expected_files": len(expected),
            "indexed_files": len(indexed & expected),
            "pending_files": len(pending),
            "stale_files": len(stale),
            "sqlite_chunks": sqlite.chunk_count(),
            "first_pending": pending[:12],
            "first_stale": stale[:12],
        }
        mcp_debug(projects_dir, "oracle_index status ok")
        _MCP_INDEX_STATUS_CACHE[cache_key] = (time.monotonic(), status)
    if (
        int(status.get("expected_files") or 0) > 0
        and (
            int(status.get("indexed_files") or 0) == 0
            or int(status.get("sqlite_chunks") or 0) == 0
        )
    ):
        # PRIVACY: no absolute paths in the message (the index root may reveal a
        # user home directory). Keep it ACTIONABLE for an agent operator.
        raise McpError(
            "Oracle index not ready — open Aspis -> Oracle -> Index now "
            "(or wait for the resident indexer). "
            f"(indexed={status.get('indexed_files')} "
            f"pending={status.get('pending_files')} stale={status.get('stale_files')})"
        )
    return status


def enforce_mini_oracle_project_scope(
    projects_dir: Path, agent_id: str, role: str, args: dict[str, Any]
) -> None:
    """SEC#9: a "mini" role session may only read its OWN project's corpus. The
    mini's currentProjectId is reliably set at spawn, so a project_id that
    differs from it is a cross-project read — reject. Non-mini roles and an
    empty/own project_id are unaffected."""
    if role != "mini":
        return
    requested = str(args.get("project_id") or "").strip()
    if not requested:
        return
    with file_lock(projects_dir / f"{AGENTS_STATE_FILE}.lock"):
        state = read_agents_state(projects_dir)
    session = next(
        (s for s in state["sessions"] if s.get("agentId") == agent_id), None
    )
    own = str((session or {}).get("currentProjectId") or "").strip()
    if requested != own:
        raise McpError(
            "A mini agent may only read its own project via oracle_context "
            f"(scoped to {own or 'its spawning project'}, requested {requested})."
        )


def oracle_allowed_file_ids(projects_dir: Path, args: dict[str, Any]) -> set[str] | None:
    paths = mcp_oracle_paths(projects_dir)
    manifest_path = paths["root"] / "oracle-data" / "chunk-index-manifest.json"
    from oracle.ingestion.chunk_index import load_manifest, manifest_files_for_root

    manifest = load_manifest(manifest_path)
    management_root = management_root_from_projects_dir(projects_dir).resolve()

    project_id = str(args.get("project_id") or "").strip()
    if not project_id:
        # SECURITY: an unscoped Oracle read must NEVER surface the union of all
        # indexed project work roots. Default to the management root only, so an
        # agent without an explicit project scope cannot read another project's
        # corpus (or its embedded files) through oracle_ask / oracle_context.
        return set(manifest_files_for_root(manifest, management_root, create=False))

    root = oracle_index_root_for_args(projects_dir, args)
    allowed = set(manifest_files_for_root(manifest, root, create=False))
    if root != management_root:
        allowed.update(manifest_files_for_root(manifest, management_root, create=False))
    return allowed


def ensure_inside_projects(projects_dir: Path, path: Path) -> Path:
    resolved = path.resolve()
    try:
        resolved.relative_to(projects_dir.resolve())
    except ValueError as exc:
        raise McpError("Resolved project path escapes the projects folder.") from exc
    return resolved


@contextmanager
def file_lock(lock_path: Path):
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    handle = lock_path.open("a+b")
    try:
        if os.name == "nt":
            import msvcrt

            handle.seek(0)
            for _ in range(100):
                try:
                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                    break
                except OSError:
                    time.sleep(0.05)
            else:
                raise McpError(f"Could not acquire lock: {lock_path}")
            try:
                yield
            finally:
                handle.seek(0)
                try:
                    msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
                except OSError:
                    pass
        else:
            import fcntl

            fcntl.flock(handle, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(handle, fcntl.LOCK_UN)
    finally:
        handle.close()


def sha256_text(content: str) -> str:
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def hash_launch_token(token: str) -> str:
    return sha256_text(str(token or "").strip())


def generate_session_token() -> str:
    return uuid.uuid4().hex + uuid.uuid4().hex


def hash_session_token(token: str) -> str:
    return sha256_text(str(token or "").strip())


def unmanaged_privileged_agents_allowed() -> bool:
    return os.getenv(ALLOW_UNMANAGED_PRIVILEGED_ENV, "").strip() == "1"


def validate_launch_token_for_registration(
    state: dict[str, Any],
    agent_id: str,
    role: str,
    launch_token: str | None,
) -> dict[str, Any] | None:
    session = next((item for item in state["sessions"] if item.get("agentId") == agent_id), None)
    if session is None:
        if not unmanaged_privileged_agents_allowed():
            raise McpError("Agent registration requires an app-issued launch token from Aspis Management.")
        return None
    # MINOR 2 (defense-in-depth): use the NON-raising `coerce_role` on the STORED
    # role, not the raising `normalize_role`. `normalize_role` is correct for inbound
    # tool args (it must reject garbage), but here `session["role"]` is stored state.
    # It is normally sanitized by `normalize_agents_state` on load, yet relying on
    # that implicit ordering is fragile: a directly-constructed state dict (a test, a
    # future caller) with a corrupt role would otherwise RAISE here and brick
    # registration. `coerce_role` maps any unknown role to the safe "coder" default so
    # the comparison below stays well-defined. The incoming `role` is already
    # normalized by the `agent_register` caller, so the comparison is canonical-vs-
    # canonical.
    existing_role = coerce_role(session.get("role", ""))
    if existing_role != role:
        raise McpError(f"Agent {agent_id} is already registered as {existing_role}.")
    expected_hash = str(session.get("launchTokenHash") or "").strip()
    if expected_hash:
        token = str(launch_token or "").strip()
        if not token:
            raise McpError("Agent registration requires the app-issued launch_token from the launch prompt.")
        issued_at = parse_iso_timestamp(session.get("launchTokenIssuedAt"))
        if issued_at is None or datetime.now(timezone.utc) - issued_at > LAUNCH_TOKEN_WINDOW:
            raise McpError("Agent launch token expired. Relaunch the agent from Aspis Management.")
        if not hmac.compare_digest(hash_launch_token(token), expected_hash):
            raise McpError("Agent launch token is invalid for this agent id and role.")
        return session
    if str(session.get("status") or "").strip().lower() == "launch_pending":
        raise McpError("Pending agent session is missing a launch token. Relaunch the agent from Aspis Management.")
    # SEC#7: a session whose launch token was already CONSUMED cannot be
    # re-registered tokenless — the one-shot launch credential is spent. (A
    # session that never had a hash, i.e. pure unmanaged self-registration, has
    # no launchConsumedAt and is unaffected.)
    if str(session.get("launchConsumedAt") or "").strip():
        raise McpError(
            "Agent launch credential already consumed; relaunch the agent from Aspis Management to register again."
        )
    return session


def parse_simple_yaml(raw: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for line in raw.splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        fields[key.strip()] = unquote_simple_yaml_value(value.strip())
    return fields


def unquote_simple_yaml_value(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] == '"':
        return value[1:-1].replace('\\"', '"').replace("\\\\", "\\")
    if len(value) >= 2 and value[0] == value[-1] == "'":
        return value[1:-1].replace("''", "'")
    return value


def parse_frontmatter(content: str, path: Path) -> tuple[dict[str, Any], int]:
    if not content.startswith("---"):
        raise McpError(f"Project file {path} is missing frontmatter.")
    first_newline = content.find("\n")
    if first_newline < 0:
        raise McpError(f"Project file {path} has malformed frontmatter.")
    close = content.find("\n---", first_newline + 1)
    if close < 0:
        raise McpError(f"Project file {path} has unterminated frontmatter.")
    close_end = content.find("\n", close + 1)
    if close_end < 0:
        close_end = len(content)
    else:
        close_end += 1
    fields = parse_simple_yaml(content[first_newline + 1 : close])
    fallback_id = path.stem
    canonical_id = normalize_project_id(fallback_id)
    project_id = normalize_project_id(fields.get("id", fallback_id))
    if path.is_absolute() and path.exists() and project_id != canonical_id:
        raise McpError(
            f"Project file {path} has id '{project_id}' but filename expects '{canonical_id}'."
        )
    return (
        {
            "id": project_id,
            "title": clean_text(fields.get("title", project_id), "Project title", 500),
            "status": normalize_project_status(fields.get("status", "active")),
            "updatedAt": fields.get("updated_at") or fields.get("updatedAt") or now(),
            "rootPath": fields.get("root_path") or fields.get("rootPath") or fields.get("root"),
        },
        close_end,
    )


def yaml_quote(value: str) -> str:
    return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


def find_state_block(content: str) -> tuple[dict[str, Any], tuple[int, int]]:
    start = content.find(BLOCK_MARKER)
    if start < 0:
        raise McpError("Project file is missing ```aspis-project block.")
    body_start = content.find("\n", start)
    if body_start < 0:
        raise McpError("Project state block is malformed.")
    body_start += 1
    cursor = body_start
    for line in content[body_start:].splitlines(keepends=True):
        line_start = cursor
        cursor += len(line)
        if line.strip() == BLOCK_CLOSE:
            body = content[body_start:line_start].strip()
            try:
                state = json.loads(body) if body else {"version": 1, "tasks": [], "notes": []}
            except json.JSONDecodeError as exc:
                raise McpError(f"Project state JSON is invalid: {exc}") from exc
            state.setdefault("tasks", [])
            state.setdefault("notes", [])
            return state, (start, cursor)
    raise McpError("Project state block is not closed.")


def replace_frontmatter(content: str, metadata: dict[str, Any]) -> str:
    _, frontmatter_end = parse_frontmatter(content, Path("project.md"))
    root_line = f"root_path: {yaml_quote(metadata['rootPath'])}\n" if metadata.get("rootPath") else ""
    frontmatter = (
        "---\n"
        f"id: {metadata['id']}\n"
        f"title: {metadata['title']}\n"
        f"status: {metadata['status']}\n"
        f"updated_at: {metadata['updatedAt']}\n"
        f"{root_line}"
        "---\n"
    )
    return f"{frontmatter}{content[frontmatter_end:]}"


def write_text_crash_safe(path: Path, content: str, label: str) -> None:
    suffix = f"{os.getpid()}-{time.time_ns()}"
    temp_path = path.with_suffix(path.suffix + f".{suffix}.tmp")
    backup_path = path.with_suffix(path.suffix + f".{suffix}.bak")
    try:
        with temp_path.open("w", encoding="utf-8") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        if path.exists():
            shutil.copy2(path, backup_path)
        os.replace(temp_path, path)
        if backup_path.exists():
            try:
                backup_path.unlink()
            except OSError:
                pass
    except Exception as exc:
        try:
            if temp_path.exists():
                temp_path.unlink()
        finally:
            if backup_path.exists() and not path.exists():
                try:
                    shutil.copy2(backup_path, path)
                except Exception:
                    pass
        raise McpError(f"Could not save {label}: {exc}") from exc


def read_project_file(path: Path) -> dict[str, Any]:
    content = path.read_text(encoding="utf-8")
    metadata, _ = parse_frontmatter(content, path)
    state, block_range = find_state_block(content)
    validate_project_state(state)
    modified = datetime.fromtimestamp(path.stat().st_mtime, timezone.utc).isoformat()
    return {
        "metadata": metadata,
        "state": state,
        "markdown": content,
        "revision": sha256_text(content),
        "path": str(path),
        "modifiedAt": modified,
        "_blockRange": block_range,
    }


def validate_task_dependency_dag(deps_by_id: dict[str, list[str]]) -> None:
    """Phase 11.5-B (Piece 1a): the `dependsOn` graph must reference only EXISTING
    task ids and be ACYCLIC.

    `deps_by_id` maps EVERY task id -> its (already-normalized) list of `dependsOn`
    ids. Its keys ARE the complete node set, so the dangling-ref check compares each
    dep against `deps_by_id` directly (no separate id list — they cannot drift).
    Splitting extraction from validation lets the bulk-create path validate the
    REMAPPED graph (post id-allocation) with the same algorithm the on-load
    `validate_project_state` uses on the stored graph.

    Ported verbatim from the Rust planner's `detect_cycle` (Kahn's algorithm): an
    edge `dep -> task` means `task` waits on `dep`, so `task`'s in-degree is its
    `dependsOn` count. We repeatedly remove zero-in-degree nodes; if any remain, they
    form a cycle. The dangling-ref pass runs FIRST (a dangling dep would otherwise
    never decrement an in-degree to zero and be misreported as a cycle).

    Manual tasks (empty `dependsOn`) contribute a zero in-degree node and never
    trigger either failure, so back-compat is preserved.
    """
    known_ids = set(deps_by_id.keys())
    # First pass: every dependsOn entry references an EXISTING task id (no dangling),
    # is not a self-dependency, and is not duplicated (a dup corrupts the in-degree
    # bookkeeping). Mirrors the Rust planner's second pass before detect_cycle.
    for task_id, deps in deps_by_id.items():
        seen: set[str] = set()
        for dep in deps:
            if dep == task_id:
                raise McpError(f"Task {task_id} dependsOn references itself.")
            if dep not in known_ids:
                raise McpError(
                    f"Task {task_id} dependsOn references unknown task id {dep}."
                )
            if dep in seen:
                raise McpError(
                    f"Task {task_id} has a duplicate dependsOn entry {dep}."
                )
            seen.add(dep)
    # Second pass: Kahn's algorithm. in_degree[id] = unresolved prerequisites of id;
    # dependents[dep] = tasks that depend on dep (reverse edges).
    in_degree: dict[str, int] = {tid: len(deps) for tid, deps in deps_by_id.items()}
    dependents: dict[str, list[str]] = {}
    for task_id, deps in deps_by_id.items():
        for dep in deps:
            dependents.setdefault(dep, []).append(task_id)
    queue = [tid for tid, degree in in_degree.items() if degree == 0]
    resolved = 0
    while queue:
        node = queue.pop()
        resolved += 1
        for child in dependents.get(node, []):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                queue.append(child)
    if resolved != len(known_ids):
        raise McpError("Project task dependsOn graph has a cycle (it must be acyclic).")


def validate_project_state(state: dict[str, Any]) -> None:
    if not isinstance(state, dict):
        raise McpError("Project state must be a JSON object.")
    if type(state.get("version")) is not int or state["version"] < 1:
        raise McpError("Project state version is required.")
    tasks = state.get("tasks", [])
    if not isinstance(tasks, list):
        raise McpError("Project state tasks must be a list.")
    task_ids: set[str] = set()
    # Phase 11.5-B (Piece 1a): collect each task's normalized `dependsOn` list so the
    # whole-graph DAG check runs ONCE after the per-task pass (dangling refs can point
    # forward to a task not yet seen in this loop).
    deps_by_id: dict[str, list[str]] = {}
    for task in tasks:
        if not isinstance(task, dict):
            raise McpError("Project state task is invalid.")
        task_id = normalize_task_id(task.get("id", ""))
        if task_id in task_ids:
            raise McpError(f"Duplicate project task id: {task_id}")
        task_ids.add(task_id)
        normalize_task_status(task.get("status", ""))
        clean_text(task.get("title"), "Task title", 500)
        clean_text(task.get("updatedAt"), "Task updatedAt", 80)
        if "linkedResources" in task and not isinstance(task.get("linkedResources"), list):
            raise McpError("Project task linkedResources must be a list.")
        # Phase 11.5-B (Piece 1a) — new optional fields. Each is OPTIONAL so an old
        # `.md` block written before they existed validates UNCHANGED (mirrors the
        # Rust `#[serde(default)]`). When present, enforce the on-disk wire shape:
        #   dependsOn: list[str]  scope: list[str]  acceptance: str  planId: str|null
        depends_on = task.get("dependsOn", [])
        if not isinstance(depends_on, list) or not all(isinstance(d, str) for d in depends_on):
            raise McpError("Project task dependsOn must be a list of task ids.")
        if "scope" in task and (
            not isinstance(task.get("scope"), list)
            or not all(isinstance(s, str) for s in task.get("scope"))
        ):
            raise McpError("Project task scope must be a list of file paths.")
        if "acceptance" in task and not isinstance(task.get("acceptance"), str):
            raise McpError("Project task acceptance must be a string.")
        if task.get("planId") is not None and not isinstance(task.get("planId"), str):
            raise McpError("Project task planId must be a string or null.")
        deps_by_id[task_id] = [normalize_task_id(d) for d in depends_on]
    # Phase 11.5-B (Piece 1a): when ANY task declares dependencies, the whole graph
    # must reference existing ids and be acyclic. Tasks with no deps are unaffected
    # (each is a zero-in-degree node), so manual-task projects keep validating as
    # before. Skip the call entirely when no task has deps to avoid touching the hot
    # path for the common manual-only project.
    if any(deps for deps in deps_by_id.values()):
        validate_task_dependency_dag(deps_by_id)
    notes = state.get("notes", [])
    if not isinstance(notes, list):
        raise McpError("Project state notes must be a list.")
    for note in notes:
        if not isinstance(note, dict):
            raise McpError("Project state note is invalid.")
        clean_text(note.get("id"), "Note id", 120)
        clean_text(note.get("text"), "Note text", 4000)
        clean_text(note.get("source"), "Note source", 120)
        clean_text(note.get("createdAt"), "Note createdAt", 80)


def write_project_file(project: dict[str, Any]) -> dict[str, Any]:
    path = Path(project["path"])
    content = project["markdown"]
    start, end = project["_blockRange"]
    block = f"{BLOCK_MARKER}\n{json.dumps(project['state'], indent=2, ensure_ascii=False)}\n{BLOCK_CLOSE}\n"
    content = content[:start] + block + content[end:]
    content = replace_frontmatter(content, project["metadata"])
    write_text_crash_safe(path, content, "project file")
    return read_project_file(path)


def project_path(projects_dir: Path, project_id: str) -> Path:
    normalized = normalize_project_id(project_id)
    return ensure_inside_projects(projects_dir, projects_dir / f"{normalized}.md")


def project_lock_path(projects_dir: Path, project_id: str) -> Path:
    return project_path(projects_dir, project_id).with_suffix(".md.lock")


def task_counts(tasks: list[dict[str, Any]]) -> dict[str, int]:
    counts = {"todo": 0, "wip": 0, "review": 0, "blocked": 0, "done": 0, "total": len(tasks)}
    for task in tasks:
        status = task.get("status")
        if status in counts:
            counts[status] += 1
    return counts


def summarize_project(project: dict[str, Any]) -> dict[str, Any]:
    return {
        "id": project["metadata"]["id"],
        "title": project["metadata"]["title"],
        "status": project["metadata"]["status"],
        "updatedAt": project["metadata"]["updatedAt"],
        "rootPath": project["metadata"].get("rootPath"),
        "revision": project["revision"],
        "path": project["path"],
        "taskCounts": task_counts(project["state"].get("tasks", [])),
    }


def public_project(project: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in project.items() if not key.startswith("_")}


def next_task_id(tasks: list[dict[str, Any]]) -> str:
    max_id = 0
    for task in tasks:
        task_id = str(task.get("id", ""))
        if task_id.startswith("T") and task_id[1:].isdigit():
            max_id = max(max_id, int(task_id[1:]))
    return f"T{max_id + 1}"


def read_agents_state(projects_dir: Path) -> dict[str, Any]:
    path = projects_dir / AGENTS_STATE_FILE
    if not path.exists():
        return default_agents_state()
    try:
        state = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise McpError(f"Agents state is invalid JSON: {exc}") from exc
    return reconcile_agents_state_with_projects(projects_dir, normalize_agents_state(state))


def write_agents_state(projects_dir: Path, state: dict[str, Any]) -> dict[str, Any]:
    state = normalize_agents_state(state)
    state["updatedAt"] = now()
    state["events"] = state["events"][-MAX_EVENTS:]
    path = projects_dir / AGENTS_STATE_FILE
    write_text_crash_safe(path, json.dumps(state, indent=2, ensure_ascii=False), "agent state file")
    return state


def public_agents_state(state: dict[str, Any], session_token: str | None = None) -> dict[str, Any]:
    public = json.loads(json.dumps(state))
    for session in public.get("sessions", []):
        session.pop("launchTokenHash", None)
        session.pop("launchTokenIssuedAt", None)
        session.pop("sessionTokenHash", None)
        session.pop("sessionTokenIssuedAt", None)
    if session_token:
        public["sessionToken"] = session_token
    return public


def default_agents_state() -> dict[str, Any]:
    return {
        "version": AGENTS_STATE_VERSION,
        "updatedAt": now(),
        "sessions": [],
        "claims": [],
        "events": [],
        "rules": ROLE_RULES,
        "miniCoderDirectives": [],
    }


def normalize_agents_state(state: dict[str, Any]) -> dict[str, Any]:
    # Version: tolerant READ that UPGRADES on load. Old files (version 1, or
    # missing/garbage) load fine and are NOT rejected. The schema additions below
    # are purely additive, so once we backfill them the file genuinely conforms to
    # AGENTS_STATE_VERSION — stamp it as such so it stops looking like a v1 file
    # forever. A FUTURE (higher) version is left untouched: we never downgrade.
    stored_version = state.get("version")
    if isinstance(stored_version, bool) or not isinstance(stored_version, int):
        # Missing or garbage (incl. bool, which is an int subclass) -> current.
        state["version"] = AGENTS_STATE_VERSION
    elif stored_version < AGENTS_STATE_VERSION:
        state["version"] = AGENTS_STATE_VERSION
    # else: stored_version >= AGENTS_STATE_VERSION -> leave as is (never downgrade).
    state.setdefault("updatedAt", now())
    state.setdefault("sessions", [])
    state.setdefault("claims", [])
    state.setdefault("events", [])
    # Mini-coder directive queue (P2): co-owned with the Rust executor. Backfill so
    # an older `.aspis-agents.json` (no key) forward-loads; a non-list value (hand
    # edit / partial write) is reset to [] rather than bricking the read. Capped at
    # the single normalize choke point so the queue cannot grow without bound.
    # PASSTHROUGH: each directive dict is preserved VERBATIM (only filtered/reordered
    # by cap). The Rust-set fields `scratchPath`/`claimedAt` (BLOCKER/WARNING 3+4) are
    # therefore round-tripped untouched — Python never sets, validates, or strips
    # them. Do NOT add per-field whitelisting here or those Rust-owned keys would be
    # dropped, breaking finalization (scratchPath) and the launch-stuck cap (claimedAt).
    directives = state.get("miniCoderDirectives")
    if not isinstance(directives, list):
        directives = []
    state["miniCoderDirectives"] = cap_mini_coder_directives(directives)
    visual_directives = state.get("visualCheckDirectives")
    if not isinstance(visual_directives, list):
        visual_directives = []
    state["visualCheckDirectives"] = cap_visual_check_directives(visual_directives)
    # GH-P4: git push-approval queue, co-owned with the Rust approve/deny commands.
    # Same backfill + non-list-reset + single-choke-point cap discipline as the
    # mini-coder queue. PASSTHROUGH: each request dict is preserved VERBATIM (only
    # filtered/reordered by cap); the Rust-set `result` is round-tripped untouched.
    push_requests = state.get("gitPushRequests")
    if not isinstance(push_requests, list):
        push_requests = []
    capped_push = cap_git_push_requests(push_requests)
    # NO-CHURN: only persist the key when there is push activity (matches the Rust
    # serde skip_serializing_if on an empty Vec), so an unrelated state with no
    # pushes is not rewritten with an injected empty `gitPushRequests`.
    if capped_push:
        state["gitPushRequests"] = capped_push
    elif "gitPushRequests" in state:
        del state["gitPushRequests"]
    # Phase 1: plan-approval queue, co-owned with the Rust approve/reject commands.
    # Same backfill + non-list-reset + single-choke-point cap + NO-CHURN key deletion
    # discipline as gitPushRequests. PASSTHROUGH: each request dict is preserved
    # VERBATIM (only filtered/reordered by cap); the Rust-set `decidedAt`/`note` are
    # round-tripped untouched. Capped at 20 (evict oldest TERMINAL first, never pending).
    plan_requests = state.get("planApprovalRequests")
    if not isinstance(plan_requests, list):
        plan_requests = []
    capped_plans = cap_plan_approval_requests(plan_requests)
    if capped_plans:
        state["planApprovalRequests"] = capped_plans
    elif "planApprovalRequests" in state:
        del state["planApprovalRequests"]
    for session in state["sessions"]:
        # Schema seam: every read backfills the v2 per-session fields so older
        # `.aspis-agents.json` files (no subagents/needsUser) are forward-compatible.
        session.setdefault("subagents", [])
        session.setdefault("needsUser", None)
        # WARNING 7 / BLOCKER C: sanitize the stored role on load so a corrupt,
        # unknown, OR MISSING role can never brick a session (every tool call goes
        # through require_registered_role, which raises on an unknown/empty role).
        # This is UNCONDITIONAL: a session with NO "role" key (hand-edited file, an
        # older writer, a partial write) previously fell through with no role and
        # bricked every subsequent tool call + re-registration. We now always stamp
        # a usable role. PRESERVE valid roles AND known aliases
        # ("orchestrator"/"architect"/"code") verbatim — the alias is load-bearing
        # for the derived badge + back-compat — but fold any UNKNOWN/MISSING/empty
        # role to the safe "coder" default. (lowercase/trim so a casing-only variant
        # canonicalizes.)
        stored = str(session.get("role") or "").strip().lower()
        if stored in VALID_ROLES or stored in ROLE_ALIASES:
            session["role"] = stored
        else:
            session["role"] = "coder"
    seen_event_ids: set[str] = set()
    for event in state["events"]:
        current_id = str(event.get("id") or "").strip()
        if not current_id or current_id in seen_event_ids:
            current_id = event_id()
            event["id"] = current_id
        seen_event_ids.add(current_id)
    # FIX 6: bound the sessions/claims lists at the single normalize choke point.
    state["sessions"] = cap_sessions(state["sessions"])
    state["claims"] = cap_claims(state["claims"])
    state["rules"] = ROLE_RULES
    return state


def _session_sort_key(session: dict[str, Any]) -> str:
    # Most-recent-first ordering uses lastSeenAt, falling back to firstSeenAt so a
    # session that only ever registered still orders sensibly. Empty sorts oldest.
    return str(session.get("lastSeenAt") or session.get("firstSeenAt") or "")


def cap_sessions(sessions: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """FIX 6: keep at most MAX_SESSIONS sessions. A session is droppable ONLY when
    its status is 'closed'; live sessions are NEVER dropped even past the cap (if
    200+ are live they are all kept). When over the cap, drop the OLDEST closed
    sessions (by lastSeenAt) first, preserving the original list order otherwise."""
    if len(sessions) <= MAX_SESSIONS:
        return sessions
    closed = [s for s in sessions if str(s.get("status") or "").strip().lower() == "closed"]
    drop_count = len(sessions) - MAX_SESSIONS
    if drop_count <= 0 or not closed:
        return sessions
    # Oldest closed sessions are the drop candidates.
    closed_sorted_oldest = sorted(closed, key=_session_sort_key)
    to_drop = {id(s) for s in closed_sorted_oldest[:drop_count]}
    return [s for s in sessions if id(s) not in to_drop]


def _claim_is_terminal(claim: dict[str, Any], reference: datetime) -> bool:
    # A claim is droppable when its task reached a terminal status OR its lease has
    # expired. Open/working claims (todo/wip/review/claimed/blocked/provider_*) and
    # claims with no/future lease are kept regardless of the cap.
    if str(claim.get("status") or "").strip().lower() in TERMINAL_CLAIM_STATUSES:
        return True
    lease_until = parse_iso_timestamp(claim.get("leaseUntil"))
    return lease_until is not None and lease_until < reference


def _claim_sort_key(claim: dict[str, Any]) -> str:
    return str(claim.get("updatedAt") or claim.get("claimedAt") or "")


def cap_claims(claims: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """FIX 6: keep at most MAX_CLAIMS claims. OPEN/working claims are NEVER dropped;
    only TERMINAL claims (done or lease-expired) are pruned, oldest first, when over
    the cap. Original list order is otherwise preserved."""
    if len(claims) <= MAX_CLAIMS:
        return claims
    reference = datetime.now(timezone.utc)
    terminal = [c for c in claims if _claim_is_terminal(c, reference)]
    drop_count = len(claims) - MAX_CLAIMS
    if drop_count <= 0 or not terminal:
        return claims
    terminal_sorted_oldest = sorted(terminal, key=_claim_sort_key)
    to_drop = {id(c) for c in terminal_sorted_oldest[:drop_count]}
    return [c for c in claims if id(c) not in to_drop]


# Mini-coder directive lifecycle statuses (mirror `MiniCoderStatus` in
# src-tauri/src/backend/mini_coder.rs). ACTIVE = launching|running (never dropped,
# never re-claimed); TERMINAL = the rest (eligible for eviction). `escalated` is the
# retry/escalation chain's exhausted-terminal (retries spent, Censor still dirty).
# `awaiting_retry` is deliberately NEITHER active nor terminal here: it is a
# predecessor PARKED waiting for its retry's verdict, so it must NOT be evicted as
# terminal — it is not in this set (cap_mini_coder_directives keeps it).
_MINI_ACTIVE_STATUSES = {"pending", "launching", "running"}
_MINI_TERMINAL_STATUSES = {
    "done",
    "needs_clarification",
    "aborted_by_human",
    "failed",
    "timeout",
    "escalated",
}


def cap_mini_coder_directives(directives: list[Any]) -> list[dict[str, Any]]:
    """Keep at most MAX_MINI_CODER_DIRECTIVES directives. Mirrors `cap_directives`
    in mini_coder.rs: only TERMINAL directives are evicted (oldest by createdAt,
    id tie-break); ACTIVE/pending directives are NEVER dropped even past the cap (a
    pending request or a running mini must not be lost). Non-dict entries (hand
    edit / partial write) are filtered out so a stray value cannot brick the read.
    Original order is otherwise preserved."""
    clean = [d for d in directives if isinstance(d, dict)]
    if len(clean) <= MAX_MINI_CODER_DIRECTIVES:
        return clean
    drop_count = len(clean) - MAX_MINI_CODER_DIRECTIVES
    terminal = [
        d
        for d in clean
        if str(d.get("status") or "").strip().lower() in _MINI_TERMINAL_STATUSES
    ]
    if drop_count <= 0 or not terminal:
        return clean
    terminal_sorted_oldest = sorted(
        terminal,
        key=lambda d: (str(d.get("createdAt") or ""), str(d.get("id") or "")),
    )
    to_drop = {id(d) for d in terminal_sorted_oldest[:drop_count]}
    return [d for d in clean if id(d) not in to_drop]


_VISUAL_CHECK_ACTIVE_STATUSES = {"pending", "running"}
_VISUAL_CHECK_TERMINAL_STATUSES = {"done", "failed", "timeout"}


def cap_visual_check_directives(directives: list[Any]) -> list[dict[str, Any]]:
    """Keep visual-check directives bounded without dropping pending/running work."""
    clean = [d for d in directives if isinstance(d, dict)]
    if len(clean) <= MAX_VISUAL_CHECK_DIRECTIVES:
        return clean
    drop_count = len(clean) - MAX_VISUAL_CHECK_DIRECTIVES
    terminal = [
        d
        for d in clean
        if str(d.get("status") or "").strip().lower() in _VISUAL_CHECK_TERMINAL_STATUSES
    ]
    if drop_count <= 0 or not terminal:
        return clean
    terminal_sorted_oldest = sorted(
        terminal,
        key=lambda d: (str(d.get("createdAt") or ""), str(d.get("id") or "")),
    )
    to_drop = {id(d) for d in terminal_sorted_oldest[:drop_count]}
    return [d for d in clean if id(d) not in to_drop]


# GH-P4: terminal push-request statuses (eligible for eviction), mirroring
# `GitPushStatus::is_terminal` in the Rust git_push.rs.
_GIT_PUSH_TERMINAL_STATUSES = {
    "pushed",
    "push_failed",
    "denied",
    "timeout",
}


def cap_git_push_requests(requests: list[Any]) -> list[dict[str, Any]]:
    """Keep at most MAX_GIT_PUSH_REQUESTS push requests. Mirrors `cap_push_requests`
    in git_push.rs: only TERMINAL requests are evicted (oldest by createdAt, id
    tie-break); ACTIVE requests (pending_approval/approved/pushing) are NEVER dropped
    even past the cap (an unanswered ask or an in-flight push must not be lost).
    Non-dict entries (hand edit / partial write) are filtered out so a stray value
    cannot brick the read. Original order is otherwise preserved."""
    clean = [r for r in requests if isinstance(r, dict)]
    if len(clean) <= MAX_GIT_PUSH_REQUESTS:
        return clean
    drop_count = len(clean) - MAX_GIT_PUSH_REQUESTS
    terminal = [
        r
        for r in clean
        if str(r.get("status") or "").strip().lower() in _GIT_PUSH_TERMINAL_STATUSES
    ]
    if drop_count <= 0 or not terminal:
        return clean
    terminal_sorted_oldest = sorted(
        terminal,
        key=lambda r: (str(r.get("createdAt") or ""), str(r.get("id") or "")),
    )
    to_drop = {id(r) for r in terminal_sorted_oldest[:drop_count]}
    return [r for r in clean if id(r) not in to_drop]


# Phase 1: terminal plan-approval statuses (eligible for eviction). The active state
# is `pending_approval` (awaiting the human) — it is NEVER evicted even past the cap.
_PLAN_TERMINAL_STATUSES = {
    "approved",
    "rejected",
    "timeout",
}


def cap_plan_approval_requests(requests: list[Any]) -> list[dict[str, Any]]:
    """Keep at most MAX_PLAN_APPROVAL_REQUESTS plan-approval requests. Same discipline
    as cap_git_push_requests: only TERMINAL requests are evicted (oldest by createdAt,
    id tie-break); a `pending_approval` (an unanswered plan awaiting the human) is NEVER
    dropped even past the cap. Non-dict entries (hand edit / partial write) are filtered
    out so a stray value cannot brick the read. Original order is otherwise preserved."""
    clean = [r for r in requests if isinstance(r, dict)]
    if len(clean) <= MAX_PLAN_APPROVAL_REQUESTS:
        return clean
    drop_count = len(clean) - MAX_PLAN_APPROVAL_REQUESTS
    terminal = [
        r
        for r in clean
        if str(r.get("status") or "").strip().lower() in _PLAN_TERMINAL_STATUSES
    ]
    if drop_count <= 0 or not terminal:
        return clean
    terminal_sorted_oldest = sorted(
        terminal,
        key=lambda r: (str(r.get("createdAt") or ""), str(r.get("id") or "")),
    )
    to_drop = {id(r) for r in terminal_sorted_oldest[:drop_count]}
    return [r for r in clean if id(r) not in to_drop]


def reconcile_agents_state_with_projects(projects_dir: Path, state: dict[str, Any]) -> dict[str, Any]:
    project_cache: dict[str, dict[str, Any] | None] = {}

    def project_for(project_id: str) -> dict[str, Any] | None:
        if project_id not in project_cache:
            try:
                project_cache[project_id] = read_project_file(project_path(projects_dir, project_id))
            except Exception:
                project_cache[project_id] = None
        return project_cache[project_id]

    for claim in state.get("claims", []):
        project_id = str(claim.get("projectId") or "").strip()
        task_id = str(claim.get("taskId") or "").strip()
        if not project_id or not task_id:
            continue
        project = project_for(project_id)
        if not project:
            claim["status"] = "blocked"
            if not claim.get("evidence"):
                claim["evidence"] = "Project file missing during agent-state reconciliation."
            continue
        claim["projectTitle"] = project["metadata"].get("title") or claim.get("projectTitle")
        task = next((item for item in project["state"].get("tasks", []) if item.get("id") == task_id), None)
        if not task:
            claim["status"] = "blocked"
            if not claim.get("evidence"):
                claim["evidence"] = "Task missing during agent-state reconciliation."
            continue
        claim["taskTitle"] = task.get("title") or claim.get("taskTitle")
    return state


def event_id() -> str:
    return f"E{time.time_ns()}-{uuid.uuid4().hex[:8]}"


def note_id() -> str:
    return f"N{time.time_ns()}-{uuid.uuid4().hex[:8]}"


def add_event(
    state: dict[str, Any],
    agent_id: str,
    role: str,
    event_type: str,
    message: str,
    project_id: str | None = None,
    task_id: str | None = None,
    status: str | None = None,
    evidence: str | None = None,
) -> None:
    state["events"].append(
        {
            "id": event_id(),
            "timestamp": now(),
            "agentId": agent_id,
            "role": role,
            "eventType": event_type,
            "projectId": project_id,
            "taskId": task_id,
            "status": status,
            "message": clean_text(message, "Event message", 1000),
            "evidence": clean_text(evidence, "Evidence", 2000) if evidence else None,
        }
    )


# Sentinel for "param not provided" so callers can distinguish an ABSENT
# `subagents` (leave the stored value untouched) from an explicit empty list
# `[]` (clear the stored breakdown). None alone cannot carry that distinction
# because normalize_subagents(None) is also "not provided".
_UNSET = object()


def upsert_session(
    state: dict[str, Any],
    agent_id: str,
    role: str,
    model: str | None = None,
    status: str = "active",
    message: str | None = None,
    project_id: str | None = None,
    task_id: str | None = None,
    client: str | None = None,
    file_path: str | None = None,
    subagents: Any = _UNSET,
    parent_agent_id: str | None = None,
) -> None:
    # FIX 3: lowest write choke point — a spoofed id can never be persisted.
    clean_agent_id = normalize_agent_id(agent_id)
    # Preserve the raw status text (cleaned for control chars only) so the
    # needsUser.reason can record what the agent literally reported, then fold
    # the needs_user aliases for the STORED status. `blocked` is left distinct.
    raw_status = clean_text(status, "Agent status", 80)
    clean_status = normalize_agent_status(raw_status) or raw_status
    session = next(
        (item for item in state["sessions"] if item.get("agentId") == clean_agent_id),
        None,
    )
    normalized_file_path = (
        normalize_current_file_path(file_path)
        if file_path is not None and str(file_path).strip()
        else None
    )
    # `subagents`: overwrite-only-when-provided (mirrors the file_path pattern).
    # _UNSET means the caller did not pass it -> keep whatever the session has.
    # A provided value (including []) is normalized and replaces the stored one.
    # normalize_subagents returns None for a non-list input ("not provided"); we
    # collapse that back to _UNSET so a malformed value leaves the stored
    # breakdown untouched instead of overwriting it with None.
    if subagents is _UNSET:
        normalized_subagents: Any = _UNSET
    else:
        normalized_subagents = normalize_subagents(subagents)
        if normalized_subagents is None:
            normalized_subagents = _UNSET
    if session is None:
        session = {"agentId": clean_agent_id, "firstSeenAt": now()}
        state["sessions"].append(session)
    # BLOCKER 1: never DOWNGRADE a stored alias role to its own canonical. A legacy
    # session stored as role="orchestrator" is now first-class and normalizes to
    # "orchestrator" (not downgraded); "architect" and "code" still normalize to "coder".
    # Writing it back would destroy the legacy role string on disk and with it the
    # derived UI badge, diverging from Rust (which never rewrites the role). So when
    # the session ALREADY has a role whose normalized form equals the incoming
    # normalized role, PRESERVE the stored string verbatim. Only a genuinely
    # DIFFERENT canonical role (e.g. coder -> verifier) is allowed to overwrite it.
    stored_role = session.get("role")
    if stored_role and _roles_same_canonical(stored_role, role):
        role = stored_role
    # `currentFilePath`: the file the agent is currently editing/working on, so
    # Polis can place its building on the EXACT file (not a "representative"
    # one). It is OPTIONAL and backward-compatible: when `file_path` is None
    # (the agent did not declare a file) we leave whatever the session already
    # carries untouched, so an agent that never sets it keeps the field
    # absent/None and Polis falls back to the representative building.
    # Stored as the agent gave it (normalized for separators/limits only): the
    # path may be absolute, project-relative, or scanned-folder-relative. The
    # Rust/Polis side resolves it to a building (exact rel-path, then suffix,
    # then basename match) since only Polis knows the scanned-folder root.
    # `model`: normalized for fleet aggregation (opus/sonnet/haiku family or the
    # cleaned raw string). Overwrite only when a non-blank model is reported, so
    # a heartbeat that omits it keeps the registration-time value.
    normalized_model = normalize_model(model)
    session.update(
        {
            "role": role,
            "model": normalized_model if normalized_model else session.get("model", ""),
            "status": clean_status,
            # Strip-check first: a whitespace-only message is truthy under a bare
            # `if message` but clean_text raises on it. Treat blank/whitespace as
            # "no message update" and keep the prior session message (#5).
            "message": clean_text(message, "Message", 1000)
            if str(message or "").strip()
            else session.get("message"),
            "currentProjectId": project_id if project_id is not None else session.get("currentProjectId"),
            "currentTaskId": task_id if task_id is not None else session.get("currentTaskId"),
            "client": clean_text(client, "Client", 40) if client else session.get("client"),
            "currentFilePath": normalized_file_path
            if normalized_file_path is not None
            else session.get("currentFilePath"),
            "lastSeenAt": now(),
        }
    )
    # `subagents`: replace only when provided (incl. empty list to clear).
    if normalized_subagents is not _UNSET:
        session["subagents"] = normalized_subagents
    else:
        session.setdefault("subagents", [])
    # `parentAgentId` (P2): set ONLY when a non-blank value is supplied (i.e. this
    # session is a mini-coder registering under its parent). NO-CHURN: an ordinary
    # agent never passes it, so the key stays absent and the Rust round-trip / the
    # TS mirror stay byte-identical for non-mini sessions. Headless P2 minis do not
    # register, so this is dormant until P3/P4; kept here so a real mini that DOES
    # register lands nested under its coder. Validated through the same agent-id
    # allowlist so a spoofed parent id can never be persisted.
    if parent_agent_id is not None and str(parent_agent_id).strip():
        session["parentAgentId"] = normalize_agent_id(parent_agent_id)
    # `needsUser`: set when the normalized status is `needs_user`, cleared on any
    # other (working) status. The reason records the ORIGINAL alias the agent
    # reported (`awaiting_user`/`blocked_on_user`/`needs_user`). The `since`
    # transition timestamp is PRESERVED across repeated needs_user heartbeats so
    # the frontend can dedup OS notifications on it — a heartbeat loop must not
    # keep resetting it. The message is refreshed each time (capped/cleaned).
    if clean_status == NEEDS_USER_STATUS:
        previous = session.get("needsUser") if isinstance(session.get("needsUser"), dict) else None
        previous_since = (previous or {}).get("since")
        # Strip-check FIRST: a whitespace-only message ("   ") is truthy under a
        # bare `if message`, but clean_text rejects an all-whitespace value and
        # raises McpError. Treat blank/whitespace-only as "no message" and fall
        # back to the status sentinel so a needs_user heartbeat never fails on it.
        needs_message = (
            clean_text(message, "Message", 1000)
            if str(message or "").strip()
            else NEEDS_USER_STATUS
        )
        session["needsUser"] = {
            "reason": raw_status or NEEDS_USER_STATUS,
            "message": needs_message,
            "since": previous_since or now(),
        }
    else:
        session["needsUser"] = None


def require_session_token(session: dict[str, Any], session_token: str | None) -> None:
    expected_hash = str(session.get("sessionTokenHash") or "").strip()
    # SECURITY: the compat kill switch
    # (ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS) only covers self-REGISTRATION
    # of agents that have no app-issued session token. It must NEVER let a request
    # skip per-call session-token verification once a session token hash exists for
    # the agent: an app-launched agent always presents its session token on each
    # call. Only when no hash exists may the compat flag allow a tokenless call.
    if not expected_hash:
        if unmanaged_privileged_agents_allowed() and not str(session_token or "").strip():
            return
        raise McpError("Agent session is missing a session token. Relaunch the agent from Aspis Management.")
    token = str(session_token or "").strip()
    if not token:
        raise McpError("Tool call requires the session_token returned by agent_register.")
    issued_at = parse_iso_timestamp(session.get("sessionTokenIssuedAt"))
    if issued_at is None or datetime.now(timezone.utc) - issued_at > SESSION_TOKEN_WINDOW:
        raise McpError("Agent session token expired. Relaunch the agent from Aspis Management.")
    if not hmac.compare_digest(hash_session_token(token), expected_hash):
        raise McpError("Agent session token is invalid for this agent id and role.")


def require_registered_role(
    projects_dir: Path,
    agent_id: str,
    requested_role: str,
    tool_name: str,
    session_token: str | None = None,
) -> str:
    # FIX 3: validate the id before any session lookup so a spoofed id matches
    # nothing AND is rejected with a clear error.
    clean_agent_id = normalize_agent_id(agent_id)
    role = normalize_role(requested_role)
    with file_lock(projects_dir / f"{AGENTS_STATE_FILE}.lock"):
        state = read_agents_state(projects_dir)
    session = next(
        (item for item in state["sessions"] if item.get("agentId") == clean_agent_id),
        None,
    )
    if session is None:
        raise McpError("Agent must call agent_register before using project or provider tools.")
    if str(session.get("status") or "").strip().lower() == "launch_pending":
        raise McpError("Agent launch is pending. Call agent_register before using project or provider tools.")
    registered_role = normalize_role(session.get("role", ""))
    if registered_role != role:
        raise McpError(
            f"Agent role mismatch: registered as {registered_role}, requested {role}."
        )
    if tool_name not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use {tool_name}.")
    require_session_token(session, session_token)
    return role


def validate_transition(
    role: str,
    status: str,
    evidence: str,
    confidence: float,
    current_status: str | None = None,
) -> None:
    if current_status == "done":
        raise McpError("Done tasks cannot be changed through project_update_status.")
    # Phase B merge: the coder absorbs the former orchestrator's planning power,
    # so it may also reopen a task to `todo` (in addition to wip/review/blocked).
    # The orchestrator (devboule-coder) shares these exact coder semantics
    # (CODER_LIKE_ROLES) — same status set, never the verifier-only `done`.
    if role in CODER_LIKE_ROLES and status not in {"todo", "wip", "review", "blocked"}:
        raise McpError("Coder can only set todo, wip, review or blocked.")
    if role == "verifier" and status not in {"done", "blocked"}:
        raise McpError("Verifier can only set done or blocked.")
    if status in {"review", "blocked"} and len(evidence.strip()) < 12:
        raise McpError(f"{status.capitalize()} requires concrete evidence.")
    if status == "done":
        if role != "verifier":
            raise McpError("Only verifier agents can set done.")
        if len(evidence.strip()) < 12:
            raise McpError("Done requires concrete evidence.")
        if confidence < 0.70:
            raise McpError("Done requires confidence >= 0.70.")
        if current_status != "review":
            raise McpError("Done requires the task to be in review first.")


def parse_iso_timestamp(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=timezone.utc)
    return parsed


def claim_is_active(claim: dict[str, Any]) -> bool:
    if claim.get("status") in {"done", "review", "blocked"}:
        return False
    lease_until = parse_iso_timestamp(claim.get("leaseUntil"))
    if lease_until is None:
        updated_at = parse_iso_timestamp(claim.get("updatedAt") or claim.get("claimedAt"))
        return updated_at is not None and datetime.now(timezone.utc) - updated_at <= LEASELESS_CLAIM_WINDOW
    return lease_until > datetime.now(timezone.utc)


def active_claim_for_task(
    state: dict[str, Any],
    project_id: str,
    task_id: str,
) -> dict[str, Any] | None:
    for claim in state.get("claims", []):
        if claim.get("projectId") == project_id and claim.get("taskId") == task_id and claim_is_active(claim):
            return claim
    return None


def owns_own_claim_for_task(
    state: dict[str, Any],
    agent_id: str,
    role: str,
    project_id: str,
    task_id: str,
) -> dict[str, Any] | None:
    # The agent's OWN claim on this task, regardless of whether claim_is_active()
    # currently treats it as active. A claim the agent moved to "review" reports
    # inactive (review is a handoff-to-verifier state), but the owner still owns it.
    for claim in state.get("claims", []):
        if (
            claim.get("projectId") == project_id
            and claim.get("taskId") == task_id
            and claim.get("agentId") == agent_id
            and normalize_role(claim.get("role", "")) == role
            and claim.get("status") != "done"
        ):
            return claim
    return None


def require_claim_for_status_update(
    state: dict[str, Any],
    agent_id: str,
    role: str,
    project_id: str,
    task_id: str,
    target_status: str | None = None,
) -> None:
    claim = active_claim_for_task(state, project_id, task_id)
    if claim is None:
        # WARNING 3: a coder may move its OWN task to "review" and then reopen it to
        # "todo" (the merge documented "coder can reopen to todo"). But once the
        # claim is in "review", claim_is_active() reports it inactive, so the active
        # lookup above misses it and the agent would wrongly hit "must claim the
        # task". For a →todo REOPEN by the claim OWNER, treat the owner's own
        # (non-done) claim as sufficient. This does NOT loosen any verifier/other
        # gate: a different agent still has no claim, and validate_transition() still
        # enforces which statuses each role may set.
        if target_status == "todo":
            own = owns_own_claim_for_task(state, agent_id, role, project_id, task_id)
            if own is not None:
                # Re-activate the owner's claim inline so the subsequent claim-status
                # sync in project_update_status finds a live claim to update.
                own["status"] = "wip"
                own["updatedAt"] = now()
                return
        raise McpError("Agent must claim the task before updating status.")
    if claim.get("agentId") != agent_id:
        raise McpError(
            f"Task is claimed by {claim.get('agentId')} until {claim.get('leaseUntil')}."
        )
    if normalize_role(claim.get("role", "")) != role:
        raise McpError("Claim role does not match the registered agent role.")


def provider_mutation_approval_enforced() -> bool:
    # Default: ENFORCED. The opt-out exists only for legacy/local single-operator
    # setups and must be set explicitly. Never silently self-attested.
    return os.getenv("ASPIS_MCP_ALLOW_SELF_ATTESTED_PROVIDER_MUTATION", "").strip() != "1"


def task_provider_mutation_approver(task: dict[str, Any]) -> str:
    for field in ("approvedBy", "approved_by", "providerMutationApprovedBy"):
        value = str(task.get(field) or "").strip()
        if value:
            return value
    return ""


def require_live_task_for_provider_mutation(
    projects_dir: Path,
    project_id: str,
    task_id: str,
) -> dict[str, Any]:
    with file_lock(project_lock_path(projects_dir, project_id)):
        project = read_project_file(project_path(projects_dir, project_id))
        project_status = project["metadata"].get("status")
        if project_status != "active":
            raise McpError("Provider mutations require an active Management project.")
        task = next((item for item in project["state"].get("tasks", []) if item.get("id") == task_id), None)
        if task is None:
            raise McpError("Provider mutations require a live task in the Management project.")
        if task.get("status") not in {"wip", "blocked"}:
            raise McpError("Provider mutations require the live task to be wip or blocked.")
        # SECURITY (H4): a destructive provider mutation (Worker secret rotation,
        # VM terminate) must not be fully self-attested by a single coder. The MCP
        # surface lets a coder self-set wip + write its own claim + supply >=12-char
        # evidence, so wip + claim alone is not a real second party. Require an
        # explicit approval marker on the task. A coder agent cannot set this field
        # through any MCP tool (project_update_status only writes status/updatedAt),
        # so the approver must be a non-coder: the human operator via the app, or a
        # verifier-authored task edit.
        #
        # SECURITY TODO: the approval marker currently lives in the project markdown,
        # which trusts that only non-coder parties (app/human/verifier) can edit that
        # file. A full fix would record the approval as a signed, non-coder MCP event
        # (e.g. a verifier-only `project_approve_provider_mutation` tool) so it cannot
        # be forged by a coder with raw filesystem access. Tracked as a product
        # decision; until then the approval field is the minimal safe tightening.
        if provider_mutation_approval_enforced() and not task_provider_mutation_approver(task):
            raise McpError(
                "Provider mutations require a non-coder approval marker (approvedBy) on the "
                "task. A verifier or human must approve the destructive action before a coder "
                "can rotate secrets or terminate compute."
            )
        return task


def matching_active_claim(
    state: dict[str, Any],
    agent_id: str,
    role: str,
    project_id: str,
    task_id: str,
) -> dict[str, Any] | None:
    claim = active_claim_for_task(state, project_id, task_id)
    if (
        claim
        and claim.get("agentId") == agent_id
        and normalize_role(claim.get("role", "")) == role
    ):
        return claim
    return None


def require_provider_mutation_role(role: str) -> None:
    if normalize_role(role) != "coder":
        raise McpError("Only coder agents can mutate Cloudflare or Scaleway. Verifiers are read-only.")


def provider_mutation_project_context(args: dict[str, Any]) -> tuple[str, str, str]:
    project_id = args.get("management_project_id") or args.get("aspis_project_id")
    if not project_id:
        raise McpError("Provider mutations require management_project_id and task_id so the Kanban can audit the action.")
    task_id = normalize_task_id(args.get("task_id", ""))
    evidence = str(args.get("evidence") or "").strip()
    if len(evidence) < 12:
        raise McpError("Provider mutations require concrete evidence.")
    return normalize_project_id(project_id), task_id, evidence[:2000]


def require_provider_mutation_context(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
    tool_name: str,
) -> tuple[str, str, str, str, str]:
    agent_id, role = require_agent_tool(projects_dir, args, tool_name)
    require_provider_mutation_role(role)
    project_id, task_id, evidence = provider_mutation_project_context(args)
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        require_claim_for_status_update(state, agent_id, role, project_id, task_id)
        require_live_task_for_provider_mutation(projects_dir, project_id, task_id)
    return agent_id, role, project_id, task_id, evidence


def reserve_provider_mutation(
    projects_dir: Path,
    state_lock: Path,
    agent_id: str,
    role: str,
    project_id: str,
    task_id: str,
    tool_name: str,
    evidence: str,
) -> None:
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        require_claim_for_status_update(state, agent_id, role, project_id, task_id)
        require_live_task_for_provider_mutation(projects_dir, project_id, task_id)
        claim = matching_active_claim(state, agent_id, role, project_id, task_id)
        if claim is not None:
            claim["status"] = "provider_action_pending"
            claim["evidence"] = evidence
            claim["updatedAt"] = now()
        upsert_session(
            state,
            agent_id,
            role,
            status="provider_action_pending",
            message=f"{tool_name} pending.",
            project_id=project_id,
            task_id=task_id,
        )
        add_event(state, agent_id, role, "provider_action_pending", f"{tool_name} authorized.", project_id, task_id, evidence=evidence)
        write_agents_state(projects_dir, state)


def release_provider_mutation_reservation(
    projects_dir: Path,
    state_lock: Path,
    agent_id: str,
    role: str,
    project_id: str,
    task_id: str,
    tool_name: str,
    reason: str,
) -> None:
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        claim = matching_active_claim(state, agent_id, role, project_id, task_id)
        if claim is not None and claim.get("status") == "provider_action_pending":
            claim["status"] = "wip"
            claim["updatedAt"] = now()
        upsert_session(
            state,
            agent_id,
            role,
            status="wip",
            message=f"{tool_name} failed: {reason}",
            project_id=project_id,
            task_id=task_id,
        )
        add_event(state, agent_id, role, "provider_action_failed", f"{tool_name} failed: {reason}", project_id, task_id)
        write_agents_state(projects_dir, state)


def record_provider_mutation(
    projects_dir: Path,
    state_lock: Path,
    agent_id: str,
    role: str,
    project_id: str,
    task_id: str,
    event_type: str,
    message: str,
    evidence: str,
) -> None:
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        require_claim_for_status_update(state, agent_id, role, project_id, task_id)
        with file_lock(project_lock_path(projects_dir, project_id)):
            project = read_project_file(project_path(projects_dir, project_id))
            if not any(item.get("id") == task_id for item in project["state"].get("tasks", [])):
                raise McpError("Task not found.")
            project["state"].setdefault("notes", []).append(
                {
                    "id": note_id(),
                    "text": f"{message} Evidence: {evidence}",
                    "source": f"agent:{agent_id}",
                    "createdAt": now(),
                }
            )
            project["metadata"]["updatedAt"] = now()
            write_project_file(project)
        claim = matching_active_claim(state, agent_id, role, project_id, task_id)
        if claim is not None and claim.get("status") == "provider_action_pending":
            claim["status"] = "wip"
            claim["updatedAt"] = now()
        upsert_session(
            state,
            agent_id,
            role,
            status=event_type,
            message=message,
            project_id=project_id,
            task_id=task_id,
        )
        add_event(state, agent_id, role, event_type, message, project_id, task_id, evidence=evidence)
        write_agents_state(projects_dir, state)


def require_agent_tool(
    projects_dir: Path,
    args: dict[str, Any],
    tool_name: str,
) -> tuple[str, str]:
    agent_id = normalize_agent_id(args.get("agent_id"))
    role = require_registered_role(projects_dir, agent_id, args.get("role", ""), tool_name, args.get("session_token"))
    return agent_id, role


def audit_agent_read(
    projects_dir: Path,
    state_lock: Path,
    agent_id: str,
    role: str,
    event_type: str,
    message: str,
    project_id: str | None = None,
    task_id: str | None = None,
) -> None:
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        upsert_session(state, agent_id, role, status=event_type, message=message, project_id=project_id, task_id=task_id)
        add_event(state, agent_id, role, event_type, message, project_id, task_id)
        write_agents_state(projects_dir, state)


def mcp_debug(projects_dir: Path, message: str) -> None:
    if os.getenv("ASPIS_MCP_DEBUG", "").strip() != "1":
        return
    try:
        log_dir = management_root_from_projects_dir(projects_dir) / "oracle-data" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        with (log_dir / "mcp-debug.log").open("a", encoding="utf-8") as handle:
            handle.write(f"{now()} {message}\n")
    except Exception:
        pass


def app_vault_target(account: str) -> str:
    return f"{account}.{APP_VAULT_SERVICE}"


def read_windows_credential_password(target_name: str) -> str | None:
    if os.name != "nt":
        return None
    import ctypes
    from ctypes import wintypes

    CRED_TYPE_GENERIC = 1

    class FILETIME(ctypes.Structure):
        _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]

    class CREDENTIALW(ctypes.Structure):
        _fields_ = [
            ("Flags", wintypes.DWORD),
            ("Type", wintypes.DWORD),
            ("TargetName", wintypes.LPWSTR),
            ("Comment", wintypes.LPWSTR),
            ("LastWritten", FILETIME),
            ("CredentialBlobSize", wintypes.DWORD),
            ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
            ("Persist", wintypes.DWORD),
            ("AttributeCount", wintypes.DWORD),
            ("Attributes", ctypes.c_void_p),
            ("TargetAlias", wintypes.LPWSTR),
            ("UserName", wintypes.LPWSTR),
        ]

    advapi = ctypes.windll.advapi32
    advapi.CredReadW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(ctypes.POINTER(CREDENTIALW)),
    ]
    advapi.CredReadW.restype = wintypes.BOOL
    advapi.CredFree.argtypes = [ctypes.c_void_p]
    advapi.CredFree.restype = None

    credential = ctypes.POINTER(CREDENTIALW)()
    if not advapi.CredReadW(target_name, CRED_TYPE_GENERIC, 0, ctypes.byref(credential)):
        return None
    try:
        blob_size = int(credential.contents.CredentialBlobSize)
        if blob_size <= 0:
            return ""
        raw = ctypes.string_at(credential.contents.CredentialBlob, blob_size)
        return raw.decode("utf-16-le").rstrip("\x00")
    finally:
        advapi.CredFree(credential)


def read_macos_keychain_password(service: str, account: str) -> str | None:
    if sys.platform != "darwin":
        return None
    try:
        result = subprocess.run(
            ["security", "find-generic-password", "-s", service, "-a", account, "-w"],
            capture_output=True, text=True, timeout=5,
        )
    except Exception:
        return None
    if result.returncode != 0:
        return None
    # `-w` prints the secret as text (trailing newline). The Rust keyring stores
    # UTF-8 token/key strings, so no hex-decoding is needed.
    return result.stdout.rstrip("\n") or None


def app_vault_secret(key: str) -> str | None:
    account = APP_VAULT_ACCOUNTS.get(key)
    if not account:
        return None
    return app_vault_account_secret(account)


def app_vault_account_secret(account: str) -> str | None:
    if os.environ.get("ASPIS_MCP_DISABLE_APP_VAULT") == "1":
        return None
    try:
        if sys.platform == "darwin":
            value = read_macos_keychain_password(APP_VAULT_SERVICE, account)
        else:
            value = read_windows_credential_password(app_vault_target(account))
    except Exception:
        return None
    if value and value.strip():
        return value.strip()
    return None


def secret_from_app_vault_or_env(vault_key: str, *env_names: str) -> str | None:
    if vault_key == "cloudflare_token" and cloudflare_profile_mode():
        return optional_env(*env_names)
    value = app_vault_secret(vault_key)
    if value:
        return value
    return optional_env(*env_names)


def cloudflare_profile_mode() -> bool:
    return os.environ.get("ASPIS_MCP_CLOUDFLARE_PROFILE_MODE", "").strip() == "1"


def credential_status_for_env_only(account: str, *env_names: str) -> dict[str, Any]:
    source = "missing"
    for env_name in env_names:
        if os.environ.get(env_name, "").strip():
            source = f"env:{env_name}"
            break
    return {
        "configured": source != "missing",
        "source": source,
        "target": app_vault_target(account),
        "envNames": list(env_names),
    }


def credential_status_for_account(account: str, *env_names: str) -> dict[str, Any]:
    vault_configured = bool(app_vault_account_secret(account))
    source = "app_vault" if vault_configured else "missing"
    if not vault_configured:
        for env_name in env_names:
            if os.environ.get(env_name, "").strip():
                source = f"env:{env_name}"
                break
    return {
        "configured": source != "missing",
        "source": source,
        "target": app_vault_target(account),
        "envNames": list(env_names),
    }


def credential_status_for_key(vault_key: str, *env_names: str) -> dict[str, Any]:
    account = APP_VAULT_ACCOUNTS[vault_key]
    if vault_key == "cloudflare_token" and cloudflare_profile_mode():
        return credential_status_for_env_only(account, *env_names)
    return credential_status_for_account(account, *env_names)


def provider_token(*names: str) -> str:
    for name in names:
        value = os.environ.get(name)
        if value and value.strip():
            return value.strip()
    raise McpError(f"Missing provider token env var: {', '.join(names)}.")


def provider_token_from_sources(vault_key: str, *env_names: str) -> str:
    value = secret_from_app_vault_or_env(vault_key, *env_names)
    if value:
        return value
    raise McpError(
        "Missing provider token. Save it in Aspis Management > Secrets, or set env var: "
        + ", ".join(env_names)
    )


def cloudflare_token_from_sources(*env_names: str) -> str:
    names = env_names or CF_TOKEN_ENVS
    return provider_token_from_sources("cloudflare_token", *names)


def read_oracle_llm_settings_from_app_vault() -> dict[str, Any] | None:
    raw = app_vault_account_secret("oracle:llm_settings")
    if not raw:
        return None
    try:
        settings = json.loads(raw)
    except json.JSONDecodeError:
        return None
    return settings if isinstance(settings, dict) else None


def oracle_llm_setting(settings: dict[str, Any], snake: str, camel: str, default: Any = None) -> Any:
    if snake in settings:
        return settings.get(snake)
    if camel in settings:
        return settings.get(camel)
    return default


def bool_setting(value: Any, default: bool = False) -> bool:
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() in {"1", "true", "yes", "on"}


def oracle_llm_key_scope(settings: dict[str, Any]) -> str:
    provider = str(oracle_llm_setting(settings, "provider", "provider", "") or "").strip().lower()
    base_url = str(oracle_llm_setting(settings, "base_url", "baseUrl", "") or "").strip().lower()
    return hashlib.sha256(f"{provider}\n{base_url}".encode("utf-8")).hexdigest()


def oracle_llm_api_key_for_settings(settings: dict[str, Any]) -> str | None:
    scoped = app_vault_account_secret(f"oracle:llm_api_key:{oracle_llm_key_scope(settings)}")
    if scoped:
        return scoped
    provider = str(oracle_llm_setting(settings, "provider", "provider", "") or "").strip().lower()
    if provider == "scaleway":
        return secret_from_app_vault_or_env(
            "scaleway_ai_token",
            *SCW_AI_TOKEN_ENVS,
        )
    if provider == "infomaniak":
        return secret_from_app_vault_or_env(
            "infomaniak_token",
            *INFOMANIAK_TOKEN_ENVS,
        )
    if provider == "mistral":
        return secret_from_app_vault_or_env(
            "mistral_token",
            *MISTRAL_TOKEN_ENVS,
        )
    legacy = app_vault_account_secret("oracle:llm_api_key")
    return legacy


def oracle_llm_api_key_status_for_settings(settings: dict[str, Any]) -> dict[str, Any]:
    provider = str(oracle_llm_setting(settings, "provider", "provider", "") or "").strip().lower()
    scoped_account = f"oracle:llm_api_key:{oracle_llm_key_scope(settings)}"
    scoped = credential_status_for_account(scoped_account)
    if scoped["configured"]:
        scoped["provider"] = provider
        return scoped
    if provider == "scaleway":
        status = credential_status_for_key("scaleway_ai_token", *SCW_AI_TOKEN_ENVS)
    elif provider == "infomaniak":
        status = credential_status_for_key("infomaniak_token", *INFOMANIAK_TOKEN_ENVS)
    elif provider == "mistral":
        status = credential_status_for_key("mistral_token", *MISTRAL_TOKEN_ENVS)
    else:
        status = credential_status_for_account("oracle:llm_api_key")
    status["provider"] = provider
    status["scopedTarget"] = app_vault_target(scoped_account)
    return status


def provider_credentials_status() -> dict[str, Any]:
    settings = read_oracle_llm_settings_from_app_vault()
    llm: dict[str, Any] = {
        "settingsConfigured": settings is not None,
        "settingsTarget": app_vault_target("oracle:llm_settings"),
    }
    if settings:
        provider = str(oracle_llm_setting(settings, "provider", "provider", "ollama") or "ollama").strip().lower()
        model = str(oracle_llm_setting(settings, "model", "model", "qwen3.5:4b") or "qwen3.5:4b").strip()
        remote_enabled = bool_setting(oracle_llm_setting(settings, "remote_enabled", "remoteEnabled"), False)
        runtime_provider = provider if remote_enabled and provider != "ollama" else "ollama"
        primary_settings = {
            "provider": provider,
            "base_url": oracle_llm_setting(settings, "base_url", "baseUrl"),
        }
        llm.update(
            {
                "provider": runtime_provider,
                "configuredProvider": provider,
                "model": model,
                "baseUrl": oracle_llm_setting(settings, "base_url", "baseUrl"),
                "remoteEnabled": remote_enabled,
                "credential": {"configured": runtime_provider == "ollama", "source": "local" if runtime_provider == "ollama" else "missing"},
            }
        )
        if runtime_provider != "ollama":
            llm["credential"] = oracle_llm_api_key_status_for_settings(primary_settings)
    return {
        "providers": {
            "cloudflare": {
                "targetName": CF_TARGET_ACCOUNT_NAME,
                "token": credential_status_for_key(
                    "cloudflare_token",
                    *CF_TOKEN_ENVS,
                    *CF_READONLY_TOKEN_ENVS,
                    *CF_CODER_TOKEN_ENVS,
                    *CF_SECRET_ROTATOR_TOKEN_ENVS,
                ),
                "accountId": credential_status_for_key("cloudflare_account_id", *CF_ACCOUNT_ENVS),
                "agentProfiles": {
                    "verifierReadonly": credential_status_for_account(
                        "provider:cloudflare_agent_profile:verifier-readonly",
                        *CF_READONLY_TOKEN_ENVS,
                    ),
                    "coderWorkerWrite": credential_status_for_account(
                        "provider:cloudflare_agent_profile:coder-worker-write",
                        *CF_CODER_TOKEN_ENVS,
                    ),
                    "secretsRotator": credential_status_for_account(
                        "provider:cloudflare_agent_profile:secrets-rotator",
                        *CF_SECRET_ROTATOR_TOKEN_ENVS,
                    ),
                },
            },
            "scaleway": {
                "targetProjectName": SCW_TARGET_PROJECT_NAME,
                "token": credential_status_for_key("scaleway_token", *SCW_TOKEN_ENVS),
                "projectId": credential_status_for_key("scaleway_project_id", *SCW_PROJECT_ENVS),
                "objectAccessKey": credential_status_for_key(
                    "scaleway_object_access_key",
                    *SCW_OBJECT_ACCESS_KEY_ENVS,
                ),
                "objectSecretKey": credential_status_for_key(
                    "scaleway_object_secret_key",
                    *SCW_OBJECT_SECRET_KEY_ENVS,
                ),
            },
            # GitHub stays on its own bespoke vault account ("provider:github",
            # written by the Rust github.rs path), NOT a ProviderId. Report
            # status only — configured/missing + the vault target — never the
            # token value. No env fallback: the app-vault keyring is the only
            # source for the GitHub app token.
            "github": credential_status_for_account("provider:github"),
        },
        "oracleLlm": llm,
    }


def oracle_llm_config_from_app_vault() -> dict[str, Any] | None:
    settings = read_oracle_llm_settings_from_app_vault()
    if not settings:
        return None
    provider = str(oracle_llm_setting(settings, "provider", "provider", "ollama") or "ollama").strip().lower()
    model = str(oracle_llm_setting(settings, "model", "model", "qwen3.5:4b") or "qwen3.5:4b").strip()
    remote_enabled = bool_setting(oracle_llm_setting(settings, "remote_enabled", "remoteEnabled"), False)
    runtime_provider = provider if remote_enabled and provider != "ollama" else "ollama"
    runtime_settings = {
        "provider": runtime_provider,
        "model": model,
        "base_url": oracle_llm_setting(settings, "base_url", "baseUrl"),
    }
    if runtime_provider != "ollama":
        runtime_settings["api_key"] = oracle_llm_api_key_for_settings(
            {
                "provider": provider,
                "base_url": oracle_llm_setting(settings, "base_url", "baseUrl"),
            }
        )
    return runtime_settings


def mcp_oracle_context(
    engine: Any,
    query: str,
    limit: int,
    allowed_file_ids: set[str] | None = None,
) -> list[dict]:
    if os.getenv("ASPIS_MCP_DENSE_CONTEXT", "").strip() == "1":
        return engine.context(query, limit, allowed_file_ids=allowed_file_ids)
    chunks = engine.sqlite.all_chunks()
    if allowed_file_ids is not None:
        chunks = [chunk for chunk in chunks if chunk["file_id"] in allowed_file_ids]
    return lexical_chunk_context(query, chunks, max(1, limit))


def mcp_oracle_ask(
    engine: Any,
    query: str,
    limit: int,
    allowed_file_ids: set[str] | None = None,
) -> dict:
    if os.getenv("ASPIS_MCP_DENSE_ASK", "").strip() == "1":
        return engine.ask(
            query,
            limit,
            llm_config=oracle_llm_config_from_app_vault(),
            allowed_file_ids=allowed_file_ids,
        )
    chunks = mcp_oracle_context(engine, query, max(1, limit), allowed_file_ids=allowed_file_ids)
    generated = answer_from_context(query, chunks, llm_config=oracle_llm_config_from_app_vault())
    return {
        "mode": "oracle-mcp-bounded",
        "query": query,
        "summary": generated["answer"],
        "answer": generated["answer"],
        "citations": generated["citations"],
        "not_found": generated["not_found"],
        "suggested_path": generated["suggested_path"],
        "answer_source": generated.get("answer_source"),
        "fallback_reason": generated.get("fallback_reason"),
        "llm_provider": generated.get("llm_provider"),
        "llm_model": generated.get("llm_model"),
        "results": [mcp_chunk_result(chunk) for chunk in chunks[: max(1, limit)]],
    }


def _http_readiness_placeholder() -> dict[str, Any]:
    # FIX 4: on the HTTP path the resident server owns the (possibly
    # different-root) index and is only published once ready; the LOCAL gate
    # would falsely report "not ready". Trust the server; return a benign
    # status the call site can surface without the local manifest/SQLite.
    return {
        "root": None,
        "indexed_files": None,
        "pending_files": None,
        "stale_files": None,
        "source": "resident-server",
    }


def _safe_index_root(value: Any) -> str | None:
    """Reduce any index-status `root` to a basename before it reaches an agent.

    PRIVACY (FIX 1b, defense-in-depth): `ensure_oracle_index_ready` already
    stores a basename, but the oracle_ask/oracle_context responses forward
    whatever the status dict carries (including the HTTP placeholder, or a
    future/patched producer). An absolute path here would leak the OS username
    + machine layout, so we collapse anything path-shaped to its final
    component and pass through None unchanged.
    """
    if not value:
        return None
    return Path(str(value)).name


def _require_concrete_scope(allowed_file_ids: set[str] | None) -> None:
    """FIX 6 + FIX 3: a bounded Oracle query requires a concrete (non-None) scope
    set on BOTH the HTTP and the in-process path.

    On HTTP, `_scope_payload` maps None -> empty list (no docs); the in-process
    `mcp_oracle_context` maps None -> FULL corpus. Either way a None scope is a
    scope-escalation bug: over HTTP it silently diverges from in-process, and
    in-process it would WIDEN to the entire corpus (reading other projects).
    The callers always pass a concrete set (oracle_allowed_file_ids never
    returns None today), so a None is a bug; we fail closed by raising in both
    dispatch paths rather than reading the wrong / full scope.
    """
    if allowed_file_ids is None:
        raise McpError("Oracle bounded scope must be a concrete set, not None.")


def dispatch_oracle_context(
    projects_dir: Path,
    query: str,
    limit: int,
    allowed_file_ids: set[str] | None,
    args: dict[str, Any] | None = None,
) -> tuple[list[dict], dict[str, Any]]:
    """Return (context chunks, index_status) via the resident HTTP server.

    FIX 4: readiness is owned here. When an HTTP target resolves, retrieval is
    routed through the HTTP engine (single embedder) with the SAME locally
    computed `allowed_file_ids` scope and the LOCAL readiness gate is SKIPPED
    (the resident server is authoritative). On any HTTP failure we log
    (redacted) and fall back to the in-process engine — and ONLY then run the
    fail-closed LOCAL `ensure_oracle_index_ready` gate, so an empty in-process
    index still surfaces the actionable not-ready error. With no target the
    in-process path runs the gate too. The in-process engine (and its lazy
    embedder) is only built on the fallback / no-target path.
    """
    args = args or {}
    # FIX 3 + FIX 6: a None scope is fail-closed on BOTH paths. Over HTTP it
    # would send an empty scope; in-process `mcp_oracle_context` maps None to the
    # FULL corpus (scope escalation). Guarding here, before the path split,
    # makes both paths reject a missing scope identically.
    _require_concrete_scope(allowed_file_ids)
    target = resolve_oracle_http_target(projects_dir)
    if target is not None:
        base_url, token = target
        try:
            chunks = HttpOracleEngine(base_url, token).context(
                query, limit, allowed_file_ids=allowed_file_ids
            )
            return chunks, _http_readiness_placeholder()
        except OracleHttpError as exc:
            logger.warning("Oracle HTTP context failed, using in-process fallback: %s", exc)
    index_status = ensure_oracle_index_ready(projects_dir, args)
    engine = make_mcp_engine(projects_dir)
    return mcp_oracle_context(engine, query, limit, allowed_file_ids=allowed_file_ids), index_status


def dispatch_oracle_ask(
    projects_dir: Path,
    query: str,
    limit: int,
    allowed_file_ids: set[str] | None,
    args: dict[str, Any] | None = None,
) -> tuple[dict, dict[str, Any]]:
    """Return (answer dict, index_status). Same routing/fallback/readiness
    contract as `dispatch_oracle_context`.
    """
    args = args or {}
    # FIX 3 + FIX 6: fail-closed on a None scope before the path split (see
    # dispatch_oracle_context) — in-process None would widen to the full corpus.
    _require_concrete_scope(allowed_file_ids)
    target = resolve_oracle_http_target(projects_dir)
    if target is not None:
        base_url, token = target
        try:
            answer = HttpOracleEngine(base_url, token).ask(
                query, limit, allowed_file_ids=allowed_file_ids
            )
            return answer, _http_readiness_placeholder()
        except OracleHttpError as exc:
            logger.warning("Oracle HTTP ask failed, using in-process fallback: %s", exc)
    index_status = ensure_oracle_index_ready(projects_dir, args)
    engine = make_mcp_engine(projects_dir)
    return mcp_oracle_ask(engine, query, limit, allowed_file_ids=allowed_file_ids), index_status


def mcp_chunk_result(chunk: dict[str, Any]) -> dict[str, Any]:
    file_source = str(chunk.get("file_source") or "")
    return {
        "id": file_source or chunk.get("chunk_id"),
        "label": Path(file_source).name if file_source else str(chunk.get("chunk_id") or "chunk"),
        "node_type": "chunk",
        "cluster": 0,
        "score": float(chunk.get("score") or 0.0),
        "file_source": file_source,
        "function_primary": summarize_mcp_chunk(chunk),
        "dependencies": [],
        "chunk_id": chunk.get("chunk_id"),
        "chunk_index": chunk.get("chunk_index"),
        "start_char": chunk.get("start_char"),
        "end_char": chunk.get("end_char"),
        "chunk_preview": summarize_mcp_chunk(chunk),
    }


def summarize_mcp_chunk(chunk: dict[str, Any]) -> str:
    text = re.sub(r"\s+", " ", str(chunk.get("text") or "")).strip()
    return text[:420] if text else "Chunk-level match from the full-file Oracle index."


def optional_env(*names: str) -> str | None:
    for name in names:
        value = os.environ.get(name)
        if value and value.strip():
            return value.strip()
    return None


def import_httpx():
    try:
        import httpx
    except Exception as exc:  # pragma: no cover
        raise McpError("Install oracle/requirements.txt; provider MCP tools need httpx.") from exc
    return httpx


class OracleHttpError(Exception):
    """Raised inside the thin-client when an HTTP Oracle call fails.

    The handler catches it, logs (redacted) and falls back to the in-process
    engine for that single call. It is never surfaced to the agent.
    """


_LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1"}
# FIX 8: short TTL cache of the resolved (base_url, token) keyed on the
# projects_dir string, mirroring _MCP_INDEX_STATUS_CACHE. resolve_* is called on
# every oracle_ask/oracle_context, so a per-call stat+read+json-parse is wasteful.
_MCP_ORACLE_TARGET_CACHE: dict[str, tuple[float, tuple[str, str] | None]] = {}
_ORACLE_TARGET_TTL_SECONDS = 5.0


def _reset_oracle_target_cache() -> None:
    """Test/seam helper: clear the resolved-target TTL cache."""
    _MCP_ORACLE_TARGET_CACHE.clear()


def _read_oracle_discovery_file(projects_dir: Path) -> dict[str, Any] | None:
    """Read+parse the discovery JSON, or None on any missing/corrupt input.

    Isolated so the TTL cache can wrap a single, easily-patched reader.
    """
    try:
        discovery = Path(projects_dir) / ORACLE_DISCOVERY_FILENAME
        if not discovery.is_file():
            return None
        data = json.loads(discovery.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return None
    return data if isinstance(data, dict) else None


def _is_loopback_http_base(base: str) -> bool:
    """True only for an http(s) URL whose host is a loopback address.

    SECURITY (FIX 2): the resolved base_url carries the Oracle auth token and
    the agent's (project-scoped) corpus queries. A poisoned discovery file or an
    injected env var must NEVER be able to redirect those to a remote host. We
    require the scheme to be http/https and the hostname to be one of the
    loopback names, rejecting everything else.
    """
    from urllib.parse import urlparse

    try:
        parsed = urlparse(base)
    except (ValueError, TypeError):
        return False
    if parsed.scheme not in {"http", "https"}:
        return False
    hostname = (parsed.hostname or "").strip().lower()
    return hostname in _LOOPBACK_HOSTS


def resolve_oracle_http_target(projects_dir: Path) -> tuple[str, str] | None:
    """Resolve the resident HTTP Oracle server's (base_url, auth_token).

    Resolution order (Step 4a / 4b contract):
      1. Env override (app-launched agents): ASPIS_ORACLE_HTTP_BASE +
         ASPIS_ORACLE_AUTH_TOKEN. BOTH must be present, else this source is
         skipped.
      2. Discovery file `<projects_dir>/.oracle-server.json` written by the
         Rust supervisor: JSON with at least `baseUrl` + `authToken`. NOTE
         (Step 4b contract): the `authToken` published here is the AGENT token
         (bounded-only), NOT the operator token. The supervisor generates both
         tokens, sets both in the server env (ORACLE_AUTH_TOKEN +
         ORACLE_AGENT_AUTH_TOKEN), and publishes ONLY the agent token in this
         file (and injects it into app-launched agents via
         ASPIS_ORACLE_AUTH_TOKEN). So an agent holding this token can only reach
         the /*-bounded scoped endpoints, never the unscoped corpus.
      3. None -> caller uses the in-process engine (today's behavior).

    Defensive by design: a missing, corrupt, or partial file resolves to None
    (never raises), so a closed/restarting app degrades to in-process instead
    of erroring the agent. SECURITY (FIX 2): the resolved base_url MUST be a
    loopback http(s) URL or this returns None — the token+queries are never sent
    to a remote host. FIX 8: results are TTL-cached (~5s) per projects_dir.
    """
    cache_key = str(projects_dir)
    cached = _MCP_ORACLE_TARGET_CACHE.get(cache_key)
    if cached is not None and time.monotonic() - cached[0] < _ORACLE_TARGET_TTL_SECONDS:
        return cached[1]
    result = _resolve_oracle_http_target_uncached(projects_dir)
    _MCP_ORACLE_TARGET_CACHE[cache_key] = (time.monotonic(), result)
    return result


def _resolve_oracle_http_target_uncached(projects_dir: Path) -> tuple[str, str] | None:
    env_base = str(os.environ.get(ORACLE_HTTP_BASE_ENV) or "").strip()
    env_token = str(os.environ.get(ORACLE_HTTP_TOKEN_ENV) or "").strip()
    if env_base and env_token:
        if not _is_loopback_http_base(env_base):
            logger.warning("Oracle HTTP target env override is not loopback; ignoring.")
            return None
        return env_base, env_token

    data = _read_oracle_discovery_file(projects_dir)
    if data is None:
        return None
    base = str(data.get("baseUrl") or "").strip()
    token = str(data.get("authToken") or "").strip()
    if not base or not token:
        return None
    if not _is_loopback_http_base(base):
        logger.warning("Oracle discovery baseUrl is not loopback; ignoring.")
        return None
    return base, token


class HttpOracleEngine:
    """Thin-client engine: routes retrieval through the resident HTTP server.

    Exposes `context`/`ask` with the SAME signatures as the in-process
    QueryEngine so the MCP handler can use either interchangeably. The locally
    computed `allowed_file_ids` scope is forwarded verbatim; the server never
    widens it. An empty scope short-circuits to a grounded-empty result without
    a network call. Any transport/HTTP failure raises OracleHttpError so the
    handler can fall back to the in-process engine.

    PRIVACY: this client never logs the auth token or absolute paths.
    """

    def __init__(self, base_url: str, auth_token: str, timeout: float = 20.0):
        self._base_url = base_url.rstrip("/")
        self._auth_token = auth_token
        self._timeout = timeout

    def _headers(self) -> dict[str, str]:
        return {"x-oracle-auth-token": self._auth_token, "Content-Type": "application/json"}

    @staticmethod
    def _scope_payload(allowed_file_ids: set[str] | None) -> list[str]:
        # The thin-client always computes a concrete scope before calling, so a
        # None here would be a programming error; treat it as empty (no docs)
        # to fail closed rather than implicitly widening to the whole corpus.
        if not allowed_file_ids:
            return []
        return sorted(allowed_file_ids)

    def _post(self, path: str, payload: dict[str, Any]) -> dict[str, Any]:
        httpx = import_httpx()
        status_error = getattr(httpx, "HTTPStatusError", None)
        try:
            with httpx.Client(timeout=self._timeout) as client:
                response = client.post(self._base_url + path, headers=self._headers(), json=payload)
                response.raise_for_status()
                if not getattr(response, "content", b""):
                    return {}
                data = response.json()
        except Exception as exc:
            # FIX 5: a 4xx is a real config/auth/payload break (e.g. 401/403 bad
            # token, 422 bad payload). Silently falling back to the in-process
            # engine would HIDE it, so we SURFACE 4xx to the operator as McpError
            # (logged with the status code only — never token/url/abs path). A
            # 5xx, timeout, or connection error is transient/server-side, so we
            # keep the current behavior and fall back via OracleHttpError.
            if status_error is not None and isinstance(exc, status_error):
                status = getattr(getattr(exc, "response", None), "status_code", None)
                if isinstance(status, int) and 400 <= status < 500:
                    logger.warning("Oracle HTTP call returned client error HTTP %s", status)
                    raise McpError(
                        f"Oracle HTTP call failed with HTTP {status}. "
                        "Check the agent auth token / request (see server logs)."
                    ) from exc
            # Redact: only the endpoint PATH (not base_url/token/abs paths) is
            # safe to surface. base_url may embed a host; path is generic.
            raise OracleHttpError(f"Oracle HTTP call to {path} failed: {type(exc).__name__}") from exc
        # FIX 3: validate the decoded body BEFORE the caller indexes into it. A
        # non-dict (null/list/str) would otherwise crash `result["index_status"]`
        # at the call site; instead raise OracleHttpError so dispatch falls back.
        if not isinstance(data, dict):
            raise OracleHttpError(
                f"Oracle HTTP call to {path} returned a non-object body ({type(data).__name__})."
            )
        return data

    def context(self, query: str, limit: int = 8, allowed_file_ids: set[str] | None = None) -> list[dict]:
        scope = self._scope_payload(allowed_file_ids)
        if not scope:
            return []
        data = self._post(
            "/context-bounded",
            {"query": query, "limit": int(limit), "allowed_file_ids": scope},
        )
        # FIX 3: the server contract is a dict with a `chunks` list. A missing or
        # non-list `chunks` is a malformed response -> fall back in-process.
        chunks = data.get("chunks")
        if not isinstance(chunks, list):
            raise OracleHttpError("Oracle HTTP context response is missing a chunks list.")
        return list(chunks)

    def ask(
        self,
        query: str,
        limit: int = 5,
        llm_config: dict | None = None,
        allowed_file_ids: set[str] | None = None,
    ) -> dict:
        # llm_config is intentionally NOT forwarded: the resident server derives
        # provider config server-side only (see routes.server_side_llm_config),
        # so the thin-client cannot inject a provider/api_key. Mirrors /ask.
        scope = self._scope_payload(allowed_file_ids)
        if not scope:
            # FIX 7: mirror the EXACT key set of the in-process empty-scope
            # `mcp_oracle_ask` result so an agent sees an identical shape whether
            # the app is open (HTTP path) or closed (in-process). Safe defaults
            # for the LLM provenance fields (no answer was generated).
            return {
                "mode": "oracle-http-bounded",
                "query": query,
                "summary": "No Oracle documents are in scope for this request.",
                "answer": "No Oracle documents are in scope for this request.",
                "citations": [],
                "not_found": True,
                "suggested_path": None,
                "answer_source": None,
                "fallback_reason": None,
                "llm_provider": None,
                "llm_model": None,
                "results": [],
            }
        return self._post(
            "/ask-bounded",
            {"query": query, "limit": int(limit), "allowed_file_ids": scope},
        )


def sanitize_provider_error(message: str) -> str:
    text = str(message)
    text = re.sub(r"SCW[A-Za-z0-9]{8,}", "SCW[redacted]", text)
    text = re.sub(r"Bearer\s+[^\s,;]+", "Bearer [redacted]", text)
    text = re.sub(r"X-Auth-Token\s+[^\s,;]+", "X-Auth-Token [redacted]", text)
    return text


def raise_for_provider_status(response: Any, label: str) -> None:
    try:
        response.raise_for_status()
    except Exception as exc:
        status = getattr(response, "status_code", None)
        url = getattr(getattr(response, "request", None), "url", "")
        prefix = f"{label} rejected"
        if status:
            prefix = f"{prefix} with HTTP {status}"
        raise McpError(sanitize_provider_error(f"{prefix}: {url}")) from exc


def api_get(url: str, headers: dict[str, str], params: dict[str, Any] | None = None) -> dict[str, Any]:
    httpx = import_httpx()
    with httpx.Client(timeout=20.0) as client:
        response = client.get(url, headers=headers, params=params)
        raise_for_provider_status(response, "Provider GET")
        return response.json()


def api_post_json(url: str, headers: dict[str, str], payload: dict[str, Any] | None = None) -> dict[str, Any]:
    httpx = import_httpx()
    with httpx.Client(timeout=20.0) as client:
        response = client.post(url, headers=headers, json=payload or {})
        raise_for_provider_status(response, "Provider POST")
        if not response.content:
            return {}
        return response.json()


def api_put_json(url: str, headers: dict[str, str], payload: dict[str, Any]) -> dict[str, Any]:
    httpx = import_httpx()
    with httpx.Client(timeout=20.0) as client:
        response = client.put(url, headers=headers, json=payload)
        raise_for_provider_status(response, "Provider PUT")
        if not response.content:
            return {}
        return response.json()


def cf_headers(token: str) -> dict[str, str]:
    return {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}


def cf_result(envelope: dict[str, Any]) -> Any:
    if envelope.get("success") is False:
        raise McpError("Cloudflare API rejected the request.")
    return envelope.get("result")


def resolve_cloudflare_account(token: str, requested_account_id: str | None = None) -> dict[str, str]:
    headers = cf_headers(token)
    accounts = cf_result(api_get(f"{CF_API}/accounts", headers)) or []
    requested = requested_account_id or secret_from_app_vault_or_env(
        "cloudflare_account_id",
        *CF_ACCOUNT_ENVS,
    )
    if requested:
        for account in accounts:
            if account.get("id") == requested:
                name = str(account.get("name") or "")
                return {"id": account["id"], "name": name}
        raise McpError("Pinned Cloudflare Aspis Bio account was not visible to this token.")
    matches = [item for item in accounts if normalize_provider_name(str(item.get("name") or "")) == CF_TARGET_ACCOUNT_NAME]
    if len(matches) != 1:
        if len(accounts) == 1:
            account = accounts[0]
            return {"id": account["id"], "name": str(account.get("name") or "")}
        raise McpError("Cloudflare Aspis Bio account is ambiguous or missing. Set ASPIS_CLOUDFLARE_ACCOUNT_ID.")
    return {"id": matches[0]["id"], "name": matches[0].get("name") or CF_TARGET_ACCOUNT_NAME}


def cloudflare_worker_in_aspis_bio_scope(name: str, routes: list[Any]) -> bool:
    normalized_name = str(name or "").strip().lower()
    if normalized_name in CF_ASPIS_BIO_WORKERS or normalized_name.startswith("aspis-bio-"):
        return True
    for route in routes:
        if isinstance(route, dict) and "aspis-bio.com" in str(route.get("pattern") or "").lower():
            return True
    return False


def cloudflare_list_workers(token: str, account_id: str | None = None) -> dict[str, Any]:
    account = resolve_cloudflare_account(token, account_id)
    headers = cf_headers(token)
    workers = cf_result(api_get(f"{CF_API}/accounts/{account['id']}/workers/scripts", headers)) or []
    safe_workers = []
    hidden_sibling_workers = 0
    for worker in workers:
        name = worker.get("id") or worker.get("name")
        if not name:
            continue
        routes = worker.get("routes") or []
        tags = worker.get("tags") or []
        if not cloudflare_worker_in_aspis_bio_scope(name, routes):
            hidden_sibling_workers += 1
            continue
        safe_workers.append(
            {
                "id": name,
                "name": name,
                "createdOn": worker.get("created_on"),
                "modifiedOn": worker.get("modified_on"),
                "usageModel": worker.get("usage_model"),
                "routes": [
                    route.get("pattern")
                    for route in routes
                    if isinstance(route, dict) and route.get("pattern")
                ],
                "compatibilityDate": worker.get("compatibility_date"),
                "tags": tags if isinstance(tags, list) else [],
            }
        )
    return {"account": account, "workers": safe_workers, "hiddenSiblingWorkers": hidden_sibling_workers}


def cloudflare_rotate_secret(
    token: str,
    account_id: str | None,
    worker_name: str,
    secret_name: str,
    secret_value: str,
) -> dict[str, Any]:
    worker_name = clean_text(worker_name, "Worker name", 128)
    secret_name = clean_text(secret_name, "Secret name", 128)
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", secret_name):
        raise McpError("Cloudflare secret name must be a valid binding identifier.")
    secret_value = str(secret_value or "")
    if len(secret_value.strip()) < 8:
        raise McpError("Cloudflare secret value is too short.")
    inventory = cloudflare_list_workers(token, account_id)
    account = inventory["account"]
    if not any(worker["name"] == worker_name for worker in inventory["workers"]):
        raise McpError("Worker is not in the Aspis Bio Cloudflare inventory.")
    encoded_worker = worker_name.replace("/", "%2F")
    url = f"{CF_API}/accounts/{account['id']}/workers/scripts/{encoded_worker}/secrets"
    payload = {"name": secret_name, "text": secret_value, "type": "secret_text"}
    cf_result(api_put_json(url, cf_headers(token), payload))
    return {"account": account, "workerName": worker_name, "secretName": secret_name, "rotatedAt": now()}


def scw_headers(token: str) -> dict[str, str]:
    return {"X-Auth-Token": token, "Content-Type": "application/json"}


def resolve_scaleway_project(token: str, requested_project_id: str | None = None) -> dict[str, str]:
    headers = scw_headers(token)
    requested = requested_project_id or secret_from_app_vault_or_env(
        "scaleway_project_id",
        *SCW_PROJECT_ENVS,
    )
    try:
        projects = api_get(f"{SCW_API}/account/v3/projects", headers).get("projects", [])
    except Exception:
        if requested:
            access_key = secret_from_app_vault_or_env(
                "scaleway_object_access_key",
                *SCW_OBJECT_ACCESS_KEY_ENVS,
            )
            if access_key:
                info = api_get(f"{SCW_API}/iam/v1alpha1/api-keys/{access_key}", headers)
                if info.get("default_project_id") == requested:
                    return {"id": requested, "name": SCW_TARGET_PROJECT_NAME}
            raise
        raise
    if requested:
        for project in projects:
            if project.get("id") == requested:
                name = str(project.get("name") or "")
                if normalize_provider_name(name) != SCW_TARGET_PROJECT_NAME:
                    raise McpError("Pinned Scaleway project is visible, but it is not aspis-bio.")
                return {"id": project["id"], "name": name}
        raise McpError("Pinned Scaleway Aspis Bio project was not visible to this token.")
    matches = [item for item in projects if normalize_provider_name(str(item.get("name") or "")) == SCW_TARGET_PROJECT_NAME]
    if len(matches) != 1:
        raise McpError("Scaleway Aspis Bio project is ambiguous or missing. Set ASPIS_SCALEWAY_PROJECT_ID.")
    return {"id": matches[0]["id"], "name": matches[0].get("name") or SCW_TARGET_PROJECT_NAME}


def scaleway_list_resources(token: str, project_id: str | None = None) -> dict[str, Any]:
    project = resolve_scaleway_project(token, project_id)
    headers = scw_headers(token)
    resources: list[dict[str, Any]] = []
    for zone in SCW_ZONES:
        url = f"{SCW_API}/instance/v1/zones/{zone}/servers"
        try:
            payload = api_get(url, headers, {"project": project["id"], "page": 1, "per_page": 100})
        except Exception:
            continue
        for server in payload.get("servers", []):
            resources.append(
                {
                    "id": server.get("id"),
                    "name": server.get("name"),
                    "resourceType": "instance_server",
                    "region": zone,
                    "state": server.get("state"),
                    "commercialType": server.get("commercial_type"),
                    "projectId": project["id"],
                    "availableActions": ["start", "stop", "reboot", "delete", "terminate"],
                }
            )
    for region in SCW_REGIONS:
        try:
            namespaces = api_get(
                f"{SCW_API}/functions/v1beta1/regions/{region}/namespaces",
                headers,
                {"project_id": project["id"], "page": 1, "page_size": 100},
            ).get("namespaces", [])
        except Exception:
            namespaces = []
        for namespace in namespaces:
            try:
                payload = api_get(
                    f"{SCW_API}/functions/v1beta1/regions/{region}/functions",
                    headers,
                    {
                        "namespace_id": namespace.get("id"),
                        "project_id": project["id"],
                        "page": 1,
                        "page_size": 100,
                    },
                )
            except Exception:
                continue
            for item in payload.get("functions", []):
                resources.append(
                    {
                        "id": item.get("id"),
                        "name": item.get("name"),
                        "resourceType": "serverless_function",
                        "region": region,
                        "state": item.get("status") or item.get("state"),
                        "runtime": item.get("runtime"),
                        "projectId": project["id"],
                        "namespaceId": namespace.get("id"),
                        "availableActions": ["deploy"],
                    }
                )
        try:
            payload = api_get(
                f"{SCW_API}/containers/v1beta1/regions/{region}/containers",
                headers,
                {"project_id": project["id"], "page": 1, "page_size": 100},
            )
        except Exception:
            continue
        for item in payload.get("containers", []):
            resources.append(
                {
                    "id": item.get("id"),
                    "name": item.get("name"),
                    "resourceType": "serverless_container",
                    "region": region,
                    "state": item.get("status") or item.get("state"),
                    "runtime": item.get("runtime"),
                    "projectId": project["id"],
                    "availableActions": ["deploy"],
                }
            )
    # Each api_get below is independent: a failure (or a non-dict payload from an
    # unstable v1alpha endpoint) yields an empty list and we PROCEED to the sibling
    # call — `continue` here would skip the rest of the zone/region and silently
    # truncate the inventory (e.g. a volumes 5xx must not hide that zone's snapshots).
    def scw_items(url: str, params: dict[str, Any], envelope: str) -> list[dict[str, Any]]:
        try:
            payload = api_get(url, headers, params)
        except Exception:
            return []
        if not isinstance(payload, dict):
            return []
        items = payload.get(envelope, [])
        return [item for item in items if isinstance(item, dict)]

    for zone in SCW_ZONES:
        zone_params = {"project_id": project["id"], "page": 1, "per_page": 100}
        for item in scw_items(f"{SCW_API}/block/v1/zones/{zone}/volumes", zone_params, "volumes"):
            resources.append(
                {
                    "id": item.get("id"),
                    "name": item.get("name"),
                    "resourceType": "block_volume",
                    "region": zone,
                    "state": item.get("status") or item.get("state"),
                    "projectId": project["id"],
                    "availableActions": [],
                }
            )
        for item in scw_items(f"{SCW_API}/block/v1/zones/{zone}/snapshots", zone_params, "snapshots"):
            resources.append(
                {
                    "id": item.get("id"),
                    "name": item.get("name"),
                    "resourceType": "block_snapshot",
                    "region": zone,
                    "state": item.get("status") or item.get("state"),
                    "projectId": project["id"],
                    "availableActions": [],
                }
            )
    for region in SCW_REGIONS:
        region_params = {"project_id": project["id"], "page": 1, "page_size": 100}
        for item in scw_items(
            f"{SCW_API}/file/v1alpha1/regions/{region}/filesystems", region_params, "filesystems"
        ):
            resources.append(
                {
                    "id": item.get("id"),
                    "name": item.get("name"),
                    "resourceType": "file_system",
                    "region": region,
                    "state": item.get("status") or item.get("state"),
                    "projectId": project["id"],
                    "availableActions": [],
                }
            )
        for item in scw_items(
            f"{SCW_API}/serverless-sqldb/v1alpha1/regions/{region}/databases", region_params, "databases"
        ):
            resources.append(
                {
                    "id": item.get("id"),
                    "name": item.get("name"),
                    # DSN/endpoint deliberately NOT emitted — may carry credentials.
                    "resourceType": "serverless_sql_database",
                    "region": region,
                    "state": item.get("status") or item.get("state"),
                    "projectId": project["id"],
                    "availableActions": [],
                }
            )
    return {"project": project, "resources": [item for item in resources if item.get("id")]}


def scaleway_resource_action(
    token: str,
    resource_id: str,
    action: str,
    confirm_resource_name: str | None = None,
    project_id: str | None = None,
) -> dict[str, Any]:
    resource_id = clean_text(resource_id, "Resource id", 160)
    action = clean_text(action, "Action", 40).lower()
    inventory = scaleway_list_resources(token, project_id)
    resource = next((item for item in inventory["resources"] if item.get("id") == resource_id), None)
    if not resource:
        raise McpError("Scaleway resource is not in the Aspis Bio inventory.")
    action_aliases = {
        "start": "poweron",
        "stop": "poweroff",
        "reboot": "reboot",
        "delete": "terminate",
        "terminate": "terminate",
    }
    if action not in resource.get("availableActions", []):
        raise McpError("Scaleway action is not available for this resource type.")
    if action in {"delete", "terminate"} and confirm_resource_name != resource.get("name"):
        raise McpError("Scaleway terminate/delete requires exact resource-name confirmation.")
    headers = scw_headers(token)
    resource_type = resource["resourceType"]
    region = resource["region"]
    if resource_type == "instance_server":
        if action in {"delete", "terminate"}:
            delete_scaleway_instance_with_volumes(token, region, resource_id)
        else:
            api_action = action_aliases[action]
            url = f"{SCW_API}/instance/v1/zones/{region}/servers/{resource_id}/action"
            api_post_json(url, headers, {"action": api_action})
    elif resource_type == "serverless_function":
        url = f"{SCW_API}/functions/v1beta1/regions/{region}/functions/{resource_id}/deploy"
        api_post_json(url, headers, {})
    elif resource_type == "serverless_container":
        url = f"{SCW_API}/containers/v1beta1/regions/{region}/containers/{resource_id}/deploy"
        api_post_json(url, headers, {})
    else:
        raise McpError("Unsupported Scaleway resource type.")
    return {
        "project": inventory["project"],
        "resourceId": resource_id,
        "resourceName": resource.get("name"),
        "resourceType": resource_type,
        "action": action,
        "triggeredAt": now(),
    }


def delete_scaleway_instance_with_volumes(token: str, zone: str, server_id: str) -> None:
    httpx = import_httpx()
    headers = scw_headers(token)
    delete_url = f"{SCW_API}/instance/v1/zones/{zone}/servers/{server_id}"
    params = {"with_volumes": "all", "with_ip": "true", "force_shutdown": "true"}
    with httpx.Client(timeout=20.0) as client:
        volume_ids = scaleway_instance_attached_volume_ids(client, headers, zone, server_id)
        response = client.delete(delete_url, headers=headers, params=params)
        if response.is_success or response.status_code == 404:
            delete_scaleway_instance_volumes(client, headers, zone, volume_ids)
            return
        action_url = f"{SCW_API}/instance/v1/zones/{zone}/servers/{server_id}/action"
        terminate = client.post(action_url, headers=headers, json={"action": "terminate"})
        if terminate.is_success or terminate.status_code == 404:
            delete_scaleway_instance_volumes(client, headers, zone, volume_ids)
            return
        client.post(action_url, headers=headers, json={"action": "poweroff"})
        final_delete = client.delete(
            delete_url,
            headers=headers,
            params={"with_volumes": "all", "with_ip": "true"},
        )
        final_delete.raise_for_status()
        delete_scaleway_instance_volumes(client, headers, zone, volume_ids)


def scaleway_instance_attached_volume_ids(client: Any, headers: dict[str, str], zone: str, server_id: str) -> list[str]:
    try:
        response = client.get(f"{SCW_API}/instance/v1/zones/{zone}/servers/{server_id}", headers=headers)
        if not response.is_success:
            return []
        server = response.json().get("server") or {}
        volumes = server.get("volumes") or {}
        if isinstance(volumes, dict):
            return [
                str(volume.get("id") or "").strip()
                for volume in volumes.values()
                if isinstance(volume, dict) and str(volume.get("id") or "").strip()
            ]
    except Exception:
        return []
    return []


def delete_scaleway_instance_volumes(client: Any, headers: dict[str, str], zone: str, volume_ids: list[str]) -> None:
    for volume_id in volume_ids:
        safe_volume_id = clean_text(volume_id, "Volume id", 160)
        response = client.delete(f"{SCW_API}/instance/v1/zones/{zone}/volumes/{safe_volume_id}", headers=headers)
        if not response.is_success and response.status_code != 404:
            response.raise_for_status()


def load_project_locked(projects_dir: Path, project_id: str) -> dict[str, Any]:
    path = project_path(projects_dir, project_id)
    if not path.exists():
        raise McpError("Project not found.")
    with file_lock(path.with_suffix(path.suffix + ".lock")):
        if not path.exists():
            raise McpError("Project not found.")
        return read_project_file(path)


# ---------------------------------------------------------------------------
# Censor ledger access (the two-writer half of the Censor subsystem).
#
# The Censor engine lives in Rust (src-tauri/src/backend/censor/) and writes
# per-file shards at `<projectRoot>/.aspis-censor/<sha256(relPath)>.json`, each
# guarded by a `<shard>.json.lock` sidecar acquired with fs2. The Python MCP
# server is the SECOND writer: it lock-reads / lock-modifies the SAME shards
# through `file_lock` (msvcrt/fcntl) on that exact `.json.lock` path, mirroring
# how `agent_heartbeat` interoperates with the Rust-written `.aspis-agents.json`.
# The on-disk schema is a contract — camelCase keys identical to the Rust serde
# in `backend/censor/schema.rs`, so a shard written by either language round-
# trips through the other.
#
# PRIVACY: a finding's `title`/`body` are English summaries REDACTED at write
# time on the Rust side (gitleaks/semgrep secret literals never reach disk). The
# MCP tool still returns ONLY a fixed allowlist of safe fields and never adds raw
# content, so even a hand-edited shard cannot leak an unexpected key to an agent.
# ---------------------------------------------------------------------------

# The directory under a project root that holds the per-file shards. MUST match
# `CENSOR_DIR` in src-tauri/src/backend/censor/ledger.rs.
CENSOR_DIR = ".aspis-censor"

# The ONLY fields `censor_findings` returns to an agent. Anything else on the
# shard (now or added by a future build) is dropped — a strict allowlist, not a
# denylist, so a new field can never silently leak. Keys are camelCase, identical
# to the Rust serde so the same shard round-trips both languages.
CENSOR_SAFE_FINDING_FIELDS = (
    "id",
    "file",
    "line",
    "severity",
    "category",
    "source",
    "title",
    "body",
    "verdict",
    "disposition",
    "provenance",
)

# Provenance `action` verb per disposition — IDENTICAL to the Rust
# `disposition_action` in ledger.rs so the audit trail reads the same regardless
# of which writer set it. The accept-set below is DERIVED from these keys so the
# two can never drift (a new disposition added here is automatically accepted and
# given an action; an action-less disposition is impossible — N2).
CENSOR_DISPOSITION_ACTION = {
    "open": "reopen",
    "fixed": "fixed",
    "fp": "fp",
    "wontfix": "wontfix",
}

# Dispositions an agent may set via `censor_dispose`. SINGLE SOURCE OF TRUTH:
# derived from `CENSOR_DISPOSITION_ACTION.keys()` so the accept-set and the
# action map cannot diverge (a divergence would let an accepted disposition reach
# a `CENSOR_DISPOSITION_ACTION[disposition]` lookup that KeyErrors inside the
# shard lock). Matches the Rust `Disposition` enum tokens + `parse_disposition`.
# An unknown token is rejected (never silently defaulted — a typo must surface).
CENSOR_DISPOSITIONS = frozenset(CENSOR_DISPOSITION_ACTION.keys())

# Dispositions that represent a VERIFIER adjudication a coder must not silently
# undo (WARNING 2). A verifier may set/override anything; a coder may set these
# and reopen its OWN prior disposition, but may NOT override a finding a verifier
# has already disposed. `fixed` is excluded: a "fixed" claim is a coder-routine
# lifecycle step, not an adjudication verdict.
CENSOR_VERIFIER_ADJUDICATED = frozenset({"fp", "wontfix"})

# The maximum provenance entries kept on a finding. A repeated dispose loop (or a
# malicious agent hammering `censor_dispose`) must not grow a shard unbounded
# (shard bloat / DoS — BLOCKER 1). Oldest entries are dropped past this cap.
# MIRROR: `CENSOR_PROVENANCE_MAX` in src-tauri/src/backend/censor/ledger.rs.
CENSOR_PROVENANCE_MAX = 50


def _redact_secrets(text: str) -> str:
    """Replace secret-shaped tokens in a human-readable string with `[redacted]`.

    SECOND-LAYER DEFENSE (BLOCKER A): the Rust runners already redact a tool
    message before it is written into a shard's title/body, but this Python
    process is the egress boundary for `censor_findings`/`censor_finding`. A shard
    that was written by an OLDER Rust build (pre-redaction) or hand-edited could
    still carry a raw secret in `title`/`body`; redact here so it can never leave
    via the MCP surface.

    VERBATIM MIRROR of `redact_secrets` in
    src-tauri/src/backend/censor/runners/mod.rs — the heuristic MUST match so the
    two layers agree on what is/isn't a secret:
      - a token is the maximal run of chars in the secret alphabet
        (`A-Za-z0-9` plus `+ / = _ - .`);
      - a token <12 chars is never a secret;
      - an `AKIA`/`ASIA` + 16 base32 char token (len 20) is always a secret;
      - a "mostly separators" identifier (has a symbol, no digit, not mixed case)
        is prose-ish → kept;
      - otherwise a secret if it mixes classes (digits+letters, upper+lower, or a
        symbol alongside alphanumerics).
    Conservative: redact when in doubt; never raise.
    """
    REDACTED = "[redacted]"

    def is_token_char(c: str) -> bool:
        return c.isascii() and (c.isalnum() or c in "+/=_-.")

    def looks_secret(tok: str) -> bool:
        if len(tok) < 12:
            return False
        # AWS access key id: AKIA/ASIA + 16 uppercase base32 chars.
        if (
            (tok.startswith("AKIA") or tok.startswith("ASIA"))
            and len(tok) == 20
            and all(c.isascii() and (c.isupper() or c.isdigit()) for c in tok[4:])
        ):
            return True
        has_digit = any(c.isascii() and c.isdigit() for c in tok)
        has_upper = any(c.isascii() and c.isupper() for c in tok)
        has_lower = any(c.isascii() and c.islower() for c in tok)
        has_symbol = any(c in "+/=_-." for c in tok)
        mostly_separators = has_symbol and not has_digit and not (has_upper and has_lower)
        if mostly_separators:
            return False
        return (
            (has_digit and (has_upper or has_lower))
            or (has_upper and has_lower)
            or has_symbol
        )

    out: list[str] = []
    token: list[str] = []

    def flush() -> None:
        if not token:
            return
        tok = "".join(token)
        out.append(REDACTED if looks_secret(tok) else tok)
        token.clear()

    for c in str(text or ""):
        if is_token_char(c):
            token.append(c)
        else:
            flush()
            out.append(c)
    flush()
    return "".join(out)


def validate_censor_rel_path(rel: str) -> str:
    """Reject a relative path that could escape `.aspis-censor/` or break the
    hash-as-filename contract. Verbatim mirror of `validate_rel_path` in
    src-tauri/src/backend/censor/ledger.rs: no absolute paths (drive letters /
    UNC / leading slash), no `..` parent component, and no path component that
    starts with `-` (argv-injection guard, since the Rust runners hand the path
    to linters). Returns the original `rel` on success."""
    text = str(rel or "")
    if not text.strip():
        raise McpError("Censor file path is required.")
    # Absolute (POSIX leading slash, or Windows drive-letter / UNC root).
    if text.startswith("/") or text.startswith("\\"):
        raise McpError(f"Censor rel path must be relative, got absolute: {rel}")
    if len(text) >= 2 and text[1] == ":":
        raise McpError(f"Censor rel path must be relative, got absolute: {rel}")
    for component in re.split(r"[\\/]+", text):
        if component in ("", "."):
            continue
        if component == "..":
            raise McpError(f"Censor rel path must not contain '..': {rel}")
        if component.startswith("-"):
            raise McpError(f"Censor rel path component must not start with '-': {rel}")
    return text


def validate_plan_scope_path(rel: str) -> str:
    """Phase 11.5-B (Piece 1a): validate ONE `scope` entry (a file the plan task may
    MODIFY). Same path-safety class as the Rust planner's `check_rel_path` and the
    Censor rel-path guard: must be a non-empty RELATIVE repo path with no `..` parent
    escape and no component starting with `-` (argv-injection guard, since the runner
    hands the scope to tools). Returns the trimmed path on success."""
    text = str(rel or "").strip()
    if not text:
        raise McpError("Plan task scope path is required.")
    # Length cap matching the Rust planner's `check_rel_path` MAX_PATH_LEN (1024 chars):
    # a path Python accepts but the 11.5-B runner's `check_rel_path` would reject would
    # blow up the task at execution time instead of here, so reject it at creation.
    if len(text) > 1024:
        raise McpError(f"Plan scope path too long (max 1024 chars): got {len(text)}")
    if text.startswith("/") or text.startswith("\\"):
        raise McpError(f"Plan scope path must be relative, got absolute: {rel}")
    if len(text) >= 2 and text[1] == ":":
        raise McpError(f"Plan scope path must be relative, got absolute: {rel}")
    for component in re.split(r"[\\/]+", text):
        if component in ("", "."):
            continue
        if component == "..":
            raise McpError(f"Plan scope path must not contain '..': {rel}")
        if component.startswith("-"):
            raise McpError(f"Plan scope path component must not start with '-': {rel}")
    return text


def censor_dir(root: Path) -> Path:
    return root / CENSOR_DIR


def censor_shard_path(root: Path, file_rel_path: str) -> Path:
    """Shard path for one file: `<root>/.aspis-censor/<sha256(normalizedRel)>.json`.
    Backslashes are normalized to `/` BEFORE hashing so `a\\b.rs` (a Windows Rust
    writer) and `a/b.rs` (this Python writer) map to the SAME shard — verbatim
    mirror of `shard_path` in ledger.rs."""
    validate_censor_rel_path(file_rel_path)
    normalized = normalize_censor_rel_path(file_rel_path)
    name = hashlib.sha256(normalized.encode("utf-8")).hexdigest()
    return censor_dir(root) / f"{name}.json"


def normalize_censor_rel_path(file_rel_path: str) -> str:
    """Canonical byte-form of a rel path used for the shard HASH: backslashes to
    `/`, then consecutive slashes collapsed (`//` → `/`). So `src//a.rs`,
    `src/a.rs` and a Windows `src\\a.rs` all hash to the SAME shard (NITPICK 1).
    BYTE-IDENTICAL to `normalize_rel_path` in src-tauri/src/backend/censor/
    ledger.rs — the two writers MUST produce the same sha256 for one file."""
    collapsed = re.sub(r"/+", "/", str(file_rel_path).replace("\\", "/"))
    return collapsed


def censor_shard_lock_path(shard: Path) -> Path:
    """The `<shard>.json.lock` sidecar Rust's `lock_shard` uses. We lock the
    SAME path so the two writers are mutually exclusive on a shard."""
    return shard.with_suffix(shard.suffix + ".lock")


def resolve_project_work_root(projects_dir: Path, project_id: str) -> Path:
    """Resolve a project's configured working root (the tree Censor watches) from
    its Markdown `root_path`, reusing `load_project_locked` + the same
    `validate_project_work_root` allowlist every other root-resolving tool uses.
    A project without a configured root has no Censor ledger — that is an error,
    not an empty list, because the caller asked about a specific project."""
    project = load_project_locked(projects_dir, project_id)
    root_path = str(project["metadata"].get("rootPath") or "").strip()
    if not root_path:
        raise McpError("Project has no configured working root for Censor findings.")
    management_root = management_root_from_projects_dir(projects_dir).resolve()
    return validate_project_work_root(Path(root_path), management_root)


# --- Phase 11.2: project_structure (shared, read-only STRUCTURE graph) ---------------
#
# `project_structure` exposes the deterministic, no-LLM cross-file STRUCTURE graph + the
# architectural "spine" (the handful of files the rest of the code reaches into) so ANY
# coder (orchestrator / claude / codex / mini) can orient BEFORE asking the Oracle.
#
# REUSE (load-bearing): the graph is built by the Rust tree-sitter builder
# (`src-tauri/.../backend/structure.rs`), invoked via the app binary's headless
# `structure --root <path>` subcommand. There is NO second parser here — Python only
# resolves the project root (the SAME allowlist the censor/oracle tools use), shells out,
# parses the JSON, and returns a COMPACT result.

# Cache: (resolved_root_str, freshness_key) -> (built_at_monotonic, parsed_graph). Bounded
# in size so a long-lived server with many projects cannot grow it without limit.
#
# CONCURRENCY (FastMCP runs sync tools on a worker-thread pool, so N agents calling
# project_structure land on N threads concurrently). Three primitives guard the build:
#   1. `_STRUCTURE_CACHE_LOCK` serializes every read/write/eviction of the cache dict AND
#      the in-flight map below. It is held ONLY for the quick dict bookkeeping — NEVER
#      across the ~60s subprocess (the build runs outside the lock).
#   2. `_STRUCTURE_INFLIGHT` dedups concurrent builds for the SAME cache_key: the first
#      caller becomes the builder, later same-key callers WAIT on the shared Condition and
#      then re-read the cache instead of each spawning a subprocess. On builder error the
#      waiters are woken (no entry appears) so exactly one of them retries / they surface
#      the same failure — waiters never hang forever.
#   3. `_STRUCTURE_BUILD_SEMAPHORE` caps TOTAL concurrent Rust subprocess walks across ALL
#      keys, so N distinct projects cannot fan out into N concurrent walkers (DoS amp).
_STRUCTURE_CACHE: dict[tuple[str, str], tuple[float, dict[str, Any]]] = {}
_STRUCTURE_CACHE_MAX_ENTRIES = 64
_STRUCTURE_CACHE_LOCK = threading.Lock()
# cache_key -> Condition (shared lock = _STRUCTURE_CACHE_LOCK) signalling "a build for this
# key just finished (success or failure)". Presence of a key == "a build is in flight".
_STRUCTURE_INFLIGHT: dict[tuple[str, str], threading.Condition] = {}
# Max concurrent Rust structure-walk subprocesses, regardless of distinct cache keys. Small
# on purpose: a walk is bounded but CPU/IO-heavy, and a build storm (many agents, many
# projects) must not spawn an unbounded number of children.
_STRUCTURE_MAX_CONCURRENT_BUILDS = 4
_STRUCTURE_BUILD_SEMAPHORE = threading.BoundedSemaphore(_STRUCTURE_MAX_CONCURRENT_BUILDS)
# How long a caller will block trying to acquire a build slot before returning a clean
# "busy, try again" error. Bounded well under the subprocess timeout so a caller never
# stacks both waits; the in-flight dedup means only the FIRST same-key caller ever reaches
# this acquire, so contention here is across DISTINCT keys only.
_STRUCTURE_BUILD_SLOT_TIMEOUT_S = 15.0


def resolve_structure_bridge_binary() -> str:
    """The Aspis Management app binary that owns the headless `structure` subcommand,
    from `ASPIS_APP_BIN` (wired by the launch sites). Validated to be an existing,
    executable file so a stale/empty/non-executable env fails closed with a clear message
    instead of an opaque spawn error (e.g. EACCES). NEVER a bare command name or a guessed
    path — the tool degrades to an error result.
    SCOPE: this verifies the path is a runnable file, NOT its identity. Binary integrity
    (hash/signature/ownership) is an OS/install-level concern — the launch site wires
    `current_exe()` (the trusted running app), so identity is enforced upstream, not here."""
    raw = str(os.environ.get(ASPIS_APP_BIN_ENV) or "").strip()
    if not raw:
        raise McpError(
            "project_structure is unavailable: the app binary path is not configured "
            f"({ASPIS_APP_BIN_ENV} unset). Relaunch the agent from the app so the bridge "
            "is wired."
        )
    candidate = Path(raw)
    # Require BOTH "is a file" AND "is executable": a present-but-non-executable path would
    # otherwise pass here and fail later as an opaque spawn EACCES. Fail closed now with an
    # accurate message. (os.access checks the real uid/gid permission bits.)
    if not (candidate.is_file() and os.access(candidate, os.X_OK)):
        # PRIVACY: name only the basename, never echo the full path back to an agent.
        raise McpError(
            f"project_structure is unavailable: configured app binary '{candidate.name}' "
            "is not an executable file."
        )
    return str(candidate)


def _structure_freshness_key(root: Path) -> str:
    """A cheap freshness signal for `root`: the newest mtime + the count of files seen in
    a BOUNDED top-down walk (skipping the same build-artifact dirs the Rust builder skips
    + hidden dirs). Same tree ⇒ same key ⇒ cache hit; an edit bumps an mtime and a
    add/remove bumps the count, invalidating the entry. The TTL backstops a same-second
    edit the mtime resolution might miss. Never raises — on any error returns a sentinel
    that simply prevents caching (correctness over speed)."""
    skip_dirs = {"target", "node_modules", "dist", "build", "out", ".git"}
    newest_ns = 0
    count = 0
    try:
        for dirpath, dirnames, filenames in os.walk(root):
            # Prune build artifacts + hidden dirs in place (mirrors the Rust SKIP_DIRS +
            # the `ignore` crate's hidden filtering) so the probe stays cheap.
            dirnames[:] = [
                d for d in dirnames if d not in skip_dirs and not d.startswith(".")
            ]
            for name in filenames:
                count += 1
                if count > PROJECT_STRUCTURE_FRESHNESS_MAX_ENTRIES:
                    # Too large to sample cheaply; fold the cap into the key so we still
                    # cache deterministically (the TTL bounds staleness for huge trees).
                    return f"capped:{PROJECT_STRUCTURE_FRESHNESS_MAX_ENTRIES}"
                try:
                    st = os.stat(os.path.join(dirpath, name))
                except OSError:
                    continue
                if st.st_mtime_ns > newest_ns:
                    newest_ns = st.st_mtime_ns
    except OSError:
        # Cannot walk ⇒ a unique-ish key so we do NOT serve a stale cache entry.
        return f"nowalk:{time.time_ns()}"
    return f"{count}:{newest_ns}"


def _run_structure_bridge(app_bin: str, root: Path) -> dict[str, Any]:
    """Invoke `<app_bin> structure --root <root>` and parse its stdout JSON into the
    graph dict. Bounded by `PROJECT_STRUCTURE_TIMEOUT_S` (the child is killed on elapse).
    `PROJECT_STRUCTURE_MAX_OUTPUT_BYTES` is a POST-HOC reject — `capture_output=True`
    buffers all stdout first, so the cap rejects an over-large payload after the fact
    rather than streaming-bounding memory; it relies on the trusted Rust-side caps to keep
    the real output small (see the const's note). Raises `McpError` (never lets a
    subprocess exception escape) so the dispatcher returns a clean error result, never a
    crash."""
    try:
        proc = subprocess.run(
            [app_bin, "structure", "--root", str(root)],
            capture_output=True,
            timeout=PROJECT_STRUCTURE_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise McpError(
            f"project_structure timed out after {PROJECT_STRUCTURE_TIMEOUT_S:.0f}s "
            "building the graph."
        ) from exc
    except OSError as exc:
        raise McpError(f"project_structure could not run the structure bridge: {exc}") from exc

    if proc.returncode != 0:
        # The Rust bridge prints a one-line diagnostic to stderr on failure. Surface a
        # bounded slice of it (never the whole output) so the agent gets a usable reason.
        detail = (proc.stderr or b"").decode("utf-8", "replace").strip()
        detail = detail.splitlines()[0][:200] if detail else "no diagnostic"
        raise McpError(f"project_structure bridge failed (exit {proc.returncode}): {detail}")

    raw = proc.stdout or b""
    if len(raw) > PROJECT_STRUCTURE_MAX_OUTPUT_BYTES:
        raise McpError("project_structure graph output exceeded the size limit.")
    try:
        graph = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError) as exc:
        raise McpError("project_structure bridge returned unparseable JSON.") from exc
    if not isinstance(graph, dict):
        raise McpError("project_structure bridge returned a non-object graph.")
    return graph


def _structure_summary(graph: dict[str, Any]) -> dict[str, Any]:
    """The bounded summary counts from the StructureGraph (camelCase wire shape)."""
    return {
        "scanned": graph.get("scanned"),
        "skippedTooLarge": graph.get("skippedTooLarge"),
        "skippedUnsupported": graph.get("skippedUnsupported"),
        "skippedUnreadable": graph.get("skippedUnreadable"),
        "capped": bool(graph.get("capped")),
    }


def compact_structure(graph: dict[str, Any], full: bool) -> dict[str, Any]:
    """Project the full StructureGraph down to the payload a model can actually use:
    the `spine` (the 5-8 central files + their `topReferencedSymbols`) + the summary
    counts. The full `files`/edge list is HUGE for a big repo, so it is returned ONLY when
    `full=True` is explicitly requested. Unknown/missing fields degrade to None/[]."""
    spine = graph.get("spine")
    spine_out = spine if isinstance(spine, list) else []
    result: dict[str, Any] = {
        "spine": spine_out,
        "summary": _structure_summary(graph),
    }
    if full:
        files = graph.get("files")
        result["files"] = files if isinstance(files, list) else []
    return result


def _structure_cache_get(cache_key: tuple[str, str]) -> dict[str, Any] | None:
    """Return the cached FULL graph for `cache_key` if present AND within TTL, else None.
    MUST be called holding `_STRUCTURE_CACHE_LOCK`."""
    cached = _STRUCTURE_CACHE.get(cache_key)
    if cached is not None and (time.monotonic() - cached[0]) <= PROJECT_STRUCTURE_CACHE_TTL_S:
        return cached[1]
    return None


def _structure_cache_put(cache_key: tuple[str, str], graph: dict[str, Any]) -> None:
    """Insert `graph` under `cache_key`, evicting the oldest entry first if the cache is at
    its bound. MUST be called holding `_STRUCTURE_CACHE_LOCK` (so the read-min-pop sequence
    is atomic and can never race another writer into popping the wrong entry / a KeyError).
    """
    _STRUCTURE_CACHE.pop(cache_key, None)
    if len(_STRUCTURE_CACHE) >= _STRUCTURE_CACHE_MAX_ENTRIES:
        # Held under the lock, so the dict is stable here: a plain min over items() is safe
        # (no "changed size during iteration", no racy wrong-entry pop).
        oldest_key = min(_STRUCTURE_CACHE.items(), key=lambda kv: kv[1][0])[0]
        _STRUCTURE_CACHE.pop(oldest_key, None)
    _STRUCTURE_CACHE[cache_key] = (time.monotonic(), graph)


def build_project_structure(
    work_root: Path,
    full: bool,
    *,
    runner: Any = None,
) -> dict[str, Any]:
    """Resolve the bridge binary, build (or reuse a cached) StructureGraph for `work_root`,
    and return the compact payload. `runner` is a seam for tests (defaults to the real
    `_run_structure_bridge`) so the dispatch + cache logic is exercisable without a real
    binary. Cached per (root, freshness-key) with a TTL; a fresh edit invalidates the key,
    the TTL backstops a missed same-second edit. The cached value is the FULL parsed graph,
    so a later `full=True` call reuses the same build.

    CONCURRENCY (FastMCP worker-thread pool): the cache dict + the in-flight map are guarded
    by `_STRUCTURE_CACHE_LOCK` (held ONLY for quick bookkeeping, NEVER across the build);
    concurrent callers for the SAME cache_key dedup onto ONE builder via `_STRUCTURE_INFLIGHT`
    (the rest wait then read the cache); and the actual subprocess launch is gated by the
    global `_STRUCTURE_BUILD_SEMAPHORE` so total concurrent walkers are bounded regardless
    of distinct keys."""
    run = runner if runner is not None else _run_structure_bridge
    app_bin = resolve_structure_bridge_binary()
    root_str = str(work_root)
    freshness = _structure_freshness_key(work_root)
    cache_key = (root_str, freshness)

    # Decide builder-vs-waiter atomically under the lock, in a loop so that if the current
    # builder ERRORS (clears the in-flight marker without a cache entry), exactly the next
    # thread to win the lock becomes the new builder while the others keep waiting on
    # whatever Condition is current. `condition` is set only on the path where we become the
    # builder; in that case we break out to run the build OUTSIDE the lock.
    condition: threading.Condition
    while True:
        with _STRUCTURE_CACHE_LOCK:
            cached = _structure_cache_get(cache_key)
            if cached is not None:
                return compact_structure(cached, full)
            inflight = _STRUCTURE_INFLIGHT.get(cache_key)
            if inflight is None:
                # No build in flight for this key: WE become the builder. Publish an
                # in-flight Condition (sharing the cache lock) so concurrent same-key
                # callers dedup onto us, then leave the lock to run the build.
                condition = threading.Condition(_STRUCTURE_CACHE_LOCK)
                _STRUCTURE_INFLIGHT[cache_key] = condition
                break
            # A build for this key is already in flight: wait on the CURRENT Condition,
            # then loop to re-check (cache hit, or builder gone → we may become builder).
            inflight.wait(timeout=PROJECT_STRUCTURE_CACHE_TTL_S)

    # OUTSIDE the lock: gate the actual subprocess on the global semaphore, run it, then
    # re-acquire the lock only for the quick cache write + waking waiters. `finally` ALWAYS
    # clears the in-flight marker and wakes waiters — so a builder error never leaves
    # same-key callers hung, and never leaks a semaphore slot.
    acquired = _STRUCTURE_BUILD_SEMAPHORE.acquire(timeout=_STRUCTURE_BUILD_SLOT_TIMEOUT_S)
    if not acquired:
        # Could not get a build slot in time: drop the in-flight marker and wake any waiters
        # so one of them retries, then return a clean busy error (bounded, not a hang).
        with _STRUCTURE_CACHE_LOCK:
            _STRUCTURE_INFLIGHT.pop(cache_key, None)
            condition.notify_all()
        raise McpError(
            "project_structure is busy building other projects; try again in a moment."
        )
    try:
        graph = run(app_bin, work_root)
        with _STRUCTURE_CACHE_LOCK:
            _structure_cache_put(cache_key, graph)
        return compact_structure(graph, full)
    finally:
        _STRUCTURE_BUILD_SEMAPHORE.release()
        with _STRUCTURE_CACHE_LOCK:
            _STRUCTURE_INFLIGHT.pop(cache_key, None)
            condition.notify_all()


def dispatch_project_structure(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
    *,
    runner: Any = None,
) -> dict[str, Any]:
    """The `project_structure` tool. GATING: the caller is validated via
    `require_agent_tool` (registered, token-bearing, role-allowed); the project ROOT is
    resolved the SAME way every other root-scoped tool resolves it
    (`resolve_project_work_root` + the `validate_project_work_root` allowlist), so the tool
    can NEVER walk an arbitrary path. Returns the compact spine + summary (the full graph
    only when `full=True`). Fails SOFT: a bridge failure becomes a clean error result."""
    agent_id, role = require_agent_tool(projects_dir, args, "project_structure")
    if "project_structure" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use project_structure.")
    project_id = normalize_project_id(str(args.get("project_id") or "").strip())
    if not project_id:
        raise McpError("project_id is required.")
    # SEC: a mini is scoped to its spawning project exactly like oracle_context — reuse the
    # same guard so a mini cannot read a DIFFERENT project's structure.
    # TOCTOU NOTE: the scope check runs against `project_id` BEFORE the root is resolved
    # below, so in principle the project's configured root could change in between. This is
    # bounded — and benign — because `resolve_project_work_root` re-resolves through the
    # SAME `validate_project_work_root` allowlist every root-scoped tool uses, so even a
    # raced root can only ever be an already-allowed management-root path, never an
    # arbitrary tree. The scope check gates WHICH project; the allowlist gates WHICH paths.
    enforce_mini_oracle_project_scope(projects_dir, agent_id, role, args)
    full = bool(args.get("full"))
    work_root = resolve_project_work_root(projects_dir, project_id)

    payload = build_project_structure(work_root, full, runner=runner)
    # Audit the read (identity + project only — never path/contents), mirroring the other
    # read tools' privacy posture.
    audit_agent_read(
        projects_dir,
        state_lock,
        agent_id,
        role,
        "project_structure",
        f"Read project structure spine ({len(payload.get('spine') or [])} files).",
        project_id,
    )
    return {
        "projectId": project_id,
        "spine": payload.get("spine") or [],
        "summary": payload.get("summary") or {},
        **({"files": payload["files"]} if "files" in payload else {}),
    }


def _read_censor_shard(path: Path) -> dict[str, Any] | None:
    """Read one shard JSON. `None` for a genuinely-absent file; a present-but-
    corrupt shard is returned as `None` here for the LISTING path (best-effort,
    a single broken file must not blank the panel) — mirrors `list_shards`'
    skip-corrupt behavior. The dispose path uses `_read_censor_shard_strict`."""
    try:
        content = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except OSError:
        return None
    try:
        data = json.loads(content)
    except (ValueError, TypeError):
        return None
    return data if isinstance(data, dict) else None


def _read_censor_shard_strict(path: Path) -> dict[str, Any] | None:
    """Read one shard for the WRITE path. `None` only for a missing file; a
    present-but-corrupt shard RAISES (never silently overwrite unreadable prior
    dispositions/provenance) — mirrors `read_shard_at` returning Err on corrupt.
    The error carries the shard PATH only, never the contents."""
    try:
        content = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except OSError as exc:
        raise McpError(f"Could not read Censor shard: {path.name}: {exc}") from exc
    try:
        data = json.loads(content)
    except (ValueError, TypeError) as exc:
        raise McpError(f"Corrupt Censor shard (unparseable JSON): {path.name}") from exc
    if not isinstance(data, dict):
        raise McpError(f"Corrupt Censor shard (not an object): {path.name}")
    return data


def _safe_censor_finding(finding: dict[str, Any]) -> dict[str, Any]:
    """Project a stored finding down to the safe allowlist (strips any field that
    could carry raw content / a secret, and any unknown future field). Provenance
    entries are likewise reduced to `{actor, action, at}`. The `body` is the
    already-redacted stored summary — we never add raw tool output."""
    safe: dict[str, Any] = {}
    for key in CENSOR_SAFE_FINDING_FIELDS:
        if key == "provenance":
            entries = finding.get("provenance")
            safe["provenance"] = [
                {
                    "actor": str(entry.get("actor", "")),
                    "action": str(entry.get("action", "")),
                    # `role` is part of the safe allowlist: it is the coder/verifier
                    # token (no content), and the precedence rule (WARNING 2) needs
                    # it to survive round-trips. Absent on legacy entries → "".
                    "role": str(entry.get("role", "")),
                    "at": str(entry.get("at", "")),
                }
                for entry in entries
                if isinstance(entry, dict)
            ] if isinstance(entries, list) else []
        elif key in finding:
            value = finding[key]
            # SECOND-LAYER DEFENSE (BLOCKER A): the free-text fields are the only
            # ones that could carry a raw secret if a shard was written by an older
            # (pre-redaction) Rust build or hand-edited. Run them through the
            # Python redaction mirror before they egress via the MCP surface. All
            # other allowlisted fields are structured tokens (id/severity/...).
            if key in ("title", "body") and isinstance(value, str):
                safe[key] = _redact_secrets(value)
            else:
                safe[key] = value
    return safe


def read_censor_open_findings(root: Path, file_rel_path: str | None) -> list[dict[str, Any]]:
    """OPEN findings (disposition == "open") across the project's shards, each
    stripped to the safe allowlist. `file_rel_path` filters to one file's shard.
    A missing `.aspis-censor/` dir → empty list (project never reviewed). Each
    shard is lock-read on its `<shard>.json.lock` sidecar so a concurrent Rust
    review pass cannot tear a read."""
    shards: list[dict[str, Any]] = []
    if file_rel_path:
        shard_path = censor_shard_path(root, file_rel_path)
        with file_lock(censor_shard_lock_path(shard_path)):
            data = _read_censor_shard(shard_path)
        if data is not None:
            shards.append(data)
    else:
        directory = censor_dir(root)
        try:
            entries = sorted(directory.iterdir())
        except FileNotFoundError:
            return []
        except OSError:
            return []
        for entry in entries:
            # Only `.json` shards; skip `.lock`/`.tmp`/`.bak` sidecars.
            if entry.suffix != ".json" or not entry.is_file():
                continue
            with file_lock(censor_shard_lock_path(entry)):
                data = _read_censor_shard(entry)
            if data is not None:
                shards.append(data)

    open_findings: list[dict[str, Any]] = []
    for shard in shards:
        findings = shard.get("findings")
        if not isinstance(findings, list):
            continue
        for finding in findings:
            if not isinstance(finding, dict):
                continue
            # A finding missing `disposition` defaults to "open" (mirrors the Rust
            # `Disposition` default), so legacy/hand-edited shards still surface.
            disposition = str(finding.get("disposition", "open") or "open")
            if disposition == "open":
                open_findings.append(_safe_censor_finding(finding))
    return open_findings


def _last_provenance_role(provenance: list[Any]) -> str | None:
    """The `role` of the most recent provenance entry, or `None` if there are no
    entries / the entry carries no role. Used by the WARNING 2 precedence check to
    decide whether the CURRENT disposition was set by a verifier."""
    for entry in reversed(provenance):
        if isinstance(entry, dict):
            role = str(entry.get("role", "")).strip()
            return role or None
    return None


def _append_provenance(provenance: list[Any], entry: dict[str, Any]) -> list[Any]:
    """Append a provenance entry with two BLOCKER-1 guards (mirrored in Rust
    `push_provenance`):
      - DEDUP: if the last entry has the same `(actor, action)` as `entry`, do not
        append (an idempotent re-dispose must not grow the trail);
      - CAP: keep at most `CENSOR_PROVENANCE_MAX`, dropping the OLDEST first.
    The list is mutated in place and also returned for convenience."""
    if provenance:
        last = provenance[-1]
        if (
            isinstance(last, dict)
            and str(last.get("actor", "")) == str(entry.get("actor", ""))
            and str(last.get("action", "")) == str(entry.get("action", ""))
        ):
            return provenance
    provenance.append(entry)
    if len(provenance) > CENSOR_PROVENANCE_MAX:
        del provenance[: len(provenance) - CENSOR_PROVENANCE_MAX]
    return provenance


def dispose_censor_finding(
    root: Path,
    file_rel_path: str,
    finding_id: str,
    disposition: str,
    actor: str,
    stamp: str,
    role: str = "",
) -> dict[str, Any]:
    """Set a finding's `disposition` and APPEND a `{actor, action, role, at}`
    provenance entry, under the shard's `<shard>.json.lock` (same lock the Rust
    writer takes), then atomic-write. Matches the Rust `dispose_finding` semantics:
    locate by id, set disposition, append (never rewrite) provenance, stamp
    `updatedAt`.

    BLOCKER 1 — bounded provenance: an identical re-dispose (same actor+action as
    the last entry) does NOT append, and the trail is capped at
    `CENSOR_PROVENANCE_MAX` (oldest dropped), so a repeated/idempotent dispose
    cannot bloat the shard.

    WARNING 2 — role precedence: a `coder` may set fp/fixed/wontfix and may reopen
    its OWN prior disposition, but may NOT override a disposition a `verifier`
    already adjudicated (current disposition ∈ {fp, wontfix} set by a verifier).
    A `verifier` may dispose/override anything. The check uses the latest
    provenance entry's `role`; legacy entries without a role are treated as
    non-verifier (so old shards never lock a coder out).

    Returns the safe view of the disposed finding. Raises if the shard or id is
    absent (the caller passes the file)."""
    if disposition not in CENSOR_DISPOSITIONS:
        raise McpError(f"Unknown disposition: {disposition}")
    shard_path = censor_shard_path(root, file_rel_path)
    with file_lock(censor_shard_lock_path(shard_path)):
        shard = _read_censor_shard_strict(shard_path)
        if shard is None:
            raise McpError(f"No Censor shard for file: {file_rel_path}")
        findings = shard.get("findings")
        if not isinstance(findings, list):
            raise McpError(f"No Censor finding with id {finding_id} in {file_rel_path}")
        target = next(
            (f for f in findings if isinstance(f, dict) and f.get("id") == finding_id),
            None,
        )
        if target is None:
            raise McpError(f"No Censor finding with id {finding_id} in {file_rel_path}")

        provenance = target.get("provenance")
        if not isinstance(provenance, list):
            provenance = []

        # Canonical caller role WITHOUT raising: a known role maps to coder/verifier;
        # an empty/unknown role stays "" (NOT coerced to "coder"). The precedence
        # check below only BLOCKS a definite coder, and an unknown role must never be
        # silently treated as a coder (that would wrongly block) nor as a verifier
        # (that would wrongly allow) — it simply carries no override privilege here.
        caller_role = str(role or "").strip().lower()
        caller_role = ROLE_ALIASES.get(caller_role, caller_role)
        if caller_role not in VALID_ROLES:
            caller_role = ""

        # WARNING 2 precedence: a coder cannot override a verifier's adjudication.
        if caller_role in CODER_LIKE_ROLES:
            current = str(target.get("disposition", "open") or "open")
            if (
                current in CENSOR_VERIFIER_ADJUDICATED
                and _last_provenance_role(provenance) == "verifier"
            ):
                raise McpError(
                    "A coder cannot override a verifier-adjudicated Censor finding; "
                    "ask a verifier to change it."
                )

        target["disposition"] = disposition
        _append_provenance(
            provenance,
            {
                "actor": actor,
                "action": CENSOR_DISPOSITION_ACTION[disposition],
                "role": caller_role,
                "at": stamp,
            },
        )
        target["provenance"] = provenance
        shard["updatedAt"] = stamp
        # Atomic write under the lock we already hold (TOCTOU-free), pretty JSON
        # to match the Rust writer's `to_string_pretty` on-disk shape.
        write_text_crash_safe(
            shard_path,
            json.dumps(shard, indent=2, ensure_ascii=False),
            "Censor shard",
        )
        return _safe_censor_finding(target)


def _mini_directive_result(
    projects_dir: Path, state_lock: Path, directive_id: str
) -> tuple[bool, str, dict[str, Any] | None]:
    """Re-read the agents state UNDER THE LOCK and report the directive's poll state.

    Returns `(present, status, result)`:
      * `(True, <status>, <result dict>)` — the directive reached a terminal state.
      * `(True, <status>, None)` — the directive is present but still active.
      * `(False, "", None)` — the directive is NOT in the array.

    WARNING 5: the caller distinguishes `(True, _, None)` (still working) from
    `(False, _, None)` (vanished). A directive that was SEEN at least once and then
    disappears (capped out, hand-edited away, or a co-writer dropped it) must NOT be
    waited on for the full poll cap — the caller synthesizes a `failed`/`gone`
    outcome promptly instead. A directive never yet visible (the executor hasn't
    observed our just-written append, or a transient read) is also `(False, _, None)`,
    so the caller only treats absence as "vanished" once it has confirmed presence.

    FIX 3: the `status` lets the caller tell a directive the executor actually CLAIMED
    (`running`/`launching`) from one still `pending` at the deadline (never picked up).
    The former times out; the latter is a `failed` (the executor never started it).

    Holds the lock only for the read; NEVER across the caller's sleep (the Rust
    executor co-writes this file)."""
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
    for directive in state.get("miniCoderDirectives", []):
        if not isinstance(directive, dict):
            continue
        if str(directive.get("id") or "") == directive_id:
            status = str(directive.get("status") or "")
            result = directive.get("result")
            if isinstance(result, dict) and result:
                return True, status, result
            return True, status, None
    return False, "", None


def dispatch_spawn_mini_coder(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """Coder-only: delegate a cheap sub-task to a one-shot mini-coder the APP hosts.

    Writes a `pending` directive into `.aspis-agents.json` (the file-only bridge to
    the Rust executor — there is no push/reverse-trigger), then BLOCKS the caller's
    MCP thread on a BOUNDED poll of that directive's `result`. The executor claims
    the directive, spawns the one-shot PTY, reads the mini's result file on EOF, and
    stamps the terminal `MiniCoderOutcome` back onto the directive — which this poll
    returns. On the hard wall-clock cap the tool returns a synthesized `timeout`
    outcome (and the executor's own per-mini cap independently kills a runaway mini).

    GATING: the CALLER (`agent_id`/`role`/`session_token`) is validated via
    `require_agent_tool` (so only a registered, token-bearing coder reaches here);
    `spawn_mini_coder` is in the coder role's allowedTools. The caller IS the
    directive's `parentAgentId`, and it must be a LIVE (active) session — a coder
    that already closed cannot spawn a mini that would outlive its only
    human-contact point.
    """
    # 1) Authn/authz the CALLER (registered coder + valid session token).
    agent_id, role = require_agent_tool(projects_dir, args, "spawn_mini_coder")
    if "spawn_mini_coder" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use spawn_mini_coder.")

    # 2) Validate the task + files (the directive payload the executor + mini act on).
    task = clean_text(args.get("task"), "Mini-coder task", MINI_CODER_MAX_TASK_LEN)
    raw_files = args.get("files")
    if not isinstance(raw_files, list) or not raw_files:
        raise McpError("spawn_mini_coder requires a non-empty `files` list of project-relative paths.")
    if len(raw_files) > MINI_CODER_MAX_FILES:
        raise McpError(f"spawn_mini_coder accepts at most {MINI_CODER_MAX_FILES} files.")
    files: list[str] = []
    for entry in raw_files:
        # Reuse the strict rel-path validator (rejects absolute / `..` / `-`-leading
        # components) so a file path can never escape the project or smuggle an argv
        # flag into a downstream backend.
        files.append(validate_censor_rel_path(str(entry)))
    backend = args.get("backend")
    backend = clean_text(backend, "Mini-coder backend", 40) if str(backend or "").strip() else None
    allow_oracle = bool(args.get("allow_oracle", False))

    # 3) The caller must be a LIVE session (its parent-of-the-mini role). Verified
    #    under the lock in the SAME pass that appends the directive, so a coder that
    #    closes between the check and the append cannot slip a mini through.
    directive_id = uuid.uuid4().hex
    created_at = now()
    directive = {
        "id": directive_id,
        "parentAgentId": agent_id,
        "status": "pending",
        "task": task,
        "files": files,
        # `resultPath` is the REL filename under the executor-owned scratch root
        # (`<project_root>/.aspis-mini/`); the Rust executor finalizes the root and
        # `read_result_file` confines the read to it (canonicalize-after-open). We
        # keep it minimal + `..`-free so the two writers agree on one location.
        "resultPath": f"{directive_id}.json",
        "createdAt": created_at,
    }
    if backend is not None:
        directive["backend"] = backend
    # NO-CHURN: only emit allowOracle when true (matches the Rust serde skip).
    if allow_oracle:
        directive["allowOracle"] = True
    # P4 NO-CHURN: only emit write when true (matches the Rust serde skip). A
    # write directive's mini EMITS structured edits that the Rust executor
    # validates against `files` and applies — the model never touches disk.
    # STRICT boolean (review F7): a truthy non-bool ("true", 1) is NOT a grant.
    if args.get("write") is True:
        # Max-recall fix: the Rust apply enforces a 1..=10 allowlist for write
        # directives (MAX_MINI_ALLOWLIST_FILES) — failing here is cheap, failing
        # there wastes a full mini run. Keep the two caps in sync.
        if len(files) > 10:
            raise McpError(
                "Write directives allow at most 10 files in the allowlist "
                f"(got {len(files)}). Split the task."
            )
        directive["write"] = True

    # A2 PLUMBING: carry the WRITE mode end-to-end. Validate against the enum (the
    # wire strings MUST match the Rust `WriteMode` serde repr), then NO-CHURN:
    # only emit `writeMode` when it is the NON-default (`agenticIterative`), so an
    # `emitEdits` directive stays byte-identical to today and round-trips through
    # the Rust serde skip. Nothing branches on it yet (later workstream reads it).
    raw_write_mode = args.get("write_mode", MINI_CODER_WRITE_MODE_DEFAULT)
    if raw_write_mode is None:
        raw_write_mode = MINI_CODER_WRITE_MODE_DEFAULT
    if raw_write_mode not in MINI_CODER_WRITE_MODES:
        raise McpError(
            "spawn_mini_coder `write_mode` must be one of "
            f"{', '.join(MINI_CODER_WRITE_MODES)} (got {raw_write_mode!r})."
        )
    if raw_write_mode != MINI_CODER_WRITE_MODE_DEFAULT:
        # `write_mode` governs HOW the mini writes, so it is only coherent on a
        # WRITE directive — reject a non-default mode without `write: true` rather
        # than leaving a dangling `writeMode` on a read directive (review F1).
        if args.get("write") is not True:
            raise McpError(
                "spawn_mini_coder `write_mode` is only meaningful on a write "
                "directive — pass `write: true` (write_mode governs HOW the mini writes)."
            )
        directive["writeMode"] = raw_write_mode

    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        session = next(
            (item for item in state["sessions"] if item.get("agentId") == agent_id),
            None,
        )
        status = str((session or {}).get("status") or "").strip().lower()
        if session is None or status in ("", "closed", "launch_pending"):
            raise McpError(
                "spawn_mini_coder requires a live parent session; register (and keep it active) before delegating."
            )
        directives = state.setdefault("miniCoderDirectives", [])
        directives.append(directive)
        # Cap here too (write_agents_state normalizes again, but capping pre-write
        # keeps the in-memory list bounded and never drops our just-appended pending
        # directive — it is active, so eviction skips it).
        state["miniCoderDirectives"] = cap_mini_coder_directives(directives)
        add_event(
            state,
            agent_id,
            role,
            "mini_coder_spawn",
            f"Delegated a mini-coder sub-task on {len(files)} file(s).",
        )
        write_agents_state(projects_dir, state)

    # 4) BOUNDED poll for the executor's terminal verdict. Re-read under the lock
    #    each pass; NEVER hold the lock across the sleep. On the hard cap, return a
    #    synthesized `timeout` outcome the coder can act on (and best-effort mark the
    #    directive timed-out so the executor stops chasing it).
    deadline = time.monotonic() + MINI_CODER_POLL_TIMEOUT_SECS
    seen = False
    ever_ran = False  # FIX 3: observed in running/launching at least once (was claimed).
    while True:
        present, status, result = _mini_directive_result(
            projects_dir, state_lock, directive_id
        )
        if result is not None:
            return {"directiveId": directive_id, "result": result}
        if present:
            seen = True
            if status in ("running", "launching"):
                ever_ran = True
        elif seen:
            # WARNING 5: the directive was visible earlier and is now GONE (capped
            # out / dropped) with no terminal result we ever read. Do NOT block for
            # the full poll cap — return a synthesized `failed`/`gone` outcome now.
            return {
                "directiveId": directive_id,
                "result": {
                    "status": "failed",
                    "error": "mini-coder directive vanished before producing a result.",
                },
            }
        if time.monotonic() >= deadline:
            break
        time.sleep(MINI_CODER_POLL_INTERVAL_SECS)

    # Cap exceeded. FIX 3: the synthesized terminal outcome DEPENDS on whether the
    # executor ever actually CLAIMED the directive within the poll window:
    #   * it ran (status was running/launching at least once) -> `timeout` (it started
    #     but did not finish in time);
    #   * it is still `pending` (the executor never picked it up — e.g. the app is
    #     locked, the executor is down, or contention) -> `failed`, NOT `timeout`,
    #     with a clear error. A never-started directive timing out would mislead the
    #     coder into thinking the mini ran and merely overran.
    if ever_ran:
        synthesized = {
            "status": "timeout",
            "error": "spawn_mini_coder poll timed out waiting for the mini result.",
        }
    else:
        synthesized = {
            "status": "failed",
            "error": "executor did not start this mini within the poll window.",
        }
    try:
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            for directive in state.get("miniCoderDirectives", []):
                if not isinstance(directive, dict):
                    continue
                if str(directive.get("id") or "") == directive_id:
                    existing = directive.get("result")
                    # If the executor stamped a real terminal result in the tiny
                    # window since our last read, PREFER it (do not clobber a done).
                    if isinstance(existing, dict) and existing:
                        synthesized = existing
                    elif directive.get("killRequested"):
                        # BLOCKER 1 — killRequested WINS over the poll timeout. A human
                        # hit Stop (the Rust executor set killRequested=true + killed the
                        # PTY) but hasn't written the aborted_by_human result yet when our
                        # deadline fires. We must NOT stamp `timeout` here: that would make
                        # this directive terminal, the executor's later aborted_by_human
                        # `apply_result` would be refused (already terminal), and the human
                        # Stop would be silently lost at the MCP return (the coder retries
                        # instead of stop+escalate). Synthesize aborted_by_human instead so
                        # the contract honors killRequested-WINS end-to-end.
                        synthesized = {
                            "status": "aborted_by_human",
                            "error": "stopped by human (Stop button) — do not retry, escalate.",
                        }
                        directive["status"] = "aborted_by_human"
                        directive["result"] = synthesized
                    else:
                        # FIX 3: re-derive from the directive's CURRENT status under the
                        # lock (authoritative) — a directive still `pending` here was
                        # never claimed, so stamp `failed`; otherwise `timeout`.
                        # BLOCKER F-2: `awaiting_retry` means the mini DID start, ran, and
                        # triggered a retry chain that is still live at the deadline. It is
                        # NOT a "never started" failure — stamping `failed`/"did not start"
                        # would mislead the orchestrator into re-spawning the whole task
                        # from scratch. Treat it as a `timeout` (still running / retrying).
                        live_status = str(directive.get("status") or "")
                        if live_status in ("running", "launching", "awaiting_retry"):
                            synthesized = {
                                "status": "timeout",
                                "error": "spawn_mini_coder poll timed out (mini still running / retry chain in progress).",
                            }
                        else:
                            synthesized = {
                                "status": "failed",
                                "error": "executor did not start this mini within the poll window.",
                            }
                        directive["status"] = synthesized["status"]
                        directive["result"] = synthesized
                    break
            write_agents_state(projects_dir, state)
    except McpError:
        # A failed best-effort stamp must not change the contract: the coder still
        # gets a terminal outcome it can act on.
        pass
    return {"directiveId": directive_id, "result": synthesized}


def dispatch_steer_mini_coder(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """ASYNC STEERING (a): a supervising coder/orchestrator steers a RUNNING mini it
    spawned, by APPENDING a mid-flight correction to the directive's `steerQueue` — the
    SAME external-signal-to-a-running-directive channel as the Stop button's
    `killRequested`, generalized from one bool to a bounded FIFO. The Rust executor
    DRAINS the queue at the next fix-pass round boundary and folds it into that round's
    task (build_retry_directive / fold_steer_block); it never injects mid-token.

    Mirrors the `mini_coder_steer` Tauri command (the HUMAN's Console hook). A `message`
    equal to the STOP sentinel (`MINI_CODER_STEER_STOP_SENTINEL`, case-insensitive) maps
    to the kill path — it sets `killRequested` (the SAME field the Stop button sets) so
    the executor aborts the mini — rather than queueing prose. Everything else is a queued
    correction.

    Co-writer DISCIPLINE: under `file_lock(state_lock)` find the directive in
    `miniCoderDirectives` (the live attempt of its chain — `id == directive_id` or
    `parentDirectiveId == directive_id`, preferring an ACTIVE attempt), reject when absent
    or already TERMINAL, else mutate + `write_agents_state` (the same find-under-lock +
    write pattern as `dispatch_spawn_mini_coder`). NO-CHURN: an empty `steerQueue` is never
    written (omitted exactly like the Rust `Vec::is_empty` serde skip).

    Returns `{directiveId, status, queued?}`:
      * `status="queued"` (+ `queued`: new length) — correction appended;
      * `status="stopped"` — the stop sentinel set `killRequested`;
      * `status="queue_full"` (+ `queued`) — the FIFO is full; the message was REFUSED
        (never drops an already-queued correction);
      * `status="not_found"` — no such directive;
      * `status="terminal"` — the mini already finished (nothing to steer).
    """
    # 1) Authn/authz the CALLER (registered coder/orchestrator + valid session token).
    agent_id, role = require_agent_tool(projects_dir, args, "steer_mini_coder")
    if "steer_mini_coder" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use steer_mini_coder.")

    directive_id = clean_text(args.get("directive_id"), "Mini-coder directive id", 200)
    # The message rides the fix-pass prompt; cap it (CO-WRITER PARITY with the Rust
    # MAX_STEER_MESSAGE_LEN) so a pathological steer cannot bloat the prompt.
    message = clean_text(args.get("message"), "Steer message", MINI_CODER_MAX_STEER_LEN)
    is_stop = message.strip().casefold() == MINI_CODER_STEER_STOP_SENTINEL

    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        directives = state.get("miniCoderDirectives") or []
        # Resolve the chain's LIVE attempt: the directive whose id == directive_id, or any
        # retry child (parentDirectiveId == directive_id), preferring an ACTIVE one (the
        # attempt whose next round boundary drains the queue). Mirrors Rust
        # mark_steer_requested's targeting.
        in_chain = [
            d
            for d in directives
            if isinstance(d, dict)
            and (
                str(d.get("id") or "") == directive_id
                or str(d.get("parentDirectiveId") or "") == directive_id
            )
        ]
        if not in_chain:
            return {"directiveId": directive_id, "status": "not_found"}
        active = next(
            (d for d in in_chain if str(d.get("status") or "") in _MINI_ACTIVE_STATUSES),
            None,
        )
        target = active or next(
            (d for d in in_chain if str(d.get("id") or "") == directive_id),
            in_chain[0],
        )
        # A whole chain that already reached a TERMINAL state cannot be steered. (An
        # awaiting_retry predecessor is neither active nor terminal — its chain has a live
        # retry, which the `active` lookup above already preferred.)
        if all(str(d.get("status") or "") in _MINI_TERMINAL_STATUSES for d in in_chain):
            return {"directiveId": directive_id, "status": "terminal"}

        if is_stop:
            # STOP reuses the kill path: flag killRequested on EVERY non-terminal attempt in
            # the chain (the executor's EOF-finalize then synthesizes aborted_by_human),
            # mirroring the Rust mark_kill_requested chain flag. NO prose is queued.
            for d in in_chain:
                if str(d.get("status") or "") not in _MINI_TERMINAL_STATUSES:
                    d["killRequested"] = True
            add_event(
                state,
                agent_id,
                role,
                "mini_coder_steer",
                "Sent a STOP steer to a mini-coder (kill path).",
            )
            write_agents_state(projects_dir, state)
            return {"directiveId": directive_id, "status": "stopped"}

        queue = target.get("steerQueue")
        if not isinstance(queue, list):
            queue = []
        # Bounded FIFO: refuse (do not drop the oldest) when full so a queued correction is
        # never lost. CO-WRITER PARITY with Rust MAX_STEER_QUEUE_LEN.
        if len(queue) >= MINI_CODER_MAX_STEER_QUEUE:
            return {
                "directiveId": directive_id,
                "status": "queue_full",
                "queued": len(queue),
            }
        queue.append(message)
        # NO-CHURN: only write steerQueue when non-empty (it always is here) — an empty
        # queue is never serialized, matching the Rust Vec::is_empty serde skip.
        target["steerQueue"] = queue
        add_event(
            state,
            agent_id,
            role,
            "mini_coder_steer",
            f"Queued a steer correction for a mini-coder ({len(queue)} pending).",
        )
        write_agents_state(projects_dir, state)
        return {"directiveId": directive_id, "status": "queued", "queued": len(queue)}


def clean_visual_html_path(value: Any) -> str:
    text = strip_invisible_and_bidi(str(value or "")).strip()
    if not text:
        raise McpError("visual_check requires html_path.")
    if len(text) > VISUAL_CHECK_MAX_HTML_PATH_CHARS:
        raise McpError("visual_check html_path is too long.")
    if any(ord(ch) < 32 or ch == "\x7f" for ch in text):
        raise McpError("visual_check html_path must not contain control characters.")
    return text.replace("\\", "/")


def _visual_directive_result(
    projects_dir: Path, state_lock: Path, directive_id: str
) -> tuple[bool, str, dict[str, Any] | None]:
    """Read one visual-check directive under the lock.

    Same contract as `_mini_directive_result`: present/status/result or absent.
    """
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        for directive in state.get("visualCheckDirectives", []):
            if not isinstance(directive, dict):
                continue
            if str(directive.get("id") or "") == directive_id:
                status = str(directive.get("status") or "")
                result = directive.get("result")
                if isinstance(result, dict) and result:
                    return True, status, result
                return True, status, None
    return False, "", None


def _visual_tool_result(directive_id: str, result: dict[str, Any]) -> dict[str, Any]:
    status = str(result.get("status") or "").strip().lower()
    if status == "done":
        critique = clean_text(result.get("critique"), "Visual critique", 4000)
        return {"directiveId": directive_id, "critique": critique}
    error = clean_text(result.get("error") or "visual_check failed.", "Visual check error", 1000)
    return {"directiveId": directive_id, "error": error}


def dispatch_visual_check(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """Ask the app to render and visually critique one confined HTML artifact."""
    agent_id, role = require_agent_tool(projects_dir, args, "visual_check")
    if "visual_check" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use visual_check.")

    html_path = clean_visual_html_path(args.get("html_path"))
    raw_focus = str(args.get("focus") or "").strip()
    focus = clean_text(raw_focus, "Visual check focus", VISUAL_CHECK_MAX_FOCUS_CHARS) if raw_focus else None

    directive_id = uuid.uuid4().hex
    created_at = now()
    directive = {
        "id": directive_id,
        "parentAgentId": agent_id,
        "status": "pending",
        "htmlPath": html_path,
        "resultPath": f"{directive_id}.json",
        "createdAt": created_at,
    }
    if focus is not None:
        directive["focus"] = focus

    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        session = next(
            (item for item in state["sessions"] if item.get("agentId") == agent_id),
            None,
        )
        status = str((session or {}).get("status") or "").strip().lower()
        if session is None or status in ("", "closed", "launch_pending"):
            raise McpError(
                "visual_check requires a live registered session."
            )
        directives = state.setdefault("visualCheckDirectives", [])
        directives.append(directive)
        state["visualCheckDirectives"] = cap_visual_check_directives(directives)
        add_event(
            state,
            agent_id,
            role,
            "visual_check",
            "Requested a local visual critique for one HTML artifact.",
        )
        write_agents_state(projects_dir, state)

    deadline = time.monotonic() + VISUAL_CHECK_POLL_TIMEOUT_SECS
    seen = False
    ever_ran = False
    while True:
        present, status, result = _visual_directive_result(projects_dir, state_lock, directive_id)
        if result is not None:
            return _visual_tool_result(directive_id, result)
        if present:
            seen = True
            if status == "running":
                ever_ran = True
        elif seen:
            return {
                "directiveId": directive_id,
                "error": "visual_check directive vanished before producing a result.",
            }
        if time.monotonic() >= deadline:
            break
        time.sleep(VISUAL_CHECK_POLL_INTERVAL_SECS)

    synthesized = (
        {"status": "timeout", "error": "visual_check timed out waiting for the local critique."}
        if ever_ran
        else {"status": "failed", "error": "visual-check executor did not start this request within the poll window."}
    )
    try:
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            for directive in state.get("visualCheckDirectives", []):
                if not isinstance(directive, dict):
                    continue
                if str(directive.get("id") or "") == directive_id:
                    existing = directive.get("result")
                    if isinstance(existing, dict) and existing:
                        synthesized = existing
                    else:
                        live_status = str(directive.get("status") or "")
                        if live_status == "running":
                            synthesized = {
                                "status": "timeout",
                                "error": "visual_check timed out waiting for the local critique.",
                            }
                        directive["status"] = synthesized["status"]
                        directive["result"] = synthesized
                    break
            write_agents_state(projects_dir, state)
    except McpError:
        pass
    return _visual_tool_result(directive_id, synthesized)


# FIX 4: cheap mirror of the Rust `sanitize_error` GitHub-token families
# (src-tauri/src/backend/github.rs): classic PATs `ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`
# and fine-grained `github_pat_`, each followed by a run of token-body chars
# `[A-Za-z0-9_]`. One compiled regex; the whole token (prefix + body) is replaced.
_GITHUB_TOKEN_RE = re.compile(r"(?:gh[pousr]_|github_pat_)[A-Za-z0-9_]+")


def _scrub_push_result(result: dict[str, Any]) -> dict[str, Any]:
    """DEFENSE-IN-DEPTH egress scrub for a GitPushResult before it is returned to
    the agent over the MCP surface.

    INVARIANT: the Rust push path already redacts the PAT out of `output`/`error`
    (it injects the credential off-argv via GIT_ASKPASS and runs git stderr through
    `sanitize_error` + a literal `redact_token`). This Python boundary is the egress
    point that hands the on-disk result dict to the agent verbatim, so it must not
    rely SOLELY on that Rust invariant: a result written by an older/pre-redaction
    Rust build, or a hand-edited `.aspis-agents.json`, could still carry a raw
    `gh*_`/`github_pat_` token. We re-redact those two string fields here so a token
    can never egress, regardless of who wrote the file. Cheap: a single regex sub
    over two short strings, only when present; the dict is shallow-copied so the
    on-disk/in-memory state is never mutated."""
    if not isinstance(result, dict):
        return result
    scrubbed = dict(result)
    for key in ("output", "error"):
        value = scrubbed.get(key)
        if isinstance(value, str) and value:
            scrubbed[key] = _GITHUB_TOKEN_RE.sub("[redacted-github-token]", value)
    return scrubbed


def _git_push_request_result(
    projects_dir: Path, state_lock: Path, request_id: str
) -> tuple[bool, str, dict[str, Any] | None]:
    """Re-read the agents state UNDER THE LOCK and report a push request's poll state.

    Returns `(present, status, result)`:
      * `(True, <status>, <result dict>)` — the request reached a terminal state.
      * `(True, <status>, None)` — the request is present but still active.
      * `(False, "", None)` — the request is NOT in the array.

    Mirrors `_mini_directive_result`: the caller distinguishes `(True, _, None)`
    (still awaiting the human) from `(False, _, None)` (vanished — capped out /
    hand-edited away). Holds the lock ONLY for the read; NEVER across the caller's
    sleep (the Rust approve/deny command co-writes this file)."""
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
    for request in state.get("gitPushRequests", []):
        if not isinstance(request, dict):
            continue
        if str(request.get("id") or "") == request_id:
            status = str(request.get("status") or "")
            result = request.get("result")
            if isinstance(result, dict) and result:
                return True, status, result
            return True, status, None
    return False, "", None


def dispatch_request_git_push(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """Coder-only: REQUEST human approval for a git push, then BLOCK on the verdict.

    Agents may COMMIT freely, but every PUSH must be approved by the human. This
    writes a `pending_approval` GitPushRequest into `.aspis-agents.json` (the
    file-only bridge to the app — there is no reverse-trigger), SETS the requesting
    session's `needsUser` so the existing bell lights, then BOUNDED-polls the
    request's verdict. The HUMAN (via the PushApprovalCard) approves — and the Rust
    `approve_git_push_request` command performs the actual push and stamps the
    terminal result — or denies. On the hard poll cap the tool returns a synthesized
    `timeout` outcome and best-effort stamps the request `timeout`; the agent MUST
    then STOP (it must NOT retry and MUST NOT raw-push).

    PYTHON NEVER PUSHES — it only requests and polls. The push is performed solely by
    the Rust approve command (which injects the credential off argv via GIT_ASKPASS).

    GATING: the CALLER (`agent_id`/`role`/`session_token`) is validated via
    `require_agent_tool`; `request_git_push` is in the coder or orchestrator role's
    allowedTools ONLY (a verifier cannot call it). The caller must be a LIVE session.
    """
    # 1) Authn/authz the CALLER (registered coder + valid session token).
    agent_id, role = require_agent_tool(projects_dir, args, "request_git_push")
    if "request_git_push" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use request_git_push.")

    # 2) Validate the request payload.
    project_id = normalize_project_id(args.get("project_id", ""))
    if not project_id:
        raise McpError("request_git_push requires a project_id.")
    # FIX F5 — CROSS-PROJECT PUSH AUTHORIZATION (residual gap, DELIBERATELY NOT
    # enforced here): ideally we would also assert the requesting coder is registered
    # to `project_id` (e.g. session["currentProjectId"] == project_id). We verified
    # that `currentProjectId` is NOT reliably populated for a live coder: neither
    # `agent_register` nor `agent_heartbeat` passes a project_id to `upsert_session`,
    # so the field is set ONLY when the coder happens to call a project-scoped tool
    # (claim_task / update_task / note / followup) — a coder can legitimately
    # `request_git_push` without ever having stamped it. A strict equality check would
    # therefore reject legitimate pushes. We leave authorization to the HUMAN approval
    # gate (every push is human-approved in the PushApprovalCard, which shows the
    # requesting agent + project), and Rust re-resolves `project_id` to a real repo at
    # push time. If/when registration starts carrying the project reliably, tighten
    # this to `session.currentProjectId == project_id`.
    branch_raw = str(args.get("branch") or "").strip()
    branch = clean_text(branch_raw, "Branch", 200) if branch_raw else None
    # FIX F9: validate the remote against the SAME allowlist Rust enforces at push
    # time, so an invalid remote is rejected at REQUEST time (fail fast) instead of
    # occupying a queue slot + ringing the bell for a push Rust can never approve.
    remote = validate_push_remote(args.get("remote"))
    force = bool(args.get("force", False))

    request_id = uuid.uuid4().hex
    created_at = now()
    request = {
        "id": request_id,
        "agentId": agent_id,
        "projectId": project_id,
        "status": "pending_approval",
        "createdAt": created_at,
    }
    if branch is not None:
        request["branch"] = branch
    if remote is not None:
        request["remote"] = remote
    # NO-CHURN: only emit force when true (matches the Rust serde skip).
    if force:
        request["force"] = True

    # 3) Append the request + SET the requesting session's needsUser (bell), under
    #    the SAME lock, after confirming the caller is a LIVE session.
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        session = next(
            (item for item in state["sessions"] if item.get("agentId") == agent_id),
            None,
        )
        status = str((session or {}).get("status") or "").strip().lower()
        if session is None or status in ("", "closed", "launch_pending"):
            raise McpError(
                "request_git_push requires a live session; register (and keep it active) before requesting a push."
            )
        force_note = " (FORCE)" if force else ""
        target = branch or "current branch"
        # Light the existing needs-you bell on the requesting session. The Rust
        # approve/deny command CLEARS it on every terminal path.
        session["needsUser"] = {
            "reason": "needs_push_approval",
            "message": f"Awaiting approval to push {target}{force_note} to {remote or 'origin'}.",
            "since": now(),
        }
        requests = state.setdefault("gitPushRequests", [])
        requests.append(request)
        state["gitPushRequests"] = cap_git_push_requests(requests)
        add_event(
            state,
            agent_id,
            role,
            "git_push_request",
            f"Requested human approval to push{force_note}.",
            project_id,
        )
        write_agents_state(projects_dir, state)

    # 4) BOUNDED poll for the human's verdict (approve+push-result / denied /
    #    timeout). Re-read under the lock each pass; NEVER hold the lock across the
    #    sleep. On the hard cap, return a synthesized `timeout` outcome and best-
    #    effort stamp the still-pending request `timeout` so a later approve no-ops.
    deadline = time.monotonic() + GIT_PUSH_POLL_TIMEOUT_SECS
    seen = False
    while True:
        present, _status, result = _git_push_request_result(
            projects_dir, state_lock, request_id
        )
        if result is not None:
            return {"requestId": request_id, "result": _scrub_push_result(result)}
        if present:
            seen = True
        elif seen:
            # Was visible earlier and is now GONE (capped out / dropped) with no
            # terminal result — synthesize a `failed`/`gone` outcome now.
            # FIX 8: the request vanished but the requesting session's needsUser bell
            # is still lit (nothing cleared it — the Rust approve/deny never ran on a
            # request that no longer exists). Make a best-effort clear so the bell does
            # not stay lit forever; swallow any McpError (lock contention) since the
            # synthetic failure must still be returned to the agent.
            try:
                with file_lock(state_lock):
                    state = read_agents_state(projects_dir)
                    cleared = False
                    for s in state["sessions"]:
                        if s.get("agentId") == agent_id and s.get("needsUser") is not None:
                            s["needsUser"] = None
                            cleared = True
                            break
                    if cleared:
                        write_agents_state(projects_dir, state)
            except McpError:
                pass
            return {
                "requestId": request_id,
                "result": {
                    "status": "push_failed",
                    "error": "push request vanished before producing a result.",
                },
            }
        if time.monotonic() >= deadline:
            break
        time.sleep(GIT_PUSH_POLL_INTERVAL_SECS)

    # Cap exceeded: the human never acted. Synthesize `timeout` and best-effort stamp
    # the request terminal ONLY IF it is still `pending_approval` (so a human approve
    # that landed in the tiny window — moving it to approved/pushing/terminal — is
    # never clobbered; we PREFER any existing terminal result). The agent must STOP.
    synthesized = {
        "status": "timeout",
        "error": "push approval timed out — STOP, do not retry, do not push directly.",
    }
    try:
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            # FIX 7: only persist if we actually MUTATED the state. The existing-result
            # branch and the approved/pushing no-op branch change nothing, so writing
            # there is a spurious updatedAt churn (and a redundant disk write that can
            # race the Rust approve command's own write). Track a `modified` flag and
            # write ONLY when we transitioned the request to `timeout`.
            modified = False
            for request in state.get("gitPushRequests", []):
                if not isinstance(request, dict):
                    continue
                if str(request.get("id") or "") == request_id:
                    existing = request.get("result")
                    if isinstance(existing, dict) and existing:
                        # The human acted in the window; PREFER the real result. No-op.
                        synthesized = existing
                    elif str(request.get("status") or "") == "pending_approval":
                        request["status"] = "timeout"
                        request["result"] = synthesized
                        modified = True
                        # Clear the bell: the agent gave up, so the request is no
                        # longer pending the human (mirrors the Rust clear-on-terminal).
                        for s in state["sessions"]:
                            if s.get("agentId") == agent_id:
                                s["needsUser"] = None
                                break
                    else:
                        # approved/pushing: leave it — the approve command owns the
                        # result. Return timeout to the agent (it has given up), but
                        # the human's push still proceeds and the card shows it. No-op.
                        pass
                    break
            if modified:
                write_agents_state(projects_dir, state)
    except McpError:
        pass
    return {"requestId": request_id, "result": _scrub_push_result(synthesized)}


# Phase 1: a plan id is EXACTLY 32 lowercase hex (uuid4().hex). NEVER trust a value
# that does not match as a path segment (plan_status reads a `<plan_id>.json` sidecar)
# — this is the only barrier between an attacker-supplied id and a filesystem read.
_PLAN_ID_RE = re.compile(r"[0-9a-f]{32}")


def plans_dir(projects_dir: Path) -> Path:
    """Root of the plan artifacts, OUTSIDE the agents-state file. Each project namespaces
    its plans under `<project_id>/`."""
    return projects_dir / ".aspis-plans"


def _plan_artifact_paths(projects_dir: Path, project_id: str, plan_id: str) -> tuple[Path, Path]:
    """Resolve the (markdown, sidecar-json) paths for a plan. `project_id` is already
    normalized (lowercase allowlist) and `plan_id` is already validated 32-hex, so the
    join cannot escape the plans dir, but we still confine it via ensure_inside_projects
    on the base for defense in depth."""
    base = ensure_inside_projects(projects_dir, plans_dir(projects_dir) / project_id)
    return base / f"{plan_id}.md", base / f"{plan_id}.json"


def _update_plan_sidecar_status(
    projects_dir: Path,
    project_id: str,
    plan_id: str,
    status: str,
    note: str | None = None,
) -> None:
    """Best-effort: stamp the sidecar JSON to a terminal status (+ decidedAt/note). The
    sidecar is the durable record once the queue entry is evicted, so the timeout sweep
    keeps it in sync. Swallows all errors — a sidecar write must never break the tool
    return (the queue entry is authoritative for the live verdict)."""
    _, sidecar_path = _plan_artifact_paths(projects_dir, project_id, plan_id)
    try:
        if not sidecar_path.exists():
            return
        data = json.loads(sidecar_path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            return
        data["status"] = status
        data["decidedAt"] = now()
        if note is not None:
            data["note"] = note
        write_text_crash_safe(sidecar_path, json.dumps(data, ensure_ascii=False, indent=2), "plan sidecar")
    except (OSError, json.JSONDecodeError, McpError):
        pass


def _plan_request_outcome(
    projects_dir: Path, state_lock: Path, plan_id: str
) -> tuple[bool, str, str | None]:
    """Re-read the agents state UNDER THE LOCK and report a plan request's poll state.

    Returns `(present, status, note)`:
      * `(True, <status>, <note|None>)` — the request is present (terminal or pending).
      * `(False, "", None)` — the request is NOT in the array (vanished / evicted).

    Mirrors `_git_push_request_result`: the caller distinguishes `(True, _, _)` (still
    tracked) from `(False, _, _)` (vanished). Holds the lock ONLY for the read; NEVER
    across the caller's sleep (the Rust approve/reject command co-writes this file)."""
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
    for request in state.get("planApprovalRequests", []):
        if not isinstance(request, dict):
            continue
        if str(request.get("id") or "") == plan_id:
            status = str(request.get("status") or "")
            note = request.get("note")
            return True, status, note if isinstance(note, str) else None
    return False, "", None


# Plan-approval terminal statuses the poll treats as a final human verdict.
_PLAN_VERDICT_STATUSES = {"approved", "rejected"}


def dispatch_plan_submit(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """Coder-only: SUBMIT an implementation plan for human approval, then BLOCK on the
    verdict. Before doing multi-file work the coder writes its plan; this persists the
    markdown + a sidecar OUTSIDE the state file, appends a `pending_approval` entry to
    `planApprovalRequests` in `.aspis-agents.json` (the file-only bridge to the app —
    there is no reverse-trigger), SETS the requesting session's `needsUser` (bell), then
    BOUNDED-polls the verdict. The HUMAN approves/rejects via the app (the Rust command
    stamps the terminal status + an optional note). On the hard poll cap the tool returns
    a synthesized `timeout` and best-effort stamps the still-pending request `timeout`.

    GATING: the CALLER is validated via `require_agent_tool`; `plan_submit` is in the
    coder or orchestrator role's allowedTools ONLY (a verifier cannot call it). The
    caller must be a LIVE session.
    """
    # 1) Authn/authz the CALLER (registered coder + valid session token).
    agent_id, role = require_agent_tool(projects_dir, args, "plan_submit")
    if "plan_submit" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use plan_submit.")

    # 2) Validate the payload. `project_id` must resolve to a REAL project (reusing
    #    load_project_locked — it raises McpError if the markdown does not exist). The title
    #    is single-line cleaned + capped; the markdown is prose (newlines PRESERVED, so
    #    NOT run through clean_text which collapses whitespace) — non-empty + hard cap.
    project_id = normalize_project_id(args.get("project_id", ""))
    load_project_locked(projects_dir, project_id)  # raises McpError("Project not found.")
    title = clean_text(args.get("title"), "Plan title", 200)
    plan_markdown = str(args.get("plan_markdown") or "")
    if not strip_invisible_and_bidi(plan_markdown).strip():
        raise McpError("plan_submit requires a non-empty plan_markdown.")
    if len(plan_markdown) > PLAN_MAX_MARKDOWN_CHARS:
        raise McpError(
            f"plan_markdown is too long (max {PLAN_MAX_MARKDOWN_CHARS} characters)."
        )

    plan_id = uuid.uuid4().hex
    created_at = now()

    # 3) Write the artifacts OUTSIDE the state lock, atomically (temp+rename). The
    #    sidecar is the durable record; the queue entry is the live bell. Written BEFORE
    #    the state mutation so a crash between the two leaves an orphan artifact (cheap)
    #    rather than a queue entry pointing at a missing plan.
    md_path, sidecar_path = _plan_artifact_paths(projects_dir, project_id, plan_id)
    md_path.parent.mkdir(parents=True, exist_ok=True)
    sidecar = {
        "id": plan_id,
        "projectId": project_id,
        "agentId": agent_id,
        "title": title,
        "status": "pending_approval",
        "createdAt": created_at,
    }
    write_text_crash_safe(md_path, plan_markdown, "plan markdown")
    write_text_crash_safe(sidecar_path, json.dumps(sidecar, ensure_ascii=False, indent=2), "plan sidecar")

    request = {
        "id": plan_id,
        "agentId": agent_id,
        "projectId": project_id,
        "title": title,
        "status": "pending_approval",
        "createdAt": created_at,
    }

    # 4) Append the request + SET the requesting session's needsUser (bell), under the
    #    SAME lock, after confirming the caller is a LIVE session.
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        session = next(
            (item for item in state["sessions"] if item.get("agentId") == agent_id),
            None,
        )
        status = str((session or {}).get("status") or "").strip().lower()
        if session is None or status in ("", "closed", "launch_pending"):
            raise McpError(
                "plan_submit requires a live session; register (and keep it active) before submitting a plan."
            )
        # Do NOT clobber an active bell raised for a DIFFERENT reason (e.g. an
        # in-flight question, or an earlier plan still awaiting approval). The agent
        # must resolve the outstanding needsUser first. A same-reason bell is the
        # dedup case: preserve the original `since` so the wait time is not reset.
        existing_needs = session.get("needsUser")
        existing_reason = (
            str(existing_needs.get("reason") or "")
            if isinstance(existing_needs, dict)
            else ""
        )
        if existing_reason and existing_reason != "needs_plan_approval":
            raise McpError(
                "This session already has an outstanding needsUser "
                f"(reason: {existing_reason}); resolve it before submitting a plan."
            )
        since = (
            existing_needs.get("since")
            if existing_reason == "needs_plan_approval"
            and isinstance(existing_needs, dict)
            and existing_needs.get("since")
            else now()
        )
        session["needsUser"] = {
            "reason": "needs_plan_approval",
            "message": clean_text(f"Plan '{title}' awaits approval.", "Message", 1000),
            "since": since,
        }
        requests = state.setdefault("planApprovalRequests", [])
        requests.append(request)
        state["planApprovalRequests"] = cap_plan_approval_requests(requests)
        add_event(
            state,
            agent_id,
            role,
            "plan_submit",
            f"Submitted a plan for approval: {title}.",
            project_id,
        )
        write_agents_state(projects_dir, state)

    # 5) BOUNDED poll for the human's verdict (approved/rejected/timeout). Re-read under
    #    the lock each pass; NEVER hold the lock across the sleep. On the hard cap, return
    #    a synthesized `timeout` and best-effort stamp the still-pending request `timeout`
    #    (a human verdict that raced in WINS — we PREFER any existing terminal status).
    deadline = time.monotonic() + PLAN_POLL_TIMEOUT_SECS
    seen = False
    first = True
    while True:
        # Check the deadline at the TOP, before the locked read — but always run at
        # least one pass so a verdict that is already present (or raced in) is seen.
        # This prevents a sleep from pushing the effective timeout one interval past
        # the cap (the old bottom-of-loop check could overshoot by ~one interval).
        if not first and time.monotonic() >= deadline:
            break
        first = False
        present, status, note = _plan_request_outcome(projects_dir, state_lock, plan_id)
        if present and status in _PLAN_VERDICT_STATUSES:
            result = {"planId": plan_id, "status": status}
            if note is not None:
                result["note"] = note
            return result
        if present:
            seen = True
        elif seen:
            # Was visible earlier and is now GONE (capped out / evicted) with no verdict.
            # Clear the still-lit bell best-effort, then return a clear error result.
            try:
                with file_lock(state_lock):
                    state = read_agents_state(projects_dir)
                    cleared = False
                    for s in state["sessions"]:
                        if s.get("agentId") == agent_id and s.get("needsUser") is not None:
                            s["needsUser"] = None
                            cleared = True
                            break
                    if cleared:
                        write_agents_state(projects_dir, state)
            except McpError:
                pass
            return {
                "planId": plan_id,
                "status": "vanished",
                "note": "plan request vanished before producing a verdict.",
            }
        if time.monotonic() >= deadline:
            break
        time.sleep(PLAN_POLL_INTERVAL_SECS)

    # Cap exceeded: the human never acted. Synthesize `timeout` and best-effort stamp the
    # request terminal ONLY IF it is still `pending_approval` (so a human approve/reject
    # that landed in the tiny window is never clobbered — PREFER the existing verdict).
    final_status = "timeout"
    final_note: str | None = None
    try:
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            modified = False
            for request in state.get("planApprovalRequests", []):
                if not isinstance(request, dict):
                    continue
                if str(request.get("id") or "") == plan_id:
                    current = str(request.get("status") or "")
                    existing_note = request.get("note")
                    if current in _PLAN_VERDICT_STATUSES:
                        # The human acted in the window; PREFER the real verdict. No-op.
                        final_status = current
                        final_note = existing_note if isinstance(existing_note, str) else None
                    elif current == "pending_approval":
                        request["status"] = "timeout"
                        request["decidedAt"] = now()
                        modified = True
                        # Clear the bell: the agent gave up (mirrors clear-on-terminal).
                        for s in state["sessions"]:
                            if s.get("agentId") == agent_id:
                                s["needsUser"] = None
                                break
                    else:
                        # Already terminal as timeout (or some other state) — leave it.
                        final_status = current or "timeout"
                        final_note = existing_note if isinstance(existing_note, str) else None
                    break
            if modified:
                write_agents_state(projects_dir, state)
    except McpError:
        pass

    # Best-effort: keep the durable sidecar in sync with the terminal status we report.
    _update_plan_sidecar_status(projects_dir, project_id, plan_id, final_status, final_note)

    result = {"planId": plan_id, "status": final_status}
    if final_note is not None:
        result["note"] = final_note
    return result


def dispatch_plan_status(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """Read-only: report a plan's current status. Available to coder AND verifier. The
    `plan_id` MUST be exactly 32 lowercase hex (NEVER trusted as a path segment otherwise
    — it feeds a sidecar glob). Looks up the live queue entry first; if absent (evicted),
    falls back to the durable sidecar on disk via a BOUNDED glob across all projects."""
    agent_id, role = require_agent_tool(projects_dir, args, "plan_status")
    if "plan_status" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use plan_status.")

    plan_id = str(args.get("plan_id") or "").strip().lower()
    if not _PLAN_ID_RE.fullmatch(plan_id):
        raise McpError("plan_id must be exactly 32 lowercase hexadecimal characters.")

    # 1) Live queue entry (authoritative for an active/just-decided plan).
    present, status, note = _plan_request_outcome(projects_dir, state_lock, plan_id)
    if present:
        result = {"planId": plan_id, "status": status or "pending_approval"}
        if note is not None:
            result["note"] = note
        return result

    # 2) Fall back to the durable sidecar. BOUNDED glob: the id is validated 32-hex, so
    #    the `*` only varies the project namespace; we read the FIRST matching sidecar.
    base = plans_dir(projects_dir)
    if base.is_dir():
        for sidecar_path in base.glob(f"*/{plan_id}.json"):
            try:
                data = json.loads(sidecar_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if not isinstance(data, dict):
                continue
            result = {"planId": plan_id, "status": str(data.get("status") or "pending_approval")}
            note_value = data.get("note")
            if isinstance(note_value, str):
                result["note"] = note_value
            return result

    return {"planId": plan_id, "status": "not_found"}


def dispatch_ask_user(
    projects_dir: Path,
    state_lock: Path,
    args: dict[str, Any],
) -> dict[str, Any]:
    """Coder OR verifier: ask the HUMAN a blocking question and wait for the reply.

    Sets `session.pendingQuestion` + lights the `needsUser` bell, then BOUNDED-polls for
    `session.userReply` whose `questionId` matches the pending question. A stale reply
    (answer to an OLDER question) is ignored AND cleared so it cannot satisfy this poll.
    On a match: consume (delete pendingQuestion + userReply, clear the question bell) and
    return the text. On timeout: clear pendingQuestion + the question bell, return a
    `timeout` shape. Tolerates the Rust side having already cleared `needsUser` on reply.
    """
    agent_id, role = require_agent_tool(projects_dir, args, "ask_user")
    if "ask_user" not in ROLE_ALLOWED_TOOLS.get(role, set()):
        raise McpError(f"{role} agents cannot use ask_user.")

    question = clean_text(args.get("question"), "Question", 4000)
    question_id = uuid.uuid4().hex
    created_at = now()

    # 1) Set the pending question + the bell, under the lock, after confirming the caller
    #    is a LIVE session.
    with file_lock(state_lock):
        state = read_agents_state(projects_dir)
        session = next(
            (item for item in state["sessions"] if item.get("agentId") == agent_id),
            None,
        )
        status = str((session or {}).get("status") or "").strip().lower()
        if session is None or status in ("", "closed", "launch_pending"):
            raise McpError(
                "ask_user requires a live session; register (and keep it active) before asking the human."
            )
        # Symmetric to plan_submit: do NOT clobber an active bell raised for a
        # DIFFERENT reason (e.g. a pending plan approval). The agent must resolve the
        # outstanding needsUser first. A same-reason "question" bell is the dedup case:
        # preserve the original `since` so the wait time is not reset.
        existing_needs = session.get("needsUser")
        existing_reason = (
            str(existing_needs.get("reason") or "")
            if isinstance(existing_needs, dict)
            else ""
        )
        if existing_reason and existing_reason != "question":
            raise McpError(
                "This session already has an outstanding needsUser "
                f"(reason: {existing_reason}); resolve it before asking the human."
            )
        since = (
            existing_needs.get("since")
            if existing_reason == "question"
            and isinstance(existing_needs, dict)
            and existing_needs.get("since")
            else now()
        )
        session["pendingQuestion"] = {
            "id": question_id,
            "question": question,
            "createdAt": created_at,
        }
        # A new question supersedes any stale reply to a PRIOR question.
        session.pop("userReply", None)
        session["needsUser"] = {
            "reason": "question",
            "message": clean_text(question, "Message", 1000),
            "since": since,
        }
        add_event(state, agent_id, role, "ask_user", "Asked the human a question.")
        write_agents_state(projects_dir, state)

    # 2) BOUNDED poll for a MATCHING reply. Re-read under the lock each pass; NEVER hold
    #    the lock across the sleep (the Rust reply command co-writes this file).
    deadline = time.monotonic() + ASK_USER_POLL_TIMEOUT_SECS
    first = True
    while True:
        # Check the deadline at the TOP, before the locked read (symmetric to the
        # plan_submit poll) — but always run at least one pass so a reply already
        # present (or raced in) is consumed. Prevents overshooting the cap by ~one
        # interval, which the old bottom-of-loop check could do after a sleep.
        if not first and time.monotonic() >= deadline:
            break
        first = False
        matched_text: str | None = None
        consumed = False
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            session = next(
                (item for item in state["sessions"] if item.get("agentId") == agent_id),
                None,
            )
            if session is None:
                # The session vanished (closed/capped) — give up cleanly.
                return {"timeout": True, "note": "session ended before the human replied."}
            pending = session.get("pendingQuestion")
            pending_id = str((pending or {}).get("id") or "") if isinstance(pending, dict) else ""
            reply = session.get("userReply")
            if isinstance(reply, dict):
                reply_qid = str(reply.get("questionId") or "")
                if pending_id and reply_qid == question_id and reply_qid == pending_id:
                    # MATCH: consume both the question and the reply, clear the question
                    # bell (tolerate it already being cleared by the Rust side).
                    matched_text = str(reply.get("text") or "")
                    session.pop("pendingQuestion", None)
                    session.pop("userReply", None)
                    needs = session.get("needsUser")
                    if isinstance(needs, dict) and needs.get("reason") == "question":
                        session["needsUser"] = None
                    consumed = True
                else:
                    # STALE reply (answer to an older question, or our pending was
                    # superseded) — drop it so it cannot satisfy this poll, keep waiting.
                    session.pop("userReply", None)
                    consumed = True
            if consumed:
                write_agents_state(projects_dir, state)
        if matched_text is not None:
            return {"reply": matched_text}
        if time.monotonic() >= deadline:
            break
        time.sleep(ASK_USER_POLL_INTERVAL_SECS)

    # Timeout: clear our pending question + the question bell (the agent gave up).
    try:
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            modified = False
            for session in state["sessions"]:
                if session.get("agentId") != agent_id:
                    continue
                pending = session.get("pendingQuestion")
                if isinstance(pending, dict) and str(pending.get("id") or "") == question_id:
                    session.pop("pendingQuestion", None)
                    modified = True
                needs = session.get("needsUser")
                if isinstance(needs, dict) and needs.get("reason") == "question":
                    session["needsUser"] = None
                    modified = True
                break
            if modified:
                write_agents_state(projects_dir, state)
    except McpError:
        pass
    return {"timeout": True}


def handle_tool_call(
    name: str,
    arguments: dict[str, Any] | None = None,
    root: str | Path | None = None,
    projects_dir: str | Path | None = None,
) -> Any:
    args = arguments or {}
    projects_path = resolve_projects_dir(root, projects_dir)
    state_lock = projects_path / f"{AGENTS_STATE_FILE}.lock"
    projects_dir = projects_path

    if name == "agent_rules":
        return {"roles": ROLE_RULES, "tools": TOOLS}

    if name == "agent_state":
        require_agent_tool(projects_path, args, name)
        with file_lock(state_lock):
            return public_agents_state(read_agents_state(projects_path))

    if name == "agent_register":
        role = normalize_role(args.get("role", ""))
        agent_id = normalize_agent_id(args.get("agent_id"))
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            existing = validate_launch_token_for_registration(
                state,
                agent_id,
                role,
                args.get("launch_token"),
            )
            if existing is not None:
                # SEC#7: if a launch-token hash was present (managed launch), this
                # register CONSUMES it — stamp launchConsumedAt so a later
                # tokenless re-register is rejected (validate_launch_token_for_
                # registration), not silently re-issued a fresh session token.
                if str(existing.get("launchTokenHash") or "").strip():
                    existing["launchConsumedAt"] = now()
                existing.pop("launchTokenHash", None)
                existing.pop("launchTokenIssuedAt", None)
            # A managed registration is one backed by an app-issued launch/session
            # (existing pending session) OR any registration when the unmanaged
            # compat kill switch is off. Only these get a session token hash that
            # is then enforced on every subsequent call. Pure self-registration via
            # the compat kill switch stays tokenless so the per-call enforcement can
            # legitimately allow it; the kill switch covers REGISTRATION only.
            managed_registration = existing is not None or not unmanaged_privileged_agents_allowed()
            session_token = generate_session_token() if managed_registration else ""
            upsert_session(
                state,
                agent_id=agent_id,
                role=role,
                model=args.get("model"),
                status="active",
                message=args.get("message") or "registered",
                client=(args.get("client") or "") or None,
            )
            session = next(item for item in state["sessions"] if item.get("agentId") == agent_id)
            if managed_registration:
                session["sessionTokenHash"] = hash_session_token(session_token)
                session["sessionTokenIssuedAt"] = now()
            else:
                session.pop("sessionTokenHash", None)
                session.pop("sessionTokenIssuedAt", None)
            add_event(state, agent_id, role, "register", args.get("message") or "Agent registered.")
            # Soft signal: the fleet UI groups agents by model x role, so a blank
            # model leaves a gap. We still register the agent (model is optional),
            # but record a non-fatal event so the omission is visible.
            if not normalize_model(args.get("model")):
                add_event(
                    state,
                    agent_id,
                    role,
                    "register_incomplete",
                    "Agent registered without reporting a model; declare `model` at agent_register.",
                )
            return public_agents_state(
                write_agents_state(projects_dir, state),
                session_token=session_token or None,
            )

    if name == "agent_heartbeat":
        agent_id = normalize_agent_id(args.get("agent_id"))
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            session = next((item for item in state["sessions"] if item.get("agentId") == agent_id), None)
            if session is None:
                raise McpError("Agent must call agent_register before heartbeat.")
            if str(session.get("status") or "").strip().lower() == "launch_pending":
                raise McpError("Agent launch is pending. Call agent_register before heartbeat.")
            role = normalize_role(session.get("role", ""))
            if "agent_heartbeat" not in ROLE_ALLOWED_TOOLS.get(role, set()):
                raise McpError(f"{role} agents cannot use agent_heartbeat.")
            require_session_token(session, args.get("session_token"))
            # OPTIONAL `subagents`: distinguish ABSENT (key not in args -> leave the
            # stored breakdown untouched) from a provided list, INCLUDING [] which
            # explicitly clears it. We pass the _UNSET sentinel when the key is
            # missing so upsert_session can tell the two apart.
            subagents_arg = args.get("subagents")
            if subagents_arg is None:
                subagents_arg = _UNSET
            upsert_session(
                state,
                agent_id=agent_id,
                role=role,
                status=args.get("status") or "active",
                message=args.get("message"),
                # OPTIONAL: the file the agent is currently working on. Accept
                # either `file_path` or `current_file_path` for ergonomics; when
                # absent/blank the session's currentFilePath is left untouched
                # (backward-compatible — agents that never set it are unaffected).
                file_path=args.get("file_path") or args.get("current_file_path"),
                subagents=subagents_arg,
            )
            return public_agents_state(write_agents_state(projects_dir, state))

    if name == "spawn_mini_coder":
        return dispatch_spawn_mini_coder(projects_dir, state_lock, args)

    if name == "steer_mini_coder":
        return dispatch_steer_mini_coder(projects_dir, state_lock, args)

    if name == "visual_check":
        return dispatch_visual_check(projects_dir, state_lock, args)

    if name == "request_git_push":
        return dispatch_request_git_push(projects_dir, state_lock, args)

    if name == "plan_submit":
        return dispatch_plan_submit(projects_dir, state_lock, args)

    if name == "plan_status":
        return dispatch_plan_status(projects_dir, state_lock, args)

    if name == "ask_user":
        return dispatch_ask_user(projects_dir, state_lock, args)

    if name == "project_structure":
        return dispatch_project_structure(projects_dir, state_lock, args)

    if name == "project_list":
        agent_id, role = require_agent_tool(projects_dir, args, name)
        projects = []
        for path in projects_dir.glob("*.md"):
            try:
                with file_lock(path.with_suffix(path.suffix + ".lock")):
                    projects.append(summarize_project(read_project_file(path)))
            except Exception as exc:
                projects.append({"id": path.stem, "path": str(path), "error": str(exc)})
        projects.sort(key=lambda item: (item.get("updatedAt") or "", item.get("title") or ""), reverse=True)
        audit_agent_read(projects_dir, state_lock, agent_id, role, "project_read", f"Listed {len(projects)} projects.")
        return {"projectsDir": str(projects_dir), "projects": projects}

    if name == "project_get":
        agent_id, role = require_agent_tool(projects_dir, args, name)
        project = load_project_locked(projects_dir, args.get("project_id", ""))
        audit_agent_read(
            projects_dir,
            state_lock,
            agent_id,
            role,
            "project_read",
            f"Read project {project['metadata']['id']}.",
            project["metadata"]["id"],
        )
        return public_project(project)

    if name == "project_next_task":
        agent_id, role = require_agent_tool(projects_dir, args, name)
        project = load_project_locked(projects_dir, args.get("project_id", ""))
        tasks = project["state"].get("tasks", [])
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            claimed_by_others = {
                claim.get("taskId")
                for claim in state.get("claims", [])
                if claim.get("projectId") == project["metadata"]["id"]
                and claim.get("agentId") != agent_id
                and claim_is_active(claim)
            }
        # WARNING 4/6: only {coder, orchestrator, verifier} reach here — `role` comes
        # from require_agent_tool -> normalize_role, which returns "orchestrator" as a
        # first-class role (no longer an alias). Orchestrator and coder fall into the
        # same `else` branch by design (same Kanban semantics); the runtime behavior is
        # unchanged, only the comment was previously wrong about the normalized roles.
        if role == "verifier":
            preferred = ["review", "blocked"]
        else:
            preferred = ["todo", "wip", "blocked"]
        for status in preferred:
            for task in tasks:
                if task.get("id") in claimed_by_others:
                    continue
                if task.get("status") == status:
                    audit_agent_read(
                        projects_dir,
                        state_lock,
                        agent_id,
                        role,
                        "project_next",
                        f"Selected next task {task.get('id')}.",
                        project["metadata"]["id"],
                        task.get("id"),
                    )
                    return {"project": summarize_project(project), "task": task}
        audit_agent_read(
            projects_dir,
            state_lock,
            agent_id,
            role,
            "project_next",
            "No next task available.",
            project["metadata"]["id"],
        )
        return {"project": summarize_project(project), "task": None}

    if name == "project_claim_task":
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        project_id = normalize_project_id(args.get("project_id", ""))
        task_id = normalize_task_id(args.get("task_id", ""))
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            existing_claim = active_claim_for_task(state, project_id, task_id)
            if existing_claim and existing_claim.get("agentId") != agent_id:
                raise McpError(
                    f"Task is already claimed by {existing_claim.get('agentId')} until {existing_claim.get('leaseUntil')}."
                )
            with file_lock(project_lock_path(projects_dir, project_id)):
                project = read_project_file(project_path(projects_dir, project_id))
                if project["metadata"].get("status") in {"paused", "archived", "done"}:
                    raise McpError("Cannot claim tasks on paused, done or archived projects.")
                task = next((item for item in project["state"].get("tasks", []) if item.get("id") == task_id), None)
                if not task:
                    raise McpError("Task not found.")
                task_status = str(task.get("status") or "")
                if task_status == "done":
                    raise McpError("Done tasks cannot be claimed.")
                if role == "verifier" and task_status not in {"review", "blocked"}:
                    raise McpError("Verifier agents can only claim review or blocked tasks.")
                # Orchestrator shares the coder's claim semantics (CODER_LIKE_ROLES):
                # it may claim todo/wip/blocked and auto-advances a todo to wip. It is
                # NOT a verifier, so it can never claim a review task — tighter-or-equal.
                if role in CODER_LIKE_ROLES and task_status not in {"todo", "wip", "blocked"}:
                    raise McpError("Coder agents can only claim todo, wip or blocked tasks.")
                if role in CODER_LIKE_ROLES and task_status == "todo":
                    task["status"] = "wip"
                    task["updatedAt"] = now()
                    project["metadata"]["status"] = "active"
                    project["metadata"]["updatedAt"] = now()
                    project["state"].setdefault("notes", []).append(
                        {
                            "id": note_id(),
                            "text": f"{agent_id} ({role}) claimed {task_id} and moved it to wip.",
                            "source": f"agent:{agent_id}",
                            "createdAt": now(),
                        }
                    )
                    project = write_project_file(project)
            lease_until = (datetime.now(timezone.utc) + timedelta(minutes=45)).isoformat()
            claim_status = "wip" if role in CODER_LIKE_ROLES and task.get("status") == "wip" else "claimed"
            state["claims"] = [
                item
                for item in state["claims"]
                if not (item.get("projectId") == project_id and item.get("taskId") == task_id)
            ]
            state["claims"].append(
                {
                    "projectId": project_id,
                    "projectTitle": project["metadata"]["title"],
                    "taskId": task_id,
                    "taskTitle": task.get("title"),
                    "agentId": agent_id,
                    "role": role,
                    "status": claim_status,
                    "claimedAt": now(),
                    "updatedAt": now(),
                    "leaseUntil": lease_until,
                }
            )
            upsert_session(state, agent_id, role, status=claim_status, project_id=project_id, task_id=task_id)
            add_event(
                state,
                agent_id,
                role,
                "claim",
                f"Claimed {task_id}." if claim_status == "claimed" else f"Claimed {task_id} and moved it to wip.",
                project_id,
                task_id,
                task.get("status"),
            )
            return public_agents_state(write_agents_state(projects_dir, state))

    if name == "project_update_status":
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        project_id = normalize_project_id(args.get("project_id", ""))
        task_id = normalize_task_id(args.get("task_id", ""))
        status = normalize_task_status(args.get("status", ""))
        evidence = str(args.get("evidence") or "").strip()
        confidence = float(args.get("confidence") or 0)

        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            require_claim_for_status_update(state, agent_id, role, project_id, task_id, target_status=status)

            with file_lock(project_lock_path(projects_dir, project_id)):
                project = read_project_file(project_path(projects_dir, project_id))
                if project["metadata"].get("status") in {"paused", "archived"}:
                    raise McpError("Cannot update tasks on paused or archived projects.")
                task = next((item for item in project["state"].get("tasks", []) if item.get("id") == task_id), None)
                if not task:
                    raise McpError("Task not found.")
                validate_transition(role, status, evidence, confidence, task.get("status"))
                task["status"] = status
                task["updatedAt"] = now()
                note_text = f"{agent_id} ({role}) set {task_id} to {status}."
                if evidence:
                    note_text = f"{note_text} Evidence: {evidence[:1200]}"
                project["state"].setdefault("notes", []).append(
                    {
                        "id": note_id(),
                        "text": note_text,
                        "source": f"agent:{agent_id}",
                        "createdAt": now(),
                    }
                )
                if all(item.get("status") == "done" for item in project["state"].get("tasks", [])):
                    project["metadata"]["status"] = "done"
                elif project["metadata"].get("status") == "done" and status != "done":
                    project["metadata"]["status"] = "active"
                project["metadata"]["updatedAt"] = now()
                saved = write_project_file(project)

            upsert_session(state, agent_id, role, status=status, message=evidence or f"{task_id} -> {status}", project_id=project_id, task_id=task_id)
            for claim in state["claims"]:
                if (
                    claim.get("projectId") == project_id
                    and claim.get("taskId") == task_id
                    and claim.get("agentId") == agent_id
                    and normalize_role(claim.get("role", "")) == role
                ):
                    claim["status"] = status
                    claim["updatedAt"] = now()
                    claim["evidence"] = evidence or claim.get("evidence")
            add_event(state, agent_id, role, "status", f"{task_id} -> {status}", project_id, task_id, status, evidence)
            write_agents_state(projects_dir, state)

        return public_project(saved)

    if name == "project_append_note":
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        project_id = normalize_project_id(args.get("project_id", ""))
        text = clean_text(args.get("text"), "Note", 4000)
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            with file_lock(project_lock_path(projects_dir, project_id)):
                project = read_project_file(project_path(projects_dir, project_id))
                project["state"].setdefault("notes", []).append(
                    {
                        "id": note_id(),
                        "text": text,
                        "source": f"agent:{agent_id}",
                        "createdAt": now(),
                    }
                )
                project["metadata"]["updatedAt"] = now()
                saved = write_project_file(project)
            upsert_session(state, agent_id, role, status="noted", message=text, project_id=project_id)
            add_event(state, agent_id, role, "note", text, project_id)
            write_agents_state(projects_dir, state)
        return public_project(saved)

    if name == "project_create_followup":
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        project_id = normalize_project_id(args.get("project_id", ""))
        title = clean_text(args.get("title"), "Task title", 500)
        reason = clean_text(args.get("reason"), "Reason", 2000)
        # INTENTIONAL DIVERGENCE from the UI: the desktop app makes `category`
        # MANDATORY on create (a blank value is rejected), but the agent MCP path
        # defaults an ABSENT category to "other" (see `normalize_task_category`). An
        # agent rarely knows the right bucket, and forcing it would block useful
        # follow-ups; "other" is the honest neutral default. An EXPLICIT invalid
        # value is still rejected in both paths.
        category = normalize_task_category(args.get("category"))
        # Optional free-form description, mirroring the Rust `clean_description`
        # (trim + cap 4000, newlines preserved, None when blank). P2 Oracle
        # localization uses it as part of the suspect-retrieval query.
        description = clean_description(args.get("description"))
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            with file_lock(project_lock_path(projects_dir, project_id)):
                project = read_project_file(project_path(projects_dir, project_id))
                if project["metadata"].get("status") in {"paused", "archived", "done"}:
                    raise McpError("Cannot create follow-up tasks on paused, done or archived projects.")
                tasks = project["state"].setdefault("tasks", [])
                task = {
                    "id": next_task_id(tasks),
                    "title": title,
                    "status": "todo",
                    "priority": "medium",
                    "assignee": None,
                    "due": None,
                    "linkedResources": [],
                    "updatedAt": now(),
                    # P1: categorize agent-created follow-ups. suspectFileIds is
                    # populated by Oracle localization in P2; empty for now.
                    "category": category,
                    "suspectFileIds": [],
                }
                # Match the Rust serde shape: `description` is `Option<String>`, so a
                # None description is OMITTED from the dict (serde-default loads it
                # back as None) rather than written as an explicit null.
                if description is not None:
                    task["description"] = description
                tasks.append(task)
                project["state"].setdefault("notes", []).append(
                    {
                        "id": note_id(),
                        "text": f"Follow-up created by {agent_id} ({role}): {reason}",
                        "source": f"agent:{agent_id}",
                        "createdAt": now(),
                    }
                )
                project["metadata"]["status"] = "active"
                project["metadata"]["updatedAt"] = now()
                saved = write_project_file(project)
            upsert_session(state, agent_id, role, status="followup", message=title, project_id=project_id, task_id=task["id"])
            add_event(state, agent_id, role, "followup", reason, project_id, task["id"], "todo")
            write_agents_state(projects_dir, state)
        return {"project": public_project(saved), "task": task}

    if name == "project_create_plan_tasks":
        # Phase 11.5-B (Piece 1a): bulk-create an approved plan's tasks ON the project
        # Kanban (the single shared task store). Authn/authz mirrors the other
        # project_* write tools; gated to the orchestrator (devboule-coder) via
        # ROLE_ALLOWED_TOOLS. The planner sends its OWN internal ids in id/dependsOn;
        # we allocate FRESH T<n> ids (no collision with manual tasks) and remap deps.
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        project_id = normalize_project_id(args.get("project_id", ""))
        plan_id = clean_text(args.get("plan_id"), "Plan id", 200)
        incoming = args.get("tasks")
        if not isinstance(incoming, list) or not incoming:
            raise McpError("project_create_plan_tasks requires a non-empty tasks list.")
        if len(incoming) > MAX_PLAN_TASKS:
            raise McpError(f"Too many plan tasks: {len(incoming)} (max {MAX_PLAN_TASKS}).")

        # --- Pre-lock validation of the INCOMING (planner-internal) shape. The plan
        # must be SELF-CONTAINED: every dependsOn references an id that is also in
        # this batch (a plan cannot depend on a manual/other task). We validate the
        # incoming graph here so a malformed plan is rejected BEFORE we take the lock
        # or allocate any id. ---
        seen_incoming: set[str] = set()
        parsed: list[dict[str, Any]] = []
        for entry in incoming:
            if not isinstance(entry, dict):
                raise McpError("Each plan task must be an object.")
            internal_id = normalize_task_id(entry.get("id", ""))
            if internal_id in seen_incoming:
                raise McpError(f"Duplicate plan task id in request: {internal_id}.")
            seen_incoming.add(internal_id)
            title = clean_text(entry.get("title"), "Task title", 500)
            # Sanitize like every other user-facing string (acceptance MAY be empty, so
            # we cannot use clean_text which rejects empties): strip invisible/BiDi control
            # chars so a U+202E-obfuscated acceptance can't survive into the field piece 1b
            # will EXECUTE + the human reads in the project markdown.
            acceptance = strip_invisible_and_bidi(str(entry.get("acceptance") or "")).strip()[:4000]
            raw_scope = entry.get("scope", [])
            if not isinstance(raw_scope, list) or not all(isinstance(s, str) for s in raw_scope):
                raise McpError("Plan task scope must be a list of file paths.")
            if len(raw_scope) > MAX_PLAN_TASK_SCOPE:
                raise McpError(
                    f"Plan task {internal_id} scope has {len(raw_scope)} files (max {MAX_PLAN_TASK_SCOPE})."
                )
            scope = [validate_plan_scope_path(s) for s in raw_scope]
            raw_deps = entry.get("dependsOn", [])
            if not isinstance(raw_deps, list) or not all(isinstance(d, str) for d in raw_deps):
                raise McpError("Plan task dependsOn must be a list of task ids.")
            deps = [normalize_task_id(d) for d in raw_deps]
            parsed.append(
                {
                    "internal_id": internal_id,
                    "title": title,
                    "acceptance": acceptance,
                    "scope": scope,
                    "deps": deps,
                }
            )
        # A plan is self-contained: a dependsOn must reference an id PRESENT in this
        # batch (reject before allocating ids — the planner's ids are not yet remapped).
        for entry in parsed:
            for dep in entry["deps"]:
                if dep not in seen_incoming:
                    raise McpError(
                        f"Plan task {entry['internal_id']} dependsOn references id {dep} not in the request."
                    )

        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            with file_lock(project_lock_path(projects_dir, project_id)):
                project = read_project_file(project_path(projects_dir, project_id))
                if project["metadata"].get("status") in {"paused", "archived", "done"}:
                    raise McpError("Cannot create plan tasks on paused, done or archived projects.")
                tasks = project["state"].setdefault("tasks", [])
                # Allocate a FRESH T<n> per incoming task. next_task_id derives the
                # next free id from the CURRENT tasks; append each allocated task to a
                # working list as we go so the next allocation sees it and we never
                # collide within this batch OR with existing manual tasks.
                id_map: dict[str, str] = {}
                allocated: list[dict[str, Any]] = []
                for entry in parsed:
                    new_id = next_task_id(tasks + allocated)
                    id_map[entry["internal_id"]] = new_id
                    allocated.append({"id": new_id})
                # Build the final tasks with deps REMAPPED through id_map. Every dep is
                # guaranteed present in id_map by the self-contained check above.
                created: list[dict[str, Any]] = []
                created_deps_by_id: dict[str, list[str]] = {}
                ts = now()
                for entry, holder in zip(parsed, allocated):
                    new_id = holder["id"]
                    remapped_deps = [id_map[d] for d in entry["deps"]]
                    task = {
                        "id": new_id,
                        "title": entry["title"],
                        "status": "todo",
                        "priority": "medium",
                        "assignee": None,
                        "due": None,
                        "linkedResources": [],
                        "updatedAt": ts,
                        # Plan provenance: planId tags the task so the runner (piece 1b)
                        # knows to auto-execute it; scope/acceptance/dependsOn carry the
                        # mini's write allowlist, the acceptance check, and the DAG.
                        "planId": plan_id,
                    }
                    # OMIT the new fields WHEN EMPTY, mirroring the Rust struct's
                    # `skip_serializing_if` (Vec::is_empty / String::is_empty). Otherwise
                    # Python writes `"scope":[]`/`"acceptance":""`/`"dependsOn":[]` and the
                    # next RUST re-serialize (e.g. a UI task edit) drops them → the content
                    # hash changes → spurious git-dirty + Oracle re-index. validate_project_state
                    # handles their absence (its guards are `"x" in task` / `.get(...,[])`).
                    if entry["scope"]:
                        task["scope"] = entry["scope"]
                    if entry["acceptance"]:
                        task["acceptance"] = entry["acceptance"]
                    if remapped_deps:
                        task["dependsOn"] = remapped_deps
                    created.append(task)
                    created_deps_by_id[new_id] = remapped_deps
                # Validate the REMAPPED DAG (within this batch) is acyclic BEFORE
                # writing — reuse the exact Kahn check used on project load.
                validate_task_dependency_dag(created_deps_by_id)
                tasks.extend(created)
                # The full state must still validate as a whole (e.g. the new tasks'
                # deps reference only ids that now exist in the project). This also
                # re-runs the global DAG check across manual + plan tasks together.
                validate_project_state(project["state"])
                project["state"].setdefault("notes", []).append(
                    {
                        "id": note_id(),
                        "text": f"{agent_id} ({role}) created {len(created)} task(s) from plan {plan_id}.",
                        "source": f"agent:{agent_id}",
                        "createdAt": ts,
                    }
                )
                project["metadata"]["status"] = "active"
                project["metadata"]["updatedAt"] = now()
                saved = write_project_file(project)
            upsert_session(
                state,
                agent_id,
                role,
                status="plan_tasks",
                message=f"Created {len(created)} plan task(s).",
                project_id=project_id,
            )
            add_event(
                state,
                agent_id,
                role,
                "plan_tasks",
                f"Created {len(created)} task(s) from plan {plan_id}.",
                project_id,
            )
            write_agents_state(projects_dir, state)
        return {
            "project": public_project(saved),
            "planId": plan_id,
            # old (planner-internal) id -> new (allocated Kanban) id, so 1b can wire
            # the runner to the freshly-created board tasks.
            "idMap": id_map,
            "tasks": created,
        }

    if name == "provider_credentials_status":
        agent_id, role = require_agent_tool(projects_dir, args, name)
        result = provider_credentials_status()
        audit_agent_read(
            projects_dir,
            state_lock,
            agent_id,
            role,
            "provider_credentials_status",
            "Read provider credential readiness without secrets.",
        )
        return result

    if name == "cloudflare_list_workers":
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        token = cloudflare_token_from_sources(
            *CF_TOKEN_ENVS,
            *CF_READONLY_TOKEN_ENVS,
            *CF_CODER_TOKEN_ENVS,
        )
        result = cloudflare_list_workers(token, args.get("account_id") or None)
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            upsert_session(state, agent_id, role, status="cloudflare-read", message="Read Cloudflare Workers inventory.")
            add_event(state, agent_id, role, "cloudflare_read", f"Read {len(result['workers'])} Cloudflare Workers.")
            write_agents_state(projects_dir, state)
        return result

    if name == "cloudflare_rotate_worker_secret":
        agent_id, role, management_project_id, task_id, evidence = require_provider_mutation_context(
            projects_dir,
            state_lock,
            args,
            name,
        )
        token = cloudflare_token_from_sources(
            *CF_SECRET_ROTATOR_TOKEN_ENVS,
            *CF_CODER_TOKEN_ENVS,
            *CF_TOKEN_ENVS,
        )
        reserve_provider_mutation(
            projects_dir,
            state_lock,
            agent_id,
            role,
            management_project_id,
            task_id,
            name,
            evidence,
        )
        try:
            result = cloudflare_rotate_secret(
                token=token,
                account_id=args.get("account_id") or None,
                worker_name=args.get("worker_name", ""),
                secret_name=args.get("secret_name", ""),
                secret_value=args.get("secret_value", ""),
            )
        except Exception as exc:
            release_provider_mutation_reservation(
                projects_dir,
                state_lock,
                agent_id,
                role,
                management_project_id,
                task_id,
                name,
                str(exc)[:240],
            )
            raise
        message = f"Rotated Worker secret {result['secretName']} on {result['workerName']}."
        record_provider_mutation(
            projects_dir,
            state_lock,
            agent_id,
            role,
            management_project_id,
            task_id,
            "cloudflare_secret",
            message,
            evidence,
        )
        result.update({"managementProjectId": management_project_id, "taskId": task_id})
        return result

    if name == "scaleway_list_resources":
        agent_id = normalize_agent_id(args.get("agent_id"))
        role = require_registered_role(projects_dir, agent_id, args.get("role", ""), name, args.get("session_token"))
        token = provider_token_from_sources(
            "scaleway_token",
            *SCW_TOKEN_ENVS,
        )
        result = scaleway_list_resources(token, args.get("project_id") or None)
        with file_lock(state_lock):
            state = read_agents_state(projects_dir)
            upsert_session(state, agent_id, role, status="scaleway-read", message="Read Scaleway Aspis Bio inventory.")
            add_event(state, agent_id, role, "scaleway_read", f"Read {len(result['resources'])} Scaleway resources.")
            write_agents_state(projects_dir, state)
        return result

    if name == "scaleway_resource_action":
        agent_id, role, management_project_id, task_id, evidence = require_provider_mutation_context(
            projects_dir,
            state_lock,
            args,
            name,
        )
        token = provider_token_from_sources(
            "scaleway_token",
            *SCW_TOKEN_ENVS,
        )
        reserve_provider_mutation(
            projects_dir,
            state_lock,
            agent_id,
            role,
            management_project_id,
            task_id,
            name,
            evidence,
        )
        try:
            result = scaleway_resource_action(
                token=token,
                resource_id=args.get("resource_id", ""),
                action=args.get("action", ""),
                confirm_resource_name=args.get("confirm_resource_name") or None,
                project_id=args.get("scaleway_project_id") or args.get("provider_project_id") or args.get("project_id") or None,
            )
        except Exception as exc:
            release_provider_mutation_reservation(
                projects_dir,
                state_lock,
                agent_id,
                role,
                management_project_id,
                task_id,
                name,
                str(exc)[:240],
            )
            raise
        message = f"{result['action']} {result['resourceName'] or result['resourceId']}."
        record_provider_mutation(
            projects_dir,
            state_lock,
            agent_id,
            role,
            management_project_id,
            task_id,
            "scaleway_action",
            message,
            evidence,
        )
        result.update({"managementProjectId": management_project_id, "taskId": task_id})
        return result

    if name == "oracle_ask":
        agent_id, role = require_agent_tool(projects_dir, args, name)
        # FIX 4: dispatch owns readiness. On the HTTP path the resident server
        # is authoritative (LOCAL gate skipped); only the in-process / fallback
        # path runs the fail-closed LOCAL `ensure_oracle_index_ready`. The scope
        # is computed locally and forwarded; the resident server never widens it.
        result, index_status = dispatch_oracle_ask(
            projects_dir,
            clean_text(args.get("query"), "Query", 2000),
            int(args.get("limit", 5)),
            oracle_allowed_file_ids(projects_dir, args),
            args=args,
        )
        result["index_status"] = {
            "root": _safe_index_root(index_status.get("root")),
            "indexedFiles": index_status.get("indexed_files"),
            "pendingFiles": index_status.get("pending_files"),
            "staleFiles": index_status.get("stale_files"),
        }
        audit_agent_read(projects_dir, state_lock, agent_id, role, "oracle_ask", clean_text(args.get("query"), "Query", 2000), args.get("project_id") or None)
        return result

    if name == "oracle_context":
        mcp_debug(projects_dir, "oracle_context begin")
        agent_id, role = require_agent_tool(projects_dir, args, name)
        mcp_debug(projects_dir, "oracle_context agent ok")
        # SEC#9: the mini's read-only grant is SCOPED to its spawning project.
        # Its session carries a reliable currentProjectId (set by the Rust
        # upsert_mini_session at launch — unlike a coder, which only stamps it
        # when it touches a project tool). A mini asking for a DIFFERENT
        # project_id is a cross-project corpus read — reject it. An empty
        # project_id is fine (defaults to the management root only).
        enforce_mini_oracle_project_scope(projects_dir, agent_id, role, args)
        query = clean_text(args.get("query"), "Query", 2000)
        mcp_debug(projects_dir, "oracle_context index begin")
        audit_agent_read(projects_dir, state_lock, agent_id, role, "oracle_context", query, args.get("project_id") or None)
        mcp_debug(projects_dir, "oracle_context audit ok")
        # FIX 4: dispatch owns readiness (HTTP path trusts the resident server;
        # only the in-process / fallback path runs the LOCAL fail-closed gate).
        chunks, index_status = dispatch_oracle_context(
            projects_dir,
            query,
            int(args.get("limit", 8)),
            oracle_allowed_file_ids(projects_dir, args),
            args=args,
        )
        mcp_debug(projects_dir, f"oracle_context chunks ok count={len(chunks)}")
        return {
            "query": query,
            "indexStatus": {
                "root": _safe_index_root(index_status.get("root")),
                "indexedFiles": index_status.get("indexed_files"),
                "pendingFiles": index_status.get("pending_files"),
                "staleFiles": index_status.get("stale_files"),
            },
            "chunks": chunks,
        }

    if name == "censor_findings":
        # Both roles (coder per-step, verifier residual) consume the ledger. The
        # role-allowed-tools gate + session-token enforcement run inside
        # require_agent_tool; an unauthorized/unregistered caller never reaches the
        # shards.
        agent_id, role = require_agent_tool(projects_path, args, name)
        project_id = normalize_project_id(str(args.get("project_id") or "").strip())
        if not project_id:
            raise McpError("project_id is required.")
        file_arg = str(args.get("file") or "").strip() or None
        if file_arg is not None:
            validate_censor_rel_path(file_arg)
        work_root = resolve_project_work_root(projects_path, project_id)
        findings = read_censor_open_findings(work_root, file_arg)
        # Audit the read (identity only — never shard contents, which is why we log
        # just the count, mirroring the privacy posture of the other read tools).
        audit_agent_read(
            projects_path,
            state_lock,
            agent_id,
            role,
            "censor_findings",
            f"Read {len(findings)} open Censor finding(s)"
            + (f" for {file_arg}" if file_arg else ""),
            project_id,
        )
        return {"projectId": project_id, "file": file_arg, "findings": findings}

    if name == "censor_dispose":
        agent_id, role = require_agent_tool(projects_path, args, name)
        project_id = normalize_project_id(str(args.get("project_id") or "").strip())
        if not project_id:
            raise McpError("project_id is required.")
        file_arg = str(args.get("file") or "").strip()
        validate_censor_rel_path(file_arg)
        finding_id = str(args.get("id") or "").strip()
        if not finding_id:
            raise McpError("Finding id is required.")
        disposition = str(args.get("disposition") or "").strip()
        work_root = resolve_project_work_root(projects_path, project_id)
        dispose_censor_finding(
            work_root,
            file_arg,
            finding_id,
            disposition,
            agent_id,
            now(),
            role,
        )
        audit_agent_read(
            projects_path,
            state_lock,
            agent_id,
            role,
            "censor_dispose",
            f"Disposed Censor finding {finding_id} as {disposition} in {file_arg}",
            project_id,
        )
        # B3 — DO NOT echo the finding body/title/content back to the agent (it
        # already had every safe field from `censor_findings`); return only the
        # identity + applied disposition to eliminate the extra egress surface.
        return {
            "projectId": project_id,
            "file": file_arg,
            "id": finding_id,
            "disposition": disposition,
            "ok": True,
        }

    raise McpError(f"Unknown Aspis MCP tool: {name}")


def create_mcp_server(root: str | Path | None = None, projects_dir: str | Path | None = None):
    try:
        from mcp.server.fastmcp import FastMCP
    except Exception as exc:  # pragma: no cover
        raise RuntimeError("Install oracle/requirements.txt to run the Aspis MCP server.") from exc

    server = FastMCP("aspis-management")

    def call(name: str, arguments: dict[str, Any] | None = None) -> Any:
        return handle_tool_call(name, arguments, root=root, projects_dir=projects_dir)

    @server.tool()
    def agent_rules() -> dict:
        """Practical roles, responsibilities and prohibitions for Aspis agents."""
        return call("agent_rules")

    @server.tool()
    def agent_state(agent_id: str, role: str, session_token: str = "") -> dict:
        """Live state of agent sessions, claims and recent events."""
        return call("agent_state", {"agent_id": agent_id, "role": role, "session_token": session_token})

    @server.tool()
    def agent_register(agent_id: str, role: str, model: str = "", client: str = "", message: str = "", launch_token: str = "") -> dict:
        """Register a CLI agent in the live dashboard."""
        return call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": role,
                "model": model,
                "client": client,
                "message": message,
                "launch_token": launch_token,
            },
        )

    @server.tool()
    def agent_heartbeat(
        agent_id: str,
        status: str = "active",
        message: str = "",
        session_token: str = "",
        file_path: str = "",
        subagents: list | None = None,
    ) -> dict:
        """Update the agent's live presence in the dashboard.

        Pass `file_path` (absolute, project-relative, or scanned-folder-relative)
        to place the agent on the EXACT file's building in Polis; omit it to leave
        the current file untouched.

        Pass `subagents` (a list of {label, model, count, role?}) to report the
        current subagent breakdown for the fleet UI; omit it (None) to leave the
        stored breakdown untouched; pass [] to clear it. The handler's sentinel
        logic treats None as "untouched", so forward it verbatim.
        """
        return call(
            "agent_heartbeat",
            {
                "agent_id": agent_id,
                "status": status,
                "message": message,
                "session_token": session_token,
                "file_path": file_path,
                "subagents": subagents,
            },
        )

    @server.tool()
    def spawn_mini_coder(
        agent_id: str,
        role: str,
        task: str,
        files: list,
        backend: str = "",
        allow_oracle: bool = False,
        write: bool = False,
        session_token: str = "",
    ) -> dict:
        """Coder-only: delegate a cheap, well-scoped sub-task to a one-shot mini-coder
        the app hosts as a tracked terminal.

        Use it for boilerplate, bulk-read->summary, simple edits, docstrings or
        tests — NOT for anything needing judgement you should do yourself. `files`
        is a non-empty list of PROJECT-RELATIVE paths the mini may touch. `backend`
        optionally overrides the configured mini backend.

        BLOCKS until the mini finishes and returns its terminal result:
          - done -> accept/verify its output + filesTouched;
          - needs_clarification -> re-invoke with the answer, or do it yourself;
          - aborted_by_human -> STOP and escalate to the human (never silently
            retry); the mini never contacts the human, you are the only contact point;
          - failed/timeout -> handle as an error.
        """
        return call(
            "spawn_mini_coder",
            {
                "agent_id": agent_id,
                "role": role,
                "task": task,
                "files": files,
                "backend": backend,
                "allow_oracle": allow_oracle,
                "write": write,
                "session_token": session_token,
            },
        )

    @server.tool()
    def steer_mini_coder(
        agent_id: str,
        role: str,
        directive_id: str,
        message: str,
        session_token: str = "",
    ) -> dict:
        """Coder/orchestrator-only: steer a RUNNING mini-coder you spawned.

        Append a mid-flight correction to the mini's steer queue; the app folds queued
        corrections into the mini's NEXT fix-pass round (it takes effect at a round
        boundary, not mid-token), reusing the same channel as the Stop button. Send the
        message 'stop' to ABORT the mini (it maps to the kill path). Pass the
        `directive_id` returned by `spawn_mini_coder`.

        Returns `{directiveId, status}` where status is queued (+ queued length),
        stopped, queue_full (the FIFO is full — your message was refused, not dropped),
        not_found, or terminal (the mini already finished).
        """
        return call(
            "steer_mini_coder",
            {
                "agent_id": agent_id,
                "role": role,
                "directive_id": directive_id,
                "message": message,
                "session_token": session_token,
            },
        )

    @server.tool()
    def visual_check(
        agent_id: str,
        role: str,
        html_path: str,
        focus: str = "",
        session_token: str = "",
    ) -> dict:
        """Render one self-contained HTML artifact through the app and return a local visual critique."""
        return call(
            "visual_check",
            {
                "agent_id": agent_id,
                "role": role,
                "html_path": html_path,
                "focus": focus,
                "session_token": session_token,
            },
        )

    @server.tool()
    def request_git_push(
        agent_id: str,
        role: str,
        project_id: str,
        branch: str = "",
        remote: str = "",
        force: bool = False,
        session_token: str = "",
    ) -> dict:
        """Coder-only: REQUEST human approval to git push (you may COMMIT freely, but
        every PUSH is approved by the human).

        BLOCKS until the human acts and returns the terminal result:
          - pushed -> the human approved and the app performed the push (off-argv
            credential); continue;
          - push_failed -> the push ran but git failed (e.g. non-fast-forward);
            read the (sanitized) error and decide;
          - denied -> the human declined; do NOT push, do NOT retry — escalate;
          - timeout -> the human did not respond in time; STOP this line of work, do
            NOT retry, and NEVER push directly.

        `project_id` is the project whose repo to push. `branch` is informational
        (the push targets the repo's current HEAD). `remote` defaults to origin.
        `force` requests a force-push (the human sees a FORCE warning and must still
        approve it). NEVER attempt a raw `git push` yourself — it is the human's call.
        """
        return call(
            "request_git_push",
            {
                "agent_id": agent_id,
                "role": role,
                "project_id": project_id,
                "branch": branch,
                "remote": remote,
                "force": force,
                "session_token": session_token,
            },
        )

    @server.tool()
    def plan_submit(
        agent_id: str,
        role: str,
        project_id: str,
        title: str,
        plan_markdown: str,
        session_token: str = "",
    ) -> dict:
        """Coder-only: SUBMIT an implementation plan for human approval and BLOCK on the
        verdict.

        Before doing multi-file work, write your plan here. The app shows it to the human
        who APPROVES or REJECTS it. BLOCKS until the human acts and returns:
          - approved -> proceed with implementation (an optional decider `note` may refine it);
          - rejected -> revise the plan per the `note` and re-submit (do NOT proceed);
          - timeout  -> the human did not respond in time; STOP, do not implement unapproved.

        `project_id` is the project the plan targets. `title` is a short one-line label.
        `plan_markdown` is the full plan body (markdown; newlines preserved).
        """
        return call(
            "plan_submit",
            {
                "agent_id": agent_id,
                "role": role,
                "project_id": project_id,
                "title": title,
                "plan_markdown": plan_markdown,
                "session_token": session_token,
            },
        )

    @server.tool()
    def plan_status(agent_id: str, role: str, plan_id: str, session_token: str = "") -> dict:
        """Coder or verifier: read the current status of a previously submitted plan.

        Returns `{planId, status, note?}` where status is one of pending_approval /
        approved / rejected / timeout, or `not_found` if no such plan exists. `plan_id`
        is the id returned by plan_submit (32 hex characters).
        """
        return call(
            "plan_status",
            {"agent_id": agent_id, "role": role, "plan_id": plan_id, "session_token": session_token},
        )

    @server.tool()
    def ask_user(agent_id: str, role: str, question: str, session_token: str = "") -> dict:
        """Coder or verifier: ask the HUMAN a blocking question and wait for the reply.

        Use this instead of stalling in the terminal when you genuinely need a decision
        from the human. BLOCKS until the human replies (returns `{reply}`) or the wait
        times out (returns `{timeout: true}`). Ask one focused question at a time.
        """
        return call(
            "ask_user",
            {"agent_id": agent_id, "role": role, "question": question, "session_token": session_token},
        )

    @server.tool()
    def project_list(agent_id: str, role: str, session_token: str = "") -> dict:
        """List local Markdown projects."""
        return call("project_list", {"agent_id": agent_id, "role": role, "session_token": session_token})

    @server.tool()
    def project_get(project_id: str, agent_id: str, role: str, session_token: str = "") -> dict:
        """Read a project with its tasks, notes, revision and path."""
        return call("project_get", {"project_id": project_id, "agent_id": agent_id, "role": role, "session_token": session_token})

    @server.tool()
    def project_next_task(project_id: str, agent_id: str, role: str = "coder", session_token: str = "") -> dict:
        """Suggest the next incomplete task for a role."""
        return call("project_next_task", {"project_id": project_id, "agent_id": agent_id, "role": role, "session_token": session_token})

    @server.tool()
    def project_claim_task(project_id: str, task_id: str, agent_id: str, role: str, session_token: str = "") -> dict:
        """Create a lease claim on the task."""
        return call(
            "project_claim_task",
            {"project_id": project_id, "task_id": task_id, "agent_id": agent_id, "role": role, "session_token": session_token},
        )

    @server.tool()
    def project_update_status(
        project_id: str,
        task_id: str,
        status: str,
        agent_id: str,
        role: str,
        evidence: str = "",
        confidence: float = 0.0,
        session_token: str = "",
    ) -> dict:
        """Update task/project status with notes and an auditable event."""
        return call(
            "project_update_status",
            {
                "project_id": project_id,
                "task_id": task_id,
                "status": status,
                "agent_id": agent_id,
                "role": role,
                "evidence": evidence,
                "confidence": confidence,
                "session_token": session_token,
            },
        )

    @server.tool()
    def project_append_note(project_id: str, text: str, agent_id: str, role: str, session_token: str = "") -> dict:
        """Append a structured note to the project."""
        return call(
            "project_append_note",
            {"project_id": project_id, "text": text, "agent_id": agent_id, "role": role, "session_token": session_token},
        )

    @server.tool()
    def project_create_followup(project_id: str, title: str, reason: str, agent_id: str, role: str, category: str = "other", description: str = "", session_token: str = "") -> dict:
        """Create a follow-up TODO task. category: feature|hardening|bug|other (default other). description: optional free-form context (used by Oracle suspect localization)."""
        return call(
            "project_create_followup",
            {"project_id": project_id, "title": title, "reason": reason, "category": category, "description": description, "agent_id": agent_id, "role": role, "session_token": session_token},
        )

    @server.tool()
    def project_create_plan_tasks(
        project_id: str,
        plan_id: str,
        tasks: list,
        agent_id: str,
        role: str,
        session_token: str = "",
    ) -> dict:
        """Bulk-create an approved plan's tasks on the project Kanban as todo, tagged
        with planId. Each task is {id, title, scope, acceptance, dependsOn} where id
        and dependsOn use the planner's INTERNAL ids; fresh T<n> ids are allocated
        (no collision with manual tasks) and dependsOn is remapped to them. The DAG
        must be acyclic and self-contained (every dependsOn references an id in the
        request). Returns {project, planId, idMap (internal->allocated), tasks}."""
        return call(
            "project_create_plan_tasks",
            {
                "project_id": project_id,
                "plan_id": plan_id,
                "tasks": tasks,
                "agent_id": agent_id,
                "role": role,
                "session_token": session_token,
            },
        )

    @server.tool()
    def provider_credentials_status(agent_id: str, role: str, session_token: str = "") -> dict:
        """Read-only: diagnose provider/Oracle credentials without exposing secrets."""
        return call(
            "provider_credentials_status",
            {"agent_id": agent_id, "role": role, "session_token": session_token},
        )

    @server.tool()
    def cloudflare_list_workers(agent_id: str, role: str, account_id: str = "", session_token: str = "") -> dict:
        """Read-only: list Workers in the Aspis Bio Cloudflare account."""
        return call(
            "cloudflare_list_workers",
            {"agent_id": agent_id, "role": role, "account_id": account_id, "session_token": session_token},
        )

    @server.tool()
    def cloudflare_rotate_worker_secret(
        agent_id: str,
        role: str,
        worker_name: str,
        secret_name: str,
        secret_value: str,
        management_project_id: str,
        task_id: str,
        evidence: str,
        account_id: str = "",
        session_token: str = "",
    ) -> dict:
        """Coder-only: rotate a Cloudflare Worker secret from a claimed Kanban task."""
        return call(
            "cloudflare_rotate_worker_secret",
            {
                "agent_id": agent_id,
                "role": role,
                "account_id": account_id,
                "worker_name": worker_name,
                "secret_name": secret_name,
                "secret_value": secret_value,
                "management_project_id": management_project_id,
                "task_id": task_id,
                "evidence": evidence,
                "session_token": session_token,
            },
        )

    @server.tool()
    def scaleway_list_resources(agent_id: str, role: str, project_id: str = "", session_token: str = "") -> dict:
        """Read-only: list VMs and serverless resources in the Aspis Bio Scaleway project."""
        return call(
            "scaleway_list_resources",
            {"agent_id": agent_id, "role": role, "project_id": project_id, "session_token": session_token},
        )

    @server.tool()
    def scaleway_resource_action(
        agent_id: str,
        role: str,
        resource_id: str,
        action: str,
        management_project_id: str,
        task_id: str,
        evidence: str,
        confirm_resource_name: str = "",
        project_id: str = "",
        scaleway_project_id: str = "",
        session_token: str = "",
    ) -> dict:
        """Coder-only: start/stop/reboot/terminate a VM or deploy serverless from a claimed Kanban task."""
        return call(
            "scaleway_resource_action",
            {
                "agent_id": agent_id,
                "role": role,
                "resource_id": resource_id,
                "action": action,
                "confirm_resource_name": confirm_resource_name,
                "project_id": project_id,
                "scaleway_project_id": scaleway_project_id,
                "management_project_id": management_project_id,
                "task_id": task_id,
                "evidence": evidence,
                "session_token": session_token,
            },
        )

    @server.tool()
    def oracle_ask(query: str, agent_id: str, role: str, limit: int = 5, project_id: str = "", session_token: str = "") -> dict:
        """Ask the Oracle about the project's architecture/codebase.

        TIP: ask PRECISE, single-subsystem questions. Retrieval is similarity-based:
        a broad question mixing several areas (e.g. "GPU and CPU") is dominated by
        the strongest-matching subsystem and may ignore the others. To cover more
        than one area, ask a targeted question for each (e.g. first "GPU VM
        spawning", then "CPU VM spawning for RNA-seq")."""
        return call(
            "oracle_ask",
            {"query": query, "limit": limit, "agent_id": agent_id, "role": role, "project_id": project_id, "session_token": session_token},
        )

    @server.tool()
    def oracle_context(query: str, agent_id: str, role: str, limit: int = 8, project_id: str = "", session_token: str = "") -> dict:
        """Return semantically relevant text chunks for agents.

        TIP: precise, single-topic queries return better chunks. A broad query
        spanning several subsystems tends to fill up with chunks from the
        strongest-matching file/area; to cover more than one, run a targeted
        query for each."""
        return call(
            "oracle_context",
            {"query": query, "limit": limit, "agent_id": agent_id, "role": role, "project_id": project_id, "session_token": session_token},
        )

    @server.tool()
    def project_structure(project_id: str, agent_id: str, role: str, full: bool = False, session_token: str = "") -> dict:
        """Return the project's architecturally-central 'spine' files + summary counts,
        computed DETERMINISTICALLY (no-LLM, tree-sitter cross-file graph) from the project
        root. Use this FIRST to orient — the spine is the handful of files the rest of the
        codebase reaches into — then ask oracle_ask precise questions about them. Returns
        `spine` (each: path, inDegree, topReferencedSymbols) + `summary` (scanned, capped,
        skipped counts). Pass full=True to also get the whole per-file node list (large for
        big repos; usually unnecessary)."""
        return call(
            "project_structure",
            {"project_id": project_id, "full": full, "agent_id": agent_id, "role": role, "session_token": session_token},
        )

    @server.tool()
    def censor_findings(project_id: str, agent_id: str, role: str, file: str = "", session_token: str = "") -> dict:
        """Read OPEN Censor code-review findings for a project (local linters + the
        optional Gemma tier). Pass `file` (a project-relative path) to scope to the
        files you just touched. CODER: call this at each step boundary for your
        changed files, fix real local findings, and censor_dispose false positives.
        VERIFIER: call without `file` for the whole residual ledger and adjudicate
        cross-file/architectural issues. Returns only safe summary fields (never
        raw tool output)."""
        return call(
            "censor_findings",
            {"project_id": project_id, "file": file, "agent_id": agent_id, "role": role, "session_token": session_token},
        )

    @server.tool()
    def censor_dispose(project_id: str, file: str, id: str, disposition: str, agent_id: str, role: str, session_token: str = "") -> dict:
        """Set a Censor finding's disposition and append an audit entry.
        disposition: open|fixed|fp|wontfix. `file` is the project-relative path the
        finding belongs to (from censor_findings). Use `fp` for a false positive,
        `wontfix` for an accepted-but-unfixed finding, `fixed` once resolved."""
        return call(
            "censor_dispose",
            {"project_id": project_id, "file": file, "id": id, "disposition": disposition, "agent_id": agent_id, "role": role, "session_token": session_token},
        )

    return server


def main() -> None:
    parser = argparse.ArgumentParser(description="Aspis Management MCP server")
    parser.add_argument("--root", default=None, help="Aspis Management root folder")
    parser.add_argument("--projects-dir", default=None, help="Shared Aspis Management projects folder")
    args = parser.parse_args()
    create_mcp_server(root=args.root, projects_dir=args.projects_dir).run()


if __name__ == "__main__":
    main()
