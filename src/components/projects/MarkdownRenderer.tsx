// Pure renderer for the typed block array produced by parseMarkdown.
// All text is rendered as TEXT NODES — no dangerouslySetInnerHTML, no links,
// no images, no raw HTML passthrough. Agent-authored content is untrusted.

import type {
  CodeBlock,
  HeadingBlock,
  InlineSegment,
  ListItemBlock,
  MarkdownBlock,
  ParagraphBlock,
} from "../../utils/planMarkdown";
import { stripSpoofChars } from "../agents/attentionNotifier";

// ---- inline renderer --------------------------------------------------------
//
// PRIVACY/SECURITY: plan markdown is UNTRUSTED agent-authored text. Stored data is
// kept RAW; only the DISPLAYED text is sanitized of BIDI/zero-width spoof code points
// (Trojan-Source style) via stripSpoofChars — the last gate before the user's eyes.

function InlineSegments({ segments }: { segments: InlineSegment[] }) {
  return (
    <>
      {segments.map((seg, i) =>
        seg.kind === "inline_code" ? (
          <code
            key={i}
            className="rounded bg-cream-100 px-1 py-0.5 font-mono text-[0.9em] text-cream-800"
          >
            {stripSpoofChars(seg.code)}
          </code>
        ) : (
          <span key={i}>{stripSpoofChars(seg.text)}</span>
        ),
      )}
    </>
  );
}

// ---- block renderers --------------------------------------------------------

function HeadingBlockView({ block }: { block: HeadingBlock }) {
  const cls =
    block.depth === 1
      ? "text-[15px] font-bold text-cream-900 mt-3 mb-1"
      : block.depth === 2
        ? "text-[13px] font-semibold text-cream-800 mt-2.5 mb-1"
        : block.depth === 3
          ? "text-[12px] font-semibold text-cream-700 mt-2 mb-0.5"
          : "text-[11px] font-semibold text-cream-600 mt-1.5 mb-0.5";
  return (
    <p className={cls}>
      <InlineSegments segments={block.segments} />
    </p>
  );
}

function ListItemView({ block }: { block: ListItemBlock }) {
  return (
    <li className="ml-4 text-[12px] leading-relaxed text-cream-700 list-disc marker:text-cream-400">
      {block.ordered && block.number !== undefined && (
        <span className="mr-1 text-cream-400">{block.number}.</span>
      )}
      <InlineSegments segments={block.segments} />
    </li>
  );
}

function CodeBlockView({ block }: { block: CodeBlock }) {
  return (
    <pre className="overflow-x-auto rounded-lg bg-cream-50 p-3 text-[11px] font-mono leading-relaxed text-cream-800 border border-cream-200">
      <code>{stripSpoofChars(block.code)}</code>
    </pre>
  );
}

function ParagraphBlockView({ block }: { block: ParagraphBlock }) {
  return (
    <p className="text-[12px] leading-relaxed text-cream-700">
      <InlineSegments segments={block.segments} />
    </p>
  );
}

// ---- main renderer ----------------------------------------------------------

export interface MarkdownRendererProps {
  blocks: MarkdownBlock[];
}

export function MarkdownRenderer({ blocks }: MarkdownRendererProps) {
  if (blocks.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5">
      {blocks.map((block, i) => {
        switch (block.kind) {
          case "heading":
            return <HeadingBlockView key={i} block={block} />;
          case "list_item":
            return <ListItemView key={i} block={block} />;
          case "code":
            return <CodeBlockView key={i} block={block} />;
          case "paragraph":
            return <ParagraphBlockView key={i} block={block} />;
        }
      })}
    </div>
  );
}
