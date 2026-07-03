use super::model::{
    ActivityEvent, AuthState, AuxCredentialStatus, CloudDashboardSnapshot,
    CloudflareAgentTokenProfileStatus, CloudflareAiGatewaySettings,
    CloudflareAiGatewaySettingsPatch, CloudflareAutoragReindexResult, CloudflareBilling,
    CloudflareD1QueryResult, CloudflareEnvDryRunResult, CloudflareEnvWriteResult,
    CloudflareKvKeysPage, CloudflareKvValue, CloudflareKvWriteResult, CloudflareR2Config,
    CloudflareR2WriteResult, CloudflareSmokeDryRunResult, CloudflareWorkerSettings,
    CloudflareWorkerSummary, DashboardKpi, OracleIndexPreferences, OracleLlmSettings,
    OracleLlmSettingsStatus, ProviderConnectionAudit, ProviderConsoleResourceSummary,
    ProviderHealth, ProviderId, ProviderScopeSelection, ProviderScopeStatus,
    ProviderServiceSummary, RiskFlag, ScalewayActionResult, ScalewayBilling,
    ScalewayInstanceCreateRequest, ScalewayInstanceDryRunResult, ScalewayOfferSummary,
    ScalewayResourceSummary, ScalewayStorageSummary, SecretRotationResult, SecretStatus,
};
use super::providers::{
    cloudflare_ai_gateway_exists, cloudflare_autorag_instance_exists,
    cloudflare_d1_database_exists, cloudflare_env_dry_run as cloudflare_env_dry_run_compute,
    cloudflare_kv_namespace_exists, cloudflare_r2_bucket_exists,
    cloudflare_worker_name_in_aspis_bio_scope, create_scaleway_block_snapshot_request,
    create_scaleway_block_volume_request, create_scaleway_container_namespace_request,
    create_scaleway_container_request, create_scaleway_filesystem_request,
    create_scaleway_function_namespace_request, create_scaleway_function_request,
    create_scaleway_instance_request, create_scaleway_object_bucket_request,
    create_scaleway_sql_database_request, d1_sql_is_write,
    delete_cloudflare_kv_value as delete_cloudflare_kv_value_request,
    delete_scaleway_block_snapshot_request, delete_scaleway_block_volume_request,
    delete_scaleway_container_request, delete_scaleway_filesystem_request,
    delete_scaleway_function_request, delete_scaleway_object_bucket_request,
    delete_scaleway_sql_database_request, fetch_cloudflare,
    fetch_cloudflare_ai_gateway_settings as fetch_cloudflare_ai_gateway_settings_request,
    fetch_cloudflare_billing as fetch_cloudflare_billing_request,
    fetch_cloudflare_kv_keys as fetch_cloudflare_kv_keys_request,
    fetch_cloudflare_kv_value as fetch_cloudflare_kv_value_request,
    fetch_cloudflare_platform_inventory,
    fetch_cloudflare_r2_config as fetch_cloudflare_r2_config_request,
    fetch_cloudflare_worker_settings as fetch_cloudflare_worker_settings_request, fetch_scaleway,
    fetch_scaleway_billing_request, fetch_scaleway_extended_console_resources,
    fetch_scaleway_iam_console_resources, fetch_scaleway_namespace_request, fetch_scaleway_offers,
    patch_cloudflare_worker_plain_text, perform_scaleway_resource_action_request,
    put_cloudflare_ai_gateway_settings, put_cloudflare_kv_value, put_cloudflare_r2_config,
    resize_scaleway_block_volume_request, resolve_scaleway_org_id, run_cloudflare_d1_query,
    sanitize_error_message, scaleway_block_resize_is_allowed, scaleway_instance_create_body,
    scaleway_instance_offer_cost, scaleway_uuid_is_valid,
    set_scaleway_object_bucket_lifecycle_request, trigger_cloudflare_autorag_reindex,
    CloudflarePlatformCounts, CloudflarePlatformInventory, ProviderInventory,
    ScalewayBlockCreateRequest, ScalewayContainerCreateRequest, ScalewayExtendedInventory,
    ScalewayFunctionCreateRequest, ScalewaySqlCreateRequest,
};
use super::state::BackendState;
use super::vault;
use chrono::Utc;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tauri::State;

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::super::providers::ProviderInventory;
    use super::*;

    #[test]
    fn provider_token_validation_rejects_error_inventory() {
        let inventory = ProviderInventory::error(
            ProviderId::Cloudflare,
            "Cloudflare token verify rejected: 401 Unauthorized".into(),
        );

        let result = provider_token_validation_result(ProviderId::Cloudflare, &inventory);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Cloudflare token verify rejected: 401 Unauthorized"
        );
    }

    #[test]
    fn secret_rotation_request_rejects_invalid_binding_names() {
        let result = validate_cloudflare_secret_rotation_request(
            "023e105f4ecef8ad9ca31a8372d0c353",
            "worker-name",
            "bad-name",
            "long-enough-secret",
        );

        assert_eq!(
            result.unwrap_err(),
            "Secret binding name must be a valid JavaScript identifier."
        );
    }

    #[test]
    fn provider_scope_validation_rejects_unsafe_values() {
        assert_eq!(
            validate_provider_scope_value(ProviderId::Scaleway, "bio-project").unwrap(),
            "bio-project"
        );
        assert_eq!(
            validate_provider_scope_value(
                ProviderId::Cloudflare,
                "023e105f4ecef8ad9ca31a8372d0c353"
            )
            .unwrap(),
            "023e105f4ecef8ad9ca31a8372d0c353"
        );
        assert!(validate_provider_scope_value(ProviderId::Cloudflare, "").is_err());
        assert!(validate_provider_scope_value(ProviderId::Cloudflare, "bio-account").is_err());
        assert!(validate_provider_scope_value(ProviderId::Cloudflare, "../account").is_err());
        assert!(validate_provider_scope_value(ProviderId::Scaleway, &"x".repeat(129)).is_err());
    }

    #[test]
    fn provider_connection_audit_omits_token_and_reports_scope() {
        let inventory = ProviderInventory {
            health: ProviderHealth {
                id: ProviderId::Cloudflare,
                name: "Cloudflare".into(),
                status: "degraded".into(),
                last_sync: Some("2026-05-27T00:00:00Z".into()),
                token_health: "valid_read_only".into(),
                credential_kind: Some("cloudflare_profile_token".into()),
                resource_count: 2,
                message: Some("Read-only token.".into()),
            },
            workers: vec![cloudflare_worker("worker-1"), cloudflare_worker("worker-2")],
            compute: Vec::new(),
            storage: Vec::new(),
            risks: vec![RiskFlag {
                id: "risk-1".into(),
                severity: "medium".into(),
                title: "Write unavailable".into(),
                description: "Secret rotation requires write.".into(),
                source: "Cloudflare".into(),
                timestamp: "2026-05-27T00:00:00Z".into(),
            }],
            activity: Vec::new(),
            selected_scope: Some(super::super::model::ProviderScopeSelection {
                provider: ProviderId::Cloudflare,
                id: "account-1".into(),
                name: Some("Aspis Bio".into()),
                source: "pinned".into(),
            }),
        };

        let audit = provider_connection_audit(ProviderId::Cloudflare, &inventory);
        let raw = serde_json::to_string(&audit).unwrap();

        assert_eq!(audit.status, "degraded");
        assert_eq!(audit.resource_count, 2);
        assert_eq!(audit.selected_scope.unwrap().id, "account-1");
        assert!(raw.contains("Write unavailable"));
        assert!(!raw.contains("Bearer"));
        assert!(!raw.contains("tokenValue"));
    }

    #[test]
    fn provider_token_storage_allows_cloudflare_inventory_only_tokens() {
        assert!(provider_token_health_is_storable(
            ProviderId::Cloudflare,
            "valid"
        ));
        assert!(provider_token_health_is_storable(
            ProviderId::Cloudflare,
            "valid_read_only"
        ));
        assert!(provider_token_health_is_storable(
            ProviderId::Cloudflare,
            "valid_unverified"
        ));
        assert!(!provider_token_health_is_storable(
            ProviderId::Scaleway,
            "valid_read_only"
        ));
        assert!(!provider_token_health_is_storable(
            ProviderId::Scaleway,
            "valid_unverified"
        ));
    }

    #[test]
    fn missing_saved_provider_connection_audit_omits_token() {
        let audit = missing_saved_provider_connection_audit(ProviderId::Scaleway);
        let raw = serde_json::to_string(&audit).unwrap();

        assert_eq!(audit.status, "error");
        assert_eq!(audit.token_health, "missing");
        assert!(audit.message.unwrap().contains("Stored Scaleway token"));
        assert!(!raw.contains("Bearer"));
        assert!(!raw.contains("tokenValue"));
    }

    #[test]
    fn cloudflare_rotation_scope_guard_rejects_unverified_single_account() {
        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Cloudflare);
        inventory.selected_scope = Some(super::super::model::ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-1".into(),
            name: Some("Personal Account".into()),
            source: "single_account_token".into(),
        });

        state.replace_provider_inventory(inventory).unwrap();

        assert!(cloudflare_rotation_scope_guard(&state, "account-1").is_err());
    }

    #[test]
    fn cloudflare_rotation_scope_guard_accepts_explicit_pin_with_personal_account_name() {
        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Cloudflare);
        inventory.selected_scope = Some(super::super::model::ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-1".into(),
            name: Some("Aspis Launcher".into()),
            source: "pinned".into(),
        });

        state.replace_provider_inventory(inventory).unwrap();

        assert!(cloudflare_rotation_scope_guard(&state, "account-1").is_ok());
    }

    #[test]
    fn cloudflare_rotation_scope_guard_accepts_aspis_bio_scope() {
        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Cloudflare);
        inventory.selected_scope = Some(super::super::model::ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-1".into(),
            name: Some("Aspis Bio".into()),
            source: "single_account_token".into(),
        });

        state.replace_provider_inventory(inventory).unwrap();

        assert!(cloudflare_rotation_scope_guard(&state, "account-1").is_ok());
    }

    #[test]
    fn scaleway_destructive_action_requires_resource_name_confirmation() {
        let resource = scaleway_resource("srv-1", "trainer-a", "GPU", "running");

        assert!(validate_scaleway_action_request(&resource, "delete", Some("wrong")).is_err());
        assert!(validate_scaleway_action_request(&resource, "delete", Some("trainer-a")).is_ok());
        assert!(
            validate_scaleway_action_request(&resource, "terminate", Some("trainer-a")).is_err()
        );
    }

    #[test]
    fn scaleway_action_request_is_a_strict_allowlist() {
        let resource = scaleway_resource("srv-1", "trainer-a", "GPU", "running");

        // Non-destructive lifecycle verbs on the allowlist pass without a name.
        // `deploy` (Serverless functions/containers) is non-destructive too: it must
        // be accepted with no confirm-by-name.
        for action in ["start", "stop", "reboot", "poweron", "poweroff", "deploy"] {
            assert!(
                validate_scaleway_action_request(&resource, action, None).is_ok(),
                "expected allow for {action:?}"
            );
        }
        // Anything off the allowlist is rejected outright (no longer forwarded).
        for action in ["force_wipe", "snapshot", "migrate", "", "rebootnow"] {
            assert!(
                validate_scaleway_action_request(&resource, action, None).is_err(),
                "expected reject for {action:?}"
            );
        }
        // `delete` stays confirm-by-name gated even though it is allowlisted.
        assert!(validate_scaleway_action_request(&resource, "delete", None).is_err());
        assert!(validate_scaleway_action_request(&resource, "delete", Some("wrong")).is_err());
        assert!(validate_scaleway_action_request(&resource, "delete", Some("trainer-a")).is_ok());
        // `terminate` remains rejected (the UI must route destruction via delete).
        assert!(
            validate_scaleway_action_request(&resource, "terminate", Some("trainer-a")).is_err()
        );
    }

    #[test]
    fn scaleway_action_guard_blocks_stale_or_unknown_actions() {
        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Scaleway);
        inventory.health.status = "error".into();
        inventory
            .compute
            .push(scaleway_resource("srv-1", "trainer-a", "GPU", "running"));
        state.replace_provider_inventory(inventory).unwrap();

        assert!(scaleway_action_inventory_guard(&state, "srv-1", "delete").is_err());

        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Scaleway);
        inventory.health.status = "healthy".into();
        inventory
            .compute
            .push(scaleway_resource("srv-1", "trainer-a", "GPU", "running"));
        state.replace_provider_inventory(inventory).unwrap();

        assert!(scaleway_action_inventory_guard(&state, "srv-1", "delete").is_err());
    }

    #[test]
    fn cloudflare_rotation_requires_in_scope_worker_name() {
        // C4
        assert!(cloudflare_worker_name_in_aspis_bio_scope("aspis-bio-api"));
        assert!(cloudflare_worker_name_in_aspis_bio_scope(
            "aspis-bio-custom"
        ));
        assert!(!cloudflare_worker_name_in_aspis_bio_scope(
            "aspis-food-worker"
        ));
        assert!(!cloudflare_worker_name_in_aspis_bio_scope("worker-name"));
    }

    #[test]
    fn scaleway_action_reasserts_pinned_project_scope() {
        // C1
        let resource = scaleway_resource("srv-1", "trainer-a", "GPU", "running");
        // project_id == pinned -> ok
        assert!(assert_scaleway_resource_in_pinned_project(&resource, Some("bio-project")).is_ok());
        // project_id != pinned -> hard fail
        assert!(
            assert_scaleway_resource_in_pinned_project(&resource, Some("attacker-project"))
                .is_err()
        );
        // no pinned project -> refuse
        assert!(assert_scaleway_resource_in_pinned_project(&resource, None).is_err());

        // resource with unknown project -> refuse even if a project is pinned
        let mut unknown = scaleway_resource("srv-2", "trainer-b", "GPU", "running");
        unknown.project_id = None;
        assert!(assert_scaleway_resource_in_pinned_project(&unknown, Some("bio-project")).is_err());
    }

    #[test]
    fn scaleway_storage_delete_requires_resource_name_confirmation() {
        let volume = scaleway_storage_summary("vol-1", "data-vol", "Block Storage 5K");

        // wrong name -> rejected
        assert!(
            validate_scaleway_storage_action_request(&volume, "delete", Some("wrong")).is_err()
        );
        // exact name -> accepted
        assert!(
            validate_scaleway_storage_action_request(&volume, "delete", Some("data-vol")).is_ok()
        );
        // missing confirmation -> rejected
        assert!(validate_scaleway_storage_action_request(&volume, "delete", None).is_err());
        // empty/whitespace confirmation -> rejected
        assert!(validate_scaleway_storage_action_request(&volume, "delete", Some("   ")).is_err());
        // terminate alias is refused (delete is the only destructive verb)
        assert!(
            validate_scaleway_storage_action_request(&volume, "terminate", Some("data-vol"))
                .is_err()
        );
    }

    #[test]
    fn scaleway_storage_action_reasserts_pinned_project_scope() {
        let volume = scaleway_storage_summary("vol-1", "data-vol", "Block Storage 5K");

        // project_id == pinned -> ok
        assert!(assert_scaleway_storage_in_pinned_project(&volume, Some("bio-project")).is_ok());
        // project_id != pinned -> hard fail
        assert!(
            assert_scaleway_storage_in_pinned_project(&volume, Some("attacker-project")).is_err()
        );
        // no pinned project -> refuse
        assert!(assert_scaleway_storage_in_pinned_project(&volume, None).is_err());

        // storage with unknown project -> refuse even if a project is pinned
        let mut unknown = scaleway_storage_summary("vol-2", "other-vol", "File System");
        unknown.project_id = None;
        assert!(assert_scaleway_storage_in_pinned_project(&unknown, Some("bio-project")).is_err());
    }

    #[test]
    fn scaleway_serverless_create_target_project_must_equal_pinned() {
        // SQL / function / container CREATE all funnel through the same CREATE
        // guard: the target project_id MUST equal the pinned project or the
        // create is refused BEFORE any network call.
        assert!(
            assert_scaleway_create_project_is_pinned("bio-project", Some("bio-project")).is_ok()
        );
        assert!(
            assert_scaleway_create_project_is_pinned("attacker-project", Some("bio-project"))
                .is_err()
        );
        assert!(assert_scaleway_create_project_is_pinned("bio-project", None).is_err());
    }

    #[test]
    fn scaleway_serverless_delete_requires_resource_name_confirmation() {
        // Functions and containers are "Serverless"; SQL is "Serverless SQL".
        // Their delete reuses the compute-side confirm-by-name validator.
        for resource_type in ["Serverless", "Serverless SQL"] {
            let resource = scaleway_resource("res-1", "ingest-fn", resource_type, "running");
            // `deploy` is the Serverless redeploy verb: accepted with no confirm name.
            assert!(validate_scaleway_action_request(&resource, "deploy", None).is_ok());
            // wrong name -> rejected
            assert!(validate_scaleway_action_request(&resource, "delete", Some("wrong")).is_err());
            // exact name -> accepted
            assert!(
                validate_scaleway_action_request(&resource, "delete", Some("ingest-fn")).is_ok()
            );
            // missing confirmation -> rejected
            assert!(validate_scaleway_action_request(&resource, "delete", None).is_err());
            // empty/whitespace confirmation -> rejected
            assert!(validate_scaleway_action_request(&resource, "delete", Some("   ")).is_err());
            // terminate alias is refused (delete is the only destructive verb)
            assert!(
                validate_scaleway_action_request(&resource, "terminate", Some("ingest-fn"))
                    .is_err()
            );
        }
    }

    #[test]
    fn scaleway_create_target_project_must_equal_pinned() {
        // A create whose target project_id matches the pin -> ok.
        assert!(
            assert_scaleway_create_project_is_pinned("bio-project", Some("bio-project")).is_ok()
        );
        // Mismatch -> HARD FAIL (cannot create into another project).
        assert!(
            assert_scaleway_create_project_is_pinned("attacker-project", Some("bio-project"))
                .is_err()
        );
        // No pin configured -> refuse rather than create unscoped.
        assert!(assert_scaleway_create_project_is_pinned("bio-project", None).is_err());
        // Empty target -> refuse.
        assert!(assert_scaleway_create_project_is_pinned("   ", Some("bio-project")).is_err());
    }

    fn instance_create_request(
        name: &str,
        zone: &str,
        commercial_type: &str,
        image: &str,
        project_id: &str,
    ) -> ScalewayInstanceCreateRequest {
        ScalewayInstanceCreateRequest {
            name: name.into(),
            zone: zone.into(),
            commercial_type: commercial_type.into(),
            image: image.into(),
            project_id: project_id.into(),
            dynamic_ip_required: true,
            tags: vec!["alpha".into()],
        }
    }

    const VALID_IMAGE: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const VALID_PROJECT: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn scaleway_instance_validation_accepts_well_formed_request() {
        let req =
            instance_create_request("trainer-a", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        let validated =
            validate_scaleway_instance_request(&req, Some(VALID_PROJECT)).expect("should validate");
        assert_eq!(validated.name, "trainer-a");
        assert_eq!(validated.zone, "fr-par-1");
        assert_eq!(validated.commercial_type, "GP1-S");
        assert_eq!(validated.image, VALID_IMAGE);
        assert_eq!(validated.project_id, VALID_PROJECT);
        assert!(validated.dynamic_ip_required);
    }

    #[test]
    fn scaleway_instance_validation_rejects_bad_inputs() {
        // Bad zone (contains '@').
        let bad_zone =
            instance_create_request("srv", "fr-par-1@", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        assert!(validate_scaleway_instance_request(&bad_zone, Some(VALID_PROJECT)).is_err());

        // Non-UUID image.
        let bad_image =
            instance_create_request("srv", "fr-par-1", "GP1-S", "ubuntu_noble", VALID_PROJECT);
        assert!(validate_scaleway_instance_request(&bad_image, Some(VALID_PROJECT)).is_err());

        // Empty name.
        let empty_name =
            instance_create_request("   ", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        assert!(validate_scaleway_instance_request(&empty_name, Some(VALID_PROJECT)).is_err());

        // Empty commercial_type.
        let empty_type =
            instance_create_request("srv", "fr-par-1", "  ", VALID_IMAGE, VALID_PROJECT);
        assert!(validate_scaleway_instance_request(&empty_type, Some(VALID_PROJECT)).is_err());

        // Non-UUID project id.
        let bad_project =
            instance_create_request("srv", "fr-par-1", "GP1-S", VALID_IMAGE, "not-a-uuid");
        assert!(validate_scaleway_instance_request(&bad_project, Some(VALID_PROJECT)).is_err());
    }

    #[test]
    fn scaleway_instance_validation_bounds_name_type_and_tags() {
        // FIX 3: a well-formed request (short name/type, ≤20 tags each ≤128 bytes) passes.
        let mut ok =
            instance_create_request("srv", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        ok.tags = vec!["a".repeat(128), "b".into()];
        assert!(validate_scaleway_instance_request(&ok, Some(VALID_PROJECT)).is_ok());

        // name > 255 bytes -> reject.
        let long_name = instance_create_request(
            &"n".repeat(256),
            "fr-par-1",
            "GP1-S",
            VALID_IMAGE,
            VALID_PROJECT,
        );
        assert!(validate_scaleway_instance_request(&long_name, Some(VALID_PROJECT)).is_err());

        // commercial_type > 64 bytes -> reject.
        let long_type = instance_create_request(
            "srv",
            "fr-par-1",
            &"T".repeat(65),
            VALID_IMAGE,
            VALID_PROJECT,
        );
        assert!(validate_scaleway_instance_request(&long_type, Some(VALID_PROJECT)).is_err());

        // a single tag > 128 bytes -> reject.
        let mut long_tag =
            instance_create_request("srv", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        long_tag.tags = vec!["x".repeat(129)];
        assert!(validate_scaleway_instance_request(&long_tag, Some(VALID_PROJECT)).is_err());

        // more than 20 tags -> reject.
        let mut too_many =
            instance_create_request("srv", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        too_many.tags = (0..21).map(|i| format!("t{i}")).collect();
        assert!(validate_scaleway_instance_request(&too_many, Some(VALID_PROJECT)).is_err());
    }

    #[test]
    fn scaleway_instance_create_target_project_must_equal_pinned() {
        // Target project != pinned -> HARD FAIL before any network call.
        let req = instance_create_request("srv", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        let other_pin = "99999999-8888-7777-6666-555555555555";
        assert!(validate_scaleway_instance_request(&req, Some(other_pin)).is_err());
        // No pin configured -> refuse.
        assert!(validate_scaleway_instance_request(&req, None).is_err());
        // Matching pin -> ok.
        assert!(validate_scaleway_instance_request(&req, Some(VALID_PROJECT)).is_ok());
    }

    #[test]
    fn scaleway_instance_dry_run_computes_cost_from_catalog() {
        let req =
            instance_create_request("trainer-a", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        let validated =
            validate_scaleway_instance_request(&req, Some(VALID_PROJECT)).expect("validate");
        let offers = vec![ScalewayOfferSummary {
            id: "fr-par-1:GP1-S".into(),
            name: "GP1-S".into(),
            zone: "fr-par-1".into(),
            category: "CPU VM".into(),
            architecture: "x86_64".into(),
            vcpus: 4,
            memory_gb: 16.0,
            gpu_count: 0,
            gpu_label: None,
            hourly_price_eur: Some(0.12),
            monthly_price_eur: Some(87.6),
            availability: "available".into(),
            tags: Vec::new(),
        }];
        let result = build_scaleway_instance_dry_run(&validated, &offers);
        assert_eq!(result.estimated_hourly_eur, Some(0.12));
        assert_eq!(result.estimated_monthly_eur, Some(87.6));
        assert_eq!(result.project_id, VALID_PROJECT);
        assert_eq!(result.zone, "fr-par-1");
        // body_preview carries `project` (not `project_id`) and the chosen type.
        assert!(result.body_preview.contains("\"project\""));
        assert!(!result.body_preview.contains("\"project_id\""));
        assert!(result.body_preview.contains("GP1-S"));
        // FIX 5: pin the project VALUE, not just the key — catch a regression that
        // emits a wrong/empty project in the body.
        assert!(result.body_preview.contains(VALID_PROJECT));
    }

    #[test]
    fn scaleway_instance_dry_run_missing_offer_yields_none_and_risk() {
        let req =
            instance_create_request("trainer-a", "fr-par-1", "GP1-S", VALID_IMAGE, VALID_PROJECT);
        let validated =
            validate_scaleway_instance_request(&req, Some(VALID_PROJECT)).expect("validate");
        // Empty catalog -> no fabricated cost, a risk note instead.
        let result = build_scaleway_instance_dry_run(&validated, &[]);
        assert_eq!(result.estimated_hourly_eur, None);
        assert_eq!(result.estimated_monthly_eur, None);
        assert!(result
            .risks
            .iter()
            .any(|r| r.to_lowercase().contains("offer")));
    }

    #[test]
    fn scaleway_scale_requires_max_when_min_is_positive() {
        // FIX 5: an explicit min>0 with NO max is ambiguous — the API may default
        // max below min and reject the create. Require max whenever min>0.
        assert!(validate_scaleway_scale(Some(2), None).is_err());
        assert!(validate_scaleway_scale(Some(1), None).is_err());
        // min == 0 (or absent) with no max is fine (scale-to-zero, API default max).
        assert!(validate_scaleway_scale(Some(0), None).is_ok());
        assert!(validate_scaleway_scale(None, None).is_ok());
        // min>0 WITH a valid max >= min -> ok.
        assert_eq!(
            validate_scaleway_scale(Some(2), Some(5)).unwrap(),
            (Some(2), Some(5))
        );
        // min>0 with max<min -> still rejected by the existing min<=max guard.
        assert!(validate_scaleway_scale(Some(5), Some(2)).is_err());
        // max alone (min absent) -> ok.
        assert_eq!(
            validate_scaleway_scale(None, Some(3)).unwrap(),
            (None, Some(3))
        );
    }

    #[test]
    fn scaleway_serverless_kind_disambiguates_function_and_container() {
        // FIX 4: a `Serverless` resource is a FUNCTION when its runtime is a
        // language runtime, a CONTAINER when its runtime is `container`/`container/*`,
        // and AMBIGUOUS (refused) when the runtime is missing/empty.
        let mut func = scaleway_resource("fn-1", "ingest-fn", "Serverless", "running");
        func.runtime = Some("python311".into());
        assert_eq!(
            scaleway_serverless_kind(&func).unwrap(),
            ScalewayServerlessKind::Function
        );

        let mut container = scaleway_resource("ct-1", "edge-ct", "Serverless", "running");
        container.runtime = Some("container/http1".into());
        assert_eq!(
            scaleway_serverless_kind(&container).unwrap(),
            ScalewayServerlessKind::Container
        );

        let mut bare_container = scaleway_resource("ct-2", "edge-ct2", "Serverless", "running");
        bare_container.runtime = Some("container".into());
        assert_eq!(
            scaleway_serverless_kind(&bare_container).unwrap(),
            ScalewayServerlessKind::Container
        );

        // runtime None -> ambiguous -> refused.
        let none_runtime = scaleway_resource("amb-1", "amb", "Serverless", "running");
        assert!(none_runtime.runtime.is_none());
        assert!(scaleway_serverless_kind(&none_runtime).is_err());

        // runtime empty/whitespace -> ambiguous -> refused.
        let mut empty_runtime = scaleway_resource("amb-2", "amb2", "Serverless", "running");
        empty_runtime.runtime = Some("   ".into());
        assert!(scaleway_serverless_kind(&empty_runtime).is_err());
    }

    #[test]
    fn scaleway_namespace_project_matches_rejects_foreign_project() {
        // FIX 1: an explicit namespace_id must belong to the pinned project. The
        // project-equality check is a pure helper over the namespace GET payload.
        let pinned_ns = serde_json::json!({ "id": "ns-1", "project_id": "bio-project" });
        let foreign_ns = serde_json::json!({ "id": "ns-2", "project_id": "attacker-project" });
        assert!(scaleway_namespace_project_matches(
            &pinned_ns,
            "bio-project"
        ));
        assert!(!scaleway_namespace_project_matches(
            &foreign_ns,
            "bio-project"
        ));
        // Missing/blank project_id on the namespace -> no match (fail closed).
        let no_project = serde_json::json!({ "id": "ns-3" });
        assert!(!scaleway_namespace_project_matches(
            &no_project,
            "bio-project"
        ));
        let blank_project = serde_json::json!({ "id": "ns-4", "project_id": "  " });
        assert!(!scaleway_namespace_project_matches(
            &blank_project,
            "bio-project"
        ));
    }

    #[test]
    fn provider_service_catalog_surfaces_console_map_and_scaleway_offers() {
        let parts = SnapshotInventoryParts {
            provider_health: vec![
                provider_health(ProviderId::Cloudflare, "healthy", 1),
                provider_health(ProviderId::Scaleway, "healthy", 1),
            ],
            selected_scopes: vec![ProviderScopeSelection {
                provider: ProviderId::Scaleway,
                id: "project-1".into(),
                name: Some("aspis-bio".into()),
                source: "pinned".into(),
            }],
            workers: vec![cloudflare_worker("worker-1")],
            compute: vec![scaleway_resource("srv-1", "trainer-a", "GPU", "running")],
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
        };
        let offers = vec![ScalewayOfferSummary {
            id: "fr-par-1:GPU-H100".into(),
            name: "GPU-H100".into(),
            zone: "fr-par-1".into(),
            category: "GPU".into(),
            architecture: "x86_64".into(),
            vcpus: 24,
            memory_gb: 128.0,
            gpu_count: 1,
            gpu_label: Some("H100".into()),
            hourly_price_eur: Some(2.5),
            monthly_price_eur: Some(1800.0),
            availability: "available".into(),
            tags: vec!["available".into()],
        }];

        let services = provider_service_catalog(
            &parts,
            &offers,
            &CloudflarePlatformCounts {
                r2_buckets: 1,
                d1_databases: 1,
                ..CloudflarePlatformCounts::default()
            },
            1,
            1,
            2,
            1,
            1,
        );

        let workers = services
            .iter()
            .find(|service| service.id == "cf-workers-pages")
            .unwrap();
        assert_eq!(workers.status, "partial");
        assert_eq!(workers.live_count, 1);

        let spawnable = services
            .iter()
            .find(|service| service.id == "scw-spawnable-offers")
            .unwrap();
        assert_eq!(spawnable.status, "live");
        assert_eq!(spawnable.live_count, 1);
        assert!(spawnable.notes[0].contains("1 GPU"));

        let r2 = services
            .iter()
            .find(|service| service.id == "cf-storage-data")
            .unwrap();
        assert_eq!(r2.status, "partial");
        assert_eq!(r2.live_count, 2);
        let network = services
            .iter()
            .find(|service| service.id == "scw-network-security")
            .unwrap();
        assert_eq!(network.status, "partial");
        assert_eq!(network.live_count, 1);

        let ai = services
            .iter()
            .find(|service| service.id == "cf-ai-observability")
            .unwrap();
        assert_eq!(ai.status, "partial");
        assert_eq!(ai.live_count, 1);
    }

    #[test]
    fn provider_console_resources_include_cloudflare_data_and_scaleway_iam() {
        let parts = SnapshotInventoryParts {
            provider_health: Vec::new(),
            selected_scopes: Vec::new(),
            workers: vec![cloudflare_worker("worker-1")],
            compute: vec![scaleway_resource("srv-1", "trainer-a", "GPU", "running")],
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
        };
        let cloudflare_platform = CloudflarePlatformInventory {
            counts: CloudflarePlatformCounts::default(),
            resources: vec![ProviderConsoleResourceSummary {
                id: "cloudflare:cf-storage-data:R2 Bucket:bucket-a".into(),
                provider: ProviderId::Cloudflare,
                service_id: "cf-storage-data".into(),
                resource_type: "R2 Bucket".into(),
                name: "bucket-a".into(),
                region: None,
                status: "available".into(),
                description: "R2 bucket".into(),
                metadata: Vec::new(),
                docs_url: "https://developers.cloudflare.com/api/resources/r2/".into(),
                updated_at: None,
            }],
        };
        let iam = vec![ProviderConsoleResourceSummary {
            id: "scaleway:scw-iam-projects:Policy:policy-a".into(),
            provider: ProviderId::Scaleway,
            service_id: "scw-iam-projects".into(),
            resource_type: "Policy".into(),
            name: "policy-a".into(),
            region: None,
            status: "listed".into(),
            description: "IAM policy".into(),
            metadata: Vec::new(),
            docs_url: "https://www.scaleway.com/en/developers/api/iam".into(),
            updated_at: None,
        }];

        let resources = provider_console_resources(
            &parts,
            &[],
            &cloudflare_platform,
            &ScalewayExtendedInventory {
                resources: vec![ProviderConsoleResourceSummary {
                    id: "scaleway:scw-network-security:Private Network:pn-a".into(),
                    provider: ProviderId::Scaleway,
                    service_id: "scw-network-security".into(),
                    resource_type: "Private Network".into(),
                    name: "pn-a".into(),
                    region: Some("fr-par".into()),
                    status: "listed".into(),
                    description: "private network".into(),
                    metadata: Vec::new(),
                    docs_url: "https://www.scaleway.com/en/docs/vpc/".into(),
                    updated_at: None,
                }],
            },
            iam,
        );

        assert!(resources
            .iter()
            .any(|resource| resource.resource_type == "Worker" && resource.name == "worker-1"));
        assert!(resources
            .iter()
            .any(|resource| resource.resource_type == "R2 Bucket" && resource.name == "bucket-a"));
        assert!(resources
            .iter()
            .any(|resource| resource.resource_type == "Policy" && resource.name == "policy-a"));
        assert!(resources
            .iter()
            .any(|resource| resource.resource_type == "GPU" && resource.name == "trainer-a"));
        assert!(
            resources
                .iter()
                .any(|resource| resource.resource_type == "Private Network"
                    && resource.name == "pn-a")
        );
    }

    #[test]
    fn sync_error_preserves_last_known_provider_scope() {
        let state = BackendState::new();
        let previous = ProviderInventory {
            health: provider_health(ProviderId::Cloudflare, "healthy", 1),
            workers: vec![cloudflare_worker("worker-1")],
            compute: Vec::new(),
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: Some(super::super::model::ProviderScopeSelection {
                provider: ProviderId::Cloudflare,
                id: "account-1".into(),
                name: Some("Aspis Bio".into()),
                source: "pinned".into(),
            }),
        };
        state.replace_provider_inventory(previous).unwrap();

        let failed = ProviderInventory::error(ProviderId::Cloudflare, "temporary outage".into());
        let preserved =
            preserve_cached_resources_on_sync_error(&state, failed, Some("account-1")).unwrap();

        assert_eq!(
            preserved
                .selected_scope
                .as_ref()
                .map(|scope| scope.id.as_str()),
            Some("account-1")
        );
        assert_eq!(preserved.workers.len(), 1);
    }

    #[test]
    fn secret_rotation_response_omits_secret_value() {
        let result =
            secret_rotation_result("023e105f4ecef8ad9ca31a8372d0c353", "worker-name", "API_KEY");

        let raw = serde_json::to_string(&result).unwrap();
        let probe_secret_value = ["super", "secret", "value"].join("-");
        assert!(raw.contains("API_KEY"));
        assert!(!raw.contains(&probe_secret_value));
        assert!(!raw.contains("secretValue"));
    }

    #[test]
    fn secret_rotation_validation_returns_trimmed_values() {
        let request = validate_cloudflare_secret_rotation_request(
            "023e105f4ecef8ad9ca31a8372d0c353",
            " worker-name ",
            " API_KEY ",
            " value-with-space ",
        )
        .unwrap();

        assert_eq!(request.worker_name, "worker-name");
        assert_eq!(request.secret_name, "API_KEY");
        assert_eq!(request.secret_value, "value-with-space");
    }

    #[test]
    fn secret_rotation_activity_omits_secret_value() {
        let result =
            secret_rotation_result("023e105f4ecef8ad9ca31a8372d0c353", "worker-name", "API_KEY");

        let event = secret_rotation_activity_event(&result);
        let raw = serde_json::to_string(&event).unwrap();
        let probe_secret_value = ["super", "secret", "value"].join("-");

        assert_eq!(event.event_type, "secret");
        assert_eq!(event.source, "Cloudflare");
        assert!(event.message.contains("API_KEY"));
        assert!(event.message.contains("worker-name"));
        assert!(!raw.contains(&probe_secret_value));
        assert!(!raw.contains("secretValue"));
    }

    #[test]
    fn cloudflare_smoke_dry_run_reports_api_scope_and_write_block() {
        let inventory = ProviderInventory {
            health: ProviderHealth {
                id: ProviderId::Cloudflare,
                name: "Cloudflare".into(),
                status: "degraded".into(),
                last_sync: Some("2026-05-29T00:00:00Z".into()),
                token_health: "valid_read_only".into(),
                credential_kind: Some("cloudflare_profile_token".into()),
                resource_count: 1,
                message: Some("Read-only token.".into()),
            },
            workers: vec![cloudflare_worker("aspis-bio-api")],
            compute: Vec::new(),
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: Some(ProviderScopeSelection {
                provider: ProviderId::Cloudflare,
                id: "023e105f4ecef8ad9ca31a8372d0c353".into(),
                name: Some("Aspis Bio".into()),
                source: "pinned".into(),
            }),
        };

        let result = cloudflare_smoke_dry_run_result(&inventory);

        assert!(result.dry_run);
        assert_eq!(result.resource_count, 1);
        assert!(!result.can_rotate_worker_secret);
        assert_eq!(
            result.credential_kind.as_deref(),
            Some("cloudflare_profile_token")
        );
        assert!(result
            .blocked_reason
            .as_deref()
            .unwrap()
            .contains("Workers Scripts Write"));
        assert!(result
            .api_equivalent
            .iter()
            .any(|line| line.contains("/workers/scripts")));
        assert!(result
            .api_equivalent
            .iter()
            .any(|line| line.contains("(not executed)")));
    }

    #[test]
    fn scaleway_action_result_omits_token_and_records_resource_context() {
        let resource = scaleway_resource("srv-1", "trainer-a", "GPU", "running");
        let result = scaleway_action_result(&resource, "stop");
        let event = scaleway_action_activity_event(&result);
        let raw = serde_json::to_string(&result).unwrap();

        assert_eq!(result.provider, ProviderId::Scaleway);
        assert_eq!(result.resource_id, "srv-1");
        assert!(result.message.contains("trainer-a"));
        assert_eq!(event.event_type, "action");
        assert_eq!(event.source, "Scaleway");
        assert!(!raw.contains("X-Auth-Token"));
        assert!(!raw.contains("Bearer"));
    }

    #[test]
    fn scaleway_lifecycle_events_detect_new_resources_and_state_changes() {
        let previous = vec![scaleway_resource("vm-1", "trainer-a", "CPU VM", "stopped")];
        let current = vec![
            scaleway_resource("vm-1", "trainer-a", "CPU VM", "running"),
            scaleway_resource("gpu-1", "gpu-burst", "GPU", "provisioning"),
        ];

        let events = scaleway_lifecycle_events(&previous, &current, "2026-05-27T10:00:00Z");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "scale");
        assert!(events[0]
            .message
            .contains("trainer-a changed state stopped -> running"));
        assert_eq!(events[1].event_type, "spawn");
        assert!(events[1]
            .message
            .contains("gpu-burst appeared as provisioning"));
    }

    #[test]
    fn snapshot_activity_merge_keeps_past_lifecycle_without_duplicates() {
        let state = BackendState::new();
        let past = ActivityEvent {
            id: "scw_spawn_gpu-1_running".into(),
            message: "gpu-1 appeared as running GPU in fr-par-1.".into(),
            timestamp: "2026-05-27T10:00:00Z".into(),
            event_type: "spawn".into(),
            source: "Scaleway".into(),
        };
        let current = ActivityEvent {
            id: "scw_state_vm-1_stopped_running".into(),
            message: "vm-1 changed state stopped -> running.".into(),
            timestamp: "2026-05-27T10:01:00Z".into(),
            event_type: "scale".into(),
            source: "Scaleway".into(),
        };
        state
            .record_activity_events(&[past.clone(), current.clone()])
            .unwrap();

        let mut activity = vec![current.clone()];
        append_recent_activity_without_duplicates(&state, &mut activity).unwrap();

        assert_eq!(activity.len(), 2);
        assert_eq!(activity[0].id, current.id);
        assert_eq!(activity[1].id, past.id);
    }

    #[test]
    fn cached_inventory_snapshot_parts_keep_cloudflare_and_scaleway() {
        let selected_scope = super::super::model::ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-1".into(),
            name: Some("Aspis Bio".into()),
            source: "pinned".into(),
        };
        let cf = ProviderInventory {
            health: provider_health(ProviderId::Cloudflare, "healthy", 1),
            workers: vec![cloudflare_worker("worker-1")],
            compute: Vec::new(),
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: Some(selected_scope.clone()),
        };
        let scw = ProviderInventory {
            health: provider_health(ProviderId::Scaleway, "healthy", 1),
            workers: Vec::new(),
            compute: vec![scaleway_resource("gpu-1", "gpu-1", "GPU", "running")],
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: None,
        };

        let parts = snapshot_parts_from_provider_inventories(vec![cf, scw]);

        assert_eq!(parts.provider_health.len(), 2);
        assert_eq!(parts.selected_scopes, vec![selected_scope]);
        assert_eq!(parts.workers.len(), 1);
        assert_eq!(parts.compute.len(), 1);
        assert_eq!(parts.workers[0].name, "worker-1");
        assert_eq!(parts.compute[0].name, "gpu-1");
    }

    #[test]
    fn sync_error_preserves_last_known_provider_resources() {
        let state = BackendState::new();
        let previous = ProviderInventory {
            health: provider_health(ProviderId::Cloudflare, "healthy", 1),
            workers: vec![cloudflare_worker("worker-1")],
            compute: Vec::new(),
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: None,
        };
        state.replace_provider_inventory(previous).unwrap();

        let failed = ProviderInventory::error(ProviderId::Cloudflare, "temporary outage".into());
        let preserved = preserve_cached_resources_on_sync_error(&state, failed, None).unwrap();

        assert_eq!(preserved.health.status, "error");
        assert_eq!(preserved.health.resource_count, 1);
        assert_eq!(preserved.workers.len(), 1);
        assert_eq!(preserved.workers[0].name, "worker-1");
    }

    #[test]
    fn provider_kpi_status_follows_provider_health_not_resource_count_only() {
        let health = vec![provider_health(ProviderId::Cloudflare, "degraded", 3)];
        assert_eq!(
            provider_kpi_status(&health, ProviderId::Cloudflare, true),
            "warning"
        );

        let health = vec![provider_health(ProviderId::Cloudflare, "error", 3)];
        assert_eq!(
            provider_kpi_status(&health, ProviderId::Cloudflare, true),
            "error"
        );
    }

    #[test]
    fn sync_error_does_not_preserve_cache_for_different_pinned_scope() {
        let state = BackendState::new();
        let previous = ProviderInventory {
            health: provider_health(ProviderId::Cloudflare, "healthy", 1),
            workers: vec![cloudflare_worker("worker-1")],
            compute: Vec::new(),
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: Some(super::super::model::ProviderScopeSelection {
                provider: ProviderId::Cloudflare,
                id: "account-1".into(),
                name: Some("Aspis Bio".into()),
                source: "pinned".into(),
            }),
        };
        state.replace_provider_inventory(previous).unwrap();

        let failed = ProviderInventory::error(ProviderId::Cloudflare, "temporary outage".into());
        let preserved =
            preserve_cached_resources_on_sync_error(&state, failed, Some("account-2")).unwrap();

        assert!(preserved.workers.is_empty());
        assert_eq!(preserved.health.resource_count, 0);
    }

    #[test]
    fn caching_validated_scaleway_inventory_sets_baseline_without_activity() {
        let state = BackendState::new();
        let inventory = ProviderInventory {
            health: provider_health(ProviderId::Scaleway, "healthy", 1),
            workers: Vec::new(),
            compute: vec![scaleway_resource("gpu-1", "gpu-1", "GPU", "running")],
            storage: Vec::new(),
            risks: Vec::new(),
            activity: Vec::new(),
            selected_scope: None,
        };

        cache_validated_provider_inventory(&state, inventory).unwrap();

        let cached = state.cached_provider_inventories().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].compute.len(), 1);
        let replacement = state
            .replace_scaleway_compute(vec![scaleway_resource("gpu-1", "gpu-1", "GPU", "running")])
            .unwrap();
        assert!(replacement.had_previous_snapshot);
        assert_eq!(replacement.previous.len(), 1);
    }

    fn provider_health(
        provider: ProviderId,
        status: &str,
        resource_count: usize,
    ) -> ProviderHealth {
        ProviderHealth {
            id: provider,
            name: provider.label().into(),
            status: status.into(),
            last_sync: Some("2026-05-27T10:00:00Z".into()),
            token_health: "ok".into(),
            credential_kind: None,
            resource_count,
            message: None,
        }
    }

    fn cloudflare_worker(name: &str) -> CloudflareWorkerSummary {
        CloudflareWorkerSummary {
            id: name.into(),
            account_id: "023e105f4ecef8ad9ca31a8372d0c353".into(),
            account_name: Some("Aspis Bio".into()),
            name: name.into(),
            status: "healthy".into(),
            purpose: "test worker".into(),
            purpose_source: "test".into(),
            routes: Vec::new(),
            last_deploy: None,
            usage_model: None,
            compatibility_date: None,
            compatibility_flags: Vec::new(),
            handlers: Vec::new(),
            tags: Vec::new(),
            oracle_query: name.into(),
        }
    }

    fn scaleway_resource(
        id: &str,
        name: &str,
        resource_type: &str,
        state: &str,
    ) -> ScalewayResourceSummary {
        ScalewayResourceSummary {
            id: id.into(),
            name: name.into(),
            resource_type: resource_type.into(),
            region: "fr-par-1".into(),
            project_id: Some("bio-project".into()),
            project_name: Some("Aspis Bio".into()),
            state: state.into(),
            commercial_type: Some("DEV1-S".into()),
            runtime: None,
            min_scale: None,
            max_scale: None,
            domain_name: None,
            endpoint: None,
            privacy: None,
            purpose: format!("Resource inferred from name: {name}."),
            purpose_source: "name".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: None,
            updated_at: None,
            oracle_query: format!("{name} {resource_type}"),
            available_actions: Vec::new(),
            idle_cost_risk: false,
        }
    }

    fn scaleway_storage_summary(
        id: &str,
        name: &str,
        storage_type: &str,
    ) -> ScalewayStorageSummary {
        ScalewayStorageSummary {
            id: id.into(),
            name: name.into(),
            storage_type: storage_type.into(),
            region: "fr-par-1".into(),
            project_id: Some("bio-project".into()),
            project_name: Some("Aspis Bio".into()),
            state: "available".into(),
            size_gb: 10.0,
            price_eur_per_gb_hour: None,
            estimated_eur_month: None,
            pricing_label: String::new(),
            pricing_note: String::new(),
            created_at: None,
            updated_at: None,
            tags: Vec::new(),
            billable: true,
        }
    }

    #[test]
    fn scaleway_location_allowlist_rejects_injection_and_accepts_real_zones() {
        // FIX 1: a region/zone is interpolated raw into the S3 host and the
        // api.scaleway.com path, so it MUST be a strict zone/region grammar
        // (ascii-lowercase / digit / '-', length-capped) — never an authority
        // or query/fragment injection vector.
        for hostile in [
            "fr-par@evil.com",
            "fr-par?x=1",
            "fr-par#",
            "fr-par/x",
            "fr-par\\x",
            "FR-PAR",
            "fr_par",
            "fr par",
            "fr.par",
            &"a".repeat(33),
        ] {
            assert!(
                validate_scaleway_location(hostile, "Region").is_err(),
                "expected reject for {hostile:?}"
            );
        }
        for valid in ["fr-par", "fr-par-1", "nl-ams-2", "pl-waw-3"] {
            assert_eq!(
                validate_scaleway_location(valid, "Region").unwrap(),
                valid,
                "expected accept for {valid:?}"
            );
        }
        // Trim is applied before the allowlist check.
        assert_eq!(
            validate_scaleway_location("  fr-par-1  ", "Region").unwrap(),
            "fr-par-1"
        );
    }

    #[test]
    fn scaleway_object_bucket_name_enforces_s3_rules_on_create() {
        // FIX 2: S3 bucket names are 3-63 chars, lowercase letters/digits/hyphens
        // — no uppercase, '_', '.', '@'. A violating name guarantees a confusing
        // 400 from S3, so reject it before the signed request is built.
        for invalid in [
            "ab",            // too short (<3)
            &"a".repeat(64), // too long (>63)
            "MyBucket",      // uppercase
            "my_bucket",     // underscore
            "my.bucket",     // dot
            "my@bucket",     // at
            "my/bucket",     // slash
            "my bucket",     // whitespace
        ] {
            assert!(
                validate_scaleway_object_bucket_name(invalid).is_err(),
                "expected reject for {invalid:?}"
            );
        }
        for valid in ["abc", "aspis-bio-data", "bucket123", &"a".repeat(63)] {
            assert_eq!(
                validate_scaleway_object_bucket_name(valid).unwrap(),
                *valid,
                "expected accept for {valid:?}"
            );
        }
        // Trim is applied before validation.
        assert_eq!(
            validate_scaleway_object_bucket_name("  aspis-bio  ").unwrap(),
            "aspis-bio"
        );
    }

    #[test]
    fn scaleway_storage_inventory_guard_rejects_resource_absent_from_inventory() {
        // FIX 5: the inventory gate is the load-bearing guard for storage DELETE.
        // A resource_id NOT in the synced inventory MUST be rejected so a future
        // refactor that weakens the gate is caught here.
        let state = BackendState::new();
        let present = scaleway_storage_summary("bucket-known", "known", "Object Bucket");
        let mut inventory = ProviderInventory::missing(ProviderId::Scaleway);
        inventory.health.status = "healthy".into();
        inventory.storage = vec![present];
        state.replace_provider_inventory(inventory).unwrap();

        // A resource that IS present passes the gate.
        assert_eq!(
            scaleway_storage_inventory_guard(&state, "bucket-known")
                .unwrap()
                .id,
            "bucket-known"
        );
        // A resource that is NOT present is rejected.
        let err = scaleway_storage_inventory_guard(&state, "bucket-unknown").unwrap_err();
        assert!(
            err.contains("not in the current Scaleway inventory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn scaleway_serverless_sql_routes_to_managed_data_card() {
        // "Serverless SQL" must land on the "Managed data" card (scw-data-managed),
        // not compute-live, so the managed-data count is non-zero for SQL databases.
        let sql = scaleway_resource("sqldb-1", "aspis-bio-db", "Serverless SQL", "running");
        let console = scaleway_compute_console_resource(&sql);
        assert_eq!(console.service_id, "scw-data-managed");

        // Serverless functions stay on the serverless card.
        let func = scaleway_resource("fn-1", "aspis-fn", "Serverless", "running");
        assert_eq!(
            scaleway_compute_console_resource(&func).service_id,
            "scw-serverless"
        );

        // A plain VM stays on compute-live.
        let vm = scaleway_resource("vm-1", "vm", "CPU VM", "running");
        assert_eq!(
            scaleway_compute_console_resource(&vm).service_id,
            "scw-compute-live"
        );
    }

    #[test]
    fn cloudflare_env_request_accepts_a_valid_value_and_trims_name() {
        let (worker, var, value) =
            validate_cloudflare_env_request("  aspis-bio-api  ", "  API_BASE  ", "https://x")
                .unwrap();
        assert_eq!(worker, "aspis-bio-api");
        assert_eq!(var, "API_BASE");
        // The value is validated but NOT trimmed (whitespace may be significant).
        assert_eq!(value, "https://x");
    }

    #[test]
    fn cloudflare_env_request_rejects_empty_or_whitespace_value() {
        let err = validate_cloudflare_env_request("aspis-bio-api", "API_BASE", "   ").unwrap_err();
        assert!(err.contains("cannot be empty"), "unexpected error: {err}");
        assert!(validate_cloudflare_env_request("aspis-bio-api", "API_BASE", "").is_err());
    }

    #[test]
    fn cloudflare_env_request_rejects_oversized_value() {
        let big = "a".repeat(CF_ENV_VALUE_MAX_BYTES + 1);
        let err = validate_cloudflare_env_request("aspis-bio-api", "API_BASE", &big).unwrap_err();
        assert!(err.contains("maximum length"), "unexpected error: {err}");
        // Exactly at the limit is accepted.
        let at_limit = "a".repeat(CF_ENV_VALUE_MAX_BYTES);
        assert!(validate_cloudflare_env_request("aspis-bio-api", "API_BASE", &at_limit).is_ok());
    }

    #[test]
    fn cloudflare_env_request_rejects_control_characters() {
        let err =
            validate_cloudflare_env_request("aspis-bio-api", "API_BASE", "ab\nc").unwrap_err();
        assert!(
            err.contains("control characters"),
            "unexpected error: {err}"
        );
        assert!(validate_cloudflare_env_request("aspis-bio-api", "API_BASE", "a\0b").is_err());
    }

    #[test]
    fn cloudflare_env_write_activity_event_id_is_unique_per_write() {
        let mut first = cloudflare_env_write_result("aspis-bio-api", "API_BASE");
        first.written_at = "2026-06-01T10:00:00Z".into();
        let mut second = cloudflare_env_write_result("aspis-bio-api", "API_BASE");
        second.written_at = "2026-06-01T10:05:00Z".into();
        let id_a = cloudflare_env_write_activity_event(&first).id;
        let id_b = cloudflare_env_write_activity_event(&second).id;
        assert_ne!(
            id_a, id_b,
            "repeated writes to the same var must get distinct ids"
        );
    }
}

#[tauri::command]
pub fn get_auth_state(state: State<'_, BackendState>) -> Result<AuthState, String> {
    state.auth_state()
}

#[tauri::command]
pub fn request_unlock(
    state: State<'_, BackendState>,
    reason: Option<String>,
) -> Result<AuthState, String> {
    let message = reason.unwrap_or_else(|| "Unlock Devboule".into());
    state.verify_unlock(&message)
}

#[tauri::command]
pub fn lock_app(state: State<'_, BackendState>) -> Result<AuthState, String> {
    state.lock("manual")
}

#[tauri::command]
pub fn get_secret_status(state: State<'_, BackendState>) -> Result<Vec<SecretStatus>, String> {
    state.ensure_unlocked()?;
    vault::all_statuses()
}

#[tauri::command]
pub fn get_provider_scope_status(
    state: State<'_, BackendState>,
) -> Result<Vec<ProviderScopeStatus>, String> {
    state.ensure_unlocked()?;
    vault::all_scope_statuses()
}

#[tauri::command]
pub fn get_cloudflare_agent_token_profiles(
    state: State<'_, BackendState>,
) -> Result<Vec<CloudflareAgentTokenProfileStatus>, String> {
    state.ensure_unlocked()?;
    vault::all_cloudflare_agent_token_profile_statuses()
}

#[tauri::command]
pub fn save_cloudflare_agent_token_profile(
    state: State<'_, BackendState>,
    profile_id: String,
    token: String,
) -> Result<CloudflareAgentTokenProfileStatus, String> {
    state.ensure_unlocked()?;
    vault::save_cloudflare_agent_token_profile(&profile_id, &token)
}

#[tauri::command]
pub fn delete_cloudflare_agent_token_profile(
    state: State<'_, BackendState>,
    profile_id: String,
) -> Result<CloudflareAgentTokenProfileStatus, String> {
    state.ensure_unlocked()?;
    vault::delete_cloudflare_agent_token_profile(&profile_id)
}

#[tauri::command]
pub fn get_scaleway_object_access_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::scaleway_object_access_key_status()
}

#[tauri::command]
pub fn save_scaleway_object_access_key(
    state: State<'_, BackendState>,
    access_key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::save_scaleway_object_access_key(&access_key)?;
    state.clear_provider_inventory(ProviderId::Scaleway)?;
    Ok(status)
}

#[tauri::command]
pub fn delete_scaleway_object_access_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::delete_scaleway_object_access_key()?;
    state.clear_provider_inventory(ProviderId::Scaleway)?;
    Ok(status)
}

#[tauri::command]
pub fn get_scaleway_object_secret_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::scaleway_object_secret_key_status()
}

#[tauri::command]
pub fn save_scaleway_object_secret_key(
    state: State<'_, BackendState>,
    secret_key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::save_scaleway_object_secret_key(&secret_key)?;
    state.clear_provider_inventory(ProviderId::Scaleway)?;
    Ok(status)
}

#[tauri::command]
pub fn delete_scaleway_object_secret_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::delete_scaleway_object_secret_key()?;
    state.clear_provider_inventory(ProviderId::Scaleway)?;
    Ok(status)
}

// L2.4 — Exa web-search key for the local Devboule orchestrator. WRITE-ONLY from
// the UI: the key value is never returned. `get_*_status` reports present/absent,
// `save_*` SETs it, `delete_*` CLEARs it. The orchestrator launch reads the key
// (backend-internal `vault::read_exa_key`) and sets `EXA_API_KEY` only when present.

#[tauri::command]
pub fn get_exa_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::exa_key_status()
}

#[tauri::command]
pub fn save_exa_key(
    state: State<'_, BackendState>,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::save_exa_key(&key)
}

#[tauri::command]
pub fn delete_exa_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::delete_exa_key()
}

// Censor CLOUD LLM API key — WRITE-ONLY from the UI: the key value is never returned.
// `get_*_status` reports present/absent, `save_*` SETs it, `delete_*` CLEARs it. The async
// Censor review reads it backend-internal (`vault::read_censor_cloud_key`) to authenticate
// the configured https endpoint — the one Censor path that egresses code off-device (opt-in).

#[tauri::command]
pub fn get_censor_cloud_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::censor_cloud_key_status()
}

#[tauri::command]
pub fn save_censor_cloud_key(
    state: State<'_, BackendState>,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::save_censor_cloud_key(&key)
}

#[tauri::command]
pub fn delete_censor_cloud_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::delete_censor_cloud_key()
}

// Cloud main-coder API key for the local Devboule orchestrator's OPT-IN Cloud mode.
// WRITE-ONLY from the UI: the key value is never returned. `get_*_status` reports
// present/absent, `save_*` SETs it, `delete_*` CLEARs it. The orchestrator launch reads
// the key (backend-internal `vault::read_cloud_llm_key`) and sets `DEVBOULE_CLOUD_API_KEY`
// only when present AND the configured backend is `cloud`.

#[tauri::command]
pub fn get_cloud_llm_key_status(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::cloud_llm_key_status()
}

#[tauri::command]
pub fn save_cloud_llm_key(
    state: State<'_, BackendState>,
    key: String,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::save_cloud_llm_key(&key)
}

#[tauri::command]
pub fn delete_cloud_llm_key(
    state: State<'_, BackendState>,
) -> Result<AuxCredentialStatus, String> {
    state.ensure_unlocked()?;
    vault::delete_cloud_llm_key()
}

#[tauri::command]
pub fn get_oracle_llm_settings(
    state: State<'_, BackendState>,
) -> Result<OracleLlmSettingsStatus, String> {
    state.ensure_unlocked()?;
    vault::oracle_llm_settings_status()
}

#[tauri::command]
pub fn save_oracle_llm_settings(
    state: State<'_, BackendState>,
    settings: OracleLlmSettings,
    api_key: Option<String>,
) -> Result<OracleLlmSettingsStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::save_oracle_llm_settings(&settings, api_key.as_deref())?;
    // The resident Oracle server captures the LLM credentials in its spawn env (see
    // oracle::python_oracle::spawn_oracle_server) and never re-reads the vault, so an
    // already-running server would keep its STALE key after a save. We must NOT tear
    // it down synchronously here: `stop_python_oracle_runtime` does `child.kill()` +
    // `child.wait()`, and a slow reap (pipe-reader threads / OS) would BLOCK this
    // Tauri command, freezing the UI (it looks like a crash right after entering the
    // key). Instead set a lightweight "needs restart" flag and return immediately;
    // the supervisor (oracle_service, ~10s tick) observes it and tears the server
    // down OFF the UI thread, then respawns with the fresh credentials. The next
    // /ask also respawns on demand. This command now returns within ~100ms
    // regardless of server state.
    crate::backend::oracle_service::request_llm_restart();
    Ok(status)
}

#[tauri::command]
pub fn delete_oracle_llm_api_key(
    state: State<'_, BackendState>,
) -> Result<OracleLlmSettingsStatus, String> {
    state.ensure_unlocked()?;
    vault::delete_oracle_llm_api_key()
}

#[tauri::command]
pub fn get_oracle_index_preferences(
    state: State<'_, BackendState>,
) -> Result<OracleIndexPreferences, String> {
    state.ensure_unlocked()?;
    vault::read_oracle_index_preferences()
}

#[tauri::command]
pub fn save_oracle_index_preferences(
    state: State<'_, BackendState>,
    preferences: OracleIndexPreferences,
) -> Result<OracleIndexPreferences, String> {
    state.ensure_unlocked()?;
    let saved = vault::save_oracle_index_preferences(&preferences)?;
    // Re-arm the watcher one-shot so the supervisor's next tick picks up the
    // new index_mode (e.g. switching from "watch" to "commit" takes effect
    // immediately on the next ~10s tick rather than waiting for a restart).
    crate::backend::oracle_service::reset_watcher_armed();
    Ok(saved)
}

#[tauri::command]
pub async fn save_provider_scope(
    state: State<'_, BackendState>,
    provider: ProviderId,
    pinned_id: String,
) -> Result<ProviderScopeStatus, String> {
    let session_id = state.sensitive_session_id()?;
    let cleaned = validate_provider_scope_value(provider, &pinned_id)?;
    let validation_inventory =
        if let Some(token) = vault::read_token(provider).map_err(|e| sanitize_error_message(&e))? {
            let inventory = match provider {
                ProviderId::Cloudflare => {
                    fetch_cloudflare(&state.http, Some(token), Some(cleaned.clone())).await
                }
                ProviderId::Scaleway => {
                    let access_key = vault::read_scaleway_object_access_key()
                        .map_err(|e| sanitize_error_message(&e))?;
                    let secret_key = vault::read_scaleway_object_secret_key()
                        .map_err(|e| sanitize_error_message(&e))?;
                    fetch_scaleway(
                        &state.http,
                        Some(token),
                        Some(cleaned.clone()),
                        access_key,
                        secret_key,
                    )
                    .await
                }
            };
            provider_token_validation_result(provider, &inventory)
                .map_err(|message| format!("Provider scope validation failed: {message}"))?;
            Some(inventory)
        } else {
            None
        };
    state.ensure_same_sensitive_session(session_id)?;
    let status = vault::save_scope(provider, &cleaned)?;
    state.clear_provider_inventory(provider)?;
    if let Some(inventory) = validation_inventory {
        cache_validated_provider_inventory(&state, inventory)?;
    }
    Ok(status)
}

#[tauri::command]
pub fn delete_provider_scope(
    state: State<'_, BackendState>,
    provider: ProviderId,
) -> Result<ProviderScopeStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::delete_scope(provider)?;
    state.clear_provider_inventory(provider)?;
    Ok(status)
}

#[tauri::command]
pub async fn audit_provider_connection(
    state: State<'_, BackendState>,
    provider: ProviderId,
    token: String,
    pinned_id: Option<String>,
) -> Result<ProviderConnectionAudit, String> {
    state.ensure_unlocked()?;
    let cleaned = token.trim().to_string();
    if cleaned.len() < 16 {
        return Ok(ProviderConnectionAudit {
            provider,
            status: "error".into(),
            token_health: "invalid".into(),
            selected_scope: None,
            resource_count: 0,
            message: Some("Token is too short to audit.".into()),
            risks: Vec::new(),
        });
    }
    let scope = provider_connection_scope(provider, pinned_id)?;
    let inventory = match provider {
        ProviderId::Cloudflare => fetch_cloudflare(&state.http, Some(cleaned), scope).await,
        ProviderId::Scaleway => {
            let access_key =
                vault::read_scaleway_object_access_key().map_err(|e| sanitize_error_message(&e))?;
            let secret_key =
                vault::read_scaleway_object_secret_key().map_err(|e| sanitize_error_message(&e))?;
            fetch_scaleway(&state.http, Some(cleaned), scope, access_key, secret_key).await
        }
    };
    Ok(provider_connection_audit(provider, &inventory))
}

#[tauri::command]
pub async fn audit_saved_provider_connection(
    state: State<'_, BackendState>,
    provider: ProviderId,
    pinned_id: Option<String>,
) -> Result<ProviderConnectionAudit, String> {
    state.ensure_unlocked()?;
    let scope = provider_connection_scope(provider, pinned_id)?;
    let Some(token) = vault::read_token(provider).map_err(|e| sanitize_error_message(&e))? else {
        if provider == ProviderId::Scaleway {
            let access_key =
                vault::read_scaleway_object_access_key().map_err(|e| sanitize_error_message(&e))?;
            let secret_key =
                vault::read_scaleway_object_secret_key().map_err(|e| sanitize_error_message(&e))?;
            let inventory = fetch_scaleway(&state.http, None, scope, access_key, secret_key).await;
            return Ok(provider_connection_audit(provider, &inventory));
        }
        return Ok(missing_saved_provider_connection_audit(provider));
    };
    let inventory = match provider {
        ProviderId::Cloudflare => fetch_cloudflare(&state.http, Some(token), scope).await,
        ProviderId::Scaleway => {
            let access_key =
                vault::read_scaleway_object_access_key().map_err(|e| sanitize_error_message(&e))?;
            let secret_key =
                vault::read_scaleway_object_secret_key().map_err(|e| sanitize_error_message(&e))?;
            fetch_scaleway(&state.http, Some(token), scope, access_key, secret_key).await
        }
    };
    Ok(provider_connection_audit(provider, &inventory))
}

#[tauri::command]
pub async fn save_provider_token(
    state: State<'_, BackendState>,
    provider: ProviderId,
    token: String,
    pinned_id: Option<String>,
) -> Result<SecretStatus, String> {
    let session_id = state.sensitive_session_id()?;
    let cleaned = token.trim().to_string();
    if cleaned.len() < 16 {
        return Ok(secret_error(provider, "Token is too short to save."));
    }
    let scope = match provider_connection_scope(provider, pinned_id.clone()) {
        Ok(scope) => scope,
        Err(message) => return Ok(secret_error(provider, &message)),
    };
    let inventory =
        match validate_provider_token_with_scope(&state, provider, &cleaned, scope).await {
            Ok(inventory) => inventory,
            Err(message) => return Ok(secret_error(provider, &message)),
        };
    state.ensure_same_sensitive_session(session_id)?;
    if let Some(pinned_id) = pinned_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        vault::save_scope(provider, pinned_id)?;
    }
    let status = vault::save_token(provider, &cleaned)?;
    cache_validated_provider_inventory(&state, inventory)?;
    Ok(status)
}

#[tauri::command]
pub fn delete_provider_token(
    state: State<'_, BackendState>,
    provider: ProviderId,
) -> Result<SecretStatus, String> {
    state.ensure_unlocked()?;
    let status = vault::delete_token(provider)?;
    state.clear_provider_inventory(provider)?;
    Ok(status)
}

#[tauri::command]
pub async fn rotate_cloudflare_worker_secret(
    state: State<'_, BackendState>,
    account_id: String,
    worker_name: String,
    secret_name: String,
    secret_value: String,
) -> Result<SecretRotationResult, String> {
    let session_id = state.sensitive_session_id()?;
    let request = validate_cloudflare_secret_rotation_request(
        &account_id,
        &worker_name,
        &secret_name,
        &secret_value,
    )?;
    let token = vault::read_token(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Cloudflare token is not configured.".to_string())?;
    if let Some(scope) =
        vault::read_scope(ProviderId::Cloudflare).map_err(|e| sanitize_error_message(&e))?
    {
        if scope != request.account_id {
            return Err("Worker account does not match the pinned Cloudflare account.".into());
        }
    }
    if !state.has_cloudflare_worker(&request.account_id, &request.worker_name)? {
        return Err(
            "Worker is not in the current Cloudflare inventory. Sync Cloudflare before rotating."
                .into(),
        );
    }
    // C4: explicit name-scope check before the PUT, not relying solely on the
    // cache filter having already excluded out-of-scope workers.
    if !cloudflare_worker_name_in_aspis_bio_scope(&request.worker_name) {
        return Err("Worker is not in the Aspis Bio scope. Refusing to rotate its secret.".into());
    }
    cloudflare_rotation_scope_guard(&state, &request.account_id)?;
    state.ensure_same_sensitive_session(session_id)?;
    put_cloudflare_worker_secret(
        &state,
        &token,
        &request.account_id,
        &request.worker_name,
        &request.secret_name,
        &request.secret_value,
    )
    .await?;
    state.ensure_same_sensitive_session(session_id)?;
    let result = secret_rotation_result(
        &request.account_id,
        &request.worker_name,
        &request.secret_name,
    );
    state.record_activity_events(&[secret_rotation_activity_event(&result)])?;
    Ok(result)
}

/// Reads a worker's bindings (env vars, secret NAMES, and other bindings) for a
/// worker that is in the Aspis-Bio scope and present in the cached inventory.
/// Mirrors the scope-guard order of `rotate_cloudflare_worker_secret`: resolve
/// the saved token, resolve the pinned account scope, confirm the worker is in
/// the cached inventory, then confirm the worker name is in the Aspis-Bio scope.
/// Read-only: secret VALUES are never returned by Cloudflare and never surfaced.
#[tauri::command]
pub async fn fetch_cloudflare_worker_settings(
    state: State<'_, BackendState>,
    worker_name: String,
) -> Result<CloudflareWorkerSettings, String> {
    let session_id = state.sensitive_session_id()?;
    let worker_name = worker_name.trim();
    if worker_name.is_empty() || worker_name.contains('/') || worker_name.contains('\\') {
        return Err("Worker name is invalid.".into());
    }
    let token = vault::read_token(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Cloudflare token is not configured.".to_string())?;
    let scope = vault::read_scope(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .or_else(|| {
            state
                .cloudflare_selected_scope()
                .ok()
                .flatten()
                .map(|scope| scope.id)
        })
        .ok_or_else(|| {
            "Cloudflare account scope is not loaded. Sync Cloudflare before reading Worker settings."
                .to_string()
        })?;
    let account_id = scope.trim();
    // Defense-in-depth: re-validate the resolved id (incl. the in-memory scope
    // fallback) as 32-hex before it reaches any CF URL.
    if !cloudflare_account_id_is_valid(account_id) {
        return Err("Cloudflare account id is invalid.".into());
    }
    if !state.has_cloudflare_worker(account_id, worker_name)? {
        return Err(
            "Worker is not in the current Cloudflare inventory. Sync Cloudflare before reading its settings."
                .into(),
        );
    }
    if !cloudflare_worker_name_in_aspis_bio_scope(worker_name) {
        return Err("Worker is not in the Aspis Bio scope. Refusing to read its settings.".into());
    }
    cloudflare_rotation_scope_guard(&state, account_id)?;
    let settings =
        fetch_cloudflare_worker_settings_request(&state.http, &token, account_id, worker_name)
            .await;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(settings)
}

/// Lazily-loaded account-level billing (plan + recent invoices). Called by the
/// frontend when the Billing tab is selected — NOT part of the sync snapshot.
///
/// Applies the SAME scope proof as the other CF read commands
/// (`cloudflare_rotation_scope_guard`) so billing is only read for the proven
/// Aspis-Bio account. Missing token/scope degrade to a `readable: false`
/// `CloudflareBilling` with a clear message instead of erroring. Per-worker €
/// cost is NOT available from any Cloudflare API; only the account plan is.
#[tauri::command]
pub async fn fetch_cloudflare_billing(
    state: State<'_, BackendState>,
) -> Result<CloudflareBilling, String> {
    let session_id = state.sensitive_session_id()?;
    let unreadable = |message: &str| CloudflareBilling {
        plans: Vec::new(),
        invoices: Vec::new(),
        readable: false,
        message: Some(message.to_string()),
    };

    let Some(token) =
        vault::read_token(ProviderId::Cloudflare).map_err(|e| sanitize_error_message(&e))?
    else {
        return Ok(unreadable("Cloudflare token is not configured."));
    };
    let scope = vault::read_scope(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .or_else(|| {
            state
                .cloudflare_selected_scope()
                .ok()
                .flatten()
                .map(|scope| scope.id)
        });
    let Some(scope) = scope else {
        return Ok(unreadable(
            "Cloudflare account scope is not loaded. Sync Cloudflare before reading billing.",
        ));
    };
    let account_id = scope.trim();
    // Defense-in-depth: re-validate the resolved id (incl. the in-memory scope
    // fallback) as 32-hex before it reaches any CF URL.
    if !cloudflare_account_id_is_valid(account_id) {
        return Ok(unreadable("Cloudflare account id is invalid."));
    }
    // Same scope proof as the other CF read commands: only the proven Aspis-Bio
    // account. A guard failure is a degraded read, not a hard error.
    if let Err(message) = cloudflare_rotation_scope_guard(&state, account_id) {
        return Ok(unreadable(&message));
    }

    let billing = fetch_cloudflare_billing_request(&state.http, &token, account_id).await;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(billing)
}

/// Lazily-loaded Scaleway ORGANIZATION billing (consumptions + invoices). Called
/// by the frontend when the Billing tab is selected — NOT part of the sync
/// snapshot.
///
/// Resolves the Scaleway token + the pinned/configured project id, then the
/// org id behind that project (env `ASPIS_SCALEWAY_ORG_ID`, else a project
/// lookup) since billing is org-scoped while the app pins a project. Computes
/// the current `YYYY-MM` billing period. Missing token/project/org degrade to a
/// `readable: false` `ScalewayBilling` with a clear message instead of erroring.
/// Unlike Cloudflare, real per-category € cost IS surfaced. Never logs the token
/// or amounts.
#[tauri::command]
pub async fn fetch_scaleway_billing(
    state: State<'_, BackendState>,
) -> Result<ScalewayBilling, String> {
    let session_id = state.sensitive_session_id()?;
    let unreadable = |message: &str| ScalewayBilling {
        consumptions: Vec::new(),
        total_untaxed: None,
        total_discount: None,
        invoices: Vec::new(),
        updated_at: None,
        readable: false,
        message: Some(message.to_string()),
    };

    let Some(token) =
        vault::read_token(ProviderId::Scaleway).map_err(|e| sanitize_error_message(&e))?
    else {
        return Ok(unreadable("Scaleway token is not configured."));
    };

    let Some(project_id) = configured_or_pinned_scaleway_project_id()? else {
        return Ok(unreadable(
            "Scaleway billing needs the organization id; sync the project first.",
        ));
    };
    let project_id = project_id.trim();
    // Defense-in-depth: only a UUID-shaped project id reaches the org lookup URL.
    if !scaleway_uuid_is_valid(project_id) {
        return Ok(unreadable(
            "Scaleway billing needs the organization id; sync the project first.",
        ));
    }

    let Some(org_id) = resolve_scaleway_org_id(&state.http, &token, project_id).await else {
        return Ok(unreadable(
            "Scaleway billing needs the organization id; sync the project first.",
        ));
    };

    // Current billing period as YYYY-MM (UTC). Computed here, passed into the
    // pure request fn so it never reaches for the clock itself.
    let billing_period = Utc::now().format("%Y-%m").to_string();

    let billing =
        fetch_scaleway_billing_request(&state.http, &token, &org_id, &billing_period).await;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(billing)
}

/// Resolves the saved Cloudflare token and the in-scope account id, applying the
/// SAME read-only guard chain as `fetch_cloudflare_worker_settings`: worker must
/// be in the cached inventory, in the Aspis-Bio name scope, and the rotation
/// scope guard must pass. Returns `(token, account_id)` on success. Shared by the
/// env dry-run and the env write so both gate identically before any network.
fn resolve_cloudflare_worker_write_target(
    state: &BackendState,
    worker_name: &str,
) -> Result<(String, String), String> {
    let token = vault::read_token(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Cloudflare token is not configured.".to_string())?;
    let scope = vault::read_scope(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .or_else(|| {
            state
                .cloudflare_selected_scope()
                .ok()
                .flatten()
                .map(|scope| scope.id)
        })
        .ok_or_else(|| {
            "Cloudflare account scope is not loaded. Sync Cloudflare before changing Worker env vars."
                .to_string()
        })?;
    let account_id = scope.trim().to_string();
    // Defense-in-depth: re-validate the resolved id (incl. the in-memory scope
    // fallback) as 32-hex before it reaches any CF URL.
    if !cloudflare_account_id_is_valid(&account_id) {
        return Err("Cloudflare account id is invalid.".into());
    }
    if !state.has_cloudflare_worker(&account_id, worker_name)? {
        return Err(
            "Worker is not in the current Cloudflare inventory. Sync Cloudflare before changing its env vars."
                .into(),
        );
    }
    if !cloudflare_worker_name_in_aspis_bio_scope(worker_name) {
        return Err(
            "Worker is not in the Aspis Bio scope. Refusing to change its env vars.".into(),
        );
    }
    cloudflare_rotation_scope_guard(state, &account_id)?;
    Ok((token, account_id))
}

/// Maximum byte length of a `plain_text` env-var value we will write. Generous
/// for config values while bounding request size and rejecting accidental blobs.
const CF_ENV_VALUE_MAX_BYTES: usize = 5120;

/// Validates and normalizes a worker name + env var name + value for an env-var
/// change. Mirrors the worker-name checks used elsewhere and reuses
/// `is_valid_js_identifier` (the same validator secret rotation applies to
/// binding names). The value is validated but NOT trimmed: a value may
/// legitimately contain leading/trailing significant whitespace, so we only
/// reject a value that is empty or whitespace-ONLY, too large, or carries
/// control characters (incl. NUL) that would corrupt the binding.
fn validate_cloudflare_env_request(
    worker_name: &str,
    var_name: &str,
    new_value: &str,
) -> Result<(String, String, String), String> {
    let worker_name = worker_name.trim();
    if worker_name.is_empty() || worker_name.contains('/') || worker_name.contains('\\') {
        return Err("Worker name is invalid.".into());
    }
    let var_name = var_name.trim();
    if !is_valid_js_identifier(var_name) {
        return Err("Environment variable name must be a valid JavaScript identifier.".into());
    }
    if new_value.trim().is_empty() {
        return Err("Environment variable value cannot be empty.".into());
    }
    if new_value.len() > CF_ENV_VALUE_MAX_BYTES {
        return Err("Environment variable value exceeds the maximum length.".into());
    }
    if new_value.chars().any(|c| c.is_control()) {
        return Err("Environment variable value contains invalid control characters.".into());
    }
    Ok((
        worker_name.to_string(),
        var_name.to_string(),
        new_value.to_string(),
    ))
}

/// Read-only preview of a `plain_text` env-var write. Re-fetches live worker
/// settings (secret VALUES are never returned by Cloudflare) and computes the
/// before/after plus the bindings that the write would preserve. No mutation.
#[tauri::command]
pub async fn cloudflare_env_dry_run(
    state: State<'_, BackendState>,
    worker_name: String,
    var_name: String,
    new_value: String,
) -> Result<CloudflareEnvDryRunResult, String> {
    let session_id = state.sensitive_session_id()?;
    let (worker_name, var_name, new_value) =
        validate_cloudflare_env_request(&worker_name, &var_name, &new_value)?;
    let (token, account_id) = resolve_cloudflare_worker_write_target(&state, &worker_name)?;
    let settings =
        fetch_cloudflare_worker_settings_request(&state.http, &token, &account_id, &worker_name)
            .await;
    state.ensure_same_sensitive_session(session_id)?;
    if !settings.readable {
        return Err(settings.message.unwrap_or_else(|| {
            "Worker settings are not readable; cannot preview the change.".into()
        }));
    }
    Ok(cloudflare_env_dry_run_compute(
        &settings, &var_name, &new_value,
    ))
}

/// Creates or updates a single `plain_text` env var on an Aspis-Bio Worker via a
/// lossless raw-bindings PATCH (see `patch_cloudflare_worker_plain_text`). Uses
/// the FULL sensitive-session + scope guard chain of secret rotation: the
/// session id is captured before, re-asserted after, the var name is validated,
/// the account scope must match, and the rotation scope guard must pass. The
/// successful write is recorded as an audited activity event.
#[tauri::command]
pub async fn cloudflare_set_worker_env(
    state: State<'_, BackendState>,
    worker_name: String,
    var_name: String,
    new_value: String,
) -> Result<CloudflareEnvWriteResult, String> {
    let session_id = state.sensitive_session_id()?;
    let (worker_name, var_name, new_value) =
        validate_cloudflare_env_request(&worker_name, &var_name, &new_value)?;
    let (token, account_id) = resolve_cloudflare_worker_write_target(&state, &worker_name)?;
    state.ensure_same_sensitive_session(session_id)?;
    patch_cloudflare_worker_plain_text(
        &state.http,
        &token,
        &account_id,
        &worker_name,
        &var_name,
        &new_value,
    )
    .await?;
    state.ensure_same_sensitive_session(session_id)?;
    let result = cloudflare_env_write_result(&worker_name, &var_name);
    state.record_activity_events(&[cloudflare_env_write_activity_event(&result)])?;
    Ok(result)
}

fn cloudflare_env_write_result(worker_name: &str, var_name: &str) -> CloudflareEnvWriteResult {
    CloudflareEnvWriteResult {
        worker_name: worker_name.trim().into(),
        var_name: var_name.trim().into(),
        applied: true,
        message: format!(
            "Updated plain-text env var {} on {}. Secrets were preserved via inherit.",
            var_name.trim(),
            worker_name.trim()
        ),
        written_at: now(),
    }
}

fn cloudflare_env_write_activity_event(result: &CloudflareEnvWriteResult) -> ActivityEvent {
    ActivityEvent {
        // Include the write timestamp so repeated writes to the SAME var are each
        // recorded; a static id would be deduplicated and silently drop repeats.
        id: format!(
            "cf_env_written_{}_{}_{}",
            result.worker_name, result.var_name, result.written_at
        ),
        message: format!(
            "Set Cloudflare Worker env var {} on {}.",
            result.var_name, result.worker_name
        ),
        timestamp: result.written_at.clone(),
        event_type: "config".into(),
        source: "Cloudflare".into(),
    }
}

// ===========================================================================
// Phase 4 — per-type safe-edit actions (AI Gateway, AutoRAG, KV, D1, R2).
//
// Every command resolves the saved token + the proven Aspis-Bio account scope
// and runs `cloudflare_rotation_scope_guard` BEFORE any network call (so a
// caller can never target a resource outside the synced Aspis-Bio account).
// Reads do not re-check the sensitive session; WRITES capture the session id up
// front, re-assert it after each network round-trip, and record an audited
// "config" activity event. Resource presence is validated with a live list
// lookup against the proven account (the platform inventory is not persisted in
// state, so an in-account list is the feasible presence proof here).
// ===========================================================================

/// Max byte length of a KV value we will accept on a write. Mirrors the read cap.
const CF_KV_WRITE_VALUE_MAX_BYTES: usize = 65_536;
/// Max byte length of a D1 SQL statement we will accept.
const CF_D1_SQL_MAX_BYTES: usize = 100_000;

/// Resolves `(token, account_id)` for an Aspis-Bio Cloudflare resource action,
/// applying the SAME scope proof as the worker commands: saved token, resolved
/// account scope (pinned vault scope else in-memory selected scope), 32-hex
/// re-validation, and the rotation scope guard. Resource-type-specific presence
/// is checked separately by each command (it needs a network call).
fn resolve_cloudflare_account_action_target(
    state: &BackendState,
) -> Result<(String, String), String> {
    let token = vault::read_token(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Cloudflare token is not configured.".to_string())?;
    let scope = vault::read_scope(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .or_else(|| {
            state
                .cloudflare_selected_scope()
                .ok()
                .flatten()
                .map(|scope| scope.id)
        })
        .ok_or_else(|| {
            "Cloudflare account scope is not loaded. Sync Cloudflare before acting on its resources."
                .to_string()
        })?;
    let account_id = scope.trim().to_string();
    if !cloudflare_account_id_is_valid(&account_id) {
        return Err("Cloudflare account id is invalid.".into());
    }
    cloudflare_rotation_scope_guard(state, &account_id)?;
    Ok((token, account_id))
}

/// Validates an opaque Cloudflare resource id (gateway/db/bucket/namespace id,
/// AutoRAG instance name). Rejects empty, path separators, control chars, and
/// over-long values that could escape the URL path or smuggle a second segment.
fn validate_cloudflare_resource_id(id: &str, label: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(format!("{label} is required."));
    }
    if id.len() > 256 {
        return Err(format!("{label} is too long."));
    }
    if id.contains('/') || id.contains('\\') || id.chars().any(|c| c.is_control()) {
        return Err(format!("{label} is invalid."));
    }
    Ok(id.to_string())
}

/// Reads editable AI Gateway settings (rate limiting / caching / logging) for a
/// gateway proven to exist in the Aspis-Bio account. Read-only: degrades to
/// `readable: false` on failure; no sensitive-session recheck required.
#[tauri::command]
pub async fn fetch_cloudflare_ai_gateway_settings(
    state: State<'_, BackendState>,
    gateway_id: String,
) -> Result<CloudflareAiGatewaySettings, String> {
    let session_id = state.sensitive_session_id()?;
    let gateway_id = validate_cloudflare_resource_id(&gateway_id, "AI Gateway id")?;
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    // Like every other per-type command, prove the resource is in the Aspis-Bio
    // account before touching it; do not rely on CF's 404 alone. A network/other
    // error fails closed (Err); a confirmed absence degrades to unreadable so the
    // read-only contract still holds.
    if !cloudflare_ai_gateway_exists(&state.http, &token, &account_id, &gateway_id).await? {
        state.ensure_same_sensitive_session(session_id)?;
        return Ok(CloudflareAiGatewaySettings {
            account_id,
            gateway_id,
            cache_ttl: None,
            cache_invalidate_on_update: None,
            collect_logs: None,
            logpush: None,
            rate_limiting_interval: None,
            rate_limiting_limit: None,
            rate_limiting_technique: None,
            readable: false,
            message: Some("AI Gateway is not in the Aspis Bio account.".into()),
        });
    }
    let settings =
        fetch_cloudflare_ai_gateway_settings_request(&state.http, &token, &account_id, &gateway_id)
            .await;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(settings)
}

/// Updates AI Gateway settings via a LOSSLESS re-fetch + full-object PUT. Uses
/// the full sensitive-session + scope guard chain; the gateway must exist in the
/// proven account; the successful write is audited.
#[tauri::command]
pub async fn set_cloudflare_ai_gateway_settings(
    state: State<'_, BackendState>,
    gateway_id: String,
    settings: CloudflareAiGatewaySettingsPatch,
) -> Result<CloudflareAiGatewaySettings, String> {
    let session_id = state.sensitive_session_id()?;
    let gateway_id = validate_cloudflare_resource_id(&gateway_id, "AI Gateway id")?;
    if let Some(technique) = settings.rate_limiting_technique.as_deref() {
        if !matches!(technique, "fixed" | "sliding") {
            return Err("Rate limiting technique must be 'fixed' or 'sliding'.".into());
        }
    }
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    // Prove the gateway is in the Aspis-Bio account before writing; fail closed on
    // any network/other error (Err) and refuse on a confirmed absence.
    if !cloudflare_ai_gateway_exists(&state.http, &token, &account_id, &gateway_id).await? {
        return Err(
            "AI Gateway is not in the Aspis Bio account. Refusing to update settings.".into(),
        );
    }
    // Re-assert the sensitive session immediately BEFORE the write (matching
    // set_cloudflare_kv_value / cloudflare_set_worker_env) so a session revoked
    // during the existence check cannot still reach the PUT.
    state.ensure_same_sensitive_session(session_id)?;
    put_cloudflare_ai_gateway_settings(&state.http, &token, &account_id, &gateway_id, &settings)
        .await?;
    state.ensure_same_sensitive_session(session_id)?;
    // Re-read so the UI reflects the committed state, not the requested patch.
    let updated =
        fetch_cloudflare_ai_gateway_settings_request(&state.http, &token, &account_id, &gateway_id)
            .await;
    state.ensure_same_sensitive_session(session_id)?;
    let written_at = now();
    state.record_activity_events(&[ActivityEvent {
        id: format!("cf_ai_gateway_updated_{gateway_id}_{written_at}"),
        message: format!("Updated Cloudflare AI Gateway settings for {gateway_id}."),
        timestamp: written_at,
        event_type: "config".into(),
        source: "Cloudflare".into(),
    }])?;
    Ok(updated)
}

/// Triggers an AI Search (AutoRAG) sync/reindex job for an instance proven to
/// exist in the Aspis-Bio account. A trigger, not a destructive replace; still
/// uses the write session + scope chain and is audited.
#[tauri::command]
pub async fn cloudflare_autorag_reindex(
    state: State<'_, BackendState>,
    instance_id: String,
) -> Result<CloudflareAutoragReindexResult, String> {
    let session_id = state.sensitive_session_id()?;
    let instance_id = validate_cloudflare_resource_id(&instance_id, "AI Search instance id")?;
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_autorag_instance_exists(&state.http, &token, &account_id, &instance_id).await? {
        return Err(
            "AI Search instance is not in the Aspis Bio account. Refusing to trigger a sync."
                .into(),
        );
    }
    state.ensure_same_sensitive_session(session_id)?;
    let result =
        trigger_cloudflare_autorag_reindex(&state.http, &token, &account_id, &instance_id).await?;
    state.ensure_same_sensitive_session(session_id)?;
    state.record_activity_events(&[ActivityEvent {
        id: format!("cf_autorag_reindex_{instance_id}_{}", result.triggered_at),
        message: format!("Triggered Cloudflare AI Search sync for {instance_id}."),
        timestamp: result.triggered_at.clone(),
        event_type: "config".into(),
        source: "Cloudflare".into(),
    }])?;
    Ok(result)
}

/// Lists a capped page of KV keys for a namespace proven to exist in the
/// Aspis-Bio account. Read-only.
#[tauri::command]
pub async fn fetch_cloudflare_kv_keys(
    state: State<'_, BackendState>,
    namespace_id: String,
    prefix: Option<String>,
    cursor: Option<String>,
) -> Result<CloudflareKvKeysPage, String> {
    let session_id = state.sensitive_session_id()?;
    let namespace_id = validate_cloudflare_resource_id(&namespace_id, "KV namespace id")?;
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_kv_namespace_exists(&state.http, &token, &account_id, &namespace_id).await? {
        return Err("KV namespace is not in the Aspis Bio account. Refusing to list keys.".into());
    }
    let page = fetch_cloudflare_kv_keys_request(
        &state.http,
        &token,
        &account_id,
        &namespace_id,
        prefix.as_deref(),
        cursor.as_deref(),
    )
    .await?;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(page)
}

/// Reads one KV value (capped, truncated if oversized) from a namespace proven
/// to exist in the Aspis-Bio account. Read-only.
#[tauri::command]
pub async fn fetch_cloudflare_kv_value(
    state: State<'_, BackendState>,
    namespace_id: String,
    key: String,
) -> Result<CloudflareKvValue, String> {
    let session_id = state.sensitive_session_id()?;
    let namespace_id = validate_cloudflare_resource_id(&namespace_id, "KV namespace id")?;
    let key = key.trim();
    if key.is_empty() {
        return Err("KV key is required.".into());
    }
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_kv_namespace_exists(&state.http, &token, &account_id, &namespace_id).await? {
        return Err(
            "KV namespace is not in the Aspis Bio account. Refusing to read a value.".into(),
        );
    }
    let value =
        fetch_cloudflare_kv_value_request(&state.http, &token, &account_id, &namespace_id, key)
            .await?;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(value)
}

/// Writes (PUT) a single KV value into a namespace proven to exist in the
/// Aspis-Bio account. Full write session + scope chain; audited.
#[tauri::command]
pub async fn set_cloudflare_kv_value(
    state: State<'_, BackendState>,
    namespace_id: String,
    key: String,
    value: String,
) -> Result<CloudflareKvWriteResult, String> {
    let session_id = state.sensitive_session_id()?;
    let namespace_id = validate_cloudflare_resource_id(&namespace_id, "KV namespace id")?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("KV key is required.".into());
    }
    if value.len() > CF_KV_WRITE_VALUE_MAX_BYTES {
        return Err("KV value exceeds the maximum length.".into());
    }
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_kv_namespace_exists(&state.http, &token, &account_id, &namespace_id).await? {
        return Err(
            "KV namespace is not in the Aspis Bio account. Refusing to write a value.".into(),
        );
    }
    state.ensure_same_sensitive_session(session_id)?;
    put_cloudflare_kv_value(
        &state.http,
        &token,
        &account_id,
        &namespace_id,
        &key,
        &value,
    )
    .await?;
    state.ensure_same_sensitive_session(session_id)?;
    let written_at = now();
    let result = CloudflareKvWriteResult {
        namespace_id: namespace_id.clone(),
        key: key.clone(),
        action: "set".into(),
        applied: true,
        message: format!("Set KV key {key} in namespace {namespace_id}."),
        written_at: written_at.clone(),
    };
    state.record_activity_events(&[ActivityEvent {
        id: format!("cf_kv_set_{namespace_id}_{key}_{written_at}"),
        message: format!("Set Cloudflare KV key {key} in namespace {namespace_id}."),
        timestamp: written_at,
        event_type: "config".into(),
        source: "Cloudflare".into(),
    }])?;
    Ok(result)
}

/// Deletes a single KV value. Destructive: requires `confirm_key` to equal `key`.
/// Full write session + scope chain; audited.
#[tauri::command]
pub async fn delete_cloudflare_kv_value(
    state: State<'_, BackendState>,
    namespace_id: String,
    key: String,
    confirm_key: String,
) -> Result<CloudflareKvWriteResult, String> {
    let session_id = state.sensitive_session_id()?;
    let namespace_id = validate_cloudflare_resource_id(&namespace_id, "KV namespace id")?;
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("KV key is required.".into());
    }
    if confirm_key.trim() != key {
        return Err(
            "Delete not confirmed: the confirmation key must equal the key to delete.".into(),
        );
    }
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_kv_namespace_exists(&state.http, &token, &account_id, &namespace_id).await? {
        return Err(
            "KV namespace is not in the Aspis Bio account. Refusing to delete a value.".into(),
        );
    }
    state.ensure_same_sensitive_session(session_id)?;
    delete_cloudflare_kv_value_request(&state.http, &token, &account_id, &namespace_id, &key)
        .await?;
    state.ensure_same_sensitive_session(session_id)?;
    let written_at = now();
    let result = CloudflareKvWriteResult {
        namespace_id: namespace_id.clone(),
        key: key.clone(),
        action: "delete".into(),
        applied: true,
        message: format!("Deleted KV key {key} from namespace {namespace_id}."),
        written_at: written_at.clone(),
    };
    state.record_activity_events(&[ActivityEvent {
        id: format!("cf_kv_delete_{namespace_id}_{key}_{written_at}"),
        message: format!("Deleted Cloudflare KV key {key} from namespace {namespace_id}."),
        timestamp: written_at,
        event_type: "config".into(),
        source: "Cloudflare".into(),
    }])?;
    Ok(result)
}

/// Runs a D1 query against a database proven to exist in the Aspis-Bio account.
/// Reads run freely; WRITES (per the PURE `d1_sql_is_write`) require `confirm:
/// true` or this returns a typed `requiresConfirmation` result WITHOUT executing.
/// Confirmed writes use the full write session chain and are audited.
#[tauri::command]
pub async fn cloudflare_d1_query(
    state: State<'_, BackendState>,
    database_id: String,
    sql: String,
    confirm: bool,
) -> Result<CloudflareD1QueryResult, String> {
    let session_id = state.sensitive_session_id()?;
    let database_id = validate_cloudflare_resource_id(&database_id, "D1 database id")?;
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("D1 SQL is required.".into());
    }
    if sql.len() > CF_D1_SQL_MAX_BYTES {
        return Err("D1 SQL exceeds the maximum length.".into());
    }
    let is_write = d1_sql_is_write(sql);
    // Confirmation gate is checked BEFORE any network call so an unconfirmed
    // write never reaches Cloudflare.
    if is_write && !confirm {
        return Ok(CloudflareD1QueryResult {
            database_id,
            is_write: true,
            requires_confirmation: true,
            executed: false,
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            truncated: false,
            rows_read: None,
            rows_written: None,
            message: "This statement modifies data. Re-run with confirmation to execute.".into(),
        });
    }
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_d1_database_exists(&state.http, &token, &account_id, &database_id).await? {
        return Err(
            "D1 database is not in the Aspis Bio account. Refusing to run the query.".into(),
        );
    }
    if is_write {
        state.ensure_same_sensitive_session(session_id)?;
    }
    let result = run_cloudflare_d1_query(
        &state.http,
        &token,
        &account_id,
        &database_id,
        sql,
        is_write,
    )
    .await?;
    state.ensure_same_sensitive_session(session_id)?;
    if is_write {
        let written_at = now();
        state.record_activity_events(&[ActivityEvent {
            id: format!("cf_d1_write_{database_id}_{written_at}"),
            message: format!("Ran a Cloudflare D1 write query against {database_id}."),
            timestamp: written_at,
            event_type: "config".into(),
            source: "Cloudflare".into(),
        }])?;
    }
    Ok(result)
}

/// Reads R2 bucket lifecycle + CORS config for a bucket proven to exist in the
/// Aspis-Bio account. Read-only; each part degrades independently.
#[tauri::command]
pub async fn fetch_cloudflare_r2_config(
    state: State<'_, BackendState>,
    bucket: String,
) -> Result<CloudflareR2Config, String> {
    let session_id = state.sensitive_session_id()?;
    let bucket = validate_cloudflare_resource_id(&bucket, "R2 bucket name")?;
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_r2_bucket_exists(&state.http, &token, &account_id, &bucket).await? {
        return Err(
            "R2 bucket is not in the Aspis Bio account. Refusing to read its config.".into(),
        );
    }
    let config =
        fetch_cloudflare_r2_config_request(&state.http, &token, &account_id, &bucket).await;
    state.ensure_same_sensitive_session(session_id)?;
    Ok(config)
}

/// Writes (PUT) an R2 bucket lifecycle configuration. Token-based (no S3 creds).
/// Full write session + scope chain; the bucket must exist in the proven account.
#[tauri::command]
pub async fn set_cloudflare_r2_lifecycle(
    state: State<'_, BackendState>,
    bucket: String,
    rules: serde_json::Value,
) -> Result<CloudflareR2WriteResult, String> {
    set_cloudflare_r2_target(state, bucket, "lifecycle", rules).await
}

/// Writes (PUT) an R2 bucket CORS configuration. Token-based (no S3 creds).
/// Full write session + scope chain; the bucket must exist in the proven account.
#[tauri::command]
pub async fn set_cloudflare_r2_cors(
    state: State<'_, BackendState>,
    bucket: String,
    rules: serde_json::Value,
) -> Result<CloudflareR2WriteResult, String> {
    set_cloudflare_r2_target(state, bucket, "cors", rules).await
}

/// Shared R2 lifecycle/CORS write path. `target` is a fixed literal ("lifecycle"
/// or "cors") chosen by the caller, never user input, so it cannot widen the URL.
async fn set_cloudflare_r2_target(
    state: State<'_, BackendState>,
    bucket: String,
    target: &str,
    rules: serde_json::Value,
) -> Result<CloudflareR2WriteResult, String> {
    let session_id = state.sensitive_session_id()?;
    let bucket = validate_cloudflare_resource_id(&bucket, "R2 bucket name")?;
    // The `rules` payload must be an array (CF expects `{ "rules": [...] }`).
    if !rules.is_array() {
        return Err(format!("R2 {target} rules must be an array."));
    }
    let (token, account_id) = resolve_cloudflare_account_action_target(&state)?;
    if !cloudflare_r2_bucket_exists(&state.http, &token, &account_id, &bucket).await? {
        return Err(format!(
            "R2 bucket is not in the Aspis Bio account. Refusing to write its {target}."
        ));
    }
    state.ensure_same_sensitive_session(session_id)?;
    put_cloudflare_r2_config(&state.http, &token, &account_id, &bucket, target, &rules).await?;
    state.ensure_same_sensitive_session(session_id)?;
    let written_at = now();
    let result = CloudflareR2WriteResult {
        bucket: bucket.clone(),
        target: target.to_string(),
        applied: true,
        message: format!("Updated R2 {target} configuration for {bucket}."),
        written_at: written_at.clone(),
    };
    state.record_activity_events(&[ActivityEvent {
        id: format!("cf_r2_{target}_{bucket}_{written_at}"),
        message: format!("Updated Cloudflare R2 {target} configuration for {bucket}."),
        timestamp: written_at,
        event_type: "config".into(),
        source: "Cloudflare".into(),
    }])?;
    Ok(result)
}

#[tauri::command]
pub async fn cloudflare_smoke_dry_run(
    state: State<'_, BackendState>,
) -> Result<CloudflareSmokeDryRunResult, String> {
    let session_id = state.sensitive_session_id()?;
    let token = vault::read_token(ProviderId::Cloudflare)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Cloudflare token is not configured.".to_string())?;
    let scope =
        vault::read_scope(ProviderId::Cloudflare).map_err(|e| sanitize_error_message(&e))?;

    let inventory = fetch_cloudflare(&state.http, Some(token), scope).await;
    state.ensure_same_sensitive_session(session_id)?;

    let result = cloudflare_smoke_dry_run_result(&inventory);
    state.replace_provider_inventory(inventory)?;
    state.record_activity_events(&[cloudflare_smoke_dry_run_activity_event(&result)])?;
    Ok(result)
}

#[tauri::command]
pub async fn perform_scaleway_resource_action(
    state: State<'_, BackendState>,
    resource_id: String,
    action: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim();
    if resource_id.is_empty() || resource_id.contains('/') || resource_id.contains('\\') {
        return Err("Scaleway resource id is invalid.".into());
    }
    let resource = state.scaleway_resource(resource_id)?.ok_or_else(|| {
        "Resource is not in the current Scaleway inventory. Sync Scaleway before acting."
            .to_string()
    })?;
    if resource.project_id.is_none() {
        return Err("Scaleway resource project is unknown. Sync Scaleway before acting.".into());
    }
    let action =
        validate_scaleway_action_request(&resource, &action, confirm_resource_name.as_deref())?;
    scaleway_action_inventory_guard(&state, resource_id, &action)?;
    // C1: re-assert project scope at mutation time. Today scoping is only an
    // emergent property of the cache filter; make it an explicit hard guard
    // before any start/stop/reboot/DELETE HTTP call.
    let pinned_project_id = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_resource_in_pinned_project(&resource, pinned_project_id.as_deref())?;
    // DEFENSE-IN-DEPTH: `resource.region` is a CACHED value from the Scaleway API
    // response, not user input validated on a create path. It flows into the
    // instance/serverless URL path segments, so reject any region that is not a
    // strict location token BEFORE the mutating request is built, and pass the
    // VALIDATED value downstream (mirrors `resize_scaleway_block_volume`) rather
    // than discarding it and re-reading the raw cached field.
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = vault::read_token(ProviderId::Scaleway)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Scaleway token is not configured.".to_string())?;

    state.ensure_same_sensitive_session(session_id)?;
    let action =
        perform_scaleway_resource_action_request(&state.http, &token, &resource, &region, &action)
            .await
            .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;

    let result = scaleway_action_result(&resource, &action);
    state.record_activity_events(&[scaleway_action_activity_event(&result)])?;
    Ok(result)
}

// ===========================================================================
// Scaleway STORAGE CRUD (Block / File / Object). Every mutation reuses the same
// guard chain as `perform_scaleway_resource_action`:
//   1. sensitive-session bracket (capture id, re-assert after each network hop)
//   2. resource-in-inventory (storage via `scaleway_storage_resource`)
//   3. confirm-by-name on delete (`validate_scaleway_storage_action_request`)
//   4. project-scope HARD-FAIL on EVERY mutation, incl. CREATE (the create's
//      target project_id MUST equal the pinned project)
//   5. vault credential (project token, or S3 keypair for Object Storage)
//   6. activity event, `.timeout` (in the provider fns), typed errors, no secret
//      logging, `urlencoding::encode` on every path segment in the provider fns.
// ===========================================================================

/// Max bytes we accept for a storage resource name. Scaleway names are short;
/// this also caps a hostile payload before it reaches the API.
const SCW_STORAGE_NAME_MAX_BYTES: usize = 64;

/// 1 GiB in bytes — UI sizes are entered in GiB and converted here so the API
/// receives bytes (Block `from_empty.size`, File `size`).
const BYTES_PER_GIB: u64 = 1_073_741_824;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayBlockVolumeCreateRequest {
    pub name: String,
    pub zone: String,
    pub project_id: String,
    pub size_gib: u64,
    pub perf_iops: u32,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayFilesystemCreateRequest {
    pub name: String,
    pub region: String,
    pub project_id: String,
    pub size_gib: u64,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayObjectBucketCreateRequest {
    pub name: String,
    pub region: String,
    pub project_id: String,
}

/// Max CPU units the UI may request for a Serverless SQL database. Scaleway's
/// autoscale ceiling is modest today; this also caps a hostile payload.
const SCW_SQL_MAX_CPU: u32 = 16;

/// Hard ceiling on the requested function/container memory (MB) and scale, so a
/// hostile payload cannot request an absurd allocation. Generous vs real limits.
const SCW_SERVERLESS_MAX_MEMORY_MB: u32 = 8_192;
const SCW_SERVERLESS_MAX_SCALE: u32 = 100;

/// Allowed Serverless Function runtimes (strict allowlist). The runtime string is
/// sent verbatim in the create body; restricting it to a known, slug-shaped set
/// keeps a hostile value from reaching the API and documents what the UI offers.
const SCW_FUNCTION_RUNTIMES: &[&str] = &[
    "node18",
    "node20",
    "node22",
    "python310",
    "python311",
    "python312",
    "go122",
    "go123",
    "php82",
    "php83",
    "rust185",
];

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewaySqlDatabaseCreateRequest {
    pub name: String,
    pub region: String,
    pub project_id: String,
    pub cpu_min: u32,
    pub cpu_max: u32,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayFunctionCreateCommandRequest {
    pub name: String,
    pub region: String,
    pub project_id: String,
    /// Namespace to create the function in. When absent, a namespace named
    /// `namespace_name` (or the function name) is created first.
    #[serde(default)]
    pub namespace_id: Option<String>,
    #[serde(default)]
    pub namespace_name: Option<String>,
    pub runtime: String,
    #[serde(default)]
    pub memory_limit: Option<u32>,
    #[serde(default)]
    pub min_scale: Option<u32>,
    #[serde(default)]
    pub max_scale: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScalewayContainerCreateCommandRequest {
    pub name: String,
    pub region: String,
    pub project_id: String,
    #[serde(default)]
    pub namespace_id: Option<String>,
    #[serde(default)]
    pub namespace_name: Option<String>,
    pub registry_image: String,
    #[serde(default)]
    pub memory_limit: Option<u32>,
    #[serde(default)]
    pub min_scale: Option<u32>,
    #[serde(default)]
    pub max_scale: Option<u32>,
}

/// Validate a storage resource name: non-empty, length-capped, and free of path
/// separators or whitespace so it cannot widen a URL or break the request body.
fn validate_scaleway_storage_name(name: &str, label: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("{label} is required."));
    }
    if name.len() > SCW_STORAGE_NAME_MAX_BYTES {
        return Err(format!("{label} is too long."));
    }
    if name.contains('/') || name.contains('\\') || name.chars().any(char::is_whitespace) {
        return Err(format!("{label} contains invalid characters."));
    }
    Ok(name.to_string())
}

/// Validate a Scaleway zone/region against its real grammar via a strict ALLOWLIST:
/// non-empty, <= 32 bytes, and every character ascii-lowercase / digit / '-'. This
/// is load-bearing: the value is interpolated raw into the S3 host
/// `format!("s3.{region}.scw.cloud")` and into api.scaleway.com paths, so any '@',
/// '?', '#', '/', '\\', uppercase, '_' or '.' could rewrite the URL authority and
/// divert a SigV4-signed request to an attacker host. A denylist is not safe here.
fn validate_scaleway_location(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} is required."));
    }
    if value.len() > 32
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("{label} is invalid."));
    }
    Ok(value.to_string())
}

/// Validate an Object Storage bucket name against the S3 naming rules used on the
/// CREATE path: 3-63 characters, only lowercase letters, digits and hyphens (no
/// uppercase, '_', '.', '@', '/', or whitespace). A violating name guarantees a
/// confusing S3 400, so reject it before the signed request is built. (Block/File
/// names use the more permissive `validate_scaleway_storage_name`; this stricter
/// check is dedicated to bucket creation and is NOT applied to those types.)
fn validate_scaleway_object_bucket_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.len() < 3 || name.len() > 63 {
        return Err("Bucket name must be 3 to 63 characters long.".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("Bucket name may contain only lowercase letters, digits and hyphens.".into());
    }
    Ok(name.to_string())
}

/// GiB -> bytes with overflow protection.
fn scaleway_size_gib_to_bytes(size_gib: u64) -> Result<u64, String> {
    if size_gib == 0 {
        return Err("Storage size must be at least 1 GiB.".into());
    }
    size_gib
        .checked_mul(BYTES_PER_GIB)
        .ok_or_else(|| "Storage size is too large.".to_string())
}

/// The vault Scaleway project token, or a typed error.
fn scaleway_project_token() -> Result<String, String> {
    vault::read_token(ProviderId::Scaleway)
        .map_err(|e| sanitize_error_message(&e))?
        .ok_or_else(|| "Scaleway token is not configured.".to_string())
}

/// The S3 keypair for Object Storage mutations, or a clear degrade message.
fn scaleway_object_storage_keypair() -> Result<(String, String), String> {
    let access_key = vault::read_scaleway_object_access_key()
        .map_err(|e| sanitize_error_message(&e))?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "Scaleway Object Storage access key is not configured. Save the S3 keypair before acting."
                .to_string()
        })?;
    let secret_key = vault::read_scaleway_object_secret_key()
        .map_err(|e| sanitize_error_message(&e))?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "Scaleway Object Storage secret key is not configured. Save the S3 keypair before acting."
                .to_string()
        })?;
    Ok((access_key, secret_key))
}

/// Build a storage-shaped `ScalewayActionResult` + record an activity event.
fn scaleway_storage_result(
    state: &BackendState,
    resource_id: &str,
    resource_name: &str,
    storage_type: &str,
    action: &str,
    message: String,
) -> Result<ScalewayActionResult, String> {
    let triggered_at = now();
    let result = ScalewayActionResult {
        provider: ProviderId::Scaleway,
        resource_id: resource_id.to_string(),
        resource_name: resource_name.to_string(),
        resource_type: storage_type.to_string(),
        action: action.to_string(),
        triggered_at: triggered_at.clone(),
        message,
    };
    state.record_activity_events(&[ActivityEvent {
        id: format!(
            "scw_storage_{}_{}_{}",
            result.action,
            result.resource_id,
            triggered_at.replace([':', '.', '+', '-'], "_")
        ),
        message: result.message.clone(),
        timestamp: triggered_at,
        event_type: "action".into(),
        source: "Scaleway".into(),
    }])?;
    Ok(result)
}

/// CREATE a Block Storage volume. Session-gated; the target project HARD-FAILS
/// unless it equals the pinned Aspis Bio project.
#[tauri::command]
pub async fn create_scaleway_block_volume(
    state: State<'_, BackendState>,
    request: ScalewayBlockVolumeCreateRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let name = validate_scaleway_storage_name(&request.name, "Volume name")?;
    let zone = validate_scaleway_location(&request.zone, "Zone")?;
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    let size_bytes = scaleway_size_gib_to_bytes(request.size_gib)?;
    // Project HARD-FAIL on CREATE: target project must equal the pin.
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_create_project_is_pinned(&project_id, pinned.as_deref())?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    let req = ScalewayBlockCreateRequest {
        zone: &zone,
        name: &name,
        project_id: &project_id,
        size_bytes,
        perf_iops: request.perf_iops,
        tags: &request.tags,
    };
    let new_id = create_scaleway_block_volume_request(&state.http, &token, &req)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &name,
        "Block Storage",
        "create",
        format!("Created Block Storage volume {name}. Sync to confirm the resulting state."),
    )
}

/// RESIZE a Block Storage volume. REFUSES a shrink (pure check) before any call.
#[tauri::command]
pub async fn resize_scaleway_block_volume(
    state: State<'_, BackendState>,
    resource_id: String,
    new_size_gib: u64,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if !scaleway_uuid_is_valid(&resource_id) {
        return Err("Scaleway volume id is invalid.".into());
    }
    let resource = scaleway_storage_inventory_guard(&state, &resource_id)?;
    if !resource.storage_type.starts_with("Block Storage") {
        return Err("Resize applies only to Block Storage volumes.".into());
    }
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_storage_in_pinned_project(&resource, pinned.as_deref())?;
    let new_size_bytes = scaleway_size_gib_to_bytes(new_size_gib)?;
    // REFUSE shrink. Compare on ONE consistent basis (bytes): the volume's current
    // byte size is reconstructed from the inventory `size_gb` (DECIMAL GB, i.e.
    // bytes / 1e9) and compared against the requested new size in bytes (GiB
    // basis). A true grow always passes and a true shrink always fails; the only
    // edge is a "same logical size" re-entered in GiB, which is larger in bytes
    // than the decimal-GB figure, so it passes (treated as a harmless grow/no-op).
    let current_bytes = (resource.size_gb * 1_000_000_000.0).round() as u64;
    scaleway_block_resize_is_allowed(current_bytes, new_size_bytes)?;
    // DEFENSE-IN-DEPTH: validate the CACHED inventory region before it enters the
    // URL path. The create path validates the user region; the mutation paths must
    // not trust the cached value raw (see `validate_scaleway_location`).
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    resize_scaleway_block_volume_request(
        &state.http,
        &token,
        &region,
        &resource_id,
        new_size_bytes,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        &resource.storage_type,
        "resize",
        format!(
            "Resized Block Storage volume {} to {} GiB. Sync to confirm.",
            resource.name, new_size_gib
        ),
    )
}

/// CREATE a Block Storage snapshot from a volume already proven in inventory.
#[tauri::command]
pub async fn create_scaleway_block_snapshot(
    state: State<'_, BackendState>,
    volume_id: String,
    name: String,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let volume_id = volume_id.trim().to_string();
    if !scaleway_uuid_is_valid(&volume_id) {
        return Err("Scaleway volume id is invalid.".into());
    }
    let name = validate_scaleway_storage_name(&name, "Snapshot name")?;
    let volume = scaleway_storage_inventory_guard(&state, &volume_id)?;
    if !volume.storage_type.starts_with("Block Storage") {
        return Err("Snapshots apply only to Block Storage volumes.".into());
    }
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_storage_in_pinned_project(&volume, pinned.as_deref())?;
    let project_id = volume
        .project_id
        .clone()
        .ok_or_else(|| "Scaleway volume project is unknown. Sync before acting.".to_string())?;
    // DEFENSE-IN-DEPTH: validate the CACHED inventory region before it enters the
    // URL path (the create path validates user input; mutations must not trust the
    // cached value raw).
    let region = validate_scaleway_location(&volume.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    let new_id = create_scaleway_block_snapshot_request(
        &state.http,
        &token,
        &region,
        &name,
        &project_id,
        &volume_id,
        &[],
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &name,
        "Block Snapshot",
        "create",
        format!(
            "Created Block snapshot {name} from {}. Sync to confirm.",
            volume.name
        ),
    )
}

/// DELETE a Block Storage volume or snapshot. Confirm-by-name. If a volume is
/// attached to a running instance, surface a best-effort warning (the delete
/// still proceeds — Scaleway itself rejects an attached volume).
#[tauri::command]
pub async fn delete_scaleway_block_storage(
    state: State<'_, BackendState>,
    resource_id: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if !scaleway_uuid_is_valid(&resource_id) {
        return Err("Scaleway storage id is invalid.".into());
    }
    let resource = scaleway_storage_inventory_guard(&state, &resource_id)?;
    let is_snapshot = resource.storage_type == "Block Snapshot";
    let is_volume = resource.storage_type.starts_with("Block Storage");
    if !is_snapshot && !is_volume {
        return Err("This resource is not a Block Storage volume or snapshot.".into());
    }
    validate_scaleway_storage_action_request(
        &resource,
        "delete",
        confirm_resource_name.as_deref(),
    )?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_storage_in_pinned_project(&resource, pinned.as_deref())?;
    // DEFENSE-IN-DEPTH: validate the CACHED inventory region before it enters the
    // URL path (the create path validates user input; mutations must not trust the
    // cached value raw).
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    if is_snapshot {
        delete_scaleway_block_snapshot_request(&state.http, &token, &region, &resource_id)
            .await
            .map_err(|e| sanitize_error_message(&e))?;
    } else {
        delete_scaleway_block_volume_request(&state.http, &token, &region, &resource_id)
            .await
            .map_err(|e| sanitize_error_message(&e))?;
    }
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        &resource.storage_type,
        "delete",
        format!(
            "Delete requested for {} {}. Sync to confirm the resulting state.",
            resource.storage_type, resource.name
        ),
    )
}

/// CREATE a File Storage filesystem. Session-gated; project HARD-FAIL on CREATE.
#[tauri::command]
pub async fn create_scaleway_filesystem(
    state: State<'_, BackendState>,
    request: ScalewayFilesystemCreateRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let name = validate_scaleway_storage_name(&request.name, "Filesystem name")?;
    let region = validate_scaleway_location(&request.region, "Region")?;
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    let size_bytes = scaleway_size_gib_to_bytes(request.size_gib)?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_create_project_is_pinned(&project_id, pinned.as_deref())?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    let new_id = create_scaleway_filesystem_request(
        &state.http,
        &token,
        &region,
        &name,
        &project_id,
        size_bytes,
        &request.tags,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &name,
        "File System",
        "create",
        format!("Created File Storage filesystem {name}. Sync to confirm."),
    )
}

/// DELETE a File Storage filesystem. Confirm-by-name.
#[tauri::command]
pub async fn delete_scaleway_filesystem(
    state: State<'_, BackendState>,
    resource_id: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if !scaleway_uuid_is_valid(&resource_id) {
        return Err("Scaleway filesystem id is invalid.".into());
    }
    let resource = scaleway_storage_inventory_guard(&state, &resource_id)?;
    if resource.storage_type != "File System" {
        return Err("This resource is not a File Storage filesystem.".into());
    }
    validate_scaleway_storage_action_request(
        &resource,
        "delete",
        confirm_resource_name.as_deref(),
    )?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_storage_in_pinned_project(&resource, pinned.as_deref())?;
    // DEFENSE-IN-DEPTH: validate the CACHED inventory region before it enters the
    // URL path (mutations must not trust the cached value raw).
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    delete_scaleway_filesystem_request(&state.http, &token, &region, &resource_id)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        &resource.storage_type,
        "delete",
        format!(
            "Delete requested for File Storage filesystem {}. Sync to confirm.",
            resource.name
        ),
    )
}

/// CREATE an Object Storage bucket (S3 SigV4). Gated by the S3 keypair; the
/// target project HARD-FAILS unless it equals the pinned project.
#[tauri::command]
pub async fn create_scaleway_object_bucket(
    state: State<'_, BackendState>,
    request: ScalewayObjectBucketCreateRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let name = validate_scaleway_object_bucket_name(&request.name)?;
    let region = validate_scaleway_location(&request.region, "Region")?;
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    // Project HARD-FAIL on CREATE (assert before the call). The S3 layer itself
    // is not project-scoped, so this guard is what binds a new bucket to the pin.
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_create_project_is_pinned(&project_id, pinned.as_deref())?;
    let (access_key, secret_key) = scaleway_object_storage_keypair()?;
    state.ensure_same_sensitive_session(session_id)?;
    create_scaleway_object_bucket_request(
        &state.http,
        &access_key,
        &secret_key,
        &project_id,
        &region,
        &name,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &format!("object_bucket_{region}_{name}"),
        &name,
        "Object Bucket",
        "create",
        format!("Created Object Storage bucket {name} in {region}. Sync to confirm."),
    )
}

/// DELETE an Object Storage bucket (S3 SigV4). Confirm-by-name; the bucket must
/// be in the synced inventory. A non-empty bucket is refused by S3 and the error
/// is surfaced verbatim (NO automatic cascade).
#[tauri::command]
pub async fn delete_scaleway_object_bucket(
    state: State<'_, BackendState>,
    resource_id: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if resource_id.is_empty() || resource_id.contains('/') || resource_id.contains('\\') {
        return Err("Scaleway bucket id is invalid.".into());
    }
    let resource = scaleway_storage_inventory_guard(&state, &resource_id)?;
    if resource.storage_type != "Object Bucket" {
        return Err("This resource is not an Object Storage bucket.".into());
    }
    validate_scaleway_storage_action_request(
        &resource,
        "delete",
        confirm_resource_name.as_deref(),
    )?;
    // Bucket carries its project id in inventory; pin guard applies here too.
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_storage_in_pinned_project(&resource, pinned.as_deref())?;
    let project_id = resource
        .project_id
        .clone()
        .ok_or_else(|| "Scaleway bucket project is unknown. Sync before acting.".to_string())?;
    // CRITICAL: the region flows into the S3 host `s3.{region}.scw.cloud`, which
    // CANNOT be URL-encoded — a value like `fr-par@evil.com` would be an SSRF /
    // host-injection vector. The strict location allowlist is the ONLY defense
    // here, so validate the cached inventory region before building the request.
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let (access_key, secret_key) = scaleway_object_storage_keypair()?;
    state.ensure_same_sensitive_session(session_id)?;
    delete_scaleway_object_bucket_request(
        &state.http,
        &access_key,
        &secret_key,
        &project_id,
        &region,
        &resource.name,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        &resource.storage_type,
        "delete",
        format!(
            "Delete requested for Object Storage bucket {}. Sync to confirm.",
            resource.name
        ),
    )
}

/// SET an Object Storage bucket lifecycle (S3 SigV4 PUT ?lifecycle). Gated by
/// the S3 keypair + the bucket being in the synced inventory + project pin.
#[tauri::command]
pub async fn set_scaleway_object_bucket_lifecycle(
    state: State<'_, BackendState>,
    resource_id: String,
    rules: serde_json::Value,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if resource_id.is_empty() || resource_id.contains('/') || resource_id.contains('\\') {
        return Err("Scaleway bucket id is invalid.".into());
    }
    let resource = scaleway_storage_inventory_guard(&state, &resource_id)?;
    if resource.storage_type != "Object Bucket" {
        return Err("This resource is not an Object Storage bucket.".into());
    }
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_storage_in_pinned_project(&resource, pinned.as_deref())?;
    let project_id = resource
        .project_id
        .clone()
        .ok_or_else(|| "Scaleway bucket project is unknown. Sync before acting.".to_string())?;
    // CRITICAL: the region flows into the S3 host `s3.{region}.scw.cloud`, which
    // CANNOT be URL-encoded. The strict location allowlist is the ONLY defense, so
    // validate the cached inventory region before building the signed request.
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let (access_key, secret_key) = scaleway_object_storage_keypair()?;
    state.ensure_same_sensitive_session(session_id)?;
    set_scaleway_object_bucket_lifecycle_request(
        &state.http,
        &access_key,
        &secret_key,
        &project_id,
        &region,
        &resource.name,
        &rules,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        &resource.storage_type,
        "lifecycle",
        format!(
            "Updated lifecycle configuration for Object Storage bucket {}.",
            resource.name
        ),
    )
}

// ===========================================================================
// Scaleway SERVERLESS CRUD (Serverless SQL / Functions / Containers). These are
// compute-side resources (`scaleway_resource` inventory, `ScalewayResourceSummary`)
// rather than storage, so delete reuses the COMPUTE confirm-by-name validator
// (`validate_scaleway_action_request`) and the compute project-pin guard
// (`assert_scaleway_resource_in_pinned_project`). CREATE reuses the same
// project-pin-on-create guard as storage (`assert_scaleway_create_project_is_pinned`).
//
// NOTE: there is intentionally NO Serverless SQL "query" command — a Serverless
// SQL `endpoint` is a raw PostgreSQL DSN and querying it needs a Postgres
// wire-protocol client (a new crate / Cargo.toml dependency, off-limits here).
// The endpoint is surfaced in inventory so the UI can offer "connect with psql"
// + a console link; query is deferred pending a Postgres-client dependency.
// ===========================================================================

/// Serverless-side presence/health gate (mirrors `scaleway_storage_inventory_guard`
/// but returns a `ScalewayResourceSummary`). It does NOT check `available_actions`,
/// because serverless resources expose only `deploy` there while delete maps to
/// the `terminate` api action — a presence + project check is what binds the call.
fn scaleway_serverless_inventory_guard(
    state: &BackendState,
    resource_id: &str,
) -> Result<ScalewayResourceSummary, String> {
    let health = state.scaleway_health()?.ok_or_else(|| {
        "Scaleway inventory is not loaded. Sync Scaleway before acting.".to_string()
    })?;
    if !matches!(health.status.as_str(), "healthy" | "degraded") {
        return Err(
            "Scaleway inventory is stale or unavailable. Sync successfully before acting.".into(),
        );
    }
    state.scaleway_resource(resource_id)?.ok_or_else(|| {
        "Resource is not in the current Scaleway inventory. Sync Scaleway before acting."
            .to_string()
    })
}

/// Resolve the Scaleway organization id for an org-scoped create (Serverless SQL).
/// HARD-FAILS when it cannot be resolved, because the create body REQUIRES it —
/// proceeding without it would send a malformed request.
async fn scaleway_org_id_for_create(
    state: &BackendState,
    token: &str,
    project_id: &str,
) -> Result<String, String> {
    resolve_scaleway_org_id(&state.http, token, project_id)
        .await
        .filter(|org| scaleway_uuid_is_valid(org))
        .ok_or_else(|| {
            "Scaleway organization id could not be resolved. Set ASPIS_SCALEWAY_ORG_ID or ensure the pinned project is readable before creating a Serverless SQL database."
                .to_string()
        })
}

/// Validate a Serverless Function runtime against the strict allowlist.
fn validate_scaleway_function_runtime(runtime: &str) -> Result<String, String> {
    let runtime = runtime.trim();
    if !SCW_FUNCTION_RUNTIMES.contains(&runtime) {
        return Err("Scaleway Function runtime is not supported.".into());
    }
    Ok(runtime.to_string())
}

/// Validate an optional memory limit (MB) against the hard ceiling.
fn validate_scaleway_memory_limit(memory_limit: Option<u32>) -> Result<Option<u32>, String> {
    if let Some(memory) = memory_limit {
        if memory == 0 || memory > SCW_SERVERLESS_MAX_MEMORY_MB {
            return Err("Scaleway memory limit is out of range.".into());
        }
    }
    Ok(memory_limit)
}

/// Validate optional min/max scale: each within the ceiling, and min <= max.
fn validate_scaleway_scale(
    min_scale: Option<u32>,
    max_scale: Option<u32>,
) -> Result<(Option<u32>, Option<u32>), String> {
    if let Some(min) = min_scale {
        if min > SCW_SERVERLESS_MAX_SCALE {
            return Err("Scaleway min scale is out of range.".into());
        }
    }
    if let Some(max) = max_scale {
        if max == 0 || max > SCW_SERVERLESS_MAX_SCALE {
            return Err("Scaleway max scale is out of range.".into());
        }
    }
    // FIX 5: an explicit min>0 with NO max is ambiguous — the API may default the
    // max below min and reject the create. Require an explicit max whenever min>0
    // so the two-sided range is always well-formed before we send it. min==0 (or
    // absent) with no max is fine: that is scale-to-zero with the API default max.
    if let Some(min) = min_scale {
        if min > 0 && max_scale.is_none() {
            return Err(
                "Scaleway max scale is required when min scale is greater than zero.".into(),
            );
        }
    }
    if let (Some(min), Some(max)) = (min_scale, max_scale) {
        if min > max {
            return Err("Scaleway min scale cannot exceed max scale.".into());
        }
    }
    Ok((min_scale, max_scale))
}

/// Validate a registry image reference: non-empty, length-capped, and free of
/// whitespace or characters that could break the JSON body / inject. We do NOT
/// over-constrain the grammar (registry refs carry '/', '.', ':', '@'), but a
/// hostile control character or space is rejected.
fn validate_scaleway_registry_image(image: &str) -> Result<String, String> {
    let image = image.trim();
    if image.is_empty() {
        return Err("Container registry image is required.".into());
    }
    if image.len() > 512 {
        return Err("Container registry image reference is too long.".into());
    }
    if image.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Container registry image reference contains invalid characters.".into());
    }
    Ok(image.to_string())
}

/// CREATE a Serverless SQL database. Session-gated; the target project HARD-FAILS
/// unless it equals the pinned project. The org id is resolved (required by the
/// API) and the create fails closed if it cannot be determined.
#[tauri::command]
pub async fn create_scaleway_sql_database(
    state: State<'_, BackendState>,
    request: ScalewaySqlDatabaseCreateRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let name = validate_scaleway_storage_name(&request.name, "Database name")?;
    let region = validate_scaleway_location(&request.region, "Region")?;
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    if request.cpu_max == 0
        || request.cpu_min > request.cpu_max
        || request.cpu_max > SCW_SQL_MAX_CPU
    {
        return Err("Scaleway Serverless SQL CPU range is invalid.".into());
    }
    // Project HARD-FAIL on CREATE: target project must equal the pin.
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_create_project_is_pinned(&project_id, pinned.as_deref())?;
    let token = scaleway_project_token()?;
    // FIX 3: re-assert the sensitive session BEFORE the org-id GET (a network call),
    // mirroring the function/container ordering, so no network call straddles a
    // stale session.
    state.ensure_same_sensitive_session(session_id)?;
    let organization_id = scaleway_org_id_for_create(&state, &token, &project_id).await?;
    state.ensure_same_sensitive_session(session_id)?;
    let req = ScalewaySqlCreateRequest {
        region: &region,
        name: &name,
        organization_id: &organization_id,
        project_id: &project_id,
        cpu_min: request.cpu_min,
        cpu_max: request.cpu_max,
    };
    let new_id = create_scaleway_sql_database_request(&state.http, &token, &req)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &name,
        "Serverless SQL",
        "create",
        format!("Created Serverless SQL database {name}. Sync to confirm; connect with psql via its endpoint."),
    )
}

/// DELETE a Serverless SQL database. Confirm-by-name + inventory presence +
/// project HARD-FAIL.
#[tauri::command]
pub async fn delete_scaleway_sql_database(
    state: State<'_, BackendState>,
    resource_id: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if !scaleway_uuid_is_valid(&resource_id) {
        return Err("Scaleway database id is invalid.".into());
    }
    let resource = scaleway_serverless_inventory_guard(&state, &resource_id)?;
    if resource.resource_type != "Serverless SQL" {
        return Err("This resource is not a Serverless SQL database.".into());
    }
    validate_scaleway_action_request(&resource, "delete", confirm_resource_name.as_deref())?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_resource_in_pinned_project(&resource, pinned.as_deref())?;
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    delete_scaleway_sql_database_request(&state.http, &token, &region, &resource_id)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        &resource.resource_type,
        "delete",
        format!(
            "Delete requested for Serverless SQL database {}. Sync to confirm.",
            resource.name
        ),
    )
}

/// A validated Instance create request: every field has passed the strict input
/// guards (non-empty name + commercial_type, allowlisted zone, UUID image + project,
/// project == pin). Produced by `validate_scaleway_instance_request`; consumed by
/// both the dry-run and the mutation so they can never diverge.
struct ValidatedScalewayInstance {
    name: String,
    zone: String,
    commercial_type: String,
    image: String,
    project_id: String,
    dynamic_ip_required: bool,
    tags: Vec<String>,
}

/// PURE: validate an Instance create request and assert the target project equals
/// the pinned project. No network, no state — unit-tested directly. Mirrors the
/// guard chain of the storage creates: name/type non-empty, zone via the strict
/// location allowlist, image + project as UUIDs, project HARD-FAIL on mismatch.
fn validate_scaleway_instance_request(
    request: &ScalewayInstanceCreateRequest,
    pinned_project_id: Option<&str>,
) -> Result<ValidatedScalewayInstance, String> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err("Instance name is required.".into());
    }
    // Bound name length (Scaleway caps server names at 255 bytes) so an oversized
    // string can never reach the API body.
    if name.len() > 255 {
        return Err("Instance name must be 255 bytes or fewer.".into());
    }
    let zone = validate_scaleway_location(&request.zone, "Zone")?;
    let commercial_type = request.commercial_type.trim();
    if commercial_type.is_empty() {
        return Err("Instance commercial type is required.".into());
    }
    // Bound commercial_type length defensively; real offer names are short.
    if commercial_type.len() > 64 {
        return Err("Instance commercial type must be 64 bytes or fewer.".into());
    }
    let image = request.image.trim().to_string();
    if !scaleway_uuid_is_valid(&image) {
        return Err("Scaleway image id is invalid.".into());
    }
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    // Project HARD-FAIL on CREATE: target project must equal the pin (asserted here,
    // BEFORE any network call in the mutation path).
    assert_scaleway_create_project_is_pinned(&project_id, pinned_project_id)?;
    // Normalize tags (drop blanks) so the body never carries empty strings.
    let tags = request
        .tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    // Bound the tag set: at most 20 tags, each ≤128 bytes (matches Scaleway limits)
    // so an oversized tag list can never reach the API body.
    if tags.len() > 20 {
        return Err("An Instance may carry at most 20 tags.".into());
    }
    if let Some(tag) = tags.iter().find(|tag| tag.len() > 128) {
        return Err(format!(
            "Tag \"{}\" exceeds the 128-byte limit.",
            tag.chars().take(32).collect::<String>()
        ));
    }
    Ok(ValidatedScalewayInstance {
        name: name.to_string(),
        zone,
        commercial_type: commercial_type.to_string(),
        image,
        project_id,
        dynamic_ip_required: request.dynamic_ip_required,
        tags,
    })
}

/// PURE: assemble the dry-run preview from a validated request + the synced offer
/// catalog. Builds the EXACT body that the mutation would POST and prices the
/// chosen offer; a missing offer yields `None` cost + a risk note (never a fake 0).
fn build_scaleway_instance_dry_run(
    validated: &ValidatedScalewayInstance,
    offers: &[ScalewayOfferSummary],
) -> ScalewayInstanceDryRunResult {
    let body = scaleway_instance_create_body(
        &validated.name,
        &validated.project_id,
        &validated.commercial_type,
        &validated.image,
        validated.dynamic_ip_required,
        &validated.tags,
    );
    let body_preview = serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
    let cost = scaleway_instance_offer_cost(offers, &validated.zone, &validated.commercial_type);
    let mut risks = Vec::new();
    if let Some(risk) = cost.risk {
        risks.push(risk);
    }
    ScalewayInstanceDryRunResult {
        zone: validated.zone.clone(),
        commercial_type: validated.commercial_type.clone(),
        image: validated.image.clone(),
        project_id: validated.project_id.clone(),
        dynamic_ip_required: validated.dynamic_ip_required,
        estimated_hourly_eur: cost.hourly_eur,
        estimated_monthly_eur: cost.monthly_eur,
        body_preview,
        risks,
    }
}

/// READ-ONLY preview of an Instance CREATE: validates inputs (incl. project pin),
/// builds the exact POST body, and prices the chosen `commercial_type` from the
/// synced offer catalog. NO network, NO token, NO mutation. A missing offer is
/// surfaced as `None` cost + a risk note, never a fabricated zero.
#[tauri::command]
pub async fn scaleway_instance_create_dry_run(
    state: State<'_, BackendState>,
    request: ScalewayInstanceCreateRequest,
) -> Result<ScalewayInstanceDryRunResult, String> {
    // Gate on an unlocked session like the Cloudflare dry-runs: even though this is
    // read-only/no-network, a locked app must not leak the pinned project UUID or
    // offer pricing. Capture the id first, re-check after building the preview.
    let session_id = state.sensitive_session_id()?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    let validated = validate_scaleway_instance_request(&request, pinned.as_deref())?;
    let offers = state.scaleway_offers()?;
    let result = build_scaleway_instance_dry_run(&validated, &offers);
    state.ensure_same_sensitive_session(session_id)?;
    Ok(result)
}

/// CREATE a Scaleway Instance (compute server). Full guard chain: sensitive-session
/// bracket, strict input validation, project HARD-FAIL before the call, vault token,
/// 20s write budget, `error_for_status`, and create-returned-empty-id -> error.
#[tauri::command]
pub async fn create_scaleway_instance(
    state: State<'_, BackendState>,
    request: ScalewayInstanceCreateRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    let validated = validate_scaleway_instance_request(&request, pinned.as_deref())?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    let new_id = create_scaleway_instance_request(
        &state.http,
        &token,
        &validated.zone,
        &validated.name,
        &validated.project_id,
        &validated.commercial_type,
        &validated.image,
        validated.dynamic_ip_required,
        &validated.tags,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &validated.name,
        "Instance",
        "create",
        format!(
            "Created Instance {} ({}) in {}. Sync to confirm.",
            validated.name, validated.commercial_type, validated.zone
        ),
    )
}

/// FIX 1: whether a namespace GET payload's `project_id` equals the pinned project.
/// Pure so it is unit-tested directly. Fails closed: a missing or blank `project_id`
/// on the namespace does NOT match (we never treat an unattributed namespace as in
/// the pinned project).
fn scaleway_namespace_project_matches(ns_json: &serde_json::Value, pinned: &str) -> bool {
    let pinned = pinned.trim();
    if pinned.is_empty() {
        return false;
    }
    ns_json
        .get("project_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value == pinned)
        .unwrap_or(false)
}

/// Resolve the target namespace for a function/container create: an explicit
/// `namespace_id` (validated UUID) is reused after verifying it belongs to the
/// pinned project; otherwise a new namespace is created with the given name (or
/// the resource name) in the pinned project.
async fn scaleway_resolve_or_create_namespace(
    state: &BackendState,
    token: &str,
    region: &str,
    project_id: &str,
    explicit_namespace_id: Option<&str>,
    namespace_name: Option<&str>,
    fallback_name: &str,
    is_container: bool,
) -> Result<String, String> {
    if let Some(ns) = explicit_namespace_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !scaleway_uuid_is_valid(ns) {
            return Err("Scaleway namespace id is invalid.".into());
        }
        // FIX 1: an explicitly-supplied namespace_id must be verified to belong to
        // the pinned project BEFORE we create a function/container inside it.
        // Otherwise a caller could pass a namespace from a FOREIGN project (whose
        // own project_id == pinned would still pass the create-pin guard) and the
        // resource would be created in that foreign project, bypassing the pin.
        // GET the namespace, read its project_id, and fail CLOSED on any mismatch
        // or unreadable namespace. (The auto-create path below already targets the
        // pinned project, so it needs no extra check.)
        let ns_json =
            fetch_scaleway_namespace_request(&state.http, token, region, ns, is_container).await?;
        if !scaleway_namespace_project_matches(&ns_json, project_id) {
            return Err(
                "Scaleway namespace does not belong to the pinned project — refusing to create into a foreign project."
                    .into(),
            );
        }
        return Ok(ns.to_string());
    }
    let ns_name = namespace_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_name);
    let ns_name = validate_scaleway_storage_name(ns_name, "Namespace name")?;
    if is_container {
        create_scaleway_container_namespace_request(
            &state.http,
            token,
            region,
            &ns_name,
            project_id,
        )
        .await
    } else {
        create_scaleway_function_namespace_request(&state.http, token, region, &ns_name, project_id)
            .await
    }
}

/// CREATE a Serverless Function (the function resource only — code upload is a
/// separate deploy step). Creates the namespace first when one is not supplied.
/// Project HARD-FAIL on CREATE.
#[tauri::command]
pub async fn create_scaleway_function(
    state: State<'_, BackendState>,
    request: ScalewayFunctionCreateCommandRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let name = validate_scaleway_storage_name(&request.name, "Function name")?;
    let region = validate_scaleway_location(&request.region, "Region")?;
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    let runtime = validate_scaleway_function_runtime(&request.runtime)?;
    let memory_limit = validate_scaleway_memory_limit(request.memory_limit)?;
    let (min_scale, max_scale) = validate_scaleway_scale(request.min_scale, request.max_scale)?;
    // Project HARD-FAIL on CREATE.
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_create_project_is_pinned(&project_id, pinned.as_deref())?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    let namespace_id = scaleway_resolve_or_create_namespace(
        &state,
        &token,
        &region,
        &project_id,
        request.namespace_id.as_deref(),
        request.namespace_name.as_deref(),
        &name,
        false,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    let req = ScalewayFunctionCreateRequest {
        region: &region,
        namespace_id: &namespace_id,
        name: &name,
        runtime: &runtime,
        memory_limit,
        min_scale,
        max_scale,
    };
    let new_id = create_scaleway_function_request(&state.http, &token, &req)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &name,
        "Serverless",
        "create",
        format!("Created Serverless function {name}. Deploy its code, then sync to confirm."),
    )
}

/// DELETE a Serverless Function. Confirm-by-name + inventory presence + project
/// HARD-FAIL. Refuses a container (which is also `Serverless`) via its runtime.
#[tauri::command]
pub async fn delete_scaleway_function(
    state: State<'_, BackendState>,
    resource_id: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if !scaleway_uuid_is_valid(&resource_id) {
        return Err("Scaleway function id is invalid.".into());
    }
    let resource = scaleway_serverless_inventory_guard(&state, &resource_id)?;
    if resource.resource_type != "Serverless" {
        return Err("This resource is not a Serverless function.".into());
    }
    // FIX 4: refuse an ambiguous (no-runtime) resource rather than deleting against
    // the wrong product; only proceed when the runtime unambiguously says function.
    if scaleway_serverless_kind(&resource)? != ScalewayServerlessKind::Function {
        return Err("This resource is not a Serverless function.".into());
    }
    validate_scaleway_action_request(&resource, "delete", confirm_resource_name.as_deref())?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_resource_in_pinned_project(&resource, pinned.as_deref())?;
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    delete_scaleway_function_request(&state.http, &token, &region, &resource_id)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        "Serverless",
        "delete",
        format!(
            "Delete requested for Serverless function {}. Sync to confirm.",
            resource.name
        ),
    )
}

/// CREATE a Serverless Container referencing an EXISTING registry image (no image
/// build). Creates the namespace first when one is not supplied. Project
/// HARD-FAIL on CREATE.
#[tauri::command]
pub async fn create_scaleway_container(
    state: State<'_, BackendState>,
    request: ScalewayContainerCreateCommandRequest,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let name = validate_scaleway_storage_name(&request.name, "Container name")?;
    let region = validate_scaleway_location(&request.region, "Region")?;
    let project_id = request.project_id.trim().to_string();
    if !scaleway_uuid_is_valid(&project_id) {
        return Err("Scaleway project id is invalid.".into());
    }
    let registry_image = validate_scaleway_registry_image(&request.registry_image)?;
    let memory_limit = validate_scaleway_memory_limit(request.memory_limit)?;
    let (min_scale, max_scale) = validate_scaleway_scale(request.min_scale, request.max_scale)?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_create_project_is_pinned(&project_id, pinned.as_deref())?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    let namespace_id = scaleway_resolve_or_create_namespace(
        &state,
        &token,
        &region,
        &project_id,
        request.namespace_id.as_deref(),
        request.namespace_name.as_deref(),
        &name,
        true,
    )
    .await
    .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    let req = ScalewayContainerCreateRequest {
        region: &region,
        namespace_id: &namespace_id,
        name: &name,
        registry_image: &registry_image,
        memory_limit,
        min_scale,
        max_scale,
    };
    let new_id = create_scaleway_container_request(&state.http, &token, &req)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &new_id,
        &name,
        "Serverless",
        "create",
        format!("Created Serverless container {name}. Deploy it, then sync to confirm."),
    )
}

/// DELETE a Serverless Container. Confirm-by-name + inventory presence + project
/// HARD-FAIL. Refuses a function (which is also `Serverless`) via its runtime.
#[tauri::command]
pub async fn delete_scaleway_container(
    state: State<'_, BackendState>,
    resource_id: String,
    confirm_resource_name: Option<String>,
) -> Result<ScalewayActionResult, String> {
    let session_id = state.sensitive_session_id()?;
    let resource_id = resource_id.trim().to_string();
    if !scaleway_uuid_is_valid(&resource_id) {
        return Err("Scaleway container id is invalid.".into());
    }
    let resource = scaleway_serverless_inventory_guard(&state, &resource_id)?;
    if resource.resource_type != "Serverless" {
        return Err("This resource is not a Serverless container.".into());
    }
    // FIX 4: refuse an ambiguous (no-runtime) resource rather than deleting against
    // the wrong product; only proceed when the runtime unambiguously says container.
    if scaleway_serverless_kind(&resource)? != ScalewayServerlessKind::Container {
        return Err("This resource is not a Serverless container.".into());
    }
    validate_scaleway_action_request(&resource, "delete", confirm_resource_name.as_deref())?;
    let pinned = configured_or_pinned_scaleway_project_id()?;
    assert_scaleway_resource_in_pinned_project(&resource, pinned.as_deref())?;
    let region = validate_scaleway_location(&resource.region, "Region")?;
    let token = scaleway_project_token()?;
    state.ensure_same_sensitive_session(session_id)?;
    delete_scaleway_container_request(&state.http, &token, &region, &resource_id)
        .await
        .map_err(|e| sanitize_error_message(&e))?;
    state.ensure_same_sensitive_session(session_id)?;
    scaleway_storage_result(
        &state,
        &resource.id,
        &resource.name,
        "Serverless",
        "delete",
        format!(
            "Delete requested for Serverless container {}. Sync to confirm.",
            resource.name
        ),
    )
}

/// Whether a `Serverless` resource is a function or a container. Functions and
/// containers share the `Serverless` resource_type, so the runtime is the only
/// signal that disambiguates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalewayServerlessKind {
    Function,
    Container,
}

/// Classify a `Serverless` resource as a function or a container from its runtime.
///
/// FIX 4: a container's runtime is `container` / `container/*`; a function's is a
/// language runtime (e.g. `python311`). When the runtime is MISSING or blank the
/// type is AMBIGUOUS — we must NOT guess, because guessing the wrong product makes
/// the delete hit the wrong endpoint and swallow the 404 as idempotent (the real
/// resource survives while the UI reports success). Refuse and ask for a sync.
fn scaleway_serverless_kind(
    resource: &ScalewayResourceSummary,
) -> Result<ScalewayServerlessKind, String> {
    match resource.runtime.as_deref().map(str::trim) {
        Some(runtime) if runtime == "container" || runtime.starts_with("container/") => {
            Ok(ScalewayServerlessKind::Container)
        }
        Some(runtime) if !runtime.is_empty() => Ok(ScalewayServerlessKind::Function),
        _ => Err("Cannot determine if this is a function or container — sync and retry.".into()),
    }
}

#[tauri::command]
pub async fn get_cloud_dashboard_snapshot(
    state: State<'_, BackendState>,
) -> Result<CloudDashboardSnapshot, String> {
    let auth = state.auth_state()?;
    if auth.locked {
        return Ok(CloudDashboardSnapshot::locked(auth));
    }
    build_snapshot(&state, None).await
}

#[tauri::command]
pub async fn sync_provider_inventory(
    state: State<'_, BackendState>,
    provider: Option<ProviderId>,
) -> Result<CloudDashboardSnapshot, String> {
    state.sensitive_session_id()?;
    build_snapshot(&state, provider).await
}

async fn build_snapshot(
    state: &BackendState,
    provider_filter: Option<ProviderId>,
) -> Result<CloudDashboardSnapshot, String> {
    let session_id = state.sensitive_session_id()?;
    let include_cf = provider_filter.is_none() || provider_filter == Some(ProviderId::Cloudflare);
    let include_scw = provider_filter.is_none() || provider_filter == Some(ProviderId::Scaleway);

    let cf_token = if include_cf {
        vault::read_token(ProviderId::Cloudflare).map_err(|e| sanitize_error_message(&e))?
    } else {
        None
    };
    let scw_token = if include_scw {
        vault::read_token(ProviderId::Scaleway).map_err(|e| sanitize_error_message(&e))?
    } else {
        None
    };
    let cf_scope = if include_cf {
        vault::read_scope(ProviderId::Cloudflare).map_err(|e| sanitize_error_message(&e))?
    } else {
        None
    };
    let scw_scope = if include_scw {
        vault::read_scope(ProviderId::Scaleway).map_err(|e| sanitize_error_message(&e))?
    } else {
        None
    };
    let scw_object_access_key = if include_scw {
        vault::read_scaleway_object_access_key().map_err(|e| sanitize_error_message(&e))?
    } else {
        None
    };
    let scw_object_secret_key = if include_scw {
        vault::read_scaleway_object_secret_key().map_err(|e| sanitize_error_message(&e))?
    } else {
        None
    };

    let cf = if include_cf {
        Some(fetch_cloudflare(&state.http, cf_token.clone(), cf_scope.clone()).await)
    } else {
        None
    };
    let scw = if include_scw {
        Some(
            fetch_scaleway(
                &state.http,
                scw_token.clone(),
                scw_scope.clone(),
                scw_object_access_key,
                scw_object_secret_key,
            )
            .await,
        )
    } else {
        None
    };

    let mut activity = Vec::new();

    state.ensure_same_sensitive_session(session_id)?;
    if let Some(cf) = cf {
        let cf = preserve_cached_resources_on_sync_error(state, cf, cf_scope.as_deref())?;
        state.replace_provider_inventory(cf)?;
    }
    if let Some(scw) = scw {
        let scw = preserve_cached_resources_on_sync_error(state, scw, scw_scope.as_deref())?;
        if matches!(scw.health.status.as_str(), "healthy" | "degraded") {
            let replacement = state.replace_scaleway_compute(scw.compute.clone())?;
            if replacement.had_previous_snapshot {
                let lifecycle_events =
                    scaleway_lifecycle_events(&replacement.previous, &scw.compute, &now());
                state.record_activity_events(&lifecycle_events)?;
                activity.extend(lifecycle_events);
            }
        }
        state.replace_provider_inventory(scw)?;
    }

    state.ensure_same_sensitive_session(session_id)?;
    let parts = snapshot_parts_from_provider_inventories(state.cached_provider_inventories()?);
    let scaleway_offers = fetch_scaleway_offers(&state.http).await;
    // FIX 2: re-check the sensitive session BEFORE writing the cache. A lock during
    // the fetch await clears the offers cache; without this guard the stale-session
    // fetch below would resurrect it. Reject a post-lock write.
    state.ensure_same_sensitive_session(session_id)?;
    // Cache the offer catalog so the Instance create dry-run can price a chosen
    // commercial_type without a network call. Skip caching an empty fetch so a
    // transient offer-API failure does not wipe a previously good catalog.
    // FIX 4: fetch_scaleway_offers flattens per-zone results and contributes
    // Vec::new() for any zone that fails, so a partial fetch can be non-empty yet
    // smaller than a previously-complete catalog. Only overwrite when the fresh set
    // is at least as large as the cached one, so a partial sync cannot degrade the
    // cache (dropping a zone whose price was already cached). A genuine shrink still
    // lands on the next full (all-zones) sync.
    if !scaleway_offers.is_empty() {
        let cached_len = state.scaleway_offers()?.len();
        if scaleway_offers.len() >= cached_len {
            state.replace_scaleway_offers(scaleway_offers.clone())?;
        }
    }
    let cloudflare_platform_inventory = fetch_cloudflare_platform_inventory(
        &state.http,
        cf_token.as_deref(),
        parts
            .selected_scopes
            .iter()
            .find(|scope| scope.provider == ProviderId::Cloudflare),
    )
    .await;
    let selected_scaleway_project_id = parts
        .selected_scopes
        .iter()
        .find(|scope| scope.provider == ProviderId::Scaleway)
        .map(|scope| scope.id.as_str());
    let scaleway_iam_resources = fetch_scaleway_iam_console_resources(
        &state.http,
        scw_token.as_deref(),
        selected_scaleway_project_id,
    )
    .await;
    let scaleway_extended_inventory = fetch_scaleway_extended_console_resources(
        &state.http,
        scw_token.as_deref(),
        parts
            .selected_scopes
            .iter()
            .find(|scope| scope.provider == ProviderId::Scaleway)
            .map(|scope| scope.id.as_str()),
    )
    .await;
    let cloudflare_security_count = cloudflare_platform_inventory
        .resources
        .iter()
        .filter(|resource| {
            resource.service_id == "cf-security-network" && live_console_resource(resource)
        })
        .count();
    let cloudflare_account_count = cloudflare_platform_inventory
        .resources
        .iter()
        .filter(|resource| {
            resource.service_id == "cf-account-iam" && live_console_resource(resource)
        })
        .count();
    let cloudflare_ai_count = cloudflare_platform_inventory
        .resources
        .iter()
        .filter(|resource| {
            resource.service_id == "cf-ai-observability" && live_console_resource(resource)
        })
        .count();
    let scaleway_network_count = scaleway_extended_inventory
        .resources
        .iter()
        .filter(|resource| {
            resource.service_id == "scw-network-security" && live_console_resource(resource)
        })
        .count();
    let scaleway_data_count = scaleway_extended_inventory
        .resources
        .iter()
        .filter(|resource| {
            resource.service_id == "scw-data-managed" && live_console_resource(resource)
        })
        .count();
    let provider_services = provider_service_catalog(
        &parts,
        &scaleway_offers,
        &cloudflare_platform_inventory.counts,
        cloudflare_account_count,
        cloudflare_ai_count,
        cloudflare_security_count,
        scaleway_network_count,
        scaleway_data_count,
    );
    let console_resources = provider_console_resources(
        &parts,
        &scaleway_offers,
        &cloudflare_platform_inventory,
        &scaleway_extended_inventory,
        scaleway_iam_resources,
    );
    activity.extend(parts.activity);

    activity.push(ActivityEvent {
        id: "dashboard_snapshot_refreshed".into(),
        message: "Dashboard snapshot refreshed.".into(),
        timestamp: now(),
        event_type: "sync".into(),
        source: "Devboule".into(),
    });
    append_recent_activity_without_duplicates(state, &mut activity)?;

    let kpis = vec![
        DashboardKpi {
            id: "cloudflare_workers".into(),
            label: "Cloudflare Workers".into(),
            value: parts.workers.len().to_string(),
            subtext: "read-only live inventory".into(),
            status: provider_kpi_status(
                &parts.provider_health,
                ProviderId::Cloudflare,
                !parts.workers.is_empty(),
            ),
        },
        DashboardKpi {
            id: "scaleway_compute".into(),
            label: "Scaleway Compute".into(),
            value: parts.compute.len().to_string(),
            subtext: "instances and serverless functions".into(),
            status: provider_kpi_status(
                &parts.provider_health,
                ProviderId::Scaleway,
                !parts.compute.is_empty(),
            ),
        },
        DashboardKpi {
            id: "risk_flags".into(),
            label: "Risk Flags".into(),
            value: parts.risks.len().to_string(),
            subtext: "provider and cost warnings".into(),
            status: if parts.risks.is_empty() {
                "healthy"
            } else {
                "warning"
            }
            .into(),
        },
    ];

    state.ensure_same_sensitive_session(session_id)?;
    let auth = state.auth_state()?;

    Ok(CloudDashboardSnapshot {
        auth,
        provider_health: parts.provider_health,
        selected_scopes: parts.selected_scopes,
        kpis,
        provider_services,
        console_resources,
        workers: parts.workers,
        compute: parts.compute,
        storage: parts.storage,
        scaleway_offers,
        risks: parts.risks,
        activity,
        last_sync_at: Some(now()),
    })
}

fn preserve_cached_resources_on_sync_error(
    state: &BackendState,
    mut inventory: ProviderInventory,
    requested_scope: Option<&str>,
) -> Result<ProviderInventory, String> {
    if inventory.health.status != "error" {
        return Ok(inventory);
    }

    if let Some(previous) = state
        .cached_provider_inventories()?
        .into_iter()
        .find(|cached| cached.health.id == inventory.health.id)
    {
        if !cached_scope_matches_request(previous.selected_scope.as_ref(), requested_scope) {
            return Ok(inventory);
        }
        if inventory.selected_scope.is_none() {
            inventory.selected_scope = previous.selected_scope;
        }
        inventory.workers = previous.workers;
        inventory.compute = previous.compute;
        inventory.storage = previous.storage;
        inventory.health.resource_count =
            inventory.workers.len() + inventory.compute.len() + inventory.storage.len();
    }
    Ok(inventory)
}

fn cached_scope_matches_request(
    selected_scope: Option<&ProviderScopeSelection>,
    requested_scope: Option<&str>,
) -> bool {
    match requested_scope {
        Some(scope) => selected_scope
            .map(|selected| selected.id == scope.trim())
            .unwrap_or(false),
        None => true,
    }
}

fn cloudflare_rotation_scope_guard(state: &BackendState, account_id: &str) -> Result<(), String> {
    let scope = state.cloudflare_selected_scope()?.ok_or_else(|| {
        "Cloudflare account scope is not loaded. Sync Cloudflare before rotating Worker secrets."
            .to_string()
    })?;
    if scope.id != account_id.trim() {
        return Err("Worker account does not match the current Cloudflare account scope.".into());
    }
    let name_matches_bio = scope
        .name
        .as_deref()
        .map(normalize_provider_name)
        .as_deref()
        == Some("aspis-bio");
    let explicit_pin = matches!(scope.source.as_str(), "pinned");
    if !name_matches_bio && !explicit_pin {
        return Err(
            "Cloudflare account is not proven as Aspis Bio. Save the pinned Aspis Bio account id before rotating Worker secrets."
                .into(),
        );
    }
    Ok(())
}

/// The exhaustive set of Scaleway resource actions this app is allowed to forward.
/// Anything outside this allowlist is rejected BEFORE it can reach the API — we do
/// not pass arbitrary user-supplied verbs through. `terminate` is recognised here
/// only so it can be rejected with a dedicated message that routes destruction
/// through the confirm-by-name `delete` path. `deploy` is the Serverless
/// functions/containers redeploy verb (their `available_actions` is `["deploy"]`)
/// and is NON-destructive, so it passes without confirm-by-name.
const SCW_ALLOWED_ACTIONS: &[&str] = &[
    "start",
    "stop",
    "reboot",
    "poweron",
    "poweroff",
    "deploy",
    "delete",
    "terminate",
];

fn validate_scaleway_action_request(
    resource: &ScalewayResourceSummary,
    action: &str,
    confirm_resource_name: Option<&str>,
) -> Result<String, String> {
    let action = action.trim().to_ascii_lowercase();
    // ALLOWLIST: reject any verb we do not explicitly support, rather than
    // defaulting to `Ok(action)` and forwarding it to the API.
    if !SCW_ALLOWED_ACTIONS.contains(&action.as_str()) {
        return Err("Unsupported Scaleway resource action.".into());
    }
    if action == "terminate" {
        return Err(
            "Use delete with exact resource-name confirmation for destructive Scaleway actions."
                .into(),
        );
    }
    if action == "delete" {
        let confirmed = confirm_resource_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "Deleting Scaleway resources requires exact resource-name confirmation.".to_string()
            })?;
        if confirmed != resource.name {
            return Err("Scaleway delete confirmation does not match the resource name.".into());
        }
    }
    Ok(action)
}

fn scaleway_action_inventory_guard(
    state: &BackendState,
    resource_id: &str,
    action: &str,
) -> Result<(), String> {
    let health = state.scaleway_health()?.ok_or_else(|| {
        "Scaleway inventory is not loaded. Sync Scaleway before acting.".to_string()
    })?;
    if !matches!(health.status.as_str(), "healthy" | "degraded") {
        return Err(
            "Scaleway inventory is stale or unavailable. Sync successfully before acting.".into(),
        );
    }
    let resource = state.scaleway_resource(resource_id)?.ok_or_else(|| {
        "Resource is not in the current Scaleway inventory. Sync Scaleway before acting."
            .to_string()
    })?;
    if resource.available_actions.is_empty() {
        return Err(
            "Scaleway available actions are unknown. Sync successfully before acting.".into(),
        );
    }
    let provider_action = scaleway_api_action(action);
    if !resource
        .available_actions
        .iter()
        .any(|available| available.eq_ignore_ascii_case(provider_action))
    {
        return Err("Scaleway action is not available for this resource state.".into());
    }
    Ok(())
}

/// C1: resolve the pinned/configured Scaleway project id. Prefers the vault
/// scope pin, falling back to the ASPIS_SCALEWAY_PROJECT_ID env var.
fn configured_or_pinned_scaleway_project_id() -> Result<Option<String>, String> {
    if let Some(pinned) = vault::read_scope(ProviderId::Scaleway)
        .map_err(|e| sanitize_error_message(&e))?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(pinned));
    }
    Ok(std::env::var("ASPIS_SCALEWAY_PROJECT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

/// C1: HARD-FAIL unless the resource's project id matches the pinned project.
/// If no project is pinned/configured, refuse rather than allow an unscoped
/// destructive call.
fn assert_scaleway_resource_in_pinned_project(
    resource: &ScalewayResourceSummary,
    pinned_project_id: Option<&str>,
) -> Result<(), String> {
    let pinned = pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Scaleway project scope is not pinned. Save the Aspis Bio project id before acting."
                .to_string()
        })?;
    let resource_project = resource
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Scaleway resource project is unknown. Sync Scaleway before acting.".to_string()
        })?;
    if resource_project != pinned {
        return Err(
            "Scaleway resource is outside the pinned Aspis Bio project. Refusing the action."
                .into(),
        );
    }
    Ok(())
}

/// Storage-side mirror of `validate_scaleway_action_request`: `delete` requires
/// an exact resource-name confirmation; `terminate` is refused (delete is the
/// only destructive verb on storage). Returns the normalized action.
fn validate_scaleway_storage_action_request(
    resource: &ScalewayStorageSummary,
    action: &str,
    confirm_resource_name: Option<&str>,
) -> Result<String, String> {
    let action = action.trim().to_ascii_lowercase();
    if action == "terminate" {
        return Err(
            "Use delete with exact resource-name confirmation for destructive Scaleway storage actions."
                .into(),
        );
    }
    if action == "delete" {
        let confirmed = confirm_resource_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "Deleting Scaleway storage requires exact resource-name confirmation.".to_string()
            })?;
        if confirmed != resource.name {
            return Err(
                "Scaleway storage delete confirmation does not match the resource name.".into(),
            );
        }
    }
    Ok(action)
}

/// Storage-side mirror of `assert_scaleway_resource_in_pinned_project`: HARD-FAIL
/// unless the storage resource's project id matches the pinned project. Object
/// buckets carry their project id in inventory just like Block/File resources.
fn assert_scaleway_storage_in_pinned_project(
    resource: &ScalewayStorageSummary,
    pinned_project_id: Option<&str>,
) -> Result<(), String> {
    let pinned = pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Scaleway project scope is not pinned. Save the Aspis Bio project id before acting."
                .to_string()
        })?;
    let resource_project = resource
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Scaleway storage project is unknown. Sync Scaleway before acting.".to_string()
        })?;
    if resource_project != pinned {
        return Err(
            "Scaleway storage is outside the pinned Aspis Bio project. Refusing the action.".into(),
        );
    }
    Ok(())
}

/// CREATE guard: the target project id of a new resource MUST equal the pinned
/// project. Refuses an empty target or a missing pin rather than creating an
/// unscoped resource.
fn assert_scaleway_create_project_is_pinned(
    target_project_id: &str,
    pinned_project_id: Option<&str>,
) -> Result<(), String> {
    let target = target_project_id.trim();
    if target.is_empty() {
        return Err("Scaleway create requires a target project id.".into());
    }
    let pinned = pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Scaleway project scope is not pinned. Save the Aspis Bio project id before creating."
                .to_string()
        })?;
    if target != pinned {
        return Err(
            "Scaleway create target project does not match the pinned Aspis Bio project. Refusing."
                .into(),
        );
    }
    Ok(())
}

/// Shared storage inventory/health gate (mirrors `scaleway_action_inventory_guard`
/// but storage summaries do not carry per-resource available_actions). Confirms
/// the inventory is loaded and healthy/degraded, and the resource is present.
fn scaleway_storage_inventory_guard(
    state: &BackendState,
    resource_id: &str,
) -> Result<ScalewayStorageSummary, String> {
    let health = state.scaleway_health()?.ok_or_else(|| {
        "Scaleway inventory is not loaded. Sync Scaleway before acting.".to_string()
    })?;
    if !matches!(health.status.as_str(), "healthy" | "degraded") {
        return Err(
            "Scaleway inventory is stale or unavailable. Sync successfully before acting.".into(),
        );
    }
    state.scaleway_storage_resource(resource_id)?.ok_or_else(|| {
        "Storage resource is not in the current Scaleway inventory. Sync Scaleway before acting."
            .to_string()
    })
}

fn scaleway_api_action(action: &str) -> &str {
    match action {
        "start" => "poweron",
        "stop" => "poweroff",
        "delete" => "terminate",
        other => other,
    }
}

fn normalize_provider_name(name: &str) -> String {
    name.trim()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase()
}

fn provider_kpi_status(
    health: &[ProviderHealth],
    provider: ProviderId,
    has_resources: bool,
) -> String {
    match health
        .iter()
        .find(|entry| entry.id == provider)
        .map(|entry| entry.status.as_str())
    {
        Some("healthy") if has_resources => "healthy",
        Some("healthy") => "unknown",
        Some("degraded") => "warning",
        Some("missing_token" | "error" | "down") => "error",
        Some(_) => "unknown",
        None => "unknown",
    }
    .into()
}

fn provider_service_catalog(
    parts: &SnapshotInventoryParts,
    scaleway_offers: &[ScalewayOfferSummary],
    cloudflare_platform_counts: &CloudflarePlatformCounts,
    cloudflare_account_count: usize,
    cloudflare_ai_count: usize,
    cloudflare_security_count: usize,
    scaleway_network_count: usize,
    scaleway_data_count: usize,
) -> Vec<ProviderServiceSummary> {
    let cloudflare_health = provider_health_status(parts, ProviderId::Cloudflare);
    let scaleway_health = provider_health_status(parts, ProviderId::Scaleway);
    let cloudflare_scope_count = parts
        .selected_scopes
        .iter()
        .filter(|scope| scope.provider == ProviderId::Cloudflare)
        .count();
    let scaleway_scope_count = parts
        .selected_scopes
        .iter()
        .filter(|scope| scope.provider == ProviderId::Scaleway)
        .count();
    let scaleway_serverless_count = parts
        .compute
        .iter()
        .filter(|resource| resource.resource_type == "Serverless")
        .count();
    let gpu_offer_count = scaleway_offers
        .iter()
        .filter(|offer| offer.category == "GPU")
        .count();
    let cpu_offer_count = scaleway_offers
        .iter()
        .filter(|offer| offer.category == "CPU VM")
        .count();
    let cloudflare_data_count = cloudflare_platform_counts.total();
    let cloudflare_data_note = format!(
        "R2 {}, D1 {}, KV {}, Queues {}, Vectorize {}, Durable Objects {}",
        cloudflare_platform_counts.r2_buckets,
        cloudflare_platform_counts.d1_databases,
        cloudflare_platform_counts.kv_namespaces,
        cloudflare_platform_counts.queues,
        cloudflare_platform_counts.vectorize_indexes,
        cloudflare_platform_counts.durable_object_namespaces
    );
    let cloudflare_account_live_count = cloudflare_scope_count + cloudflare_account_count;

    vec![
        service_summary(
            "cf-account-iam",
            ProviderId::Cloudflare,
            "Account",
            "Account, IAM, audit logs",
            "Accounts, roles, API tokens, account audit logs and permission posture.",
            partial_status(cloudflare_health, cloudflare_account_live_count),
            "scope verified; audit logs live when token allows",
            cloudflare_account_live_count,
            "Account Read, User API Tokens Read, Account Settings Read for deeper views.",
            "https://developers.cloudflare.com/api/resources/accounts/",
            &["scope pin", "token verify", "audit logs", "future members/roles"],
            &[
                "Aspis Bio account only",
                "Pinned accounts may warn when the display name is personal",
            ],
        ),
        service_summary(
            "cf-workers-pages",
            ProviderId::Cloudflare,
            "Developer platform",
            "Workers, Pages, routes, secrets",
            "Worker scripts, routes, deployments, settings, secrets, Pages projects and builds.",
            partial_status(cloudflare_health, parts.workers.len()),
            "Workers live; Pages/builds/routes/settings next",
            parts.workers.len(),
            "Workers Scripts Read; Workers Scripts Write only for secret rotation.",
            "https://developers.cloudflare.com/api/resources/workers/",
            &["list workers", "deployments", "rotate worker secret", "future Pages"],
            &["Secret values are never read back", "Destructive deploy/delete actions remain disabled"],
        ),
        service_summary(
            "cf-storage-data",
            ProviderId::Cloudflare,
            "Storage & data",
            "R2, D1, KV, Queues, Vectorize",
            "Object buckets, SQL databases, KV namespaces, Queues, Vectorize, Hyperdrive and data catalog surfaces.",
            partial_or_roadmap_status(cloudflare_health, cloudflare_data_count),
            "counts live when token includes product read permissions",
            cloudflare_data_count,
            "R2 Read, D1 Read, Workers KV Storage Read, Queues Read, Vectorize Read.",
            "https://developers.cloudflare.com/api/resources/r2/",
            &["R2 buckets", "D1 databases", "KV namespaces", "Queues", "Vectorize"],
            &[
                cloudflare_data_note.as_str(),
                "R2 object listing may need separate bucket-level policy",
                "Write actions should stay behind explicit confirmers",
            ],
        ),
        service_summary(
            "cf-security-network",
            ProviderId::Cloudflare,
            "Security & network",
            "Zones, DNS, WAF, Access, Tunnel",
            "Zone settings, DNS records, WAF/rulesets, Zero Trust Access, tunnels, certificates and analytics.",
            partial_or_roadmap_status(cloudflare_health, cloudflare_security_count),
            "zones, DNS, rulesets, Access apps and tunnels live when token allows",
            cloudflare_security_count,
            "Zone Read, DNS Read, WAF Read, Access Read and Tunnel Read depending on enabled modules.",
            "https://developers.cloudflare.com/api/",
            &["zones", "DNS", "WAF/rulesets", "Access", "Tunnel"],
            &["Many enterprise features will show as unavailable with narrow tokens"],
        ),
        service_summary(
            "cf-ai-observability",
            ProviderId::Cloudflare,
            "AI & observability",
            "Workers AI, AI Gateway, logs",
            "AI usage, AI Gateway, analytics, Logpush/log explorer, billing and product usage controls.",
            partial_or_roadmap_status(cloudflare_health, cloudflare_ai_count),
            "AI Gateway and Logpush live when token allows; analytics/billing next",
            cloudflare_ai_count,
            "AI Gateway Read, Workers AI Read, Account Analytics Read, Logs Read, Billing Read.",
            "https://developers.cloudflare.com/api/resources/ai_gateway/",
            &["AI Gateway", "Logpush", "Workers AI", "analytics", "billing"],
            &["Usage/cost APIs can lag; budget alerts need conservative wording"],
        ),
        service_summary(
            "scw-iam-projects",
            ProviderId::Scaleway,
            "Identity",
            "Projects, IAM policies, keys",
            "Organization projects, IAM policies, groups, applications, users and API key posture.",
            partial_status(scaleway_health, scaleway_scope_count),
            "project pin live; IAM policy explorer next",
            scaleway_scope_count,
            "IAM read permissions for policies, applications, groups and API keys.",
            "https://www.scaleway.com/en/developers/api/iam",
            &["Aspis Bio project pin", "future policies", "future API keys"],
            &["Default project is intentionally excluded unless it is Aspis Bio"],
        ),
        service_summary(
            "scw-compute-live",
            ProviderId::Scaleway,
            "Compute",
            "CPU/GPU Instances",
            "Live Instances plus guarded start, stop, reboot and delete operations.",
            partial_status(scaleway_health, parts.compute.len()),
            "live inventory and guarded actions",
            parts
                .compute
                .iter()
                .filter(|resource| resource.resource_type == "GPU" || resource.resource_type == "CPU VM")
                .count(),
            "Instances Read; Instances Write only for operational actions.",
            "https://www.scaleway.com/en/developers/api/instance",
            &["list Instances", "available actions", "guarded delete", "future create wizard"],
            &["Backend blocks stale inventory and public terminate alias"],
        ),
        service_summary(
            "scw-spawnable-offers",
            ProviderId::Scaleway,
            "Compute catalog",
            "Spawnable CPU/GPU types",
            "Public Scaleway Instance product catalog with per-zone availability, vCPU, RAM, GPU count and public prices.",
            if scaleway_offers.is_empty() { "unknown" } else { "live" },
            "public product API",
            scaleway_offers.len(),
            "No secret needed for product catalog; create actions will require Project-scoped write permission.",
            "https://www.scaleway.com/en/developers/api/instance",
            &["CPU offers", "GPU offers", "availability", "prices"],
            &[&format!("{cpu_offer_count} CPU offers, {gpu_offer_count} GPU offers")],
        ),
        service_summary(
            "scw-serverless",
            ProviderId::Scaleway,
            "Serverless",
            "Functions, Containers, Jobs",
            "Serverless functions, containers, jobs, namespaces, runtimes, scaling and deploy state.",
            partial_status(scaleway_health, scaleway_serverless_count),
            "functions/containers live; jobs and create flows next",
            scaleway_serverless_count,
            "Functions Read, Containers Read, Jobs Read; write only for deploy/redeploy.",
            "https://www.scaleway.com/en/docs/serverless-containers",
            &["functions", "containers", "deploy", "future jobs", "future runtimes"],
            &["Deploy is supported only when Scaleway reports the action available"],
        ),
        service_summary(
            "scw-storage",
            ProviderId::Scaleway,
            "Storage",
            "Object, Block, snapshots",
            "Block volumes, snapshots, Object Storage buckets and bounded usage estimates.",
            partial_status(scaleway_health, parts.storage.len()),
            "Block/snapshot live; Object requires access key",
            parts.storage.len(),
            "Block Storage Read and Object Storage IAM/S3 credentials for bucket inventory.",
            "https://www.scaleway.com/en/docs/object-storage",
            &["volumes", "snapshots", "object buckets", "usage estimates"],
            &["Object bucket scan is bounded to avoid expensive/deep listing by default"],
        ),
        service_summary(
            "scw-network-security",
            ProviderId::Scaleway,
            "Network & security",
            "VPC, gateways, IPs, security groups",
            "Private Networks, public gateways, flexible IPs, load balancers, security groups, KMS and audit trail.",
            partial_or_roadmap_status(scaleway_health, scaleway_network_count),
            "private networks, gateways and load balancers live when token allows",
            scaleway_network_count,
            "VPC, Instance networking, Load Balancer, KMS and Audit Trail read permissions.",
            "https://www.scaleway.com/en/docs/",
            &["private networks", "public gateways", "flexible IPs", "security groups"],
            &["Network write actions should be separate guarded flows"],
        ),
        service_summary(
            "scw-data-managed",
            ProviderId::Scaleway,
            "Managed data",
            "Databases, registry, queues",
            "Managed databases, Serverless SQL, container registry, messaging and queuing, data orchestrator and observability.",
            partial_or_roadmap_status(scaleway_health, scaleway_data_count),
            "managed DB, registry and Kubernetes live when token allows",
            scaleway_data_count,
            "Product-specific read permissions for DB, registry, queues and observability.",
            "https://www.scaleway.com/en/docs/",
            &["managed DB", "serverless SQL", "registry", "queues", "observability"],
            &["Some products are regional or beta and may not be available in every region"],
        ),
    ]
}

fn service_summary(
    id: &str,
    provider: ProviderId,
    category: &str,
    name: &str,
    description: &str,
    status: &str,
    coverage: &str,
    live_count: usize,
    permission: &str,
    docs_url: &str,
    actions: &[&str],
    notes: &[&str],
) -> ProviderServiceSummary {
    ProviderServiceSummary {
        id: id.into(),
        provider,
        category: category.into(),
        name: name.into(),
        description: description.into(),
        status: status.into(),
        coverage: coverage.into(),
        live_count,
        permission: permission.into(),
        docs_url: docs_url.into(),
        actions: actions.iter().map(|item| (*item).into()).collect(),
        notes: notes.iter().map(|item| (*item).into()).collect(),
    }
}

fn provider_health_status(parts: &SnapshotInventoryParts, provider: ProviderId) -> &str {
    parts
        .provider_health
        .iter()
        .find(|health| health.id == provider)
        .map(|health| health.status.as_str())
        .unwrap_or("unknown")
}

fn partial_status(provider_status: &str, live_count: usize) -> &'static str {
    match provider_status {
        "missing_token" | "error" | "down" => "blocked",
        "healthy" | "degraded" if live_count > 0 => "partial",
        "healthy" | "degraded" => "ready",
        _ => "unknown",
    }
}

fn roadmap_status(provider_status: &str) -> &'static str {
    match provider_status {
        "missing_token" | "error" | "down" => "blocked",
        "healthy" | "degraded" => "roadmap",
        _ => "unknown",
    }
}

fn partial_or_roadmap_status(provider_status: &str, live_count: usize) -> &'static str {
    if live_count > 0 {
        return partial_status(provider_status, live_count);
    }
    roadmap_status(provider_status)
}

fn live_console_resource(resource: &ProviderConsoleResourceSummary) -> bool {
    !matches!(
        resource.status.as_str(),
        "forbidden" | "unavailable" | "missing_token"
    )
}

fn provider_console_resources(
    parts: &SnapshotInventoryParts,
    scaleway_offers: &[ScalewayOfferSummary],
    cloudflare_platform_inventory: &CloudflarePlatformInventory,
    scaleway_extended_inventory: &ScalewayExtendedInventory,
    scaleway_iam_resources: Vec<ProviderConsoleResourceSummary>,
) -> Vec<ProviderConsoleResourceSummary> {
    let mut resources = Vec::new();
    resources.extend(parts.workers.iter().map(cloudflare_worker_console_resource));
    resources.extend(cloudflare_platform_inventory.resources.clone());
    resources.extend(parts.compute.iter().map(scaleway_compute_console_resource));
    resources.extend(parts.storage.iter().map(scaleway_storage_console_resource));
    resources.extend(
        scaleway_offers
            .iter()
            .take(80)
            .map(scaleway_offer_console_resource),
    );
    resources.extend(scaleway_extended_inventory.resources.clone());
    resources.extend(scaleway_iam_resources);
    resources.sort_by(|a, b| {
        a.provider
            .as_str()
            .cmp(b.provider.as_str())
            .then_with(|| a.service_id.cmp(&b.service_id))
            .then_with(|| a.resource_type.cmp(&b.resource_type))
            .then_with(|| a.name.cmp(&b.name))
    });
    resources
}

fn cloudflare_worker_console_resource(
    worker: &CloudflareWorkerSummary,
) -> ProviderConsoleResourceSummary {
    let mut metadata = Vec::new();
    metadata.push(format!(
        "account: {}",
        worker.account_name.as_deref().unwrap_or(&worker.account_id)
    ));
    if let Some(compatibility_date) = &worker.compatibility_date {
        metadata.push(format!("compatibility: {compatibility_date}"));
    }
    if !worker.routes.is_empty() {
        metadata.push(format!("routes: {}", worker.routes.len()));
    }

    ProviderConsoleResourceSummary {
        id: format!("cloudflare:cf-workers-pages:worker:{}", worker.id),
        provider: ProviderId::Cloudflare,
        service_id: "cf-workers-pages".into(),
        resource_type: "Worker".into(),
        name: worker.name.clone(),
        region: None,
        status: worker.status.clone(),
        description: worker.purpose.clone(),
        metadata,
        docs_url: "https://developers.cloudflare.com/api/resources/workers/".into(),
        updated_at: worker.last_deploy.clone(),
    }
}

fn scaleway_compute_console_resource(
    resource: &ScalewayResourceSummary,
) -> ProviderConsoleResourceSummary {
    let mut metadata = Vec::new();
    if let Some(commercial_type) = &resource.commercial_type {
        metadata.push(format!("type: {commercial_type}"));
    }
    if let Some(public_ip) = &resource.public_ip {
        metadata.push(format!("public ip: {public_ip}"));
    }
    if !resource.available_actions.is_empty() {
        metadata.push(format!(
            "actions: {}",
            resource.available_actions.join(", ")
        ));
    }

    ProviderConsoleResourceSummary {
        id: format!(
            "scaleway:scw-compute-live:{}:{}",
            resource.resource_type, resource.id
        ),
        provider: ProviderId::Scaleway,
        service_id: match resource.resource_type.as_str() {
            // Serverless SQL is managed data, not generic serverless compute, and must
            // feed the "Managed data" card (scw-data-managed). Check it before the
            // "Serverless" prefix would otherwise also match a `starts_with`.
            "Serverless SQL" => "scw-data-managed",
            "Serverless" => "scw-serverless",
            _ => "scw-compute-live",
        }
        .into(),
        resource_type: resource.resource_type.clone(),
        name: resource.name.clone(),
        region: Some(resource.region.clone()),
        status: resource.state.clone(),
        description: resource.purpose.clone(),
        metadata,
        docs_url: "https://www.scaleway.com/en/developers/api/instance".into(),
        updated_at: resource
            .updated_at
            .clone()
            .or_else(|| resource.created_at.clone()),
    }
}

fn scaleway_storage_console_resource(
    storage: &ScalewayStorageSummary,
) -> ProviderConsoleResourceSummary {
    let mut metadata = vec![
        format!("size: {:.2} GB", storage.size_gb),
        storage.pricing_label.clone(),
    ];
    if let Some(estimated) = storage.estimated_eur_month {
        metadata.push(format!("est: EUR {estimated:.2}/mo"));
    }

    ProviderConsoleResourceSummary {
        id: format!(
            "scaleway:scw-storage:{}:{}",
            storage.storage_type, storage.id
        ),
        provider: ProviderId::Scaleway,
        service_id: "scw-storage".into(),
        resource_type: storage.storage_type.clone(),
        name: storage.name.clone(),
        region: Some(storage.region.clone()),
        status: storage.state.clone(),
        description: storage.pricing_note.clone(),
        metadata,
        docs_url: "https://www.scaleway.com/en/docs/object-storage".into(),
        updated_at: storage
            .updated_at
            .clone()
            .or_else(|| storage.created_at.clone()),
    }
}

fn scaleway_offer_console_resource(offer: &ScalewayOfferSummary) -> ProviderConsoleResourceSummary {
    let mut metadata = vec![
        format!("{} vCPU", offer.vcpus),
        format!("{:.1} GB RAM", offer.memory_gb),
    ];
    if let Some(gpu_label) = &offer.gpu_label {
        metadata.push(gpu_label.clone());
    }
    if let Some(hourly) = offer.hourly_price_eur {
        metadata.push(format!("EUR {hourly:.4}/h"));
    }

    ProviderConsoleResourceSummary {
        id: format!("scaleway:scw-spawnable-offers:{}", offer.id),
        provider: ProviderId::Scaleway,
        service_id: "scw-spawnable-offers".into(),
        resource_type: format!("{} Offer", offer.category),
        name: offer.name.clone(),
        region: Some(offer.zone.clone()),
        status: offer.availability.clone(),
        description: "Scaleway public Instance product offer.".into(),
        metadata,
        docs_url: "https://www.scaleway.com/en/developers/api/instance".into(),
        updated_at: None,
    }
}

struct SnapshotInventoryParts {
    provider_health: Vec<ProviderHealth>,
    selected_scopes: Vec<ProviderScopeSelection>,
    workers: Vec<CloudflareWorkerSummary>,
    compute: Vec<ScalewayResourceSummary>,
    storage: Vec<ScalewayStorageSummary>,
    risks: Vec<RiskFlag>,
    activity: Vec<ActivityEvent>,
}

fn snapshot_parts_from_provider_inventories(
    inventories: Vec<ProviderInventory>,
) -> SnapshotInventoryParts {
    let mut provider_health = Vec::new();
    let mut selected_scopes = Vec::new();
    let mut workers = Vec::new();
    let mut compute = Vec::new();
    let mut storage = Vec::new();
    let mut risks = Vec::new();
    let mut activity = Vec::new();

    for inventory in inventories {
        provider_health.push(inventory.health);
        if let Some(selected_scope) = inventory.selected_scope {
            selected_scopes.push(selected_scope);
        }
        workers.extend(inventory.workers);
        compute.extend(inventory.compute);
        storage.extend(inventory.storage);
        risks.extend(inventory.risks);
        activity.extend(inventory.activity);
    }

    SnapshotInventoryParts {
        provider_health,
        selected_scopes,
        workers,
        compute,
        storage,
        risks,
        activity,
    }
}

fn append_recent_activity_without_duplicates(
    state: &BackendState,
    activity: &mut Vec<ActivityEvent>,
) -> Result<(), String> {
    let mut seen = activity
        .iter()
        .map(|event| event.id.clone())
        .collect::<HashSet<_>>();
    for event in state.recent_activity()? {
        if seen.insert(event.id.clone()) {
            activity.push(event);
        }
    }
    Ok(())
}

async fn validate_provider_token_with_scope(
    state: &BackendState,
    provider: ProviderId,
    token: &str,
    scope: Option<String>,
) -> Result<ProviderInventory, String> {
    let inventory = match provider {
        ProviderId::Cloudflare => {
            fetch_cloudflare(&state.http, Some(token.to_string()), scope).await
        }
        ProviderId::Scaleway => {
            let access_key =
                vault::read_scaleway_object_access_key().map_err(|e| sanitize_error_message(&e))?;
            let secret_key =
                vault::read_scaleway_object_secret_key().map_err(|e| sanitize_error_message(&e))?;
            fetch_scaleway(
                &state.http,
                Some(token.to_string()),
                scope,
                access_key,
                secret_key,
            )
            .await
        }
    };
    provider_token_validation_result(provider, &inventory)?;
    if !provider_token_health_is_storable(provider, &inventory.health.token_health) {
        return Err(format!(
            "{} token validation failed: token health is {}.",
            provider.label(),
            inventory.health.token_health
        ));
    }
    Ok(inventory)
}

fn provider_token_health_is_storable(provider: ProviderId, token_health: &str) -> bool {
    match provider {
        ProviderId::Cloudflare => {
            matches!(
                token_health,
                "valid" | "valid_read_only" | "valid_unverified"
            )
        }
        ProviderId::Scaleway => token_health == "valid",
    }
}

/// Defense-in-depth: a Cloudflare account id is exactly 32 ASCII hex chars (the
/// same rule `validate_provider_scope_value` enforces on write). The vault path
/// is already validated, but the in-memory `cloudflare_selected_scope()` fallback
/// is not — so every CF command re-validates the *resolved* id with this before
/// interpolating it into an API URL.
fn cloudflare_account_id_is_valid(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_provider_scope_value(provider: ProviderId, value: &str) -> Result<String, String> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Err("Provider scope id cannot be empty.".into());
    }
    if provider == ProviderId::Cloudflare {
        if cleaned.len() != 32 || !cleaned.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err("Cloudflare scope id must be a 32-character account id.".into());
        }
        return Ok(cleaned.into());
    }
    if cleaned.len() > 128
        || !cleaned
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!("{} scope id is invalid.", provider.label()));
    }
    Ok(cleaned.into())
}

fn provider_connection_scope(
    provider: ProviderId,
    pinned_id: Option<String>,
) -> Result<Option<String>, String> {
    match pinned_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => Ok(Some(validate_provider_scope_value(provider, value)?)),
        None => vault::read_scope(provider).map_err(|e| sanitize_error_message(&e)),
    }
}

fn provider_connection_audit(
    provider: ProviderId,
    inventory: &ProviderInventory,
) -> ProviderConnectionAudit {
    ProviderConnectionAudit {
        provider,
        status: inventory.health.status.clone(),
        token_health: inventory.health.token_health.clone(),
        selected_scope: inventory.selected_scope.clone(),
        resource_count: inventory.health.resource_count,
        message: inventory.health.message.clone(),
        risks: inventory
            .risks
            .iter()
            .map(|risk| format!("{}: {}", risk.title, risk.description))
            .collect(),
    }
}

fn missing_saved_provider_connection_audit(provider: ProviderId) -> ProviderConnectionAudit {
    ProviderConnectionAudit {
        provider,
        status: "error".into(),
        token_health: "missing".into(),
        selected_scope: None,
        resource_count: 0,
        message: Some(format!(
            "Stored {} token is not configured.",
            provider.label()
        )),
        risks: Vec::new(),
    }
}

fn cache_validated_provider_inventory(
    state: &BackendState,
    inventory: ProviderInventory,
) -> Result<(), String> {
    if inventory.health.id == ProviderId::Scaleway
        && matches!(inventory.health.status.as_str(), "healthy" | "degraded")
    {
        state.replace_scaleway_compute(inventory.compute.clone())?;
    }
    state.replace_provider_inventory(inventory)
}

fn provider_token_validation_result(
    provider: ProviderId,
    inventory: &ProviderInventory,
) -> Result<(), String> {
    match inventory.health.status.as_str() {
        "healthy" | "degraded" => Ok(()),
        _ => Err(sanitize_error_message(
            inventory
                .health
                .message
                .as_deref()
                .unwrap_or_else(|| provider_token_validation_fallback(provider)),
        )),
    }
}

fn provider_token_validation_fallback(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Cloudflare => "Cloudflare token validation failed.",
        ProviderId::Scaleway => "Scaleway token validation failed.",
    }
}

fn secret_error(provider: ProviderId, message: &str) -> SecretStatus {
    SecretStatus {
        provider,
        configured: false,
        status: "error".into(),
        last_checked_at: Some(now()),
        message: Some(sanitize_error_message(message)),
    }
}

fn scaleway_lifecycle_events(
    previous: &[ScalewayResourceSummary],
    current: &[ScalewayResourceSummary],
    timestamp: &str,
) -> Vec<ActivityEvent> {
    let previous_by_id = previous
        .iter()
        .map(|resource| (resource.id.as_str(), resource))
        .collect::<HashMap<_, _>>();
    let current_by_id = current
        .iter()
        .map(|resource| (resource.id.as_str(), resource))
        .collect::<HashMap<_, _>>();

    let mut events = Vec::new();
    for resource in current {
        match previous_by_id.get(resource.id.as_str()) {
            None => events.push(ActivityEvent {
                id: format!("scw_spawn_{}_{}", resource.id, resource.state),
                message: format!(
                    "{} appeared as {} {} in {}.",
                    resource.name, resource.state, resource.resource_type, resource.region
                ),
                timestamp: timestamp.into(),
                event_type: "spawn".into(),
                source: "Scaleway".into(),
            }),
            Some(previous) if previous.state != resource.state => events.push(ActivityEvent {
                id: format!(
                    "scw_state_{}_{}_{}",
                    resource.id, previous.state, resource.state
                ),
                message: format!(
                    "{} changed state {} -> {}.",
                    resource.name, previous.state, resource.state
                ),
                timestamp: timestamp.into(),
                event_type: "scale".into(),
                source: "Scaleway".into(),
            }),
            _ => {}
        }
    }

    for resource in previous {
        if !current_by_id.contains_key(resource.id.as_str()) {
            events.push(ActivityEvent {
                id: format!("scw_removed_{}", resource.id),
                message: format!("{} is no longer reported by Scaleway.", resource.name),
                timestamp: timestamp.into(),
                event_type: "scale".into(),
                source: "Scaleway".into(),
            });
        }
    }
    events
}

#[derive(Serialize)]
struct CloudflareSecretUpdateBody<'a> {
    name: &'a str,
    text: &'a str,
    #[serde(rename = "type")]
    binding_type: &'static str,
}

#[derive(Debug)]
struct CloudflareSecretRotationRequest {
    account_id: String,
    worker_name: String,
    secret_name: String,
    secret_value: String,
}

async fn put_cloudflare_worker_secret(
    state: &BackendState,
    token: &str,
    account_id: &str,
    worker_name: &str,
    secret_name: &str,
    secret_value: &str,
) -> Result<(), String> {
    let worker_name = urlencoding::encode(worker_name);
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/workers/scripts/{worker_name}/secrets"
    );
    let body = CloudflareSecretUpdateBody {
        name: secret_name,
        text: secret_value,
        binding_type: "secret_text",
    };
    state
        .http
        .put(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("Cloudflare secret update failed: {e}")))?
        .error_for_status()
        .map_err(|e| sanitize_error_message(&format!("Cloudflare secret update rejected: {e}")))?;
    Ok(())
}

fn validate_cloudflare_secret_rotation_request(
    account_id: &str,
    worker_name: &str,
    secret_name: &str,
    secret_value: &str,
) -> Result<CloudflareSecretRotationRequest, String> {
    let account_id = account_id.trim();
    let worker_name = worker_name.trim();
    let secret_name = secret_name.trim();
    let secret_value = secret_value.trim();

    if account_id.len() != 32 || !account_id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("Cloudflare account id must be a 32-character identifier.".into());
    }
    if worker_name.is_empty() || worker_name.contains('/') || worker_name.contains('\\') {
        return Err("Worker name is invalid.".into());
    }
    if !is_valid_js_identifier(secret_name) {
        return Err("Secret binding name must be a valid JavaScript identifier.".into());
    }
    if secret_value.len() < 8 {
        return Err("Secret value is too short to rotate.".into());
    }
    Ok(CloudflareSecretRotationRequest {
        account_id: account_id.into(),
        worker_name: worker_name.into(),
        secret_name: secret_name.into(),
        secret_value: secret_value.into(),
    })
}

fn is_valid_js_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn secret_rotation_result(
    account_id: &str,
    worker_name: &str,
    secret_name: &str,
) -> SecretRotationResult {
    SecretRotationResult {
        provider: ProviderId::Cloudflare,
        account_id: account_id.trim().into(),
        worker_name: worker_name.trim().into(),
        secret_name: secret_name.trim().into(),
        rotated_at: now(),
        message: "Cloudflare Worker secret rotated. Secret value was not stored or returned."
            .into(),
    }
}

fn secret_rotation_activity_event(result: &SecretRotationResult) -> ActivityEvent {
    ActivityEvent {
        id: format!(
            "cf_secret_rotated_{}_{}",
            result.worker_name, result.secret_name
        ),
        message: format!(
            "Rotated Cloudflare Worker secret {} for {}.",
            result.secret_name, result.worker_name
        ),
        timestamp: result.rotated_at.clone(),
        event_type: "secret".into(),
        source: "Cloudflare".into(),
    }
}

fn cloudflare_smoke_dry_run_result(inventory: &ProviderInventory) -> CloudflareSmokeDryRunResult {
    // valid_unverified is allowed to attempt rotation: Cloudflare did not expose
    // policy details, so write cannot be proven up front, but the actual PUT will
    // reject loudly if the token is under-scoped. Only valid_read_only / missing
    // tokens are blocked before the call.
    let can_rotate_worker_secret = matches!(
        inventory.health.token_health.as_str(),
        "valid" | "valid_unverified"
    ) && inventory.selected_scope.is_some();
    let blocked_reason = if can_rotate_worker_secret {
        None
    } else if inventory.health.token_health == "valid_read_only" {
        Some("Token is read-only; Workers Scripts Write is required for secret rotation.".into())
    } else if inventory.selected_scope.is_none() {
        Some("Cloudflare account scope is not resolved.".into())
    } else {
        inventory.health.message.clone()
    };
    let account_api = inventory
        .selected_scope
        .as_ref()
        .map(|scope| format!("GET /accounts/{}/workers/scripts", scope.id))
        .unwrap_or_else(|| "GET /accounts/{account_id}/workers/scripts".into());
    let deploy_api = inventory
        .selected_scope
        .as_ref()
        .map(|scope| {
            format!(
                "GET /accounts/{}/workers/scripts/{{worker_name}}/deployments",
                scope.id
            )
        })
        .unwrap_or_else(|| {
            "GET /accounts/{account_id}/workers/scripts/{worker_name}/deployments".into()
        });
    let secret_api = inventory
        .selected_scope
        .as_ref()
        .map(|scope| {
            format!(
                "PUT /accounts/{}/workers/scripts/{{worker_name}}/secrets/{{secret_name}} (not executed)",
                scope.id
            )
        })
        .unwrap_or_else(|| {
            "PUT /accounts/{account_id}/workers/scripts/{worker_name}/secrets/{secret_name} (not executed)".into()
        });
    let message = if matches!(inventory.health.status.as_str(), "healthy" | "degraded") {
        format!(
            "Dry run read {} Aspis Bio Worker(s). Secret rotation {}.",
            inventory.workers.len(),
            if can_rotate_worker_secret {
                "would be allowed after explicit confirmation"
            } else {
                "is blocked"
            }
        )
    } else {
        inventory
            .health
            .message
            .clone()
            .unwrap_or_else(|| "Cloudflare dry run could not read live inventory.".into())
    };

    CloudflareSmokeDryRunResult {
        provider: ProviderId::Cloudflare,
        status: inventory.health.status.clone(),
        action: "cloudflare_inventory_and_secret_rotation_guard".into(),
        dry_run: true,
        api_equivalent: vec!["GET /accounts".into(), account_api, deploy_api, secret_api],
        selected_scope: inventory.selected_scope.clone(),
        credential_kind: inventory.health.credential_kind.clone(),
        token_health: inventory.health.token_health.clone(),
        resource_count: inventory.workers.len(),
        can_rotate_worker_secret,
        blocked_reason,
        message,
        risks: inventory
            .risks
            .iter()
            .map(|risk| format!("{}: {}", risk.title, risk.description))
            .collect(),
        audited_at: now(),
    }
}

fn cloudflare_smoke_dry_run_activity_event(result: &CloudflareSmokeDryRunResult) -> ActivityEvent {
    ActivityEvent {
        id: format!(
            "cloudflare_smoke_dry_run_{}",
            result.audited_at.replace([':', '.', '+', '-'], "_")
        ),
        message: result.message.clone(),
        timestamp: result.audited_at.clone(),
        event_type: "dry_run".into(),
        source: "Cloudflare".into(),
    }
}

fn scaleway_action_result(
    resource: &ScalewayResourceSummary,
    action: &str,
) -> ScalewayActionResult {
    let triggered_at = now();
    ScalewayActionResult {
        provider: ProviderId::Scaleway,
        resource_id: resource.id.clone(),
        resource_name: resource.name.clone(),
        resource_type: resource.resource_type.clone(),
        action: action.into(),
        triggered_at: triggered_at.clone(),
        message: format!(
            "Scaleway {} requested for {}. Sync to confirm the resulting state.",
            if action == "delete" {
                "delete/terminate"
            } else {
                action
            },
            resource.name
        ),
    }
}

fn scaleway_action_activity_event(result: &ScalewayActionResult) -> ActivityEvent {
    ActivityEvent {
        id: format!(
            "scw_action_{}_{}_{}",
            result.action,
            result.resource_id,
            result.triggered_at.replace([':', '.', '+', '-'], "_")
        ),
        message: result.message.clone(),
        timestamp: result.triggered_at.clone(),
        event_type: "action".into(),
        source: "Scaleway".into(),
    }
}
