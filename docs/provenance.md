# Provenance

Pump's provenance model has two jobs:

- explain the final value at a path;
- make rule execution auditable enough for review and CI.

## Current State

`inflate --provenance-out` writes machine-readable generated-field provenance.
Each entry records the document index, JSON Pointer path, rule name, operation,
reason, operation-level rule location, and best-effort value-level rule
location when the rule file can be indexed.

`explain` inflates the input, prints the final value for the requested path, and
reports whether the value came from input or a rule. By default it prints only
the rule operations related to the inspected path.

Add `--all` when debugging rule behavior and the full document trace is useful:

- rules skipped by selectors;
- rules whose match path found no values;
- rules that matched but changed nothing;
- operations that generated, overwrote, replaced, or deleted values.

Use JSON mode for automation:

```sh
pump explain app.yaml --rules platform.pump.yaml \
  --path '$.spec.template.spec.containers.*.resources.requests.memory' \
  --json

pump explain app.yaml --rules platform.pump.yaml \
  --path '$.spec.template.spec.containers.*.resources.requests.memory' \
  --all
```

## Source Locations

Pump keeps `serde_yml` as the YAML parser and adds a second source-index pass
over `serde_yml::loader::Loader`. Its parsed document events carry marks with
byte index, line, and column. That avoids adding a second YAML parser only for
spans.

Pump records operation-level locations:

- `/rules/0/defaults`
- `/rules/0/overrides`
- `/rules/0/delete`
- `/rules/0/replace`

Pump also records best-effort value-level locations below those operations:

- `/rules/0/defaults/resources/requests/cpu`
- `/rules/0/overrides/mode`
- `/rules/0/replace/enabled`

The output shape is:

```json
{
  "rule": "container-runtime-defaults",
  "operation": "defaults",
  "operationLocation": {
    "file": "rules.pump.yaml",
    "line": 12,
    "column": 5
  },
  "valueLocation": {
    "file": "rules.pump.yaml",
    "line": 18,
    "column": 11
  }
}
```

`operationLocation` is the default display because it is stable and usually
enough to answer "which rule did this?" `valueLocation` is available in JSON
provenance and `explain --json` for tools that need to jump to the exact rule
leaf. When exact child source cannot be found, Pump omits `valueLocation` and
falls back to `operationLocation`.

## Open Questions

- Should JSON rule files get source locations in v1, or should rule files stay
  YAML-only until the source-index path is stable?
- Should provenance output record normalized JSON Pointer paths only, or also
  the original rule-path syntax?
