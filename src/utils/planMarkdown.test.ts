import { describe, expect, it } from "vitest";
import { parseMarkdown, parseInline } from "./planMarkdown";
import type { MarkdownBlock } from "./planMarkdown";

// ---- parseInline ------------------------------------------------------------

describe("parseInline", () => {
  it("returns a single text segment for plain text", () => {
    const segs = parseInline("hello world");
    expect(segs).toEqual([{ kind: "text", text: "hello world" }]);
  });

  it("splits on inline code spans", () => {
    const segs = parseInline("run `npm install` now");
    expect(segs).toEqual([
      { kind: "text", text: "run " },
      { kind: "inline_code", code: "npm install" },
      { kind: "text", text: " now" },
    ]);
  });

  it("handles inline code at the start", () => {
    const segs = parseInline("`foo` bar");
    expect(segs).toEqual([
      { kind: "inline_code", code: "foo" },
      { kind: "text", text: " bar" },
    ]);
  });

  it("handles inline code at the end", () => {
    const segs = parseInline("see `bar`");
    expect(segs).toEqual([
      { kind: "text", text: "see " },
      { kind: "inline_code", code: "bar" },
    ]);
  });

  it("emits unmatched backtick as text", () => {
    const segs = parseInline("it's `broken");
    expect(segs).toEqual([
      { kind: "text", text: "it's " },
      { kind: "text", text: "`broken" },
    ]);
  });

  it("returns empty array for empty string", () => {
    expect(parseInline("")).toEqual([]);
  });
});

// ---- parseMarkdown: headings ------------------------------------------------

describe("parseMarkdown headings", () => {
  it("parses H1", () => {
    const blocks = parseMarkdown("# Hello");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ kind: "heading", depth: 1 });
  });

  it("parses H2", () => {
    const blocks = parseMarkdown("## World");
    expect(blocks[0]).toMatchObject({ kind: "heading", depth: 2 });
  });

  it("parses H3 and H4", () => {
    const h3 = parseMarkdown("### Three")[0];
    const h4 = parseMarkdown("#### Four")[0];
    expect(h3).toMatchObject({ kind: "heading", depth: 3 });
    expect(h4).toMatchObject({ kind: "heading", depth: 4 });
  });

  it("caps heading depth at 4 (##### → depth 4)", () => {
    const blocks = parseMarkdown("##### Five");
    // ##### does not match the 1-4 heading pattern → treated as paragraph
    expect(blocks[0].kind).toBe("paragraph");
  });

  it("heading text contains inline code", () => {
    const blocks = parseMarkdown("## Run `cargo build`");
    expect(blocks[0]).toMatchObject({ kind: "heading", depth: 2 });
    const h = blocks[0] as Extract<MarkdownBlock, { kind: "heading" }>;
    expect(h.segments).toContainEqual({ kind: "inline_code", code: "cargo build" });
  });
});

// ---- parseMarkdown: lists ---------------------------------------------------

describe("parseMarkdown lists", () => {
  it("parses unordered list with -", () => {
    const blocks = parseMarkdown("- item one\n- item two");
    expect(blocks).toHaveLength(2);
    expect(blocks[0]).toMatchObject({ kind: "list_item", ordered: false });
    expect(blocks[1]).toMatchObject({ kind: "list_item", ordered: false });
  });

  it("parses unordered list with *", () => {
    const blocks = parseMarkdown("* alpha\n* beta");
    expect(blocks[0]).toMatchObject({ kind: "list_item", ordered: false });
  });

  it("parses ordered list", () => {
    const blocks = parseMarkdown("1. first\n2. second");
    expect(blocks[0]).toMatchObject({ kind: "list_item", ordered: true, number: 1 });
    expect(blocks[1]).toMatchObject({ kind: "list_item", ordered: true, number: 2 });
  });

  it("list item text is parsed for inline code", () => {
    const blocks = parseMarkdown("- run `make test`");
    const item = blocks[0] as Extract<MarkdownBlock, { kind: "list_item" }>;
    expect(item.segments).toContainEqual({ kind: "inline_code", code: "make test" });
  });
});

// ---- parseMarkdown: fenced code blocks --------------------------------------

describe("parseMarkdown fenced code", () => {
  it("parses triple-backtick fenced block", () => {
    const md = "```\nconst x = 1;\n```";
    const blocks = parseMarkdown(md);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ kind: "code", code: "const x = 1;" });
  });

  it("parses tilde fenced block", () => {
    const md = "~~~\necho hello\n~~~";
    const blocks = parseMarkdown(md);
    expect(blocks[0]).toMatchObject({ kind: "code", code: "echo hello" });
  });

  it("preserves multiline code verbatim (no processing)", () => {
    const md = "```\nline one\nline two\n```";
    const block = parseMarkdown(md)[0] as Extract<MarkdownBlock, { kind: "code" }>;
    expect(block.code).toBe("line one\nline two");
  });

  it("ignores language specifier on the opening fence", () => {
    const md = "```typescript\nconst x = 1;\n```";
    const block = parseMarkdown(md)[0] as Extract<MarkdownBlock, { kind: "code" }>;
    expect(block.kind).toBe("code");
    expect(block.code).toBe("const x = 1;");
  });

  // WARNING #9: a line that merely STARTS WITH the fence (e.g. an info-string line
  // ```python inside the block) must NOT close the outer code block. A closing fence
  // is a line consisting ONLY of fence chars (>= opening length, trailing ws allowed).
  it("does not close on a nested info-string fence line (```python inside)", () => {
    const md = "```\n```python\ncode = 1\n```";
    const blocks = parseMarkdown(md);
    expect(blocks).toHaveLength(1);
    const block = blocks[0] as Extract<MarkdownBlock, { kind: "code" }>;
    expect(block.kind).toBe("code");
    // The whole inner content stays inside one code block.
    expect(block.code).toBe("```python\ncode = 1");
  });

  it("closes on a bare fence line with trailing whitespace", () => {
    const md = "```\ncode = 1\n```   ";
    const block = parseMarkdown(md)[0] as Extract<MarkdownBlock, { kind: "code" }>;
    expect(block.kind).toBe("code");
    expect(block.code).toBe("code = 1");
  });

  it("closes on a longer closing fence run (CommonMark: >= opening length)", () => {
    const md = "```\ncode = 1\n`````";
    const block = parseMarkdown(md)[0] as Extract<MarkdownBlock, { kind: "code" }>;
    expect(block.kind).toBe("code");
    expect(block.code).toBe("code = 1");
  });
});

// ---- parseMarkdown: paragraphs ----------------------------------------------

describe("parseMarkdown paragraphs", () => {
  it("parses a plain paragraph", () => {
    const blocks = parseMarkdown("Hello world");
    expect(blocks[0]).toMatchObject({ kind: "paragraph" });
  });

  it("separates paragraphs on blank lines", () => {
    const blocks = parseMarkdown("First para\n\nSecond para");
    expect(blocks).toHaveLength(2);
    expect(blocks[0]).toMatchObject({ kind: "paragraph" });
    expect(blocks[1]).toMatchObject({ kind: "paragraph" });
  });

  it("merges consecutive non-blank lines into one paragraph", () => {
    const blocks = parseMarkdown("Line one\nLine two\nLine three");
    expect(blocks).toHaveLength(1);
    const p = blocks[0] as Extract<MarkdownBlock, { kind: "paragraph" }>;
    expect(p.segments[0]).toMatchObject({ kind: "text" });
    const text = (p.segments[0] as { kind: "text"; text: string }).text;
    expect(text).toContain("Line one");
    expect(text).toContain("Line two");
  });
});

// ---- hostile input: security ------------------------------------------------

describe("parseMarkdown hostile input stays inert text", () => {
  it("raw <script> tag in paragraph is kept as literal text (no block type)", () => {
    const blocks = parseMarkdown("<script>alert(1)</script>");
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind).toBe("paragraph");
    const p = blocks[0] as Extract<MarkdownBlock, { kind: "paragraph" }>;
    // The text must contain the raw angle-bracket string — NOT interpreted.
    const allText = p.segments
      .filter((s) => s.kind === "text")
      .map((s) => (s as { kind: "text"; text: string }).text)
      .join("");
    expect(allText).toContain("<script>alert(1)</script>");
  });

  it("javascript: URI in markdown-link syntax is kept as text (no link block)", () => {
    // [x](javascript:alert(1)) must never become a clickable link.
    const blocks = parseMarkdown("[click](javascript:alert(1))");
    // There is no "link" block type at all — it falls through to paragraph.
    expect(blocks[0].kind).toBe("paragraph");
    // The "link" block type never exists in the union — this assertion documents the
    // security invariant (no link passthrough) without a compile-time false comparison.
    const kinds = blocks.map((b) => b.kind as string);
    expect(kinds.includes("link")).toBe(false);
  });

  it("HTML image tag is kept as literal text", () => {
    const blocks = parseMarkdown('<img src="x" onerror="alert(1)">');
    expect(blocks[0].kind).toBe("paragraph");
    const p = blocks[0] as Extract<MarkdownBlock, { kind: "paragraph" }>;
    const allText = p.segments.map((s) => ("text" in s ? s.text : "")).join("");
    expect(allText).toContain("<img");
  });

  it("heading with embedded <script> tag is still a heading (text node only)", () => {
    const blocks = parseMarkdown("# Title <script>bad()</script>");
    expect(blocks[0].kind).toBe("heading");
    const h = blocks[0] as Extract<MarkdownBlock, { kind: "heading" }>;
    // Segments are text/inline_code only — no html or link type.
    const allText = h.segments.map((s) => ("text" in s ? s.text : "")).join("");
    expect(allText).toContain("<script>bad()</script>");
  });

  it("empty input returns empty array", () => {
    expect(parseMarkdown("")).toEqual([]);
    expect(parseMarkdown("   \n   ")).toEqual([]);
  });
});
