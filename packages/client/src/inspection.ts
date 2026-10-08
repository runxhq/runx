import type { ExecutionBinding, SkillInspection } from "./types.js";
import { record as isRecord } from "./decode.js";
function textOrUndefined(value: unknown): string | undefined { return typeof value === "string" && value.trim() ? value.trim() : undefined; }


export function executionBindingFromInspection(
  inspection: SkillInspection,
): ExecutionBinding {
  const runner = isRecord(inspection.runner)
    ? textOrUndefined(inspection.runner.name)
    : undefined;
  const packageDigest = textOrUndefined(inspection.package_digest);
  if (!runner || !packageDigest) {
    throw new Error("Native skill inspection omitted its selected runner or package digest.");
  }
  const executionClosureDigest = executionClosureFromInspection(inspection).closureDigest;
  return { runner, packageDigest, executionClosureDigest };
}

export function assertInspectionMatchesBinding(
  binding: ExecutionBinding,
  inspection: SkillInspection,
): void {
  const inspected = executionBindingFromInspection(inspection);
  if (inspected.runner !== binding.runner) {
    throw new Error(
      `Native inspection selected runner '${inspected.runner}' instead of bound runner '${binding.runner}'.`,
    );
  }
  if (inspected.packageDigest !== binding.packageDigest) {
    throw new Error(
      `Native inspection resolved package '${inspected.packageDigest}' instead of bound package '${binding.packageDigest}'.`,
    );
  }
  if (inspected.executionClosureDigest !== binding.executionClosureDigest) {
    throw new Error(
      `Native inspection resolved execution closure '${inspected.executionClosureDigest}' instead of bound closure '${binding.executionClosureDigest}'.`,
    );
  }
}

export interface ExecutionPackageBinding {
  readonly skill: string;
  readonly runner: string;
  readonly packageDigest: string;
  readonly sourceKind: "source_root" | "registry";
  readonly sourcePath: string;
  readonly sourceFiles: readonly string[];
}

export interface ExecutionClosureInspection {
  readonly closureDigest: string;
  readonly packageBindings: readonly ExecutionPackageBinding[];
}

export function executionClosureFromInspection(
  inspection: SkillInspection,
): ExecutionClosureInspection {
  const closure = isRecord(inspection.execution_closure)
    ? inspection.execution_closure
    : undefined;
  const closureDigest = closure
    ? textOrUndefined(closure.closure_digest)
    : undefined;
  if (!closureDigest || closure?.fully_bound !== true) {
    throw new Error(
      "Native skill inspection did not return a fully bound execution closure.",
    );
  }
  if (!Array.isArray(closure.package_bindings) || closure.package_bindings.length === 0) {
    throw new Error("Native skill inspection returned no execution-closure packages.");
  }
  const packageBindings = closure.package_bindings.map((value, index) => {
    if (!isRecord(value)) {
      throw new Error(`Native execution-closure package ${index} is not an object.`);
    }
    const skill = textOrUndefined(value.skill);
    const runner = textOrUndefined(value.runner);
    const packageDigest = textOrUndefined(value.package_digest);
    const sourceKind = value.source_kind;
    const admittedSourceKind: ExecutionPackageBinding["sourceKind"] | undefined =
      sourceKind === "source_root" || sourceKind === "registry"
      ? sourceKind
      : undefined;
    const sourcePath = textOrUndefined(value.source_path);
    const sourceFiles = stringList(value.source_files);
    if (
      !skill
      || !runner
      || !packageDigest
      || !admittedSourceKind
      || !sourcePath
      || sourceFiles.length === 0
    ) {
      throw new Error(
        `Native execution-closure package ${index} omitted skill, runner, package digest, source kind, source path, or admitted files.`,
      );
    }
    return { skill, runner, packageDigest, sourceKind: admittedSourceKind, sourcePath, sourceFiles };
  });
  return { closureDigest, packageBindings };
}

function stringList(value: unknown): readonly string[] {
  if (!Array.isArray(value) || value.some((entry) => typeof entry !== "string" || !entry.trim())) {
    return [];
  }
  return value.map((entry) => entry.trim());
}
