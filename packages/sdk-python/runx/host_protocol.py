from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Mapping, Sequence


@dataclass(frozen=True)
class HostBoundaryContext:
    request: Mapping[str, Any]
    events: tuple[Mapping[str, Any], ...] = ()


HostBoundaryResolver = Callable[[HostBoundaryContext], Any | None]
HostRunCallable = Callable[[str, Mapping[str, Any] | None], Mapping[str, Any]]
HostContinueCallable = Callable[[str, Sequence[Mapping[str, Any]] | None], Mapping[str, Any]]
HostInspectCallable = Callable[[str], Mapping[str, Any]]


@dataclass(frozen=True)
class HostNeedsAgentResult:
    status: str
    skill_name: str
    run_id: str
    requests: tuple[Mapping[str, Any], ...]
    step_ids: tuple[str, ...] = ()
    step_labels: tuple[str, ...] = ()
    events: tuple[Mapping[str, Any], ...] = ()


@dataclass(frozen=True)
class HostCompletedResult:
    status: str
    skill_name: str
    receipt_id: str
    output: str
    events: tuple[Mapping[str, Any], ...] = ()


@dataclass(frozen=True)
class HostFailedResult:
    status: str
    skill_name: str
    error: str
    receipt_id: str | None = None
    events: tuple[Mapping[str, Any], ...] = ()


@dataclass(frozen=True)
class HostEscalatedResult:
    status: str
    skill_name: str
    error: str
    receipt_id: str
    events: tuple[Mapping[str, Any], ...] = ()


@dataclass(frozen=True)
class HostDeniedResult:
    status: str
    skill_name: str
    reasons: tuple[str, ...]
    receipt_id: str | None = None
    events: tuple[Mapping[str, Any], ...] = ()


HostRunResult = (
    HostNeedsAgentResult
    | HostCompletedResult
    | HostFailedResult
    | HostEscalatedResult
    | HostDeniedResult
)


@dataclass(frozen=True)
class HostNeedsAgentState:
    status: str
    skill_name: str
    run_id: str
    requested_path: str | None = None
    resolved_path: str | None = None
    selected_runner: str | None = None
    requests: tuple[Mapping[str, Any], ...] = ()
    step_ids: tuple[str, ...] = ()
    step_labels: tuple[str, ...] = ()
    lineage: Mapping[str, Any] | None = None


@dataclass(frozen=True)
class HostTerminalState:
    status: str
    kind: str
    skill_name: str
    run_id: str
    receipt_id: str
    verification: Mapping[str, Any]
    source_type: str | None = None
    started_at: str | None = None
    completed_at: str | None = None
    actors: tuple[str, ...] = ()
    artifact_types: tuple[str, ...] = ()
    runner_provider: str | None = None
    approval: Mapping[str, Any] | None = None
    lineage: Mapping[str, Any] | None = None


HostRunState = HostNeedsAgentState | HostTerminalState


class HostBridge:
    def __init__(
        self,
        run: HostRunCallable,
        continue_run: HostContinueCallable | None = None,
        inspect: HostInspectCallable | None = None,
    ) -> None:
        self._run = run
        self._continue_run = continue_run
        self._inspect = inspect

    def run(
        self,
        skill_path: str,
        inputs: Mapping[str, Any] | None = None,
        resolver: HostBoundaryResolver | None = None,
    ) -> HostRunResult:
        initial = self._run(skill_path, inputs)
        return self._drive(initial, resolver=resolver)

    def continue_run(
        self,
        run_id: str,
        resolver: HostBoundaryResolver | None = None,
    ) -> HostRunResult:
        initial = self._continue_payload(run_id, None)
        return self._drive(initial, resolver=resolver)

    def inspect(self, reference_id: str) -> HostRunState:
        if self._inspect is None:
            raise RuntimeError("This host bridge does not support inspect().")
        return normalize_host_state(self._inspect(reference_id))

    def _drive(
        self,
        payload: Mapping[str, Any],
        resolver: HostBoundaryResolver | None,
    ) -> HostRunResult:
        current = dict(payload)
        while True:
            result = normalize_host_result(current)
            if not isinstance(result, HostNeedsAgentResult):
                return result
            if resolver is None:
                return result

            responses: list[dict[str, Any]] = []
            for request in result.requests:
                reply = resolver(HostBoundaryContext(request=request, events=result.events))
                normalized = _normalize_resolution_reply(request, reply)
                if normalized is None:
                    continue
                responses.append(
                    {
                        "requestId": str(request.get("id") or ""),
                        # The digest an answer binds to in a runx.resume.v1
                        # document, as the run reported it.
                        "requestDigest": str(request.get("request_digest") or ""),
                        "actor": normalized["actor"],
                        "payload": normalized["payload"],
                    }
                )

            if not responses:
                return result

            current = self._continue_payload(result.run_id, responses)

    def _continue_payload(
        self,
        run_id: str,
        responses: Sequence[Mapping[str, Any]] | None,
    ) -> Mapping[str, Any]:
        if self._continue_run is None:
            raise RuntimeError("This host bridge does not support continue_run().")
        return self._continue_run(run_id, responses)


class ProviderHostAdapter:
    def __init__(self, bridge: HostBridge, formatter: Callable[[HostRunResult], Mapping[str, Any]]) -> None:
        self.bridge = bridge
        self.formatter = formatter

    def run(
        self,
        skill_path: str,
        inputs: Mapping[str, Any] | None = None,
        resolver: HostBoundaryResolver | None = None,
    ) -> Mapping[str, Any]:
        return self.formatter(self.bridge.run(skill_path, inputs=inputs, resolver=resolver))

    def continue_run(
        self,
        run_id: str,
        resolver: HostBoundaryResolver | None = None,
    ) -> Mapping[str, Any]:
        return self.formatter(self.bridge.continue_run(run_id, resolver=resolver))


def create_host_bridge(
    run: HostRunCallable,
    continue_run: HostContinueCallable | None = None,
    inspect: HostInspectCallable | None = None,
) -> HostBridge:
    return HostBridge(run=run, continue_run=continue_run, inspect=inspect)


def create_openai_host_adapter(bridge: HostBridge) -> ProviderHostAdapter:
    return ProviderHostAdapter(bridge, _to_openai_response)


def create_anthropic_host_adapter(bridge: HostBridge) -> ProviderHostAdapter:
    return ProviderHostAdapter(bridge, _to_anthropic_response)


def create_vercel_ai_host_adapter(bridge: HostBridge) -> ProviderHostAdapter:
    return ProviderHostAdapter(bridge, _to_vercel_response)


def create_langchain_host_adapter(bridge: HostBridge) -> ProviderHostAdapter:
    return ProviderHostAdapter(bridge, _to_langchain_response)


def create_crewai_host_adapter(bridge: HostBridge) -> ProviderHostAdapter:
    return ProviderHostAdapter(bridge, _to_crewai_response)


def normalize_host_result(payload: Mapping[str, Any]) -> HostRunResult:
    if _is_canonical_host_result(payload):
        return _normalize_canonical_host_result(payload)

    status = str(payload.get("status") or "")
    skill = payload.get("skill")
    skill_name = str(skill.get("name")) if isinstance(skill, Mapping) else str(skill or "")
    if status == "needs_agent":
        return HostNeedsAgentResult(
            status="needs_agent",
            skill_name=skill_name,
            run_id=str(payload.get("run_id") or ""),
            requests=tuple(payload.get("requests") or ()),
            step_ids=tuple(str(item) for item in payload.get("step_ids") or ()),
            step_labels=tuple(str(item) for item in payload.get("step_labels") or ()),
        )
    if status == "policy_denied":
        reasons = payload.get("reasons") or ()
        receipt = payload.get("receipt") or {}
        return HostDeniedResult(
            status="denied",
            skill_name=skill_name,
            reasons=tuple(str(item) for item in reasons),
            receipt_id=_nested_str(receipt, "id") if isinstance(receipt, Mapping) else None,
        )
    if status == "success":
        execution = payload.get("execution") or {}
        receipt = payload.get("receipt") or {}
        return HostCompletedResult(
            status="completed",
            skill_name=skill_name,
            receipt_id=str(receipt.get("id") or "") if isinstance(receipt, Mapping) else "",
            output=str(execution.get("stdout") or "") if isinstance(execution, Mapping) else "",
        )

    execution = payload.get("execution") or {}
    receipt = payload.get("receipt") or {}
    outcome = str(receipt.get("outcome") or "") if isinstance(receipt, Mapping) else ""
    error = str(execution.get("errorMessage") or execution.get("stderr") or execution.get("stdout") or "") if isinstance(execution, Mapping) else ""
    receipt_id = _nested_str(receipt, "id") if isinstance(receipt, Mapping) else None
    if outcome == "escalated":
        return HostEscalatedResult(
            status="escalated",
            skill_name=skill_name,
            error=error,
            receipt_id=receipt_id or "",
        )
    return HostFailedResult(
        status="failed",
        skill_name=skill_name,
        error=error,
        receipt_id=receipt_id,
    )


def normalize_host_state(payload: Mapping[str, Any]) -> HostRunState:
    status = str(payload.get("status") or "")
    if status == "needs_agent":
        return HostNeedsAgentState(
            status="needs_agent",
            skill_name=str(payload.get("skillName") or ""),
            run_id=str(payload.get("runId") or ""),
            requested_path=_optional_str(payload.get("requestedPath")),
            resolved_path=_optional_str(payload.get("resolvedPath")),
            selected_runner=_optional_str(payload.get("selectedRunner")),
            requests=tuple(payload.get("requests") or ()),
            step_ids=tuple(str(item) for item in payload.get("stepIds") or ()),
            step_labels=tuple(str(item) for item in payload.get("stepLabels") or ()),
            lineage=payload.get("lineage") if isinstance(payload.get("lineage"), Mapping) else None,
        )
    return HostTerminalState(
        status=status,
        kind=str(payload.get("kind") or ""),
        skill_name=str(payload.get("skillName") or ""),
        run_id=str(payload.get("runId") or ""),
        receipt_id=str(payload.get("receiptId") or ""),
        verification=dict(payload.get("verification") or {}),
        source_type=_optional_str(payload.get("sourceType")),
        started_at=_optional_str(payload.get("startedAt")),
        completed_at=_optional_str(payload.get("completedAt")),
        actors=tuple(str(item) for item in payload.get("actors") or ()),
        artifact_types=tuple(str(item) for item in payload.get("artifactTypes") or ()),
        runner_provider=_optional_str(payload.get("runnerProvider")),
        approval=payload.get("approval") if isinstance(payload.get("approval"), Mapping) else None,
        lineage=payload.get("lineage") if isinstance(payload.get("lineage"), Mapping) else None,
    )


def _normalize_resolution_reply(
    request: Mapping[str, Any],
    reply: Any | None,
) -> Mapping[str, Any] | None:
    if reply is None:
        return None
    if isinstance(reply, Mapping) and "actor" in reply and "payload" in reply:
        return {
            "actor": str(reply.get("actor") or _default_actor_for_request(request)),
            "payload": reply.get("payload"),
        }
    if isinstance(reply, Mapping) and "payload" in reply:
        return {
            "actor": str(reply.get("actor") or _default_actor_for_request(request)),
            "payload": reply.get("payload"),
        }
    if isinstance(reply, bool) and request.get("kind") == "approval":
        return {"actor": "human", "payload": reply}
    return {
        "actor": _default_actor_for_request(request),
        "payload": reply,
    }


def _default_actor_for_request(request: Mapping[str, Any]) -> str:
    return "agent" if request.get("kind") == "cognitive_work" else "human"


def _summary(result: HostRunResult) -> str:
    if isinstance(result, HostCompletedResult):
        return f"{result.skill_name} completed. Inspect receipt {result.receipt_id}."
    if isinstance(result, HostNeedsAgentResult):
        return f"{result.skill_name} needs agent input at {result.run_id}. Continue after resolving {len(result.requests)} request(s)."
    if isinstance(result, HostDeniedResult):
        return f"{result.skill_name} was denied by policy."
    if isinstance(result, HostEscalatedResult):
        return f"{result.skill_name} escalated. Inspect receipt {result.receipt_id}."
    return f"{result.skill_name} failed. Inspect receipt {result.receipt_id or 'n/a'}."


def _is_canonical_host_result(payload: Mapping[str, Any]) -> bool:
    return isinstance(payload.get("skillName"), str) and str(payload.get("status") or "") in {
        "needs_agent",
        "completed",
        "failed",
        "escalated",
        "denied",
    }


def _normalize_canonical_host_result(payload: Mapping[str, Any]) -> HostRunResult:
    status = str(payload.get("status") or "")
    if status == "needs_agent":
        return HostNeedsAgentResult(
            status="needs_agent",
            skill_name=str(payload.get("skillName") or ""),
            run_id=str(payload.get("runId") or ""),
            requests=tuple(payload.get("requests") or ()),
            step_ids=tuple(str(item) for item in payload.get("stepIds") or ()),
            step_labels=tuple(str(item) for item in payload.get("stepLabels") or ()),
            events=tuple(payload.get("events") or ()),
        )
    if status == "completed":
        return HostCompletedResult(
            status="completed",
            skill_name=str(payload.get("skillName") or ""),
            receipt_id=str(payload.get("receiptId") or ""),
            output=str(payload.get("output") or ""),
            events=tuple(payload.get("events") or ()),
        )
    if status == "denied":
        return HostDeniedResult(
            status="denied",
            skill_name=str(payload.get("skillName") or ""),
            reasons=tuple(str(item) for item in payload.get("reasons") or ()),
            receipt_id=_optional_str(payload.get("receiptId")),
            events=tuple(payload.get("events") or ()),
        )
    if status == "escalated":
        return HostEscalatedResult(
            status="escalated",
            skill_name=str(payload.get("skillName") or ""),
            error=str(payload.get("error") or ""),
            receipt_id=str(payload.get("receiptId") or ""),
            events=tuple(payload.get("events") or ()),
        )
    return HostFailedResult(
        status="failed",
        skill_name=str(payload.get("skillName") or ""),
        error=str(payload.get("error") or ""),
        receipt_id=_optional_str(payload.get("receiptId")),
        events=tuple(payload.get("events") or ()),
    )


def _to_openai_response(result: HostRunResult) -> Mapping[str, Any]:
    return {
        "role": "tool",
        "content": [{"type": "text", "text": _summary(result)}],
        "structuredContent": {"runx": _result_to_dict(result)},
    }


def _to_anthropic_response(result: HostRunResult) -> Mapping[str, Any]:
    return {
        "content": [{"type": "text", "text": _summary(result)}],
        "metadata": {"runx": _result_to_dict(result)},
    }


def _to_vercel_response(result: HostRunResult) -> Mapping[str, Any]:
    return {
        "messages": [{"role": "assistant", "content": _summary(result)}],
        "data": {"runx": _result_to_dict(result)},
    }


def _to_langchain_response(result: HostRunResult) -> Mapping[str, Any]:
    return {
        "content": _summary(result),
        "additional_kwargs": {"runx": _result_to_dict(result)},
    }


def _to_crewai_response(result: HostRunResult) -> Mapping[str, Any]:
    return {
        "raw": _summary(result),
        "json_dict": {"runx": _result_to_dict(result)},
    }


def _result_to_dict(result: HostRunResult) -> Mapping[str, Any]:
    if isinstance(result, HostNeedsAgentResult):
        return {
            "status": result.status,
            "skillName": result.skill_name,
            "runId": result.run_id,
            "requests": list(result.requests),
            "stepIds": list(result.step_ids),
            "stepLabels": list(result.step_labels),
            "events": list(result.events),
        }
    if isinstance(result, HostCompletedResult):
        return {
            "status": result.status,
            "skillName": result.skill_name,
            "receiptId": result.receipt_id,
            "output": result.output,
            "events": list(result.events),
        }
    if isinstance(result, HostDeniedResult):
        return {
            "status": result.status,
            "skillName": result.skill_name,
            "reasons": list(result.reasons),
            "receiptId": result.receipt_id,
            "events": list(result.events),
        }
    return {
        "status": result.status,
        "skillName": result.skill_name,
        "error": result.error,
        "receiptId": result.receipt_id,
        "events": list(result.events),
    }


def _nested_str(payload: Mapping[str, Any], key: str) -> str | None:
    value = payload.get(key)
    return None if value is None else str(value)


def _optional_str(value: Any) -> str | None:
    return None if value is None else str(value)
