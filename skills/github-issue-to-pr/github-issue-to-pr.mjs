// Repository procedure and evidence interpretation. Native Git, scafld and
// GitHub operations retain all IO, authorization and independent observations.
const object = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : {};
const text = (value) => (typeof value === "string" ? value : "");
const scalar = (value) =>
  typeof value === "string"
    ? value
    : Number.isSafeInteger(value)
      ? String(value)
      : "";
const commit = (value) =>
  typeof value === "string" && /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(value);
const digest = (value) =>
  typeof value === "string" && /^sha256:[0-9a-f]{64}$/.test(value);
const sameRepo = (a, b) => text(a).toLowerCase() === text(b).toLowerCase();
const names = (value, key) =>
  Array.isArray(value)
    ? value.map((item) => text(item?.[key])).filter(Boolean)
    : [];

function provider(value, operation, repository) {
  return (
    value?.schema === "runx.provider.operation.v1" &&
    value.status === "success" &&
    value.provider === "github" &&
    value.operation === operation &&
    sameRepo(value.target, repository)
  );
}

export function normalizeIssueEvidence(input) {
  const operation = object(input.provider_operation);
  const issue = object(operation.result?.issue);
  if (
    !provider(operation, "issue.read", input.requested_repository) ||
    scalar(issue.number) !== scalar(input.requested_issue_number) ||
    issue.pull_request ||
    issue.html_url !==
      `https://github.com/${input.requested_repository}/issues/${input.requested_issue_number}`
  ) {
    throw new Error(
      "The provider evidence does not identify the selected GitHub issue.",
    );
  }
  return {
    issue_evidence: {
      schema: "runx/github-issue-to-pr.issue_evidence.v1",
      repository: input.requested_repository,
      number: scalar(issue.number),
      title: text(issue.title),
      state: text(issue.state),
      body: text(issue.body),
      url: text(issue.html_url),
      labels: names(issue.labels, "name"),
      assignees: names(issue.assignees, "login"),
      source: {
        provider: operation.provider,
        transport: text(operation.transport),
        principal_uri: text(operation.principal_uri),
        readback_uri: text(operation.readback_uri),
      },
    },
  };
}

export function admitHostResult({ host_result: host, issue_evidence: issue }) {
  const errors = [...(Array.isArray(host.errors) ? host.errors : [])];
  if (
    !sameRepo(host.repository, issue.repository) ||
    host.issue_number !== issue.number
  )
    errors.push("Host work does not match the selected issue.");
  if (host.status !== "completed") errors.push("Host work did not complete.");
  if (!commit(host.commit))
    errors.push(
      "An exact tested commit is required; obtain commit permission rather than inventing one.",
    );
  if (!text(host.repo_root) || !text(host.branch))
    errors.push("The candidate checkout or branch is missing.");
  if (!Array.isArray(host.files) || !host.files.length)
    errors.push("No changed files are reported.");
  if (
    !Array.isArray(host.tests) ||
    !host.tests.length ||
    host.tests.some(
      (check) =>
        check.status !== "passed" ||
        !text(check.command) ||
        !text(check.evidence),
    )
  )
    errors.push("No complete passing host check report is available.");
  if (
    !["repository", "scafld"].includes(host.governance?.kind) ||
    !text(host.governance?.reason)
  )
    errors.push("Repository governance was not established.");
  if (
    host.governance?.kind === "scafld" &&
    (!text(host.governance.receipt_path) ||
      !digest(host.governance.contract_digest))
  )
    errors.push("Required scafld finalization evidence is missing.");
  return {
    admission: {
      decision: errors.length ? "blocked" : "ready",
      errors: [...new Set(errors)],
    },
  };
}

export function verifyCandidate({
  host_result: host,
  admission,
  status,
  branch,
  history,
}) {
  const errors = [...admission.errors];
  const head = history?.commits?.[0]?.commit;
  if (status?.clean !== true)
    errors.push(
      "The candidate checkout is dirty; the tested commit does not cover its working tree.",
    );
  if (branch?.detached || branch?.branch !== host.branch)
    errors.push("The candidate branch differs from the host report.");
  if (head !== host.commit)
    errors.push("The checkout HEAD differs from the tested commit.");
  if (
    !text(status?.repo_root) ||
    status.repo_root !== branch?.repo_root ||
    status.repo_root !== history?.repo_root ||
    (host.repo_root !== "." && host.repo_root !== status.repo_root)
  )
    errors.push(
      "Host work is not in the invocation checkout used for publication.",
    );
  return {
    candidate: {
      route: errors.length ? "blocked" : host.governance.kind,
      decision: errors.length ? "blocked" : "ready",
      errors,
      observation: {
        status: errors.length ? "unverified" : "verified",
        repo_root: text(status?.repo_root),
        branch: text(branch?.branch),
        commit: text(head),
      },
    },
  };
}

export function finishVerification({
  host_result: host,
  admission,
  candidate,
  receipt_verification,
}) {
  const current = candidate ?? {
    decision: "blocked",
    errors: admission.errors,
    observation: {
      status: "unverified",
      repo_root: "",
      branch: "",
      commit: "",
    },
  };
  const errors = [...current.errors];
  const governed = host.governance.kind === "scafld";
  const operation = object(receipt_verification);
  const result = object(operation.result);
  const body = object(result.receipt?.body);
  const verified =
    governed &&
    current.decision === "ready" &&
    operation.schema === "runx.provider.operation.v1" &&
    operation.provider === "scafld" &&
    operation.operation === "receipt.verify" &&
    operation.status === "success" &&
    result.passed === true &&
    digest(result.receipt_digest) &&
    body.verdict === "pass" &&
    text(body.task_id) !== "" &&
    body.head_commit === host.commit &&
    body.spec_fingerprint ===
      text(host.governance.contract_digest).replace(/^sha256:/, "") &&
    Array.isArray(body.open_blockers) &&
    body.open_blockers.length === 0;
  if (governed && !verified && current.decision === "ready")
    errors.push(
      "The repository-required scafld finalization did not verify for this exact commit and contract.",
    );
  return {
    verification: {
      decision: errors.length ? "blocked" : "ready",
      errors,
      candidate: current.observation,
      governance: {
        kind: host.governance.kind,
        status: governed
          ? verified
            ? "verified"
            : "unverified"
          : "not_required",
        receipt_digest: verified ? result.receipt_digest : "",
        contract_digest: governed ? text(host.governance.contract_digest) : "",
      },
    },
  };
}

export function planDelivery({
  host_result: host,
  verification,
  publication_mode = "respect",
}) {
  const errors = [...verification.errors];
  const request =
    publication_mode === "publish" ||
    (publication_mode === "respect" && host.publication.decision === "ready");
  const publication = host.publication;
  if (request) {
    if (publication.decision !== "ready")
      errors.push("Publication was requested without a ready host proposal.");
    for (const field of ["title", "body", "head", "base", "idempotency_key"])
      if (!text(publication[field]))
        errors.push(`Publication ${field} is missing.`);
    if (
      publication.head !== host.branch ||
      publication.head === publication.base
    )
      errors.push(
        "The publication branch is not the verified candidate or equals its base.",
      );
  }
  return {
    delivery_plan: {
      route: errors.length ? "blocked" : request ? "publish" : "local",
      errors,
      requested: request,
    },
  };
}

export function planPullRequest({ host_result: host }) {
  return {
    pull_request_plan: {
      find: {
        repository: host.repository,
        state: "open",
        head: `${host.repository.split("/")[0]}:${host.publication.head}`,
        base: host.publication.base,
        per_page: 2,
      },
    },
  };
}

export function choosePullRequest({ host_result: host, found }) {
  if (
    !provider(found, "pullrequests.list", host.repository) ||
    !Array.isArray(found.result?.pulls)
  )
    throw new Error(
      "Pull-request discovery did not return admitted provider evidence.",
    );
  const pulls = found.result.pulls;
  return {
    pull_request_choice: {
      action:
        pulls.length === 0 ? "open" : pulls.length === 1 ? "reuse" : "blocked",
      existing: pulls.length === 1 ? pulls[0] : null,
      open: {
        repository: host.repository,
        title: host.publication.title,
        body: host.publication.body,
        head: host.publication.head,
        base: host.publication.base,
        draft: host.publication.draft,
      },
    },
  };
}

export function settlePullRequest({
  host_result: host,
  pushed,
  choice,
  found,
  opened,
}) {
  const reused = choice.action === "reuse";
  const source = reused ? found : opened;
  const pull = reused ? object(choice.existing) : object(opened?.result?.pull);
  const valid =
    choice.action !== "blocked" &&
    provider(pushed, "branch.push", host.repository) &&
    pushed.result?.commit === host.commit &&
    pushed.result?.branch === host.publication.head &&
    provider(
      source,
      reused ? "pullrequests.list" : "pullrequest.open",
      host.repository,
    ) &&
    pull.head?.ref === host.publication.head &&
    pull.head?.sha === host.commit &&
    pull.base?.ref === host.publication.base &&
    sameRepo(pull.base?.repo?.full_name, host.repository) &&
    pull.state === "open" &&
    Number.isSafeInteger(pull.number) &&
    pull.number > 0 &&
    pull.html_url ===
      `https://github.com/${host.repository}/pull/${pull.number}` &&
    Boolean(text(source?.readback_uri));
  return {
    publication: {
      status: valid ? "published" : "failed",
      repository: host.repository,
      commit: valid ? host.commit : "",
      head: text(pull.head?.ref),
      base: text(pull.base?.ref),
      number: scalar(pull.number),
      url: text(pull.html_url),
      readback_uri: text(source?.readback_uri),
    },
  };
}

export function finalizeIssueToPr({
  host_result: host,
  verification,
  delivery_plan,
  publication_result,
}) {
  const publication = object(publication_result);
  const errors = [...delivery_plan.errors];
  const published =
    delivery_plan.route === "publish" &&
    publication.status === "published" &&
    sameRepo(publication.repository, host.repository) &&
    publication.commit === host.commit;
  if (delivery_plan.route === "publish" && !published)
    errors.push(
      "Exact-ref pull-request publication did not verify; resume the same proposal and retry identity.",
    );
  return {
    issue_to_pr_result: {
      schema: "runx/github-issue-to-pr.result.v1",
      status: errors.length ? "blocked" : "completed",
      outcome: errors.length
        ? "blocked"
        : published
          ? "published"
          : "local_change",
      repository: host.repository,
      issue_number: host.issue_number,
      branch: host.branch,
      commit: host.commit,
      summary: host.summary,
      files: host.files,
      tests: host.tests,
      test_assurance: "host_reported",
      candidate: verification.candidate,
      governance: verification.governance,
      publication: {
        status: published
          ? "published"
          : delivery_plan.requested
            ? "failed"
            : "not_requested",
        pr_number: scalar(publication.number),
        url: text(publication.url),
        readback_uri: text(publication.readback_uri),
      },
      errors,
    },
  };
}
