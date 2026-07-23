import { describe, it, expect } from "vitest";
import {
  buildDisplayFindings,
  findingTextForPage,
  previewScale,
  PREVIEW_LAYOUT_WIDTH,
  type PreviewStatusLike,
} from "./websearchPreview";
import type { StagePage, StageFinding } from "../projects/planner/plannerModel";

describe("previewScale", () => {
  it("scales a 1024 layout into a narrower frame", () => {
    expect(previewScale(256, 1024)).toBeCloseTo(0.25);
    expect(previewScale(FRAME_LIKE, PREVIEW_LAYOUT_WIDTH)).toBeCloseTo(
      FRAME_LIKE / PREVIEW_LAYOUT_WIDTH,
    );
  });

  it("never upscales past 1×", () => {
    expect(previewScale(2000, 1024)).toBe(1);
  });

  it("treats non-finite / non-positive as 1", () => {
    expect(previewScale(0)).toBe(1);
    expect(previewScale(-10)).toBe(1);
    expect(previewScale(Number.NaN)).toBe(1);
  });
});

const FRAME_LIKE = 334;

describe("findingTextForPage", () => {
  it("prefers a non-empty provider summary", () => {
    const r = findingTextForPage("provider says hi", "long excerpt ignored");
    expect(r).toEqual({ text: "provider says hi", source: "provider" });
  });

  it("falls back to a capped excerpt when summary is empty", () => {
    const excerpt = "a".repeat(400);
    const r = findingTextForPage("", excerpt, 280);
    expect(r).not.toBeNull();
    expect(r!.source).toBe("excerpt");
    expect(r!.text.endsWith("…")).toBe(true);
    expect(r!.text.length).toBeLessThanOrEqual(281); // 280 + ellipsis, or less at word boundary
  });

  it("returns null when both are empty", () => {
    expect(findingTextForPage("  ", null)).toBeNull();
    expect(findingTextForPage(undefined, "   ")).toBeNull();
  });
});

describe("buildDisplayFindings", () => {
  it("uses provider summary when present (no preview needed)", () => {
    const pages: StagePage[] = [
      { url: "https://a.example/x", title: "A", summary: "from provider" },
    ];
    const out = buildDisplayFindings(pages, [], {});
    expect(out).toEqual([{ text: "from provider", source: "provider" }]);
  });

  it("uses text_excerpt when summary is empty and preview is ready", () => {
    const pages: StagePage[] = [
      { url: "https://b.example/y", title: "B", summary: "" },
    ];
    const previews: Record<string, PreviewStatusLike> = {
      "https://b.example/y": {
        state: "ready",
        preview: {
          url: "https://b.example/y",
          finalUrl: "https://b.example/y",
          title: "B",
          sanitizedHtml: "<p>hi</p>",
          textExcerpt: "real page body that a reader would see",
          byteLen: 12,
          truncated: false,
        },
      },
    };
    const out = buildDisplayFindings(pages, [], previews);
    expect(out).toHaveLength(1);
    expect(out[0].source).toBe("excerpt");
    expect(out[0].text).toContain("real page body");
  });

  it("falls back to parent findings when there are no pages", () => {
    const findings: StageFinding[] = [{ text: "use a bounded channel", task: 3 }];
    const out = buildDisplayFindings([], findings, {});
    expect(out).toEqual([
      { text: "use a bounded channel", task: 3, source: "given" },
    ]);
  });
});
