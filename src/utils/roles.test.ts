import { describe, it, expect } from "vitest";
import { isViewAllowedForRole, navIdsForRole } from "./roles";

describe("isViewAllowedForRole", () => {
  it("lets admin open every view, including devices", () => {
    expect(isViewAllowedForRole("admin", "devices")).toBe(true);
    expect(isViewAllowedForRole("admin", "providers")).toBe(true);
    expect(isViewAllowedForRole("admin", "settings")).toBe(true);
  });

  it("blocks collaborators from the admin-only devices surface", () => {
    expect(isViewAllowedForRole("collaborator", "devices")).toBe(false);
  });

  it("lets collaborators open the compressed top-level nav", () => {
    // The standalone "oracle" view was restored and is visible to all roles
    // (it is not in the admin-only denylist).
    for (const view of ["dashboard", "projects", "providers", "polis", "oracle", "settings"]) {
      expect(isViewAllowedForRole("collaborator", view)).toBe(true);
    }
  });

  it("defaults a null/loading role to the restricted (collaborator) set", () => {
    expect(isViewAllowedForRole(null, "devices")).toBe(false);
    expect(isViewAllowedForRole(null, "providers")).toBe(true);
    expect(isViewAllowedForRole(undefined, "settings")).toBe(true);
  });
});

describe("navIdsForRole", () => {
  // The standalone "oracle" view was restored to the top-level nav.
  const TOP_LEVEL = ["dashboard", "projects", "providers", "polis", "oracle"];

  it("returns the full top-level nav for admin", () => {
    expect(navIdsForRole("admin", TOP_LEVEL)).toEqual(TOP_LEVEL);
  });

  it("returns the full top-level nav for collaborators (no admin-only top-level entry remains)", () => {
    expect(navIdsForRole("collaborator", TOP_LEVEL)).toEqual(TOP_LEVEL);
  });

  it("would still filter devices out of a list that contained it for a collaborator", () => {
    expect(navIdsForRole("collaborator", [...TOP_LEVEL, "devices"])).toEqual(
      TOP_LEVEL,
    );
  });
});
