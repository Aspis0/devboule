// @vitest-environment jsdom
//
// Panel-slice tests (Phase A2 final pass): the prototype's Assistant panel wired to the
// real generation pipeline. These cover the panel's OWN behavior (suggestions, message
// lifecycle, model popover persistence, the resizer clamp, the B4 CLI note) at the
// component level — the DesignView↔pipeline parity is covered by
// DesignView.generation.test.tsx. Rendered with the raw react-dom harness the other
// design tests use (no testing-library in this repo).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { AssistantPanel } from "./AssistantPanel";
import { ModelPopover } from "./ModelPopover";
import type { AssistantMessage } from "./types";
import type { DesignLlmBackend } from "../../../types/config";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

function mount(node: React.ReactElement) {
  act(() => {
    root = createRoot(container);
    root.render(node);
  });
  return () => act(() => root.unmount());
}

function click(el: Element) {
  act(() => el.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

const noop = () => {};

function panelProps(overrides: Partial<Parameters<typeof AssistantPanel>[0]> = {}) {
  return {
    width: 350,
    messages: [] as AssistantMessage[],
    doneCount: 0,
    selectedNodeName: null,
    onClearContext: noop,
    onSend: noop,
    onSuggest: noop,
    onRerun: noop,
    onLocate: noop,
    onStop: noop,
    busy: false,
    backend: null as DesignLlmBackend | null,
    onSaveBackend: noop,
    onOpenSettings: noop,
    draft: "",
    setDraft: noop,
    focusSignal: 0,
    onVisualCheck: noop,
    visualCheckDisabled: false,
    visualChecking: false,
    ...overrides,
  };
}

describe("AssistantPanel — empty state + suggestions", () => {
  it("renders the three suggestions and SEEDS the draft (does not send) on click", () => {
    const onSuggest = vi.fn();
    const onSend = vi.fn();
    mount(createElement(AssistantPanel, panelProps({ onSuggest, onSend })));

    const suggs = container.querySelectorAll(".sugg");
    expect(suggs.length).toBe(3);
    click(suggs[0]);
    // onSuggest (DesignView wires it to setDraft) seeds the composer; nothing is sent.
    expect(onSuggest).toHaveBeenCalledWith(
      "A pricing section coherent with our app",
    );
    expect(onSend).not.toHaveBeenCalled();
  });

  it('shows "<N> generations" only once an assistant card exists', () => {
    // No assistant rows -> empty subtitle.
    const unmount = mount(createElement(AssistantPanel, panelProps()));
    expect(container.querySelector(".assist-head .sub")?.textContent).toBe("");
    unmount();

    const messages: AssistantMessage[] = [
      { id: 1, role: "assistant", status: "done", title: "Added 1 node" },
      { id: 2, role: "assistant", status: "working", title: "Generating…" },
    ];
    mount(createElement(AssistantPanel, panelProps({ messages, doneCount: 1 })));
    expect(container.querySelector(".assist-head .sub")?.textContent).toBe(
      "1 generations",
    );
  });
});

describe("AssistantPanel — message lifecycle", () => {
  it("working card shows the spinner + a Stop button wired to cancel", () => {
    const onStop = vi.fn();
    const messages: AssistantMessage[] = [
      { id: 1, role: "user", text: "make a hero" },
      { id: 2, role: "assistant", status: "working", title: "Generating…", desc: "…" },
    ];
    mount(createElement(AssistantPanel, panelProps({ messages, onStop, busy: true })));

    expect(container.querySelector(".msg-ai .head .ic.spin")).toBeTruthy();
    const stop = Array.from(container.querySelectorAll(".foot .mini-btn")).find(
      (b) => b.textContent?.includes("Stop"),
    )!;
    click(stop);
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("done card renders src-chips + Select-on-canvas (locate) + Regenerate (rerun)", () => {
    const onLocate = vi.fn();
    const onRerun = vi.fn();
    const done: AssistantMessage = {
      id: 2,
      role: "assistant",
      status: "done",
      title: "Added 1 node",
      desc: "1 node",
      sources: ["src/Pricing.tsx", "src/tokens.css"],
      nodeIds: ["gen-1"],
      instruction: "a pricing section",
    };
    mount(createElement(AssistantPanel, panelProps({ messages: [done], onLocate, onRerun })));

    const chips = container.querySelectorAll(".src-chip");
    expect(chips.length).toBe(2);
    expect(chips[0].textContent).toContain("src/Pricing.tsx");

    const locate = Array.from(container.querySelectorAll(".foot .mini-btn")).find(
      (b) => b.textContent?.includes("Select on canvas"),
    )!;
    click(locate);
    expect(onLocate).toHaveBeenCalledWith(["gen-1"]);

    const regen = Array.from(container.querySelectorAll(".foot .mini-btn")).find(
      (b) => b.textContent?.includes("Regenerate"),
    )!;
    click(regen);
    expect(onRerun).toHaveBeenCalledWith(done);
  });

  it("error card renders the err class + a Retry that re-runs", () => {
    const onRerun = vi.fn();
    const err: AssistantMessage = {
      id: 2,
      role: "assistant",
      status: "error",
      title: "Generation failed",
      desc: "boom",
      instruction: "a footer",
    };
    mount(createElement(AssistantPanel, panelProps({ messages: [err], onRerun })));

    expect(container.querySelector(".msg-ai.err")).toBeTruthy();
    expect(container.querySelector(".msg-ai .head .ic.alert")).toBeTruthy();
    const retry = Array.from(container.querySelectorAll(".foot .mini-btn")).find(
      (b) => b.textContent?.includes("Retry"),
    )!;
    click(retry);
    expect(onRerun).toHaveBeenCalledWith(err);
  });

  it("B4: a CLI/agentic run shows the MCP note and NO fetched src-chips", () => {
    const agentic: AssistantMessage = {
      id: 2,
      role: "assistant",
      status: "done",
      title: "Added 1 node",
      agentic: true,
      // Even if sources were set, agentic must win and render only the MCP note.
      sources: ["src/should-not-show.tsx"],
      nodeIds: ["gen-1"],
      instruction: "x",
    };
    mount(createElement(AssistantPanel, panelProps({ messages: [agentic] })));

    const chips = container.querySelectorAll(".src-chip");
    expect(chips.length).toBe(1);
    expect(chips[0].textContent).toContain("grounds agentically via MCP");
    expect(container.textContent).not.toContain("should-not-show");
  });

  it("a user row with an edit ctx renders the ctx-chip", () => {
    const messages: AssistantMessage[] = [
      { id: 1, role: "user", text: "make it blue", ctx: "Editing cta" },
    ];
    mount(createElement(AssistantPanel, panelProps({ messages })));
    expect(container.querySelector(".msg-user .ctx-chip")?.textContent).toContain(
      "Editing cta",
    );
    expect(container.querySelector(".msg-user .bubble")?.textContent).toBe(
      "make it blue",
    );
  });
});

describe("Composer (via AssistantPanel) — send + context", () => {
  it("Enter sends the trimmed draft; Shift+Enter does not", () => {
    const onSend = vi.fn();
    mount(
      createElement(
        AssistantPanel,
        panelProps({ draft: "  a pricing section  ", onSend }),
      ),
    );
    const ta = container.querySelector("textarea") as HTMLTextAreaElement;
    act(() =>
      ta.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      ),
    );
    expect(onSend).toHaveBeenCalledWith("a pricing section");

    onSend.mockClear();
    act(() =>
      ta.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true }),
      ),
    );
    expect(onSend).not.toHaveBeenCalled();
  });

  it("the context chip clears the edit selection", () => {
    const onClearContext = vi.fn();
    mount(
      createElement(
        AssistantPanel,
        panelProps({ selectedNodeName: "cta", onClearContext }),
      ),
    );
    expect(container.querySelector(".composer-ctx")?.textContent).toContain(
      "Editing cta",
    );
    click(container.querySelector(".composer-ctx .x")!);
    expect(onClearContext).toHaveBeenCalledTimes(1);
  });

  it("the send button is disabled while busy and the icon spins", () => {
    mount(
      createElement(AssistantPanel, panelProps({ busy: true, draft: "hi" })),
    );
    const send = container.querySelector(".send-btn") as HTMLButtonElement;
    expect(send.disabled).toBe(true);
  });

  it("the attachment button is present but disabled (deferred)", () => {
    mount(createElement(AssistantPanel, panelProps()));
    const clip = container.querySelector(".composer-bar .icon-btn") as HTMLButtonElement;
    expect(clip.disabled).toBe(true);
    expect(clip.getAttribute("title")).toContain("coming soon");
  });
});

describe("ModelPopover — persistence + invalid gating", () => {
  function popProps(overrides: Partial<Parameters<typeof ModelPopover>[0]> = {}) {
    return {
      open: true,
      onClose: noop,
      backend: { kind: "ollama", model: "qwen2.5-coder" } as DesignLlmBackend,
      onSave: noop,
      onOpenSettings: noop,
      ...overrides,
    };
  }

  it("picking effort persists the lowercase value via onSave", () => {
    const onSave = vi.fn();
    mount(createElement(ModelPopover, popProps({ onSave })));
    const low = Array.from(container.querySelectorAll(".seg button")).find(
      (b) => b.textContent === "Low",
    )!;
    click(low);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave.mock.calls[0][0]).toMatchObject({
      kind: "ollama",
      model: "qwen2.5-coder",
      effort: "low",
    });
  });

  it("switching to a kind the config can satisfy persists a valid backend", () => {
    const onSave = vi.fn();
    // codex needs no fields, so switching from ollama to codex is always valid.
    mount(createElement(ModelPopover, popProps({ onSave })));
    const codexRow = Array.from(container.querySelectorAll(".mp-row")).find((r) =>
      r.textContent?.includes("Codex"),
    )!;
    click(codexRow);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave.mock.calls[0][0]).toMatchObject({ kind: "codex" });
  });

  it("switching to a kind that needs a missing field does NOT save (links to Settings)", () => {
    const onSave = vi.fn();
    // From codex (no fields) to omlx (needs model + baseUrl) — the config can't supply
    // them, so no save happens and the inline Settings note appears.
    mount(
      createElement(
        ModelPopover,
        popProps({ backend: { kind: "codex" }, onSave }),
      ),
    );
    const omlxRow = Array.from(container.querySelectorAll(".mp-row")).find((r) =>
      r.textContent?.includes("oMLX"),
    )!;
    click(omlxRow);
    expect(onSave).not.toHaveBeenCalled();
  });

  it("the timeout slider persists only on release (change-end), not on every input", () => {
    const onSave = vi.fn();
    mount(createElement(ModelPopover, popProps({ onSave })));
    const slider = container.querySelector(
      ".mp-slider input[type=range]",
    ) as HTMLInputElement;

    // A bare input event (drag tick) must NOT persist.
    act(() => {
      slider.value = "300";
      slider.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(onSave).not.toHaveBeenCalled();

    // Releasing (pointerup) commits the value.
    act(() => {
      slider.value = "300";
      slider.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
    });
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave.mock.calls[0][0]).toMatchObject({ timeoutSecs: 300 });
  });

  it("the Settings link closes the popover and navigates", () => {
    const onClose = vi.fn();
    const onOpenSettings = vi.fn();
    mount(createElement(ModelPopover, popProps({ onClose, onOpenSettings })));
    click(container.querySelector(".mp-settings")!);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("shows the configure-in-Settings note when no valid backend is set", () => {
    mount(createElement(ModelPopover, popProps({ backend: null })));
    // Multiple `.mp-note` elements exist (the always-on "global setting" hint + the
    // needs-config hint); assert one of them carries the configure prompt.
    const notes = Array.from(container.querySelectorAll(".mp-note")).map(
      (n) => n.textContent ?? "",
    );
    expect(notes.some((t) => t.includes("Configure model/URL in Settings"))).toBe(
      true,
    );
  });
});
