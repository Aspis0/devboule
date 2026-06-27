// Tests for the shared generate-and-register helper (Phase 3).
// Verifies mode-branching: interactive calls design_write_artifact + registers
// kind:'interactive'; static calls design_save_project + omits kind. Also verifies
// that the frame field is carried through for interactive artifacts.

import { describe, it, expect, vi, beforeEach } from 'vitest';

// ---------------------------------------------------------------------------
// Module mocks — must appear before the import under test.
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async (..._args: unknown[]) => undefined as unknown);
vi.mock('../../../context/AppContext', () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...args),
}));

// Spy for the dispose handle so Fix 2 can be asserted.
const disposeMock = vi.fn();

// Controllable stub for startDesignGeneration: calls onText + onStatus('done') sync,
// then returns a DesignStreamHandle-shaped object with a dispose spy.
const startDesignGenerationMock = vi.fn(
  async (_prompt: unknown, callbacks: { onText?: (t: string) => void; onStatus?: (s: string, m?: string) => void }) => {
    callbacks.onText?.('<!DOCTYPE html><html><body>stub</body></html>');
    callbacks.onStatus?.('done');
    return { genId: 'test-gen', cancel: vi.fn(), dispose: disposeMock };
  },
);
vi.mock('../useDesignStream', () => ({
  startDesignGeneration: (...args: unknown[]) =>
    (startDesignGenerationMock as (...a: unknown[]) => unknown)(...args),
}));

const applyGenerationMock = vi.fn((_project: unknown, _text: string) => ({
  project: { meta: { name: 'Stub' } },
  newIds: [],
  shapes: {},
  warnings: [],
  remainingViolations: [],
}));
vi.mock('./pipeline', () => ({
  applyGeneration: (...args: unknown[]) =>
    (applyGenerationMock as (...a: unknown[]) => unknown)(...args),
}));

const applyInteractiveMock = vi.fn((_text: unknown): { html: string; warnings: string[]; neutralizedCount: number; wrapped: boolean } => ({
  html: '<html><body>interactive stub</body></html>',
  warnings: [],
  neutralizedCount: 0,
  wrapped: false,
}));
vi.mock('./interactivePipeline', () => ({
  applyInteractiveGeneration: (...args: unknown[]) =>
    (applyInteractiveMock as (...a: unknown[]) => unknown)(...args),
}));

// Prompt builders are pure — we don't need to spy on them, but we mock the module to
// keep the test hermetic (avoids pulling in the large DESIGN_SYSTEM_PROMPT constants).
vi.mock('./prompt', () => ({
  buildGeneratePrompt: (instruction: string) => `static:${instruction}`,
  buildInteractivePrompt: (instruction: string) => `interactive:${instruction}`,
}));

import { generateAndRegisterDesign } from './generateAndRegister';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const FOLDER = '/proj/.aspis-design/d1';

/**
 * Reset all mocks and install default invoke behavior for this call sequence.
 * The `design_registry_remember` handler ECHOES BACK the id from the entry
 * passed in (matching the Fix 1 contract: lookup is by id, not by path).
 */
function setupInvoke() {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (command: unknown, args: unknown) => {
    if (command === 'design_create_project') {
      return { meta: { name: 'My Design', id: 'p1', schemaVersion: 1, createdAt: '', updatedAt: '', canvas: { w: 1440, h: 1024, grid: 8 }, nodeOrder: [] }, manifest: { schemaVersion: 1, nodes: {} }, components: {} };
    }
    if (command === 'design_save_project' || command === 'design_write_artifact') {
      return undefined;
    }
    if (command === 'design_registry_remember') {
      // Echo back the id that was sent in the entry — id-based lookup must find it.
      const entry = (args as { entry: { id: string; workingFolderPath: string } }).entry;
      return [{ id: entry.id, name: 'My Design', workingFolderPath: entry.workingFolderPath, createdAt: '', updatedAt: '', lastOpenedAt: '' }];
    }
    throw new Error(`Unexpected command: ${String(command)}`);
  });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

beforeEach(() => {
  applyGenerationMock.mockClear();
  applyInteractiveMock.mockClear();
  startDesignGenerationMock.mockClear();
  disposeMock.mockClear();
});

describe('generateAndRegisterDesign — static mode', () => {
  it('calls design_save_project and registers without kind', async () => {
    setupInvoke();
    const id = await generateAndRegisterDesign({
      mode: 'static',
      prompt: 'dashboard',
      workingFolderPath: FOLDER,
      designName: 'My Design',
    });
    // id now comes from blank.meta.id (echoed back by the registry mock)
    expect(id).toBe('p1');

    // design_save_project must be called (static pipeline)
    const saveCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_save_project');
    expect(saveCall).toBeDefined();

    // design_write_artifact must NOT be called for static
    const artifactCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_write_artifact');
    expect(artifactCall).toBeUndefined();

    // applyGeneration must be called; applyInteractiveGeneration must NOT
    expect(applyGenerationMock).toHaveBeenCalledOnce();
    expect(applyInteractiveMock).not.toHaveBeenCalled();

    // Registry entry must NOT carry kind/artifactPath
    const rememberCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_registry_remember')!;
    const entry = (rememberCall[1] as { entry: Record<string, unknown> }).entry;
    expect(entry['kind']).toBeUndefined();
    expect(entry['artifactPath']).toBeUndefined();
  });

  it('uses buildGeneratePrompt (prefix "static:")', async () => {
    setupInvoke();
    await generateAndRegisterDesign({
      mode: 'static',
      prompt: 'hero section',
      workingFolderPath: FOLDER,
      designName: 'test',
    });
    // The stub startDesignGeneration records the prompt it was called with.
    const [promptArg] = startDesignGenerationMock.mock.calls[0] as [string, ...unknown[]];
    expect(promptArg).toMatch(/^static:/);
  });
});

describe('generateAndRegisterDesign — interactive mode', () => {
  it('calls design_write_artifact and registers kind:interactive + artifactPath', async () => {
    setupInvoke();
    const id = await generateAndRegisterDesign({
      mode: 'interactive',
      prompt: 'mobile login screen',
      workingFolderPath: FOLDER,
      designName: 'My Design',
    });
    // id now comes from blank.meta.id (echoed back by the registry mock)
    expect(id).toBe('p1');

    // design_write_artifact must be called (interactive pipeline)
    const artifactCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_write_artifact');
    expect(artifactCall).toBeDefined();

    // design_save_project must NOT be called for interactive
    const saveCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_save_project');
    expect(saveCall).toBeUndefined();

    // applyInteractiveGeneration must be called; applyGeneration must NOT
    expect(applyInteractiveMock).toHaveBeenCalledOnce();
    expect(applyGenerationMock).not.toHaveBeenCalled();

    // Registry entry must carry kind:'interactive' + artifactPath:'artifact/index.html'
    const rememberCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_registry_remember')!;
    const entry = (rememberCall[1] as { entry: Record<string, unknown> }).entry;
    expect(entry['kind']).toBe('interactive');
    expect(entry['artifactPath']).toBe('artifact/index.html');
    expect(entry['frame']).toBeUndefined();
  });

  it('carries frame through to the registry entry', async () => {
    setupInvoke();
    await generateAndRegisterDesign({
      mode: 'interactive',
      frame: 'android',
      prompt: 'Android home screen',
      workingFolderPath: FOLDER,
      designName: 'Android Home',
    });
    const rememberCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_registry_remember')!;
    const entry = (rememberCall[1] as { entry: Record<string, unknown> }).entry;
    expect(entry['frame']).toBe('android');
  });

  it('omits frame when not supplied', async () => {
    setupInvoke();
    await generateAndRegisterDesign({
      mode: 'interactive',
      prompt: 'browser tab',
      workingFolderPath: FOLDER,
      designName: 'Browser Tab',
    });
    const rememberCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_registry_remember')!;
    const entry = (rememberCall[1] as { entry: Record<string, unknown> }).entry;
    expect(entry['frame']).toBeUndefined();
  });

  it('uses buildInteractivePrompt (prefix "interactive:")', async () => {
    setupInvoke();
    await generateAndRegisterDesign({
      mode: 'interactive',
      prompt: 'settings page',
      workingFolderPath: FOLDER,
      designName: 'test',
    });
    const [promptArg] = startDesignGenerationMock.mock.calls[0] as [string, ...unknown[]];
    expect(promptArg).toMatch(/^interactive:/);
  });

  it('writes the html returned by applyInteractiveGeneration to design_write_artifact', async () => {
    setupInvoke();
    applyInteractiveMock.mockReturnValueOnce({
      html: '<html><body>custom html</body></html>',
      warnings: [],
      neutralizedCount: 0,
      wrapped: false,
    });
    await generateAndRegisterDesign({
      mode: 'interactive',
      prompt: 'test',
      workingFolderPath: FOLDER,
      designName: 'test',
    });
    const artifactCall = invokeMock.mock.calls.find(([cmd]) => cmd === 'design_write_artifact')!;
    expect((artifactCall[1] as { html: string }).html).toBe('<html><body>custom html</body></html>');
  });
});

describe('generateAndRegisterDesign — registry lookup by id (Fix 1)', () => {
  it('returns blank.meta.id exactly (not a path-derived id)', async () => {
    setupInvoke();
    const id = await generateAndRegisterDesign({
      mode: 'static',
      prompt: 'test',
      workingFolderPath: FOLDER,
      designName: 'test',
    });
    // blank.meta.id = 'p1' from the design_create_project mock
    expect(id).toBe('p1');
  });

  it('resolves by id even when the registry returns a canonicalized path different from input', async () => {
    // Simulates the macOS /tmp → /private/tmp canonicalization: workingFolderPath in
    // the returned entry differs from what was passed in, but id matches.
    invokeMock.mockImplementation(async (command: unknown, args: unknown) => {
      if (command === 'design_create_project') {
        return { meta: { name: 'test', id: 'p42', schemaVersion: 1, createdAt: '', updatedAt: '', canvas: { w: 1440, h: 1024, grid: 8 }, nodeOrder: [] }, manifest: { schemaVersion: 1, nodes: {} }, components: {} };
      }
      if (command === 'design_save_project') return undefined;
      if (command === 'design_registry_remember') {
        const entry = (args as { entry: { id: string } }).entry;
        // The Rust backend canonicalized the path — workingFolderPath differs, but id matches.
        return [{ id: entry.id, name: 'test', workingFolderPath: '/private/tmp/resolved', createdAt: '', updatedAt: '', lastOpenedAt: '' }];
      }
      throw new Error(`Unexpected: ${String(command)}`);
    });
    const id = await generateAndRegisterDesign({
      mode: 'static',
      prompt: 'p',
      workingFolderPath: '/tmp/design-folder',
      designName: 'd',
    });
    // Must succeed via id match, not fail due to path mismatch.
    expect(id).toBe('p42');
  });
});

describe('generateAndRegisterDesign — registry lookup failure', () => {
  it('throws if the registry does not return an entry matching blank.meta.id', async () => {
    invokeMock.mockImplementation(async (command: unknown, _args: unknown) => {
      if (command === 'design_create_project') {
        return { meta: { name: 'test', id: 'p1', schemaVersion: 1, createdAt: '', updatedAt: '', canvas: { w: 1440, h: 1024, grid: 8 }, nodeOrder: [] }, manifest: { schemaVersion: 1, nodes: {} }, components: {} };
      }
      if (command === 'design_save_project') return undefined;
      if (command === 'design_registry_remember') {
        // Return an entry with a DIFFERENT id — simulates a backend bug / stale list.
        return [{ id: 'other-id', name: 'x', workingFolderPath: FOLDER, createdAt: '', updatedAt: '', lastOpenedAt: '' }];
      }
      throw new Error(`Unexpected: ${String(command)}`);
    });
    await expect(
      generateAndRegisterDesign({ mode: 'static', prompt: 'p', workingFolderPath: FOLDER, designName: 'd' }),
    ).rejects.toThrow('not found in registry');
  });
});

describe('generateAndRegisterDesign — stream handle disposal (Fix 2)', () => {
  it('calls dispose() on the stream handle after a successful generation', async () => {
    setupInvoke();
    await generateAndRegisterDesign({
      mode: 'static',
      prompt: 'test',
      workingFolderPath: FOLDER,
      designName: 'test',
    });
    expect(disposeMock).toHaveBeenCalledOnce();
  });

  it('calls dispose() on the stream handle after a rejected generation (error path)', async () => {
    setupInvoke();
    // Override the stream mock to simulate a backend error.
    startDesignGenerationMock.mockImplementationOnce(
      async (_prompt: unknown, callbacks: { onText?: (t: string) => void; onStatus?: (s: string, m?: string) => void }) => {
        callbacks.onStatus?.('error', 'backend boom');
        return { genId: 'test-gen', cancel: vi.fn(), dispose: disposeMock };
      },
    );
    await expect(
      generateAndRegisterDesign({ mode: 'static', prompt: 'p', workingFolderPath: FOLDER, designName: 'd' }),
    ).rejects.toThrow('backend boom');
    expect(disposeMock).toHaveBeenCalledOnce();
  });
});

describe('generateAndRegisterDesign — pipeline warnings (Fix 4)', () => {
  it('logs warnings to console.warn when applyInteractiveGeneration emits them', async () => {
    setupInvoke();
    applyInteractiveMock.mockReturnValueOnce({
      html: '<html><body>partial</body></html>',
      warnings: ['model returned no usable markup', 'neutralized 3 remote refs'],
      neutralizedCount: 3,
      wrapped: false,
    });
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      await generateAndRegisterDesign({
        mode: 'interactive',
        prompt: 'test',
        workingFolderPath: FOLDER,
        designName: 'test',
      });
      expect(warnSpy).toHaveBeenCalledOnce();
      const [label, warnings] = warnSpy.mock.calls[0] as [string, unknown];
      expect(label).toContain('[design]');
      expect(Array.isArray(warnings)).toBe(true);
    } finally {
      warnSpy.mockRestore();
    }
  });

  it('does not call console.warn when there are no warnings', async () => {
    setupInvoke();
    // Default mock already returns warnings: []
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      await generateAndRegisterDesign({
        mode: 'interactive',
        prompt: 'test',
        workingFolderPath: FOLDER,
        designName: 'test',
      });
      expect(warnSpy).not.toHaveBeenCalled();
    } finally {
      warnSpy.mockRestore();
    }
  });
});
