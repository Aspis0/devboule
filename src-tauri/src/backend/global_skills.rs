use std::path::{Path, PathBuf};
   use serde::{Deserialize, Serialize};
   use tauri::{State, Manager, AppHandle};
   use std::io::Read;

   use super::design::{atomic_write, design_write_guard};
   use super::state::BackendState;

   const MAX_SKILL_BYTES: usize = 8192;
   const MAX_NAME_LEN: usize = 64;

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
               let mut file = match std::fs::File::open(&skill_path) {
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
   }
