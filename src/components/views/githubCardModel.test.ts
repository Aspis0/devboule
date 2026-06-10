import { describe, expect, it, vi } from "vitest";
import type { GithubConnectionStatus } from "../../types/backend";
import {
  githubCardPill,
  shouldShowGithubImportButton,
  shouldShowGithubRemoveButton,
  loadGithubStatus,
  saveGithubToken,
  importGithubTokenFromCli,
  deleteGithubToken,
  type GithubInvoke,
} from "./githubCardModel";

function status(over: Partial<GithubConnectionStatus>): GithubConnectionStatus {
  return {
    configured: false,
    status: "missing",
    source: "windows_vault",
    cliAvailable: false,
    login: null,
    name: null,
    avatarUrl: null,
    profileUrl: null,
    scopes: [],
    rateLimitRemaining: null,
    lastCheckedAt: null,
    message: null,
    ...over,
  };
}

describe("githubCardPill", () => {
  it("shows 'Checking auth' before the first status load", () => {
    expect(githubCardPill(null)).toEqual({ tone: "checking", label: "Checking auth" });
  });

  it("shows the connected login when the token is valid", () => {
    expect(githubCardPill(status({ status: "valid", login: "octocat" }))).toEqual({
      tone: "valid",
      label: "Connected as octocat",
    });
  });

  it("connected without a login still reads valid", () => {
    expect(githubCardPill(status({ status: "valid", login: null }))).toEqual({
      tone: "valid",
      label: "Connected",
    });
  });

  it("maps an error status to the fix-needed pill", () => {
    expect(githubCardPill(status({ status: "error" }))).toEqual({
      tone: "error",
      label: "Auth needs fix",
    });
  });

  it("maps a missing token to 'Not connected'", () => {
    expect(githubCardPill(status({ status: "missing" }))).toEqual({
      tone: "missing",
      label: "Not connected",
    });
  });
});

describe("shouldShowGithubImportButton", () => {
  it("is hidden before the first load (cli availability unknown)", () => {
    expect(shouldShowGithubImportButton(null)).toBe(false);
  });

  it("is hidden when the GitHub CLI is not available", () => {
    expect(shouldShowGithubImportButton(status({ cliAvailable: false }))).toBe(false);
  });

  it("is shown when the GitHub CLI is available", () => {
    expect(shouldShowGithubImportButton(status({ cliAvailable: true }))).toBe(true);
  });
});

describe("shouldShowGithubRemoveButton", () => {
  it("is hidden when no token is stored", () => {
    expect(shouldShowGithubRemoveButton(status({ configured: false }))).toBe(false);
  });

  it("is shown once a token is stored (even if invalid)", () => {
    expect(shouldShowGithubRemoveButton(status({ configured: true, status: "error" }))).toBe(
      true,
    );
  });
});

describe("github IPC actions", () => {
  // A spy that satisfies the generic GithubInvoke signature. The inner cast is
  // needed because every action returns GithubConnectionStatus, but the generic
  // `invoke<T>` is wider; the spy records the (command, args) we assert on.
  function mockInvoke(result: GithubConnectionStatus) {
    const spy = vi.fn(
      (_command: string, _args?: Record<string, unknown>) => Promise.resolve(result),
    );
    return spy as typeof spy & GithubInvoke;
  }

  it("loadGithubStatus calls get_github_connection_status with no args", async () => {
    const invoke = mockInvoke(status({}));
    await loadGithubStatus(invoke);
    expect(invoke).toHaveBeenCalledWith("get_github_connection_status");
  });

  it("saveGithubToken calls save_github_token with the token arg (camelCase parity)", async () => {
    const invoke = mockInvoke(status({ status: "valid" }));
    await saveGithubToken(invoke, "ghp_example_token_value");
    expect(invoke).toHaveBeenCalledWith("save_github_token", {
      token: "ghp_example_token_value",
    });
  });

  it("importGithubTokenFromCli calls import_github_token_from_cli with no token arg", async () => {
    const invoke = mockInvoke(status({}));
    await importGithubTokenFromCli(invoke);
    expect(invoke).toHaveBeenCalledWith("import_github_token_from_cli");
    // The CLI import must never receive a token from the UI.
    expect(invoke.mock.calls[0]).toHaveLength(1);
  });

  it("deleteGithubToken calls delete_github_token with no args", async () => {
    const invoke = mockInvoke(status({}));
    await deleteGithubToken(invoke);
    expect(invoke).toHaveBeenCalledWith("delete_github_token");
  });
});
