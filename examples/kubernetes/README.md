# Kubernetes Example

This example models a small GitOps app with sparse Kubernetes YAML and Pump
rules that add the repeated platform shape.

The sparse source intentionally keeps only workload-specific intent:

- the Deployment name, container image, container port, and authored CPU request;
- the Service name and public port;
- no repeated labels, selectors, replica count, pod security context, image pull
  policy, memory request, Service selector, Service type, or port protocol.

Run from this directory to keep paths short:

```sh
cd examples/kubernetes
```

Show the generated shape:

```sh
pump diff
pump diff --explain
```

The important part of the diff is the repeated platform shape, not a second
hand-written manifest:

```diff
 metadata:
   name: checkout-api
+  labels:
+    app.kubernetes.io/name: checkout-api
+    app.kubernetes.io/part-of: checkout
+    app.kubernetes.io/team: payments
+    app.kubernetes.io/managed-by: pump
 spec:
+  replicas: 2
+  selector:
+    matchLabels:
+      app.kubernetes.io/name: checkout-api
```

Inflate and compare with the expected rendered manifest:

```sh
pump inflate --out /tmp/pump-kubernetes-inflated.yaml
diff -u inflated.yaml /tmp/pump-kubernetes-inflated.yaml
```

Show provenance for a generated value:

```sh
pump explain source.yaml --rules rules.pump.yaml --path '$.spec.replicas'
```

Verify that sparse input is not already inflated:

```sh
pump check --strict
```

Deflate the rendered form back toward sparse intent:

```sh
pump deflate inflated.yaml --rules rules.pump.yaml --dry-run
```

This example intentionally includes an `overrides` rule for
`app.kubernetes.io/managed-by`. Deflation removes values that match rule-owned
defaults, but it cannot recover the original value of an overridden field from
the rendered manifest alone.
