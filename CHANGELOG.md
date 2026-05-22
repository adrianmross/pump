# Changelog

## Unreleased

## v0.2.0 - 2026-05-22

- Add optional project config discovery for `pump.yaml`, `.pump.yaml`,
  `pump.yml`, `.pump.yml`, `pump.json`, and `.pump.json`.
- Add batch `inflate` output with `--out-dir` / config `outDir`.
- Add `diff --explain` to print rule-operation provenance after the unified
  diff.
- Add `suggest` / `discover` for conservative repeated-value default rule
  suggestions.
- Add a JSON Schema for Pump rule files and editor setup docs.
- Add a local release helper script for patch-release planning.
- Validate rule match paths while parsing rule files.

## v0.1.1 - 2026-05-21

- Replace the YAML backend with Saphyr to remove `serde_yml` / `libyml` from
  release artifacts while preserving YAML streams, custom tags, normalized
  output, and rule source locations.
- Improve the object and Kubernetes examples so the README demo shows sparse
  source plus rules producing inflated output, not hand-maintained inflated
  fixtures.
- Regenerate and rename the README GIF asset to `docs/assets/pump-demo-diff.gif`
  so the refreshed demo is visible through GitHub/browser caches.

## v0.1.0 - 2026-05-21

- Add the initial Pump Rust CLI.
- Support `inflate`, `deflate`, `explain`, `diff`, and `check`.
- Support `defaults`, `overrides`, `delete`, and `replace` rule operations.
- Support YAML streams, JSON input, JSON Pointer paths, escaped dot paths, and
  wildcard matching over objects and arrays.
- Support YAML custom tags for YAML output.
- Support Kubernetes-style rule selectors for `apiVersion`, `kind`, and
  `metadata.name`.
- Add machine-readable provenance output, JSON explain output, and rule
  execution traces in `explain`.
- Default `explain` traces to operations related to the inspected path, with
  `--all` for the full document rule trace.
- Add operation-level source locations to generated-field provenance and
  explain traces.
- Add best-effort leaf-level `valueLocation` provenance for generated values.
- Support multi-file `check`, including strict drift detection and in-place
  `--write` / `--fix` enforcement across multiple inputs.
- Add a richer Kubernetes/GitOps example and adjacent-tool positioning notes.
- Add local-ready `cargo-dist`, Homebrew, shell installer, and README demo
  scaffolding.
