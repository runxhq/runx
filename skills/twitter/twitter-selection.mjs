export function prepareSelection(inputs) {
  const target = text(inputs.target) || "posts";
  const predicate = record(inputs.predicate);
  const files = [];
  const blockers = [];

  if (!new Set(["posts", "users"]).has(target)) {
    blockers.push(`target must be posts or users, not ${target}`);
  } else if (target === "posts") {
    addFile(files, blockers, "tweets", inputs.archive_file, "archive_file is required for target posts");
  } else {
    addFile(files, blockers, "following", inputs.following_file, "following_file is required for target users");
    if (predicate.non_mutual === true) {
      addFile(
        files,
        blockers,
        "followers",
        inputs.followers_file,
        "followers_file is required for the non_mutual predicate",
      );
    } else if (!Array.isArray(predicate.user_ids)) {
      blockers.push(
        "users predicate must set non_mutual: true or user_ids: [...]; refusing to unfollow everyone by default",
      );
    }
  }
  if (Object.keys(predicate).length === 0) {
    blockers.push("predicate is required; refusing to select every item by default");
  }

  return {
    twitter_selection_plan: {
      decision: blockers.length === 0 ? "ready" : "needs_input",
      target,
      predicate,
      files: target === "users" ? files : [],
      paths: target === "users" ? files.map((file) => file.path) : [],
      blockers,
    },
  };
}

export function selectUsersArchive(inputs) {
  const context = selectionContext(inputs);
  if (context.target !== "users") {
    context.blockers.push("user archive selection requires target users");
  }
  if (context.blockers.length > 0) return blockedDraft(context);
  return selectionDraft(
    context,
    selectUsers(
      context.selectionPlan,
      inputs.file_read_bundle,
      context.predicate,
      context.maxActs,
    ),
  );
}

export function selectArchivePage(inputs) {
  const context = selectionContext(inputs);
  const page = record(inputs.runx_page);
  const prior = record(page.state);
  const acts = array(prior.acts);
  let scanned = numeric(prior.scanned);
  let truncated = prior.truncated === true;

  if (context.target !== "posts") {
    context.blockers.push("paged tweet archive selection requires target posts");
  }
  for (const encoded of array(page.records)) {
    let entry;
    try {
      entry = record(JSON.parse(String(encoded)));
    } catch {
      context.blockers.push("runtime-framed archive record could not be decoded");
      break;
    }
    const tweet = record(entry.tweet ?? entry);
    scanned += 1;
    if (!matchesPost(tweet, context.predicate)) continue;
    const id = tweet.id_str ?? tweet.id;
    if (!id) continue;
    if (acts.length >= context.maxActs) {
      truncated = true;
      break;
    }
    acts.push({
      act_id: `act-del-${String(id)}`,
      kind: "delete_post",
      params: { post_id: String(id) },
      consequence: "live_mutation",
      rationale: "Matched the approved archive predicate.",
    });
  }

  const state = { acts, scanned, truncated };
  const done = context.blockers.length > 0 || truncated || page.eof === true;
  const runx_page = { state, done };
  if (!done) return { runx_page };
  if (context.blockers.length > 0) {
    return { runx_page, ...blockedDraft(context) };
  }
  return {
    runx_page,
    ...selectionDraft(context, {
      acts,
      scanned,
      evidenceLocators: [text(page.whole_digest)],
      actLabel: "delete",
      blockers: [],
      truncated,
    }),
  };
}

function selectionContext(inputs) {
  const objective = text(inputs.objective);
  const principal = text(inputs.principal);
  const selectionPlan = record(inputs.selection_plan);
  const target = text(selectionPlan.target) || "posts";
  const predicate = record(selectionPlan.predicate ?? inputs.predicate);
  const maxActs = positiveInteger(inputs.max_acts, 5000);
  const source = target === "users" ? "users_archive" : "archive";
  const blockers = [
    ...(!objective ? ["objective is required"] : []),
    ...(!principal ? ["principal is required"] : []),
    ...array(selectionPlan.blockers).map(text).filter(Boolean),
  ];
  return { objective, principal, selectionPlan, target, predicate, maxActs, source, blockers };
}

function blockedDraft(context) {
  return {
    twitter_selection_draft: selectionPacket({
      decision: "needs_input",
      objective: context.objective,
      principal: context.principal,
      source: context.source,
      predicate: context.predicate,
      blockers: context.blockers,
    }),
  };
}

function selectionDraft(context, selection) {
  if (selection.blockers.length > 0) {
    context.blockers.push(...selection.blockers);
    return blockedDraft(context);
  }

  const plan = {
    decision: "ready",
    objective: context.objective,
    principal: context.principal,
    acts: selection.acts,
    gates: { human_approval_required: true, approval_id: "approval:pending" },
    evidence_locators: selection.evidenceLocators,
    open_questions: [],
    blockers: [],
    success_checkpoint: {
      milestone: "bulk_plan_ready_for_approval",
      description: `${selection.acts.length} ${selection.actLabel} acts selected by predicate, ready for one approval and staged execution.`,
    },
  };
  return {
    twitter_selection_draft: selectionPacket({
      objective: context.objective,
      principal: context.principal,
      source: context.source,
      predicate: context.predicate,
      matched: selection.acts.length,
      scanned: selection.scanned,
      truncated: selection.truncated === true,
      twitter_plan: plan,
    }),
  };
}

export function bindSelectionDigest(inputs) {
  const draft = record(inputs.twitter_selection_draft);
  const digest = text(record(inputs.digest_result).digest);
  return {
    twitter_selection: {
      ...draft,
      plan_digest: draft.twitter_plan ? digest : "",
    },
  };
}

function selectUsers(plan, bundle, predicate, maxActs) {
  const followingArchive = archiveEntries(plan, bundle, "following");
  if (followingArchive.blockers.length > 0) {
    return emptySelection("unfollow", followingArchive.blockers);
  }
  const following = accountIds(followingArchive.entries, "following");
  const evidenceLocators = [`archive:${followingArchive.path}`];
  let selected;
  if (predicate.non_mutual === true) {
    const followersArchive = archiveEntries(plan, bundle, "followers");
    if (followersArchive.blockers.length > 0) {
      return emptySelection("unfollow", followersArchive.blockers);
    }
    const followers = new Set(accountIds(followersArchive.entries, "follower"));
    selected = following.filter((id) => !followers.has(id));
    evidenceLocators.push(`archive:${followersArchive.path}`);
  } else if (Array.isArray(predicate.user_ids)) {
    const followingSet = new Set(following);
    selected = predicate.user_ids.map(String).filter((id) => followingSet.has(id));
  } else {
    return emptySelection("unfollow", [
      "users predicate must set non_mutual: true or user_ids: [...]; refusing to unfollow everyone by default",
    ]);
  }
  const rationale = `Matched the operator predicate ${JSON.stringify(predicate)}.`;
  const acts = selected.slice(0, maxActs).map((id) => ({
    act_id: `act-unf-${id}`,
    kind: "unfollow",
    params: { target_user_id: id },
    consequence: "live_mutation",
    rationale,
  }));
  return {
    acts,
    scanned: following.length,
    evidenceLocators,
    actLabel: "unfollow",
    blockers: [],
    truncated: selected.length > acts.length,
  };
}

function archiveEntries(plan, bundle, role) {
  const archiveFile = array(plan.files).map(record).find((file) => text(file.role) === role);
  if (!archiveFile) return { path: "", entries: [], blockers: [`archive plan is missing ${role}`] };
  const path = text(archiveFile.path);
  const file = array(record(bundle).files).map(record).find((entry) => text(entry.path) === path);
  if (!file) return { path, entries: [], blockers: [`native file evidence is missing ${path}`] };
  if (file.truncated === true) {
    return { path, entries: [], blockers: [`archive file ${path} exceeded the native read limit`] };
  }
  try {
    return { path, entries: parseArchive(String(file.contents ?? "")), blockers: [] };
  } catch {
    return { path, entries: [], blockers: [`archive file ${path} is not a valid X export`] };
  }
}

function parseArchive(raw) {
  const equals = raw.indexOf("=");
  const entries = JSON.parse((equals >= 0 ? raw.slice(equals + 1) : raw).trim());
  if (!Array.isArray(entries)) throw new Error("archive file did not contain an array");
  return entries.map(record);
}

function accountIds(entries, key) {
  const ids = [];
  for (const entry of entries) {
    const value = record(entry[key] ?? entry);
    const id = value.accountId ?? value.id;
    if (id) ids.push(String(id));
  }
  return ids;
}

function matchesPost(tweet, predicate) {
  const content = text(tweet.full_text ?? tweet.text);
  const author = /^RT @([A-Za-z0-9_]+):/u.exec(content)?.[1] ?? null;
  if (predicate.is_retweet === true && !author) return false;
  if (predicate.is_retweet === false && author) return false;
  if (predicate.rt_of && text(author).toLowerCase() !== text(predicate.rt_of).toLowerCase()) return false;
  if (predicate.text_prefix && !content.startsWith(text(predicate.text_prefix))) return false;
  if (predicate.text_contains && !content.toLowerCase().includes(text(predicate.text_contains).toLowerCase())) return false;
  if (predicate.max_likes !== undefined && numeric(tweet.favorite_count) > numeric(predicate.max_likes)) return false;
  if (predicate.max_reposts !== undefined && numeric(tweet.retweet_count) > numeric(predicate.max_reposts)) return false;
  const yearMatch = /(\d{4})$/u.exec(text(tweet.created_at));
  const year = yearMatch ? Number(yearMatch[1]) : null;
  if (predicate.before_year !== undefined && !(year !== null && year < numeric(predicate.before_year))) return false;
  if (predicate.after_year !== undefined && !(year !== null && year > numeric(predicate.after_year))) return false;
  return true;
}

function selectionPacket(overrides) {
  return {
    decision: "ready",
    objective: "",
    principal: "",
    source: "archive",
    predicate: {},
    matched: 0,
    scanned: 0,
    truncated: false,
    twitter_plan: null,
    plan_digest: "",
    blockers: [],
    ...overrides,
  };
}

function emptySelection(actLabel, blockers) {
  return { acts: [], scanned: 0, evidenceLocators: [], actLabel, blockers };
}

function addFile(files, blockers, role, value, message) {
  const path = text(value);
  if (path) files.push({ role, path });
  else blockers.push(message);
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function array(value) {
  return Array.isArray(value) ? value : [];
}

function text(value) {
  return typeof value === "string" ? value.trim() : value == null ? "" : String(value).trim();
}

function numeric(value) {
  const parsed = Number(value ?? 0);
  return Number.isFinite(parsed) ? parsed : 0;
}

function positiveInteger(value, fallback) {
  const parsed = numeric(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : fallback;
}
