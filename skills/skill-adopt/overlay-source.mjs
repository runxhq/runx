import {
  boundedMessage,
  normalizedBounds,
  numberValue,
  record,
  records,
  requiredRecord,
  requiredString,
  stringValue,
  uniqueStrings,
} from "./overlay-common.mjs";

export function planSource(inputs) {
  const findings = [];
  const rawPath = stringValue(inputs.skill_path);
  const { bounds, errors: boundsErrors } = normalizedBounds(inputs.bounds);
  for (const message of boundsErrors) {
    findings.push({ code: "bounds.invalid", path: "bounds", message });
  }
  let base = "workspace";
  let path = "";
  const label = rawPath || "";
  try {
    if (!rawPath) throw new Error("A pinned local SKILL.md path is required.");
    if (rawPath.startsWith("skill://")) {
      base = "skill";
      path = normalizeRelative(rawPath.slice("skill://".length));
    } else {
      path = normalizeRelative(rawPath);
    }
    if (!path.endsWith("SKILL.md")) throw new Error("skill_path must name a SKILL.md file");
  } catch (error) {
    findings.push({ code: "source.invalid_path", path: "skill_path", message: boundedMessage(error) });
  }
  return {
    source_request: {
      decision: findings.length === 0 ? "read" : "reject",
      base,
      path,
      label,
      upstream: record(inputs.upstream),
      registry: record(inputs.registry),
      bounds,
      tags: uniqueStrings(inputs.tags),
      publication: Object.keys(record(inputs.publication)).length > 0 ? inputs.publication : { status: "not_published" },
      findings,
    },
  };
}

export function inspectSource(inputs) {
  const request = requiredRecord(inputs.source_request, "source_request");
  const file = requiredRecord(inputs.file_read, "file_read");
  const blobDigest = requiredRecord(inputs.git_blob_digest, "git_blob_digest");
  const findings = [...records(request.findings)];
  if (file.truncated === true) {
    findings.push({ code: "source.too_large", path: "skill_path", message: "SKILL.md exceeds the bounded read limit." });
  }
  const upstream = requiredRecord(request.upstream, "upstream");
  const registry = requiredRecord(request.registry, "registry");
  const observedBlobSha = requiredString(blobDigest.digest, "git_blob_digest.digest");
  validateUpstream(upstream, observedBlobSha, findings);
  validateRegistry(registry, findings);
  return {
    source_evidence: {
      decision: findings.length === 0 ? "ready" : "reject",
      findings,
      source: {
        path: request.label,
        bytes: numberValue(blobDigest.bytes),
        sha256: requiredString(file.content_digest, "file_read.content_digest"),
        git_blob_sha: observedBlobSha,
      },
      upstream,
      registry,
      bounds: request.bounds ?? null,
      tags: uniqueStrings(request.tags),
      publication: request.publication,
    },
  };
}

function validateUpstream(value, observedBlobSha, findings) {
  if (value.host !== "github.com") {
    reject(findings, "upstream.unsupported_host", "upstream.host", "Native upstream bindings currently require github.com provenance.");
  }
  const owner = safeSegment(value.owner, "upstream.owner", findings);
  const repo = safeSegment(value.repo, "upstream.repo", findings);
  if (value.path !== "SKILL.md") {
    reject(findings, "upstream.invalid_path", "upstream.path", "The upstream source-of-truth path must be SKILL.md.");
  }
  const commit = hex(value.commit, 40, "upstream.commit", findings);
  const blobSha = hex(value.blob_sha, 40, "upstream.blob_sha", findings);
  if (blobSha && blobSha !== observedBlobSha) {
    reject(findings, "upstream.blob_mismatch", "upstream.blob_sha", "The local SKILL.md does not match the pinned upstream Git blob.");
  }
  if (value.source_of_truth !== true) {
    reject(findings, "upstream.not_source_of_truth", "upstream.source_of_truth", "The binding requires an upstream source-of-truth assertion.");
  }
  pinnedUrl(value.html_url, [owner, repo, commit, "SKILL.md"], "upstream.html_url", findings);
  pinnedUrl(value.raw_url, [owner, repo, commit, "SKILL.md"], "upstream.raw_url", findings);
}

function validateRegistry(value, findings) {
  safeSegment(value.owner, "registry.owner", findings);
  if (!["community", "verified", "first_party"].includes(value.trust_tier)) {
    reject(findings, "registry.invalid_trust_tier", "registry.trust_tier", "Registry trust tier must be community, verified, or first_party.");
  }
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/u.test(stringValue(value.version) || "")) {
    reject(findings, "registry.invalid_version", "registry.version", "Registry version must be an immutable package segment.");
  }
}

function normalizeRelative(value) {
  const raw = String(value);
  const parts = raw.replaceAll("\\", "/").split("/");
  if (parts.some((part) => part === "..") || raw.startsWith("/")) {
    throw new Error("skill_path must stay inside its source root");
  }
  const normalized = parts.filter((part) => part && part !== ".").join("/");
  if (!normalized) throw new Error("skill_path must name a file");
  return normalized;
}

function safeSegment(value, field, findings) {
  const parsed = stringValue(value);
  if (!parsed || !/^[a-z0-9][a-z0-9-]*$/u.test(parsed)) {
    reject(findings, "binding.invalid_segment", field, `${field} must be a lowercase package segment.`);
    return "invalid";
  }
  return parsed;
}

function hex(value, length, field, findings) {
  const parsed = stringValue(value);
  if (!parsed || !new RegExp(`^[a-f0-9]{${length}}$`, "iu").test(parsed)) {
    reject(findings, "binding.invalid_digest", field, `${field} must be a ${length}-character hex digest.`);
    return null;
  }
  return parsed.toLowerCase();
}

function pinnedUrl(value, parts, field, findings) {
  const parsed = stringValue(value);
  if (!parsed || parts.some((part) => part && !parsed.includes(part))) {
    reject(findings, "upstream.unpinned_url", field, "Pinned upstream URLs must include owner, repo, commit, and SKILL.md path.");
  }
}

function reject(findings, code, path, message) {
  findings.push({ code, path, message });
}
