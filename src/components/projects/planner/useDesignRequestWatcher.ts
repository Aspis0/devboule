import { useEffect, useRef } from 'react';
import { invokeBackendCommand } from '../../../context/AppContext';
import { generateAndRegisterDesign } from '../../design/generation/generateAndRegister';
import type { ArtifactFrameKind } from '../../../types/design';

// Mirror of the Rust `DesignRequestDirective` (camelCase). Only the fields the watcher
// reads are declared here; extra Rust-side fields are safely ignored by TS.
interface DesignRequestDirective {
  id: string;
  parentAgentId: string;
  status: string;
  prompt: string;
  planContext?: string;
  /** Phase 3: output mode. Absent on legacy directives ⇒ treated as 'static'. */
  mode?: 'static' | 'interactive';
  /** Phase 3: device frame skin for interactive artifacts. */
  frame?: ArtifactFrameKind;
  /** Phase 8 (iterate): refine the design with this registry id (from a prior result). */
  refineFrom?: string;
  /** Phase 8 (iterate): refine the project's CURRENT design. Ignored when refineFrom is set. */
  refine?: boolean;
}

// Subset of the Rust `DesignReadMarkupResult` the refine path reads.
interface DesignReadMarkupResult {
  registryId: string;
  designProjectPath: string;
  name: string;
  markup: string;
  truncated: boolean;
  kind?: 'static' | 'interactive';
}

export function useDesignRequestWatcher(
  orchestratorAgentId: string | null,
  projectRoot: string | null,
  onCompleted: () => void
): void {
  const onCompletedRef = useRef(onCompleted);
  onCompletedRef.current = onCompleted;

  const processingRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (!orchestratorAgentId || !projectRoot) return;
    // Set on unmount: the in-flight generation cannot be cancelled (it should still finish
    // + record the design in the backend), but we must NOT call onCompleted (setState) after
    // unmount. The design surfaces on the next mount via the registry.
    let aborted = false;

    const tick = async () => {
      try {
        const pending = await invokeBackendCommand<DesignRequestDirective[]>('list_pending_design_requests');
        for (const d of pending) {
          if (d.parentAgentId !== orchestratorAgentId || processingRef.current.has(d.id)) {
            continue;
          }
          processingRef.current.add(d.id);
          const claimed = await invokeBackendCommand<DesignRequestDirective | null>('design_request_claim', { directiveId: d.id });
          if (!claimed) {
            processingRef.current.delete(d.id);
            continue;
          }

          // Absolute folder for generation + registry (on-disk writes need a real path).
          const workingFolderPath = `${projectRoot}/.aspis-design/${d.id}`;
          // Outcome path must be RELATIVE to the project root: `validate_design_outcome_path`
          // rejects absolute paths (security: stored outcome must stay under the project).
          // Passing the absolute `workingFolderPath` here used to fail validation → complete
          // was swallowed → directive never Done → orchestrator MCP poll timed out (-32001).
          const designProjectPath = `.aspis-design/${d.id}`;
          const name = (d.prompt || 'Design').slice(0, 60);
          // Legacy directives without a `mode` field default to 'static' (backward compat).
          let mode = d.mode ?? 'static';

          // Phase 8 (iterate): when the orchestrator asked to refine, `refineFrom` pins a
          // SPECIFIC registry id; a bare `refine:true` targets the project's current design.
          const pinnedRefineId = typeof d.refineFrom === 'string' && d.refineFrom.length > 0 ? d.refineFrom : null;

          try {
            // Resolve the refine base INSIDE this try so any hard error (bad/foreign refineFrom
            // id, empty markup) flows into `design_request_complete(error)` below — the
            // orchestrator's MCP poll then returns a clear message instead of timing out.
            // Load the base design's current markup so the model edits it in place. The base
            // KIND wins over the directive `mode` (the refine prompt MUST match the base markup —
            // an interactive prompt fed static fragments produces garbage), defaulting to
            // 'static' when the base kind is unknown/absent (legacy static designs record no kind).
            let refineBaseMarkup: string | undefined;
            if (pinnedRefineId || d.refine === true) {
              let base: DesignReadMarkupResult | null = null;
              try {
                base = await invokeBackendCommand<DesignReadMarkupResult>('design_read_current', {
                  projectRoot,
                  registryId: pinnedRefineId,
                });
              } catch (e) {
                // A PINNED refineFrom that can't be resolved is a hard error: the orchestrator
                // named a specific design (possibly a bad id, or one outside this project — the
                // backend confines by project root), so silently regenerating would mislead it.
                // A bare `refine:true` with no current design is fine → fall through to fresh.
                if (pinnedRefineId) {
                  throw new Error(
                    `refine target '${pinnedRefineId}' could not be read: ${e instanceof Error ? e.message : String(e)}`,
                  );
                }
                console.warn('[design] refine=current requested but no base design yet; generating fresh:', e);
              }
              if (base?.markup?.trim()) {
                refineBaseMarkup = base.markup;
                // Default unknown/absent kind to 'static' (see contract above).
                mode = base.kind === 'interactive' ? 'interactive' : 'static';
              } else if (pinnedRefineId) {
                // Pinned target resolved but has empty markup — nothing to iterate on. Fail loud.
                throw new Error(`refine target '${pinnedRefineId}' has no readable markup to iterate on`);
              }
            }

            const registryId = await generateAndRegisterDesign({
              mode,
              frame: d.frame,
              prompt: d.prompt,
              context: d.planContext,
              workingFolderPath,
              designName: name,
              refineBaseMarkup,
            });
            // The design is SAVED + REGISTERED now. Stamp the directive done (best-effort:
            // may race with the Python timeout write-back — harmless) and ALWAYS refresh the
            // Stage, because the design exists in the registry regardless of the directive.
            await invokeBackendCommand('design_request_complete', {
              directiveId: d.id,
              designProjectPath,
              registryId,
              error: null,
            }).catch(() => {});
            if (!aborted) onCompletedRef.current();
          } catch (e) {
            await invokeBackendCommand('design_request_complete', {
              directiveId: d.id,
              designProjectPath: null,
              registryId: null,
              error: e instanceof Error ? e.message : String(e),
            }).catch(() => {});
          }
        }
      } catch (err) {
        console.error('Design request watcher tick failed:', err);
      }
    };

    const intervalId = setInterval(tick, 3000);

    return () => {
      aborted = true;
      clearInterval(intervalId);
    };
  }, [orchestratorAgentId, projectRoot]);
}
