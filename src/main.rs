use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_yml::{
    Value,
    de::{Event, Progress},
    loader::Loader,
};
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

        #[arg(
            long,
            help = "Include the full rule trace, not just related operations"
        )]
        all: bool,
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
        #[arg(
            value_name = "INPUT",
            required = true,
            help = "Input JSON/YAML file(s)"
        )]
        inputs: Vec<PathBuf>,

        #[arg(short, long, help = "Pump rule file")]
        rules: PathBuf,

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

    #[serde(skip)]
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
            all,
        } => {
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
        } => {
            let mut docs = read_input(&input)?;
            let rule_file = read_rules(&rules)?;
            let before = format_docs(&docs, output_format(&input, None, format))?;
            let mut provenance = HashMap::new();
            inflate(&mut docs, &rule_file, &mut provenance)?;
            let after = format_docs(&docs, output_format(&input, None, format))?;
            print_diff(&before, &after, "inflated");
        }
        Commands::Check {
            inputs,
            rules,
            strict,
            write,
            format,
        } => {
            let rule_file = read_rules(&rules)?;
            check_inputs(&inputs, &rule_file, strict, write, format)?;
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
    let mut rules: RuleFile = serde_yml::from_str(&input)
        .with_context(|| format!("failed to parse rules {}", path.display()))?;

    if rules.rules.is_empty() {
        bail!("{} did not define any rules", path.display());
    }

    rules.source_locations = rule_source_locations(path, &input)
        .with_context(|| format!("failed to index rule source locations {}", path.display()))?;

    validate_rules(&rules)?;

    Ok(rules)
}

fn rule_source_locations(path: &Path, input: &str) -> Result<RuleSourceIndex> {
    let mut loader = Loader::new(Progress::Str(input))?;
    let Some(document) = loader.next_document() else {
        return Ok(HashMap::new());
    };

    if let Some(error) = document.error {
        bail!("failed to parse rule source locations: {}", error);
    }

    let mut index = HashMap::new();
    let mut position = 0;
    collect_rule_source_locations(
        &document.events,
        &mut position,
        Vec::new(),
        path,
        &mut index,
    )?;

    Ok(index)
}

fn collect_rule_source_locations(
    events: &[(Event<'_>, serde_yml::libyml::error::Mark)],
    position: &mut usize,
    path: Vec<String>,
    file: &Path,
    index: &mut RuleSourceIndex,
) -> Result<()> {
    let Some((event, _mark)) = events.get(*position) else {
        return Ok(());
    };

    match event {
        Event::MappingStart(_) => {
            *position += 1;
            while let Some((event, mark)) = events.get(*position) {
                if matches!(event, Event::MappingEnd) {
                    *position += 1;
                    break;
                }

                let Some(key) = scalar_event_text(event) else {
                    bail!("expected scalar mapping key while indexing rule locations");
                };
                *position += 1;

                let mut child_path = path.clone();
                child_path.push(key.clone());
                record_rule_source_location(&child_path, file, *mark, index);
                collect_rule_source_locations(events, position, child_path, file, index)?;
            }
        }
        Event::SequenceStart(_) => {
            *position += 1;
            let mut item_index = 0;
            while let Some((event, mark)) = events.get(*position) {
                if matches!(event, Event::SequenceEnd) {
                    *position += 1;
                    break;
                }

                let mut child_path = path.clone();
                child_path.push(item_index.to_string());
                record_rule_source_location(&child_path, file, *mark, index);
                collect_rule_source_locations(events, position, child_path, file, index)?;
                item_index += 1;
            }
        }
        Event::Alias(_) | Event::Scalar(_) | Event::Void => {
            *position += 1;
        }
        Event::MappingEnd | Event::SequenceEnd => {}
    }

    Ok(())
}

fn record_rule_source_location(
    path: &[String],
    file: &Path,
    mark: serde_yml::libyml::error::Mark,
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
            line: mark.line() + 1,
            column: mark.column() + 1,
            byte_index: mark.index(),
        },
    );
}

fn scalar_event_text(event: &Event<'_>) -> Option<String> {
    let Event::Scalar(scalar) = event else {
        return None;
    };

    Some(String::from_utf8_lossy(&scalar.value).into_owned())
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
        let Some(key) = key_value.as_str() else {
            continue;
        };
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
                let Some(key) = key_value.as_str() else {
                    continue;
                };
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
        let Some(key) = key_value.as_str() else {
            continue;
        };
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
        target_map.shift_remove(key);
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
            let Some(child_key) = child_key.as_str() else {
                continue;
            };
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
                    let Some(key) = key.as_str() else {
                        continue;
                    };
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
        value if value.is_mapping() || value.is_sequence() => {
            serde_json::to_string(value).unwrap_or_default()
        }
        _ => format_yaml_inline(value),
    }
}

fn format_yaml_inline(value: &Value) -> String {
    serde_yml::to_string(value)
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
            serde_yml::to_value(serde_json::json!($($json)+)).unwrap()
        };
    }

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
    fn inflate_with_traces_records_applied_and_skipped_rules() {
        let rule_file: RuleFile = serde_yml::from_str(
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

        let index = rule_source_locations(Path::new("rules.pump.yaml"), source).unwrap();

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
        let rule_file: RuleFile = serde_yml::from_str(
            r#"
rules:
  - name: default-item-shape
    match: "$.*"
    defaults:
      bottom: z
"#,
        )
        .unwrap();
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
    fn strict_check_passes_when_input_is_already_inflated() {
        let mut docs = vec![json!({
            "first": {
                "top": "a",
                "middle": 5,
                "bottom": "z"
            }
        })];
        let rule_file: RuleFile = serde_yml::from_str(
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
    fn inflate_preserves_yaml_custom_tags() {
        let rule_file: RuleFile = serde_yml::from_str(
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
            serde_yml::from_str(
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
        let rule_file: RuleFile = serde_yml::from_str(
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
            serde_yml::from_str(
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

        let first_doc: Value = serde_json::from_str(&fs::read_to_string(&first).unwrap()).unwrap();
        let second_doc: Value =
            serde_json::from_str(&fs::read_to_string(&second).unwrap()).unwrap();

        assert_eq!(first_doc["first"]["bottom"], json!("z"));
        assert_eq!(second_doc["second"]["bottom"], json!("z"));
        fs::remove_file(first).ok();
        fs::remove_file(second).ok();
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
