from typing import Any, Mapping, Sequence

from runx import RunxClient, create_host_bridge, create_openai_host_adapter


def resume_payload_to_cli_payload(
    responses: Sequence[Mapping[str, Any]] | None,
) -> tuple[dict[str, Any], dict[str, bool]]:
    answers: dict[str, Any] = {}
    approvals: dict[str, bool] = {}
    for response in responses or ():
        request_id = str(response.get("requestId") or "")
        payload = response.get("payload")
        if isinstance(payload, bool):
            approvals[request_id] = payload
        else:
            answers[request_id] = payload
    return answers, approvals


def main() -> None:
    client = RunxClient()

    def run(skill_path: str, inputs: Mapping[str, Any] | None = None) -> Mapping[str, Any]:
        return client.run_skill(skill_path, inputs=inputs)

    def resume(run_id: str, responses: Sequence[Mapping[str, Any]] | None = None) -> Mapping[str, Any]:
        answers, approvals = resume_payload_to_cli_payload(responses)
        return client.resume_run(run_id, answers=answers, approvals=approvals)

    bridge = create_host_bridge(run=run, resume=resume)
    adapter = create_openai_host_adapter(bridge)
    response = adapter.run(
        "skills/issue-triage",
        inputs={"repository": "acme/app", "issue_number": "7"},
        resolver=lambda context: True if context.request.get("kind") == "approval" else None,
    )
    print(response)


if __name__ == "__main__":
    main()
