from __future__ import annotations

from dataclasses import dataclass
import json
import os
import subprocess
from typing import Any, Mapping, Sequence


@dataclass(frozen=True)
class SkillSearchResult:
    skill_id: str
    name: str
    owner: str
    source: str
    source_label: str
    source_type: str
    trust_tier: str
    required_scopes: tuple[str, ...]
    tags: tuple[str, ...]
    summary: str | None = None
    version: str | None = None
    digest: str | None = None
    add_command: str | None = None
    run_command: str | None = None

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "SkillSearchResult":
        return cls(
            skill_id=str(value["skill_id"]),
            name=str(value["name"]),
            owner=str(value["owner"]),
            source=str(value["source"]),
            source_label=str(value["source_label"]),
            source_type=str(value["source_type"]),
            trust_tier=str(value["trust_tier"]),
            required_scopes=tuple(str(item) for item in value.get("required_scopes", [])),
            tags=tuple(str(item) for item in value.get("tags", [])),
            summary=_optional_str(value.get("summary")),
            version=_optional_str(value.get("version")),
            digest=_optional_str(value.get("digest")),
            add_command=_optional_str(value.get("add_command")),
            run_command=_optional_str(value.get("run_command")),
        )


class RunxCommandError(RuntimeError):
    def __init__(self, args: Sequence[str], returncode: int, stderr: str) -> None:
        super().__init__(f"runx command failed with exit code {returncode}: {' '.join(args)}\n{stderr}")
        self.args_list = tuple(args)
        self.returncode = returncode
        self.stderr = stderr


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
        if completed.returncode != 0:
            raise RunxCommandError(json_args, completed.returncode, completed.stderr)
        parsed = json.loads(completed.stdout)
        if not isinstance(parsed, dict):
            raise ValueError("runx JSON output must be an object")
        return parsed

    def search_skills(self, query: str, source: str | None = None) -> list[SkillSearchResult]:
        args = ["skill", "search", query]
        if source is not None:
            args.extend(["--source", source])
        payload = self.run_json(args)
        return [SkillSearchResult.from_dict(item) for item in payload.get("results", [])]

    def run_skill(
        self,
        skill_path: str,
        inputs: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        args = ["skill", skill_path]
        for key, value in (inputs or {}).items():
            args.extend([f"--{key}", _cli_input_value(value)])
        return self.run_json(args)

    def continue_run(
        self,
        skill_path: str,
        run_id: str,
        answers_file: str,
        inputs: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        del skill_path
        if inputs:
            raise ValueError("runx resume reads answers from the answers file; pass fresh inputs only on a new skill run.")
        args = ["resume", run_id, answers_file]
        return self.run_json(args)


def _cli_input_value(value: Any) -> str:
    """Encode one skill input for the `runx skill --name <value>` contract.

    The CLI parses each value as JSON and falls back to the literal string, so
    non-string values must be JSON text to survive the boundary. Values outside
    the JSON model keep their string projection.
    """
    if isinstance(value, str):
        return value
    return json.dumps(value, default=str)


def _optional_str(value: Any) -> str | None:
    return None if value is None else str(value)


from .host_protocol import (  # noqa: E402
    ProviderHostAdapter,
    HostBoundaryContext,
    HostBoundaryResolver,
    HostBridge,
    HostCompletedResult,
    HostDeniedResult,
    HostEscalatedResult,
    HostFailedResult,
    HostNeedsAgentResult,
    HostRunResult,
    HostRunState,
    create_anthropic_host_adapter,
    create_crewai_host_adapter,
    create_host_bridge,
    create_langchain_host_adapter,
    create_openai_host_adapter,
    create_vercel_ai_host_adapter,
    normalize_host_result,
    normalize_host_state,
)


__all__ = [
    "RunxClient",
    "RunxCommandError",
    "SkillSearchResult",
    "ProviderHostAdapter",
    "HostBoundaryContext",
    "HostBoundaryResolver",
    "HostBridge",
    "HostCompletedResult",
    "HostDeniedResult",
    "HostEscalatedResult",
    "HostFailedResult",
    "HostNeedsAgentResult",
    "HostRunResult",
    "HostRunState",
    "create_anthropic_host_adapter",
    "create_crewai_host_adapter",
    "create_host_bridge",
    "create_langchain_host_adapter",
    "create_openai_host_adapter",
    "create_vercel_ai_host_adapter",
    "normalize_host_result",
    "normalize_host_state",
]
