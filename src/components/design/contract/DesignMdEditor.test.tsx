// @vitest-environment jsdom
//
// DesignMdEditor: preset pick replaces content + tokens, Save calls onSave with the
// content + the pending tokens, Skip calls onSkip and NEVER onSave, the byte counter
// + over-cap Save disable. The hard invariant under test: nothing leaves the editor
// except via Save.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { DesignMdEditor, type DesignMdEditorProps } from "./DesignMdEditor";
import { PRESET_CATALOG } from "./presets";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function render(over: Partial<DesignMdEditorProps> = {}): {
  container: HTMLElement;
  onSave: ReturnType<typeof vi.fn>;
  onSkip: ReturnType<typeof vi.fn>;
} {
  const onSave = vi.fn();
  const onSkip = vi.fn();
  const props: DesignMdEditorProps = {
    open: true,
    initialContent: "# Draft\nhello",
    draftTokens: undefined,
    onSave,
    onSkip,
    ...over,
  };
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => createRoot(container).render(createElement(DesignMdEditor, props)));
  return { container, onSave, onSkip };
}

function textarea(c: HTMLElement) {
  return c.querySelector(".dc-textarea") as HTMLTextAreaElement;
}
function clickText(c: HTMLElement, text: string) {
  const btn = Array.from(c.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === text,
  ) as HTMLButtonElement;
  act(() => btn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

describe("DesignMdEditor", () => {
  it("renders nothing when closed", () => {
    const { container } = render({ open: false });
    expect(container.querySelector("[data-testid=design-md-editor]")).toBeNull();
  });

  it("shows the initial content and a byte counter", () => {
    const { container } = render({ initialContent: "abc" });
    expect(textarea(container).value).toBe("abc");
    expect(
      container.querySelector("[data-testid=dc-counter]")?.textContent,
    ).toContain("3");
  });

  it("Save passes the current content and the draft tokens", () => {
    const tokens = { color: { brand: { $value: "#abc", $type: "color" } } };
    const { container, onSave } = render({
      initialContent: "# X",
      draftTokens: tokens,
    });
    clickText(container, "Save contract");
    expect(onSave).toHaveBeenCalledWith("# X", tokens);
  });

  it("picking a preset REPLACES the content and the tokens written on Save", () => {
    const { container, onSave } = render({ initialContent: "" });
    const preset = PRESET_CATALOG[1]; // material-ish
    const card = container.querySelector(
      `.dc-preset[data-preset="${preset.id}"]`,
    ) as HTMLButtonElement;
    act(() => card.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(textarea(container).value).toBe(preset.designMd);
    clickText(container, "Save contract");
    expect(onSave).toHaveBeenCalledWith(preset.designMd, preset.tokens);
  });

  it("Skip calls onSkip and NEVER onSave", () => {
    const { container, onSave, onSkip } = render();
    clickText(container, "Skip");
    expect(onSkip).toHaveBeenCalledTimes(1);
    expect(onSave).not.toHaveBeenCalled();
  });

  it("keeps the picked preset's tokens when the parent re-renders with a NEW draftTokens reference (Fix 7)", () => {
    const onSave = vi.fn();
    const onSkip = vi.fn();
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const tokensA = { color: { a: { $value: "#a", $type: "color" } } };
    const renderWith = (draftTokens: unknown) =>
      act(() =>
        root.render(
          createElement(DesignMdEditor, {
            open: true,
            initialContent: "# same",
            draftTokens: draftTokens as DesignMdEditorProps["draftTokens"],
            onSave,
            onSkip,
          }),
        ),
      );
    renderWith(tokensA);
    // User picks a preset INSIDE the editor (replaces content + pendingTokens).
    const preset = PRESET_CATALOG[1];
    const card = container.querySelector(
      `.dc-preset[data-preset="${preset.id}"]`,
    ) as HTMLButtonElement;
    act(() => card.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    // Parent re-renders with a brand-new draftTokens REFERENCE (same modal/content).
    const tokensB = { color: { b: { $value: "#b", $type: "color" } } };
    renderWith(tokensB);
    // Save must still carry the PRESET's tokens, not tokensB nor tokensA.
    clickText(container, "Save contract");
    expect(onSave).toHaveBeenCalledWith(preset.designMd, preset.tokens);
  });

  it("disables Save and flags the counter when over the 64 KiB cap", () => {
    const { container, onSave } = render({
      initialContent: "x".repeat(70 * 1024),
    });
    const save = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Save contract"),
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(
      container.querySelector("[data-testid=dc-counter]")?.getAttribute("data-over"),
    ).toBe("true");
    // Clicking the disabled Save does nothing.
    act(() => save.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onSave).not.toHaveBeenCalled();
  });
});
