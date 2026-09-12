import { createHash } from "node:crypto";

function readJsonInput(name, fallback = null) {
  const raw = process.env[`RUNX_INPUT_${name}`];
  if (raw === undefined || raw === "") return fallback;
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

function readStringInput(name, fallback = "") {
  const value = readJsonInput(name, fallback);
  return typeof value === "string" ? value : fallback;
}

function sha256(value) {
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}

function publicGithubRunUrl(value) {
  const url = new URL(String(value ?? ""));
  const match = url.pathname.match(/^\/repos\/([^/]+)\/([^/]+)\/actions\/runs\/(\d+)\/?$/);
  if (url.protocol !== "https:" || url.hostname !== "api.github.com" || !match) {
    throw new Error("source_url must be a public GitHub Actions run API URL");
  }
  return { url, owner: match[1], repo: match[2], runId: match[3] };
}

function safeMarkerPath(value) {
  const path = String(value || "public/deploy-target.json");
  if (path.startsWith("/") || path.split("/").includes("..")) {
    throw new Error("marker_path must be a repository-relative path");
  }
  return path;
}

async function fetchJson(url) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10_000);
  let response;
  try {
    response = await fetch(url, {
      headers: {
        accept: "application/vnd.github+json, application/json",
        "user-agent": "runx-rollback-judge",
      },
      redirect: "error",
      signal: controller.signal,
    });
  } finally {
    clearTimeout(timeout);
  }
  const body = await response.text();
  if (!response.ok) throw new Error(`source read returned HTTP ${response.status}`);
  return {
    data: JSON.parse(body),
    evidence: {
      url,
      fetched_at: new Date().toISOString(),
      response_sha256: sha256(body),
      etag: response.headers.get("etag"),
    },
  };
}

async function readDeploymentRun(sourceUrl, markerPath) {
  const source = publicGithubRunUrl(sourceUrl);
  const runRead = await fetchJson(source.url.href);
  const run = runRead.data;
  if (String(run.id) !== source.runId || run.repository?.full_name !== `${source.owner}/${source.repo}`) {
    throw new Error("GitHub run response does not match source_url");
  }
  if (typeof run.head_sha !== "string" || !/^[0-9a-f]{40}$/i.test(run.head_sha)) {
    throw new Error("GitHub run response is missing a full head SHA");
  }

  const path = safeMarkerPath(markerPath);
  const markerUrl = `https://raw.githubusercontent.com/${source.owner}/${source.repo}/${run.head_sha}/${path}`;
  const markerRead = await fetchJson(markerUrl);
  return {
    run,
    marker: markerRead.data,
    evidence: {
      provider: "github_actions",
      repository: `${source.owner}/${source.repo}`,
      run_id: run.id,
      workflow_name: run.name,
      workflow_path: run.path,
      status: run.status,
      conclusion: run.conclusion,
      head_sha: run.head_sha,
      run_started_at: run.run_started_at,
      completed_at: run.updated_at,
      run_url: run.html_url,
      api_read: runRead.evidence,
      deployment_marker: {
        ...markerRead.evidence,
        schema: markerRead.data?.schema ?? null,
        service: markerRead.data?.service ?? null,
        version: markerRead.data?.version ?? null,
        health: markerRead.data?.health ?? null,
        signal_kind: markerRead.data?.signal_kind ?? null,
        metric: markerRead.data?.metric ?? null,
        incident: markerRead.data?.incident ?? null,
      },
    },
  };
}

function releaseTargetRef(service, version) {
  return service && version ? `runx:release:${service}@${version}` : "";
}

function approval(approved, reason) {
  return {
    gate_id: "release.publish.approval",
    approved,
    reason,
    answer: { approved, reason },
    dispatch: {
      skill: "release",
      answer_key: "release.publish.approval",
      note: "Release owns the downstream consequence; rollback-judge performs no deploy effect.",
    },
  };
}

function hold(reason, missingEvidence = [], sourceRead = null) {
  return {
    act_decision: "defer",
    act_reason: `action=hold reason=${reason}`,
    act_target_ref: "",
    decision: { action: "hold", reason, version_target: null },
    escalation: { required: true, reason, missing_evidence: missingEvidence },
    release_publish_approval: approval(false, reason),
    review_record: {
      form: "review",
      signal: sourceRead?.current?.deployment_marker
        ? {
            severity: "unknown",
            kind: sourceRead.current.deployment_marker.signal_kind,
          }
        : { severity: "unknown", kind: "unknown" },
      evidence_used: sourceRead ? ["source_read.current"] : [],
      refused: { reason },
    },
    source_read: sourceRead,
  };
}

const deploySignal = readJsonInput("DEPLOY_SIGNAL", {});
const currentVersion = readJsonInput("CURRENT_VERSION", {});
const priorVersion = readJsonInput("PRIOR_VERSION", null);
const forwardFixEvidence = readJsonInput("FORWARD_FIX_EVIDENCE", {});
const monitorRunRef = readStringInput("MONITOR_RUN_REF");

let packet;
try {
  const currentSourceUrl = deploySignal?.evidence?.source_url;
  if (!currentSourceUrl) {
    packet = hold("missing_monitor_source", ["deploy_signal.evidence.source_url"]);
  } else {
    const current = await readDeploymentRun(currentSourceUrl, currentVersion?.marker_path);
    const expectedWorkflow = deploySignal?.evidence?.workflow_name;
    const observed = Number(current.marker?.metric?.observed_percent);
    const threshold = Number(current.marker?.metric?.threshold_percent);
    const metricComplete = Number.isFinite(observed) && Number.isFinite(threshold);
    const workflowMatches = !expectedWorkflow || current.run.name === expectedWorkflow;
    const completed = current.run.status === "completed";
    const monitorFailed = current.run.conclusion === "failure";
    const markerFailed = current.marker?.health !== "healthy" && metricComplete && observed > threshold;
    const sourceRead = { current: current.evidence, prior: null };

    if (!monitorRunRef || monitorRunRef !== current.run.html_url) {
      packet = hold("monitor_subject_ref_mismatch", ["monitor_run_ref"], sourceRead);
    } else if (!workflowMatches) {
      packet = hold("unexpected_monitor_workflow", ["deploy_signal.evidence.workflow_name"], sourceRead);
    } else if (!completed) {
      packet = hold("monitor_run_incomplete", ["deploy_signal.evidence.source_url"], sourceRead);
    } else if (monitorFailed !== markerFailed) {
      packet = hold("contradictory_monitor_signal", ["current_version.marker_path"], sourceRead);
    } else if (!monitorFailed) {
      packet = hold("nonfailing_signal", ["failing deployment monitor run"], sourceRead);
    } else if (!priorVersion?.source_url) {
      const tests = forwardFixEvidence?.test_runs;
      if (Array.isArray(tests) && tests.length > 0 && forwardFixEvidence?.review_signoff) {
        packet = {
          act_decision: "approve",
          act_reason: "action=roll_forward reason=tested_forward_fix",
          act_target_ref: "",
          decision: { action: "roll_forward", reason: "tested_forward_fix", version_target: null },
          escalation: { required: false, reason: null, missing_evidence: [] },
          release_publish_approval: approval(true, "tested_forward_fix"),
          review_record: {
            form: "review",
            signal: { severity: "critical", kind: current.marker.signal_kind },
            evidence_used: ["source_read.current", "forward_fix_evidence.test_runs", "forward_fix_evidence.review_signoff"],
            refused: { reason: null },
          },
          source_read: sourceRead,
        };
      } else {
        packet = hold("missing_rollback_or_forward_fix_evidence", [
          "prior_version.source_url",
          "forward_fix_evidence.test_runs",
          "forward_fix_evidence.review_signoff",
        ], sourceRead);
      }
    } else {
      const prior = await readDeploymentRun(priorVersion.source_url, priorVersion?.marker_path);
      sourceRead.prior = prior.evidence;
      const priorObserved = Number(prior.marker?.metric?.observed_percent);
      const priorThreshold = Number(prior.marker?.metric?.threshold_percent);
      const priorHealthy =
        prior.run.status === "completed" &&
        prior.run.conclusion === "success" &&
        prior.marker?.health === "healthy" &&
        Number.isFinite(priorObserved) &&
        Number.isFinite(priorThreshold) &&
        priorObserved <= priorThreshold;

      if (!priorHealthy) {
        packet = hold("prior_version_not_proven_healthy", ["prior_version.source_url"], sourceRead);
      } else {
        const service = String(current.marker?.service || currentVersion?.service || "");
        const version = String(prior.marker?.version || prior.run.head_sha);
        const targetRef = releaseTargetRef(service, version);
        const reason = "error_rate_critical";
        packet = {
          act_decision: "approve",
          act_reason: `action=rollback reason=${reason} target=${version} source=github-actions:${current.run.id}`,
          act_target_ref: targetRef,
          decision: {
            action: "rollback",
            reason,
            version_target: {
              version,
              digest: `git:${prior.run.head_sha}`,
              source: "prior_version.source_url",
              source_url: priorVersion.source_url,
              deployed_at: prior.run.run_started_at,
            },
          },
          escalation: { required: false, reason: null, missing_evidence: [] },
          release_publish_approval: approval(true, reason),
          review_record: {
            form: "review",
            signal: { severity: "critical", kind: current.marker.signal_kind },
            evidence_used: [
              "source_read.current.api_read",
              "source_read.current.deployment_marker.metric",
              "source_read.prior.api_read",
              "source_read.prior.deployment_marker",
            ],
            refused: { reason: null },
          },
          source_read: sourceRead,
        };
      }
    }
  }
} catch (error) {
  const message = error instanceof Error ? error.message : "unknown source read error";
  packet = hold("monitor_source_read_failed", [message]);
}

process.stdout.write(`${JSON.stringify(packet)}\n`);
