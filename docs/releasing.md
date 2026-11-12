# Releasing the runx CLI

Maintainer doc. Most contributors do not need it.

## Identity

The CLI ships from `github.com/runxhq/runx`. Release tags are `cli-vX.Y.Z`
(prefixed so they do not collide with the repo's other release trains). In the
workspace, `release/status.json` records verified release status and Cloud
adoption. The OSS platform topology owns the CLI package set. The git tag is the
immutable OSS release event that promotes the selected rehearsal artifacts.

Release policy lives at the workspace root (`AGENTS.md` plus
`release/status.json`). This doc is the OSS maintainer runbook. If a release doc,
package manifest, cloud pin, or channel table disagrees with the verified
release identity, fix the owning fact and run the root checks; do not invent a
second release flow.

The same product version is used on every active channel. The governed release
is verified only after independent readback from its required synchronous
channels:

- GitHub Release: `cli-vX.Y.Z` (the hub; serves the raw per-target archives)
- npm: `@runxhq/cli@X.Y.Z` (+ `@runxhq/cli-<platform>@X.Y.Z`)
- Docker (GHCR): `ghcr.io/runxhq/runx:X.Y.Z`, anonymously pullable
- Homebrew and Scoop: `X.Y.Z`
- AUR: `runx`, which builds the exact rehearsed commit archive from source. AUR's
  `runx-bin` is an unrelated project's runx, and a prebuilt package must end in
  `-bin`, so the bare name is the source build.

Winget and Chocolatey are asynchronous: winget is recorded after its pull
request exists, and Chocolatey's moderation reviews each pushed version.
Neither can silently weaken or block verification of the synchronous release
set.

The crates.io package is not a CLI release channel. `runx-cli` depends on the
internal Rust crate graph, whose versions move under a separate, explicit
library release. A CLI tag must not publish those libraries implicitly.

`runx --version` reports `CARGO_PKG_VERSION`, so the crate and npm versions are
stamped in the candidate commit before its rehearsal and the number is truthful regardless of how
the binary was installed.

## Versioning model

`packages/cli/package.json` owns the reviewed CLI candidate version. The
version-setting command projects it into the Rust CLI package and lockfile and
the execution-runtime release identity. The release profile and tag workflow
check those committed values without changing the release checkout; a mismatched
tag fails before publication. `cli-vX.Y.Z` is the CLI distribution version, not a workspace-wide
library-crate release. One command updates only the CLI package surfaces: npm
`package.json` + its `optionalDependencies`, `runx-cli`, the execution-runtime
release identity, and the `runx-cli` lockfile entry.

```bash
pnpm exec tsx scripts/set-release-version.ts X.Y.Z          # write
pnpm exec tsx scripts/set-release-version.ts --check X.Y.Z  # CI drift guard
```

It accepts a raw `cli-vX.Y.Z` / `vX.Y.Z` tag and strips the prefix.

CLI releases do not publish Cargo packages. `runx-cli` is stamped so the native
binary reports the same truthful version as the npm distribution, but its
internal Rust dependencies may be published only through a separately approved
library-crate release. Never cut a new patch just to repair a package-manager
manifest; repair the existing release asset, channel manifest, or workflow in
place.

## Pipeline

`.github/workflows/release.yml` fires on `cli-v*` tags. `workflow_dispatch`
(with a `version` input) runs a build + render dry-run with no publishing.

The rehearsal runs once on the final candidate commit. It verifies the committed
version and release notes, performs the runtime checks, builds and smokes every
supported platform, and retains four publication artifacts:

- `release-assets`: archives, worker evidence, SBOM, install scripts and checksums;
- `npm-tarballs`: the checked contracts/client and native/selector tarballs;
- `channel-manifests`: rendered and checked package-manager files;
- `channel-input`: exact source commit, archive hashes and contained executable hashes.

AUR uses the immutable commit archive, which is available before a release tag
exists. The candidate is retained for thirty days; expired artifacts require a
new rehearsal. A rerun changes the attempt and needs a newly selected profile.

An annotated release tag names `release-candidate: <run-id>/<attempt>`. Before
any publication, the workflow checks that this was a successful dispatch of
this release workflow in this repository at exactly the tagged commit. Each
required artifact must have one unexpired immutable ID and a native GitHub
digest. Artifact downloads reject digest mismatches.

Tags skip runtime compilation, native smoke and npm packing. They download the
selected release assets and tarballs by artifact ID, verify their checksums,
and publish those bytes. Package-manager publication uses the retained manifests
and verifies them against the retained archives before forwarding them. The
GitHub Release remains the hub for archive downloads. npm contracts and native
packages publish before the selector/client that resolve them.

Docker assembles an image from the same digest-bound Linux archives. Native
container smoke precedes promotion to `latest`. AUR builds from source as its
package requires. Independent `contracts-v*` and `client-v*` tags retain their
separate package-only publication path.

GitHub Actions is deliberately disabled on the `winget-pkgs` fork. The fork only
hosts the PR head branch; the checks that gate a winget submission run on
`microsoft/winget-pkgs`. Upstream's workflows are written for that repo's
`pull_request_target` context, where spell check reads only the files a PR
touches. A push to a fork branch has no PR context, so the same workflow scans
the whole repository and fails on upstream's own vocabulary (`HKLM`, `UAC`,
`dsc`) no matter what the manifest contains. Leave Actions off; a red run there
is not a signal about the release.

## Installing (end users)

These work the moment a `cli-v*` tag ships, with no package-manager setup:

```sh
# macOS / Linux
curl -fsSL runx.ai/install | sh
```
```powershell
# Windows
irm runx.ai/install.ps1 | iex
```

`runx.ai/install` and `runx.ai/install.ps1` are clean public paths that **proxy**
to the scripts in this repo ([scripts/install](../scripts/install) and
[scripts/install.ps1](../scripts/install.ps1) on `main`); the script bodies are
not duplicated on the site. Both detect OS/arch, download the matching archive
from the GitHub Release, verify its sha256, and install to a user bin dir.
Overrides: `RUNX_VERSION`, `RUNX_INSTALL_DIR`, `RUNX_BASE_URL` (private mirror).

The archive is one release unit: installers place `runx-js-worker` beside
`runx`, and deterministic-module execution fails closed if that
version-compatible worker is absent.

> Site proxy: point `runx.ai/install` → the raw `scripts/install` and
> `runx.ai/install.ps1` → raw `scripts/install.ps1` (302 or pass-through). Keep
> the path extensionless for the shell installer.

## Channel credentials

Missing credentials for the required synchronous channels fail publication or
independent verification. Optional asynchronous channels never masquerade as
verified release evidence.

| Secret | Channel | Role |
| --- | --- | --- |
| `HOMEBREW_TAP_TOKEN` | Homebrew | required push to `runxhq/homebrew-tap` |
| `SCOOP_BUCKET_TOKEN` | Scoop | required push to `runxhq/scoop-bucket` |
| `WINGET_TOKEN` | winget | optional asynchronous PR to `microsoft/winget-pkgs` |
| `GITHUB_TOKEN` | GitHub Release, GHCR | provided automatically |

npm publication uses GitHub Actions OIDC, not a personal login or a stored npm
token. Each selector and platform package trusts `runxhq/runx`'s `release.yml`.
`@runxhq/contracts` and `@runxhq/client` use that same validated identity for
independent `contracts-v*` and `client-v*` tags, which skip native builds and
other CLI release channels. Preserve that exact workflow identity and
`id-token: write` permission. Configure
direct publishing, rather than staged publishing that requires a maintainer's
interactive approval for each release.

A new npm publisher binding must authenticate successfully within two days to
become valid. Check its status after activation; a saved, pending binding is
not a completed setup. Valid bindings do not require periodic token renewal.
Do not publish unchanged packages or bump versions merely to activate a binding.

Runx owns `runxhq/homebrew-tap` and `runxhq/scoop-bucket`. The winget package is
tracked through its upstream PR. Do not configure an AUR key for a package
owned by an unrelated maintainer.

## Cutting a release

Set the candidate version, write `release/notes/X.Y.Z.md`, and complete the
repository checks before the rehearsal. On the exact pushed candidate commit:

```bash
gh workflow run release.yml --ref main -f version=X.Y.Z
```

Wait for that workflow to finish successfully. Select its exact run ID and
attempt; the project adapter writes a concrete local profile whose argv includes
both values. The profile refuses replacement by another candidate:

```bash
node scripts/runx-cli-release-profile.mjs profile \
  --candidate-run RUN_ID --candidate-attempt ATTEMPT
```

Invoke `skills/release#release` with a JSON inputs file containing
`project_root: "."`, `profile_path: ".runx/release/profile-RUN_ID-ATTEMPT.json"`,
`channel: "runx-cli"` and the exact `version`. The native lane prepares the brief,
pauses for its one approval, publishes the annotated tag and independently reads
back every required channel. Resume that same run after approval.

The project preparation checks the clean checkout, exact remote-main commit,
CI aggregate, pinned rehearsal and artifacts, committed versions, release notes
and anonymous GHCR access. It reuses the rehearsal’s checks rather than running
another platform build or local verification wall. Publication rechecks the
same rehearsal and refuses a changed or unavailable tag identity.

After the release method returns verified, add the reviewed Cloud changelog
entry and adopt the release through the workspace's existing commands:

```bash
pnpm release:adopt -- --version X.Y.Z
pnpm release:check
pnpm release:check:live
```

Never move a published semver tag. Never bump a new patch just to repair channel
drift; fix the existing channel artifact or workflow unless the binary itself is
wrong.

## Layout

```
crates/rust-toolchain.toml    # pinned Rust version for reproducible builds
scripts/
  check-runx-cli-release-notes.mjs # validate complete versioned release notes
  set-release-version.ts      # stamp / --check the version across manifests
  release-platform-matrix.mjs # canonical topology -> GitHub matrix
  build-release-archives.ts   # CLI + worker archive + .sha256 per target
  build-channel-input.mjs     # checksums -> channel manifest input
  gen-channel-manifests.ts    # render Homebrew / Scoop / winget / AUR
  check-channel-manifests.mjs # verify channel manifests against real archives
  publish-winget-manifest.mjs # submit the validated winget manifest set
  make-signature-manifest.ts  # CLI + worker signature manifest
  package-rust-cli.ts         # selector + paired native package staging
  check-rust-cli-release-artifacts.ts  # npm release contract validator
  install / install.ps1       # end-user one-liner installers (proxied via runx.ai/install)
packaging/
  docker/Dockerfile           # GHCR image (fetches the musl archive)
release/
  notes/X.Y.Z.md               # exact reviewed GitHub Release body
```
