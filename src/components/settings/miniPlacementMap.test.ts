import { describe, expect, it } from "vitest";
import {
	allMiniCoderBackendKinds,
	defaultMiniKindForPlacement,
	miniEnginesForPlacement,
	miniEngineLabel,
	miniKindAfterPlacementSwitch,
	miniKindBelongsToPlacement,
	miniKindIsUnsupported,
	miniPlacementFromKind,
	type MiniPlacement,
} from "./miniPlacementMap";

const PLACEMENTS: MiniPlacement[] = ["On this Mac", "Cloud API", "Agent CLI"];

describe("miniPlacementFromKind / engines", () => {
	it("maps every MiniCoderBackendKind into a placement that owns it", () => {
		for (const kind of allMiniCoderBackendKinds()) {
			const placement = miniPlacementFromKind(kind);
			expect(PLACEMENTS).toContain(placement);
			expect(miniKindBelongsToPlacement(kind, placement)).toBe(true);
		}
	});

	it("round-trips: kind → placement ownership holds for every kind", () => {
		const table: Record<string, MiniPlacement> = {
			ollama: "On this Mac",
			omlx: "On this Mac",
			appleFm: "On this Mac",
			cloud: "Cloud API",
			api: "Cloud API",
			codex: "Agent CLI",
			openai: "Agent CLI",
		};
		for (const [kind, placement] of Object.entries(table)) {
			expect(miniPlacementFromKind(kind as keyof typeof table)).toBe(placement);
		}
	});

	it("selectable engines exclude openai; openai is unsupported stub", () => {
		expect(miniEnginesForPlacement("Agent CLI")).toEqual(["codex"]);
		expect(miniEnginesForPlacement("Agent CLI")).not.toContain("openai");
		expect(miniKindIsUnsupported("openai")).toBe(true);
		expect(miniKindIsUnsupported("codex")).toBe(false);
		expect(miniEngineLabel("openai")).toBe("OpenAI (unsupported)");
	});

	it("api keeps original Custom command (advanced) label under Cloud API", () => {
		expect(miniPlacementFromKind("api")).toBe("Cloud API");
		expect(miniEnginesForPlacement("Cloud API")).toContain("api");
		expect(miniEngineLabel("api")).toBe("Custom command (advanced)");
	});

	it("default kind for each placement is selectable (never unsupported)", () => {
		for (const p of PLACEMENTS) {
			const d = defaultMiniKindForPlacement(p);
			expect(miniEnginesForPlacement(p)).toContain(d);
			expect(miniKindIsUnsupported(d)).toBe(false);
		}
	});

	it("placement switch keeps kind when valid (incl. saved openai), else defaults", () => {
		expect(miniKindAfterPlacementSwitch("omlx", "On this Mac")).toBe("omlx");
		expect(miniKindAfterPlacementSwitch("omlx", "Cloud API")).toBe("cloud");
		expect(miniKindAfterPlacementSwitch("codex", "Agent CLI")).toBe("codex");
		// Saved openai stays under Agent CLI (belongs) — not coerced away until user picks.
		expect(miniKindAfterPlacementSwitch("openai", "Agent CLI")).toBe("openai");
		expect(miniKindAfterPlacementSwitch("cloud", "Agent CLI")).toBe("codex");
		expect(miniKindAfterPlacementSwitch("api", "On this Mac")).toBe("ollama");
	});
});
