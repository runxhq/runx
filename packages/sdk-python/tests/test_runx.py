import json
import sys
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from runx import (
    RunxClient,
    SkillSearchResult,
    create_host_bridge,
    create_openai_host_adapter,
    normalize_host_result,
    normalize_host_state,
)


class RunxClientTests(unittest.TestCase):
    def test_parses_search_results(self) -> None:
        result = SkillSearchResult.from_dict(
            {
                "skill_id": "acme/sourcey",
                "name": "sourcey",
                "owner": "acme",
                "source": "runx-registry",
                "source_label": "runx registry",
                "trust_tier": "community",
                "required_scopes": ["repo:read"],
                "tags": ["docs"],
                "version": "1.0.0",
            }
        )

        self.assertEqual(result.skill_id, "acme/sourcey")
        self.assertEqual(result.required_scopes, ("repo:read",))
        self.assertEqual(result.tags, ("docs",))

    def test_invokes_runx_cli_json_output(self) -> None:
        with TemporaryDirectory() as tmp:
            fake_runx = Path(tmp) / "fake_runx.py"
            fake_runx.write_text(
                textwrap.dedent(
                    """
                    import json
                    import sys

                    args = sys.argv[1:]
                    if args[:2] == ["skill", "search"]:
                        print(json.dumps({
                            "status": "success",
                            "results": [{
                                "skill_id": "acme/sourcey",
                                "name": "sourcey",
                                "owner": "acme",
                                "source": "runx-registry",
                                "source_label": "runx registry",
                                                "trust_tier": "community",
                                "required_scopes": [],
                                "tags": [],
                            }],
                        }))
                    else:
                        print(json.dumps({"status": "success", "args": args}))
                    """
                ).strip()
            )

            client = RunxClient(command=(sys.executable, str(fake_runx)))
            results = client.search_skills("sourcey")
            run_report = client.run_skill("skills/example", inputs={"message": "hi"})

            self.assertEqual(results[0].skill_id, "acme/sourcey")
            self.assertEqual(
                run_report["args"],
                ["skill", "skills/example", "--message", "hi", "--json"],
            )

    def test_continue_run_invokes_skill_with_run_id_and_answers_file(self) -> None:
        with TemporaryDirectory() as tmp:
            fake_runx = Path(tmp) / "fake_runx.py"
            answers_path = Path(tmp) / "answers.json"
            answers_path.write_text(json.dumps({"req-1": {"ok": True}, "gate-1": True}))
            fake_runx.write_text(
                textwrap.dedent(
                    """
                    import json
                    import sys

                    args = sys.argv[1:]
                    print(json.dumps({"status": "success", "args": args}))
                    """
                ).strip()
            )

            client = RunxClient(command=(sys.executable, str(fake_runx)))
            report = client.continue_run("skills/example", run_id="run-123", answers_file=str(answers_path))

            self.assertEqual(
                report["args"],
                [
                    "resume",
                    "run-123",
                    str(answers_path),
                    "--json",
                ],
            )

    def test_host_bridge_continues_needs_agent_runs(self) -> None:
        continue_calls: list[tuple[str, list[dict[str, object]]]] = []

        def run(skill_path: str, inputs=None):
            return {
                "status": "needs_agent",
                "runId": "run-123",
                "skillName": skill_path,
                "requests": [
                    {"id": "req-1", "kind": "cognitive_work", "prompt": "Need a fix"},
                    {"id": "gate-1", "kind": "approval", "gate": {"id": "gate-1"}},
                ],
            }

        def continue_run(run_id: str, responses=None):
            continue_calls.append((run_id, list(responses or [])))
            return {
                "status": "completed",
                "skillName": "skills/sourcey",
                "output": "done",
                "receiptId": "receipt-123",
            }

        bridge = create_host_bridge(run=run, continue_run=continue_run)
        result = bridge.run(
            "skills/sourcey",
            resolver=lambda context: True if context.request.get("kind") == "approval" else {"draft": "apply docs update"},
        )

        self.assertEqual(result.status, "completed")
        self.assertEqual(result.receipt_id, "receipt-123")
        self.assertEqual(continue_calls[0][0], "run-123")

    def test_openai_host_adapter_formats_framework_result(self) -> None:
        def run(skill_path: str, inputs=None):
            return {
                "status": "completed",
                "skillName": skill_path,
                "output": "built docs",
                "receiptId": "receipt-456",
            }

        adapter = create_openai_host_adapter(create_host_bridge(run=run))
        response = adapter.run("skills/sourcey")

        self.assertEqual(response["role"], "tool")
        self.assertEqual(response["structuredContent"]["runx"]["status"], "completed")
        self.assertEqual(response["structuredContent"]["runx"]["receiptId"], "receipt-456")

    def test_normalize_host_result_maps_denial(self) -> None:
        result = normalize_host_result(
            {
                "status": "policy_denied",
                "skill": {"name": "skills/sourcey"},
                "reasons": ["missing approval"],
                "receipt": {"id": "receipt-789"},
            }
        )

        self.assertEqual(result.status, "denied")
        self.assertEqual(result.reasons, ("missing approval",))

    def test_normalize_host_result_maps_success(self) -> None:
        host = normalize_host_result(
            {
                "status": "success",
                "skill": {"name": "skills/sourcey"},
                "execution": {"stdout": "done"},
                "receipt": {"id": "receipt-321"},
            }
        )

        self.assertEqual(host.status, "completed")
        self.assertEqual(host.receipt_id, "receipt-321")

    def test_normalize_host_state_maps_terminal_snapshot(self) -> None:
        state = normalize_host_state(
            {
                "status": "completed",
                "kind": "harness",
                "skillName": "skills/sourcey",
                "runId": "run-123",
                "receiptId": "receipt-321",
                "verification": {"status": "verified"},
            }
        )

        self.assertEqual(state.status, "completed")
        self.assertEqual(state.receipt_id, "receipt-321")


if __name__ == "__main__":
    unittest.main()
