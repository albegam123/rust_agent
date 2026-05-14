use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use ragent_core::agent::Agent;
use ragent_core::config_loader;
use ragent_core::context::Context;
use ragent_core::skill_loader::SkillLoader;
use ragent_core::GetSkillTool;
use ragent_core::SessionLog;
use ragent_types::config::SkillsConfig;
use ragent_types::event::AgentEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

mod tui_input;
use tui_input::TuiInput;

#[derive(Parser)]
#[command(name = "ragent", version, about = "A Rust LLM agent")]
struct Cli {
    /// Config file path (default: auto-detect)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Run a single prompt non-interactively
    #[arg(short, long)]
    prompt: Option<String>,

    /// Working directory
    #[arg(short, long)]
    workdir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = config_loader::load_config(cli.config.as_deref())?;

    init_tracing(&config.logging.level);

    info!(
        provider = %config.llm.provider,
        model = %config.llm.model,
        "ragent starting"
    );

    if let Some(dir) = cli.workdir.as_ref().or(config.agent.workspace_dir.as_ref()) {
        std::env::set_current_dir(dir)?;
    }

    let provider = ragent_llm::create_provider(&config.llm)?;
    let mut tools = ragent_tools::create_tools(&config.tools);
    let mut system_prompt =
        config_loader::load_system_prompt(config.agent.system_prompt_path.as_deref())?;

    if config.skills.enabled {
        let skills_dir = resolve_skills_dir(&config.skills)?;
        if skills_dir.exists() {
            let loader = Arc::new(SkillLoader::new(skills_dir));
            let skills = loader.discover()?;
            let block = SkillLoader::skills_metadata_prompt(&skills);
            system_prompt = system_prompt.replace("{SKILLS_METADATA}", &block);
            if !skills.is_empty() {
                info!(
                    count = skills.len(),
                    "skills available; get_skill tool registered"
                );
                tools.push(Box::new(GetSkillTool::new(loader)));
            }
        } else {
            warn!(
                dir = %skills_dir.display(),
                "skills enabled but directory does not exist"
            );
            system_prompt = system_prompt.replace("{SKILLS_METADATA}", "");
        }
    } else {
        system_prompt = system_prompt.replace("{SKILLS_METADATA}", "");
    }

    let tool_names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    info!(tools = ?tool_names, "tools registered");

    let session_log = Arc::new(SessionLog::from_config(
        config.logging.session_log,
        config.logging.log_dir.as_ref(),
    )?);

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let agent = Agent::new(provider, tools, config.agent.clone(), event_tx, session_log);
    let mut context = Context::new(system_prompt);

    if let Some(prompt) = cli.prompt {
        run_single(&agent, &mut context, event_rx, &prompt).await
    } else {
        run_interactive(&agent, &mut context, event_rx).await
    }
}

async fn run_single(
    agent: &Agent,
    context: &mut Context,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    prompt: &str,
) -> Result<()> {
    let cancel = CancellationToken::new();
    let event_handle = tokio::spawn(print_events(event_rx));

    let result = agent.run(prompt, context, cancel).await?;

    if let Some(content) = result {
        println!("\n{content}");
    }

    event_handle.abort();
    Ok(())
}

async fn run_interactive(
    agent: &Agent,
    context: &mut Context,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<()> {
    println!(
        "ragent v{} — type /help for commands, /quit to exit",
        env!("CARGO_PKG_VERSION")
    );
    println!();

    // Initialize TUI input handler
    let mut tui = TuiInput::new();

    // Try to enable TUI mode
    match tui.enable_raw_mode() {
        Ok(_) => {
            let result = run_interactive_tui(&mut tui, agent, context, event_rx).await;
            let _ = tui.disable_raw_mode();
            result
        }
        Err(e) => {
            eprintln!("Warning: Failed to enable TUI mode: {}. Falling back to basic mode.", e);
            run_interactive_basic(agent, context, event_rx).await
        }
    }
}

async fn run_interactive_tui(
    tui: &mut TuiInput,
    agent: &Agent,
    context: &mut Context,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<()> {
    loop {
        // Read input with full line editing support
        match tui.read_line(">>> ") {
            Ok(Some(input)) => {
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }

                // Check for commands
                match input {
                    "/quit" | "/exit" | "/q" => break,
                    "/help" | "/h" => {
                        print_help();
                        continue;
                    }
                    "/clear" => {
                        context.clear();
                        tui.clear_history();
                        println!("conversation cleared.");
                        continue;
                    }
                    "/history" | "/hist" => {
                        let history = tui.get_history();
                        if history.is_empty() {
                            println!("No command history.");
                        } else {
                            println!("Command history:");
                            for (i, cmd) in history.iter().enumerate() {
                                println!("  {}: {}", i + 1, cmd);
                            }
                        }
                        continue;
                    }
                    _ => {}
                }

                let cancel = CancellationToken::new();
                let result = agent.run(input, context, cancel).await;

                // Process pending events
                while let Ok(event) = event_rx.try_recv() {
                    handle_event(&event);
                }

                match result {
                    Ok(Some(content)) => {
                        println!("\n{content}\n");
                    }
                    Ok(None) => {
                        println!("\n(no response)\n");
                    }
                    Err(e) => {
                        eprintln!("\nerror: {e}\n");
                    }
                }
            }
            Ok(None) => {
                // Ctrl+C or Ctrl+D
                break;
            }
            Err(e) => {
                eprintln!("\nInput error: {e}\n");
                break;
            }
        }
    }

    println!("goodbye!");
    Ok(())
}

async fn run_interactive_basic(
    agent: &Agent,
    context: &mut Context,
    mut event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!(">>> ");
        stdout.flush()?;

        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        match input {
            "/quit" | "/exit" | "/q" => break,
            "/help" | "/h" => {
                print_help();
                continue;
            }
            "/clear" => {
                context.clear();
                println!("conversation cleared.");
                continue;
            }
            _ => {}
        }

        let cancel = CancellationToken::new();
        let result = agent.run(input, context, cancel).await;

        while let Ok(event) = event_rx.try_recv() {
            handle_event(&event);
        }

        match result {
            Ok(Some(content)) => {
                println!("\n{content}\n");
            }
            Ok(None) => {
                println!("\n(no response)\n");
            }
            Err(e) => {
                eprintln!("\nerror: {e}\n");
            }
        }
    }

    println!("goodbye!");
    Ok(())
}

async fn print_events(mut rx: mpsc::UnboundedReceiver<AgentEvent>) {
    while let Some(event) = rx.recv().await {
        handle_event(&event);
    }
}

fn handle_event(event: &AgentEvent) {
    match event {
        AgentEvent::TurnStarted { step } => {
            if *step > 0 {
                eprintln!("  [step {}]", step + 1);
            }
        }
        AgentEvent::Thinking { content } => {
            eprintln!("  💭 {}", truncate(content, 80));
        }
        AgentEvent::ToolCallStart { name, .. } => {
            eprint!("  🔧 {name}...");
        }
        AgentEvent::ToolCallComplete { name, result, .. } => {
            if result.success {
                eprintln!(" ✓");
            } else {
                eprintln!(" ✗ {}", result.error.as_deref().unwrap_or("error"));
            }
            let _ = name;
        }
        AgentEvent::ContextSummarized {
            original_messages,
            new_messages,
        } => {
            eprintln!("  📝 context summarized: {original_messages} → {new_messages} messages");
        }
        AgentEvent::Error { message } => {
            eprintln!("  ❌ {message}");
        }
        AgentEvent::Cancelled => {
            eprintln!("  ⛔ cancelled");
        }
        AgentEvent::TurnComplete { .. } | AgentEvent::Response { .. } => {}
    }
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let end = s.char_indices().nth(max_len).map_or(s.len(), |(i, _)| i);
        &s[..end]
    }
}

fn print_help() {
    println!(
        "\
Commands:
  /help, /h     Show this help
  /clear        Clear conversation history
  /quit, /q     Exit

Just type your message and press Enter to chat."
    );
}

fn resolve_skills_dir(skills: &SkillsConfig) -> Result<PathBuf> {
    let path = skills
        .skills_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("skills"));
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
