// @vitest-environment jsdom
//
// Tests for ProjectMcpServersCard.
//
// The card is a thin wrapper over McpServerList (already tested in
// UserMcpServersCard.test.tsx). These tests verify the project-specific
// wiring: scope=project is always passed, projectRoot is forwarded, and the
// card renders without crashing with an arbitrary root path.

import { describe, expect, it, vi, afterEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { UserMcpServer } from "../../types/userMcpServers";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

let mockServers: UserMcpServer[] = [];
const invokeMock = vi.fn(async (cmd: unknown, ..._rest: unknown[]) => {
  if (cmd === "user_mcp_list") return mockServers;
  return undefined;
});

vi.mock("../settings/UserMcpConsentDialog", () => ({
  UserMcpConsentDialog: () => createElement("div", { "data-testid": "mock-dialog" }),
}));

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) =>
    invokeMock(...(args as [unknown, ...unknown[]])),
}));

import { ProjectMcpServersCard } from "./ProjectMcpServersCard";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("ProjectMcpServersCard — static shape", () => {
  afterEach(() => {
    invokeMock.mockClear();
    mockServers = [];
  });

  it("renders the project-scope heading", () => {
    const html = renderToStaticMarkup(
      createElement(ProjectMcpServersCard, { projectRoot: "/some/project" }),
    );
    expect(html).toContain("MCP servers (project)");
    expect(html).toContain(".devboule/mcp-servers.json");
  });
});

describe("ProjectMcpServersCard — commands use project scope", () => {
  let container: HTMLDivElement;
  let root: Root;

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    invokeMock.mockClear();
    mockServers = [];
  });

  it("calls user_mcp_list with scope=project and the projectRoot", async () => {
    mockServers = [];
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(ProjectMcpServersCard, {
          projectRoot: "/workspace/my-repo",
        }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    const listCall = invokeMock.mock.calls.find((c) => c[0] === "user_mcp_list");
    expect(listCall).toBeTruthy();
    expect(listCall![1]).toMatchObject({
      scope: "project",
      projectRoot: "/workspace/my-repo",
    });
  });

  it("renders server rows for the projectRoot", async () => {
    mockServers = [
      {
        name: "schema-tool",
        transport: "stdio",
        command: "node",
        args: ["schema.js"],
        env: {},
        enabled: true,
      },
    ];
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(
        createElement(ProjectMcpServersCard, {
          projectRoot: "/workspace/my-repo",
        }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(
      container.querySelector("[data-testid='server-row-schema-tool']"),
    ).toBeTruthy();
  });
});
