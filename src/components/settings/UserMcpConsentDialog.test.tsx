// @vitest-environment jsdom
//
// Tests for UserMcpConsentDialog.
//
// Covers:
//   - Add button disabled until consent ticked + name + command filled.
//   - Ticking consent + filling fields enables Add.
//   - Cancel button calls no backend command.
//   - Successful add calls user_mcp_add and then onAdded, NOT onCancel.
//   - Backend validation error (e.g. reserved name) is shown inline.
//   - Env values are NOT shown in the review block (only keys).
//   - Add disabled while busy (prevents double-submit).

import { describe, expect, it, vi, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async (..._args: unknown[]) => undefined);

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { UserMcpConsentDialog } from "./UserMcpConsentDialog";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Set a text input's value in a way React's synthetic event system recognises.
// Works for both HTMLInputElement and HTMLTextAreaElement.
function setInputValue(
  el: HTMLInputElement | HTMLTextAreaElement,
  value: string,
): void {
  // Use the native setter from whichever prototype the element actually lives on.
  const proto =
    el.tagName === "TEXTAREA"
      ? Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")
      : Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
  proto!.set!.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

async function mountDialog(
  onAdded: () => void,
  onCancel: () => void,
  scope: "global" | "project" = "global",
  projectRoot?: string,
): Promise<{ container: HTMLDivElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      createElement(UserMcpConsentDialog, {
        scope,
        projectRoot,
        onAdded,
        onCancel,
      }),
    );
  });
  return { container, root };
}

function addButton(container: HTMLElement): HTMLButtonElement {
  return container.querySelector("[data-testid='mcp-add-btn']") as HTMLButtonElement;
}

function consentCheckbox(container: HTMLElement): HTMLInputElement {
  return container.querySelector("[data-testid='mcp-consent-ack']") as HTMLInputElement;
}

function nameInput(container: HTMLElement): HTMLInputElement {
  return container.querySelector("input[placeholder='my-db']") as HTMLInputElement;
}

function commandInput(container: HTMLElement): HTMLInputElement {
  return container.querySelector("input[placeholder='python']") as HTMLInputElement;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("UserMcpConsentDialog — Add button gating", () => {
  let container: HTMLDivElement;
  let root: Root;

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    invokeMock.mockClear();
  });

  it("Add is disabled when nothing is filled", async () => {
    ({ container, root } = await mountDialog(vi.fn(), vi.fn()));
    expect(addButton(container).disabled).toBe(true);
  });

  it("Add remains disabled when consent ticked but name+command empty", async () => {
    ({ container, root } = await mountDialog(vi.fn(), vi.fn()));
    await act(async () => {
      consentCheckbox(container).click();
    });
    expect(addButton(container).disabled).toBe(true);
  });

  it("Add remains disabled when name+command filled but consent not ticked", async () => {
    ({ container, root } = await mountDialog(vi.fn(), vi.fn()));
    await act(async () => {
      setInputValue(nameInput(container), "my-db");
      setInputValue(commandInput(container), "python");
    });
    expect(addButton(container).disabled).toBe(true);
  });

  it("Add enables when consent ticked AND name+command filled", async () => {
    ({ container, root } = await mountDialog(vi.fn(), vi.fn()));
    await act(async () => {
      setInputValue(nameInput(container), "my-db");
      setInputValue(commandInput(container), "python");
      consentCheckbox(container).click();
    });
    expect(addButton(container).disabled).toBe(false);
  });
});

describe("UserMcpConsentDialog — Cancel", () => {
  it("Cancel button calls onCancel and NO backend command", async () => {
    const onAdded = vi.fn();
    const onCancel = vi.fn();
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded,
          onCancel,
        }),
      );
    });

    const cancelBtns = Array.from(container.querySelectorAll("button")).filter(
      (b) => b.textContent?.trim() === "Cancel",
    );
    expect(cancelBtns.length).toBeGreaterThan(0);
    await act(async () => {
      cancelBtns[0].click();
    });

    expect(onCancel).toHaveBeenCalledOnce();
    expect(onAdded).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalled();

    act(() => root.unmount());
    container.remove();
    invokeMock.mockClear();
  });
});

describe("UserMcpConsentDialog — Successful add", () => {
  afterEach(() => {
    invokeMock.mockClear();
  });

  it("calls user_mcp_add with the correct args and then onAdded", async () => {
    const onAdded = vi.fn();
    const onCancel = vi.fn();
    invokeMock.mockResolvedValueOnce(undefined);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded,
          onCancel,
        }),
      );
    });

    // Fill name.
    await act(async () => {
      setInputValue(nameInput(container), "my-db");
    });
    // Fill command.
    await act(async () => {
      setInputValue(commandInput(container), "python");
    });
    // Fill args textarea (first textarea element).
    await act(async () => {
      const textareas = container.querySelectorAll("textarea");
      setInputValue(textareas[0] as HTMLTextAreaElement, "-m\nmydb_mcp");
    });
    // Tick consent.
    await act(async () => {
      consentCheckbox(container).click();
    });

    expect(addButton(container).disabled).toBe(false);

    await act(async () => {
      addButton(container).dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_add");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({
      scope: "global",
      server: {
        name: "my-db",
        command: "python",
        args: ["-m", "mydb_mcp"],
        transport: "stdio",
        enabled: true,
      },
    });
    expect(onAdded).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();

    act(() => root.unmount());
    container.remove();
  });

  it("passes projectRoot when scope is project", async () => {
    const onAdded = vi.fn();
    invokeMock.mockResolvedValueOnce(undefined);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "project",
          projectRoot: "/home/user/project",
          onAdded,
          onCancel: vi.fn(),
        }),
      );
    });

    await act(async () => {
      setInputValue(nameInput(container), "proj-tool");
    });
    await act(async () => {
      setInputValue(commandInput(container), "node");
    });
    await act(async () => {
      consentCheckbox(container).click();
    });

    await act(async () => {
      addButton(container).dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_add");
    expect(call![1]).toMatchObject({
      scope: "project",
      projectRoot: "/home/user/project",
    });

    act(() => root.unmount());
    container.remove();
  });
});

describe("UserMcpConsentDialog — Backend validation error", () => {
  afterEach(() => {
    invokeMock.mockClear();
  });

  it("shows backend error inline when user_mcp_add rejects (e.g. reserved name)", async () => {
    const onAdded = vi.fn();
    invokeMock.mockRejectedValueOnce(
      new Error('Server name "oracle" is reserved and cannot be used.'),
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded,
          onCancel: vi.fn(),
        }),
      );
    });

    await act(async () => {
      setInputValue(nameInput(container), "oracle");
    });
    await act(async () => {
      setInputValue(commandInput(container), "python");
    });
    await act(async () => {
      consentCheckbox(container).click();
    });

    await act(async () => {
      addButton(container).dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // Two extra flushes: one for the async invoke, one for the state update.
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      await Promise.resolve();
    });

    // The error message must appear somewhere in the rendered UI.
    const html = container.innerHTML;
    expect(html).toContain("reserved");
    expect(onAdded).not.toHaveBeenCalled();

    act(() => root.unmount());
    container.remove();
  });
});

describe("UserMcpConsentDialog — Double-submit guard (F2)", () => {
  afterEach(() => {
    invokeMock.mockClear();
  });

  it("rapid double-click fires user_mcp_add only once", async () => {
    const onAdded = vi.fn();
    // Resolve only on the first call so a second invoke would return undefined too,
    // but we want to assert it is never called.
    let resolveFirst: () => void;
    const firstCall = new Promise<void>((res) => { resolveFirst = res; });
    invokeMock.mockImplementationOnce(async () => {
      await firstCall;
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded,
          onCancel: vi.fn(),
        }),
      );
    });

    await act(async () => { setInputValue(nameInput(container), "my-db"); });
    await act(async () => { setInputValue(commandInput(container), "python"); });
    await act(async () => { consentCheckbox(container).click(); });

    // Simulate two rapid clicks without awaiting the first async operation.
    const btn = addButton(container);
    await act(async () => {
      btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // Let the invoke resolve.
    resolveFirst!();
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    const addCalls = invokeMock.mock.calls.filter((c) => c[0] === "user_mcp_add");
    expect(addCalls.length).toBe(1);

    act(() => root.unmount());
    container.remove();
  });
});

describe("UserMcpConsentDialog — parseArgs comma+newline (F4)", () => {
  afterEach(() => {
    invokeMock.mockClear();
  });

  it("mixed newline+comma input yields correct flat args array", async () => {
    const onAdded = vi.fn();
    invokeMock.mockResolvedValueOnce(undefined);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded,
          onCancel: vi.fn(),
        }),
      );
    });

    await act(async () => { setInputValue(nameInput(container), "my-db"); });
    await act(async () => { setInputValue(commandInput(container), "python"); });
    // Mixed newline-and-comma input: should yield ["-m", "mydb", "--debug"]
    await act(async () => {
      const textareas = container.querySelectorAll("textarea");
      setInputValue(textareas[0] as HTMLTextAreaElement, "-m\nmydb,--debug");
    });
    await act(async () => { consentCheckbox(container).click(); });

    await act(async () => {
      addButton(container).dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => { await Promise.resolve(); });

    const call = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_add");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({
      server: { args: ["-m", "mydb", "--debug"] },
    });

    act(() => root.unmount());
    container.remove();
  });
});

describe("UserMcpConsentDialog — CRLF env value stripping (F3)", () => {
  afterEach(() => {
    invokeMock.mockClear();
  });

  it("a Windows CRLF-pasted env value has \\r stripped before sending", async () => {
    const onAdded = vi.fn();
    invokeMock.mockResolvedValueOnce(undefined);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded,
          onCancel: vi.fn(),
        }),
      );
    });

    await act(async () => { setInputValue(nameInput(container), "my-db"); });
    await act(async () => { setInputValue(commandInput(container), "python"); });
    // Simulate Windows paste: KEY=value\r\n
    await act(async () => {
      const textareas = container.querySelectorAll("textarea");
      setInputValue(textareas[1] as HTMLTextAreaElement, "MY_TOKEN=secret\r\n");
    });
    await act(async () => { consentCheckbox(container).click(); });

    await act(async () => {
      addButton(container).dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => { await Promise.resolve(); });

    const call = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_add");
    expect(call).toBeTruthy();
    const env = (call![1] as { server: { env: Record<string, string> } }).server.env;
    // The value must NOT contain \r
    expect(env["MY_TOKEN"]).toBe("secret");
    expect(env["MY_TOKEN"]).not.toContain("\r");

    act(() => root.unmount());
    container.remove();
  });
});

describe("UserMcpConsentDialog — Env value redaction", () => {
  it("shows env KEYS in the review block but NOT their values", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(UserMcpConsentDialog, {
          scope: "global",
          onAdded: vi.fn(),
          onCancel: vi.fn(),
        }),
      );
    });

    // Fill name and command so the review block appears.
    await act(async () => {
      setInputValue(nameInput(container), "secret-tool");
    });
    await act(async () => {
      setInputValue(commandInput(container), "/usr/bin/tool");
    });
    // Set env with a secret value (second textarea).
    await act(async () => {
      const textareas = container.querySelectorAll("textarea");
      setInputValue(textareas[1] as HTMLTextAreaElement, "MY_TOKEN=super-secret-value-12345");
    });

    // The review block (dl element inside the review div) must show the key but
    // NOT the value. We target the review block specifically, not the textarea.
    const reviewBlock = container.querySelector("dl");
    expect(reviewBlock).toBeTruthy();
    const reviewHtml = reviewBlock!.innerHTML;

    // Key must appear.
    expect(reviewHtml).toContain("MY_TOKEN");
    // Value must NOT appear in the review block.
    expect(reviewHtml).not.toContain("super-secret-value-12345");
    // The "values redacted" label must be present in the review.
    expect(reviewHtml).toContain("redacted");

    act(() => root.unmount());
    container.remove();
    invokeMock.mockClear();
  });
});
