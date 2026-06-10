// The inner markup of ONE canvas node. Split into its own memoized component so an
// unrelated canvas re-render (pan, zoom, selecting a DIFFERENT node, dragging a
// sibling) does NOT re-run sanitization / `dangerouslySetInnerHTML` / re-parse this
// node's markup. React.memo compares the `markup`/`generating` props by identity;
// the RAW markup string is stable across renders unless the node's content actually
// changed, so the chokepoint + innerHTML run exactly once per content change.
//
// SECURITY: this component is the LAST sanitization point before the DOM. N3: the
// `sanitizeNodeMarkup` chokepoint lives HERE (not in the parent canvas) wrapped in a
// `useMemo` keyed on the raw markup, so it runs only when the markup changes yet
// stays immediately before `dangerouslySetInnerHTML`. The parent passes RAW,
// untrusted markup; everything written to innerHTML below has passed the chokepoint.
// (Defense in depth: the parent app DOM has no `allow-scripts` sandbox, so the
// sanitizer IS the boundary.)

import { memo, useMemo } from "react";

import { sanitizeNodeMarkup } from "../sanitize";

interface NodeContentProps {
  /** RAW (untrusted) inner markup for this node — sanitized here before innerHTML. */
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
  // N3: sanitize ONLY when the raw markup changes (not on every parent re-render).
  // This is the single sanitization chokepoint and the LAST step before innerHTML.
  const clean = useMemo(() => sanitizeNodeMarkup(markup), [markup]);
  const html = generating && !markup ? SKELETON : clean;
  return (
    <div className="node-content" dangerouslySetInnerHTML={{ __html: html }} />
  );
}

export const NodeContent = memo(NodeContentImpl);
