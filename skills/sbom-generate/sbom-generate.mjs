export function makeSbom(inputs) {
  const digest = requiredDigest(inputs.lockfile_digest);
  const lockfileType = typeof inputs.lockfile_type === "string" && inputs.lockfile_type.trim()
    ? inputs.lockfile_type.trim()
    : "package-lock";
  const lockfile = inputs.lockfile;
  const findings = [];
  if (!isRecord(lockfile)) {
    findings.push({ code: "lockfile.invalid", message: "lockfile must be a parsed JSON object." });
  }
  const components = findings.length === 0 ? extractComponents(lockfile, findings) : [];
  if (findings.length === 0 && components.length === 0) {
    findings.push({ code: "lockfile.empty", message: "lockfile has no dependency map with pinned components." });
  }
  if (findings.length > 0) {
    return {
      sbom_result: {
        schema: "runx.sbom.v1",
        decision: "refused",
        lockfile_type: lockfileType,
        lockfile_digest: digest,
        sbom: null,
        component_count: 0,
        license_summary: null,
        license_risks: [],
        validation: { status: "fail", findings },
      },
    };
  }

  components.sort(
    (left, right) =>
      left.name.localeCompare(right.name) ||
      left.version.localeCompare(right.version) ||
      left.evidence_location.localeCompare(right.evidence_location),
  );
  const licenseCounts = {};
  const licenseRisks = [];
  for (const entry of components) {
    licenseCounts[entry.license] = (licenseCounts[entry.license] ?? 0) + 1;
    const risk = licenseRisk(entry);
    if (risk) licenseRisks.push(risk);
  }
  const rootPackage = isRecord(lockfile.packages) && isRecord(lockfile.packages[""]) ? lockfile.packages[""] : {};
  const projectName = firstString(rootPackage.name, lockfile.name, "unnamed-project");
  const projectVersion = firstString(rootPackage.version, lockfile.version, "0.0.0");
  return {
    sbom_result: {
      schema: "runx.sbom.v1",
      decision: "generated",
      lockfile_type: lockfileType,
      lockfile_digest: digest,
      sbom: {
        bomFormat: "CycloneDX",
        specVersion: "1.5",
        serialNumber: `urn:uuid:${digestUuid(digest)}`,
        version: 1,
        metadata: {
          component: { type: "application", name: projectName, version: projectVersion },
          properties: [
            { name: "runx:source_digest", value: digest },
            { name: "runx:lockfile_type", value: lockfileType },
          ],
        },
        components: components.map((entry) => ({
          type: "library",
          name: entry.name,
          version: entry.version,
          license: entry.license,
          properties: [{ name: "runx:evidence_location", value: entry.evidence_location }],
        })),
      },
      component_count: components.length,
      license_summary: { total_components: components.length, license_counts: sortRecord(licenseCounts) },
      license_risks: licenseRisks,
      validation: { status: "pass", findings: [] },
    },
  };
}

function extractComponents(lockfile, findings) {
  if (isRecord(lockfile.packages)) {
    return Object.entries(lockfile.packages)
      .filter(([packagePath, details]) => packagePath !== "" && isRecord(details))
      .flatMap(([packagePath, details]) => {
        const version = firstString(details.version);
        const marker = "node_modules/";
        const markerIndex = packagePath.lastIndexOf(marker);
        if (!version || markerIndex < 0) return [];
        const name = packagePath.slice(markerIndex + marker.length);
        return [component(name, version, details.license, `packages[${JSON.stringify(packagePath)}]`)];
      });
  }
  if (isRecord(lockfile.dependencies)) {
    const components = [];
    walkClassicDependencies(lockfile.dependencies, "dependencies", components);
    return components;
  }
  findings.push({ code: "lockfile.unsupported", message: "lockfile carries neither a packages map nor a dependencies map." });
  return [];
}

function walkClassicDependencies(dependencies, location, output) {
  for (const [name, details] of Object.entries(dependencies)) {
    if (!isRecord(details)) continue;
    const componentLocation = `${location}[${JSON.stringify(name)}]`;
    const version = firstString(details.version);
    if (version) output.push(component(name, version, details.license, componentLocation));
    if (isRecord(details.dependencies)) {
      walkClassicDependencies(details.dependencies, `${componentLocation}.dependencies`, output);
    }
  }
}

function component(name, version, license, evidenceLocation) {
  return { name, version, license: normalizeLicense(license), evidence_location: evidenceLocation };
}

function normalizeLicense(value) {
  if (typeof value === "string" && value.trim()) return value.trim();
  if (isRecord(value) && typeof value.type === "string" && value.type.trim()) return value.type.trim();
  return "UNKNOWN";
}

function licenseRisk(entry) {
  const license = entry.license.toUpperCase();
  const base = { component: entry.name, version: entry.version, license: entry.license, evidence_location: entry.evidence_location };
  if (license.includes("AGPL") || license.includes("GPL-3")) {
    return { ...base, risk: "high", reason: "strong copyleft license requires distribution and linking review" };
  }
  if (license.includes("LGPL") || license.includes("MPL")) {
    return { ...base, risk: "medium", reason: "weak copyleft license requires modification and relinking review" };
  }
  if (license === "UNKNOWN") {
    return { ...base, risk: "review", reason: "lockfile contains no license evidence" };
  }
  return null;
}

function digestUuid(digest) {
  const hex = digest.slice("sha256:".length, "sha256:".length + 32);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

function sortRecord(value) {
  return Object.fromEntries(Object.entries(value).sort(([left], [right]) => left.localeCompare(right)));
}

function firstString(...values) {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return values.at(-1);
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function isRecord(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
