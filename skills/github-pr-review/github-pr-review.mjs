export function prepareReview({ repository, number, expected_head, patch_char_limit, pull, files, checks }) {
  const current = pull.result.pull;
  const comparison = files.result.comparison;
  const changed = comparison.files;
  const checkResult = checks.result.checks;
  const blockers = [];
  if (current.number !== number
      || current.base?.repo?.full_name?.toLowerCase() !== repository.toLowerCase()) {
    blockers.push("provider pull-request identity does not match the selected repository and number");
  }
  if (current.head?.sha !== expected_head) {
    blockers.push("pull-request head is not the expected head; collect a new review snapshot");
  }
  if (comparison.base_commit?.sha !== current.base?.sha || !/^[0-9a-f]{40}$/u.test(comparison.merge_base_commit?.sha ?? "")) {
    blockers.push("comparison evidence does not bind to the selected base and merge base");
  }
  if (!Array.isArray(changed) || changed.length === 0 || changed.length > 100 || changed.length !== current.changed_files) {
    blockers.push("changed-file evidence is empty or incomplete");
  }
  let patchChars = 0;
  const reviewFiles = [];
  for (const file of Array.isArray(changed) ? changed.slice(0, 100) : []) {
    if (typeof file.patch !== "string" || !file.patch.trim()) {
      blockers.push(`${file.filename}: patch is unavailable; source inspection is required`);
      continue;
    }
    patchChars += file.patch.length;
    const anchors = patchAnchors(file.patch);
    if (!anchors || anchors.additions !== file.additions || anchors.deletions !== file.deletions) {
      blockers.push(`${file.filename}: patch is malformed or truncated`);
      continue;
    }
    if (reviewFiles.some((item) => item.path === file.filename)) {
      blockers.push(`${file.filename}: file evidence is duplicated`);
      continue;
    }
    reviewFiles.push({ path: file.filename, previous_path: file.previous_filename ?? null,
      status: file.status, patch: file.patch, left_lines: anchors.left, right_lines: anchors.right });
  }
  if (patchChars > patch_char_limit) blockers.push("patch evidence exceeds the selected review bound");
  if (!Array.isArray(checkResult?.check_runs)
      || checkResult.check_runs.some((check) => check.head_sha !== expected_head)) {
    blockers.push("check evidence is missing or belongs to a different commit");
  }
  return {
    review_context: {
      decision: blockers.length ? "needs_more_evidence" : "ready",
      repository, number, base_sha: current.base?.sha ?? null, head_sha: expected_head,
      diff_base_sha: comparison.merge_base_commit?.sha ?? null,
      title: current.title ?? "", description: current.body ?? "",
      files: blockers.length ? [] : reviewFiles,
      checks: Array.isArray(checkResult?.check_runs) ? checkResult.check_runs.map((check) => ({
        name: check.name, status: check.status, conclusion: check.conclusion,
      })) : [],
      checks_complete: Array.isArray(checkResult?.check_runs)
        && checkResult.check_runs.length === checkResult.total_count,
      coverage: "available_pr_patches_only",
      blockers,
    },
  };
}

export function finalizeReview({ review_context: context, context_digest, review_draft }) {
  const blockers = [...context.blockers];
  const findings = review_draft?.findings ?? [];
  for (const finding of findings) {
    const file = context.files.find((item) => item.path === finding.path);
    if (!file || !(finding.side === "LEFT" ? file.left_lines : file.right_lines).includes(finding.line)) {
      blockers.push("review finding does not bind to an admitted changed line");
    }
  }
  const ready = blockers.length === 0 && context.decision === "ready";
  return {
    pr_review: {
      decision: ready ? "draft_ready" : "needs_more_evidence",
      repository: context.repository, number: context.number,
      base_sha: context.base_sha, head_sha: context.head_sha,
      diff_base_sha: context.diff_base_sha,
      context_digest: context_digest ?? null,
      coverage: context.coverage, checks_complete: context.checks_complete,
      summary: ready ? review_draft.summary : "",
      findings: ready ? findings : [],
      limitations: ready ? review_draft.limitations : [],
      blockers, publication_status: "not_posted", execution_status: "not_tested",
    },
  };
}

// GitHub unified patch coordinates, not a repository parser or a generic validator.
function patchAnchors(patch) {
  const left = [], right = [];
  let oldLine, newLine, oldRemaining = 0, newRemaining = 0, additions = 0, deletions = 0;
  for (const line of patch.split("\n")) {
    const header = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/u.exec(line);
    if (header) {
      if (oldRemaining || newRemaining) return null;
      oldLine = Number(header[1]); newLine = Number(header[3]);
      oldRemaining = Number(header[2] ?? 1); newRemaining = Number(header[4] ?? 1);
    } else if (line.startsWith("\\ No newline")) continue;
    else if (line.startsWith("+") && newRemaining > 0) {
      right.push(newLine++); newRemaining--; additions++;
    } else if (line.startsWith("-") && oldRemaining > 0) {
      left.push(oldLine++); oldRemaining--; deletions++;
    } else if (line.startsWith(" ") && oldRemaining > 0 && newRemaining > 0) {
      oldLine++; newLine++; oldRemaining--; newRemaining--;
    } else if (line !== "") return null;
  }
  return oldRemaining || newRemaining ? null : { left, right, additions, deletions };
}
