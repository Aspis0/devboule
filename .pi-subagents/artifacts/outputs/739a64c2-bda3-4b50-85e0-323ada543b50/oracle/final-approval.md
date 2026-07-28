# Oracle Final Approval — PORT_MACOS_TO_WINDOWS_FINAL.md

> Run: 739a64c2 (oracle, fresh context, background)
> Date: 2026-07-27
> Verdetto: APPROVED — procedi con M0 dopo i due fix pre-flight sotto.

---

## Inherited decisions (reconstructed)

The FINAL plan is the SSOT, supersedes the original + two amendments, hardened through two hostile oracle reviews (557534c2, 779c81b5) plus one delegate investigation (4a0acb47). Key locked decisions:

| Decision | Choice | Verified in repo |
|---|---|---|
| Windows crate version | Extend existing windows=0.58 (NOT 0.62) | src-tauri/Cargo.toml:152 confirmed 0.58 |
| Job Object API | Raw windows::Win32::System::JobObjects, NOT win32job crate | win32job not in any Cargo.toml |
| Windows Hello | UserConsentVerifier (already shipped), NOT KeyCredentialManager | auth.rs shipped code untouched |
| GPU detection | DXGI + WARP filter (already shipped) | hardware.rs untouched |
| ort unify | rc.10 to rc.12 single-RC + api-24 | oracle-core/Cargo.toml:50,57,61 still rc.10 (unify pending) |
| is_enforced() | false on Windows until C1+C2+C3+C4 + reviewer + oracle | mod.rs:207 returns false on Windows |
| Sandbox file | windows.rs does NOT exist | sandbox has only mod.rs + seatbelt.rs |
| CI | .github/workflows does NOT exist | .github absent entirely |
| bundle.windows | NOT in tauri.conf.json | bundle has no windows key |

## Websearch verification (this session, 3 queries)

| Claim | Source | Result |
|---|---|---|
| windows=0.58 exposes Win32_System_JobObjects, Win32_Security, Win32_NetworkManagement_WindowsFilteringPlatform | docs.rs/crate/windows/0.58.0/features | CONFIRMED all three present in 0.58 |
| ort 2.0.0-rc.12 with default-features=false requires explicit api-24 | docs.rs/crate/ort/2.0.0-rc.12/features | CONFIRMED api-24 is default feature, must re-add when defaults disabled. directml and coreml both present |
| Tauri 2 bundle.windows schema (webviewInstallMode, nsis) valid | schema.tauri.app/config/2 + docs.rs/tauri-utils | CONFIRMED schema unchanged, plan block valid |

## Repo state verified

- git status: 4 modified files (ui-pilot removal), specs/ and .pi-subagents/ untracked
- git log -10: HEAD d97cb1d Update README.md, clean main
- src-tauri/Cargo.toml: windows=0.58 with 9 existing features; webview2-com=0.38.2, windows_capture=package windows=0.61.3 (second-version pin, untouched by M0)
- oracle-core/Cargo.toml: ort=2.0.0-rc.10 lines 50 (macOS/coreml), 57 (windows/directml), 61 (linux). No default-features=false, no api-*. Unify not started
- tauri.conf.json: bundle object exists but has NO windows key
- mod.rs:207: is_enforced() returns false on cfg(target_os=windows). Correct
- src-tauri/src/backend/sandbox/windows.rs: does NOT exist (dir has mod.rs, seatbelt.rs)
- .github/workflows/: does NOT exist (.github/ absent entirely)

---

## Risultato: APPROVED

Il piano e coerente, i claim load-bearing sono verificati, lo stato del repo corrisponde a quanto il piano assume. Due fix pre-flight necessari (sotto), nessun blocker strutturale.

---

## Rischi residui che il piano NON copre (5)

1. CI usa sintassi workspace (cargo test --all, cargo check -p devboule) ma il repo NON ha un root Cargo.toml workspace. Ogni crate (src-tauri/, oracle-core/, devboule-mcp/) e indipendente. cargo test --all dalla root fallisce con could-not-find-Cargo.toml. Il worker di Milestone H deve: (a) cd in ogni crate dir e lanciare cargo test separatamente, oppure (b) il piano deve aggiungere un root [workspace] Cargo.toml (scope change non nel piano). Lo stesso problema colpisce i comandi verify di M0 (cargo check -p devboule) e ort-unify (cargo check -p oracle-core). Severity: medium — blocca H ma non M0.

2. nsis.installMode=perMachine richiede elevazione UAC all'installazione. Amendment 1 sez C lo noto (overrides Tauri default of CurrentUser) ma il FINAL plan non lo riporta nella tabella decisioni sez 4. Per single-maintainer dev tool, CurrentUser (default) potrebbe essere piu appropriato. Severity: low — facilmente reversibile, ma l'utente dovrebbe decidere consciamente.

3. Cargo.lock non menzionato nell'acceptance di M0. Aggiungere 4 feature al windows=0.58 block rigenerera il Cargo.lock con nuovi dep transitivi. Il worker deve committare il Cargo.lock aggiornato insieme al Cargo.toml di M0. Severity: low — disciplina di commit, non blocker tecnico.

4. CI non installa o builda il frontend. Il workflow Milestone H ha setup-node@v4 ma nessun npm install / npm run build. cargo check -p devboule invoca tauri-build che valida tauri.conf.json ma NON esegue beforeBuildCommand. cargo test potrebbe peró fallire se un test dipende dal frontendDist (../dist). La smoke test di Milestone A legge solo JSON, quindi passa. Severity: low-medium.

5. Structural change ort: il piano propone ort in [dependencies] base con default-features=false + override per-target, ma l'attuale oracle-core/Cargo.toml ha ort SOLO in sezioni target-conditional. Cambiamento strutturale, non solo version bump. Cargo #11779 potrebbe causare feature unification inaspettata. Il verify step (cargo metadata | grep ort) e vago — il worker dovrebbe usare cargo tree -i ort e verificare UNA sola versione risolta. Severity: medium.

---

## Pre-flight checklist prima del primo commit di M0 (8)

1. Committa la rimozione di ui-pilot PRIMA di iniziare M0. I 4 file modificati (Cargo.toml, Cargo.lock, lib.rs, package-lock.json) sono una rimozione pulita e autonoma, logicamente scollegata dal port Windows. M0 modifica src-tauri/Cargo.toml — lo STESSO file. Non mischiare i due commit.
2. Verifica rustup target add x86_64-pc-windows-msvc sia installato (one-time).
3. Le feature da AGGIUNGERE in M0 sono 4, non 6: Win32_System_JobObjects, Win32_Security, Win32_System_Memory, Win32_NetworkManagement_WindowsFilteringPlatform. Win32_System_Threading e Win32_Foundation sono GIA presenti — Cargo dedupe, ma non duplicarle inutilmente.
4. Preserva TUTTE le 9 feature esistenti del block windows=0.58: Foundation, Security_Credentials_UI, Win32_Foundation, Win32_Graphics_Dxgi, Win32_Graphics_Dxgi_Common, Win32_Storage_FileSystem, Win32_System_Threading, Win32_System_WinRT, Win32_UI_WindowsAndMessaging. Il worker deve copiare quelle reali, non inferire dalla lista commento.
5. Lancia M0 verify da src-tauri/ (NON dalla root): cd src-tauri poi cargo check --target x86_64-pc-windows-msvc. Il -p devboule del piano richiede workspace che non esiste.
6. Committa il src-tauri/Cargo.lock rigenerato nello STESSO commit di M0.
7. Dopo M0, verifica cargo tree -i windows mostra ancora solo 2 versioni: 0.58 (principale) e 0.61.3 (pin di windows_capture). Nessuna terza versione.
8. Non toccare windows_capture o webview2-com — M0 e puramente additive al block 0.58.

---

## Working tree note

I file src-tauri/Cargo.toml, src-tauri/Cargo.lock, src-tauri/src/lib.rs, package-lock.json hanno modifiche non committate dalla rimozione di ui-pilot. Il diff:
- Cargo.toml: rimosso feature ui-pilot, rimosso dep tauri-plugin-pilot (optional path dep)
- lib.rs: rimosso mut dal builder, rimossi due blocchi cfg(all(debug_assertions, feature=ui-pilot))
- Cargo.lock: -187 righe (transitivi di tauri-plugin-pilot)
- package-lock.json: +1 riga (campo license)

Verdetto: NON e sicuro partire con M0 sopra questo stato. Committa prima. Motivo: M0 modifica src-tauri/Cargo.toml, lo stesso file gia modificato dalla rimozione ui-pilot. Procedere su un tree sporco mischierebbe due cambiamenti logicamente indipendenti in un unico commit, rendendo il review gate di M0 non riproducibile. La rimozione ui-pilot e un commit atomico autonomo — committalo come refactor: remove ui-pilot dev-only plugin, poi parti M0 su un tree pulito.

package-lock.json (campo license) e banale ma va nello stesso commit della rimozione ui-pilot.

---

## Conclusion

Il piano e solido. I due oracle review precedenti 

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Concrete findings with file paths and severities: src-tauri/Cargo.toml:152 (windows=0.58 confirmed), mod.rs:207 (is_enforced returns false on Windows), oracle-core/Cargo.toml:50/57/61 (ort still rc.10), windows.rs absent, .github absent, tauri.conf.json bundle.windows absent. 5 residual risks with severity levels identified."
    }
  ],
  "changedFiles": [],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    { "command": "git status && git log --oneline -10", "result": "passed", "summary": "4 modified files (ui-pilot removal), HEAD d97cb1d clean main" },
    { "command": "git diff --stat && git diff Cargo.toml lib.rs", "result": "passed", "summary": "ui-pilot removal: Cargo.toml -10 lines, lib.rs -19 lines, Cargo.lock -187 lines, package-lock +1 line" },
    { "command": "ls src-tauri/src/backend/sandbox/ && ls .github 2>&1", "result": "passed", "summary": "sandbox has mod.rs + seatbelt.rs only (no windows.rs); .github absent entirely" },
    { "command": "grep -n is_enforced mod.rs && grep -n ort= oracle-core/Cargo.toml", "result": "passed", "summary": "is_enforced at line 207 returns false on Windows; ort at lines 50/57/61 still rc.10" },
    { "command": "web_search: windows 0.58 features + ort rc.12 api-24 + tauri 2 bundle.windows", "result": "passed", "summary": "All 3 load-bearing claims confirmed via docs.rs primary sources" },
    { "command": "fetch_content: docs.rs/crate/ort/2.0.0-rc.12/features", "result": "passed", "summary": "45 features for ort rc.12: api-24 is default, directml and coreml both present" },
    { "command": "ls Cargo.toml 2>&1 (root workspace check)", "result": "passed", "summary": "No root Cargo.toml — repo has NO workspace, CI cargo test --all will fail from root" }
  ],
  "validationOutput": [
    "All plan assumptions verified against repo HEAD: windows=0.58 present, windows.rs absent, .github absent, ort still rc.10, bundle.windows absent, is_enforced returns false on Windows",
    "All 3 load-bearing websearch claims confirmed via docs.rs primary sources",
    "5 residual risks identified with severity ratings (2 medium, 3 low/low-medium)",
    "8-item pre-flight checklist for M0 first commit",
    "Working tree assessment: ui-pilot removal must be committed before M0 starts"
  ],
  "residualRisks": [
    "CI cargo test --all / -p devboule fails without root workspace Cargo.toml (medium — blocks H not M0)",
    "nsis installMode perMachine requires UAC elevation, not re-flagged in FINAL plan decision table (low)",
    "Cargo.lock regeneration after M0 feature augmentation not in acceptance criteria (low)",
    "CI does not install/build frontend, cargo test may fail for Tauri-instantiating tests (low-medium)",
    "ort structural change (base [dependencies] entry vs current target-only) + Cargo #11779 unification risk (medium)"
  ],
  "noStagedFiles": true,
  "diffSummary": "No files changed by oracle — read-only review. 4 pre-existing uncommitted files from ui-pilot removal identified and assessed in working tree note.",
  "reviewFindings": [
    "no blockers: plan is APPROVED. All inherited decisions verified against repo state. 5 residual risks documented.",
    "pre-flight: commit ui-pilot removal (4 files) as separate atomic commit before starting M0 — M0 touches same src-tauri/Cargo.toml",
    "pre-flight: M0 adds 4 new features not 6 (Threading+Foundation already present) — preserve all 9 existing features",
    "pre-flight: run M0 verify from src-tauri/ not root (-p devboule requires workspace)",
    "gap: Milestone H CI uses workspace syntax (cargo test --all) but repo has no root Cargo.toml — needs fix before H lands"
  ],
  "manualNotes": "Verdetto: APPROVED. Piano SSOT valido e pronto per esecuzione. Procedi: (1) commit ui-pilot removal, (2) taglia M0. Nessun blocker strutturale."
}
```
