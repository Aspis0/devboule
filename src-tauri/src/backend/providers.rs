use super::model::{
    ActivityEvent, CloudflareAiGatewaySettings, CloudflareAiGatewaySettingsPatch,
    CloudflareAutoragReindexResult, CloudflareBilling, CloudflareBillingPlan,
    CloudflareD1QueryResult, CloudflareEnvBindingChange, CloudflareEnvDryRunResult,
    CloudflareInvoiceSummary, CloudflareKvKey, CloudflareKvKeysPage, CloudflareKvValue,
    CloudflareR2Config, CloudflareWorkerBinding, CloudflareWorkerSettings, CloudflareWorkerSummary,
    ProviderConsoleResourceSummary, ProviderHealth, ProviderId, ProviderScopeSelection, RiskFlag,
    ScalewayBilling, ScalewayConsumptionLine, ScalewayInvoiceLine, ScalewayOfferSummary,
    ScalewayResourceSummary, ScalewayStorageSummary,
};
use chrono::Utc;
use futures_util::future::join_all;
use hmac::{Hmac, Mac};
use quick_xml::de::from_str;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const CF_API: &str = "https://api.cloudflare.com/client/v4";
const CF_DEPLOYMENT_CONCURRENCY: usize = 8;
const CF_DEPLOYMENT_TIMEOUT_SECS: u64 = 4;
/// A worker env-var write re-reads `/settings` and then PATCHes it back. A slow
/// mutation must not false-timeout into an ambiguous state (we would not know
/// whether the PATCH landed), so the write path gets a longer, dedicated budget
/// than the read-only deployment probes.
const CF_WRITE_TIMEOUT_SECS: u64 = 20;
/// Max KV keys we list in a single page (the UI paginates with the cursor). Caps
/// the response so a namespace with millions of keys cannot flood memory/UI.
const CF_KV_KEY_PAGE_LIMIT: u32 = 100;
/// Max byte length of a KV value we will read into the UI or accept on a write.
/// Larger reads are reported as truncated; larger writes are rejected.
const CF_KV_VALUE_MAX_BYTES: usize = 65_536;
/// Max rows of a D1 query we surface; the rest are dropped and `truncated` set.
const CF_D1_MAX_ROWS: usize = 500;
/// Page size for paginated existence checks (resources that lack a by-id GET).
const CF_EXISTS_PAGE_SIZE: u64 = 100;
/// Page cap for paginated existence checks: 50 pages × 100 = 5_000 resources,
/// far beyond any realistic Aspis-Bio account, so a found resource is never
/// missed while a runaway/abusive list cannot loop unbounded.
const CF_EXISTS_MAX_PAGES: u64 = 50;
const SCW_ACTION_TIMEOUT_SECS: u64 = 4;
/// Storage mutations (Block/File/Object create + delete + resize + snapshot +
/// lifecycle) get a longer write budget than the 4s instant instance actions: a
/// slow create/lifecycle can be ACCEPTED by the server after several seconds, and
/// a premature client timeout would surface an ambiguous "failed" while the
/// resource is actually provisioned (and billed). 20s tolerates that latency.
const SCW_STORAGE_WRITE_TIMEOUT_SECS: u64 = 20;
/// Read-only billing fetch (consumptions + invoices). A small budget keeps the
/// lazily-loaded Billing tab snappy; a slow call degrades gracefully, not hangs.
const SCW_BILLING_TIMEOUT_SECS: u64 = 8;
/// C2: pre-delete volume inventory read gets a longer timeout so a transient
/// slow response does not turn into a silent failed lookup (and orphaned volumes).
const SCW_PRE_DELETE_LOOKUP_TIMEOUT_SECS: u64 = 12;
const CF_TARGET_ACCOUNT_NAME: &str = "aspis-bio";
const CF_ASPIS_BIO_WORKERS: &[&str] = &[
    "aspis-bio-api",
    "aspis-biovision-worker",
    "orasis-worker",
    "aspis-bio-rnaseq-api",
    "aspis-bio-papers",
    "aspis-bio-oauth",
    "aspis-bio-mta-sts",
    "aspis-bio-resend-webhooks",
];
const SCW_API: &str = "https://api.scaleway.com";
const SCW_MONTHLY_HOURS: f64 = 730.0;
const SCW_BLOCK_5K_EUR_PER_GB_HOUR: f64 = 0.000118;
const SCW_BLOCK_15K_EUR_PER_GB_HOUR: f64 = 0.000177;
const SCW_SNAPSHOT_EUR_PER_GB_HOUR: f64 = 0.000044;
const SCW_OBJECT_STANDARD_MULTI_AZ_EUR_PER_GB_HOUR: f64 = 0.000020;
const SCW_OBJECT_STANDARD_ONE_ZONE_EUR_PER_GB_HOUR: f64 = 0.0000103;
const SCW_OBJECT_GLACIER_EUR_PER_GB_HOUR: f64 = 0.0000035;
const SCW_OBJECT_BUCKET_MAX_SCAN_PAGES: u32 = 3;
const SCW_OBJECT_BUCKET_PAGE_SIZE: u32 = 1000;
// File Storage public price: €0.0803/GB/month (incl. ~50% public-beta discount,
// fr-par only). Stored as a GB-hour rate to match the Block Storage pattern.
const SCW_FILE_STORAGE_EUR_PER_GB_HOUR: f64 = 0.0803 / SCW_MONTHLY_HOURS;
// File Storage + Serverless SQL are region-scoped and fr-par-only today.
const SCW_FR_PAR_ONLY_REGIONS: &[&str] = &["fr-par"];
// Generative APIs use a different base + `Authorization: Bearer` (OpenAI-shaped).
const SCW_GENERATIVE_API: &str = "https://api.scaleway.ai";
const SCW_GENERATIVE_TIMEOUT_SECS: u64 = 8;
const SCW_ZONES: &[&str] = &[
    "fr-par-1", "fr-par-2", "fr-par-3", "nl-ams-1", "nl-ams-2", "nl-ams-3", "pl-waw-1", "pl-waw-2",
    "pl-waw-3",
];
const SCW_REGIONS: &[&str] = &["fr-par", "nl-ams", "pl-waw"];
const SCW_TARGET_PROJECT_NAME: &str = "aspis-bio";
const SCW_PAGE_SIZE: usize = 100;
const SCW_MAX_PAGES: u32 = 100;

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn vec_default_on_null<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

fn missing_token_description(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Cloudflare => {
            "Live Worker inventory needs a Workers read token; Worker secret rotation also requires Workers Scripts Write."
        }
        ProviderId::Scaleway => {
            "Live inventory and VM/serverless operations require a Scaleway token with Aspis Bio project access and Instance/Serverless operation permissions."
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderInventory {
    pub health: ProviderHealth,
    pub workers: Vec<CloudflareWorkerSummary>,
    pub compute: Vec<ScalewayResourceSummary>,
    pub storage: Vec<ScalewayStorageSummary>,
    pub risks: Vec<RiskFlag>,
    pub activity: Vec<ActivityEvent>,
    pub selected_scope: Option<ProviderScopeSelection>,
}

#[derive(Debug, Default, Clone)]
pub struct CloudflarePlatformCounts {
    pub r2_buckets: usize,
    pub d1_databases: usize,
    pub kv_namespaces: usize,
    pub queues: usize,
    pub vectorize_indexes: usize,
    pub durable_object_namespaces: usize,
}

impl CloudflarePlatformCounts {
    pub fn total(&self) -> usize {
        self.r2_buckets
            + self.d1_databases
            + self.kv_namespaces
            + self.queues
            + self.vectorize_indexes
            + self.durable_object_namespaces
    }
}

#[derive(Debug, Default, Clone)]
pub struct CloudflarePlatformInventory {
    pub counts: CloudflarePlatformCounts,
    pub resources: Vec<ProviderConsoleResourceSummary>,
}

#[derive(Debug, Default, Clone)]
pub struct ScalewayExtendedInventory {
    pub resources: Vec<ProviderConsoleResourceSummary>,
}

impl ProviderInventory {
    pub fn missing(provider: ProviderId) -> Self {
        Self {
            health: ProviderHealth {
                id: provider,
                name: provider.label().into(),
                status: "missing_token".into(),
                last_sync: None,
                token_health: "missing".into(),
                credential_kind: None,
                resource_count: 0,
                message: Some(format!("{} token is not configured.", provider.label())),
            },
            workers: Vec::new(),
            compute: Vec::new(),
            storage: Vec::new(),
            risks: vec![RiskFlag {
                id: format!("{}_missing_token", provider.as_str()),
                severity: "high".into(),
                title: format!("{} token missing", provider.label()),
                description: missing_token_description(provider).into(),
                source: provider.label().into(),
                timestamp: now(),
            }],
            activity: Vec::new(),
            selected_scope: None,
        }
    }

    pub fn error(provider: ProviderId, message: String) -> Self {
        // B2: inventory-layer errors (Cloudflare/Scaleway fetch_*_inner) reach the
        // UI through here; sanitize so no token/secret can leak into the surface.
        let message = sanitize_error_message(&message);
        let token_health = token_health_from_provider_error(&message);
        Self {
            health: ProviderHealth {
                id: provider,
                name: provider.label().into(),
                status: "error".into(),
                last_sync: Some(now()),
                token_health: token_health.into(),
                credential_kind: None,
                resource_count: 0,
                message: Some(message.clone()),
            },
            workers: Vec::new(),
            compute: Vec::new(),
            storage: Vec::new(),
            risks: vec![RiskFlag {
                id: format!("{}_sync_error", provider.as_str()),
                severity: "medium".into(),
                title: format!("{} sync failed", provider.label()),
                description: message,
                source: provider.label().into(),
                timestamp: now(),
            }],
            activity: Vec::new(),
            selected_scope: None,
        }
    }
}

fn token_health_from_provider_error(message: &str) -> &'static str {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("401") || lowered.contains("unauthorized") {
        "invalid"
    } else if lowered.contains("403") || lowered.contains("forbidden") {
        "insufficient_scope"
    } else {
        "unknown"
    }
}

#[derive(Debug, Deserialize)]
struct CfEnvelope<T> {
    success: bool,
    result: T,
    #[serde(default)]
    result_info: Option<CfResultInfo>,
}

#[derive(Debug, Deserialize)]
struct CfResultInfo {
    #[serde(default)]
    total_pages: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CfTokenVerify {
    #[serde(default)]
    id: Option<String>,
    status: Option<String>,
    expires_on: Option<String>,
    #[serde(default, deserialize_with = "vec_default_on_null")]
    policies: Vec<CfTokenPolicy>,
}

#[derive(Debug, Deserialize)]
struct CfTokenDetail {
    #[serde(default, deserialize_with = "vec_default_on_null")]
    policies: Vec<CfTokenPolicy>,
}

#[derive(Debug, Deserialize)]
struct CfTokenPolicy {
    #[serde(default, deserialize_with = "vec_default_on_null")]
    permission_groups: Vec<CfPermissionGroup>,
}

#[derive(Debug, Deserialize)]
struct CfPermissionGroup {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CfAccount {
    id: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CfWorkerScript {
    id: String,
    #[serde(default)]
    created_on: Option<String>,
    #[serde(default)]
    modified_on: Option<String>,
    #[serde(default)]
    usage_model: Option<String>,
    #[serde(default, deserialize_with = "vec_default_on_null")]
    routes: Vec<CfWorkerRoute>,
    #[serde(default)]
    compatibility_date: Option<String>,
    #[serde(default, deserialize_with = "vec_default_on_null")]
    compatibility_flags: Vec<String>,
    #[serde(default, deserialize_with = "vec_default_on_null")]
    handlers: Vec<String>,
    #[serde(default, deserialize_with = "vec_default_on_null")]
    tags: Vec<String>,
    #[serde(default)]
    annotations: Option<CfWorkerAnnotations>,
}

#[derive(Debug, Deserialize)]
struct CfWorkerRoute {
    pattern: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CfWorkerAnnotations {
    #[serde(rename = "workers/message", default)]
    message: Option<String>,
    #[serde(rename = "workers/tag", default)]
    tag: Option<String>,
    #[serde(rename = "workers/triggered_by", default)]
    triggered_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CfDeploymentsResult {
    #[serde(default, deserialize_with = "vec_default_on_null")]
    deployments: Vec<CfWorkerDeployment>,
}

#[derive(Debug, Clone, Deserialize)]
struct CfWorkerDeployment {
    #[serde(default)]
    created_on: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, deserialize_with = "vec_default_on_null")]
    versions: Vec<CfDeploymentVersion>,
    #[serde(default)]
    annotations: Option<CfDeploymentAnnotations>,
}

#[derive(Debug, Clone, Deserialize)]
struct CfDeploymentVersion {
    percentage: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct CfDeploymentAnnotations {
    #[serde(rename = "workers/message", default)]
    message: Option<String>,
    #[serde(rename = "workers/triggered_by", default)]
    triggered_by: Option<String>,
}

pub async fn fetch_cloudflare(
    http: &reqwest::Client,
    token: Option<String>,
    pinned_account_id: Option<String>,
) -> ProviderInventory {
    let Some(token) = token else {
        return ProviderInventory::missing(ProviderId::Cloudflare);
    };

    match fetch_cloudflare_inner(http, &token, pinned_account_id.as_deref()).await {
        Ok(inventory) => inventory,
        Err(e) => ProviderInventory::error(ProviderId::Cloudflare, e),
    }
}

async fn fetch_cloudflare_inner(
    http: &reqwest::Client,
    token: &str,
    pinned_account_id: Option<&str>,
) -> Result<ProviderInventory, String> {
    let mut verification = fetch_cloudflare_token_verification(http, token).await;

    let all_accounts: Vec<CfAccount> =
        fetch_cloudflare_paginated(http, token, &format!("{CF_API}/accounts"), "accounts").await?;
    let selected_scope_source = cloudflare_selection_source(
        &all_accounts,
        pinned_account_id,
        configured_cloudflare_account_id().as_deref(),
    );
    let accounts = select_cloudflare_accounts(&all_accounts, pinned_account_id)?;
    let selected_scope = accounts.first().map(|account| ProviderScopeSelection {
        provider: ProviderId::Cloudflare,
        id: account.id.clone(),
        name: account.name.clone(),
        source: selected_scope_source,
    });
    let scope_name_unverified = cloudflare_scope_name_unverified(selected_scope.as_ref());
    if verification.verify.is_none() {
        if let Some(scope) = selected_scope.as_ref() {
            let account_verification =
                fetch_cloudflare_account_token_verification(http, token, &scope.id).await;
            if account_verification.verify.is_some() {
                verification = account_verification;
            } else if let Some(account_issue) = account_verification.issue {
                let user_issue = verification.issue.take();
                verification.issue = Some(match user_issue {
                    Some(user_issue) => format!("{user_issue} {account_issue}"),
                    None => account_issue,
                });
            }
        }
    }
    // `GET /tokens/verify` never returns `policies`, so the verify response alone
    // can only ever yield `Unknown` for write permission. After verifying, fetch the
    // token's real policies by id (requires "API Tokens Read") and classify write from
    // those. If the detail fetch is unreadable (e.g. token lacks API Tokens Read), we
    // fall back to the verify policies (usually empty → `Unknown` → `valid_unverified`)
    // and never hard-fail.
    let write_permission = match verification.verify.as_ref() {
        Some(verify) => {
            let detail_policies = match (verify.id.as_deref(), selected_scope.as_ref()) {
                (Some(token_id), Some(scope)) if !token_id.is_empty() => {
                    fetch_cloudflare_token_detail_policies(
                        http,
                        token,
                        verification.source,
                        &scope.id,
                        token_id,
                    )
                    .await
                }
                _ => None,
            };
            let policies = detail_policies.as_deref().unwrap_or(&verify.policies);
            cloudflare_workers_scripts_write_permission(policies)
        }
        None => CloudflareWorkersWritePermission::Unknown,
    };
    let verification_source = verification.source;

    let mut workers = Vec::new();
    let mut deployment_failure_count = 0usize;
    let mut hidden_sibling_worker_count = 0usize;
    for account in accounts {
        let scripts: Vec<CfWorkerScript> = fetch_cloudflare_paginated(
            http,
            token,
            &format!("{CF_API}/accounts/{}/workers/scripts", account.id),
            "worker scripts",
        )
        .await
        .map_err(|e| {
            format!(
                "Cloudflare worker request failed for {}: {e}",
                account.name.as_deref().unwrap_or(&account.id)
            )
        })?;
        let script_count = scripts.len();
        let scoped_scripts = scripts
            .into_iter()
            .filter(cloudflare_worker_in_aspis_bio_scope)
            .collect::<Vec<_>>();
        hidden_sibling_worker_count += script_count.saturating_sub(scoped_scripts.len());

        deployment_failure_count +=
            fetch_cloudflare_worker_summaries(http, token, &account, scoped_scripts, &mut workers)
                .await;
    }

    let mut risks = Vec::new();
    if let Some(expires_on) = verification
        .verify
        .as_ref()
        .and_then(|verify| verify.expires_on.clone())
    {
        risks.push(RiskFlag {
            id: "cloudflare_token_expiry_visible".into(),
            severity: "low".into(),
            title: "Cloudflare token has an expiry date".into(),
            description: format!("Token expires on {expires_on}. Track rotation before that date."),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }
    if let Some(issue) = verification.issue {
        risks.push(RiskFlag {
            id: "cloudflare_token_verify_unavailable".into(),
            severity: "medium".into(),
            title: "Cloudflare token policy not introspectable".into(),
            description: format!(
                "{issue} Inventory sync worked, but Worker secret rotation stays disabled until an API token proves Workers Scripts Write."
            ),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }
    if write_permission == CloudflareWorkersWritePermission::Missing {
        risks.push(RiskFlag {
            id: "cloudflare_secret_rotation_unavailable".into(),
            severity: "medium".into(),
            title: "Cloudflare secret rotation unavailable".into(),
            description: "Inventory sync works with this token, but rotating Worker secrets requires Workers Scripts Write.".into(),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }
    if write_permission == CloudflareWorkersWritePermission::Unknown {
        risks.push(RiskFlag {
            id: "cloudflare_workers_write_unverified".into(),
            severity: "medium".into(),
            title: "Cloudflare write permission not proven".into(),
            description: "Token policy details were not returned by Cloudflare. Ensure the token includes Workers Scripts Write before rotating Worker secrets.".into(),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }
    if deployment_failure_count > 0 {
        risks.push(RiskFlag {
            id: "cloudflare_worker_deployment_metadata_partial".into(),
            severity: "low".into(),
            title: "Cloudflare deployment metadata partial".into(),
            description: format!(
                "{deployment_failure_count} Worker deployment lookup(s) failed. Worker health is based on script listing where deployment metadata is missing."
            ),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }
    if hidden_sibling_worker_count > 0 {
        risks.push(RiskFlag {
            id: "cloudflare_sibling_workers_hidden".into(),
            severity: "low".into(),
            title: "Cloudflare sibling workers hidden".into(),
            description: format!(
                "{hidden_sibling_worker_count} Worker(s) in the account are outside the Aspis Bio allowlist and are hidden from mutation surfaces."
            ),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }
    if scope_name_unverified {
        risks.push(RiskFlag {
            id: "cloudflare_scope_name_unverified".into(),
            severity: "medium".into(),
            title: "Cloudflare account scope not proven as Aspis Bio".into(),
            description: format!(
                "The selected Cloudflare account name does not match '{CF_TARGET_ACCOUNT_NAME}'. Keep the saved account id pinned and verify listed resources before rotating Worker secrets."
            ),
            source: "Cloudflare".into(),
            timestamp: now(),
        });
    }

    Ok(ProviderInventory {
        health: ProviderHealth {
            id: ProviderId::Cloudflare,
            name: "Cloudflare".into(),
            status: if write_permission != CloudflareWorkersWritePermission::Present
                || deployment_failure_count > 0
                || scope_name_unverified
            {
                "degraded"
            } else {
                "healthy"
            }
            .into(),
            last_sync: Some(now()),
            token_health: match write_permission {
                CloudflareWorkersWritePermission::Present => "valid",
                CloudflareWorkersWritePermission::Missing => "valid_read_only",
                CloudflareWorkersWritePermission::Unknown => "valid_unverified",
            }
            .into(),
            credential_kind: Some(cloudflare_credential_kind(verification_source).into()),
            resource_count: workers.len(),
            message: if write_permission == CloudflareWorkersWritePermission::Missing {
                Some(
                    "Read-only token. Worker secret rotation requires Workers Scripts Write."
                        .into(),
                )
            } else if write_permission == CloudflareWorkersWritePermission::Unknown {
                Some(
                    "Cloudflare inventory is readable, but Workers Scripts Write could not be proven."
                        .into(),
                )
            } else if deployment_failure_count > 0 {
                Some("Some Worker deployment metadata could not be read.".into())
            } else if scope_name_unverified {
                Some("Cloudflare account name does not prove Aspis Bio scope.".into())
            } else {
                None
            },
        },
        workers,
        compute: Vec::new(),
        storage: Vec::new(),
        risks,
        activity: vec![ActivityEvent {
            id: "cloudflare_sync_completed".into(),
            message: match verification_source {
                CloudflareTokenVerificationSource::Account => {
                    "Cloudflare inventory sync completed with an account-owned API token.".into()
                }
                CloudflareTokenVerificationSource::User => {
                    "Cloudflare inventory sync completed with a user API token.".into()
                }
                CloudflareTokenVerificationSource::Unverified => {
                    "Cloudflare inventory sync completed with unverified token policy.".into()
                }
            },
            timestamp: now(),
            event_type: "sync".into(),
            source: "Cloudflare".into(),
        }],
        selected_scope,
    })
}

#[derive(Debug)]
struct CloudflareTokenVerification {
    verify: Option<CfTokenVerify>,
    issue: Option<String>,
    source: CloudflareTokenVerificationSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudflareTokenVerificationSource {
    User,
    Account,
    Unverified,
}

fn cloudflare_credential_kind(source: CloudflareTokenVerificationSource) -> &'static str {
    match source {
        CloudflareTokenVerificationSource::User => "cloudflare_profile_token",
        CloudflareTokenVerificationSource::Account => "cloudflare_account_owned_token",
        CloudflareTokenVerificationSource::Unverified => "cloudflare_unverified_policy_token",
    }
}

async fn fetch_cloudflare_token_verification(
    http: &reqwest::Client,
    token: &str,
) -> CloudflareTokenVerification {
    fetch_cloudflare_token_verification_url(
        http,
        token,
        &format!("{CF_API}/user/tokens/verify"),
        CloudflareTokenVerificationSource::User,
        "Cloudflare user token verify",
    )
    .await
}

async fn fetch_cloudflare_account_token_verification(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
) -> CloudflareTokenVerification {
    fetch_cloudflare_token_verification_url(
        http,
        token,
        &format!("{CF_API}/accounts/{account_id}/tokens/verify"),
        CloudflareTokenVerificationSource::Account,
        "Cloudflare account token verify",
    )
    .await
}

async fn fetch_cloudflare_token_verification_url(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    source: CloudflareTokenVerificationSource,
    label: &str,
) -> CloudflareTokenVerification {
    let response = match http.get(url).bearer_auth(token).send().await {
        Ok(response) => response,
        Err(e) => {
            return CloudflareTokenVerification {
                verify: None,
                issue: Some(format!("{label} request failed: {e}.")),
                source: CloudflareTokenVerificationSource::Unverified,
            }
        }
    };

    if !response.status().is_success() {
        return CloudflareTokenVerification {
            verify: None,
            issue: Some(format!(
                "{label} rejected the token with HTTP {}.",
                response.status().as_u16()
            )),
            source: CloudflareTokenVerificationSource::Unverified,
        };
    }

    let envelope: CfEnvelope<CfTokenVerify> = match response.json().await {
        Ok(envelope) => envelope,
        Err(e) => {
            return CloudflareTokenVerification {
                verify: None,
                issue: Some(format!("{label} response could not be parsed: {e}.")),
                source: CloudflareTokenVerificationSource::Unverified,
            }
        }
    };

    if !envelope.success || envelope.result.status.as_deref() != Some("active") {
        return CloudflareTokenVerification {
            verify: None,
            issue: Some(format!("{label} did not report an active API token.")),
            source: CloudflareTokenVerificationSource::Unverified,
        };
    }

    CloudflareTokenVerification {
        verify: Some(envelope.result),
        issue: None,
        source,
    }
}

async fn fetch_cloudflare_paginated<T>(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    label: &str,
) -> Result<Vec<T>, String>
where
    T: DeserializeOwned,
{
    let mut page = 1u32;
    let mut out = Vec::new();

    loop {
        let page_value = page.to_string();
        let envelope: CfEnvelope<Vec<T>> = http
            .get(url)
            .query(&[("page", page_value.as_str()), ("per_page", "100")])
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Cloudflare {label} request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Cloudflare {label} request rejected: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Cloudflare {label} response was invalid: {e}"))?;

        if !envelope.success {
            return Err(format!("Cloudflare {label} request was not successful."));
        }

        out.extend(envelope.result);
        let total_pages = envelope
            .result_info
            .and_then(|info| info.total_pages)
            .unwrap_or(1);
        if page >= total_pages {
            break;
        }
        if page >= 100 {
            return Err(format!("Cloudflare {label} pagination exceeded 100 pages."));
        }
        page += 1;
    }

    Ok(out)
}

async fn fetch_cloudflare_worker_summaries(
    http: &reqwest::Client,
    token: &str,
    account: &CfAccount,
    scripts: Vec<CfWorkerScript>,
    workers: &mut Vec<CloudflareWorkerSummary>,
) -> usize {
    let mut deployment_failure_count = 0usize;
    let mut batch = Vec::with_capacity(CF_DEPLOYMENT_CONCURRENCY);

    for script in scripts {
        batch.push(script);
        if batch.len() == CF_DEPLOYMENT_CONCURRENCY {
            deployment_failure_count +=
                fetch_cloudflare_worker_summary_batch(http, token, account, batch, workers).await;
            batch = Vec::with_capacity(CF_DEPLOYMENT_CONCURRENCY);
        }
    }

    if !batch.is_empty() {
        deployment_failure_count +=
            fetch_cloudflare_worker_summary_batch(http, token, account, batch, workers).await;
    }

    deployment_failure_count
}

async fn fetch_cloudflare_worker_summary_batch(
    http: &reqwest::Client,
    token: &str,
    account: &CfAccount,
    scripts: Vec<CfWorkerScript>,
    workers: &mut Vec<CloudflareWorkerSummary>,
) -> usize {
    let mut handles = Vec::with_capacity(scripts.len());
    for script in scripts {
        let http = http.clone();
        let token = token.to_string();
        let account_id = account.id.clone();
        let script_name = script.id.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            let deployment =
                fetch_cloudflare_worker_latest_deployment(&http, &token, &account_id, &script_name)
                    .await;
            (script, deployment)
        }));
    }

    let mut deployment_failure_count = 0usize;
    for handle in handles {
        match handle.await {
            Ok((script, deployment)) => {
                if deployment.is_none() {
                    deployment_failure_count += 1;
                }
                workers.push(cloudflare_worker_summary(account, script, deployment));
            }
            Err(_) => deployment_failure_count += 1,
        }
    }
    deployment_failure_count
}

async fn fetch_cloudflare_worker_latest_deployment(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    script_name: &str,
) -> Option<CfWorkerDeployment> {
    let encoded_script = urlencoding::encode(script_name);
    let deployments: CfEnvelope<CfDeploymentsResult> = http
        .get(format!(
            "{CF_API}/accounts/{account_id}/workers/scripts/{encoded_script}/deployments"
        ))
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    if !deployments.success {
        return None;
    }
    deployments.result.deployments.into_iter().next()
}

/// Reads a worker's bindings via `GET .../workers/scripts/{name}/settings`.
///
/// JSON path: the envelope is `{ success, errors, messages, result }` and the
/// bindings live under `result.bindings` (an array). `result` also carries
/// `compatibility_date`. Each binding object has at least `{ name, type }`.
/// A `plain_text` binding additionally carries its value under `text`; a
/// `secret_text` binding is returned by Cloudflare WITHOUT any value (name +
/// type only), so we never populate `text` for it. All other binding kinds
/// (`kv_namespace`, `r2_bucket`, `service`, `durable_object_namespace`, `d1`,
/// `queue`, …) carry a kind-specific reference id which we surface in
/// `reference`.
///
/// On 401/403 or any failure (network, non-success status, parse error,
/// `success: false`) this returns a `readable: false` struct with a short
/// `message` instead of a hard error, so the caller/UI degrades gracefully.
pub async fn fetch_cloudflare_worker_settings(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    worker_name: &str,
) -> CloudflareWorkerSettings {
    let unreadable = |message: &str| CloudflareWorkerSettings {
        account_id: account_id.to_string(),
        worker_name: worker_name.to_string(),
        plain_text: Vec::new(),
        secrets: Vec::new(),
        other_bindings: Vec::new(),
        compatibility_date: None,
        readable: false,
        message: Some(message.to_string()),
    };

    let encoded_script = urlencoding::encode(worker_name);
    let response = match http
        .get(format!(
            "{CF_API}/accounts/{account_id}/workers/scripts/{encoded_script}/settings"
        ))
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return unreadable("Worker settings request failed."),
    };

    let status = response.status();
    if !status.is_success() {
        let message = if status.as_u16() == 401 || status.as_u16() == 403 {
            "Worker settings are not readable with the current token permissions."
        } else {
            "Worker settings endpoint returned an error."
        };
        return unreadable(message);
    }

    let Ok(payload) = response.json::<Value>().await else {
        return unreadable("Worker settings response was invalid.");
    };

    if payload
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        return unreadable("Worker settings could not be read.");
    }

    let result = payload.get("result").unwrap_or(&payload);
    // Schema-drift guard: a 200 with neither a top-level `result` object nor a
    // `bindings` ARRAY means we fell back to the whole payload and found nothing
    // we can classify with confidence. Treat a missing `bindings`, an explicit
    // `null`, and any non-array shape identically here: reporting `readable: true`
    // with empty buckets would falsely imply the worker has no env vars. This is
    // the read path, so the response stays non-fatal (`readable: false` + a
    // message); no write/PATCH is possible from here.
    if payload.get("result").is_none() && !matches!(result.get("bindings"), Some(Value::Array(_))) {
        return unreadable("Worker settings response was missing the expected bindings.");
    }
    let (plain_text, secrets, other_bindings) = classify_cloudflare_worker_bindings(result);

    CloudflareWorkerSettings {
        account_id: account_id.to_string(),
        worker_name: worker_name.to_string(),
        plain_text,
        secrets,
        other_bindings,
        compatibility_date: string_field(result, &["compatibility_date"]),
        readable: true,
        message: None,
    }
}

/// Splits a worker `/settings` `result` object's `bindings` array into
/// (plain_text, secrets, other). `plain_text` carries its value under `text`;
/// `secret_text` never carries a value (Cloudflare omits it), so `text` stays
/// `None`; every other kind keeps a best-effort reference id. Returned in the
/// same order as the source array within each bucket.
fn classify_cloudflare_worker_bindings(
    result: &Value,
) -> (
    Vec<CloudflareWorkerBinding>,
    Vec<CloudflareWorkerBinding>,
    Vec<CloudflareWorkerBinding>,
) {
    let mut plain_text = Vec::new();
    let mut secrets = Vec::new();
    let mut other_bindings = Vec::new();

    let bindings = result.get("bindings").and_then(Value::as_array);
    for binding in bindings.into_iter().flatten() {
        let name = string_field(binding, &["name"]).unwrap_or_default();
        let binding_type = string_field(binding, &["type"]).unwrap_or_default();
        match binding_type.as_str() {
            "plain_text" => plain_text.push(CloudflareWorkerBinding {
                name,
                binding_type,
                text: string_field(binding, &["text"]),
                reference: None,
            }),
            "secret_text" => secrets.push(CloudflareWorkerBinding {
                name,
                binding_type,
                text: None,
                reference: None,
            }),
            _ => {
                let reference = string_field(
                    binding,
                    &[
                        "namespace_id",
                        "bucket_name",
                        "database_id",
                        "id",
                        "service",
                        "class_name",
                        "script_name",
                        "queue_name",
                        "namespace",
                        "index_name",
                    ],
                );
                other_bindings.push(CloudflareWorkerBinding {
                    name,
                    binding_type,
                    text: None,
                    reference,
                });
            }
        }
    }

    (plain_text, secrets, other_bindings)
}

/// Pure, network-free preview of a `plain_text` env-var write. Computes the
/// before/after of the targeted variable and enumerates the bindings that the
/// write path will preserve (secrets via `inherit`, every other binding re-sent
/// verbatim). NEVER echoes a secret value — only secret NAMES appear, and they
/// are never available as values in `settings` anyway. The reported
/// `api_equivalent` describes the PATCH shape; `risks` spells out the
/// replace-all semantics and the inherit safety net.
pub fn cloudflare_env_dry_run(
    settings: &CloudflareWorkerSettings,
    var_name: &str,
    new_value: &str,
) -> CloudflareEnvDryRunResult {
    let existing = settings
        .plain_text
        .iter()
        .find(|binding| binding.name == var_name);
    let (before, kind) = match existing {
        Some(binding) => (binding.text.clone(), "update".to_string()),
        None => (None, "create".to_string()),
    };
    let changes = vec![CloudflareEnvBindingChange {
        name: var_name.to_string(),
        before,
        after: new_value.to_string(),
        kind,
    }];

    let preserved_secrets: Vec<String> = settings
        .secrets
        .iter()
        .map(|binding| binding.name.clone())
        .collect();
    let preserved_other: Vec<String> = settings
        .other_bindings
        .iter()
        .map(|binding| format!("{} ({})", binding.name, binding.binding_type))
        .collect();

    let other_plain_count = settings
        .plain_text
        .iter()
        .filter(|binding| binding.name != var_name)
        .count();

    let api_equivalent = vec![
        format!(
            "GET /accounts/{}/workers/scripts/{}/settings (re-read before write)",
            settings.account_id, settings.worker_name
        ),
        format!(
            "PATCH /accounts/{}/workers/scripts/{}/settings",
            settings.account_id, settings.worker_name
        ),
        "body: { \"bindings\": [<full existing array; target plain_text set; every secret_text -> {type:inherit,name}>] }".to_string(),
    ];

    let risks = vec![
        "Cloudflare PATCH /settings REPLACES the entire bindings array; any omitted binding is dropped.".to_string(),
        format!(
            "{} secret binding(s) preserved by converting each to an `inherit` binding (Cloudflare keeps the existing secret value).",
            preserved_secrets.len()
        ),
        format!(
            "{} other binding(s) (KV/R2/DO/D1/queue/service/...) re-sent byte-for-byte verbatim.",
            preserved_other.len()
        ),
        format!(
            "{} other plain_text variable(s) re-sent unchanged.",
            other_plain_count
        ),
        "The write re-fetches the live /settings immediately before patching; the UI snapshot is never trusted.".to_string(),
    ];

    CloudflareEnvDryRunResult {
        worker_name: settings.worker_name.clone(),
        var_name: var_name.to_string(),
        changes,
        preserved_secrets,
        preserved_other,
        api_equivalent,
        risks,
    }
}

/// Pure transform of a RAW `bindings` array (as returned verbatim under
/// `result.bindings` by `GET /settings`) into the array to PATCH back. This is
/// the lossless core of the write path:
///
/// - The `plain_text` binding named `var_name` has its `text` set to
///   `new_value`; if no such binding exists one is appended as
///   `{type:"plain_text", name:var_name, text:new_value}`.
/// - Every `secret_text` binding is converted to `{type:"inherit", name}` so
///   Cloudflare keeps the existing secret value (we never have it).
/// - Every other binding object is passed through byte-for-byte unchanged, so
///   KV/R2/DO/D1/queue/service/etc. survive the replace-all PATCH intact.
///
/// Operating on the raw JSON (not the simplified `CloudflareWorkerBinding`
/// model) is what guarantees no field is lost on round-trip.
fn rewrite_bindings_for_plain_text(
    raw: &[Value],
    var_name: &str,
    new_value: &str,
) -> Result<Vec<Value>, String> {
    let mut out: Vec<Value> = Vec::with_capacity(raw.len() + 1);
    let mut updated_existing = false;

    for binding in raw {
        let binding_type = binding.get("type").and_then(Value::as_str).unwrap_or("");
        let binding_name = binding.get("name").and_then(Value::as_str).unwrap_or("");
        match binding_type {
            "plain_text" if binding_name == var_name => {
                // Preserve any sibling fields CF may add; only overwrite `text`.
                let mut rewritten = binding.clone();
                if let Value::Object(map) = &mut rewritten {
                    map.insert("text".to_string(), Value::String(new_value.to_string()));
                }
                out.push(rewritten);
                updated_existing = true;
            }
            "secret_text" => {
                // Drop the (absent) value and tell CF to keep the existing secret
                // via `inherit`. A nameless secret cannot be inherited: emitting
                // `{type:"inherit",name:""}` would silently DROP the secret on the
                // replace-all PATCH. Refuse the whole write instead.
                if binding_name.is_empty() {
                    return Err(
                        "Worker has a secret binding with no name; refusing to write to avoid dropping it."
                            .into(),
                    );
                }
                out.push(json!({ "type": "inherit", "name": binding_name }));
            }
            _ => out.push(binding.clone()),
        }
    }

    if !updated_existing {
        out.push(json!({
            "type": "plain_text",
            "name": var_name,
            "text": new_value,
        }));
    }

    Ok(out)
}

/// Pure guard that extracts the raw `bindings` array from a re-fetched
/// `GET /settings` payload, refusing on ANY ambiguity. This is the load-bearing
/// wipe-prevention check: we only ever PATCH back an array Cloudflare actually
/// gave us as a concrete `Value::Array`.
///
/// - `result` absent  → `Err` (cannot trust the shape; do not write).
/// - `result` present but not a JSON object → `Err` (unexpected shape).
/// - `result.bindings` is a `Value::Array` → `Ok(array.clone())`. An EMPTY array
///   is valid and meaningful: a genuinely empty worker returns `bindings: []`,
///   and the target var will be appended to it.
/// - `result.bindings` is `null`, absent, or any non-array → `Err`. We must NOT
///   coerce these to an empty array: doing so on `null`/missing would WIPE every
///   binding of a live worker on the replace-all PATCH.
fn extract_worker_bindings(payload: &Value) -> Result<Vec<Value>, String> {
    let Some(result) = payload.get("result") else {
        return Err("Worker settings response was missing result; refusing to write.".into());
    };
    if !result.is_object() {
        return Err(
            "Worker settings response had an unexpected result shape; refusing to write.".into(),
        );
    }
    match result.get("bindings") {
        Some(Value::Array(bindings)) => Ok(bindings.clone()),
        _ => Err("Worker settings response bindings was not an array; refusing to write.".into()),
    }
}

/// Writes (creates or updates) a single `plain_text` env var on an Aspis-Bio
/// Worker. Re-reads the live `/settings` immediately, takes the raw bindings
/// array verbatim, rewrites only the target (see
/// [`rewrite_bindings_for_plain_text`]), and PATCHes the FULL array back.
///
/// keep_bindings decision: we do NOT send `keep_bindings`. The documented,
/// load-bearing safety net is the per-binding `inherit` conversion of every
/// `secret_text` plus re-sending all other bindings verbatim, which preserves
/// everything on a replace-all PATCH without depending on `keep_bindings` being
/// accepted by THIS endpoint (its acceptance on `/settings` PATCH is
/// unverified). Revisit on a live throwaway binding in Phase 5.
pub async fn patch_cloudflare_worker_plain_text(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    worker_name: &str,
    var_name: &str,
    new_value: &str,
) -> Result<(), String> {
    let encoded_script = urlencoding::encode(worker_name);
    let settings_url =
        format!("{CF_API}/accounts/{account_id}/workers/scripts/{encoded_script}/settings");

    // 1. Re-fetch the RAW settings immediately before writing. Never trust a
    //    stale UI snapshot.
    let response = http
        .get(&settings_url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("Worker settings re-read failed: {e}")))?
        .error_for_status()
        .map_err(|e| sanitize_error_message(&format!("Worker settings re-read rejected: {e}")))?;

    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("Worker settings response invalid: {e}")))?;

    if payload
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        let detail = cf_envelope_error_message(&payload);
        return Err(format!(
            "Worker settings could not be re-read before write.{detail}"
        ));
    }

    // Wipe-prevention guard: only proceed on a concrete `result.bindings` array
    // (an explicit empty array is the legitimate "empty worker" case). A missing
    // `result`, non-object `result`, or `null`/absent/non-array `bindings` all
    // refuse here — no PATCH is sent, so a live worker can never be wiped by an
    // ambiguous re-read.
    let raw_bindings = extract_worker_bindings(&payload)?;

    // 2-5. Build the lossless replacement array.
    let new_bindings = rewrite_bindings_for_plain_text(&raw_bindings, var_name, new_value)?;

    // 6. PATCH the full modified array back.
    //
    // Defensive against replace-vs-merge ambiguity on `PATCH /settings`: if the
    // endpoint is full-replace (UNVERIFIED — must be confirmed on a live
    // throwaway worker in Phase 5), sending only `{"bindings":[...]}` would reset
    // top-level settings like `compatibility_date`/`compatibility_flags`/
    // `usage_model` and could break the worker. So we re-send those top-level
    // fields verbatim FROM THE SAME RE-FETCHED RESULT, but ONLY when they were
    // present (never send a null). This is correct under BOTH semantics: under
    // replace they are restored; under merge they are harmless no-ops.
    // `logpush`/`observability`/`tags`/`tail_consumers` are intentionally left
    // out of scope: under merge they are preserved, and we have no safe value to
    // re-send under replace.
    let result = payload.get("result").unwrap_or(&payload);
    let mut body_map = serde_json::Map::new();
    body_map.insert("bindings".to_string(), Value::Array(new_bindings));
    if let Some(value @ Value::String(_)) = result.get("compatibility_date") {
        body_map.insert("compatibility_date".to_string(), value.clone());
    }
    if let Some(value @ Value::Array(_)) = result.get("compatibility_flags") {
        body_map.insert("compatibility_flags".to_string(), value.clone());
    }
    if let Some(value @ Value::String(_)) = result.get("usage_model") {
        body_map.insert("usage_model".to_string(), value.clone());
    }
    let body = Value::Object(body_map);
    let patch = http
        .patch(&settings_url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("Worker env write failed: {e}")))?;

    let status = patch.status();
    let patch_payload: Value = patch
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("Worker env write response invalid: {e}")))?;

    let envelope_ok = patch_payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&patch_payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the Worker env write ({status}).{detail}"
        )));
    }

    Ok(())
}

/// Extracts a sanitized `errors[].message` summary from a Cloudflare API
/// envelope, prefixed with " " when present so it can be appended to a message.
fn cf_envelope_error_message(payload: &Value) -> String {
    let messages: Vec<String> = payload
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| error.get("message").and_then(Value::as_str))
                .map(sanitize_error_message)
                .collect()
        })
        .unwrap_or_default();
    if messages.is_empty() {
        String::new()
    } else {
        format!(" {}", messages.join("; "))
    }
}

/// Fetches the token's real policy list by id. The `GET /tokens/verify` endpoint
/// never returns `policies`, so write detection must read the token detail. This
/// endpoint requires the token to hold "API Tokens Read"; on ANY failure
/// (network, non-success status, 403, parse error, `success: false`) we return
/// `None` so the caller treats write permission as `Unknown` rather than hard-failing.
async fn fetch_cloudflare_token_detail_policies(
    http: &reqwest::Client,
    token: &str,
    source: CloudflareTokenVerificationSource,
    account_id: &str,
    token_id: &str,
) -> Option<Vec<CfTokenPolicy>> {
    // A user/profile token's detail lives under `/user/tokens/{id}`; an
    // account-owned token's under `/accounts/{id}/tokens/{id}`. Hitting the wrong
    // one 404/403s, which would (harmlessly but uselessly) fall back to `Unknown`.
    let encoded_token = urlencoding::encode(token_id);
    let url = match source {
        CloudflareTokenVerificationSource::User => {
            format!("{CF_API}/user/tokens/{encoded_token}")
        }
        _ => {
            let encoded_account = urlencoding::encode(account_id);
            format!("{CF_API}/accounts/{encoded_account}/tokens/{encoded_token}")
        }
    };
    let detail: CfEnvelope<CfTokenDetail> = http
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    if !detail.success {
        return None;
    }
    Some(detail.result.policies)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudflareWorkersWritePermission {
    Present,
    Missing,
    Unknown,
}

fn cloudflare_workers_scripts_write_permission(
    policies: &[CfTokenPolicy],
) -> CloudflareWorkersWritePermission {
    if policies.is_empty() {
        return CloudflareWorkersWritePermission::Unknown;
    }

    let mut saw_named_group = false;
    let has_write = policies.iter().any(|policy| {
        policy.permission_groups.iter().any(|group| {
            let Some(name) = group.name.as_deref() else {
                return false;
            };
            saw_named_group = true;
            normalize_permission_name(name) == "workers scripts write"
        })
    });

    if has_write {
        CloudflareWorkersWritePermission::Present
    } else if !saw_named_group {
        CloudflareWorkersWritePermission::Unknown
    } else {
        CloudflareWorkersWritePermission::Missing
    }
}

fn normalize_permission_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn normalize_target_name(name: &str) -> String {
    name.trim()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase()
}

fn cloudflare_scope_name_unverified(selected_scope: Option<&ProviderScopeSelection>) -> bool {
    let Some(scope) = selected_scope else {
        return false;
    };
    if !matches!(scope.source.as_str(), "single_account_token" | "pinned") {
        return false;
    }
    let target = normalize_target_name(CF_TARGET_ACCOUNT_NAME);
    scope.name.as_deref().map(normalize_target_name).as_deref() != Some(target.as_str())
}

#[cfg(not(test))]
fn configured_cloudflare_account_id() -> Option<String> {
    std::env::var("ASPIS_CLOUDFLARE_ACCOUNT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
fn configured_cloudflare_account_id() -> Option<String> {
    None
}

fn select_cloudflare_accounts(
    accounts: &[CfAccount],
    pinned_account_id: Option<&str>,
) -> Result<Vec<CfAccount>, String> {
    if let Some(account_id) = pinned_account_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(configured_cloudflare_account_id)
    {
        let selected = accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
            .ok_or_else(|| {
                "Configured Cloudflare Aspis Bio account id was not visible to this token."
                    .to_string()
            })?;
        return Ok(vec![selected]);
    }

    let target = normalize_target_name(CF_TARGET_ACCOUNT_NAME);
    let selected = accounts
        .iter()
        .filter(|account| {
            account
                .name
                .as_deref()
                .map(normalize_target_name)
                .as_deref()
                == Some(target.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();

    match selected.len() {
        1 => return Ok(selected),
        len if len > 1 => {
            return Err(format!(
                "Multiple Cloudflare accounts matched '{CF_TARGET_ACCOUNT_NAME}'. Set ASPIS_CLOUDFLARE_ACCOUNT_ID to pin the Aspis Bio account."
            ));
        }
        _ => {}
    }

    if accounts.len() == 1 {
        return Ok(vec![accounts[0].clone()]);
    }

    Err(format!(
        "Cloudflare account '{CF_TARGET_ACCOUNT_NAME}' was not found. Pin the Aspis Bio account id before reading account-wide inventory."
    ))
}

fn cloudflare_selection_source(
    accounts: &[CfAccount],
    pinned_account_id: Option<&str>,
    env_account_id: Option<&str>,
) -> String {
    if pinned_account_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        || env_account_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
    {
        "pinned".into()
    } else if accounts.len() == 1 {
        "single_account_token".into()
    } else {
        "name_match".into()
    }
}

fn cloudflare_worker_in_aspis_bio_scope(script: &CfWorkerScript) -> bool {
    if cloudflare_worker_name_in_aspis_bio_scope(&script.id) {
        return true;
    }
    script.routes.iter().any(|route| {
        route
            .pattern
            .as_deref()
            .map(route_pattern_in_aspis_bio_host)
            .unwrap_or(false)
    })
}

/// C4: name-only Aspis Bio scope check, callable from the rotation command before
/// the PUT so it does not rely solely on the cache filter.
pub fn cloudflare_worker_name_in_aspis_bio_scope(worker_name: &str) -> bool {
    let name = worker_name.trim().to_ascii_lowercase();
    CF_ASPIS_BIO_WORKERS.iter().any(|allowed| *allowed == name) || name.starts_with("aspis-bio-")
}

/// C5: match on a host boundary, not a substring. A route pattern's host must be
/// exactly `aspis-bio.com` or a subdomain `*.aspis-bio.com`, so a lookalike like
/// `aspis-bio.com.evil.tld` is NOT treated as in-scope.
fn route_pattern_in_aspis_bio_host(pattern: &str) -> bool {
    let lowered = pattern.trim().to_ascii_lowercase();
    // Strip an optional scheme, then take the host portion up to the first '/'.
    let without_scheme = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .trim_end_matches('*')
        .trim_end_matches('.');
    host == "aspis-bio.com" || host.ends_with(".aspis-bio.com")
}

fn cloudflare_worker_summary(
    account: &CfAccount,
    script: CfWorkerScript,
    deployment: Option<CfWorkerDeployment>,
) -> CloudflareWorkerSummary {
    let routes = script
        .routes
        .into_iter()
        .filter_map(|route| route.pattern)
        .collect::<Vec<_>>();
    let annotations = script.annotations;
    let (purpose, purpose_source) =
        cloudflare_worker_purpose(&script.id, &routes, &annotations, deployment.as_ref());
    let mut tags = script.tags;
    if let Some(annotation) = &annotations {
        if let Some(tag) = annotation
            .tag
            .as_deref()
            .filter(|tag| !tag.trim().is_empty())
        {
            tags.push(tag.trim().into());
        }
        if let Some(triggered_by) = annotation
            .triggered_by
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            tags.push(format!("triggered_by:{}", triggered_by.trim()));
        }
    }
    if let Some(deployment) = &deployment {
        if let Some(source) = deployment
            .source
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            tags.push(format!("deployment_source:{}", source.trim()));
        }
        if let Some(triggered_by) = deployment
            .annotations
            .as_ref()
            .and_then(|annotation| annotation.triggered_by.as_deref())
            .filter(|value| !value.trim().is_empty())
        {
            tags.push(format!("deployment_triggered_by:{}", triggered_by.trim()));
        }
    }
    tags.sort();
    tags.dedup();

    let oracle_query = cloudflare_worker_oracle_query(&script.id, &routes, &tags, &purpose);
    let status = cloudflare_worker_deployment_status(deployment.as_ref());
    let last_deploy = deployment
        .as_ref()
        .and_then(|deployment| deployment.created_on.clone())
        .or(script.modified_on)
        .or(script.created_on);

    CloudflareWorkerSummary {
        id: format!("{}:{}", account.id, script.id),
        account_id: account.id.clone(),
        account_name: account.name.clone(),
        name: script.id,
        status,
        purpose,
        purpose_source,
        routes,
        last_deploy,
        usage_model: script.usage_model,
        compatibility_date: script.compatibility_date,
        compatibility_flags: script.compatibility_flags,
        handlers: script.handlers,
        tags,
        oracle_query,
    }
}

fn cloudflare_worker_purpose(
    script_name: &str,
    routes: &[String],
    annotations: &Option<CfWorkerAnnotations>,
    deployment: Option<&CfWorkerDeployment>,
) -> (String, String) {
    if let Some(message) = deployment
        .and_then(|deployment| deployment.annotations.as_ref())
        .and_then(|annotation| annotation.message.as_deref())
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return (message.into(), "deployment".into());
    }

    if let Some(message) = annotations
        .as_ref()
        .and_then(|annotation| annotation.message.as_deref())
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return (message.into(), "annotation".into());
    }

    if let Some(route) = routes
        .first()
        .map(String::as_str)
        .map(str::trim)
        .filter(|route| !route.is_empty())
    {
        return (format!("Handles traffic for {route}."), "route".into());
    }

    let readable_name = script_name.replace(['-', '_'], " ");
    (
        format!("Worker inferred from name: {readable_name}."),
        "name".into(),
    )
}

fn cloudflare_worker_deployment_status(deployment: Option<&CfWorkerDeployment>) -> String {
    let Some(deployment) = deployment else {
        return "unknown".into();
    };
    let active_percentage = deployment
        .versions
        .iter()
        .map(|version| version.percentage)
        .sum::<f64>();
    if active_percentage >= 99.9 {
        "healthy"
    } else {
        "degraded"
    }
    .into()
}

fn cloudflare_worker_oracle_query(
    script_name: &str,
    routes: &[String],
    tags: &[String],
    purpose: &str,
) -> String {
    [script_name]
        .into_iter()
        .chain(routes.iter().map(String::as_str))
        .chain(tags.iter().map(String::as_str))
        .chain([purpose])
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn fetch_cloudflare_platform_inventory(
    http: &reqwest::Client,
    token: Option<&str>,
    selected_scope: Option<&ProviderScopeSelection>,
) -> CloudflarePlatformInventory {
    let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) else {
        return CloudflarePlatformInventory::default();
    };
    let Some(scope) = selected_scope.filter(|scope| scope.provider == ProviderId::Cloudflare)
    else {
        return CloudflarePlatformInventory::default();
    };
    let account_id = scope.id.as_str();

    let lists = join_all([
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/r2/buckets"),
            &["buckets"],
            "cf-storage-data",
            "R2 Bucket",
            "https://developers.cloudflare.com/api/resources/r2/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/d1/database?per_page=100"),
            &["databases"],
            "cf-storage-data",
            "D1 Database",
            "https://developers.cloudflare.com/api/resources/d1/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/storage/kv/namespaces?per_page=100"),
            &["namespaces"],
            "cf-storage-data",
            "KV Namespace",
            "https://developers.cloudflare.com/api/node/resources/kv/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/queues"),
            &["queues"],
            "cf-storage-data",
            "Queue",
            "https://developers.cloudflare.com/api/resources/queues/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/vectorize/v2/indexes"),
            &["indexes"],
            "cf-storage-data",
            "Vectorize Index",
            "https://developers.cloudflare.com/api/resources/vectorize/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/workers/durable_objects/namespaces"),
            &["namespaces"],
            "cf-storage-data",
            "Durable Object Namespace",
            "https://developers.cloudflare.com/api/resources/durable_objects/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/pages/projects"),
            &["projects"],
            "cf-workers-pages",
            "Pages Project",
            "https://developers.cloudflare.com/api/resources/pages/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/access/apps"),
            &["apps", "applications"],
            "cf-security-network",
            "Access Application",
            "https://developers.cloudflare.com/api/resources/zero_trust/subresources/access/subresources/applications/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/cfd_tunnel"),
            &["tunnels"],
            "cf-security-network",
            "Tunnel",
            "https://developers.cloudflare.com/api/resources/zero_trust/subresources/tunnels/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/ai-search/namespaces"),
            &["namespaces"],
            "cf-ai-observability",
            "AI Search Namespace",
            "https://developers.cloudflare.com/ai-search/api/search/rest-api/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/ai-search/instances"),
            &["instances"],
            "cf-ai-observability",
            "AI Search Instance",
            "https://developers.cloudflare.com/ai-search/api/search/rest-api/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/ai-gateway/gateways"),
            &["gateways"],
            "cf-ai-observability",
            "AI Gateway",
            "https://developers.cloudflare.com/api/resources/ai_gateway/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/logpush/jobs"),
            &["jobs"],
            "cf-ai-observability",
            "Logpush Job",
            "https://developers.cloudflare.com/api/resources/logpush/",
        ),
        cloudflare_list_endpoint(
            http,
            token,
            &format!("{CF_API}/accounts/{account_id}/audit_logs?per_page=50"),
            &["audit_logs"],
            "cf-account-iam",
            "Audit Log",
            "https://developers.cloudflare.com/api/resources/audit_logs/",
        ),
    ])
    .await;
    let mut resources = lists.into_iter().flatten().collect::<Vec<_>>();
    resources.extend(fetch_cloudflare_zone_console_resources(http, token, account_id).await);
    resources.sort_by(|a, b| {
        a.resource_type
            .cmp(&b.resource_type)
            .then_with(|| a.name.cmp(&b.name))
    });

    CloudflarePlatformInventory {
        counts: CloudflarePlatformCounts {
            r2_buckets: resources
                .iter()
                .filter(|resource| resource.resource_type == "R2 Bucket")
                .count(),
            d1_databases: resources
                .iter()
                .filter(|resource| resource.resource_type == "D1 Database")
                .count(),
            kv_namespaces: resources
                .iter()
                .filter(|resource| resource.resource_type == "KV Namespace")
                .count(),
            queues: resources
                .iter()
                .filter(|resource| resource.resource_type == "Queue")
                .count(),
            vectorize_indexes: resources
                .iter()
                .filter(|resource| resource.resource_type == "Vectorize Index")
                .count(),
            durable_object_namespaces: resources
                .iter()
                .filter(|resource| resource.resource_type == "Durable Object Namespace")
                .count(),
        },
        resources,
    }
}

async fn cloudflare_list_endpoint(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    collection_keys: &[&str],
    service_id: &str,
    resource_type: &str,
    docs_url: &str,
) -> Vec<ProviderConsoleResourceSummary> {
    let response = match http
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            return vec![cloudflare_console_diagnostic(
                service_id,
                resource_type,
                docs_url,
                "unavailable",
                &format!("Cloudflare {resource_type} request failed: {e}"),
            )]
        }
    };
    let status = response.status();
    if !status.is_success() {
        return vec![cloudflare_console_diagnostic(
            service_id,
            resource_type,
            docs_url,
            if status.as_u16() == 401 || status.as_u16() == 403 {
                "forbidden"
            } else {
                "unavailable"
            },
            &format!("Cloudflare {resource_type} endpoint returned {status}."),
        )];
    }
    let Ok(payload) = response.json::<Value>().await else {
        return vec![cloudflare_console_diagnostic(
            service_id,
            resource_type,
            docs_url,
            "unavailable",
            &format!("Cloudflare {resource_type} response was invalid."),
        )];
    };
    json_result_items(&payload, collection_keys)
        .unwrap_or_default()
        .into_iter()
        .map(|item| cloudflare_console_resource(service_id, resource_type, docs_url, item))
        .collect()
}

fn cloudflare_console_diagnostic(
    service_id: &str,
    resource_type: &str,
    docs_url: &str,
    status: &str,
    message: &str,
) -> ProviderConsoleResourceSummary {
    ProviderConsoleResourceSummary {
        id: format!("cloudflare:{service_id}:{resource_type}:diagnostic:{status}"),
        provider: ProviderId::Cloudflare,
        service_id: service_id.into(),
        resource_type: format!("{resource_type} access"),
        name: format!("{resource_type} access"),
        region: None,
        status: status.into(),
        description: message.into(),
        metadata: vec!["scope: account".into()],
        docs_url: docs_url.into(),
        updated_at: Some(now()),
    }
}

async fn fetch_cloudflare_zone_console_resources(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
) -> Vec<ProviderConsoleResourceSummary> {
    let zones = cloudflare_list_endpoint(
        http,
        token,
        &format!(
            "{CF_API}/zones?account.id={}",
            urlencoding::encode(account_id)
        ),
        &["zones"],
        "cf-security-network",
        "Zone",
        "https://developers.cloudflare.com/api/resources/zones/",
    )
    .await;
    let mut resources = zones.clone();

    let zone_children = join_all(
        zones
            .iter()
            .filter(|zone| zone.resource_type == "Zone")
            .map(|zone| async move {
                let zone_id = zone
                    .id
                    .rsplit(':')
                    .next()
                    .map(str::to_string)
                    .unwrap_or_else(|| zone.name.clone());
                let lists = join_all([
                    cloudflare_list_endpoint(
                        http,
                        token,
                        &format!("{CF_API}/zones/{zone_id}/dns_records?per_page=100"),
                        &["dns_records"],
                        "cf-security-network",
                        "DNS Record",
                        "https://developers.cloudflare.com/api/resources/dns/subresources/records/",
                    ),
                    cloudflare_list_endpoint(
                        http,
                        token,
                        &format!("{CF_API}/zones/{zone_id}/rulesets"),
                        &["rulesets"],
                        "cf-security-network",
                        "Ruleset",
                        "https://developers.cloudflare.com/api/resources/rulesets/",
                    ),
                ])
                .await;
                lists.into_iter().flatten().collect::<Vec<_>>()
            }),
    )
    .await;
    resources.extend(zone_children.into_iter().flatten());
    resources
}

/// Maps one `/accounts/{id}/subscriptions` `result[]` item into a billing plan.
/// `name`/`id`/`currency` come from the nested `rate_plan`; `frequency` and
/// `price` live on the SUBSCRIPTION object itself (the rate_plan has no price),
/// so we read them from the item with the rate_plan as a fallback. Pure: no I/O.
fn cloudflare_billing_plan_from_subscription(item: &Value) -> CloudflareBillingPlan {
    let rate_plan = item.get("rate_plan");
    let id = rate_plan.and_then(|plan| string_field(plan, &["id"]));
    let name = rate_plan.and_then(|plan| string_field(plan, &["public_name", "id"]));
    // Currency may sit on either the subscription or the rate_plan.
    let currency = string_field(item, &["currency"])
        .or_else(|| rate_plan.and_then(|plan| string_field(plan, &["currency"])));
    let frequency = string_field(item, &["frequency"]);
    let price = number_field(item, &["price"]);
    // A short human summary of the plan state, useful when name/price are absent.
    let component_summary = string_field(item, &["state"]);
    CloudflareBillingPlan {
        id,
        name,
        currency,
        frequency,
        price,
        component_summary,
    }
}

/// Maps one `/user/billing/history` `result[]` item into an invoice summary.
/// `kind` comes from the item's `type`; `status` from `action` (the closest the
/// history payload has to a status). Pure: no I/O.
fn cloudflare_invoice_from_history(item: &Value) -> CloudflareInvoiceSummary {
    CloudflareInvoiceSummary {
        id: string_field(item, &["id"]),
        occurred_at: string_field(item, &["occurred_at"]),
        amount: number_field(item, &["amount"]),
        currency: string_field(item, &["currency"]),
        status: string_field(item, &["status", "action"]),
        kind: string_field(item, &["type"]),
    }
}

/// Account-level real billing, loaded lazily (NOT part of the sync snapshot).
///
/// Reads the account plan from `/accounts/{id}/subscriptions` and recent charges
/// from `/user/billing/history`. Per-worker € cost is NOT exposed by any
/// Cloudflare API; this returns only the account plan + invoices.
///
/// Graceful degradation: a 401/403 or transport error on the subscriptions call
/// yields `readable: false` with a guidance message. If subscriptions succeed
/// but the user-scoped history call fails (it may be unreachable with an
/// account-owned token), the plans are still returned with `readable: true` and
/// a message noting invoices were unavailable. Never hard-errors. Never logs
/// amounts or the token.
pub async fn fetch_cloudflare_billing(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
) -> CloudflareBilling {
    // --- Plans: GET /accounts/{id}/subscriptions ---
    let plans_outcome = match http
        .get(format!("{CF_API}/accounts/{account_id}/subscriptions"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                match response.json::<Value>().await {
                    // Plans are the floor: a 200 whose body is not the expected
                    // result array (e.g. `{"result":null}`) must read as
                    // UNREADABLE, never as a silently-empty plan list. A 200 whose
                    // envelope reports `success:false` is a Cloudflare LOGICAL error
                    // (token/scope/quota); `json_result_items` would map it to an
                    // empty list (correct for unwrap_or_default list endpoints, but
                    // here it would hide the error as "0 plans, readable"). Reject it
                    // explicitly BEFORE `json_result_items` so the combiner marks the
                    // whole billing view unreadable with a message.
                    Ok(payload) => {
                        if payload.get("success").and_then(Value::as_bool) == Some(false) {
                            Err("Cloudflare billing response was invalid.".to_string())
                        } else {
                            json_result_items(&payload, &["subscriptions"])
                                .map(|items| {
                                    items
                                        .iter()
                                        .map(cloudflare_billing_plan_from_subscription)
                                        .collect::<Vec<_>>()
                                })
                                .ok_or_else(|| {
                                    "Cloudflare billing response was invalid.".to_string()
                                })
                        }
                    }
                    Err(_) => Err("Cloudflare billing response was invalid.".to_string()),
                }
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                Err("Billing needs an account-owned token with Billing Read.".to_string())
            } else {
                Err("Cloudflare billing endpoint returned an error.".to_string())
            }
        }
        Err(_) => Err("Cloudflare billing request failed.".to_string()),
    };

    // --- Invoices: GET /user/billing/history?per_page=20 ---
    // User-scoped; may be unreachable with an account-owned token. Treated as
    // best-effort: a failure here downgrades to a message, not an error. Unlike
    // plans, a malformed 200 (None) is tolerated as "no invoices".
    let invoices_outcome = match http
        .get(format!("{CF_API}/user/billing/history?per_page=20"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<Value>().await {
                    Ok(payload) => Ok(json_result_items(&payload, &["history"])
                        .unwrap_or_default()
                        .iter()
                        .map(cloudflare_invoice_from_history)
                        .collect::<Vec<_>>()),
                    Err(_) => Err("Invoice history was unavailable.".to_string()),
                }
            } else {
                Err("Invoice history was unavailable.".to_string())
            }
        }
        Err(_) => Err("Invoice history was unavailable.".to_string()),
    };

    cloudflare_billing_from_outcomes(plans_outcome, invoices_outcome)
}

/// Pure combiner for the billing outcomes — encodes the "plans are the floor"
/// degradation contract with no I/O so it can be exhaustively tested:
/// * plans `Err`            → `readable: false` + that message (whole view unreadable).
/// * plans `Ok` + inv `Err` → `readable: true`, plans kept, empty invoices, a note.
/// * both `Ok`              → `readable: true`, plans + invoices, no message.
fn cloudflare_billing_from_outcomes(
    plans: Result<Vec<CloudflareBillingPlan>, String>,
    invoices: Result<Vec<CloudflareInvoiceSummary>, String>,
) -> CloudflareBilling {
    let plans = match plans {
        Ok(plans) => plans,
        Err(message) => {
            return CloudflareBilling {
                plans: Vec::new(),
                invoices: Vec::new(),
                readable: false,
                message: Some(message),
            };
        }
    };

    match invoices {
        Ok(invoices) => CloudflareBilling {
            plans,
            invoices,
            readable: true,
            message: None,
        },
        Err(_) => CloudflareBilling {
            plans,
            invoices: Vec::new(),
            readable: true,
            message: Some(
                "Account plan loaded. Invoice history was unavailable — it may need a user-owned token (the account-owned token cannot reach /user/billing/history)."
                    .to_string(),
            ),
        },
    }
}

/// Parsed Scaleway `Money` object: `(value, currency_code)`.
///
/// Scaleway monetary fields (consumption `value`, invoice `total_untaxed`/
/// `total_taxed`) use the protobuf Money shape `{currency_code, units, nanos}`:
/// the amount is `units + nanos / 1e9`. The protobuf-JSON encoding may emit the
/// int64 `units` as a STRING, so both are read via `number_field`. Returns
/// `(None, None)` for a missing/empty/malformed object so callers surface
/// `Option`s rather than fabricating a zero amount.
fn scaleway_money_from(value: Option<&Value>) -> (Option<f64>, Option<String>) {
    let Some(money) = value.filter(|money| money.is_object()) else {
        return (None, None);
    };
    let currency = string_field(money, &["currency_code"]);
    let units = number_field(money, &["units"]);
    let nanos = number_field(money, &["nanos"]);
    // Only synthesize an amount when at least one numeric component was present;
    // an object with neither units nor nanos yields `None`, not `0.0`.
    let amount = match (units, nanos) {
        (None, None) => None,
        (units, nanos) => {
            let total = units.unwrap_or(0.0) + nanos.unwrap_or(0.0) / 1_000_000_000.0;
            total.is_finite().then_some(total)
        }
    };
    (amount, currency)
}

/// Maps one `consumptions[]` item from `GET /billing/v2beta1/consumptions`.
/// `category` ← `category_name`; `value_untaxed`/`currency` are flattened from
/// the `value` `Money`. `billing_period` is NOT on the line, so the request's
/// period is back-filled in. Pure: no I/O.
fn scaleway_consumption_line_from(item: &Value, billing_period: &str) -> ScalewayConsumptionLine {
    let (value_untaxed, currency) = scaleway_money_from(item.get("value"));
    let billing_period = billing_period.trim();
    ScalewayConsumptionLine {
        category: string_field(item, &["category_name"]),
        project_id: string_field(item, &["project_id"]),
        value_untaxed,
        currency,
        billing_period: (!billing_period.is_empty()).then(|| billing_period.to_string()),
    }
}

/// Maps one `invoices[]` item from `GET /billing/v2beta1/invoices`.
/// `issued_at` ← `issued_date`; `stop_date` prefers a real `stop_date` and falls
/// back to `due_date`. `state` is read best-effort. Totals/currency are flattened
/// from the `Money` objects. All reads are tolerant of absent fields (empty
/// string / None), so a payload-shape change across billing generations degrades
/// gracefully rather than dropping the invoice.
/// Pure: no I/O.
fn scaleway_invoice_from(item: &Value) -> ScalewayInvoiceLine {
    let (total_untaxed, untaxed_currency) = scaleway_money_from(item.get("total_untaxed"));
    let (total_taxed, taxed_currency) = scaleway_money_from(item.get("total_taxed"));
    ScalewayInvoiceLine {
        id: string_field(item, &["id"]),
        issued_at: string_field(item, &["issued_date"]),
        start_date: string_field(item, &["start_date"]),
        stop_date: string_field(item, &["stop_date", "due_date"]),
        total_untaxed,
        total_taxed,
        currency: untaxed_currency.or(taxed_currency),
        state: string_field(item, &["state"]),
    }
}

/// Consumptions outcome carried into the combiner: the mapped lines, the summed
/// untaxed total, the API's `total_discount_untaxed_value`, and `updated_at`.
type ScalewayConsumptionsOutcome = (
    Vec<ScalewayConsumptionLine>,
    Option<f64>,
    Option<f64>,
    Option<String>,
);

/// Pure parser for a `GET /billing/v2beta1/consumptions` 200 envelope. Returns
/// `None` when the body is not the expected shape (no `consumptions` array), so
/// the caller can mark the whole view unreadable; `Some(outcome)` otherwise.
///
/// `total_discount_untaxed_value` is a protobuf `Money` object
/// (`{currency_code, units, nanos}`), NOT a flat number, so it is flattened via
/// `scaleway_money_from` — reading it with `number_field` would always yield
/// `None`. The untaxed grand total is summed from the per-line values because
/// the envelope exposes no untaxed grand total. Pure: no I/O.
fn scaleway_billing_parse_consumptions(
    payload: &Value,
    billing_period: &str,
) -> Option<ScalewayConsumptionsOutcome> {
    let items = payload.get("consumptions").and_then(Value::as_array)?;
    let lines: Vec<ScalewayConsumptionLine> = items
        .iter()
        .map(|item| scaleway_consumption_line_from(item, billing_period))
        .collect();
    // Sum the per-line untaxed values for a real total (the consumptions
    // envelope exposes a discount total but not an untaxed grand total).
    let total_untaxed = if lines.iter().any(|l| l.value_untaxed.is_some()) {
        Some(lines.iter().filter_map(|l| l.value_untaxed).sum::<f64>())
    } else {
        None
    };
    // `total_discount_untaxed_value` is a Money object, not a flat number.
    let total_discount = scaleway_money_from(payload.get("total_discount_untaxed_value")).0;
    let updated_at = string_field(payload, &["updated_at"]);
    Some((lines, total_untaxed, total_discount, updated_at))
}

/// Organization-scoped Scaleway billing, loaded lazily (NOT part of the sync
/// snapshot). Reads consumptions from `/billing/v2beta1/consumptions` and recent
/// invoices from `/billing/v2beta1/invoices` (both v2beta1 — the current billing
/// generation; the older v2alpha1 invoices endpoint is superseded).
/// `billing_period` is the caller-computed `YYYY-MM`.
///
/// Graceful degradation mirrors the Cloudflare billing contract: consumptions
/// are the floor (a 401/403/transport error or malformed 200 yields
/// `readable:false` with a message); invoices are best-effort (a failure
/// downgrades to a note, consumptions stay readable). Never hard-errors, never
/// logs amounts or the token (`X-Auth-Token` header, never the URL).
pub async fn fetch_scaleway_billing_request(
    http: &reqwest::Client,
    token: &str,
    org_id: &str,
    billing_period: &str,
) -> ScalewayBilling {
    // --- Consumptions: GET /billing/v2beta1/consumptions ---
    let consumptions_outcome: Result<ScalewayConsumptionsOutcome, String> = match http
        .get(format!("{SCW_API}/billing/v2beta1/consumptions"))
        .query(&[
            ("organization_id", org_id),
            ("billing_period", billing_period),
        ])
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_BILLING_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                match response.json::<Value>().await {
                    Ok(payload) => {
                        // Consumptions are the floor: a 200 whose body is not the
                        // expected envelope (no `consumptions` array) must read as
                        // UNREADABLE, never silently-empty. Reject it explicitly so
                        // the combiner marks the whole view unreadable.
                        match scaleway_billing_parse_consumptions(&payload, billing_period) {
                            Some(outcome) => Ok(outcome),
                            None => Err("Scaleway billing response was invalid.".to_string()),
                        }
                    }
                    Err(_) => Err("Scaleway billing response was invalid.".to_string()),
                }
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                Err("Billing needs a key with Billing Read on this organization.".to_string())
            } else {
                Err("Scaleway billing endpoint returned an error.".to_string())
            }
        }
        Err(_) => Err("Scaleway billing request failed.".to_string()),
    };

    // --- Invoices: GET /billing/v2beta1/invoices (best-effort) ---
    // A malformed 200 (no `invoices` array) is tolerated as "no invoices" here,
    // unlike consumptions; only a transport/HTTP error downgrades to a note.
    let invoices_outcome: Result<(Vec<ScalewayInvoiceLine>, usize), String> = match http
        .get(format!("{SCW_API}/billing/v2beta1/invoices"))
        .query(&[("organization_id", org_id)])
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_BILLING_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<Value>().await {
                    Ok(payload) => {
                        let invoices = payload
                            .get("invoices")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items.iter().map(scaleway_invoice_from).collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let total_count = number_field(&payload, &["total_count"])
                            .map(|count| count as usize)
                            .unwrap_or(invoices.len());
                        Ok((invoices, total_count))
                    }
                    Err(_) => Err("Scaleway invoices were unavailable.".to_string()),
                }
            } else {
                Err("Scaleway invoices were unavailable.".to_string())
            }
        }
        Err(_) => Err("Scaleway invoices were unavailable.".to_string()),
    };

    scaleway_billing_from_outcomes(consumptions_outcome, invoices_outcome)
}

/// Pure combiner for the Scaleway billing outcomes — encodes the "consumptions
/// are the floor" degradation contract with no I/O so it can be exhaustively
/// tested:
/// * consumptions `Err`            → `readable:false` + that message.
/// * consumptions `Ok` + inv `Err` → `readable:true`, consumptions kept, a note.
/// * both `Ok`                     → `readable:true`, no message.
fn scaleway_billing_from_outcomes(
    consumptions: Result<ScalewayConsumptionsOutcome, String>,
    invoices: Result<(Vec<ScalewayInvoiceLine>, usize), String>,
) -> ScalewayBilling {
    let (consumptions, total_untaxed, total_discount, updated_at) = match consumptions {
        Ok(payload) => payload,
        Err(message) => {
            return ScalewayBilling {
                consumptions: Vec::new(),
                total_untaxed: None,
                total_discount: None,
                invoices: Vec::new(),
                updated_at: None,
                readable: false,
                message: Some(message),
            };
        }
    };

    match invoices {
        Ok((invoices, _total_count)) => ScalewayBilling {
            consumptions,
            total_untaxed,
            total_discount,
            invoices,
            updated_at,
            readable: true,
            message: None,
        },
        Err(_) => ScalewayBilling {
            consumptions,
            total_untaxed,
            total_discount,
            invoices: Vec::new(),
            updated_at,
            readable: true,
            message: Some(
                "Consumption loaded. Scaleway invoices were unavailable — the key may lack Billing Read for invoices."
                    .to_string(),
            ),
        },
    }
}

#[cfg(test)]
fn cloudflare_json_result_count(payload: &Value, collection_keys: &[&str]) -> Option<usize> {
    json_result_items(payload, collection_keys).map(|items| items.len())
}

fn json_result_items(payload: &Value, collection_keys: &[&str]) -> Option<Vec<Value>> {
    if payload
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        return Some(Vec::new());
    }
    let result = payload.get("result").unwrap_or(payload);
    if let Some(items) = result.as_array() {
        return Some(items.clone());
    }
    let object = result.as_object()?;
    for key in collection_keys {
        if let Some(items) = object.get(*key).and_then(Value::as_array) {
            return Some(items.clone());
        }
    }
    None
}

fn cloudflare_console_resource(
    service_id: &str,
    resource_type: &str,
    docs_url: &str,
    item: Value,
) -> ProviderConsoleResourceSummary {
    let name = string_field(&item, &["name", "title", "queue_name", "bucket_name", "id"])
        .unwrap_or_else(|| "unnamed".into());
    let raw_id =
        string_field(&item, &["id", "name", "title", "queue_id"]).unwrap_or_else(|| name.clone());
    let status = string_field(&item, &["status", "state"]).unwrap_or_else(|| "available".into());
    let region = string_field(&item, &["jurisdiction", "location", "region"]);
    let updated_at = string_field(&item, &["modified_on", "updated_at", "created_at"]);
    let mut metadata = Vec::new();
    for key in [
        "id",
        "created_at",
        "modified_on",
        "updated_at",
        "jurisdiction",
        "location",
        "type",
        "source",
        "engine_version",
        "enable",
        "ai_gateway_id",
        "dataset",
        "destination_conf",
        "enabled",
        "action",
        "actor",
        "interface",
    ] {
        if let Some(value) = string_field(&item, &[key]) {
            metadata.push(format!("{key}: {value}"));
        }
    }

    ProviderConsoleResourceSummary {
        id: format!("cloudflare:{service_id}:{resource_type}:{raw_id}"),
        provider: ProviderId::Cloudflare,
        service_id: service_id.into(),
        resource_type: resource_type.into(),
        name,
        region,
        status,
        description: "Cloudflare account resource listed through the API.".into(),
        metadata,
        docs_url: docs_url.into(),
        updated_at,
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(String::from)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .or_else(|| value.as_f64().map(|number| number.to_string()))
                .or_else(|| value.as_bool().map(|flag| flag.to_string()))
        })
}

/// Reads the first numeric value among `keys`, accepting either a JSON number or
/// a numeric string (Cloudflare returns prices as strings like `"5.00"` in some
/// billing payloads). Non-finite values are rejected so we never surface NaN.
fn number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    let object = value.as_object()?;
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| {
            value.as_f64().or_else(|| {
                value
                    .as_str()
                    .and_then(|raw| raw.trim().parse::<f64>().ok())
            })
        })
        .filter(|number| number.is_finite())
}

#[derive(Debug, Deserialize)]
struct ScwServersEnvelope {
    #[serde(default)]
    servers: Vec<ScwServer>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwServerActionsEnvelope {
    #[serde(default)]
    actions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ScwServerProductsEnvelope {
    #[serde(default)]
    servers: HashMap<String, ScwServerProduct>,
}

#[derive(Debug, Deserialize)]
struct ScwServerProduct {
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    ncpus: Option<u32>,
    #[serde(default)]
    ram: Option<u64>,
    #[serde(default)]
    gpu: Option<u32>,
    #[serde(default)]
    gpu_info: Option<Value>,
    #[serde(default)]
    monthly_price: Option<f64>,
    #[serde(default)]
    hourly_price: Option<f64>,
    #[serde(default)]
    capabilities: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ScwServerAvailabilityEnvelope {
    #[serde(default)]
    servers: HashMap<String, ScwServerAvailability>,
}

#[derive(Debug, Deserialize)]
struct ScwServerAvailability {
    #[serde(default)]
    availability: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScwBlockVolumesEnvelope {
    #[serde(default)]
    volumes: Vec<ScwBlockVolume>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwBlockSnapshotsEnvelope {
    #[serde(default)]
    snapshots: Vec<ScwBlockSnapshot>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScwS3ListBucketsResult {
    buckets: Option<ScwS3Buckets>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScwS3Buckets {
    #[serde(default, rename = "Bucket")]
    buckets: Vec<ScwS3Bucket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScwS3Bucket {
    name: String,
    #[serde(default)]
    creation_date: Option<String>,
}

impl ScwS3Bucket {
    fn into_summary(
        self,
        region: &str,
        project: &ScwProject,
        usage: Option<ScwObjectBucketUsage>,
    ) -> ScalewayStorageSummary {
        let usage = usage.unwrap_or_default();
        let size_gb = bytes_to_gb(usage.total_bytes);
        let estimated_eur_month = if usage.total_bytes > 0 && !usage.has_unknown_storage_class {
            Some(usage.estimated_eur_month)
        } else {
            None
        };
        let pricing_label = if usage.total_bytes == 0 {
            "No listed objects or usage scan unavailable".into()
        } else if usage.partial {
            format!(
                "Partial scan: {} object(s), {} page(s)",
                usage.object_count, usage.pages_scanned
            )
        } else {
            format!("Scanned {} object(s)", usage.object_count)
        };
        let pricing_note = if usage.partial {
            format!(
                "Partial Object Storage estimate from first {} page(s), max {} objects/page. Full bucket usage needs provider metrics or a deeper scan.",
                usage.pages_scanned, SCW_OBJECT_BUCKET_PAGE_SIZE
            )
        } else if usage.has_unknown_storage_class {
            "Object Storage usage found, but at least one object storage class was unknown; no euro estimate shown.".into()
        } else if usage.total_bytes > 0 {
            format!(
                "Object Storage public prices before tax applied per listed object storage class: Standard Multi-AZ €{SCW_OBJECT_STANDARD_MULTI_AZ_EUR_PER_GB_HOUR:.7}/GB/hour, Standard One Zone €{SCW_OBJECT_STANDARD_ONE_ZONE_EUR_PER_GB_HOUR:.7}/GB/hour, Glacier €{SCW_OBJECT_GLACIER_EUR_PER_GB_HOUR:.7}/GB/hour."
            )
        } else {
            "Bucket listed, but no object usage was returned by the bounded scan.".into()
        };

        ScalewayStorageSummary {
            id: format!("object_bucket_{region}_{}", self.name),
            name: self.name,
            storage_type: "Object Bucket".into(),
            region: region.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: "available".into(),
            size_gb,
            price_eur_per_gb_hour: None,
            estimated_eur_month,
            pricing_label,
            pricing_note,
            created_at: self.creation_date,
            updated_at: None,
            tags: if usage.partial {
                vec!["partial-scan".into()]
            } else {
                Vec::new()
            },
            billable: true,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct ScwObjectBucketUsage {
    total_bytes: u64,
    estimated_eur_month: f64,
    object_count: usize,
    pages_scanned: u32,
    partial: bool,
    has_unknown_storage_class: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScwS3ListObjectsV2Result {
    #[serde(default, rename = "Contents")]
    contents: Vec<ScwS3Object>,
    #[serde(default)]
    is_truncated: Option<bool>,
    #[serde(default)]
    next_continuation_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScwS3Object {
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    storage_class: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScwProjectsEnvelope {
    #[serde(default)]
    projects: Vec<ScwProject>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScwProject {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ScwApiKeyInfo {
    #[serde(default)]
    default_project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScwBlockVolume {
    id: String,
    name: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    perf_iops: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ScwBlockVolume {
    fn into_summary(self, zone: &str, project: &ScwProject) -> ScalewayStorageSummary {
        let price = match self.perf_iops {
            Some(iops) if iops >= 15_000 => SCW_BLOCK_15K_EUR_PER_GB_HOUR,
            _ => SCW_BLOCK_5K_EUR_PER_GB_HOUR,
        };
        let size_gb = bytes_to_gb(self.size.unwrap_or(0));
        let storage_type = match self.perf_iops {
            Some(iops) if iops >= 15_000 => "Block Storage 15K",
            _ => "Block Storage 5K",
        };

        ScalewayStorageSummary {
            id: self.id,
            name: self.name,
            storage_type: storage_type.into(),
            region: zone.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: normalize_scaleway_state(
                self.status
                    .as_deref()
                    .or(self.state.as_deref())
                    .unwrap_or("unknown"),
            ),
            size_gb,
            price_eur_per_gb_hour: Some(price),
            estimated_eur_month: Some(estimate_monthly_storage_eur(size_gb, price)),
            pricing_label: format!("€{price:.6}/GB/hour"),
            pricing_note: "Official Scaleway Block Storage GB-hour public price, before tax."
                .into(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            tags: self.tags,
            billable: true,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScwBlockSnapshot {
    id: String,
    name: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ScwBlockSnapshot {
    fn into_summary(self, zone: &str, project: &ScwProject) -> ScalewayStorageSummary {
        let size_gb = bytes_to_gb(self.size.unwrap_or(0));

        ScalewayStorageSummary {
            id: self.id,
            name: self.name,
            storage_type: "Block Snapshot".into(),
            region: zone.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: normalize_scaleway_state(
                self.status
                    .as_deref()
                    .or(self.state.as_deref())
                    .unwrap_or("unknown"),
            ),
            size_gb,
            price_eur_per_gb_hour: Some(SCW_SNAPSHOT_EUR_PER_GB_HOUR),
            estimated_eur_month: Some(estimate_monthly_storage_eur(
                size_gb,
                SCW_SNAPSHOT_EUR_PER_GB_HOUR,
            )),
            pricing_label: format!("€{SCW_SNAPSHOT_EUR_PER_GB_HOUR:.6}/GB/hour"),
            pricing_note: "Official Scaleway Block Snapshot GB-hour public price, before tax."
                .into(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            tags: self.tags,
            billable: true,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScwFilesystemsEnvelope {
    #[serde(default)]
    filesystems: Vec<ScwFilesystem>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwFilesystem {
    id: String,
    name: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ScwFilesystem {
    fn into_summary(self, region: &str, project: &ScwProject) -> ScalewayStorageSummary {
        let size_gb = bytes_to_gb(self.size.unwrap_or(0));

        ScalewayStorageSummary {
            id: self.id,
            name: self.name,
            storage_type: "File System".into(),
            region: region.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: normalize_scaleway_state(self.status.as_deref().unwrap_or("unknown")),
            size_gb,
            price_eur_per_gb_hour: Some(SCW_FILE_STORAGE_EUR_PER_GB_HOUR),
            estimated_eur_month: Some(estimate_monthly_storage_eur(
                size_gb,
                SCW_FILE_STORAGE_EUR_PER_GB_HOUR,
            )),
            pricing_label: format!("€{SCW_FILE_STORAGE_EUR_PER_GB_HOUR:.6}/GB/hour"),
            pricing_note:
                "Scaleway File Storage public price (~€0.0803/GB/month incl. public-beta discount), fr-par only, before tax."
                    .into(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            tags: self.tags,
            billable: true,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScwSqlDatabasesEnvelope {
    #[serde(default)]
    databases: Vec<ScwSqlDatabase>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwSqlDatabase {
    id: String,
    name: String,
    #[serde(default)]
    status: Option<String>,
    // Raw API uses `cpu_min`/`cpu_max`; Terraform/Pulumi expose `min_cpu`/`max_cpu`.
    #[serde(default, alias = "min_cpu")]
    cpu_min: Option<u32>,
    #[serde(default, alias = "max_cpu")]
    cpu_max: Option<u32>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ScwSqlDatabase {
    fn into_summary(self, region: &str, project: &ScwProject) -> ScalewayResourceSummary {
        let cpu_min = self.cpu_min;
        let cpu_max = self.cpu_max;
        let purpose = format!(
            "Serverless SQL database, autoscale {}-{} vCPU.",
            cpu_min.unwrap_or(0),
            cpu_max.unwrap_or(0)
        );
        let oracle_query =
            scaleway_oracle_query(&self.name, "Serverless SQL", None, None, &[], &purpose);

        ScalewayResourceSummary {
            id: self.id,
            name: self.name,
            resource_type: "Serverless SQL".into(),
            region: region.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: normalize_scaleway_state(self.status.as_deref().unwrap_or("unknown")),
            commercial_type: None,
            runtime: None,
            min_scale: cpu_min,
            max_scale: cpu_max,
            domain_name: None,
            endpoint: self.endpoint,
            privacy: None,
            purpose,
            purpose_source: "plan".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: self.created_at,
            updated_at: self.updated_at,
            oracle_query,
            available_actions: Vec::new(),
            idle_cost_risk: cpu_min.unwrap_or(0) > 0,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScwGenerativeModelsEnvelope {
    #[serde(default)]
    data: Vec<ScwGenerativeModel>,
}

#[derive(Debug, Deserialize)]
struct ScwGenerativeModel {
    // ROBUSTNESS: a single `data[]` element missing `id` must not fail the whole
    // `Vec<ScwGenerativeModel>` deserialization (that would drop ALL models).
    // Default to an empty string here and filter empty-id models out after
    // deserialization (see `fetch_scaleway_generative_models`).
    #[serde(default)]
    id: String,
}

impl ScwGenerativeModel {
    fn into_summary(self, project: &ScwProject) -> ScalewayResourceSummary {
        let purpose = format!("Scaleway Generative API model {}.", self.id);
        let oracle_query =
            scaleway_oracle_query(&self.id, "Generative API Model", None, None, &[], &purpose);

        ScalewayResourceSummary {
            id: self.id.clone(),
            name: self.id,
            resource_type: "Generative API Model".into(),
            // Generative APIs are served from fr-par today and are not project-scoped
            // at the inventory layer; we still tag the pinned project for grouping.
            region: "fr-par".into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: "available".into(),
            commercial_type: None,
            runtime: None,
            min_scale: None,
            max_scale: None,
            domain_name: None,
            endpoint: None,
            privacy: None,
            purpose,
            purpose_source: "plan".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: None,
            updated_at: None,
            oracle_query,
            available_actions: Vec::new(),
            idle_cost_risk: false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScwServer {
    id: String,
    name: String,
    state: String,
    #[serde(default)]
    commercial_type: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    image: Option<ScwServerImage>,
    #[serde(default)]
    public_ip: Option<ScwPublicIp>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScwServerImage {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScwPublicIp {
    #[serde(default)]
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScwNamespacesEnvelope {
    #[serde(default)]
    namespaces: Vec<ScwNamespace>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwNamespace {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ScwFunctionsEnvelope {
    #[serde(default)]
    functions: Vec<ScwFunction>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwFunction {
    id: String,
    name: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    min_scale: Option<u32>,
    #[serde(default)]
    max_scale: Option<u32>,
    #[serde(default)]
    domain_name: Option<String>,
    #[serde(default)]
    privacy: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ScwFunction {
    fn into_summary(self, region: &str, project: &ScwProject) -> ScalewayResourceSummary {
        let min_scale = self.min_scale;
        let purpose = scaleway_function_purpose(
            &self.tags,
            self.description.as_deref(),
            self.runtime.as_deref(),
            min_scale,
            self.max_scale,
            &self.name,
        );
        let oracle_query = scaleway_oracle_query(
            &self.name,
            "Serverless",
            self.runtime.as_deref(),
            None,
            &self.tags,
            &purpose.0,
        );

        ScalewayResourceSummary {
            id: self.id,
            name: self.name,
            resource_type: "Serverless".into(),
            region: region.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: normalize_scaleway_state(self.status.as_deref().unwrap_or("unknown")),
            commercial_type: None,
            runtime: self.runtime,
            min_scale,
            max_scale: self.max_scale,
            domain_name: self.domain_name,
            endpoint: None,
            privacy: self.privacy,
            purpose: purpose.0,
            purpose_source: purpose.1,
            tags: self.tags,
            image: None,
            public_ip: None,
            created_at: self.created_at,
            updated_at: self.updated_at,
            oracle_query,
            available_actions: vec!["deploy".into()],
            idle_cost_risk: min_scale.unwrap_or(0) > 0,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScwContainersEnvelope {
    #[serde(default)]
    containers: Vec<ScwContainer>,
    #[serde(default)]
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScwContainer {
    id: String,
    name: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    min_scale: Option<u32>,
    #[serde(default)]
    max_scale: Option<u32>,
    #[serde(default)]
    memory_limit_bytes: Option<u64>,
    #[serde(default)]
    mvcpu_limit: Option<u32>,
    #[serde(default)]
    privacy: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    public_endpoint: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl ScwContainer {
    fn into_summary(self, region: &str, project: &ScwProject) -> ScalewayResourceSummary {
        let min_scale = self.min_scale;
        let runtime = self
            .protocol
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|protocol| format!("container/{protocol}"))
            .unwrap_or_else(|| "container".into());
        let commercial_type = scaleway_container_plan(self.mvcpu_limit, self.memory_limit_bytes);
        let purpose = scaleway_function_purpose(
            &self.tags,
            self.description.as_deref(),
            Some(&runtime),
            min_scale,
            self.max_scale,
            &self.name,
        );
        let oracle_query = scaleway_oracle_query(
            &self.name,
            "Serverless",
            commercial_type.as_deref().or(Some(runtime.as_str())),
            self.image.as_deref(),
            &self.tags,
            &purpose.0,
        );

        ScalewayResourceSummary {
            id: self.id,
            name: self.name,
            resource_type: "Serverless".into(),
            region: region.into(),
            project_id: Some(project.id.clone()),
            project_name: Some(project.name.clone()),
            state: normalize_scaleway_state(self.status.as_deref().unwrap_or("unknown")),
            commercial_type,
            runtime: Some(runtime),
            min_scale,
            max_scale: self.max_scale,
            domain_name: self.public_endpoint,
            endpoint: None,
            privacy: self.privacy,
            purpose: purpose.0,
            purpose_source: purpose.1,
            tags: self.tags,
            image: self.image,
            public_ip: None,
            created_at: self.created_at,
            updated_at: self.updated_at,
            oracle_query,
            available_actions: vec!["deploy".into()],
            idle_cost_risk: min_scale.unwrap_or(0) > 0,
        }
    }
}

fn scaleway_server_summary(
    server: ScwServer,
    zone: &str,
    project: &ScwProject,
) -> ScalewayResourceSummary {
    let commercial = server.commercial_type.clone().unwrap_or_default();
    let lower = format!("{} {}", server.name, commercial).to_lowercase();
    let resource_type = if lower.contains("gpu") {
        "GPU"
    } else {
        "CPU VM"
    };
    let image = server
        .image
        .as_ref()
        .map(|image| image.name.clone().unwrap_or_else(|| image.id.clone()));
    let public_ip = server
        .public_ip
        .as_ref()
        .and_then(|public_ip| public_ip.address.clone());
    let purpose = scaleway_server_purpose(
        &server.tags,
        &server.name,
        resource_type,
        server.commercial_type.as_deref(),
        image.as_deref(),
    );
    let oracle_query = scaleway_oracle_query(
        &server.name,
        resource_type,
        server.commercial_type.as_deref(),
        image.as_deref(),
        &server.tags,
        &purpose.0,
    );
    let idle_cost_risk = server.state == "running" && lower.contains("idle");

    ScalewayResourceSummary {
        id: server.id,
        name: server.name,
        resource_type: resource_type.into(),
        region: zone.into(),
        project_id: Some(project.id.clone()),
        project_name: Some(project.name.clone()),
        state: normalize_scaleway_state(&server.state),
        commercial_type: server.commercial_type,
        runtime: None,
        min_scale: None,
        max_scale: None,
        domain_name: None,
        endpoint: None,
        privacy: None,
        purpose: purpose.0,
        purpose_source: purpose.1,
        tags: server.tags,
        image,
        public_ip,
        created_at: server.created_at,
        updated_at: server.updated_at,
        oracle_query,
        available_actions: Vec::new(),
        idle_cost_risk,
    }
}

fn scaleway_tag_purpose(tags: &[String]) -> Option<String> {
    tags.iter().find_map(|tag| {
        let trimmed = tag.trim();
        let lower = trimmed.to_ascii_lowercase();
        for prefix in ["purpose:", "work:", "role:"] {
            if lower.starts_with(prefix) {
                let value = trimmed[prefix.len()..].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    })
}

fn scaleway_server_purpose(
    tags: &[String],
    name: &str,
    resource_type: &str,
    commercial_type: Option<&str>,
    image: Option<&str>,
) -> (String, String) {
    if let Some(purpose) = scaleway_tag_purpose(tags) {
        return (purpose, "tag".into());
    }

    if commercial_type.is_some() || image.is_some() {
        let mut parts = vec![resource_type.to_string()];
        if let Some(commercial_type) = commercial_type.filter(|value| !value.trim().is_empty()) {
            parts.push(commercial_type.to_string());
        }
        if let Some(image) = image.filter(|value| !value.trim().is_empty()) {
            parts.push(format!("running image {image}"));
        }
        return (format!("{}.", parts.join(" ")), "plan".into());
    }

    (
        format!(
            "Resource inferred from name: {}.",
            name.replace(['-', '_'], " ")
        ),
        "name".into(),
    )
}

fn scaleway_function_purpose(
    tags: &[String],
    description: Option<&str>,
    runtime: Option<&str>,
    min_scale: Option<u32>,
    max_scale: Option<u32>,
    name: &str,
) -> (String, String) {
    if let Some(purpose) = scaleway_tag_purpose(tags) {
        return (purpose, "tag".into());
    }

    if let Some(description) = description.map(str::trim).filter(|value| !value.is_empty()) {
        return (description.into(), "description".into());
    }

    if let Some(runtime) = runtime.map(str::trim).filter(|value| !value.is_empty()) {
        let min = min_scale.unwrap_or(0);
        let max = max_scale
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".into());
        return (
            format!("Serverless function running {runtime}, scale {min}-{max}."),
            "runtime".into(),
        );
    }

    (
        format!(
            "Resource inferred from name: {}.",
            name.replace(['-', '_'], " ")
        ),
        "name".into(),
    )
}

fn scaleway_container_plan(
    mvcpu_limit: Option<u32>,
    memory_limit_bytes: Option<u64>,
) -> Option<String> {
    match (mvcpu_limit, memory_limit_bytes.and_then(bytes_to_mebibytes)) {
        (Some(cpu), Some(memory_mb)) => Some(format!("{cpu} mCPU / {memory_mb} MB")),
        (Some(cpu), None) => Some(format!("{cpu} mCPU")),
        (None, Some(memory_mb)) => Some(format!("{memory_mb} MB")),
        (None, None) => None,
    }
}

fn bytes_to_mebibytes(bytes: u64) -> Option<u64> {
    if bytes == 0 {
        return None;
    }
    Some(bytes / 1_048_576)
}

fn scaleway_oracle_query(
    name: &str,
    resource_type: &str,
    plan_or_runtime: Option<&str>,
    image: Option<&str>,
    tags: &[String],
    purpose: &str,
) -> String {
    [name, resource_type, purpose]
        .into_iter()
        .chain(plan_or_runtime)
        .chain(image)
        .chain(tags.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_scaleway_state(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "running" | "ready" | "created" => "running",
        // "available" is a coherent idle/detached state (Block volumes, Generative
        // models, Object storage). It is deliberately NOT folded into "running" (that
        // mislabels an idle/detached volume) nor the "unknown" catch-all. It passes
        // through so the UI can render a neutral "Available" badge.
        "available" => "available",
        "stopped" | "stopped in place" | "stopped_in_place" => "stopped",
        "booting" | "starting" | "stopping" | "creating" | "pending" | "provisioning" => {
            "provisioning"
        }
        "deleting" | "deleted" | "error" | "locked" => "error",
        "unknown" | "" => "unknown",
        _ => "unknown",
    }
    .into()
}

/// PURE: decides the hard-error "all core APIs failed" gate. Keys ONLY on the
/// core project-token (`X-Auth-Token`) APIs — Instances, Functions, Containers,
/// Block, File Storage and Serverless SQL. Object Storage (SigV4) and the
/// Generative API (Bearer) are ANCILLARY: their success must never mask a total
/// core-API auth failure, otherwise a token with only generative permission
/// returns a misleadingly empty-but-"synced" inventory instead of a clear error.
fn scaleway_core_sync_failed(core_request_count: usize, core_success_count: usize) -> bool {
    core_request_count > 0 && core_success_count == 0
}

/// PURE: builds the "Serverless SQL inventory partial" risk flag, or `None` when
/// no SQL lookup failed. Mirrors the Generative API partial-risk shape.
fn scaleway_sql_partial_risk(sql_failure_count: usize) -> Option<RiskFlag> {
    if sql_failure_count == 0 {
        return None;
    }
    Some(RiskFlag {
        id: "scaleway_serverless_sql_inventory_partial".into(),
        severity: "medium".into(),
        title: "Scaleway Serverless SQL inventory partial".into(),
        description: format!(
            "{sql_failure_count} Serverless SQL database listing request(s) failed. The database list may be incomplete until the project token and IAM permissions are verified."
        ),
        source: "Scaleway".into(),
        timestamp: now(),
    })
}

/// PURE: builds the "File Storage inventory partial" risk flag, or `None` when no
/// File Storage lookup failed. File Storage is fr-par-only / optional; a failure
/// here must NOT be reported as a Block Storage failure (its own wording + id) and
/// must NOT flip the provider to "degraded".
fn scaleway_file_partial_risk(file_failure_count: usize) -> Option<RiskFlag> {
    if file_failure_count == 0 {
        return None;
    }
    Some(RiskFlag {
        id: "scaleway_file_storage_inventory_partial".into(),
        severity: "medium".into(),
        title: "Scaleway File Storage inventory partial".into(),
        description: format!(
            "{file_failure_count} File Storage filesystem listing request(s) failed. File Storage is fr-par-only today; the filesystem list may be incomplete until the project token and IAM permissions are verified."
        ),
        source: "Scaleway".into(),
        timestamp: now(),
    })
}

/// The per-domain failure tallies accumulated during a Scaleway sync. Split out so
/// the status/message decision is a PURE, unit-tested function rather than inline
/// async logic. Only the genuinely-degrading domains (`failure_count` for core
/// inventory, `action_failure_count`, Block `storage_failure_count`, and
/// `object_storage_failure_count`) flip the provider to "degraded". File Storage,
/// Serverless SQL and the Generative API are fr-par-only / optional beta products:
/// a 404 on an account that does not use them must surface its OWN risk flag but
/// must NOT degrade the provider or pollute the partial-sync message.
#[derive(Debug, Default, Clone, Copy)]
struct ScalewayInventoryCounters {
    request_count: usize,
    success_count: usize,
    /// Generic core-inventory failures (instances/functions/containers). Degrades.
    failure_count: usize,
    /// Instance action lookups. Degrades.
    action_failure_count: usize,
    /// Block Storage volume/snapshot lookups. Degrades.
    storage_failure_count: usize,
    /// Object Storage (S3 SigV4) listings. Degrades.
    object_storage_failure_count: usize,
    /// File Storage (fr-par-only). Own risk only; NON-degrading.
    file_failure_count: usize,
    /// Serverless SQL (fr-par-only). Own risk only; NON-degrading.
    sql_failure_count: usize,
    /// Generative API (Bearer, optional). Own risk only; NON-degrading.
    generative_api_failure_count: usize,
}

impl ScalewayInventoryCounters {
    /// True when ANY degrading domain reported a failure. File/SQL/Generative are
    /// deliberately excluded.
    fn is_degraded(&self) -> bool {
        self.failure_count > 0
            || self.action_failure_count > 0
            || self.storage_failure_count > 0
            || self.object_storage_failure_count > 0
    }
}

/// PURE: decide the provider `status` and partial-sync `message` from the per-domain
/// counters. Only degrading domains gate `status`/`message`; File/SQL/Generative are
/// reported in their own risk flags (built separately) and never appear here. Tested
/// directly so the misattribution / false-degraded regressions cannot return.
fn scaleway_inventory_status(
    counters: &ScalewayInventoryCounters,
) -> (&'static str, Option<String>) {
    if !counters.is_degraded() {
        return ("healthy", None);
    }
    let ScalewayInventoryCounters {
        request_count,
        success_count,
        failure_count,
        action_failure_count,
        storage_failure_count,
        object_storage_failure_count,
        file_failure_count,
        sql_failure_count,
        generative_api_failure_count,
    } = *counters;
    let message = format!(
        "Scaleway inventory partially synced: {success_count}/{request_count} request(s) succeeded, {failure_count} inventory request(s) failed, {action_failure_count} action lookup(s) failed, {storage_failure_count} Block Storage lookup(s) failed, {object_storage_failure_count} Object Storage lookup(s) failed. Optional fr-par-only products (non-degrading): {file_failure_count} File Storage, {sql_failure_count} Serverless SQL, {generative_api_failure_count} Generative API lookup(s) failed."
    );
    ("degraded", Some(message))
}

/// PURE: idle-cost-risk description text, branched per resource type. Serverless
/// SQL reserves a minimum CPU and bills even without queries (distinct from a
/// Serverless function staying warm via `min_scale`), so it gets its own copy.
fn scaleway_idle_risk_description(resource_type: &str, name: &str) -> String {
    match resource_type {
        "Serverless SQL" => {
            format!("{name} reserves a minimum CPU and bills even without queries.")
        }
        "Serverless" => {
            format!("{name} has min_scale > 0 and may stay warm even without traffic.")
        }
        _ => format!("{name} is running and marked as idle."),
    }
}

fn normalize_scaleway_project_name(name: &str) -> String {
    name.trim()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase()
}

#[cfg(not(test))]
fn configured_scaleway_project_id() -> Option<String> {
    std::env::var("ASPIS_SCALEWAY_PROJECT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
fn configured_scaleway_project_id() -> Option<String> {
    None
}

fn select_scaleway_project(
    projects: &[ScwProject],
    pinned_project_id: Option<&str>,
) -> Result<ScwProject, String> {
    let target = normalize_scaleway_project_name(SCW_TARGET_PROJECT_NAME);
    if let Some(project_id) = pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(configured_scaleway_project_id)
    {
        let project = projects
            .iter()
            .find(|project| project.id == project_id)
            .cloned()
            .ok_or_else(|| {
                "Configured Scaleway Aspis Bio project id was not visible to this token."
                    .to_string()
            })?;
        if normalize_scaleway_project_name(&project.name) != target {
            return Err(format!(
                "Pinned Scaleway project '{}' is visible, but it is not '{SCW_TARGET_PROJECT_NAME}'. Refusing to show non-Aspis Bio inventory.",
                project.name
            ));
        }
        return Ok(project);
    }

    let matches = projects
        .iter()
        .filter(|project| normalize_scaleway_project_name(&project.name) == target)
        .cloned()
        .collect::<Vec<_>>();

    match matches.len() {
        1 => Ok(matches[0].clone()),
        len if len > 1 => Err(format!(
            "Multiple Scaleway projects matched '{SCW_TARGET_PROJECT_NAME}'. Set ASPIS_SCALEWAY_PROJECT_ID to pin the Aspis Bio project."
        )),
        _ => Err({
            format!(
                "Scaleway project '{SCW_TARGET_PROJECT_NAME}' was not found. Refusing to show default-project inventory."
            )
        }),
    }
}

fn scaleway_selection_source(
    pinned_project_id: Option<&str>,
    env_project_id: Option<&str>,
) -> String {
    if pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        || env_project_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
    {
        "pinned".into()
    } else {
        "name_match".into()
    }
}

fn configured_or_pinned_scaleway_project_id(pinned_project_id: Option<&str>) -> Option<String> {
    pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(configured_scaleway_project_id)
}

async fn scaleway_project_from_api_key(
    http: &reqwest::Client,
    token: &str,
    access_key: Option<&str>,
    pinned_project_id: Option<&str>,
) -> Result<ScwProject, String> {
    let project_id =
        configured_or_pinned_scaleway_project_id(pinned_project_id).ok_or_else(|| {
            "Scaleway project list was not readable and no Aspis Bio project id is pinned."
                .to_string()
        })?;
    let access_key = access_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Scaleway project list was not readable; save the API access key so the pinned Aspis Bio project can be verified.".to_string()
        })?;
    // B2: the access key is in the URL path; reqwest's error Display echoes the
    // URL, so use static messages here (no `{e}` / URL interpolation) to avoid
    // leaking the access key into the error surface.
    let info: ScwApiKeyInfo = http
        .get(format!("{SCW_API}/iam/v1alpha1/api-keys/{access_key}"))
        .header("X-Auth-Token", token)
        .send()
        .await
        .map_err(|_| "Scaleway API key self-check request failed.".to_string())?
        .error_for_status()
        .map_err(|_| "Scaleway API key self-check rejected.".to_string())?
        .json()
        .await
        .map_err(|_| "Scaleway API key self-check response was invalid.".to_string())?;
    match info.default_project_id.as_deref() {
        Some(default_project_id) if default_project_id == project_id => Ok(ScwProject {
            id: project_id,
            name: SCW_TARGET_PROJECT_NAME.into(),
        }),
        Some(_) => Err(
            "Scaleway API key default project does not match the pinned Aspis Bio project.".into(),
        ),
        None => Err("Scaleway API key has no default project to verify against Aspis Bio.".into()),
    }
}

/// True when `value` is a canonical UUID (8-4-4-4-12 lowercase/uppercase hex
/// with dashes), the shape of every Scaleway project/organization id. Used to
/// gate ids before they are interpolated into a URL path.
pub fn scaleway_uuid_is_valid(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for &len in &groups {
        match parts.next() {
            Some(part) if part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

/// Resolves the Scaleway organization id for billing, which is
/// ORGANIZATION-scoped while the app pins only a project id.
///
/// `ASPIS_SCALEWAY_ORG_ID` (if set and non-empty) wins so an operator can pin
/// the org explicitly. Otherwise we look the org id up from the selected project
/// via `GET /account/v3/projects/{project_id}`, which returns `organization_id`
/// on the project object. Returns `Ok(None)` (never an error) when the env is
/// unset and the lookup cannot produce an id, so the caller degrades to an
/// `unreadable` billing view rather than hard-failing. The `X-Auth-Token` goes
/// in the header (never the URL); `project_id` is a server-issued UUID and the
/// only path-interpolated value.
pub async fn resolve_scaleway_org_id(
    http: &reqwest::Client,
    token: &str,
    project_id: &str,
) -> Option<String> {
    if let Ok(env_org) = std::env::var("ASPIS_SCALEWAY_ORG_ID") {
        let env_org = env_org.trim();
        if !env_org.is_empty() {
            // A set-but-invalid override is treated as unresolvable (return
            // `None`) so the command degrades to a clear `unreadable` message
            // rather than sending a garbage org id to the billing API.
            return scaleway_uuid_is_valid(env_org).then(|| env_org.to_string());
        }
    }
    let project_id = project_id.trim();
    // Defense-in-depth: only a UUID-shaped id (8-4-4-4-12 hex) reaches the URL
    // path, so a malformed/hostile pinned value cannot inject into the request.
    if !scaleway_uuid_is_valid(project_id) {
        return None;
    }
    let payload = http
        .get(format!("{SCW_API}/account/v3/projects/{project_id}"))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_BILLING_TIMEOUT_SECS))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    // Only a UUID-shaped id may reach the billing URL; a malformed API value is
    // rejected so it cannot be interpolated into a request.
    string_field(&payload, &["organization_id"]).filter(|id| scaleway_uuid_is_valid(id))
}

fn scaleway_has_next_page(page: u32, item_count: usize, total_count: Option<usize>) -> bool {
    if page >= SCW_MAX_PAGES {
        return false;
    }
    match total_count {
        Some(total) => (page as usize) * SCW_PAGE_SIZE < total,
        None => item_count == SCW_PAGE_SIZE,
    }
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000_000.0
}

fn estimate_monthly_storage_eur(size_gb: f64, price_eur_per_gb_hour: f64) -> f64 {
    size_gb * price_eur_per_gb_hour * SCW_MONTHLY_HOURS
}

fn scaleway_servers_url(zone: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/instance/v1/zones/{zone}/servers?project={}&page={page}&per_page={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id),
    )
}

fn scaleway_server_products_url(zone: &str) -> String {
    format!("{SCW_API}/instance/v1/zones/{zone}/products/servers")
}

fn scaleway_server_availability_url(zone: &str) -> String {
    format!("{SCW_API}/instance/v1/zones/{zone}/products/servers/availability")
}

fn scaleway_block_volumes_url(zone: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/block/v1/zones/{zone}/volumes?project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id),
    )
}

fn scaleway_block_snapshots_url(zone: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/block/v1/zones/{zone}/snapshots?project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id),
    )
}

fn scaleway_server_actions_url(zone: &str, server_id: &str) -> String {
    format!(
        "{SCW_API}/instance/v1/zones/{zone}/servers/{}/action",
        urlencoding::encode(server_id)
    )
}

fn scaleway_server_url(zone: &str, server_id: &str) -> String {
    format!(
        "{SCW_API}/instance/v1/zones/{zone}/servers/{}",
        urlencoding::encode(server_id)
    )
}

fn scaleway_server_delete_url(zone: &str, server_id: &str, force_shutdown: bool) -> String {
    let mut url = format!(
        "{}?with_volumes=all&with_ip=true",
        scaleway_server_url(zone, server_id),
    );
    if force_shutdown {
        url.push_str("&force_shutdown=true");
    }
    url
}

fn scaleway_volume_delete_url(zone: &str, volume_id: &str) -> String {
    format!(
        "{SCW_API}/instance/v1/zones/{zone}/volumes/{}",
        urlencoding::encode(volume_id)
    )
}

// ---------------------------------------------------------------------------
// Storage CRUD (Block / File Storage / Object Storage).
//
// Block Storage and File Storage are project-token (`X-Auth-Token`) APIs; their
// URL builders mirror the read-path builders above so the zone/region path
// segment stays consistent with the inventory that proves the resource exists.
// Object Storage is S3 SigV4 and reuses `scaleway_s3_authorization`.
//
// Endpoint shapes confirmed against the official Scaleway docs:
//   - Block:  https://www.scaleway.com/en/developers/api/block/
//   - File:   https://www.scaleway.com/en/developers/api/file-storage/
//   - Object: S3-compatible (AWS PutBucketLifecycleConfiguration + Scaleway
//             Object Storage lifecycle docs; Content-MD5 is REQUIRED by Scaleway).
// ---------------------------------------------------------------------------

/// Allowed Block Storage IOPS classes (Scaleway: exactly 5000 or 15000).
const SCW_BLOCK_ALLOWED_PERF_IOPS: &[u32] = &[5_000, 15_000];

fn scaleway_block_create_volume_url(zone: &str) -> String {
    format!(
        "{SCW_API}/block/v1/zones/{}/volumes",
        urlencoding::encode(zone)
    )
}

fn scaleway_block_volume_url(zone: &str, volume_id: &str) -> String {
    format!(
        "{SCW_API}/block/v1/zones/{}/volumes/{}",
        urlencoding::encode(zone),
        urlencoding::encode(volume_id)
    )
}

fn scaleway_block_create_snapshot_url(zone: &str) -> String {
    format!(
        "{SCW_API}/block/v1/zones/{}/snapshots",
        urlencoding::encode(zone)
    )
}

fn scaleway_block_snapshot_url(zone: &str, snapshot_id: &str) -> String {
    format!(
        "{SCW_API}/block/v1/zones/{}/snapshots/{}",
        urlencoding::encode(zone),
        urlencoding::encode(snapshot_id)
    )
}

fn scaleway_file_create_url(region: &str) -> String {
    format!(
        "{SCW_API}/file/v1alpha1/regions/{}/filesystems",
        urlencoding::encode(region)
    )
}

fn scaleway_file_url(region: &str, filesystem_id: &str) -> String {
    format!(
        "{SCW_API}/file/v1alpha1/regions/{}/filesystems/{}",
        urlencoding::encode(region),
        urlencoding::encode(filesystem_id)
    )
}

/// PURE: validate the requested Block Storage IOPS class. Scaleway accepts only
/// 5000 or 15000; anything else is rejected before any network call.
pub fn scaleway_block_perf_iops_is_valid(perf_iops: u32) -> bool {
    SCW_BLOCK_ALLOWED_PERF_IOPS.contains(&perf_iops)
}

/// PURE: Block create-volume body. `size_bytes` is the empty-volume size in
/// bytes (Scaleway `from_empty.size`). `perf_iops` MUST be one of 5000/15000 —
/// callers validate with `scaleway_block_perf_iops_is_valid` first.
fn scaleway_block_create_volume_body(
    name: &str,
    project_id: &str,
    size_bytes: u64,
    perf_iops: u32,
    tags: &[String],
) -> Value {
    json!({
        "name": name,
        "project_id": project_id,
        "perf_iops": perf_iops,
        "from_empty": { "size": size_bytes },
        "tags": tags,
    })
}

/// PURE: Block resize (PATCH) body. Only `size` is sent.
fn scaleway_block_resize_body(size_bytes: u64) -> Value {
    json!({ "size": size_bytes })
}

/// PURE: Block create-snapshot body.
fn scaleway_block_create_snapshot_body(
    name: &str,
    project_id: &str,
    volume_id: &str,
    tags: &[String],
) -> Value {
    json!({
        "name": name,
        "project_id": project_id,
        "volume_id": volume_id,
        "tags": tags,
    })
}

/// PURE: File Storage create body. `size_bytes` is the filesystem size in bytes.
fn scaleway_file_create_body(
    name: &str,
    project_id: &str,
    size_bytes: u64,
    tags: &[String],
) -> Value {
    json!({
        "name": name,
        "project_id": project_id,
        "size": size_bytes,
        "tags": tags,
    })
}

/// PURE: REFUSE a Block Storage shrink. Returns Ok only when `new_size_bytes` is
/// greater than or equal to the current size. A resize to the SAME size is a
/// no-op and allowed; a smaller size is rejected (Block Storage cannot shrink).
pub fn scaleway_block_resize_is_allowed(
    current_size_bytes: u64,
    new_size_bytes: u64,
) -> Result<(), String> {
    if new_size_bytes < current_size_bytes {
        return Err(
            "Block Storage volumes cannot be shrunk. The new size must be at least the current size."
                .into(),
        );
    }
    Ok(())
}

// --- Object Storage (S3 SigV4) bucket + lifecycle mutations -----------------

fn scaleway_s3_bucket_url(host: &str, bucket: &str) -> String {
    // Path-style addressing (matches the read path's list/usage calls).
    format!("https://{host}/{}", urlencoding::encode(bucket))
}

/// PURE: minimal S3 lifecycle XML body. `rules` are pre-validated lifecycle
/// rules; each rule is rendered as `<Rule>` with ID, Status, optional Filter
/// Prefix, and an Expiration `<Days>`. Mirrors the AWS/Scaleway
/// PutBucketLifecycleConfiguration schema. The root carries the standard S3
/// namespace so strict parsers accept it.
fn scaleway_lifecycle_xml(rules: &[ScalewayLifecycleRule]) -> String {
    let mut out =
        String::from(r#"<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#);
    for rule in rules {
        out.push_str("<Rule>");
        out.push_str(&format!("<ID>{}</ID>", xml_escape(&rule.id)));
        // <Filter><Prefix>..</Prefix></Filter> — an empty prefix matches all
        // objects but the element must still be present and well-formed.
        out.push_str("<Filter><Prefix>");
        out.push_str(&xml_escape(&rule.prefix));
        out.push_str("</Prefix></Filter>");
        out.push_str(&format!(
            "<Status>{}</Status>",
            if rule.enabled { "Enabled" } else { "Disabled" }
        ));
        out.push_str(&format!(
            "<Expiration><Days>{}</Days></Expiration>",
            rule.expiration_days
        ));
        out.push_str("</Rule>");
    }
    out.push_str("</LifecycleConfiguration>");
    out
}

/// PURE: XML-escape the five predefined entities so a rule id/prefix cannot
/// break out of the element or inject markup.
fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// A single, pre-validated S3 lifecycle rule (expire-by-age). Kept intentionally
/// minimal to mirror the Cloudflare R2 lifecycle UX (prefix + age-based expiry).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScalewayLifecycleRule {
    id: String,
    prefix: String,
    enabled: bool,
    expiration_days: u32,
}

/// PURE: standard Base64 (RFC 4648) encode. Self-contained so no new crate is
/// pulled in for the single Content-MD5 header Scaleway's lifecycle PUT needs.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// PURE: RFC 1321 MD5 digest. Hand-rolled (no `md-5` crate is available and the
/// dependency set is fixed) solely to compute the Content-MD5 header that
/// Scaleway's Object Storage lifecycle PUT requires. Not used for any security
/// decision — integrity of the request is bound by the SigV4 payload hash.
fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// PURE: Base64(MD5(body)) — the value Scaleway requires in the `Content-MD5`
/// header for a lifecycle PUT.
fn md5_base64(body: &[u8]) -> String {
    base64_encode(&md5_digest(body))
}

/// S3 SigV4 Authorization for a request that ALSO signs `content-md5` (required
/// for the lifecycle PUT). Mirrors `scaleway_s3_authorization` exactly but adds
/// `content-md5` to both the canonical headers and the SignedHeaders list, in
/// the canonical (alphabetical) header order S3 requires.
#[allow(clippy::too_many_arguments)]
fn scaleway_s3_authorization_with_md5(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    host: &str,
    content_md5: &str,
    amz_date: &str,
    date_stamp: &str,
    region: &str,
    payload_hash: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<String, String> {
    let signed_headers = "content-md5;host;x-amz-content-sha256;x-amz-date";
    let canonical_headers = format!(
        "content-md5:{content_md5}\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        hex_sha256(&canonical_request)
    );
    let signing_key = aws4_signing_key(secret_key, date_stamp, region, "s3")?;
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes())?);
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    ))
}

fn scaleway_namespaces_url(region: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/functions/v1beta1/regions/{region}/namespaces?project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id)
    )
}

fn scaleway_functions_url(region: &str, namespace_id: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/functions/v1beta1/regions/{region}/functions?namespace_id={}&project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(namespace_id),
        urlencoding::encode(project_id)
    )
}

fn scaleway_containers_url(region: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/containers/v1beta1/regions/{region}/containers?project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id)
    )
}

fn scaleway_filesystems_url(region: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/file/v1alpha1/regions/{region}/filesystems?project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id)
    )
}

fn scaleway_sql_databases_url(region: &str, project_id: &str, page: u32) -> String {
    format!(
        "{SCW_API}/serverless-sqldb/v1alpha1/regions/{region}/databases?project_id={}&page={page}&page_size={SCW_PAGE_SIZE}",
        urlencoding::encode(project_id)
    )
}

// ---------------------------------------------------------------------------
// Serverless CRUD (Serverless SQL / Functions / Containers).
//
// All three are region-scoped `X-Auth-Token` APIs. The CRUD URL builders mirror
// the read-path builders above so the region path segment stays consistent with
// the inventory that proves the resource exists. Endpoint shapes + request body
// fields confirmed against the official Scaleway docs:
//   - SQL:        https://www.scaleway.com/en/developers/api/serverless-sql-databases/
//                 (Create requires organization_id + project_id + name + cpu_min + cpu_max)
//   - Functions:  https://www.scaleway.com/en/developers/api/serverless-functions/
//                 (Namespace create requires name + project_id; Function create
//                  requires name + namespace_id + runtime)
//   - Containers: https://www.scaleway.com/en/developers/api/serverless-containers/
//                 (Namespace create requires name + project_id; Container create
//                  requires name + namespace_id; registry_image references an
//                  EXISTING image — no image build is performed here)
//
// NOTE: there is intentionally NO Serverless SQL "query" action. A Serverless SQL
// database `endpoint` is a raw PostgreSQL DSN, so running SQL needs a Postgres
// wire-protocol client (a new crate / Cargo.toml dependency, which is off-limits
// for this change). The database `endpoint` is surfaced in the inventory summary
// so the frontend can offer "connect with psql" + a console link; query support
// is deferred pending a Postgres-client dependency.
// ---------------------------------------------------------------------------

fn scaleway_sql_create_url(region: &str) -> String {
    format!(
        "{SCW_API}/serverless-sqldb/v1alpha1/regions/{}/databases",
        urlencoding::encode(region)
    )
}

fn scaleway_sql_database_url(region: &str, database_id: &str) -> String {
    format!(
        "{SCW_API}/serverless-sqldb/v1alpha1/regions/{}/databases/{}",
        urlencoding::encode(region),
        urlencoding::encode(database_id)
    )
}

fn scaleway_function_namespace_create_url(region: &str) -> String {
    format!(
        "{SCW_API}/functions/v1beta1/regions/{}/namespaces",
        urlencoding::encode(region)
    )
}

fn scaleway_function_namespace_url(region: &str, namespace_id: &str) -> String {
    format!(
        "{SCW_API}/functions/v1beta1/regions/{}/namespaces/{}",
        urlencoding::encode(region),
        urlencoding::encode(namespace_id)
    )
}

fn scaleway_container_namespace_url(region: &str, namespace_id: &str) -> String {
    format!(
        "{SCW_API}/containers/v1beta1/regions/{}/namespaces/{}",
        urlencoding::encode(region),
        urlencoding::encode(namespace_id)
    )
}

fn scaleway_function_create_url(region: &str) -> String {
    format!(
        "{SCW_API}/functions/v1beta1/regions/{}/functions",
        urlencoding::encode(region)
    )
}

fn scaleway_function_url(region: &str, function_id: &str) -> String {
    format!(
        "{SCW_API}/functions/v1beta1/regions/{}/functions/{}",
        urlencoding::encode(region),
        urlencoding::encode(function_id)
    )
}

fn scaleway_container_namespace_create_url(region: &str) -> String {
    format!(
        "{SCW_API}/containers/v1beta1/regions/{}/namespaces",
        urlencoding::encode(region)
    )
}

fn scaleway_container_create_url(region: &str) -> String {
    format!(
        "{SCW_API}/containers/v1beta1/regions/{}/containers",
        urlencoding::encode(region)
    )
}

fn scaleway_container_url(region: &str, container_id: &str) -> String {
    format!(
        "{SCW_API}/containers/v1beta1/regions/{}/containers/{}",
        urlencoding::encode(region),
        urlencoding::encode(container_id)
    )
}

#[derive(Debug, PartialEq, Eq)]
struct ScalewayActionPlan {
    url: String,
    body: Value,
    display_action: &'static str,
}

#[derive(Serialize)]
struct ScalewayInstanceActionBody<'a> {
    action: &'a str,
}

fn scaleway_resource_action_plan(
    resource: &ScalewayResourceSummary,
    // VALIDATED location token from the caller; URL path segments use THIS value.
    region: &str,
    action: &str,
) -> Result<ScalewayActionPlan, String> {
    let action = action.trim().to_ascii_lowercase();
    if resource.id.trim().is_empty() {
        return Err("Scaleway resource id is missing.".into());
    }
    if region.trim().is_empty() || region.contains('/') || region.contains('\\') {
        return Err("Scaleway resource region is invalid.".into());
    }

    match resource.resource_type.as_str() {
        "GPU" | "CPU VM" => {
            let (api_action, display_action) = match action.as_str() {
                "start" => ("poweron", "start"),
                "stop" => ("poweroff", "stop"),
                "reboot" => ("reboot", "reboot"),
                "delete" | "terminate" => ("terminate", "delete"),
                _ => {
                    return Err(
                        "Scaleway Instance action must be start, stop, reboot, or delete.".into(),
                    );
                }
            };
            if !resource.available_actions.is_empty()
                && !resource
                    .available_actions
                    .iter()
                    .any(|available| available == api_action)
            {
                return Err(format!(
                    "Scaleway reports {} is not currently available for {}. Sync and retry.",
                    display_action, resource.name
                ));
            }
            Ok(ScalewayActionPlan {
                url: format!(
                    "{SCW_API}/instance/v1/zones/{}/servers/{}/action",
                    region,
                    urlencoding::encode(&resource.id)
                ),
                body: json!(ScalewayInstanceActionBody { action: api_action }),
                display_action,
            })
        }
        "Serverless" => {
            if action != "deploy" {
                return Err("Scaleway serverless action must be deploy.".into());
            }
            let product_path = if resource
                .runtime
                .as_deref()
                .map(|runtime| runtime.starts_with("container/"))
                .unwrap_or(false)
            {
                "containers/v1beta1"
            } else {
                "functions/v1beta1"
            };
            let resource_path = if product_path.starts_with("containers") {
                "containers"
            } else {
                "functions"
            };
            Ok(ScalewayActionPlan {
                url: format!(
                    "{SCW_API}/{product_path}/regions/{}/{resource_path}/{}/deploy",
                    region,
                    urlencoding::encode(&resource.id)
                ),
                body: json!({}),
                display_action: "deploy",
            })
        }
        _ => Err("Unsupported Scaleway resource type for operational actions.".into()),
    }
}

async fn fetch_scaleway_server_actions(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    server_id: &str,
) -> Result<Vec<String>, String> {
    let envelope: ScwServerActionsEnvelope = http
        .get(scaleway_server_actions_url(zone, server_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway server actions request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway server actions request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway server actions response was invalid: {e}"))?;

    let mut actions = envelope
        .actions
        .into_iter()
        .map(|action| action.trim().to_ascii_lowercase())
        .filter(|action| !action.is_empty())
        .collect::<Vec<_>>();
    actions.sort();
    actions.dedup();
    Ok(actions)
}

async fn fetch_scaleway_object_buckets(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project_id: &str,
    region: &str,
) -> Result<Vec<ScwS3Bucket>, String> {
    let host = format!("s3.{region}.scw.cloud");
    let url = format!("https://{host}/");
    let signed_access_key = format!("{}@{}", access_key.trim(), project_id.trim());
    let payload_hash = hex_sha256("");
    let date = Utc::now();
    let amz_date = date.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = date.format("%Y%m%d").to_string();
    let authorization = scaleway_s3_authorization(
        "GET",
        "/",
        "",
        &host,
        &amz_date,
        &date_stamp,
        region,
        &payload_hash,
        &signed_access_key,
        secret_key,
    )?;

    let xml = http
        .get(url)
        .header("Authorization", authorization)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", amz_date)
        .send()
        .await
        .map_err(|e| format!("Scaleway Object Storage bucket request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Object Storage bucket request rejected: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Scaleway Object Storage bucket response was invalid: {e}"))?;

    let parsed: ScwS3ListBucketsResult = from_str(&xml)
        .map_err(|e| format!("Scaleway Object Storage bucket XML was invalid: {e}"))?;
    Ok(parsed
        .buckets
        .map(|buckets| buckets.buckets)
        .unwrap_or_default())
}

async fn fetch_scaleway_object_bucket_usage(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project_id: &str,
    region: &str,
    bucket_name: &str,
) -> Result<ScwObjectBucketUsage, String> {
    let mut usage = ScwObjectBucketUsage::default();
    let mut continuation_token: Option<String> = None;

    for page in 1..=SCW_OBJECT_BUCKET_MAX_SCAN_PAGES {
        let host = format!("s3.{region}.scw.cloud");
        let encoded_bucket = urlencoding::encode(bucket_name);
        let canonical_uri = format!("/{encoded_bucket}");
        let mut canonical_query = format!("list-type=2&max-keys={SCW_OBJECT_BUCKET_PAGE_SIZE}");
        if let Some(token) = continuation_token.as_deref() {
            canonical_query.push_str("&continuation-token=");
            canonical_query.push_str(&urlencoding::encode(token));
        }
        let url = format!("https://{host}{canonical_uri}?{canonical_query}");
        let signed_access_key = format!("{}@{}", access_key.trim(), project_id.trim());
        let payload_hash = hex_sha256("");
        let date = Utc::now();
        let amz_date = date.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = date.format("%Y%m%d").to_string();
        let authorization = scaleway_s3_authorization(
            "GET",
            &canonical_uri,
            &canonical_query,
            &host,
            &amz_date,
            &date_stamp,
            region,
            &payload_hash,
            &signed_access_key,
            secret_key,
        )?;

        let xml = http
            .get(url)
            .header("Authorization", authorization)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .send()
            .await
            .map_err(|e| format!("Scaleway Object Storage object listing failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Scaleway Object Storage object listing rejected: {e}"))?
            .text()
            .await
            .map_err(|e| {
                format!("Scaleway Object Storage object listing response was invalid: {e}")
            })?;

        let parsed: ScwS3ListObjectsV2Result = from_str(&xml)
            .map_err(|e| format!("Scaleway Object Storage object listing XML was invalid: {e}"))?;
        usage.pages_scanned = page;
        for object in parsed.contents {
            let size = object.size.unwrap_or(0);
            usage.total_bytes = usage.total_bytes.saturating_add(size);
            usage.object_count += 1;
            match scaleway_object_storage_price_for_class(object.storage_class.as_deref()) {
                Some(price) => {
                    usage.estimated_eur_month +=
                        estimate_monthly_storage_eur(bytes_to_gb(size), price);
                }
                None => usage.has_unknown_storage_class = true,
            }
        }

        continuation_token = parsed.next_continuation_token;
        if parsed.is_truncated != Some(true) || continuation_token.is_none() {
            usage.partial = false;
            return Ok(usage);
        }
    }

    usage.partial = true;
    Ok(usage)
}

fn scaleway_object_storage_price_for_class(storage_class: Option<&str>) -> Option<f64> {
    match storage_class
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("STANDARD")
        .to_ascii_uppercase()
        .as_str()
    {
        "STANDARD" => Some(SCW_OBJECT_STANDARD_MULTI_AZ_EUR_PER_GB_HOUR),
        "ONEZONE_IA" | "ONEZONE" | "STANDARD_ONEZONE" | "STANDARD_ONE_ZONE" => {
            Some(SCW_OBJECT_STANDARD_ONE_ZONE_EUR_PER_GB_HOUR)
        }
        "GLACIER" => Some(SCW_OBJECT_GLACIER_EUR_PER_GB_HOUR),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn scaleway_s3_authorization(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    host: &str,
    amz_date: &str,
    date_stamp: &str,
    region: &str,
    payload_hash: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<String, String> {
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        hex_sha256(&canonical_request)
    );
    let signing_key = aws4_signing_key(secret_key, date_stamp, region, "s3")?;
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes())?);

    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    ))
}

fn aws4_signing_key(
    secret_key: &str,
    date_stamp: &str,
    region: &str,
    service: &str,
) -> Result<Vec<u8>, String> {
    let k_date = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    )?;
    let k_region = hmac_sha256(&k_date, region.as_bytes())?;
    let k_service = hmac_sha256(&k_region, service.as_bytes())?;
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| "Could not initialize S3 request signer.".to_string())?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hex_sha256(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

pub async fn perform_scaleway_resource_action_request(
    http: &reqwest::Client,
    token: &str,
    resource: &ScalewayResourceSummary,
    // VALIDATED location token (caller ran `validate_scaleway_location` on the
    // cached `resource.region`). All URL path segments are built from THIS value,
    // not the raw cached field, so the validation is load-bearing rather than a
    // discarded gate.
    region: &str,
    action: &str,
) -> Result<String, String> {
    let clean_action = action.trim().to_ascii_lowercase();
    if matches!(resource.resource_type.as_str(), "GPU" | "CPU VM")
        && matches!(clean_action.as_str(), "delete" | "terminate")
    {
        // C3: this destructive short-circuit bypasses scaleway_resource_action_plan,
        // so re-assert the available_actions membership here.
        assert_scaleway_terminate_available(resource)?;
        delete_scaleway_instance_with_volumes(http, token, resource, region).await?;
        return Ok("delete".into());
    }
    let plan = scaleway_resource_action_plan(resource, region, action)?;
    http.post(&plan.url)
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .json(&plan.body)
        .send()
        .await
        .map_err(|e| format!("Scaleway {} request failed: {e}", plan.display_action))?
        .error_for_status()
        .map_err(|e| format!("Scaleway {} request rejected: {e}", plan.display_action))?;
    Ok(plan.display_action.into())
}

/// C3: if the cached inventory reports available actions for this instance,
/// "terminate" must be among them before the destructive path proceeds.
fn assert_scaleway_terminate_available(resource: &ScalewayResourceSummary) -> Result<(), String> {
    if !resource.available_actions.is_empty()
        && !resource
            .available_actions
            .iter()
            .any(|available| available.eq_ignore_ascii_case("terminate"))
    {
        return Err(format!(
            "Scaleway reports terminate is not currently available for {}. Sync and retry.",
            resource.name
        ));
    }
    Ok(())
}

async fn delete_scaleway_instance_with_volumes(
    http: &reqwest::Client,
    token: &str,
    resource: &ScalewayResourceSummary,
    // VALIDATED location token from the caller; all server/volume URL path
    // segments use THIS value instead of the raw cached `resource.region`.
    region: &str,
) -> Result<(), String> {
    // C2: a FAILED volume lookup is NOT the same as "no volumes". Using
    // unwrap_or_default() here would silently orphan the instance's volumes (and
    // keep billing them) if the pre-delete inventory read failed. Abort instead,
    // so the caller can retry once the lookup succeeds.
    let volume_ids = fetch_scaleway_instance_volume_ids(http, token, resource, region)
        .await
        .map_err(|e| {
            format!(
                "Refusing to delete {}: could not confirm its volumes before deletion ({e}). \
Retry once Scaleway inventory is reachable so attached volumes are not orphaned.",
                resource.name
            )
        })?;
    let delete_url = scaleway_server_delete_url(region, &resource.id, true);
    let delete = http
        .delete(&delete_url)
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway delete request failed: {e}"))?;
    if delete.status().is_success() || delete.status().as_u16() == 404 {
        delete_scaleway_instance_volume_ids(http, token, region, &volume_ids).await?;
        return Ok(());
    }

    let action_url = scaleway_server_actions_url(region, &resource.id);
    let terminate = http
        .post(&action_url)
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .json(&json!(ScalewayInstanceActionBody {
            action: "terminate"
        }))
        .send()
        .await
        .map_err(|e| format!("Scaleway terminate fallback request failed: {e}"))?;
    if terminate.status().is_success() || terminate.status().as_u16() == 404 {
        delete_scaleway_instance_volume_ids(http, token, region, &volume_ids).await?;
        return Ok(());
    }

    let _ = http
        .post(&action_url)
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .json(&json!(ScalewayInstanceActionBody { action: "poweroff" }))
        .send()
        .await;
    let final_delete_url = scaleway_server_delete_url(region, &resource.id, false);
    http.delete(&final_delete_url)
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway delete-after-poweroff request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway delete-after-poweroff request rejected: {e}"))?;
    delete_scaleway_instance_volume_ids(http, token, region, &volume_ids).await?;
    Ok(())
}

async fn fetch_scaleway_instance_volume_ids(
    http: &reqwest::Client,
    token: &str,
    resource: &ScalewayResourceSummary,
    // VALIDATED location token from the caller; the server lookup URL uses THIS
    // value instead of the raw cached `resource.region`.
    region: &str,
) -> Result<Vec<String>, String> {
    let payload: Value = http
        .get(scaleway_server_url(region, &resource.id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_PRE_DELETE_LOOKUP_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway server volume lookup failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway server volume lookup rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway server volume lookup response was invalid: {e}"))?;
    Ok(scaleway_volume_ids_from_server_payload(&payload))
}

fn scaleway_volume_ids_from_server_payload(payload: &Value) -> Vec<String> {
    payload
        .get("server")
        .and_then(|server| server.get("volumes"))
        .and_then(Value::as_object)
        .map(|volumes| {
            volumes
                .values()
                .filter_map(|volume| volume.get("id").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

async fn delete_scaleway_instance_volume_ids(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    volume_ids: &[String],
) -> Result<(), String> {
    for volume_id in volume_ids {
        let response = http
            .delete(scaleway_volume_delete_url(zone, volume_id))
            .header("X-Auth-Token", token)
            .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| format!("Scaleway volume delete request failed: {e}"))?;
        if !(response.status().is_success() || response.status().as_u16() == 404) {
            response
                .error_for_status()
                .map_err(|e| format!("Scaleway volume delete request rejected: {e}"))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Storage mutation request functions. All inputs (names, ids, sizes, project,
// zone/region) are validated by the command layer BEFORE these are called. The
// project-token (`X-Auth-Token`) functions never log the token; the Object
// Storage functions reuse the SigV4 signer and never log the secret key.
// ---------------------------------------------------------------------------

/// Parameters for creating a Block Storage volume. `size_bytes` is the
/// empty-volume size; `perf_iops` is pre-validated (5000 or 15000).
pub struct ScalewayBlockCreateRequest<'a> {
    pub zone: &'a str,
    pub name: &'a str,
    pub project_id: &'a str,
    pub size_bytes: u64,
    pub perf_iops: u32,
    pub tags: &'a [String],
}

/// POST a Block Storage create-volume request. Returns the new volume id.
pub async fn create_scaleway_block_volume_request(
    http: &reqwest::Client,
    token: &str,
    req: &ScalewayBlockCreateRequest<'_>,
) -> Result<String, String> {
    if !scaleway_block_perf_iops_is_valid(req.perf_iops) {
        return Err("Block Storage IOPS class must be 5000 or 15000.".into());
    }
    let body = scaleway_block_create_volume_body(
        req.name,
        req.project_id,
        req.size_bytes,
        req.perf_iops,
        req.tags,
    );
    let payload: Value = http
        .post(scaleway_block_create_volume_url(req.zone))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Block create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Block create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Block create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// PATCH a Block Storage volume to a new size (bytes). Shrink refusal is enforced
/// by the caller via `scaleway_block_resize_is_allowed`.
pub async fn resize_scaleway_block_volume_request(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    volume_id: &str,
    new_size_bytes: u64,
) -> Result<(), String> {
    http.patch(scaleway_block_volume_url(zone, volume_id))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&scaleway_block_resize_body(new_size_bytes))
        .send()
        .await
        .map_err(|e| format!("Scaleway Block resize request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Block resize request rejected: {e}"))?;
    Ok(())
}

/// POST a Block Storage create-snapshot request. Returns the new snapshot id.
pub async fn create_scaleway_block_snapshot_request(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    name: &str,
    project_id: &str,
    volume_id: &str,
    tags: &[String],
) -> Result<String, String> {
    let body = scaleway_block_create_snapshot_body(name, project_id, volume_id, tags);
    let payload: Value = http
        .post(scaleway_block_create_snapshot_url(zone))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Block snapshot request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Block snapshot request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Block snapshot response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// DELETE a Block Storage volume. A 404 is treated as already-gone (idempotent).
pub async fn delete_scaleway_block_volume_request(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    volume_id: &str,
) -> Result<(), String> {
    let response = http
        .delete(scaleway_block_volume_url(zone, volume_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Block volume delete request failed: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    // Scaleway refuses to delete a volume that is still attached to an Instance
    // (typically 400/409/412). Surface that case explicitly so the user detaches
    // first instead of seeing a bare HTTP status.
    if matches!(response.status().as_u16(), 400 | 409 | 412) {
        return Err(
            "Block Storage volume delete was refused. The volume is likely still attached to an Instance — detach it (or delete the Instance) first, then retry."
                .into(),
        );
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway Block volume delete request rejected: {e}"))?;
    Ok(())
}

/// DELETE a Block Storage snapshot. A 404 is treated as already-gone.
pub async fn delete_scaleway_block_snapshot_request(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    snapshot_id: &str,
) -> Result<(), String> {
    let response = http
        .delete(scaleway_block_snapshot_url(zone, snapshot_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Block snapshot delete request failed: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway Block snapshot delete request rejected: {e}"))?;
    Ok(())
}

/// POST a File Storage create-filesystem request. Returns the new filesystem id.
pub async fn create_scaleway_filesystem_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    name: &str,
    project_id: &str,
    size_bytes: u64,
    tags: &[String],
) -> Result<String, String> {
    let body = scaleway_file_create_body(name, project_id, size_bytes, tags);
    let payload: Value = http
        .post(scaleway_file_create_url(region))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway File Storage create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway File Storage create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway File Storage create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// DELETE a File Storage filesystem. A 404 is treated as already-gone.
pub async fn delete_scaleway_filesystem_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    filesystem_id: &str,
) -> Result<(), String> {
    let response = http
        .delete(scaleway_file_url(region, filesystem_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway File Storage delete request failed: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway File Storage delete request rejected: {e}"))?;
    Ok(())
}

/// PURE: Serverless SQL create-database body. `organization_id` is REQUIRED by
/// the API (the database is org-scoped) alongside `project_id`; `cpu_min`/
/// `cpu_max` bound the autoscale range.
fn scaleway_sql_create_body(
    name: &str,
    organization_id: &str,
    project_id: &str,
    cpu_min: u32,
    cpu_max: u32,
) -> Value {
    json!({
        "name": name,
        "organization_id": organization_id,
        "project_id": project_id,
        "cpu_min": cpu_min,
        "cpu_max": cpu_max,
    })
}

/// PURE: Serverless Functions / Containers namespace create body. Both products
/// share the same minimal shape (`name` + `project_id`).
fn scaleway_namespace_create_body(name: &str, project_id: &str) -> Value {
    json!({ "name": name, "project_id": project_id })
}

/// PURE: Serverless Function create body. `namespace_id`, `name` and `runtime`
/// are the API-required fields; `memory_limit` and the scale bounds are sent only
/// when provided so we never pollute the request with nulls.
fn scaleway_function_create_body(
    namespace_id: &str,
    name: &str,
    runtime: &str,
    memory_limit: Option<u32>,
    min_scale: Option<u32>,
    max_scale: Option<u32>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("namespace_id".into(), json!(namespace_id));
    body.insert("name".into(), json!(name));
    body.insert("runtime".into(), json!(runtime));
    if let Some(memory) = memory_limit {
        body.insert("memory_limit".into(), json!(memory));
    }
    if let Some(min) = min_scale {
        body.insert("min_scale".into(), json!(min));
    }
    if let Some(max) = max_scale {
        body.insert("max_scale".into(), json!(max));
    }
    Value::Object(body)
}

/// PURE: Serverless Container create body. References an EXISTING `registry_image`
/// (no image build). `namespace_id` + `name` are the API-required fields; the
/// image, memory limit and scale bounds are sent only when provided.
fn scaleway_container_create_body(
    namespace_id: &str,
    name: &str,
    registry_image: &str,
    memory_limit: Option<u32>,
    min_scale: Option<u32>,
    max_scale: Option<u32>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("namespace_id".into(), json!(namespace_id));
    body.insert("name".into(), json!(name));
    body.insert("registry_image".into(), json!(registry_image));
    if let Some(memory) = memory_limit {
        body.insert("memory_limit".into(), json!(memory));
    }
    if let Some(min) = min_scale {
        body.insert("min_scale".into(), json!(min));
    }
    if let Some(max) = max_scale {
        body.insert("max_scale".into(), json!(max));
    }
    Value::Object(body)
}

/// Parameters for creating a Serverless SQL database. `cpu_min`/`cpu_max` are
/// pre-validated by the caller; `organization_id` is resolved separately because
/// the API is org-scoped while the app pins only a project.
pub struct ScalewaySqlCreateRequest<'a> {
    pub region: &'a str,
    pub name: &'a str,
    pub organization_id: &'a str,
    pub project_id: &'a str,
    pub cpu_min: u32,
    pub cpu_max: u32,
}

/// POST a Serverless SQL create-database request. Returns the new database id.
pub async fn create_scaleway_sql_database_request(
    http: &reqwest::Client,
    token: &str,
    req: &ScalewaySqlCreateRequest<'_>,
) -> Result<String, String> {
    let body = scaleway_sql_create_body(
        req.name,
        req.organization_id,
        req.project_id,
        req.cpu_min,
        req.cpu_max,
    );
    let payload: Value = http
        .post(scaleway_sql_create_url(req.region))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Serverless SQL create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Serverless SQL create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Serverless SQL create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// DELETE a Serverless SQL database. A 404 is treated as already-gone (idempotent).
pub async fn delete_scaleway_sql_database_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    database_id: &str,
) -> Result<(), String> {
    let response = http
        .delete(scaleway_sql_database_url(region, database_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Serverless SQL delete request failed: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway Serverless SQL delete request rejected: {e}"))?;
    Ok(())
}

/// Instance (compute server) create endpoint. Zone-scoped; the path segment is a
/// validated zone slug (`validate_scaleway_location`), so no encoding is needed,
/// but we encode defensively to match the established pattern.
fn scaleway_instance_create_url(zone: &str) -> String {
    format!(
        "{SCW_API}/instance/v1/zones/{}/servers",
        urlencoding::encode(zone)
    )
}

/// PURE: the exact body POSTed to create an Instance. The Scaleway Instance API
/// uses `project` (NOT `project_id`), plus `name`, `commercial_type`, `image`
/// (image id) and `dynamic_ip_required`. `tags` is sent ONLY when non-empty so we
/// never transmit an empty array the API could read as "clear all tags". This is
/// sent verbatim, so it is unit-tested against the API contract.
pub fn scaleway_instance_create_body(
    name: &str,
    project_id: &str,
    commercial_type: &str,
    image: &str,
    dynamic_ip_required: bool,
    tags: &[String],
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("name".into(), json!(name));
    // CRITICAL: the Instance API field is `project`, NOT `project_id`.
    body.insert("project".into(), json!(project_id));
    body.insert("commercial_type".into(), json!(commercial_type));
    body.insert("image".into(), json!(image));
    body.insert("dynamic_ip_required".into(), json!(dynamic_ip_required));
    if !tags.is_empty() {
        body.insert("tags".into(), json!(tags));
    }
    Value::Object(body)
}

/// Estimated cost of an Instance offer, looked up from the synced catalog. Both
/// fields are `None` (and `risk` is `Some`) when the offer is absent — never a
/// fabricated zero.
pub struct ScalewayInstanceOfferCost {
    pub hourly_eur: Option<f64>,
    pub monthly_eur: Option<f64>,
    pub risk: Option<String>,
}

/// PURE: look up the (zone, commercial_type) offer in the synced catalog and read
/// its hourly/monthly price. A missing offer yields `None` + a risk note so the
/// caller surfaces "cost unknown" rather than a misleading €0. Unit-tested.
pub fn scaleway_instance_offer_cost(
    offers: &[ScalewayOfferSummary],
    zone: &str,
    commercial_type: &str,
) -> ScalewayInstanceOfferCost {
    match offers
        .iter()
        .find(|offer| offer.zone == zone && offer.name == commercial_type)
    {
        Some(offer) => ScalewayInstanceOfferCost {
            hourly_eur: offer.hourly_price_eur,
            monthly_eur: offer.monthly_price_eur,
            risk: if offer.hourly_price_eur.is_none() && offer.monthly_price_eur.is_none() {
                Some(format!(
                    "Offer {commercial_type} in {zone} is in the catalog but carries no price; cost is unknown."
                ))
            } else {
                None
            },
        },
        None => ScalewayInstanceOfferCost {
            hourly_eur: None,
            monthly_eur: None,
            risk: Some(format!(
                "Offer {commercial_type} in {zone} is not in the synced catalog; estimated cost is unknown. Sync Scaleway to price it."
            )),
        },
    }
}

/// POST an Instance create request. Returns the new server id. Mirrors the storage
/// create fns: longer write budget, `error_for_status`, and a missing-id response
/// surfaces as an error (never a silent success).
pub async fn create_scaleway_instance_request(
    http: &reqwest::Client,
    token: &str,
    zone: &str,
    name: &str,
    project_id: &str,
    commercial_type: &str,
    image: &str,
    dynamic_ip_required: bool,
    tags: &[String],
) -> Result<String, String> {
    let body = scaleway_instance_create_body(
        name,
        project_id,
        commercial_type,
        image,
        dynamic_ip_required,
        tags,
    );
    let payload: Value = http
        .post(scaleway_instance_create_url(zone))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Instance create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Instance create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Instance create response was invalid: {e}"))?;
    // The create response wraps the server under `server` ({"server": {"id": ...}}).
    // Fall back to a top-level `id` for resilience. `string_field` matches keys at a
    // single object level, so descend into `server` first, then try the root.
    payload
        .get("server")
        .and_then(|server| string_field(server, &["id"]))
        .or_else(|| string_field(&payload, &["id"]))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// GET a Serverless namespace by id and return its raw JSON. `is_container`
/// selects the Containers vs Functions product endpoint. The caller uses this to
/// verify an explicitly-supplied namespace belongs to the pinned project BEFORE
/// creating a function/container inside it (a foreign-project namespace would
/// otherwise bypass the create-pin). A non-2xx (e.g. 404 for a namespace in
/// another project the token cannot read) surfaces as an error so the create
/// fails closed.
pub async fn fetch_scaleway_namespace_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    namespace_id: &str,
    is_container: bool,
) -> Result<Value, String> {
    let url = if is_container {
        scaleway_container_namespace_url(region, namespace_id)
    } else {
        scaleway_function_namespace_url(region, namespace_id)
    };
    http.get(url)
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway namespace lookup request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway namespace lookup request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway namespace lookup response was invalid: {e}"))
}

/// POST a Serverless Functions namespace create request. Returns the namespace id.
pub async fn create_scaleway_function_namespace_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    name: &str,
    project_id: &str,
) -> Result<String, String> {
    let body = scaleway_namespace_create_body(name, project_id);
    let payload: Value = http
        .post(scaleway_function_namespace_create_url(region))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Functions namespace create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Functions namespace create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Functions namespace create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// Parameters for creating a Serverless Function. `namespace_id` is created (or
/// reused) by the caller before this call.
pub struct ScalewayFunctionCreateRequest<'a> {
    pub region: &'a str,
    pub namespace_id: &'a str,
    pub name: &'a str,
    pub runtime: &'a str,
    pub memory_limit: Option<u32>,
    pub min_scale: Option<u32>,
    pub max_scale: Option<u32>,
}

/// POST a Serverless Function create request. Returns the new function id. NOTE:
/// this creates the function resource only — uploading its code is a separate
/// deploy step (the existing `deploy` action) and is NOT performed here.
pub async fn create_scaleway_function_request(
    http: &reqwest::Client,
    token: &str,
    req: &ScalewayFunctionCreateRequest<'_>,
) -> Result<String, String> {
    let body = scaleway_function_create_body(
        req.namespace_id,
        req.name,
        req.runtime,
        req.memory_limit,
        req.min_scale,
        req.max_scale,
    );
    let payload: Value = http
        .post(scaleway_function_create_url(req.region))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Function create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Function create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Function create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// DELETE a Serverless Function. A 404 is treated as already-gone (idempotent).
pub async fn delete_scaleway_function_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    function_id: &str,
) -> Result<(), String> {
    let response = http
        .delete(scaleway_function_url(region, function_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Function delete request failed: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway Function delete request rejected: {e}"))?;
    Ok(())
}

/// POST a Serverless Containers namespace create request. Returns the namespace id.
pub async fn create_scaleway_container_namespace_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    name: &str,
    project_id: &str,
) -> Result<String, String> {
    let body = scaleway_namespace_create_body(name, project_id);
    let payload: Value = http
        .post(scaleway_container_namespace_create_url(region))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Containers namespace create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Containers namespace create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Containers namespace create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// Parameters for creating a Serverless Container. `registry_image` references an
/// EXISTING image in a registry (no build is performed).
pub struct ScalewayContainerCreateRequest<'a> {
    pub region: &'a str,
    pub namespace_id: &'a str,
    pub name: &'a str,
    pub registry_image: &'a str,
    pub memory_limit: Option<u32>,
    pub min_scale: Option<u32>,
    pub max_scale: Option<u32>,
}

/// POST a Serverless Container create request. Returns the new container id. NOTE:
/// this creates the container resource referencing an existing image — pushing a
/// new image and deploying it is a separate step (the existing `deploy` action).
pub async fn create_scaleway_container_request(
    http: &reqwest::Client,
    token: &str,
    req: &ScalewayContainerCreateRequest<'_>,
) -> Result<String, String> {
    let body = scaleway_container_create_body(
        req.namespace_id,
        req.name,
        req.registry_image,
        req.memory_limit,
        req.min_scale,
        req.max_scale,
    );
    let payload: Value = http
        .post(scaleway_container_create_url(req.region))
        .header("X-Auth-Token", token)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Scaleway Container create request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Container create request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Scaleway Container create response was invalid: {e}"))?;
    string_field(&payload, &["id"])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Scaleway create succeeded but returned no resource id.".into())
}

/// DELETE a Serverless Container. A 404 is treated as already-gone (idempotent).
pub async fn delete_scaleway_container_request(
    http: &reqwest::Client,
    token: &str,
    region: &str,
    container_id: &str,
) -> Result<(), String> {
    let response = http
        .delete(scaleway_container_url(region, container_id))
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Container delete request failed: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        return Ok(());
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway Container delete request rejected: {e}"))?;
    Ok(())
}

/// PUT (create) an Object Storage bucket. Empty body, SigV4 signed, region-scoped.
pub async fn create_scaleway_object_bucket_request(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project_id: &str,
    region: &str,
    bucket: &str,
) -> Result<(), String> {
    let host = format!("s3.{region}.scw.cloud");
    let canonical_uri = format!("/{}", urlencoding::encode(bucket));
    let url = scaleway_s3_bucket_url(&host, bucket);
    let signed_access_key = format!("{}@{}", access_key.trim(), project_id.trim());
    let payload_hash = hex_sha256("");
    let date = Utc::now();
    let amz_date = date.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = date.format("%Y%m%d").to_string();
    let authorization = scaleway_s3_authorization(
        "PUT",
        &canonical_uri,
        "",
        &host,
        &amz_date,
        &date_stamp,
        region,
        &payload_hash,
        &signed_access_key,
        secret_key,
    )?;
    http.put(url)
        .header("Authorization", authorization)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", amz_date)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Object Storage bucket create failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Object Storage bucket create rejected: {e}"))?;
    Ok(())
}

/// DELETE an Object Storage bucket. SigV4 signed. S3 refuses a non-empty bucket
/// (409 BucketNotEmpty); that error is surfaced verbatim — we do NOT cascade.
pub async fn delete_scaleway_object_bucket_request(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project_id: &str,
    region: &str,
    bucket: &str,
) -> Result<(), String> {
    let host = format!("s3.{region}.scw.cloud");
    let canonical_uri = format!("/{}", urlencoding::encode(bucket));
    let url = scaleway_s3_bucket_url(&host, bucket);
    let signed_access_key = format!("{}@{}", access_key.trim(), project_id.trim());
    let payload_hash = hex_sha256("");
    let date = Utc::now();
    let amz_date = date.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = date.format("%Y%m%d").to_string();
    let authorization = scaleway_s3_authorization(
        "DELETE",
        &canonical_uri,
        "",
        &host,
        &amz_date,
        &date_stamp,
        region,
        &payload_hash,
        &signed_access_key,
        secret_key,
    )?;
    let response = http
        .delete(url)
        .header("Authorization", authorization)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", amz_date)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Object Storage bucket delete failed: {e}"))?;
    if response.status().as_u16() == 409 {
        return Err(
            "Object Storage bucket is not empty. Empty the bucket before deleting it (no automatic cascade)."
                .into(),
        );
    }
    response
        .error_for_status()
        .map_err(|e| format!("Scaleway Object Storage bucket delete rejected: {e}"))?;
    Ok(())
}

/// PURE: parse + validate the UI's JSON lifecycle rules into typed rules. Mirrors
/// the R2 lifecycle UX: an array of `{ id, prefix?, enabled?, expirationDays }`.
/// Rejects empty/oversized input so a malformed rule never reaches S3.
fn parse_scaleway_lifecycle_rules(rules: &Value) -> Result<Vec<ScalewayLifecycleRule>, String> {
    let array = rules
        .as_array()
        .ok_or_else(|| "Object Storage lifecycle rules must be an array.".to_string())?;
    if array.is_empty() {
        return Err("At least one lifecycle rule is required.".into());
    }
    if array.len() > 1_000 {
        return Err("Too many lifecycle rules (max 1000).".into());
    }
    let mut parsed = Vec::with_capacity(array.len());
    for rule in array {
        let id = rule
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 255)
            .ok_or_else(|| "Each lifecycle rule needs a non-empty id (max 255 chars).".to_string())?
            .to_string();
        let prefix = rule
            .get("prefix")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if prefix.len() > 1_024 {
            return Err("Lifecycle rule prefix is too long (max 1024 chars).".into());
        }
        // Default to Enabled when omitted (matches the S3/R2 default intent).
        let enabled = rule.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let expiration_days = rule
            .get("expirationDays")
            .and_then(Value::as_u64)
            .filter(|days| *days >= 1 && *days <= 36_500)
            .ok_or_else(|| {
                "Each lifecycle rule needs expirationDays between 1 and 36500.".to_string()
            })? as u32;
        parsed.push(ScalewayLifecycleRule {
            id,
            prefix,
            enabled,
            expiration_days,
        });
    }
    Ok(parsed)
}

/// Public lifecycle entry point: validate the JSON rules, then PUT the XML.
pub async fn set_scaleway_object_bucket_lifecycle_request(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project_id: &str,
    region: &str,
    bucket: &str,
    rules: &Value,
) -> Result<(), String> {
    let parsed = parse_scaleway_lifecycle_rules(rules)?;
    put_scaleway_object_bucket_lifecycle(
        http, access_key, secret_key, project_id, region, bucket, &parsed,
    )
    .await
}

/// PUT a bucket lifecycle configuration. SigV4 signed WITH a `content-md5` header
/// (required by Scaleway). The lifecycle XML is built from pre-validated rules.
async fn put_scaleway_object_bucket_lifecycle(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project_id: &str,
    region: &str,
    bucket: &str,
    rules: &[ScalewayLifecycleRule],
) -> Result<(), String> {
    let host = format!("s3.{region}.scw.cloud");
    let canonical_uri = format!("/{}", urlencoding::encode(bucket));
    // SigV4 canonicalizes a valueless query key as `key=`; build the wire URL the
    // same way so the signed canonical query and the transmitted query are byte-
    // identical (no reliance on server-side canonicalization equivalence).
    let canonical_query = "lifecycle=";
    let url = format!("https://{host}{canonical_uri}?lifecycle=");
    let signed_access_key = format!("{}@{}", access_key.trim(), project_id.trim());
    let body = scaleway_lifecycle_xml(rules);
    let payload_hash = hex_sha256(&body);
    let content_md5 = md5_base64(body.as_bytes());
    let date = Utc::now();
    let amz_date = date.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = date.format("%Y%m%d").to_string();
    let authorization = scaleway_s3_authorization_with_md5(
        "PUT",
        &canonical_uri,
        canonical_query,
        &host,
        &content_md5,
        &amz_date,
        &date_stamp,
        region,
        &payload_hash,
        &signed_access_key,
        secret_key,
    )?;
    http.put(url)
        .header("Authorization", authorization)
        .header("Content-MD5", content_md5)
        .header("x-amz-content-sha256", payload_hash)
        .header("x-amz-date", amz_date)
        .header("Content-Type", "application/xml")
        .body(body)
        .timeout(Duration::from_secs(SCW_STORAGE_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Object Storage lifecycle write failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Scaleway Object Storage lifecycle write rejected: {e}"))?;
    Ok(())
}

pub async fn fetch_scaleway_offers(http: &reqwest::Client) -> Vec<ScalewayOfferSummary> {
    let mut offers = join_all(
        SCW_ZONES
            .iter()
            .map(|zone| fetch_scaleway_zone_offers(http, zone)),
    )
    .await
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    offers.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.zone.cmp(&b.zone))
    });
    offers
}

pub async fn fetch_scaleway_iam_console_resources(
    http: &reqwest::Client,
    token: Option<&str>,
    project_id: Option<&str>,
) -> Vec<ProviderConsoleResourceSummary> {
    let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    let project_id = project_id.map(str::trim).filter(|value| !value.is_empty());

    let lists = join_all([
        scaleway_iam_list_endpoint(
            http,
            token,
            project_id,
            "https://api.scaleway.com/iam/v1alpha1/policies",
            &["policies"],
            "Policy",
            "https://www.scaleway.com/en/developers/api/iam",
        ),
        scaleway_iam_list_endpoint(
            http,
            token,
            project_id,
            "https://api.scaleway.com/iam/v1alpha1/applications",
            &["applications"],
            "Application",
            "https://www.scaleway.com/en/developers/api/iam",
        ),
        scaleway_iam_list_endpoint(
            http,
            token,
            project_id,
            "https://api.scaleway.com/iam/v1alpha1/groups",
            &["groups"],
            "Group",
            "https://www.scaleway.com/en/developers/api/iam",
        ),
        scaleway_iam_list_endpoint(
            http,
            token,
            project_id,
            "https://api.scaleway.com/iam/v1alpha1/api-keys",
            &["api_keys", "apiKeys"],
            "API Key",
            "https://www.scaleway.com/en/developers/api/iam",
        ),
    ])
    .await;
    let mut resources = lists.into_iter().flatten().collect::<Vec<_>>();
    resources.sort_by(|a, b| {
        a.resource_type
            .cmp(&b.resource_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    resources
}

pub async fn fetch_scaleway_extended_console_resources(
    http: &reqwest::Client,
    token: Option<&str>,
    project_id: Option<&str>,
) -> ScalewayExtendedInventory {
    let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) else {
        return ScalewayExtendedInventory::default();
    };
    let Some(project_id) = project_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return ScalewayExtendedInventory::default();
    };

    let mut tasks = Vec::new();
    for region in SCW_REGIONS {
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/vpc/v2/regions/{region}/private-networks?project_id={}",
                urlencoding::encode(project_id)
            ),
            &["private_networks", "privateNetworks"],
            "scw-network-security",
            "Private Network",
            Some(region),
            "https://www.scaleway.com/en/docs/vpc/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/vpc-gw/v2/regions/{region}/gateways?project_id={}",
                urlencoding::encode(project_id)
            ),
            &["gateways"],
            "scw-network-security",
            "Public Gateway",
            Some(region),
            "https://www.scaleway.com/en/developers/api/public-gateway/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/lb/v1/regions/{region}/lbs?project_id={}",
                urlencoding::encode(project_id)
            ),
            &["lbs", "load_balancers", "loadBalancers"],
            "scw-network-security",
            "Load Balancer",
            Some(region),
            "https://www.scaleway.com/en/docs/load-balancer/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/rdb/v1/regions/{region}/instances?project_id={}",
                urlencoding::encode(project_id)
            ),
            &["instances"],
            "scw-data-managed",
            "Managed Database",
            Some(region),
            "https://www.scaleway.com/en/developers/api/managed-database-postgre-mysql/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/registry/v1/regions/{region}/namespaces?project_id={}",
                urlencoding::encode(project_id)
            ),
            &["namespaces"],
            "scw-data-managed",
            "Registry Namespace",
            Some(region),
            "https://www.scaleway.com/en/developers/api/registry/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/k8s/v1/regions/{region}/clusters?project_id={}",
                urlencoding::encode(project_id)
            ),
            &["clusters"],
            "scw-data-managed",
            "Kubernetes Cluster",
            Some(region),
            "https://www.scaleway.com/en/docs/containers/kubernetes/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/key-manager/v1alpha1/regions/{region}/keys?project_id={}&page_size={SCW_PAGE_SIZE}",
                urlencoding::encode(project_id)
            ),
            &["keys"],
            "scw-network-security",
            "KMS Key",
            Some(region),
            "https://www.scaleway.com/en/developers/api/key-manager/keys",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/mnq/v1beta1/regions/{region}/nats-accounts?project_id={}&page_size={SCW_PAGE_SIZE}",
                urlencoding::encode(project_id)
            ),
            &["nats_accounts", "natsAccounts"],
            "scw-data-managed",
            "NATS Account",
            Some(region),
            "https://www.scaleway.com/en/developers/api/nats/nats-api/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/mnq/v1beta1/regions/{region}/sqs-credentials?project_id={}&page_size={SCW_PAGE_SIZE}",
                urlencoding::encode(project_id)
            ),
            &["sqs_credentials", "sqsCredentials", "credentials"],
            "scw-data-managed",
            "SQS Credential",
            Some(region),
            "https://www.scaleway.com/en/developers/api/messaging-and-queuing/sqs-api/",
        ));
        tasks.push(scaleway_project_list_endpoint(
            http,
            token,
            format!(
                "{SCW_API}/mnq/v1beta1/regions/{region}/sns-credentials?project_id={}&page_size={SCW_PAGE_SIZE}",
                urlencoding::encode(project_id)
            ),
            &["sns_credentials", "snsCredentials", "credentials"],
            "scw-data-managed",
            "SNS Credential",
            Some(region),
            "https://www.scaleway.com/en/developers/api/messaging-and-queuing/sns-api/",
        ));
    }

    let mut resources = join_all(tasks)
        .await
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    resources.sort_by(|a, b| {
        a.service_id
            .cmp(&b.service_id)
            .then_with(|| a.resource_type.cmp(&b.resource_type))
            .then_with(|| a.name.cmp(&b.name))
    });
    ScalewayExtendedInventory { resources }
}

async fn scaleway_project_list_endpoint(
    http: &reqwest::Client,
    token: &str,
    url: String,
    collection_keys: &[&str],
    service_id: &str,
    resource_type: &str,
    region: Option<&str>,
    docs_url: &str,
) -> Vec<ProviderConsoleResourceSummary> {
    let response = match http
        .get(url)
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            return vec![scaleway_console_diagnostic(
                service_id,
                resource_type,
                region,
                docs_url,
                "unavailable",
                &format!("Scaleway {resource_type} request failed: {e}"),
            )]
        }
    };
    let status = response.status();
    if !status.is_success() {
        return vec![scaleway_console_diagnostic(
            service_id,
            resource_type,
            region,
            docs_url,
            if status.as_u16() == 401 || status.as_u16() == 403 {
                "forbidden"
            } else {
                "unavailable"
            },
            &format!("Scaleway {resource_type} endpoint returned {status}."),
        )];
    }
    let Ok(payload) = response.json::<Value>().await else {
        return vec![scaleway_console_diagnostic(
            service_id,
            resource_type,
            region,
            docs_url,
            "unavailable",
            &format!("Scaleway {resource_type} response was invalid."),
        )];
    };
    json_result_items(&payload, collection_keys)
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            scaleway_project_console_resource(service_id, resource_type, region, docs_url, item)
        })
        .collect()
}

fn scaleway_console_diagnostic(
    service_id: &str,
    resource_type: &str,
    region: Option<&str>,
    docs_url: &str,
    status: &str,
    message: &str,
) -> ProviderConsoleResourceSummary {
    ProviderConsoleResourceSummary {
        id: format!("scaleway:{service_id}:{resource_type}:diagnostic:{status}"),
        provider: ProviderId::Scaleway,
        service_id: service_id.into(),
        resource_type: format!("{resource_type} access"),
        name: format!("{resource_type} access"),
        region: region.map(String::from),
        status: status.into(),
        description: message.into(),
        metadata: Vec::new(),
        docs_url: docs_url.into(),
        updated_at: Some(now()),
    }
}

fn scaleway_project_console_resource(
    service_id: &str,
    resource_type: &str,
    region: Option<&str>,
    docs_url: &str,
    item: Value,
) -> ProviderConsoleResourceSummary {
    let name =
        string_field(&item, &["name", "namespace", "id"]).unwrap_or_else(|| "unnamed".into());
    let raw_id = string_field(&item, &["id", "name"]).unwrap_or_else(|| name.clone());
    let status = string_field(&item, &["status", "state", "visibility"])
        .or_else(|| string_field(&item, &["is_public"]).map(|value| format!("public:{value}")))
        .unwrap_or_else(|| "listed".into());
    let updated_at = string_field(&item, &["updated_at", "created_at"]);
    let mut metadata = Vec::new();
    for key in [
        "id",
        "project_id",
        "organization_id",
        "created_at",
        "updated_at",
        "endpoint",
        "engine",
        "version",
        "usage",
        "algorithm",
        "access_key",
        "nats_endpoint_url",
        "sqs_endpoint_url",
        "sns_endpoint_url",
        "description",
    ] {
        if let Some(value) = string_field(&item, &[key]) {
            metadata.push(format!("{key}: {value}"));
        }
    }

    ProviderConsoleResourceSummary {
        id: format!("scaleway:{service_id}:{resource_type}:{raw_id}"),
        provider: ProviderId::Scaleway,
        service_id: service_id.into(),
        resource_type: resource_type.into(),
        name,
        region: region.map(String::from),
        status,
        description: "Scaleway project resource listed through a read-only API endpoint.".into(),
        metadata,
        docs_url: docs_url.into(),
        updated_at,
    }
}

fn scaleway_iam_item_visible_for_project(item: &Value, project_id: Option<&str>) -> bool {
    let Some(project_id) = project_id else {
        return true;
    };
    let has_project_binding = json_has_any_key(item, &["project_id", "default_project_id"]);
    if has_project_binding {
        return json_contains_string_for_keys(
            item,
            &["project_id", "default_project_id"],
            project_id,
        );
    }
    true
}

fn json_has_any_key(value: &Value, keys: &[&str]) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            keys.iter().any(|candidate| key == candidate) || json_has_any_key(child, keys)
        }),
        Value::Array(items) => items.iter().any(|item| json_has_any_key(item, keys)),
        _ => false,
    }
}

fn json_contains_string_for_keys(value: &Value, keys: &[&str], needle: &str) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            (keys.iter().any(|candidate| key == candidate)
                && json_value_contains_string(child, needle))
                || json_contains_string_for_keys(child, keys, needle)
        }),
        Value::Array(items) => items
            .iter()
            .any(|item| json_contains_string_for_keys(item, keys, needle)),
        _ => false,
    }
}

fn json_value_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value == needle,
        Value::Array(items) => items
            .iter()
            .any(|item| json_value_contains_string(item, needle)),
        Value::Object(_) => {
            json_contains_string_for_keys(value, &["project_id", "default_project_id"], needle)
        }
        _ => false,
    }
}

async fn scaleway_iam_list_endpoint(
    http: &reqwest::Client,
    token: &str,
    project_id: Option<&str>,
    url: &str,
    collection_keys: &[&str],
    resource_type: &str,
    docs_url: &str,
) -> Vec<ProviderConsoleResourceSummary> {
    let response = match http
        .get(url)
        .header("X-Auth-Token", token)
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            return vec![scaleway_console_diagnostic(
                "scw-iam-projects",
                resource_type,
                None,
                docs_url,
                "unavailable",
                &format!("Scaleway IAM {resource_type} request failed: {e}"),
            )]
        }
    };
    let status = response.status();
    if !status.is_success() {
        return vec![scaleway_console_diagnostic(
            "scw-iam-projects",
            resource_type,
            None,
            docs_url,
            if status.as_u16() == 401 || status.as_u16() == 403 {
                "forbidden"
            } else {
                "unavailable"
            },
            &format!("Scaleway IAM {resource_type} endpoint returned {status}."),
        )];
    }
    let Ok(payload) = response.json::<Value>().await else {
        return vec![scaleway_console_diagnostic(
            "scw-iam-projects",
            resource_type,
            None,
            docs_url,
            "unavailable",
            &format!("Scaleway IAM {resource_type} response was invalid."),
        )];
    };
    json_result_items(&payload, collection_keys)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| scaleway_iam_console_resource(project_id, resource_type, docs_url, item))
        .collect()
}

fn scaleway_iam_console_resource(
    project_id: Option<&str>,
    resource_type: &str,
    docs_url: &str,
    item: Value,
) -> Option<ProviderConsoleResourceSummary> {
    if !scaleway_iam_item_visible_for_project(&item, project_id) {
        return None;
    }
    let name = string_field(&item, &["name", "description", "access_key", "id"])
        .unwrap_or_else(|| "unnamed".into());
    let raw_id = string_field(&item, &["id", "access_key"]).unwrap_or_else(|| name.clone());
    let updated_at = string_field(&item, &["updated_at", "created_at", "expires_at"]);
    let status = string_field(&item, &["status", "state"])
        .or_else(|| string_field(&item, &["editable"]).map(|value| format!("editable:{value}")))
        .unwrap_or_else(|| "listed".into());
    let mut metadata = Vec::new();
    for key in [
        "id",
        "organization_id",
        "project_id",
        "default_project_id",
        "created_at",
        "updated_at",
        "expires_at",
    ] {
        if let Some(value) = string_field(&item, &[key]) {
            metadata.push(format!("{key}: {value}"));
        }
    }

    if let Some(project_id) = project_id {
        if !json_contains_string_for_keys(&item, &["project_id", "default_project_id"], project_id)
        {
            metadata.push("scope: org-wide or not project-scoped by IAM API".into());
        }
    }

    Some(ProviderConsoleResourceSummary {
        id: format!("scaleway:scw-iam-projects:{resource_type}:{raw_id}"),
        provider: ProviderId::Scaleway,
        service_id: "scw-iam-projects".into(),
        resource_type: resource_type.into(),
        name,
        region: None,
        status,
        description: "Scaleway IAM resource listed through the IAM API.".into(),
        metadata,
        docs_url: docs_url.into(),
        updated_at,
    })
}

async fn fetch_scaleway_zone_offers(
    http: &reqwest::Client,
    zone: &str,
) -> Vec<ScalewayOfferSummary> {
    let products = http
        .get(scaleway_server_products_url(zone))
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
        .ok()
        .and_then(|response| response.error_for_status().ok());
    let Some(products) = products else {
        return Vec::new();
    };
    let Ok(products) = products.json::<ScwServerProductsEnvelope>().await else {
        return Vec::new();
    };

    let availability = http
        .get(scaleway_server_availability_url(zone))
        .timeout(Duration::from_secs(SCW_ACTION_TIMEOUT_SECS))
        .send()
        .await
        .ok()
        .and_then(|response| response.error_for_status().ok());
    let availability = match availability {
        Some(response) => response
            .json::<ScwServerAvailabilityEnvelope>()
            .await
            .map(|envelope| envelope.servers)
            .unwrap_or_default(),
        None => HashMap::new(),
    };

    products
        .servers
        .into_iter()
        .map(|(name, product)| scaleway_offer_summary(zone, name, product, &availability))
        .collect()
}

fn scaleway_offer_summary(
    zone: &str,
    name: String,
    product: ScwServerProduct,
    availability: &HashMap<String, ScwServerAvailability>,
) -> ScalewayOfferSummary {
    let gpu_count = product.gpu.unwrap_or_default();
    let category = if gpu_count > 0 { "GPU" } else { "CPU VM" };
    let architecture = product.arch.unwrap_or_else(|| "unknown".into());
    let availability = availability
        .get(&name)
        .and_then(|item| item.availability.clone())
        .unwrap_or_else(|| "unknown".into());
    let gpu_label = product
        .gpu_info
        .as_ref()
        .and_then(gpu_info_label)
        .or_else(|| (gpu_count > 0).then(|| format!("{gpu_count} GPU")));
    let mut tags = vec![architecture.clone(), availability.clone()];
    if let Some(capabilities) = product.capabilities.as_ref() {
        if json_bool(capabilities, "block_storage") {
            tags.push("block-storage".into());
        }
        if json_u64(capabilities, "private_network").unwrap_or_default() > 0 {
            tags.push("private-network".into());
        }
        if json_u64(capabilities, "max_file_systems").unwrap_or_default() > 0 {
            tags.push("filesystem".into());
        }
    }
    tags.sort();
    tags.dedup();

    ScalewayOfferSummary {
        id: format!("{zone}:{name}"),
        name,
        zone: zone.into(),
        category: category.into(),
        architecture,
        vcpus: product.ncpus.unwrap_or_default(),
        memory_gb: bytes_to_gb(product.ram.unwrap_or_default()),
        gpu_count,
        gpu_label,
        hourly_price_eur: product.hourly_price,
        monthly_price_eur: product.monthly_price,
        availability,
        tags,
    }
}

fn gpu_info_label(value: &Value) -> Option<String> {
    if let Some(label) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(label.into());
    }
    let object = value.as_object()?;
    ["name", "model", "gpu", "display_name"]
        .into_iter()
        .find_map(|key| object.get(key)?.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
}

fn json_bool(value: &Value, key: &str) -> bool {
    value
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn json_u64(value: &Value, key: &str) -> Option<u64> {
    value
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_u64)
}

struct ScalewayObjectStorageInventory {
    storage: Vec<ScalewayStorageSummary>,
    request_count: usize,
    success_count: usize,
    failure_count: usize,
    usage_failure_count: usize,
}

async fn fetch_scaleway_object_storage_inventory(
    http: &reqwest::Client,
    access_key: &str,
    secret_key: &str,
    project: &ScwProject,
) -> ScalewayObjectStorageInventory {
    let mut storage = Vec::new();
    let mut seen_bucket_names = HashSet::new();
    let mut request_count = 0usize;
    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    let mut usage_failure_count = 0usize;

    for region in SCW_REGIONS {
        request_count += 1;
        match fetch_scaleway_object_buckets(http, access_key, secret_key, &project.id, region).await
        {
            Ok(buckets) => {
                success_count += 1;
                for bucket in buckets {
                    if seen_bucket_names.insert(bucket.name.clone()) {
                        let usage = match fetch_scaleway_object_bucket_usage(
                            http,
                            access_key,
                            secret_key,
                            &project.id,
                            region,
                            &bucket.name,
                        )
                        .await
                        {
                            Ok(usage) => Some(usage),
                            Err(_) => {
                                usage_failure_count += 1;
                                None
                            }
                        };
                        storage.push(bucket.into_summary(region, project, usage));
                    }
                }
            }
            Err(_) => {
                failure_count += 1;
            }
        }
    }

    ScalewayObjectStorageInventory {
        storage,
        request_count,
        success_count,
        failure_count,
        usage_failure_count,
    }
}

async fn fetch_scaleway_object_storage_only(
    http: &reqwest::Client,
    pinned_project_id: Option<&str>,
    object_access_key: Option<&str>,
    object_secret_key: Option<&str>,
    token_error: Option<String>,
) -> Option<ProviderInventory> {
    let project_id = pinned_project_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(configured_scaleway_project_id)?;
    let access_key = object_access_key
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let secret_key = object_secret_key
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let project = ScwProject {
        id: project_id.clone(),
        name: SCW_TARGET_PROJECT_NAME.into(),
    };
    let object_result =
        fetch_scaleway_object_storage_inventory(http, access_key, secret_key, &project).await;
    let object_storage_failures = object_result.failure_count + object_result.usage_failure_count;
    let storage = object_result.storage;
    let selected_scope = Some(ProviderScopeSelection {
        provider: ProviderId::Scaleway,
        id: project_id,
        name: Some(project.name.clone()),
        source: "pinned_object_storage".into(),
    });
    let mut risks = vec![RiskFlag {
        id: "scaleway_api_token_missing_or_invalid".into(),
        severity: "high".into(),
        title: "Scaleway VM and Serverless inventory unavailable".into(),
        description: "Object Storage is live through S3 credentials, but Instance and Serverless inventory/actions require a valid Scaleway API token for the pinned Aspis Bio project.".into(),
        source: "Scaleway".into(),
        timestamp: now(),
    }];
    if object_storage_failures > 0 {
        risks.push(RiskFlag {
            id: "scaleway_object_storage_inventory_partial".into(),
            severity: "medium".into(),
            title: "Scaleway Object Storage inventory partial".into(),
            description: format!(
                "{object_storage_failures} Object Storage bucket or usage lookup(s) failed."
            ),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    let token_health = if token_error.is_some() {
        "invalid"
    } else {
        "missing"
    };
    let token_note = token_error
        .as_deref()
        .map(sanitize_error_message)
        .map(|message| format!(" Scaleway API token check failed: {message}"))
        .unwrap_or_default();
    let message = if object_result.success_count > 0 {
        format!(
            "Scaleway Object Storage synced from S3 credentials; VM/serverless inventory and actions are unavailable until a valid Scaleway API token is saved.{token_note}"
        )
    } else {
        format!(
            "Scaleway Object Storage credentials are configured, but bucket listing did not return a successful response.{token_note}"
        )
    };

    Some(ProviderInventory {
        health: ProviderHealth {
            id: ProviderId::Scaleway,
            name: "Scaleway".into(),
            status: "degraded".into(),
            last_sync: Some(now()),
            token_health: token_health.into(),
            credential_kind: Some("scaleway_object_storage".into()),
            resource_count: storage.len(),
            message: Some(message),
        },
        workers: Vec::new(),
        compute: Vec::new(),
        storage,
        risks,
        activity: vec![ActivityEvent {
            id: "scaleway_object_storage_sync_completed".into(),
            message: format!(
                "Scaleway Object Storage sync completed for project {SCW_TARGET_PROJECT_NAME}."
            ),
            timestamp: now(),
            event_type: "sync".into(),
            source: "Scaleway".into(),
        }],
        selected_scope,
    })
}

pub async fn fetch_scaleway(
    http: &reqwest::Client,
    token: Option<String>,
    pinned_project_id: Option<String>,
    object_access_key: Option<String>,
    object_secret_key: Option<String>,
) -> ProviderInventory {
    let token = token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if let Some(token) = token {
        match fetch_scaleway_inner(
            http,
            &token,
            pinned_project_id.as_deref(),
            object_access_key.as_deref(),
            object_secret_key.as_deref(),
        )
        .await
        {
            Ok(inventory) => inventory,
            Err(e) => {
                match fetch_scaleway_object_storage_only(
                    http,
                    pinned_project_id.as_deref(),
                    object_access_key.as_deref(),
                    object_secret_key.as_deref(),
                    Some(e.clone()),
                )
                .await
                {
                    Some(inventory) => inventory,
                    None => ProviderInventory::error(ProviderId::Scaleway, e),
                }
            }
        }
    } else if let Some(inventory) = fetch_scaleway_object_storage_only(
        http,
        pinned_project_id.as_deref(),
        object_access_key.as_deref(),
        object_secret_key.as_deref(),
        None,
    )
    .await
    {
        inventory
    } else {
        ProviderInventory::missing(ProviderId::Scaleway)
    }
}

/// Fetch the list of Generative API models available to the Scaleway secret key.
///
/// Generative APIs live on a DIFFERENT base (`https://api.scaleway.ai`) and use
/// `Authorization: Bearer <secret_key>` (OpenAI-compatible `{ data: [...] }`),
/// NOT the `X-Auth-Token` mgmt header. The authorising credential is the same
/// Scaleway IAM secret key used elsewhere here as `X-Auth-Token`. The call is
/// non-paginated and never logs the secret.
async fn fetch_scaleway_generative_models(
    http: &reqwest::Client,
    secret_key: &str,
) -> Result<Vec<ScwGenerativeModel>, String> {
    let res = http
        .get(format!("{SCW_GENERATIVE_API}/v1/models"))
        .bearer_auth(secret_key)
        .timeout(Duration::from_secs(SCW_GENERATIVE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| format!("Scaleway Generative APIs request failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!(
            "Scaleway Generative APIs returned HTTP {}.",
            res.status().as_u16()
        ));
    }
    let envelope: ScwGenerativeModelsEnvelope = res
        .json()
        .await
        .map_err(|e| format!("Scaleway Generative APIs response was invalid: {e}"))?;
    // Drop any model whose `id` was missing/blank (parsed via `serde(default)`),
    // so one malformed element never produces a summary with an empty id and
    // never nukes the otherwise-valid models alongside it.
    Ok(envelope
        .data
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .collect())
}

async fn fetch_scaleway_inner(
    http: &reqwest::Client,
    token: &str,
    pinned_project_id: Option<&str>,
    object_access_key: Option<&str>,
    object_secret_key: Option<&str>,
) -> Result<ProviderInventory, String> {
    let projects_result = http
        .get(format!("{SCW_API}/account/v3/projects"))
        .header("X-Auth-Token", token)
        .send()
        .await
        .map_err(|e| format!("Scaleway projects request failed: {e}"));
    let mut selected_scope_source = scaleway_selection_source(
        pinned_project_id,
        configured_scaleway_project_id().as_deref(),
    );
    let project = match projects_result {
        Ok(response) => match response.error_for_status() {
            Ok(response) => {
                let projects: ScwProjectsEnvelope = response
                    .json()
                    .await
                    .map_err(|e| format!("Scaleway projects response was invalid: {e}"))?;
                select_scaleway_project(&projects.projects, pinned_project_id)?
            }
            Err(project_error) => {
                selected_scope_source = "pinned_api_key_default".into();
                scaleway_project_from_api_key(http, token, object_access_key, pinned_project_id)
                    .await
                    .map_err(|fallback_error| {
                        format!(
                            "Scaleway projects request rejected: {project_error}. {fallback_error}"
                        )
                    })?
            }
        },
        Err(project_error) => {
            selected_scope_source = "pinned_api_key_default".into();
            scaleway_project_from_api_key(http, token, object_access_key, pinned_project_id)
                .await
                .map_err(|fallback_error| format!("{project_error}. {fallback_error}"))?
        }
    };
    let selected_scope = Some(ProviderScopeSelection {
        provider: ProviderId::Scaleway,
        id: project.id.clone(),
        name: Some(project.name.clone()),
        source: selected_scope_source,
    });

    let mut compute = Vec::new();
    let mut storage = Vec::new();
    let mut activity = Vec::new();
    let mut request_count = 0usize;
    let mut success_count = 0usize;
    // Core (project-token / X-Auth-Token) API tallies, tracked SEPARATELY from the
    // ancillary Object Storage (SigV4) and Generative API (Bearer) calls. The
    // all-fail hard-error gate keys on these so a generative-only success cannot
    // mask a total core-API auth failure. See `scaleway_core_sync_failed`.
    //
    // File Storage and Serverless SQL are fr-par-only / optional beta products and
    // are DELIBERATELY excluded from the core tallies: counting them would let a
    // File/SQL-only success mask a total instance/core auth failure (and a File/SQL
    // 404 would otherwise look like a core failure). They get their own non-core,
    // non-degrading counters below.
    let mut core_request_count = 0usize;
    let mut core_success_count = 0usize;
    let mut failure_count = 0usize;
    let mut action_failure_count = 0usize;
    let mut storage_failure_count = 0usize;
    let mut file_failure_count = 0usize;
    let mut sql_failure_count = 0usize;
    let mut object_storage_failure_count = 0usize;
    let mut object_storage_credentials_missing = false;
    let mut generative_api_failure_count = 0usize;
    let mut generative_credentials_missing = false;

    for zone in SCW_ZONES {
        for page in 1..=SCW_MAX_PAGES {
            let url = scaleway_servers_url(zone, &project.id, page);
            request_count += 1;
            core_request_count += 1;
            let res = http.get(url).header("X-Auth-Token", token).send().await;
            let Ok(res) = res else {
                failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                failure_count += 1;
                break;
            }
            let Ok(envelope) = res.json::<ScwServersEnvelope>().await else {
                failure_count += 1;
                break;
            };
            success_count += 1;
            core_success_count += 1;

            let item_count = envelope.servers.len();
            let total_count = envelope.total_count;
            for server in envelope.servers {
                let mut summary = scaleway_server_summary(server, zone, &project);
                match fetch_scaleway_server_actions(http, token, zone, &summary.id).await {
                    Ok(actions) => summary.available_actions = actions,
                    Err(_) => action_failure_count += 1,
                }
                compute.push(summary);
            }
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }
    }

    for region in SCW_REGIONS {
        let mut namespaces = Vec::new();
        for page in 1..=SCW_MAX_PAGES {
            let namespaces_url = scaleway_namespaces_url(region, &project.id, page);
            request_count += 1;
            core_request_count += 1;
            let res = http
                .get(namespaces_url)
                .header("X-Auth-Token", token)
                .send()
                .await;
            let Ok(res) = res else {
                failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                failure_count += 1;
                break;
            }
            let Ok(envelope) = res.json::<ScwNamespacesEnvelope>().await else {
                failure_count += 1;
                break;
            };
            success_count += 1;
            core_success_count += 1;

            let item_count = envelope.namespaces.len();
            let total_count = envelope.total_count;
            namespaces.extend(envelope.namespaces);
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }

        for namespace in namespaces {
            for page in 1..=SCW_MAX_PAGES {
                let functions_url =
                    scaleway_functions_url(region, &namespace.id, &project.id, page);
                request_count += 1;
                core_request_count += 1;
                let res = http
                    .get(functions_url)
                    .header("X-Auth-Token", token)
                    .send()
                    .await;
                let Ok(res) = res else {
                    failure_count += 1;
                    break;
                };
                if !res.status().is_success() {
                    failure_count += 1;
                    break;
                }
                let Ok(functions) = res.json::<ScwFunctionsEnvelope>().await else {
                    failure_count += 1;
                    break;
                };
                success_count += 1;
                core_success_count += 1;
                let item_count = functions.functions.len();
                let total_count = functions.total_count;
                compute.extend(
                    functions
                        .functions
                        .into_iter()
                        .map(|function| function.into_summary(region, &project)),
                );
                if !scaleway_has_next_page(page, item_count, total_count) {
                    break;
                }
            }
        }
    }

    for region in SCW_REGIONS {
        for page in 1..=SCW_MAX_PAGES {
            let containers_url = scaleway_containers_url(region, &project.id, page);
            request_count += 1;
            core_request_count += 1;
            let res = http
                .get(containers_url)
                .header("X-Auth-Token", token)
                .send()
                .await;
            let Ok(res) = res else {
                failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                failure_count += 1;
                break;
            }
            let Ok(containers) = res.json::<ScwContainersEnvelope>().await else {
                failure_count += 1;
                break;
            };
            success_count += 1;
            core_success_count += 1;
            let item_count = containers.containers.len();
            let total_count = containers.total_count;
            compute.extend(
                containers
                    .containers
                    .into_iter()
                    .map(|container| container.into_summary(region, &project)),
            );
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }
    }

    for zone in SCW_ZONES {
        for page in 1..=SCW_MAX_PAGES {
            let url = scaleway_block_volumes_url(zone, &project.id, page);
            request_count += 1;
            core_request_count += 1;
            let res = http.get(url).header("X-Auth-Token", token).send().await;
            let Ok(res) = res else {
                failure_count += 1;
                storage_failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                failure_count += 1;
                storage_failure_count += 1;
                break;
            }
            let Ok(envelope) = res.json::<ScwBlockVolumesEnvelope>().await else {
                failure_count += 1;
                storage_failure_count += 1;
                break;
            };
            success_count += 1;
            core_success_count += 1;

            let item_count = envelope.volumes.len();
            let total_count = envelope.total_count;
            storage.extend(
                envelope
                    .volumes
                    .into_iter()
                    .map(|volume| volume.into_summary(zone, &project)),
            );
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }

        for page in 1..=SCW_MAX_PAGES {
            let url = scaleway_block_snapshots_url(zone, &project.id, page);
            request_count += 1;
            core_request_count += 1;
            let res = http.get(url).header("X-Auth-Token", token).send().await;
            let Ok(res) = res else {
                failure_count += 1;
                storage_failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                failure_count += 1;
                storage_failure_count += 1;
                break;
            }
            let Ok(envelope) = res.json::<ScwBlockSnapshotsEnvelope>().await else {
                failure_count += 1;
                storage_failure_count += 1;
                break;
            };
            success_count += 1;
            core_success_count += 1;

            let item_count = envelope.snapshots.len();
            let total_count = envelope.total_count;
            storage.extend(
                envelope
                    .snapshots
                    .into_iter()
                    .map(|snapshot| snapshot.into_summary(zone, &project)),
            );
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }
    }

    // File Storage is region-scoped and fr-par-only today. A 404 / other-region
    // rejection is NON-fatal and NON-degrading: it is an optional beta product, so
    // a failure bumps ONLY the dedicated `file_failure_count` (its own risk flag)
    // and is NOT counted toward the core tallies, the generic `failure_count`, or
    // the Block-Storage `storage_failure_count`.
    for region in SCW_FR_PAR_ONLY_REGIONS {
        for page in 1..=SCW_MAX_PAGES {
            let url = scaleway_filesystems_url(region, &project.id, page);
            request_count += 1;
            let res = http.get(url).header("X-Auth-Token", token).send().await;
            let Ok(res) = res else {
                file_failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                file_failure_count += 1;
                break;
            }
            let Ok(envelope) = res.json::<ScwFilesystemsEnvelope>().await else {
                file_failure_count += 1;
                break;
            };
            success_count += 1;

            let item_count = envelope.filesystems.len();
            let total_count = envelope.total_count;
            storage.extend(
                envelope
                    .filesystems
                    .into_iter()
                    .map(|filesystem| filesystem.into_summary(region, &project)),
            );
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }
    }

    // Serverless SQL is region-scoped and fr-par-only today. Same non-fatal,
    // NON-degrading, non-core policy as File Storage above: a failure bumps ONLY
    // the dedicated `sql_failure_count` (its own risk flag) and never touches the
    // core tallies or the generic `failure_count`.
    for region in SCW_FR_PAR_ONLY_REGIONS {
        for page in 1..=SCW_MAX_PAGES {
            let url = scaleway_sql_databases_url(region, &project.id, page);
            request_count += 1;
            let res = http.get(url).header("X-Auth-Token", token).send().await;
            let Ok(res) = res else {
                sql_failure_count += 1;
                break;
            };
            if !res.status().is_success() {
                sql_failure_count += 1;
                break;
            }
            let Ok(envelope) = res.json::<ScwSqlDatabasesEnvelope>().await else {
                sql_failure_count += 1;
                break;
            };
            success_count += 1;

            let item_count = envelope.databases.len();
            let total_count = envelope.total_count;
            compute.extend(
                envelope
                    .databases
                    .into_iter()
                    .map(|database| database.into_summary(region, &project)),
            );
            if !scaleway_has_next_page(page, item_count, total_count) {
                break;
            }
        }
    }

    let clean_object_access_key = object_access_key
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let clean_object_secret_key = object_secret_key
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let (Some(access_key), Some(secret_key)) = (clean_object_access_key, clean_object_secret_key)
    {
        let object_result =
            fetch_scaleway_object_storage_inventory(http, access_key, secret_key, &project).await;
        request_count += object_result.request_count;
        success_count += object_result.success_count;
        // Object Storage (S3 SigV4) failures are accounted ONLY under the dedicated
        // `object_storage_failure_count`. They MUST NOT also be added to the generic
        // core `failure_count` (which is reported as "inventory request(s) failed"),
        // or the same S3 failure would be double-counted and double-attributed in the
        // partial-sync message. Object Storage still degrades the view via
        // `object_storage_failure_count > 0` in `is_degraded()`, and it was never a
        // core counter, so `scaleway_core_sync_failed` is unaffected.
        object_storage_failure_count +=
            object_result.failure_count + object_result.usage_failure_count;
        storage.extend(object_result.storage);
    } else {
        object_storage_credentials_missing = true;
    }

    // Generative APIs: the authorising credential is the same Scaleway IAM secret
    // key used for `X-Auth-Token` above (passed in as `token`). Single non-paginated
    // call on a different base with `Authorization: Bearer`. Non-fatal: a failure is
    // accounted as a degraded lookup and surfaced as a risk, never errors the sync.
    let clean_generative_secret = token.trim();
    if clean_generative_secret.is_empty() {
        generative_credentials_missing = true;
    } else {
        request_count += 1;
        match fetch_scaleway_generative_models(http, clean_generative_secret).await {
            Ok(models) => {
                success_count += 1;
                compute.extend(models.into_iter().map(|model| model.into_summary(&project)));
            }
            Err(_) => {
                // NON-degrading, non-core: bump ONLY the dedicated generative
                // counter (its own risk flag). The Generative API is optional.
                generative_api_failure_count += 1;
            }
        }
    }

    // Hard-error ONLY when every CORE (project-token) API failed. A generative-only
    // or object-storage-only success must not mask a total core-API auth failure.
    if scaleway_core_sync_failed(core_request_count, core_success_count) {
        return Err(format!(
            "Scaleway API sync failed for all {core_request_count} attempted core API requests."
        ));
    }

    activity.push(ActivityEvent {
        id: "scaleway_sync_completed".into(),
        message: format!(
            "Scaleway inventory sync completed for project {}.",
            project.name
        ),
        timestamp: now(),
        event_type: "sync".into(),
        source: "Scaleway".into(),
    });

    let mut risks = compute
        .iter()
        .filter(|resource| resource.idle_cost_risk)
        .map(|resource| RiskFlag {
            id: format!("scw_idle_{}", resource.id),
            severity: "medium".into(),
            title: "Scaleway idle cost risk".into(),
            description: scaleway_idle_risk_description(&resource.resource_type, &resource.name),
            source: "Scaleway".into(),
            timestamp: now(),
        })
        .collect::<Vec<_>>();

    if object_storage_credentials_missing {
        risks.push(RiskFlag {
            id: "scaleway_object_storage_credentials_missing".into(),
            severity: "medium".into(),
            title: "Scaleway Object Storage buckets not inventoried".into(),
            description: "Live bucket listing requires a saved Scaleway Object Storage access key and secret key because buckets use S3 Signature V4.".into(),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    if action_failure_count > 0 {
        risks.push(RiskFlag {
            id: "scaleway_instance_actions_partial".into(),
            severity: "medium".into(),
            title: "Scaleway Instance actions partial".into(),
            description: format!(
                "{action_failure_count} Instance action lookup(s) failed. VM operation buttons may be hidden until the next sync."
            ),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    if storage_failure_count > 0 {
        risks.push(RiskFlag {
            id: "scaleway_storage_inventory_partial".into(),
            severity: "medium".into(),
            title: "Scaleway storage inventory partial".into(),
            description: format!(
                "{storage_failure_count} Block Storage volume/snapshot lookup(s) failed. Budget storage totals may be incomplete until the next sync."
            ),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    if let Some(file_risk) = scaleway_file_partial_risk(file_failure_count) {
        risks.push(file_risk);
    }

    if let Some(sql_risk) = scaleway_sql_partial_risk(sql_failure_count) {
        risks.push(sql_risk);
    }

    if object_storage_failure_count > 0 {
        risks.push(RiskFlag {
            id: "scaleway_object_storage_inventory_partial".into(),
            severity: "medium".into(),
            title: "Scaleway Object Storage inventory partial".into(),
            description: format!(
                "{object_storage_failure_count} Object Storage bucket listing request(s) failed. Bucket count may be incomplete until access key, preferred project, and IAM permissions are verified."
            ),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    if generative_credentials_missing {
        risks.push(RiskFlag {
            id: "scaleway_generative_api_credentials_missing".into(),
            severity: "medium".into(),
            title: "Scaleway Generative API models not inventoried".into(),
            description: "Listing Generative API models requires a saved Scaleway secret key (IAM API key) for Bearer authentication against api.scaleway.ai.".into(),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    if generative_api_failure_count > 0 {
        risks.push(RiskFlag {
            id: "scaleway_generative_api_inventory_partial".into(),
            severity: "medium".into(),
            title: "Scaleway Generative API inventory partial".into(),
            description: format!(
                "{generative_api_failure_count} Generative API model listing request(s) failed. Available model list may be incomplete until the secret key and IAM permissions are verified."
            ),
            source: "Scaleway".into(),
            timestamp: now(),
        });
    }

    // PURE decision: only the degrading domains gate status/message. File Storage,
    // Serverless SQL and the Generative API surface their OWN risk flags above and
    // must NOT degrade the provider (see `ScalewayInventoryCounters`).
    let (status, message) = scaleway_inventory_status(&ScalewayInventoryCounters {
        request_count,
        success_count,
        failure_count,
        action_failure_count,
        storage_failure_count,
        object_storage_failure_count,
        file_failure_count,
        sql_failure_count,
        generative_api_failure_count,
    });

    Ok(ProviderInventory {
        health: ProviderHealth {
            id: ProviderId::Scaleway,
            name: "Scaleway".into(),
            status: status.into(),
            last_sync: Some(now()),
            token_health: "valid".into(),
            credential_kind: Some("scaleway_project_api_token".into()),
            resource_count: compute.len() + storage.len(),
            message,
        },
        workers: Vec::new(),
        compute,
        storage,
        risks,
        activity,
        selected_scope,
    })
}

/// B3: redact ALL occurrences of known secret/token shapes in an error string,
/// in place, without truncating the surrounding text. Covers `Bearer <tok>`,
/// `X-Auth-Token: <tok>`, AWS SigV4 `Credential=<...>` / `Signature=<...>`,
/// GitHub `ghp_<...>` / `github_pat_<...>`, and Scaleway access keys
/// (`SCW...` / UUID-shaped).
// ===========================================================================
// Phase 4 — per-type safe-edit actions (AI Gateway, AutoRAG, KV, D1, R2).
// PURE decision logic is factored out and unit-tested; the async request fns
// degrade gracefully (typed error / `readable: false`) and never log secrets.
// ===========================================================================

/// PURE: classifies a SQL statement as a WRITE (mutating) or a read. Returns
/// `true` when, after stripping leading whitespace and SQL comments (`-- line`
/// and `/* block */`), the FIRST token is a mutating verb. We deliberately also
/// scan EVERY statement separated by `;`, so a leading benign `SELECT` cannot
/// smuggle a trailing `DELETE`. Verb match is case-insensitive and whole-word.
///
/// A statement whose first token is `WITH` (a common-table-expression) is a
/// special case: the actual operation lives AFTER the CTE definitions
/// (`WITH x AS (...) DELETE FROM t`). Since robustly skipping balanced CTE
/// parentheses across nested subqueries is fragile, we conservatively scan the
/// WHOLE statement's tokens for any mutating verb and classify it as a write
/// when one is present. A pure read-only CTE (`WITH x AS (SELECT 1) SELECT ...`)
/// contains no write verb and is therefore still classified as a read.
pub fn d1_sql_is_write(sql: &str) -> bool {
    const WRITE_VERBS: &[&str] = &[
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "REPLACE", "TRUNCATE", "MERGE",
        "PRAGMA", "ATTACH", "DETACH", "REINDEX", "VACUUM",
    ];
    let is_write_verb = |token: &str| {
        let upper = token.to_ascii_uppercase();
        WRITE_VERBS.iter().any(|verb| *verb == upper)
    };
    for statement in split_sql_statements(sql) {
        let stripped = strip_sql_leading_noise(&statement);
        let mut tokens = stripped
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|token| !token.is_empty());
        let Some(first) = tokens.next() else {
            continue;
        };
        if is_write_verb(first) {
            return true;
        }
        // EXPLAIN [QUERY PLAN] <stmt>: the first token is the harmless verb EXPLAIN,
        // so the verb/CTE checks below would never see the wrapped statement and a
        // mutating `EXPLAIN INSERT ...` would be misclassified as a read. Scan the
        // rest of this statement's tokens (which also covers `EXPLAIN QUERY PLAN`
        // and `EXPLAIN WITH ... DELETE`) and treat as a write if any write verb is
        // present anywhere (conservative-safe; a pure `EXPLAIN SELECT` stays read).
        if first.eq_ignore_ascii_case("EXPLAIN") && tokens.clone().any(is_write_verb) {
            return true;
        }
        // CTE: the mutating verb (if any) appears after the WITH clause. Scan the
        // rest of this statement's tokens; treat as a write if any write verb is
        // present anywhere in the statement (conservative-safe).
        if first.eq_ignore_ascii_case("WITH") && tokens.any(is_write_verb) {
            return true;
        }
    }
    false
}

/// Splits a SQL string on top-level `;` separators. String literals (single or
/// double quoted, with `''`/`""` escaping) are respected so a `;` inside a
/// literal does not split a statement. Empty fragments are skipped by the caller.
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while let Some(ch) = chars.next() {
        if in_line_comment {
            current.push(ch);
            if ch == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if in_block_comment {
            current.push(ch);
            if ch == '*' && chars.peek() == Some(&'/') {
                current.push(chars.next().unwrap());
                in_block_comment = false;
            }
            continue;
        }
        if in_single {
            current.push(ch);
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            current.push(ch);
            if ch == '"' {
                in_double = false;
            }
            continue;
        }
        match ch {
            '\'' => {
                in_single = true;
                current.push(ch);
            }
            '"' => {
                in_double = true;
                current.push(ch);
            }
            '-' if chars.peek() == Some(&'-') => {
                in_line_comment = true;
                current.push(ch);
            }
            '/' if chars.peek() == Some(&'*') => {
                in_block_comment = true;
                current.push(ch);
            }
            ';' => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    out.push(current);
    out
}

/// PURE: strips leading whitespace and any number of leading SQL comments
/// (`-- ...` to end of line, `/* ... */` blocks) from the FRONT of a statement,
/// returning the remainder so the caller can read the first real token.
fn strip_sql_leading_noise(statement: &str) -> String {
    let mut rest = statement.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = match after.find('\n') {
                Some(idx) => after[idx + 1..].trim_start(),
                None => "",
            };
            continue;
        }
        if let Some(after) = rest.strip_prefix("/*") {
            rest = match after.find("*/") {
                Some(idx) => after[idx + 2..].trim_start(),
                // Unterminated block comment: nothing executable follows.
                None => "",
            };
            continue;
        }
        break;
    }
    rest.to_string()
}

/// PURE: parses a live AI Gateway `result` object into the typed settings view.
fn ai_gateway_settings_from_value(
    account_id: &str,
    gateway_id: &str,
    result: &Value,
) -> CloudflareAiGatewaySettings {
    let u64_field = |key: &str| result.get(key).and_then(Value::as_u64);
    CloudflareAiGatewaySettings {
        account_id: account_id.to_string(),
        gateway_id: gateway_id.to_string(),
        cache_ttl: u64_field("cache_ttl"),
        cache_invalidate_on_update: result
            .get("cache_invalidate_on_update")
            .and_then(Value::as_bool),
        collect_logs: result.get("collect_logs").and_then(Value::as_bool),
        logpush: result.get("logpush").and_then(Value::as_bool),
        rate_limiting_interval: u64_field("rate_limiting_interval"),
        rate_limiting_limit: u64_field("rate_limiting_limit"),
        rate_limiting_technique: result
            .get("rate_limiting_technique")
            .and_then(Value::as_str)
            .map(str::to_string),
        readable: true,
        message: None,
    }
}

/// PURE: builds the LOSSLESS PUT body for an AI Gateway update. The gateway
/// `PUT` is a FULL-OBJECT REPLACE (confirmed against the Cloudflare API ref), so
/// we start from the re-fetched live `result`, drop server-managed/read-only
/// keys CF rejects on write (`id`, `created_at`, `modified_at`, `account_id`,
/// `account_tag`, `internal_id`), and overlay ONLY the caller's edited fields.
/// The five documented REQUIRED fields (`cache_ttl`, `cache_invalidate_on_update`,
/// `collect_logs`, `rate_limiting_interval`, `rate_limiting_limit`) are
/// backfilled with safe defaults ONLY when the live object omitted them AND the
/// caller did not set them, so the replace can never 400 on a missing required
/// field while still preserving every other live setting verbatim.
///
/// PRIVACY: `collect_logs` is a REQUIRED field on the PUT (CF rejects the
/// full-replace without it), but it controls request/response logging. We
/// therefore backfill it FAIL-SAFE to `false` — never `true` — so a sparse live
/// object can never cause this command to silently ENABLE logging the operator
/// did not ask for. The only way `collect_logs` becomes `true` is the live
/// object already had it `true`, or the caller's patch explicitly set it.
fn ai_gateway_lossless_put_body(
    live_result: &Value,
    patch: &CloudflareAiGatewaySettingsPatch,
) -> Result<Value, String> {
    const READ_ONLY_KEYS: &[&str] = &[
        "id",
        "created_at",
        "modified_at",
        "account_id",
        "account_tag",
        "internal_id",
    ];
    // REFUSE on an unexpected shape rather than silently defaulting to a blank
    // object: this PUT is a FULL-OBJECT REPLACE, so building the body from `{}`
    // would RESET every gateway setting. The production caller already
    // `.filter(is_object)`s before calling, so this is defence-in-depth that also
    // mirrors the worker-bindings "refuse on unexpected shape" hardening.
    let Some(mut map) = live_result.as_object().cloned() else {
        return Err(
            "AI Gateway live settings were not an object; refusing to build the write.".into(),
        );
    };
    for key in READ_ONLY_KEYS {
        map.remove(*key);
    }
    if let Some(value) = patch.cache_ttl {
        map.insert("cache_ttl".into(), json!(value));
    }
    if let Some(value) = patch.cache_invalidate_on_update {
        map.insert("cache_invalidate_on_update".into(), json!(value));
    }
    if let Some(value) = patch.collect_logs {
        map.insert("collect_logs".into(), json!(value));
    }
    if let Some(value) = patch.logpush {
        map.insert("logpush".into(), json!(value));
    }
    if let Some(value) = patch.rate_limiting_interval {
        map.insert("rate_limiting_interval".into(), json!(value));
    }
    if let Some(value) = patch.rate_limiting_limit {
        map.insert("rate_limiting_limit".into(), json!(value));
    }
    if let Some(value) = patch.rate_limiting_technique.as_deref() {
        map.insert("rate_limiting_technique".into(), json!(value));
    }
    // Backfill REQUIRED fields the live object never had (so a freshly-created
    // gateway with omitted defaults still passes the full-replace contract).
    for (key, default) in [
        ("cache_ttl", json!(0)),
        ("cache_invalidate_on_update", json!(false)),
        // FAIL-SAFE: never enable logging implicitly (see doc comment / privacy).
        ("collect_logs", json!(false)),
        ("rate_limiting_interval", json!(0)),
        ("rate_limiting_limit", json!(0)),
    ] {
        map.entry(key.to_string()).or_insert(default);
    }
    Ok(Value::Object(map))
}

/// PURE: flattens a D1 `result[0]` object into (columns, capped rows, row_count,
/// truncated, rows_read, rows_written). Column order is taken from the first row;
/// cells are stringified (numbers/bools as text, null as empty, objects/arrays as
/// compact JSON) so the UI needs no typed schema. Rows beyond `CF_D1_MAX_ROWS`
/// are dropped and `truncated` is set.
#[allow(clippy::type_complexity)]
fn d1_rows_from_result(
    first: &Value,
) -> (
    Vec<String>,
    Vec<Vec<String>>,
    usize,
    bool,
    Option<u64>,
    Option<u64>,
) {
    let meta = first.get("meta");
    let rows_read = meta
        .and_then(|m| m.get("rows_read"))
        .and_then(Value::as_u64);
    let rows_written = meta
        .and_then(|m| m.get("rows_written"))
        .and_then(Value::as_u64);
    let results = first.get("results").and_then(Value::as_array);
    let Some(results) = results else {
        return (Vec::new(), Vec::new(), 0, false, rows_read, rows_written);
    };
    // Column order: union of keys preserving first-seen order across rows so a
    // sparse first row does not hide later columns.
    let mut columns: Vec<String> = Vec::new();
    for row in results {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !columns.iter().any(|existing| existing == key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    let total = results.len();
    let truncated = total > CF_D1_MAX_ROWS;
    let rows = results
        .iter()
        .take(CF_D1_MAX_ROWS)
        .map(|row| {
            columns
                .iter()
                .map(|col| row.get(col).map(d1_cell_to_string).unwrap_or_default())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    (columns, rows, total, truncated, rows_read, rows_written)
}

/// PURE: stringifies a single D1 cell without leaking quotes for plain scalars.
fn d1_cell_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Reads the editable AI Gateway settings. Degrades to `readable: false` on any
/// failure. Caller MUST have proven the Aspis-Bio account scope first.
pub async fn fetch_cloudflare_ai_gateway_settings(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    gateway_id: &str,
) -> CloudflareAiGatewaySettings {
    let unreadable = |message: &str| CloudflareAiGatewaySettings {
        account_id: account_id.to_string(),
        gateway_id: gateway_id.to_string(),
        cache_ttl: None,
        cache_invalidate_on_update: None,
        collect_logs: None,
        logpush: None,
        rate_limiting_interval: None,
        rate_limiting_limit: None,
        rate_limiting_technique: None,
        readable: false,
        message: Some(message.to_string()),
    };
    let encoded_gateway = urlencoding::encode(gateway_id);
    let url = format!("{CF_API}/accounts/{account_id}/ai-gateway/gateways/{encoded_gateway}");
    let response = match http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return unreadable("AI Gateway request failed."),
    };
    let status = response.status();
    if !status.is_success() {
        return unreadable(if status.as_u16() == 401 || status.as_u16() == 403 {
            "AI Gateway is not readable with the current token permissions."
        } else if status.as_u16() == 404 {
            "AI Gateway was not found."
        } else {
            "AI Gateway endpoint returned an error."
        });
    }
    let Ok(payload) = response.json::<Value>().await else {
        return unreadable("AI Gateway response was invalid.");
    };
    if payload
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        return unreadable("AI Gateway settings could not be read.");
    }
    let Some(result) = payload.get("result").filter(|value| value.is_object()) else {
        return unreadable("AI Gateway response was missing the expected settings.");
    };
    ai_gateway_settings_from_value(account_id, gateway_id, result)
}

/// LOSSLESS AI Gateway update: re-fetch the live object, overlay only the edited
/// fields, PUT the full object back. Caller MUST have proven the Aspis-Bio scope
/// and re-asserted the sensitive session. Never logs the gateway body.
pub async fn put_cloudflare_ai_gateway_settings(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    gateway_id: &str,
    patch: &CloudflareAiGatewaySettingsPatch,
) -> Result<(), String> {
    let encoded_gateway = urlencoding::encode(gateway_id);
    let url = format!("{CF_API}/accounts/{account_id}/ai-gateway/gateways/{encoded_gateway}");
    // 1. Re-fetch the RAW gateway immediately before writing.
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Gateway re-read failed: {e}")))?
        .error_for_status()
        .map_err(|e| sanitize_error_message(&format!("AI Gateway re-read rejected: {e}")))?;
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Gateway response invalid: {e}")))?;
    if payload
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        let detail = cf_envelope_error_message(&payload);
        return Err(format!(
            "AI Gateway could not be re-read before write.{detail}"
        ));
    }
    let Some(live_result) = payload.get("result").filter(|value| value.is_object()) else {
        // Refuse to PUT against an ambiguous re-read; a missing/non-object result
        // would mean rebuilding the object from nothing and could wipe settings.
        return Err("AI Gateway re-read returned no settings object; refusing to write.".into());
    };
    let body = ai_gateway_lossless_put_body(live_result, patch)?;
    let put = http
        .put(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Gateway write failed: {e}")))?;
    let status = put.status();
    let put_payload: Value = put
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Gateway write response invalid: {e}")))?;
    let envelope_ok = put_payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&put_payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the AI Gateway write ({status}).{detail}"
        )));
    }
    Ok(())
}

/// Triggers an AI Search (AutoRAG) sync/reindex job for an instance via
/// `POST /accounts/{id}/ai-search/instances/{name}/jobs`. Returns the job id when
/// present. Caller MUST have proven the Aspis-Bio scope first. A trigger, not a
/// destructive replace.
pub async fn trigger_cloudflare_autorag_reindex(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    instance_id: &str,
) -> Result<CloudflareAutoragReindexResult, String> {
    let encoded_instance = urlencoding::encode(instance_id);
    let url = format!("{CF_API}/accounts/{account_id}/ai-search/instances/{encoded_instance}/jobs");
    let response = http
        .post(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Search sync request failed: {e}")))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Search sync response invalid: {e}")))?;
    let envelope_ok = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the AI Search sync ({status}).{detail}"
        )));
    }
    let result = payload.get("result").unwrap_or(&payload);
    let job_id = string_field(result, &["id", "job_id", "jobId"]);
    Ok(CloudflareAutoragReindexResult {
        instance_id: instance_id.to_string(),
        job_id,
        triggered_at: now(),
        message: format!("Triggered AI Search sync for {instance_id}."),
    })
}

/// Confirms an AI Search instance exists in the proven account before acting.
/// CF's single-instance GET is namespaced (`.../ai-search/namespaces/{ns}/
/// instances/{id}`) and we do not carry the namespace here, so we PAGINATE the
/// flat list (`GET .../ai-search/instances?page=N&per_page=100`) following
/// `result_info` until the instance is found or the list is exhausted (bounded by
/// `CF_EXISTS_MAX_PAGES`). This avoids the page-1-only false negative for an
/// account with more than 100 instances. Any non-2xx / network failure ⇒ `Err`
/// so the caller FAILS CLOSED.
pub async fn cloudflare_autorag_instance_exists(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    instance_id: &str,
) -> Result<bool, String> {
    for page in 1..=CF_EXISTS_MAX_PAGES {
        let url = format!(
            "{CF_API}/accounts/{account_id}/ai-search/instances?page={page}&per_page={CF_EXISTS_PAGE_SIZE}"
        );
        let response = http
            .get(&url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| sanitize_error_message(&format!("AI Search list failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(sanitize_error_message(&format!(
                "AI Search instance existence check returned an unexpected status ({status})."
            )));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|e| sanitize_error_message(&format!("AI Search list invalid: {e}")))?;
        let items = json_result_items(&payload, &["instances", "result"]).unwrap_or_default();
        // `string_field(item, &["id", "name"])` already falls back to `name`, so a
        // separate name-only check would be redundant.
        if items
            .iter()
            .any(|item| string_field(item, &["id", "name"]).as_deref() == Some(instance_id))
        {
            return Ok(true);
        }
        // Stop when this page returned fewer than a full page, or when result_info
        // reports we have seen every instance.
        if items.len() < CF_EXISTS_PAGE_SIZE as usize {
            break;
        }
        if let Some(info) = payload.get("result_info") {
            let seen = page * CF_EXISTS_PAGE_SIZE;
            if let Some(total) = info.get("total_count").and_then(Value::as_u64) {
                if seen >= total {
                    break;
                }
            }
        }
    }
    Ok(false)
}

/// Confirms an AI Gateway exists in the proven account before acting. CF exposes
/// a direct by-id read (`GET .../ai-gateway/gateways/{gateway_id}`): 200 ⇒ exists,
/// 404 ⇒ does not exist, any other status / network failure ⇒ `Err` so the caller
/// FAILS CLOSED and refuses to act against an unverified gateway. Mirrors the
/// other `*_exists` helpers; never logs secrets.
pub async fn cloudflare_ai_gateway_exists(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    gateway_id: &str,
) -> Result<bool, String> {
    let encoded_gateway = urlencoding::encode(gateway_id);
    let url = format!("{CF_API}/accounts/{account_id}/ai-gateway/gateways/{encoded_gateway}");
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("AI Gateway existence check failed: {e}")))?;
    let status = response.status();
    if status.is_success() {
        // FAIL-CLOSED: a 200 can still carry a logical `success:false` envelope
        // (Cloudflare returns these for some error classes). Treating that as
        // "exists" would let a destructive action proceed against an unverified
        // resource, so parse the body and refuse on an explicit failure.
        let payload: Value = response.json().await.map_err(|e| {
            sanitize_error_message(&format!("AI Gateway existence response invalid: {e}"))
        })?;
        if payload.get("success").and_then(Value::as_bool) == Some(false) {
            let detail = cf_envelope_error_message(&payload);
            return Err(sanitize_error_message(&format!(
                "AI Gateway existence check returned a Cloudflare error.{detail}"
            )));
        }
        return Ok(true);
    }
    if status.as_u16() == 404 {
        return Ok(false);
    }
    Err(sanitize_error_message(&format!(
        "AI Gateway existence check returned an unexpected status ({status})."
    )))
}

/// Lists a single capped page of KV keys for a namespace. Caller MUST have proven
/// the Aspis-Bio scope first. `prefix` and `cursor` are optional.
pub async fn fetch_cloudflare_kv_keys(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    namespace_id: &str,
    prefix: Option<&str>,
    cursor: Option<&str>,
) -> Result<CloudflareKvKeysPage, String> {
    let encoded_ns = urlencoding::encode(namespace_id);
    let mut url = format!(
        "{CF_API}/accounts/{account_id}/storage/kv/namespaces/{encoded_ns}/keys?limit={CF_KV_KEY_PAGE_LIMIT}"
    );
    if let Some(prefix) = prefix.map(str::trim).filter(|value| !value.is_empty()) {
        url.push_str(&format!("&prefix={}", urlencoding::encode(prefix)));
    }
    if let Some(cursor) = cursor.map(str::trim).filter(|value| !value.is_empty()) {
        url.push_str(&format!("&cursor={}", urlencoding::encode(cursor)));
    }
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV keys request failed: {e}")))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV keys response invalid: {e}")))?;
    let envelope_ok = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the KV keys read ({status}).{detail}"
        )));
    }
    let keys = json_result_items(&payload, &["result"])
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let name = string_field(item, &["name"])?;
            let metadata = item
                .get("metadata")
                .filter(|value| !value.is_null())
                .map(|value| value.to_string());
            Some(CloudflareKvKey {
                name,
                expiration: item.get("expiration").and_then(Value::as_i64),
                metadata,
            })
        })
        .collect::<Vec<_>>();
    let result_info = payload.get("result_info");
    let cursor = result_info
        .and_then(|info| info.get("cursor"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty());
    Ok(CloudflareKvKeysPage {
        namespace_id: namespace_id.to_string(),
        keys,
        list_complete: cursor.is_none(),
        cursor,
    })
}

/// Reads a single KV value. The value is capped at `CF_KV_VALUE_MAX_BYTES`;
/// larger values are truncated (on a UTF-8 char boundary) and `truncated` set.
pub async fn fetch_cloudflare_kv_value(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    namespace_id: &str,
    key: &str,
) -> Result<CloudflareKvValue, String> {
    let encoded_ns = urlencoding::encode(namespace_id);
    let encoded_key = urlencoding::encode(key);
    let url = format!(
        "{CF_API}/accounts/{account_id}/storage/kv/namespaces/{encoded_ns}/values/{encoded_key}"
    );
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV value request failed: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the KV value read ({status})."
        )));
    }
    // The values endpoint returns the RAW stored bytes, not a JSON envelope.
    let bytes = response
        .bytes()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV value body invalid: {e}")))?;
    let (value, truncated) = if bytes.len() > CF_KV_VALUE_MAX_BYTES {
        // Truncate at the cap. `from_utf8_lossy` replaces any partial UTF-8
        // sequence split at the boundary with U+FFFD, so we never panic or emit
        // invalid UTF-8 even if the cut lands mid-character.
        (
            String::from_utf8_lossy(&bytes[..CF_KV_VALUE_MAX_BYTES]).into_owned(),
            true,
        )
    } else {
        (String::from_utf8_lossy(&bytes).into_owned(), false)
    };
    Ok(CloudflareKvValue {
        namespace_id: namespace_id.to_string(),
        key: key.to_string(),
        value,
        truncated,
    })
}

/// Writes (PUT) a single KV value as a raw `text/plain` body. Caller MUST have
/// proven the Aspis-Bio scope and re-asserted the sensitive session.
pub async fn put_cloudflare_kv_value(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    namespace_id: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let encoded_ns = urlencoding::encode(namespace_id);
    let encoded_key = urlencoding::encode(key);
    let url = format!(
        "{CF_API}/accounts/{account_id}/storage/kv/namespaces/{encoded_ns}/values/{encoded_key}"
    );
    let response = http
        .put(&url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "text/plain")
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .body(value.to_string())
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV value write failed: {e}")))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV value write response invalid: {e}")))?;
    let envelope_ok = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the KV value write ({status}).{detail}"
        )));
    }
    Ok(())
}

/// Deletes a single KV value. Destructive; caller MUST require a confirm token
/// equal to the key and re-assert the sensitive session before calling this.
pub async fn delete_cloudflare_kv_value(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    namespace_id: &str,
    key: &str,
) -> Result<(), String> {
    let encoded_ns = urlencoding::encode(namespace_id);
    let encoded_key = urlencoding::encode(key);
    let url = format!(
        "{CF_API}/accounts/{account_id}/storage/kv/namespaces/{encoded_ns}/values/{encoded_key}"
    );
    let response = http
        .delete(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV value delete failed: {e}")))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV value delete response invalid: {e}")))?;
    let envelope_ok = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the KV value delete ({status}).{detail}"
        )));
    }
    Ok(())
}

/// Confirms a KV namespace exists in the proven account via the DIRECT by-id read
/// (`GET .../storage/kv/namespaces/{namespace_id}`): 200 ⇒ exists, 404 ⇒ absent,
/// any other status / network failure ⇒ `Err` so the caller FAILS CLOSED. A
/// direct GET avoids the page-1-only list bug (a valid namespace beyond the first
/// 100 used to be wrongly refused).
pub async fn cloudflare_kv_namespace_exists(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    namespace_id: &str,
) -> Result<bool, String> {
    let encoded_ns = urlencoding::encode(namespace_id);
    let url = format!("{CF_API}/accounts/{account_id}/storage/kv/namespaces/{encoded_ns}");
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("KV namespace check failed: {e}")))?;
    let status = response.status();
    if status.is_success() {
        // FAIL-CLOSED: a 200 with a logical `success:false` envelope must refuse the
        // action, not be treated as "exists" (see cloudflare_ai_gateway_exists).
        let payload: Value = response.json().await.map_err(|e| {
            sanitize_error_message(&format!("KV namespace existence response invalid: {e}"))
        })?;
        if payload.get("success").and_then(Value::as_bool) == Some(false) {
            let detail = cf_envelope_error_message(&payload);
            return Err(sanitize_error_message(&format!(
                "KV namespace existence check returned a Cloudflare error.{detail}"
            )));
        }
        return Ok(true);
    }
    if status.as_u16() == 404 {
        return Ok(false);
    }
    Err(sanitize_error_message(&format!(
        "KV namespace existence check returned an unexpected status ({status})."
    )))
}

/// Runs a D1 query. WRITE detection is the caller's responsibility (it owns the
/// confirm gate); this fn assumes execution is allowed and POSTs the statement.
/// Caller MUST have proven the Aspis-Bio scope first.
pub async fn run_cloudflare_d1_query(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    database_id: &str,
    sql: &str,
    is_write: bool,
) -> Result<CloudflareD1QueryResult, String> {
    let encoded_db = urlencoding::encode(database_id);
    let url = format!("{CF_API}/accounts/{account_id}/d1/database/{encoded_db}/query");
    let timeout = if is_write {
        CF_WRITE_TIMEOUT_SECS
    } else {
        CF_DEPLOYMENT_TIMEOUT_SECS
    };
    let response = http
        .post(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(timeout))
        .json(&json!({ "sql": sql }))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("D1 query request failed: {e}")))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("D1 query response invalid: {e}")))?;
    let envelope_ok = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the D1 query ({status}).{detail}"
        )));
    }
    // `result` is an array of statement results; we surface the FIRST.
    let first = payload
        .get("result")
        .and_then(Value::as_array)
        .and_then(|results| results.first())
        .cloned()
        .unwrap_or(Value::Null);
    let (columns, rows, row_count, truncated, rows_read, rows_written) =
        d1_rows_from_result(&first);
    Ok(CloudflareD1QueryResult {
        database_id: database_id.to_string(),
        is_write,
        requires_confirmation: false,
        executed: true,
        columns,
        rows,
        row_count,
        truncated,
        rows_read,
        rows_written,
        message: format!("Executed D1 query against {database_id}."),
    })
}

/// Confirms a D1 database exists in the proven account via the DIRECT by-id read
/// (`GET .../d1/database/{database_id}`): 200 ⇒ exists, 404 ⇒ absent, any other
/// status / network failure ⇒ `Err` so the caller FAILS CLOSED. A direct GET
/// avoids the page-1-only list bug (a valid database beyond the first 100 used to
/// be wrongly refused).
pub async fn cloudflare_d1_database_exists(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    database_id: &str,
) -> Result<bool, String> {
    let encoded_db = urlencoding::encode(database_id);
    let url = format!("{CF_API}/accounts/{account_id}/d1/database/{encoded_db}");
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("D1 database check failed: {e}")))?;
    let status = response.status();
    if status.is_success() {
        // FAIL-CLOSED: a 200 with a logical `success:false` envelope must refuse the
        // action, not be treated as "exists" (see cloudflare_ai_gateway_exists).
        let payload: Value = response.json().await.map_err(|e| {
            sanitize_error_message(&format!("D1 database existence response invalid: {e}"))
        })?;
        if payload.get("success").and_then(Value::as_bool) == Some(false) {
            let detail = cf_envelope_error_message(&payload);
            return Err(sanitize_error_message(&format!(
                "D1 database existence check returned a Cloudflare error.{detail}"
            )));
        }
        return Ok(true);
    }
    if status.as_u16() == 404 {
        return Ok(false);
    }
    Err(sanitize_error_message(&format!(
        "D1 database existence check returned an unexpected status ({status})."
    )))
}

/// Reads R2 bucket lifecycle + CORS in one struct. Each is independently degraded
/// (`*_readable: false`) so a bucket with no CORS rules (CF 404s CORS) still
/// returns its lifecycle. Caller MUST have proven the Aspis-Bio scope first.
pub async fn fetch_cloudflare_r2_config(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    bucket: &str,
) -> CloudflareR2Config {
    let encoded_bucket = urlencoding::encode(bucket);
    let lifecycle = fetch_r2_subresource(
        http,
        token,
        &format!("{CF_API}/accounts/{account_id}/r2/buckets/{encoded_bucket}/lifecycle"),
    )
    .await;
    let cors = fetch_r2_subresource(
        http,
        token,
        &format!("{CF_API}/accounts/{account_id}/r2/buckets/{encoded_bucket}/cors"),
    )
    .await;
    let mut messages = Vec::new();
    if lifecycle.is_none() {
        messages.push("lifecycle not readable".to_string());
    }
    if cors.is_none() {
        messages.push("CORS not readable or not set".to_string());
    }
    CloudflareR2Config {
        bucket: bucket.to_string(),
        lifecycle_readable: lifecycle.is_some(),
        cors_readable: cors.is_some(),
        lifecycle_rules: lifecycle.unwrap_or(Value::Null),
        cors_rules: cors.unwrap_or(Value::Null),
        message: if messages.is_empty() {
            None
        } else {
            Some(messages.join("; "))
        },
    }
}

/// Reads one R2 subresource envelope, returning its `result` on success or `None`
/// on any failure (so a missing CORS config degrades, never errors the whole read).
async fn fetch_r2_subresource(http: &reqwest::Client, token: &str, url: &str) -> Option<Value> {
    let response = http
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: Value = response.json().await.ok()?;
    if payload
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        return None;
    }
    payload.get("result").cloned()
}

/// Writes (PUT) an R2 lifecycle OR CORS configuration. `target` is "lifecycle" or
/// "cors"; `rules` is sent as `{ "rules": <rules> }`. Caller MUST have proven the
/// Aspis-Bio scope and re-asserted the sensitive session.
pub async fn put_cloudflare_r2_config(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    bucket: &str,
    target: &str,
    rules: &Value,
) -> Result<(), String> {
    // HARD PRECONDITION: `target` is interpolated raw into the request path, so it
    // must be one of the two known R2 subresources. Today every caller passes a
    // literal, but the fn is `pub`; guard against a future caller passing attacker-
    // influenced input that could reach an unintended R2 endpoint.
    if target != "lifecycle" && target != "cors" {
        return Err("Unsupported R2 config target.".to_string());
    }
    let encoded_bucket = urlencoding::encode(bucket);
    let url = format!("{CF_API}/accounts/{account_id}/r2/buckets/{encoded_bucket}/{target}");
    let body = json!({ "rules": rules });
    let response = http
        .put(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_WRITE_TIMEOUT_SECS))
        .json(&body)
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("R2 {target} write failed: {e}")))?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| sanitize_error_message(&format!("R2 {target} write response invalid: {e}")))?;
    let envelope_ok = payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(status.is_success());
    if !status.is_success() || !envelope_ok {
        let detail = cf_envelope_error_message(&payload);
        return Err(sanitize_error_message(&format!(
            "Cloudflare rejected the R2 {target} write ({status}).{detail}"
        )));
    }
    Ok(())
}

/// Confirms an R2 bucket exists in the proven account via the DIRECT by-name read
/// (`GET .../r2/buckets/{bucket}`): 200 ⇒ exists, 404 ⇒ absent, any other status /
/// network failure ⇒ `Err` so the caller FAILS CLOSED. A direct GET avoids the
/// page-1-only list bug (a valid bucket beyond the first page used to be wrongly
/// refused).
pub async fn cloudflare_r2_bucket_exists(
    http: &reqwest::Client,
    token: &str,
    account_id: &str,
    bucket: &str,
) -> Result<bool, String> {
    let encoded_bucket = urlencoding::encode(bucket);
    let url = format!("{CF_API}/accounts/{account_id}/r2/buckets/{encoded_bucket}");
    let response = http
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(CF_DEPLOYMENT_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| sanitize_error_message(&format!("R2 bucket check failed: {e}")))?;
    let status = response.status();
    if status.is_success() {
        // FAIL-CLOSED: a 200 with a logical `success:false` envelope must refuse the
        // action, not be treated as "exists" (see cloudflare_ai_gateway_exists).
        let payload: Value = response.json().await.map_err(|e| {
            sanitize_error_message(&format!("R2 bucket existence response invalid: {e}"))
        })?;
        if payload.get("success").and_then(Value::as_bool) == Some(false) {
            let detail = cf_envelope_error_message(&payload);
            return Err(sanitize_error_message(&format!(
                "R2 bucket existence check returned a Cloudflare error.{detail}"
            )));
        }
        return Ok(true);
    }
    if status.as_u16() == 404 {
        return Ok(false);
    }
    Err(sanitize_error_message(&format!(
        "R2 bucket existence check returned an unexpected status ({status})."
    )))
}

pub fn sanitize_error_message(message: &str) -> String {
    let mut out = message.to_string();
    // Header-style markers: redact the token value that follows the marker
    // (optionally preceded by ':' and whitespace).
    out = redact_after_marker(&out, "Bearer");
    out = redact_after_marker(&out, "X-Auth-Token");
    // key=value markers (AWS SigV4 style), case-sensitive keys.
    out = redact_after_eq(&out, "Credential");
    out = redact_after_eq(&out, "Signature");
    // Prefixed tokens anywhere in the string.
    out = redact_prefixed_token(&out, "github_pat_");
    out = redact_prefixed_token(&out, "ghp_");
    out = redact_prefixed_token(&out, "SCW");
    // UUID-shaped Scaleway access keys (e.g. 11111111-2222-3333-4444-555555555555).
    out = redact_uuids(&out);
    out
}

/// Returns true for characters that can appear inside an opaque token value.
fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | '+' | '=')
}

/// Redact every `<marker>[: ]<token>` occurrence, keeping the marker.
fn redact_after_marker(input: &str, marker: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find(marker) {
        out.push_str(&rest[..pos + marker.len()]);
        let after = &rest[pos + marker.len()..];
        // Skip an optional ':' and any leading whitespace before the token.
        let mut chars = after.char_indices().peekable();
        let mut value_start = 0usize;
        let mut saw_sep = false;
        while let Some(&(idx, ch)) = chars.peek() {
            if ch == ':' && !saw_sep {
                saw_sep = true;
                chars.next();
                continue;
            }
            if ch.is_whitespace() {
                chars.next();
                continue;
            }
            value_start = idx;
            break;
        }
        // Find the end of the token value.
        let value_region = &after[value_start..];
        let value_len = value_region
            .char_indices()
            .take_while(|(_, ch)| is_token_char(*ch))
            .map(|(i, ch)| i + ch.len_utf8())
            .last()
            .unwrap_or(0);
        if value_len == 0 {
            // No token value followed the marker; leave the separator text as-is.
            rest = after;
            continue;
        }
        out.push_str(&after[..value_start]);
        out.push_str("[redacted]");
        rest = &value_region[value_len..];
    }
    out.push_str(rest);
    out
}

/// Redact every `<key>=<token>` occurrence, keeping `<key>=`.
fn redact_after_eq(input: &str, key: &str) -> String {
    let needle = format!("{key}=");
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find(&needle) {
        out.push_str(&rest[..pos + needle.len()]);
        let after = &rest[pos + needle.len()..];
        let value_len = after
            .char_indices()
            .take_while(|(_, ch)| is_token_char(*ch))
            .map(|(i, ch)| i + ch.len_utf8())
            .last()
            .unwrap_or(0);
        if value_len == 0 {
            rest = after;
            continue;
        }
        out.push_str("[redacted]");
        rest = &after[value_len..];
    }
    out.push_str(rest);
    out
}

/// Redact every token that begins with `prefix` (e.g. `ghp_...`, `SCW...`).
fn redact_prefixed_token(input: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find(prefix) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let token_len = after
            .char_indices()
            .take_while(|(_, ch)| is_token_char(*ch))
            .map(|(i, ch)| i + ch.len_utf8())
            .last()
            .unwrap_or(0);
        // Only redact if there is at least one more char beyond the bare prefix.
        if token_len > prefix.len() {
            out.push_str("[redacted]");
            rest = &after[token_len..];
        } else {
            out.push_str(&after[..prefix.len()]);
            rest = &after[prefix.len()..];
        }
    }
    out.push_str(rest);
    out
}

/// Redact UUID-shaped substrings (8-4-4-4-12 hex), used by Scaleway access keys.
fn redact_uuids(input: &str) -> String {
    fn is_uuid_at(bytes: &[u8]) -> bool {
        const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
        let mut idx = 0usize;
        for (g, len) in GROUPS.iter().enumerate() {
            for _ in 0..*len {
                match bytes.get(idx) {
                    Some(b) if b.is_ascii_hexdigit() => idx += 1,
                    _ => return false,
                }
            }
            if g < GROUPS.len() - 1 {
                match bytes.get(idx) {
                    Some(b'-') => idx += 1,
                    _ => return false,
                }
            }
        }
        true
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // Anchor on a word boundary so we don't redact inside a longer token.
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if boundary && is_uuid_at(&bytes[i..]) {
            // Ensure it is not immediately followed by another token char.
            let end = i + 36;
            let followed = bytes.get(end).map(|b| b.is_ascii_alphanumeric()) == Some(true);
            if !followed {
                out.push_str("[redacted]");
                i = end;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_bindings_updates_target_converts_secrets_and_preserves_rest() {
        let raw = vec![
            json!({ "type": "plain_text", "name": "API_BASE", "text": "old" }),
            json!({ "type": "secret_text", "name": "SIGNING_KEY" }),
            json!({
                "type": "kv_namespace",
                "name": "CACHE",
                "namespace_id": "abc123"
            }),
            json!({
                "type": "r2_bucket",
                "name": "ASSETS",
                "bucket_name": "aspis-assets"
            }),
            json!({
                "type": "durable_object_namespace",
                "name": "ROOMS",
                "class_name": "Room",
                "script_name": "rooms-worker"
            }),
        ];

        let out = rewrite_bindings_for_plain_text(&raw, "API_BASE", "new").unwrap();

        // Existing plain_text target is updated in place (order preserved).
        assert_eq!(
            out[0],
            json!({ "type": "plain_text", "name": "API_BASE", "text": "new" })
        );
        // secret_text -> inherit, value dropped.
        assert_eq!(out[1], json!({ "type": "inherit", "name": "SIGNING_KEY" }));
        // KV / R2 / DO bindings survive byte-for-byte.
        assert_eq!(out[2], raw[2]);
        assert_eq!(out[3], raw[3]);
        assert_eq!(out[4], raw[4]);
        // No appended binding when the target already existed.
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn rewrite_bindings_appends_new_var_when_absent() {
        let raw = vec![
            json!({ "type": "secret_text", "name": "SIGNING_KEY" }),
            json!({ "type": "kv_namespace", "name": "CACHE", "namespace_id": "abc123" }),
        ];

        let out = rewrite_bindings_for_plain_text(&raw, "FEATURE_FLAG", "on").unwrap();

        // Secret converted, KV preserved, new var appended last.
        assert_eq!(out[0], json!({ "type": "inherit", "name": "SIGNING_KEY" }));
        assert_eq!(out[1], raw[1]);
        assert_eq!(
            out[2],
            json!({ "type": "plain_text", "name": "FEATURE_FLAG", "text": "on" })
        );
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn rewrite_bindings_preserves_sibling_fields_on_target_plain_text() {
        // CF may carry extra fields on a plain_text binding; only `text` changes.
        let raw = vec![json!({
            "type": "plain_text",
            "name": "API_BASE",
            "text": "old",
            "extra": "keep-me"
        })];

        let out = rewrite_bindings_for_plain_text(&raw, "API_BASE", "new").unwrap();

        assert_eq!(
            out[0],
            json!({
                "type": "plain_text",
                "name": "API_BASE",
                "text": "new",
                "extra": "keep-me"
            })
        );
    }

    #[test]
    fn cloudflare_billing_plan_maps_rate_plan_and_subscription_price() {
        // rate_plan carries id/public_name/currency; price+frequency are on the
        // subscription object itself (the rate_plan has no price).
        let subscription = json!({
            "id": "sub-123",
            "currency": "EUR",
            "frequency": "monthly",
            "price": 20.0,
            "state": "Paid",
            "rate_plan": {
                "id": "business",
                "public_name": "Business Plan",
                "currency": "USD"
            }
        });

        let plan = cloudflare_billing_plan_from_subscription(&subscription);

        assert_eq!(plan.id.as_deref(), Some("business"));
        assert_eq!(plan.name.as_deref(), Some("Business Plan"));
        // Subscription currency wins over rate_plan currency.
        assert_eq!(plan.currency.as_deref(), Some("EUR"));
        assert_eq!(plan.frequency.as_deref(), Some("monthly"));
        assert_eq!(plan.price, Some(20.0));
        assert_eq!(plan.component_summary.as_deref(), Some("Paid"));
    }

    #[test]
    fn cloudflare_billing_plan_tolerates_missing_fields_and_string_price() {
        // No rate_plan, price as a numeric string (some payloads do this).
        let subscription = json!({ "price": "5.00" });

        let plan = cloudflare_billing_plan_from_subscription(&subscription);

        assert!(plan.id.is_none());
        assert!(plan.name.is_none());
        assert!(plan.currency.is_none());
        assert!(plan.frequency.is_none());
        assert_eq!(plan.price, Some(5.0));
        assert!(plan.component_summary.is_none());
    }

    #[test]
    fn cloudflare_invoice_maps_history_item() {
        let item = json!({
            "id": "b69a9f3492637782896352daae219e7d",
            "action": "subscription",
            "amount": 20.99,
            "currency": "USD",
            "description": "The billing item description",
            "occurred_at": "2014-03-01T12:21:59.3456Z",
            "type": "charge"
        });

        let invoice = cloudflare_invoice_from_history(&item);

        assert_eq!(
            invoice.id.as_deref(),
            Some("b69a9f3492637782896352daae219e7d")
        );
        assert_eq!(
            invoice.occurred_at.as_deref(),
            Some("2014-03-01T12:21:59.3456Z")
        );
        assert_eq!(invoice.amount, Some(20.99));
        assert_eq!(invoice.currency.as_deref(), Some("USD"));
        assert_eq!(invoice.kind.as_deref(), Some("charge"));
        // No `status` field -> falls back to `action`.
        assert_eq!(invoice.status.as_deref(), Some("subscription"));
    }

    #[test]
    fn cloudflare_invoice_tolerates_missing_fields() {
        let invoice = cloudflare_invoice_from_history(&json!({}));
        assert!(invoice.id.is_none());
        assert!(invoice.occurred_at.is_none());
        assert!(invoice.amount.is_none());
        assert!(invoice.currency.is_none());
        assert!(invoice.status.is_none());
        assert!(invoice.kind.is_none());
    }

    fn sample_billing_plan() -> CloudflareBillingPlan {
        cloudflare_billing_plan_from_subscription(&json!({
            "price": 20.0,
            "rate_plan": { "id": "business", "public_name": "Business Plan" }
        }))
    }

    #[test]
    fn cloudflare_billing_plans_err_is_unreadable() {
        // Plans are the floor: an Err here means the whole view is unreadable,
        // even if invoices would have succeeded.
        let billing = cloudflare_billing_from_outcomes(
            Err("Cloudflare billing response was invalid.".to_string()),
            Ok(vec![cloudflare_invoice_from_history(
                &json!({ "id": "i1" }),
            )]),
        );
        assert!(!billing.readable);
        assert!(billing.plans.is_empty());
        assert!(billing.invoices.is_empty());
        assert_eq!(
            billing.message.as_deref(),
            Some("Cloudflare billing response was invalid.")
        );
    }

    #[test]
    fn cloudflare_billing_plans_ok_invoices_err_keeps_plans_with_note() {
        // Invoices are best-effort: an Err degrades to a note but the plans are
        // kept and the view stays readable.
        let billing = cloudflare_billing_from_outcomes(
            Ok(vec![sample_billing_plan()]),
            Err("Invoice history was unavailable.".to_string()),
        );
        assert!(billing.readable);
        assert_eq!(billing.plans.len(), 1);
        assert_eq!(billing.plans[0].id.as_deref(), Some("business"));
        assert!(billing.invoices.is_empty());
        let message = billing
            .message
            .expect("expected an invoices-unavailable note");
        assert!(message.contains("Invoice history was unavailable"));
    }

    #[test]
    fn cloudflare_billing_both_ok_has_no_message() {
        let billing = cloudflare_billing_from_outcomes(
            Ok(vec![sample_billing_plan()]),
            Ok(vec![cloudflare_invoice_from_history(
                &json!({ "id": "i1" }),
            )]),
        );
        assert!(billing.readable);
        assert_eq!(billing.plans.len(), 1);
        assert_eq!(billing.invoices.len(), 1);
        assert!(billing.message.is_none());
    }

    #[test]
    fn scaleway_consumption_line_maps_category_value_currency_period() {
        // `value` is a Scaleway Money object: value_untaxed = units + nanos/1e9,
        // currency from currency_code. billing_period is back-filled from the
        // request param (not present on the line). category from category_name.
        let line = scaleway_consumption_line_from(
            &json!({
                "category_name": "Compute",
                "project_id": "11111111-2222-3333-4444-555555555555",
                "value": { "currency_code": "EUR", "units": 12, "nanos": 500_000_000 }
            }),
            "2026-06",
        );
        assert_eq!(line.category.as_deref(), Some("Compute"));
        assert_eq!(
            line.project_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(line.value_untaxed, Some(12.5));
        assert_eq!(line.currency.as_deref(), Some("EUR"));
        assert_eq!(line.billing_period.as_deref(), Some("2026-06"));
    }

    #[test]
    fn scaleway_consumption_line_tolerates_string_money_units() {
        // Scaleway's protobuf-JSON sometimes serializes the int64 `units` as a
        // string ("7"); the Money parser must still read it via number_field.
        let line = scaleway_consumption_line_from(
            &json!({
                "category_name": "Storage",
                "value": { "currency_code": "EUR", "units": "7", "nanos": "250000000" }
            }),
            "2026-06",
        );
        assert_eq!(line.value_untaxed, Some(7.25));
        assert_eq!(line.currency.as_deref(), Some("EUR"));
        assert!(line.project_id.is_none());
    }

    #[test]
    fn scaleway_invoice_maps_id_dates_totals_state() {
        let invoice = scaleway_invoice_from(&json!({
            "id": "inv-123",
            "issued_date": "2026-05-01T00:00:00Z",
            "start_date": "2026-04-01T00:00:00Z",
            "due_date": "2026-05-15T00:00:00Z",
            "total_untaxed": { "currency_code": "EUR", "units": 100, "nanos": 0 },
            "total_taxed": { "currency_code": "EUR", "units": 120, "nanos": 0 },
            "state": "paid"
        }));
        assert_eq!(invoice.id.as_deref(), Some("inv-123"));
        assert_eq!(invoice.issued_at.as_deref(), Some("2026-05-01T00:00:00Z"));
        assert_eq!(invoice.start_date.as_deref(), Some("2026-04-01T00:00:00Z"));
        assert_eq!(invoice.stop_date.as_deref(), Some("2026-05-15T00:00:00Z"));
        assert_eq!(invoice.total_untaxed, Some(100.0));
        assert_eq!(invoice.total_taxed, Some(120.0));
        assert_eq!(invoice.currency.as_deref(), Some("EUR"));
        assert_eq!(invoice.state.as_deref(), Some("paid"));
    }

    fn sample_consumption_line() -> ScalewayConsumptionLine {
        scaleway_consumption_line_from(
            &json!({
                "category_name": "Compute",
                "value": { "currency_code": "EUR", "units": 5, "nanos": 0 }
            }),
            "2026-06",
        )
    }

    #[test]
    fn scaleway_billing_consumptions_err_is_unreadable() {
        // Consumptions are the floor: an Err here means the whole view is
        // unreadable with that message, even if invoices would have succeeded.
        let billing = scaleway_billing_from_outcomes(
            Err("Scaleway billing response was invalid.".to_string()),
            Ok((Vec::new(), 0)),
        );
        assert!(!billing.readable);
        assert!(billing.consumptions.is_empty());
        assert!(billing.invoices.is_empty());
        assert_eq!(
            billing.message.as_deref(),
            Some("Scaleway billing response was invalid.")
        );
    }

    #[test]
    fn scaleway_billing_consumptions_ok_invoices_err_keeps_consumptions_with_note() {
        // Invoices are best-effort: an Err degrades to a note but consumptions
        // are kept and the view stays readable.
        let billing = scaleway_billing_from_outcomes(
            Ok((
                vec![sample_consumption_line()],
                Some(5.0),
                Some(1.0),
                Some("2026-06-01T00:00:00Z".to_string()),
            )),
            Err("Scaleway invoices were unavailable.".to_string()),
        );
        assert!(billing.readable);
        assert_eq!(billing.consumptions.len(), 1);
        assert_eq!(billing.total_untaxed, Some(5.0));
        assert_eq!(billing.total_discount, Some(1.0));
        assert_eq!(billing.updated_at.as_deref(), Some("2026-06-01T00:00:00Z"));
        assert!(billing.invoices.is_empty());
        let message = billing
            .message
            .expect("expected an invoices-unavailable note");
        assert!(message.contains("invoices"));
    }

    #[test]
    fn scaleway_billing_both_ok_has_no_message() {
        let billing = scaleway_billing_from_outcomes(
            Ok((vec![sample_consumption_line()], Some(5.0), None, None)),
            Ok((vec![scaleway_invoice_from(&json!({ "id": "inv-1" }))], 1)),
        );
        assert!(billing.readable);
        assert_eq!(billing.consumptions.len(), 1);
        assert_eq!(billing.invoices.len(), 1);
        assert!(billing.message.is_none());
    }

    #[test]
    fn scaleway_billing_parse_consumptions_reads_discount_money_object() {
        // `total_discount_untaxed_value` is a protobuf Money object, NOT a flat
        // number. The previous `number_field` read always yielded `None` here;
        // it must be flattened via `scaleway_money_from`. The per-line `value`
        // is likewise a Money object and must parse into the summed untaxed
        // total. This exercises the REAL parser the HTTP path uses.
        let envelope = json!({
            "consumptions": [
                {
                    "category_name": "Compute",
                    "project_id": "11111111-2222-3333-4444-555555555555",
                    "value": { "currency_code": "EUR", "units": "10", "nanos": 0 }
                },
                {
                    "category_name": "Storage",
                    "value": { "currency_code": "EUR", "units": 2, "nanos": 500_000_000 }
                }
            ],
            "total_discount_untaxed_value": { "currency_code": "EUR", "units": "5", "nanos": 0 },
            "updated_at": "2026-06-01T00:00:00Z"
        });

        let (lines, total_untaxed, total_discount, updated_at) =
            scaleway_billing_parse_consumptions(&envelope, "2026-06")
                .expect("well-formed consumptions envelope must parse");

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].value_untaxed, Some(10.0));
        assert_eq!(lines[1].value_untaxed, Some(2.5));
        assert_eq!(total_untaxed, Some(12.5));
        // The bug: discount Money object must flatten to 5.0, not None.
        assert_eq!(total_discount, Some(5.0));
        assert_eq!(updated_at.as_deref(), Some("2026-06-01T00:00:00Z"));
    }

    #[test]
    fn scaleway_billing_parse_consumptions_rejects_missing_array() {
        // A 200 whose body is not the expected envelope (no `consumptions`
        // array) yields `None` so the caller marks the view unreadable.
        assert!(scaleway_billing_parse_consumptions(&json!({}), "2026-06").is_none());
    }

    #[test]
    fn scaleway_uuid_is_valid_gate() {
        // The org-id resolution (env override and API-resolved value) and the
        // project-id path both gate on this validator before a value can reach
        // the billing URL. A valid 8-4-4-4-12 hex id passes; anything malformed
        // is rejected so it cannot be interpolated into a request.
        assert!(scaleway_uuid_is_valid(
            "11111111-2222-3333-4444-555555555555"
        ));
        assert!(!scaleway_uuid_is_valid("not-a-uuid"));
        assert!(!scaleway_uuid_is_valid(""));
        // Right shape but a non-hex char.
        assert!(!scaleway_uuid_is_valid(
            "1111111g-2222-3333-4444-555555555555"
        ));
        // Trailing junk after a valid UUID must not be accepted.
        assert!(!scaleway_uuid_is_valid(
            "11111111-2222-3333-4444-555555555555-extra"
        ));
    }

    #[test]
    fn scaleway_instance_create_body_matches_api_contract() {
        // The Instance API uses `project` (NOT `project_id`), plus `name`,
        // `commercial_type`, `image` (image id), `dynamic_ip_required` and `tags`.
        // This body is sent verbatim, so assert the exact field names + values.
        let body = scaleway_instance_create_body(
            "trainer-a",
            "11111111-2222-3333-4444-555555555555",
            "GP1-S",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            true,
            &["gpu".to_string(), "alpha".to_string()],
        );
        assert_eq!(body["name"], json!("trainer-a"));
        // CRITICAL: the field is `project`, not `project_id`.
        assert_eq!(
            body["project"],
            json!("11111111-2222-3333-4444-555555555555")
        );
        assert!(
            body.get("project_id").is_none(),
            "Instance API must use `project`, never `project_id`"
        );
        assert_eq!(body["commercial_type"], json!("GP1-S"));
        assert_eq!(body["image"], json!("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
        assert_eq!(body["dynamic_ip_required"], json!(true));
        assert_eq!(body["tags"], json!(["gpu", "alpha"]));
    }

    #[test]
    fn scaleway_instance_create_body_omits_empty_tags() {
        // No tags => no `tags` key (never send an empty array that the API may
        // interpret as "clear all tags").
        let body = scaleway_instance_create_body(
            "srv",
            "11111111-2222-3333-4444-555555555555",
            "DEV1-S",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            false,
            &[],
        );
        assert!(body.get("tags").is_none(), "empty tags must be omitted");
        assert_eq!(body["dynamic_ip_required"], json!(false));
    }

    #[test]
    fn scaleway_instance_offer_cost_reads_catalog() {
        // The dry-run cost is the offer's hourly/monthly price, matched on the
        // exact (zone, commercial_type) pair from the synced catalog.
        let offers = vec![seed_instance_offer(
            "fr-par-1",
            "GP1-S",
            Some(0.5),
            Some(365.0),
        )];
        let cost = scaleway_instance_offer_cost(&offers, "fr-par-1", "GP1-S");
        assert_eq!(cost.hourly_eur, Some(0.5));
        assert_eq!(cost.monthly_eur, Some(365.0));
        assert!(cost.risk.is_none());
    }

    #[test]
    fn scaleway_instance_offer_cost_missing_offer_yields_none_and_risk() {
        // Offer absent (wrong zone OR wrong type) => None + None and a risk note,
        // NEVER a fabricated 0 cost.
        let offers = vec![seed_instance_offer(
            "fr-par-1",
            "GP1-S",
            Some(0.5),
            Some(365.0),
        )];
        let other_zone = scaleway_instance_offer_cost(&offers, "nl-ams-1", "GP1-S");
        assert_eq!(other_zone.hourly_eur, None);
        assert_eq!(other_zone.monthly_eur, None);
        assert!(other_zone.risk.is_some());

        let other_type = scaleway_instance_offer_cost(&offers, "fr-par-1", "PRO2-M");
        assert_eq!(other_type.hourly_eur, None);
        assert_eq!(other_type.monthly_eur, None);
        assert!(other_type.risk.is_some());

        // Empty catalog (never synced) => None + risk, no panic.
        let empty = scaleway_instance_offer_cost(&[], "fr-par-1", "GP1-S");
        assert_eq!(empty.hourly_eur, None);
        assert!(empty.risk.is_some());
    }

    fn seed_instance_offer(
        zone: &str,
        name: &str,
        hourly: Option<f64>,
        monthly: Option<f64>,
    ) -> ScalewayOfferSummary {
        ScalewayOfferSummary {
            id: format!("{zone}:{name}"),
            name: name.into(),
            zone: zone.into(),
            category: "CPU VM".into(),
            architecture: "x86_64".into(),
            vcpus: 2,
            memory_gb: 4.0,
            gpu_count: 0,
            gpu_label: None,
            hourly_price_eur: hourly,
            monthly_price_eur: monthly,
            availability: "available".into(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn rewrite_bindings_refuses_secret_text_with_no_name() {
        // A nameless secret cannot be converted to a valid `inherit` binding;
        // emitting {type:inherit,name:""} would drop the secret on replace-all.
        let raw = vec![json!({ "type": "secret_text" })];
        let err = rewrite_bindings_for_plain_text(&raw, "API_BASE", "new").unwrap_err();
        assert!(
            err.contains("secret binding with no name"),
            "unexpected error: {err}"
        );

        // An empty-string name is just as unusable.
        let raw_empty = vec![json!({ "type": "secret_text", "name": "" })];
        assert!(rewrite_bindings_for_plain_text(&raw_empty, "API_BASE", "new").is_err());
    }

    #[test]
    fn extract_worker_bindings_refuses_missing_result() {
        let err = extract_worker_bindings(&json!({ "success": true })).unwrap_err();
        assert!(err.contains("missing result"), "unexpected error: {err}");
    }

    #[test]
    fn extract_worker_bindings_refuses_non_object_result() {
        let err = extract_worker_bindings(&json!({ "result": "oops" })).unwrap_err();
        assert!(
            err.contains("unexpected result shape"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn extract_worker_bindings_refuses_null_bindings() {
        let err = extract_worker_bindings(&json!({ "result": { "bindings": null } })).unwrap_err();
        assert!(err.contains("not an array"), "unexpected error: {err}");
    }

    #[test]
    fn extract_worker_bindings_refuses_missing_bindings() {
        let err =
            extract_worker_bindings(&json!({ "result": { "compatibility_date": "2026-05-01" } }))
                .unwrap_err();
        assert!(err.contains("not an array"), "unexpected error: {err}");
    }

    #[test]
    fn extract_worker_bindings_refuses_non_array_bindings() {
        let err =
            extract_worker_bindings(&json!({ "result": { "bindings": "nope" } })).unwrap_err();
        assert!(err.contains("not an array"), "unexpected error: {err}");
    }

    #[test]
    fn extract_worker_bindings_accepts_explicit_empty_array() {
        // A genuinely empty worker returns `bindings: []` — this is a real,
        // writable state (the target var gets appended), NOT an error.
        let out = extract_worker_bindings(&json!({ "result": { "bindings": [] } })).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn extract_worker_bindings_accepts_populated_array() {
        let out = extract_worker_bindings(&json!({
            "result": {
                "bindings": [
                    { "type": "plain_text", "name": "API_BASE", "text": "x" }
                ]
            }
        }))
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "API_BASE");
    }

    #[test]
    fn env_dry_run_reports_update_and_preserved_bindings() {
        let settings = CloudflareWorkerSettings {
            account_id: "023e105f4ecef8ad9ca31a8372d0c353".into(),
            worker_name: "aspis-bio-api".into(),
            plain_text: vec![CloudflareWorkerBinding {
                name: "API_BASE".into(),
                binding_type: "plain_text".into(),
                text: Some("old".into()),
                reference: None,
            }],
            secrets: vec![CloudflareWorkerBinding {
                name: "SIGNING_KEY".into(),
                binding_type: "secret_text".into(),
                text: None,
                reference: None,
            }],
            other_bindings: vec![CloudflareWorkerBinding {
                name: "CACHE".into(),
                binding_type: "kv_namespace".into(),
                text: None,
                reference: Some("abc123".into()),
            }],
            compatibility_date: None,
            readable: true,
            message: None,
        };

        let result = cloudflare_env_dry_run(&settings, "API_BASE", "new");

        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, "update");
        assert_eq!(result.changes[0].before.as_deref(), Some("old"));
        assert_eq!(result.changes[0].after, "new");
        assert_eq!(result.preserved_secrets, vec!["SIGNING_KEY".to_string()]);
        assert_eq!(
            result.preserved_other,
            vec!["CACHE (kv_namespace)".to_string()]
        );
        // Never echoes a secret value (there is none) and risks mention inherit.
        assert!(result.risks.iter().any(|risk| risk.contains("inherit")));
    }

    #[test]
    fn env_dry_run_reports_create_when_var_absent() {
        let settings = CloudflareWorkerSettings {
            account_id: "023e105f4ecef8ad9ca31a8372d0c353".into(),
            worker_name: "aspis-bio-api".into(),
            plain_text: Vec::new(),
            secrets: Vec::new(),
            other_bindings: Vec::new(),
            compatibility_date: None,
            readable: true,
            message: None,
        };

        let result = cloudflare_env_dry_run(&settings, "NEW_VAR", "value");

        assert_eq!(result.changes[0].kind, "create");
        assert!(result.changes[0].before.is_none());
    }

    #[test]
    fn sanitize_error_redacts_bearer_token() {
        let raw = "request failed with Bearer abc123secret";
        assert_eq!(
            sanitize_error_message(raw),
            "request failed with Bearer [redacted]"
        );
    }

    #[test]
    fn sanitize_error_redacts_all_token_shapes_in_place() {
        // B3: every match is redacted, surrounding text is preserved (no truncation).
        let raw = "Bearer abc123 then X-Auth-Token: deadbeef and more text";
        let sanitized = sanitize_error_message(raw);
        assert!(!sanitized.contains("abc123"));
        assert!(!sanitized.contains("deadbeef"));
        assert!(sanitized.contains("and more text"));

        let sigv4 =
            "AWS4 Credential=AKIA123/20260529/eu/s3 Signature=ff00 aa Signature=second tail";
        let sanitized = sanitize_error_message(sigv4);
        assert!(!sanitized.contains("AKIA123/20260529/eu/s3"));
        assert!(!sanitized.contains("ff00"));
        assert!(!sanitized.contains("second"));
        assert!(sanitized.contains("tail"));

        let github = "token ghp_aB3dEf and github_pat_11ABCDEFG_long rest";
        let sanitized = sanitize_error_message(github);
        assert!(!sanitized.contains("ghp_aB3dEf"));
        assert!(!sanitized.contains("github_pat_11ABCDEFG_long"));
        assert!(sanitized.contains("rest"));

        let scaleway =
            "error for key SCWXXXXXXXXXXXXXXXXX and 11111111-2222-3333-4444-555555555555 done";
        let sanitized = sanitize_error_message(scaleway);
        assert!(!sanitized.contains("SCWXXXXXXXXXXXXXXXXX"));
        assert!(!sanitized.contains("11111111-2222-3333-4444-555555555555"));
        assert!(sanitized.contains("done"));
    }

    #[test]
    fn provider_error_token_health_classifies_auth_failures() {
        let unauthorized =
            ProviderInventory::error(ProviderId::Scaleway, "HTTP 401 Unauthorized".into());
        let forbidden =
            ProviderInventory::error(ProviderId::Cloudflare, "HTTP 403 Forbidden".into());
        let timeout = ProviderInventory::error(ProviderId::Cloudflare, "request timed out".into());

        assert_eq!(unauthorized.health.token_health, "invalid");
        assert_eq!(forbidden.health.token_health, "insufficient_scope");
        assert_eq!(timeout.health.token_health, "unknown");
    }

    #[test]
    fn cloudflare_permission_check_detects_workers_scripts_write() {
        let policies = vec![CfTokenPolicy {
            permission_groups: vec![CfPermissionGroup {
                name: Some("Workers Scripts Write".into()),
            }],
        }];

        assert_eq!(
            cloudflare_workers_scripts_write_permission(&policies),
            CloudflareWorkersWritePermission::Present
        );
    }

    #[test]
    fn cloudflare_permission_check_marks_read_only_workers_tokens() {
        let policies = vec![CfTokenPolicy {
            permission_groups: vec![CfPermissionGroup {
                name: Some("Workers Scripts Read".into()),
            }],
        }];

        assert_eq!(
            cloudflare_workers_scripts_write_permission(&policies),
            CloudflareWorkersWritePermission::Missing
        );
    }

    #[test]
    fn cloudflare_permission_check_returns_unknown_for_empty_policies() {
        let policies: Vec<CfTokenPolicy> = Vec::new();

        assert_eq!(
            cloudflare_workers_scripts_write_permission(&policies),
            CloudflareWorkersWritePermission::Unknown
        );
    }

    #[test]
    fn cloudflare_credential_kind_distinguishes_profile_and_account_tokens() {
        assert_eq!(
            cloudflare_credential_kind(CloudflareTokenVerificationSource::User),
            "cloudflare_profile_token"
        );
        assert_eq!(
            cloudflare_credential_kind(CloudflareTokenVerificationSource::Account),
            "cloudflare_account_owned_token"
        );
        assert_eq!(
            cloudflare_credential_kind(CloudflareTokenVerificationSource::Unverified),
            "cloudflare_unverified_policy_token"
        );
    }

    #[test]
    fn cloudflare_account_selector_targets_aspis_bio_only() {
        let accounts = vec![
            CfAccount {
                id: "launcher-account".into(),
                name: Some("Aspis Launcher".into()),
            },
            CfAccount {
                id: "bio-account".into(),
                name: Some("Aspis Bio".into()),
            },
        ];

        let selected = select_cloudflare_accounts(&accounts, None).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "bio-account");
    }

    #[test]
    fn cloudflare_account_selector_accepts_single_named_bio_account() {
        let accounts = vec![CfAccount {
            id: "single-account".into(),
            name: Some("Aspis Bio".into()),
        }];

        let selected = select_cloudflare_accounts(&accounts, None).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "single-account");
    }

    #[test]
    fn cloudflare_account_selector_accepts_single_non_bio_account_with_warning_elsewhere() {
        let accounts = vec![CfAccount {
            id: "single-account".into(),
            name: Some("Personal Account".into()),
        }];

        let selected = select_cloudflare_accounts(&accounts, None).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "single-account");
    }

    #[test]
    fn cloudflare_account_selector_refuses_multi_account_without_bio_match() {
        let accounts = vec![
            CfAccount {
                id: "default-account".into(),
                name: Some("Default".into()),
            },
            CfAccount {
                id: "launcher-account".into(),
                name: Some("Aspis Launcher".into()),
            },
        ];

        assert!(select_cloudflare_accounts(&accounts, None).is_err());
    }

    #[test]
    fn cloudflare_account_selector_refuses_ambiguous_bio_matches() {
        let accounts = vec![
            CfAccount {
                id: "bio-1".into(),
                name: Some("Aspis Bio".into()),
            },
            CfAccount {
                id: "bio-2".into(),
                name: Some("aspis-bio".into()),
            },
        ];

        assert!(select_cloudflare_accounts(&accounts, None).is_err());
    }

    #[test]
    fn cloudflare_account_selector_uses_pinned_account_id() {
        let accounts = vec![
            CfAccount {
                id: "launcher-account".into(),
                name: Some("Aspis Launcher".into()),
            },
            CfAccount {
                id: "bio-account".into(),
                name: Some("Aspis Bio".into()),
            },
        ];

        let selected = select_cloudflare_accounts(&accounts, Some("bio-account")).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "bio-account");
    }

    #[test]
    fn cloudflare_account_selector_accepts_pinned_visible_account() {
        let accounts = vec![
            CfAccount {
                id: "launcher-account".into(),
                name: Some("Aspis Launcher".into()),
            },
            CfAccount {
                id: "bio-account".into(),
                name: Some("Aspis Bio".into()),
            },
        ];

        let selected = select_cloudflare_accounts(&accounts, Some("launcher-account")).unwrap();
        assert_eq!(selected[0].id, "launcher-account");

        let single_non_bio = vec![CfAccount {
            id: "launcher-account".into(),
            name: Some("Aspis Launcher".into()),
        }];
        let selected =
            select_cloudflare_accounts(&single_non_bio, Some("launcher-account")).unwrap();
        assert_eq!(selected[0].id, "launcher-account");
    }

    #[test]
    fn cloudflare_scope_warning_detects_unverified_account_name() {
        let unverified = ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-1".into(),
            name: Some("Personal Account".into()),
            source: "single_account_token".into(),
        };
        let verified = ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-2".into(),
            name: Some("Aspis Bio".into()),
            source: "single_account_token".into(),
        };
        let pinned = ProviderScopeSelection {
            provider: ProviderId::Cloudflare,
            id: "account-3".into(),
            name: Some("Personal Account".into()),
            source: "pinned".into(),
        };

        assert!(cloudflare_scope_name_unverified(Some(&unverified)));
        assert!(!cloudflare_scope_name_unverified(Some(&verified)));
        assert!(cloudflare_scope_name_unverified(Some(&pinned)));
    }

    #[test]
    fn cloudflare_worker_summary_prefers_annotation_message_for_purpose() {
        let script = CfWorkerScript {
            id: "aspis-oracle-router".into(),
            created_on: Some("2026-05-01T00:00:00Z".into()),
            modified_on: Some("2026-05-02T00:00:00Z".into()),
            usage_model: Some("bundled".into()),
            routes: vec![CfWorkerRoute {
                pattern: Some("oracle.aspis.bio/*".into()),
            }],
            compatibility_date: Some("2026-05-01".into()),
            compatibility_flags: vec!["nodejs_compat".into()],
            handlers: vec!["fetch".into()],
            tags: vec!["oracle".into(), "bio".into()],
            annotations: Some(CfWorkerAnnotations {
                message: Some("Routes Architecture Oracle traffic.".into()),
                tag: Some("release-7".into()),
                triggered_by: Some("wrangler".into()),
            }),
        };

        let summary = cloudflare_worker_summary(
            &CfAccount {
                id: "account-1".into(),
                name: Some("Aspis Bio".into()),
            },
            script,
            None,
        );

        assert_eq!(summary.purpose, "Routes Architecture Oracle traffic.");
        assert_eq!(summary.purpose_source, "annotation");
        assert_eq!(summary.status, "unknown");
        assert_eq!(summary.compatibility_date.as_deref(), Some("2026-05-01"));
        assert_eq!(summary.compatibility_flags, vec!["nodejs_compat"]);
        assert_eq!(summary.handlers, vec!["fetch"]);
        assert_eq!(
            summary.tags,
            vec!["bio", "oracle", "release-7", "triggered_by:wrangler"]
        );
        assert!(summary.oracle_query.contains("aspis-oracle-router"));
        assert!(summary.oracle_query.contains("oracle.aspis.bio"));
    }

    #[test]
    fn cloudflare_worker_script_deserializes_null_lists_as_empty() {
        let script: CfWorkerScript = serde_json::from_value(json!({
            "id": "aspis-bio-api",
            "routes": null,
            "compatibility_flags": null,
            "handlers": null,
            "tags": null
        }))
        .unwrap();

        assert!(script.routes.is_empty());
        assert!(script.compatibility_flags.is_empty());
        assert!(script.handlers.is_empty());
        assert!(script.tags.is_empty());
    }

    #[test]
    fn cloudflare_worker_scope_filter_hides_sibling_workers() {
        fn script(id: &str, routes: Vec<CfWorkerRoute>) -> CfWorkerScript {
            CfWorkerScript {
                id: id.into(),
                created_on: None,
                modified_on: None,
                usage_model: None,
                routes,
                compatibility_date: None,
                compatibility_flags: Vec::new(),
                handlers: Vec::new(),
                tags: Vec::new(),
                annotations: None,
            }
        }
        let bio_by_name = script("aspis-bio-api", Vec::new());
        let bio_by_route = script(
            "custom-worker",
            vec![CfWorkerRoute {
                pattern: Some("api.aspis-bio.com/*".into()),
            }],
        );
        let sibling = script("aspis-food-worker", Vec::new());
        // C5: a lookalike host that merely contains "aspis-bio.com" as a substring
        // must NOT be treated as in-scope.
        let lookalike = script(
            "evil-worker",
            vec![CfWorkerRoute {
                pattern: Some("aspis-bio.com.evil.tld/*".into()),
            }],
        );
        let apex = script(
            "apex-worker",
            vec![CfWorkerRoute {
                pattern: Some("https://aspis-bio.com/api/*".into()),
            }],
        );

        assert!(cloudflare_worker_in_aspis_bio_scope(&bio_by_name));
        assert!(cloudflare_worker_in_aspis_bio_scope(&bio_by_route));
        assert!(cloudflare_worker_in_aspis_bio_scope(&apex));
        assert!(!cloudflare_worker_in_aspis_bio_scope(&sibling));
        assert!(!cloudflare_worker_in_aspis_bio_scope(&lookalike));
    }

    #[test]
    fn cloudflare_worker_summary_prefers_deployment_message_and_marks_healthy() {
        let script = CfWorkerScript {
            id: "aspis-oracle-router".into(),
            created_on: Some("2026-05-01T00:00:00Z".into()),
            modified_on: Some("2026-05-02T00:00:00Z".into()),
            usage_model: Some("bundled".into()),
            routes: vec![CfWorkerRoute {
                pattern: Some("oracle.aspis.bio/*".into()),
            }],
            compatibility_date: Some("2026-05-01".into()),
            compatibility_flags: vec!["nodejs_compat".into()],
            handlers: vec!["fetch".into()],
            tags: vec!["oracle".into(), "bio".into()],
            annotations: Some(CfWorkerAnnotations {
                message: Some("Older script annotation.".into()),
                tag: Some("release-7".into()),
                triggered_by: Some("wrangler".into()),
            }),
        };
        let deployment = CfWorkerDeployment {
            created_on: Some("2026-05-03T00:00:00Z".into()),
            source: Some("api".into()),
            versions: vec![CfDeploymentVersion { percentage: 100.0 }],
            annotations: Some(CfDeploymentAnnotations {
                message: Some("Latest deployment routes Oracle traffic.".into()),
                triggered_by: Some("wrangler".into()),
            }),
        };

        let summary = cloudflare_worker_summary(
            &CfAccount {
                id: "account-1".into(),
                name: Some("Aspis Bio".into()),
            },
            script,
            Some(deployment),
        );

        assert_eq!(summary.purpose, "Latest deployment routes Oracle traffic.");
        assert_eq!(summary.purpose_source, "deployment");
        assert_eq!(summary.status, "healthy");
        assert_eq!(summary.last_deploy.as_deref(), Some("2026-05-03T00:00:00Z"));
        assert!(summary.tags.contains(&"deployment_source:api".into()));
        assert!(summary
            .tags
            .contains(&"deployment_triggered_by:wrangler".into()));
    }

    #[test]
    fn cloudflare_worker_summary_marks_partial_deployment_degraded() {
        let summary = cloudflare_worker_summary(
            &CfAccount {
                id: "account-1".into(),
                name: None,
            },
            CfWorkerScript {
                id: "api-router".into(),
                created_on: None,
                modified_on: None,
                usage_model: None,
                routes: Vec::new(),
                compatibility_date: None,
                compatibility_flags: Vec::new(),
                handlers: Vec::new(),
                tags: Vec::new(),
                annotations: None,
            },
            Some(CfWorkerDeployment {
                created_on: None,
                source: Some("api".into()),
                versions: vec![CfDeploymentVersion { percentage: 50.0 }],
                annotations: None,
            }),
        );

        assert_eq!(summary.status, "degraded");
    }

    #[test]
    fn cloudflare_worker_summary_falls_back_to_route_or_name_for_purpose() {
        let routed = cloudflare_worker_summary(
            &CfAccount {
                id: "account-1".into(),
                name: None,
            },
            CfWorkerScript {
                id: "api-router".into(),
                created_on: None,
                modified_on: None,
                usage_model: None,
                routes: vec![CfWorkerRoute {
                    pattern: Some("api.aspis.bio/*".into()),
                }],
                compatibility_date: None,
                compatibility_flags: Vec::new(),
                handlers: Vec::new(),
                tags: Vec::new(),
                annotations: None,
            },
            None,
        );
        assert_eq!(routed.purpose, "Handles traffic for api.aspis.bio/*.");
        assert_eq!(routed.purpose_source, "route");

        let named = cloudflare_worker_summary(
            &CfAccount {
                id: "account-1".into(),
                name: None,
            },
            CfWorkerScript {
                id: "food-transit-worker".into(),
                created_on: None,
                modified_on: None,
                usage_model: None,
                routes: Vec::new(),
                compatibility_date: None,
                compatibility_flags: Vec::new(),
                handlers: Vec::new(),
                tags: Vec::new(),
                annotations: None,
            },
            None,
        );
        assert_eq!(
            named.purpose,
            "Worker inferred from name: food transit worker."
        );
        assert_eq!(named.purpose_source, "name");
    }

    #[test]
    fn scaleway_function_summary_keeps_runtime_scale_and_warm_cost_risk() {
        let summary = ScwFunction {
            id: "fn-1".into(),
            name: "aspis-oracle".into(),
            status: Some("ready".into()),
            runtime: Some("node20".into()),
            min_scale: Some(1),
            max_scale: Some(10),
            domain_name: Some("oracle.functions.fnc.fr-par.scw.cloud".into()),
            privacy: Some("private".into()),
            description: None,
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
        .into_summary(
            "fr-par",
            &ScwProject {
                id: "bio-project".into(),
                name: "Aspis Bio".into(),
            },
        );

        assert_eq!(summary.resource_type, "Serverless");
        assert_eq!(summary.project_id.as_deref(), Some("bio-project"));
        assert_eq!(summary.project_name.as_deref(), Some("Aspis Bio"));
        assert_eq!(summary.state, "running");
        assert_eq!(summary.runtime.as_deref(), Some("node20"));
        assert_eq!(summary.min_scale, Some(1));
        assert_eq!(summary.max_scale, Some(10));
        assert_eq!(
            summary.domain_name.as_deref(),
            Some("oracle.functions.fnc.fr-par.scw.cloud")
        );
        assert_eq!(summary.privacy.as_deref(), Some("private"));
        assert!(summary.idle_cost_risk);
    }

    #[test]
    fn scaleway_server_summary_prefers_purpose_tag_and_keeps_operational_metadata() {
        let summary = scaleway_server_summary(
            ScwServer {
                id: "srv-1".into(),
                name: "gpu-trainer".into(),
                state: "running".into(),
                commercial_type: Some("GPU-3070-S".into()),
                tags: vec!["purpose:model-training".into(), "nightly".into()],
                image: Some(ScwServerImage {
                    id: "img-1".into(),
                    name: Some("Ubuntu 24.04".into()),
                }),
                public_ip: Some(ScwPublicIp {
                    address: Some("203.0.113.10".into()),
                }),
                created_at: Some("2026-05-27T10:00:00Z".into()),
                updated_at: Some("2026-05-27T11:00:00Z".into()),
            },
            "fr-par-1",
            &ScwProject {
                id: "bio-project".into(),
                name: "Aspis Bio".into(),
            },
        );

        assert_eq!(summary.resource_type, "GPU");
        assert_eq!(summary.purpose, "model-training");
        assert_eq!(summary.purpose_source, "tag");
        assert_eq!(summary.tags, vec!["purpose:model-training", "nightly"]);
        assert_eq!(summary.image.as_deref(), Some("Ubuntu 24.04"));
        assert_eq!(summary.public_ip.as_deref(), Some("203.0.113.10"));
        assert!(summary.oracle_query.contains("model-training"));
        assert!(summary.oracle_query.contains("Ubuntu 24.04"));
    }

    #[test]
    fn scaleway_function_summary_prefers_description_then_runtime_fallback() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let described = ScwFunction {
            id: "fn-2".into(),
            name: "oracle-api".into(),
            status: Some("ready".into()),
            runtime: Some("python312".into()),
            min_scale: Some(0),
            max_scale: Some(5),
            domain_name: None,
            privacy: Some("private".into()),
            description: Some("Answers Architecture Oracle questions".into()),
            tags: vec!["oracle".into(), "backend".into()],
            created_at: Some("2026-05-27T10:00:00Z".into()),
            updated_at: None,
        }
        .into_summary("fr-par", &project);

        assert_eq!(described.purpose, "Answers Architecture Oracle questions");
        assert_eq!(described.purpose_source, "description");
        assert!(described.oracle_query.contains("python312"));
        assert!(described.oracle_query.contains("oracle"));

        let runtime_only = ScwFunction {
            id: "fn-3".into(),
            name: "protein-worker".into(),
            status: Some("created".into()),
            runtime: Some("go122".into()),
            min_scale: Some(0),
            max_scale: Some(2),
            domain_name: None,
            privacy: None,
            description: None,
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
        .into_summary("fr-par", &project);

        assert_eq!(
            runtime_only.purpose,
            "Serverless function running go122, scale 0-2."
        );
        assert_eq!(runtime_only.purpose_source, "runtime");
    }

    #[test]
    fn scaleway_container_summary_keeps_cpu_memory_endpoint_and_warm_cost_risk() {
        let summary = ScwContainer {
            id: "ctr-1".into(),
            name: "oracle-cpu-worker".into(),
            status: Some("ready".into()),
            description: Some("CPU serverless workload for Oracle requests".into()),
            min_scale: Some(1),
            max_scale: Some(6),
            memory_limit_bytes: Some(536_870_912),
            mvcpu_limit: Some(500),
            privacy: Some("private".into()),
            image: Some("rg.fr-par.scw.cloud/aspis/oracle:latest".into()),
            protocol: Some("http1".into()),
            public_endpoint: Some("https://oracle.functions.example".into()),
            tags: vec!["purpose:oracle-cpu".into(), "serverless".into()],
            created_at: Some("2026-05-27T10:00:00Z".into()),
            updated_at: Some("2026-05-27T11:00:00Z".into()),
        }
        .into_summary(
            "fr-par",
            &ScwProject {
                id: "bio-project".into(),
                name: "Aspis Bio".into(),
            },
        );

        assert_eq!(summary.resource_type, "Serverless");
        assert_eq!(summary.state, "running");
        assert_eq!(
            summary.commercial_type.as_deref(),
            Some("500 mCPU / 512 MB")
        );
        assert_eq!(summary.runtime.as_deref(), Some("container/http1"));
        assert_eq!(
            summary.domain_name.as_deref(),
            Some("https://oracle.functions.example")
        );
        assert_eq!(
            summary.image.as_deref(),
            Some("rg.fr-par.scw.cloud/aspis/oracle:latest")
        );
        assert_eq!(summary.purpose, "oracle-cpu");
        assert_eq!(summary.purpose_source, "tag");
        assert!(summary.idle_cost_risk);
        assert!(summary.oracle_query.contains("oracle-cpu"));
        assert!(summary.oracle_query.contains("500 mCPU"));
    }

    #[test]
    fn scaleway_filesystem_summary_maps_fields_size_and_state() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let summary = ScwFilesystem {
            id: "fs-1".into(),
            name: "shared-datasets".into(),
            // 50 GB in bytes (Scaleway sizes are in bytes).
            size: Some(50_000_000_000),
            status: Some("available".into()),
            tags: vec!["datasets".into()],
            created_at: Some("2026-05-27T10:00:00Z".into()),
            updated_at: Some("2026-05-27T11:00:00Z".into()),
        }
        .into_summary("fr-par", &project);

        assert_eq!(summary.storage_type, "File System");
        assert_eq!(summary.region, "fr-par");
        assert_eq!(summary.project_id.as_deref(), Some("bio-project"));
        assert_eq!(summary.project_name.as_deref(), Some("Aspis Bio"));
        // A "available" filesystem must pass through as "available" (neutral), not be
        // mislabeled "running". See `normalize_scaleway_state`.
        assert_eq!(summary.state, "available");
        assert!((summary.size_gb - 50.0).abs() < 1e-9);
        assert_eq!(
            summary.price_eur_per_gb_hour,
            Some(SCW_FILE_STORAGE_EUR_PER_GB_HOUR)
        );
        // Monthly estimate is size_gb * per-GB-hour * 730 hours.
        let expected_month = 50.0 * SCW_FILE_STORAGE_EUR_PER_GB_HOUR * SCW_MONTHLY_HOURS;
        assert!((summary.estimated_eur_month.unwrap() - expected_month).abs() < 1e-9);
        assert!(summary.billable);
        assert_eq!(summary.tags, vec!["datasets".to_string()]);
        assert_eq!(summary.created_at.as_deref(), Some("2026-05-27T10:00:00Z"));
    }

    #[test]
    fn scaleway_filesystem_summary_handles_missing_size_and_unknown_state() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let summary = ScwFilesystem {
            id: "fs-2".into(),
            name: "empty".into(),
            size: None,
            status: None,
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
        .into_summary("fr-par", &project);

        assert_eq!(summary.size_gb, 0.0);
        assert_eq!(summary.state, "unknown");
        assert_eq!(summary.estimated_eur_month, Some(0.0));
    }

    #[test]
    fn scaleway_sql_database_summary_maps_status_cpu_endpoint_and_idle_risk() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let summary = ScwSqlDatabase {
            id: "sqldb-1".into(),
            name: "aspis-bio-db".into(),
            status: Some("ready".into()),
            cpu_min: Some(2),
            cpu_max: Some(8),
            endpoint: Some("postgres://db.fr-par.scw.cloud:5432/aspis".into()),
            created_at: Some("2026-05-27T10:00:00Z".into()),
            updated_at: None,
        }
        .into_summary("fr-par", &project);

        assert_eq!(summary.resource_type, "Serverless SQL");
        assert_eq!(summary.region, "fr-par");
        assert_eq!(summary.state, "running");
        assert_eq!(summary.min_scale, Some(2));
        assert_eq!(summary.max_scale, Some(8));
        assert_eq!(
            summary.endpoint.as_deref(),
            Some("postgres://db.fr-par.scw.cloud:5432/aspis")
        );
        assert!(summary.available_actions.is_empty());
        // min cpu > 0 => idle cost risk.
        assert!(summary.idle_cost_risk);
    }

    #[test]
    fn scaleway_sql_database_summary_no_idle_risk_when_min_cpu_zero() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let summary = ScwSqlDatabase {
            id: "sqldb-2".into(),
            name: "scale-to-zero".into(),
            status: Some("unknown_status".into()),
            cpu_min: Some(0),
            cpu_max: Some(15),
            endpoint: None,
            created_at: None,
            updated_at: None,
        }
        .into_summary("fr-par", &project);

        assert!(!summary.idle_cost_risk);
        assert_eq!(summary.state, "unknown");
        assert_eq!(summary.endpoint, None);
    }

    #[test]
    fn scaleway_sql_database_deserializes_terraform_cpu_aliases() {
        let raw = r#"{
            "id": "sqldb-3",
            "name": "tf-named",
            "status": "ready",
            "min_cpu": 1,
            "max_cpu": 4,
            "endpoint": "postgres://host:5432/db"
        }"#;
        let database: ScwSqlDatabase = serde_json::from_str(raw).unwrap();

        assert_eq!(database.cpu_min, Some(1));
        assert_eq!(database.cpu_max, Some(4));
    }

    #[test]
    fn scaleway_generative_model_summary_maps_id_region_and_state() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let summary = ScwGenerativeModel {
            id: "llama-3.1-8b-instruct".into(),
        }
        .into_summary(&project);

        assert_eq!(summary.resource_type, "Generative API Model");
        assert_eq!(summary.region, "fr-par");
        assert_eq!(summary.state, "available");
        assert_eq!(summary.id, "llama-3.1-8b-instruct");
        assert_eq!(summary.name, "llama-3.1-8b-instruct");
        assert!(summary.available_actions.is_empty());
        assert!(!summary.idle_cost_risk);
        assert_eq!(summary.project_name.as_deref(), Some("Aspis Bio"));
    }

    #[test]
    fn scaleway_generative_models_envelope_reads_openai_data_array() {
        let raw = r#"{ "object": "list", "data": [
            { "id": "llama-3.1-8b-instruct", "object": "model" },
            { "id": "mistral-nemo-instruct-2407", "object": "model" }
        ] }"#;
        let envelope: ScwGenerativeModelsEnvelope = serde_json::from_str(raw).unwrap();

        assert_eq!(envelope.data.len(), 2);
        assert_eq!(envelope.data[0].id, "llama-3.1-8b-instruct");
    }

    #[test]
    fn scaleway_generative_models_envelope_skips_malformed_entries() {
        // A single `data[]` element missing `id` must NOT fail the whole list
        // deserialization (it would otherwise drop ALL models). Empty-id models
        // are parsed (via `serde(default)`) and then filtered out, so a malformed
        // middle element is skipped while its neighbours survive.
        let raw = r#"{ "data": [ { "id": "a" }, {}, { "id": "b" } ] }"#;
        let envelope: ScwGenerativeModelsEnvelope = serde_json::from_str(raw).unwrap();
        let ids: Vec<String> = envelope
            .data
            .into_iter()
            .filter(|model| !model.id.trim().is_empty())
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn scaleway_new_read_urls_are_region_scoped_and_fr_par_only_capable() {
        assert_eq!(
            scaleway_filesystems_url("fr-par", "bio-project", 2),
            "https://api.scaleway.com/file/v1alpha1/regions/fr-par/filesystems?project_id=bio-project&page=2&page_size=100"
        );
        assert_eq!(
            scaleway_sql_databases_url("fr-par", "bio-project", 3),
            "https://api.scaleway.com/serverless-sqldb/v1alpha1/regions/fr-par/databases?project_id=bio-project&page=3&page_size=100"
        );
        assert_eq!(SCW_FR_PAR_ONLY_REGIONS, &["fr-par"]);
    }

    #[test]
    fn scaleway_state_normalizer_does_not_mark_unknown_as_error() {
        assert_eq!(normalize_scaleway_state("ready"), "running");
        assert_eq!(normalize_scaleway_state("creating"), "provisioning");
        assert_eq!(normalize_scaleway_state("some_future_state"), "unknown");
    }

    #[test]
    fn scaleway_core_sync_failed_when_all_core_loops_failed_despite_generative_success() {
        // The core all-fail guard must key ONLY on the core (X-Auth-Token) APIs.
        // A token with ONLY generative permission would otherwise mask a total
        // core-API auth failure as an empty-but-"synced" inventory.
        // All core loops attempted and none succeeded => hard error.
        assert!(scaleway_core_sync_failed(6, 0));
        // Even one core success means we have a real (possibly partial) inventory.
        assert!(!scaleway_core_sync_failed(6, 1));
        // No core requests attempted at all => not a failure (nothing to fail).
        assert!(!scaleway_core_sync_failed(0, 0));
    }

    #[test]
    fn scaleway_sql_partial_risk_fires_only_when_sql_failures_present() {
        assert!(scaleway_sql_partial_risk(0).is_none());
        let risk = scaleway_sql_partial_risk(3).expect("sql failures must surface a risk");
        assert_eq!(risk.id, "scaleway_serverless_sql_inventory_partial");
        assert_eq!(risk.source, "Scaleway");
        assert_eq!(risk.severity, "medium");
        assert!(
            risk.description.contains("Serverless SQL"),
            "unexpected description: {}",
            risk.description
        );
    }

    #[test]
    fn scaleway_file_partial_risk_uses_dedicated_file_storage_wording() {
        // File Storage failures must surface their OWN risk, never the Block one.
        assert!(scaleway_file_partial_risk(0).is_none());
        let risk = scaleway_file_partial_risk(2).expect("file failures must surface a risk");
        assert_eq!(risk.id, "scaleway_file_storage_inventory_partial");
        assert_eq!(risk.title, "Scaleway File Storage inventory partial");
        assert_eq!(risk.source, "Scaleway");
        assert_eq!(risk.severity, "medium");
        assert!(
            risk.description.contains("File Storage"),
            "File risk must say File Storage: {}",
            risk.description
        );
        assert!(
            !risk.description.contains("Block Storage"),
            "File risk must NOT misattribute to Block Storage: {}",
            risk.description
        );
    }

    #[test]
    fn scaleway_file_storage_failure_alone_is_healthy_not_degraded() {
        // A File Storage 404 on an account that does not use File Storage must NOT
        // flip the provider to degraded, and the message must not be produced.
        let counters = ScalewayInventoryCounters {
            request_count: 10,
            success_count: 8,
            file_failure_count: 2,
            ..Default::default()
        };
        let (status, message) = scaleway_inventory_status(&counters);
        assert_eq!(status, "healthy");
        assert!(message.is_none());
    }

    #[test]
    fn scaleway_block_storage_failure_is_degraded_with_block_wording() {
        let counters = ScalewayInventoryCounters {
            request_count: 10,
            success_count: 8,
            storage_failure_count: 2,
            ..Default::default()
        };
        let (status, message) = scaleway_inventory_status(&counters);
        assert_eq!(status, "degraded");
        let message = message.expect("degraded sync must carry a message");
        assert!(
            message.contains("Block Storage"),
            "Block failure message must say Block Storage: {message}"
        );
        // The dedicated Block risk uses Block wording (not File).
        let risk = RiskFlag {
            id: "scaleway_storage_inventory_partial".into(),
            severity: "medium".into(),
            title: "Scaleway storage inventory partial".into(),
            description: format!(
                "{} Block Storage volume/snapshot lookup(s) failed. Budget storage totals may be incomplete until the next sync.",
                counters.storage_failure_count
            ),
            source: "Scaleway".into(),
            timestamp: now(),
        };
        assert!(risk.description.contains("Block Storage"));
    }

    #[test]
    fn scaleway_serverless_sql_failure_alone_is_healthy() {
        let counters = ScalewayInventoryCounters {
            request_count: 10,
            success_count: 8,
            sql_failure_count: 2,
            ..Default::default()
        };
        let (status, message) = scaleway_inventory_status(&counters);
        assert_eq!(status, "healthy");
        assert!(message.is_none());
        // Its own SQL risk is the only one that should fire for this counter.
        assert!(scaleway_sql_partial_risk(counters.sql_failure_count).is_some());
        assert!(scaleway_file_partial_risk(counters.file_failure_count).is_none());
    }

    #[test]
    fn scaleway_generative_failure_alone_is_healthy() {
        let counters = ScalewayInventoryCounters {
            request_count: 10,
            success_count: 9,
            generative_api_failure_count: 1,
            ..Default::default()
        };
        let (status, message) = scaleway_inventory_status(&counters);
        assert_eq!(status, "healthy");
        assert!(message.is_none());
    }

    #[test]
    fn scaleway_core_inventory_failure_is_degraded() {
        let counters = ScalewayInventoryCounters {
            request_count: 10,
            success_count: 9,
            failure_count: 1,
            ..Default::default()
        };
        let (status, message) = scaleway_inventory_status(&counters);
        assert_eq!(status, "degraded");
        assert!(message.is_some());
    }

    #[test]
    fn scaleway_action_and_object_storage_failures_degrade() {
        let action = ScalewayInventoryCounters {
            request_count: 5,
            success_count: 4,
            action_failure_count: 1,
            ..Default::default()
        };
        assert_eq!(scaleway_inventory_status(&action).0, "degraded");
        let object = ScalewayInventoryCounters {
            request_count: 5,
            success_count: 4,
            object_storage_failure_count: 1,
            ..Default::default()
        };
        assert_eq!(scaleway_inventory_status(&object).0, "degraded");
    }

    #[test]
    fn scaleway_object_storage_only_failure_degrades_without_core_misattribution() {
        // REGRESSION GUARD: an Object Storage (S3) failure must NOT be double-counted
        // under the generic core `failure_count`. It still degrades the provider via
        // `object_storage_failure_count`, but the partial-sync message must attribute
        // it ONLY to "Object Storage lookup(s) failed" — never to "inventory
        // request(s) failed" (which would be the core misattribution we removed).
        let counters = ScalewayInventoryCounters {
            request_count: 6,
            success_count: 5,
            // No core failures: an S3-only failure leaves `failure_count` at 0.
            failure_count: 0,
            object_storage_failure_count: 1,
            ..Default::default()
        };
        // Object Storage alone is enough to degrade.
        assert!(counters.is_degraded());
        // The all-core-fail detector is unaffected: object storage was never a core
        // counter, so a healthy core (all attempted core requests succeeded) does not
        // trip it even though Object Storage failed.
        assert!(!scaleway_core_sync_failed(5, 5));

        let (status, message) = scaleway_inventory_status(&counters);
        assert_eq!(status, "degraded");
        let message = message.expect("degraded sync must carry a partial-sync message");
        // The S3 failure is attributed to Object Storage, and the core portion is 0.
        assert!(
            message.contains("1 Object Storage lookup(s) failed"),
            "S3 failure must be attributed to Object Storage: {message}"
        );
        assert!(
            message.contains("0 inventory request(s) failed"),
            "S3 failure must NOT be attributed to core inventory requests: {message}"
        );
    }

    #[test]
    fn scaleway_idle_risk_description_has_dedicated_branches() {
        let sql = scaleway_idle_risk_description("Serverless SQL", "aspis-bio-db");
        assert!(
            sql.contains("aspis-bio-db") && sql.to_lowercase().contains("cpu"),
            "SQL idle risk must mention the minimum CPU reservation: {sql}"
        );
        let serverless = scaleway_idle_risk_description("Serverless", "aspis-fn");
        assert!(
            serverless.contains("min_scale"),
            "Serverless idle risk keeps its min_scale message: {serverless}"
        );
        let generic = scaleway_idle_risk_description("GPU", "gpu-1");
        assert!(
            generic.contains("running and marked as idle"),
            "generic idle risk keeps its message: {generic}"
        );
        // SQL must NOT fall into the generic branch.
        assert_ne!(sql, generic);
    }

    #[test]
    fn scaleway_state_normalizer_passes_available_through_not_running_not_error() {
        // An idle/detached Block volume or a Generative model reports "available".
        // It is NOT "running" (would mislabel an idle volume) and NOT the catch-all
        // "unknown" (which the frontend renders neutrally but loses the real state).
        // It must pass through as "available" so the UI can render a neutral badge.
        assert_eq!(normalize_scaleway_state("available"), "available");
        assert_eq!(normalize_scaleway_state("AVAILABLE"), "available");
        assert_eq!(normalize_scaleway_state("  available "), "available");
        // Guard against regression: available must never map to error.
        assert_ne!(normalize_scaleway_state("available"), "error");
        assert_ne!(normalize_scaleway_state("available"), "running");
    }

    #[test]
    fn scaleway_project_selector_targets_aspis_bio_not_default() {
        let projects = vec![
            ScwProject {
                id: "default-project".into(),
                name: "default".into(),
            },
            ScwProject {
                id: "launcher-project".into(),
                name: "aspis launcher".into(),
            },
            ScwProject {
                id: "bio-project".into(),
                name: "Aspis Bio".into(),
            },
        ];

        let project = select_scaleway_project(&projects, None).unwrap();

        assert_eq!(project.id, "bio-project");
        assert_eq!(project.name, "Aspis Bio");
    }

    #[test]
    fn scaleway_project_selector_refuses_default_when_bio_is_missing() {
        let projects = vec![ScwProject {
            id: "default-project".into(),
            name: "default".into(),
        }];

        assert!(select_scaleway_project(&projects, None).is_err());
    }

    #[test]
    fn scaleway_project_selector_refuses_ambiguous_bio_matches() {
        let projects = vec![
            ScwProject {
                id: "bio-project-1".into(),
                name: "Aspis Bio".into(),
            },
            ScwProject {
                id: "bio-project-2".into(),
                name: "aspis-bio".into(),
            },
        ];

        assert!(select_scaleway_project(&projects, None).is_err());
    }

    #[test]
    fn scaleway_project_selector_uses_pinned_project_id() {
        let projects = vec![
            ScwProject {
                id: "launcher-project".into(),
                name: "aspis launcher".into(),
            },
            ScwProject {
                id: "bio-project".into(),
                name: "Aspis Bio".into(),
            },
        ];

        let project = select_scaleway_project(&projects, Some("bio-project")).unwrap();

        assert_eq!(project.id, "bio-project");
    }

    #[test]
    fn scaleway_project_selector_rejects_pinned_non_bio_project() {
        let projects = vec![
            ScwProject {
                id: "launcher-project".into(),
                name: "aspis launcher".into(),
            },
            ScwProject {
                id: "bio-project".into(),
                name: "Aspis Bio".into(),
            },
        ];

        assert!(select_scaleway_project(&projects, Some("launcher-project")).is_err());
    }

    #[test]
    fn scaleway_urls_are_scoped_to_project_id() {
        assert_eq!(
            scaleway_servers_url("fr-par-1", "bio-project", 2),
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers?project=bio-project&page=2&per_page=100"
        );
        assert_eq!(
            scaleway_server_products_url("fr-par-1"),
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/products/servers"
        );
        assert_eq!(
            scaleway_server_availability_url("fr-par-1"),
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/products/servers/availability"
        );
        assert_eq!(
            scaleway_block_volumes_url("fr-par-1", "bio-project", 2),
            "https://api.scaleway.com/block/v1/zones/fr-par-1/volumes?project_id=bio-project&page=2&page_size=100"
        );
        assert_eq!(
            scaleway_block_snapshots_url("fr-par-1", "bio-project", 2),
            "https://api.scaleway.com/block/v1/zones/fr-par-1/snapshots?project_id=bio-project&page=2&page_size=100"
        );
        assert_eq!(
            scaleway_server_actions_url("fr-par-1", "srv-1"),
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers/srv-1/action"
        );
        assert_eq!(
            scaleway_server_delete_url("fr-par-1", "srv-1", true),
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers/srv-1?with_volumes=all&with_ip=true&force_shutdown=true"
        );
        assert_eq!(
            scaleway_server_delete_url("fr-par-1", "srv-1", false),
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers/srv-1?with_volumes=all&with_ip=true"
        );
        assert_eq!(
            scaleway_namespaces_url("fr-par", "bio-project", 3),
            "https://api.scaleway.com/functions/v1beta1/regions/fr-par/namespaces?project_id=bio-project&page=3&page_size=100"
        );
        assert_eq!(
            scaleway_functions_url("fr-par", "ns-1", "bio-project", 4),
            "https://api.scaleway.com/functions/v1beta1/regions/fr-par/functions?namespace_id=ns-1&project_id=bio-project&page=4&page_size=100"
        );
        assert_eq!(
            scaleway_containers_url("fr-par", "bio-project", 5),
            "https://api.scaleway.com/containers/v1beta1/regions/fr-par/containers?project_id=bio-project&page=5&page_size=100"
        );
    }

    #[test]
    fn scaleway_container_deserialization_ignores_environment_values() {
        let raw = r#"{
            "id": "container-1",
            "name": "specialists",
            "status": "ready",
            "environment_variables": {"SPECIALISTS_API_KEY": "do-not-surface"},
            "secret_environment_variables": {"PRIVATE_KEY": "do-not-surface-either"}
        }"#;

        let container: ScwContainer = serde_json::from_str(raw).unwrap();
        let project = ScwProject {
            id: "bio-project".into(),
            name: "aspis-bio".into(),
        };
        let summary = container.into_summary("fr-par", &project);
        let serialized = serde_json::to_string(&summary).unwrap();

        assert!(!serialized.contains("do-not-surface"));
        assert!(!serialized.contains("SPECIALISTS_API_KEY"));
        assert!(!serialized.contains("PRIVATE_KEY"));
    }

    #[test]
    fn scaleway_volume_ids_from_server_payload_reads_attached_volumes() {
        let payload = json!({
            "server": {
                "volumes": {
                    "0": {"id": "volume-root"},
                    "1": {"id": "volume-extra"},
                    "2": {"id": ""}
                }
            }
        });

        assert_eq!(
            scaleway_volume_ids_from_server_payload(&payload),
            vec!["volume-root".to_string(), "volume-extra".to_string()]
        );
    }

    #[test]
    fn scaleway_offer_summary_classifies_gpu_and_prices() {
        let mut availability = HashMap::new();
        availability.insert(
            "GPU-H100".into(),
            ScwServerAvailability {
                availability: Some("available".into()),
            },
        );
        let summary = scaleway_offer_summary(
            "fr-par-1",
            "GPU-H100".into(),
            ScwServerProduct {
                arch: Some("x86_64".into()),
                ncpus: Some(24),
                ram: Some(128_000_000_000),
                gpu: Some(1),
                gpu_info: Some(json!({ "name": "H100" })),
                monthly_price: Some(1800.0),
                hourly_price: Some(2.5),
                capabilities: Some(json!({
                    "block_storage": true,
                    "private_network": 8,
                    "max_file_systems": 1
                })),
            },
            &availability,
        );

        assert_eq!(summary.category, "GPU");
        assert_eq!(summary.gpu_label.as_deref(), Some("H100"));
        assert_eq!(summary.availability, "available");
        assert!(summary.tags.contains(&"block-storage".into()));
        assert!(summary.tags.contains(&"private-network".into()));
        assert!(summary.tags.contains(&"filesystem".into()));
    }

    #[test]
    fn cloudflare_json_result_count_reads_common_collection_shapes() {
        assert_eq!(
            cloudflare_json_result_count(
                &json!({"success": true, "result": [{"id": "one"}, {"id": "two"}]}),
                &["buckets"],
            ),
            Some(2)
        );
        assert_eq!(
            cloudflare_json_result_count(
                &json!({"success": true, "result": {"buckets": [{"name": "bio"}]}}),
                &["buckets"],
            ),
            Some(1)
        );
        assert_eq!(
            cloudflare_json_result_count(
                &json!({"success": false, "result": {"buckets": [{"name": "bio"}]}}),
                &["buckets"],
            ),
            Some(0)
        );
    }

    #[test]
    fn cloudflare_durable_objects_inventory_reads_top_level_result_array() {
        // The Durable Objects namespaces endpoint returns `result` as a
        // top-level ARRAY, so item extraction must fall through the
        // `result.as_array()` branch regardless of the configured
        // collection keys (here the DO endpoint's `["namespaces"]`).
        let payload = json!({
            "success": true,
            "result": [
                { "id": "ns-1", "name": "Counter", "class": "Counter" },
                { "id": "ns-2", "name": "Sessions", "class": "Sessions" }
            ]
        });

        assert_eq!(
            cloudflare_json_result_count(&payload, &["namespaces"]),
            Some(2)
        );

        let items = json_result_items(&payload, &["namespaces"]).expect("DO result array");
        let resources: Vec<_> = items
            .into_iter()
            .map(|item| {
                cloudflare_console_resource(
                    "cf-storage-data",
                    "Durable Object Namespace",
                    "https://developers.cloudflare.com/api/resources/durable_objects/",
                    item,
                )
            })
            .collect();

        assert_eq!(resources.len(), 2);
        assert!(resources
            .iter()
            .all(|resource| resource.provider == ProviderId::Cloudflare
                && resource.resource_type == "Durable Object Namespace"));
        assert_eq!(resources[0].name, "Counter");
        assert_eq!(resources[1].name, "Sessions");
    }

    #[test]
    fn cloudflare_console_resource_maps_zone_security_metadata() {
        let resource = cloudflare_console_resource(
            "cf-security-network",
            "DNS Record",
            "https://developers.cloudflare.com/api/resources/dns/subresources/records/",
            json!({
                "id": "dns-id",
                "name": "api.aspis.bio",
                "type": "CNAME",
                "content": "worker.example",
                "modified_on": "2026-05-28T00:00:00Z"
            }),
        );

        assert_eq!(resource.provider, ProviderId::Cloudflare);
        assert_eq!(resource.service_id, "cf-security-network");
        assert_eq!(resource.resource_type, "DNS Record");
        assert_eq!(resource.name, "api.aspis.bio");
        assert_eq!(resource.updated_at.as_deref(), Some("2026-05-28T00:00:00Z"));
    }

    #[test]
    fn cloudflare_console_resource_maps_ai_observability_metadata() {
        let gateway = cloudflare_console_resource(
            "cf-ai-observability",
            "AI Gateway",
            "https://developers.cloudflare.com/api/resources/ai_gateway/",
            json!({
                "id": "gateway-id",
                "name": "aspis-oracle",
                "created_at": "2026-05-28T00:00:00Z"
            }),
        );
        assert_eq!(gateway.provider, ProviderId::Cloudflare);
        assert_eq!(gateway.service_id, "cf-ai-observability");
        assert_eq!(gateway.resource_type, "AI Gateway");
        assert_eq!(gateway.name, "aspis-oracle");

        let ai_search = cloudflare_console_resource(
            "cf-ai-observability",
            "AI Search Instance",
            "https://developers.cloudflare.com/ai-search/api/search/rest-api/",
            json!({
                "id": "aspis-bio-papers",
                "type": "r2",
                "source": "aspis-bio-papers",
                "engine_version": 3,
                "enable": true,
                "ai_gateway_id": "default"
            }),
        );
        let raw = serde_json::to_string(&ai_search).unwrap();
        assert_eq!(ai_search.resource_type, "AI Search Instance");
        assert_eq!(ai_search.name, "aspis-bio-papers");
        assert!(raw.contains("source: aspis-bio-papers"));
        assert!(raw.contains("engine_version: 3"));
        assert!(raw.contains("enable: true"));

        let logpush = cloudflare_console_resource(
            "cf-ai-observability",
            "Logpush Job",
            "https://developers.cloudflare.com/api/resources/logpush/",
            json!({
                "id": 42,
                "name": "worker-observability",
                "dataset": "workers_trace_events",
                "enabled": true,
                "destination_conf": "r2://bio-logs",
                "updated_at": "2026-05-28T01:00:00Z"
            }),
        );
        let raw = serde_json::to_string(&logpush).unwrap();
        assert_eq!(logpush.status, "available");
        assert!(raw.contains("dataset: workers_trace_events"));
        assert!(raw.contains("enabled: true"));
        assert!(raw.contains("destination_conf: r2://bio-logs"));
    }

    #[test]
    fn classify_cloudflare_worker_bindings_splits_by_type_and_never_leaks_secret_values() {
        // Shape mirrors `GET .../workers/scripts/{name}/settings` -> result.bindings.
        let result = json!({
            "compatibility_date": "2026-05-01",
            "bindings": [
                { "name": "LOG_LEVEL", "type": "plain_text", "text": "info" },
                // A secret_text binding from Cloudflare never carries its value;
                // even if a stray "text" appeared we must not surface it.
                { "name": "API_KEY", "type": "secret_text", "text": "should-be-dropped" },
                { "name": "SESSIONS", "type": "kv_namespace", "namespace_id": "kv-123" },
                { "name": "ASSETS", "type": "r2_bucket", "bucket_name": "bio-assets" },
                { "name": "COUNTER", "type": "durable_object_namespace", "class_name": "Counter" },
                { "name": "DB", "type": "d1", "database_id": "d1-456" },
                { "name": "JOBS", "type": "queue", "queue_name": "ingest" },
                { "name": "AUTH", "type": "service", "service": "aspis-auth" }
            ]
        });

        let (plain_text, secrets, other) = classify_cloudflare_worker_bindings(&result);

        assert_eq!(plain_text.len(), 1);
        assert_eq!(plain_text[0].name, "LOG_LEVEL");
        assert_eq!(plain_text[0].text.as_deref(), Some("info"));

        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].name, "API_KEY");
        assert_eq!(secrets[0].binding_type, "secret_text");
        // The value must NEVER be surfaced, even when present in the payload.
        assert_eq!(secrets[0].text, None);

        assert_eq!(other.len(), 6);
        let by_name = |name: &str| other.iter().find(|b| b.name == name).unwrap();
        assert_eq!(by_name("SESSIONS").reference.as_deref(), Some("kv-123"));
        assert_eq!(by_name("ASSETS").reference.as_deref(), Some("bio-assets"));
        assert_eq!(by_name("COUNTER").reference.as_deref(), Some("Counter"));
        assert_eq!(by_name("DB").reference.as_deref(), Some("d1-456"));
        assert_eq!(by_name("JOBS").reference.as_deref(), Some("ingest"));
        assert_eq!(by_name("AUTH").reference.as_deref(), Some("aspis-auth"));
        assert!(other.iter().all(|b| b.text.is_none()));
    }

    #[test]
    fn classify_cloudflare_worker_bindings_handles_missing_bindings() {
        let (plain_text, secrets, other) = classify_cloudflare_worker_bindings(&json!({}));
        assert!(plain_text.is_empty());
        assert!(secrets.is_empty());
        assert!(other.is_empty());
    }

    #[test]
    fn scaleway_iam_console_resource_omits_secret_values() {
        let resource = scaleway_iam_console_resource(
            Some("bio-project"),
            "API Key",
            "https://www.scaleway.com/en/developers/api/iam",
            json!({
                "id": "key-id",
                "access_key": "SCWACCESS",
                "secret_key": "do-not-leak",
                "default_project_id": "bio-project",
                "created_at": "2026-05-28T00:00:00Z"
            }),
        );

        let resource = resource.unwrap();
        let raw = serde_json::to_string(&resource).unwrap();
        assert_eq!(resource.name, "SCWACCESS");
        assert!(raw.contains("SCWACCESS"));
        assert!(!raw.contains("do-not-leak"));
    }

    #[test]
    fn scaleway_iam_console_resource_filters_other_project_keys() {
        let resource = scaleway_iam_console_resource(
            Some("bio-project"),
            "API Key",
            "https://www.scaleway.com/en/developers/api/iam",
            json!({
                "id": "key-id",
                "access_key": "SCWACCESS",
                "default_project_id": "launcher-project"
            }),
        );

        assert!(resource.is_none());
    }

    #[test]
    fn scaleway_project_console_resource_maps_network_and_database_metadata() {
        let network = scaleway_project_console_resource(
            "scw-network-security",
            "Private Network",
            Some("fr-par"),
            "https://www.scaleway.com/en/docs/vpc/",
            json!({
                "id": "pn-id",
                "name": "bio-private",
                "project_id": "bio-project",
                "created_at": "2026-05-28T00:00:00Z"
            }),
        );
        assert_eq!(network.provider, ProviderId::Scaleway);
        assert_eq!(network.service_id, "scw-network-security");
        assert_eq!(network.resource_type, "Private Network");
        assert_eq!(network.region.as_deref(), Some("fr-par"));
        assert!(network
            .metadata
            .iter()
            .any(|item| item.contains("bio-project")));

        let database = scaleway_project_console_resource(
            "scw-data-managed",
            "Managed Database",
            Some("nl-ams"),
            "https://www.scaleway.com/en/developers/api/managed-database-postgre-mysql/",
            json!({
                "id": "db-id",
                "name": "bio-db",
                "status": "ready",
                "engine": "PostgreSQL",
                "version": "16"
            }),
        );
        assert_eq!(database.status, "ready");
        assert!(database
            .metadata
            .iter()
            .any(|item| item.contains("PostgreSQL")));
    }

    #[test]
    fn scaleway_project_console_resource_maps_kms_and_messaging_metadata_without_secrets() {
        let key = scaleway_project_console_resource(
            "scw-network-security",
            "KMS Key",
            Some("fr-par"),
            "https://www.scaleway.com/en/developers/api/key-manager/keys",
            json!({
                "id": "key-id",
                "name": "bio-signing",
                "project_id": "bio-project",
                "usage": "symmetric_encryption",
                "algorithm": "aes_256_gcm",
                "created_at": "2026-05-28T00:00:00Z"
            }),
        );
        assert_eq!(key.provider, ProviderId::Scaleway);
        assert_eq!(key.service_id, "scw-network-security");
        assert_eq!(key.resource_type, "KMS Key");
        assert!(key
            .metadata
            .iter()
            .any(|item| item.contains("symmetric_encryption")));

        let credential = scaleway_project_console_resource(
            "scw-data-managed",
            "SQS Credential",
            Some("nl-ams"),
            "https://www.scaleway.com/en/developers/api/messaging-and-queuing/sqs-api/",
            json!({
                "id": "cred-id",
                "name": "bio-queue-reader",
                "access_key": "SCWQUEUEACCESS",
                "secret_key": "do-not-leak",
                "sqs_endpoint_url": "https://sqs.mnq.nl-ams.scaleway.com"
            }),
        );
        let raw = serde_json::to_string(&credential).unwrap();
        assert_eq!(credential.service_id, "scw-data-managed");
        assert_eq!(credential.resource_type, "SQS Credential");
        assert!(raw.contains("SCWQUEUEACCESS"));
        assert!(raw.contains("sqs_endpoint_url"));
        assert!(!raw.contains("do-not-leak"));
    }

    #[test]
    fn scaleway_block_storage_summary_estimates_monthly_public_price() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let volume = ScwBlockVolume {
            id: "vol-1".into(),
            name: "data-volume".into(),
            size: Some(100_000_000_000),
            perf_iops: Some(15_000),
            status: Some("available".into()),
            state: None,
            tags: vec!["purpose:backup".into()],
            created_at: None,
            updated_at: None,
        }
        .into_summary("fr-par-1", &project);
        let snapshot = ScwBlockSnapshot {
            id: "snap-1".into(),
            name: "snapshot".into(),
            size: Some(100_000_000_000),
            status: Some("available".into()),
            state: None,
            tags: Vec::new(),
            created_at: None,
            updated_at: None,
        }
        .into_summary("fr-par-1", &project);

        assert_eq!(volume.storage_type, "Block Storage 15K");
        assert_eq!(volume.size_gb, 100.0);
        assert_eq!(
            volume.estimated_eur_month.unwrap(),
            100.0 * SCW_BLOCK_15K_EUR_PER_GB_HOUR * SCW_MONTHLY_HOURS
        );
        assert_eq!(snapshot.storage_type, "Block Snapshot");
        assert_eq!(
            snapshot.estimated_eur_month.unwrap(),
            100.0 * SCW_SNAPSHOT_EUR_PER_GB_HOUR * SCW_MONTHLY_HOURS
        );
    }

    #[test]
    fn scaleway_s3_authorization_uses_project_scoped_access_key_without_leaking_secret() {
        let auth = scaleway_s3_authorization(
            "GET",
            "/",
            "",
            "s3.fr-par.scw.cloud",
            "20260527T120000Z",
            "20260527",
            "fr-par",
            &hex_sha256(""),
            "SCWACCESSKEY@bio-project",
            "super-secret-scaleway-key",
        )
        .unwrap();

        assert!(auth.contains("AWS4-HMAC-SHA256"));
        assert!(
            auth.contains("Credential=SCWACCESSKEY@bio-project/20260527/fr-par/s3/aws4_request")
        );
        assert!(auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(!auth.contains("super-secret-scaleway-key"));
    }

    #[test]
    fn scaleway_object_bucket_summary_uses_bounded_usage_estimate() {
        let project = ScwProject {
            id: "bio-project".into(),
            name: "Aspis Bio".into(),
        };
        let bucket = ScwS3Bucket {
            name: "bio-bucket".into(),
            creation_date: Some("2026-05-27T00:00:00Z".into()),
        };
        let usage = ScwObjectBucketUsage {
            total_bytes: 2_000_000_000,
            estimated_eur_month: estimate_monthly_storage_eur(
                2.0,
                SCW_OBJECT_STANDARD_MULTI_AZ_EUR_PER_GB_HOUR,
            ),
            object_count: 2,
            pages_scanned: 1,
            partial: false,
            has_unknown_storage_class: false,
        };

        let summary = bucket.into_summary("fr-par", &project, Some(usage));

        assert_eq!(summary.storage_type, "Object Bucket");
        assert_eq!(summary.size_gb, 2.0);
        assert_eq!(
            summary.estimated_eur_month.unwrap(),
            2.0 * SCW_OBJECT_STANDARD_MULTI_AZ_EUR_PER_GB_HOUR * SCW_MONTHLY_HOURS
        );
        assert!(summary.pricing_label.contains("Scanned 2 object"));
    }

    #[test]
    fn missing_provider_messages_are_permission_specific() {
        let cloudflare = ProviderInventory::missing(ProviderId::Cloudflare);
        assert!(cloudflare.risks[0]
            .description
            .contains("Workers Scripts Write"));

        let scaleway = ProviderInventory::missing(ProviderId::Scaleway);
        assert!(scaleway.risks[0]
            .description
            .contains("VM/serverless operations"));
        assert!(!scaleway.risks[0].description.contains("read-only"));
    }

    #[test]
    fn scaleway_instance_action_plan_maps_safe_power_actions() {
        let resource = test_scaleway_resource("srv-1", "CPU VM", "fr-par-1", None);
        let region = resource.region.clone();

        let start = scaleway_resource_action_plan(&resource, &region, "start").unwrap();
        let stop = scaleway_resource_action_plan(&resource, &region, "stop").unwrap();
        let reboot = scaleway_resource_action_plan(&resource, &region, "reboot").unwrap();

        assert_eq!(
            start.url,
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers/srv-1/action"
        );
        assert_eq!(start.body, json!({ "action": "poweron" }));
        assert_eq!(stop.body, json!({ "action": "poweroff" }));
        assert_eq!(reboot.body, json!({ "action": "reboot" }));
        assert!(scaleway_resource_action_plan(&resource, &region, "backup").is_err());
    }

    #[test]
    fn scaleway_instance_action_plan_supports_delete_as_terminate() {
        let resource = test_scaleway_resource("srv-1", "GPU", "fr-par-1", None);
        let region = resource.region.clone();

        let delete = scaleway_resource_action_plan(&resource, &region, "delete").unwrap();
        let terminate = scaleway_resource_action_plan(&resource, &region, "terminate").unwrap();

        assert_eq!(
            delete.url,
            "https://api.scaleway.com/instance/v1/zones/fr-par-1/servers/srv-1/action"
        );
        assert_eq!(delete.body, json!({ "action": "terminate" }));
        assert_eq!(terminate.body, json!({ "action": "terminate" }));
        assert_eq!(delete.display_action, "delete");
    }

    #[test]
    fn scaleway_instance_action_plan_rejects_unavailable_reported_action() {
        let mut resource = test_scaleway_resource("srv-1", "GPU", "fr-par-1", None);
        resource.available_actions = vec!["poweroff".into()];
        let region = resource.region.clone();

        assert!(scaleway_resource_action_plan(&resource, &region, "start").is_err());
        assert!(scaleway_resource_action_plan(&resource, &region, "stop").is_ok());
    }

    #[test]
    fn scaleway_serverless_action_plan_deploys_function_or_container() {
        let function = test_scaleway_resource("fn-1", "Serverless", "fr-par", Some("node20"));
        let container =
            test_scaleway_resource("ctr-1", "Serverless", "nl-ams", Some("container/http1"));
        let function_region = function.region.clone();
        let container_region = container.region.clone();

        let function_plan =
            scaleway_resource_action_plan(&function, &function_region, "deploy").unwrap();
        let container_plan =
            scaleway_resource_action_plan(&container, &container_region, "deploy").unwrap();

        assert_eq!(
            function_plan.url,
            "https://api.scaleway.com/functions/v1beta1/regions/fr-par/functions/fn-1/deploy"
        );
        assert_eq!(
            container_plan.url,
            "https://api.scaleway.com/containers/v1beta1/regions/nl-ams/containers/ctr-1/deploy"
        );
        assert_eq!(function_plan.body, json!({}));
        assert!(scaleway_resource_action_plan(&function, &function_region, "stop").is_err());
    }

    #[test]
    fn scaleway_pagination_continues_only_when_more_items_are_possible() {
        assert!(scaleway_has_next_page(1, 100, None));
        assert!(!scaleway_has_next_page(1, 12, None));
        assert!(scaleway_has_next_page(1, 100, Some(101)));
        assert!(!scaleway_has_next_page(2, 1, Some(101)));
        assert!(!scaleway_has_next_page(SCW_MAX_PAGES, 100, Some(10_001)));
    }

    fn test_scaleway_resource(
        id: &str,
        resource_type: &str,
        region: &str,
        runtime: Option<&str>,
    ) -> ScalewayResourceSummary {
        ScalewayResourceSummary {
            id: id.into(),
            name: id.into(),
            resource_type: resource_type.into(),
            region: region.into(),
            project_id: Some("bio-project".into()),
            project_name: Some("Aspis Bio".into()),
            state: "running".into(),
            commercial_type: None,
            runtime: runtime.map(str::to_string),
            min_scale: None,
            max_scale: None,
            domain_name: None,
            endpoint: None,
            privacy: None,
            purpose: "test".into(),
            purpose_source: "test".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: None,
            updated_at: None,
            oracle_query: id.into(),
            available_actions: match resource_type {
                "GPU" | "CPU VM" => vec![
                    "poweron".into(),
                    "poweroff".into(),
                    "reboot".into(),
                    "terminate".into(),
                ],
                "Serverless" => vec!["deploy".into()],
                _ => Vec::new(),
            },
            idle_cost_risk: false,
        }
    }

    #[test]
    fn scaleway_destructive_path_reasserts_terminate_available() {
        // C3: terminate present -> ok.
        let ok = test_scaleway_resource("srv-1", "GPU", "fr-par-1", None);
        assert!(assert_scaleway_terminate_available(&ok).is_ok());

        // C3: actions reported but terminate missing -> hard fail.
        let mut blocked = test_scaleway_resource("srv-2", "GPU", "fr-par-1", None);
        blocked.available_actions = vec!["poweron".into(), "reboot".into()];
        assert!(assert_scaleway_terminate_available(&blocked).is_err());

        // No actions known at all -> not blocked here (other guards apply).
        let mut unknown = test_scaleway_resource("srv-3", "GPU", "fr-par-1", None);
        unknown.available_actions = Vec::new();
        assert!(assert_scaleway_terminate_available(&unknown).is_ok());
    }

    // ----- Phase 4: D1 write classification -----

    #[test]
    fn cloudflare_d1_sql_is_write_detects_basic_reads_and_writes() {
        // Reads.
        assert!(!d1_sql_is_write("SELECT * FROM t"));
        assert!(!d1_sql_is_write("  select 1"));
        // Pure read-only CTE: no mutating verb anywhere -> still a read.
        assert!(!d1_sql_is_write("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(!d1_sql_is_write("EXPLAIN QUERY PLAN SELECT 1"));
        // Writes (every documented verb).
        for verb in [
            "INSERT INTO t VALUES (1)",
            "update t set a=1",
            "Delete from t",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN a",
            "CREATE TABLE t (a)",
            "REPLACE INTO t VALUES (1)",
            "MERGE INTO t USING s ON (1) WHEN MATCHED THEN UPDATE SET a=1",
            "TRUNCATE t",
            "PRAGMA foreign_keys=ON",
            "ATTACH DATABASE 'x' AS y",
            "REINDEX t",
            "VACUUM",
        ] {
            assert!(d1_sql_is_write(verb), "expected write: {verb}");
        }
    }

    #[test]
    fn cloudflare_d1_sql_is_write_detects_cte_wrapped_mutations() {
        // A CTE-wrapped mutation must be classified as a WRITE so it cannot skip
        // the confirm gate (`WITH x AS (...) INSERT/UPDATE/DELETE ...`).
        assert!(d1_sql_is_write(
            "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x"
        ));
        assert!(d1_sql_is_write(
            "WITH x AS (SELECT id FROM s) UPDATE t SET a=1 WHERE id IN (SELECT id FROM x)"
        ));
        assert!(d1_sql_is_write(
            "WITH x AS (SELECT id FROM s) DELETE FROM t WHERE id IN (SELECT id FROM x)"
        ));
        // Case-insensitive / leading noise still detected.
        assert!(d1_sql_is_write("  with cte as (select 1) delete from t"));
        // A read-only CTE that merely SELECTS from a table named after a verb is
        // not a write (word-boundary safety is preserved through the scan).
        assert!(!d1_sql_is_write(
            "WITH updates AS (SELECT 1) SELECT * FROM updates"
        ));
    }

    #[test]
    fn cloudflare_d1_sql_is_write_strips_leading_whitespace_and_comments() {
        assert!(d1_sql_is_write("\n\t  DELETE FROM t"));
        assert!(d1_sql_is_write("-- a comment\nDELETE FROM t"));
        assert!(d1_sql_is_write("/* block */ UPDATE t SET a=1"));
        assert!(d1_sql_is_write(
            "-- one\n-- two\n  /* three */\nINSERT INTO t VALUES (1)"
        ));
        // Comments in front of a read stay a read.
        assert!(!d1_sql_is_write("-- delete this later\nSELECT 1"));
        assert!(!d1_sql_is_write("/* UPDATE */ SELECT 1"));
        // Unterminated block comment hides nothing executable -> not a write.
        assert!(!d1_sql_is_write("/* never closed SELECT 1"));
    }

    #[test]
    fn cloudflare_d1_sql_is_write_scans_every_statement() {
        // A benign leading read cannot smuggle a trailing write.
        assert!(d1_sql_is_write("SELECT 1; DELETE FROM t"));
        assert!(d1_sql_is_write("SELECT 1; DROP TABLE x"));
        assert!(d1_sql_is_write("SELECT 1;\n  -- c\n  DROP TABLE t"));
        // A trailing CTE-wrapped mutation after a leading read is still a write.
        assert!(d1_sql_is_write(
            "SELECT 1; WITH x AS (SELECT 1) DELETE FROM t"
        ));
        // Multiple reads remain a read.
        assert!(!d1_sql_is_write("SELECT 1; SELECT 2;"));
        // A `;` inside a string literal must not be mistaken for a separator that
        // exposes a fake write; this whole thing is one SELECT.
        assert!(!d1_sql_is_write("SELECT 'a; DELETE FROM t' AS x"));
    }

    #[test]
    fn cloudflare_d1_sql_is_write_handles_word_boundaries_and_case() {
        // A column/table named like a verb must not trip detection.
        assert!(!d1_sql_is_write("SELECT updated_at FROM t"));
        assert!(!d1_sql_is_write("SELECT * FROM insertions"));
        // Mixed case verbs are writes.
        assert!(d1_sql_is_write("InSeRt INTO t VALUES (1)"));
        // Empty / whitespace-only is not a write.
        assert!(!d1_sql_is_write(""));
        assert!(!d1_sql_is_write("   \n\t  "));
    }

    #[test]
    fn cloudflare_d1_sql_is_write_treats_explain_of_mutation_as_write() {
        // `EXPLAIN` is a harmless leading verb, so a naive first-token check would
        // misclassify a wrapped mutation as a read. The EXPLAIN branch must scan the
        // rest of the statement and gate it.
        assert!(d1_sql_is_write("EXPLAIN INSERT INTO t VALUES (1)"));
        // EXPLAIN wrapping a CTE-wrapped mutation is still a write.
        assert!(d1_sql_is_write(
            "EXPLAIN WITH x AS (SELECT 1) DELETE FROM t"
        ));
        // EXPLAIN QUERY PLAN of a pure read stays a read.
        assert!(!d1_sql_is_write("EXPLAIN QUERY PLAN SELECT * FROM t"));
        // EXPLAIN QUERY PLAN of a mutation is conservatively a write.
        assert!(d1_sql_is_write(
            "EXPLAIN QUERY PLAN DELETE FROM t WHERE id = 1"
        ));
        // A plain `EXPLAIN SELECT` is still a read.
        assert!(!d1_sql_is_write("EXPLAIN SELECT 1"));
    }

    // ----- Phase 4: AI Gateway lossless PUT rebuild -----

    #[test]
    fn ai_gateway_lossless_put_preserves_unedited_fields_and_drops_readonly() {
        let live = json!({
            "id": "default",
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-02T00:00:00Z",
            "account_id": "acc",
            "account_tag": "tag",
            "internal_id": "int",
            "cache_ttl": 60,
            "cache_invalidate_on_update": true,
            "collect_logs": true,
            "logpush": false,
            "rate_limiting_interval": 10,
            "rate_limiting_limit": 100,
            "rate_limiting_technique": "sliding",
            // An unrelated live field must round-trip verbatim under full-replace.
            "guardrails": { "enabled": true },
            "otel": { "exporter": "x" }
        });
        let patch = CloudflareAiGatewaySettingsPatch {
            cache_ttl: Some(120),
            collect_logs: Some(false),
            ..Default::default()
        };

        let body = ai_gateway_lossless_put_body(&live, &patch).unwrap();
        let obj = body.as_object().unwrap();

        // Read-only / server-managed keys are stripped.
        for key in [
            "id",
            "created_at",
            "modified_at",
            "account_id",
            "account_tag",
            "internal_id",
        ] {
            assert!(!obj.contains_key(key), "{key} must be dropped");
        }
        // Edited fields applied.
        assert_eq!(obj["cache_ttl"], json!(120));
        assert_eq!(obj["collect_logs"], json!(false));
        // Unedited fields preserved verbatim (incl. nested unrelated objects).
        assert_eq!(obj["cache_invalidate_on_update"], json!(true));
        assert_eq!(obj["logpush"], json!(false));
        assert_eq!(obj["rate_limiting_interval"], json!(10));
        assert_eq!(obj["rate_limiting_limit"], json!(100));
        assert_eq!(obj["rate_limiting_technique"], json!("sliding"));
        assert_eq!(obj["guardrails"], json!({ "enabled": true }));
        assert_eq!(obj["otel"], json!({ "exporter": "x" }));
    }

    #[test]
    fn ai_gateway_lossless_put_backfills_required_when_live_omits_them() {
        // A sparse live object missing the required fields must still produce a
        // body that carries all five required keys (so the full-replace PUT
        // cannot 400), while an empty patch changes nothing else.
        let live = json!({ "rate_limiting_technique": "fixed" });
        let body =
            ai_gateway_lossless_put_body(&live, &CloudflareAiGatewaySettingsPatch::default())
                .unwrap();
        let obj = body.as_object().unwrap();
        for key in [
            "cache_ttl",
            "cache_invalidate_on_update",
            "collect_logs",
            "rate_limiting_interval",
            "rate_limiting_limit",
        ] {
            assert!(obj.contains_key(key), "required {key} must be backfilled");
        }
        assert_eq!(obj["cache_ttl"], json!(0));
        // PRIVACY: backfilled collect_logs must be FAIL-SAFE false, never true.
        assert_eq!(obj["collect_logs"], json!(false));
        // Patch did not touch technique -> preserved (WARNING-4 passthrough).
        assert_eq!(obj["rate_limiting_technique"], json!("fixed"));
    }

    #[test]
    fn ai_gateway_lossless_put_never_force_enables_collect_logs() {
        // A live object with logging OFF and an empty patch must keep it OFF.
        let live = json!({ "collect_logs": false });
        let body =
            ai_gateway_lossless_put_body(&live, &CloudflareAiGatewaySettingsPatch::default())
                .unwrap();
        assert_eq!(body["collect_logs"], json!(false));

        // A live object with logging ON is preserved (we never silently DISABLE
        // an explicit operator choice either).
        let live_on = json!({ "collect_logs": true });
        let body_on =
            ai_gateway_lossless_put_body(&live_on, &CloudflareAiGatewaySettingsPatch::default())
                .unwrap();
        assert_eq!(body_on["collect_logs"], json!(true));

        // The ONLY way logging turns on through this command is an explicit patch.
        let patch_on = CloudflareAiGatewaySettingsPatch {
            collect_logs: Some(true),
            ..Default::default()
        };
        let body_patched = ai_gateway_lossless_put_body(&json!({}), &patch_on).unwrap();
        assert_eq!(body_patched["collect_logs"], json!(true));
    }

    #[test]
    fn ai_gateway_lossless_put_passes_through_unedited_advanced_fields() {
        // Advanced/nested live fields not in the read-only drop list must round-trip
        // verbatim under the full-replace, including rate_limiting_technique
        // (WARNING-4) and guardrails/otel/dlp/cache_* blocks.
        let live = json!({
            "cache_ttl": 60,
            "cache_invalidate_on_update": false,
            "collect_logs": false,
            "rate_limiting_interval": 5,
            "rate_limiting_limit": 50,
            "rate_limiting_technique": "sliding",
            "guardrails": { "enabled": true, "categories": ["x"] },
            "otel": { "exporter": "otlp" },
            "dlp": { "enabled": false },
            "cache_key": "k"
        });
        let body =
            ai_gateway_lossless_put_body(&live, &CloudflareAiGatewaySettingsPatch::default())
                .unwrap();
        let obj = body.as_object().unwrap();
        assert_eq!(obj["rate_limiting_technique"], json!("sliding"));
        assert_eq!(
            obj["guardrails"],
            json!({ "enabled": true, "categories": ["x"] })
        );
        assert_eq!(obj["otel"], json!({ "exporter": "otlp" }));
        assert_eq!(obj["dlp"], json!({ "enabled": false }));
        assert_eq!(obj["cache_key"], json!("k"));
    }

    #[test]
    fn ai_gateway_lossless_put_refuses_non_object_live_result() {
        // A non-object live result must REFUSE (not silently build a blank body that
        // would reset every gateway setting under the full-replace PUT).
        let patch = CloudflareAiGatewaySettingsPatch::default();
        for live in [
            json!(null),
            json!("x"),
            json!(7),
            json!([1, 2]),
            json!(true),
        ] {
            assert!(
                ai_gateway_lossless_put_body(&live, &patch).is_err(),
                "non-object live {live} must be refused"
            );
        }
        // An empty OBJECT is still a valid (sparse) shape and is allowed.
        assert!(ai_gateway_lossless_put_body(&json!({}), &patch).is_ok());
    }

    #[test]
    fn ai_gateway_settings_from_value_reads_typed_fields() {
        let result = json!({
            "cache_ttl": 30,
            "collect_logs": false,
            "rate_limiting_limit": 50,
            "rate_limiting_technique": "sliding"
        });
        let settings = ai_gateway_settings_from_value("acc", "gw", &result);
        assert!(settings.readable);
        assert_eq!(settings.cache_ttl, Some(30));
        assert_eq!(settings.collect_logs, Some(false));
        assert_eq!(settings.rate_limiting_limit, Some(50));
        assert_eq!(settings.rate_limiting_technique.as_deref(), Some("sliding"));
        // Absent fields stay None (we never fabricate values).
        assert_eq!(settings.logpush, None);
        assert_eq!(settings.cache_invalidate_on_update, None);
    }

    // ----- Phase 4: D1 row flattening -----

    #[test]
    fn d1_rows_from_result_flattens_columns_and_cells() {
        let first = json!({
            "meta": { "rows_read": 2, "rows_written": 0 },
            "results": [
                { "id": 1, "name": "a", "active": true, "note": null },
                { "id": 2, "name": "b", "tags": ["x", "y"] }
            ]
        });
        let (columns, rows, count, truncated, read, written) = d1_rows_from_result(&first);
        // Union of keys preserving first-seen order, including the late `tags`.
        assert_eq!(columns, vec!["id", "name", "active", "note", "tags"]);
        assert_eq!(count, 2);
        assert!(!truncated);
        assert_eq!(read, Some(2));
        assert_eq!(written, Some(0));
        assert_eq!(rows[0], vec!["1", "a", "true", "", ""]);
        // Missing key -> empty; array cell -> compact JSON.
        assert_eq!(rows[1], vec!["2", "b", "", "", "[\"x\",\"y\"]"]);
    }

    #[test]
    fn d1_rows_from_result_caps_rows_and_handles_empty() {
        let many: Vec<Value> = (0..(CF_D1_MAX_ROWS + 25))
            .map(|i| json!({ "id": i }))
            .collect();
        let first = json!({ "results": many });
        let (_, rows, count, truncated, _, _) = d1_rows_from_result(&first);
        assert_eq!(count, CF_D1_MAX_ROWS + 25);
        assert_eq!(rows.len(), CF_D1_MAX_ROWS);
        assert!(truncated);

        // No `results` -> empty, not a panic.
        let (cols, rows, count, truncated, _, _) = d1_rows_from_result(&Value::Null);
        assert!(cols.is_empty());
        assert!(rows.is_empty());
        assert_eq!(count, 0);
        assert!(!truncated);
    }

    #[test]
    fn scaleway_block_create_body_matches_documented_shape() {
        let tags = vec!["aspis".to_string(), "bio".to_string()];
        let body = scaleway_block_create_volume_body(
            "data-vol",
            "bio-project",
            10_000_000_000,
            5_000,
            &tags,
        );
        assert_eq!(
            body,
            json!({
                "name": "data-vol",
                "project_id": "bio-project",
                "perf_iops": 5_000,
                "from_empty": { "size": 10_000_000_000u64 },
                "tags": ["aspis", "bio"],
            })
        );
    }

    #[test]
    fn scaleway_block_perf_iops_gate_only_accepts_documented_classes() {
        assert!(scaleway_block_perf_iops_is_valid(5_000));
        assert!(scaleway_block_perf_iops_is_valid(15_000));
        assert!(!scaleway_block_perf_iops_is_valid(0));
        assert!(!scaleway_block_perf_iops_is_valid(10_000));
    }

    #[test]
    fn scaleway_block_resize_refuses_shrink_allows_grow_and_equal() {
        // shrink -> refused
        assert!(scaleway_block_resize_is_allowed(20_000_000_000, 10_000_000_000).is_err());
        // grow -> allowed
        assert!(scaleway_block_resize_is_allowed(10_000_000_000, 20_000_000_000).is_ok());
        // equal -> allowed (no-op)
        assert!(scaleway_block_resize_is_allowed(10_000_000_000, 10_000_000_000).is_ok());
    }

    #[test]
    fn scaleway_block_resize_body_carries_only_size() {
        assert_eq!(scaleway_block_resize_body(42), json!({ "size": 42u64 }));
    }

    #[test]
    fn scaleway_block_snapshot_body_matches_documented_shape() {
        let body = scaleway_block_create_snapshot_body("snap-1", "bio-project", "vol-123", &[]);
        assert_eq!(
            body,
            json!({
                "name": "snap-1",
                "project_id": "bio-project",
                "volume_id": "vol-123",
                "tags": [],
            })
        );
    }

    #[test]
    fn scaleway_file_create_body_uses_bytes_size() {
        let body = scaleway_file_create_body("fs-1", "bio-project", 30_000_000_000, &[]);
        assert_eq!(
            body,
            json!({
                "name": "fs-1",
                "project_id": "bio-project",
                "size": 30_000_000_000u64,
                "tags": [],
            })
        );
    }

    #[test]
    fn scaleway_sql_create_body_includes_org_project_cpu() {
        // Serverless SQL create REQUIRES organization_id (org-scoped) in addition
        // to project_id; cpu_min/cpu_max bound the autoscale range. Exact shape
        // confirmed against the Serverless SQL Databases API.
        let body = scaleway_sql_create_body("aspis-bio-db", "org-1", "bio-project", 0, 8);
        assert_eq!(
            body,
            json!({
                "name": "aspis-bio-db",
                "organization_id": "org-1",
                "project_id": "bio-project",
                "cpu_min": 0,
                "cpu_max": 8,
            })
        );
    }

    #[test]
    fn scaleway_function_create_body_includes_namespace_runtime_memory() {
        // Function create minimally needs namespace_id + name + runtime; we also
        // send memory_limit and the optional scale bounds when provided.
        let body = scaleway_function_create_body(
            "ns-1",
            "ingest-fn",
            "python311",
            Some(256),
            Some(0),
            Some(3),
        );
        assert_eq!(
            body,
            json!({
                "namespace_id": "ns-1",
                "name": "ingest-fn",
                "runtime": "python311",
                "memory_limit": 256,
                "min_scale": 0,
                "max_scale": 3,
            })
        );
        // Optional fields are omitted when None (no null pollution).
        let minimal = scaleway_function_create_body("ns-1", "fn", "go123", None, None, None);
        assert_eq!(
            minimal,
            json!({
                "namespace_id": "ns-1",
                "name": "fn",
                "runtime": "go123",
            })
        );
    }

    #[test]
    fn scaleway_container_create_body_includes_namespace_image_memory() {
        // Container create references an EXISTING registry image (registry_image),
        // never an image build. namespace_id + name are the API-required fields.
        let body = scaleway_container_create_body(
            "ns-1",
            "api-ctr",
            "rg.fr-par.scw.cloud/funcscwbio/api:latest",
            Some(512),
            Some(0),
            Some(2),
        );
        assert_eq!(
            body,
            json!({
                "namespace_id": "ns-1",
                "name": "api-ctr",
                "registry_image": "rg.fr-par.scw.cloud/funcscwbio/api:latest",
                "memory_limit": 512,
                "min_scale": 0,
                "max_scale": 2,
            })
        );
    }

    #[test]
    fn scaleway_namespace_create_body_includes_name_project() {
        let fn_ns = scaleway_namespace_create_body("ingest-ns", "bio-project");
        assert_eq!(
            fn_ns,
            json!({ "name": "ingest-ns", "project_id": "bio-project" })
        );
    }

    #[test]
    fn scaleway_serverless_crud_urls_use_region_segments() {
        assert_eq!(
            scaleway_sql_create_url("fr-par"),
            "https://api.scaleway.com/serverless-sqldb/v1alpha1/regions/fr-par/databases"
        );
        assert_eq!(
            scaleway_sql_database_url("fr-par", "db-1"),
            "https://api.scaleway.com/serverless-sqldb/v1alpha1/regions/fr-par/databases/db-1"
        );
        assert_eq!(
            scaleway_function_namespace_create_url("fr-par"),
            "https://api.scaleway.com/functions/v1beta1/regions/fr-par/namespaces"
        );
        assert_eq!(
            scaleway_function_create_url("fr-par"),
            "https://api.scaleway.com/functions/v1beta1/regions/fr-par/functions"
        );
        assert_eq!(
            scaleway_function_url("fr-par", "fn-1"),
            "https://api.scaleway.com/functions/v1beta1/regions/fr-par/functions/fn-1"
        );
        assert_eq!(
            scaleway_container_namespace_create_url("nl-ams"),
            "https://api.scaleway.com/containers/v1beta1/regions/nl-ams/namespaces"
        );
        assert_eq!(
            scaleway_container_create_url("nl-ams"),
            "https://api.scaleway.com/containers/v1beta1/regions/nl-ams/containers"
        );
        assert_eq!(
            scaleway_container_url("nl-ams", "ctr-1"),
            "https://api.scaleway.com/containers/v1beta1/regions/nl-ams/containers/ctr-1"
        );
    }

    #[test]
    fn scaleway_storage_urls_use_plural_zone_and_region_segments() {
        assert_eq!(
            scaleway_block_create_volume_url("fr-par-1"),
            "https://api.scaleway.com/block/v1/zones/fr-par-1/volumes"
        );
        assert_eq!(
            scaleway_block_volume_url("fr-par-1", "vol-1"),
            "https://api.scaleway.com/block/v1/zones/fr-par-1/volumes/vol-1"
        );
        assert_eq!(
            scaleway_block_create_snapshot_url("fr-par-1"),
            "https://api.scaleway.com/block/v1/zones/fr-par-1/snapshots"
        );
        assert_eq!(
            scaleway_block_snapshot_url("fr-par-1", "snap-1"),
            "https://api.scaleway.com/block/v1/zones/fr-par-1/snapshots/snap-1"
        );
        assert_eq!(
            scaleway_file_create_url("fr-par"),
            "https://api.scaleway.com/file/v1alpha1/regions/fr-par/filesystems"
        );
        assert_eq!(
            scaleway_file_url("fr-par", "fs-1"),
            "https://api.scaleway.com/file/v1alpha1/regions/fr-par/filesystems/fs-1"
        );
    }

    #[test]
    fn md5_base64_matches_known_vectors() {
        // RFC 1321 / openssl golden vectors.
        assert_eq!(
            hex::encode(md5_digest(b"")),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(
            hex::encode(md5_digest(b"abc")),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            hex::encode(md5_digest(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        // Base64(MD5("")) == "1B2M2Y8AsgTpgAmY7PhCfg=="
        assert_eq!(md5_base64(b""), "1B2M2Y8AsgTpgAmY7PhCfg==");
    }

    #[test]
    fn base64_encode_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn scaleway_lifecycle_xml_renders_rule_and_escapes() {
        let rules = vec![ScalewayLifecycleRule {
            id: "expire-logs".into(),
            prefix: "logs/".into(),
            enabled: true,
            expiration_days: 30,
        }];
        let xml = scaleway_lifecycle_xml(&rules);
        assert_eq!(
            xml,
            r#"<LifecycleConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Rule><ID>expire-logs</ID><Filter><Prefix>logs/</Prefix></Filter><Status>Enabled</Status><Expiration><Days>30</Days></Expiration></Rule></LifecycleConfiguration>"#
        );

        // A disabled rule and an id needing escaping.
        let escaped = scaleway_lifecycle_xml(&[ScalewayLifecycleRule {
            id: "a&b<c".into(),
            prefix: String::new(),
            enabled: false,
            expiration_days: 1,
        }]);
        assert!(escaped.contains("<ID>a&amp;b&lt;c</ID>"));
        assert!(escaped.contains("<Status>Disabled</Status>"));
        assert!(escaped.contains("<Prefix></Prefix>"));
    }

    #[test]
    fn parse_scaleway_lifecycle_rules_validates_input() {
        // Valid.
        let ok = parse_scaleway_lifecycle_rules(&json!([
            { "id": "r1", "prefix": "logs/", "expirationDays": 30 }
        ]))
        .unwrap();
        assert_eq!(ok.len(), 1);
        assert!(ok[0].enabled, "enabled defaults to true");
        assert_eq!(ok[0].expiration_days, 30);

        // Not an array.
        assert!(parse_scaleway_lifecycle_rules(&json!({})).is_err());
        // Empty array.
        assert!(parse_scaleway_lifecycle_rules(&json!([])).is_err());
        // Missing id.
        assert!(parse_scaleway_lifecycle_rules(&json!([{ "expirationDays": 1 }])).is_err());
        // Missing/zero expirationDays.
        assert!(parse_scaleway_lifecycle_rules(&json!([{ "id": "r" }])).is_err());
        assert!(
            parse_scaleway_lifecycle_rules(&json!([{ "id": "r", "expirationDays": 0 }])).is_err()
        );
    }

    #[test]
    fn scaleway_s3_lifecycle_authorization_signs_content_md5_without_leaking_secret() {
        let auth = scaleway_s3_authorization_with_md5(
            "PUT",
            "/bio-bucket",
            "lifecycle=",
            "s3.fr-par.scw.cloud",
            "1B2M2Y8AsgTpgAmY7PhCfg==",
            "20260527T120000Z",
            "20260527",
            "fr-par",
            &hex_sha256("<LifecycleConfiguration/>"),
            "SCWACCESSKEY@bio-project",
            "super-secret-scaleway-key",
        )
        .unwrap();
        assert!(auth.contains("AWS4-HMAC-SHA256"));
        assert!(auth.contains("SignedHeaders=content-md5;host;x-amz-content-sha256;x-amz-date"));
        assert!(!auth.contains("super-secret-scaleway-key"));
    }
}
