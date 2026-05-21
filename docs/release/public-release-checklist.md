# Public Release Checklist

This is the gate for moving Pump from private/local validation to a public
repository and first public release. It is intentionally a checklist, not an
automation trigger.

Do not make the repository public, push release tags, run release workflows,
publish artifacts, or update a Homebrew tap until this checklist has been
reviewed and explicitly approved.

## Readiness Gates

- Core CLI behavior is stable enough for public users:
  - `inflate`, `deflate`, `explain`, `diff`, and `check` are documented.
  - `check --strict`, `check --write`, and `check --fix` behavior is covered.
  - JSON, YAML, YAML streams, and YAML custom tags are covered by tests.
  - Provenance includes `operationLocation` and best-effort `valueLocation`.
- Real-manifest validation has been completed:
  - at least one isolated clone validation beyond the examples,
  - parse failures recorded,
  - semantic round-trip failures recorded,
  - formatting churn understood and either accepted or tracked.
- Deferred scope is explicit:
  - format-preserving YAML remains deferred unless issue #2 is promoted,
  - KRM/kpt wrapper remains deferred unless issue #1 is promoted,
  - no arbitrary programming is added to rule files.
- Public-facing documentation is ready:
  - README explains the product wedge and non-goals,
  - examples run from a clean checkout,
  - demo GIF is current,
  - adjacent-tool positioning is accurate,
  - limitations are clear and not surprising.

## Local Validation Before Any Public Step

Run these locally and inspect output:

```sh
make ci
make demo
dist plan --allow-dirty
dist build --allow-dirty
```

Also verify the generated release artifacts under `target/distrib/`:

- archive names and target triples,
- `pump` binary presence,
- `README.md`, `LICENSE`, and `CHANGELOG.md` inclusion,
- checksums,
- shell installer,
- generated `pump.rb` formula.

## Visibility Decision

Before changing repository visibility:

- Confirm the issue tracker contains no private operational details that should
  be rewritten or moved.
- Confirm docs and examples do not depend on private infrastructure.
- Confirm generated assets are appropriate to publish.
- Confirm license, README, and changelog are present.
- Decide whether to keep initial issues open as public roadmap items or close
  private-only validation issues first.

Visibility change is a separate explicit action from release. Making the repo
public must not automatically run a release.

## First Public Release Decision

Before the first public release:

- Move `CHANGELOG.md` entries from `Unreleased` into the target version.
- Create the release tag locally and inspect it.
- Re-run `dist plan` for the exact tag.
- Decide whether GitHub Releases are the canonical artifact host.
- Decide whether the release workflow should only build artifacts or also run
  `dist host`.
- Confirm required repository variables and secrets are set.
- Confirm the workflow cannot publish on ordinary pushes or merges.

Commands that require explicit approval:

```sh
git push origin main
git push origin vX.Y.Z
gh workflow run release.yml -f tag=vX.Y.Z
```

## Homebrew Decision

Before publishing Homebrew support:

- Decide the tap repository and visibility.
- Decide whether tap updates are manual commits, PRs, or automated.
- Confirm the token/secret name used for tap writes.
- Confirm the formula downloads from the chosen release artifact host.
- Test the generated `pump.rb` locally before publishing tap changes.

Until this is settled, Homebrew remains configured-but-held packaging.

## Rollback And Pause Criteria

Pause the public release if any of these are true:

- local `make ci` fails,
- `dist plan` produces unexpected artifacts,
- release workflow behavior differs from the local plan,
- issue or docs review finds private-only details,
- generated formula points at missing or private artifacts,
- semantic validation finds unexplained drift.

Rollback posture:

- repository visibility can be changed back only as a last resort;
  pre-public review should catch private details first,
- delete or supersede a bad tag before advertising it,
- close a bad GitHub Release as draft or delete it before publishing links,
- do not publish tap changes until release assets are verified.
