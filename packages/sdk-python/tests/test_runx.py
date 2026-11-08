import json
import sys
import textwrap
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from runx import RunxClient, SkillSearchResult


class RunxClientTests(unittest.TestCase):
    def test_parses_search_results(self) -> None:
        result = SkillSearchResult.from_dict(
            {
                "skill_id": "acme/sourcey",
                "name": "sourcey",
                "source": "runx-registry",
                "source_label": "runx registry",
                "trust_tier": "community",
                "required_scopes": ["repo:read"],
                "tags": ["docs"],
                "version": "1.0.0",
                "install_command": {"shell": "posix", "argv": ["runx", "registry", "install", "acme/sourcey@1.0.0"], "copy_text": "runx registry install acme/sourcey@1.0.0"},
                "run_command": {"shell": "posix", "argv": ["runx", "skill", "acme/sourcey@1.0.0"], "copy_text": "runx skill acme/sourcey@1.0.0"},
            }
        )

        self.assertEqual(result.skill_id, "acme/sourcey")
        self.assertEqual(result.publisher, "acme")
        self.assertEqual(result.source, "runx-registry")
        self.assertEqual(result.source_label, "runx registry")
        self.assertEqual(result.required_scopes, ("repo:read",))
        self.assertEqual(result.tags, ("docs",))
        self.assertEqual(result.install_command.copy_text, "runx registry install acme/sourcey@1.0.0")
        self.assertEqual(result.run_command.argv, ("runx", "skill", "acme/sourcey@1.0.0"))

    def test_parses_search_results_without_a_source(self) -> None:
        # The CLI omits source and source_label when the registry has none.
        result = SkillSearchResult.from_dict(
            {
                "skill_id": "acme/sourcey",
                "name": "sourcey",
                "trust_tier": "community",
                "required_scopes": [],
                "tags": [],
                "install_command": {"shell": "posix", "argv": ["runx", "registry", "install", "acme/sourcey"], "copy_text": "runx registry install acme/sourcey"},
                "run_command": {"shell": "posix", "argv": ["runx", "skill", "acme/sourcey"], "copy_text": "runx skill acme/sourcey"},
            }
        )

        self.assertEqual(result.skill_id, "acme/sourcey")
        self.assertIsNone(result.source)
        self.assertIsNone(result.source_label)

    def test_invokes_runx_cli_json_output(self) -> None:
        with TemporaryDirectory() as tmp:
            fake_runx = Path(tmp) / "fake_runx.py"
            fake_runx.write_text(
                textwrap.dedent(
                    """
                    import json
                    import sys

                    args = sys.argv[1:]
                    if args[:2] == ["registry", "search"]:
                        command = lambda *verb: {"shell": "posix", "argv": ["runx", *verb, "acme/sourcey@1.0.0"], "copy_text": f"runx {' '.join(verb)} acme/sourcey@1.0.0"}
                        print(json.dumps({
                            "status": "success",
                            "registry": {"action": "search", "results": [{
                                "skill_id": "acme/sourcey",
                                "name": "sourcey",
                                "source": "runx-registry",
                                "source_label": "runx registry",
                                                "trust_tier": "community",
                                "required_scopes": [],
                                "tags": [],
                                "install_command": command("registry", "install"),
                                "run_command": command("skill"),
                            }]},
                        }))
                    else:
                        print(json.dumps({"status": "success", "args": args}))
                    """
                ).strip()
            )

            client = RunxClient(command=(sys.executable, str(fake_runx)))
            results = client.search_skills("sourcey")
            run_report = client.run_skill(
                "skills/example",
                inputs={
                    "message": "hi",
                    "enabled": False,
                    "note": None,
                    "count": 7,
                    "config": {"mode": "fast"},
                    "tags": ["a", "b"],
                },
            )

            self.assertEqual(results[0].skill_id, "acme/sourcey")
            self.assertEqual(
                run_report["args"],
                [
                    "skill", "skills/example",
                    "-i", "message=hi",
                    "-i", "enabled=false",
                    "-i", "note=null",
                    "-i", "count=7",
                    "-i", 'config={"mode": "fast"}',
                    "-i", 'tags=["a", "b"]',
                    "--json",
                ],
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


if __name__ == "__main__":
    unittest.main()
