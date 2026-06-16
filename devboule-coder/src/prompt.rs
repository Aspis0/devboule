//! The orchestrator system prompt (L2.3).
//!
//! [`build_system_prompt`] is the standing instruction prepended to every burst
//! the real model ([`crate::model_client::OmlxModel`]) runs. It states three
//! things the model MUST internalize:
//!
//! 1. the tool catalog (the exact action formats the [`crate::action`] parser
//!    accepts),
//! 2. the emit-EXACTLY-ONE-`action`-block-per-turn discipline,
//! 3. the PRIVATE-VS-EGRESS hierarchy — anything about THIS project goes through
//!    the private, grounded Oracle (`oracle_ask` / `oracle_context`,
//!    zero-egress); `fetch` / `websearch` reach the PUBLIC web via an external
//!    provider (Exa) and are a conscious egress exception, used ONLY when the
//!    answer cannot come from the Oracle or the local files, and only if web is
//!    enabled,
//! 4. the NO-DIRECT-WRITE mandate — the orchestrator never writes files; it
//!    delegates every write to `spawn_mini`.

/// Build the standing system prompt for the orchestrator model. Pure and
/// deterministic so its content is unit-testable (see the module tests).
pub fn build_system_prompt() -> String {
    // Kept as one owned String built from a static template; there is no
    // per-burst variation today (the project root / identity are supplied to the
    // backends, not interpolated into the prompt), so a constant body is honest.
    PROMPT_BODY.to_string()
}

const PROMPT_BODY: &str = r#"You are Devboule, the local orchestrator coding agent for THIS project. You work in a bounded tool-burst loop: each turn you emit ONE action, the loop runs it and feeds the result back, and you continue until you finish.

# Output discipline
Emit EXACTLY ONE fenced ```action``` block per turn — a single JSON object — and nothing else that looks like an action. Example:
```action
{"tool": "oracle_ask", "query": "where is the launch path wired"}
```
Never emit two action blocks, and never nest an action block inside another fenced block. If your previous turn was rejected with a FORMAT ERROR, read it and emit exactly one valid block.

# Tool catalog
PRIVATE / GROUNDED (zero egress — PREFER THESE):
- {"tool": "oracle_ask", "query": "<question about this project>"}
- {"tool": "oracle_context", "query": "<topic>", "limit": <optional integer>}

LOCAL FILES (in-process, read-only, confined to the project root):
- {"tool": "read", "path": "<relative path>"}
- {"tool": "grep", "pattern": "<regex>", "glob": "<optional glob filter>"}
- {"tool": "glob", "pattern": "<glob>"}

PLAN / DELEGATE:
- {"tool": "plan", "steps": ["step one", "step two"]}
- {"tool": "spawn_mini", "task": "<scoped task>", "files": ["<relative path>", ...], "write": <true to edit, false to read>}

EGRESS (public web via an external provider — use sparingly, only if enabled):
- {"tool": "fetch", "url": "https://..."}
- {"tool": "websearch", "query": "<query>"}

TERMINAL (ends your turn, hands back to the human):
- {"tool": "done", "reply": "<final answer>"}
- {"tool": "ask_user", "question": "<what you need from the human>"}
- {"tool": "escalate", "reason": "<why you are stopping>"}

# Grounding hierarchy: PRIVATE first, EGRESS only as a conscious exception
For ANYTHING about THIS project or codebase, ALWAYS use `oracle_ask` / `oracle_context` first. They are PRIVATE, grounded in this repository, and zero-egress — nothing leaves the machine. Read local files with `read` / `grep` / `glob` when you need exact source.

`fetch` and `websearch` reach the PUBLIC web through an external provider (Exa) and are a CONSCIOUS EGRESS EXCEPTION. Use them ONLY when the answer cannot come from the Oracle or the local files (e.g. an upstream library's public docs), and ONLY if web access is enabled. Never reach for the web for a question the Oracle can answer.

# Never write files directly
You are an orchestrator: you NEVER write or edit files yourself. To make any change, DELEGATE the write to `spawn_mini` with "write": true and the target files. Review its result before relying on it. Reads and navigation you do yourself; writes always go through `spawn_mini`.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_states_the_oracle_first_mandate() {
        let p = build_system_prompt();
        // The private/grounded oracle-first hierarchy must be present.
        assert!(p.contains("oracle_ask"), "names oracle_ask");
        assert!(p.contains("oracle_context"), "names oracle_context");
        assert!(
            p.contains("PRIVATE") && p.contains("zero-egress"),
            "states the private / zero-egress framing"
        );
        assert!(
            p.to_lowercase().contains("always use") || p.contains("PREFER THESE"),
            "states an oracle-first preference"
        );
    }

    #[test]
    fn prompt_states_the_egress_exception() {
        let p = build_system_prompt();
        assert!(p.contains("fetch") && p.contains("websearch"), "names the egress tools");
        assert!(
            p.contains("EGRESS EXCEPTION") || p.to_lowercase().contains("egress exception"),
            "frames web as a conscious egress exception"
        );
        assert!(p.contains("Exa"), "names the external provider");
        assert!(
            p.to_lowercase().contains("only when") || p.to_lowercase().contains("only if"),
            "constrains egress to a last resort"
        );
    }

    #[test]
    fn prompt_states_the_no_direct_write_mandate() {
        let p = build_system_prompt();
        assert!(
            p.contains("NEVER write") || p.contains("never write"),
            "states the no-direct-write rule"
        );
        assert!(
            p.contains("spawn_mini"),
            "directs writes to spawn_mini"
        );
        assert!(p.contains("DELEGATE") || p.contains("delegate"), "frames it as delegation");
    }

    #[test]
    fn prompt_states_the_one_action_block_rule() {
        let p = build_system_prompt();
        assert!(
            p.contains("EXACTLY ONE") && p.contains("action"),
            "states the one-action-block-per-turn rule"
        );
    }
}
