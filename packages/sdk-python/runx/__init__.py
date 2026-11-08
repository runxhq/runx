from __future__ import annotations

from dataclasses import dataclass
import json
import os
import subprocess
from typing import Any, Mapping, Sequence


@dataclass(frozen=True)
class CopyableCommand:
    """A command the CLI printed: its arguments, and the shell line to show
    and copy, which the CLI checked is exactly what the arguments render to."""

    shell: str
    argv: tuple[str, ...]
    copy_text: str

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "CopyableCommand":
        return cls(
            shell=str(value["shell"]),
            argv=tuple(str(item) for item in value["argv"]),
            copy_text=str(value["copy_text"]),
        )


@dataclass(frozen=True)
class SkillSearchResult:
    skill_id: str
    name: str
    publisher: str
    trust_tier: str
    required_scopes: tuple[str, ...]
    tags: tuple[str, ...]
    install_command: CopyableCommand
    run_command: CopyableCommand
    summary: str | None = None
    version: str | None = None
    digest: str | None = None
    source: str | None = None
    source_label: str | None = None

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "SkillSearchResult":
        return cls(
            skill_id=str(value["skill_id"]),
            name=str(value["name"]),
            publisher=str(value["skill_id"]).split("/", 1)[0],
            trust_tier=str(value["trust_tier"]),
            required_scopes=tuple(str(item) for item in value.get("required_scopes", [])),
            tags=tuple(str(item) for item in value.get("tags", [])),
            install_command=CopyableCommand.from_dict(value["install_command"]),
            run_command=CopyableCommand.from_dict(value["run_command"]),
            summary=_optional_str(value.get("summary")),
            version=_optional_str(value.get("version")),
            digest=_optional_str(value.get("digest")),
            source=_optional_str(value.get("source")),
            source_label=_optional_str(value.get("source_label")),
        )


class RunxCommandError(RuntimeError):
    def __init__(self, args: Sequence[str], returncode: int, stderr: str) -> None:
        super().__init__(f"runx command failed with exit code {returncode}: {' '.join(args)}\n{stderr}")
        self.args_list = tuple(args)
        self.returncode = returncode
        self.stderr = stderr


# A run that waits on a person or an agent exits 2 and still reports itself,
# with the run id its resume names. Any other non-zero exit is a failure.
RUN_WAITS = 2


class RunxClient:
    def __init__(
        self,
        command: Sequence[str] = ("runx",),
        cwd: str | None = None,
        env: Mapping[str, str] | None = None,
    ) -> None:
        self.command = tuple(command)
        self.cwd = cwd
        self.env = dict(env) if env is not None else None

    def build_command(self, args: Sequence[str]) -> list[str]:
        return [*self.command, *args]

    def run_json(self, args: Sequence[str], input: str | None = None) -> dict[str, Any]:
        json_args = [*args]
        if "--json" not in json_args:
            json_args.append("--json")
        completed = subprocess.run(
            self.build_command(json_args),
            input=input,
            text=True,
            capture_output=True,
            check=False,
            cwd=self.cwd,
            env={**os.environ, **self.env} if self.env is not None else None,
        )
        if completed.returncode not in (0, RUN_WAITS):
            raise RunxCommandError(json_args, completed.returncode, completed.stderr)
        parsed = json.loads(completed.stdout)
        if not isinstance(parsed, dict):
            raise ValueError("runx JSON output must be an object")
        return parsed

    def search_skills(self, query: str, registry: str | None = None) -> list[SkillSearchResult]:
        """Search the registry, or the registry at `registry` (a URL or a local registry directory)."""
        args = ["registry", "search", query]
        if registry is not None:
            args.extend(["--registry", registry])
        payload = self.run_json(args)
        return [SkillSearchResult.from_dict(item) for item in payload["registry"]["results"]]

    def run_skill(
        self,
        skill_path: str,
        inputs: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Run a skill and return its report. A run that waits returns its
        report too, with the `run_id` that `resume` names."""
        args = ["skill", skill_path]
        for key, value in (inputs or {}).items():
            text = value if isinstance(value, str) else json.dumps(value)
            args.extend(["-i", f"{key}={text}"])
        return self.run_json(args)

    def resume(self, run_id: str, answers_file: str) -> dict[str, Any]:
        """Resume a waiting run with the runx.resume.v1 document in answers_file.

        Each entry under answers and approvals carries the request_digest the
        run reported for the request it answers; a close comes alone.
        """
        return self.run_json(["resume", run_id, answers_file])

def _optional_str(value: Any) -> str | None:
    return None if value is None else str(value)


__all__ = [
    "CopyableCommand",
    "RunxClient",
    "RunxCommandError",
    "SkillSearchResult",
]
