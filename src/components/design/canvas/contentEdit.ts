// Content-edit (CE) mode DOM helpers — ported from the prototype's
// `content-edit.jsx`. These operate on LIVE DOM nodes inside a CE node's content
// root (inline-editing, helper-class/attr stripping, clean serialization).
//
// SECURITY: `cleanSerialize` ONLY strips the CE helper classes/attributes and
// returns `innerHTML`. It is NOT a sanitizer — the CALLER (DesignView) MUST re-run
// the serialized markup through `sanitizeNodeMarkup` before it is persisted or
// re-rendered. CE edits happen on already-sanitized DOM, and `startInlineTextEdit`
// intercepts PASTE to insert plain text only (so rich HTML — `<img onerror>` — can
// never land in the live editable DOM and fire before the commit-time sanitize). The
// upstream sanitize on commit stays the authoritative boundary regardless: serialize
// here, sanitize upstream.

// The transient helper classes CE adds to live elements during editing. They must
// never survive into persisted markup (and the sanitizer would strip `class`
// entirely anyway — this keeps the serialized intermediate clean for diffing).
const CE_HELPER_CLASSES = ["ce-hover", "ce-sel"];

/**
 * Serialize an edited CE content root back to clean node markup. Clones the
 * container (so the live DOM is never mutated), removes the CE helper classes and
 * the `contenteditable`/`spellcheck` attributes from EVERY descendant, then returns
 * the clone's `innerHTML`. PURE w.r.t. the input node (operates on a clone).
 *
 * NOTE: this strips `class` ENTIRELY when only CE helper classes remain (the
 * prototype removed the whole `class` attribute). We mirror that: after removing the
 * helper tokens, an empty `class` attribute is dropped so the intermediate markup
 * has no dangling `class=""`. (The downstream sanitizer strips `class` anyway.)
 *
 * The CALLER must re-sanitize the result before persistence (see file header).
 */
export function cleanSerialize(container: HTMLElement): string {
  const clone = container.cloneNode(true) as HTMLElement;
  const all = clone.querySelectorAll<HTMLElement>("*");
  for (const el of Array.from(all)) {
    // Remove CE helper classes; drop the whole attribute if nothing meaningful
    // remains (matches the prototype's `removeAttribute("class")`, but preserves a
    // genuine author class that was present before CE — defensive vs. the prototype
    // which blew away every class; the sanitizer strips class regardless).
    if (el.classList.length > 0) {
      for (const c of CE_HELPER_CLASSES) el.classList.remove(c);
      if (el.classList.length === 0) el.removeAttribute("class");
    }
    el.removeAttribute("contenteditable");
    el.removeAttribute("spellcheck");
  }
  return clone.innerHTML;
}

/**
 * True when `el` has at least one DIRECT non-empty text child node. Used to enable
 * the "Edit text" affordance / inline-edit entry only on elements that actually own
 * text (not pure containers). Ported verbatim from the prototype.
 */
export function elHasText(el: Element | null | undefined): boolean {
  if (!el) return false;
  return Array.from(el.childNodes).some(
    (n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? "").trim().length > 0,
  );
}

/**
 * Make `el` inline-editable (contenteditable) until blur / Enter / Esc. On entry it
 * focuses the element and places the caret at the END of its content. Enter (without
 * Shift) COMMITS the typed text; Esc REVERTS to the text the element had on entry;
 * a plain blur COMMITS. Any of these tear down the listeners and remove the
 * contenteditable/spellcheck attributes; `onDone` then fires exactly once.
 *
 * Esc-reverts (vs. the prototype, which committed on both Enter and Esc) so a typo
 * mid-edit can be abandoned — the spec requires "Esc reverts text". The revert
 * captures the element's `innerHTML` at entry and restores it before blur.
 *
 * Returns a `cancel()` that force-ends the edit (used when CE mode aborts because the
 * node vanished, so a dangling contenteditable element doesn't linger). Calling
 * `cancel()` after a natural end is a no-op.
 */
export function startInlineTextEdit(
  el: HTMLElement | null | undefined,
  onDone?: () => void,
): () => void {
  if (!el) return () => {};
  // Snapshot the original content so Esc can revert (the typed-but-abandoned edit).
  const original = el.innerHTML;
  el.setAttribute("contenteditable", "true");
  el.setAttribute("spellcheck", "false");
  el.focus();
  // Select-all then collapse to the end (caret at end of existing content).
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(false);
  const sel = window.getSelection();
  if (sel) {
    sel.removeAllRanges();
    sel.addRange(range);
  }

  let ended = false;
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      e.stopPropagation();
      el.blur(); // commit the typed text
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      el.innerHTML = original; // revert before exiting
      el.blur();
    }
  };
  // SECURITY: a contenteditable element accepts rich HTML on paste, and the browser
  // inserts it into the LIVE DOM *before* anything we do — an `<img onerror>` (or a
  // `<script>` in browsers that keep it) would fire on that live insertion, BEFORE the
  // commit path ever reaches `sanitizeNodeMarkup`. So we intercept paste, take the
  // PLAIN-TEXT clipboard payload only, and insert it as text. No markup ever lands in
  // the live editable DOM via paste.
  const onPaste = (e: ClipboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const text = e.clipboardData?.getData("text/plain") ?? "";
    if (!text) return;
    // Prefer the Selection API (range-based) so the caret position is honored; fall
    // back to execCommand("insertText") where a selection isn't available.
    const selection = window.getSelection();
    if (selection && selection.rangeCount > 0) {
      const r = selection.getRangeAt(0);
      r.deleteContents();
      r.insertNode(document.createTextNode(text));
      r.collapse(false); // caret after the inserted text
      selection.removeAllRanges();
      selection.addRange(r);
    } else {
      document.execCommand("insertText", false, text);
    }
  };
  const end = () => {
    if (ended) return;
    ended = true;
    el.removeAttribute("contenteditable");
    el.removeAttribute("spellcheck");
    el.removeEventListener("blur", end);
    el.removeEventListener("keydown", onKeyDown);
    el.removeEventListener("paste", onPaste);
    onDone?.();
  };
  el.addEventListener("blur", end);
  el.addEventListener("keydown", onKeyDown);
  el.addEventListener("paste", onPaste);
  return end;
}
