---
name: sbom-generate
description: Derive a CycloneDX software bill of materials from a supplied lockfile deterministically, digest-bound to the exact bytes, with license risks surfaced and unsupported or unpinned lockfiles refused.
---

# SBOM Generate

Turn one lockfile into a verifiable component inventory. The caller supplies
the parsed lockfile; nothing is fetched, so the same input always seals the
same SBOM. This is the inventory layer of the security pipeline: `sbom-generate`
derives what is present, `cve-audit` detects what is vulnerable.

## Procedure

1. Native `data.digest` binds the exact lockfile object; the SBOM serial number
   and metadata carry that digest, so the inventory cannot drift from its
   source.
2. Deterministic extraction reads the modern `packages` map (or the classic
   nested `dependencies` map) and keeps only pinned components with versions,
   each with its evidence location inside the lockfile.
3. Licenses are normalized, counted, and risk-flagged: strong copyleft is
   `high`, weak copyleft is `medium`, missing license evidence is `review`.
   Risks are surfaced, never suppressed.
4. A lockfile that is not an object, carries no dependency map, or pins no
   components refuses with findings instead of emitting an empty inventory.

To inventory a remote lockfile, compose `web-fetch` for the governed read and
pass its content here; the digest binds whatever bytes were supplied.

## Output

`sbom_result` (`runx.sbom.v1`) carries `decision` (`generated`, `refused`),
the CycloneDX 1.5 `sbom` or null, `component_count`, `license_summary`,
`license_risks`, `validation`, and the lockfile digest.

Inputs are `lockfile` and optional `lockfile_type` (`package-lock`,
`npm-shrinkwrap`).
