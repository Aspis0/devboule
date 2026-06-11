// @vitest-environment jsdom
//
// Tests for the CE DOM helpers: clean serialization (strips helper classes/attrs),
// elHasText, and the inline-text-edit lifecycle (Enter commits, Esc reverts, blur
// commits). jsdom provides the DOM + Selection/Range the helpers use.

import { describe, it, expect, afterEach, vi } from "vitest";
import { cleanSerialize, elHasText, startInlineTextEdit } from "./contentEdit";

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("cleanSerialize", () => {
  it("strips ce-hover/ce-sel helper classes and contenteditable/spellcheck attrs", () => {
    const root = document.createElement("div");
    root.innerHTML =
      '<section class="ce-sel keep" contenteditable="true" spellcheck="false">' +
      '<h1 class="ce-hover">Title</h1><p>Body</p></section>';
    const out = cleanSerialize(root);
    expect(out).not.toContain("ce-sel");
    expect(out).not.toContain("ce-hover");
    expect(out).not.toContain("contenteditable");
    expect(out).not.toContain("spellcheck");
    // A genuine author class survives (the helper tokens are removed, not the attr).
    expect(out).toContain("keep");
    expect(out).toContain("Title");
    expect(out).toContain("Body");
  });

  it("drops the class attribute entirely when only helper classes remained", () => {
    const root = document.createElement("div");
    root.innerHTML = '<p class="ce-hover">x</p>';
    const out = cleanSerialize(root);
    expect(out).not.toContain("class");
    expect(out).toBe("<p>x</p>");
  });

  it("does NOT mutate the live container (operates on a clone)", () => {
    const root = document.createElement("div");
    root.innerHTML = '<p class="ce-sel" contenteditable="true">x</p>';
    cleanSerialize(root);
    // The live DOM still carries the helper class + attr.
    const p = root.querySelector("p")!;
    expect(p.classList.contains("ce-sel")).toBe(true);
    expect(p.getAttribute("contenteditable")).toBe("true");
  });
});

describe("elHasText", () => {
  it("is true for an element with a direct non-empty text child", () => {
    const el = document.createElement("h1");
    el.textContent = "Hello";
    expect(elHasText(el)).toBe(true);
  });

  it("is false for a pure container (only element children)", () => {
    const el = document.createElement("div");
    el.innerHTML = "<span>x</span>";
    expect(elHasText(el)).toBe(false);
  });

  it("is false for whitespace-only text and null", () => {
    const el = document.createElement("p");
    el.textContent = "   \n ";
    expect(elHasText(el)).toBe(false);
    expect(elHasText(null)).toBe(false);
  });
});

describe("startInlineTextEdit lifecycle", () => {
  function mountEl(html = "<p>original</p>"): HTMLElement {
    const host = document.createElement("div");
    host.innerHTML = html;
    document.body.appendChild(host);
    return host.firstElementChild as HTMLElement;
  }

  it("makes the element contenteditable and selects its content", () => {
    const el = mountEl();
    startInlineTextEdit(el);
    expect(el.getAttribute("contenteditable")).toBe("true");
    expect(el.getAttribute("spellcheck")).toBe("false");
  });

  it("Enter commits the typed text and exits (onDone fires once)", () => {
    const el = mountEl();
    const onDone = vi.fn();
    startInlineTextEdit(el, onDone);
    el.textContent = "edited";
    el.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    expect(el.textContent).toBe("edited"); // committed
    expect(el.getAttribute("contenteditable")).toBeNull();
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("Esc reverts to the original text and exits", () => {
    const el = mountEl("<p>original</p>");
    const onDone = vi.fn();
    startInlineTextEdit(el, onDone);
    el.textContent = "garbage";
    el.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(el.textContent).toBe("original"); // reverted
    expect(el.getAttribute("contenteditable")).toBeNull();
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("blur commits the typed text", () => {
    const el = mountEl();
    const onDone = vi.fn();
    startInlineTextEdit(el, onDone);
    el.textContent = "typed";
    el.dispatchEvent(new FocusEvent("blur"));
    expect(el.textContent).toBe("typed");
    expect(el.getAttribute("contenteditable")).toBeNull();
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("paste inserts ONLY the plain-text payload — rich HTML (img onerror) is dropped", () => {
    const el = mountEl("<p></p>");
    startInlineTextEdit(el);
    // Place the caret inside the editable element.
    const range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    const sel = window.getSelection()!;
    sel.removeAllRanges();
    sel.addRange(range);

    // A clipboard carrying a malicious HTML payload + a benign text/plain fallback.
    // jsdom has no DataTransfer, so build a minimal clipboardData stub the handler
    // reads via `getData("text/plain")`, and attach it to a cancelable paste event.
    const clipboardData = {
      getData: (type: string) =>
        type === "text/html"
          ? '<img src=x onerror="window.__pwned=1">'
          : type === "text/plain"
            ? "hello"
            : "",
    };
    const evt = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(evt, "clipboardData", { value: clipboardData });
    el.dispatchEvent(evt);

    // Only the plain text landed — no <img> element, no executed handler.
    expect(el.querySelector("img")).toBeNull();
    expect(el.innerHTML).not.toContain("img");
    expect(el.innerHTML).not.toContain("onerror");
    expect(el.textContent).toContain("hello");
    expect(evt.defaultPrevented).toBe(true);
  });

  it("paste listener is torn down on edit end (no paste handling after exit)", () => {
    const el = mountEl("<p>x</p>");
    const cancel = startInlineTextEdit(el);
    cancel();
    const evt = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(evt, "clipboardData", {
      value: { getData: () => "late" },
    });
    el.dispatchEvent(evt);
    // The handler is gone, so it did NOT preventDefault the late paste.
    expect(evt.defaultPrevented).toBe(false);
  });

  it("cancel() force-ends a live edit (onDone fires once, attrs removed)", () => {
    const el = mountEl();
    const onDone = vi.fn();
    const cancel = startInlineTextEdit(el, onDone);
    cancel();
    expect(el.getAttribute("contenteditable")).toBeNull();
    expect(onDone).toHaveBeenCalledTimes(1);
    // A second cancel / a later blur is a no-op (onDone still once).
    cancel();
    el.dispatchEvent(new FocusEvent("blur"));
    expect(onDone).toHaveBeenCalledTimes(1);
  });
});
