# Adjacent Tools

Pump's wedge is narrow: keep authored JSON/YAML sparse, inflate it into plain
JSON/YAML for policy and delivery, deflate inflated config back to sparse intent,
and explain generated values with provenance.

It is not trying to replace the larger configuration systems below. The useful
question is where Pump can sit beside them without forcing every config author
to learn a programming or packaging model.

| Tool | Strong fit | Pump's wedge |
| --- | --- | --- |
| [Jsonnet](https://jsonnet.org/) | Programmable JSON generation with functions, locals, imports, mixins, and object inheritance. | Pump is smaller and less expressive. It keeps source files as ordinary JSON/YAML and focuses on bidirectional inflate/deflate plus provenance instead of generation from a full language. |
| [CUE](https://cuelang.org/) | Constraints, schemas, validation, data unification, and generation from a typed configuration language. | Pump can be useful when the team wants sparse manifests but does not want to move the source of truth into CUE. CUE can validate the inflated output; Pump can own repetitive fill-in and stripping. |
| [Kustomize](https://kubernetes.io/docs/tasks/manage-kubernetes-objects/kustomization/) | Kubernetes-native bases, overlays, patches, generators, and composition without templates. | Pump is not an overlay engine. Its value is compressing repeated object shape and then checking that inflated YAML is current. It can run before or after Kustomize if both stages are made explicit. |
| [Helm](https://helm.sh/docs/) | Kubernetes application packaging, charts, release metadata, values, hooks, and templating. | Pump is not a package manager and does not install releases. It can reduce repeated values in chart inputs or post-rendered manifests, but Helm should remain the chart distribution boundary. |
| [ytt](https://carvel.dev/ytt/) | YAML templating, overlays, data values, schema, and Starlark-powered reuse. | ytt is much more expressive. Pump's tradeoff is lower authoring surface: rules say what fields to fill, override, delete, or replace; source files remain readable manifests. |
| [kpt](https://kpt.dev/) | Package-centric Kubernetes configuration, KRM functions, validation, and mutation pipelines. | Pump fits naturally as a small KRM-style function later: inflate before validation or deflate before review. The distinctive behavior is reversible sparse/inflated workflows with field provenance. |

## Positioning

Pump should be strongest where teams already have declarative YAML/JSON and the
pain is repetition rather than abstraction. A good use case looks like this:

- reviewers want to read sparse intent in Git;
- automation and policy engines need fully inflated manifests;
- generated values must be explainable;
- CI needs a strict check that rendered output is up to date;
- the team wants to avoid embedding arbitrary programming into every config file.

Pump should not compete on raw expressiveness. If the problem needs loops,
conditionals, package management, chart dependencies, complex overlays, or typed
schema design, one of the adjacent tools is probably the primary tool. Pump's
real value is when the missing primitive is "make this repeated shape implicit,
but keep the inflated form inspectable and reversible."

## Interop Shape

The cleanest integration shape is an explicit pipeline:

```text
sparse source + pump rules
  -> pump inflate
  -> ordinary JSON/YAML
  -> policy, validation, kustomize/helm/ytt/kpt stage, or deployment
```

For review-heavy GitOps repositories, the stricter shape is:

```text
sparse source + pump rules
  -> pump inflate --out rendered.yaml
  -> pump check rendered.yaml --rules rules.pump.yaml --strict
```

For repositories that do not commit rendered output, CI can still run `pump
check --strict` on files that are expected to already be inflated, or `pump diff`
on sparse files so reviewers see exactly what the rules add.
