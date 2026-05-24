// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use futures_util::{SinkExt, StreamExt};
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::{MatchingBracketValidator, Validator};
use rustyline::Helper;
use rustyline::{Cmd, CompletionType, Config, EditMode, Editor, KeyEvent};
use std::borrow::Cow::{self, Borrowed, Owned};
use std::collections::HashSet;
use streamkit_api::{MessageType, Request, RequestPayload, Response, ResponsePayload, SessionInfo};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, warn};
use url::Url;

struct SkitHelper {
    completer: SkitCompleter,
    hinter: HistoryHinter,
    validator: MatchingBracketValidator,
}

impl Helper for SkitHelper {}

impl Completer for SkitHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        self.completer.complete(line, pos, ctx)
    }
}

impl Hinter for SkitHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &rustyline::Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Validator for SkitHelper {
    fn validate(
        &self,
        ctx: &mut rustyline::validate::ValidationContext,
    ) -> rustyline::Result<rustyline::validate::ValidationResult> {
        self.validator.validate(ctx)
    }
}

impl Highlighter for SkitHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        let _ = default;
        Borrowed(prompt)
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Owned("\x1b[1m".to_owned() + hint + "\x1b[m")
    }

    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        let _ = pos;
        Borrowed(line)
    }

    fn highlight_char(&self, line: &str, pos: usize, kind: CmdKind) -> bool {
        let _ = (line, pos, kind);
        false
    }
}

struct SkitCompleter {
    sessions: HashSet<String>,
    commands: Vec<String>,
    filename_completer: FilenameCompleter,
}

impl SkitCompleter {
    fn new() -> Self {
        Self {
            sessions: HashSet::new(),
            commands: vec![
                "create".to_string(),
                "destroy".to_string(),
                "tune".to_string(),
                "list".to_string(),
                "watch".to_string(),
                "oneshot".to_string(),
                "loadtest".to_string(),
                "lt".to_string(),
                "help".to_string(),
                "exit".to_string(),
                "quit".to_string(),
            ],
            filename_completer: FilenameCompleter::new(),
        }
    }

    fn update_sessions(&mut self, sessions: Vec<SessionInfo>) {
        self.sessions.clear();
        for session in sessions {
            self.sessions.insert(session.id);
            if let Some(name) = session.name {
                self.sessions.insert(name);
            }
        }
    }
}

impl Completer for SkitCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let words: Vec<&str> = line[..pos].split_whitespace().collect();

        if words.is_empty() || (words.len() == 1 && !line.ends_with(' ')) {
            let start = line.rfind(' ').map_or(0, |i| i + 1);
            let prefix = &line[start..pos];
            let matches: Vec<Pair> = self
                .commands
                .iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .map(|cmd| Pair { display: cmd.clone(), replacement: cmd.clone() })
                .collect();
            Ok((start, matches))
        } else if words.len() == 2 && words[0] == "create" && !line.ends_with(' ') {
            // Complete YAML filenames for pipeline files in create command
            let (start, candidates) = self.filename_completer.complete(line, pos, ctx)?;

            // We already have lowercase strings, so ASCII case-insensitive comparison is unnecessary
            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            let yaml_matches: Vec<Pair> = candidates
                .into_iter()
                .filter(|pair| {
                    let lower = pair.replacement.to_lowercase();
                    !lower.contains('.') || lower.ends_with(".yaml") || lower.ends_with(".yml")
                })
                .collect();

            Ok((start, yaml_matches))
        } else if words[0] == "oneshot" && words.len() <= 3 && !line.ends_with(' ') {
            let (start, candidates) = self.filename_completer.complete(line, pos, ctx)?;

            if words.len() == 2 {
                // We already have lowercase strings, so ASCII case-insensitive comparison is unnecessary
                #[allow(clippy::case_sensitive_file_extension_comparisons)]
                let yaml_matches: Vec<Pair> = candidates
                    .into_iter()
                    .filter(|pair| {
                        let lower = pair.replacement.to_lowercase();
                        !lower.contains('.') || lower.ends_with(".yaml") || lower.ends_with(".yml")
                    })
                    .collect();
                Ok((start, yaml_matches))
            } else {
                Ok((start, candidates))
            }
        } else if (words[0] == "loadtest" || words[0] == "lt")
            && words.len() == 2
            && !line.ends_with(' ')
        {
            let (start, candidates) = self.filename_completer.complete(line, pos, ctx)?;

            // We already have lowercase strings, so ASCII case-insensitive comparison is unnecessary
            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            let toml_matches: Vec<Pair> = candidates
                .into_iter()
                .filter(|pair| {
                    let lower = pair.replacement.to_lowercase();
                    !lower.contains('.') || lower.ends_with(".toml")
                })
                .collect();
            Ok((start, toml_matches))
        } else if words.len() >= 2
            && (words[0] == "destroy" || words[0] == "tune" || words[0] == "watch")
        {
            let start = line.rfind(' ').map_or(0, |i| i + 1);
            let prefix = &line[start..pos];
            let matches: Vec<Pair> = self
                .sessions
                .iter()
                .filter(|session| session.starts_with(prefix))
                .map(|session| Pair { display: session.clone(), replacement: session.clone() })
                .collect();
            Ok((start, matches))
        } else {
            Ok((pos, vec![]))
        }
    }
}

pub struct Shell {
    ws_url: String,
    editor: Editor<SkitHelper, DefaultHistory>,
    current_sessions: Vec<SessionInfo>,
}

impl Shell {
    /// Creates a new Shell instance
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The server URL cannot be parsed
    /// - The URL scheme is not http(s) or ws(s)
    /// - Editor initialization fails
    ///
    /// # Panics
    ///
    /// Panics if URL scheme conversion fails (unreachable in practice as "ws" and "wss" are always valid)
    pub fn new(server_url: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut ws_url = Url::parse(server_url)?;
        match ws_url.scheme() {
            // Hardcoded valid schemes; set_scheme cannot fail.
            #[allow(clippy::expect_used)]
            "http" => {
                ws_url.set_scheme("ws").expect("ws is a valid URL scheme");
            },
            #[allow(clippy::expect_used)]
            "https" => {
                ws_url.set_scheme("wss").expect("wss is a valid URL scheme");
            },
            "ws" | "wss" => (),
            _ => return Err("Server URL must be http(s) or ws(s)".into()),
        }
        ws_url.set_path("/api/v1/control");

        let config = Config::builder()
            .history_ignore_space(true)
            .completion_type(CompletionType::List)
            .edit_mode(EditMode::Emacs)
            .build();

        let helper = SkitHelper {
            completer: SkitCompleter::new(),
            hinter: HistoryHinter::new(),
            validator: MatchingBracketValidator::new(),
        };

        let mut editor = Editor::with_config(config)?;
        editor.set_helper(Some(helper));
        editor.bind_sequence(KeyEvent::alt('n'), Cmd::HistorySearchForward);
        editor.bind_sequence(KeyEvent::alt('p'), Cmd::HistorySearchBackward);

        if editor.load_history(".skit_history").is_err() {
            debug!("No previous history found");
        }

        Ok(Self { ws_url: ws_url.to_string(), editor, current_sessions: Vec::new() })
    }

    /// Runs the interactive shell loop
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - WebSocket connection to the server fails
    /// - History file operations fail
    /// - Terminal interaction errors occur
    pub async fn run(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ws_url = &self.ws_url;
        println!("🚀 StreamKit Interactive Shell");
        println!("Connected to: {ws_url}");
        println!("Type 'help' for available commands, 'exit' to quit");
        println!();

        if let Err(e) = self.refresh_sessions().await {
            warn!("Failed to load initial session list: {e}");
        }

        loop {
            let prompt = if self.current_sessions.is_empty() {
                "skit> ".to_string()
            } else {
                format!("skit ({} sessions)> ", self.current_sessions.len())
            };

            let readline = self.editor.readline(&prompt);
            match readline {
                Ok(line) => {
                    let line = line.trim();
                    if !line.is_empty() {
                        self.editor.add_history_entry(line)?;

                        if let Err(e) = self.handle_command(line).await {
                            eprintln!("Error: {e}");
                        }
                    }
                },
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    // Continue to next prompt
                },
                Err(ReadlineError::Eof) => {
                    println!("Goodbye!");
                    break;
                },
                Err(err) => {
                    eprintln!("Error: {err:?}");
                    break;
                },
            }
        }

        if let Err(e) = self.editor.save_history(".skit_history") {
            warn!("Failed to save history: {e}");
        }

        Ok(())
    }

    async fn handle_command(
        &mut self,
        line: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0] {
            "help" | "h" => Self::show_help(),
            "exit" | "quit" | "q" => std::process::exit(0),
            "list" | "ls" => self.list_sessions().await?,
            "create" => self.create_session(&parts[1..]).await?,
            "destroy" | "rm" => self.destroy_session(&parts[1..]).await?,
            "tune" => self.tune_node(&parts[1..]).await?,
            "watch" => self.watch_session(&parts[1..]).await?,
            "oneshot" => self.oneshot(&parts[1..]).await?,
            "loadtest" | "lt" => self.loadtest(&parts[1..]).await?,
            cmd => {
                eprintln!("Unknown command: {cmd}. Type 'help' for available commands.");
            },
        }

        Ok(())
    }

    fn show_help() {
        println!("Available commands:");
        println!();
        println!("Dynamic Sessions:");
        println!("  list, ls                                List all active sessions");
        println!("  create <pipeline.yaml> [--name <name>]  Create a new dynamic session");
        println!("  destroy <session>, rm <session>         Destroy a session");
        println!("  tune <session> <node> <param> <value>   Tune a node parameter");
        println!("  watch <session>                         Watch events for a session");
        println!();
        println!("One-Shot Processing:");
        println!("  oneshot <pipeline.yaml> <input> <output>  Process file through pipeline");
        println!();
        println!("Load Testing:");
        println!("  loadtest <config.toml> [flags]          Run load test with config");
        println!("  lt <config.toml> [flags]                Alias for loadtest");
        println!("    --server <url>                        Override server URL");
        println!("    --duration <seconds>                  Override test duration");
        println!("    --cleanup                             Cleanup sessions after test");
        println!();
        println!("General:");
        println!("  help, h                                 Show this help message");
        println!("  exit, quit, q                           Exit the shell");
        println!();
        println!("Tab completion is available for commands, session names, and file paths.");
    }

    async fn refresh_sessions(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sessions = self.fetch_sessions().await?;

        if let Some(helper) = self.editor.helper_mut() {
            helper.completer.update_sessions(sessions.clone());
        }

        self.current_sessions.clone_from(&sessions);

        Ok(())
    }

    async fn list_sessions(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sessions = self.fetch_sessions().await?;

        if sessions.is_empty() {
            println!("No active sessions found.");
        } else {
            println!("Active Sessions:");
            println!("{:<20} {:<36} STATUS", "NAME", "SESSION ID");
            println!("{}", "-".repeat(70));

            for session in &sessions {
                let name = session.name.as_deref().unwrap_or("<unnamed>");
                println!("{:<20} {:<36} Running", name, session.id);
            }
        }

        if let Some(helper) = self.editor.helper_mut() {
            helper.completer.update_sessions(sessions.clone());
        }

        self.current_sessions.clone_from(&sessions);

        Ok(())
    }

    async fn fetch_sessions(
        &self,
    ) -> Result<Vec<SessionInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let (mut ws_stream, _) = connect_async(&self.ws_url).await?;

        let list_sessions_req = Request {
            message_type: MessageType::Request,
            correlation_id: Some(uuid::Uuid::new_v4().to_string()),
            payload: RequestPayload::ListSessions,
        };
        let req_json = serde_json::to_string(&list_sessions_req)?;
        ws_stream.send(Message::Text(req_json.into())).await?;

        let sessions = loop {
            match ws_stream.next().await {
                Some(Ok(Message::Text(res_text))) => {
                    let v: serde_json::Value = serde_json::from_str(&res_text)?;
                    if v.get("type").and_then(|t| t.as_str()) == Some("response") {
                        let response: Response = serde_json::from_str(&res_text)?;
                        match response.payload {
                            ResponsePayload::SessionsListed { sessions } => break sessions,
                            ResponsePayload::Error { message } => {
                                ws_stream.close(None).await?;
                                return Err(message.into());
                            },
                            _ => {
                                ws_stream.close(None).await?;
                                return Err("Unexpected response from server".into());
                            },
                        }
                    }
                },
                Some(Ok(_)) => {},
                Some(Err(e)) => {
                    ws_stream.close(None).await?;
                    return Err(e.into());
                },
                None => {
                    ws_stream.close(None).await?;
                    return Err("Did not receive list sessions response".into());
                },
            }
        };

        ws_stream.close(None).await?;
        Ok(sessions)
    }

    async fn create_session(
        &mut self,
        args: &[&str],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if args.is_empty() {
            eprintln!("Usage: create <pipeline.yaml> [--name <name>]");
            return Ok(());
        }

        let pipeline_path = args[0];
        let mut name = None;

        let mut i = 1;
        while i < args.len() {
            match args[i] {
                "--name" => {
                    if i + 1 < args.len() {
                        name = Some(args[i + 1].to_string());
                        i += 2;
                    } else {
                        eprintln!("--name requires a value");
                        return Ok(());
                    }
                },
                arg => {
                    eprintln!("Unknown argument: {arg}");
                    return Ok(());
                },
            }
        }

        let http_url = self
            .ws_url
            .replace("ws://", "http://")
            .replace("wss://", "https://")
            .replace("/api/v1/control", "");

        crate::client::create_session(pipeline_path, &name, &http_url).await?;
        self.refresh_sessions().await?;

        Ok(())
    }

    async fn destroy_session(
        &mut self,
        args: &[&str],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if args.is_empty() {
            eprintln!("Usage: destroy <session_id_or_name>");
            return Ok(());
        }

        let session_id = args[0];

        crate::client::destroy_session(session_id, &self.ws_url.replace("/api/v1/control", ""))
            .await?;

        println!("✅ Session '{session_id}' destroyed successfully");

        // Refresh sessions after destruction
        self.refresh_sessions().await?;

        Ok(())
    }

    async fn tune_node(
        &self,
        args: &[&str],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if args.len() < 4 {
            eprintln!("Usage: tune <session> <node> <param> <value>");
            return Ok(());
        }

        let session_id = args[0];
        let node_id = args[1];
        let param = args[2];
        let value = args[3];

        // Use the existing tune_node function from client.rs
        crate::client::tune_node(
            session_id,
            node_id,
            param,
            value,
            &self.ws_url.replace("/api/v1/control", ""),
        )
        .await?;

        Ok(())
    }

    async fn watch_session(
        &self,
        args: &[&str],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session_filter = args.first().copied();
        if session_filter.is_none() {
            eprintln!("Usage: watch <session_id_or_name>");
            return Ok(());
        }

        // Reuse the shared watch implementation (prints JSON events; Ctrl-C to stop).
        crate::client::watch_events(session_filter, false, &self.ws_url).await
    }

    async fn oneshot(&self, args: &[&str]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if args.len() < 3 {
            eprintln!("Usage: oneshot <pipeline.yaml> <input> <output>");
            return Ok(());
        }

        let pipeline_path = args[0];
        let input_path = args[1];
        let output_path = args[2];

        println!("🚀 Processing oneshot pipeline: {input_path} → {pipeline_path} → {output_path}");

        let http_url = self
            .ws_url
            .replace("ws://", "http://")
            .replace("wss://", "https://")
            .replace("/api/v1/control", "");

        let inputs = vec![crate::client::InputFile {
            field: "media".to_string(),
            path: input_path.to_string(),
            content_type: None,
        }];
        crate::client::process_oneshot(pipeline_path, &inputs, output_path, &http_url).await?;

        println!("✅ Oneshot processing completed successfully");

        Ok(())
    }

    async fn loadtest(
        &self,
        args: &[&str],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if args.is_empty() {
            eprintln!(
                "Usage: loadtest <config.toml> [--server <url>] [--sessions <n>] [--duration <seconds>] [--cleanup]"
            );
            eprintln!("Example: loadtest samples/loadtest/stress-moq-peer.toml --duration 30");
            return Ok(());
        }

        let config_path = args[0];

        // Check if file exists and provide helpful error
        if !std::path::Path::new(config_path).exists() {
            let cwd = std::env::current_dir()?;
            eprintln!("Error: Config file not found: {config_path}");
            eprintln!("Current directory: {}", cwd.display());
            eprintln!("Hint: Use a path relative to: {}", cwd.display());
            return Ok(());
        }

        let mut server_override = None;
        let mut sessions_override = None;
        let mut duration_override = None;
        let mut cleanup = false;

        // Parse optional flags
        let mut i = 1;
        while i < args.len() {
            match args[i] {
                "--server" => {
                    if i + 1 < args.len() {
                        server_override = Some(args[i + 1].to_string());
                        i += 2;
                    } else {
                        eprintln!("--server requires a URL value");
                        return Ok(());
                    }
                },
                "--duration" => {
                    if i + 1 < args.len() {
                        if let Ok(duration) = args[i + 1].parse::<u64>() {
                            duration_override = Some(duration);
                            i += 2;
                        } else {
                            eprintln!("--duration requires a numeric value (seconds)");
                            return Ok(());
                        }
                    } else {
                        eprintln!("--duration requires a value");
                        return Ok(());
                    }
                },
                "--sessions" => {
                    if i + 1 < args.len() {
                        if let Ok(sessions) = args[i + 1].parse::<usize>() {
                            sessions_override = Some(sessions);
                            i += 2;
                        } else {
                            eprintln!("--sessions requires a numeric value");
                            return Ok(());
                        }
                    } else {
                        eprintln!("--sessions requires a value");
                        return Ok(());
                    }
                },
                "--cleanup" => {
                    cleanup = true;
                    i += 1;
                },
                flag => {
                    eprintln!("Unknown flag: {flag}");
                    return Ok(());
                },
            }
        }

        println!("🚀 Starting load test from config: {config_path}");

        // Use the run_load_test function from load_test module
        match crate::load_test::run_load_test(
            config_path,
            server_override,
            sessions_override,
            duration_override,
            cleanup,
        )
        .await
        {
            Ok(()) => {
                println!("✅ Load test completed successfully");
                Ok(())
            },
            Err(e) => {
                eprintln!("❌ Load test failed: {e}");
                // Print the full error chain for better debugging
                let mut source = e.source();
                while let Some(err) = source {
                    eprintln!("  Caused by: {err}");
                    source = err.source();
                }
                Ok(())
            },
        }
    }
}
