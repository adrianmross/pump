# Kubernetes Example

This example models a small GitOps app with sparse Kubernetes YAML and Pump
rules that add the repeated platform shape.

The sparse source intentionally keeps only workload-specific intent:

- object names, labels, images, ports, and authored replica counts;
- a worker Deployment with no `replicas`, so the rule default applies;
- a Service with a sparse port, so service defaults apply;
- author-only annotations that are removed from the inflated output;
- one authored `resources.requests.cpu` value to show that defaults do not
  replace source values.

Inflate and compare with the expected rendered manifest:

```sh
cargo run -- inflate examples/kubernetes/source.yaml \
  --rules examples/kubernetes/rules.pump.yaml \
  --out /tmp/pump-kubernetes-inflated.yaml

diff -u examples/kubernetes/inflated.yaml /tmp/pump-kubernetes-inflated.yaml
```

Show provenance for a generated value:

```sh
cargo run -- explain examples/kubernetes/source.yaml \
  --rules examples/kubernetes/rules.pump.yaml \
  --path '$.spec.template.spec.containers.*.resources.requests.memory'
```

Verify that sparse input is not already inflated:

```sh
cargo run -- check examples/kubernetes/source.yaml \
  --rules examples/kubernetes/rules.pump.yaml \
  --strict
```

Deflate the rendered form back toward sparse intent:

```sh
cargo run -- deflate examples/kubernetes/inflated.yaml \
  --rules examples/kubernetes/rules.pump.yaml \
  --dry-run
```

This example intentionally includes `overrides` and `delete` rules. Deflation
removes rule-owned values from the inflated file, but it cannot recover the
original value of an overridden field or a deleted author-only annotation from
the rendered manifest alone.
