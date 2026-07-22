# Cloudflare / Scaleway (cloud-ish) command matrix

**Generated:** 2026-07-20 static

Write-ish commands: **22** · Read/status: **39**

## WRITE commands

| Command | Session | Pin/scope | Inventory | Confirm* | Vault token | Sanitize err |
|---------|:-------:|:---------:|:---------:|:--------:|:-----------:|:------------:|
| `create_scaleway_block_snapshot` | Y | Y | Y | Y | Y | Y |
| `create_scaleway_block_volume` | Y | Y | — | Y | Y | Y |
| `create_scaleway_container` | Y | Y | Y | Y | Y | Y |
| `create_scaleway_filesystem` | Y | Y | — | Y | Y | Y |
| `create_scaleway_function` | Y | Y | Y | Y | Y | Y |
| `create_scaleway_instance` | Y | Y | — | Y | Y | Y |
| `create_scaleway_object_bucket` | Y | Y | Y | Y | — | Y |
| `create_scaleway_sql_database` | Y | Y | Y | Y | Y | Y |
| `delete_scaleway_block_storage` | Y | Y | Y | Y | Y | Y |
| `delete_scaleway_container` | Y | Y | Y | Y | Y | Y |
| `delete_scaleway_filesystem` | Y | Y | Y | Y | Y | Y |
| `delete_scaleway_function` | Y | Y | Y | Y | Y | Y |
| `delete_scaleway_object_access_key` | Y | — | Y | — | — | — |
| `delete_scaleway_object_bucket` | Y | Y | Y | Y | — | Y |
| `delete_scaleway_object_secret_key` | Y | — | Y | — | — | — |
| `delete_scaleway_sql_database` | Y | Y | Y | Y | Y | Y |
| `perform_scaleway_resource_action` | Y | Y | Y | Y | Y | Y |
| `rotate_cloudflare_worker_secret` | Y | Y | Y | Y | Y | Y |
| `set_cloudflare_ai_gateway_settings` | Y | Y | — | — | Y | — |
| `set_cloudflare_kv_value` | Y | Y | — | Y | Y | — |
| `set_cloudflare_r2_cors` | Y | Y | Y | — | Y | — |
| `set_cloudflare_r2_lifecycle` | Y | Y | Y | — | — | — |

\* Confirm = keyword heuristics in command body (confirm-by-name may live in validators).

## Heuristic gaps

- **CONFIRM_UNCLEAR**: `delete_scaleway_object_access_key`
- **CONFIRM_UNCLEAR**: `delete_scaleway_object_secret_key`

## READ / status commands (cloud-related)

| Command | Session |
|---------|:-------:|
| `audit_provider_connection` | Y |
| `audit_saved_provider_connection` | Y |
| `cloudflare_autorag_reindex` | Y |
| `cloudflare_d1_query` | Y |
| `cloudflare_env_dry_run` | Y |
| `cloudflare_set_worker_env` | Y |
| `cloudflare_smoke_dry_run` | Y |
| `delete_cloudflare_agent_token_profile` | Y |
| `delete_cloudflare_kv_value` | Y |
| `delete_github_token` | Y |
| `delete_provider_scope` | Y |
| `delete_provider_token` | Y |
| `design_write_tokens` | Y |
| `detect_providers` | — |
| `fetch_cloudflare_ai_gateway_settings` | Y |
| `fetch_cloudflare_billing` | Y |
| `fetch_cloudflare_kv_keys` | Y |
| `fetch_cloudflare_kv_value` | Y |
| `fetch_cloudflare_r2_config` | Y |
| `fetch_cloudflare_worker_settings` | Y |
| `fetch_scaleway_billing` | Y |
| `get_agent_token_usage` | Y |
| `get_cloudflare_agent_token_profiles` | Y |
| `get_github_connection_status` | Y |
| `get_provider_scope_status` | Y |
| `get_scaleway_object_access_key_status` | Y |
| `get_scaleway_object_secret_key_status` | Y |
| `get_secret_status` | Y |
| `import_github_token_from_cli` | Y |
| `resize_scaleway_block_volume` | Y |
| `save_cloudflare_agent_token_profile` | Y |
| `save_github_token` | Y |
| `save_provider_scope` | Y |
| `save_provider_token` | Y |
| `save_scaleway_object_access_key` | Y |
| `save_scaleway_object_secret_key` | Y |
| `scaleway_instance_create_dry_run` | Y |
| `set_scaleway_object_bucket_lifecycle` | Y |
| `sync_provider_inventory` | Y |

## Deep sample: validate_* / assert_* presence in write bodies

- `save_cloudflare_agent_token_profile`: **NO pin/session keywords in body**
- `delete_cloudflare_agent_token_profile`: **NO pin/session keywords in body**
- `save_scaleway_object_access_key`: **NO pin/session keywords in body**
- `delete_scaleway_object_access_key`: **NO pin/session keywords in body**
- `save_scaleway_object_secret_key`: **NO pin/session keywords in body**
- `delete_scaleway_object_secret_key`: **NO pin/session keywords in body**
- `rotate_cloudflare_worker_secret`: cloudflare_rotation_scope_guard, cloudflare_worker_name_in_aspis_bio_scope, sensitive_session_id, confirm
- `cloudflare_set_worker_env`: sensitive_session_id
- `set_cloudflare_ai_gateway_settings`: resolve_cloudflare_account_action_target, sensitive_session_id, confirm
- `set_cloudflare_kv_value`: resolve_cloudflare_account_action_target, sensitive_session_id, confirm
- `delete_cloudflare_kv_value`: resolve_cloudflare_account_action_target, sensitive_session_id, d1_sql_is_write, confirm
- `set_cloudflare_r2_lifecycle`: **NO pin/session keywords in body**
- `set_cloudflare_r2_cors`: resolve_cloudflare_account_action_target, sensitive_session_id
- `perform_scaleway_resource_action`: assert_scaleway_resource_in_pinned_project, configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm
- `create_scaleway_block_volume`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `resize_scaleway_block_volume`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `create_scaleway_block_snapshot`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `delete_scaleway_block_storage`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm
- `create_scaleway_filesystem`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `delete_scaleway_filesystem`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm
- `create_scaleway_object_bucket`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `delete_scaleway_object_bucket`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm
- `set_scaleway_object_bucket_lifecycle`: assert_scaleway_resource_in_pinned_project, configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `create_scaleway_sql_database`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `delete_scaleway_sql_database`: assert_scaleway_resource_in_pinned_project, configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm
- `scaleway_instance_create_dry_run`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id
- `create_scaleway_instance`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `create_scaleway_function`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `delete_scaleway_function`: assert_scaleway_resource_in_pinned_project, configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm
- `create_scaleway_container`: configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm
- `delete_scaleway_container`: assert_scaleway_resource_in_pinned_project, configured_or_pinned_scaleway_project_id, validate_scaleway, sensitive_session_id, confirm_resource_name, confirm

---

## Truth-check

Pass 6: see [VERIFICATION.md](./VERIFICATION.md).
