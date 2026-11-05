import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import Ajv from "ajv";
import { normalize, fromFile } from "./inventory.mjs";
import { makeSbom, renderDocument } from "./sbom-generate.mjs";

const source_digest = `sha256:${"a".repeat(64)}`;
const inventory_digest = `sha256:${"b".repeat(64)}`;
const inventory = (lockfile, extra = {}) =>
  normalize({ lockfile, source_digest, ...extra }).inventory;
const lock = (packages) => ({
  lockfileVersion: 3,
  packages: { "": { name: "demo" }, ...packages },
});
const standard = (name) =>
  JSON.parse(
    readFileSync(
      new URL(`./fixtures/standards/${name}`, import.meta.url),
      "utf8",
    ),
  );
const ajv = new Ajv({ strict: false, validateFormats: false });
ajv.addSchema(standard("spdx.schema.json"));
ajv.addSchema(standard("jsf-0.82.schema.json"));
const conforms = ajv.compile(standard("bom-1.5.schema.json"));

test("standard conformance replaces the invalid license field and invented root metadata", () => {
  const completed = inventory(
    lock({
      "node_modules/alias": {
        name: "@real/package",
        version: "1.2.3",
        license: "LGPL-3.0-only",
        resolved:
          "https://registry.npmjs.org/@real/package/-/package-1.2.3.tgz",
      },
      "node_modules/missing-license": { version: "2.0.0" },
    }),
  );
  const result = makeSbom({ inventory: completed, inventory_digest });
  assert.equal(
    conforms(result.sbom_result.document),
    true,
    JSON.stringify(conforms.errors),
  );
  const component = result.sbom_result.document.components.find(
    (value) => value.name === "package",
  );
  assert.deepEqual(component.licenses, [
    { license: { name: "LGPL-3.0-only" } },
  ]);
  assert.equal(component.purl, "pkg:npm/%40real/package@1.2.3");
  assert.equal(Object.hasOwn(component, "license"), false);
  assert.equal(
    Object.hasOwn(result.sbom_result.document, "serialNumber"),
    false,
  );
  assert.equal(
    Object.hasOwn(result.sbom_result.document.metadata.component, "version"),
    false,
  );
  assert.equal(result.sbom_result.license_summary.missing_evidence, 1);
  const saved = JSON.parse(renderDocument(result).contents);
  assert.deepEqual(saved, result.sbom_result.document);
  const broken = structuredClone(saved);
  broken.components[0].license = "MIT";
  assert.equal(
    conforms(broken),
    false,
    "the pinned official schema must reject the old defect",
  );
});

test("local links and unresolved registry declarations survive with truthful coverage", () => {
  const completed = inventory(
    lock({
      "node_modules/local": { resolved: "packages/local", link: true },
      "node_modules/range": { version: "^1.0.0" },
      "node_modules/no-version": { license: "MIT" },
    }),
  );
  assert.equal(completed.components.length, 3);
  assert.equal(
    completed.components.find((value) => value.name === "local").source_kind,
    "local",
  );
  assert.equal(completed.coverage.complete, false);
  const result = makeSbom({ inventory: completed, inventory_digest });
  assert.equal(result.sbom_result.decision, "partial");
  assert.equal(
    conforms(result.sbom_result.document),
    true,
    JSON.stringify(conforms.errors),
  );
});

test("pnpm aliases and peer variants retain one registry identity and every snapshot", () => {
  const completed = inventory(
    {
      lockfileVersion: "9.0",
      importers: {
        ".": {
          dependencies: {
            alias: {
              specifier: "npm:real@1.0.0",
              version: "real@1.0.0(peer@2.0.0)",
            },
            local: { specifier: "workspace:*", version: "link:../local" },
          },
        },
      },
      packages: {
        "real@1.0.0": { resolution: { integrity: "sha512-fixture" } },
      },
      snapshots: { "real@1.0.0(peer@2.0.0)": {}, "real@1.0.0(peer@3.0.0)": {} },
    },
    { lockfile_type: "pnpm" },
  );
  assert.equal(completed.components.length, 2);
  const real = completed.components.find((value) => value.name === "real");
  assert.equal(real.direct, true);
  assert.equal(real.development, null);
  assert.equal(
    real.source_kind,
    "registry",
    "registry resolution is not proof of package visibility",
  );
  assert.equal(real.snapshot_locations.length, 2);
  const declared = inventory(
    {
      lockfileVersion: "9.0",
      importers: { ".": {} },
      packages: {
        "real@1.0.0": { resolution: { integrity: "sha512-fixture" } },
      },
      snapshots: {},
    },
    { lockfile_type: "pnpm" },
  );
  assert.equal(declared.components[0].source_kind, "registry");
});

test("direct-production excludes development-only rows and surfaces missing resolutions", () => {
  const completed = inventory(
    {
      lockfileVersion: 3,
      packages: {
        "": {
          dependencies: { prod: "1.0.0", missing: "2.0.0" },
          devDependencies: { dev: "1.0.0" },
        },
        "node_modules/prod": { version: "1.0.0" },
        "node_modules/dev": { version: "1.0.0", dev: true },
      },
    },
    { scope: "direct-production" },
  );
  assert.deepEqual(
    completed.components.map((value) => value.name),
    ["prod"],
  );
  assert.equal(completed.coverage.complete, false);
  assert.ok(
    completed.gaps.some((value) => value.code === "missing_direct_resolution"),
  );
});

test("Cargo preserves registry, git and local identity without inventing membership or licenses", () => {
  const completed = inventory(
    {
      version: 4,
      package: [
        {
          name: "public",
          version: "1.0.0",
          source: "registry+https://github.com/rust-lang/crates.io-index",
          checksum: "c".repeat(64),
        },
        {
          name: "git",
          version: "1.0.0",
          source: "git+https://example.test/repo#" + "d".repeat(40),
        },
        { name: "workspace", version: "0.1.0" },
      ],
    },
    { lockfile_type: "cargo" },
  );
  assert.equal(completed.components.length, 3);
  assert.deepEqual(
    new Set(completed.components.map((value) => value.source_kind)),
    new Set(["registry", "git", "local"]),
  );
  assert.ok(
    completed.components.every(
      (value) =>
        value.direct === null &&
        value.development === null &&
        value.licenses.length === 0,
    ),
  );
  const result = makeSbom({ inventory: completed, inventory_digest });
  assert.equal(
    conforms(result.sbom_result.document),
    true,
    JSON.stringify(conforms.errors),
  );
  assert.equal(
    Object.hasOwn(result.sbom_result.document.metadata, "component"),
    false,
  );
  assert.throws(
    () =>
      inventory(
        { version: 4, package: [] },
        { lockfile_type: "cargo", scope: "direct-production" },
      ),
    /metadata/,
  );
});

test("unsupported or mislabeled data and truncated source never produce an empty success", () => {
  assert.throws(() => inventory({ packages: {} }), /lockfileVersion/);
  assert.throws(
    () => inventory({ lockfileVersion: "8.0" }, { lockfile_type: "pnpm" }),
    /9.0/,
  );
  assert.throws(
    () => inventory({ lockfileVersion: "9.0" }, { lockfile_type: "pnpm" }),
    /importers/,
  );
  assert.throws(
    () =>
      inventory(
        { lockfileVersion: 3, packages: { "": {} } },
        { lockfile_type: "cargo" },
      ),
    /Cargo/,
  );
  assert.throws(
    () =>
      fromFile({
        page: { eof: false, offset: 0, encoding: "utf8", data: "{}" },
      }),
    /truncated/,
  );
});

test("parsed provenance is explicit, deterministic, and not capped at 10000 dependencies", () => {
  const packages = {};
  for (let i = 0; i < 10001; i++)
    packages[`node_modules/pkg-${i}`] = { version: "1.0.0" };
  const completed = inventory(lock(packages));
  assert.equal(completed.components.length, 10001);
  assert.equal(completed.source.digest_encoding, "canonical_json");
  assert.equal(completed.coverage.installed_state_verified, false);
  const reversed = Object.fromEntries(Object.entries(packages).reverse());
  assert.deepEqual(completed.components, inventory(lock(reversed)).components);
});

test("registry identity does not establish public package visibility", () => {
  const completed = inventory(
    lock({
      "node_modules/@company/private": {
        version: "1.0.0",
        resolved:
          "https://registry.npmjs.org/@company/private/-/private-1.0.0.tgz",
      },
    }),
  );
  assert.equal(completed.components[0].source_kind, "registry");
  assert.equal(completed.coverage.package_visibility_verified, false);
});

test("pnpm distinguishes declared git, directory, unknown and custom sources", () => {
  const result = inventory(
    {
      lockfileVersion: "9.0",
      importers: { ".": {} },
      packages: {
        "git-package@1.0.0": {
          name: "git-package",
          version: "1.0.0",
          resolution: {
            type: "git",
            repo: "https://example.invalid/project",
            commit: "actual-source-value",
          },
        },
        "local-package@1.0.0": {
          name: "local-package",
          version: "1.0.0",
          resolution: { type: "directory", directory: "../local" },
        },
        "unknown@1.0.0": { resolution: {} },
        "custom@1.0.0": {
          resolution: { type: "custom:cdn", integrity: "sha512-fixture" },
        },
      },
      snapshots: {},
    },
    { lockfile_type: "pnpm" },
  );
  assert.deepEqual(
    Object.fromEntries(result.components.map((c) => [c.name, c.source_kind])),
    {
      custom: "other-source",
      "git-package": "git",
      "local-package": "local",
      unknown: "unknown",
    },
  );
  assert.equal(result.coverage.package_visibility_verified, false);
});

test("an all-locked pnpm inventory keeps a workspace's development link", () => {
  const lockfile = {
    lockfileVersion: "9.0",
    importers: { ".": { devDependencies: { internal: { version: "link:../internal" } } } },
    packages: {},
    snapshots: {},
  };
  const { components } = inventory(lockfile, { lockfile_type: "pnpm", scope: "all-locked" });
  assert.deepEqual(
    components.map(({ name, direct, development }) => ({ name, direct, development })),
    [{ name: "internal", direct: true, development: true }],
  );
});

test("an npm v1 lockfile does not claim which packages are direct", () => {
  const lockfile = {
    lockfileVersion: 1,
    dependencies: {
      app: { version: "1.0.0", requires: { transitive: "2.0.0" } },
      transitive: { version: "2.0.0" },
    },
  };
  assert.throws(
    () => inventory(lockfile, { scope: "direct-production" }),
    /cannot establish direct-production/,
  );
  const { components } = inventory(lockfile, { scope: "all-locked" });
  assert.deepEqual(components.map(({ direct }) => direct), [null, null]);
});
