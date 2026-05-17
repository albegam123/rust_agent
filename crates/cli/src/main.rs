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
use crossterm::{
    style::{Color, ResetColor, SetForegroundColor},
    terminal,
};
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

    let hosted = config.tools.hosted_web_search_tool_specs();

    let use_native_ws = matches!(config.llm.provider.as_str(), "openai") && !hosted.is_empty();
    let provider = ragent_llm::create_provider(&config.llm, use_native_ws)?;
    let mut tools = ragent_tools::create_tools(&config.tools);
    if config.tools.enable_mcp {
        match ragent_tools::mcp::load_mcp_tools(&config.tools).await {
            Ok(mcp_tools) => tools.extend(mcp_tools),
            Err(error) => warn!("failed to load MCP tools: {error:#}"),
        }
    }
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

    let tool_names: Vec<String> = tools
        .iter()
        .filter_map(|t| t.spec().callable_name().map(ToString::to_string))
        .collect();
    info!(tools = ?tool_names, "executable tools registered");

    let session_log = Arc::new(SessionLog::from_config(
        config.logging.session_log,
        config.logging.log_dir.as_ref(),
    )?);

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let agent = Agent::new(
        provider,
        tools,
        hosted,
        config.agent.clone(),
        event_tx,
        session_log,
    );
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
        print_assistant_response(&content);
    }

    event_handle.abort();
    Ok(())
}

async fn run_interactive(
    agent: &Agent,
    context: &mut Context,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> Result<()> {
    println!(
        "ragent v{} — type /help for commands, /quit to exit",
        env!("CARGO_PKG_VERSION")
    );
    println!();

    // Initialize TUI input handler
    let mut tui = TuiInput::new();

    // Try to enable TUI mode once to ensure the terminal supports raw input.
    // Raw mode is then enabled only while reading a line and disabled before
    // model/tool output, because normal println!/eprintln! rendering requires
    // cooked terminal newlines (CRLF behavior). Keeping raw mode during output
    // causes the diagonal/staircase logs shown in some terminals.
    match tui.enable_raw_mode() {
        Ok(_) => {
            let _ = tui.disable_raw_mode();
            run_interactive_tui(&mut tui, agent, context, event_rx).await
        }
        Err(e) => {
            eprintln!(
                "Warning: Failed to enable TUI mode: {}. Falling back to basic mode.",
                e
            );
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
        // Enable raw mode only for line editing. Disable it immediately before
        // rendering commands, model/tool events, and assistant panels.
        if let Err(e) = tui.enable_raw_mode() {
            eprintln!(
                "Warning: Failed to re-enable TUI mode: {}. Falling back to basic mode.",
                e
            );
            return run_interactive_basic(agent, context, event_rx).await;
        }

        let line = tui.read_line(">>> ");
        let _ = tui.disable_raw_mode();

        match line {
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
                        print_assistant_response(&content);
                    }
                    Ok(None) => {
                        print_info_panel("no response", "The model returned no final text.");
                    }
                    Err(e) => {
                        print_error_panel(&format!("{e}"));
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
                print_assistant_response(&content);
            }
            Ok(None) => {
                print_info_panel("no response", "The model returned no final text.");
            }
            Err(e) => {
                print_error_panel(&format!("{e}"));
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
                print_event_line("↻", Color::DarkGrey, &format!("step {}", step + 1));
            }
        }
        AgentEvent::Thinking { content } => {
            print_event_line("💭", Color::Magenta, truncate(content, 80));
        }
        AgentEvent::ToolCallStart { name, .. } => {
            print_event_line("🔧", Color::Cyan, &format!("running tool: {name}"));
        }
        AgentEvent::ToolCallComplete { name, result, .. } => {
            if result.success {
                print_event_line("✓", Color::Green, &format!("tool complete: {name}"));
            } else {
                print_event_line(
                    "✗",
                    Color::Red,
                    &format!(
                        "tool failed: {name}: {}",
                        result.error.as_deref().unwrap_or("error")
                    ),
                );
            }
        }
        AgentEvent::ContextSummarized {
            original_messages,
            new_messages,
        } => {
            print_event_line(
                "📝",
                Color::Yellow,
                &format!("context summarized: {original_messages} → {new_messages} messages"),
            );
        }
        AgentEvent::Error { message } => {
            print_event_line("❌", Color::Red, message);
        }
        AgentEvent::Cancelled => {
            print_event_line("⛔", Color::Red, "cancelled");
        }
        AgentEvent::TurnComplete { .. } | AgentEvent::Response { .. } => {}
    }
}

fn print_assistant_response(content: &str) {
    print_panel("assistant", "✦", Color::Cyan, content);
}

fn print_error_panel(message: &str) {
    print_panel("error", "!", Color::Red, message);
}

fn print_info_panel(title: &str, message: &str) {
    print_panel(title, "·", Color::DarkGrey, message);
}

fn print_event_line(icon: &str, color: Color, message: &str) {
    eprintln!(
        "{}{}{} {}{}{}",
        SetForegroundColor(color),
        icon,
        ResetColor,
        SetForegroundColor(Color::DarkGrey),
        message,
        ResetColor
    );
}

fn print_panel(title: &str, icon: &str, color: Color, content: &str) {
    let width = panel_width();
    let inner_width = width.saturating_sub(4);
    let title_text = format!(" {icon} {title} ");
    let title_width = display_width(&title_text);
    let remaining = inner_width.saturating_sub(title_width);

    println!();
    println!(
        "{}╭{}{}{}╮{}",
        SetForegroundColor(color),
        title_text,
        "─".repeat(remaining),
        ResetColor,
        ResetColor
    );

    for raw_line in content.lines() {
        if raw_line.is_empty() {
            print_panel_line("", inner_width, color);
            continue;
        }
        for line in wrap_line(raw_line, inner_width) {
            print_panel_line(&line, inner_width, color);
        }
    }

    println!(
        "{}╰{}╯{}\n",
        SetForegroundColor(color),
        "─".repeat(inner_width),
        ResetColor
    );
}

fn print_panel_line(line: &str, inner_width: usize, color: Color) {
    let padding = inner_width.saturating_sub(display_width(line));
    println!(
        "{}│{} {}{} │{}",
        SetForegroundColor(color),
        ResetColor,
        line,
        " ".repeat(padding),
        ResetColor
    );
}

fn panel_width() -> usize {
    terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(88)
        .clamp(48, 100)
}

fn wrap_line(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for ch in line.chars() {
        let ch_width = char_display_width(ch);
        if current_width > 0 && current_width + ch_width > max_width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_display_width).sum()
}

fn char_display_width(c: char) -> usize {
    if c as u32 >= 0x1100
        && (c as u32 <= 0x115F
            || c as u32 == 0x2329
            || c as u32 == 0x232A
            || c as u32 >= 0x2E80 && c as u32 <= 0x303E
            || c as u32 >= 0x3040 && c as u32 <= 0xA4CF
            || c as u32 >= 0xAC00 && c as u32 <= 0xD7A3
            || c as u32 >= 0xF900 && c as u32 <= 0xFAFF
            || c as u32 >= 0xFE10 && c as u32 <= 0xFE1F
            || c as u32 >= 0xFE30 && c as u32 <= 0xFE6F
            || c as u32 >= 0xFF00 && c as u32 <= 0xFF60
            || c as u32 >= 0xFFE0 && c as u32 <= 0xFFE6
            || c as u32 >= 0x20000 && c as u32 <= 0x2FFFD
            || c as u32 >= 0x30000 && c as u32 <= 0x3FFFD)
    {
        2
    } else {
        1
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
