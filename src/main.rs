use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
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

        #[arg(long, help = "Write machine-readable provenance JSON to this file")]
        provenance_out: Option<PathBuf>,
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

        #[arg(long, help = "Print the deflate diff without writing output")]
        dry_run: bool,

        #[arg(
            long,
            value_name = "PATH",
            help = "Do not remove this path or its descendants during deflate"
        )]
        protect: Vec<String>,
    },

    #[command(about = "Show where a value came from after inflation")]
    Explain {
        #[arg(help = "Input JSON/YAML file")]
        input: PathBuf,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,

        #[arg(short, long, help = "Path to inspect, such as '$.spec.replicas'")]
        path: String,

        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
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

    #[serde(default, rename = "apiVersion")]
    api_version: Option<String>,

    #[serde(default)]
    kind: Option<String>,

    #[serde(default, rename = "metadata.name", alias = "metadataName")]
    metadata_name: Option<String>,

    #[serde(default)]
    defaults: Option<Value>,

    #[serde(default)]
    overrides: Option<Value>,

    #[serde(default)]
    delete: Vec<String>,

    #[serde(default)]
    replace: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct ProvenanceEntry {
    doc: usize,
    path: String,
    rule: String,
    operation: String,
    reason: String,
}

type Provenance = HashMap<String, ProvenanceEntry>;

#[derive(Debug, Serialize)]
struct ProvenanceOutput {
    entries: Vec<ProvenanceEntry>,
}

#[derive(Debug, Serialize)]
struct ExplainEntry {
    doc: usize,
    path: String,
    value: Value,
    source: ExplainSource,
}

#[derive(Debug, Serialize)]
struct ExplainSource {
    #[serde(rename = "type")]
    source_type: String,
    rule: Option<String>,
    operation: Option<String>,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Wildcard,
}

#[derive(Debug, Default)]
struct DeflateOptions {
    protected_paths: Vec<Vec<Segment>>,
}

impl DeflateOptions {
    fn from_protect_paths(paths: &[String]) -> Result<Self> {
        let protected_paths = paths
            .iter()
            .map(|path| parse_path(path).with_context(|| format!("invalid protected path {path}")))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { protected_paths })
    }

    fn protects(&self, path: &[String]) -> bool {
        self.protected_paths
            .iter()
            .any(|protected| paths_overlap(protected, path))
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Inflate {
            input,
            rules,
            out,
            format,
            provenance_out,
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            write_output(&docs, &input, out.as_deref(), format)?;
            if let Some(provenance_out) = provenance_out {
                write_provenance(&provenance, &provenance_out)?;
            }
        }
        Commands::Deflate {
            input,
            rules,
            out,
            format,
            dry_run,
            protect,
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let output_format = output_format(&input, out.as_deref(), format);
            let before = dry_run
                .then(|| format_docs(&docs, output_format))
                .transpose()?;
            let options = DeflateOptions::from_protect_paths(&protect)?;
            deflate_with_options(&mut docs, &rule_file, &options)?;

            if let Some(before) = before {
                let after = format_docs(&docs, output_format)?;
                print_diff(&before, &after, "deflated");
            } else {
                write_output(&docs, &input, out.as_deref(), format)?;
            }
        }
        Commands::Explain {
            input,
            rules,
            path,
            json,
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            explain(&docs, &provenance, &path, json)?;
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
            print_diff(&before, &after, "inflated");
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

    validate_rules(&rules)?;

    Ok(rules)
}

fn validate_rules(rule_file: &RuleFile) -> Result<()> {
    for rule in &rule_file.rules {
        validate_rule(rule)?;
    }

    Ok(())
}

fn validate_rule(rule: &Rule) -> Result<()> {
    let has_defaults = rule.defaults.is_some();
    let has_overrides = rule.overrides.is_some();
    let has_delete = !rule.delete.is_empty();
    let has_replace = rule.replace.is_some();

    if !(has_defaults || has_overrides || has_delete || has_replace) {
        bail!("rule {} does not define an operation", rule.name);
    }

    if has_replace && (has_defaults || has_overrides || has_delete) {
        bail!(
            "rule {} cannot combine replace with defaults, overrides, or delete",
            rule.name
        );
    }

    Ok(())
}

fn inflate(docs: &mut [Value], rule_file: &RuleFile, provenance: &mut Provenance) -> Result<()> {
    validate_rules(rule_file)?;

    for rule in &rule_file.rules {
        let segments = parse_path(&rule.match_path)
            .with_context(|| format!("invalid match path for rule {}", rule.name))?;

        for (doc_index, doc) in docs.iter_mut().enumerate() {
            if !rule_matches_doc(rule, doc) {
                continue;
            }

            let target_paths = matching_paths(doc, &segments);

            for target_path in target_paths {
                if let Some(replacement) = &rule.replace
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    *target = replacement.clone();
                    record_generated_paths(
                        doc_index,
                        &target_path,
                        replacement,
                        &rule.name,
                        "replace",
                        "value replaced by rule",
                        provenance,
                    );
                    continue;
                }

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

                if let Some(overrides) = &rule.overrides
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    apply_overrides(
                        target,
                        overrides,
                        doc_index,
                        &target_path,
                        &rule.name,
                        provenance,
                    );
                }

                apply_delete_paths(
                    doc,
                    doc_index,
                    &target_path,
                    &rule.delete,
                    &rule.name,
                    provenance,
                )
                .with_context(|| format!("invalid delete path for rule {}", rule.name))?;
            }
        }
    }

    Ok(())
}

fn deflate_with_options(
    docs: &mut [Value],
    rule_file: &RuleFile,
    options: &DeflateOptions,
) -> Result<()> {
    validate_rules(rule_file)?;

    for rule in &rule_file.rules {
        let segments = parse_path(&rule.match_path)
            .with_context(|| format!("invalid match path for rule {}", rule.name))?;

        for (doc_index, doc) in docs.iter_mut().enumerate() {
            if !rule_matches_doc(rule, doc) {
                continue;
            }

            let target_paths = matching_paths(doc, &segments);

            for target_path in target_paths {
                if let Some(defaults) = &rule.defaults
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    remove_values_matching(target, defaults, &target_path, options);
                }

                if let Some(overrides) = &rule.overrides
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    remove_values_matching(target, overrides, &target_path, options);
                }

                remove_delete_paths(doc, doc_index, &target_path, &rule.delete, options)
                    .with_context(|| format!("invalid delete path for rule {}", rule.name))?;
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
    provenance: &mut Provenance,
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
                    "default",
                    "field was missing in source",
                    provenance,
                );
            }
        }
    }
}

fn apply_overrides(
    target: &mut Value,
    overrides: &Value,
    doc_index: usize,
    path: &[String],
    rule_name: &str,
    provenance: &mut Provenance,
) {
    match (target, overrides) {
        (Value::Object(target_map), Value::Object(override_map)) => {
            for (key, override_value) in override_map {
                let mut child_path = path.to_vec();
                child_path.push(key.clone());

                match target_map.get_mut(key) {
                    Some(existing) if existing.is_object() && override_value.is_object() => {
                        apply_overrides(
                            existing,
                            override_value,
                            doc_index,
                            &child_path,
                            rule_name,
                            provenance,
                        );
                    }
                    Some(existing) => {
                        *existing = override_value.clone();
                        record_generated_paths(
                            doc_index,
                            &child_path,
                            override_value,
                            rule_name,
                            "override",
                            "value forced by rule",
                            provenance,
                        );
                    }
                    None => {
                        target_map.insert(key.clone(), override_value.clone());
                        record_generated_paths(
                            doc_index,
                            &child_path,
                            override_value,
                            rule_name,
                            "override",
                            "value forced by rule",
                            provenance,
                        );
                    }
                }
            }
        }
        (target, overrides) => {
            *target = overrides.clone();
            record_generated_paths(
                doc_index,
                path,
                overrides,
                rule_name,
                "override",
                "value forced by rule",
                provenance,
            );
        }
    }
}

fn remove_values_matching(
    target: &mut Value,
    template: &Value,
    path: &[String],
    options: &DeflateOptions,
) {
    let (Value::Object(target_map), Value::Object(template_map)) = (target, template) else {
        return;
    };

    let mut remove_keys = Vec::new();

    for (key, template_value) in template_map {
        let mut child_path = path.to_vec();
        child_path.push(key.clone());

        let Some(existing) = target_map.get_mut(key) else {
            continue;
        };

        if existing == template_value {
            if !options.protects(&child_path) {
                remove_keys.push(key.clone());
            }
        } else if existing.is_object() && template_value.is_object() {
            remove_values_matching(existing, template_value, &child_path, options);

            if existing.as_object().is_some_and(|object| object.is_empty())
                && !options.protects(&child_path)
            {
                remove_keys.push(key.clone());
            }
        }
    }

    for key in remove_keys {
        target_map.remove(&key);
    }
}

fn apply_delete_paths(
    doc: &mut Value,
    doc_index: usize,
    base_path: &[String],
    delete_paths: &[String],
    rule_name: &str,
    provenance: &mut Provenance,
) -> Result<()> {
    for delete_path in delete_paths {
        let segments = parse_rule_path(delete_path, base_path)?;
        let mut paths = matching_paths(doc, &segments);
        sort_deletion_paths(&mut paths);

        for path in paths {
            if get_at_path(doc, &path).is_none() {
                continue;
            }

            record_provenance(
                doc_index,
                &path,
                rule_name,
                "delete",
                "value removed by rule",
                provenance,
            );
            remove_at_path(doc, &path);
        }
    }

    Ok(())
}

fn remove_delete_paths(
    doc: &mut Value,
    _doc_index: usize,
    base_path: &[String],
    delete_paths: &[String],
    options: &DeflateOptions,
) -> Result<()> {
    for delete_path in delete_paths {
        let segments = parse_rule_path(delete_path, base_path)?;
        let mut paths = matching_paths(doc, &segments)
            .into_iter()
            .filter(|path| !options.protects(path))
            .collect::<Vec<_>>();
        sort_deletion_paths(&mut paths);

        for path in paths {
            remove_at_path(doc, &path);
        }
    }

    Ok(())
}

fn sort_deletion_paths(paths: &mut [Vec<String>]) {
    paths.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| right.cmp(left)));
}

fn remove_at_path(value: &mut Value, path: &[String]) -> Option<Value> {
    let (last, parent_path) = path.split_last()?;
    let parent = get_mut_at_path(value, parent_path)?;

    match parent {
        Value::Object(map) => map.remove(last),
        Value::Array(items) => last
            .parse::<usize>()
            .ok()
            .filter(|index| *index < items.len())
            .map(|index| items.remove(index)),
        _ => None,
    }
}

fn record_provenance(
    doc_index: usize,
    path: &[String],
    rule_name: &str,
    operation: &str,
    reason: &str,
    provenance: &mut Provenance,
) {
    provenance.insert(
        provenance_key(doc_index, path),
        ProvenanceEntry {
            doc: doc_index,
            path: json_pointer(path),
            rule: rule_name.to_string(),
            operation: operation.to_string(),
            reason: reason.to_string(),
        },
    );
}

fn rule_matches_doc(rule: &Rule, doc: &Value) -> bool {
    selector_matches(doc, &["apiVersion"], rule.api_version.as_deref())
        && selector_matches(doc, &["kind"], rule.kind.as_deref())
        && selector_matches(doc, &["metadata", "name"], rule.metadata_name.as_deref())
}

fn selector_matches(doc: &Value, path: &[&str], expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };

    let mut current = doc;
    for segment in path {
        let Some(next) = value_child(current, segment) else {
            return false;
        };
        current = next;
    }

    current.as_str() == Some(expected)
}

fn paths_overlap(protected: &[Segment], path: &[String]) -> bool {
    if protected.len() > path.len() {
        return false;
    }

    protected
        .iter()
        .zip(path)
        .all(|(protected, segment)| match protected {
            Segment::Wildcard => true,
            Segment::Key(key) => key == segment,
        })
}

fn record_generated_paths(
    doc_index: usize,
    path: &[String],
    value: &Value,
    rule_name: &str,
    operation: &str,
    reason: &str,
    provenance: &mut Provenance,
) {
    let key = provenance_key(doc_index, path);
    provenance.insert(
        key,
        ProvenanceEntry {
            doc: doc_index,
            path: json_pointer(path),
            rule: rule_name.to_string(),
            operation: operation.to_string(),
            reason: reason.to_string(),
        },
    );

    if let Value::Object(map) = value {
        for (child_key, child_value) in map {
            let mut child_path = path.to_vec();
            child_path.push(child_key.clone());
            record_generated_paths(
                doc_index,
                &child_path,
                child_value,
                rule_name,
                operation,
                reason,
                provenance,
            );
        }
    } else if let Value::Array(items) = value {
        for (index, child_value) in items.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(index.to_string());
            record_generated_paths(
                doc_index,
                &child_path,
                child_value,
                rule_name,
                operation,
                reason,
                provenance,
            );
        }
    }
}

fn explain(docs: &[Value], provenance: &Provenance, query: &str, json: bool) -> Result<()> {
    let segments = parse_path(query)?;
    let mut found = false;
    let mut entries = Vec::new();

    for (doc_index, doc) in docs.iter().enumerate() {
        for path in matching_paths(doc, &segments) {
            found = true;
            let Some(value) = get_at_path(doc, &path) else {
                continue;
            };

            let source = provenance.get(&provenance_key(doc_index, &path));

            if json {
                entries.push(ExplainEntry {
                    doc: doc_index,
                    path: json_pointer(&path),
                    value: value.clone(),
                    source: source.map_or_else(
                        || ExplainSource {
                            source_type: "input".to_string(),
                            rule: None,
                            operation: None,
                            reason: "authored value or existing container".to_string(),
                        },
                        |source| ExplainSource {
                            source_type: "rule".to_string(),
                            rule: Some(source.rule.clone()),
                            operation: Some(source.operation.clone()),
                            reason: source.reason.clone(),
                        },
                    ),
                });
                continue;
            }

            let doc_prefix = if docs.len() > 1 {
                format!("doc {} ", doc_index)
            } else {
                String::new()
            };
            let human_path = human_path(&path);
            println!("{}{} = {}", doc_prefix, human_path, format_inline(value));

            if let Some(source) = source {
                println!("source: rule {}", source.rule);
                println!("operation: {}", source.operation);
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

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
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
    if path.is_empty() || path == "$" {
        return Ok(Vec::new());
    }

    if path.starts_with('/') {
        return parse_json_pointer(path);
    }

    if let Some(rest) = path.strip_prefix("$.") {
        if rest.is_empty() {
            bail!("path {} is empty after $.", path);
        }

        return parse_dot_segments(rest, path);
    }

    bail!("paths must start with $, $., or /; got {}", path);
}

fn parse_rule_path(path: &str, base_path: &[String]) -> Result<Vec<Segment>> {
    if path.is_empty() || path == "." {
        return Ok(base_path.iter().cloned().map(Segment::Key).collect());
    }

    if path == "$" || path.starts_with("$.") || path.starts_with('/') {
        return parse_path(path);
    }

    let relative = path.strip_prefix('.').unwrap_or(path);
    let mut segments = base_path
        .iter()
        .cloned()
        .map(Segment::Key)
        .collect::<Vec<_>>();
    segments.extend(parse_dot_segments(relative, path)?);
    Ok(segments)
}

fn parse_json_pointer(path: &str) -> Result<Vec<Segment>> {
    path.split('/')
        .skip(1)
        .map(|part| {
            let segment = unescape_json_pointer_segment(part)?;
            if segment == "*" {
                Ok(Segment::Wildcard)
            } else {
                Ok(Segment::Key(segment))
            }
        })
        .collect()
}

fn unescape_json_pointer_segment(segment: &str) -> Result<String> {
    let mut output = String::new();
    let mut chars = segment.chars();

    while let Some(char) = chars.next() {
        if char != '~' {
            output.push(char);
            continue;
        }

        match chars.next() {
            Some('0') => output.push('~'),
            Some('1') => output.push('/'),
            Some(other) => bail!("invalid JSON Pointer escape ~{other}"),
            None => bail!("invalid trailing JSON Pointer escape"),
        }
    }

    Ok(output)
}

fn parse_dot_segments(path: &str, original: &str) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    let mut current_had_escape = false;

    for char in path.chars() {
        if escaped {
            current.push(char);
            current_had_escape = true;
            escaped = false;
            continue;
        }

        match char {
            '\\' => escaped = true,
            '.' => {
                push_dot_segment(&mut segments, &current, current_had_escape, original)?;
                current.clear();
                current_had_escape = false;
            }
            _ => current.push(char),
        }
    }

    if escaped {
        bail!("path {} ends with an incomplete escape", original);
    }

    push_dot_segment(&mut segments, &current, current_had_escape, original)?;
    Ok(segments)
}

fn push_dot_segment(
    segments: &mut Vec<Segment>,
    current: &str,
    escaped: bool,
    original: &str,
) -> Result<()> {
    if current.is_empty() {
        bail!("path {} contains an empty segment", original);
    }

    if current == "*" && !escaped {
        segments.push(Segment::Wildcard);
    } else {
        segments.push(Segment::Key(current.to_string()));
    }

    Ok(())
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

fn print_diff(before: &str, after: &str, after_name: &str) {
    if before == after {
        println!("no changes");
        return;
    }

    let diff = TextDiff::from_lines(before, after);
    print!(
        "{}",
        diff.unified_diff()
            .header("input", after_name)
            .context_radius(3)
    );
}

fn write_provenance(provenance: &Provenance, out_path: &Path) -> Result<()> {
    let mut entries = provenance.values().cloned().collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.doc
            .cmp(&right.doc)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.rule.cmp(&right.rule))
    });

    let output = ProvenanceOutput { entries };
    let json = serde_json::to_string_pretty(&output)?;
    fs::write(out_path, format!("{json}\n"))
        .with_context(|| format!("failed to write provenance {}", out_path.display()))?;

    Ok(())
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

        deflate_with_options(&mut docs, &rules(), &DeflateOptions::default()).unwrap();

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
    fn selectors_limit_rules_to_matching_kubernetes_documents() {
        let rule_file: RuleFile = serde_yml::from_str(
            r#"
rules:
  - name: deployment-defaults
    match: "$.spec"
    apiVersion: apps/v1
    kind: Deployment
    metadataName: api
    defaults:
      replicas: 2
"#,
        )
        .unwrap();
        let mut docs = vec![
            json!({
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {"name": "api"},
                "spec": {}
            }),
            json!({
                "apiVersion": "apps/v1",
                "kind": "Deployment",
                "metadata": {"name": "worker"},
                "spec": {}
            }),
        ];
        let mut provenance = HashMap::new();

        inflate(&mut docs, &rule_file, &mut provenance).unwrap();

        assert_eq!(docs[0]["spec"]["replicas"], json!(2));
        assert!(docs[1]["spec"].get("replicas").is_none());
    }

    #[test]
    fn overrides_delete_and_replace_apply_in_rule_order() {
        let rule_file: RuleFile = serde_yml::from_str(
            r#"
rules:
  - name: base
    match: "$.app"
    defaults:
      mode: safe
      removeMe: true
  - name: force-mode
    match: "$.app"
    overrides:
      mode: enforced
  - name: delete-field
    match: "$.app"
    delete:
      - removeMe
  - name: replace-block
    match: "$.app.nested"
    replace:
      final: true
"#,
        )
        .unwrap();
        let mut docs = vec![json!({
            "app": {
                "mode": "custom",
                "nested": {"old": true}
            }
        })];
        let mut provenance = HashMap::new();

        inflate(&mut docs, &rule_file, &mut provenance).unwrap();

        assert_eq!(docs[0]["app"]["mode"], json!("enforced"));
        assert!(docs[0]["app"].get("removeMe").is_none());
        assert_eq!(docs[0]["app"]["nested"], json!({"final": true}));
        assert_eq!(provenance.get("0:/app/mode").unwrap().operation, "override");
        assert_eq!(
            provenance.get("0:/app/removeMe").unwrap().operation,
            "delete"
        );
        assert_eq!(
            provenance.get("0:/app/nested/final").unwrap().operation,
            "replace"
        );
    }

    #[test]
    fn json_pointer_and_escaped_dot_paths_are_supported() {
        let doc = json!({
            "metadata": {
                "labels": {
                    "app.kubernetes.io/name": "api"
                }
            },
            "items": [{"name": "first"}]
        });

        assert_eq!(
            matching_paths(
                &doc,
                &parse_path("$.metadata.labels.app\\.kubernetes\\.io/name").unwrap()
            ),
            vec![vec![
                "metadata".to_string(),
                "labels".to_string(),
                "app.kubernetes.io/name".to_string()
            ]]
        );
        assert_eq!(
            matching_paths(&doc, &parse_path("/items/0/name").unwrap()),
            vec![vec![
                "items".to_string(),
                "0".to_string(),
                "name".to_string()
            ]]
        );
    }

    #[test]
    fn deflate_respects_protected_paths() {
        let mut docs = vec![json!({
            "app": {
                "replicas": 2,
                "mode": "safe"
            }
        })];
        let rule_file: RuleFile = serde_yml::from_str(
            r#"
rules:
  - name: defaults
    match: "$.app"
    defaults:
      replicas: 2
      mode: safe
"#,
        )
        .unwrap();
        let options = DeflateOptions::from_protect_paths(&["$.app.replicas".to_string()]).unwrap();

        deflate_with_options(&mut docs, &rule_file, &options).unwrap();

        assert_eq!(docs[0]["app"], json!({"replicas": 2}));
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
