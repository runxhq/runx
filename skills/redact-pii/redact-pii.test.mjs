import assert from "node:assert/strict";
import test from "node:test";
import { scrubRedaction } from "./redact-pii.mjs";
const detection = (quote, extra = {}, piiClass = "name") => ({
  class: piiClass,
  selector: { quote, ...extra },
  confidence: 0.95,
});
const scrub = (content, detected = [], extra = {}) =>
  scrubRedaction({
    content,
    mode: "redact",
    locale: "und",
    redaction_draft: {
      decision: "ready",
      detected,
      residual_risk: { level: "low", reason_code: "none" },
    },
    ...extra,
  }).redaction_candidate;
test("a clean scanner never clears acknowledged semantic risk", () => {
  for (const residual_risk of [
    { level: "high", reason_code: "ambiguous_semantics" },
    { level: "medium", reason_code: "insufficient_context" },
    { level: "low", reason_code: "scrubbing_destroys_meaning" },
  ]) {
    const out = scrub("The only specialist in the small clinic.", [], {
      redaction_draft: { decision: "ready", detected: [], residual_risk },
    });
    assert.equal(out.decision, "needs_review");
    assert.equal(out.redacted_content, "");
    assert.equal(out.residual_risk.level, residual_risk.level);
    assert.equal(out.residual_risk.reason_code, residual_risk.reason_code);
  }
});
test("Unicode selectors compute the source UTF-16 spans without agent arithmetic", () => {
  const source = "😀 José and Jose\u0301 work here.";
  const out = scrub(source, [detection("José"), detection("Jose\u0301")]);
  assert.equal(
    out.redacted_content,
    "😀 [REDACTED:NAME] and [REDACTED:NAME] work here.",
  );
  assert.deepEqual(
    out.detected.map((d) => source.slice(...d.span)),
    ["José", "Jose\u0301"],
  );
});
test("ambiguous repeated text is withheld unless occurrence or all is explicit", () => {
  assert.equal(
    scrub("May replied in May.", [detection("May")]).decision,
    "blocked",
  );
  const selected = scrub("May replied in May.", [
    detection("May", { occurrence: 1 }),
  ]);
  assert.equal(selected.redacted_content, "[REDACTED:NAME] replied in May.");
});
test("exact adjacent context disambiguates repeated text", () => {
  const out = scrub("May replied in May.", [
    detection("May", { prefix: "in ", suffix: "." }),
  ]);
  assert.equal(out.redacted_content, "May replied in [REDACTED:NAME].");
});
test("stable tokens preserve repeated exact class/value identity within one document", () => {
  const out = scrub(
    "Alice met Bob, then Alice replied.",
    [detection("Alice", { all: true }), detection("Bob")],
    { mode: "tokenize" },
  );
  assert.equal(
    out.redacted_content,
    "[TOKEN:NAME:1] met [TOKEN:NAME:2], then [TOKEN:NAME:1] replied.",
  );
  assert.equal(out.detected.length, 3);
});
test("missing, conflicting and overlapping selections never release partial content", () => {
  for (const detections of [
    [detection("Absent")],
    [detection("Alice", { all: true, occurrence: 1 })],
    [detection("Alice"), detection("Alice Smith")],
  ]) {
    const out = scrub("Alice Smith replied.", detections);
    assert.equal(out.decision, "blocked");
    assert.equal(out.redacted_content, "");
  }
});
test("the selected-class policy is enforced without silently broadening it", () => {
  const out = scrub("Alice uses alice@example.test.", [detection("Alice")], {
    classes: ["name"],
  });
  assert.equal(out.decision, "ready");
  assert.equal(
    out.redacted_content,
    "[REDACTED:NAME] uses alice@example.test.",
  );
  assert.equal(
    scrub("Alice", [detection("Alice")], { classes: ["email"] }).decision,
    "blocked",
  );
});
test("requested residual email patterns still hold, including obfuscation", () => {
  for (const source of [
    "Email alice@example.test.",
    "Email alice [at] example [dot] test.",
  ]) {
    const out = scrub(source);
    assert.equal(out.decision, "needs_review");
    assert.equal(out.redacted_content, "");
    assert.ok(out.scanner.findings.some((f) => f.class === "email"));
  }
});
test("an identifier ending the text is found with its exact span, by every pattern", () => {
  for (const [rule, identifier] of [
    ["direct_email", "jane@example.test"],
    ["us_ssn", "123-45-6789"],
    ["payment_card_digits", "4111 1111 1111 1111"],
    ["phone_digits", "+1 415 555 0100"],
    ["ipv4_address", "192.168.0.1"],
    ["obfuscated_email", "jane at example dot test"],
  ]) {
    const content = `Reach them at ${identifier}`;
    const out = scrub(content, [], {
      classes: ["email", "government_id", "payment_card", "phone", "precise_geolocation"],
    });
    assert.deepEqual(
      out.scanner.findings.filter((finding) => finding.rule === rule).map((finding) => finding.span),
      [[content.length - identifier.length, content.length]],
      rule,
    );
  }
});
test("an intentionally empty source can have an empty successful result", () => {
  const out = scrub("");
  assert.equal(out.decision, "ready");
  assert.equal(out.redacted_content, "");
});
test("selected text stays private and never appears in the public detection records", () => {
  const out = scrub("Alice", [detection("Alice")]);
  assert.deepEqual(Object.keys(out.detected[0]).sort(), [
    "class",
    "confidence",
    "span",
  ]);
  assert.ok(!JSON.stringify(out).includes("Alice"));
});
// The positions a search visits, counted as the worker's engine visits them:
// its indexOf from a position first walks up to that position from the
// string's start, its lastIndexOf walks down from the position it's given,
// and startsWith and endsWith at a position compare only the search.
function positionsVisited(run) {
  const { indexOf, lastIndexOf, startsWith, endsWith } = String.prototype;
  let visited = 0;
  String.prototype.indexOf = function (search, from = 0) {
    const found = indexOf.call(this, search, from);
    visited += (found < 0 ? this.length : found) + 1;
    return found;
  };
  String.prototype.lastIndexOf = function (search, from = Infinity) {
    const found = lastIndexOf.call(this, search, from);
    visited += Math.min(from, this.length) - (found < 0 ? -1 : found);
    return found;
  };
  String.prototype.startsWith = function (search, at) {
    visited += String(search).length;
    return startsWith.call(this, search, at);
  };
  String.prototype.endsWith = function (search, at) {
    visited += String(search).length;
    return endsWith.call(this, search, at);
  };
  try {
    run();
  } finally {
    Object.assign(String.prototype, { indexOf, lastIndexOf, startsWith, endsWith });
  }
  return visited;
}
test("selecting every mention visits each position a bounded number of times", () => {
  const perMention = [1000, 2000, 5000].map((count) => {
    const source = Array(count).fill("Alice").join(" ");
    const visited = positionsVisited(() =>
      scrub(source, [detection("Alice", { all: true })], { mode: "tokenize" }),
    );
    return visited / count;
  });
  assert.ok(
    perMention.every((visited) => visited <= 2 * "Alice ".length),
    `positions visited per mention at 1,000, 2,000 and 5,000: ${perMention}`,
  );
});
test("one-pass replacement remains exact at two materially different match counts", () => {
  for (const count of [200, 5000]) {
    const source = Array(count).fill("Alice").join(" ");
    const out = scrub(source, [detection("Alice", { all: true })], {
      mode: "tokenize",
    });
    assert.equal(out.decision, "ready");
    assert.equal(out.detected.length, count);
    assert.equal(
      out.redacted_content,
      Array(count).fill("[TOKEN:NAME:1]").join(" "),
    );
  }
});
