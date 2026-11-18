/**
 * Whether the draft recalls from memory first: the levels it was given, and
 * what it's about as the index's focus. Nothing is recalled without levels.
 */
export function recallPlan({ memory, focus }) {
  if (!Array.isArray(memory) || memory.length === 0) return { memory_recall: { path: "none" } };
  return { memory_recall: { path: "index", levels: memory, ...(typeof focus === "string" && focus.trim() ? { focus: focus.trim() } : {}) } };
}

export function selectEvidence({ evidence_pack, facts }) {
  const research = evidence_pack !== undefined;
  const supplied = facts !== undefined;
  return {
    selected_source: {
      kind:
        research && supplied
          ? "conflicting_sources"
          : research
            ? "research_packet"
            : supplied
              ? "operator_supplied_facts"
              : "missing",
      evidence:
        research && supplied
          ? { evidence_pack, facts }
          : research
            ? evidence_pack
            : supplied
              ? { facts }
              : {},
    },
  };
}

export function prepareEvidence(inputs) {
  const objective = text(inputs.objective);
  const audience = text(inputs.audience);
  const channel = text(inputs.channel);
  const evidence = object(inputs.evidence_pack);
  const evidencePacketDigest = text(inputs.evidence_packet_digest);
  const supplied = inputs.evidence_kind === "operator_supplied_facts";
  const blockers = [];

  if (!objective) blockers.push("objective is missing");
  if (!audience) blockers.push("audience is missing");
  if (!channel) blockers.push("channel is missing");
  if (inputs.evidence_kind === "conflicting_sources")
    blockers.push("supply either facts or a research packet, not both");
  else if (inputs.evidence_kind === "missing")
    blockers.push("supply facts or a ready research packet");
  else if (!supplied) {
    if (evidence.decision !== "ready")
      blockers.push("evidence decision is not ready");
    if (object(evidence.validation).status !== "pass")
      blockers.push("evidence validation did not pass");
  }

  const sourceEvidence = supplied
    ? [{ evidence_digest: evidencePacketDigest }]
    : Array.isArray(evidence.source_evidence)
      ? evidence.source_evidence
      : [];
  const sourceDigests = [
    ...new Set(
      sourceEvidence
        .map((source) => text(source?.evidence_digest))
        .filter(isDigest),
    ),
  ];
  if (sourceDigests.length === 0)
    blockers.push("evidence has no source digests");
  const evidenceLog = supplied
    ? evidence.facts.map((claim) => ({
        claim,
        source_digest: evidencePacketDigest,
        content_digest: evidencePacketDigest,
        confidence: "operator_supplied",
        relevance:
          "Factual material supplied for this writing task; not independently verified.",
      }))
    : Array.isArray(evidence.evidence_log)
      ? evidence.evidence_log
      : [];
  if (evidenceLog.length === 0) blockers.push("evidence log is empty");
  for (const entry of evidenceLog) {
    if (!sourceDigests.includes(text(entry?.source_digest))) {
      blockers.push("evidence log cites an unknown source digest");
    }
  }

  const scope = { audience, channel };
  const writingContexts = [
    admitWritingContext(
      inputs.brand_context,
      "brand_voice",
      inputs.brand_context_digest,
      scope,
      blockers,
    ),
    admitWritingContext(
      inputs.taste_context,
      "taste_profile",
      inputs.taste_context_digest,
      scope,
      blockers,
    ),
  ].filter(Boolean);

  if (!isDigest(evidencePacketDigest))
    blockers.push("native evidence packet digest is missing");

  return {
    evidence_context: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      objective,
      audience,
      channel,
      evidence_packet_digest: evidencePacketDigest,
      evidence_assurance: blockers.length
        ? "not_admitted"
        : supplied
          ? "operator_supplied_facts"
          : "validated_research_packet",
      research_brief: object(evidence.research_brief),
      evidence_log: evidenceLog,
      decision_support: Array.isArray(evidence.decision_support)
        ? evidence.decision_support
        : [],
      source_digests: sourceDigests,
      writing_contexts: writingContexts,
      context_requirements: writingContexts.map(
        ({
          type,
          packet_digest,
          scope,
          applicability,
          rules,
          stop_conditions,
        }) => ({
          type,
          packet_digest,
          scope,
          applicability,
          rules,
          stop_conditions,
        }),
      ),
      blockers: [...new Set(blockers)],
    },
  };
}

export function prepareContent(inputs) {
  const channel = text(inputs.channel);
  const packet = object(inputs.draft);
  const draft = object(packet.draft);
  const binding = object(packet.evidence_binding);
  const evidencePacketDigest = text(binding.evidence_packet_digest);
  const sourceDigests = strings(binding.source_digests);
  const blockers = [];
  const priorContextRequirements = Array.isArray(
    object(packet.context_binding).required,
  )
    ? packet.context_binding.required
    : [];
  if (priorContextRequirements.length) {
    const reviewed = reviewContextUse({
      evidence_context: {
        context_requirements: priorContextRequirements,
        blockers: [],
      },
      content_draft: packet,
    });
    blockers.push(...reviewed.review_context.blockers);
  }
  if (!channel) blockers.push("channel is missing");
  if (
    packet.decision !== "ready" ||
    object(packet.validation).status !== "pass"
  ) {
    blockers.push("draft is not ready and validated");
  }
  if (!text(draft.title) || !text(draft.body))
    blockers.push("draft has no title and body");
  if (
    !isDigest(evidencePacketDigest) ||
    sourceDigests.length === 0 ||
    !sourceDigests.every(isDigest)
  ) {
    blockers.push("draft is not bound to its evidence");
  }
  const claims = Array.isArray(draft.claims) ? draft.claims : [];
  if (
    claims.some((claim) => {
      const cited = strings(object(claim).source_digests);
      return (
        cited.length === 0 ||
        cited.some((digest) => !sourceDigests.includes(digest))
      );
    })
  ) {
    blockers.push("a claim cites a source the draft is not bound to");
  }
  const ready = blockers.length === 0;
  const payload = ready
    ? { title: text(draft.title), body: text(draft.body) }
    : { title: "", body: "" };
  return {
    publish_draft: {
      decision: ready ? "ready_for_handoff" : "needs_input",
      channel,
      headline: payload.title,
      body: payload.body,
      evidence_binding: ready
        ? {
            evidence_packet_digest: evidencePacketDigest,
            source_digests: sourceDigests,
          }
        : {},
      delivery_status: "not_sent",
      blockers,
    },
    digest_input: ready ? JSON.stringify({ channel, payload }) : "",
  };
}

export function bindContent(inputs) {
  const draft = object(inputs.publish_draft);
  const ready = draft.decision === "ready_for_handoff";
  const payloadDigest = text(object(inputs.digest_result).digest);
  if (ready && !isDigest(payloadDigest))
    throw new Error("native payload digest is missing");
  return {
    content_publish_packet: {
      ...draft,
      payload_digest: ready ? payloadDigest : "",
    },
  };
}

export function prepareHandoff(inputs) {
  const channel = text(inputs.channel);
  const boundaryKind = inputs.boundary_kind;
  const packet = object(inputs.packet);
  const target = object(inputs.target);
  const blockers = [];
  if (
    packet.decision !== "ready_for_handoff" ||
    packet.delivery_status !== "not_sent"
  ) {
    blockers.push("publication packet is not ready and unsent");
  }
  if (!isDigest(text(packet.payload_digest)))
    blockers.push("publication packet has no payload digest");
  if (!channel || text(packet.channel) !== channel)
    blockers.push("publication packet is for another channel");
  if (Object.keys(target).length === 0) blockers.push("target is missing");
  const ready = blockers.length === 0;
  const external = boundaryKind === "external";
  return {
    handoff_packet: {
      decision: ready ? "handoff_ready" : "needs_input",
      channel,
      payload_digest: text(packet.payload_digest),
      packet,
      target,
      delivery_status: "not_sent",
      blockers,
    },
    boundary_state: {
      boundary_kind: boundaryKind,
      completion_state: ready ? "handoff_ready" : "needs_input",
      next_actor: ready ? "governed_provider_adapter" : "operator",
      approval_required_for_delivery: external,
      ack_expected: external,
    },
  };
}

// The applicability field each writing-context packet declares its channels in:
// runx/brand-voice.context.v1 names `channels`, runx/taste-profile.context.v1 names `surfaces`.
const CONTEXT_CHANNEL_FIELD = {
  brand_voice: "channels",
  taste_profile: "surfaces",
};

function admitWritingContext(raw, type, packetDigest, scope, blockers) {
  if (raw === undefined || raw === null) return null;
  const packet = object(raw);
  if (
    packet.decision !== "ready" ||
    object(packet.validation).status !== "pass"
  ) {
    blockers.push(`${type} context is not ready and validated`);
  }
  const bindings = Array.isArray(object(packet.evidence).bindings)
    ? object(packet.evidence).bindings
    : [];
  const rules = [
    ...new Set(bindings.map((binding) => text(binding?.rule)).filter(Boolean)),
  ];
  if (rules.length === 0) blockers.push(`${type} context has no bounded rules`);
  const admittedDigest = text(packetDigest);
  if (!isDigest(admittedDigest))
    blockers.push(`${type} native context digest is missing`);
  const applicability = object(packet.applicability);
  if (!text(applicability.audience))
    blockers.push(`${type} context has no declared audience`);
  if (strings(applicability[CONTEXT_CHANNEL_FIELD[type]]).length === 0)
    blockers.push(`${type} context has no declared channel or surface`);
  return {
    type,
    packet_digest: admittedDigest,
    scope: { audience: scope.audience, channel: scope.channel },
    applicability,
    profile: object(packet[type]),
    evidence_bindings: bindings,
    rules,
    stop_conditions: strings(packet.stop_conditions),
  };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function strings(value) {
  return Array.isArray(value) ? value.map(text).filter(Boolean) : [];
}

function isDigest(value) {
  return /^sha256:[0-9a-f]{64}$/u.test(value);
}

// Source readiness and digests are mechanical; applicability of prose-scoped
// writing guidance is the writer’s bounded judgment, recorded for each context.
/** Numbers written in words, by what each is worth. */
const ONES = Object.fromEntries(
  "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen"
    .split(" ")
    .map((word, value) => [word, value]),
);
const TENS = { twenty: 20, thirty: 30, forty: 40, fifty: 50, sixty: 60, seventy: 70, eighty: 80, ninety: 90 };
const SCALES = { thousand: 1e3, million: 1e6, billion: 1e9 };

/** What a number measures when "one" or "a" states it: "one" is otherwise mostly a pronoun. */
const MEASURES = new Set(["second", "minute", "hour", "day", "week", "fortnight", "month", "quarter", "year", "percent", "dollar", "euro", "pound", "cent"]);

/** Words that name nothing a number counts. */
const FUNCTION_WORDS = new Set(
  ("a an the of in on at to for from by with about as and or but nor so than then since per into onto over under after before " +
    "between through during within without across around is are was were be been being has have had do does did will would " +
    "can could should may might must shall it its this that these those we our ours you your they their them he she his her " +
    "i my me us who whom which what when where why how more less most least other another each every any some all no not " +
    "only just also very too up out off if because while till until ago now").split(" "),
);

function isNumberWord(word) {
  return word in ONES || word in TENS || word in SCALES || word === "hundred" || word === "dozen";
}

function isContent(word) {
  return /^[\p{L}']+$/u.test(word ?? "") && !FUNCTION_WORDS.has(word) && !isNumberWord(word);
}

/** A word as what it counts, whatever its number: "pipelines" and "pipeline's" are "pipeline". */
function singular(word) {
  return word.replace(/'s?$/u, "").replace(/ies$/u, "y").replace(/(?<!s)s$/u, "");
}

/** The number a run of number words from `from` states, as "two hundred and fifty", and where it ends. */
function spelled(tokens, from) {
  let total = 0;
  let current = 0;
  let end = from;
  for (let at = from; at < tokens.length; at += 1) {
    const word = tokens[at];
    if (word === "and" && at > from && isNumberWord(tokens[at + 1])) continue;
    if (word in ONES) current += ONES[word];
    else if (word in TENS) current += TENS[word];
    else if (word === "hundred" || word === "dozen") current = (current || 1) * (word === "hundred" ? 100 : 12);
    else if (word in SCALES) {
      total += (current || 1) * SCALES[word];
      current = 0;
    } else break;
    end = at + 1;
  }
  return { value: total + current, end };
}

/**
 * The quantities a text states, each a number and what it counts: a
 * figure's currency or percent, else the word after it. `window`, the words
 * after it, is what a quantity of the same number stated in other words may
 * count. `admitting` reads a text for every quantity it holds, "an hour"
 * among them; a draft's "one" counts only where it measures.
 */
function quantitiesIn(value, admitting) {
  // A sentence's end is a token of its own, so a number ending one never
  // counts the first word of the next: "since 2024. Sam" counts no Sams, nor
  // does a sentence closed inside a quotation or brackets, "since 2024.” Sam".
  const tokens = text(value).toLowerCase().match(/[$€£¥]?\d+(?:[.,:]\d+)*%?|[\p{L}']+|[.!?;](?=["'”’)\]]*(?:\s|$))/gu) ?? [];
  const found = [];
  for (let at = 0; at < tokens.length; at += 1) {
    const figure = /^([$€£¥]?)(\d+(?:[.,:]\d+)*)(%?)$/u.exec(tokens[at]);
    let number;
    let end;
    if (figure) {
      const plain = /^(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?$/u.test(figure[2]);
      number = plain ? Number(figure[2].replaceAll(",", "")) : figure[2];
      end = at + 1;
      // A figure with its scale in words, as "2 million".
      while (typeof number === "number" && tokens[end] in SCALES) number *= SCALES[tokens[end++]];
    } else if (isNumberWord(tokens[at])) {
      ({ value: number, end } = spelled(tokens, at));
    } else if (admitting && (tokens[at] === "a" || tokens[at] === "an") && MEASURES.has(singular(tokens[at + 1] ?? ""))) {
      number = 1;
      end = at + 1;
    } else continue;
    const symbol = figure ? figure[1] || figure[3] || null : null;
    const following = tokens.slice(end, end + 4);
    const stop = following.findIndex((token) => /^[.!?;]$/u.test(token));
    const after = stop < 0 ? following : following.slice(0, stop);
    const unit = symbol ?? (isContent(after[0]) ? singular(after[0]) : null);
    const said = tokens.slice(at, end).join(" ") + (!symbol && unit ? ` ${after[0]}` : "");
    const one = !figure && end === at + 1 && tokens[at] === "one";
    if (admitting || !one || MEASURES.has(unit)) {
      found.push({ value: number, unit, window: [symbol, ...after.filter(isContent).map(singular)].filter(Boolean), said });
    }
    at = end - 1;
  }
  return found;
}

/**
 * The quantities a draft states that nothing admitted holds: not the
 * evidence's claims, the brief, nor the text the caller gives it to carry
 * unchanged. A quantity is held when something admitted states its number
 * counting what it counts, so "40 pipelines" never holds "40 dollars" nor
 * "two" and "a million" "two million". A time, a price, a date or a metric
 * a writer made up is one of them.
 */
export function inventedNumbers(context, draft, quoted = []) {
  const admitted = [
    context.objective,
    context.audience,
    object(context.research_brief).summary,
    ...(Array.isArray(context.evidence_log) ? context.evidence_log.map((entry) => entry?.claim) : []),
    ...strings(quoted),
  ].flatMap((value) => quantitiesIn(value, true));
  const held = (stated) =>
    admitted.some((known) => known.value === stated.value && (stated.unit === null || known.window.includes(stated.unit)));
  const stated = [draft.title, draft.body].flatMap((value) => quantitiesIn(value, false));
  return [...new Set(stated.filter((quantity) => !held(quantity)).map((quantity) => quantity.said))];
}

export function reviewContextUse({ evidence_context, content_draft, quoted }) {
  const context = object(evidence_context);
  const candidate = object(content_draft);
  const bindings = Array.isArray(object(candidate.context_binding).applied)
    ? candidate.context_binding.applied
    : [];
  const requirements = Array.isArray(context.context_requirements)
    ? context.context_requirements
    : [];
  const blockers = strings(context.blockers);
  if (candidate.decision !== "ready") {
    blockers.push(...strings(candidate.review_checklist));
    if (blockers.length === 0)
      blockers.push(
        `The writer returned ${text(candidate.decision) || "no decision"}: the evidence supports no draft.`,
      );
  } else {
    const invented = inventedNumbers(context, object(candidate.draft), quoted);
    if (invented.length > 0)
      blockers.push(
        `The draft states ${invented.join(", ")}, which nothing admitted holds: a number comes from the evidence.`,
      );
  }
  for (const requirement of requirements) {
    const matching = bindings.filter(
      (binding) =>
        text(binding?.packet_digest) === text(requirement?.packet_digest),
    );
    const review = object(matching[0]?.scope_review);
    if (matching.length !== 1 || !text(review.reason))
      blockers.push(
        `Review applicability once for the admitted ${text(requirement?.type)} context and record its reason.`,
      );
    else if (review.decision !== "applies")
      blockers.push(
        `${text(requirement?.type)} context applicability is ${text(review.decision) || "unresolved"}: ${text(review.reason)}`,
      );
  }
  // A withheld draft reaches verification whole: a writer that withheld
  // wrote no draft, so it's an empty one, and nothing it binds.
  const draft = object(candidate.draft);
  return {
    reviewed_draft: blockers.length
      ? {
          ...candidate,
          decision: "needs_more_evidence",
          draft: {
            title: text(draft.title),
            body: text(draft.body),
            claims: Array.isArray(draft.claims) ? draft.claims : [],
          },
          context_binding: { ...object(candidate.context_binding), applied: bindings },
        }
      : candidate,
    review_context: { ...context, blockers: [...new Set(blockers)] },
  };
}
