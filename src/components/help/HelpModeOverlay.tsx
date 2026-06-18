import { useEffect, useRef, useState } from "react";
import { useAppContext } from "../../context/AppContext";

type HelpItem = {
  id: string;
  top: number;
  left: number;
  title: string;
  lines: string[];
};

const MAX_HELP_ITEMS = 120;
const HELP_WIDTH = 380;
const HELP_HEIGHT = 238;
const HELP_MAX_LINES = 8;

const pageUseLines: Record<string, string> = {
  cloudflare:
    "For Aspis Bio, Cloudflare is the edge layer: Workers, routes, R2/KV/D1/queues, secrets, smoke checks, and agent-safe provider operations.",
  compute:
    "For Aspis Bio, compute is where expensive CPU/GPU resources live, so this page helps prevent idle VM cost and wrong-project operations.",
  secrets:
    "For Aspis Bio, this page keeps provider keys, project scopes, object-storage credentials, and model keys out of code and project notes.",
  oracle:
    "For Aspis Bio, Oracle is the local memory agents should query before touching code, cloud resources, plans, or project notes.",
  projects:
    "For Aspis Bio, Projects is the mini-Notion control board where human plans, agent claims, evidence, and verifier gates meet.",
  agents:
    "For Aspis Bio, Agents is the bridge between the Kanban, CLI terminals, MCP tools, Oracle, and provider permissions.",
  providers:
    "For Aspis Bio, Providers maps what parts of Cloudflare and Scaleway are already dashboard-ready and what still needs safer tooling.",
  budget:
    "For Aspis Bio, Budget is the cost-warning layer before GPU, VM, storage, or Worker usage becomes invisible spend.",
  graph:
    "For Aspis Bio, Graph is a structural map of code relationships; Oracle is the stronger source for semantic answers.",
};

function cleanText(value: string | null | undefined) {
  return (value ?? "").replace(/\s+/g, " ").trim();
}

function fallbackLabel(element: HTMLElement) {
  const explicit =
    element.getAttribute("aria-label") ||
    element.getAttribute("title") ||
    element.getAttribute("placeholder") ||
    cleanText(element.textContent);
  return cleanText(explicit) || element.tagName.toLowerCase();
}

function fallbackTitle(element: HTMLElement) {
  const tag = element.tagName.toLowerCase();
  const label = fallbackLabel(element);
  if (tag === "input" || tag === "textarea") return "This field is where you type a value.";
  if (tag === "select") return "This menu chooses which thing the app works on.";
  if (tag === "button") return `This button runs "${label}".`;
  return `This part controls "${label}".`;
}

function areaText(element: HTMLElement) {
  const section = element.closest("section, article, aside, header, main, div");
  return cleanText(section?.textContent).slice(0, 600);
}

function semanticLines(element: HTMLElement, title: string) {
  const haystack = `${title} ${fallbackLabel(element)} ${element.dataset.helpLines ?? ""} ${areaText(element)}`.toLowerCase();
  const lines: string[] = [];

  if (haystack.includes("secret")) {
    lines.push("For Aspis Bio, secrets keep model, provider, and Worker credentials out of code, Markdown, Oracle chunks, and agent prompts.");
  }
  if (haystack.includes("token") || haystack.includes("api key") || haystack.includes("key")) {
    lines.push("For Aspis Bio, tokens decide whether the app can read inventory, rotate Worker secrets, query remote models, or give agents scoped access.");
    lines.push("Temporary keys expire: replace them in the app vault instead of hardcoding them or pasting them into project notes.");
  }
  if (haystack.includes("cloudflare") || haystack.includes("worker") || haystack.includes("r2") || haystack.includes("kv") || haystack.includes("d1")) {
    lines.push("For Aspis Bio, Cloudflare can host edge APIs, Workers, storage, queues, routing, and smoke checks used by pipelines and agent tooling.");
  }
  if (haystack.includes("scaleway") || haystack.includes("gpu") || haystack.includes("cpu") || haystack.includes(" vm") || haystack.includes("compute")) {
    lines.push("For Aspis Bio, Scaleway resources are real spend: sync before action, stop idle machines, and confirm project scope before delete or terminate.");
  }
  if (haystack.includes("oracle") || haystack.includes("index") || haystack.includes("chunk") || haystack.includes("embedding") || haystack.includes("lancedb")) {
    lines.push("For Aspis Bio, Oracle must retrieve real files and project evidence before any local or remote model answer is trusted.");
  }
  if (haystack.includes("agent") || haystack.includes("mcp") || haystack.includes("codex") || haystack.includes("claude") || haystack.includes("verifier") || haystack.includes("coder")) {
    lines.push("For Aspis Bio, agents should work through MCP: read project state, ask Oracle, claim tasks, use allowed provider tools, then update status.");
  }
  if (haystack.includes("project") || haystack.includes("task") || haystack.includes("kanban") || haystack.includes("note")) {
    lines.push("For Aspis Bio, project Markdown is the durable source of truth that the UI, Oracle, and CLI agents can all read.");
  }
  if (haystack.includes("budget") || haystack.includes("cost") || haystack.includes("billing") || haystack.includes("price")) {
    lines.push("For Aspis Bio, cost signals matter because GPU/CPU VM mistakes can burn money faster than Worker or storage mistakes.");
  }
  if (haystack.includes("dry") || haystack.includes("smoke") || haystack.includes("audit")) {
    lines.push("For Aspis Bio, dry runs and audits are proof steps: they should show scope, token, API equivalent, and evidence before a real write.");
  }

  return lines;
}

function fallbackLines(element: HTMLElement) {
  const disabled =
    element.hasAttribute("disabled") || element.getAttribute("aria-disabled") === "true";
  const tag = element.tagName.toLowerCase();
  const label = fallbackLabel(element);
  const lowerLabel = label.toLowerCase();
  if (tag === "select") {
    return [
      `This menu chooses ${label === "select" ? "which item the next action uses" : label}.`,
      "A menu is usually safe by itself: it changes context, not cloud state.",
      "The important part is the next button you press, because it may use this selection.",
      "For Aspis Bio, check project, provider, account, model, and role selections before launching agents or cloud actions.",
      disabled ? "It is disabled because the required data is not ready yet." : "If the list looks empty, sync or reload the page first.",
    ];
  }
  if (tag === "input" || tag === "textarea") {
    if (lowerLabel.includes("password") || lowerLabel.includes("token") || lowerLabel.includes("key") || element.getAttribute("type") === "password") {
      return [
        "This field is for a private credential or key-like value.",
        "For Aspis Bio, credentials should live in the Windows vault, not in code, project Markdown, Oracle chunks, or agent prompts.",
        "The app should save the value only when you press the matching Save/Rotate action.",
        "Temporary provider keys expire; replace them here when sync, model calls, or agent operations start failing.",
        disabled ? "It is disabled because another required condition is missing." : "Before saving, check that the token belongs to the pinned Aspis Bio account or project.",
      ];
    }
    return [
      `This field lets you type ${label === "input" || label === "textarea" ? "a value for this page" : label}.`,
      "It normally changes only local form state until you press the matching action.",
      "For Aspis Bio, prefer concrete names: project title, task goal, provider id, model, root path, or evidence note.",
      "Do not type raw secrets in ordinary notes, search fields, or project text.",
      disabled ? "It is disabled because another required value or job is missing." : "If the value controls agents or cloud resources, verify it before saving.",
    ];
  }
  if (tag === "button") {
    return [
      `This button runs "${label}".`,
      "Disabled usually means a required token, project, selection, sync result, or confirmation is missing.",
      "For Aspis Bio, cloud and agent actions should run through the Tauri backend so permissions, scopes, and audit evidence are controlled.",
      "Provider writes should show the provider scope, token role, API equivalent, and project evidence when wired.",
      "For destructive actions, read the confirmation text before accepting.",
    ];
  }
  return [
    "This area affects what you see, what is selected, or what the next action will use.",
    "Provider data comes from live sync when tokens and scopes are configured.",
    "For Aspis Bio, project evidence should be written to local Markdown so Oracle and agents can recover context.",
    "If something looks stale, refresh the page section before acting.",
  ];
}

function uniqueLines(lines: string[]) {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const line of lines.map(cleanText).filter(Boolean)) {
    const key = line.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(line);
  }
  return result;
}

function readHelp(element: HTMLElement, activeView: string) {
  const title = cleanText(element.dataset.helpTitle) || fallbackTitle(element);
  const rawLines = element.dataset.helpLines;
  const baseLines = rawLines
    ? rawLines
        .split("|")
        .map((line) => cleanText(line))
        .filter(Boolean)
    : fallbackLines(element);
  const lines = uniqueLines([
    ...baseLines,
    ...semanticLines(element, title),
    pageUseLines[activeView] ?? "For Aspis Bio, use this only when it makes the local project, agents, cloud state, or Oracle memory more reliable.",
  ]);
  return { title, lines: lines.slice(0, HELP_MAX_LINES) };
}

function collectHelpItems(activeView: string) {
  const selector =
    "[data-help-title], [data-help-lines], button, input, select, textarea, [role='button'], a[href]";
  const elements = Array.from(document.querySelectorAll<HTMLElement>(selector));
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const items: HelpItem[] = [];

  for (const element of elements) {
    if (items.length >= MAX_HELP_ITEMS) break;
    if (element.closest("[data-help-skip='true']")) continue;
    const rect = element.getBoundingClientRect();
    if (rect.width < 3 || rect.height < 3) continue;
    if (rect.bottom < 0 || rect.top > viewportHeight || rect.right < 0 || rect.left > viewportWidth) {
      continue;
    }

    const { title, lines } = readHelp(element, activeView);
    const canPlaceRight = rect.right + HELP_WIDTH + 12 < viewportWidth;
    const canPlaceLeft = rect.left - HELP_WIDTH - 12 > 0;
    const left = canPlaceRight
      ? rect.right + 8
      : canPlaceLeft
        ? rect.left - HELP_WIDTH - 8
        : Math.min(
            Math.max(8, rect.left),
            Math.max(8, viewportWidth - HELP_WIDTH - 8),
          );
    const below = rect.bottom + HELP_HEIGHT < viewportHeight || rect.top < HELP_HEIGHT;
    const top = canPlaceRight || canPlaceLeft
      ? Math.min(Math.max(8, rect.top), Math.max(8, viewportHeight - HELP_HEIGHT - 8))
      : below
        ? Math.min(rect.bottom + 6, Math.max(8, viewportHeight - HELP_HEIGHT - 8))
        : Math.max(8, rect.top - HELP_HEIGHT - 6);

    items.push({
      id: `${items.length}:${Math.round(rect.left)}:${Math.round(rect.top)}:${title}`,
      top,
      left,
      title,
      lines,
    });
  }

  return items;
}

export function HelpModeOverlay() {
  const { activeView } = useAppContext();
  // helpMode lives here, not in the global AppContext: holding/releasing Alt
  // would otherwise re-render the whole app. Only this overlay needs it.
  const [helpMode, setHelpMode] = useState(false);
  const [items, setItems] = useState<HelpItem[]>([]);

  // Alt key drives help mode. Kept local so the global provider never re-renders
  // on Alt press/release.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Alt" || event.altKey) setHelpMode(true);
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === "Alt" || !event.altKey) setHelpMode(false);
    };
    const onBlur = () => setHelpMode(false);

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  // Cache the active view in a ref so scroll/resize handlers always read the
  // current value without re-subscribing listeners.
  const activeViewRef = useRef(activeView);
  activeViewRef.current = activeView;

  useEffect(() => {
    if (!helpMode) {
      setItems([]);
      return;
    }

    // Recompute help-item rects on a requestAnimationFrame, throttled so that a
    // burst of scroll/resize events collapses into a single layout read per
    // frame. No setInterval: nothing forces sync layout on a timer anymore.
    let frame = 0;
    let scheduled = false;
    const run = () => {
      scheduled = false;
      frame = 0;
      setItems(collectHelpItems(activeViewRef.current));
    };
    const update = () => {
      if (scheduled) return;
      scheduled = true;
      frame = window.requestAnimationFrame(run);
    };

    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [helpMode]);

  // Recompute once when the active view changes while help mode is held.
  useEffect(() => {
    if (!helpMode) return;
    const frame = window.requestAnimationFrame(() =>
      setItems(collectHelpItems(activeView)),
    );
    return () => window.cancelAnimationFrame(frame);
  }, [activeView, helpMode]);

  if (!helpMode) return null;

  return (
    <div className="pointer-events-none fixed inset-0 z-[120]">
      <div className="absolute right-4 top-3 max-w-xs rounded-xl border border-terracotta/20 bg-white/95 px-3 py-2 text-[11px] font-semibold text-cream-700 shadow-soft-lg">
        Help mode: hold Alt to read what each command does and why it matters for Aspis Bio.
      </div>
      {items.map((item) => (
        <div
          key={item.id}
          style={{ top: item.top, left: item.left, width: HELP_WIDTH }}
          className="absolute max-h-60 overflow-hidden rounded-xl border border-terracotta/20 bg-white/95 px-3 py-2 text-left shadow-soft-lg backdrop-blur"
        >
          <p className="text-[12px] font-semibold leading-4 text-cream-900">
            {item.title}
          </p>
          <div className="mt-1 space-y-0.5">
            {item.lines.map((line, index) => (
              <p key={`${item.id}:${index}`} className="text-[10.5px] leading-4 text-cream-600">
                {line}
              </p>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
