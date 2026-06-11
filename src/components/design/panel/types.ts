// Assistant-panel data model + provider metadata (Phase A2 final pass).
//
// The Assistant panel is the prototype's right column (panel.jsx) wired to the REAL
// generation pipeline in DesignView. This module owns the PRESENTATION data shapes
// only — it never touches the pipeline. The message list is a pure projection of the
// existing flow control points (runGenerate / runEdit / the done-effect / errors /
// cancel / self-repair), so the underlying pipeline logic is untouched.

import type { LucideIcon } from "lucide-react";
import { Cpu, Image, Sparkles, Terminal, Wand2 } from "lucide-react";
import type { DesignLlmBackendKind } from "../../../types/config";

/** A single chat row in the assistant transcript. */
export interface AssistantMessage {
  /** Stable, monotonic id used as the React key and for in-place patching. */
  id: number;
  role: "user" | "assistant";
  /** User-message body, or absent for assistant cards (which use title/desc). */
  text?: string;
  /** Optional context label shown as a `.ctx-chip` above a user bubble (e.g. "Editing Hero"). */
  ctx?: string;
  /** Assistant card lifecycle. Absent on user rows. */
  status?: "working" | "done" | "error";
  /** Assistant card bold title. */
  title?: string;
  /** Assistant card detail line (`.desc`). */
  desc?: string;
  /** Fetched grounding sources (HTTP providers only) — file paths shown as `.src-chip`s. */
  sources?: string[];
  /** True when this assistant run was a CLI/agentic provider (B4): no fetched sources;
   *  render the muted "grounds agentically via MCP" note instead. */
  agentic?: boolean;
  /** Node ids this run created/edited — enables "Select on canvas". */
  nodeIds?: string[];
  /** The instruction that produced this run — enables Regenerate / Retry re-runs. */
  instruction?: string;
  /** For an edit run, the node id being edited — so Regenerate/Retry re-runs the edit. */
  editNodeId?: string;
}

/** Cap on the retained transcript to bound memory (oldest dropped first). */
export const MAX_MESSAGES = 200;

/** One design provider's display metadata, keyed by the REAL backend kind. Mirrors
 *  the prototype's DESIGN_PROVIDERS shape (data.jsx) but covers all FIVE kinds the
 *  Rust boundary accepts (the prototype only mocked four). */
export interface DesignProviderMeta {
  id: DesignLlmBackendKind;
  name: string;
  desc: string;
  badge: "MCP" | "LOCAL" | "API";
  icon: LucideIcon;
  /** Fields this kind REQUIRES to form a valid backend (validated by validateDesignBackend).
   *  Used to detect "switching to a kind the saved config can't satisfy yet". */
  needs: Array<"model" | "command" | "baseUrl">;
}

/** All five design providers, in popover order. Labels/descriptions match the
 *  Settings card copy + the prototype's badges (CLI=MCP, HTTP=LOCAL, api=API). */
export const DESIGN_PROVIDERS: readonly DesignProviderMeta[] = [
  {
    id: "claude",
    name: "Claude Code",
    desc: "CLI agent · grounds via MCP",
    badge: "MCP",
    icon: Terminal,
    needs: [],
  },
  {
    id: "codex",
    name: "Codex",
    desc: "CLI agent · grounds via MCP",
    badge: "MCP",
    icon: Terminal,
    needs: [],
  },
  {
    id: "ollama",
    name: "Ollama",
    desc: "Local HTTP · streams live",
    badge: "LOCAL",
    icon: Cpu,
    needs: ["model"],
  },
  {
    id: "omlx",
    name: "oMLX",
    desc: "Local HTTP · streams live",
    badge: "LOCAL",
    icon: Cpu,
    needs: ["model", "baseUrl"],
  },
  {
    id: "api",
    name: "Cheap-API CLI",
    desc: "Your own CLI · prompt via stdin",
    badge: "API",
    icon: Terminal,
    needs: ["command"],
  },
] as const;

/** Explicit metadata for an unknown/legacy backend kind. Returned by providerMeta
 *  instead of masquerading as the first provider, so the chip honestly reads
 *  "Unknown provider" rather than mislabeling the config as e.g. "Claude Code". */
const UNKNOWN_PROVIDER_META: DesignProviderMeta = {
  id: "claude",
  name: "Unknown provider",
  desc: "Unrecognized backend · open Settings to reconfigure",
  badge: "MCP",
  icon: Cpu,
  needs: [],
};

/** Look up provider metadata by kind, falling back to an explicit "Unknown provider"
 *  entry so the chip never crashes on an unknown/legacy kind and never mislabels it. */
export function providerMeta(kind: string | undefined): DesignProviderMeta {
  return DESIGN_PROVIDERS.find((p) => p.id === kind) ?? UNKNOWN_PROVIDER_META;
}

/** Effort levels in selector order, matching the prototype's EFFORT_LEVELS (display
 *  case) — persisted lowercase via validateDesignEffort. */
export const EFFORT_LEVELS: readonly { value: "low" | "medium" | "high"; label: string }[] = [
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
] as const;

/** Title-case an effort value for the model chip ("high" -> "High"). */
export function effortLabel(effort: "low" | "medium" | "high" | undefined): string {
  switch (effort) {
    case "low":
      return "Low";
    case "medium":
      return "Medium";
    case "high":
      return "High";
    default:
      return "High"; // the provider default mirrors the prototype's initial High
  }
}

/** Default per-run timeout (seconds) when the backend has none — mirrors the Rust 180s. */
export const DEFAULT_TIMEOUT_SECS = 180;

/** The three suggestion seeds from the prototype (data.jsx SUGGESTIONS). Clicking one
 *  SEEDS the composer draft (it does NOT send immediately) — matching app.jsx onSuggest. */
export const SUGGESTIONS: readonly { icon: LucideIcon; text: string }[] = [
  { icon: Sparkles, text: "A pricing section coherent with our app" },
  { icon: Image, text: "Hero with a product screenshot placeholder" },
  { icon: Wand2, text: "Redesign the CTA using our brand accent" },
] as const;
