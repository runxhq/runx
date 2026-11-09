const record = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : {};
const text = (value) => (typeof value === "string" ? value.trim() : "");
const exact = (value) =>
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(
    text(value),
  );
const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const pointer = (value) => value.replaceAll("~", "~0").replaceAll("/", "~1");
const licenses = (value) =>
  text(value)
    ? [text(value)]
    : text(record(value).type)
      ? [text(value.type)]
      : [];

export function fromFile(inputs) {
  const page = record(inputs.page);
  if (
    page.eof !== true ||
    page.offset !== 0 ||
    page.encoding !== "utf8" ||
    typeof page.data !== "string"
  ) {
    throw new Error(
      "A complete admitted UTF-8 lockfile is required; do not normalize a truncated page. Reuse a completed inventory or select a source within the declared worker budget.",
    );
  }
  return fromSource({
    ...inputs,
    source_text: page.data,
    source_digest: page.digest,
    source_bytes: page.length,
    digest_encoding: "source_bytes",
  });
}

export function fromSource(inputs) {
  const path = text(inputs.path) || text(inputs.source_label);
  const type =
    text(inputs.lockfile_type) ||
    {
      "package-lock.json": "package-lock",
      "npm-shrinkwrap.json": "npm-shrinkwrap",
      "pnpm-lock.yaml": "pnpm",
      "Cargo.lock": "cargo",
    }[path.split("/").at(-1)];
  const format = {
    "package-lock": "json",
    "npm-shrinkwrap": "json",
    pnpm: "yaml",
    cargo: "toml",
  }[type];
  if (!format)
    throw new Error(
      "Select package-lock, npm-shrinkwrap, pnpm or cargo for this source.",
    );
  const lockfile = Runx.parseData(inputs.source_text, format);
  return normalize({
    ...inputs,
    lockfile,
    lockfile_type: type,
    digest_encoding: inputs.digest_encoding || "utf8_text",
  });
}

export function normalize(inputs) {
  const lock = record(inputs.lockfile);
  const type = text(inputs.lockfile_type) || "package-lock";
  const scope = text(inputs.scope) || "all-locked";
  const digest = text(inputs.source_digest);
  if (!/^sha256:[0-9a-f]{64}$/.test(digest))
    throw new Error("Native source digest is required.");
  if (!["all-locked", "direct-production"].includes(scope))
    throw new Error("Select all-locked or direct-production.");
  const components = [],
    gaps = [];
  const gap = (code, location, message) =>
    gaps.push({ code, location, message });
  const add = (entry) => {
    const version = text(entry.version);
    if (!text(entry.name)) {
      gap("unresolved_name", entry.location, "Dependency name is unavailable.");
      return;
    }
    if (entry.source_kind !== "local" && !exact(version)) {
      gap(
        "unresolved_version",
        entry.location,
        "Dependency has no supported exact registry version; retained as an unqueryable component.",
      );
    }
    components.push({
      ...entry,
      version: version || null,
      licenses: entry.licenses || [],
      exact_version: exact(version),
    });
  };
  let version,
    project = {};
  if (["package-lock", "npm-shrinkwrap"].includes(type)) {
    version = lock.lockfileVersion;
    if (![1, 2, 3].includes(version))
      throw new Error("Supported npm lockfileVersion values are 1, 2 and 3.");
    const packages = record(lock.packages),
      root = record(packages[""]);
    project = {
      ...(text(lock.name) && { name: text(lock.name) }),
      ...(text(lock.version) && { version: text(lock.version) }),
      ...(text(root.name) && { name: text(root.name) }),
      ...(text(root.version) && { version: text(root.version) }),
    };
    const declared = new Set([
      ...Object.keys(record(root.dependencies)),
      ...Object.keys(record(root.optionalDependencies)),
    ]);
    if (version >= 2) {
      if (!Object.hasOwn(packages, ""))
        throw new Error("The npm lockfile packages map needs its root entry.");
      for (const [path, raw] of Object.entries(packages)) {
        if (!path) continue;
        const entry = record(raw),
          marker = path.lastIndexOf("node_modules/");
        const alias = marker >= 0 ? path.slice(marker + 13) : "";
        const direct = marker === 0 && declared.has(alias);
        if (scope === "direct-production" && !direct) continue;
        const location = `/packages/${pointer(path)}`;
        if (entry.link === true) {
          add({
            name: alias || text(entry.name) || path,
            version: null,
            ecosystem: "npm",
            location,
            source_kind: "local",
            direct,
            development: entry.dev === true,
            licenses: [],
          });
          continue;
        }
        if (marker < 0) continue; // workspace package metadata, not a second installed dependency
        const source = text(entry.resolved);
        const source_kind = /^(file:|link:|workspace:)/.test(source)
          ? "local"
          : /^(git\+|git:|github:)/.test(source)
            ? "git"
            : source.startsWith("https://registry.npmjs.org/")
              ? "registry"
              : source
                ? "other-source"
                : "unknown";
        add({
          name: text(entry.name) || alias,
          version: entry.version,
          ecosystem: "npm",
          location,
          source_kind,
          direct,
          development: entry.dev === true,
          licenses: licenses(entry.license),
        });
      }
      if (scope === "direct-production")
        for (const name of declared) {
          if (!Object.hasOwn(packages, `node_modules/${name}`))
            gap(
              "missing_direct_resolution",
              `/packages//dependencies/${pointer(name)}`,
              "Declared production dependency has no root lockfile resolution.",
            );
        }
    } else {
      if (!Object.hasOwn(lock, "dependencies"))
        throw new Error("npm lockfileVersion 1 needs a dependencies map.");
      if (scope !== "all-locked")
        throw new Error(
          "npm lockfileVersion 1 hoists transitive packages beside direct ones, so it cannot establish direct-production scope. Use all-locked.",
        );
      const queue = [{ values: record(lock.dependencies), path: "/dependencies" }];
      for (let i = 0; i < queue.length; i++)
        for (const [name, raw] of Object.entries(queue[i].values)) {
          const entry = record(raw),
            location = `${queue[i].path}/${pointer(name)}`;
          {
            const source = text(entry.resolved),
              source_kind = source.startsWith("https://registry.npmjs.org/")
                ? "registry"
                : /^(file:|link:)/.test(source)
                  ? "local"
                  : "unknown";
            add({
              name: text(entry.name) || name,
              version: entry.version,
              ecosystem: "npm",
              location,
              source_kind,
              direct: null,
              development: entry.dev === true,
              licenses: licenses(entry.license),
            });
          }
          queue.push({
            values: record(entry.dependencies),
            path: `${location}/dependencies`,
          });
        }
    }
  } else if (type === "pnpm") {
    version = String(lock.lockfileVersion);
    if (version !== "9.0")
      throw new Error("Supported pnpm lockfileVersion is 9.0.");
    if (
      !Object.hasOwn(lock, "importers") ||
      !lock.importers ||
      typeof lock.importers !== "object" ||
      Array.isArray(lock.importers)
    )
      throw new Error("pnpm 9 needs an importers map.");
    const packages = record(lock.packages),
      snapshots = record(lock.snapshots),
      direct = new Map(),
      snapshotLocations = new Map();
    for (const key of Object.keys(snapshots)) {
      const identity = key.split("(")[0],
        locations = snapshotLocations.get(identity) || [];
      locations.push(`/snapshots/${pointer(key)}`);
      snapshotLocations.set(identity, locations);
    }
    for (const [importer, raw] of Object.entries(record(lock.importers))) {
      for (const field of [
        "dependencies",
        "optionalDependencies",
        ...(scope === "all-locked" ? ["devDependencies"] : []),
      ])
        for (const [alias, rawRef] of Object.entries(
          record(record(raw)[field]),
        )) {
          const ref = text(record(rawRef).version);
          if (/^(link:|workspace:|file:)/.test(ref)) {
            add({
              name: alias,
              version: null,
              ecosystem: "npm",
              location: `/importers/${pointer(importer)}/${field}/${pointer(alias)}`,
              source_kind: "local",
              direct: true,
              development: field === "devDependencies",
            });
          } else {
            const identity = (
              exact(ref.split("(")[0]) ? `${alias}@${ref}` : ref
            ).split("(")[0];
            const locations = direct.get(identity) || [];
            locations.push(
              `/importers/${pointer(importer)}/${field}/${pointer(alias)}`,
            );
            direct.set(identity, locations);
          }
        }
    }
    for (const [identity, raw] of Object.entries(packages)) {
      const at = identity.lastIndexOf("@"),
        entry = record(raw),
        resolution = record(entry.resolution);
      const name = text(entry.name) || identity.slice(0, at),
        resolvedVersion = text(entry.version) || identity.slice(at + 1);
      if (scope === "direct-production" && !direct.has(identity)) continue;
      const tarball = text(resolution.tarball);
      // pnpm's integrity-only resolution is registry-backed. This identifies
      // the source kind, never the registry host or package visibility.
      const source_kind =
        resolution.type === "git"
          ? "git"
          : resolution.type === "directory"
            ? "local"
            : tarball
              ? /^(file:|link:|workspace:)/.test(tarball)
                ? "local"
                : tarball.startsWith("https://registry.npmjs.org/")
                  ? "registry"
                  : "other-source"
              : text(resolution.type)
                ? "other-source"
                : text(resolution.integrity) && exact(resolvedVersion)
                  ? "registry"
                  : "unknown";
      add({
        name,
        version: resolvedVersion,
        ecosystem: "npm",
        location: `/packages/${pointer(identity)}`,
        source_kind,
        direct: direct.has(identity),
        development: null,
        licenses: licenses(entry.license),
        snapshot_locations: snapshotLocations.get(identity) || [],
        direct_locations: direct.get(identity) || [],
      });
    }
    for (const identity of direct.keys())
      if (!Object.hasOwn(packages, identity))
        gap(
          "missing_direct_resolution",
          "/importers",
          "A declared registry dependency has no package metadata entry.",
        );
    if (scope === "all-locked")
      gaps.push({
        code: "platform_selection_not_applied",
        location: "/snapshots",
        message:
          "The inventory describes locked variants across platforms and peer contexts; it is not proof of installed packages.",
      });
  } else if (type === "cargo") {
    version = lock.version;
    if (![3, 4].includes(version) || !Array.isArray(lock.package))
      throw new Error(
        "Supported Cargo.lock versions are 3 and 4 with package entries.",
      );
    if (scope !== "all-locked")
      throw new Error(
        "Cargo direct-production scope requires Cargo metadata; Cargo.lock alone cannot establish it. Use all-locked.",
      );
    for (const [index, raw] of lock.package.entries()) {
      const entry = record(raw),
        source = text(entry.source);
      const source_kind = !source
        ? "local"
        : [
              "registry+https://github.com/rust-lang/crates.io-index",
              "sparse+https://index.crates.io/",
            ].includes(source)
          ? "registry"
          : source.startsWith("git+")
            ? "git"
            : "other-source";
      add({
        name: entry.name,
        version: entry.version,
        ecosystem: "crates.io",
        location: `/package/${index}`,
        source_kind,
        direct: null,
        development: null,
        licenses: [],
        ...(text(entry.checksum) && { checksum: text(entry.checksum) }),
      });
    }
    gaps.push({
      code: "manifest_context_absent",
      location: "/package",
      message:
        "Cargo.lock does not establish selected features, target activation, direct/development membership or license declarations.",
    });
  } else throw new Error("Unsupported lockfile type.");
  components.sort((a, b) =>
    compare(
      `${a.ecosystem}\0${a.name}\0${a.version || ""}\0${a.location}`,
      `${b.ecosystem}\0${b.name}\0${b.version || ""}\0${b.location}`,
    ),
  );
  return {
    inventory: {
      schema: "runx/sbom-generate.inventory.v1",
      source: {
        label: text(inputs.path) || text(inputs.source_label) || type,
        digest,
        digest_encoding: text(inputs.digest_encoding) || "canonical_json",
        ...(Number.isSafeInteger(inputs.source_bytes) && {
          bytes: inputs.source_bytes,
        }),
      },
      lockfile: { type, version },
      scope,
      project,
      components,
      gaps,
      coverage: {
        basis: "lockfile",
        installed_state_verified: false,
        package_visibility_verified: false,
        complete: !gaps.some((g) =>
          [
            "unresolved_name",
            "unresolved_version",
            "missing_direct_resolution",
          ].includes(g.code),
        ),
      },
    },
  };
}
