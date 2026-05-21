use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::Value;
use similar::TextDiff;

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Sparse config hydration for JSON/YAML manifests"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        alias = "in",
        about = "Inflate sparse config by applying rule defaults"
    )]
    Inflate {
        #[arg(help = "Input JSON/YAML file")]
        input: PathBuf,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,

        #[arg(short, long, help = "Output file; prints to stdout when omitted")]
        out: Option<PathBuf>,

        #[arg(long, value_enum, help = "Force output format")]
        format: Option<OutputFormat>,
    },

    #[command(
        alias = "out",
        about = "Deflate verbose config by removing values equal to defaults"
    )]
    Deflate {
        #[arg(help = "Input JSON/YAML file")]
        input: PathBuf,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,

        #[arg(short, long, help = "Output file; prints to stdout when omitted")]
        out: Option<PathBuf>,

        #[arg(long, value_enum, help = "Force output format")]
        format: Option<OutputFormat>,
    },

    #[command(about = "Show where a value came from after inflation")]
    Explain {
        #[arg(help = "Input JSON/YAML file")]
        input: PathBuf,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,

        #[arg(short, long, help = "Path to inspect, such as '$.spec.replicas'")]
        path: String,
    },

    #[command(about = "Show a unified diff from input to inflated output")]
    Diff {
        #[arg(help = "Input JSON/YAML file")]
        input: PathBuf,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,

        #[arg(long, value_enum, help = "Force output format")]
        format: Option<OutputFormat>,
    },

    #[command(about = "Validate that input and rules can be inflated")]
    Check {
        #[arg(help = "Input JSON/YAML file")]
        input: PathBuf,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Yaml,
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
struct Rule {
    name: String,

    #[serde(rename = "match")]
    match_path: String,

    #[serde(default)]
    defaults: Option<Value>,
}

#[derive(Debug, Clone)]
struct Provenance {
    rule: String,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Wildcard,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Inflate {
            input,
            rules,
            out,
            format,
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            write_output(&docs, &input, out.as_deref(), format)?;
        }
        Commands::Deflate {
            input,
            rules,
            out,
            format,
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            deflate(&mut docs, &rule_file)?;
            write_output(&docs, &input, out.as_deref(), format)?;
        }
        Commands::Explain { input, rules, path } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            explain(&docs, &provenance, &path)?;
        }
        Commands::Diff {
            input,
            rules,
            format,
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let before = format_docs(&docs, output_format(&input, None, format))?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            let after = format_docs(&docs, output_format(&input, None, format))?;
            print_diff(&before, &after);
        }
        Commands::Check { input, rules } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            println!(
                "ok: {} document(s), {} rule(s)",
                docs.len(),
                rule_file.rules.len()
            );
        }
    }

    Ok(())
}

fn read_input(path: &Path) -> Result<Vec<Value>> {
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read input {}", path.display()))?;

    match detect_format(path) {
        OutputFormat::Json => {
            let doc = serde_json::from_str(&input)
                .with_context(|| format!("failed to parse JSON {}", path.display()))?;
            Ok(vec![doc])
        }
        OutputFormat::Yaml => {
            let docs = serde_yml::Deserializer::from_str(&input)
                .map(Value::deserialize)
                .collect::<std::result::Result<Vec<_>, _>>()
                .with_context(|| format!("failed to parse YAML {}", path.display()))?;

            if docs.is_empty() {
                bail!("{} did not contain any YAML documents", path.display());
            }

            Ok(docs)
        }
    }
}

fn read_rules(path: &Path) -> Result<RuleFile> {
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read rules {}", path.display()))?;
    let rules: RuleFile = serde_yml::from_str(&input)
        .with_context(|| format!("failed to parse rules {}", path.display()))?;

    if rules.rules.is_empty() {
        bail!("{} did not define any rules", path.display());
    }

    Ok(rules)
}

fn inflate(
    docs: &mut [Value],
    rule_file: &RuleFile,
    provenance: &mut HashMap<String, Provenance>,
) -> Result<()> {
    for rule in &rule_file.rules {
        let segments = parse_path(&rule.match_path)
            .with_context(|| format!("invalid match path for rule {}", rule.name))?;

        for (doc_index, doc) in docs.iter_mut().enumerate() {
            let target_paths = matching_paths(doc, &segments);

            for target_path in target_paths {
                if let Some(defaults) = &rule.defaults
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    apply_defaults(
                        target,
                        defaults,
                        doc_index,
                        &target_path,
                        &rule.name,
                        provenance,
                    );
                }
            }
        }
    }

    Ok(())
}

fn deflate(docs: &mut [Value], rule_file: &RuleFile) -> Result<()> {
    for rule in &rule_file.rules {
        let segments = parse_path(&rule.match_path)
            .with_context(|| format!("invalid match path for rule {}", rule.name))?;

        for doc in docs.iter_mut() {
            let target_paths = matching_paths(doc, &segments);

            for target_path in target_paths {
                if let Some(defaults) = &rule.defaults
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    remove_defaults(target, defaults);
                }
            }
        }
    }

    Ok(())
}

fn apply_defaults(
    target: &mut Value,
    defaults: &Value,
    doc_index: usize,
    path: &[String],
    rule_name: &str,
    provenance: &mut HashMap<String, Provenance>,
) {
    let (Value::Object(target_map), Value::Object(default_map)) = (target, defaults) else {
        return;
    };

    for (key, default_value) in default_map {
        let mut child_path = path.to_vec();
        child_path.push(key.clone());

        match target_map.get_mut(key) {
            Some(existing) if existing.is_object() && default_value.is_object() => {
                apply_defaults(
                    existing,
                    default_value,
                    doc_index,
                    &child_path,
                    rule_name,
                    provenance,
                );
            }
            Some(_) => {}
            None => {
                target_map.insert(key.clone(), default_value.clone());
                record_generated_paths(
                    doc_index,
                    &child_path,
                    default_value,
                    rule_name,
                    provenance,
                );
            }
        }
    }
}

fn remove_defaults(target: &mut Value, defaults: &Value) {
    let (Value::Object(target_map), Value::Object(default_map)) = (target, defaults) else {
        return;
    };

    let mut remove_keys = Vec::new();

    for (key, default_value) in default_map {
        let Some(existing) = target_map.get_mut(key) else {
            continue;
        };

        if existing == default_value {
            remove_keys.push(key.clone());
        } else if existing.is_object() && default_value.is_object() {
            remove_defaults(existing, default_value);

            if existing.as_object().is_some_and(|object| object.is_empty()) {
                remove_keys.push(key.clone());
            }
        }
    }

    for key in remove_keys {
        target_map.remove(&key);
    }
}

fn record_generated_paths(
    doc_index: usize,
    path: &[String],
    value: &Value,
    rule_name: &str,
    provenance: &mut HashMap<String, Provenance>,
) {
    let key = provenance_key(doc_index, path);
    provenance.insert(
        key,
        Provenance {
            rule: rule_name.to_string(),
            reason: "field was missing in source".to_string(),
        },
    );

    if let Value::Object(map) = value {
        for (child_key, child_value) in map {
            let mut child_path = path.to_vec();
            child_path.push(child_key.clone());
            record_generated_paths(doc_index, &child_path, child_value, rule_name, provenance);
        }
    }
}

fn explain(docs: &[Value], provenance: &HashMap<String, Provenance>, query: &str) -> Result<()> {
    let segments = parse_path(query)?;
    let mut found = false;

    for (doc_index, doc) in docs.iter().enumerate() {
        for path in matching_paths(doc, &segments) {
            found = true;
            let Some(value) = get_at_path(doc, &path) else {
                continue;
            };

            let doc_prefix = if docs.len() > 1 {
                format!("doc {} ", doc_index)
            } else {
                String::new()
            };
            let human_path = human_path(&path);
            println!("{}{} = {}", doc_prefix, human_path, format_inline(value));

            if let Some(source) = provenance.get(&provenance_key(doc_index, &path)) {
                println!("source: rule {}", source.rule);
                println!("reason: {}", source.reason);
            } else {
                println!("source: input");
                println!("reason: authored value or existing container");
            }
        }
    }

    if !found {
        bail!("path {} did not match any value after inflate", query);
    }

    Ok(())
}

fn matching_paths(value: &Value, segments: &[Segment]) -> Vec<Vec<String>> {
    let mut output = Vec::new();
    collect_matching_paths(value, segments, Vec::new(), &mut output);
    output
}

fn collect_matching_paths(
    value: &Value,
    segments: &[Segment],
    current_path: Vec<String>,
    output: &mut Vec<Vec<String>>,
) {
    let Some((head, tail)) = segments.split_first() else {
        output.push(current_path);
        return;
    };

    match head {
        Segment::Key(key) => {
            if let Some(child) = value_child(value, key) {
                let mut child_path = current_path;
                child_path.push(key.clone());
                collect_matching_paths(child, tail, child_path, output);
            }
        }
        Segment::Wildcard => match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let mut child_path = current_path.clone();
                    child_path.push(key.clone());
                    collect_matching_paths(child, tail, child_path, output);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(index.to_string());
                    collect_matching_paths(child, tail, child_path, output);
                }
            }
            _ => {}
        },
    }
}

fn value_child<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map.get(key),
        Value::Array(items) => key.parse::<usize>().ok().and_then(|index| items.get(index)),
        _ => None,
    }
}

fn value_child_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    match value {
        Value::Object(map) => map.get_mut(key),
        Value::Array(items) => key
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get_mut(index)),
        _ => None,
    }
}

fn parse_path(path: &str) -> Result<Vec<Segment>> {
    if path == "$" {
        return Ok(Vec::new());
    }

    let Some(rest) = path.strip_prefix("$.") else {
        bail!("paths must start with $ or $.; got {}", path);
    };

    if rest.is_empty() {
        bail!("path {} is empty after $.", path);
    }

    rest.split('.')
        .map(|part| {
            if part.is_empty() {
                bail!("path {} contains an empty segment", path);
            } else if part == "*" {
                Ok(Segment::Wildcard)
            } else {
                Ok(Segment::Key(part.to_string()))
            }
        })
        .collect()
}

fn get_at_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = value;

    for segment in path {
        current = value_child(current, segment)?;
    }

    Some(current)
}

fn get_mut_at_path<'a>(value: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut current = value;

    for segment in path {
        current = value_child_mut(current, segment)?;
    }

    Some(current)
}

fn print_diff(before: &str, after: &str) {
    if before == after {
        println!("no changes");
        return;
    }

    let diff = TextDiff::from_lines(before, after);
    print!(
        "{}",
        diff.unified_diff()
            .header("input", "inflated")
            .context_radius(3)
    );
}

fn write_output(
    docs: &[Value],
    input_path: &Path,
    out_path: Option<&Path>,
    format: Option<OutputFormat>,
) -> Result<()> {
    let output = format_docs(docs, output_format(input_path, out_path, format))?;

    if let Some(out_path) = out_path {
        fs::write(out_path, output)
            .with_context(|| format!("failed to write output {}", out_path.display()))?;
    } else {
        print!("{}", output);
    }

    Ok(())
}

fn output_format(
    input_path: &Path,
    out_path: Option<&Path>,
    override_format: Option<OutputFormat>,
) -> OutputFormat {
    override_format
        .unwrap_or_else(|| out_path.map_or_else(|| detect_format(input_path), detect_format))
}

fn detect_format(path: &Path) -> OutputFormat {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => OutputFormat::Json,
        _ => OutputFormat::Yaml,
    }
}

fn format_docs(docs: &[Value], format: OutputFormat) -> Result<String> {
    match format {
        OutputFormat::Json => {
            if docs.len() == 1 {
                Ok(format!("{}\n", serde_json::to_string_pretty(&docs[0])?))
            } else {
                Ok(format!("{}\n", serde_json::to_string_pretty(docs)?))
            }
        }
        OutputFormat::Yaml => {
            let mut output = String::new();

            for (index, doc) in docs.iter().enumerate() {
                if docs.len() > 1 || index > 0 {
                    output.push_str("---\n");
                }

                let rendered = serde_yml::to_string(doc)?;
                output.push_str(rendered.strip_prefix("---\n").unwrap_or(&rendered));
            }

            Ok(output)
        }
    }
}

fn format_inline(value: &Value) -> String {
    match value {
        Value::String(value) => format!("{value:?}"),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).unwrap_or_default(),
        _ => value.to_string(),
    }
}

fn human_path(path: &[String]) -> String {
    if path.is_empty() {
        "$".to_string()
    } else {
        format!("$.{}", path.join("."))
    }
}

fn provenance_key(doc_index: usize, path: &[String]) -> String {
    format!("{}:{}", doc_index, json_pointer(path))
}

fn json_pointer(path: &[String]) -> String {
    if path.is_empty() {
        return String::new();
    }

    let escaped = path
        .iter()
        .map(|segment| segment.replace('~', "~0").replace('/', "~1"))
        .collect::<Vec<_>>()
        .join("/");

    format!("/{escaped}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules() -> RuleFile {
        serde_yml::from_str(
            r#"
rules:
  - name: default-item-shape
    match: "$.*"
    defaults:
      top: a
      middle: 5
      bottom: z
"#,
        )
        .unwrap()
    }

    #[test]
    fn inflate_fills_missing_fields_without_overwriting() {
        let mut docs = vec![json!({
            "first": {
                "top": "a",
                "middle": 5
            },
            "second": {
                "top": "b",
                "middle": 5,
                "very-bottom": 0
            },
            "third": {
                "top": "a",
                "middle": 10
            }
        })];
        let mut provenance = HashMap::new();

        inflate(&mut docs, &rules(), &mut provenance).unwrap();

        assert_eq!(docs[0]["first"]["bottom"], json!("z"));
        assert_eq!(docs[0]["second"]["top"], json!("b"));
        assert_eq!(docs[0]["third"]["middle"], json!(10));
        assert_eq!(
            provenance.get("0:/second/bottom").unwrap().rule,
            "default-item-shape"
        );
    }

    #[test]
    fn deflate_removes_default_values() {
        let mut docs = vec![json!({
            "first": {
                "top": "a",
                "middle": 5,
                "bottom": "z"
            },
            "second": {
                "top": "b",
                "middle": 5,
                "bottom": "z",
                "very-bottom": 0
            }
        })];

        deflate(&mut docs, &rules()).unwrap();

        assert_eq!(docs[0]["first"], json!({}));
        assert_eq!(docs[0]["second"], json!({"top": "b", "very-bottom": 0}));
    }

    #[test]
    fn wildcard_matches_array_items() {
        let rule_file: RuleFile = serde_yml::from_str(
            r#"
rules:
  - name: container-defaults
    match: "$.spec.containers.*"
    defaults:
      imagePullPolicy: IfNotPresent
"#,
        )
        .unwrap();
        let mut docs = vec![json!({
            "spec": {
                "containers": [
                    {"name": "api", "image": "api:v1"},
                    {"name": "worker", "image": "worker:v1", "imagePullPolicy": "Always"}
                ]
            }
        })];
        let mut provenance = HashMap::new();

        inflate(&mut docs, &rule_file, &mut provenance).unwrap();

        assert_eq!(
            docs[0]["spec"]["containers"][0]["imagePullPolicy"],
            json!("IfNotPresent")
        );
        assert_eq!(
            docs[0]["spec"]["containers"][1]["imagePullPolicy"],
            json!("Always")
        );
        assert_eq!(
            provenance
                .get("0:/spec/containers/0/imagePullPolicy")
                .unwrap()
                .rule,
            "container-defaults"
        );
    }

    #[test]
    fn parse_path_accepts_root_and_wildcards() {
        assert_eq!(parse_path("$").unwrap(), vec![]);
        assert_eq!(
            parse_path("$.apps.*.spec").unwrap(),
            vec![
                Segment::Key("apps".to_string()),
                Segment::Wildcard,
                Segment::Key("spec".to_string())
            ]
        );
    }
}
