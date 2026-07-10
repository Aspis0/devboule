import { describe, it, mock } from 'node:test';
import assert from 'node:assert/strict';
import { handlePromptCommand } from './sidecar.mjs';

// Silence stdout during handlePromptCommand calls so emit() JSON lines don't
// pollute the TAP stream.
async function silenceStdout(fn) {
	const m = mock.method(process.stdout, 'write', () => true);
	try {
		await fn();
	} finally {
		m.mock.restore();
	}
}

// Minimal fake session — tracks calls made on it.
function fakeSession() {
	return {
		prompt: mock.fn(),
		setModel: mock.fn(),
	};
}

// Minimal fake modelRegistry (find always returns null; unused in OFF path).
const fakeRegistry = { find: () => null };

describe('handlePromptCommand — pigeon OFF (default)', () => {
	it('calls session.prompt once and never switches model', async () => {
		const session = fakeSession();

		await silenceStdout(() =>
			handlePromptCommand(
				{ message: 'hi', streamingBehavior: undefined },
				session,
				fakeRegistry,
				false, // pigeonEnabled = false
			)
		);

		assert.equal(session.prompt.mock.callCount(), 1, 'prompt should be called once');
		assert.deepEqual(
			session.prompt.mock.calls[0].arguments,
			['hi', { streamingBehavior: undefined }],
			'prompt args match',
		);
		assert.equal(session.setModel.mock.callCount(), 0, 'setModel must never be called');
	});

	it('works when pigeonEnabled is omitted (default param = false)', async () => {
		const session = fakeSession();

		await silenceStdout(() =>
			handlePromptCommand(
				{ message: 'hello' },
				session,
				fakeRegistry,
				// 4th arg omitted — defaults to false
			)
		);

		assert.equal(session.prompt.mock.callCount(), 1);
		assert.equal(session.setModel.mock.callCount(), 0);
	});

	it('passes streamingBehavior through when OFF', async () => {
		const session = fakeSession();

		await silenceStdout(() =>
			handlePromptCommand(
				{ message: 'test', streamingBehavior: 'full' },
				session,
				fakeRegistry,
				false,
			)
		);

		assert.deepEqual(
			session.prompt.mock.calls[0].arguments,
			['test', { streamingBehavior: 'full' }],
			'streamingBehavior forwarded',
		);
	});
});

// NOTE: ON-path (pigeonEnabled=true) is NOT tested here because it requires
// stubbing requestClassification (which round-trips to stdin via JSONL) and
// applyPigeonRouting. That would need a full mock harness; defer to integration tests.
