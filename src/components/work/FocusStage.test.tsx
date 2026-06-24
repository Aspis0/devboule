// @vitest-environment jsdom
//
// FocusStage is the right column: header (file/district/agent/status) + [Activity|Raw] toggle,
// the structured Activity stream (reusing AgentConsole), a Raw slot (the PTY terminal injected by
// the parent), a two-way composer (Direction A: message the agent) with redo/narrow/pause quick
// actions, and an inline amber question card (Direction B) whose answer routes via onAnswer.
// Presentational: all IO/command-routing is the parent's job (onSendMessage / onAnswer).

import { describe, it, expect, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FocusStage } from "./FocusStage";
import type { WorkNode } from "./workConsoleModel";
import { STATE_RUNNING } from "../agents/__fixtures__/agentConsoleStates";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const node: WorkNode = {
  agentId: "c1", type: "coder", file: "src/auth/login.ts", district: "auth",
  status: "round 3", label: "coder · codex", parentAgentId: null, taskId: "t1",
  pendingQuestion: null, live: true, children: [],
};

type Props = Parameters<typeof FocusStage>[0];
const baseProps = (over: Partial<Props> = {}): Props => ({
  node,
  activity: STATE_RUNNING,
  view: "activity",
  onViewChange: () => {},
  onSendMessage: () => {},
  pendingQuestion: null,
  onAnswer: () => {},
  rawSlot: createElement("div", null, "RAWTERM"),
  ...over,
});

const html = (props: Props) => renderToStaticMarkup(createElement(FocusStage, props));

let root: Root | null = null;
let container: HTMLDivElement | null = null;
function mount(props: Props) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(createElement(FocusStage, props)));
  return container;
}
function setText(ta: HTMLTextAreaElement, v: string) {
  const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value")!.set!;
  setter.call(ta, v);
  ta.dispatchEvent(new Event("input", { bubbles: true }));
}
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  if (container) container.remove();
  container = null;
});

describe("FocusStage", () => {
  it("renders the header with file, district and status", () => {
    const out = html(baseProps());
    expect(out).toContain("login.ts");
    expect(out).toContain("auth");
    expect(out).toContain("round 3");
  });

  it("renders the Activity stream (AgentConsole) when view is activity", () => {
    const out = html(baseProps({ view: "activity" }));
    expect(out).toContain('data-view="activity"');
    expect(out).toContain("harden auth flow");
    expect(out).not.toContain("RAWTERM");
  });

  it("renders the Raw slot when view is raw, not the Activity stream", () => {
    const out = html(baseProps({ view: "raw" }));
    expect(out).toContain('data-view="raw"');
    expect(out).toContain("RAWTERM");
    expect(out).not.toContain("harden auth flow");
  });

  it("calls onViewChange('raw') when the Raw tab is clicked", () => {
    let v: string | null = null;
    const c = mount(baseProps({ onViewChange: (x) => { v = x; } }));
    const rawTab = c.querySelector('[data-tab="raw"]') as HTMLElement;
    expect(rawTab).toBeTruthy();
    act(() => rawTab.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(v).toBe("raw");
  });

  it("sends a message through onSendMessage (Direction A)", () => {
    const sent: string[] = [];
    const c = mount(baseProps({ onSendMessage: (t) => sent.push(t) }));
    const ta = c.querySelector("textarea") as HTMLTextAreaElement;
    act(() => setText(ta, "narrow to the parser"));
    const send = c.querySelector('[data-action="send"]') as HTMLElement;
    act(() => send.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(sent).toEqual(["narrow to the parser"]);
  });

  it("fires onQuickAction for redo/narrow/pause", () => {
    const acts: string[] = [];
    const c = mount(baseProps({ onQuickAction: (a) => acts.push(a) }));
    (c.querySelector('[data-action="redo"]') as HTMLElement)
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(acts).toContain("redo");
  });

  it("does not fire quick actions when disabled", () => {
    const acts: string[] = [];
    const c = mount(baseProps({ disabled: true, onQuickAction: (a) => acts.push(a) }));
    (c.querySelector('[data-action="redo"]') as HTMLElement)
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(acts).toEqual([]);
  });

  it("shows the question card and routes the answer via onAnswer (Direction B)", () => {
    const answered: string[] = [];
    const sent: string[] = [];
    const c = mount(baseProps({
      pendingQuestion: "which auth provider should I wire?",
      onAnswer: (t) => answered.push(t),
      onSendMessage: (t) => sent.push(t),
    }));
    expect(c.innerHTML).toContain("which auth provider should I wire?");
    expect(c.querySelector('[data-asking="true"]')).toBeTruthy();
    const ta = c.querySelector("textarea") as HTMLTextAreaElement;
    act(() => setText(ta, "use Auth0"));
    const send = c.querySelector('[data-action="send"]') as HTMLElement;
    act(() => send.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    // while a question is pending, the composer answers — it must NOT go to onSendMessage
    expect(answered).toEqual(["use Auth0"]);
    expect(sent).toEqual([]);
  });
});
