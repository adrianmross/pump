# Changelog

## Unreleased

- Add the initial Pump Rust CLI.
- Support `inflate`, `deflate`, `explain`, `diff`, and `check`.
- Support `defaults`, `overrides`, `delete`, and `replace` rule operations.
- Support YAML streams, JSON input, JSON Pointer paths, escaped dot paths, and
  wildcard matching over objects and arrays.
- Support Kubernetes-style rule selectors for `apiVersion`, `kind`, and
  `metadata.name`.
- Add machine-readable provenance output, JSON explain output, and rule
  execution traces in `explain`.
- Default `explain` traces to operations related to the inspected path, with
  `--all` for the full document rule trace.
- Add operation-level source locations to generated-field provenance and
  explain traces.
- Support multi-file `check`, including strict drift detection and in-place
  `--write` / `--fix` enforcement across multiple inputs.
- Add a richer Kubernetes/GitOps example and adjacent-tool positioning notes.
- Add local-ready `cargo-dist`, Homebrew, shell installer, and README demo
  scaffolding.
