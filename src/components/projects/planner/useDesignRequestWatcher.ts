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
          const mode = d.mode ?? 'static';
          try {
            const registryId = await generateAndRegisterDesign({
              mode,
              frame: d.frame,
              prompt: d.prompt,
              context: d.planContext,
              workingFolderPath,
              designName: name,
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
