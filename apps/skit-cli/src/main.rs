// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allow println/eprintln in CLI binary - these are for direct user output, not logging
#![allow(clippy::disallowed_macros)]

use std::fmt::Write;

use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use streamkit_client::client::ValidateResponse;
use streamkit_client::graph::GraphFormat;
use streamkit_client::{exit_codes, CliOutput, Client, InputFile, NetworkClient, OutputFormat};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(author, version, about = "StreamKit client CLI", long_about = None)]
struct Cli {
    /// Output results as JSON instead of human-readable text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone)]
struct FieldPath {
    field: String,
    path: String,
}

fn parse_field_path(s: &str) -> Result<FieldPath, String> {
    let mut parts = s.splitn(2, '=');
    let field = parts.next().unwrap_or("").trim();
    let path = parts.next().unwrap_or("").trim();
    if field.is_empty() || path.is_empty() {
        return Err("expected form name=path".to_string());
    }
    Ok(FieldPath { field: field.to_string(), path: path.to_string() })
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Process a pipeline using a remote server (oneshot mode)
    #[command(name = "oneshot")]
    OneShot {
        /// Path to the pipeline YAML file
        pipeline: String,
        /// Primary input media file path (multipart field defaults to 'media')
        input: String,
        /// Additional input fields in the form name=path (repeatable)
        #[arg(long = "input", value_parser = parse_field_path, action = ArgAction::Append)]
        extra_input: Vec<FieldPath>,
        /// Output file path
        output: String,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Create a new dynamic session with a pipeline configuration
    Create {
        /// Path to the pipeline YAML file
        pipeline: String,
        /// Optional human-readable name for the session
        #[arg(short, long)]
        name: Option<String>,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Destroy a dynamic session and cleanup its resources
    Destroy {
        /// Session ID or name to destroy
        session_id: String,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Tune a node's parameters in a dynamic session
    Tune {
        /// Session ID or name containing the node to tune
        session_id: String,
        /// Node ID to tune
        node_id: String,
        /// Parameter name to update
        param: String,
        /// New parameter value (as YAML)
        value: String,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// List all active dynamic sessions
    List {
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Start an interactive shell session
    Shell {
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Run load test against server (aliases: lt, loadtest, load-test)
    #[command(name = "loadtest", aliases = ["lt", "load-test"])]
    LoadTest {
        /// Path to TOML configuration file (positional)
        #[arg(value_name = "CONFIG")]
        config_path: Option<String>,
        /// Path to TOML configuration file (flag form)
        #[arg(short, long, default_value = "loadtest.toml")]
        config: String,
        /// Override server URL from config
        #[arg(long)]
        server: Option<String>,
        /// Override dynamic.session_count from config
        ///
        /// Useful for quickly scaling down presets like `stress-dynamic` on laptops.
        #[arg(long)]
        sessions: Option<usize>,
        /// Override test duration (seconds)
        #[arg(short, long)]
        duration: Option<u64>,
        /// Clean up all created sessions on exit
        #[arg(long)]
        cleanup: bool,
    },
    /// Show server UI bootstrap config (GET /api/v1/config)
    Config {
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Show permissions for this request (GET /api/v1/permissions)
    Permissions {
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Fetch schemas (GET /api/v1/schema/*)
    Schema {
        #[command(subcommand)]
        command: SchemaCommands,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Inspect a session pipeline (GET /api/v1/sessions/{id}/pipeline)
    Pipeline {
        /// Session ID or name
        session_id: String,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Manage plugins (GET/POST/DELETE /api/v1/plugins)
    Plugins {
        #[command(subcommand)]
        command: PluginCommands,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Manage sample pipelines (GET/POST/DELETE /api/v1/samples/*)
    Samples {
        #[command(subcommand)]
        command: SampleCommands,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Manage audio assets (GET/POST/DELETE /api/v1/assets/audio)
    Assets {
        #[command(subcommand)]
        command: AssetCommands,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Watch WebSocket events (GET /api/v1/control)
    Watch {
        /// Optional session ID or name to filter events
        session: Option<String>,
        /// Pretty-print JSON events
        #[arg(long)]
        pretty: bool,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// WebSocket control-plane operations (GET /api/v1/control)
    Control {
        #[command(subcommand)]
        command: ControlCommands,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Browse available node types from the server's registry
    Nodes {
        #[command(subcommand)]
        command: NodesCommands,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Visualize a pipeline's graph structure (offline, no server needed)
    Graph {
        /// Path to the pipeline YAML file
        pipeline: String,
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: GraphFormat,
    },
    /// Validate a pipeline YAML against the server's node registry
    Validate {
        /// Path to the pipeline YAML file
        pipeline: String,
        /// Server URL (default: http://127.0.0.1:4545)
        #[arg(short, long, default_value = "http://127.0.0.1:4545")]
        server: String,
    },
    /// Generate shell completions
    #[command(name = "completions", hide = true)]
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
    /// Generate man pages
    #[command(name = "mangen", hide = true)]
    ManGen {
        /// Output directory for man pages
        #[arg(default_value = ".")]
        dir: String,
    },
}

#[derive(Subcommand, Debug)]
enum SchemaCommands {
    /// List node schemas (GET /api/v1/schema/nodes)
    Nodes,
    /// List packet schemas (GET /api/v1/schema/packets)
    Packets,
}

#[derive(Subcommand, Debug)]
enum NodesCommands {
    /// List all available node types from the server's registry
    List,
    /// Show detailed info for a single node kind
    Show {
        /// Node kind to inspect (e.g. audio::gain)
        kind: String,
    },
}

#[derive(Subcommand, Debug)]
enum PluginCommands {
    /// List loaded plugins
    List,
    /// Upload a plugin file (native .so/.dylib/.dll or WASM .wasm)
    Upload {
        /// Path to plugin file
        path: String,
    },
    /// Unload a plugin by kind
    Delete {
        /// Plugin kind to delete/unload (e.g. plugin::wasm::gain)
        kind: String,
        /// Keep the plugin file on disk (default: delete file)
        #[arg(long)]
        keep_file: bool,
    },
}

#[derive(Subcommand, Debug)]
enum SampleCommands {
    /// List oneshot samples (GET /api/v1/samples/oneshot)
    ListOneshot,
    /// List dynamic samples (GET /api/v1/samples/dynamic)
    ListDynamic,
    /// Fetch a sample by ID (GET /api/v1/samples/oneshot/{id})
    Get {
        /// Sample ID (may be prefixed, e.g. oneshot/whisper)
        id: String,
        /// Print only the YAML content
        #[arg(long)]
        yaml: bool,
    },
    /// Save a sample (POST /api/v1/samples/oneshot)
    Save {
        /// Sample name (filename stem)
        name: String,
        /// Human-readable description
        description: String,
        /// Path to pipeline YAML file
        yaml_path: String,
        /// Overwrite existing file
        #[arg(long)]
        overwrite: bool,
        /// Store as a fragment (partial pipeline)
        #[arg(long)]
        fragment: bool,
    },
    /// Delete a sample by ID (DELETE /api/v1/samples/oneshot/{id})
    Delete {
        /// Sample ID (must be user/* or legacy)
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum AssetCommands {
    /// List audio assets (GET /api/v1/assets/audio)
    List,
    /// Upload an audio file (POST /api/v1/assets/audio)
    Upload {
        /// Path to audio file
        path: String,
    },
    /// Delete an audio asset (DELETE /api/v1/assets/audio/{id})
    Delete {
        /// Asset ID (filename, including extension)
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum ControlCommands {
    /// List available node types (WS action: listnodes)
    Nodes,
    /// Fetch a session pipeline (WS action: getpipeline)
    Pipeline {
        /// Session ID or name
        session_id: String,
    },
    /// Add a node to a session (WS action: addnode)
    AddNode {
        /// Session ID or name
        session_id: String,
        /// Node ID to add
        node_id: String,
        /// Node kind (e.g. audio::gain)
        kind: String,
        /// Optional params as JSON or YAML (object)
        #[arg(long)]
        params: Option<String>,
    },
    /// Remove a node from a session (WS action: removenode)
    RemoveNode {
        /// Session ID or name
        session_id: String,
        /// Node ID to remove
        node_id: String,
    },
    /// Connect two nodes in a session (WS action: connect)
    Connect {
        /// Session ID or name
        session_id: String,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
    /// Disconnect two nodes in a session (WS action: disconnect)
    Disconnect {
        /// Session ID or name
        session_id: String,
        from_node: String,
        from_pin: String,
        to_node: String,
        to_pin: String,
    },
    /// Validate a batch of operations (WS action: validatebatch)
    ValidateBatch {
        /// Session ID or name
        session_id: String,
        /// Path to YAML/JSON file containing `BatchOperation[]`
        ops_file: String,
    },
    /// Apply a batch of operations (WS action: applybatch)
    ApplyBatch {
        /// Session ID or name
        session_id: String,
        /// Path to YAML/JSON file containing `BatchOperation[]`
        ops_file: String,
    },
    /// Fire-and-forget node tune (WS action: tunenodeasync)
    TuneAsync {
        /// Session ID or name
        session_id: String,
        /// Node ID to tune
        node_id: String,
        /// Parameter name to update
        param: String,
        /// New parameter value (as YAML)
        value: String,
    },
}

fn json_pretty<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|e| format!("{{\"error\": \"Failed to serialize: {e}\"}}"))
}

fn exit_code_for_error(e: &(dyn std::error::Error + Send + Sync)) -> i32 {
    let msg = e.to_string().to_lowercase();
    if msg.contains("connection refused")
        || msg.contains("dns error")
        || msg.contains("connect error")
        || msg.contains("timed out")
        || msg.contains("connection reset")
    {
        exit_codes::CONNECTION_ERROR
    } else {
        exit_codes::GENERAL_ERROR
    }
}

// ---------------------------------------------------------------------------
// Extracted command handlers (keep dispatch under 300 lines)
// ---------------------------------------------------------------------------

async fn dispatch_nodes(command: NodesCommands, server: &str, fmt: OutputFormat) {
    let client = NetworkClient::new(server);
    match command {
        NodesCommands::List => match client.list_node_schemas().await {
            Ok(nodes) => {
                CliOutput::new(fmt, nodes, |nodes| render_nodes_list(nodes)).print();
            },
            Err(e) => {
                error!(error = %e, "Failed to list node types");
                std::process::exit(exit_code_for_error(e.as_ref()));
            },
        },
        NodesCommands::Show { kind } => match client.list_node_schemas().await {
            Ok(nodes) => {
                if let Some(node) = nodes.into_iter().find(|n| n.kind == kind) {
                    CliOutput::new(fmt, node, render_node_detail).print();
                } else {
                    error!("Unknown node kind: {kind}");
                    std::process::exit(exit_codes::GENERAL_ERROR);
                }
            },
            Err(e) => {
                error!(error = %e, "Failed to fetch node schemas");
                std::process::exit(exit_code_for_error(e.as_ref()));
            },
        },
    }
}

fn dispatch_graph(pipeline_path: &str, format: GraphFormat, fmt: OutputFormat) {
    let yaml = match std::fs::read_to_string(pipeline_path) {
        Ok(content) => content,
        Err(e) => {
            error!(error = %e, path = %pipeline_path, "Failed to read pipeline file");
            std::process::exit(exit_codes::GENERAL_ERROR);
        },
    };

    let user_pipeline = match streamkit_api::yaml::parse_yaml(&yaml) {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "Failed to parse pipeline YAML");
            std::process::exit(exit_codes::GENERAL_ERROR);
        },
    };

    let compiled = match streamkit_api::yaml::compile(user_pipeline) {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "Failed to compile pipeline");
            std::process::exit(exit_codes::GENERAL_ERROR);
        },
    };

    if fmt == OutputFormat::Json {
        CliOutput::new(fmt, compiled, move |p| streamkit_client::graph::render_graph(p, format))
            .print();
    } else {
        println!("{}", streamkit_client::graph::render_graph(&compiled, format));
    }
}

async fn dispatch_validate(pipeline_path: &str, server: &str, fmt: OutputFormat) {
    let yaml = match std::fs::read_to_string(pipeline_path) {
        Ok(content) => content,
        Err(e) => {
            error!(error = %e, path = %pipeline_path, "Failed to read pipeline file");
            std::process::exit(exit_codes::GENERAL_ERROR);
        },
    };

    let client = NetworkClient::new(server);
    match client.validate_pipeline(&yaml).await {
        Ok(result) => {
            let is_valid = result.valid;
            CliOutput::new(fmt, result, render_validate_result).print();
            if !is_valid {
                std::process::exit(exit_codes::GENERAL_ERROR);
            }
        },
        Err(e) => {
            error!(error = %e, "Validation request failed");
            std::process::exit(exit_code_for_error(e.as_ref()));
        },
    }
}

// ---------------------------------------------------------------------------
// Text renderers
// ---------------------------------------------------------------------------

fn cardinality_label(cardinality: &serde_json::Value) -> &'static str {
    match cardinality {
        serde_json::Value::String(s) if s == "One" => "required",
        serde_json::Value::String(s) if s == "Broadcast" => "broadcast",
        serde_json::Value::Object(_) => "dynamic",
        _ => "unknown",
    }
}

fn render_nodes_list(nodes: &[streamkit_api::NodeDefinition]) -> String {
    if nodes.is_empty() {
        return "No node types found.".to_string();
    }
    let mut out = String::new();
    let _ = writeln!(out, "{:<16} {:<40} DESCRIPTION", "CATEGORY", "KIND");
    out.push_str(&"\u{2500}".repeat(80));
    out.push('\n');

    let mut sorted = nodes.to_vec();
    sorted.sort_by(|a, b| {
        let cat_a = a.categories.first().map_or("", String::as_str);
        let cat_b = b.categories.first().map_or("", String::as_str);
        cat_a.cmp(cat_b).then_with(|| a.kind.cmp(&b.kind))
    });

    for node in &sorted {
        let category = node.categories.first().map_or("-", String::as_str);
        let desc = node.description.as_deref().unwrap_or("");
        let desc_short = if desc.chars().count() > 40 {
            let truncated: String = desc.chars().take(37).collect();
            format!("{truncated}...")
        } else {
            desc.to_string()
        };
        let _ = writeln!(out, "{category:<16} {:<40} {desc_short}", node.kind);
    }
    out
}

fn render_node_detail(node: &streamkit_api::NodeDefinition) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Kind:       {}", node.kind);
    if !node.categories.is_empty() {
        let _ = writeln!(out, "Categories: {}", node.categories.join(", "));
    }
    if let Some(desc) = &node.description {
        let _ = writeln!(out, "Description: {desc}");
    }

    out.push_str("\nInputs:\n");
    if node.inputs.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for pin in &node.inputs {
            let types =
                pin.accepts_types.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>().join(", ");
            let card_json = serde_json::to_value(&pin.cardinality).unwrap_or_default();
            let card = cardinality_label(&card_json);
            let _ = writeln!(out, "  {:<12} {:<20} ({card})", pin.name, types);
        }
    }

    out.push_str("\nOutputs:\n");
    if node.outputs.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for pin in &node.outputs {
            let ptype = format!("{:?}", pin.produces_type);
            let card_json = serde_json::to_value(&pin.cardinality).unwrap_or_default();
            let card = cardinality_label(&card_json);
            let _ = writeln!(out, "  {:<12} {:<20} ({card})", pin.name, ptype);
        }
    }

    out.push_str("\nParameters:\n");
    let schema = &node.param_schema;
    if let Some(props) =
        schema.get("properties").and_then(|v| v.as_object()).filter(|p| !p.is_empty())
    {
        for (key, prop) in props {
            let type_label = prop.get("type").and_then(|v| v.as_str()).unwrap_or("any");
            let default_val =
                prop.get("default").map(std::string::ToString::to_string).unwrap_or_default();
            if default_val.is_empty() {
                let _ = writeln!(out, "  {key:<20} {type_label}");
            } else {
                let _ = writeln!(out, "  {key:<20} {type_label:<12} (default: {default_val})");
            }
        }
    } else {
        out.push_str("  (none)\n");
    }

    out
}

fn render_validate_result(result: &ValidateResponse) -> String {
    let mut out = String::new();
    if result.valid {
        out.push_str("Pipeline is valid.\n");
    } else {
        out.push_str("Validation errors:\n");
        for err in &result.errors {
            let node_ctx = err.node.as_deref().map_or(String::new(), |n| format!(" (node: {n})"));
            let _ = writeln!(out, "  error: {}{node_ctx}", err.message);
        }
    }
    if !result.warnings.is_empty() {
        out.push_str("Warnings:\n");
        for warn in &result.warnings {
            let node_ctx = warn.node.as_deref().map_or(String::new(), |n| format!(" (node: {n})"));
            let _ = writeln!(out, "  warning: {}{node_ctx}", warn.message);
        }
    }
    out
}

#[tokio::main]
async fn main() {
    // Initialize basic logging for client
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let format = if cli.json { OutputFormat::Json } else { OutputFormat::Text };

    // Dispatch into a heap-allocated future to keep main's stack frame small.
    // The many command variants produce a large async state machine (~660 KiB)
    // which would otherwise overflow Clippy's stack-size threshold.
    Box::pin(dispatch(cli.command, format)).await;
}

// Always called via Box::pin (heap-allocated), so the large async state machine
// never lands on the call stack. Cognitive complexity is inherent to CLI dispatch.
#[allow(clippy::large_stack_frames, clippy::cognitive_complexity, clippy::too_many_lines)]
async fn dispatch(command: Commands, format: OutputFormat) {
    match command {
        Commands::OneShot { pipeline, input, extra_input, output, server } => {
            info!("Starting StreamKit client - oneshot processing");

            let mut inputs = Vec::new();
            inputs.push(InputFile { field: "media".to_string(), path: input, content_type: None });
            for extra in extra_input {
                inputs.push(InputFile { field: extra.field, path: extra.path, content_type: None });
            }

            let client = NetworkClient::new(&server);
            if let Err(e) = client.process_oneshot(&pipeline, &inputs, &output).await {
                error!(error = %e, "Failed to process oneshot pipeline");
                std::process::exit(exit_code_for_error(e.as_ref()));
            }
        },
        Commands::Create { pipeline, name, server } => {
            info!("Starting StreamKit client - creating session");

            eprint!("Creating session... ");
            let client = NetworkClient::new(&server);
            match client.create_session(&pipeline, &name).await {
                Ok(()) => {
                    eprintln!("done.");
                },
                Err(e) => {
                    eprintln!("failed.");
                    error!(error = %e, "Failed to create dynamic session");
                    std::process::exit(exit_code_for_error(e.as_ref()));
                },
            }
        },
        Commands::Destroy { session_id, server } => {
            info!("Starting StreamKit client - destroying session");

            let client = NetworkClient::new(&server);
            if let Err(e) = client.destroy_session(&session_id).await {
                error!(error = %e, "Failed to destroy session");
                std::process::exit(exit_code_for_error(e.as_ref()));
            }
        },
        Commands::Tune { session_id, node_id, param, value, server } => {
            info!("Starting StreamKit client - tuning node");

            let client = NetworkClient::new(&server);
            if let Err(e) = client.tune_node(&session_id, &node_id, &param, &value).await {
                error!(error = %e, "Failed to tune node");
                std::process::exit(exit_code_for_error(e.as_ref()));
            }
        },
        Commands::List { server } => {
            info!("Starting StreamKit client - listing sessions");

            let client = NetworkClient::new(&server);
            match client.list_sessions().await {
                Ok(sessions) => {
                    CliOutput::new(format, sessions, |sessions| {
                        if sessions.is_empty() {
                            return "No active sessions found.".to_string();
                        }
                        let mut out = String::new();
                        let _ = writeln!(out, "Active Sessions:");
                        let _ = writeln!(out, "{:<20} {:<36} STATUS", "NAME", "SESSION ID");
                        let _ = writeln!(out, "{}", "-".repeat(70));
                        for s in sessions {
                            let name = s.name.as_deref().unwrap_or("<unnamed>");
                            let _ = writeln!(out, "{:<20} {:<36} Running", name, s.id);
                        }
                        out
                    })
                    .print();
                },
                Err(e) => {
                    error!(error = %e, "Failed to list sessions");
                    std::process::exit(exit_code_for_error(e.as_ref()));
                },
            }
        },
        Commands::Shell { server } => {
            info!("Starting StreamKit client - interactive shell");

            if let Err(e) = streamkit_client::start_shell(&server).await {
                error!(error = %e, "Failed to start interactive shell");
                std::process::exit(exit_code_for_error(e.as_ref()));
            }
        },
        Commands::LoadTest { config_path, config, server, sessions, duration, cleanup } => {
            info!("Starting StreamKit load test");

            let config = match (config_path, config) {
                (Some(_path), flag) if flag != "loadtest.toml" => {
                    error!(
                        "Provide load test config either as positional CONFIG or via --config, not both"
                    );
                    std::process::exit(exit_codes::USAGE_ERROR);
                },
                (Some(path), _) => path,
                (None, flag) => flag,
            };

            if let Err(e) =
                streamkit_client::run_load_test(&config, server, sessions, duration, cleanup).await
            {
                error!(error = %e, "Load test failed");
                std::process::exit(exit_code_for_error(e.as_ref()));
            }
        },
        Commands::Config { server } => {
            let client = NetworkClient::new(&server);
            match client.get_config().await {
                Ok(config) => {
                    CliOutput::new(format, config, json_pretty).print();
                },
                Err(e) => {
                    error!(error = %e, "Failed to fetch server config");
                    std::process::exit(exit_code_for_error(e.as_ref()));
                },
            }
        },
        Commands::Permissions { server } => {
            let client = NetworkClient::new(&server);
            match client.get_permissions().await {
                Ok(perms) => {
                    CliOutput::new(format, perms, json_pretty).print();
                },
                Err(e) => {
                    error!(error = %e, "Failed to fetch permissions");
                    std::process::exit(exit_code_for_error(e.as_ref()));
                },
            }
        },
        Commands::Schema { command, server } => {
            let client = NetworkClient::new(&server);
            match command {
                SchemaCommands::Nodes => match client.list_node_schemas().await {
                    Ok(nodes) => {
                        CliOutput::new(format, nodes, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Failed to fetch node schemas");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
                SchemaCommands::Packets => match client.list_packet_schemas().await {
                    Ok(packets) => {
                        CliOutput::new(format, packets, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Failed to fetch packet schemas");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
            }
        },
        Commands::Pipeline { session_id, server } => {
            let client = NetworkClient::new(&server);
            match client.get_pipeline(&session_id).await {
                Ok(pipeline) => {
                    CliOutput::new(format, pipeline, json_pretty).print();
                },
                Err(e) => {
                    error!(error = %e, "Failed to fetch pipeline");
                    std::process::exit(exit_code_for_error(e.as_ref()));
                },
            }
        },
        Commands::Plugins { command, server } => {
            let client = NetworkClient::new(&server);
            match command {
                PluginCommands::List => match client.list_plugins().await {
                    Ok(plugins) => {
                        CliOutput::new(format, plugins, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Plugin list failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
                PluginCommands::Upload { path } => {
                    if let Err(e) = client.upload_plugin(&path).await {
                        error!(error = %e, "Plugin upload failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                PluginCommands::Delete { kind, keep_file } => {
                    if let Err(e) = client.delete_plugin(&kind, keep_file).await {
                        error!(error = %e, "Plugin delete failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
            }
        },
        Commands::Samples { command, server } => {
            let client = NetworkClient::new(&server);
            match command {
                SampleCommands::ListOneshot => match client.list_samples_oneshot().await {
                    Ok(samples) => {
                        CliOutput::new(format, samples, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Sample list failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
                SampleCommands::ListDynamic => match client.list_samples_dynamic().await {
                    Ok(samples) => {
                        CliOutput::new(format, samples, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Sample list failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
                SampleCommands::Get { id, yaml } => {
                    if let Err(e) = client.get_sample(&id, yaml).await {
                        error!(error = %e, "Sample get failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                SampleCommands::Save { name, description, yaml_path, overwrite, fragment } => {
                    if let Err(e) = client
                        .save_sample(&name, &description, &yaml_path, overwrite, fragment)
                        .await
                    {
                        error!(error = %e, "Sample save failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                SampleCommands::Delete { id } => {
                    if let Err(e) = client.delete_sample(&id).await {
                        error!(error = %e, "Sample delete failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
            }
        },
        Commands::Assets { command, server } => {
            let client = NetworkClient::new(&server);
            match command {
                AssetCommands::List => match client.list_audio_assets().await {
                    Ok(assets) => {
                        CliOutput::new(format, assets, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Asset list failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
                AssetCommands::Upload { path } => {
                    if let Err(e) = client.upload_audio_asset(&path).await {
                        error!(error = %e, "Asset upload failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                AssetCommands::Delete { id } => {
                    if let Err(e) = client.delete_audio_asset(&id).await {
                        error!(error = %e, "Asset delete failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
            }
        },
        Commands::Watch { session, pretty, server } => {
            let client = NetworkClient::new(&server);
            if let Err(e) = client.watch_events(session.as_deref(), pretty).await {
                error!(error = %e, "Watch failed");
                std::process::exit(exit_code_for_error(e.as_ref()));
            }
        },
        Commands::Control { command, server } => {
            let client = NetworkClient::new(&server);
            match command {
                ControlCommands::Nodes => match client.control_list_nodes().await {
                    Ok(nodes) => {
                        CliOutput::new(format, nodes, json_pretty).print();
                    },
                    Err(e) => {
                        error!(error = %e, "Control nodes failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    },
                },
                ControlCommands::Pipeline { session_id } => {
                    match client.control_get_pipeline(&session_id).await {
                        Ok(pipeline) => {
                            CliOutput::new(format, pipeline, json_pretty).print();
                        },
                        Err(e) => {
                            error!(error = %e, "Control pipeline failed");
                            std::process::exit(exit_code_for_error(e.as_ref()));
                        },
                    }
                },
                ControlCommands::AddNode { session_id, node_id, kind, params } => {
                    if let Err(e) = client
                        .control_add_node(&session_id, &node_id, &kind, params.as_deref())
                        .await
                    {
                        error!(error = %e, "Control add-node failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                ControlCommands::RemoveNode { session_id, node_id } => {
                    if let Err(e) = client.control_remove_node(&session_id, &node_id).await {
                        error!(error = %e, "Control remove-node failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                ControlCommands::Connect { session_id, from_node, from_pin, to_node, to_pin } => {
                    if let Err(e) = client
                        .control_connect(&session_id, &from_node, &from_pin, &to_node, &to_pin)
                        .await
                    {
                        error!(error = %e, "Control connect failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                ControlCommands::Disconnect {
                    session_id,
                    from_node,
                    from_pin,
                    to_node,
                    to_pin,
                } => {
                    if let Err(e) = client
                        .control_disconnect(&session_id, &from_node, &from_pin, &to_node, &to_pin)
                        .await
                    {
                        error!(error = %e, "Control disconnect failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                ControlCommands::ValidateBatch { session_id, ops_file } => {
                    if let Err(e) = client.control_validate_batch(&session_id, &ops_file).await {
                        error!(error = %e, "Control validate-batch failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                ControlCommands::ApplyBatch { session_id, ops_file } => {
                    if let Err(e) = client.control_apply_batch(&session_id, &ops_file).await {
                        error!(error = %e, "Control apply-batch failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
                ControlCommands::TuneAsync { session_id, node_id, param, value } => {
                    if let Err(e) =
                        client.control_tune_async(&session_id, &node_id, &param, &value).await
                    {
                        error!(error = %e, "Control tune-async failed");
                        std::process::exit(exit_code_for_error(e.as_ref()));
                    }
                },
            }
        },
        Commands::Nodes { command, server } => {
            dispatch_nodes(command, &server, format).await;
        },
        Commands::Graph { pipeline, format: graph_format } => {
            dispatch_graph(&pipeline, graph_format, format);
        },
        Commands::Validate { pipeline, server } => {
            dispatch_validate(&pipeline, &server, format).await;
        },
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "skit-cli", &mut std::io::stdout());
        },
        Commands::ManGen { dir } => {
            let cmd = Cli::command();
            let man = clap_mangen::Man::new(cmd);
            let mut buf: Vec<u8> = Vec::new();
            if let Err(e) = man.render(&mut buf) {
                error!(error = %e, "Failed to generate man page");
                std::process::exit(exit_codes::GENERAL_ERROR);
            }
            let out_path = std::path::Path::new(&dir).join("skit-cli.1");
            if let Err(e) = std::fs::write(&out_path, buf) {
                error!(error = %e, path = %out_path.display(), "Failed to write man page");
                std::process::exit(exit_codes::GENERAL_ERROR);
            }
            info!(path = %out_path.display(), "Man page written");
        },
    }
}
