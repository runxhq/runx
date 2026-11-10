const DEFAULT_CLASSES = [
  "name",
  "email",
  "phone",
  "postal_address",
  "government_id",
  "tax_id",
  "payment_card",
  "account_id",
  "record_id",
  "precise_geolocation",
  "date_of_birth",
];

export function scrubRedaction(inputs) {
  const content = inputs.content;
  const mode = inputs.mode;
  const locale = inputs.locale;
  const classes = policyClasses(inputs.classes);
  const draft = inputs.redaction_draft;

  let decision = draft.decision;
  let detections = [];
  let residual = risk(
    draft.residual_risk.level,
    draft.residual_risk.reason_code,
  );
  let redactedContent = "";
  let findings = [];

  try {
    detections = parseDetections(draft.detected, content, classes);

    if (mode === "block") {
      decision = "blocked";
      residual = risk("high", "policy_block");
    } else if (decision === "ready") {
      const candidate = applyDetections(content, detections, mode);
      findings = scanResidual(candidate).filter((finding) =>
        classes.includes(finding.class),
      );
      if (
        findings.length > 0 ||
        residual.level !== "low" ||
        residual.reason_code !== "none"
      ) {
        decision = "needs_review";
        if (residual.level === "low" && residual.reason_code === "none")
          residual = risk("medium", "deterministic_residual_detected");
      } else {
        redactedContent = candidate;
        residual = risk("low", "none");
      }
    }
  } catch (error) {
    decision = "blocked";
    detections = [];
    findings = [
      { class: "validation", span: [0, 0], rule: "invalid_redaction_draft" },
    ];
    residual = {
      level: "high",
      reason_code: "invalid_redaction_draft",
      reason:
        error instanceof Error
          ? error.message
          : "The redaction draft failed deterministic validation.",
    };
  }

  if (decision !== "ready") {
    redactedContent = "";
  }

  return {
    redaction_candidate: {
      decision,
      detected: detections,
      redacted_content: redactedContent,
      residual_risk: residual,
      scanner: {
        status:
          decision === "ready"
            ? "pass"
            : decision === "blocked"
              ? "block"
              : "hold",
        findings,
      },
      policy: { classes, mode, locale },
    },
  };
}

export function finalizeRedaction(inputs) {
  const candidate = inputs.redaction_candidate;
  const sourceDigest = inputs.source_digest;
  const nativeRedactedDigest = inputs.redacted_digest;
  const ready = candidate.decision === "ready";
  const redactedContent =
    ready && typeof candidate.redacted_content === "string"
      ? candidate.redacted_content
      : "";
  return {
    redaction_report: {
      decision: candidate.decision,
      detected: candidate.detected,
      source_digest: sourceDigest,
      redacted_digest: ready ? nativeRedactedDigest : null,
      residual_risk: candidate.residual_risk,
      scanner: candidate.scanner,
      policy: candidate.policy,
    },
    redacted_content: redactedContent,
  };
}

// Select exact source text; the agent chooses semantics, never character arithmetic.
function parseDetections(value, source, allowedClasses) {
  const allowed = new Set(allowedClasses),
    parsed = [];
  for (const detection of value) {
    if (!allowed.has(detection.class))
      throw new Error("Detected class is outside the selected policy.");
    const selector = detection.selector;
    if (selector.all === true && selector.occurrence !== undefined)
      throw new Error("Choose all occurrences or one occurrence, not both.");
    const matches = [];
    for (const start of quoteStarts(source, selector.quote)) {
      const end = start + selector.quote.length;
      if (
        (selector.prefix === undefined ||
          source.endsWith(selector.prefix, start)) &&
        (selector.suffix === undefined ||
          source.startsWith(selector.suffix, end))
      )
        matches.push([start, end]);
    }
    const selected =
      selector.all === true
        ? matches
        : selector.occurrence === undefined
          ? matches.length === 1
            ? matches
            : []
          : matches[selector.occurrence - 1]
            ? [matches[selector.occurrence - 1]]
            : [];
    if (!selected.length)
      throw new Error(
        "The source quote is absent, ambiguous, or its selected occurrence does not exist.",
      );
    for (const span of selected)
      parsed.push({
        class: detection.class,
        span,
        confidence: detection.confidence,
      });
  }
  parsed.sort(
    (left, right) =>
      left.span[0] - right.span[0] || left.span[1] - right.span[1],
  );
  for (let i = 1; i < parsed.length; i++)
    if (parsed[i].span[0] < parsed[i - 1].span[1])
      throw new Error("Selected personal-data spans overlap.");
  return parsed;
}

// Every place `quote` starts in `source`, overlapping ones included, in
// order. The search runs from the last back: the worker's lastIndexOf walks
// down from the position it's given, while its indexOf from a position first
// walks up to it from the string's start, so a forward search from each match
// cost the square of the source's length and this one costs its length.
function quoteStarts(source, quote) {
  const starts = [];
  let at = source.lastIndexOf(quote);
  while (at >= 0) {
    starts.push(at);
    at = at === 0 ? -1 : source.lastIndexOf(quote, at - 1);
  }
  return starts.reverse();
}

function applyDetections(source, detections, treatment) {
  const counts = new Map(),
    tokens = new Map();
  const replacements = detections.map((detection) => {
    const label = detection.class.toUpperCase();
    if (treatment !== "tokenize") return "[REDACTED:" + label + "]";
    const key = JSON.stringify([
      detection.class,
      source.slice(...detection.span),
    ]);
    if (!tokens.has(key)) {
      const count = (counts.get(label) || 0) + 1;
      counts.set(label, count);
      tokens.set(key, "[TOKEN:" + label + ":" + count + "]");
    }
    return tokens.get(key);
  });
  const chunks = [];
  let offset = 0;
  for (let i = 0; i < detections.length; i++) {
    chunks.push(source.slice(offset, detections[i].span[0]), replacements[i]);
    offset = detections[i].span[1];
  }
  chunks.push(source.slice(offset));
  return chunks.join("");
}

function scanResidual(value) {
  const findings = [];
  collect(
    findings,
    value,
    /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/giu,
    "email",
    "direct_email",
  );
  collect(
    findings,
    value,
    /\b\d{3}-\d{2}-\d{4}\b/gu,
    "government_id",
    "us_ssn",
  );
  collect(
    findings,
    value,
    /\b(?:\d[ -]*?){13,19}\b/gu,
    "payment_card",
    "payment_card_digits",
    (match) => luhn(match),
  );
  collect(
    findings,
    value,
    /(?<!\w)(?:\+?\d[\d().\s-]{6,}\d)(?!\w)/gu,
    "phone",
    "phone_digits",
    (match) => {
      const digits = match.replace(/\D/gu, "");
      return digits.length >= 10 && digits.length <= 15;
    },
  );
  collect(
    findings,
    value,
    /\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b/gu,
    "precise_geolocation",
    "ipv4_address",
  );
  collect(
    findings,
    value,
    /\b[A-Z0-9._%+-]+\s*(?:\[at\]|\(at\)|\sat\s)\s*[A-Z0-9.-]+\s*(?:\[dot\]|\(dot\)|\sdot\s)\s*[A-Z]{2,}\b/giu,
    "email",
    "obfuscated_email",
  );
  return dedupeFindings(findings);
}

function collect(
  findings,
  value,
  pattern,
  piiClass,
  rule,
  predicate = () => true,
) {
  for (const match of value.matchAll(pattern)) {
    if (!predicate(match[0])) continue;
    findings.push({
      class: piiClass,
      span: [match.index, match.index + match[0].length],
      rule,
    });
  }
}

function dedupeFindings(findings) {
  const seen = new Set();
  return findings.filter((finding) => {
    const key = `${finding.class}:${finding.span[0]}:${finding.span[1]}:${finding.rule}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function luhn(value) {
  const digits = value.replace(/\D/gu, "");
  if (digits.length < 13 || digits.length > 19) return false;
  let sum = 0;
  let double = false;
  for (let index = digits.length - 1; index >= 0; index -= 1) {
    let digit = Number(digits[index]);
    if (double) {
      digit *= 2;
      if (digit > 9) digit -= 9;
    }
    sum += digit;
    double = !double;
  }
  return sum % 10 === 0;
}

function policyClasses(value) {
  return value === undefined ? [...DEFAULT_CLASSES] : [...value].sort();
}

function risk(level, reasonCode) {
  const reasons = {
    none: "No selected-class pattern matches remain after treatment; this is not proof of complete semantic anonymity.",
    ambiguous_semantics:
      "Semantic identifiers remain ambiguous and require review.",
    scrubbing_destroys_meaning:
      "Removing the identified material would destroy the content's purpose.",
    policy_block: "The selected policy blocks boundary crossing.",
    insufficient_context:
      "The supplied context is insufficient for a safe boundary decision.",
    deterministic_residual_detected:
      "The deterministic residual scanner found identifier-shaped content.",
  };
  return {
    level,
    reason_code: reasonCode,
    reason:
      reasonCode === "none" && level !== "low"
        ? "Semantic review reports elevated residual risk; a clean pattern scan cannot clear it."
        : reasons[reasonCode],
  };
}
