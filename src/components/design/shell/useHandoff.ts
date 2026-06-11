// useHandoff — Phase D orchestration for the design "Save & hand off to agents"
// flow. Keeps HandoffModal PRESENTATIONAL: this hook owns the phase machine, the
// per-step packaging rows, the project/client selection, the SINGLE dispatch, and
// the deep-link out. DesignView wires it with the real callbacks (runConsolidate /
// runExport / design_read_design_md / design_preview_capture / invoke) and the app's
// project list.
//
// Single-dispatch design: ONE "Coder agent" task row (not the prototype's 5 mock
// rows). The packaging phase runs the deterministic save/export/contract/capture
// steps; the dispatch phase fires exactly one launch_project_agent_terminal with a
// designHandoff payload; the done phase offers an "Open terminal" deep-link.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { DesignLlmBackendKind } from "../../../types/config";
import type { ProjectAgentLaunchResult, ProjectSummary } from "../../../types/backend";

/** Coding agent CLI the dispatch launches. Mirrors the backend's built-in clients. */
export type HandoffClient = "claude" | "codex";

/** A packaging step's lifecycle. "skipped" is a NON-blocking miss (e.g. no preview
 *  window to capture, or no design.md); "warn" is a non-blocking advisory that still
 *  let the flow continue. "error" is a hard stop with a Retry. */
export type HandoffStepStatus =
  | "idle"
  | "running"
  | "done"
  | "skipped"
  | "warn"
  | "error";

/** The icon a row shows when idle (lucide name resolved in the modal). */
export type HandoffStepIcon = "save" | "code" | "fileText" | "camera" | "cpu";

export interface HandoffStep {
  id: HandoffStepKind;
  label: string;
  /** Sub-line detail, updated as the step runs/finishes. */
  detail: string;
  status: HandoffStepStatus;
  icon: HandoffStepIcon;
  /** Right-side badge ("design" while packaging, "coder agent" for the dispatch row). */
  agent: string;
}

export type HandoffStepKind =
  | "save"
  | "export"
  | "contract"
  | "capture"
  | "dispatch";

export type HandoffPhase = "packaging" | "dispatch" | "done";

/** The flow's three big steps shown in the ho-flow wire (Design -> Repo -> Agents). */
export interface HandoffFlowState {
  repoDone: boolean;
  agentsStarted: boolean;
  done: boolean;
}

export interface UseHandoffArgs {
  /** The design bundle's working folder (folderRef.current). Empty => cannot open. */
  workingFolderPath: string;
  /** App projects (management plane) the dispatch can target. */
  projects: ProjectSummary[];
  /** The configured design CLI backend kind — defaults the client picker. */
  backendKind: DesignLlmBackendKind | string | null;
  /** Persist the whole design project to disk (DesignView.runConsolidate).
   *  Resolves true on a successful save, false on any failure — packaging HARD-STOPS
   *  the save step when this is false (a failed save must not dispatch a stale bundle). */
  runConsolidate: () => Promise<boolean>;
  /** Export one layout mode; resolves true on a successful write. */
  runExport: (mode: "absolute" | "flow") => Promise<boolean>;
  /** Tauri invoke bridge (design_read_design_md / design_preview_capture / launch). */
  invoke: <T = unknown>(
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<T>;
  /** Deep-link into a project's Work-mode terminal (requestView + work:<id>). */
  onOpenTerminal: (projectId: string) => void;
}

export interface UseHandoffState {
  open: boolean;
  phase: HandoffPhase;
  steps: HandoffStep[];
  flow: HandoffFlowState;
  /** Projects offered in the dispatch selector. */
  projects: ProjectSummary[];
  selectedProjectId: string | null;
  client: HandoffClient;
  /** Set once dispatch fires — the agent row shows this as its name. */
  agentId: string | null;
  /** A hard error to surface on the active stage's row (packaging or dispatch). */
  errorStage: "packaging" | "dispatch" | null;
  errorMessage: string | null;
  /** True from the moment dispatch fires until done (re-entry guard). */
  dispatching: boolean;
  /** Whether dispatch can fire: a project is selected, packaging done, not already fired. */
  canDispatch: boolean;
  /** Whether the scrim may close the modal (never mid-dispatch). */
  closable: boolean;
}

export interface UseHandoff extends UseHandoffState {
  openHandoff: () => void;
  close: () => void;
  selectProject: (projectId: string) => void;
  selectClient: (client: HandoffClient) => void;
  /** Run / re-run the packaging sequence. */
  runPackaging: () => void;
  /** Fire the single dispatch. No-op if already dispatched or not ready. */
  dispatch: () => void;
  /** Deep-link to the dispatched agent's Work-mode terminal. */
  openTerminal: () => void;
}

const PACKAGING_STEPS: ReadonlyArray<Omit<HandoffStep, "status">> = [
  {
    id: "save",
    label: "Save to repo",
    detail: "manifest.json + components/",
    icon: "save",
    agent: "design",
  },
  {
    id: "export",
    label: "Export layouts",
    detail: "export-absolute.html + export-flow.html",
    icon: "code",
    agent: "design",
  },
  {
    id: "contract",
    label: "Design contract",
    detail: "design.md",
    icon: "fileText",
    agent: "design",
  },
  {
    id: "capture",
    label: "Capture preview",
    detail: "preview.png",
    icon: "camera",
    agent: "design",
  },
];

const DISPATCH_STEP: Omit<HandoffStep, "status"> = {
  id: "dispatch",
  label: "Coder agent",
  detail: "implement the design in the repo",
  icon: "cpu",
  agent: "coder agent",
};

/** Default the client picker from the configured design backend when it is itself a
 *  CLI client (claude/codex); otherwise claude (the dispatch only supports those two). */
export function defaultClientFromBackend(
  backendKind: DesignLlmBackendKind | string | null,
): HandoffClient {
  return backendKind === "codex" ? "codex" : "claude";
}

/** The management project whose rootPath is a prefix of the design working folder, if
 *  any. Both are compared case-insensitively with normalized separators so a Windows
 *  drive-letter / slash mismatch does not defeat the match. Longest match wins so a
 *  nested project root is preferred over an ancestor. */
export function preselectProjectId(
  workingFolderPath: string,
  projects: ProjectSummary[],
): string | null {
  const folder = normalizePath(workingFolderPath);
  if (!folder) return null;
  let best: { id: string; len: number } | null = null;
  for (const p of projects) {
    const root = normalizePath(p.rootPath ?? "");
    if (!root) continue;
    if (folder === root || folder.startsWith(root + "/")) {
      if (!best || root.length > best.len) best = { id: p.id, len: root.length };
    }
  }
  return best?.id ?? null;
}

function normalizePath(value: string): string {
  return value.trim().replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

export function useHandoff(args: UseHandoffArgs): UseHandoff {
  const {
    workingFolderPath,
    projects,
    backendKind,
    runConsolidate,
    runExport,
    invoke,
    onOpenTerminal,
  } = args;

  const [open, setOpen] = useState(false);
  const [phase, setPhase] = useState<HandoffPhase>("packaging");
  const [steps, setSteps] = useState<HandoffStep[]>(() => initialSteps());
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [client, setClient] = useState<HandoffClient>(() =>
    defaultClientFromBackend(backendKind),
  );
  const [agentId, setAgentId] = useState<string | null>(null);
  const [errorStage, setErrorStage] = useState<"packaging" | "dispatch" | null>(
    null,
  );
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [dispatching, setDispatching] = useState(false);

  // Re-entry / unmount guards. `dispatchedRef` permanently latches once a dispatch
  // fires so the button can never double-fire (a second click is a no-op). `aliveRef`
  // stops state writes after the modal closes / the component unmounts.
  const dispatchedRef = useRef(false);
  const packagingRunningRef = useRef(false);
  // Generation counter bumped on every (re)open. A packaging run captures its epoch and
  // drops ALL state writes once it no longer matches — so a stale run still in flight
  // after a close -> reopen can never apply its patches over the fresh run (Fix 2).
  const packagingEpochRef = useRef(0);
  const aliveRef = useRef(true);
  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  // When the project list lands AFTER the modal opened (DesignView loads it async on
  // open), preselect by rootPath prefix if nothing is selected yet and the user has not
  // dispatched. This never overrides a user's explicit pick (only fills a null) and is
  // a no-op once dispatched.
  useEffect(() => {
    if (!open || dispatchedRef.current) return;
    if (selectedProjectId !== null) return;
    const pre = preselectProjectId(workingFolderPath, projects);
    if (pre) setSelectedProjectId(pre);
  }, [open, selectedProjectId, workingFolderPath, projects]);

  const patchStep = useCallback(
    (id: HandoffStepKind, patch: Partial<HandoffStep>) => {
      if (!aliveRef.current) return;
      setSteps((prev) =>
        prev.map((s) => (s.id === id ? { ...s, ...patch } : s)),
      );
    },
    [],
  );

  // --- packaging sequence ----------------------------------------------------
  const runPackaging = useCallback(() => {
    if (packagingRunningRef.current) return;
    packagingRunningRef.current = true;
    // Capture this run's generation. A close -> reopen bumps the epoch (in openHandoff);
    // any write below from a now-superseded run is dropped by `current()` so a stale run
    // can never patch over the fresh one (Fix 2).
    const myEpoch = packagingEpochRef.current;
    const current = () => aliveRef.current && packagingEpochRef.current === myEpoch;
    const safePatch = (id: HandoffStepKind, patch: Partial<HandoffStep>) => {
      if (!current()) return;
      patchStep(id, patch);
    };
    const folder = workingFolderPath.trim();
    setErrorStage(null);
    setErrorMessage(null);
    setPhase("packaging");

    void (async () => {
      try {
        if (!folder) {
          throw new Error("Choose a working folder before handing off.");
        }

        // 1) Save to repo (consolidate the whole project). runConsolidate resolves
        //    false on any save failure — a swallowed error must NOT mark this step done
        //    and let a stale/incomplete bundle dispatch (BLOCKER).
        safePatch("save", { status: "running" });
        const saved = await runConsolidate();
        // If a close -> reopen superseded this run while the save was in flight, stop
        // here: don't error/patch and don't run any further side-effecting steps (Fix 2).
        if (!current()) return;
        if (!saved) {
          safePatch("save", { status: "error" });
          throw new Error("Save failed — could not consolidate the project to disk.");
        }
        safePatch("save", { status: "done" });

        // 2) Export BOTH layout modes — both must succeed (the bundle is incomplete
        //    otherwise). runExport resolves false on any write failure.
        safePatch("export", { status: "running" });
        const abs = await runExport("absolute");
        const flow = abs ? await runExport("flow") : false;
        if (!current()) return;
        if (!abs || !flow) {
          safePatch("export", { status: "error" });
          throw new Error("Export failed — could not write both layout files.");
        }
        safePatch("export", { status: "done" });

        // 3) Ensure design.md exists. Missing is NON-blocking (a warning row): agents
        //    can still infer style, so the flow continues.
        safePatch("contract", { status: "running" });
        let contract = "";
        try {
          contract = await invoke<string>("design_read_design_md", {
            workingFolderPath: folder,
          });
        } catch {
          contract = "";
        }
        if (contract.trim().length > 0) {
          safePatch("contract", { status: "done" });
        } else {
          safePatch("contract", {
            status: "warn",
            detail: "No design contract — agents will infer style",
          });
        }

        // 4) Best-effort preview capture. Failure (no preview window open) is
        //    NON-blocking: the row shows skipped and the flow continues.
        safePatch("capture", { status: "running" });
        try {
          await invoke("design_preview_capture", { workingFolderPath: folder });
          safePatch("capture", { status: "done" });
        } catch {
          safePatch("capture", {
            status: "skipped",
            detail: "No preview window — skipped",
          });
        }

        if (!current()) return;
        setPhase("dispatch");
      } catch (e) {
        if (!current()) return;
        setErrorStage("packaging");
        setErrorMessage(e instanceof Error ? e.message : String(e));
      } finally {
        // Only the run that still owns the epoch releases the lock — a superseded run
        // must not clear the flag the fresh run set.
        if (packagingEpochRef.current === myEpoch) {
          packagingRunningRef.current = false;
        }
      }
    })();
  }, [workingFolderPath, runConsolidate, runExport, invoke, patchStep]);

  // --- open / close ----------------------------------------------------------
  const openHandoff = useCallback(() => {
    // Reopen guard: if a modal is already open (its packaging may still be in flight),
    // do NOT start a second concurrent run — the in-flight one keeps the screen (Fix 2).
    if (open) return;
    // Bump the generation so any packaging run still in flight from a PRIOR open is
    // superseded and its late patches are dropped (it no longer owns the epoch).
    packagingEpochRef.current += 1;
    // Reset the whole machine for a fresh run.
    dispatchedRef.current = false;
    packagingRunningRef.current = false;
    setSteps(initialSteps());
    setAgentId(null);
    setDispatching(false);
    setErrorStage(null);
    setErrorMessage(null);
    setPhase("packaging");
    setClient(defaultClientFromBackend(backendKind));
    setSelectedProjectId(preselectProjectId(workingFolderPath, projects));
    setOpen(true);
    runPackaging();
  }, [open, backendKind, workingFolderPath, projects, runPackaging]);

  const close = useCallback(() => {
    // Never close mid-dispatch (the launch is in flight). The scrim/closable guard
    // already prevents this, but re-check here defensively.
    if (dispatching) return;
    setOpen(false);
  }, [dispatching]);

  const selectProject = useCallback((projectId: string) => {
    setSelectedProjectId(projectId);
  }, []);

  const selectClient = useCallback((next: HandoffClient) => {
    setClient(next);
  }, []);

  // --- dispatch --------------------------------------------------------------
  const canDispatch =
    phase === "dispatch" &&
    !dispatchedRef.current &&
    !dispatching &&
    selectedProjectId !== null;

  const dispatch = useCallback(() => {
    // Permanent latch + re-entry guard: a second click after the first fire is a no-op.
    if (dispatchedRef.current || dispatching) return;
    if (phase !== "dispatch") return;
    const projectId = selectedProjectId;
    if (!projectId) {
      setErrorStage("dispatch");
      setErrorMessage("Pick a project to hand off to.");
      return;
    }
    const folder = workingFolderPath.trim();
    if (!folder) {
      setErrorStage("dispatch");
      setErrorMessage("The design working folder is no longer available.");
      return;
    }

    dispatchedRef.current = true;
    setDispatching(true);
    setErrorStage(null);
    setErrorMessage(null);
    // Collision-resistant id: Date.now() alone can repeat within the same millisecond
    // (two dispatches across modal sessions) — a random suffix disambiguates (Fix 4).
    const newAgentId = `coder-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    patchStep("dispatch", { status: "running" });

    void (async () => {
      try {
        const result = await invoke<ProjectAgentLaunchResult>(
          "launch_project_agent_terminal",
          {
            input: {
              projectId,
              role: "coder",
              client,
              host: "app",
              agentId: newAgentId,
              designHandoff: { workingFolderPath: folder },
            },
          },
        );
        if (!aliveRef.current) return;
        setAgentId(result.agentId || newAgentId);
        patchStep("dispatch", {
          status: "done",
          detail: `${result.agentId || newAgentId} · running in your repo`,
        });
        setDispatching(false);
        setPhase("done");
      } catch (e) {
        if (!aliveRef.current) return;
        // A failed dispatch un-latches so the user can Retry (the launch never took).
        dispatchedRef.current = false;
        setDispatching(false);
        patchStep("dispatch", { status: "error" });
        setErrorStage("dispatch");
        setErrorMessage(e instanceof Error ? e.message : String(e));
      }
    })();
  }, [
    dispatching,
    phase,
    selectedProjectId,
    workingFolderPath,
    client,
    invoke,
    patchStep,
  ]);

  const openTerminal = useCallback(() => {
    if (!selectedProjectId) return;
    onOpenTerminal(selectedProjectId);
    setOpen(false);
  }, [selectedProjectId, onOpenTerminal]);

  const flow = useMemo<HandoffFlowState>(
    () => ({
      repoDone: phase !== "packaging",
      agentsStarted: dispatching || phase === "done",
      done: phase === "done",
    }),
    [phase, dispatching],
  );

  const closable = !dispatching;

  return {
    open,
    phase,
    steps,
    flow,
    projects,
    selectedProjectId,
    client,
    agentId,
    errorStage,
    errorMessage,
    dispatching,
    canDispatch,
    closable,
    openHandoff,
    close,
    selectProject,
    selectClient,
    runPackaging,
    dispatch,
    openTerminal,
  };
}

function initialSteps(): HandoffStep[] {
  return [
    ...PACKAGING_STEPS.map((s) => ({ ...s, status: "idle" as HandoffStepStatus })),
    { ...DISPATCH_STEP, status: "idle" as HandoffStepStatus },
  ];
}
