"""Phase 1 — Clean generic extractive synthesis for `ask` (TDD).

Verifies that `structural_extractive_answer` produces a clean, deterministic,
grounded answer from chunk context WITHOUT an LLM, and that it is wired into
the answerer fallback chain BEFORE the apology-producing `extractive_answer`.
"""

import unittest

from oracle.server.answerer import (
    answer_from_context,
    extractive_answer,
    MAX_ANSWER_CHARS,
)
from oracle.server.structural_synthesis import structural_extractive_answer


class TestStructuralExtractiveAnswer(unittest.TestCase):
    """Unit tests for structural_extractive_answer (pure function)."""

    def _make_context(self):
        """Build a realistic context list of ~4 dicts across 2 files."""
        return [
            {
                "ref": "C1",
                "chunk_id": "figlyph/src-tauri/src/flow/executor.rs#0",
                "file_source": "figlyph/src-tauri/src/flow/executor.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 2400,
                "retrieval": "dense",
                "score": 0.71,
                "kind": "function",
                "symbol_name": "execute_flow",
                "signature": "pub async fn execute_flow(id: FlowId) -> Result<FlowStatus>",
                "text": (
                    "pub async fn execute_flow(id: FlowId) -> Result<FlowStatus> {\n"
                    "    let flow = db.get_flow(id).await?;\n"
                    "    match flow.run(self.pool).await {\n"
                    "        Ok(status) => { db.update_status(id, status).await?; Ok(status) }\n"
                    "        Err(e) => { self.escalate(id, &e).await; Err(e) }\n"
                    "    }\n"
                    "}\n"
                    "/// Escalate a failed flow to the big model router.\n"
                    "async fn escalate(&self, id: FlowId, error: &FlowError) {\n"
                    "    self.router.send(Escalation { id, reason: error.clone() }).await;\n"
                    "}\n"
                ),
                "language": "rust",
                "line_start": 42,
                "line_end": 58,
            },
            {
                "ref": "C2",
                "chunk_id": "figlyph/src-tauri/src/flow/executor.rs#1",
                "file_source": "figlyph/src-tauri/src/flow/executor.rs",
                "chunk_index": 1,
                "start_char": 2400,
                "end_char": 4800,
                "retrieval": "lexical",
                "score": 0.45,
                "kind": "struct",
                "symbol_name": "FlowExecutor",
                "signature": "struct FlowExecutor { pool: DbPool, router: EscalationRouter }",
                "text": (
                    "struct FlowExecutor {\n"
                    "    pool: DbPool,\n"
                    "    router: EscalationRouter,\n"
                    "}\n"
                ),
                "language": "rust",
                "line_start": 10,
                "line_end": 14,
            },
            {
                "ref": "C3",
                "chunk_id": "figlyph/src/components/FlowView.tsx#0",
                "file_source": "figlyph/src/components/FlowView.tsx",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 1800,
                "retrieval": "dense+lexical",
                "score": 0.68,
                "kind": "function",
                "symbol_name": "useFlowStatus",
                "signature": "function useFlowStatus(flowId: string): FlowStatusHook",
                "text": (
                    "function useFlowStatus(flowId: string): FlowStatusHook {\n"
                    "  const [status, setStatus] = useState<FlowStatus>('idle');\n"
                    "  useEffect(() => {\n"
                    "    oracle_ask(`flow ${flowId} status`).then(r => setStatus(r.answer));\n"
                    "  }, [flowId]);\n"
                    "  return { status };\n"
                    "}\n"
                ),
                "language": "typescript",
                "line_start": 25,
                "line_end": 33,
            },
            {
                "ref": "C4",
                "chunk_id": "figlyph/src/router/big_model.ts#0",
                "file_source": "figlyph/src/router/big_model.ts",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 2200,
                "retrieval": "dense",
                "score": 0.65,
                "kind": "function",
                "symbol_name": "escalateToBigModel",
                "signature": "export async function escalateToBigModel(request: Escalation): Promise<Resolution>",
                "text": (
                    "export async function escalateToBigModel(request: Escalation): Promise<Resolution> {\n"
                    "  const prompt = buildEscalationPrompt(request);\n"
                    "  const response = await fetch(BIG_MODEL_URL, { method: 'POST', body: JSON.stringify(prompt) });\n"
                    "  return response.json();\n"
                    "}\n"
                ),
                "language": "typescript",
                "line_start": 12,
                "line_end": 18,
            },
        ]

    def test_answer_source_is_extractive_synthesis(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        self.assertEqual(result["answer_source"], "extractive_synthesis")

    def test_answer_contains_file_paths(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        answer = result["answer"]
        self.assertIn("executor.rs", answer)
        self.assertIn("FlowView.tsx", answer)

    def test_answer_contains_symbol_names(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        answer = result["answer"]
        symbols = {
            "execute_flow",
            "FlowExecutor",
            "useFlowStatus",
            "escalateToBigModel",
        }
        found = {s for s in symbols if s in answer}
        self.assertGreaterEqual(
            len(found), 2, f"Expected ≥2 symbols in answer, found: {found}"
        )

    def test_answer_no_apology_text(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        answer = result["answer"]
        self.assertNotIn("could not produce", answer)
        self.assertNotIn("API key", answer)
        self.assertNotIn("answer model", answer)

    def test_answer_respects_max_chars(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        self.assertLessEqual(len(result["answer"]), MAX_ANSWER_CHARS)

    def test_answer_deterministic(self):
        context = self._make_context()
        result1 = structural_extractive_answer(
            "how does the flow executor handle failure", context
        )
        result2 = structural_extractive_answer(
            "how does the flow executor handle failure", context
        )
        self.assertEqual(result1["answer"], result2["answer"])
        self.assertEqual(result1["citations"], result2["citations"])

    def test_not_found_is_false(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        self.assertFalse(result["not_found"])

    def test_has_citations(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        self.assertIsInstance(result["citations"], list)
        self.assertGreaterEqual(len(result["citations"]), 1)

    def test_has_fallback_reason(self):
        result = structural_extractive_answer(
            "how does the flow executor handle failure", self._make_context()
        )
        self.assertIn("fallback_reason", result)

    def test_empty_context_returns_not_found(self):
        result = structural_extractive_answer("some query", [])
        self.assertTrue(result["not_found"])


class TestStructuralSynthesisWiring(unittest.TestCase):
    """Integration: verify structural synthesis is wired BEFORE extractive_fallback."""

    def test_no_llm_config_yields_extractive_synthesis(self):
        """When LLM config is missing and no domain match, answer_source should be
        'extractive_synthesis', not 'extractive_fallback'."""
        context = [
            {
                "ref": "C1",
                "chunk_id": "test/file.rs#0",
                "file_source": "test/file.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 500,
                "retrieval": "dense",
                "score": 0.7,
                "kind": "function",
                "symbol_name": "process_request",
                "signature": "fn process_request(req: Request) -> Response",
                "text": "fn process_request(req: Request) -> Response { /* handle */ }\n",
                "language": "rust",
                "line_start": 5,
                "line_end": 10,
            },
        ]
        # ORACLE_ASK_DISABLE_LLM=1 forces the extractive path
        import os

        old = os.environ.get("ORACLE_ASK_DISABLE_LLM")
        os.environ["ORACLE_ASK_DISABLE_LLM"] = "1"
        try:
            result = answer_from_context("how does process_request work", context)
            self.assertEqual(result["answer_source"], "extractive_synthesis")
            self.assertNotIn("could not produce", result["answer"])
        finally:
            if old is not None:
                os.environ["ORACLE_ASK_DISABLE_LLM"] = old
            else:
                os.environ.pop("ORACLE_ASK_DISABLE_LLM", None)

    def test_extractive_answer_uses_structural_synthesis(self):
        """extractive_answer now delegates to structural_extractive_answer before
        the old apology path. Even a text_slice chunk gets a clean answer."""
        context = [
            {
                "ref": "C1",
                "chunk_id": "test/file.rs#0",
                "file_source": "test/file.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 500,
                "retrieval": "dense",
                "score": 0.7,
                "kind": "text_slice",
                "symbol_name": "",
                "signature": "",
                "text": "Some generic text without structural metadata.\n",
                "language": "",
                "line_start": 0,
                "line_end": 0,
            },
        ]
        result = extractive_answer("test query", context, reason="test")
        self.assertEqual(result["answer_source"], "extractive_synthesis")
        self.assertNotIn("could not produce", result["answer"])


if __name__ == "__main__":
    unittest.main()
