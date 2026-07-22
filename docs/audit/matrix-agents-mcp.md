# Agentic tools + MCP role matrix

**Generated:** 2026-07-20 static

## ScopedAgentTools / public tools

| Method | Notes |
|--------|-------|
| `safe_rel_path` | path-safe?, run-parser |
| `parse_run_command` | run-parser, allowlist |
| `with_write_allowlist` | path-safe?, allowlist, WRITE |
| `with_net` | allowlist |
| `with_working_set` | allowlist |
| `with_oracle` | allowlist |
| `touched` | allowlist |
| `net_blocked` | allowlist |
| `out_of_scope_write` | allowlist, WRITE |
| `write_allowed` | allowlist, WRITE |
| `write_file_abs` | allowlist, WRITE |
| `canon_root` | path-safe? |
| `resolve` | path-safe? |
| `read_file` | READ |
| `list_dir` | path-safe? |
| `grep` | path-safe? |
| `write_resolve` | path-safe?, WRITE |
| `write_file` | allowlist, WRITE |
| `edit_file` | path-safe?, WRITE |
| `edit_file_abs` | allowlist, WRITE |
| `record_touched` | run-parser, allowlist, spawns |
| `run` | run-parser, allowlist, spawns |
| `agentic_run_policy` | — |
| `agentic_run_policy_with_working_set` | — |
| `looks_network_blocked` | — |
| `drain_capped` | — |
| `kill_process_group` | — |
| `walk_grep` | — |
| `call` | — |
| `agentic_run_policy_respects_net` | path-safe?, run-parser |
| `looks_network_blocked_detects_common_failures` | path-safe?, run-parser |
| `safe_rel_path_rejects_escapes` | path-safe?, run-parser |
| `safe_rel_path_normalizes_accepted` | path-safe?, run-parser |
| `parse_run_command_multilang_safe_and_escapes_blocked` | run-parser |
| `grep_does_not_follow_symlinks_out_of_scope` | — |
| `read_file_truncates_large_content` | READ |
| `unique_root` | — |
| `write_then_read_roundtrip` | WRITE, READ |
| `edit_replaces_unique_occurrence` | WRITE |
| `edit_rejects_non_unique` | WRITE |
| `write_through_symlink_out_is_refused` | allowlist, WRITE |
| `write_to_hardlinked_file_is_refused` | allowlist, WRITE |
| `touched_records_deduped_paths` | allowlist |
| `write_allowlist_blocks_out_of_scope_writes` | allowlist, WRITE |
| `net_blocked_is_false_by_default` | — |
| `net_blocked_is_set_when_network_error_detected_with_net_none` | allowlist |
| `write_to_working_set_folder_succeeds_and_no_signal` | WRITE |
| `write_outside_root_and_working_set_sets_signal` | allowlist, WRITE |
| `in_root_but_outside_allowlist_does_not_set_out_of_scope_write_signal` | allowlist, WRITE |
| `llm_write_file_abs_in_working_set_succeeds` | WRITE |
| `llm_write_file_abs_outside_scope_sets_signal_and_writes_nothing` | WRITE |
| `llm_edit_file_abs_in_working_set_succeeds` | WRITE |
| `llm_edit_file_abs_outside_scope_sets_signal` | WRITE |
| `relative_write_in_root_still_works_after_blocker1` | WRITE |
| `write_file_abs_rejects_dangling_symlink_in_working_set` | WRITE |
| `write_file_abs_rejects_hardlinked_file_in_working_set` | allowlist, WRITE |
| `write_file_abs_in_root_outside_allowlist_is_rejected` | allowlist, WRITE |
| `write_file_abs_in_root_on_allowlist_succeeds` | allowlist, WRITE |
| `write_file_abs_in_working_set_bypasses_allowlist` | allowlist, WRITE |
| `edit_file_abs_in_root_outside_allowlist_is_rejected` | allowlist, WRITE |
| `edit_file_abs_in_root_on_allowlist_succeeds` | allowlist, WRITE |
| `edit_file_abs_in_working_set_bypasses_allowlist` | allowlist, WRITE |
| `write_file_abs_in_root_is_recorded_in_touched` | allowlist, WRITE |
| `write_file_abs_in_working_set_is_recorded_in_touched` | allowlist, WRITE |
| `edit_file_abs_in_root_is_recorded_in_touched` | allowlist, WRITE |
| `edit_file_abs_in_working_set_is_recorded_in_touched` | WRITE |
| `agentic_run_policy_includes_working_set_in_writable` | — |
| `gate_root` | allowlist |
| `gate_out_of_scope_write_triggers_prompt` | allowlist, WRITE |
| `gate_respawn_includes_widened_scope` | — |

## RUN_PROGRAMS allowlist (excerpt)

```
const RUN_PROGRAMS: &[&str] = &[
    // Rust
    "cargo", "rustc", "rustfmt", "clippy-driver",
    // Go
    "go", "gofmt", "golangci-lint",
    // C / C++ / native
    "make", "cmake", "ninja", "ctest", "meson",
    // JVM
    "gradle", "./gradlew", "mvn", "./mvnw",
    // .NET
    "dotnet",
    // JS / TS
    "npm", "npx", "yarn", "pnpm", "node", "deno", "bun", "tsc", "eslint", "vitest", "jest",
    "biome",
    // Python
    "python", "python3", "pytest", "tox", "ruff", "mypy", "pip", "pip3", "poetry", "uv",
    "black", "flake8",
    // Ruby
    "ruby", "rake", "rspec", "bundle", "rubocop",
    // PHP
    "php", "composer", "phpunit",
    // Swift / others
    "swift", "zig", "dart", "flutter", "elixir", "mix",
];

/// PURE RCE gate for the `run` tool: validate a command into an argv vector for a NO-SHELL
/// exec. (1) rejects shell metacharacters (no chaining/substitution/redirection — also defangs
/// `python -c "…"` etc. since quotes/parens are blocked); (2) requires the program (token 0) to
/// be a known dev/build/test tool — LANGUAGE-AGNOSTIC, not a Rust/JS-only pair list; (3) safe
/// charset on every token; (4) blocks scope-escape in args (parent-`..` segments, absolute
```

## aspis_mcp role_rules.json

```json
{
  "$comment": "SINGLE SOURCE OF TRUTH for agent role rules (English only). Consumed by: oracle/server/aspis_mcp.py (ROLE_RULES, json.load at import), src-tauri/src/backend/agents.rs (default_role_rules, include_str! + serde at compile time), and src-tauri/src/backend/projects.rs (project_agent_prompt reads launchPrompt). Edit HERE only \u2014 there are no hand-synced copies anymore. allowedTools order is significant (pinned by schema tests both sides). launchPrompt is the prose role block injected into the cloud CLI bootstrap prompt; the local devboule-orchestrator binary builds its own system prompt (devboule-coder/src/prompt.rs) and does not consume it.",
  "roles": [
    {
      "role": "coder",
      "summary": "Plans (/plan), works on code and uses Oracle context; opens blockers, reopens tasks, and moves work to review or blocked, but never done.",
      "censor": [
        "At each step boundary call censor_findings(project_id, file=<files you touched>, drain_queue=True) to also drain the async findings accumulated in the persistent queue.",
        "Fix the real local findings; dispose false positives with censor_dispose(disposition=\"fp\").",
        "Batch at the step boundary: this is a per-step check before moving on, not a live interrupt."
      ],
      "plan": [
        "Before any multi-file work, submit the plan with plan_submit(project_id, title, plan_markdown) and WAIT for the human approval: do not start implementing before status=\"approved\".",
        "As soon as it is approved (status=\"approved\"), IMMEDIATELY call project_create_plan_tasks with the structured task list: the Kanban has ZERO tasks until you do, so never start coding before this call. Split the plan into SMALL, self-contained tasks (one testable, committable unit each; a task's scope has AT MOST 3 files \u2014 split anything larger; give every task a deterministically verifiable acceptance). Pass plan_id = the `planId` field returned by plan_submit, and tasks = that list, each REQUIRING {id, title} plus {acceptance, scope:[files], dependsOn}. `id` is a short internal ref you assign (e.g. \"P1\", \"P2\"); `dependsOn` lists ids of OTHER tasks in THIS SAME call (e.g. [\"P1\"]) \u2014 NOT the Kanban T-numbers (the server allocates those and remaps your refs).",
        "Scale clarifying questions to complexity: for a non-trivial or ambiguous task ask the human UP TO 3 targeted questions via ask_user BEFORE planning (zero is fine when it is clear); skip them on simple/obvious tasks. Do not over-consult on trivial work.",
        "If the plan is rejected (status=\"rejected\"), revise it per the reviewer's `note` and RESUBMIT with plan_submit; do not proceed on a rejected plan.",
        "If the plan request times out, STOP and escalate via needs_user (agent_heartbeat status=\"needs_user\"); do NOT proceed unapproved.",
        "When you have a blocking question for the human use ask_user(question) and wait for the reply, instead of stalling or guessing in the terminal."
      ],
      "push": [
        "Commit freely (git add -u / git commit) to save your work.",
        "NEVER run a raw `git push` \u2014 your environment has no git credentials and it will fail. To publish, call the `request_git_push` MCP tool; a human approves it.",
        "If the push request is denied or times out, STOP and escalate to the human via needs_user (agent_heartbeat status=\"needs_user\"). Do NOT retry, do NOT attempt a raw push, do NOT work around the gate."
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
        "project_set_title",
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
        "get_neighborhood",
        "find_imports",
        "censor_findings",
        "censor_dispose",
        "visual_check",
        "design_request",
        "spawn_mini_coder",
        "steer_mini_coder",
        "mini_coder_result",
        "request_git_push",
        "plan_submit",
        "plan_status",
        "ask_user"
      ],
      "forbidden": [
        "No done status: done is verifier-only with evidence.",
        "No token printing or token logging. Use only tokens from env within verified Devboule scopes.",
        "No provider action outside verified Devboule scopes.",
        "Delegate only cheap, mechanical sub-tasks to spawn_mini_coder (boilerplate, bulk read->summary, simple edits, docstrings, tests); front-load the needed context; do the thinking yourself; REVIEW the mini's output as a draft before using it.",
        "To SUPERVISE a delegated mini call spawn_mini_coder with wait=false for its directiveId, watch its activity, steer with steer_mini_coder(directiveId, message) (or \"stop\" to interrupt), then collect the outcome with mini_coder_result(directiveId); the default blocking spawn_mini_coder is fine for simple fire-and-forget delegation.",
        "For a WRITE task set spawn_mini_coder's write_mode: use 'agenticIterative' ONLY for files in a language with deterministic-gate coverage in this project AND when the local model is capable enough to iterate usefully; otherwise use 'emitEdits' (the default). When unsure, use 'emitEdits'.",
        "If spawn_mini_coder returns status='aborted_by_human', STOP that line of work, do NOT silently retry the mini, and escalate to the human via needs_user (agent_heartbeat status=\"needs_user\").",
        "If spawn_mini_coder returns status='escalated', REDO that file yourself (the mini's automatic retries failed Censor and the training rail captured them); do NOT re-spawn the mini for the same file.",
        "Before moving a task to review: run ONE review pass of your own (a Sonnet review subagent) over the files you touched, fix the findings, THEN set the task to review with a 'ready for final reviewer' note. The FINAL verdict stays with the verifier (the censorReview final pass, triggered from the app UI \u2014 it does NOT fire automatically when you set review) \u2014 never your own pass.",
        "When you produce or review a self-contained HTML artifact and need visual feedback, call visual_check(html_path, focus?) and treat the returned critique as advisory evidence."
      ],
      "contract": [
        "Declare your model (`model`) at agent_register.",
        "Whenever you spawn or close subagents, send agent_heartbeat with an updated `subagents=[{label, model, count, role?}]`.",
        "When waiting on the human (question, allow/deny permission, blocker) send agent_heartbeat with status=\"needs_user\" and a clear message."
      ],
      "launchPrompt": "Plan and code. For multi-step work, submit a plan with plan_submit and WAIT for approval; ON APPROVAL, immediately call project_create_plan_tasks with the structured task list \u2014 the Kanban has ZERO tasks until you do, so never start coding before this call. Split the plan into SMALL, self-contained tasks (one testable, committable unit each; a task's scope has AT MOST 3 files \u2014 split anything larger; give every task a deterministically verifiable acceptance). Pass plan_id = the `planId` field returned by plan_submit, and tasks = that list, each REQUIRING {id, title} plus {acceptance, scope:[files], dependsOn}. `id` is a short internal ref you assign (e.g. \"P1\", \"P2\"); `dependsOn` lists the ids of OTHER tasks in THIS SAME call (e.g. [\"P1\"]) \u2014 NOT the Kanban T-numbers (the server allocates those and remaps your refs). Scale clarifying questions to complexity: ask the human UP TO 3 targeted questions via ask_user before planning a non-trivial or
```

## aspis_mcp.py tools & role checks

File size: 444099 bytes, lines: 10130

### Role-related lines (476 hits, sample)

- L89: `# Phase B role merge: spawn-time roles collapse to {coder, verifier}. The`
- L92: `# mutation tools — enforced by ROLE_ALLOWED_TOOLS via require_registered_role.`
- L94: `# coder. The new Rust `devboule-coder` binary self-registers under this role.`
- L96: `# collapsed to "coder". It is now its OWN role and must NOT be normalized away —`
- L99: `# project semantics are IDENTICAL to coder's (see CODER_LIKE_ROLES) so it never`
- L101: `VALID_ROLES = {"coder", "verifier", "mini", "orchestrator"}`
- L103: `# role it normalizes to itself, not to coder.`
- L104: `ROLE_ALIASES = {"architect": "coder", "code": "coder"}`
- L111: `CODER_LIKE_ROLES = {"coder", "orchestrator"}`
- L229: `# planner is allowed to emit is also allowed to be bulk-created on the Kanban — the`
- L296: `# SINGLE SOURCE OF TRUTH: role rules live in oracle/server/role_rules.json`
- L297: `# (English only, 4 roles). Edit the JSON, not this module — it is loaded`
- L299: `_ROLE_RULES_PATH = Path(__file__).resolve().parent / "role_rules.json"`
- L304: `ROLE_RULES = json.loads(_ROLE_RULES_PATH.read_text(encoding="utf-8-sig"))["roles"]`
- L306: `ROLE_ALLOWED_TOOLS = {rule["role"]: set(rule["allowedTools"]) for rule in ROLE_RULES}`
- L312: `"description": "Returns roles, responsibilities, and practical restrictions for Devboule agents.",`
- L320: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L329: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L351: `# A list of {label, model, count, role?}. Omit (or pass null) to leave`
- L369: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L406: `# ROLE UNTANGLE Phase 3: the FIRST-CLASS MAIN CODER dispatch (orchestrator-`
- L423: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L453: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L478: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L496: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L524: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L553: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L566: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L578: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L588: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L598: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L608: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L614: `"description": "Suggests the next incomplete task for a role.",`
- L617: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L629: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L641: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L654: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L665: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L683: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L713: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L722: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L731: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L741: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L757: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L767: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L786: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L820: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L859: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L872: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L883: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L908: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L920: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L936: `"role": {"type": "string", "enum": sorted(VALID_ROLES)},`
- L989: `# ("{role}-{millis}") conform with margin.`
- L1099: `def normalize_role(value: str) -> str:`
- L1100: `role = str(value or "").strip().lower()`
- L1101: `role = ROLE_ALIASES.get(role, role)`
- L1102: `if role not in VALID_ROLES:`
- L1103: `aliases = ", ".join(sorted(ROLE_ALIASES))`
- L1105: `f"Role must be one of: {', '.join(sorted(VALID_ROLES))}"`
- L1108: `return role`
- L1111: `def coerce_role(value: str) -> str:`
- L1112: `# Non-raising role normalization for STORED data: maps valid roles + known`
- L1113: `# aliases to their canonical, and any UNKNOWN/garbage role to the safe "coder"`
- L1114: `# default so a corrupt stored role can never brick a session. Used when loading`
- L1115: `# state and when comparing stored vs incoming roles — distinct from`
- L1116: `# normalize_role(), which RAISES (it gates inbound tool args).`
- L1117: `role = str(value or "").strip().lower()`
- L1118: `role = ROLE_ALIASES.get(role, role)`
- L1119: `return role if role in VALID_ROLES else "coder"`
- L1122: `def _roles_same_canonical(a: str, b: str) -> bool:`
- L1123: `# True when two role strings (alias or canonical) collapse to the same canonical`
- L1124: `# role. Used to decide whether a write would merely re-alias the stored role.`
- L1125: `return coerce_role(a) == coerce_role(b)`
- L1131: `# model x role counts (e.g. "claude-opus-4-8" and "Claude Opus 4.8" both -> "opus").`
- L1222: `coerced to an int in [1, 9999] (defaults to 1 when absent); `role` is`
- L1223: `optional and normalized via the existing role rules (invalid -> None).`
- L1243: `role: str | None = None`
- L1244: `raw_role = entry.get("role")`
- L1245: `if raw_role is not None and str(raw_role).strip():`

### def names (first 80)

- `now`
- `strip_invisible_and_bidi`
- `clean_text`
- `normalize_agent_id`
- `clean_description`
- `normalize_task_category`
- `normalize_project_id`
- `validate_push_remote`
- `normalize_task_id`
- `normalize_current_file_path`
- `normalize_role`
- `coerce_role`
- `normalize_model`
- `normalize_agent_status`
- `normalize_subagents`
- `normalize_task_status`
- `normalize_readable_project_status`
- `normalize_provider_name`
- `validate_management_root`
- `approved_work_root_parents`
- `path_is_within`
- `validate_project_work_root`
- `resolve_root`
- `resolve_projects_dir`
- `management_root_from_projects_dir`
- `mcp_oracle_paths`
- `oracle_index_root_for_args`
- `enforce_mini_oracle_project_scope`
- `oracle_allowed_file_ids`
- `ensure_inside_projects`
- `file_lock`
- `sha256_text`
- `hash_launch_token`
- `generate_session_token`
- `hash_session_token`
- `unmanaged_privileged_agents_allowed`
- `validate_launch_token_for_registration`
- `parse_simple_yaml`
- `unquote_simple_yaml_value`
- `parse_frontmatter`
- `yaml_quote`
- `find_state_block`
- `replace_frontmatter`
- `write_text_crash_safe`
- `read_project_file`
- `validate_task_dependency_dag`
- `validate_project_state`
- `write_project_file`
- `project_path`
- `project_lock_path`
- `task_counts`
- `summarize_project`
- `public_project`
- `next_task_id`
- `read_agents_state`
- `write_agents_state`
- `public_agents_state`
- `compact_session_ack`
- `default_agents_state`
- `normalize_agents_state`
- `cap_sessions`
- `cap_claims`
- `cap_mini_coder_directives`
- `cap_visual_check_directives`
- `cap_git_push_requests`
- `cap_plan_approval_requests`
- `reconcile_agents_state_with_projects`
- `event_id`
- `note_id`
- `add_event`
- `upsert_session`
- `require_session_token`
- `require_registered_role`
- `validate_transition`
- `parse_iso_timestamp`
- `claim_is_active`
- `active_claim_for_task`
- `owns_own_claim_for_task`
- `require_claim_for_status_update`
- `provider_mutation_approval_enforced`

### role_rules usage context

```
 "nl-ams-1",
    "nl-ams-2",
    "nl-ams-3",
    "pl-waw-1",
    "pl-waw-2",
    "pl-waw-3",
)
SCW_REGIONS = ("fr-par", "nl-ams", "pl-waw")

# SINGLE SOURCE OF TRUTH: role rules live in oracle/server/role_rules.json
# (English only, 4 roles). Edit the JSON, not this module — it is loaded
# verbatim at import time, no hand-synced literal, no silent fallback.
_ROLE_RULES_PATH = Path(__file__).resolve().parent / "role_rules.json"
# utf-8-sig: identical to utf-8 for a clean file, but strips a leading BOM if a
# Windows editor ever saves one — json.loads on a BOM-prefixed string would
# otherwise take the whole MCP server down at import (this file is the SSoT and
# explicitly invites hand-edits).
ROLE_RULES = json.loads(_ROLE_RULES_PATH.read_text(encoding="utf-8-sig"))["roles"]

ROLE_ALLOWED_TOOLS = {rule["role"]: set(rule["allowedTools"]) for rule in ROLE_RULES}


TOOLS = [
    {
        "name": "agent_rules",
        "description": "Returns roles, responsibilities, and practical restricti
```

## skill_vet.rs surface

- `rules`
- `get_evidence`
- `scan_skill_risks`
- `worst_severity`

---

## Truth-check

Pass 6: see [VERIFICATION.md](./VERIFICATION.md).
