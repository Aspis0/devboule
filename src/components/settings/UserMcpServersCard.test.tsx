// @vitest-environment jsdom
//
// Tests for UserMcpServersCard (global scope) and McpServerList.
//
// Covers:
//   - Renders a list of servers returned by user_mcp_list.
//   - Disabled servers are grayed (opacity-60 class on the row).
//   - Enable/disable toggle calls user_mcp_set_enabled with the correct args.
//   - Remove button shows a confirm step; confirming calls user_mcp_remove.
//   - The "network access" warning indicator appears only when ≥1 server is enabled.
//   - The "Add server" button opens the consent dialog.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { UserMcpServer } from "../../types/userMcpServers";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

let mockServers: UserMcpServer[] = [];
const invokeMock = vi.fn(async (cmd: unknown, ...rest: unknown[]) => {
  if (cmd === "user_mcp_list") return mockServers;
  if (cmd === "user_mcp_set_enabled") {
    const args = rest[0] as { name: string; enabled: boolean };
    mockServers = mockServers.map((s) =>
      s.name === args.name ? { ...s, enabled: args.enabled } : s,
    );
    return undefined;
  }
  if (cmd === "user_mcp_remove") {
    const args = rest[0] as { name: string };
    mockServers = mockServers.filter((s) => s.name !== args.name);
    return undefined;
  }
  return undefined;
});

// The dialog is its own file; mock it to a no-op so the card tests don't
// invoke real backend commands through the dialog.
vi.mock("./UserMcpConsentDialog", () => ({
  UserMcpConsentDialog: ({
    onCancel,
  }: {
    onAdded: () => void;
    onCancel: () => void;
    scope: string;
    projectRoot?: string;
  }) =>
    createElement(
      "div",
      { "data-testid": "mock-consent-dialog" },
      createElement(
        "button",
        { type: "button", onClick: onCancel, "data-testid": "mock-cancel" },
        "Cancel",
      ),
    ),
}));

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) =>
    invokeMock(...(args as [unknown, ...unknown[]])),
}));

import { McpServerList } from "./UserMcpServersCard";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeServer(
  name: string,
  enabled = true,
  command = "/usr/bin/tool",
): UserMcpServer {
  return { name, transport: "stdio", command, args: [], env: {}, enabled };
}

async function mountList(
  scope: "global" | "project" = "global",
  projectRoot?: string,
): Promise<{ container: HTMLDivElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(createElement(McpServerList, { scope, projectRoot }));
  });
  // Flush the initial load.
  await act(async () => {
    await Promise.resolve();
  });
  return { container, root };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("McpServerList — rendering", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    invokeMock.mockClear();
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });

  it("shows 'No servers configured' when the list is empty", async () => {
    mockServers = [];
    ({ container, root } = await mountList());
    expect(container.textContent).toContain("No servers configured");
  });

  it("renders a row for each server", async () => {
    mockServers = [makeServer("db-tool"), makeServer("ci-tool")];
    ({ container, root } = await mountList());
    expect(container.querySelector("[data-testid='server-row-db-tool']")).toBeTruthy();
    expect(container.querySelector("[data-testid='server-row-ci-tool']")).toBeTruthy();
  });

  it("grays disabled server rows with opacity-60", async () => {
    mockServers = [makeServer("disabled-srv", false)];
    ({ container, root } = await mountList());
    const row = container.querySelector("[data-testid='server-row-disabled-srv']");
    expect(row).toBeTruthy();
    expect(row!.className).toContain("opacity-60");
  });

  it("does NOT gray enabled server rows", async () => {
    mockServers = [makeServer("enabled-srv", true)];
    ({ container, root } = await mountList());
    const row = container.querySelector("[data-testid='server-row-enabled-srv']");
    expect(row!.className).not.toContain("opacity-60");
  });
});

describe("McpServerList — network access warning", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    invokeMock.mockClear();
    mockServers = [];
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });

  it("does NOT show the warning when no servers are configured", async () => {
    mockServers = [];
    ({ container, root } = await mountList());
    expect(
      container.querySelector("[data-testid='network-access-warning']"),
    ).toBeNull();
  });

  it("does NOT show the warning when all servers are disabled", async () => {
    mockServers = [makeServer("srv", false)];
    ({ container, root } = await mountList());
    expect(
      container.querySelector("[data-testid='network-access-warning']"),
    ).toBeNull();
  });

  it("shows the warning when at least one server is enabled", async () => {
    mockServers = [makeServer("srv", true)];
    ({ container, root } = await mountList());
    expect(
      container.querySelector("[data-testid='network-access-warning']"),
    ).toBeTruthy();
  });

  it("shows the warning counting only enabled servers", async () => {
    mockServers = [makeServer("a", true), makeServer("b", false), makeServer("c", true)];
    ({ container, root } = await mountList());
    const warning = container.querySelector("[data-testid='network-access-warning']");
    expect(warning).toBeTruthy();
    expect(warning!.textContent).toContain("2");
  });
});

describe("McpServerList — toggle", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    invokeMock.mockClear();
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });

  it("clicking the toggle calls user_mcp_set_enabled with the flipped enabled value", async () => {
    mockServers = [makeServer("my-srv", true)];
    ({ container, root } = await mountList());

    const toggle = container.querySelector(
      "[data-testid='toggle-my-srv']",
    ) as HTMLButtonElement;
    expect(toggle).toBeTruthy();

    await act(async () => {
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "user_mcp_set_enabled",
    );
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ scope: "global", name: "my-srv", enabled: false });
  });

  it("toggle on a disabled server calls user_mcp_set_enabled with enabled=true", async () => {
    mockServers = [makeServer("off-srv", false)];
    ({ container, root } = await mountList());

    const toggle = container.querySelector(
      "[data-testid='toggle-off-srv']",
    ) as HTMLButtonElement;
    await act(async () => {
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "user_mcp_set_enabled",
    );
    expect(call![1]).toMatchObject({ name: "off-srv", enabled: true });
  });
});

describe("McpServerList — remove", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    invokeMock.mockClear();
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });

  it("clicking Remove shows a confirm step (does NOT call user_mcp_remove yet)", async () => {
    mockServers = [makeServer("del-srv")];
    ({ container, root } = await mountList());

    const removeBtn = container.querySelector(
      "[data-testid='remove-del-srv']",
    ) as HTMLButtonElement;
    await act(async () => {
      removeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(
      invokeMock.mock.calls.some((c) => c[0] === "user_mcp_remove"),
    ).toBe(false);
    // The confirm button must appear.
    expect(
      container.querySelector("[data-testid='confirm-remove-del-srv']"),
    ).toBeTruthy();
  });

  it("confirming remove calls user_mcp_remove with the correct name", async () => {
    mockServers = [makeServer("del-srv")];
    ({ container, root } = await mountList());

    // First click: show confirm.
    const removeBtn = container.querySelector(
      "[data-testid='remove-del-srv']",
    ) as HTMLButtonElement;
    await act(async () => {
      removeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // Second click: confirm.
    const confirmBtn = container.querySelector(
      "[data-testid='confirm-remove-del-srv']",
    ) as HTMLButtonElement;
    await act(async () => {
      confirmBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_remove");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ scope: "global", name: "del-srv" });
  });
});

describe("McpServerList — Add server button", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    invokeMock.mockClear();
    mockServers = [];
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });

  it("clicking 'Add server' opens the consent dialog", async () => {
    ({ container, root } = await mountList());
    expect(container.querySelector("[data-testid='mock-consent-dialog']")).toBeNull();

    const addBtn = container.querySelector(
      "[data-testid='add-server-btn']",
    ) as HTMLButtonElement;
    await act(async () => {
      addBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(
      container.querySelector("[data-testid='mock-consent-dialog']"),
    ).toBeTruthy();
  });

  it("cancelling the consent dialog closes it without calling any command", async () => {
    ({ container, root } = await mountList());

    const addBtn = container.querySelector(
      "[data-testid='add-server-btn']",
    ) as HTMLButtonElement;
    await act(async () => {
      addBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // Cancel from mock dialog.
    const cancelBtn = container.querySelector(
      "[data-testid='mock-cancel']",
    ) as HTMLButtonElement;
    await act(async () => {
      cancelBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(container.querySelector("[data-testid='mock-consent-dialog']")).toBeNull();
    // No add/remove commands fired.
    expect(
      invokeMock.mock.calls.some(
        (c) => c[0] === "user_mcp_add" || c[0] === "user_mcp_remove",
      ),
    ).toBe(false);
  });
});

describe("McpServerList — double-submit guard (F2)", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    invokeMock.mockClear();
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });

  it("rapid double-click on toggle fires user_mcp_set_enabled only once", async () => {
    mockServers = [makeServer("dbl-srv", true)];
    ({ container, root } = await mountList());

    // Confirm the row rendered correctly before proceeding.
    const toggle = container.querySelector(
      "[data-testid='toggle-dbl-srv']",
    ) as HTMLButtonElement;
    expect(toggle).toBeTruthy();

    // Replace the mock so user_mcp_set_enabled blocks (simulating slow network)
    // while user_mcp_list still responds normally.
    let resolveToggle!: () => void;
    const toggleCall = new Promise<void>((res) => { resolveToggle = res; });
    invokeMock.mockImplementation(async (cmd: unknown) => {
      if (cmd === "user_mcp_list") return mockServers;
      if (cmd === "user_mcp_set_enabled") { await toggleCall; return undefined; }
      return undefined;
    });

    // Two rapid clicks — the ref guard must eat the second.
    await act(async () => {
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    resolveToggle();
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    // Restore default mock.
    invokeMock.mockImplementation(async (cmd: unknown) => {
      if (cmd === "user_mcp_list") return mockServers;
      return undefined;
    });

    const calls = invokeMock.mock.calls.filter((c) => c[0] === "user_mcp_set_enabled");
    expect(calls.length).toBe(1);
  });

  it("rapid double-click on confirm-remove fires user_mcp_remove only once", async () => {
    mockServers = [makeServer("dbl-rm", true)];
    ({ container, root } = await mountList());

    const removeBtn = container.querySelector(
      "[data-testid='remove-dbl-rm']",
    ) as HTMLButtonElement;
    expect(removeBtn).toBeTruthy();

    // Open confirm step.
    await act(async () => {
      removeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    const confirmBtn = container.querySelector(
      "[data-testid='confirm-remove-dbl-rm']",
    ) as HTMLButtonElement;
    expect(confirmBtn).toBeTruthy();

    // Block user_mcp_remove while allowing list refresh afterwards.
    let resolveRemove!: () => void;
    const removeCall = new Promise<void>((res) => { resolveRemove = res; });
    invokeMock.mockImplementation(async (cmd: unknown) => {
      if (cmd === "user_mcp_list") return mockServers;
      if (cmd === "user_mcp_remove") { await removeCall; return undefined; }
      return undefined;
    });

    await act(async () => {
      confirmBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      confirmBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    resolveRemove();
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    // Restore default mock.
    invokeMock.mockImplementation(async (cmd: unknown) => {
      if (cmd === "user_mcp_list") return mockServers;
      return undefined;
    });

    const calls = invokeMock.mock.calls.filter((c) => c[0] === "user_mcp_remove");
    expect(calls.length).toBe(1);
  });
});

describe("McpServerList — project scope", () => {
  beforeEach(() => {
    invokeMock.mockClear();
    mockServers = [];
  });

  afterEach(() => {
    invokeMock.mockClear();
    mockServers = [];
  });

  it("passes scope=project and projectRoot in list + toggle calls", async () => {
    mockServers = [makeServer("proj-srv", true)];
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(McpServerList, {
          scope: "project",
          projectRoot: "/abs/project/root",
        }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    const listCall = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_list");
    expect(listCall![1]).toMatchObject({
      scope: "project",
      projectRoot: "/abs/project/root",
    });

    const toggle = container.querySelector(
      "[data-testid='toggle-proj-srv']",
    ) as HTMLButtonElement;
    await act(async () => {
      toggle.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const toggleCall = invokeMock.mock.calls.find(
      (c) => c[0] === "user_mcp_set_enabled",
    );
    expect(toggleCall![1]).toMatchObject({
      scope: "project",
      projectRoot: "/abs/project/root",
    });

    act(() => root.unmount());
    container.remove();
    mockServers = [];
  });
});
