# Releasing the runx CLI

Maintainer doc. Most contributors do not need it.

## Identity

The CLI ships from `github.com/runxhq/runx`. Release tags are `cli-vX.Y.Z`
(prefixed so they do not collide with the repo's other release trains). In the
workspace, `release/status.json` records verified release status and Cloud
adoption. The OSS platform topology owns the CLI package set. The git tag is the
immutable OSS release event that the public workflow builds.

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
- AUR: `runx`, which builds GitHub's archive of the tag from source. AUR's
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
stamped from the tag at build time and the number is truthful regardless of how
the binary was installed.

## Versioning model

`packages/cli/package.json` owns the reviewed CLI candidate version. The
version-setting command projects it into the Rust CLI package and lockfile and
the execution-runtime release identity. The release profile and tag workflow
check those committed values without changing the release checkout; a mismatched
tag fails before a build. `cli-vX.Y.Z` is the CLI distribution version, not a workspace-wide
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

Stages (the order is intentional — the GitHub Release must exist before any
channel that downloads its archives):

1. **prepare** — resolve the version, stamp + `--check` manifests, `verify:fast`,
   and pack `@runxhq/contracts` and `@runxhq/client`: a version npm already
   holds with other bytes stops the release before anything builds, so a
   changed package needs a new version before the tag.
2. **build** (5-platform matrix) — resolve the matrix from
   `packages/cli/native/supported-platforms.json`, run the shared hostile-module
   contract on each native runner, then build `runx` and `runx-js-worker`
   together with the pinned toolchain. Per platform, package both executables
   into the signed npm artifact, raw archive, and Linux `.deb`, and record
   digest-bound worker evidence.
3. **smoke** (5-platform matrix) — downloads each built archive and runs
   the extracted `runx` beside its matching worker on the real OS. In addition
   to version and archive-shape checks, the packaged candidate must execute a
   nested signed-registry skill, pass declared workspace environment into
   frozen JavaScript context, deliver complete digest-bound `SKILL.md` context
   beyond one megabyte, preserve opaque scopes, close one human approval
   without a duplicate gate, and terminate active JavaScript work on
   interruption. Windows process-tree interruption remains covered by the
   dedicated Windows host-job lifecycle gate. A broken, incomplete, wrong-arch, or
   semantically incomplete archive fails before anything is published. The
   same smoke runs in dry-runs.
4. **github-release** — assemble `checksums.txt`, generate a CycloneDX SBOM, emit
   build-provenance attestations for the binaries, stage the install scripts, and
   publish the Release with all archives. This is the hub. The tagged workflow
   refuses an existing Release before compiling, so published archive bytes are
   never replaced by a whole-workflow retry.
5. **publish-npm** — verify + publish the selector and native packages, then
   `@runxhq/contracts` and `@runxhq/client`, with npm provenance. Each package
   is packed once; a version already in npm must have the same tarball
   integrity, and publication is read back from the registry. A
   `contracts-v<version>` or `client-v<version>` tag publishes that package
   the same way without a CLI release (**Publish TypeScript packages**).
6. **package-managers** — build the channel input from the published checksums
   and the tag's source archive (`build-channel-input.mjs`), verify the archive
   and contained CLI and worker digests and sizes, render Homebrew / Scoop /
   winget / AUR / Chocolatey manifests (`gen-channel-manifests.ts`), verify them
   against the actual release archive contents (`check-channel-manifests.mjs`),
   and attach them to the Release.
7. **publish-{homebrew,scoop,aur}** — update the required package-manager
   channels; AUR refuses a version it already holds with a different package.
   **publish-winget** submits the validated `channels/winget/` manifest set
   asynchronously; it must not use a generator that guesses archive nesting.
   **publish-chocolatey** packs and installs the exact package on Windows,
   checks the CLI and its worker, then pushes it for moderation. The prepare job
   refuses a tag before any build when the AUR or Chocolatey key is missing.
8. **publish-docker** — multi-arch GHCR image (pulls the exact musl archives
   selected by the verified channel input, checks their SHA-256, and installs
   `runx` beside `runx-js-worker`; no Rust toolchain in the image build).
   The versioned tag must be absent before building; a retry cannot silently
   replace an image already published under that version.
   **smoke-docker** pulls the pushed versioned image by digest on native amd64
   and arm64 runners and runs a JavaScript-backed skill. **promote-docker** moves
   `latest` to that tested digest and independently reads it back; an untested
   build never replaces `latest`.

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

```bash
# 1. Write release/notes/X.Y.Z.md with every required section and the exact
#    previous-tag comparison link, then validate it.
node scripts/check-runx-cli-release-notes.mjs --version X.Y.Z

# 2. On a clean OSS main commit, run the project-owned profile through the
#    canonical governed release lane. This prepares, obtains approval for, and
#    pushes the exact tag, then independently verifies every required channel.
runx skill skills/release release \
  -i project_root=. \
  -i profile_ref=release/runx-cli.json \
  -i channel=runx-cli \
  -i version=X.Y.Z \
  --json

# 3. After the release runner returns verified, add the reviewed release entry
#    to the Cloud changelog. From the workspace root, adopt the independently
#    verified release across status, Cloud's exact npm pin, lockfile, and notary.
pnpm release:adopt -- --version X.Y.Z
pnpm release:check
pnpm release:check:live
```

The release profile refuses a dirty checkout, a commit that differs from
`origin/main`, an exact candidate commit without successful `checks` and
`gitleaks` results, an existing tag bound to another commit, or a GHCR package
that is not anonymously pullable. It also requires a complete versioned
release-note file; the tag workflow publishes that exact reviewed body instead
of reconstructing a partial changelog from pull requests. Use
`workflow_dispatch` with a version only when a release-pipeline change needs a
no-publish matrix rehearsal. Do not duplicate the complete platform build for
an ordinary release: the governed tag publish starts the workflow that builds
and smokes every target.

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
