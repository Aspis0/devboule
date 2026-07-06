use std::path::{Path, PathBuf};
   use serde::{Deserialize, Serialize};
   use tauri::{State, Manager, AppHandle};
   use std::io::Read;

   use super::design::{atomic_write, design_write_guard};
   use super::state::BackendState;

   const MAX_SKILL_BYTES: usize = 8192;
   const MAX_NAME_LEN: usize = 64;
   const MARKETPLACE_FETCH_MAX_BYTES: usize = 256 * 1024;
   const MARKETPLACE_FETCH_TIMEOUT_SECS: u64 = 15;

   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(rename_all = "camelCase")]
   pub struct GlobalSkillEntry {
       pub name: String,
       pub content: String,
       pub bytes: usize,
       pub truncated: bool,
   }

   pub fn validate_skill_name(name: &str) -> Result<(), String> {
       if name.is_empty() {
           return Err("Skill name cannot be empty.".to_string());
       }
       if name != name.trim() {
           return Err("Skill name cannot have leading or trailing whitespace.".to_string());
       }
       if name.len() > MAX_NAME_LEN {
           return Err(format!("Skill name exceeds maximum length of {} characters.", MAX_NAME_LEN));
       }
       if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
           return Err("Skill name must only contain ASCII alphanumeric characters, hyphens, or underscores.".to_string());
       }
       Ok(())
   }

   pub fn global_base(app: &AppHandle) -> Result<PathBuf, String> {
       let dir = app.path().app_data_dir().map_err(|_| "App data directory is unavailable.".to_string())?;
       let base = dir.join("global-skills");
       std::fs::create_dir_all(&base).map_err(|e| format!("Could not create global skills directory: {e}"))?;
       Ok(base)
   }

   pub fn list_impl(base: &Path) -> Vec<GlobalSkillEntry> {
       let mut entries = Vec::new();
       if let Ok(read_dir) = std::fs::read_dir(base) {
           for entry in read_dir.flatten() {
               let dir_path = entry.path();
               if !dir_path.is_dir() {
                   continue;
               }
               let name = match entry.file_name().into_string() {
                   Ok(n) => n,
                   Err(_) => continue,
               };
               // Skip dirs whose name isn't a valid skill name (e.g. hand-created
               // "my.skill") — listing them would show debris that save/delete reject.
               if validate_skill_name(&name).is_err() {
                   continue;
               }
               let skill_path = dir_path.join("SKILL.md");
               if !skill_path.is_file() {
                   continue;
               }
               let file = match std::fs::File::open(&skill_path) {
                   Ok(f) => f,
                   Err(_) => continue,
               };
               // Bounded read: cap the bytes so a giant global SKILL.md can never fully
               // allocate (one extra byte only to detect truncation). Mirrors project_skill.
               let mut buf = Vec::new();
               let bytes_read = file
                   .take(MAX_SKILL_BYTES as u64 + 1)
                   .read_to_end(&mut buf)
                   .unwrap_or(0);
               let truncated = bytes_read > MAX_SKILL_BYTES;
               let content_bytes = if truncated { &buf[..MAX_SKILL_BYTES] } else { &buf[..] };
               let content = String::from_utf8_lossy(content_bytes).into_owned();
               entries.push(GlobalSkillEntry {
                   name,
                   content,
                   bytes: content_bytes.len(),
                   truncated,
               });
           }
       }
       entries.sort_by(|a, b| a.name.cmp(&b.name));
       entries
   }

   pub fn save_impl(base: &Path, name: &str, content: &str) -> Result<(), String> {
       validate_skill_name(name)?;
       if content.len() > MAX_SKILL_BYTES {
           return Err(format!("Skill content exceeds maximum size of {} bytes.", MAX_SKILL_BYTES));
       }
       let dir_path = base.join(name);
       std::fs::create_dir_all(&dir_path).map_err(|e| format!("Could not create skill directory: {e}"))?;
       let target = dir_path.join("SKILL.md");
       atomic_write(&target, content, "global-skill")
   }

   pub fn delete_impl(base: &Path, name: &str) -> Result<(), String> {
       validate_skill_name(name)?;
       let target = base.join(name);
       if !target.starts_with(base) {
           return Err("Invalid skill path: path traversal detected.".to_string());
       }
       if target.exists() {
           std::fs::remove_dir_all(&target).map_err(|e| format!("Could not delete skill directory: {e}"))?;
       }
       Ok(())
   }

   #[tauri::command]
   pub fn global_skills_list(app: AppHandle, state: State<'_, BackendState>) -> Result<Vec<GlobalSkillEntry>, String> {
       state.ensure_unlocked()?;
       let base = global_base(&app)?;
       Ok(list_impl(&base))
   }

   #[tauri::command]
   pub fn global_skills_save(app: AppHandle, state: State<'_, BackendState>, name: String, content: String) -> Result<(), String> {
       state.ensure_unlocked()?;
       let _g = design_write_guard()?;
       let base = global_base(&app)?;
       save_impl(&base, &name, &content)
   }

   #[tauri::command]
   pub fn global_skills_delete(app: AppHandle, state: State<'_, BackendState>, name: String) -> Result<(), String> {
       state.ensure_unlocked()?;
       let _g = design_write_guard()?;
       let base = global_base(&app)?;
       delete_impl(&base, &name)
   }

   pub fn install_bundled_impl(base: &Path, skill_name: &str) -> Result<(), String> {
       let tpl = super::project_skill::bundled_library_skills()
           .into_iter()
           .find(|t| t.name == skill_name)
           .ok_or_else(|| format!("unknown bundled library skill '{skill_name}'"))?;
       let dest = base.join(&tpl.name).join("SKILL.md");
       if dest.exists() {
           return Err(format!(
               "'{}' is already in your global library; delete it first to reinstall",
               tpl.name
           ));
       }
       save_impl(base, &tpl.name, &tpl.body)
   }

   /// Copies a shipped bundled library skill into the user's global store.
   #[tauri::command]
   pub fn global_skills_install_bundled(
       app: AppHandle,
       state: State<'_, BackendState>,
       skill_name: String,
   ) -> Result<(), String> {
       state.ensure_unlocked()?;
       let _g = design_write_guard()?;
       let base = global_base(&app)?;
       install_bundled_impl(&base, &skill_name)
   }

   /// Network-free core of the global marketplace install: verifies the SHA-256 pin against the
   /// already-fetched `content`, then writes it into the global store via `install_skill_package`.
   /// Separated so it is unit-testable without a live fetch.
   pub fn install_marketplace_content_into(base: &Path, skill_name: &str, content: &str, expected_sha256: &str, fetched_at: &str, source_url: &str) -> Result<String, String> {
       // Gate the name with the GLOBAL-store rule (stricter than skill_marketplace's valid_skill_name,
       // which allows dots): otherwise a "foo.v3" install would land on disk but be invisible to
       // global_skills_list and unremovable by global_skills_delete (both reject dotted names).
       validate_skill_name(skill_name)?;
       if expected_sha256.is_empty() {
           return Err("expected_sha256 is required — preview the skill before installing".to_string());
       }
       let sha = super::skill_marketplace::sha256_hex(content);
       if sha != expected_sha256 {
           return Err("the skill content changed since the preview — re-preview before installing".to_string());
       }
       let prov = super::skill_marketplace::SkillProvenance {
           source_url: source_url.to_string(),
           fetched_at: fetched_at.to_string(),
           sha256: sha,
       };
       std::fs::create_dir_all(base).map_err(|e| format!("create library failed: {e}"))?;
       let dest = super::skill_marketplace::install_skill_package(base, skill_name, content, &[], &prov)?;
       Ok(dest.to_string_lossy().into_owned())
   }

   /// Install a skill from a public URL into the user's GLOBAL store (mirrors the per-project
   /// `skills_marketplace_install` but targets `global_base`). Same SSRF + SHA-256 pin guarantees.
   #[tauri::command]
   pub async fn global_skills_marketplace_install(
       app: AppHandle,
       state: State<'_, BackendState>,
       url: String,
       skill_name: String,
       expected_sha256: String,
       fetched_at: String,
   ) -> Result<String, String> {
       state.ensure_unlocked()?;
       let app_clone = app.clone();
       tokio::task::spawn_blocking(move || {
           let base = global_base(&app_clone)?;
           let (validated, addrs) = super::skill_marketplace::validate_public_url(&url)?;
           let content = super::skill_marketplace::fetch_text_capped(&validated, &addrs, MARKETPLACE_FETCH_MAX_BYTES, MARKETPLACE_FETCH_TIMEOUT_SECS)?;
           // Take the design write guard ONLY around the local write (not the network fetch) so the
           // global store can't be raced by a concurrent delete/save mid-install.
           let _g = design_write_guard()?;
           install_marketplace_content_into(&base, &skill_name, &content, &expected_sha256, &fetched_at, validated.as_str())
       })
       .await
       .map_err(|e| format!("install task failed: {e}"))?
   }

   #[cfg(test)]
   mod tests {
       use super::*;
       use std::process;

       fn fresh_base(tag: &str) -> PathBuf {
           let dir = std::env::temp_dir().join(format!("global-skills-test-{}-{}", process::id(), tag));
           std::fs::create_dir_all(&dir).unwrap();
           dir.join("global-skills")
       }

       #[test]
       fn test_save_and_list() {
           let base = fresh_base("save_list");
           let name = "test-skill";
           let content = "# Test Skill\nSome content here.";
           save_impl(&base, name, content).unwrap();
           let entries = list_impl(&base);
           assert_eq!(entries.len(), 1);
           assert_eq!(entries[0].name, name);
           assert_eq!(entries[0].content, content);
           assert_eq!(entries[0].bytes, content.len());
           assert!(!entries[0].truncated);
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_save_rejects_over_cap() {
           let base = fresh_base("over_cap");
           let content = "x".repeat(MAX_SKILL_BYTES + 1);
           let res = save_impl(&base, "big", &content);
           assert!(res.is_err());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_save_rejects_bad_names() {
           let base = fresh_base("bad_name");
           assert!(save_impl(&base, "../x", "c").is_err());
           assert!(save_impl(&base, "a b", "c").is_err());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_delete() {
           let base = fresh_base("delete");
           let name = "del-me";
           save_impl(&base, name, "content").unwrap();
           assert_eq!(list_impl(&base).len(), 1);
           delete_impl(&base, name).unwrap();
           assert_eq!(list_impl(&base).len(), 0);
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_list_empty_base() {
           let base = fresh_base("empty");
           assert!(list_impl(&base).is_empty());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_install_bundled_known_lands_in_global_store() {
           let base = fresh_base("install-bundled");
           // "code-review" is one of the shipped bundled library skills.
           install_bundled_impl(&base, "code-review").unwrap();
           let entries = list_impl(&base);
           assert_eq!(entries.len(), 1);
           assert_eq!(entries[0].name, "code-review");
           assert!(!entries[0].content.trim().is_empty());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_install_bundled_unknown_errs() {
           let base = fresh_base("install-bundled-unknown");
           assert!(install_bundled_impl(&base, "does-not-exist").is_err());
           // Nothing was written.
           assert!(list_impl(&base).is_empty());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_install_bundled_refuses_when_already_present() {
           let base = fresh_base("install-bundled-twice");
           install_bundled_impl(&base, "code-review").unwrap();
           // A second install must NOT silently clobber a possibly-customized global skill.
           assert!(install_bundled_impl(&base, "code-review").is_err());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_all_bundled_library_skills_install_under_cap() {
           // Every shipped bundled skill must install cleanly (each body under MAX_SKILL_BYTES);
           // names are distinct so there is no refuse-if-present collision.
           let base = fresh_base("install-bundled-all");
           for tpl in super::super::project_skill::bundled_library_skills() {
               install_bundled_impl(&base, &tpl.name)
                   .unwrap_or_else(|e| panic!("bundled skill {} failed to install: {e}", tpl.name));
           }
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_marketplace_content_installs_into_base_with_correct_sha() {
           let base = fresh_base("mkt-ok");
           let content = "# Fetched skill\n\nSome body text.";
           let sha = super::super::skill_marketplace::sha256_hex(content);
           install_marketplace_content_into(
               &base, "fetched-skill", content, &sha, "2026-01-01T00:00:00Z",
               "https://example.com/s.md",
           )
           .unwrap();
           let entries = list_impl(&base);
           assert!(entries.iter().any(|e| e.name == "fetched-skill"));
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_marketplace_content_rejects_sha_mismatch_and_empty() {
           let base = fresh_base("mkt-sha");
           // Wrong sha → reject (server can't swap payload between preview and install).
           assert!(install_marketplace_content_into(
               &base, "x", "body", "deadbeef", "t", "https://e.com/s"
           )
           .is_err());
           // Empty expected sha → hard error (never trust the frontend to always send it).
           assert!(install_marketplace_content_into(&base, "x", "body", "", "t", "https://e.com/s")
               .is_err());
           assert!(list_impl(&base).is_empty());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }

       #[test]
       fn test_marketplace_content_rejects_name_the_global_store_cant_list() {
           let base = fresh_base("mkt-badname");
           let content = "body";
           let sha = super::super::skill_marketplace::sha256_hex(content);
           // A dotted name would pass skill_marketplace's valid_skill_name but be invisible/unremovable
           // in the global store → must be rejected up front.
           assert!(install_marketplace_content_into(
               &base, "foo.v3", content, &sha, "t", "https://e.com/s"
           )
           .is_err());
           assert!(list_impl(&base).is_empty());
           std::fs::remove_dir_all(base.parent().unwrap()).ok();
       }
   }
