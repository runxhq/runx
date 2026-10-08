#!/usr/bin/env node
import {
  checkCliCommandOwnership,
  checkHttpTransportOwnership,
  checkRegistryOwnership,
} from "./runtime-architecture/delivery.mjs";
import {
  checkAuthoringOwnership,
  checkContractBindingOwnership,
  checkGeneratedMirrorOwnership,
} from "./runtime-architecture/extensions.mjs";
import { checkCanonicalParserOwnership } from "./runtime-architecture/parser.mjs";
import {
  checkExecutionSplit,
  checkProjectionHotPaths,
  checkServiceBoundary,
  checkSessionPooling,
} from "./runtime-architecture/phases.mjs";
import {
  checkCanonicalToolManifestOwnership,
  checkCloudOwnershipBoundary,
  checkCrateDependencyDirection,
  checkDeterministicWorkerOwnership,
  checkManagedAgentDefault,
  checkNoRuntimeCompatModules,
  checkNormativeArchitectureContract,
  checkTypedCapabilityPlane,
} from "./runtime-architecture/system.mjs";

const findings = [];
const phase = readOption("--phase", findings);

for (const check of [
  checkNormativeArchitectureContract,
  checkCrateDependencyDirection,
  checkCloudOwnershipBoundary,
  checkManagedAgentDefault,
  checkTypedCapabilityPlane,
  checkDeterministicWorkerOwnership,
  checkNoRuntimeCompatModules,
  checkCanonicalParserOwnership,
  checkCliCommandOwnership,
  checkRegistryOwnership,
  checkHttpTransportOwnership,
  checkAuthoringOwnership,
  checkContractBindingOwnership,
  checkGeneratedMirrorOwnership,
  checkCanonicalToolManifestOwnership,
]) {
  check(findings);
}

const phaseChecks = new Map([
  ["services", checkServiceBoundary],
  ["execution-split", checkExecutionSplit],
  ["projection-hot-paths", checkProjectionHotPaths],
  ["session-pooling", checkSessionPooling],
]);
if (phase === undefined) {
  for (const check of phaseChecks.values()) check(findings);
} else {
  const check = phaseChecks.get(phase);
  if (check) check(findings);
  else findings.push(`unknown runtime architecture phase '${phase}'`);
}

if (findings.length > 0) {
  console.error("Runtime architecture boundary check failed:");
  for (const finding of findings) console.error(`- ${finding}`);
  process.exit(1);
}

console.log(
  phase
    ? `Runtime architecture boundary check passed for ${phase}.`
    : "Runtime architecture boundary check passed.",
);

function readOption(name, errors) {
  const index = process.argv.indexOf(name);
  if (index < 0) return undefined;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) {
    errors.push(`${name} requires a value`);
    return undefined;
  }
  return value;
}
