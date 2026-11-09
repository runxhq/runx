// GitHub review semantics only. Native operations, artifacts and pages own IO.
const sha = (value) =>
  typeof value === "string" && /^[0-9a-f]{40}([0-9a-f]{24})?$/u.test(value);
const text = (value) => (typeof value === "string" ? value : "");
const object = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const statuses = new Set([
  "added",
  "removed",
  "modified",
  "renamed",
  "copied",
  "changed",
  "unchanged",
]);
const coverage = "available_pr_patches_only";
const contextSchema = "runx/github-pr-review.context.v1";

function provider(packet, operation, repository) {
  if (
    packet?.schema !== "runx.provider.operation.v1" ||
    packet.status !== "success" ||
    packet.provider !== "github" ||
    packet.operation !== operation ||
    text(packet.target).toLowerCase() !== repository.toLowerCase()
  ) {
    throw new Error(
      `Expected completed GitHub ${operation} evidence for the selected repository.`,
    );
  }
}

export function pinPull({
  repository,
  number,
  expected_head = "",
  pull,
  pull_artifact,
  observed_at,
}) {
  provider(pull, "pullrequest.read", repository);
  const current = pull.result?.pull;
  if (!object(current) || !sha(current.head?.sha) || !sha(current.base?.sha)) {
    throw new Error(
      "GitHub did not return valid immutable pull-request commit identities.",
    );
  }
  if (
    typeof observed_at !== "string" ||
    !Number.isFinite(Date.parse(observed_at))
  )
    throw new Error("Native capture observation time is missing.");
  const blockers = [];
  if (
    current.number !== number ||
    text(current.base.repo?.full_name).toLowerCase() !==
      repository.toLowerCase()
  ) {
    blockers.push(
      "The provider pull request does not match the selected repository and number.",
    );
  }
  if (expected_head && expected_head !== current.head.sha) {
    blockers.push(
      `The actual head ${current.head.sha} differs from expected_head; no comparison was acquired.`,
    );
  }
  if (
    !Number.isSafeInteger(current.changed_files) ||
    current.changed_files < 0
  ) {
    blockers.push("The provider did not report a valid changed-file count.");
  }
  return {
    snapshot: {
      kind: "github-pr-review-snapshot",
      decision: blockers.length ? "needs_more_evidence" : "ready",
      repository,
      number,
      base_sha: current.base.sha,
      head_sha: current.head.sha,
      diff_base_sha: null,
      observed_at,
      title: text(current.title),
      description: text(current.body),
      changed_files:
        Number.isSafeInteger(current.changed_files) &&
        current.changed_files >= 0
          ? current.changed_files
          : null,
      pull_artifact,
      comparison_artifact: null,
      files_artifact: null,
      blockers,
    },
  };
}

export function bindComparison({
  snapshot,
  comparison_artifact,
  files_artifact,
  comparison_provider,
  comparison_operation,
  comparison_status,
  comparison_target,
  comparison_base,
  comparison_merge_base,
  comparison_url,
}) {
  const blockers = [...snapshot.blockers];
  const expected = `https://api.github.com/repos/${snapshot.repository}/compare/${snapshot.base_sha}...${snapshot.head_sha}`;
  if (
    comparison_provider !== "github" ||
    comparison_operation !== "commits.compare" ||
    comparison_status !== "success" ||
    text(comparison_target).toLowerCase() !==
      snapshot.repository.toLowerCase() ||
    comparison_base !== snapshot.base_sha ||
    !sha(comparison_merge_base) ||
    text(comparison_url).toLowerCase() !== expected.toLowerCase()
  ) {
    blockers.push(
      "Comparison evidence does not bind the exact selected repository, base, head and merge base.",
    );
  }
  return {
    snapshot: {
      ...snapshot,
      decision: blockers.length ? "needs_more_evidence" : "ready",
      diff_base_sha: sha(comparison_merge_base) ? comparison_merge_base : null,
      comparison_artifact,
      files_artifact,
      comparison_url: text(comparison_url),
      blockers,
    },
  };
}

export function readSnapshot({ page, expected_head = "" }) {
  if (!page.eof || page.offset !== 0 || typeof page.data !== "string")
    throw new Error("Snapshot metadata is incomplete.");
  const snapshot = Runx.parseData(page.data, "json");
  if (
    snapshot?.kind !== "github-pr-review-snapshot" ||
    !sha(snapshot.head_sha) ||
    !sha(snapshot.base_sha) ||
    typeof snapshot.repository !== "string" ||
    !Number.isSafeInteger(snapshot.number) ||
    !Array.isArray(snapshot.blockers)
  ) {
    throw new Error("Retained source is not a valid PR review snapshot.");
  }
  if (expected_head && expected_head !== snapshot.head_sha) {
    snapshot.blockers.push(
      "The retained snapshot head differs from expected_head; no new provider read was made.",
    );
    snapshot.decision = "needs_more_evidence";
  }
  return { snapshot };
}

function baseContext(snapshot) {
  return {
    schema: contextSchema,
    decision: snapshot.decision,
    repository: snapshot.repository,
    number: snapshot.number,
    base_sha: snapshot.base_sha,
    head_sha: snapshot.head_sha,
    diff_base_sha: snapshot.diff_base_sha,
    title: snapshot.title,
    description: snapshot.description,
    observed_at: snapshot.observed_at,
    files: [],
    checks: [],
    checks_complete: false,
    checks_source: null,
    coverage,
    blockers: [...snapshot.blockers],
    limitations: [],
    sources: {
      pull: snapshot.pull_artifact,
      comparison: snapshot.comparison_artifact,
      files: snapshot.files_artifact,
    },
    selection: {
      expected_changed_files: snapshot.changed_files,
      observed_files: 0,
      selected_files: 0,
      excluded_files: 0,
      gap_count: 0,
      gap_preview: [],
      gap_preview_omitted: 0,
      scan_complete: false,
      admitted_characters: 0,
      patch_char_limit: 0,
      paths: [],
      method: "source-before-tests-docs-generated",
    },
  };
}

export function heldContext({ snapshot }) {
  return { review_context: baseContext(snapshot) };
}

function priority(path) {
  if (
    /(?:^|\/)(?:vendor|node_modules|dist|generated)(?:\/|$)|(?:^|\/)(?:package-lock\.json|pnpm-lock\.yaml|Cargo\.lock)$/u.test(
      path,
    )
  )
    return 3;
  if (/(?:^|\/)(?:docs?|readme)(?:\/|\.|$)|\.(?:md|mdx|rst)$/iu.test(path))
    return 2;
  if (
    /(?:^|\/)(?:tests?|fixtures?)(?:\/|$)|\.(?:test|spec)\.[^/]+$/u.test(path)
  )
    return 1;
  return 0;
}
function inScope(path, paths) {
  return (
    !paths.length ||
    paths.some(
      (prefix) =>
        path === prefix ||
        path.startsWith(prefix.endsWith("/") ? prefix : prefix + "/"),
    )
  );
}

export function selectFilePage({
  snapshot,
  patch_char_limit = 60000,
  paths = [],
  runx_page: page,
}) {
  if (
    !sha(snapshot.diff_base_sha) ||
    snapshot.decision !== "ready" ||
    page.digest !== snapshot.files_artifact?.digest
  ) {
    throw new Error(
      "File page does not belong to the ready immutable comparison snapshot.",
    );
  }
  const state = page.state ?? {
    context: baseContext(snapshot),
    selected: [],
    seen: [],
    seen_chars: 0,
    selected_chars: 0,
    preview_chars: 0,
  };
  // The runtime supplies fresh JSON per call; only bounded selection state survives.
  const scope = state.context.selection;
  scope.patch_char_limit = patch_char_limit;
  scope.paths = paths;
  let stopped = false;
  const gap = (path, reason) => {
    scope.gap_count++;
    const row = { path, reason },
      cost = JSON.stringify(row).length;
    if (state.preview_chars + cost <= Math.min(16000, patch_char_limit)) {
      scope.gap_preview.push(row);
      state.preview_chars += cost;
    } else scope.gap_preview_omitted++;
  };
  for (const encoded of page.records) {
    const file = Runx.parseData(encoded, "json");
    const path = text(file?.filename);
    scope.observed_files++;
    if (!path || /[\u0000-\u001f]/u.test(path)) {
      gap("(invalid file path)", "Provider file metadata has no usable path.");
      continue;
    }
    if (!inScope(path, paths)) {
      scope.excluded_files++;
      continue;
    }
    if (state.seen.includes(path)) {
      const prior = state.selected.findIndex((item) => item.path === path);
      if (prior >= 0) {
        state.selected_chars -= state.selected[prior].cost;
        state.selected.splice(prior, 1);
        gap(
          path,
          "Repeated file identity was removed from the admitted review.",
        );
      }
      gap(path, "Duplicate provider file identity.");
      continue;
    }
    const seenCost = JSON.stringify(path).length;
    if (state.seen_chars + seenCost > patch_char_limit) {
      stopped = true;
      scope.observed_files--;
      state.context.limitations.push(
        "File-identity metadata reached this review's context budget; the complete retained source can be selected again with a narrower path scope.",
      );
      break;
    }
    state.seen.push(path);
    state.seen_chars += seenCost;
    if (!statuses.has(file.status)) {
      gap(path, "Unknown provider change status.");
      continue;
    }
    if (!text(file.patch).trim()) {
      gap(
        path,
        "No reviewable text patch was supplied (for example a binary or metadata-only change).",
      );
      continue;
    }
    const anchors = patchAnchors(file.patch);
    if (
      !anchors ||
      anchors.additions !== file.additions ||
      anchors.deletions !== file.deletions
    ) {
      gap(path, "The available patch is malformed or truncated.");
      continue;
    }
    if (!anchors.left.length && !anchors.right.length) {
      gap(path, "The available patch has no changed-line anchors.");
      continue;
    }
    const candidate = {
      path,
      previous_path:
        typeof file.previous_filename === "string"
          ? file.previous_filename
          : null,
      status: file.status,
      blob_sha: sha(file.sha) ? file.sha : null,
      patch: file.patch,
    };
    const cost = JSON.stringify(candidate).length,
      rank = paths.length ? 0 : priority(path);
    if (cost > patch_char_limit) {
      gap(
        path,
        "This file's patch and metadata exceed the selected review budget.",
      );
      continue;
    }
    if (state.selected_chars + cost > patch_char_limit) {
      const replaceable = state.selected
        .filter((item) => item.rank > rank)
        .sort((a, b) => b.rank - a.rank || b.cost - a.cost);
      if (
        state.selected_chars -
          replaceable.reduce((sum, item) => sum + item.cost, 0) +
          cost >
        patch_char_limit
      ) {
        gap(
          path,
          "Omitted from this bounded review selection; full source remains retained.",
        );
        continue;
      }
      for (const item of replaceable) {
        if (state.selected_chars + cost <= patch_char_limit) break;
        state.selected.splice(state.selected.indexOf(item), 1);
        state.selected_chars -= item.cost;
        gap(
          item.path,
          "A higher-priority source change took this bounded review slot.",
        );
      }
    }
    state.selected.push({ ...candidate, rank, cost });
    state.selected_chars += cost;
  }
  if (!page.eof && !stopped) return { runx_page: { state, done: false } };
  const context = state.context;
  context.files = state.selected.map(({ rank: _, cost: __, ...file }) => file);
  scope.selected_files = context.files.length;
  scope.admitted_characters = state.selected_chars;
  scope.scan_complete = page.eof && !stopped;
  if (scope.observed_files !== snapshot.changed_files) {
    context.limitations.push(
      `The comparison scan observed ${scope.observed_files} file records; the PR reported ${snapshot.changed_files} changed files. GitHub comparison responses expose at most 300 files.`,
    );
  }
  if (scope.gap_count || scope.excluded_files || !scope.scan_complete) {
    context.limitations.push(
      "This review covers the selected valid patches; omitted, unavailable and unscanned changes are not cleared.",
    );
  }
  if (!context.files.length)
    context.blockers.push(
      "No valid changed-line patch was admitted within the selected path/context budget.",
    );
  context.decision = context.blockers.length ? "needs_more_evidence" : "ready";
  return { runx_page: { state: {}, done: true }, review_context: context };
}

export function validateContext({ review_context: context }) {
  if (
    !object(context) ||
    context.schema !== contextSchema ||
    !sha(context.head_sha) ||
    !sha(context.base_sha) ||
    typeof context.repository !== "string" ||
    !Number.isSafeInteger(context.number) ||
    !Array.isArray(context.files) ||
    !Array.isArray(context.blockers) ||
    !Array.isArray(context.limitations)
  ) {
    throw new Error("Prepared PR review context is incomplete.");
  }
  const paths = new Set();
  for (const file of context.files) {
    if (
      !object(file) ||
      !text(file.path) ||
      paths.has(file.path) ||
      !statuses.has(file.status) ||
      !patchAnchors(file.patch)
    ) {
      throw new Error(
        "Prepared context has ambiguous or invalid file evidence.",
      );
    }
    paths.add(file.path);
  }
  if (context.files.length && !sha(context.diff_base_sha))
    throw new Error("Changed-line context lacks its exact diff merge base.");
  if (
    !Array.isArray(context.checks) ||
    typeof context.checks_complete !== "boolean"
  )
    throw new Error("Prepared check coverage is missing.");
  if (context.checks.length || context.checks_complete) {
    const source = context.checks_source;
    if (
      source?.head_sha !== context.head_sha ||
      !Number.isSafeInteger(source.total_count) ||
      source.total_count < 0 ||
      !object(source.artifact)
    ) {
      throw new Error(
        "Prepared check coverage lacks an exact head, reported total and source artifact.",
      );
    }
    const ids = new Set();
    for (const check of context.checks) {
      if (
        check.head_sha !== context.head_sha ||
        !text(check.name) ||
        ids.has(String(check.id)) ||
        check.id === undefined
      ) {
        throw new Error(
          "Prepared check records are duplicated or bound to another head.",
        );
      }
      ids.add(String(check.id));
    }
    if (context.checks_complete && context.checks.length !== source.total_count)
      throw new Error(
        "Prepared check records do not cover their reported total.",
      );
  }
  return { review_context: context };
}

export function finalizeReview({
  review_context: context,
  context_digest,
  review_draft = {},
}) {
  validateContext({ review_context: context });
  const blockers = [...context.blockers],
    findings = [];
  for (const finding of review_draft.findings ?? []) {
    const file = context.files.find((item) => item.path === finding.path);
    const anchors = file && patchAnchors(file.patch);
    if (
      !anchors ||
      !["LEFT", "RIGHT"].includes(finding.side) ||
      !(finding.side === "LEFT" ? anchors.left : anchors.right).includes(
        finding.line,
      )
    ) {
      blockers.push(
        "A proposed finding was withheld because it does not bind to an admitted changed line.",
      );
    } else findings.push(finding);
  }
  if (context.decision === "ready" && !text(review_draft.summary).trim())
    blockers.push(
      "No review assessment was supplied for the ready source context.",
    );
  const ready = context.decision === "ready" && blockers.length === 0;
  const limitations = [
    ...new Set([
      ...context.limitations,
      "Coverage is limited to admitted patches and any separately receipted supporting reads; this is not whole-repository or merge approval.",
      ...(!context.checks_complete
        ? [
            "The prepared context does not establish complete check-run coverage. Reviewer reads do not establish legacy commit statuses or complete CI.",
          ]
        : []),
      ...(review_draft.limitations ?? []),
    ]),
  ];
  return {
    pr_review: {
      decision: ready ? "draft_ready" : "needs_more_evidence",
      repository: context.repository,
      number: context.number,
      base_sha: context.base_sha,
      head_sha: context.head_sha,
      diff_base_sha: context.diff_base_sha,
      context_digest,
      coverage,
      checks_complete: context.checks_complete,
      summary: text(review_draft.summary),
      findings,
      limitations:
        limitations.length > 30
          ? [
              ...limitations.slice(0, 29),
              `${limitations.length - 29} additional reviewer limitations; inspect the retained assessment and source context.`,
            ]
          : limitations,
      blockers: [...new Set(blockers)],
      publication_status: "not_posted",
      execution_status: "not_tested",
    },
  };
}

export function patchAnchors(patch) {
  if (typeof patch !== "string") return null;
  const left = [],
    right = [];
  let oldLine,
    newLine,
    oldRemaining = 0,
    newRemaining = 0,
    additions = 0,
    deletions = 0,
    hunks = 0,
    oldEnd = 0,
    newEnd = 0;
  const lines = patch.split("\n");
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index],
      header = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/u.exec(line);
    if (header) {
      if (oldRemaining || newRemaining) return null;
      oldLine = Number(header[1]);
      newLine = Number(header[3]);
      oldRemaining = Number(header[2] ?? 1);
      newRemaining = Number(header[4] ?? 1);
      hunks++;
      if (
        ![
          oldLine,
          newLine,
          oldRemaining,
          newRemaining,
          oldLine + oldRemaining,
          newLine + newRemaining,
        ].every(Number.isSafeInteger) ||
        (oldRemaining && oldLine < 1) ||
        (newRemaining && newLine < 1)
      )
        return null;
      const oldStart = oldLine + Number(oldRemaining === 0),
        newStart = newLine + Number(newRemaining === 0);
      if (
        ![oldStart, newStart].every(Number.isSafeInteger) ||
        oldStart < oldEnd ||
        newStart < newEnd
      )
        return null;
      oldEnd = oldStart + oldRemaining;
      newEnd = newStart + newRemaining;
    } else if (
      line === "\\ No newline at end of file" &&
      index > 0 &&
      /^[ +\-]/u.test(lines[index - 1])
    )
      continue;
    else if (line.startsWith("+") && newRemaining > 0) {
      right.push(newLine++);
      newRemaining--;
      additions++;
    } else if (line.startsWith("-") && oldRemaining > 0) {
      left.push(oldLine++);
      oldRemaining--;
      deletions++;
    } else if (line.startsWith(" ") && oldRemaining > 0 && newRemaining > 0) {
      oldLine++;
      newLine++;
      oldRemaining--;
      newRemaining--;
    } else if (index !== lines.length - 1 || line !== "") return null;
  }
  return !hunks || oldRemaining || newRemaining
    ? null
    : { left, right, additions, deletions };
}
