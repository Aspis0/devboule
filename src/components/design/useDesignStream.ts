// useDesignStream — frontend transport for the design-LLM generation (Phase 2 STEP 2).
//
// Pairs with the Rust `design_generate` / `design_cancel_generation` commands and their
// per-genId `design-stream:<genId>` event channel (`DesignStreamEvent` tagged on `type`).
// This is the TRANSPORT ONLY: it accumulates the raw streamed model TEXT and surfaces it
// (plus a status) to the caller. There is NO parsing/sanitize/inject/canvas wiring here —
// that is a later step. DesignView renders the accumulated text in a plain panel.
//
// Concurrency correctness (the load-bearing parts):
//   - SUBSCRIBE BEFORE INVOKE. We `listen()` on the channel and await the subscription
//     BEFORE calling `design_generate`, so a delta that arrives immediately after the
//     backend starts cannot be missed (the race the plan calls out, risk #4 family).
//   - ALWAYS UNLISTEN. The Tauri `unlisten` handle is invoked exactly once on any terminal
//     event (done/error/cancelled) AND on dispose/unmount, so no listener leaks across
//     re-runs or component unmount.
//   - cancel() invokes `design_cancel_generation(genId)`; the terminal `cancelled` event
//     then arrives over the same channel and triggers the normal cleanup.

import { useCallback, useEffect, useRef, useState } from "react";

/** The transport status surfaced to the caller. */
export type DesignStreamStatus =
  | "idle"
  | "streaming"
  | "done"
  | "error"
  | "cancelled";

/** The tagged event shape emitted by the Rust transport (`type` discriminator). */
export type DesignStreamEvent =
  | { type: "delta"; text: string }
  | { type: "done" }
  | { type: "error"; message: string }
  | { type: "cancelled" };

/** Callbacks the controller drives as the stream progresses. */
export interface DesignStreamCallbacks {
  /** Fired on every delta with the FULL accumulated text so far (not just the chunk). */
  onText?: (accumulated: string) => void;
  /** Fired once on any terminal transition. `message` is set only for `error`. */
  onStatus?: (status: DesignStreamStatus, message?: string) => void;
}

/** The handle returned by `startDesignGeneration`. */
export interface DesignStreamHandle {
  /** The generated genId this run streams on. */
  genId: string;
  /** Request cancellation. Idempotent; safe after the stream already ended. */
  cancel: () => void;
  /** Tear down the subscription WITHOUT cancelling (used by unmount after terminal). */
  dispose: () => void;
}

/**
 * Minimal Tauri surface the controller needs, injectable so vitest can drive it without
 * a real Tauri runtime. The defaults dynamically import the real `@tauri-apps/api`.
 */
export interface DesignStreamDeps {
  /** Subscribe to `channel`; resolves to an unlisten fn. */
  listen: (
    channel: string,
    handler: (event: { payload: DesignStreamEvent }) => void,
  ) => Promise<() => void>;
  /** Invoke a backend command. */
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  /** Generate a unique genId. */
  newId: () => string;
}

function defaultNewId(): string {
  // crypto.randomUUID is available in the Tauri webview and modern browsers.
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  // Fallback (older runtimes / tests without crypto): time + random, collision-safe
  // enough for a per-session channel name.
  return `gen-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

const defaultDeps: DesignStreamDeps = {
  listen: async (channel, handler) => {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<DesignStreamEvent>(channel, (event) =>
      handler({ payload: event.payload }),
    );
  },
  invoke: async (command, args) => {
    const { invokeBackendCommand } = await import("../../context/AppContext");
    return invokeBackendCommand<unknown>(command, args);
  },
  newId: defaultNewId,
};

/** The channel name for a genId — must match the Rust `design_stream_channel`. */
export function designStreamChannel(genId: string): string {
  return `design-stream:${genId}`;
}

/**
 * Start a design generation. Subscribes to the genId's channel BEFORE invoking the
 * backend, accumulates delta text, drives the callbacks, and ALWAYS unlistens on the
 * terminal event. Returns a handle exposing the genId, `cancel()`, and `dispose()`.
 *
 * The returned promise resolves once the subscription is established and the backend
 * invoke has been dispatched — NOT when the stream ends (the stream is observed via the
 * callbacks). If subscription/invoke fails, the promise resolves after firing an `error`
 * status (it never rejects, so callers don't need a try/catch around startup).
 */
export async function startDesignGeneration(
  prompt: string,
  callbacks: DesignStreamCallbacks = {},
  deps: DesignStreamDeps = defaultDeps,
  // The design project's working folder, forwarded to the backend so a CLI provider
  // (codex/claude) runs in that directory (a trusted, real context). Optional +
  // best-effort: an empty/undefined value means "no cwd override" backend-side.
  workingFolderPath?: string,
  // W3: the caller MAY pre-generate the genId (at the synchronous start() callsite) so a
  // cancel() arriving DURING this async startup can already address the right backend
  // generation via design_cancel_generation(genId), before the handle resolves. When
  // omitted we generate one here (back-compat for direct callers).
  presetGenId?: string,
): Promise<DesignStreamHandle> {
  const genId = presetGenId ?? deps.newId();
  const channel = designStreamChannel(genId);

  let accumulated = "";
  let unlisten: (() => void) | null = null;
  // Guards: the terminal handler + dispose must each run their cleanup at most once.
  let settled = false;
  // The "streaming" status must fire EXACTLY ONCE per run, and never after dispose/settle.
  let streamingEmitted = false;

  const teardown = () => {
    if (unlisten) {
      const fn = unlisten;
      unlisten = null;
      try {
        fn();
      } catch {
        // An unlisten that throws (already-disposed runtime) is non-fatal.
      }
    }
  };

  const settle = (status: DesignStreamStatus, message?: string) => {
    if (settled) return;
    settled = true;
    teardown();
    callbacks.onStatus?.(status, message);
  };

  const handler = (event: { payload: DesignStreamEvent }) => {
    if (settled) return; // ignore any late event after terminal/dispose
    const payload = event.payload;
    switch (payload.type) {
      case "delta":
        accumulated += payload.text;
        callbacks.onText?.(accumulated);
        break;
      case "done":
        settle("done");
        break;
      case "error":
        settle("error", payload.message);
        break;
      case "cancelled":
        settle("cancelled");
        break;
    }
  };

  // SUBSCRIBE FIRST so no early delta is lost.
  try {
    unlisten = await deps.listen(channel, handler);
  } catch (e) {
    settle("error", String(e));
    return {
      genId,
      cancel: () => {},
      dispose: teardown,
    };
  }

  // If the caller already disposed/settled during the listen await, bail without invoking
  // or emitting a status (dispose marks `settled` so a late streaming emit is suppressed).
  if (settled) {
    return {
      genId,
      cancel: () => {},
      dispose: teardown,
    };
  }

  // Emit "streaming" exactly once, and only while still live.
  if (!streamingEmitted) {
    streamingEmitted = true;
    callbacks.onStatus?.("streaming");
  }

  // THEN invoke. A failed invoke surfaces as an error status + cleanup. `workingFolderPath`
  // is sent only when non-empty (camelCase IPC mirrors the Rust `working_folder_path` arg).
  const invokeArgs: Record<string, unknown> = { genId, prompt };
  const folder = workingFolderPath?.trim();
  if (folder) {
    invokeArgs.workingFolderPath = folder;
  }
  deps.invoke("design_generate", invokeArgs).catch((e) => {
    settle("error", String(e));
  });

  const cancel = () => {
    // Fire-and-forget; the backend emits the terminal `cancelled` event which settles us.
    // If the stream already settled, this is a harmless no-op on the backend side.
    deps.invoke("design_cancel_generation", { genId }).catch(() => {
      // A cancel that races the natural end is fine; ignore.
    });
  };

  const dispose = () => {
    // Tear down WITHOUT emitting a status (caller is unmounting). Mark settled so any
    // in-flight late event is ignored.
    settled = true;
    teardown();
  };

  return { genId, cancel, dispose };
}

/** The reactive state a React caller consumes. */
export interface UseDesignStreamState {
  text: string;
  status: DesignStreamStatus;
  error: string | null;
  start: (prompt: string, workingFolderPath?: string) => void;
  cancel: () => void;
  reset: () => void;
}

/**
 * React hook wrapping [`startDesignGeneration`]. Owns the accumulated text + status as
 * component state, cancels any in-flight generation when a new one starts, and disposes
 * the subscription on unmount. `deps` is injectable for tests.
 */
export function useDesignStream(deps?: DesignStreamDeps): UseDesignStreamState {
  const [text, setText] = useState("");
  const [status, setStatus] = useState<DesignStreamStatus>("idle");
  const [error, setError] = useState<string | null>(null);

  // The live handle of the current generation, so start/cancel/unmount can reach it.
  const handleRef = useRef<DesignStreamHandle | null>(null);
  // W3: the genId of the CURRENT run, captured SYNCHRONOUSLY at start() — BEFORE the
  // async startup resolves a handle. This lets cancel() reach the backend generation
  // (design_cancel_generation) even during the pre-handle window. Cleared on terminal/
  // reset. The deps object owns id generation so tests can inject deterministic ids.
  const pendingGenIdRef = useRef<string | null>(null);
  // Bumped on every start so a late callback from a SUPERSEDED run is ignored.
  const runIdRef = useRef(0);
  const mountedRef = useRef(true);
  // The deps in a ref so the synchronous cancel() can invoke design_cancel_generation
  // with the pending genId without depending on (or capturing a stale) deps closure.
  const depsRef = useRef<DesignStreamDeps | undefined>(deps);
  depsRef.current = deps;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      handleRef.current?.dispose();
      handleRef.current = null;
    };
  }, []);

  const start = useCallback(
    (prompt: string, workingFolderPath?: string) => {
      // Supersede any in-flight run: CANCEL it first (so the backend generation is told to
      // stop and frees its GenGuard, not just silenced), THEN dispose its subscription.
      const prev = handleRef.current;
      if (prev) {
        prev.cancel();
        prev.dispose();
      }
      handleRef.current = null;

      const myRun = ++runIdRef.current;
      setText("");
      setError(null);
      // W3: generate the genId NOW (synchronously) so cancel() during the pre-handle
      // startup window can already address the backend generation. The effective deps'
      // newId owns generation (tests inject deterministic ids).
      const effectiveDeps = depsRef.current ?? defaultDeps;
      const genId = effectiveDeps.newId();
      pendingGenIdRef.current = genId;
      // "streaming" is driven by the onStatus callback below (exactly one transition per
      // run); we do NOT pre-set it here to avoid a double "streaming" emission.

      void startDesignGeneration(
        prompt,
        {
          onText: (accumulated) => {
            if (!mountedRef.current || runIdRef.current !== myRun) return;
            setText(accumulated);
          },
          onStatus: (s, message) => {
            if (!mountedRef.current || runIdRef.current !== myRun) return;
            // A terminal status clears the pending genId for THIS run (a superseding
            // run will have already overwritten it, so only clear when still ours).
            if (s !== "streaming" && pendingGenIdRef.current === genId) {
              pendingGenIdRef.current = null;
            }
            setStatus(s);
            setError(s === "error" ? message ?? "Generation failed." : null);
          },
        },
        deps,
        workingFolderPath,
        genId,
      ).then((handle) => {
        // If a newer run started (or we unmounted) while awaiting startup, dispose this
        // stale handle immediately so it never leaks a listener.
        if (!mountedRef.current || runIdRef.current !== myRun) {
          handle.dispose();
          return;
        }
        handleRef.current = handle;
      });
    },
    [deps],
  );

  const cancel = useCallback(() => {
    const handle = handleRef.current;
    if (handle) {
      handle.cancel();
      return;
    }
    // W3: no handle yet (cancel during the pre-handle startup window) — address the
    // backend generation directly by the synchronously-captured genId so the in-flight
    // backend run is actually cancelled, not dropped.
    const genId = pendingGenIdRef.current;
    if (genId) {
      const d = depsRef.current ?? defaultDeps;
      d.invoke("design_cancel_generation", { genId }).catch(() => {
        // Racing the natural end / not-yet-registered genId is fine; ignore.
      });
    }
  }, []);

  const reset = useCallback(() => {
    // CANCEL before dispose so an in-flight backend generation is actually stopped (frees
    // its GenGuard), not merely silenced.
    const prev = handleRef.current;
    if (prev) {
      prev.cancel();
      prev.dispose();
    }
    handleRef.current = null;
    pendingGenIdRef.current = null;
    runIdRef.current++;
    setText("");
    setError(null);
    setStatus("idle");
  }, []);

  return { text, status, error, start, cancel, reset };
}
