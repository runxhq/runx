// GitHub's issue, as `github/issue.read` returns it, as issue evidence.
export function normalizeIssueEvidence(inputs) {
  const operation = object(inputs.provider_operation);
  const issue = object(object(operation.result).issue);
  return {
    issue_evidence: {
      schema: "runx.issue_to_pr.issue_evidence.v1",
      repository: text(inputs.requested_repository),
      number: scalarText(issue.number),
      title: text(issue.title),
      state: text(issue.state),
      body: text(issue.body),
      url: text(issue.html_url),
      labels: names(issue.labels, "name"),
      assignees: names(issue.assignees, "login"),
      source: {
        provider: text(operation.provider),
        transport: text(operation.transport),
        principal_ref: text(operation.principal_ref),
        readback_ref: text(operation.readback_ref),
      },
    },
  };
}

// The open pull requests to look for before opening one: GitHub filters by
// `owner:branch`.
export function planPullRequest(inputs) {
  const host = object(inputs.host_result);
  const publication = object(host.publication);
  const owner = text(host.repository).split("/")[0];
  return {
    pull_request_plan: {
      find: {
        repository: text(host.repository),
        state: "open",
        head: `${owner}:${text(publication.head)}`,
        base: text(publication.base),
        per_page: 2,
      },
    },
  };
}

// Reuse the one open pull request for this head and base, open one when there
// is none, and stop when several are open.
export function choosePullRequest(inputs) {
  const host = object(inputs.host_result);
  const publication = object(host.publication);
  const pulls = Array.isArray(object(object(inputs.found).result).pulls) ? object(object(inputs.found).result).pulls : [];
  const action = pulls.length === 0 ? "open" : pulls.length === 1 ? "reuse" : "blocked";
  return {
    pull_request_choice: {
      action,
      existing: action === "reuse" ? pulls[0] : null,
      open: {
        repository: text(host.repository),
        title: text(publication.title),
        body: text(publication.body),
        head: text(publication.head),
        base: text(publication.base),
        draft: publication.draft === true,
      },
    },
  };
}

// The publication as one result: the pushed commit and the pull request that
// now carries it.
export function settlePullRequest(inputs) {
  const host = object(inputs.host_result);
  const publication = object(host.publication);
  const push = object(inputs.pushed);
  const choice = object(inputs.choice);
  const opened = object(inputs.opened);
  const pull = choice.action === "reuse" ? object(choice.existing) : object(object(opened.result).pull);
  const pushed = push.status === "success" && text(object(push.result).commit) === text(host.commit);
  const carries = text(object(pull.head).ref) === text(publication.head)
    && text(object(pull.head).sha) === text(host.commit)
    && text(object(pull.base).ref) === text(publication.base)
    && text(pull.state) === "open";
  return {
    publication: {
      status: pushed && carries ? "published" : "failed",
      repository: text(host.repository),
      commit: pushed ? text(host.commit) : "",
      head: text(object(pull.head).ref),
      base: text(object(pull.base).ref),
      number: scalarText(pull.number),
      url: text(pull.html_url),
      readback_ref: text(opened.readback_ref) || text(push.readback_ref),
    },
  };
}

export function admitHostResult(inputs) {
  const host = object(inputs.host_result);
  const issue = object(inputs.issue_evidence);
  const tests = Array.isArray(host.tests) ? host.tests : [];
  const errors = [];
  if (text(host.repository) !== text(issue.repository)) {
    errors.push("host result repository does not match issue evidence");
  }
  if (text(host.issue_number) !== text(issue.number)) {
    errors.push("host result issue number does not match issue evidence");
  }
  if (text(host.status) !== "completed") errors.push("host work did not complete");
  if (!text(host.repo_root)) errors.push("host repository root is missing");
  if (!exactCommit(host.commit)) errors.push("host result does not name an exact commit");
  if (!text(host.branch)) errors.push("host branch is missing");
  if (!Array.isArray(host.files) || host.files.length === 0) {
    errors.push("host work has no changed-file evidence");
  }
  if (tests.length === 0 || tests.some((test) => text(test?.status) !== "passed")) {
    errors.push("host work has no complete passing test evidence");
  }
  const finalization = object(host.finalization);
  if (!text(finalization.receipt_path)) errors.push("scafld finalize receipt path is missing");
  if (!digest(finalization.contract_digest)) errors.push("scafld contract digest is invalid");
  const publication = object(host.publication);
  const requested = text(publication.decision) === "ready";
  if (requested) {
    for (const field of ["title", "body", "head", "base", "idempotency_key"]) {
      if (!text(publication[field])) errors.push(`publication ${field} is missing`);
    }
    if (text(publication.head) !== text(host.branch)) {
      errors.push("publication head does not match the tested host branch");
    }
  }
  return {
    host_admission: {
      path: errors.length > 0 ? "blocked" : requested ? "publish" : "finalized_local",
      errors,
    },
  };
}

export function finalizeIssueToPr(inputs) {
  const host = object(inputs.host_result);
  const admission = object(inputs.host_admission);
  const verification = verifiedReceipt(inputs.receipt_verification, host);
  const publication = object(inputs.publication_result);
  const errors = Array.isArray(admission.errors) ? [...admission.errors] : [];
  const verified = verification.verified;
  if (errors.length === 0 && !verified) errors.push("scafld finalization did not verify");
  const publicationRequested = text(host.publication?.decision) === "ready";
  const published = publicationRequested
    && text(publication.status) === "published"
    && text(publication.repository) === text(host.repository)
    && text(publication.commit) === text(host.commit)
    && text(publication.head) === text(host.publication?.head)
    && text(publication.base) === text(host.publication?.base)
    && Boolean(text(publication.readback_ref));
  if (publicationRequested && !published) errors.push("exact-ref pull-request publication did not verify");
  const complete = errors.length === 0;
  const outcome = complete ? (published ? "published" : "finalized_local") : "blocked";
  return {
    issue_to_pr_result: {
      schema: "runx.issue_to_pr.result.v1",
      status: complete ? "completed" : "blocked",
      outcome,
      repository: text(host.repository),
      issue_number: text(host.issue_number),
      branch: text(host.branch),
      commit: text(host.commit),
      tested: Array.isArray(host.tests) && host.tests.length > 0
        && host.tests.every((test) => text(test?.status) === "passed"),
      finalization: {
        status: verified ? "verified" : "unverified",
        receipt_ref: verification.receipt_ref,
        contract_digest: text(host.finalization?.contract_digest),
        target: text(host.commit),
      },
      publication: {
        status: published ? "published" : publicationRequested ? "failed" : "not_requested",
        pr_number: scalarText(publication.number),
        url: text(publication.url),
        readback_ref: text(publication.readback_ref),
      },
      errors,
    },
  };
}

// The receipt `scafld/receipt.verify` verified, bound to this host result: a
// passing verdict for exactly the tested commit and contract, with no open
// blockers.
function verifiedReceipt(value, host) {
  const operation = object(value);
  const result = object(operation.result);
  const body = object(object(result.receipt).body);
  const contract = text(host.finalization?.contract_digest).replace(/^sha256:/, "");
  const receiptRef = text(result.receipt_digest);
  const verified = operation.status === "success"
    && result.passed === true
    && digest(receiptRef)
    && text(body.task_id) !== ""
    && text(body.verdict) === "pass"
    && text(body.head_commit) === text(host.commit)
    && contract !== ""
    && text(body.spec_fingerprint) === contract
    && Array.isArray(body.open_blockers)
    && body.open_blockers.length === 0;
  return { verified, receipt_ref: verified ? receiptRef : "" };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value : "";
}

function scalarText(value) {
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value)) return `${value}`;
  return "";
}

function exactCommit(value) {
  return typeof value === "string"
    && (value.length === 40 || value.length === 64)
    && /^[0-9a-f]+$/.test(value);
}

function digest(value) {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/.test(value);
}

function names(value, field) {
  return Array.isArray(value)
    ? value.map((entry) => text(object(entry)[field])).filter(Boolean)
    : [];
}
