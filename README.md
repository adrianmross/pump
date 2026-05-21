# Pump

Pump is an experiment in sparse config hydration for GitOps and declarative
configuration.

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
```

Short aliases may be useful once the semantics are stable:

```sh
pump in   # alias for inflate
pump out  # alias for deflate
```

Flags like `-i` and `-d` are best reserved for options or explicit shortcut
modes, because subcommands are easier to read in CI logs and GitOps reviews.

## Rule format

Pump v1 supports rule files with ordered `defaults` rules:

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

Defaults deep-merge into matched objects. Authored values win, and missing
fields are generated.

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

## Current limitations

- YAML comments and original formatting are not preserved.
- Path syntax does not support escaping, filters, or predicates.
- `deflate` removes values that equal defaults, which is useful for compression
  but loses whether the value was intentionally authored.
- Rule files currently support `defaults`; explicit overrides and deletes are
  not implemented yet.
