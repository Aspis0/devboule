import { useEffect, useRef } from 'react';
import { invokeBackendCommand } from '../../../context/AppContext';
import { startDesignGeneration } from '../../design/useDesignStream';
import { applyGeneration } from '../../design/generation/pipeline';
import { buildGeneratePrompt } from '../../design/generation/prompt';
import type { DesignProject, DesignProjectEntry } from '../../../types/design';

interface DesignRequestDirective {
  id: string;
  parentAgentId: string;
  status: string;
  prompt: string;
  planContext?: string;
}

async function generateAndRegisterDesign(
  prompt: string,
  context: string | undefined,
  workingFolderPath: string,
  designName: string
): Promise<string> {
  const blank = await invokeBackendCommand<DesignProject>('design_create_project', { workingFolderPath, name: designName });
  const fullPrompt = buildGeneratePrompt(prompt, { context });
  const finalText = await new Promise<string>((resolve, reject) => {
    let acc = '';
    startDesignGeneration(fullPrompt, {
      onText: t => { acc = t; },
      onStatus: (s, m) => {
        if (s === 'done') {
          if (acc.trim()) resolve(acc);
          else reject(new Error('the designer returned no content'));
        } else if (s === 'error') reject(new Error(m ?? 'generation failed'));
        else if (s === 'cancelled') reject(new Error('cancelled'));
      }
    }, undefined, workingFolderPath).catch(reject);
  });
  const { project: next } = applyGeneration(blank, finalText);
  await invokeBackendCommand('design_save_project', { workingFolderPath, project: next });
  const list = await invokeBackendCommand<DesignProjectEntry[]>('design_registry_remember', {
    entry: { id: '', name: next.meta.name, workingFolderPath: workingFolderPath.trim(), createdAt: '', updatedAt: '', lastOpenedAt: '' }
  });
  const normalizePath = (p: string) => p.trim().replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
  const targetPath = normalizePath(workingFolderPath);
  const found = (list ?? []).find(e => normalizePath(e.workingFolderPath) === targetPath);
  if (!found) throw new Error('Design project not found in registry');
  return found.id;
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

          const workingFolderPath = `${projectRoot}/.aspis-design/${d.id}`;
          const name = (d.prompt || 'Design').slice(0, 60);
          try {
            const registryId = await generateAndRegisterDesign(d.prompt, d.planContext, workingFolderPath, name);
            // The design is SAVED + REGISTERED now. Stamp the directive done (best-effort:
            // may race with the Python timeout write-back — harmless) and ALWAYS refresh the
            // Stage, because the design exists in the registry regardless of the directive.
            await invokeBackendCommand('design_request_complete', { directiveId: d.id, designProjectPath: workingFolderPath, registryId, error: null }).catch(() => {});
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
