import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import { MarkdownRenderer } from "./MarkdownRenderer";
import { parseMarkdown } from "../../utils/planMarkdown";
import type { MarkdownBlock } from "../../utils/planMarkdown";

// Trojan-Source style spoof code points the renderer must strip from DISPLAY
// (stored data stays raw; only the rendered text nodes are sanitized).
const RLO = "‮"; // right-to-left override
const LRI = "⁦"; // left-to-right isolate
const ZWSP = "​"; // zero-width space
const BOM = "﻿"; // byte-order mark

describe("MarkdownRenderer strips BIDI/invisible spoof chars from display (NIT #10)", () => {
  it("renders a heading without the RLO override char", () => {
    const blocks = parseMarkdown(`# Deploy${RLO} now`);
    const html = renderToStaticMarkup(<MarkdownRenderer blocks={blocks} />);
    expect(html).not.toContain(RLO);
    expect(html).toContain("Deploy");
    expect(html).toContain("now");
  });

  it("strips spoof chars from paragraph text", () => {
    const blocks = parseMarkdown(`Run the${ZWSP} pipeline${LRI}`);
    const html = renderToStaticMarkup(<MarkdownRenderer blocks={blocks} />);
    expect(html).not.toContain(ZWSP);
    expect(html).not.toContain(LRI);
  });

  it("strips spoof chars from inline code spans", () => {
    const blocks = parseMarkdown(`Use \`rm${RLO} -rf\` carefully`);
    const html = renderToStaticMarkup(<MarkdownRenderer blocks={blocks} />);
    expect(html).not.toContain(RLO);
  });

  it("strips spoof chars from fenced code blocks", () => {
    const blocks = parseMarkdown("```\n" + `evil${BOM}code` + "\n```");
    const html = renderToStaticMarkup(<MarkdownRenderer blocks={blocks} />);
    expect(html).not.toContain(BOM);
    expect(html).toContain("evilcode");
  });

  it("strips spoof chars from list items", () => {
    const blocks = parseMarkdown(`- step${RLO} one`);
    const html = renderToStaticMarkup(<MarkdownRenderer blocks={blocks} />);
    expect(html).not.toContain(RLO);
  });

  it("leaves ordinary text untouched", () => {
    const blocks: MarkdownBlock[] = parseMarkdown("# Hello world");
    const html = renderToStaticMarkup(<MarkdownRenderer blocks={blocks} />);
    expect(html).toContain("Hello world");
  });
});
