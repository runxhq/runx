# Skill Versioning

Every skill in this directory declares a `version:` field at the top level of
its `run.yaml`, a quoted semantic version.

## The rule

A skill's version follows what the registry published of it:

- While its content is what was published, its version is the published one.
- Once its content changes, its version is one patch above the published
  one, and it stays there however often it changes again, until that version
  is published.
- A skill the registry has never published states the version it will first
  be published at.

So a version moves once per publication, never once per commit: callers pin
a version that names exactly one published package.

## The check

Only the registry knows what it published, so the comparison runs where that
read is made, at release:

```sh
RUNX_RUST_CLI_BIN=<runx> pnpm skills:published:check
```

It reads each skill from the registry's public API, computes its content
digest with `runx registry package`, and names every skill whose version
doesn't follow the rule, with the version it should state. The catalog's
static checks, a quoted version and each method's purpose among them, run in
CI and in `verify:fast`.
