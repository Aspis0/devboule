# Censor — deterministic runners & how to install them

The Censor runs a layer of **deterministic static-analysis tools** on the project's files. Each tool
is optional: if its binary is not found on PATH the layer is **silently skipped** (the in-app doctor —
the red chips in the CensorStrip / CensorPanel — surfaces which are missing and the install command).

> The app resolves binaries against an **augmented PATH** (Homebrew/npm/cargo/… dirs a login shell
> would have), so a GUI launch from Finder/Dock still finds them (Phase 0). You do NOT need to launch
> the app from a terminal.

## When each runner fires
- **FINE** pass = per-file, on every save (snappy). **COARSE** pass = whole-project, debounced (slow
  compile/dep tools). Cross-cutting tools scan every file regardless of language.

## Runners by language

| Language / trigger | Runners (pass) | What they check |
|---|---|---|
| **Rust** `.rs` | clippy·cargo-check·cargo-audit·cargo-deny·cargo-fmt (coarse) | lints, type errors, CVEs, dep policy, formatting |
| **TS/JS** `.ts .tsx .js .jsx` | tsc·knip·npm-audit (coarse) · eslint·prettier·oxlint (fine) | types, dead exports, dep CVEs, lint, formatting |
| **Python** `.py .pyi` | pip-audit (coarse) · ruff·ruff-format·pyright·bandit·vulture (fine) | dep CVEs, lint, formatting, types, security, dead code |
| **Go** `.go` | gofmt (fine)·go-vet (coarse) | formatting, correctness |
| **C/C++** `.c .h .cpp .hpp …` | cppcheck (fine) | static analysis |
| **Kotlin** `.kt .kts` | ktlint (fine) | style/formatting |
| **Shell** `.sh .bash .zsh .ksh` | shellcheck (fine) | shell analysis |
| **YAML** `.yml .yaml` | yamllint (fine) · actionlint·zizmor for `.github/workflows` | lint, CI correctness/hardening |
| **SQL** `.sql` | sqlfluff (fine) | lint/format |
| **CSS** `.css .scss .sass .less` | stylelint (fine) | lint |
| **HTML** `.html .htm` | tidy (fine) | validity |
| **Dockerfile** | hadolint (fine) | lint |
| **Every file (cross-cutting)** | gitleaks·jscpd·zizmor (coarse) · semgrep·lizard (fine) | secrets, copy-paste, CI, security patterns, complexity |

## Install commands

**macOS (Homebrew + language toolchains):**
```sh
# Rust (rustup)
rustup component add clippy rustfmt
cargo install cargo-audit cargo-deny
# JS/TS (npm global)
npm i -g typescript eslint prettier oxlint knip jscpd stylelint
# Python (pip / pipx)
pip install ruff pyright bandit vulture pip-audit yamllint sqlfluff semgrep lizard zizmor
# Homebrew-packaged tools
brew install gitleaks cppcheck ktlint shellcheck hadolint actionlint tidy-html5
# Go ships gofmt + `go vet`; npm-audit ships with npm
```

**Linux:** same, replacing `brew install …` with your package manager (apt/dnf/pacman) where the tool
is packaged, else the language installer above (cargo/npm/pip). **Windows:** scoop/choco for the
Homebrew set (`scoop install gitleaks shellcheck hadolint …`); cargo/npm/pip identical.

## Notes
- Many newer runners are **advisory-capped** at Medium severity (they display/train but do not gate
  the mini-coder loop) until their false-positive rate is measured.
- The optional **AI Review** tier (a local Ollama/oMLX model, or cloud) is configured separately in
  Settings → "Censor local AI"; it runs async via Pigeon (opt-in) and is unrelated to these binaries.
