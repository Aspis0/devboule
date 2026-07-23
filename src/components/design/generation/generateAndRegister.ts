// Shared generate-and-register helper (Phase 3). Decoupled from any React hook or
// MCP round-trip so it can be called from BOTH:
//   - the orchestrator watcher (`useDesignRequestWatcher`) — background, MCP-driven
//   - the user-triggered affordance in `StageDesign` — foreground, direct call
//
// ISOLATION INVARIANT: INTERACTIVE output NEVER reaches the static node pipeline
// (`applyGeneration` / `design_save_project` / DOMPurify). STATIC output NEVER
// calls `design_write_artifact`. The two paths share only the generation transport
// (`startDesignGeneration`), which is provider-blind.

import { invokeBackendCommand } from '../../../context/AppContext';
import { startDesignGeneration, type DesignStreamHandle } from '../useDesignStream';
import { applyGeneration } from './pipeline';
import {
  buildGeneratePrompt,
  buildInteractivePrompt,
  buildRefineFullPrompt,
  buildInteractiveRefinePrompt,
} from './prompt';
import { applyInteractiveGeneration } from './interactivePipeline';
import type { DesignProject, DesignProjectEntry, ArtifactFrameKind } from '../../../types/design';

export interface GenerateAndRegisterOptions {
  /** Output mode. User-trigger default = 'interactive'; absent legacy directives = 'static'. */
  mode: 'static' | 'interactive';
  /** Device frame skin for interactive artifacts. Absent ⇒ default skin (Phase 4 wires skins). */
  frame?: ArtifactFrameKind;
  /** User/orchestrator instruction. */
  prompt: string;
  /** Optional plan context injected as a grounding block in the prompt. */
  context?: string;
  /** Absolute path of the working folder where the design is stored. */
  workingFolderPath: string;
  /** Human-readable name for the registry entry (capped at 60 chars by callers). */
  designName: string;
  /**
   * ITERATION (Phase 8): when set (non-empty), the CURRENT design's markup is injected
   * as the base and the model is asked to REFINE it (apply `prompt` as a change) instead
   * of generating from scratch. The `mode` MUST match the base's kind (the caller resolves
   * it from `design_read_current`). Absent ⇒ fresh generation (current behavior).
   */
  refineBaseMarkup?: string;
}

/**
 * Generate a design (static or interactive) and register it in the design registry.
 * Returns the new registry entry's id. Throws on any failure.
 *
 * Mode-branching contract:
 * - 'static':  buildGeneratePrompt → startDesignGeneration → applyGeneration →
 *   design_save_project → design_registry_remember (kind absent = treated as static).
 * - 'interactive':  buildInteractivePrompt → startDesignGeneration →
 *   applyInteractiveGeneration → design_write_artifact →
 *   design_registry_remember (kind:'interactive', artifactPath:'artifact/index.html').
 */
export async function generateAndRegisterDesign(opts: GenerateAndRegisterOptions): Promise<string> {
  const { mode, frame, prompt, context, workingFolderPath, designName } = opts;
  const refineBase = opts.refineBaseMarkup?.trim();
  const isRefine = !!refineBase;

  // Step 1: Create the project working folder. Both modes use design_create_project so the
  // folder exists when design_write_artifact later calls fs::canonicalize. The static
  // project.json / manifest.json produced for interactive designs are harmless (not read by
  // the artifact renderer).
  const blank = await invokeBackendCommand<DesignProject>('design_create_project', {
    workingFolderPath,
    name: designName,
  });

  // Step 2: Build the mode-appropriate prompt (same transport, different system prompt).
  // Refine variants inject the current markup as the base so the model edits it in place;
  // the mode still selects the isolation-safe pipeline downstream (Step 4).
  let fullPrompt: string;
  if (mode === 'interactive') {
    fullPrompt = isRefine
      ? buildInteractiveRefinePrompt(refineBase!, prompt, { context })
      : buildInteractivePrompt(prompt, { context });
  } else {
    fullPrompt = isRefine
      ? buildRefineFullPrompt(refineBase!, prompt, { context })
      : buildGeneratePrompt(prompt, { context });
  }

  // Step 3: Run the provider-blind design generation stream (same as the static path —
  // mode choice is orthogonal to which backend produced the text).
  // Fix 2: capture the handle so we can dispose() it in finally — prevents the Tauri
  // event-listener from leaking on a hung or never-terminating stream.
  let streamHandle: DesignStreamHandle | null = null;
  const finalText = await new Promise<string>((resolve, reject) => {
    let acc = '';
    startDesignGeneration(
      fullPrompt,
      {
        onText: (t) => { acc = t; },
        onStatus: (s, m) => {
          if (s === 'done') {
            if (acc.trim()) resolve(acc);
            else reject(new Error('the designer returned no content'));
          } else if (s === 'error') reject(new Error(m ?? 'generation failed'));
          else if (s === 'cancelled') reject(new Error('cancelled'));
        },
      },
      undefined,
      workingFolderPath,
    ).then((h) => { streamHandle = h; }).catch(reject);
  }).finally(() => {
    streamHandle?.dispose();
  });

  // Step 4: Apply the appropriate pipeline and persist.
  if (mode === 'interactive') {
    // INTERACTIVE: neutralize + wrap if needed → store artifact/index.html.
    // NEVER touches applyGeneration / design_save_project / DOMPurify.
    const { html, warnings, neutralizedCount } = applyInteractiveGeneration(finalText);
    // Fix 4: surface pipeline warnings (blank artifact, many neutralized remote refs, etc.)
    // as console diagnostics so they are visible in devtools without changing the return type.
    if (warnings.length > 0) {
      console.warn('[design] interactive artifact warnings:', warnings, 'neutralizedCount:', neutralizedCount);
    }
    await invokeBackendCommand('design_write_artifact', { workingFolderPath, html });
  } else {
    // STATIC: re-anchor / stamp ids / sanitize via DOMPurify → write component files.
    // NEVER calls design_write_artifact.
    const { project: next } = applyGeneration(blank, finalText);
    await invokeBackendCommand('design_save_project', { workingFolderPath, project: next });
  }

  // Step 5: Register in the design registry. kind/artifactPath/frame are included only for
  // interactive artifacts; absent ⇒ treated as 'static' by the registry (backward compat
  // with all existing entries).
  //
  // Fix 1: Use blank.meta.id (the id the Rust backend assigned to this project) as the
  // client-chosen id passed to design_registry_remember. This sidesteps the symlink
  // canonicalization mismatch: a path-based lookup fails when macOS resolves /tmp →
  // /private/tmp (Rust fs::canonicalize) while the TS side doesn't resolve symlinks. An
  // exact id match is immune to path divergence.
  const registryEntry = {
    id: blank.meta.id,
    name: blank.meta.name,
    workingFolderPath: workingFolderPath.trim(),
    createdAt: '',
    updatedAt: '',
    lastOpenedAt: '',
    ...(mode === 'interactive' && {
      kind: 'interactive' as const,
      artifactPath: 'artifact/index.html',
      ...(frame != null && { frame }),
    }),
  };
  const list = await invokeBackendCommand<DesignProjectEntry[]>('design_registry_remember', {
    entry: registryEntry,
  });

  const found = (list ?? []).find((e) => e.id === blank.meta.id);
  if (!found) throw new Error('Design project not found in registry after registration');
  return found.id;
}
