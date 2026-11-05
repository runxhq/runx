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
