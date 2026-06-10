// Minimal PURE markdown block parser for agent-authored plan text.
//
// SECURITY: agent markdown is UNTRUSTED. All text is returned as plain string
// data; the renderer must map blocks to TEXT nodes only — never dangerouslySetInnerHTML,
// never pass through links, images, or raw HTML. This parser deliberately drops
// anything that looks like HTML and never produces link/image/html block types.
//
// Supported:
//   - Headings:     # H1  ## H2  ### H3  #### H4  (ATX style, depth 1–4)
//   - Unordered lists: lines starting with `- ` or `* `
//   - Ordered lists:   lines starting with `1. ` (any digit prefix + `. `)
//   - Fenced code blocks: ``` or ~~~ fences (verbatim content, no highlighting)
//   - Inline code spans: `code` within text → split into typed segments
//   - Paragraphs:   any other non-empty line groups
//
// NOT supported (by design, for security/simplicity):
//   - Links, images, raw HTML, blockquotes, horizontal rules, tables.

// ---- block types ------------------------------------------------------------

export interface HeadingBlock {
  kind: "heading";
  depth: 1 | 2 | 3 | 4;
  /** Inline segments (text + inline-code spans). */
  segments: InlineSegment[];
}

export interface ListItemBlock {
  kind: "list_item";
  ordered: boolean;
  /** Number for ordered list items (1-based as written). */
  number?: number;
  segments: InlineSegment[];
}

export interface CodeBlock {
  kind: "code";
  /** Raw verbatim content, no processing. */
  code: string;
}

export interface ParagraphBlock {
  kind: "paragraph";
  segments: InlineSegment[];
}

export type MarkdownBlock =
  | HeadingBlock
  | ListItemBlock
  | CodeBlock
  | ParagraphBlock;

// ---- inline segments --------------------------------------------------------

export interface TextSegment {
  kind: "text";
  text: string;
}

export interface InlineCodeSegment {
  kind: "inline_code";
  code: string;
}

export type InlineSegment = TextSegment | InlineCodeSegment;

// ---- helpers ----------------------------------------------------------------

/** Parse inline `code` spans. Splits the raw text on backtick pairs.
 *  Multiple consecutive backticks are NOT supported (single-backtick spans only).
 *  Any leftover unmatched backtick is emitted as a text segment. */
export function parseInline(raw: string): InlineSegment[] {
  if (!raw) return [];
  const segments: InlineSegment[] = [];
  let rest = raw;
  while (rest.length > 0) {
    const open = rest.indexOf("`");
    if (open === -1) {
      segments.push({ kind: "text", text: rest });
      break;
    }
    // Text before the backtick.
    if (open > 0) segments.push({ kind: "text", text: rest.slice(0, open) });
    const afterOpen = rest.slice(open + 1);
    const close = afterOpen.indexOf("`");
    if (close === -1) {
      // Unmatched backtick — emit the rest as text.
      segments.push({ kind: "text", text: rest.slice(open) });
      break;
    }
    segments.push({ kind: "inline_code", code: afterOpen.slice(0, close) });
    rest = afterOpen.slice(close + 1);
  }
  return segments.filter(
    (s) => !(s.kind === "text" && s.text === ""),
  );
}

/** True when `line` is a CLOSING fence for an open block opened with `fenceLen`
 *  repetitions of `fenceChar`: the line, after trimming trailing whitespace, must
 *  consist solely of `fenceChar` repeated at least `fenceLen` times (CommonMark
 *  allows a longer closing run). Leading whitespace is NOT allowed here (the parser
 *  does not support indented fences), matching the opening anchor `^`. */
function isClosingFence(line: string, fenceChar: string, fenceLen: number): boolean {
  const trimmed = line.replace(/\s+$/, "");
  if (trimmed.length < fenceLen) return false;
  for (let k = 0; k < trimmed.length; k += 1) {
    if (trimmed[k] !== fenceChar) return false;
  }
  return true;
}

// ---- main parser ------------------------------------------------------------

/** Parse a markdown string into a typed block array.
 *  Agent-authored text is untrusted: HTML tags and link/image syntax are never
 *  converted into structural types — they are captured as literal text in the
 *  paragraph/heading text segments and rendered verbatim (harmless as text nodes). */
export function parseMarkdown(input: string): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  const lines = (input ?? "").split(/\r?\n/);
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    // --- fenced code block ---------------------------------------------------
    if (/^(`{3,}|~{3,})/.test(line)) {
      const fence = (line.match(/^(`{3,}|~{3,})/) as RegExpMatchArray)[1];
      const fenceChar = fence[0];
      const fenceLen = fence.length;
      const codeLines: string[] = [];
      i += 1;
      // CommonMark: the block closes ONLY on a line that, after trimming trailing
      // whitespace, is made up SOLELY of the same fence char, with a run length
      // >= the opening fence. A line that merely starts with the fence but carries
      // other text (e.g. an inner ```python info-string) does NOT close the block.
      while (i < lines.length && !isClosingFence(lines[i], fenceChar, fenceLen)) {
        codeLines.push(lines[i]);
        i += 1;
      }
      i += 1; // skip closing fence (or EOF)
      blocks.push({ kind: "code", code: codeLines.join("\n") });
      continue;
    }

    // --- heading -------------------------------------------------------------
    const headingMatch = line.match(/^(#{1,4}) (.+)/);
    if (headingMatch) {
      const depth = Math.min(headingMatch[1].length, 4) as 1 | 2 | 3 | 4;
      blocks.push({
        kind: "heading",
        depth,
        segments: parseInline(headingMatch[2].trim()),
      });
      i += 1;
      continue;
    }

    // --- unordered list item -------------------------------------------------
    const ulMatch = line.match(/^[-*] (.+)/);
    if (ulMatch) {
      blocks.push({
        kind: "list_item",
        ordered: false,
        segments: parseInline(ulMatch[1]),
      });
      i += 1;
      continue;
    }

    // --- ordered list item ---------------------------------------------------
    const olMatch = line.match(/^(\d+)\. (.+)/);
    if (olMatch) {
      blocks.push({
        kind: "list_item",
        ordered: true,
        number: parseInt(olMatch[1], 10),
        segments: parseInline(olMatch[2]),
      });
      i += 1;
      continue;
    }

    // --- blank line: paragraph separator -------------------------------------
    if (line.trim() === "") {
      i += 1;
      continue;
    }

    // --- paragraph: accumulate consecutive non-blank, non-structural lines --
    const paraLines: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !/^(#{1,4}) /.test(lines[i]) &&
      !/^[-*] /.test(lines[i]) &&
      !/^\d+\. /.test(lines[i]) &&
      !/^(`{3,}|~{3,})/.test(lines[i])
    ) {
      paraLines.push(lines[i]);
      i += 1;
    }
    if (paraLines.length > 0) {
      const text = paraLines.join(" ");
      const segs = parseInline(text);
      if (segs.length > 0) blocks.push({ kind: "paragraph", segments: segs });
    }
  }

  return blocks;
}
