// @vitest-environment jsdom
//
// PiExtensionsCard — settings card for managing pi extensions. Mirrors the
// jsdom + createRoot + act pattern of CliAgentsCard.test.tsx and
// DesignLlmBackendCard.test.tsx. `invokeBackendCommand` is module-mocked.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { PiExtensionsCard } from "./PiExtensionsCard";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

const STATUS_IDLE = {
  agentDir: "/home/user/.pi",
  mode: "global",
  bootstrap: "idle",
  bootstrapError: null,
} as const;

const INSTALLED_ROW = {
  source: "npm:pi-lens",
  name: "pi-lens",
  version: "1.0.0",
  description: "A lens for pi",
  author: "alice",
  installedOk: true,
} as const;

const MARKETPLACE_ROWS = [
  {
    name: "pi-lens",
    version: "1.0.0",
    description: "A lens for pi",
    author: "alice",
    date: "2024-01-01",
  },
  {
    name: "pi-utils",
    version: "0.3.0",
    description: "Utilities",
    author: "bob",
    date: "2024-02-15",
  },
];

function textContent(): string {
  return container.textContent ?? "";
}

function findButton(label: string): HTMLButtonElement | null {
  return (
    Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (b) => b.textContent?.includes(label),
    ) ?? null
  );
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);

  // Default mock: status + installed list + empty marketplace
  invokeMock.mockImplementation(async (...args: unknown[]) => {
    const cmd = args[0] as string;
    if (cmd === "pi_extensions_status") return STATUS_IDLE;
    if (cmd === "pi_extensions_list") return [INSTALLED_ROW];
    if (cmd === "pi_marketplace_search") return [];
    return undefined;
  });
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  invokeMock.mockClear();
  vi.useRealTimers();
});

async function mount(): Promise<void> {
  await act(async () => {
    root.render(createElement(PiExtensionsCard));
  });
  // Flush mount effects + marketplace auto-search microtasks.
  await act(async () => {
    await Promise.resolve();
  });
}

async function waitFor(
  predicate: () => boolean,
  timeoutMs = 2000,
): Promise<void> {
  const step = 50;
  let elapsed = 0;
  while (!predicate() && elapsed < timeoutMs) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, step));
    });
    elapsed += step;
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("PiExtensionsCard", () => {
  it("renders installed rows with name, version, and Remove button", async () => {
    await mount();
    expect(textContent()).toContain("pi-lens");
    expect(textContent()).toContain("1.0.0");
    expect(textContent()).toContain("A lens for pi");
    expect(container.querySelector('[data-testid="remove-npm:pi-lens"]')).not.toBeNull();
  });

  it("shows empty-state text when no extensions are installed", async () => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status") return STATUS_IDLE;
      if (cmd === "pi_extensions_list") return [];
      if (cmd === "pi_marketplace_search") return [];
      return undefined;
    });
    await mount();
    expect(textContent()).toContain("No extensions installed yet.");
  });

  it("install button calls pi_extension_install with typed source and re-fetches", async () => {
    await mount();
    const input = container.querySelector(
      'input[placeholder*="npm:pi-lens"]',
    ) as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    await act(async () => {
      setter.call(input, "npm:pi-utils");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    const btn = findButton("Install")!;
    await act(async () => {
      btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "pi_extension_install",
    );
    expect(call).toBeTruthy();
    expect(call![1]).toEqual({ source: "npm:pi-utils" });
    // After install, list is re-fetched.
    const listCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "pi_extensions_list",
    );
    expect(listCalls.length).toBeGreaterThanOrEqual(2); // mount + post-install
  });

  it("remove button calls pi_extension_remove with the row's source", async () => {
    await mount();
    const removeBtn = container.querySelector(
      '[data-testid="remove-npm:pi-lens"]',
    )!;
    await act(async () => {
      removeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "pi_extension_remove",
    );
    expect(call).toBeTruthy();
    expect(call![1]).toEqual({ source: "npm:pi-lens" });
  });

  it("marketplace results render with Install buttons; already-installed shows 'Installed'", async () => {
    // pi-lens is installed; pi-utils is not.
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status") return STATUS_IDLE;
      if (cmd === "pi_extensions_list") return [INSTALLED_ROW];
      if (cmd === "pi_marketplace_search") return MARKETPLACE_ROWS;
      return undefined;
    });
    await mount();

    // Wait for the auto-search to populate the marketplace results.
    await waitFor(() => textContent().includes("pi-utils"));
    expect(textContent()).toContain("pi-utils");

    // pi-lens is installed → "Installed" label, no Install button for it.
    expect(textContent()).toContain("Installed");

    // pi-utils is NOT installed → Install button present.
    expect(
      container.querySelector('[data-testid="install-marketplace-pi-utils"]'),
    ).not.toBeNull();
  });

  it("backend error on install shows error text", async () => {
    await mount();
    const input = container.querySelector(
      'input[placeholder*="npm:pi-lens"]',
    ) as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    await act(async () => {
      setter.call(input, "npm:bad-pkg");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    // Next pi_extension_install throws.
    invokeMock.mockImplementationOnce(async () => {
      throw new Error("package not found");
    });
    const btn = findButton("Install")!;
    await act(async () => {
      btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(textContent()).toContain("package not found");
  });

  it("renders status line with agentDir and mode label; bootstrap failed shows error", async () => {
    invokeMock.mockImplementationOnce(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status")
        return {
          agentDir: "/some/path",
          mode: "appManaged",
          bootstrap: "failed",
          bootstrapError: "Network timeout",
        };
      if (cmd === "pi_extensions_list") return [INSTALLED_ROW];
      if (cmd === "pi_marketplace_search") return [];
      return undefined;
    });
    await mount();
    expect(textContent()).toContain("/some/path");
    expect(textContent()).toContain("app-managed");
    expect(textContent()).toContain("Network timeout");
    expect(textContent()).toContain("Retry happens on next app launch");
  });

  it("bootstrap failed with null error shows generic message (Fix 6)", async () => {
    invokeMock.mockImplementationOnce(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status")
        return {
          agentDir: "/d",
          mode: "envOverride",
          bootstrap: "failed",
          bootstrapError: null,
        };
      if (cmd === "pi_extensions_list") return [];
      if (cmd === "pi_marketplace_search") return [];
      return undefined;
    });
    await mount();
    expect(textContent()).toContain("Extension bootstrap failed.");
    expect(textContent()).toContain("Retry happens on next app launch");
  });

  it("bootstrap running shows polling message", async () => {
    invokeMock.mockImplementationOnce(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status")
        return {
          agentDir: "/d",
          mode: "global",
          bootstrap: "running",
          bootstrapError: null,
        };
      if (cmd === "pi_extensions_list") return [];
      if (cmd === "pi_marketplace_search") return [];
      return undefined;
    });
    await mount();
    expect(textContent()).toContain("Installing the starter extension set");
  });

  it("marketplace search failure is non-fatal — installed list still renders", async () => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status") return STATUS_IDLE;
      if (cmd === "pi_extensions_list") return [INSTALLED_ROW];
      if (cmd === "pi_marketplace_search")
        throw new Error("Network unreachable");
      return undefined;
    });
    await mount();
    await waitFor(() => textContent().includes("Network unreachable"));
    // Installed list is still visible.
    expect(textContent()).toContain("pi-lens");
    expect(textContent()).toContain("Marketplace search failures are non-fatal");
  });

  it("remove error surfaces under the row", async () => {
    await mount();
    // Make next pi_extension_remove fail.
    invokeMock.mockImplementationOnce(async () => {
      throw new Error("permission denied");
    });
    const removeBtn = container.querySelector(
      '[data-testid="remove-npm:pi-lens"]',
    )!;
    await act(async () => {
      removeBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(textContent()).toContain("permission denied");
  });

  it("marketplace Install button calls pi_extension_install with npm:<name> source", async () => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "pi_extensions_status") return STATUS_IDLE;
      if (cmd === "pi_extensions_list") return []; // nothing installed
      if (cmd === "pi_marketplace_search") return MARKETPLACE_ROWS;
      return undefined;
    });
    await mount();

    await waitFor(() => textContent().includes("pi-utils"));

    const installBtn = container.querySelector(
      '[data-testid="install-marketplace-pi-utils"]',
    ) as HTMLButtonElement;
    expect(installBtn).not.toBeNull();

    await act(async () => {
      installBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "pi_extension_install",
    );
    expect(call).toBeTruthy();
    expect(call![1]).toEqual({ source: "npm:pi-utils" });
  });

  // Fix 8: fake-timers poll test
  it(
    "polls pi_extensions_status every 2 s while bootstrap is running, stops after done",
    async () => {
      vi.useFakeTimers();

      // Mount with bootstrap = running.
      invokeMock.mockImplementation(async (...args: unknown[]) => {
        const cmd = args[0] as string;
        if (cmd === "pi_extensions_status")
          return {
            agentDir: "/d",
            mode: "global",
            bootstrap: "running",
            bootstrapError: null,
          };
        if (cmd === "pi_extensions_list") return [];
        if (cmd === "pi_marketplace_search") return [];
        return undefined;
      });
      await mount();

      const statusCallsBefore = invokeMock.mock.calls.filter(
        (c) => c[0] === "pi_extensions_status",
      ).length;

      // Advance 2 s — the interval fires and re-queries status.
      await act(async () => {
        vi.advanceTimersByTime(2000);
        await Promise.resolve();
      });

      const statusCallsAfterPoll = invokeMock.mock.calls.filter(
        (c) => c[0] === "pi_extensions_status",
      ).length;
      expect(statusCallsAfterPoll).toBeGreaterThan(statusCallsBefore);

      // Switch mock so the next status call returns "done". Then advance 0 ms
      // to flush the interval's loadAll resolution + React re-render + cleanup.
      invokeMock.mockImplementation(async (...args: unknown[]) => {
        const cmd = args[0] as string;
        if (cmd === "pi_extensions_status")
          return {
            agentDir: "/d",
            mode: "global",
            bootstrap: "done",
            bootstrapError: null,
          };
        if (cmd === "pi_extensions_list") return [];
        if (cmd === "pi_marketplace_search") return [];
        return undefined;
      });

      const statusCallsBeforeDone = invokeMock.mock.calls.filter(
        (c) => c[0] === "pi_extensions_status",
      ).length;

      // Advance 0 ms: runs the interval callback's loadAll (which now returns
      // done), flushes the microtask queue so React processes the state update
      // and the useEffect cleanup clears the interval.
      await act(async () => {
        vi.advanceTimersByTime(0);
        await Promise.resolve();
      });

      const statusCallsAfterDone = invokeMock.mock.calls.filter(
        (c) => c[0] === "pi_extensions_status",
      ).length;
      // The interval was cleared when bootstrap left "running", so the count
      // must NOT have increased.
      expect(statusCallsAfterDone).toBe(statusCallsBeforeDone);
    },
    10_000,
  );
});
