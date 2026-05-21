# Language Decision

## Recommendation

Use Rust for the first serious implementation.

Pump's hard parts are not raw CPU loops. They are fast structured parsing,
loss-conscious YAML/JSON handling, deterministic merge semantics, provenance
tracking, useful diagnostics, and packaging a trustworthy CLI for CI.

Rust is the stronger fit because the ecosystem is already deep in the places
Pump needs:

- `serde` for data modeling and JSON/YAML serialization.
- mature CLI crates such as `clap`.
- strong error-reporting crates such as `miette` or `ariadne`.
- good diffing, path, glob, and filesystem libraries.
- straightforward static binary distribution.
- memory safety without a garbage collector.

Zig is still worth considering for focused low-level components or a later
rewrite if the language ecosystem catches up, but it would make the initial
version spend too much effort on libraries and tooling that Rust already has.

## Rust advantages for Pump

- Better parser ecosystem for YAML, JSON, JSON Pointer, JSONPath-like queries,
  Kubernetes-flavored YAML streams, and structured diagnostics.
- Better fit for provenance data structures, typed rule models, and exhaustive
  merge semantics.
- Easier contribution path for infra/platform engineers already using Rust
  CLIs.
- More mature release automation for macOS, Linux, Windows, Homebrew, and
  GitHub Releases.

## Zig advantages

- Very small static binaries.
- Excellent control over allocation and performance.
- Simple cross-compilation story when dependencies stay native.
- Good fit if Pump becomes mostly a custom parser/engine with few third-party
  dependencies.

## Current call

Start with Rust. Keep the core engine dependency-light and benchmarked so a Zig
port remains possible later if Rust becomes the bottleneck.
