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
    owner: str
    source: str
    source_label: str
    trust_tier: str
    required_scopes: tuple[str, ...]
    tags: tuple[str, ...]
    install_command: CopyableCommand
    run_command: CopyableCommand
    summary: str | None = None
    version: str | None = None
    digest: str | None = None

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "SkillSearchResult":
        return cls(
            skill_id=str(value["skill_id"]),
            name=str(value["name"]),
            owner=str(value["owner"]),
            source=str(value["source"]),
            source_label=str(value["source_label"]),
            trust_tier=str(value["trust_tier"]),
            required_scopes=tuple(str(item) for item in value.get("required_scopes", [])),
            tags=tuple(str(item) for item in value.get("tags", [])),
            install_command=CopyableCommand.from_dict(value["install_command"]),
            run_command=CopyableCommand.from_dict(value["run_command"]),
            summary=_optional_str(value.get("summary")),
            version=_optional_str(value.get("version")),
            digest=_optional_str(value.get("digest")),
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
        args = ["skill", skill_path]
        for key, value in (inputs or {}).items():
            args.extend([f"--{key}", str(value)])
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
    "CopyableCommand",
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
