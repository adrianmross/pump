# Provenance

Pump's provenance model has two jobs:

- explain the final value at a path;
- make rule execution auditable enough for review and CI.

## Current State

`inflate --provenance-out` writes machine-readable generated-field provenance.
Each entry records the document index, JSON Pointer path, rule name, operation,
and reason.

`explain` inflates the input, prints the final value for the requested path, and
reports whether the value came from input or a rule. It also prints an execution
trace for the document:

- rules skipped by selectors;
- rules whose match path found no values;
- rules that matched but changed nothing;
- operations that generated, overwrote, replaced, or deleted values.

Use JSON mode for automation:

```sh
pump explain app.yaml --rules platform.pump.yaml \
  --path '$.spec.template.spec.containers.*.resources.requests.memory' \
  --json
```

## Source Locations

Rule file line/column locations are not attached yet.

The likely v1 path is to keep `serde_yml` as the YAML parser and add a second
source-index pass over `serde_yml::loader::Loader`. Its parsed document events
carry marks with byte index, line, and column. That avoids adding a second YAML
parser only for spans.

The first useful slice should attach operation-level locations:

- `/rules/0/defaults`
- `/rules/0/overrides`
- `/rules/0/delete`
- `/rules/0/replace`

Leaf-level locations can come later. They require carrying each relative value
path from a rule operation through `record_generated_paths`.

## Open Questions

- Should JSON rule files get source locations in v1, or should rule files stay
  YAML-only until the source-index path is stable?
- Should `explain --json` include the full document trace by default, or offer a
  smaller `--trace related` mode later?
- Should provenance output record normalized JSON Pointer paths only, or also
  the original rule-path syntax?
