# Release Process

Pump uses `cargo-dist` as the Rust-native release system.

Use the [public release checklist](public-release-checklist.md) as the gate
before changing repository visibility, running a public release path, or
publishing a Homebrew tap update.

## Current Contract

- The first public release was `v0.1.0`.
- Until Pump is declared stable, use pre-1.0 semver:
  - patch bumps for fixes and small compatible improvements,
  - minor bumps for new user-facing capabilities or intentional CLI/rule
    behavior changes,
  - no `v1.0.0` until the rule format, CLI workflow, and release process have
    had real public usage.
- Normal CI runs Rust formatting, Clippy, tests, and a release build.
- `dist plan` is the source of truth for the release graph.
- `dist build` produces local release artifacts under `target/distrib/`.
- Homebrew and shell installers are configured through `cargo-dist`, not a
  hand-written archive workflow.
- The checked-in release workflow is manual. Dispatching it with a real tag runs
  `dist host`, creates the GitHub Release, and uploads release artifacts.
- The release workflow does not update the Homebrew tap. Until tap automation is
  added intentionally, copy the generated `pump.rb` formula into the public tap,
  commit it there, push it, and validate `brew install`.

## Local Checks

```sh
make ci
make dist-plan
make dist-build
```

`make dist-build` builds local artifacts only. It does not push tags, create
GitHub releases, update a tap, or publish packages.

For a conservative release dry-run helper, use:

```sh
scripts/release-patch.sh
```

The helper calculates the next patch version by default, or accepts
`VERSION=0.1.2` / `--version v0.1.2`. It runs `make ci` and
`dist plan --tag vX.Y.Z`, then prints the exact manual commands for tag push,
release dispatch/watch, and Homebrew tap copy/validation. By default it does
not push tags, dispatch workflows, edit the tap checkout, or run Homebrew
validation; those steps require explicit flags.

## Remote Release

- Move relevant `CHANGELOG.md` entries from `Unreleased` into the release
  version.
- Create and inspect the tag locally.
- Run `dist plan --tag vX.Y.Z` and inspect the generated manifest.
- Push `main` and the tag after local validation passes.
- Dispatch the release workflow with the exact tag.
- Verify the GitHub Release and all expected assets exist before updating the
  Homebrew tap.

Release commands:

```sh
git push origin main
git push origin vX.Y.Z
gh workflow run release.yml -f tag=vX.Y.Z
```

## Homebrew Tap

After the GitHub Release is verified:

```sh
gh release download vX.Y.Z --repo adrianmross/pump --pattern pump.rb --dir /tmp/pump-release
cp /tmp/pump-release/pump.rb /path/to/homebrew-tap/Formula/pump.rb
```

Commit and push the tap change, then validate from Homebrew:

```sh
brew update
brew install adrianmross/tap/pump
pump --version
```
