use super::model::{GithubConnectionStatus, GithubRepoAccessStatus};
use super::state::BackendState;
use super::vault;
use chrono::Utc;
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tauri::State;

const GITHUB_API: &str = "https://api.github.com";
const USER_AGENT: &str = "Aspis-Management/0.1";

/// One shared blocking GitHub HTTP client. A `reqwest::blocking` client owns an
/// internal runtime; dropping it inside a tokio async context panics. Holding it
/// in a `static` means it is only dropped at process exit on the main thread, so
/// it can never trigger the drop-in-async panic, and we avoid rebuilding it on
/// every call.
static GITHUB_HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[derive(Debug, Deserialize)]
struct GithubUserResponse {
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRepoResponse {
    html_url: Option<String>,
    description: Option<String>,
    private: Option<bool>,
    default_branch: Option<String>,
    open_issues_count: Option<u32>,
    stargazers_count: Option<u32>,
    forks_count: Option<u32>,
    pushed_at: Option<String>,
    updated_at: Option<String>,
    permissions: Option<GithubRepoPermissions>,
}

#[derive(Debug, Deserialize)]
struct GithubRepoPermissions {
    admin: Option<bool>,
    maintain: Option<bool>,
    push: Option<bool>,
    triage: Option<bool>,
    pull: Option<bool>,
}

#[tauri::command]
pub fn get_github_connection_status(
    state: State<'_, BackendState>,
) -> Result<GithubConnectionStatus, String> {
    state.ensure_unlocked()?;
    github_connection_status()
}

#[tauri::command]
pub fn save_github_token(
    state: State<'_, BackendState>,
    token: String,
) -> Result<GithubConnectionStatus, String> {
    state.ensure_unlocked()?;
    let cleaned = token.trim();
    let mut status = github_connection_status_for_token(cleaned, false, "manual_token")?;
    if status.status == "valid" {
        vault::save_github_token(cleaned)?;
        status.configured = true;
        status.message =
            Some("GitHub token is valid and stored in Windows Credential Manager.".into());
    }
    Ok(status)
}

#[tauri::command]
pub fn delete_github_token(
    state: State<'_, BackendState>,
) -> Result<GithubConnectionStatus, String> {
    state.ensure_unlocked()?;
    vault::delete_github_token()?;
    github_connection_status()
}

#[tauri::command]
pub fn import_github_token_from_cli(
    state: State<'_, BackendState>,
) -> Result<GithubConnectionStatus, String> {
    state.ensure_unlocked()?;
    let token = github_cli_token()?;
    let mut status = github_connection_status_for_token(&token, false, "github_cli")?;
    if status.status == "valid" {
        vault::save_github_token(&token)?;
        status.configured = true;
        status.source = "github_cli_imported_to_windows_vault".into();
        status.message =
            Some("Imported your GitHub CLI login into Windows Credential Manager.".into());
    }
    Ok(status)
}

#[tauri::command]
pub fn check_github_repo_access(
    state: State<'_, BackendState>,
    url: String,
) -> Result<GithubRepoAccessStatus, String> {
    state.ensure_unlocked()?;
    github_repo_access_status(&url)
}

pub fn github_connection_status() -> Result<GithubConnectionStatus, String> {
    let checked_at = Some(now());
    let Some(token) = vault::read_github_token()? else {
        return Ok(GithubConnectionStatus {
            configured: false,
            status: "missing".into(),
            source: "windows_vault".into(),
            cli_available: super::projects::command_exists("gh"),
            last_checked_at: checked_at,
            message: Some(
                "No GitHub app token is saved. You can import an existing GitHub CLI login or paste a fine-grained token."
                    .into(),
            ),
            ..GithubConnectionStatus::default()
        });
    };

    github_connection_status_for_token(&token, true, "windows_vault")
}

fn github_connection_status_for_token(
    token: &str,
    configured: bool,
    source: &str,
) -> Result<GithubConnectionStatus, String> {
    let checked_at = Some(now());
    if token.len() < 20 || token.chars().any(char::is_whitespace) {
        return Ok(GithubConnectionStatus {
            configured,
            status: "error".into(),
            source: source.into(),
            cli_available: super::projects::command_exists("gh"),
            last_checked_at: checked_at,
            message: Some("GitHub token is too short or contains whitespace.".into()),
            ..GithubConnectionStatus::default()
        });
    }
    let client = github_client();
    let response = client
        .get(format!("{GITHUB_API}/user"))
        .bearer_auth(token)
        .send()
        .map_err(|e| {
            format!(
                "GitHub user check failed: {}",
                sanitize_error(&e.to_string())
            )
        })?;
    let status = response.status();
    let scopes = parse_scopes(response.headers().get("x-oauth-scopes"));
    let rate_limit_remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    if !status.is_success() {
        return Ok(GithubConnectionStatus {
            configured,
            status: "error".into(),
            source: source.into(),
            cli_available: super::projects::command_exists("gh"),
            scopes,
            rate_limit_remaining,
            last_checked_at: checked_at,
            message: Some(github_status_message(status)),
            ..GithubConnectionStatus::default()
        });
    }
    let user = response.json::<GithubUserResponse>().map_err(|e| {
        format!(
            "GitHub user response could not be parsed: {}",
            sanitize_error(&e.to_string())
        )
    })?;
    Ok(GithubConnectionStatus {
        configured,
        status: "valid".into(),
        source: source.into(),
        cli_available: super::projects::command_exists("gh"),
        login: Some(user.login),
        name: user.name,
        avatar_url: user.avatar_url,
        profile_url: user.html_url,
        scopes,
        rate_limit_remaining,
        last_checked_at: checked_at,
        message: Some("GitHub token is valid.".into()),
    })
}

pub fn github_repo_access_status(url: &str) -> Result<GithubRepoAccessStatus, String> {
    let checked_at = now();
    let Some((owner, repo)) = parse_github_repo(url) else {
        return Ok(GithubRepoAccessStatus {
            url: url.trim().into(),
            status: "invalid".into(),
            checked_at,
            message: Some("This is not a recognized GitHub repository URL.".into()),
            ..GithubRepoAccessStatus::default()
        });
    };
    let api_url = format!("{GITHUB_API}/repos/{owner}/{repo}");
    let client = github_client();
    let mut request = client.get(api_url);
    if let Some(token) = vault::read_github_token()? {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(|e| {
        format!(
            "GitHub repo check failed: {}",
            sanitize_error(&e.to_string())
        )
    })?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(GithubRepoAccessStatus {
            url: github_web_url(&owner, &repo),
            owner: Some(owner),
            repo: Some(repo),
            accessible: false,
            status: "not_accessible".into(),
            checked_at,
            message: Some(
                "GitHub returned 404. The repo is private, missing, or the saved token lacks access."
                    .into(),
            ),
            ..GithubRepoAccessStatus::default()
        });
    }
    if !status.is_success() {
        return Ok(GithubRepoAccessStatus {
            url: github_web_url(&owner, &repo),
            owner: Some(owner),
            repo: Some(repo),
            accessible: false,
            status: "error".into(),
            checked_at,
            message: Some(github_status_message(status)),
            ..GithubRepoAccessStatus::default()
        });
    }
    let repo_response = response.json::<GithubRepoResponse>().map_err(|e| {
        format!(
            "GitHub repo response could not be parsed: {}",
            sanitize_error(&e.to_string())
        )
    })?;
    Ok(GithubRepoAccessStatus {
        url: repo_response
            .html_url
            .unwrap_or_else(|| github_web_url(&owner, &repo)),
        owner: Some(owner),
        repo: Some(repo),
        description: repo_response.description,
        accessible: true,
        private: repo_response.private,
        default_branch: repo_response.default_branch,
        open_issues_count: repo_response.open_issues_count,
        stargazers_count: repo_response.stargazers_count,
        forks_count: repo_response.forks_count,
        pushed_at: repo_response.pushed_at,
        updated_at: repo_response.updated_at,
        permissions: repo_permissions(repo_response.permissions),
        status: "accessible".into(),
        checked_at,
        message: Some("Saved GitHub auth can read this repository.".into()),
    })
}

fn github_client() -> &'static Client {
    GITHUB_HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build GitHub blocking HTTP client")
    })
}

fn github_cli_token() -> Result<String, String> {
    if !super::projects::command_exists("gh") {
        return Err(
            "GitHub CLI is not installed or not in PATH. Paste a fine-grained token instead."
                .into(),
        );
    }
    let output = command_output_timeout("gh", &["auth", "token"], Duration::from_secs(8))?
        .trim()
        .to_string();
    if output.len() < 20 || output.chars().any(char::is_whitespace) {
        return Err(
            "GitHub CLI did not return a usable token. Run `gh auth login` in a terminal first."
                .into(),
        );
    }
    Ok(output)
}

fn command_output_timeout(
    executable: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW so the background CLI (gh) never flashes a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("{executable} could not start: {e}"))?;
    let started_at = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "{executable} failed: {}",
                    sanitize_error(stderr.trim())
                ));
            }
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{executable} timed out."));
        }
        thread::sleep(Duration::from_millis(30));
    }
}

fn parse_scopes(value: Option<&reqwest::header::HeaderValue>) -> Vec<String> {
    value
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn repo_permissions(value: Option<GithubRepoPermissions>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    [
        ("admin", value.admin),
        ("maintain", value.maintain),
        ("push", value.push),
        ("triage", value.triage),
        ("pull", value.pull),
    ]
    .into_iter()
    .filter_map(|(label, enabled)| enabled.unwrap_or(false).then_some(label.to_string()))
    .collect()
}

/// Crate-visible so the clone path (`projects::project_git_clone`) can validate a
/// pasted remote URL with the EXACT same rules (https/github.com only, sanitized
/// owner/repo segments) instead of hand-rolling a second, weaker parser.
pub(crate) fn parse_github_repo(value: &str) -> Option<(String, String)> {
    let mut raw = value.trim().trim_end_matches(".git").to_string();
    if let Some(path) = raw.strip_prefix("git@github.com:") {
        raw = format!("https://github.com/{path}");
    } else if let Some(path) = raw.strip_prefix("ssh://git@github.com/") {
        raw = format!("https://github.com/{path}");
    } else if let Some(path) = raw.strip_prefix("http://github.com/") {
        raw = format!("https://github.com/{path}");
    }
    let parsed = reqwest::Url::parse(&raw).ok()?;
    if parsed.scheme() != "https" || parsed.host_str()? != "github.com" {
        return None;
    }
    let mut parts = parsed.path_segments()?;
    let owner = clean_github_path_segment(parts.next()?)?;
    let repo = clean_github_path_segment(parts.next()?)?;
    Some((owner, repo))
}

fn clean_github_path_segment(value: &str) -> Option<String> {
    let clean = value.trim();
    if clean.is_empty()
        || clean.len() > 100
        || clean
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
    {
        return None;
    }
    Some(clean.to_string())
}

fn github_web_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}")
}

fn github_status_message(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED => {
            "GitHub token was rejected. Replace it with a valid token.".into()
        }
        StatusCode::FORBIDDEN => {
            "GitHub rejected the request. Check token scopes, SSO authorization, or rate limits."
                .into()
        }
        StatusCode::NOT_FOUND => {
            "GitHub returned 404. The resource is missing or private for this token.".into()
        }
        _ => format!("GitHub returned HTTP {}.", status.as_u16()),
    }
}

/// Redact GitHub token values that may appear anywhere inside an error string.
///
/// Covers every documented GitHub token prefix (PAT `ghp_`, OAuth `gho_`, user
/// `ghu_`, server `ghs_`, refresh `ghr_`, fine-grained `github_pat_`) and scans
/// the WHOLE string, not just whitespace-delimited words — a token embedded as
/// `something:ghp_xxx` or inside a URL must still be stripped. The token body
/// (the trailing run of `[A-Za-z0-9_]`) is consumed so no fragment survives.
///
/// Crate-visible so the authenticated git path (`projects::git_run_authenticated`)
/// can scrub the SAME token families out of any git stderr it surfaces — a token
/// echoed by git in an error must be redacted with the exact same logic.
pub(crate) fn sanitize_error(value: &str) -> String {
    const PREFIXES: [&str; 6] = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(prefix) = PREFIXES.iter().find(|p| value[i..].starts_with(**p)) {
            let mut j = i + prefix.len();
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            out.push_str("[redacted-github-token]");
            i = j;
        } else {
            // Advance one full UTF-8 char so `value[i..]` stays on a boundary.
            let ch = value[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_parser_accepts_common_remote_shapes() {
        assert_eq!(
            parse_github_repo("https://github.com/Saurias92/Aspis-bio.git"),
            Some(("Saurias92".into(), "Aspis-bio".into()))
        );
        assert_eq!(
            parse_github_repo("git@github.com:Saurias92/Aspis-bio.git"),
            Some(("Saurias92".into(), "Aspis-bio".into()))
        );
        assert_eq!(
            parse_github_repo("ssh://git@github.com/Saurias92/Aspis-bio.git"),
            Some(("Saurias92".into(), "Aspis-bio".into()))
        );
        assert!(parse_github_repo("https://evil.example/Saurias92/Aspis-bio").is_none());
    }

    #[test]
    fn github_error_sanitizer_redacts_token_prefixes() {
        let clean = sanitize_error("failed ghp_secret github_pat_secret");
        assert!(!clean.contains("ghp_secret"));
        assert!(!clean.contains("github_pat_secret"));
    }

    #[test]
    fn github_error_sanitizer_redacts_all_token_families() {
        // Every documented GitHub token prefix must be stripped, not just ghp_.
        for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"] {
            let secret = format!("{prefix}AbC123deadBEEF456");
            let msg = format!("gh auth token returned {secret} unexpectedly");
            let clean = sanitize_error(&msg);
            assert!(!clean.contains(&secret), "prefix {prefix} leaked: {clean}");
            assert!(clean.contains("[redacted-github-token]"));
        }
    }

    #[test]
    fn github_error_sanitizer_redacts_tokens_not_at_word_boundary() {
        // A token embedded mid-word (colon/url-prefixed) must still be stripped.
        let clean = sanitize_error("auth=token:ghp_aBcDeF123456 in https://x/ghp_AnotherTok99");
        assert!(
            !clean.contains("ghp_aBcDeF123456"),
            "colon-embedded leaked: {clean}"
        );
        assert!(
            !clean.contains("ghp_AnotherTok99"),
            "url-embedded leaked: {clean}"
        );
    }

    #[test]
    fn github_error_sanitizer_preserves_non_token_text() {
        // Plain prose and github.com URLs without a token survive intact.
        let msg = "Could not reach https://github.com/Saurias92/Aspis-bio.git (timeout)";
        assert_eq!(sanitize_error(msg), msg);
    }
}
