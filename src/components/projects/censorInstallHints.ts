// Censor doctor — maps a missing deterministic-runner binary (as reported by `censor_status`) to a
// suggested install command. Surfaced on the red "missing tool" chips (copy-to-clipboard); the app
// NEVER runs these itself (executing brew/npm/pip on the user's behalf is out of scope). See
// docs/censor-runners-and-install.md for the full table.

export const CENSOR_INSTALL_HINTS: Record<string, string> = {
  // Rust toolchain (clippy / cargo-check / cargo-fmt all probe the `cargo` binary)
  cargo: "rustup component add clippy rustfmt",
  "cargo-deny": "cargo install cargo-deny",
  // JS/TS (npm global)
  tsc: "npm i -g typescript",
  eslint: "npm i -g eslint",
  prettier: "npm i -g prettier",
  oxlint: "npm i -g oxlint",
  knip: "npm i -g knip",
  jscpd: "npm i -g jscpd",
  stylelint: "npm i -g stylelint",
  npm: "install Node.js (ships npm)",
  // Python (pip / pipx)
  ruff: "pip install ruff",
  pyright: "pip install pyright",
  bandit: "pip install bandit",
  vulture: "pip install vulture",
  "pip-audit": "pip install pip-audit",
  yamllint: "pip install yamllint",
  sqlfluff: "pip install sqlfluff",
  semgrep: "pip install semgrep",
  lizard: "pip install lizard",
  zizmor: "pip install zizmor",
  // Go (ships its own tools)
  go: "install Go (ships gofmt + go vet)",
  gofmt: "install Go (ships gofmt)",
  // Homebrew-packaged (scoop/choco on Windows, apt/dnf on Linux)
  gitleaks: "brew install gitleaks",
  cppcheck: "brew install cppcheck",
  ktlint: "brew install ktlint",
  shellcheck: "brew install shellcheck",
  hadolint: "brew install hadolint",
  actionlint: "brew install actionlint",
  tidy: "brew install tidy-html5",
};

/** The suggested install command for a missing tool, or undefined if we have no hint. */
export function installHintFor(tool: string): string | undefined {
  return CENSOR_INSTALL_HINTS[tool];
}
