# Release Scaffold

Pump uses `cargo-dist` as the Rust-native release system.

The release setup is intentionally staged but not published by default. Keep it
local/manual until the release policy is settled.

## Current Contract

- Normal CI runs Rust formatting, Clippy, tests, and a release build.
- `dist plan` is the source of truth for the release graph.
- `dist build` produces local release artifacts under `target/distrib/`.
- Homebrew and shell installers are configured through `cargo-dist`, not a
  hand-written archive workflow.
- The checked-in release workflow is manual and builds artifacts only. It does
  not run `dist host`, create GitHub releases, or update a tap.

## Local Checks

```sh
make ci
make dist-plan
make dist-build
```

`make dist-build` builds local artifacts only. It does not push tags, create
GitHub releases, update a tap, or publish packages.

## Before Running Any Remote Release

- Move relevant `CHANGELOG.md` entries from `Unreleased` into the release
  version.
- Create and inspect the tag locally.
- Run `dist plan` and inspect the generated manifest.
- Decide whether the Homebrew tap update should be generated but held locally,
  opened as a PR, or applied by automation.
- Confirm the repository variables/secrets needed by `cargo-dist` are set.
- Add a guarded publish job that runs `dist host` only after the policy and
  credentials are settled.

## Remote Actions To Avoid Until Ready

Do not run these casually:

```sh
git push origin main
git push origin vX.Y.Z
gh workflow run release.yml -f tag=vX.Y.Z
```

The workflow files are scaffolding for later. Local validation should be enough
until release automation is intentionally enabled.
