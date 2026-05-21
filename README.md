# Pump

Pump is an experiment in sparse config hydration for GitOps and declarative
configuration.

![Pump terminal demo](docs/assets/pump-demo.gif)

The core workflow is:

```text
sparse intent + rules -> inflated manifests
inflated manifests + rules -> deflated intent
```

Pump should make repetitive JSON/YAML configuration smaller without making the
rendered result mysterious. The rendered output must stay ordinary JSON/YAML,
and every generated field should be explainable.

## Install from source

```sh
cargo install --path .
```

Release artifacts and Homebrew packaging are scaffolded with `cargo-dist`, but
not published by default. See [release notes](docs/release/release.md) and
[Homebrew notes](docs/release/homebrew.md) before enabling any remote release
pipeline.

For local development:

```sh
cargo run -- inflate examples/object/source.json --rules examples/object/rules.pump.yaml
```

## CLI shape

Primary commands:

```sh
pump inflate app.yaml --rules platform.pump.yaml --out rendered.yaml
pump deflate rendered.yaml --rules platform.pump.yaml --out app.yaml
pump explain app.yaml --rules platform.pump.yaml --path '$.spec.template.spec.securityContext'
pump diff app.yaml --rules platform.pump.yaml
pump check app.yaml --rules platform.pump.yaml
pump check rendered.yaml --rules platform.pump.yaml --strict
pump check app.yaml --rules platform.pump.yaml --write
pump check services/*.yaml --rules platform.pump.yaml --strict
```

Short aliases may be useful once the semantics are stable:

```sh
pump in   # alias for inflate
pump out  # alias for deflate
```

Flags like `-i` and `-d` are best reserved for options or explicit shortcut
modes, because subcommands are easier to read in CI logs and GitOps reviews.

## Rule format

Pump v1 supports ordered rules. Rules run top-to-bottom, so later rules can
override earlier generated values.

```yaml
rules:
  - name: deployment-platform-defaults
    match: "$.spec.template.spec"
    defaults:
      securityContext:
        runAsNonRoot: true
        seccompProfile:
          type: RuntimeDefault
```

The `match` path syntax is intentionally small:

- `$` targets the document root.
- `$.field.nested` targets object fields.
- `*` targets all object children or array items, such as
  `$.spec.template.spec.containers.*`.

Supported operations:

- `defaults`: deep-merge missing fields. Authored values win.
- `overrides`: deep-merge and force values even when authored.
- `delete`: remove relative or absolute paths from each match.
- `replace`: replace the whole matched value. It cannot be combined with other
  operations in the same rule.

Rules can be limited to Kubernetes-style documents:

```yaml
rules:
  - name: deployment-runtime-defaults
    match: "$.spec.template.spec.containers.*"
    apiVersion: apps/v1
    kind: Deployment
    metadataName: billing-api
    defaults:
      imagePullPolicy: IfNotPresent
```

Path syntax:

- `$` targets the document root.
- `$.field.nested` targets object fields.
- `$.metadata.labels.app\\.kubernetes\\.io/name` escapes dots in keys.
- `/spec/template/spec` uses JSON Pointer.
- `*` targets all object children or array items.

## Examples

JSON object defaults:

```sh
cargo run -- inflate examples/object/source.json --rules examples/object/rules.pump.yaml
cargo run -- diff examples/object/source.json --rules examples/object/rules.pump.yaml
cargo run -- explain examples/object/source.json --rules examples/object/rules.pump.yaml --path '$.second.bottom'
```

Kubernetes-style YAML stream:

```sh
cargo run -- inflate examples/kubernetes/source.yaml --rules examples/kubernetes/rules.pump.yaml
cargo run -- explain examples/kubernetes/source.yaml --rules examples/kubernetes/rules.pump.yaml --path '$.spec.template.spec.containers.*.resources.requests.cpu'
```

See [the Kubernetes example](examples/kubernetes/README.md) for a small
GitOps-style sparse/inflated workflow with generated labels, workload defaults,
Service defaults, deletes, overrides, and provenance.

Biome-style checking:

```sh
pump check app.yaml --rules platform.pump.yaml
pump check rendered.yaml --rules platform.pump.yaml --strict
pump check app.yaml --rules platform.pump.yaml --write
pump check app.yaml --rules platform.pump.yaml --fix
```

- `check` validates that input and rules can be inflated.
- `check --strict` fails with a diff if applying the rules would change the
  file.
- `check --write` inflates the file in place.
- `check --fix` is an alias for `--write`.
- `check` accepts one or more input files; multi-file strict mode evaluates
  every file before failing.

`explain` also prints the rule operations related to the inspected path. Add
`--all` to include the full rule trace for the document, including skipped and
unchanged rules. Generated values include operation-level rule locations when
available. Use `--json` when that trace needs to feed another tool.

To refresh the README terminal demo after CLI semantics settle:

```sh
make demo
```

The demo recipe expects [`vhs`](https://github.com/charmbracelet/vhs) to be
installed and writes `docs/assets/pump-demo.gif`.

## Development

```sh
make fmt
make lint
make test
make build
make dist-plan
```

The CI scaffold runs the same Rust checks on pull requests and `main`. Release
workflows are tag/manual driven and use `cargo-dist`; nothing should publish on
merge.

## Product constraints

- Source files may be plain JSON/YAML.
- Rendered files must be plain JSON/YAML.
- Authored values win over defaults unless a rule explicitly overrides.
- Rule application must be deterministic.
- Generated values need provenance.
- CI should be able to verify that rendered output is current.
- Policy engines should run against the inflated output.

## Non-goals for the first version

- Replacing Jsonnet, CUE, Helm, or Kustomize.
- Arbitrary programming inside rule files.
- Hidden mutation without an inspectable rendered artifact.

See [provenance](docs/provenance.md) for the current explain/provenance model,
and [adjacent tools](docs/adjacent-tools.md) for Pump's intended wedge beside
Jsonnet, CUE, Kustomize, Helm, ytt, and kpt.

## Current limitations

- YAML comments and original formatting are intentionally not preserved in v1;
  output is normalized. Format-preserving output is tracked as a later
  enhancement in issue #2.
- YAML custom tags are not supported yet; issue #3 tracks whether to support
  or explicitly document them.
- Path syntax does not support filters.
- `deflate` removes values that equal defaults, which is useful for compression
  but loses whether the value was intentionally authored.
