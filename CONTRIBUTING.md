# Contributing to runx

Thanks for considering a contribution. This document covers the contribution workflow and the sign-off required on every commit.

## Licensing

runx is licensed under the Apache License, Version 2.0, Copyright 2026 nilstate. By contributing, you agree that your contributions will be licensed under the same license. See [LICENSE](./LICENSE) for the full text.

## Developer Certificate of Origin (DCO)

All commits to this repository must be signed off under the [Developer Certificate of Origin](https://developercertificate.org/). The DCO is a lightweight affirmation that you have the right to submit the contribution under the project's license. There is no separate CLA to sign.

Sign off on every commit by adding a `Signed-off-by:` trailer. The easiest way is to pass `-s` to `git commit`:

```
git commit -s -m "your commit message"
```

This appends a trailer that looks like:

```
Signed-off-by: Your Name <your.email@example.com>
```

The name and email must match the real identity you wish to be associated with the contribution. Pseudonymous sign-offs are not accepted.

The full DCO text (reproduced here for reference):

> By making a contribution to this project, I certify that:
>
> (a) The contribution was created in whole or in part by me and I have the
>     right to submit it under the open source license indicated in the file;
>     or
>
> (b) The contribution is based upon previous work that, to the best of my
>     knowledge, is covered under an appropriate open source license and I
>     have the right under that license to submit that work with modifications,
>     whether created in whole or in part by me, under the same open source
>     license (unless I am permitted to submit under a different license), as
>     indicated in the file; or
>
> (c) The contribution was provided directly to me by some other person who
>     certified (a), (b) or (c) and I have not modified it.
>
> (d) I understand and agree that this project and the contribution are public
>     and that a record of the contribution (including all personal information
>     I submit with it, including my sign-off) is maintained indefinitely and
>     may be redistributed consistent with this project and the open source
>     license(s) involved.

## Contribution workflow

1. Open an issue describing the change before sending a PR for anything non-trivial. Small fixes can go straight to a PR.
2. Fork the repo and create a topic branch from `main`.
3. Make your change. Keep commits focused and conventional (`feat:`, `fix:`, `docs:`, `chore:`, etc.).
4. Run the workspace checks locally:
   - `pnpm install`
   - `pnpm build`
   - `pnpm typecheck`
   - `pnpm test`
5. Sign off your commits with `git commit -s` (see DCO above).
6. Open a pull request against `main` with a clear description of the change and any test or validation evidence.

## Development setup

The native Rust CLI needs Rust 1.97 or newer and stays useful without Node, pnpm, tsx, or TypeScript installed. The workspace and the npm wrapper need Node.js 20 or newer and pnpm 10 or newer.

### macOS Developer Tools permission

On macOS 26, the application that launches a Rust build must be enabled under
**System Settings → Privacy & Security → Developer Tools**. Enable Terminal for
Terminal shells, Visual Studio Code for its integrated terminal and Codex, or
the equivalent host application you use. Add the application with **+** if it
is not listed, then restart that application so existing processes inherit the
permission. Apple documents the setting in
[Control the ability of apps to run software that doesn't meet the system's security policy on Mac](https://support.apple.com/guide/mac-help/mchlc5fb7f9c/mac).

Without this permission, `cargo` or `rustc` can appear to stall after a
`Compiling ...` line while loading locally built procedural-macro libraries;
even `codesign` inspection of the generated library may stall. Fix the host
application's permission. Do not mask the problem by disabling incremental
compilation, downgrading Rust, changing target directories, or stripping file
metadata.

From the OSS workspace:

```bash
pnpm install
pnpm build
pnpm test
```

For a type-only check:

```bash
pnpm typecheck
```

For the fast local loop:

```bash
pnpm test:fast
```

For Rust kernel parity work, run:

```bash
pnpm rust:check
```

This is blocking evidence for Rust-owned kernel and contract surfaces. The command uses `cargo-deny` and `cargo-public-api`; if they are missing, install them with:

```bash
cargo install cargo-deny
cargo install cargo-public-api --version 0.51.0
```

The public API snapshot is read through one pinned nightly, which the check installs on first use.

`test:fast` uses `vitest.fast.config.ts` and is intended for package-adjacent iteration. `pnpm test` remains the full workspace suite and includes the isolated CLI package contract check.

See [docs/how-we-test.md](docs/how-we-test.md) for the full test lane split.

To use the local CLI from any directory:

```bash
pnpm cli:link-global
runx --help
```

Re-run `pnpm build` after source changes that affect compiled package output.

## Skill authoring paths

Use `runx new <name> --objective <outcome>` when you already have the Runx CLI
available locally and want a standalone skill package. The command enters Skill
Lab and leaves the target untouched until the exact package validates:

```bash
runx new docs-demo --objective "Create a bounded documentation decision skill"
```

Community skills should be authored as standalone packages; the runx repo itself is the lane for runx-maintained skills, runtime code, tests, and examples.

The first runnable example is documented in [docs/getting-started.md](docs/getting-started.md). The generated package export index is in [docs/api-surface.md](docs/api-surface.md).

## Releasing

The CLI ships from a reviewed `cli-vX.Y.Z` candidate to GitHub Releases, npm,
GHCR, Homebrew, and Scoop, plus the `runx.ai/install` one-liner. Winget is an
asynchronous submission and AUR is published only when Runx controls the
package. CLI versions are committed to the candidate manifests and verified
again from the tag; CLI tags never publish internal Cargo crates. Full pipeline,
versioning, channel, and release instructions are in
[docs/releasing.md](docs/releasing.md).

## Code of conduct

This project follows the [Code of Conduct](./CODE_OF_CONDUCT.md). Report conduct concerns privately through GitHub's private report flow on the repository; they are handled in confidence.

## Reporting security issues

Do not open a public issue for a vulnerability. Use GitHub's private vulnerability reporting on the repo (Security tab, "Report a vulnerability"). Disclosure is coordinated: a fix is prepared privately, then the issue and fix are disclosed together. Full details are in [SECURITY.md](./SECURITY.md).
