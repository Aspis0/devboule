# Full Tauri command gate matrix
**Generated:** 2026-07-20 static (1-hop helper resolution)
| Class | Count |
|-------|------:|
| Total commands | 303 |
| GATED | 249 |
| UNGATED_MUTATE | 16 |
| UNGATED_READ | 28 |

## UNGATED_MUTATE (priority)

> **Truth-check (pass 6):** the `MUTATE` label is a **keyword heuristic**. Several rows are **reads** (`skills_*_catalog`, `mini_activity_snapshot`, `list_pending_design_requests`, `pi_extensions_list`). Real mutates: steers, duplex, install/remove, design claim/complete, planner_reset, polis_debug_log. See [VERIFICATION.md](./VERIFICATION.md) FP-1.

| File | Command |
|------|----------|
| `backend/agents.rs` | `list_pending_design_requests` |
| `backend/agents.rs` | `design_request_claim` |
| `backend/agents.rs` | `design_request_complete` |
| `backend/cloud_duplex.rs` | `project_cloud_orchestrator_interrupt` |
| `backend/cloud_duplex.rs` | `project_cloud_orchestrator_send` |
| `backend/mini_activity.rs` | `mini_activity_snapshot` |
| `backend/mini_coder_executor.rs` | `mini_coder_steer` |
| `backend/pi_extensions.rs` | `pi_extensions_list` |
| `backend/pi_extensions.rs` | `pi_extension_install` |
| `backend/pi_extensions.rs` | `pi_extension_remove` |
| `backend/project_skill.rs` | `skills_featured_marketplaces` |
| `backend/project_skill.rs` | `skills_library_catalog` |
| `backend/project_skill.rs` | `skills_lang_catalog` |
| `backend/projects.rs` | `orchestrator_steer` |
| `backend/projects.rs` | `planner_reset_chat` |
| `polis/commands.rs` | `polis_debug_log` |

## UNGATED_READ

| File | Command |
|------|----------|
| `backend/budget.rs` | `poll_backend_memory` |
| `backend/budget.rs` | `recommend_resource_config` |
| `backend/cloud_duplex.rs` | `project_cloud_compact` |
| `backend/commands.rs` | `get_auth_state` |
| `backend/commands.rs` | `request_unlock` |
| `backend/commands.rs` | `lock_app` |
| `backend/cost.rs` | `estimate_task_cost` |
| `backend/cost.rs` | `record_cost` |
| `backend/design_generate.rs` | `design_cancel_generation` |
| `backend/hardware.rs` | `detect_hardware` |
| `backend/model_registry.rs` | `discover_installed_models` |
| `backend/oracle_service.rs` | `get_oracle_enabled` |
| `backend/oracle_service.rs` | `get_oracle_engine` |
| `backend/pi_extensions.rs` | `pi_extensions_status` |
| `backend/pi_extensions.rs` | `pi_marketplace_search` |
| `backend/pi_extensions.rs` | `pi_agents_list` |
| `backend/pigeon_service.rs` | `get_pigeon_enabled` |
| `backend/provider_detect.rs` | `detect_providers` |
| `backend/provider_detect.rs` | `detect_dependencies` |
| `polis/commands.rs` | `trigger_file_disaster` |
| `polis/commands.rs` | `resolve_file_disaster` |
| `polis/commands.rs` | `set_agent_location` |
| `polis/commands.rs` | `update_agent_status` |
| `polis/commands.rs` | `append_city_note` |
| `polis/commands.rs` | `spawn_scaleway_resource` |
| `polis/commands.rs` | `stop_scaleway_resource` |
| `polis/commands.rs` | `refresh_scaleway_status` |
| `polis/commands.rs` | `polis_stop_watch` |

## Full table

| File | Command | Risk | Direct gates | Via helpers | Mutates |
|------|---------|------|--------------|-------------|--------|
| `backend/agent_notifications.rs` | `read_agent_notification_state` | GATED | ensure_unlocked | — | False |
| `backend/agent_notifications.rs` | `write_agent_notification_state` | GATED | ensure_unlocked | — | False |
| `backend/agent_pty.rs` | `agent_pty_snapshot` | GATED | ensure_unlocked | — | True |
| `backend/agent_pty.rs` | `agent_pty_write` | GATED | ensure_unlocked | — | True |
| `backend/agent_pty.rs` | `agent_pty_send_message` | GATED | ensure_unlocked | — | True |
| `backend/agent_pty.rs` | `agent_pty_resize` | GATED | ensure_unlocked | — | False |
| `backend/agent_pty.rs` | `agent_pty_list` | GATED | ensure_unlocked | — | True |
| `backend/agents.rs` | `get_agent_live_state` | GATED | ensure_unlocked | — | True |
| `backend/agents.rs` | `list_pending_design_requests` | UNGATED_MUTATE | — | — | True |
| `backend/agents.rs` | `design_request_claim` | UNGATED_MUTATE | — | — | True |
| `backend/agents.rs` | `design_request_complete` | UNGATED_MUTATE | — | — | True |
| `backend/agents.rs` | `focus_agent_terminal` | GATED | ensure_unlocked | — | True |
| `backend/agents.rs` | `stop_agent` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked | True |
| `backend/budget.rs` | `poll_backend_memory` | UNGATED_READ | — | — | False |
| `backend/budget.rs` | `recommend_resource_config` | UNGATED_READ | — | — | False |
| `backend/censor/commands.rs` | `censor_review_now` | GATED | ensure_unlocked | — | True |
| `backend/censor/commands.rs` | `censor_get_findings` | GATED | ensure_unlocked | — | False |
| `backend/censor/commands.rs` | `censor_count_open` | GATED | ensure_unlocked | — | False |
| `backend/censor/commands.rs` | `censor_status` | GATED | ensure_unlocked | — | True |
| `backend/censor/commands.rs` | `censor_set_coarse_policy` | GATED | ensure_unlocked | — | True |
| `backend/censor/commands.rs` | `set_censor_trusted` | GATED | ensure_unlocked | — | True |
| `backend/censor/commands.rs` | `censor_dispose_finding` | GATED | ensure_unlocked | — | False |
| `backend/censor/commands.rs` | `censor_open_in_editor` | GATED | ensure_unlocked | — | False |
| `backend/censor/commands.rs` | `censor_scan_state` | GATED | ensure_unlocked | — | False |
| `backend/changes.rs` | `git_working_diff` | GATED | ensure_unlocked | — | False |
| `backend/changes.rs` | `open_in_editor` | GATED | ensure_unlocked | — | True |
| `backend/changes.rs` | `list_external_editors` | GATED | ensure_unlocked | — | False |
| `backend/cli_agents.rs` | `configure_cli_agents` | GATED | ensure_unlocked | — | False |
| `backend/cli_agents.rs` | `cli_agents_status` | GATED | ensure_unlocked | — | False |
| `backend/cli_agents.rs` | `unconfigure_cli_agents` | GATED | ensure_unlocked | — | False |
| `backend/cloud_duplex.rs` | `project_cloud_orchestrator_interrupt` | UNGATED_MUTATE | — | — | True |
| `backend/cloud_duplex.rs` | `project_cloud_orchestrator_send` | UNGATED_MUTATE | — | — | True |
| `backend/cloud_duplex.rs` | `project_cloud_compact` | UNGATED_READ | — | — | False |
| `backend/commands.rs` | `get_auth_state` | UNGATED_READ | — | — | False |
| `backend/commands.rs` | `request_unlock` | UNGATED_READ | — | — | False |
| `backend/commands.rs` | `lock_app` | UNGATED_READ | — | — | False |
| `backend/commands.rs` | `get_secret_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `get_provider_scope_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `get_cloudflare_agent_token_profiles` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_cloudflare_agent_token_profile` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `delete_cloudflare_agent_token_profile` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `get_scaleway_object_access_key_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_scaleway_object_access_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `delete_scaleway_object_access_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `get_scaleway_object_secret_key_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_scaleway_object_secret_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `delete_scaleway_object_secret_key` | GATED | ensure_unlocked | — | True |
| `backend/commands.rs` | `get_censor_cloud_key_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_censor_cloud_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `delete_censor_cloud_key` | GATED | ensure_unlocked | — | True |
| `backend/commands.rs` | `get_cloud_llm_key_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_cloud_llm_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `delete_cloud_llm_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `websearch_key_status` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `websearch_save_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `websearch_delete_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `websearch_get_config` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `websearch_set_config` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `get_oracle_llm_settings` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_oracle_llm_settings` | GATED | ensure_unlocked | — | True |
| `backend/commands.rs` | `delete_oracle_llm_api_key` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `get_oracle_index_preferences` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_oracle_index_preferences` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_provider_scope` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `delete_provider_scope` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `audit_provider_connection` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `audit_saved_provider_connection` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `save_provider_token` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_provider_token` | GATED | ensure_unlocked | — | False |
| `backend/commands.rs` | `rotate_cloudflare_worker_secret` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `fetch_cloudflare_worker_settings` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `fetch_cloudflare_billing` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `fetch_scaleway_billing` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `cloudflare_env_dry_run` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `cloudflare_set_worker_env` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `fetch_cloudflare_ai_gateway_settings` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `set_cloudflare_ai_gateway_settings` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `cloudflare_autorag_reindex` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `fetch_cloudflare_kv_keys` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `fetch_cloudflare_kv_value` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `set_cloudflare_kv_value` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_cloudflare_kv_value` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `cloudflare_d1_query` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `fetch_cloudflare_r2_config` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `set_cloudflare_r2_lifecycle` | GATED | — | set_cloudflare_r2_target→sensitive_session_id,ensure_same_sensitive_session | True |
| `backend/commands.rs` | `set_cloudflare_r2_cors` | GATED | sensitive_session_id|ensure_same_sensitive_session | set_cloudflare_r2_target→sensitive_session_id,ensure_same_sensitive_session | True |
| `backend/commands.rs` | `cloudflare_smoke_dry_run` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | False |
| `backend/commands.rs` | `perform_scaleway_resource_action` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_block_volume` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `resize_scaleway_block_volume` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_block_snapshot` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_scaleway_block_storage` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_filesystem` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_scaleway_filesystem` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_object_bucket` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_scaleway_object_bucket` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `set_scaleway_object_bucket_lifecycle` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_sql_database` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_scaleway_sql_database` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `scaleway_instance_create_dry_run` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_instance` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_function` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_scaleway_function` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `create_scaleway_container` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `delete_scaleway_container` | GATED | sensitive_session_id|ensure_same_sensitive_session | — | True |
| `backend/commands.rs` | `get_cloud_dashboard_snapshot` | GATED | — | build_snapshot→sensitive_session_id,ensure_same_sensitive_session | False |
| `backend/commands.rs` | `sync_provider_inventory` | GATED | sensitive_session_id|ensure_same_sensitive_session | build_snapshot→sensitive_session_id,ensure_same_sensitive_session | True |
| `backend/cost.rs` | `estimate_task_cost` | UNGATED_READ | — | — | False |
| `backend/cost.rs` | `record_cost` | UNGATED_READ | — | — | False |
| `backend/design.rs` | `design_create_project` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_load_project` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_save_project` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_write_manifest` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_write_node` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_oracle_context` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_oracle_status` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_read_design_md` | GATED | ensure_unlocked | — | False |
| `backend/design.rs` | `design_write_design_md` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_write_tokens` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_append_generation_log` | GATED | ensure_unlocked | — | False |
| `backend/design.rs` | `design_write_export` | GATED | ensure_unlocked | — | False |
| `backend/design.rs` | `design_write_artifact` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_registry_list` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_registry_remember` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_registry_rename` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_registry_set_linked_task` | GATED | ensure_unlocked | — | True |
| `backend/design.rs` | `design_registry_remove` | GATED | ensure_unlocked | — | True |
| `backend/design_generate.rs` | `design_generate` | GATED | ensure_unlocked | — | True |
| `backend/design_generate.rs` | `design_cancel_generation` | UNGATED_READ | — | — | False |
| `backend/design_preview.rs` | `design_preview_open` | GATED | ensure_unlocked | — | True |
| `backend/design_preview.rs` | `design_preview_capture` | GATED | ensure_unlocked | — | False |
| `backend/design_preview.rs` | `design_visual_critique` | GATED | ensure_unlocked | — | True |
| `backend/design_preview.rs` | `design_read_thumbnail` | GATED | ensure_unlocked | — | True |
| `backend/devices.rs` | `get_devices_invites_snapshot` | GATED | ensure_unlocked | — | False |
| `backend/devices.rs` | `ensure_local_device_identity` | GATED | ensure_unlocked | — | False |
| `backend/devices.rs` | `reset_local_device_identity` | GATED | ensure_unlocked | — | True |
| `backend/devices.rs` | `approve_device_invite` | GATED | ensure_unlocked|require_capability | — | False |
| `backend/devices.rs` | `revoke_device_invite` | GATED | ensure_unlocked|require_capability | — | False |
| `backend/github.rs` | `get_github_connection_status` | GATED | ensure_unlocked | — | False |
| `backend/github.rs` | `save_github_token` | GATED | ensure_unlocked | — | False |
| `backend/github.rs` | `delete_github_token` | GATED | ensure_unlocked | — | False |
| `backend/github.rs` | `import_github_token_from_cli` | GATED | ensure_unlocked | — | False |
| `backend/global_skills.rs` | `global_skills_list` | GATED | ensure_unlocked | — | True |
| `backend/global_skills.rs` | `global_skills_save` | GATED | ensure_unlocked | — | True |
| `backend/global_skills.rs` | `global_skills_delete` | GATED | ensure_unlocked | — | True |
| `backend/global_skills.rs` | `global_skills_install_bundled` | GATED | ensure_unlocked | — | True |
| `backend/global_skills.rs` | `global_skills_marketplace_install` | GATED | ensure_unlocked | — | True |
| `backend/hardware.rs` | `detect_hardware` | UNGATED_READ | — | — | False |
| `backend/mini_activity.rs` | `mini_activity_snapshot` | UNGATED_MUTATE | — | — | True |
| `backend/mini_coder_executor.rs` | `mini_coder_kill` | GATED | ensure_unlocked | — | True |
| `backend/mini_coder_executor.rs` | `mini_coder_steer` | UNGATED_MUTATE | — | — | True |
| `backend/model_registry.rs` | `get_model_registry` | GATED | ensure_unlocked | — | True |
| `backend/model_registry.rs` | `set_model_registry` | GATED | ensure_unlocked | — | True |
| `backend/model_registry.rs` | `discover_installed_models` | UNGATED_READ | — | — | False |
| `backend/oracle_service.rs` | `get_oracle_enabled` | UNGATED_READ | — | — | False |
| `backend/oracle_service.rs` | `set_oracle_enabled` | GATED | ensure_unlocked | — | True |
| `backend/oracle_service.rs` | `get_oracle_engine` | UNGATED_READ | — | — | False |
| `backend/oracle_service.rs` | `set_oracle_engine` | GATED | ensure_unlocked | — | True |
| `backend/pi_extensions.rs` | `pi_extensions_status` | UNGATED_READ | — | — | False |
| `backend/pi_extensions.rs` | `pi_extensions_list` | UNGATED_MUTATE | — | — | True |
| `backend/pi_extensions.rs` | `pi_extension_install` | UNGATED_MUTATE | — | — | True |
| `backend/pi_extensions.rs` | `pi_extension_remove` | UNGATED_MUTATE | — | — | True |
| `backend/pi_extensions.rs` | `pi_marketplace_search` | UNGATED_READ | — | — | False |
| `backend/pi_extensions.rs` | `pi_agents_list` | UNGATED_READ | — | — | False |
| `backend/pigeon_service.rs` | `get_pigeon_enabled` | UNGATED_READ | — | — | False |
| `backend/pigeon_service.rs` | `set_pigeon_enabled` | GATED | ensure_unlocked | — | True |
| `backend/plan_approval.rs` | `plan_approval_requests_list` | GATED | ensure_unlocked | — | False |
| `backend/plan_approval.rs` | `get_plan_markdown` | GATED | ensure_unlocked | — | False |
| `backend/plan_approval.rs` | `list_project_plans` | GATED | ensure_unlocked | — | True |
| `backend/plan_approval.rs` | `approve_plan_request` | GATED | — | decide_plan_request→ensure_unlocked | True |
| `backend/plan_approval.rs` | `deny_plan_request` | GATED | — | decide_plan_request→ensure_unlocked | True |
| `backend/plan_approval.rs` | `reply_to_agent` | GATED | ensure_unlocked | — | False |
| `backend/project_git.rs` | `project_git_commit` | GATED | ensure_unlocked | — | False |
| `backend/project_git.rs` | `project_git_push` | GATED | ensure_unlocked | — | True |
| `backend/project_git.rs` | `git_push_requests_list` | GATED | ensure_unlocked | — | True |
| `backend/project_git.rs` | `approve_git_push_request` | GATED | ensure_unlocked | — | True |
| `backend/project_git.rs` | `deny_git_push_request` | GATED | ensure_unlocked | — | True |
| `backend/project_git.rs` | `project_git_clone` | GATED | ensure_unlocked | create_project→ensure_unlocked | True |
| `backend/project_git.rs` | `project_git_pull` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_featured_marketplaces` | UNGATED_MUTATE | — | — | True |
| `backend/project_skill.rs` | `skills_list_profiles` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_set_enabled_profile` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_library_catalog` | UNGATED_MUTATE | — | — | True |
| `backend/project_skill.rs` | `skills_list_langs_profile` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_save_lang_profile` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_reset_lang_profile` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_lang_catalog` | UNGATED_MUTATE | — | — | True |
| `backend/project_skill.rs` | `skills_marketplace_preview` | GATED | ensure_unlocked | — | False |
| `backend/project_skill.rs` | `skills_marketplace_install` | GATED | ensure_unlocked | marketplace_install_impl→ensure_unlocked | True |
| `backend/project_skill.rs` | `skills_save_profile` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `list_projects` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `get_project` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `create_project` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `update_project_metadata` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `delete_project` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `create_project_task` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `move_project_task` | GATED | ensure_unlocked | — | False |
| `backend/projects.rs` | `plan_task_control` | GATED | ensure_unlocked | — | False |
| `backend/projects.rs` | `append_project_note` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `add_project_milestone` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `remove_project_milestone` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `refresh_project_live_status` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_custom_agent_clients` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_mini_coder_backend` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `get_design_llm_backend` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_design_llm_backend` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_local_coder_backend` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `get_mini_write_behavior` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_mini_write_behavior` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `get_cloud_cli_availability` | GATED | ensure_unlocked | — | False |
| `backend/projects.rs` | `get_agentic_coverage_languages` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_censor_local_ai` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `launch_project_agent_terminal` | GATED | — | prepare_or_launch_project_agent→ensure_unlocked | False |
| `backend/projects.rs` | `prepare_project_agent_prompt` | GATED | — | prepare_or_launch_project_agent→ensure_unlocked | True |
| `backend/projects.rs` | `set_project_sandbox_mode_cmd` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_project_main_coder_override_cmd` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `set_project_agent_controls_cmd` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `grant_net_consent` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `respond_cloud_consent` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `add_project_working_set_folder_cmd` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `remove_project_working_set_folder_cmd` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `grant_folder_consent` | GATED | ensure_unlocked | — | True |
| `backend/projects.rs` | `detect_project_language` | GATED | ensure_unlocked | — | False |
| `backend/projects.rs` | `orchestrator_steer` | UNGATED_MUTATE | — | — | True |
| `backend/projects.rs` | `planner_reset_chat` | UNGATED_MUTATE | — | — | True |
| `backend/projects.rs` | `consent_requests_list` | GATED | ensure_unlocked | — | False |
| `backend/provider_detect.rs` | `detect_providers` | UNGATED_READ | — | — | False |
| `backend/provider_detect.rs` | `detect_dependencies` | UNGATED_READ | — | — | False |
| `backend/roles.rs` | `get_local_role` | GATED | ensure_unlocked | — | False |
| `backend/roles.rs` | `issue_role_grant` | GATED | ensure_unlocked|require_capability | — | False |
| `backend/roles.rs` | `verify_and_adopt_role_grant` | GATED | ensure_unlocked | — | True |
| `backend/roles.rs` | `bake_trust_anchor` | GATED | ensure_unlocked|require_capability | — | True |
| `backend/roles.rs` | `set_debug_role` | GATED | ensure_unlocked | — | False |
| `backend/roles_config.rs` | `get_roles_config_cmd` | GATED | ensure_unlocked | — | True |
| `backend/roles_config.rs` | `set_roles_config_cmd` | GATED | ensure_unlocked | — | True |
| `backend/roles_config.rs` | `set_main_coder_backend_cmd` | GATED | ensure_unlocked | — | False |
| `backend/roles_config.rs` | `set_verifier_backend_cmd` | GATED | ensure_unlocked | — | False |
| `backend/saved_workflows.rs` | `list_saved_workflows` | GATED | ensure_unlocked | — | False |
| `backend/token_usage.rs` | `get_agent_token_usage` | GATED | ensure_unlocked | — | True |
| `backend/tools_assignment.rs` | `tools_assignment_list` | GATED | ensure_unlocked | — | False |
| `backend/tools_assignment.rs` | `tools_assignment_set` | GATED | ensure_unlocked | — | False |
| `backend/tools_assignment.rs` | `tools_library_list` | GATED | ensure_unlocked | — | False |
| `backend/user_mcp_config.rs` | `user_mcp_list` | GATED | ensure_unlocked | — | False |
| `backend/user_mcp_config.rs` | `user_mcp_add` | GATED | ensure_unlocked | — | True |
| `backend/user_mcp_config.rs` | `user_mcp_remove` | GATED | ensure_unlocked | — | False |
| `backend/user_mcp_config.rs` | `user_mcp_set_enabled` | GATED | ensure_unlocked | — | False |
| `backend/workspace.rs` | `get_workspace_hygiene_snapshot` | GATED | ensure_unlocked | — | False |
| `backend/workspace.rs` | `scan_workspace_hygiene` | GATED | ensure_unlocked | — | False |
| `backend/workspace.rs` | `get_workspace_package_snapshot` | GATED | ensure_unlocked | — | False |
| `backend/workspace.rs` | `create_workspace_bootstrap_package` | GATED | ensure_unlocked|require_capability | — | False |
| `backend/workspace.rs` | `decrypt_workspace_bootstrap_package` | GATED | ensure_unlocked | — | False |
| `backend/workspace.rs` | `download_workspace_bootstrap_package` | GATED | ensure_unlocked | — | False |
| `oracle/commands.rs` | `get_oracle_runtime_setup` | GATED | require_graph_auth | require_graph_auth→ensure_unlocked | True |
| `oracle/commands.rs` | `install_oracle_runtime` | GATED | require_graph_auth | require_graph_auth→ensure_unlocked | False |
| `oracle/commands.rs` | `get_oracle_snapshot` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | False |
| `oracle/commands.rs` | `ask_oracle` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | False |
| `oracle/commands.rs` | `localize_card_suspects` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | True |
| `oracle/commands.rs` | `get_oracle_node` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked|encode_oracle_path_segment→require_graph_auth | False |
| `oracle/commands.rs` | `get_oracle_similar` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked|encode_oracle_path_segment→require_graph_auth | False |
| `oracle/commands.rs` | `get_oracle_duplicates` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | False |
| `oracle/commands.rs` | `get_oracle_coverage` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | False |
| `oracle/commands.rs` | `get_oracle_runtime` | GATED | require_graph_auth_and_enabled | require_graph_auth_and_enabled→require_graph_auth | False |
| `oracle/commands.rs` | `get_oracle_index_status` | GATED | require_graph_auth_and_enabled | require_graph_auth_and_enabled→require_graph_auth | True |
| `oracle/commands.rs` | `get_oracle_indexed_files` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | True |
| `oracle/commands.rs` | `get_oracle_doctor` | GATED | require_oracle_auth | require_oracle_auth→ensure_unlocked | False |
| `oracle/commands.rs` | `sync_oracle_text_chunks` | GATED | require_graph_auth_and_enabled | require_graph_auth_and_enabled→require_graph_auth | True |
| `oracle/commands.rs` | `start_oracle_index_job` | GATED | require_graph_auth_and_enabled | require_graph_auth_and_enabled→require_graph_auth | True |
| `oracle/commands.rs` | `start_oracle_index_watcher` | GATED | require_graph_auth_and_enabled | require_graph_auth_and_enabled→require_graph_auth | True |
| `oracle/commands.rs` | `stop_oracle_index_watcher` | GATED | require_graph_auth_and_enabled | require_graph_auth_and_enabled→require_graph_auth | True |
| `polis/commands.rs` | `generate_city_state` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked | False |
| `polis/commands.rs` | `polis_list_sins` | GATED | ensure_unlocked | — | False |
| `polis/commands.rs` | `polis_dispose_sin` | GATED | ensure_unlocked | — | False |
| `polis/commands.rs` | `polis_fix_sin` | GATED | ensure_unlocked | — | True |
| `polis/commands.rs` | `polis_debug_log` | UNGATED_MUTATE | — | — | True |
| `polis/commands.rs` | `polis_get_scan_extensions` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked | False |
| `polis/commands.rs` | `polis_set_scan_extensions` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked | True |
| `polis/commands.rs` | `trigger_file_disaster` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `resolve_file_disaster` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `set_agent_location` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `update_agent_status` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `append_city_note` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `reset_city_to_new_era` | GATED | ensure_unlocked | — | True |
| `polis/commands.rs` | `spawn_scaleway_resource` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `stop_scaleway_resource` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `refresh_scaleway_status` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `polis_start_watch` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked | True |
| `polis/commands.rs` | `polis_stop_watch` | UNGATED_READ | — | — | False |
| `polis/commands.rs` | `polis_refresh_agents` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked | True |
| `polis/commands.rs` | `polis_reclassify_features` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked|ask_oracle→require_oracle_auth | True |
| `polis/commands.rs` | `polis_get_dossier` | GATED | ensure_unlocked | — | True |
| `polis/commands.rs` | `polis_generate_dossier` | GATED | ensure_unlocked | ask_oracle→require_oracle_auth | True |
| `polis/commands.rs` | `polis_open_in_editor` | GATED | ensure_unlocked | — | True |
| `polis/commands.rs` | `polis_get_kin` | GATED | ensure_unlocked | get_agent_live_state→ensure_unlocked|generate_city_state→ensure_unlocked | False |
