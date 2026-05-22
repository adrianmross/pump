# Pump Rule Schema

`docs/rules.schema.json` describes the current Pump rule file shape for editor
completion and validation.

Rule files have a top-level `rules` array. Each rule requires `name` and
`match`, can optionally select Kubernetes-style documents with `apiVersion`,
`kind`, and either `metadataName` or the literal dotted rule field
`metadata.name`, and must define at least one operation.

```yaml
rules:
  - name: deployment-runtime-defaults
    match: "$.spec.template.spec.containers.*"
    apiVersion: apps/v1
    kind: Deployment
    metadataName: billing-api
    defaults:
      imagePullPolicy: IfNotPresent
      resources:
        requests:
          cpu: 100m
          memory: 128Mi
```

Supported operations:

- `defaults`: deep-merge missing fields. Authored values win.
- `overrides`: deep-merge and force values even when authored.
- `delete`: remove relative or absolute paths from each match.
- `replace`: replace the whole matched value.

`replace` is exclusive. A rule with `replace` cannot also define `defaults`,
`overrides`, or `delete`.

```yaml
rules:
  - name: replace-legacy-block
    match: "$.spec.legacy"
    replace:
      enabled: false
```

## VS Code

With the Red Hat YAML extension, add a schema directive to an individual rule
file. The path is relative to that rule file.

```yaml
# yaml-language-server: $schema=../../docs/rules.schema.json
rules:
  - name: service-shape
    match: "$.spec"
    kind: Service
    defaults:
      type: ClusterIP
```

For a workspace-wide association, add this to `.vscode/settings.json`:

```json
{
  "yaml.schemas": {
    "./docs/rules.schema.json": [
      "**/rules.pump.yaml",
      "**/*.pump.yaml"
    ]
  }
}
```

If you author JSON rule files, VS Code's JSON language service can use the same
schema:

```json
{
  "json.schemas": [
    {
      "fileMatch": ["**/rules.pump.json", "**/*.pump.json"],
      "url": "./docs/rules.schema.json"
    }
  ]
}
```

## YAML Language Server

Any editor using `yaml-language-server` can use the same file association. For
example, a language-server config can map Pump rule files to the local schema:

```json
{
  "yaml.schemas": {
    "./docs/rules.schema.json": [
      "**/rules.pump.yaml",
      "**/*.pump.yaml"
    ]
  }
}
```
