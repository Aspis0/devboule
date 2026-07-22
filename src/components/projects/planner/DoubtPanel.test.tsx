// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import type { QuestionEntry } from "../../agents/agentConsoleModel";
import { DoubtPanel } from "./DoubtPanel";
import { DOUBT_OPTION_FONT_PX, DOUBT_QUESTION_FONT_PX } from "./doubtPanelModel";

function question(over: Partial<QuestionEntry> = {}): QuestionEntry {
  return {
    id: "q1",
    type: "question",
    text: "Vuoi un footer minimalista o ricco di link?",
    options: [
      { id: "a", label: "Minimalista" },
      { id: "b", label: "Ricco di link" },
    ],
    status: "open",
    lean: null,
    candidates: [],
    unrest: 0.4,
    directionConfidence: 0.3,
    affects: ["T3"],
    time: "2026-07-21T12:00:00Z",
    ...over,
  };
}

describe("DoubtPanel F38 single-fire + F37 sizing", () => {
  it("sends onSend only once when the same option is clicked twice", async () => {
    const onSend = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);

    await act(async () => {
      root.render(
        createElement(DoubtPanel, {
          questions: [question()],
          onSend,
          highlightedDoubtIds: new Set(),
          onHoverDoubt: () => {},
        }),
      );
    });

    const btn = host.querySelector(
      '[data-testid="doubt-option-q1-a"]',
    ) as HTMLButtonElement | null;
    expect(btn).toBeTruthy();

    await act(async () => {
      btn!.click();
      btn!.click();
      btn!.click();
    });

    expect(onSend).toHaveBeenCalledTimes(1);
    // Card dismissed after first answer.
    expect(host.querySelector('[data-testid="doubt-card-q1"]')).toBeNull();

    await act(async () => {
      root.unmount();
    });
    host.remove();
  });

  it("uses readable question/option font sizes (F37)", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);

    await act(async () => {
      root.render(
        createElement(DoubtPanel, {
          questions: [question()],
          onSend: () => {},
          highlightedDoubtIds: new Set(),
          onHoverDoubt: () => {},
        }),
      );
    });

    const card = host.querySelector('[data-testid="doubt-card-q1"]') as HTMLElement;
    expect(card).toBeTruthy();
    const questionEl = card.querySelector("div > div") as HTMLElement;
    // First nested flex child text node container uses DOUBT_QUESTION_FONT_PX
    const qStyle = (questionEl.style?.fontSize || "").toString();
    // Style is applied inline on the question text div — walk for fontSize
    const withQ = Array.from(card.querySelectorAll("div")).find(
      (el) => (el as HTMLElement).style.fontSize === `${DOUBT_QUESTION_FONT_PX}px`,
    );
    expect(withQ).toBeTruthy();
    const withOpt = Array.from(card.querySelectorAll("button")).find(
      (el) => (el as HTMLElement).style.fontSize === `${DOUBT_OPTION_FONT_PX}px`,
    );
    expect(withOpt).toBeTruthy();
    void qStyle;

    await act(async () => {
      root.unmount();
    });
    host.remove();
  });

  it("renders a free-form (optionless) doubt without crashing", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);

    await act(async () => {
      root.render(
        createElement(DoubtPanel, {
          // Free-form doubt: empty options (and runtime-missing options via ??) is legal.
          questions: [
            question({ options: [] }),
            question({
              id: "q-missing",
              text: "What should we call the product?",
              options: undefined as unknown as QuestionEntry["options"],
            }),
          ],
          onSend: () => {},
          highlightedDoubtIds: new Set(),
          onHoverDoubt: () => {},
        }),
      );
    });

    expect(host.querySelector('[data-testid="doubt-card-q1"]')).toBeTruthy();
    expect(host.querySelector('[data-testid="doubt-card-q-missing"]')).toBeTruthy();
    expect(host.textContent).toContain("Vuoi un footer minimalista");
    expect(host.textContent).toContain("What should we call the product?");
    // No option buttons when options is missing/empty.
    expect(host.querySelector('[data-testid^="doubt-option-"]')).toBeNull();
    // Free-reply affordance still present.
    expect(host.querySelector('[data-testid="doubt-you-decide-q1"]')).toBeTruthy();
    expect(host.querySelector('[data-testid="doubt-you-decide-q-missing"]')).toBeTruthy();

    await act(async () => {
      root.unmount();
    });
    host.remove();
  });
});
