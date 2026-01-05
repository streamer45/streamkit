// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use clap::{Parser, Subcommand};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(author, version, about = "StreamKit client CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Process a pipeline using a remote server (oneshot mode)
    #[command(name = "oneshot")]
    OneShot {
        /// Path to the pipeline YAML file
        pipeline: String,
        /// Input media file path
        input: String,
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
}

#[derive(Subcommand, Debug)]
enum SchemaCommands {
    /// List node schemas (GET /api/v1/schema/nodes)
    Nodes,
    /// List packet schemas (GET /api/v1/schema/packets)
    Packets,
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

#[tokio::main]
async fn main() {
    // Initialize basic logging for client
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::OneShot { pipeline, input, output, server } => {
            info!("Starting StreamKit client - oneshot processing");

            if let Err(e) =
                streamkit_client::process_oneshot(&pipeline, &input, &output, &server).await
            {
                // Error already logged via tracing above
                error!(error = %e, "Failed to process oneshot pipeline");
                std::process::exit(1);
            }
        },
        Commands::Create { pipeline, name, server } => {
            info!("Starting StreamKit client - creating session");

            if let Err(e) = streamkit_client::create_session(&pipeline, &name, &server).await {
                // Error already logged via tracing above
                error!(error = %e, "Failed to create dynamic session");
                std::process::exit(1);
            }
        },
        Commands::Destroy { session_id, server } => {
            info!("Starting StreamKit client - destroying session");

            if let Err(e) = streamkit_client::destroy_session(&session_id, &server).await {
                // Error already logged via tracing above
                error!(error = %e, "Failed to destroy session");
                std::process::exit(1);
            }
        },
        Commands::Tune { session_id, node_id, param, value, server } => {
            info!("Starting StreamKit client - tuning node");

            if let Err(e) =
                streamkit_client::tune_node(&session_id, &node_id, &param, &value, &server).await
            {
                // Error already logged via tracing above
                error!(error = %e, "Failed to tune node");
                std::process::exit(1);
            }
        },
        Commands::List { server } => {
            info!("Starting StreamKit client - listing sessions");

            if let Err(e) = streamkit_client::list_sessions(&server).await {
                // Error already logged via tracing above
                error!(error = %e, "Failed to list sessions");
                std::process::exit(1);
            }
        },
        Commands::Shell { server } => {
            info!("Starting StreamKit client - interactive shell");

            if let Err(e) = streamkit_client::start_shell(&server).await {
                // Error already logged via tracing above
                error!(error = %e, "Failed to start interactive shell");
                std::process::exit(1);
            }
        },
        Commands::LoadTest { config_path, config, server, sessions, duration, cleanup } => {
            info!("Starting StreamKit load test");

            let config = match (config_path, config) {
                (Some(_path), flag) if flag != "loadtest.toml" => {
                    error!(
                        "Provide load test config either as positional CONFIG or via --config, not both"
                    );
                    std::process::exit(2);
                },
                (Some(path), _) => path,
                (None, flag) => flag,
            };

            if let Err(e) =
                streamkit_client::run_load_test(&config, server, sessions, duration, cleanup).await
            {
                // Error already logged via tracing above
                error!(error = %e, "Load test failed");
                std::process::exit(1);
            }
        },
        Commands::Config { server } => {
            if let Err(e) = streamkit_client::get_config(&server).await {
                error!(error = %e, "Failed to fetch server config");
                std::process::exit(1);
            }
        },
        Commands::Permissions { server } => {
            if let Err(e) = streamkit_client::get_permissions(&server).await {
                error!(error = %e, "Failed to fetch permissions");
                std::process::exit(1);
            }
        },
        Commands::Schema { command, server } => {
            let result = match command {
                SchemaCommands::Nodes => streamkit_client::list_node_schemas(&server).await,
                SchemaCommands::Packets => streamkit_client::list_packet_schemas(&server).await,
            };
            if let Err(e) = result {
                error!(error = %e, "Failed to fetch schema");
                std::process::exit(1);
            }
        },
        Commands::Pipeline { session_id, server } => {
            if let Err(e) = streamkit_client::get_pipeline(&session_id, &server).await {
                error!(error = %e, "Failed to fetch pipeline");
                std::process::exit(1);
            }
        },
        Commands::Plugins { command, server } => {
            let result = match command {
                PluginCommands::List => streamkit_client::list_plugins(&server).await,
                PluginCommands::Upload { path } => {
                    streamkit_client::upload_plugin(&path, &server).await
                },
                PluginCommands::Delete { kind, keep_file } => {
                    streamkit_client::delete_plugin(&kind, keep_file, &server).await
                },
            };
            if let Err(e) = result {
                error!(error = %e, "Plugin command failed");
                std::process::exit(1);
            }
        },
        Commands::Samples { command, server } => {
            let result = match command {
                SampleCommands::ListOneshot => {
                    streamkit_client::list_samples_oneshot(&server).await
                },
                SampleCommands::ListDynamic => {
                    streamkit_client::list_samples_dynamic(&server).await
                },
                SampleCommands::Get { id, yaml } => {
                    streamkit_client::get_sample(&id, yaml, &server).await
                },
                SampleCommands::Save { name, description, yaml_path, overwrite, fragment } => {
                    streamkit_client::save_sample(
                        &name,
                        &description,
                        &yaml_path,
                        overwrite,
                        fragment,
                        &server,
                    )
                    .await
                },
                SampleCommands::Delete { id } => {
                    streamkit_client::delete_sample(&id, &server).await
                },
            };
            if let Err(e) = result {
                error!(error = %e, "Sample command failed");
                std::process::exit(1);
            }
        },
        Commands::Assets { command, server } => {
            let result = match command {
                AssetCommands::List => streamkit_client::list_audio_assets(&server).await,
                AssetCommands::Upload { path } => {
                    streamkit_client::upload_audio_asset(&path, &server).await
                },
                AssetCommands::Delete { id } => {
                    streamkit_client::delete_audio_asset(&id, &server).await
                },
            };
            if let Err(e) = result {
                error!(error = %e, "Asset command failed");
                std::process::exit(1);
            }
        },
        Commands::Watch { session, pretty, server } => {
            if let Err(e) =
                streamkit_client::watch_events(session.as_deref(), pretty, &server).await
            {
                error!(error = %e, "Watch failed");
                std::process::exit(1);
            }
        },
        Commands::Control { command, server } => {
            let result = match command {
                ControlCommands::Nodes => streamkit_client::control_list_nodes(&server).await,
                ControlCommands::Pipeline { session_id } => {
                    streamkit_client::control_get_pipeline(&session_id, &server).await
                },
                ControlCommands::AddNode { session_id, node_id, kind, params } => {
                    streamkit_client::control_add_node(
                        &session_id,
                        &node_id,
                        &kind,
                        params.as_deref(),
                        &server,
                    )
                    .await
                },
                ControlCommands::RemoveNode { session_id, node_id } => {
                    streamkit_client::control_remove_node(&session_id, &node_id, &server).await
                },
                ControlCommands::Connect { session_id, from_node, from_pin, to_node, to_pin } => {
                    streamkit_client::control_connect(
                        &session_id,
                        &from_node,
                        &from_pin,
                        &to_node,
                        &to_pin,
                        &server,
                    )
                    .await
                },
                ControlCommands::Disconnect {
                    session_id,
                    from_node,
                    from_pin,
                    to_node,
                    to_pin,
                } => {
                    streamkit_client::control_disconnect(
                        &session_id,
                        &from_node,
                        &from_pin,
                        &to_node,
                        &to_pin,
                        &server,
                    )
                    .await
                },
                ControlCommands::ValidateBatch { session_id, ops_file } => {
                    streamkit_client::control_validate_batch(&session_id, &ops_file, &server).await
                },
                ControlCommands::ApplyBatch { session_id, ops_file } => {
                    streamkit_client::control_apply_batch(&session_id, &ops_file, &server).await
                },
                ControlCommands::TuneAsync { session_id, node_id, param, value } => {
                    streamkit_client::control_tune_async(
                        &session_id,
                        &node_id,
                        &param,
                        &value,
                        &server,
                    )
                    .await
                },
            };
            if let Err(e) = result {
                error!(error = %e, "Control command failed");
                std::process::exit(1);
            }
        },
    }
}
