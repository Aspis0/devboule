//! File collection — collect_text_files, ignore policies, sensitive-path filters.
//!
//! Port of `collect_text_files` + helpers from `oracle/ingestion/chunk_index.py`
//! and `is_sensitive_relative_path` from `oracle/ingestion/parser.py`.

use std::fs;
use std::path::{Path, PathBuf};

use super::chunking;

// ── Excluded directories (lowercased) ────────────────────────────────────────

const EXCLUDED_DIRS: &[&str] = &[
    ".cache",
    ".cxx",
    ".agents",
    ".aspis",
    ".claude",
    ".claude-mimo",
    ".codex",
    ".deepseek",
    ".devboule",
    ".expo",
    ".expo-export",
    ".expo-export-ios",
    ".expo-export-web",
    ".externalnativebuild",
    ".git",
    ".gradle",
    ".gradle-home",
    ".gradle-home-release",
    ".idea",
    ".mypy_cache",
    ".next",
    ".npm-cache",
    ".pytest_cache",
    ".rnaseq-reference-cache",
    ".ruff_cache",
    ".secrets",
    ".dev.vars",
    "aspis-secrets",
    ".tier1-work",
    ".venv",
    ".wrangler",
    "__pycache__",
    "_archive",
    "_baseline",
    "audit-downloads",
    "cellpose_data",
    "build",
    "codex-runs",
    "codex-sessions",
    "coverage",
    "dist",
    "legacy-graph-out",
    "graphify-out",
    "logs",
    "mockups",
    "node_modules",
    "oracle-data",
    "out",
    "outputs",
    "playwright-report",
    "target",
    "test-results",
    "tmp",
    "vendor",
    "venv",
];

const EXCLUDED_RELATIVE_PREFIXES: &[&str] = &[
    "aspis-biovision/aspis-secrets/",
    "aspis-biovision/data/",
    "aspis-biovision/reports/",
    "aspis-biovision/western blot/",
];

const NOISE_FILE_NAMES: &[&str] = &[".aspis-agents.json", ".npmrc", "package-lock.json"];

const SENSITIVE_NAME_PARTS: &[&str] = &["service-account", "private-key"];

const WORKSPACE_IGNORE_FILES: &[&str] = &[".gitignore", ".oracleignore", ".aspisignore"];

// ── is_sensitive_relative_path (ported from parser.py) ───────────────────────

const SENSITIVE_PATH_COMPONENTS: &[&str] = &[
    ".secrets",
    "aspis-secrets",
    ".dev.vars",
    "node_modules",
    "oracle-data",
    "src-tauri/target",
    "target",
];

const SENSITIVE_PATH_SUBSTRINGS: &[&str] = &["package-lock.json"];

const SENSITIVE_SUFFIXES: &[&str] = &[".key", ".pem", ".pfx", ".p12", ".dev.vars"];

const SECRET_DATA_EXTENSIONS: &[&str] = &[
    ".txt", ".json", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".conf", ".env",
];

const SECRET_CONTENT_WORDS: &[&str] = &[
    "secret",
    "credential",
    "creds",
    "vault",
    "token",
    "apikey",
    "api-key",
    "api_key",
    "passwd",
    "password",
];

fn basename_is_secret(name: &str) -> bool {
    let lower = name.to_lowercase();

    // Key / certificate material and Cloudflare dev vars
    if SENSITIVE_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return true;
    }
    // Private SSH keys
    if lower == "id_rsa" || lower.starts_with("id_rsa") {
        return true;
    }
    // Dotenv files
    if lower == ".env" || lower.starts_with(".env.") {
        return true;
    }
    // Content-word secret dumps for data extensions only
    let dot = lower.rfind('.');
    if let Some(suffix) = dot.map(|d| &lower[d..]) {
        if SECRET_DATA_EXTENSIONS.contains(&suffix) {
            if SECRET_CONTENT_WORDS.iter().any(|w| lower.contains(w)) {
                return true;
            }
        }
    }
    false
}

pub fn is_sensitive_relative_path(relative: &str) -> bool {
    let text = relative.replace('\\', "/").to_lowercase();
    let parts: Vec<&str> = text.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return false;
    }
    // Check unconditional sensitive path components
    for component in &parts {
        if SENSITIVE_PATH_COMPONENTS.contains(component) {
            return true;
        }
    }
    // Substring check (catches "src-tauri/target")
    if SENSITIVE_PATH_COMPONENTS.iter().any(|s| text.contains(s)) {
        return true;
    }
    // Structural secret/manifest containers
    if SENSITIVE_PATH_SUBSTRINGS.iter().any(|s| text.contains(s)) {
        return true;
    }
    // Default-deny basename rule
    basename_is_secret(parts.last().unwrap_or(&""))
}

// ── is_vendored_env_path ─────────────────────────────────────────────────────

pub fn is_vendored_env_path(relative_path: &str) -> bool {
    let text = relative_path.replace('\\', "/").to_lowercase();
    for component in text.split('/') {
        if component.is_empty() {
            continue;
        }
        if component == "site-packages" {
            return true;
        }
        if component.ends_with(".dist-info") || component.ends_with(".egg-info") {
            return true;
        }
    }
    false
}

// ── dir_is_install_root ──────────────────────────────────────────────────────

pub fn dir_is_install_root(dirnames: &[String], filenames: &[String]) -> bool {
    for dirname in dirnames {
        let lower = dirname.to_lowercase();
        if lower.ends_with(".dist-info") || lower.ends_with(".egg-info") {
            return true;
        }
    }
    let has_record = filenames.iter().any(|f| f == "RECORD");
    if !has_record {
        return false;
    }
    filenames.iter().any(|f| f == "WHEEL" || f == "METADATA")
}

// ── Gitignore rule parsing and matching ──────────────────────────────────────

#[derive(Debug, Clone)]
struct IgnoreRule {
    negated: bool,
    anchored: bool,
    dir_only: bool,
    pattern: String, // lowercased
}

#[derive(Debug, Clone, Default)]
pub struct IgnorePolicy {
    rules: Vec<IgnoreRule>,
}

// ── fnmatch (ported from Python's fnmatch.fnmatchcase) ──────────────────────

/// fnmatch-compatible glob matching. `*` matches everything (including `/`).
fn fnmatch(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fnmatch_inner(&p, &t)
}

fn fnmatch_inner(p: &[char], t: &[char]) -> bool {
    let mut ti = 0;
    let mut pi = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0;

    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '[' {
            // Character class
            if let Some(close) = find_bracket_close(p, pi) {
                let negated = p[pi + 1] == '!' || p[pi + 1] == '^';
                let class_start = if negated { pi + 2 } else { pi + 1 };
                let matched = char_in_class(t[ti], &p[class_start..close], negated);
                if matched {
                    pi = close + 1;
                    ti += 1;
                } else if let Some(sp) = star_pi {
                    pi = sp;
                    star_ti += 1;
                    ti = star_ti;
                } else {
                    return false;
                }
            } else {
                // No closing bracket, treat [ as literal
                if p[pi] == t[ti] {
                    pi += 1;
                    ti += 1;
                } else if let Some(sp) = star_pi {
                    pi = sp;
                    star_ti += 1;
                    ti = star_ti;
                } else {
                    return false;
                }
            }
        } else if let Some(sp) = star_pi {
            pi = sp;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    // Skip trailing stars in pattern
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn find_bracket_close(p: &[char], open: usize) -> Option<usize> {
    for i in (open + 1)..p.len() {
        if p[i] == ']' {
            return Some(i);
        }
    }
    None
}

fn char_in_class(ch: char, class_body: &[char], negated: bool) -> bool {
    let mut i = 0;
    let mut matched = false;
    while i < class_body.len() {
        if i + 2 < class_body.len() && class_body[i + 1] == '-' {
            // Range
            if class_body[i] <= ch && ch <= class_body[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if class_body[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    if negated {
        !matched
    } else {
        matched
    }
}

// ── Path prefix glob match ──────────────────────────────────────────────────

fn path_prefix_glob_match(relative_text: &str, pattern: &str) -> bool {
    let parts: Vec<&str> = relative_text.split('/').collect();
    for end in 1..=parts.len() {
        let prefix = parts[..end].join("/");
        if fnmatch(pattern, &prefix) {
            return true;
        }
    }
    false
}

// ── Ignore rule matching ────────────────────────────────────────────────────

fn ignore_rule_matches(
    rule: &IgnoreRule,
    relative_text: &str,
    parts: &[&str],
    _is_dir: bool,
) -> bool {
    let pattern = &rule.pattern;

    if rule.dir_only {
        if rule.anchored {
            return relative_text == pattern.as_str()
                || relative_text.starts_with(&format!("{}/", pattern));
        }
        // Unanchored dir rule
        if parts.iter().any(|part| fnmatch(pattern, part)) {
            return true;
        }
        return path_prefix_glob_match(relative_text, pattern);
    }

    if rule.anchored {
        return fnmatch(pattern, relative_text)
            || relative_text.starts_with(&format!("{}/", pattern));
    }

    // Unanchored file/glob rule
    if parts.iter().any(|part| fnmatch(pattern, part)) {
        return true;
    }
    fnmatch(pattern, relative_text)
}

// ── workspace_ignore_matches ─────────────────────────────────────────────────

pub fn workspace_ignore_matches(
    relative: &Path,
    is_dir: bool,
    ignore_policy: &IgnorePolicy,
) -> bool {
    if ignore_policy.rules.is_empty() {
        return false;
    }
    let mut relative_text = relative.to_string_lossy().to_lowercase().replace('\\', "/");
    relative_text = relative_text.trim_end_matches('/').to_string();
    let parts: Vec<&str> = relative_text.split('/').filter(|s| !s.is_empty()).collect();
    let mut ignored = false;
    for rule in &ignore_policy.rules {
        if ignore_rule_matches(rule, &relative_text, &parts, is_dir) {
            ignored = !rule.negated;
        }
    }
    ignored
}

// ── negation_rescues_under ──────────────────────────────────────────────────

pub fn negation_rescues_under(relative: &Path, ignore_policy: &IgnorePolicy) -> bool {
    if ignore_policy.rules.is_empty() {
        return false;
    }
    let mut dir_text = relative.to_string_lossy().to_lowercase().replace('\\', "/");
    dir_text = dir_text.trim_end_matches('/').to_string();
    let prefix = if dir_text.is_empty() {
        String::new()
    } else {
        format!("{}/", dir_text)
    };

    for rule in &ignore_policy.rules {
        if !rule.negated {
            continue;
        }
        if !rule.anchored {
            // Unanchored negation can match at any depth
            return true;
        }
        // Anchored negation: does its target lie under this directory?
        if prefix.is_empty() || rule.pattern.starts_with(&prefix) || rule.pattern == dir_text {
            return true;
        }
    }
    false
}

// ── path_explicitly_rescued ─────────────────────────────────────────────────

fn path_explicitly_rescued(relative: &Path, ignore_policy: &IgnorePolicy) -> bool {
    if ignore_policy.rules.is_empty() {
        return false;
    }
    let mut relative_text = relative.to_string_lossy().to_lowercase().replace('\\', "/");
    relative_text = relative_text.trim_end_matches('/').to_string();
    let parts: Vec<&str> = relative_text.split('/').filter(|s| !s.is_empty()).collect();
    let mut decision: Option<bool> = None;
    for rule in &ignore_policy.rules {
        if ignore_rule_matches(rule, &relative_text, &parts, false) {
            decision = Some(rule.negated);
        }
    }
    decision == Some(true)
}

// ── load_workspace_ignore_policy ─────────────────────────────────────────────

pub fn load_workspace_ignore_policy(root: &Path) -> IgnorePolicy {
    let mut rules = Vec::new();

    for ignore_name in WORKSPACE_IGNORE_FILES {
        let ignore_path = root.join(ignore_name);
        if !ignore_path.is_file() {
            continue;
        }
        let content = match fs::read_to_string(&ignore_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for raw_line in content.lines() {
            let mut line = raw_line.trim().replace('\\', "/");
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let negated = line.starts_with('!');
            if negated {
                line = line[1..].to_string();
            }
            if line.is_empty() {
                continue;
            }
            let dir_only = line.ends_with('/');
            if dir_only {
                line = line.trim_end_matches('/').to_string();
            }
            if line.is_empty() {
                continue;
            }
            // Anchored if starts with / or contains an interior /
            let anchored = line.starts_with('/') || line.contains('/');
            line = line.trim_start_matches('/').to_string();
            if line.is_empty() {
                continue;
            }
            rules.push(IgnoreRule {
                negated,
                anchored,
                dir_only,
                pattern: line.to_lowercase(),
            });
        }
    }

    IgnorePolicy { rules }
}

// ── directory_path_allowed ──────────────────────────────────────────────────

pub fn directory_path_allowed(path: &Path, root: &Path, ignore_policy: &IgnorePolicy) -> bool {
    let relative = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let relative_posix = relative.to_string_lossy().replace('\\', "/");

    if is_sensitive_relative_path(&relative_posix.to_lowercase()) {
        return false;
    }
    if is_vendored_env_path(&relative_posix) {
        return false;
    }

    let rescue_under = negation_rescues_under(relative, ignore_policy);

    let dir_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if EXCLUDED_DIRS.contains(&dir_name.as_str()) && !rescue_under {
        return false;
    }

    if workspace_ignore_matches(relative, true, ignore_policy) {
        return rescue_under;
    }

    true
}

// ── chunk_path_allowed ──────────────────────────────────────────────────────

pub fn chunk_path_allowed(path: &Path, root: &Path, ignore_policy: &IgnorePolicy) -> bool {
    let relative = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let relative_posix = relative.to_string_lossy().replace('\\', "/");
    let relative_text = relative_posix.to_lowercase();

    if is_sensitive_relative_path(&relative_text) {
        return false;
    }
    if is_vendored_env_path(&relative_text) {
        return false;
    }

    let rescued = path_explicitly_rescued(relative, ignore_policy);

    if !rescued {
        let lower_parts: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        if lower_parts
            .iter()
            .any(|p| EXCLUDED_DIRS.contains(&p.as_str()))
        {
            return false;
        }
    }

    if workspace_ignore_matches(relative, false, ignore_policy) {
        if rescued {
            return true;
        }
        return false;
    }

    if rescued {
        return true;
    }

    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();

    if NOISE_FILE_NAMES.contains(&name.as_str()) {
        return false;
    }
    if name.ends_with(".min.js") || name.ends_with(".min.css") {
        return false;
    }
    if EXCLUDED_RELATIVE_PREFIXES
        .iter()
        .any(|p| relative_text.starts_with(p))
    {
        return false;
    }
    if SENSITIVE_NAME_PARTS
        .iter()
        .any(|p| relative_text.contains(p))
    {
        return false;
    }

    true
}

// ── Priority ordering ────────────────────────────────────────────────────────

pub fn priority_rank(relative: &str) -> usize {
    let r = relative.to_lowercase();
    if r.starts_with("aspis-lab/cloudflare/")
        || r.starts_with("aspis-lab/compute/")
        || r.starts_with("aspis-biovision/src/")
        || r.starts_with("aspis-biovision/scripts/")
        || r.starts_with("aspis-biovision/deploy/")
        || r.contains("/workers/")
        || r.contains("scaleway")
        || r.contains("cloudflare")
        || (r.contains("biovision") && r.contains("worker"))
    {
        return 0;
    }
    if r.starts_with("aspis-lab/src/")
        || r.starts_with("aspis-lab/android/")
        || r.starts_with("aspis-biovision/orasis/")
        || r.starts_with("aspis-lab/tests/")
        || r.starts_with("aspis-biovision/tests/")
    {
        return 1;
    }
    if r.ends_with(".md") || r.ends_with(".txt") || r.contains("/docs/") || r.starts_with("docs/") {
        return 2;
    }
    3
}

pub fn priority_key(path: &Path, root: &Path) -> (usize, String) {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    (priority_rank(&relative), relative)
}

// ── collect_text_files ──────────────────────────────────────────────────────

pub fn collect_text_files(root: &Path) -> Vec<PathBuf> {
    let root = root.to_path_buf();
    let ignore_policy = load_workspace_ignore_policy(&root);
    let mut files = Vec::new();

    walk_recursive(&root, &root, &ignore_policy, &mut files);

    files.sort_by(|a, b| {
        let ka = priority_key(a, &root);
        let kb = priority_key(b, &root);
        ka.cmp(&kb)
    });

    files
}

fn walk_recursive(
    current: &Path,
    root: &Path,
    ignore_policy: &IgnorePolicy,
    files: &mut Vec<PathBuf>,
) {
    // Read directory entries
    let entries: Vec<fs::DirEntry> = match fs::read_dir(current) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };

    // Separate into dirs and files (mimicking os.walk)
    let mut dirnames: Vec<String> = Vec::new();
    let mut filenames: Vec<String> = Vec::new();

    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                dirnames.push(name);
            } else if ft.is_file() {
                filenames.push(name);
            }
        }
    }

    // Installed-package / vendored env pruning
    if current != root && dir_is_install_root(&dirnames, &filenames) {
        return;
    }

    // Filter directories
    dirnames.retain(|dirname| {
        let child_path = current.join(dirname);
        directory_path_allowed(&child_path, root, ignore_policy)
    });

    // Check files
    for filename in &filenames {
        let path = current.join(filename);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();

        if !chunking::is_text_extension(&ext) {
            continue;
        }
        if !chunk_path_allowed(&path, root, ignore_policy) {
            continue;
        }
        if let Ok(metadata) = path.metadata() {
            if metadata.len() > chunking::CHUNK_MAX_FILE_BYTES {
                continue;
            }
        } else {
            continue;
        }
        files.push(path);
    }

    // Sort dirnames for deterministic walk order
    dirnames.sort();
    for dirname in &dirnames {
        walk_recursive(&current.join(dirname), root, ignore_policy, files);
    }
}
