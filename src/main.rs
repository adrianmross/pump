use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use indexmap::IndexMap;
use saphyr::LoadableYamlNode;
use serde::Serialize;
use similar::TextDiff;

mod value;
use value::Value;

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
        #[arg(help = "Input JSON/YAML file; defaults to config inputs")]
        input: Option<PathBuf>,

        #[arg(short, long, help = "Pump rule file; defaults to config rules")]
        rules: Option<PathBuf>,

        #[arg(short, long, help = "Output file; prints to stdout when omitted")]
        out: Option<PathBuf>,

        #[arg(long, help = "Output directory for config or batch inputs")]
        out_dir: Option<PathBuf>,

        #[arg(long, value_enum, help = "Force output format")]
        format: Option<OutputFormat>,

        #[arg(
            long,
            help = "Project config file; defaults to pump.yaml or .pump.yaml"
        )]
        config: Option<PathBuf>,

        #[arg(long, help = "Write machine-readable provenance JSON to this file")]
        provenance_out: Option<PathBuf>,
    },

    #[command(
        alias = "out",
        about = "Deflate verbose config by removing values equal to defaults"
    )]
    Deflate {
        #[arg(help = "Input JSON/YAML file; defaults to config inputs")]
        input: Option<PathBuf>,

        #[arg(short, long, help = "Pump rule file; defaults to config rules")]
        rules: Option<PathBuf>,

        #[arg(short, long, help = "Output file; prints to stdout when omitted")]
        out: Option<PathBuf>,

        #[arg(
            long,
            help = "Project config file; defaults to pump.yaml or .pump.yaml"
        )]
        config: Option<PathBuf>,

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

        #[arg(short, long, help = "Pump rule file; defaults to config rules")]
        rules: Option<PathBuf>,

        #[arg(short, long, help = "Path to inspect, such as '$.spec.replicas'")]
        path: String,

        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,

        #[arg(
            long,
            help = "Include the full rule trace, not just related operations"
        )]
        all: bool,

        #[arg(
            long,
            help = "Project config file; defaults to pump.yaml or .pump.yaml"
        )]
        config: Option<PathBuf>,
    },

    #[command(about = "Show a unified diff from input to inflated output")]
    Diff {
        #[arg(help = "Input JSON/YAML file; defaults to config inputs")]
        input: Option<PathBuf>,

        #[arg(short, long, help = "Pump rule file; defaults to config rules")]
        rules: Option<PathBuf>,

        #[arg(long, value_enum, help = "Force output format")]
        format: Option<OutputFormat>,

        #[arg(long, help = "Print generated-value rule provenance after the diff")]
        explain: bool,

        #[arg(
            long,
            help = "Project config file; defaults to pump.yaml or .pump.yaml"
        )]
        config: Option<PathBuf>,
    },

    #[command(about = "Validate that input and rules can be inflated")]
    Check {
        #[arg(value_name = "INPUT", help = "Input JSON/YAML file(s)")]
        inputs: Vec<PathBuf>,

        #[arg(short, long, help = "Pump rule file; defaults to config rules")]
        rules: Option<PathBuf>,

        #[arg(long, help = "Fail if applying rules would change the input")]
        strict: bool,

        #[arg(
            long,
            visible_alias = "fix",
            help = "Write the inflated output back to the input file"
        )]
        write: bool,

        #[arg(long, value_enum, help = "Force output format")]
        format: Option<OutputFormat>,

        #[arg(
            long,
            help = "Project config file; defaults to pump.yaml or .pump.yaml"
        )]
        config: Option<PathBuf>,
    },

    #[command(
        visible_alias = "discover",
        about = "Suggest default rules from repeated input values"
    )]
    Suggest {
        #[arg(
            value_name = "INPUT",
            help = "Input JSON/YAML file(s); defaults to config inputs"
        )]
        inputs: Vec<PathBuf>,

        #[arg(long, default_value_t = 2, help = "Minimum repeated occurrences")]
        min_occurrences: usize,

        #[arg(short, long, help = "Output rule file; prints to stdout when omitted")]
        out: Option<PathBuf>,

        #[arg(long, help = "Print machine-readable JSON suggestions")]
        json: bool,

        #[arg(
            long,
            help = "Project config file; defaults to pump.yaml or .pump.yaml"
        )]
        config: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Json,
    Yaml,
}

#[derive(Debug)]
struct ProjectConfig {
    rules: Option<PathBuf>,
    inputs: Vec<PathBuf>,
    out_dir: Option<PathBuf>,
    format: Option<OutputFormat>,
}

#[derive(Debug)]
struct RuleFile {
    rules: Vec<Rule>,

    source_locations: RuleSourceIndex,
}

impl RuleFile {
    fn operation_location(&self, rule_index: usize, operation: &str) -> Option<&SourceLocation> {
        self.source_locations
            .get(&operation_key(rule_index, operation))
    }

    fn value_location(
        &self,
        rule_index: usize,
        operation: &str,
        rule_value_path: &[String],
    ) -> Option<&SourceLocation> {
        self.source_locations
            .get(&value_location_key(rule_index, operation, rule_value_path))
    }
}

#[derive(Debug)]
struct Rule {
    name: String,

    match_path: String,

    api_version: Option<String>,

    kind: Option<String>,

    metadata_name: Option<String>,

    defaults: Option<Value>,

    overrides: Option<Value>,

    delete: Vec<String>,

    replace: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct ProvenanceEntry {
    doc: usize,
    path: String,
    rule: String,
    operation: String,
    reason: String,
    #[serde(rename = "operationLocation", skip_serializing_if = "Option::is_none")]
    operation_location: Option<SourceLocation>,
    #[serde(rename = "valueLocation", skip_serializing_if = "Option::is_none")]
    value_location: Option<SourceLocation>,
}

type Provenance = HashMap<String, ProvenanceEntry>;
type RuleSourceIndex = HashMap<String, SourceLocation>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SourceLocation {
    file: String,
    line: u64,
    column: u64,
    #[serde(rename = "byteIndex")]
    byte_index: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RuleTrace {
    doc: usize,
    rule: String,
    rule_index: usize,
    match_path: String,
    decision: String,
    reason: String,
    target_paths: Vec<String>,
    operations: Vec<OperationTrace>,
}

#[derive(Debug, Clone, Serialize)]
struct OperationTrace {
    operation: String,
    path: String,
    outcome: String,
    reason: String,
    before: Option<Value>,
    after: Option<Value>,
    #[serde(rename = "operationLocation", skip_serializing_if = "Option::is_none")]
    operation_location: Option<SourceLocation>,
    #[serde(rename = "valueLocation", skip_serializing_if = "Option::is_none")]
    value_location: Option<SourceLocation>,
}

#[derive(Debug)]
struct CheckResult {
    documents: usize,
    generated: usize,
    changed: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SuggestedDefault {
    match_path: String,
    defaults_path: String,
    value: Value,
    occurrences: usize,
}

#[derive(Debug, Serialize)]
struct SuggestOutput {
    suggestions: Vec<SuggestedDefault>,
}

#[derive(Debug)]
struct SuggestCandidate {
    match_path: String,
    defaults_path: Vec<String>,
    value: Value,
    occurrences: usize,
    roots: HashSet<String>,
}

struct OperationContext<'location, 'operations> {
    rule_file: &'location RuleFile,
    rule_index: usize,
    operation: &'location str,
    operation_location: Option<&'location SourceLocation>,
    operations: Option<&'operations mut Vec<OperationTrace>>,
}

struct OperationTraceInput<'a> {
    operation: &'a str,
    path: &'a [String],
    outcome: &'a str,
    reason: &'a str,
    before: Option<Value>,
    after: Option<Value>,
    value_location: Option<SourceLocation>,
}

impl OperationContext<'_, '_> {
    fn value_location(&self, rule_value_path: &[String]) -> Option<&SourceLocation> {
        self.rule_file
            .value_location(self.rule_index, self.operation, rule_value_path)
    }

    fn push(&mut self, input: OperationTraceInput<'_>) {
        if let Some(operations) = self.operations.as_deref_mut() {
            operations.push(OperationTrace {
                operation: input.operation.to_string(),
                path: json_pointer(input.path),
                outcome: input.outcome.to_string(),
                reason: input.reason.to_string(),
                before: input.before,
                after: input.after,
                operation_location: self.operation_location.cloned(),
                value_location: input.value_location,
            });
        }
    }
}

#[derive(Clone, Copy)]
struct RuleApplyContext<'a> {
    doc_index: usize,
    rule_name: &'a str,
}

struct ProvenanceContext<'a> {
    doc_index: usize,
    rule_name: &'a str,
    operation: &'a str,
    reason: &'a str,
    operation_location: Option<&'a SourceLocation>,
    rule_file: &'a RuleFile,
    rule_index: usize,
}

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
    trace: Vec<RuleTrace>,
}

#[derive(Debug, Serialize)]
struct ExplainSource {
    #[serde(rename = "type")]
    source_type: String,
    rule: Option<String>,
    operation: Option<String>,
    reason: String,
    #[serde(rename = "operationLocation", skip_serializing_if = "Option::is_none")]
    operation_location: Option<SourceLocation>,
    #[serde(rename = "valueLocation", skip_serializing_if = "Option::is_none")]
    value_location: Option<SourceLocation>,
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
            out_dir,
            format,
            config,
            provenance_out,
        } => {
            let config = load_project_config(config.as_deref())?;
            let inputs = resolve_inputs(input.into_iter().collect(), config.as_ref())?;
            let rules = resolve_rules(rules, config.as_ref())?;
            let out_dir = if out.is_some() {
                out_dir
            } else {
                out_dir.or_else(|| config.as_ref().and_then(|config| config.out_dir.clone()))
            };
            let format = format.or_else(|| config.as_ref().and_then(|config| config.format));
            let rule_file = read_rules(&rules)?;
            inflate_inputs(
                &inputs,
                &rule_file,
                out.as_deref(),
                out_dir.as_deref(),
                format,
                provenance_out.as_deref(),
            )?;
        }
        Commands::Deflate {
            input,
            rules,
            out,
            config,
            format,
            dry_run,
            protect,
        } => {
            let config = load_project_config(config.as_deref())?;
            let input = resolve_single_input(input, config.as_ref(), "deflate")?;
            let rules = resolve_rules(rules, config.as_ref())?;
            let format = format.or_else(|| config.as_ref().and_then(|config| config.format));
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
            all,
            config,
        } => {
            let config = load_project_config(config.as_deref())?;
            let rules = resolve_rules(rules, config.as_ref())?;
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let mut provenance = HashMap::new();
            let mut traces = Vec::new();
            inflate_with_traces(&mut docs, &rule_file, &mut provenance, Some(&mut traces))?;
            explain(&docs, &provenance, &traces, &path, json, all)?;
        }
        Commands::Diff {
            input,
            rules,
            format,
            explain,
            config,
        } => {
            let config = load_project_config(config.as_deref())?;
            let inputs = resolve_inputs(input.into_iter().collect(), config.as_ref())?;
            let rules = resolve_rules(rules, config.as_ref())?;
            let format = format.or_else(|| config.as_ref().and_then(|config| config.format));
            let rule_file = read_rules(&rules)?;
            diff_inputs(&inputs, &rule_file, format, explain)?;
        }
        Commands::Check {
            inputs,
            rules,
            strict,
            write,
            format,
            config,
        } => {
            let config = load_project_config(config.as_deref())?;
            let inputs = resolve_inputs(inputs, config.as_ref())?;
            let rules = resolve_rules(rules, config.as_ref())?;
            let format = format.or_else(|| config.as_ref().and_then(|config| config.format));
            let rule_file = read_rules(&rules)?;
            check_inputs(&inputs, &rule_file, strict, write, format)?;
        }
        Commands::Suggest {
            inputs,
            min_occurrences,
            out,
            json,
            config,
        } => {
            let config = load_project_config(config.as_deref())?;
            let inputs = resolve_inputs(inputs, config.as_ref())?;
            suggest_rules(&inputs, min_occurrences, out.as_deref(), json)?;
        }
    }

    Ok(())
}

fn load_project_config(path: Option<&Path>) -> Result<Option<ProjectConfig>> {
    let Some(path) = path.map(PathBuf::from).or_else(discover_project_config) else {
        return Ok(None);
    };

    let input = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let root = match detect_format(&path) {
        OutputFormat::Json => Value::from_json(
            serde_json::from_str(&input)
                .with_context(|| format!("failed to parse JSON config {}", path.display()))?,
        ),
        OutputFormat::Yaml => Value::parse_yaml_documents(&input)
            .with_context(|| format!("failed to parse YAML config {}", path.display()))?
            .into_iter()
            .next()
            .with_context(|| format!("{} did not contain a config document", path.display()))?,
    };
    let Some(map) = root.as_mapping() else {
        bail!("{} config must be a mapping", path.display());
    };
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let rules =
        optional_config_string(map, "rules")?.map(|rules| resolve_relative_path(base, rules));
    let mut inputs = Vec::new();

    if let Some(input) = optional_config_string(map, "input")? {
        inputs.push(resolve_relative_path(base, input));
    }

    if let Some(value) = map.get("inputs") {
        match value {
            Value::String(input) => inputs.push(resolve_relative_path(base, input)),
            _ => {
                let Some(items) = value.as_sequence() else {
                    bail!("{}.inputs must be a string or sequence", path.display());
                };

                for (index, item) in items.iter().enumerate() {
                    let Some(input) = item.as_str() else {
                        bail!("{}.inputs[{index}] must be a string", path.display());
                    };
                    inputs.push(resolve_relative_path(base, input));
                }
            }
        }
    }

    let out_dir = optional_config_string(map, "outDir")?
        .or(optional_config_string(map, "out-dir")?)
        .or(optional_config_string(map, "out_dir")?)
        .map(|out_dir| resolve_relative_path(base, out_dir));
    let format = optional_config_string(map, "format")?
        .map(|format| parse_output_format_name(&format))
        .transpose()?;

    Ok(Some(ProjectConfig {
        rules,
        inputs,
        out_dir,
        format,
    }))
}

fn discover_project_config() -> Option<PathBuf> {
    [
        "pump.yaml",
        ".pump.yaml",
        "pump.yml",
        ".pump.yml",
        "pump.json",
        ".pump.json",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.exists())
}

fn optional_config_string(map: &IndexMap<String, Value>, key: &str) -> Result<Option<String>> {
    let Some(value) = map.get(key) else {
        return Ok(None);
    };

    value
        .as_str()
        .map(|value| Some(value.to_string()))
        .with_context(|| format!("config field {key} must be a string"))
}

fn resolve_relative_path(base: &Path, value: impl AsRef<Path>) -> PathBuf {
    let path = value.as_ref();

    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn resolve_rules(explicit: Option<PathBuf>, config: Option<&ProjectConfig>) -> Result<PathBuf> {
    explicit
        .or_else(|| config.and_then(|config| config.rules.clone()))
        .context("missing rules file; pass --rules or set rules in pump.yaml")
}

fn resolve_inputs(explicit: Vec<PathBuf>, config: Option<&ProjectConfig>) -> Result<Vec<PathBuf>> {
    if !explicit.is_empty() {
        return Ok(explicit);
    }

    let inputs = config
        .map(|config| config.inputs.clone())
        .unwrap_or_default();

    if inputs.is_empty() {
        bail!("missing input file; pass input path(s) or set inputs in pump.yaml");
    }

    Ok(inputs)
}

fn resolve_single_input(
    explicit: Option<PathBuf>,
    config: Option<&ProjectConfig>,
    command: &str,
) -> Result<PathBuf> {
    if let Some(input) = explicit {
        return Ok(input);
    }

    let inputs = config
        .map(|config| config.inputs.clone())
        .unwrap_or_default();

    match inputs.as_slice() {
        [input] => Ok(input.clone()),
        [] => {
            bail!("missing input file for {command}; pass an input or set one input in pump.yaml")
        }
        _ => bail!("{command} supports one input; pass an input explicitly or configure one input"),
    }
}

fn parse_output_format_name(value: &str) -> Result<OutputFormat> {
    match value {
        "json" => Ok(OutputFormat::Json),
        "yaml" | "yml" => Ok(OutputFormat::Yaml),
        _ => bail!("format must be json or yaml, got {value}"),
    }
}

fn read_input(path: &Path) -> Result<Vec<Value>> {
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read input {}", path.display()))?;

    match detect_format(path) {
        OutputFormat::Json => {
            let doc = serde_json::from_str(&input)
                .with_context(|| format!("failed to parse JSON {}", path.display()))?;
            Ok(vec![Value::from_json(doc)])
        }
        OutputFormat::Yaml => {
            let docs = Value::parse_yaml_documents(&input)
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
    parse_rules_document(path, &input)
}

fn parse_rules_document(path: &Path, input: &str) -> Result<RuleFile> {
    let documents = saphyr::MarkedYamlOwned::load_from_str(input)
        .with_context(|| format!("failed to parse rules {}", path.display()))?;
    let Some(document) = documents.first() else {
        bail!("{} did not contain a rule document", path.display());
    };

    let mut source_locations = HashMap::new();
    collect_rule_source_locations(document, Vec::new(), path, &mut source_locations)?;

    let root = Value::from_marked_yaml(document)
        .with_context(|| format!("failed to read rules {}", path.display()))?;
    let rules_value = required_child(&root, "rules")?;
    let Some(rule_items) = rules_value.as_sequence() else {
        bail!("{} field rules must be a sequence", path.display());
    };

    let rules = rule_items
        .iter()
        .enumerate()
        .map(|(index, value)| parse_rule(index, value))
        .collect::<Result<Vec<_>>>()?;

    let rules = RuleFile {
        rules,
        source_locations,
    };

    if rules.rules.is_empty() {
        bail!("{} did not define any rules", path.display());
    }

    validate_rules(&rules)?;

    Ok(rules)
}

fn parse_rule(index: usize, value: &Value) -> Result<Rule> {
    let Some(map) = value.as_mapping() else {
        bail!("rules[{index}] must be a mapping");
    };

    let name = required_string(map, "name", index)?;
    let match_path = required_string(map, "match", index)?;
    let api_version = optional_string(map, "apiVersion", index)?;
    let kind = optional_string(map, "kind", index)?;
    let metadata_name = optional_string(map, "metadata.name", index)?.or(optional_string(
        map,
        "metadataName",
        index,
    )?);
    let defaults = map.get("defaults").cloned();
    let overrides = map.get("overrides").cloned();
    let delete = optional_string_sequence(map, "delete", index)?;
    let replace = map.get("replace").cloned();

    Ok(Rule {
        name,
        match_path,
        api_version,
        kind,
        metadata_name,
        defaults,
        overrides,
        delete,
        replace,
    })
}

fn required_child<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .get(key)
        .with_context(|| format!("missing required field {key}"))
}

fn required_string(
    map: &indexmap::IndexMap<String, Value>,
    key: &str,
    index: usize,
) -> Result<String> {
    optional_string(map, key, index)?.with_context(|| format!("rules[{index}].{key} is required"))
}

fn optional_string(
    map: &indexmap::IndexMap<String, Value>,
    key: &str,
    index: usize,
) -> Result<Option<String>> {
    let Some(value) = map.get(key) else {
        return Ok(None);
    };

    value
        .as_str()
        .map(|value| Some(value.to_string()))
        .with_context(|| format!("rules[{index}].{key} must be a string"))
}

fn optional_string_sequence(
    map: &indexmap::IndexMap<String, Value>,
    key: &str,
    index: usize,
) -> Result<Vec<String>> {
    let Some(value) = map.get(key) else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_sequence() else {
        bail!("rules[{index}].{key} must be a sequence");
    };

    values
        .iter()
        .enumerate()
        .map(|(item_index, value)| {
            value
                .as_str()
                .map(ToString::to_string)
                .with_context(|| format!("rules[{index}].{key}[{item_index}] must be a string"))
        })
        .collect()
}

fn collect_rule_source_locations(
    value: &saphyr::MarkedYamlOwned,
    path: Vec<String>,
    file: &Path,
    index: &mut RuleSourceIndex,
) -> Result<()> {
    match &value.data {
        saphyr::YamlDataOwned::Mapping(values) => {
            for (key, child) in values {
                let key_text = marked_scalar_to_string(key)?;
                let mut child_path = path.clone();
                child_path.push(key_text);
                record_rule_source_location(&child_path, file, key.span, index);
                collect_rule_source_locations(child, child_path, file, index)?;
            }
        }
        saphyr::YamlDataOwned::Sequence(values) => {
            for (item_index, child) in values.iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(item_index.to_string());
                record_rule_source_location(&child_path, file, child.span, index);
                collect_rule_source_locations(child, child_path, file, index)?;
            }
        }
        saphyr::YamlDataOwned::Tagged(_, child) => {
            collect_rule_source_locations(child, path, file, index)?;
        }
        _ => {}
    }

    Ok(())
}

fn marked_scalar_to_string(value: &saphyr::MarkedYamlOwned) -> Result<String> {
    match Value::from_marked_yaml(value)? {
        Value::String(value) => Ok(value),
        Value::Number(value::Number::Integer(value)) => Ok(value.to_string()),
        Value::Number(value::Number::Unsigned(value)) => Ok(value.to_string()),
        Value::Number(value::Number::Float(value)) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok("null".to_string()),
        Value::Tagged { value, .. } => match *value {
            Value::String(value) => Ok(value),
            _ => bail!("expected scalar mapping key while indexing rule locations"),
        },
        Value::Sequence(_) | Value::Mapping(_) => {
            bail!("expected scalar mapping key while indexing rule locations")
        }
    }
}

fn record_rule_source_location(
    path: &[String],
    file: &Path,
    span: saphyr_parser::Span,
    index: &mut RuleSourceIndex,
) {
    let [root, rule_index, operation, relative_path @ ..] = path else {
        return;
    };

    if root != "rules"
        || !matches!(
            operation.as_str(),
            "defaults" | "overrides" | "delete" | "replace"
        )
    {
        return;
    }

    let Ok(rule_index) = rule_index.parse::<usize>() else {
        return;
    };

    index.insert(
        value_location_key(rule_index, operation, relative_path),
        SourceLocation {
            file: file.display().to_string(),
            line: span.start.line() as u64,
            column: span.start.col() as u64 + 1,
            byte_index: span.start.index() as u64,
        },
    );
}

fn operation_key(rule_index: usize, operation: &str) -> String {
    let operation = match operation {
        "default" => "defaults",
        "override" => "overrides",
        other => other,
    };

    format!("{rule_index}:{operation}")
}

fn value_location_key(rule_index: usize, operation: &str, rule_value_path: &[String]) -> String {
    let operation_key = operation_key(rule_index, operation);

    if rule_value_path.is_empty() {
        operation_key
    } else {
        format!("{operation_key}:{}", json_pointer(rule_value_path))
    }
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

    parse_path(&rule.match_path).with_context(|| {
        format!(
            "rule {} has invalid match path {}",
            rule.name, rule.match_path
        )
    })?;

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
    inflate_with_traces(docs, rule_file, provenance, None)
}

fn inflate_with_traces(
    docs: &mut [Value],
    rule_file: &RuleFile,
    provenance: &mut Provenance,
    mut traces: Option<&mut Vec<RuleTrace>>,
) -> Result<()> {
    validate_rules(rule_file)?;

    for (rule_index, rule) in rule_file.rules.iter().enumerate() {
        let segments = parse_path(&rule.match_path)
            .with_context(|| format!("invalid match path for rule {}", rule.name))?;
        let default_location = rule_file.operation_location(rule_index, "default");
        let override_location = rule_file.operation_location(rule_index, "override");
        let delete_location = rule_file.operation_location(rule_index, "delete");
        let replace_location = rule_file.operation_location(rule_index, "replace");

        for (doc_index, doc) in docs.iter_mut().enumerate() {
            let mut trace = RuleTrace {
                doc: doc_index,
                rule: rule.name.clone(),
                rule_index,
                match_path: rule.match_path.clone(),
                decision: "skipped".to_string(),
                reason: String::new(),
                target_paths: Vec::new(),
                operations: Vec::new(),
            };

            if let Some(reason) = rule_skip_reason(rule, doc) {
                trace.reason = reason;
                if let Some(traces) = traces.as_deref_mut() {
                    traces.push(trace);
                }
                continue;
            }

            let target_paths = matching_paths(doc, &segments);
            trace.target_paths = target_paths
                .iter()
                .map(|path| json_pointer(path))
                .collect::<Vec<_>>();

            if target_paths.is_empty() {
                trace.reason = "match path matched no values".to_string();
                if let Some(traces) = traces.as_deref_mut() {
                    traces.push(trace);
                }
                continue;
            }

            for target_path in target_paths {
                if let Some(replacement) = &rule.replace
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    let before = target.clone();
                    *target = replacement.clone();
                    record_generated_paths(
                        &target_path,
                        replacement,
                        &[],
                        provenance,
                        &ProvenanceContext {
                            doc_index,
                            rule_name: &rule.name,
                            operation: "replace",
                            reason: "value replaced by rule",
                            operation_location: replace_location,
                            rule_file,
                            rule_index,
                        },
                    );
                    let value_location = rule_file.value_location(rule_index, "replace", &[]);
                    trace.operations.push(OperationTrace {
                        operation: "replace".to_string(),
                        path: json_pointer(&target_path),
                        outcome: "replaced".to_string(),
                        reason: "value replaced by rule".to_string(),
                        before: Some(before),
                        after: Some(replacement.clone()),
                        operation_location: replace_location.cloned(),
                        value_location: value_location.cloned(),
                    });
                    continue;
                }

                if let Some(defaults) = &rule.defaults
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    apply_defaults(
                        target,
                        defaults,
                        RuleApplyContext {
                            doc_index,
                            rule_name: &rule.name,
                        },
                        &target_path,
                        provenance,
                        &[],
                        OperationContext {
                            rule_file,
                            rule_index,
                            operation: "default",
                            operation_location: default_location,
                            operations: Some(&mut trace.operations),
                        },
                    );
                }

                if let Some(overrides) = &rule.overrides
                    && let Some(target) = get_mut_at_path(doc, &target_path)
                {
                    apply_overrides(
                        target,
                        overrides,
                        RuleApplyContext {
                            doc_index,
                            rule_name: &rule.name,
                        },
                        &target_path,
                        provenance,
                        &[],
                        OperationContext {
                            rule_file,
                            rule_index,
                            operation: "override",
                            operation_location: override_location,
                            operations: Some(&mut trace.operations),
                        },
                    );
                }

                apply_delete_paths(
                    doc,
                    doc_index,
                    &target_path,
                    &rule.delete,
                    &rule.name,
                    provenance,
                    OperationContext {
                        rule_file,
                        rule_index,
                        operation: "delete",
                        operation_location: delete_location,
                        operations: Some(&mut trace.operations),
                    },
                )
                .with_context(|| format!("invalid delete path for rule {}", rule.name))?;
            }

            trace.decision = "applied".to_string();
            trace.reason = if trace.operations.is_empty() {
                format!(
                    "matched {} target path(s), no values changed",
                    trace.target_paths.len()
                )
            } else {
                format!(
                    "matched {} target path(s), {} operation(s) changed values",
                    trace.target_paths.len(),
                    trace.operations.len()
                )
            };

            if let Some(traces) = traces.as_deref_mut() {
                traces.push(trace);
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

fn check(
    input_path: &Path,
    rule_file: &RuleFile,
    docs: &mut [Value],
    strict: bool,
    write: bool,
    format: Option<OutputFormat>,
) -> Result<()> {
    let result = check_docs(
        input_path, rule_file, docs, strict, write, format, "inflated",
    )?;

    if write {
        println!(
            "wrote: {} document(s), {} rule(s), {} generated value(s)",
            result.documents,
            rule_file.rules.len(),
            result.generated
        );
        return Ok(());
    }

    if strict && result.changed {
        io::stdout().flush()?;
        bail!(
            "strict check failed: applying rules would change {}",
            input_path.display()
        );
    }

    println!(
        "ok: {} document(s), {} rule(s), {} generated value(s)",
        result.documents,
        rule_file.rules.len(),
        result.generated
    );

    Ok(())
}

fn check_inputs(
    input_paths: &[PathBuf],
    rule_file: &RuleFile,
    strict: bool,
    write: bool,
    format: Option<OutputFormat>,
) -> Result<()> {
    if input_paths.len() == 1 {
        let input_path = &input_paths[0];
        let mut docs = read_input(input_path)?;
        return check(input_path, rule_file, &mut docs, strict, write, format);
    }

    let mut total_documents = 0;
    let mut total_generated = 0;
    let mut changed_files = 0;

    for input_path in input_paths {
        let mut docs = read_input(input_path)?;
        let result = check_docs(
            input_path,
            rule_file,
            &mut docs,
            strict,
            write,
            format,
            &format!("inflated: {}", input_path.display()),
        )?;

        total_documents += result.documents;
        total_generated += result.generated;

        if result.changed {
            changed_files += 1;
        }

        if write {
            println!(
                "wrote: {}: {} document(s), {} generated value(s)",
                input_path.display(),
                result.documents,
                result.generated
            );
        } else if strict && result.changed {
            println!(
                "changed: {}: {} document(s), {} generated value(s)",
                input_path.display(),
                result.documents,
                result.generated
            );
        } else {
            println!(
                "ok: {}: {} document(s), {} generated value(s)",
                input_path.display(),
                result.documents,
                result.generated
            );
        }
    }

    if strict && changed_files > 0 {
        io::stdout().flush()?;
        bail!(
            "strict check failed: applying rules would change {} of {} file(s)",
            changed_files,
            input_paths.len()
        );
    }

    let action = if write { "wrote" } else { "ok" };
    println!(
        "{action}: {} file(s), {} document(s), {} rule(s), {} generated value(s)",
        input_paths.len(),
        total_documents,
        rule_file.rules.len(),
        total_generated
    );

    Ok(())
}

fn inflate_inputs(
    input_paths: &[PathBuf],
    rule_file: &RuleFile,
    out: Option<&Path>,
    out_dir: Option<&Path>,
    format: Option<OutputFormat>,
    provenance_out: Option<&Path>,
) -> Result<()> {
    if out.is_some() && out_dir.is_some() {
        bail!("use either --out or --out-dir, not both");
    }

    if input_paths.len() > 1 {
        if out.is_some() {
            bail!("--out can only be used with one input; use --out-dir for multiple inputs");
        }

        if provenance_out.is_some() {
            bail!("--provenance-out can only be used with one input");
        }

        let Some(out_dir) = out_dir else {
            bail!("inflating multiple inputs requires --out-dir or config outDir");
        };

        fs::create_dir_all(out_dir)
            .with_context(|| format!("failed to create output directory {}", out_dir.display()))?;

        let mut output_names = HashSet::new();

        for input_path in input_paths {
            let file_name = input_path
                .file_name()
                .with_context(|| format!("input {} has no file name", input_path.display()))?;

            if !output_names.insert(file_name.to_os_string()) {
                bail!(
                    "multiple inputs would write {}; use unique file names for batch output",
                    out_dir.join(file_name).display()
                );
            }

            let out_path = out_dir.join(file_name);
            let mut docs = read_input(input_path)?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, rule_file, &mut provenance)?;
            write_output(&docs, input_path, Some(&out_path), format)?;
            println!("wrote: {}", out_path.display());
        }

        return Ok(());
    }

    let input_path = input_paths
        .first()
        .context("missing input file; pass input path(s) or set inputs in pump.yaml")?;
    let out_path = out_dir
        .map(|out_dir| {
            input_path
                .file_name()
                .map(|file_name| out_dir.join(file_name))
                .with_context(|| format!("input {} has no file name", input_path.display()))
        })
        .transpose()?;
    let out = out.or(out_path.as_deref());
    let mut docs = read_input(input_path)?;
    let mut provenance = HashMap::new();

    if let Some(out_dir) = out_dir {
        fs::create_dir_all(out_dir)
            .with_context(|| format!("failed to create output directory {}", out_dir.display()))?;
    }

    inflate(&mut docs, rule_file, &mut provenance)?;
    write_output(&docs, input_path, out, format)?;

    if let Some(provenance_out) = provenance_out {
        write_provenance(&provenance, provenance_out)?;
    }

    Ok(())
}

fn diff_inputs(
    input_paths: &[PathBuf],
    rule_file: &RuleFile,
    format: Option<OutputFormat>,
    explain: bool,
) -> Result<()> {
    for (index, input_path) in input_paths.iter().enumerate() {
        if input_paths.len() > 1 {
            if index > 0 {
                println!();
            }
            println!("diff: {}", input_path.display());
        }

        let mut docs = read_input(input_path)?;
        let output_format = output_format(input_path, None, format);
        let before = format_docs(&docs, output_format)?;
        let mut provenance = HashMap::new();
        let mut traces = Vec::new();

        if explain {
            inflate_with_traces(&mut docs, rule_file, &mut provenance, Some(&mut traces))?;
        } else {
            inflate(&mut docs, rule_file, &mut provenance)?;
        }

        let after = format_docs(&docs, output_format)?;
        print_diff(&before, &after, "inflated");

        if explain {
            print_diff_explanations(&traces);
        }
    }

    Ok(())
}

fn suggest_rules(
    input_paths: &[PathBuf],
    min_occurrences: usize,
    out: Option<&Path>,
    json: bool,
) -> Result<()> {
    if min_occurrences < 2 {
        bail!("--min-occurrences must be at least 2");
    }

    let mut candidates = HashMap::new();

    for (file_index, input_path) in input_paths.iter().enumerate() {
        let docs = read_input(input_path)?;

        for (doc_index, doc) in docs.iter().enumerate() {
            collect_suggest_candidates(
                doc,
                &mut Vec::new(),
                file_index,
                doc_index,
                &mut candidates,
            )?;
        }
    }

    let mut candidates = candidates
        .into_values()
        .filter(|candidate: &SuggestCandidate| candidate.occurrences >= min_occurrences)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .occurrences
            .cmp(&left.occurrences)
            .then_with(|| left.match_path.cmp(&right.match_path))
            .then_with(|| left.defaults_path.cmp(&right.defaults_path))
    });

    if json {
        let suggestions = candidates
            .into_iter()
            .map(|candidate| SuggestedDefault {
                match_path: candidate.match_path,
                defaults_path: json_pointer(&candidate.defaults_path),
                value: candidate.value,
                occurrences: candidate.occurrences,
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&SuggestOutput { suggestions })?
        );
        return Ok(());
    }

    if candidates.is_empty() {
        bail!(
            "no repeated defaults found; lower --min-occurrences or run suggest against inflated examples"
        );
    }

    let rule_file = suggested_rule_file(&candidates);
    let output = format_docs(&[rule_file], OutputFormat::Yaml)?;

    if let Some(out) = out {
        fs::write(out, output)
            .with_context(|| format!("failed to write suggested rules {}", out.display()))?;
    } else {
        print!("{output}");
    }

    Ok(())
}

fn collect_suggest_candidates(
    value: &Value,
    path: &mut Vec<String>,
    file_index: usize,
    doc_index: usize,
    candidates: &mut HashMap<String, SuggestCandidate>,
) -> Result<()> {
    if let Some(map) = value.as_mapping() {
        for (key, child) in map {
            path.push(key.clone());
            collect_suggest_candidates(child, path, file_index, doc_index, candidates)?;
            path.pop();
        }

        return Ok(());
    }

    if let Some(items) = value.as_sequence() {
        for (index, child) in items.iter().enumerate() {
            path.push(index.to_string());
            collect_suggest_candidates(child, path, file_index, doc_index, candidates)?;
            path.pop();
        }

        return Ok(());
    }

    if path.is_empty() || path.iter().any(|segment| segment.parse::<usize>().is_ok()) {
        return Ok(());
    }

    let value_key = serde_json::to_string(value)?;
    let root_key = format!("{}:{}:{}", file_index, doc_index, json_pointer(path));

    for (match_segments, defaults_path) in suggest_patterns(path) {
        let match_path = path_to_rule_path(&match_segments);
        let key = format!("{match_path}:{}:{value_key}", json_pointer(&defaults_path));
        let entry = candidates.entry(key).or_insert_with(|| SuggestCandidate {
            match_path,
            defaults_path,
            value: value.clone(),
            occurrences: 0,
            roots: HashSet::new(),
        });

        if entry.roots.insert(root_key.clone()) {
            entry.occurrences += 1;
        }
    }

    Ok(())
}

fn suggest_patterns(path: &[String]) -> Vec<(Vec<String>, Vec<String>)> {
    let Some((leaf, parent)) = path.split_last() else {
        return Vec::new();
    };

    let mut patterns = Vec::new();
    patterns.push((parent.to_vec(), vec![leaf.clone()]));

    for wildcard_index in 0..parent.len() {
        let mut match_path = parent.to_vec();
        match_path[wildcard_index] = "*".to_string();
        patterns.push((match_path, vec![leaf.clone()]));
    }

    patterns
}

fn suggested_rule_file(candidates: &[SuggestCandidate]) -> Value {
    let mut rules = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        let mut rule = IndexMap::new();
        rule.insert(
            "name".to_string(),
            Value::String(format!("suggested-default-{}", index + 1)),
        );
        rule.insert(
            "match".to_string(),
            Value::String(candidate.match_path.clone()),
        );
        rule.insert(
            "defaults".to_string(),
            nested_default(&candidate.defaults_path, candidate.value.clone()),
        );
        rules.push(Value::Mapping(rule));
    }

    let mut root = IndexMap::new();
    root.insert("rules".to_string(), Value::Sequence(rules));
    Value::Mapping(root)
}

fn nested_default(path: &[String], value: Value) -> Value {
    let Some((head, tail)) = path.split_first() else {
        return value;
    };

    let mut map = IndexMap::new();
    map.insert(head.clone(), nested_default(tail, value));
    Value::Mapping(map)
}

fn check_docs(
    input_path: &Path,
    rule_file: &RuleFile,
    docs: &mut [Value],
    strict: bool,
    write: bool,
    format: Option<OutputFormat>,
    diff_name: &str,
) -> Result<CheckResult> {
    let output_format = output_format(input_path, Some(input_path), format);
    let should_diff = strict && !write;
    let before = should_diff
        .then(|| format_docs(docs, output_format))
        .transpose()?;
    let mut provenance = HashMap::new();

    inflate(docs, rule_file, &mut provenance)?;

    if write {
        write_output(docs, input_path, Some(input_path), format)?;
    }

    let changed = if should_diff {
        let before = before.expect("strict check should capture input before inflate");
        let after = format_docs(docs, output_format)?;

        if before != after {
            print_diff(&before, &after, diff_name);
            true
        } else {
            false
        }
    } else {
        false
    };

    Ok(CheckResult {
        documents: docs.len(),
        generated: provenance.len(),
        changed,
    })
}

fn apply_defaults(
    target: &mut Value,
    defaults: &Value,
    apply: RuleApplyContext<'_>,
    path: &[String],
    provenance: &mut Provenance,
    rule_value_path: &[String],
    mut context: OperationContext<'_, '_>,
) {
    let Some(target_map) = target.as_mapping_mut() else {
        return;
    };
    let Some(default_map) = defaults.as_mapping() else {
        return;
    };

    for (key_value, default_value) in default_map {
        let key = key_value.as_str();
        let mut child_path = path.to_vec();
        child_path.push(key.to_string());
        let mut child_rule_value_path = rule_value_path.to_vec();
        child_rule_value_path.push(key.to_string());

        match target_map.get_mut(key) {
            Some(existing) if existing.is_mapping() && default_value.is_mapping() => {
                apply_defaults(
                    existing,
                    default_value,
                    apply,
                    &child_path,
                    provenance,
                    &child_rule_value_path,
                    OperationContext {
                        rule_file: context.rule_file,
                        rule_index: context.rule_index,
                        operation: context.operation,
                        operation_location: context.operation_location,
                        operations: context.operations.as_deref_mut(),
                    },
                );
            }
            Some(_) => {}
            None => {
                target_map.insert(key_value.clone(), default_value.clone());
                record_generated_paths(
                    &child_path,
                    default_value,
                    &child_rule_value_path,
                    provenance,
                    &ProvenanceContext {
                        doc_index: apply.doc_index,
                        rule_name: apply.rule_name,
                        operation: "default",
                        reason: "field was missing in source",
                        operation_location: context.operation_location,
                        rule_file: context.rule_file,
                        rule_index: context.rule_index,
                    },
                );
                let value_location = context.value_location(&child_rule_value_path).cloned();
                context.push(OperationTraceInput {
                    operation: "default",
                    path: &child_path,
                    outcome: "generated",
                    reason: "field was missing in source",
                    before: None,
                    after: Some(default_value.clone()),
                    value_location,
                });
            }
        }
    }
}

fn apply_overrides(
    target: &mut Value,
    overrides: &Value,
    apply: RuleApplyContext<'_>,
    path: &[String],
    provenance: &mut Provenance,
    rule_value_path: &[String],
    mut context: OperationContext<'_, '_>,
) {
    match (target, overrides) {
        (target, overrides) if target.is_mapping() && overrides.is_mapping() => {
            let target_map = target
                .as_mapping_mut()
                .expect("target mapping checked above");
            let override_map = overrides
                .as_mapping()
                .expect("override mapping checked above");

            for (key_value, override_value) in override_map {
                let key = key_value.as_str();
                let mut child_path = path.to_vec();
                child_path.push(key.to_string());
                let mut child_rule_value_path = rule_value_path.to_vec();
                child_rule_value_path.push(key.to_string());

                match target_map.get_mut(key) {
                    Some(existing) if existing.is_mapping() && override_value.is_mapping() => {
                        apply_overrides(
                            existing,
                            override_value,
                            apply,
                            &child_path,
                            provenance,
                            &child_rule_value_path,
                            OperationContext {
                                rule_file: context.rule_file,
                                rule_index: context.rule_index,
                                operation: context.operation,
                                operation_location: context.operation_location,
                                operations: context.operations.as_deref_mut(),
                            },
                        );
                    }
                    Some(existing) => {
                        let before = existing.clone();
                        *existing = override_value.clone();
                        record_generated_paths(
                            &child_path,
                            override_value,
                            &child_rule_value_path,
                            provenance,
                            &ProvenanceContext {
                                doc_index: apply.doc_index,
                                rule_name: apply.rule_name,
                                operation: "override",
                                reason: "value forced by rule",
                                operation_location: context.operation_location,
                                rule_file: context.rule_file,
                                rule_index: context.rule_index,
                            },
                        );
                        let value_location =
                            context.value_location(&child_rule_value_path).cloned();
                        context.push(OperationTraceInput {
                            operation: "override",
                            path: &child_path,
                            outcome: "overwritten",
                            reason: "value forced by rule",
                            before: Some(before),
                            after: Some(override_value.clone()),
                            value_location,
                        });
                    }
                    None => {
                        target_map.insert(key_value.clone(), override_value.clone());
                        record_generated_paths(
                            &child_path,
                            override_value,
                            &child_rule_value_path,
                            provenance,
                            &ProvenanceContext {
                                doc_index: apply.doc_index,
                                rule_name: apply.rule_name,
                                operation: "override",
                                reason: "value forced by rule",
                                operation_location: context.operation_location,
                                rule_file: context.rule_file,
                                rule_index: context.rule_index,
                            },
                        );
                        let value_location =
                            context.value_location(&child_rule_value_path).cloned();
                        context.push(OperationTraceInput {
                            operation: "override",
                            path: &child_path,
                            outcome: "generated",
                            reason: "value forced by rule",
                            before: None,
                            after: Some(override_value.clone()),
                            value_location,
                        });
                    }
                }
            }
        }
        (target, overrides) => {
            let before = target.clone();
            *target = overrides.clone();
            record_generated_paths(
                path,
                overrides,
                rule_value_path,
                provenance,
                &ProvenanceContext {
                    doc_index: apply.doc_index,
                    rule_name: apply.rule_name,
                    operation: "override",
                    reason: "value forced by rule",
                    operation_location: context.operation_location,
                    rule_file: context.rule_file,
                    rule_index: context.rule_index,
                },
            );
            let value_location = context.value_location(rule_value_path).cloned();
            context.push(OperationTraceInput {
                operation: "override",
                path,
                outcome: "overwritten",
                reason: "value forced by rule",
                before: Some(before),
                after: Some(overrides.clone()),
                value_location,
            });
        }
    }
}

fn remove_values_matching(
    target: &mut Value,
    template: &Value,
    path: &[String],
    options: &DeflateOptions,
) {
    let Some(target_map) = target.as_mapping_mut() else {
        return;
    };
    let Some(template_map) = template.as_mapping() else {
        return;
    };

    let mut remove_keys = Vec::new();

    for (key_value, template_value) in template_map {
        let key = key_value.as_str();
        let mut child_path = path.to_vec();
        child_path.push(key.to_string());

        let Some(existing) = target_map.get_mut(key) else {
            continue;
        };

        if existing == template_value {
            if !options.protects(&child_path) {
                remove_keys.push(key.to_string());
            }
        } else if existing.is_mapping() && template_value.is_mapping() {
            remove_values_matching(existing, template_value, &child_path, options);

            if existing
                .as_mapping()
                .is_some_and(|mapping| mapping.is_empty())
                && !options.protects(&child_path)
            {
                remove_keys.push(key.to_string());
            }
        }
    }

    for key in remove_keys {
        target_map.shift_remove(&key);
    }
}

fn apply_delete_paths(
    doc: &mut Value,
    doc_index: usize,
    base_path: &[String],
    delete_paths: &[String],
    rule_name: &str,
    provenance: &mut Provenance,
    mut context: OperationContext<'_, '_>,
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
                context.operation_location,
            );
            let before = remove_at_path(doc, &path);
            context.push(OperationTraceInput {
                operation: "delete",
                path: &path,
                outcome: "deleted",
                reason: "value removed by rule",
                before,
                after: None,
                value_location: None,
            });
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

    if let Some(map) = parent.as_mapping_mut() {
        return map.shift_remove(last);
    }

    if let Some(items) = parent.as_sequence_mut() {
        return last
            .parse::<usize>()
            .ok()
            .filter(|index| *index < items.len())
            .map(|index| items.remove(index));
    }

    None
}

fn record_provenance(
    doc_index: usize,
    path: &[String],
    rule_name: &str,
    operation: &str,
    reason: &str,
    provenance: &mut Provenance,
    operation_location: Option<&SourceLocation>,
) {
    provenance.insert(
        provenance_key(doc_index, path),
        ProvenanceEntry {
            doc: doc_index,
            path: json_pointer(path),
            rule: rule_name.to_string(),
            operation: operation.to_string(),
            reason: reason.to_string(),
            operation_location: operation_location.cloned(),
            value_location: None,
        },
    );
}

fn rule_matches_doc(rule: &Rule, doc: &Value) -> bool {
    rule_skip_reason(rule, doc).is_none()
}

fn rule_skip_reason(rule: &Rule, doc: &Value) -> Option<String> {
    selector_skip_reason(
        doc,
        &["apiVersion"],
        "apiVersion",
        rule.api_version.as_deref(),
    )
    .or_else(|| selector_skip_reason(doc, &["kind"], "kind", rule.kind.as_deref()))
    .or_else(|| {
        selector_skip_reason(
            doc,
            &["metadata", "name"],
            "metadata.name",
            rule.metadata_name.as_deref(),
        )
    })
}

fn selector_skip_reason(
    doc: &Value,
    path: &[&str],
    label: &str,
    expected: Option<&str>,
) -> Option<String> {
    let expected = expected?;

    let mut current = doc;
    for segment in path {
        let Some(next) = value_child(current, segment) else {
            return Some(format!(
                "selector {label} expected {expected}, but path is missing"
            ));
        };
        current = next;
    }

    if current.as_str() == Some(expected) {
        None
    } else {
        Some(format!(
            "selector {label} expected {expected}, got {}",
            format_inline(current)
        ))
    }
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
    path: &[String],
    value: &Value,
    rule_value_path: &[String],
    provenance: &mut Provenance,
    context: &ProvenanceContext<'_>,
) {
    let key = provenance_key(context.doc_index, path);
    let value_location =
        context
            .rule_file
            .value_location(context.rule_index, context.operation, rule_value_path);
    provenance.insert(
        key,
        ProvenanceEntry {
            doc: context.doc_index,
            path: json_pointer(path),
            rule: context.rule_name.to_string(),
            operation: context.operation.to_string(),
            reason: context.reason.to_string(),
            operation_location: context.operation_location.cloned(),
            value_location: value_location.cloned(),
        },
    );

    if let Some(map) = value.as_mapping() {
        for (child_key, child_value) in map {
            let child_key = child_key.as_str();
            let mut child_path = path.to_vec();
            child_path.push(child_key.to_string());
            let mut child_rule_value_path = rule_value_path.to_vec();
            child_rule_value_path.push(child_key.to_string());
            record_generated_paths(
                &child_path,
                child_value,
                &child_rule_value_path,
                provenance,
                context,
            );
        }
    } else if let Some(items) = value.as_sequence() {
        for (index, child_value) in items.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(index.to_string());
            let mut child_rule_value_path = rule_value_path.to_vec();
            child_rule_value_path.push(index.to_string());
            record_generated_paths(
                &child_path,
                child_value,
                &child_rule_value_path,
                provenance,
                context,
            );
        }
    }
}

fn explain(
    docs: &[Value],
    provenance: &Provenance,
    traces: &[RuleTrace],
    query: &str,
    json: bool,
    all: bool,
) -> Result<()> {
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
            let entry_traces = traces_for_doc(traces, doc_index, &path, all);

            if json {
                entries.push(ExplainEntry {
                    doc: doc_index,
                    path: json_pointer(&path),
                    value: value.clone(),
                    trace: entry_traces,
                    source: source.map_or_else(
                        || ExplainSource {
                            source_type: "input".to_string(),
                            rule: None,
                            operation: None,
                            reason: "authored value or existing container".to_string(),
                            operation_location: None,
                            value_location: None,
                        },
                        |source| ExplainSource {
                            source_type: "rule".to_string(),
                            rule: Some(source.rule.clone()),
                            operation: Some(source.operation.clone()),
                            reason: source.reason.clone(),
                            operation_location: source.operation_location.clone(),
                            value_location: source.value_location.clone(),
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
                if let Some(location) = &source.operation_location {
                    println!("location: {}", format_location(location));
                }
            } else {
                println!("source: input");
                println!("reason: authored value or existing container");
            }

            print_trace(&entry_traces);
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

fn traces_for_doc(
    traces: &[RuleTrace],
    doc_index: usize,
    path: &[String],
    include_all: bool,
) -> Vec<RuleTrace> {
    let query_path = json_pointer(path);

    traces
        .iter()
        .filter(|trace| trace.doc == doc_index)
        .filter_map(|trace| {
            if include_all {
                return Some(trace.clone());
            }

            let related_operations = trace
                .operations
                .iter()
                .filter(|operation| paths_are_related(&operation.path, &query_path))
                .cloned()
                .collect::<Vec<_>>();

            if related_operations.is_empty() {
                return None;
            }

            Some(RuleTrace {
                operations: related_operations,
                ..trace.clone()
            })
        })
        .collect()
}

fn print_trace(traces: &[RuleTrace]) {
    if traces.is_empty() {
        return;
    }

    println!("trace:");

    for trace in traces {
        let related_operations = trace.operations.iter().collect::<Vec<_>>();

        if related_operations.is_empty() {
            println!("- {}: {} ({})", trace.rule, trace.decision, trace.reason);
            continue;
        }

        for operation in related_operations {
            let location = operation
                .operation_location
                .as_ref()
                .map(|location| format!(" at {}", format_location(location)))
                .unwrap_or_default();
            println!(
                "- {}: {} {} {} ({}){}",
                trace.rule,
                operation.operation,
                operation.path,
                operation.outcome,
                operation.reason,
                location
            );
        }
    }
}

fn print_diff_explanations(traces: &[RuleTrace]) {
    let mut operations = Vec::new();
    let include_doc = traces
        .iter()
        .map(|trace| trace.doc)
        .collect::<HashSet<_>>()
        .len()
        > 1;

    for trace in traces {
        for operation in &trace.operations {
            operations.push((trace, operation));
        }
    }

    if operations.is_empty() {
        return;
    }

    operations.sort_by(|(_, left), (_, right)| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.operation.cmp(&right.operation))
    });

    println!();
    println!("explain:");

    for (trace, operation) in operations {
        let location = operation
            .value_location
            .as_ref()
            .or(operation.operation_location.as_ref())
            .map(|location| format!(" at {}", format_location(location)))
            .unwrap_or_default();
        let path = if include_doc {
            format!("doc {} {}", trace.doc, operation.path)
        } else {
            operation.path.clone()
        };
        println!(
            "- {}: {} {} {} by rule {} ({}){}",
            path,
            operation.operation,
            operation.outcome,
            format_after_value(operation),
            trace.rule,
            operation.reason,
            location
        );
    }
}

fn format_after_value(operation: &OperationTrace) -> String {
    operation
        .after
        .as_ref()
        .map(|value| format!("to {}", format_inline(value)))
        .unwrap_or_else(|| "from input".to_string())
}

fn format_location(location: &SourceLocation) -> String {
    format!("{}:{}:{}", location.file, location.line, location.column)
}

fn paths_are_related(left: &str, right: &str) -> bool {
    left == right || path_is_ancestor(left, right) || path_is_ancestor(right, left)
}

fn path_is_ancestor(parent: &str, child: &str) -> bool {
    if parent.is_empty() {
        return true;
    }

    child
        .strip_prefix(parent)
        .is_some_and(|remaining| remaining.starts_with('/'))
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
            _ if value.is_mapping() => {
                let map = value.as_mapping().expect("mapping checked above");
                for (key, child) in map {
                    let key = key.as_str();
                    let mut child_path = current_path.clone();
                    child_path.push(key.to_string());
                    collect_matching_paths(child, tail, child_path, output);
                }
            }
            _ if value.is_sequence() => {
                let items = value.as_sequence().expect("sequence checked above");
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
    if let Some(map) = value.as_mapping() {
        return map.get(key);
    }

    if let Some(items) = value.as_sequence() {
        return key.parse::<usize>().ok().and_then(|index| items.get(index));
    }

    None
}

fn value_child_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    if value.is_mapping() {
        return value
            .as_mapping_mut()
            .expect("mapping checked above")
            .get_mut(key);
    }

    if value.is_sequence() {
        return key.parse::<usize>().ok().and_then(|index| {
            value
                .as_sequence_mut()
                .expect("sequence checked above")
                .get_mut(index)
        });
    }

    None
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
                if index > 0 && !output.ends_with('\n') {
                    output.push('\n');
                }

                if docs.len() > 1 {
                    output.push_str("---\n");
                }

                let rendered = doc.to_yaml_string()?;
                output.push_str(rendered.strip_prefix("---\n").unwrap_or(&rendered));
                if !output.ends_with('\n') {
                    output.push('\n');
                }
            }

            Ok(output)
        }
    }
}

fn format_inline(value: &Value) -> String {
    match value {
        Value::String(value) => format!("{value:?}"),
        value if value.is_mapping() || value.is_sequence() => {
            serde_json::to_string(value).unwrap_or_default()
        }
        _ => format_yaml_inline(value),
    }
}

fn format_yaml_inline(value: &Value) -> String {
    value
        .to_yaml_string()
        .map(|rendered| rendered.trim().trim_start_matches("---").trim().to_string())
        .unwrap_or_default()
}

fn human_path(path: &[String]) -> String {
    if path.is_empty() {
        "$".to_string()
    } else {
        format!("$.{}", path.join("."))
    }
}

fn path_to_rule_path(path: &[String]) -> String {
    if path.is_empty() {
        return "$".to_string();
    }

    if path.iter().all(|segment| {
        segment == "*"
            || segment
                .chars()
                .all(|char| char.is_ascii_alphanumeric() || matches!(char, '_' | '-'))
    }) {
        return format!("$.{}", path.join("."));
    }

    json_pointer(path)
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
    use std::time::{SystemTime, UNIX_EPOCH};

    macro_rules! json {
        ($($json:tt)+) => {
            Value::from_json(serde_json::json!($($json)+))
        };
    }

    fn rule_file(source: &str) -> Result<RuleFile> {
        parse_rules_document(Path::new("rules.pump.yaml"), source)
    }

    fn yaml_value(source: &str) -> Result<Value> {
        Value::parse_yaml_documents(source)?
            .into_iter()
            .next()
            .context("expected a YAML document")
    }

    fn rules() -> RuleFile {
        rule_file(
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

    fn temp_input_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("pump-{name}-{}-{nanos}.json", std::process::id()))
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
        let rule_file = rule_file(
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
        let rule_file = rule_file(
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
    fn inflate_with_traces_records_applied_and_skipped_rules() {
        let rule_file = rule_file(
            r#"
rules:
  - name: deployment-defaults
    match: "$.spec"
    apiVersion: apps/v1
    kind: Deployment
    defaults:
      replicas: 2
  - name: service-defaults
    match: "$.spec"
    apiVersion: v1
    kind: Service
    defaults:
      type: ClusterIP
"#,
        )
        .unwrap();
        let mut docs = vec![json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {"name": "api"},
            "spec": {}
        })];
        let mut provenance = HashMap::new();
        let mut traces = Vec::new();

        inflate_with_traces(&mut docs, &rule_file, &mut provenance, Some(&mut traces)).unwrap();

        assert_eq!(docs[0]["spec"]["replicas"], json!(2));
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].decision, "applied");
        assert_eq!(traces[0].operations[0].path, "/spec/replicas");
        assert_eq!(traces[0].operations[0].outcome, "generated");
        assert_eq!(traces[1].decision, "skipped");
        assert!(traces[1].reason.contains("selector apiVersion expected v1"));

        let query_path = vec!["spec".to_string(), "replicas".to_string()];
        let related = traces_for_doc(&traces, 0, &query_path, false);
        let all = traces_for_doc(&traces, 0, &query_path, true);

        assert_eq!(related.len(), 1);
        assert_eq!(related[0].rule, "deployment-defaults");
        assert_eq!(related[0].operations.len(), 1);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn rule_source_locations_index_operation_keys() {
        let source = r#"rules:
  - name: base
    match: "$.app"
    defaults:
      mode: safe
    overrides:
      tier: prod
    delete:
      - debug
  - name: replace-block
    match: "$.app.legacy"
    replace:
      enabled: false
"#;

        let rule_file = parse_rules_document(Path::new("rules.pump.yaml"), source).unwrap();
        let index = &rule_file.source_locations;

        assert_eq!(
            index.get(&operation_key(0, "default")).unwrap(),
            &SourceLocation {
                file: "rules.pump.yaml".to_string(),
                line: 4,
                column: 5,
                byte_index: 45,
            }
        );
        assert_eq!(index.get(&operation_key(0, "override")).unwrap().line, 6);
        assert_eq!(index.get(&operation_key(0, "delete")).unwrap().line, 8);
        assert_eq!(index.get(&operation_key(1, "replace")).unwrap().line, 12);
        assert_eq!(
            index
                .get(&value_location_key(0, "default", &["mode".to_string()]))
                .unwrap()
                .line,
            5
        );
        assert_eq!(
            index
                .get(&value_location_key(0, "override", &["tier".to_string()]))
                .unwrap()
                .line,
            7
        );
        assert_eq!(
            index
                .get(&value_location_key(0, "delete", &["0".to_string()]))
                .unwrap()
                .line,
            9
        );
        assert_eq!(
            index
                .get(&value_location_key(1, "replace", &["enabled".to_string()]))
                .unwrap()
                .line,
            13
        );
    }

    #[test]
    fn generated_nested_defaults_record_value_locations() {
        let path = temp_input_path("value-location-rules").with_extension("pump.yaml");
        fs::write(
            &path,
            r#"rules:
  - name: nested-defaults
    match: "$.spec"
    defaults:
      template:
        spec:
          securityContext:
            runAsNonRoot: true
"#,
        )
        .unwrap();
        let rule_file = read_rules(&path).unwrap();
        let mut docs = vec![json!({
            "spec": {
                "template": {
                    "spec": {}
                }
            }
        })];
        let mut provenance = HashMap::new();
        let mut traces = Vec::new();

        inflate_with_traces(&mut docs, &rule_file, &mut provenance, Some(&mut traces)).unwrap();

        let entry = provenance
            .get("0:/spec/template/spec/securityContext/runAsNonRoot")
            .unwrap();
        assert_eq!(entry.operation_location.as_ref().unwrap().line, 4);
        assert_eq!(entry.value_location.as_ref().unwrap().line, 8);
        assert_eq!(
            traces[0].operations[0]
                .value_location
                .as_ref()
                .unwrap()
                .line,
            7
        );
        fs::remove_file(path).ok();
    }

    #[test]
    fn value_locations_fall_back_cleanly_without_source_index() {
        let mut rule_file = rule_file(
            r#"
rules:
  - name: default-item-shape
    match: "$.*"
    defaults:
      bottom: z
"#,
        )
        .unwrap();
        rule_file.source_locations.clear();
        let mut docs = vec![json!({"first": {}})];
        let mut provenance = HashMap::new();
        let mut traces = Vec::new();

        inflate_with_traces(&mut docs, &rule_file, &mut provenance, Some(&mut traces)).unwrap();

        assert!(
            provenance
                .get("0:/first/bottom")
                .unwrap()
                .value_location
                .is_none()
        );
        assert!(traces[0].operations[0].value_location.is_none());
    }

    #[test]
    fn overrides_delete_and_replace_apply_in_rule_order() {
        let rule_file = rule_file(
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
        let rule_file = rule_file(
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
    fn strict_check_passes_when_input_is_already_inflated() {
        let mut docs = vec![json!({
            "first": {
                "top": "a",
                "middle": 5,
                "bottom": "z"
            }
        })];
        let rule_file = rule_file(
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
        .unwrap();

        check(
            Path::new("source.json"),
            &rule_file,
            &mut docs,
            true,
            false,
            Some(OutputFormat::Json),
        )
        .unwrap();
    }

    #[test]
    fn strict_check_fails_when_rules_would_change_input() {
        let mut docs = vec![json!({
            "first": {
                "top": "a",
                "middle": 5
            }
        })];

        let err = check(
            Path::new("source.json"),
            &rules(),
            &mut docs,
            true,
            false,
            Some(OutputFormat::Json),
        )
        .unwrap_err();

        assert!(err.to_string().contains("strict check failed"));
    }

    #[test]
    fn write_check_inflates_docs_in_place() {
        let path =
            std::env::temp_dir().join(format!("pump-write-check-{}.json", std::process::id()));
        let mut docs = vec![json!({
            "first": {
                "top": "a",
                "middle": 5
            }
        })];

        check(
            &path,
            &rules(),
            &mut docs,
            false,
            true,
            Some(OutputFormat::Json),
        )
        .unwrap();

        assert_eq!(docs[0]["first"]["bottom"], json!("z"));
        fs::remove_file(path).ok();
    }

    #[test]
    fn yaml_custom_tags_parse_and_render() {
        let path = temp_input_path("custom-tags").with_extension("yaml");
        fs::write(
            &path,
            r#"
flow_authentication: !Find [authentik_flows.flow, [slug, default-authentication-flow]]
fields:
  - !KeyOf prompt-field-username
"#,
        )
        .unwrap();

        let docs = read_input(&path).unwrap();
        let rendered = format_docs(&docs, OutputFormat::Yaml).unwrap();

        assert!(rendered.contains("!Find"));
        assert!(rendered.contains("!KeyOf"));
        fs::remove_file(path).ok();
    }

    #[test]
    fn yaml_multi_document_output_keeps_document_boundaries() {
        let docs = vec![
            json!({"kind": "Deployment", "spec": {"replicas": 2}}),
            json!({"kind": "Service", "spec": {"type": "ClusterIP"}}),
        ];

        let rendered = format_docs(&docs, OutputFormat::Yaml).unwrap();

        assert!(rendered.starts_with("---\nkind: Deployment"));
        assert!(rendered.contains("replicas: 2\n---\nkind: Service"));
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn inflate_preserves_yaml_custom_tags() {
        let rule_file = rule_file(
            r#"
rules:
  - name: blueprint-defaults
    match: "$"
    defaults:
      enabled: true
"#,
        )
        .unwrap();
        let mut docs = vec![
            yaml_value(
                r#"
flow_authentication: !Find [authentik_flows.flow, [slug, default-authentication-flow]]
"#,
            )
            .unwrap(),
        ];
        let mut provenance = HashMap::new();

        inflate(&mut docs, &rule_file, &mut provenance).unwrap();

        let rendered = format_docs(&docs, OutputFormat::Yaml).unwrap();
        assert!(rendered.contains("!Find"));
        assert!(rendered.contains("enabled: true"));
    }

    #[test]
    fn deflate_preserves_yaml_custom_tags() {
        let rule_file = rule_file(
            r#"
rules:
  - name: blueprint-defaults
    match: "$"
    defaults:
      enabled: true
"#,
        )
        .unwrap();
        let mut docs = vec![
            yaml_value(
                r#"
enabled: true
flow_authentication: !Find [authentik_flows.flow, [slug, default-authentication-flow]]
"#,
            )
            .unwrap(),
        ];

        deflate_with_options(&mut docs, &rule_file, &DeflateOptions::default()).unwrap();

        let rendered = format_docs(&docs, OutputFormat::Yaml).unwrap();
        assert!(!rendered.contains("enabled: true"));
        assert!(rendered.contains("!Find"));
    }

    #[test]
    fn multi_input_check_validates_all_files_without_writing() {
        let first = temp_input_path("multi-check-first");
        let second = temp_input_path("multi-check-second");
        fs::write(&first, r#"{"first":{"top":"a","middle":5}}"#).unwrap();
        fs::write(&second, r#"{"second":{"top":"b","middle":5}}"#).unwrap();
        let first_before = fs::read_to_string(&first).unwrap();
        let second_before = fs::read_to_string(&second).unwrap();
        let inputs = vec![first.clone(), second.clone()];

        check_inputs(&inputs, &rules(), false, false, Some(OutputFormat::Json)).unwrap();

        assert_eq!(fs::read_to_string(&first).unwrap(), first_before);
        assert_eq!(fs::read_to_string(&second).unwrap(), second_before);
        fs::remove_file(first).ok();
        fs::remove_file(second).ok();
    }

    #[test]
    fn multi_input_strict_checks_every_file_before_failing() {
        let first = temp_input_path("multi-strict-first");
        let second = temp_input_path("multi-strict-second");
        fs::write(&first, r#"{"first":{"top":"a","middle":5}}"#).unwrap();
        fs::write(&second, r#"{"second":{"top":"b","middle":5}}"#).unwrap();
        let inputs = vec![first.clone(), second.clone()];

        let err = check_inputs(&inputs, &rules(), true, false, Some(OutputFormat::Json))
            .expect_err("strict multi-input check should fail when any input would change");

        assert!(err.to_string().contains("2 of 2 file(s)"));
        fs::remove_file(first).ok();
        fs::remove_file(second).ok();
    }

    #[test]
    fn multi_input_write_inflates_each_file_in_place() {
        let first = temp_input_path("multi-write-first");
        let second = temp_input_path("multi-write-second");
        fs::write(&first, r#"{"first":{"top":"a","middle":5}}"#).unwrap();
        fs::write(&second, r#"{"second":{"top":"b","middle":5}}"#).unwrap();
        let inputs = vec![first.clone(), second.clone()];

        check_inputs(&inputs, &rules(), false, true, Some(OutputFormat::Json)).unwrap();

        let first_doc =
            Value::from_json(serde_json::from_str(&fs::read_to_string(&first).unwrap()).unwrap());
        let second_doc =
            Value::from_json(serde_json::from_str(&fs::read_to_string(&second).unwrap()).unwrap());

        assert_eq!(first_doc["first"]["bottom"], json!("z"));
        assert_eq!(second_doc["second"]["bottom"], json!("z"));
        fs::remove_file(first).ok();
        fs::remove_file(second).ok();
    }

    #[test]
    fn project_config_resolves_paths_relative_to_config_file() {
        let dir = std::env::temp_dir().join(format!(
            "pump-config-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("pump.yaml"),
            r#"
rules: rules.pump.yaml
inputs:
  - src/app.json
outDir: rendered
format: json
"#,
        )
        .unwrap();

        let config = load_project_config(Some(&dir.join("pump.yaml")))
            .unwrap()
            .unwrap();

        assert_eq!(config.rules.unwrap(), dir.join("rules.pump.yaml"));
        assert_eq!(config.inputs, vec![dir.join("src/app.json")]);
        assert_eq!(config.out_dir.unwrap(), dir.join("rendered"));
        assert_eq!(config.format, Some(OutputFormat::Json));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn inflate_inputs_writes_batch_outputs_to_out_dir() {
        let dir = std::env::temp_dir().join(format!(
            "pump-batch-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let out_dir = dir.join("rendered");
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        fs::write(&first, r#"{"first":{"top":"a","middle":5}}"#).unwrap();
        fs::write(&second, r#"{"second":{"top":"b","middle":5}}"#).unwrap();

        inflate_inputs(
            &[first.clone(), second.clone()],
            &rules(),
            None,
            Some(&out_dir),
            Some(OutputFormat::Json),
            None,
        )
        .unwrap();

        let first_doc = Value::from_json(
            serde_json::from_str(&fs::read_to_string(out_dir.join("first.json")).unwrap()).unwrap(),
        );
        let second_doc = Value::from_json(
            serde_json::from_str(&fs::read_to_string(out_dir.join("second.json")).unwrap())
                .unwrap(),
        );

        assert_eq!(first_doc["first"]["bottom"], json!("z"));
        assert_eq!(second_doc["second"]["bottom"], json!("z"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn suggest_rules_finds_repeated_wildcard_defaults() {
        let input = temp_input_path("suggest-source");
        let out = temp_input_path("suggest-rules").with_extension("pump.yaml");
        fs::write(
            &input,
            r#"{
  "api": {"env": "prod", "image": "api:v1"},
  "worker": {"env": "prod", "image": "worker:v1"},
  "scheduler": {"env": "prod", "image": "scheduler:v1"}
}"#,
        )
        .unwrap();

        suggest_rules(std::slice::from_ref(&input), 2, Some(&out), false).unwrap();
        let rule_file = read_rules(&out).unwrap();

        assert!(rule_file.rules.iter().any(|rule| {
            rule.match_path == "$.*"
                && rule
                    .defaults
                    .as_ref()
                    .and_then(|defaults| defaults.get("env"))
                    == Some(&json!("prod"))
        }));
        fs::remove_file(input).ok();
        fs::remove_file(out).ok();
    }

    #[test]
    fn rule_validation_rejects_invalid_match_paths() {
        let err = rule_file(
            r#"
rules:
  - name: bad-path
    match: spec
    defaults:
      enabled: true
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("invalid match path"));
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
