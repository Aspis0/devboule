param(
  [switch]$Install,
  [switch]$Pull,
  [string]$Model = "qwen3.5:4b"
)

$ErrorActionPreference = "Stop"

function Require-Command($Name, $InstallHint) {
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "$Name is not available. $InstallHint"
  }
}

if ($Install) {
  Require-Command "winget" "Install App Installer from Microsoft Store or install Ollama manually."
  winget install --id Ollama.Ollama -e
}

Require-Command "ollama" "Run: winget install --id Ollama.Ollama -e"

$version = ollama --version
Write-Output $version

if ($Pull) {
  ollama pull $Model
}

python -m oracle.cli runtime
