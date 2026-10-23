import {
  boundedMessage,
  numberValue,
  packageSegment,
  record,
  records,
  requiredRecord,
  requiredString,
  sameBounds,
  stringValue,
  strings,
  uniqueStrings,
} from "./overlay-common.mjs";

export function prepareBinding(inputs) {
  const evidence = requiredRecord(inputs.source_evidence, "source_evidence");
  const draft = record(inputs.profile_draft);
  const markdown = typeof inputs.skill_markdown === "string" ? inputs.skill_markdown : "";
  const blockers = [];
  try {
    if (evidence.decision !== "ready") throw new Error("source evidence is not ready");
    if (draft.decision !== "ready") throw new Error("profile draft is not ready");
    enforceBounds(evidence.bounds, draft.bounds_applied);
    const profileDocument = requiredString(draft.profile_document, "profile_draft.profile_document");
    return {
      binding_candidate: {
        decision: "ready",
        source_evidence: evidence,
        rationale: requiredString(draft.rationale, "profile_draft.rationale"),
        profile_document: profileDocument,
        candidate_files: [
          { path: "SKILL.md", contents: markdown },
          { path: "run.yaml", contents: profileDocument },
        ],
        blockers: [],
      },
    };
  } catch (error) {
    blockers.push(boundedMessage(error));
    return {
      binding_candidate: {
        decision: "reject",
        source_evidence: evidence,
        rationale: stringValue(draft.rationale) || "The binding candidate failed deterministic validation.",
        blockers,
      },
    };
  }
}

export function assembleBinding(inputs) {
  const candidate = requiredRecord(inputs.binding_candidate, "binding_candidate");
  const validation = requiredRecord(inputs.skill_validation, "skill_validation");
  if (candidate.decision !== "ready") return rejectedDocuments(candidate, "binding candidate is not ready");
  if (validation.verdict !== "tested" || validation.harness?.status !== "passed") {
    return rejectedDocuments(candidate, "native skill validation or harness proof failed");
  }
  const evidence = requiredRecord(candidate.source_evidence, "source_evidence");
  const sourceEvidence = requiredRecord(evidence.source, "source_evidence.source");
  const inspection = requiredRecord(validation.inspect, "skill_validation.inspect");
  const skillName = packageSegment(inspection.name, "skill_validation.inspect.name");
  const description = stringValue(inspection.description) || "Upstream skill bound by Runx.";
  const source = { ...sourceEvidence, name: skillName, description };
  const publisher = requiredString(evidence.registry.publisher, "registry.publisher");
  const bindingPath = `bindings/${publisher}/${skillName}`;
  const skillId = `${publisher}/${skillName}`;
  const harness = requiredRecord(validation.harness, "skill_validation.harness");
  const binding = {
    schema: "runx.registry_binding.v1",
    state: "harness_verified",
    skill: { id: skillId, name: skillName, description: requiredString(source.description, "source.description") },
    upstream: evidence.upstream,
    registry: {
      publisher,
      trust_tier: evidence.registry.trust_tier,
      version: evidence.registry.version,
      install_command: `runx registry install ${skillId}@${evidence.registry.version}`,
      run_command: `runx skill ${skillId}@${evidence.registry.version}`,
      profile_path: `${bindingPath}/run.yaml`,
      materialized_package_is_registry_artifact: true,
    },
    harness: {
      status: "harness_verified",
      case_count: numberValue(harness.case_count),
      assertion_count: numberValue(harness.case_count),
      case_names: uniqueStrings(harness.case_names),
    },
    bounds: Object.keys(record(evidence.bounds)).length > 0 ? evidence.bounds : null,
    publication: evidence.publication,
    tags: uniqueStrings(evidence.tags),
  };
  return {
    binding_documents: {
      decision: "ready",
      binding_path: bindingPath,
      source,
      profile_document: requiredString(candidate.profile_document, "profile_document"),
      binding_document: `${JSON.stringify(binding, null, 2)}\n`,
      validation,
      rationale: candidate.rationale,
      blockers: [],
    },
  };
}

export function finalizeBinding(inputs) {
  const documents = record(inputs.binding_documents);
  if (documents.decision === "ready") {
    const profileDigest = requiredRecord(inputs.profile_digest, "profile_digest");
    const bindingDigest = requiredRecord(inputs.binding_digest, "binding_digest");
    return {
      binding_bundle: {
        decision: "ready",
        binding_path: documents.binding_path,
        source: documents.source,
        files: [
          fileEntry(`${documents.binding_path}/binding.json`, documents.binding_document, bindingDigest.digest),
          fileEntry(`${documents.binding_path}/run.yaml`, documents.profile_document, profileDigest.digest),
        ],
        validation: {
          status: "pass",
          inspect: documents.validation.inspect,
          harness: documents.validation.harness,
        },
        rationale: documents.rationale,
        blockers: [],
        success_checkpoint: {
          milestone: "binding_bundle_ready",
          description: "Exact native binding files passed profile inspection and isolated harness proof; repository write and publication remain separate.",
        },
      },
    };
  }
  const request = record(inputs.source_request);
  const evidence = record(inputs.source_evidence);
  const candidate = record(inputs.binding_candidate);
  const sourceEvidence = Object.keys(record(candidate.source_evidence)).length > 0 ? candidate.source_evidence : evidence;
  const blockers = [
    ...records(request.findings).map((finding) => stringValue(finding.message)).filter(Boolean),
    ...records(sourceEvidence.findings).map((finding) => stringValue(finding.message)).filter(Boolean),
    ...strings(candidate.blockers),
    ...strings(documents.blockers),
  ];
  return {
    binding_bundle: {
      decision: "reject",
      binding_path: stringValue(sourceEvidence.binding_path) || "",
      source: record(sourceEvidence.source),
      files: [],
      validation: { status: "hold" },
      rationale: stringValue(candidate.rationale) || "The binding candidate failed deterministic validation.",
      blockers: [...new Set(blockers.length > 0 ? blockers : ["The binding request is incomplete or invalid."])],
      success_checkpoint: { milestone: "binding_blocked", description: "No binding files were released." },
    },
  };
}

function enforceBounds(bounds, applied) {
  const declared = record(bounds);
  if (Object.keys(declared).length === 0) {
    if (Object.keys(record(applied)).length > 0) {
      throw new Error("profile draft applied bounds the caller did not supply");
    }
    return;
  }
  if (!sameBounds(declared, applied)) {
    throw new Error("profile draft did not apply the caller-supplied bounds exactly");
  }
}

function rejectedDocuments(candidate, reason) {
  return { binding_documents: { decision: "reject", blockers: [...strings(candidate.blockers), reason] } };
}

function fileEntry(path, contents, digest) {
  return { path, contents, content_digest: requiredString(digest, `${path} digest`) };
}
