export function inspectProfile(inputs) {
  const file = object(inputs.profile_file);
  if (!file.contents || file.truncated === true) {
    throw new Error("release profile must be a complete bounded UTF-8 file");
  }
  if (!/^sha256:[0-9a-f]{64}$/u.test(text(file.content_digest, 80))) {
    throw new Error("release profile digest is required");
  }
  const profile = parseObject(file.contents, "release profile");
  if (profile.schema !== "runx/release.profile.v1") {
    throw new Error("release profile schema must be runx/release.profile.v1");
  }
  const profileId = requiredText(profile.id, "profile id", 256);
  const channel = requiredText(inputs.channel, "channel", 128);
  if (!/^[a-z0-9][a-z0-9._-]*$/u.test(channel)) throw new Error("channel has an invalid format");
  if (profile.channel !== channel) throw new Error("release profile channel does not match requested channel");
  const version = requiredText(inputs.version, "version", 128);
  if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u.test(version)) {
    throw new Error("version must be SemVer");
  }
  const identity = { profileId, channel, version };
  const commands = Object.fromEntries(
    ["prepare", "publish", "verify"].map((phase) => [
      phase,
      normalizeCommand(profile.commands?.[phase], phase, identity),
    ]),
  );
  return {
    release_context: {
      schema: "runx/release.context.v1",
      profile_id: profileId,
      profile_path: requiredText(file.path, "profile path", 2_000),
      profile_digest: file.content_digest,
      project_root: requiredText(inputs.project_root, "project root", 4_000),
      channel,
      version,
      last_tag: text(inputs.last_tag, 256),
      operator_context: text(inputs.operator_context, 2_000),
      commands,
    },
  };
}

export function interpretPhase(inputs) {
  const context = object(inputs.release_context);
  if (context.schema !== "runx/release.context.v1") throw new Error("release_context is required");
  const phase = text(inputs.phase, 32);
  if (!["prepare", "publish", "verify"].includes(phase)) {
    throw new Error("phase must be prepare, publish, or verify");
  }
  const execution = object(inputs.command_execution);
  if (execution.schema !== "runx.command.execution.v1") {
    throw new Error("native command execution is required");
  }
  const command = object(context.commands?.[phase]);
  const commandPlan = object(inputs.command_plan);
  const expectedCommandDigest = text(commandPlan.command_digest || command.command_digest, 80);
  if (!expectedCommandDigest || execution.command_digest !== expectedCommandDigest) {
    throw new Error("command execution digest does not match the release plan");
  }
  const observed = normalizeObserved(execution.json);
  const executionOk = execution.decision === "completed"
    && execution.exit_code === 0
    && execution.timed_out === false
    && execution.stdout_truncated === false
    && execution.stderr_truncated === false;
  const matchesIdentity = (!observed.version || observed.version === context.version)
    && (!observed.channel || observed.channel === context.channel);
  let status = "failed";
  if (executionOk && matchesIdentity && phase === "prepare" && observed.status === "ready") status = "ready";
  if (executionOk && matchesIdentity && phase === "publish" && ["submitted", "published"].includes(observed.status)) {
    status = "command_completed";
  }
  if (executionOk
    && matchesIdentity
    && phase === "verify"
    && observed.status === "verified"
    && observed.version === context.version
    && observed.channel === context.channel
    && observed.locators.length > 0) {
    status = "verified";
  }
  const errors = Array.isArray(execution.errors) ? execution.errors.slice(0, 20) : [];
  if (executionOk && !matchesIdentity) {
    errors.push({ code: "release.identity_mismatch", message: "observed version or channel does not match the approved release" });
  }
  if (executionOk && matchesIdentity && status === "failed") {
    errors.push({ code: `release.${phase}_status`, message: `provider output did not prove the required ${phase} state` });
  }
  const phaseResult = {
    schema: "runx/release.phase_result.v1",
    phase,
    status,
    profile_id: context.profile_id,
    profile_digest: context.profile_digest,
    command_digest: execution.command_digest,
    channel: context.channel,
    version: context.version,
    observed,
    evidence: {
      stdout_digest: execution.stdout_digest,
      stderr_digest: execution.stderr_digest,
      exit_code: execution.exit_code,
      timed_out: execution.timed_out,
      duration_ms: execution.duration_ms,
      stdout_truncated: execution.stdout_truncated,
      stderr_truncated: execution.stderr_truncated,
    },
    errors,
  };
  const outputName = phase === "prepare" ? "preparation" : phase === "publish" ? "publication" : "verification";
  return { [outputName]: phaseResult };
}

export function finalizeBrief(inputs) {
  const context = bindPlans(object(inputs.release_context), {
    prepare: object(inputs.prepare_plan),
    publish: object(inputs.publish_plan),
    verify: object(inputs.verify_plan),
  });
  const preparation = object(inputs.preparation);
  const notes = object(inputs.release_notes);
  if (preparation.schema !== "runx/release.phase_result.v1" || preparation.phase !== "prepare") {
    throw new Error("preparation phase result is required");
  }
  if (preparation.command_digest !== context.commands.prepare.command_digest) {
    throw new Error("preparation digest does not match the release plan");
  }
  const forbidden = findForbiddenKeys(notes);
  const normalizedNotes = {
    headline: text(notes.headline, 300),
    summary: text(notes.summary, 4_000),
    changelog: normalizeChangelog(notes.changelog),
    upgrade_guidance: text(notes.upgrade_guidance, 4_000),
    risks: list(notes.risks, 50, 1_000),
  };
  let decision = preparation.status === "ready" ? "ready_for_approval" : "blocked";
  const findings = [...preparation.errors];
  if (forbidden.length > 0) {
    decision = "needs_agent";
    findings.push({
      code: "release.notes.side_effect_claim",
      message: `release notes contain forbidden effect fields: ${forbidden.join(", ")}`,
    });
  }
  if (decision === "ready_for_approval" && (!normalizedNotes.headline || !normalizedNotes.summary)) {
    decision = "needs_agent";
    findings.push({ code: "release.notes.missing", message: "headline and summary are required" });
  }
  return {
    release_context: context,
    release_brief: {
      schema: "runx/release.brief.v1",
      decision,
      profile_id: context.profile_id,
      profile_digest: context.profile_digest,
      profile_path: context.profile_path,
      channel: context.channel,
      version: context.version,
      last_tag: context.last_tag,
      preparation: {
        status: preparation.status,
        command_digest: preparation.command_digest,
        evidence: preparation.evidence,
        observed: preparation.observed,
      },
      release_notes: normalizedNotes,
      publish_intent: {
        ...commandIntent(context.commands.publish),
        approval_status: "pending",
      },
      verification_intent: {
        ...commandIntent(context.commands.verify),
        required_status: "verified",
      },
      findings,
      proof_boundary: {
        publication_status: "not_started",
        verification_status: "not_started",
        agent_authored_effects_accepted: false,
      },
    },
  };
}

function commandIntent(value) {
  const command = object(value);
  return {
    command_digest: text(command.command_digest, 80),
    command: text(command.command, 2_000),
    args: list(command.args, 64, 2_000),
    cwd: text(command.cwd, 512),
    timeout_ms: Number(command.timeout_ms),
    env_names: Object.keys(object(command.env)).sort().slice(0, 64),
  };
}

function normalizeCommand(value, phase, identity) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`commands.${phase} must be an object`);
  }
  if (!Array.isArray(value.argv) || value.argv.length === 0 || value.argv.length > 64) {
    throw new Error(`commands.${phase}.argv must contain 1-64 entries`);
  }
  const argv = value.argv.map((entry, index) => requiredText(entry, `commands.${phase}.argv[${index}]`, 2_000));
  if (argv.some(secretShaped)) {
    throw new Error(`commands.${phase}.argv must not contain credentials; use Runx credential delivery`);
  }
  const cwd = value.cwd == null ? "." : requiredText(value.cwd, `commands.${phase}.cwd`, 512);
  if (cwd.startsWith("/") || cwd.split(/[\\/]+/u).includes("..")) {
    throw new Error(`commands.${phase}.cwd must stay inside project_root`);
  }
  const timeoutMs = Number(value.timeout_ms ?? 120_000);
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1_000 || timeoutMs > 3_600_000) {
    throw new Error(`commands.${phase}.timeout_ms must be 1000-3600000`);
  }
  return {
    command: argv[0],
    args: argv.slice(1),
    cwd,
    timeout_ms: timeoutMs,
    env: {
      RUNX_RELEASE_PHASE: phase,
      RUNX_RELEASE_VERSION: identity.version,
      RUNX_RELEASE_CHANNEL: identity.channel,
      RUNX_RELEASE_PROFILE_ID: identity.profileId,
    },
  };
}

function bindPlans(context, plans) {
  if (context.schema !== "runx/release.context.v1") throw new Error("release_context is required");
  const commands = {};
  for (const phase of ["prepare", "publish", "verify"]) {
    const plan = plans[phase];
    if (plan.schema !== "runx.command.plan.v1" || !/^sha256:[0-9a-f]{64}$/u.test(text(plan.command_digest, 80))) {
      throw new Error(`${phase} command plan is required`);
    }
    commands[phase] = { ...object(context.commands?.[phase]), command_digest: plan.command_digest };
  }
  return { ...context, commands };
}

function normalizeObserved(value) {
  const source = object(value);
  return {
    status: text(source.status, 64),
    version: text(source.version, 128),
    channel: text(source.channel, 128),
    release_id: text(source.release_id, 512),
    commit: text(source.commit, 512),
    locators: Array.isArray(source.locators)
      ? source.locators.slice(0, 50).map((item) => text(item, 2_000)).filter(Boolean)
      : [],
    checks: normalizeChecks(source.checks),
  };
}

function normalizeChecks(value) {
  const source = object(value);
  return Object.fromEntries(
    Object.keys(source)
      .sort()
      .slice(0, 100)
      .map((key) => [text(key, 128), typeof source[key] === "boolean" ? source[key] : text(source[key], 500)])
      .filter(([key]) => key),
  );
}

function findForbiddenKeys(value, prefix = "") {
  const found = [];
  for (const [key, child] of Object.entries(object(value))) {
    const next = prefix ? `${prefix}.${key}` : key;
    if (/^(?:publish(?:ed|_report|_status)?|release_report|registry_url|side_effects|verification_status|provider_status)$/iu.test(key)) {
      found.push(next);
    }
    if (child && typeof child === "object") found.push(...findForbiddenKeys(child, next));
  }
  return found;
}

function normalizeChangelog(value) {
  const source = object(value);
  return {
    added: list(source.added, 100, 1_000),
    fixed: list(source.fixed, 100, 1_000),
    changed: list(source.changed, 100, 1_000),
    removed: list(source.removed, 100, 1_000),
    breaking: list(source.breaking, 100, 1_000),
  };
}

function list(value, maxItems, maxText) {
  return Array.isArray(value) ? value.slice(0, maxItems).map((item) => text(item, maxText)).filter(Boolean) : [];
}

function secretShaped(value) {
  return /(?:bearer\s+|-----begin|(?:token|password|secret|api[_-]?key)(?:=|:))/iu.test(value)
    || /^--(?:token|password|secret|api[_-]?key)(?:=|$)/iu.test(value);
}

function parseObject(value, label) {
  try {
    const parsed = JSON.parse(value);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error(`${label} must be a JSON object`);
    }
    return parsed;
  } catch (error) {
    if (String(error.message).includes("must be")) throw error;
    throw new Error(`${label} is invalid JSON`);
  }
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function requiredText(value, label, max) {
  const result = text(value, max);
  if (!result) throw new Error(`${label} is required`);
  return result;
}

function text(value, max) {
  return typeof value === "string" ? value.trim().slice(0, max) : "";
}
