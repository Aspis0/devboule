// The sanitized inner markup of ONE canvas node. Split into its own memoized
// component so an unrelated canvas re-render (pan, zoom, selecting a DIFFERENT
// node, dragging a sibling) does NOT re-run `dangerouslySetInnerHTML` / re-parse
// this node's markup. React.memo compares the `markup`/`generating` props by
// identity; the markup string is stable across renders unless the node's content
// actually changed, so innerHTML is written exactly once per content change.
//
// SECURITY: `markup` MUST already be the output of `sanitizeNodeMarkup`. The
// canvas is the single caller and routes every string through that chokepoint
// before it reaches here (defense in depth: the parent app DOM has no
// `allow-scripts` sandbox, so the sanitizer is the boundary).

import { memo } from "react";

interface NodeContentProps {
  /** ALREADY-sanitized inner markup for this node. */
  markup: string;
  /** When true (and no markup yet) show the generating skeleton instead. */
  generating?: boolean;
}

// A neutral loading skeleton shown while a node is being generated and has no
// markup yet. Pure static markup (no untrusted input) — safe to inline.
const SKELETON =
  '<div class="skel"><i style="height:22px;width:55%"></i>' +
  '<i style="height:13px;width:88%"></i><i style="height:13px;width:72%"></i>' +
  '<i style="height:40px;width:100%;margin-top:8px"></i></div>';

function NodeContentImpl({ markup, generating }: NodeContentProps) {
  const html = generating && !markup ? SKELETON : markup;
  return (
    <div className="node-content" dangerouslySetInnerHTML={{ __html: html }} />
  );
}

export const NodeContent = memo(NodeContentImpl);
