import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from oracle.evalbench.heldout import (
    compare,
    run_heldout,
    score_edit_schema,
    score_json_array,
    score_no_fences,
    score_output,
    strip_fences,
)

GOOD = '[{"path":"src/a.rs","oldString":"x","newString":"y"}]'


class ScorerTests(unittest.TestCase):
    def test_strip_fences_handles_clean_fenced_and_dangling(self):
        self.assertEqual(strip_fences(GOOD), GOOD)
        self.assertEqual(strip_fences(f"```json\n{GOOD}\n```"), GOOD)
        self.assertEqual(strip_fences(f"```\n{GOOD}\n```"), GOOD)

    def test_score_json_array(self):
        self.assertTrue(score_json_array(GOOD))
        self.assertTrue(score_json_array(f"```json\n{GOOD}\n```"))
        self.assertFalse(score_json_array('{"not":"a list"}'))
        self.assertFalse(score_json_array("prose, not json"))

    def test_score_edit_schema_accepts_camel_and_snake_rejects_mixed_garbage(self):
        self.assertTrue(score_edit_schema(json.loads(GOOD)))
        self.assertTrue(
            score_edit_schema([{"path": "a", "old_string": "", "new_string": "c"}])
        )
        self.assertFalse(score_edit_schema([{"path": "", "oldString": "x", "newString": "y"}]))
        self.assertFalse(score_edit_schema([{"oldString": "x", "newString": "y"}]))
        self.assertFalse(score_edit_schema([{"path": "a", "oldString": 3, "newString": "y"}]))
        self.assertFalse(score_edit_schema("not a list"))
        self.assertFalse(score_edit_schema(["not an object"]))

    def test_score_output_pass_requires_all_three(self):
        good = score_output(GOOD)
        self.assertTrue(good["pass"])
        fenced = score_output(f"```json\n{GOOD}\n```")
        self.assertTrue(fenced["json"], "fence-stripped JSON still parses")
        self.assertFalse(fenced["noFences"], "raw fences must be flagged")
        self.assertFalse(fenced["pass"], "fenced output must not pass overall")
        self.assertFalse(score_output("garbage")["pass"])


class RunHeldoutTests(unittest.TestCase):
    def _pairs_file(self, tmp, lines):
        p = Path(tmp) / "pairs.jsonl"
        p.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return str(p)

    def _pair(self, task):
        return json.dumps(
            {"task": task, "model": "recorded-model", "rejected": "r", "chosen": "c"}
        )

    def test_run_heldout_scores_with_the_candidate_model_not_the_recorded_one(self):
        # THE harness invariant: the replay must use the CANDIDATE model under
        # test; pair["model"] is provenance only.
        with tempfile.TemporaryDirectory() as tmp:
            path = self._pairs_file(tmp, [self._pair("t1"), self._pair("t2")])
            seen_models = []

            def fake_replay(base_url, model, task_prompt, timeout_secs=300, enable_thinking=False):
                seen_models.append(model)
                return GOOD

            with patch("oracle.evalbench.heldout.replay_task", side_effect=fake_replay):
                out = run_heldout(path, "http://127.0.0.1:8000/v1", "candidate-x")
            self.assertEqual(seen_models, ["candidate-x", "candidate-x"])
            self.assertEqual(out["total"], 2)
            self.assertEqual(out["passed"], 2)
            self.assertEqual(out["acceptRate"], 1.0)
            self.assertEqual(out["model"], "candidate-x")

    def test_run_heldout_skips_malformed_and_records_replay_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._pairs_file(
                tmp,
                [self._pair("ok"), "not-json", json.dumps({"task": "missing-fields"})],
            )

            def boom(base_url, model, task_prompt, timeout_secs=300, enable_thinking=False):
                raise RuntimeError("transport down")

            with patch("oracle.evalbench.heldout.replay_task", side_effect=boom):
                out = run_heldout(path, "http://127.0.0.1:8000/v1", "candidate-x")
            self.assertEqual(out["skippedMalformed"], 2)
            self.assertEqual(out["total"], 1)
            self.assertEqual(out["passed"], 0)
            self.assertEqual(out["acceptRate"], 0.0)
            self.assertIn("transport down", out["results"][0]["error"])

    def test_run_heldout_limit_caps_the_slice(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = self._pairs_file(tmp, [self._pair(f"t{i}") for i in range(5)])
            with patch(
                "oracle.evalbench.heldout.replay_task", return_value=GOOD
            ) as mock_replay:
                out = run_heldout(path, "u", "m", limit=2)
            self.assertEqual(out["total"], 2)
            self.assertEqual(mock_replay.call_count, 2)


class CompareTests(unittest.TestCase):
    def test_compare_measures_but_never_decides(self):
        base = {"acceptRate": 0.5}
        cand = {"acceptRate": 0.7}
        out = compare(base, cand)
        self.assertAlmostEqual(out["delta"], 0.2)
        self.assertTrue(out["strictlyImproves"])
        flat = compare(base, {"acceptRate": 0.5})
        self.assertFalse(flat["strictlyImproves"], "equal is NOT an improvement")


if __name__ == "__main__":
    unittest.main()