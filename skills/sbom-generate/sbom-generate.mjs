export function makeSbom({ inventory, inventory_digest }) {
  if (
    inventory.schema !== "runx/sbom-generate.inventory.v1" ||
    !/^sha256:[0-9a-f]{64}$/.test(inventory_digest)
  ) {
    throw new Error(
      "A completed inventory and its native digest are required.",
    );
  }
  const components = inventory.components.map((entry) => {
    const slash =
      entry.ecosystem === "npm" && entry.name.startsWith("@")
        ? entry.name.indexOf("/")
        : -1;
    const name = slash > 0 ? entry.name.slice(slash + 1) : entry.name;
    const group = slash > 0 ? entry.name.slice(0, slash) : "";
    const component = {
      type: "library",
      "bom-ref": `lockfile:${entry.location}`,
      name,
      ...(group && { group }),
      ...(entry.version && { version: entry.version }),
      ...(entry.licenses.length && {
        licenses: entry.licenses.map((name) => ({ license: { name } })),
      }),
      properties: [
        { name: "runx:evidence_location", value: entry.location },
        { name: "runx:source_kind", value: entry.source_kind },
        { name: "runx:ecosystem", value: entry.ecosystem },
      ],
    };
    if (entry.exact_version && entry.source_kind === "registry") {
      const namespace = group ? `${encodeURIComponent(group)}/` : "";
      component.purl = `pkg:${entry.ecosystem === "npm" ? "npm" : "cargo"}/${namespace}${encodeURIComponent(name)}@${encodeURIComponent(entry.version)}`;
    }
    if (/^[0-9a-f]{64}$/.test(entry.checksum || ""))
      component.hashes = [{ alg: "SHA-256", content: entry.checksum }];
    return component;
  });
  const project = inventory.project;
  const document = {
    bomFormat: "CycloneDX",
    specVersion: "1.5",
    version: 1,
    metadata: {
      ...(project.name && {
        component: {
          type: "application",
          name: project.name,
          ...(project.version && { version: project.version }),
        },
      }),
      properties: [
        { name: "runx:source_digest", value: inventory.source.digest },
        {
          name: "runx:source_digest_encoding",
          value: inventory.source.digest_encoding,
        },
        { name: "runx:inventory_digest", value: inventory_digest },
        { name: "runx:lockfile_type", value: inventory.lockfile.type },
        { name: "runx:dependency_scope", value: inventory.scope },
        {
          name: "runx:inventory_basis",
          value: "locked declarations; installed state is not verified",
        },
      ],
    },
    components,
  };
  const counts = new Map();
  for (const entry of inventory.components)
    for (const license of entry.licenses.length
      ? entry.licenses
      : ["UNKNOWN"]) {
      counts.set(license, (counts.get(license) || 0) + 1);
    }
  return {
    inventory,
    sbom_result: {
      schema: "runx/sbom-generate.sbom.v1",
      decision: inventory.coverage.complete ? "generated" : "partial",
      source: inventory.source,
      inventory_digest,
      component_count: components.length,
      document,
      license_summary: {
        declarations: Object.fromEntries(
          [...counts].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
        ),
        missing_evidence: inventory.components.filter(
          (entry) => !entry.licenses.length,
        ).length,
      },
      caveats: [
        "Lockfile declarations are not proof of installed packages, active features or target selection.",
        "Licenses are source declarations, not verified legal conclusions; missing evidence stays unknown.",
        ...inventory.gaps.map((gap) => `${gap.code}: ${gap.message}`),
      ],
    },
  };
}

export function renderDocument({ sbom_result }) {
  if (
    sbom_result.schema !== "runx/sbom-generate.sbom.v1" ||
    sbom_result.document.bomFormat !== "CycloneDX"
  )
    throw new Error("A generated SBOM report is required.");
  return { contents: JSON.stringify(sbom_result.document, null, 2) + "\n" };
}
