#!/usr/bin/env python3
"""Re-run 3-hop session-gate reachability for Tauri commands. Audit-only."""
from __future__ import annotations
from pathlib import Path
import re, json, sys

ROOT = Path(__file__).resolve().parents[3] / "src-tauri" / "src"
GATE = {
    "ensure_unlocked", "sensitive_session_id", "ensure_same_sensitive_session",
    "require_oracle_auth", "require_graph_auth", "require_graph_auth_and_enabled",
}
fn_def = re.compile(r"(?m)^(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*\(")
cmd_re = re.compile(r"#\[tauri::command[^\]]*\]\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)", re.M)
SKIP = {"format","vec","Ok","Err","Some","None","to_string","clone","into","from","map","and_then","unwrap","expect","print","eprintln","drop","Box","String","Path","PathBuf"}

def index_fns():
    fns = {}
    for p in ROOT.rglob("*.rs"):
        text = p.read_text(encoding="utf-8", errors="replace")
        ms = list(fn_def.finditer(text))
        for i, m in enumerate(ms):
            name = m.group(1)
            start = m.end()
            end = ms[i+1].start() if i+1 < len(ms) else min(len(text), start+15000)
            fns.setdefault(name, []).append(text[start:end])
    return fns

def cmd_body(name, fns_unused=None):
    for p in ROOT.rglob("*.rs"):
        text = p.read_text(encoding="utf-8", errors="replace")
        for m in cmd_re.finditer(text):
            if m.group(1) != name:
                continue
            start = m.end()
            nxt = re.search(r"\n(?:#\[|pub\s+(?:async\s+)?fn\s+|fn\s+\w+)", text[start:])
            end = start + (nxt.start() if nxt else min(8000, len(text)-start))
            return text[start:end]
    return None

def gates(body):
    return sorted(g for g in GATE if g+"(" in body)

def explore(name, fns, depth=3):
    body = cmd_body(name)
    if body is None:
        return {"error": "not found", "reaches_gate": False}
    visited, q, paths = set(), [(name, body, 0, [])], []
    while q:
        n, b, d, path = q.pop(0)
        g = gates(b)
        if g:
            paths.append({"path": path+[n], "gates": g, "depth": d})
            continue
        if d >= depth:
            continue
        for c in re.findall(r"\b([a-z_][a-zA-Z0-9_]*)\s*\(", b):
            if c in visited or c in SKIP or c == n:
                continue
            visited.add(c)
            if c in fns:
                q.append((c, fns[c][0], d+1, path+[n]))
    return {"direct_gates": gates(body), "gate_paths": paths[:10], "reaches_gate": bool(paths) or bool(gates(body))}

DEFAULT = [
    "list_pending_design_requests","design_request_claim","design_request_complete",
    "project_cloud_orchestrator_interrupt","project_cloud_orchestrator_send",
    "mini_activity_snapshot","mini_coder_steer","pi_extensions_list","pi_extension_install",
    "pi_extension_remove","skills_featured_marketplaces","skills_library_catalog",
    "skills_lang_catalog","orchestrator_steer","planner_reset_chat","polis_debug_log",
    "save_provider_token","rotate_cloudflare_worker_secret","perform_scaleway_resource_action",
    "launch_project_agent_terminal","ask_oracle","approve_git_push_request","mini_coder_kill",
]

def main():
    fns = index_fns()
    names = sys.argv[1:] or DEFAULT
    out = {n: explore(n, fns) for n in names}
    outp = Path(__file__).with_name("locked_invoke_callgraph.json")
    outp.write_text(json.dumps(out, indent=2))
    for n, r in out.items():
        print(f"{n:40} {'GATE' if r.get('reaches_gate') else 'NO_GATE'}")
    print("wrote", outp)

if __name__ == "__main__":
    main()
