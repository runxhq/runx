---
name: sbom-generate
description: Generate a CycloneDX SBOM from a local npm, pnpm or Cargo lockfile, or reuse a completed dependency inventory; preserve source-byte provenance, unresolved components and missing license evidence.
compatibility: Requires a Runx runtime that admits the parseData worker API.
---

# Generate a dependency SBOM

Use this for a software bill of materials or a reusable lockfile inventory. Use cve-audit to ask which locked public package versions OSV reports as affected; use extract for arbitrary document fields. Neither a valid SBOM nor an empty vulnerability result establishes that software is safe.

The default generate method takes the actual workspace lockfile path. Native artifact admission snapshots the file and binds its original bytes. The decoder interprets one complete admitted UTF-8 document; a partial read fails before inventory generation. Nothing is fetched and no dependency is installed or executed.

## Choose the source once

- generate: local file -> shared inventory -> CycloneDX document and report. Conventional filenames select package-lock, npm-shrinkwrap, pnpm or cargo; use lockfile_type for another filename.
- inventory-file: the same local intake, returning only the reusable inventory for an audit or composition.
- inventory-source: already acquired complete text; native data.digest binds its exact UTF-8 representation. Preserve the upstream transport/source receipt separately. It does not refetch or establish the upstream observation itself.
- make: already parsed lockfile. Its digest encoding is explicitly canonical_json, which cannot attest to the original file bytes.
- from-inventory: render the supplied completed inventory without another file read, decode or dependency interpretation.
- save: write only report.document to a new workspace-relative output file, normally sbom.cdx.json. It preserves the report and source identities, creates no extra source inventory, and refuses to overwrite an existing file. For an intentional replacement, use the ordinary native file writer with the exact current digest.

Prefer an already completed inventory in a graph. It remains source evidence, never new authority. Pass its native artifact reference when available, preserving the package/receipt identity; an arbitrary caller-authored JSON packet is a supplied claim, not independently acquired evidence.

## Coverage and interpretation

Supported formats are npm package-lock/npm-shrinkwrap versions 1, 2 and 3, pnpm lockfile version 9.0, and Cargo.lock versions 3 and 4. Another format/version fails explicitly. When selecting a repository lockfile, honor its package manager; npm-shrinkwrap takes precedence over package-lock when both are active. Do not infer the correct workspace from an unrelated file.

all-locked means declarations in that lockfile, including variants that may not be installed on this host. direct-production is available for npm versions 2 and 3 and for pnpm from their declared production/optional importer dependencies. Cargo.lock alone cannot establish direct/development membership or selected features; use all-locked or acquire Cargo metadata separately. No installed-state, runtime reachability, target activation or license verification is implied.

Keep alias identities, local/workspace links, peer snapshot locations, alternate registries, unknown sources and unresolved entries visible. An unresolved version makes coverage incomplete and the report partial; it is never silently discarded to obtain a clean result. Cargo license evidence is normally absent from the lockfile. Missing metadata stays missing. pnpm integrity-only resolutions identify registry-backed dependencies; explicit Git and directory resolutions retain their own source kind. A missing resolution stays unknown. This does not establish the registry host. Registry hosts can contain private packages; package visibility and permission to disclose coordinates are not established by this inventory. The vulnerability caller owns the chosen query disclosure context.

Resource limits are the native artifact/worker byte, memory and time budgets, not an arbitrary number of dependencies. Increase max_bytes only within the existing native ceiling. If a complete source or generated result exceeds that execution envelope, stop with the source identity and budget reason; do not emit a truncated inventory or relabel it complete. Reuse an appropriately scoped completed inventory rather than rereading the same oversized file.

## Deliver the useful result

The inventory packet runx/sbom-generate.inventory.v1 retains the source digest and encoding, lockfile type/version, scope, component locations and coverage gaps. cve-audit owns vulnerability acquisition and can consume this inventory without re-parsing.

The runx/sbom-generate.sbom.v1 result contains the CycloneDX 1.5 document, its inventory digest, source identity, component count, observed license declarations and caveats. Components use standard licenses entries; unknown declarations are omitted from the document and counted as missing evidence in the report. License strings are declarations, not a legal risk rating. Missing project names/versions and UUIDs are not manufactured. Exact recognized registry components receive package URLs, which do not assert public visibility; source-ambiguous or local components are not mislabeled as registry artifacts.

Show the document or saved file first, then component/coverage and missing-license facts relevant to the operator. A generated report means the selected locked inventory was represented; partial means unresolved source entries remain. A signed receipt binds the execution and declared result, not the truth of third-party license metadata.
