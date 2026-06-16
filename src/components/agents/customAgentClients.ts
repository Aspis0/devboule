// Pure, DOM-free validation + normalization for user-defined custom agent CLIs.
//
// These rules are the SINGLE source of truth for the Settings → Workspace "Custom
// agent CLIs" form and must mirror the Rust boundary validation
// (backend/projects.rs validate_custom_agent_client) so a value the UI accepts is
// never rejected by the backend and vice-versa. Kept here (next to agentRowModel)
// so it can be unit-tested in node without the DOM.

import type { CustomAgentClient } from "../../types/config";

export const CLIENT_ID_MAX_LENGTH = 32;
export const CLIENT_LABEL_MAX_LENGTH = 40;
export const CLIENT_COMMAND_MAX_LENGTH = 400;

// Built-in CLI ids a custom client must never shadow. Lowercased; the id is
// always normalized to lowercase before this check. MUST mirror the Rust
// RESERVED_CLIENT_IDS in backend/projects.rs — "orchestrator" is the L2.4 local
// Devboule main-coder client, reserved there too (normalize_agent_client).
export const RESERVED_CLIENT_IDS = [
  "codex",
  "claude",
  "powershell",
  "orchestrator",
] as const;

const ID_PATTERN = /^[a-z0-9-]{1,32}$/;

// Any ASCII control char (< 0x20: includes \n \r \0 \t and the C0 set). The
// command is embedded VERBATIM into the launch script; a control char would split
// it into extra script statements while the launch token is still in scope. MUST
// stay byte-for-byte equivalent to the Rust check in
// backend/projects.rs validate_custom_agent_client (`ch.is_control()` over the C0
// range / chars < 0x20).
// eslint-disable-next-line no-control-regex
const CONTROL_CHAR_PATTERN = /[\x00-\x1f]/;

// Derive a candidate id from a human label: lowercase, non-[a-z0-9] runs to a
// single hyphen, trimmed of leading/trailing hyphens, capped. Used to pre-fill
// the id field from the label so the operator usually does not type an id at all.
export function slugifyClientId(label: string): string {
  return label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, CLIENT_ID_MAX_LENGTH)
    // A trailing hyphen can reappear after the slice; strip it again.
    .replace(/-+$/g, "");
}

export interface CustomClientDraft {
  id: string;
  label: string;
  command: string;
}

export interface CustomClientValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"id" | "label" | "command", string>>;
  // The normalized client when ok (id lowercased+trimmed, label/command trimmed).
  value: CustomAgentClient | null;
}

// Validate one draft against the existing list (for the uniqueness check). The
// caller passes the OTHER existing clients (exclude the row being edited). Pure
// and total: never throws, returns inline messages for each invalid field.
export function validateCustomClient(
  draft: CustomClientDraft,
  existing: CustomAgentClient[],
): CustomClientValidation {
  const errors: CustomClientValidation["errors"] = {};
  const id = draft.id.trim().toLowerCase();
  const label = draft.label.trim();
  const command = draft.command.trim();

  if (id.length === 0) {
    errors.id = "Enter an id.";
  } else if (!ID_PATTERN.test(id)) {
    errors.id = "Id must be 1-32 chars of a-z, 0-9 or hyphen.";
  } else if ((RESERVED_CLIENT_IDS as readonly string[]).includes(id)) {
    errors.id = "That id is reserved by a built-in CLI.";
  } else if (existing.some((client) => client.id === id)) {
    errors.id = "That id is already in use.";
  }

  if (label.length === 0) {
    errors.label = "Enter a label.";
  } else if (label.length > CLIENT_LABEL_MAX_LENGTH) {
    errors.label = `Label must be at most ${CLIENT_LABEL_MAX_LENGTH} characters.`;
  }

  if (command.length === 0) {
    errors.command = "Enter the command line to run.";
  } else if (command.length > CLIENT_COMMAND_MAX_LENGTH) {
    errors.command = `Command must be at most ${CLIENT_COMMAND_MAX_LENGTH} characters.`;
  } else if (CONTROL_CHAR_PATTERN.test(command)) {
    errors.command = "Command must not contain newlines, tabs or control characters.";
  }

  const ok = Object.keys(errors).length === 0;
  return {
    ok,
    errors,
    value: ok ? { id, label, command } : null,
  };
}
