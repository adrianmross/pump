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
