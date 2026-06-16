// Tests for SpawnPanel — focused on the L2.4 "Local (Devboule)" orchestrator CLI
// option. Uses renderToStaticMarkup (no jsdom) to assert the static option list,
// proving the additive change: codex + claude are untouched and "orchestrator" is
// offered as a selectable coder client labelled "Local (Devboule)".

import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { SpawnPanel } from "./SpawnPanel";
import type { SpawnLaunchInput, SpawnSelection } from "./agentRowModel";

function render(extra?: Partial<React.ComponentProps<typeof SpawnPanel>>) {
  return renderToStaticMarkup(
    <SpawnPanel
      projects={[{ id: "p1", title: "Proj" }]}
      selectedProjectId="p1"
      tasks={[]}
      projectActive={true}
      isBusy={false}
      message={null}
      onLaunch={() => undefined}
      onCopyPrompt={() => undefined}
      {...extra}
    />,
  );
}

describe("SpawnPanel CLI options", () => {
  it("offers the built-in codex and claude clients (untouched)", () => {
    const html = render();
    // The radio buttons render the option labels verbatim.
    expect(html).toContain(">codex<");
    expect(html).toContain(">claude<");
  });

  it("offers the local Devboule orchestrator as a selectable client", () => {
    const html = render();
    expect(html).toContain("Local (Devboule)");
  });

  it("appends configured custom clients AFTER the built-ins", () => {
    const html = render({
      customClients: [{ id: "deepseek", label: "DeepSeek", command: "ds chat" }],
    });
    expect(html).toContain("Local (Devboule)");
    expect(html).toContain("DeepSeek");
    // Built-in orchestrator option appears before the custom client in the markup.
    expect(html.indexOf("Local (Devboule)")).toBeLessThan(html.indexOf("DeepSeek"));
  });
});

// Type-level contract: SpawnSelection.client / SpawnLaunchInput.client are plain
// strings, so "orchestrator" threads through the launch pipeline without widening.
// (Compile-time check; the assignment fails tsc if either field were narrowed.)
describe("SpawnPanel launch contract", () => {
  it("accepts orchestrator as a client value in the selection/launch shape", () => {
    const selection: SpawnSelection = {
      projectId: "p1",
      role: "coder",
      model: "",
      taskId: "",
      client: "orchestrator",
    };
    const input: SpawnLaunchInput = {
      projectId: "p1",
      role: "coder",
      client: "orchestrator",
      taskId: null,
      host: "app",
      model: null,
    };
    expect(selection.client).toBe("orchestrator");
    expect(input.client).toBe("orchestrator");
  });
});
