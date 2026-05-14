# ragent

A Rust LLM agent runtime, inspired by [Mini-Agent](https://github.com/user/mini-agent).

Architecture borrows patterns from [codex-rs](https://github.com/openai/codex) (event channel, multi-crate workspace) and [zeroclaw](https://github.com/user/zeroclaw) (trait-per-subsystem, factory pattern).

## Architecture

```
        cli (entry point, DI assembly)
       / | \
      /  |  \
   core llm tools       ← no mutual dependencies
      \  |  /
       \ | /
       traits            ← behavior contracts (LLMProvider, Tool)
         |
       types             ← pure data types (Message, Config, Event)
```

### Crates

| Crate | Purpose |
|-------|---------|
| `ragent-types` | Pure data types: Message, ToolCall, Config, Event |
| `ragent-traits` | Behavior contracts: `LLMProvider`, `Tool` traits |
| `ragent-llm` | LLM provider implementations (OpenAI, Anthropic) |
| `ragent-tools` | Tool implementations (bash, file ops, notes) |
| `ragent-core` | Agent orchestration loop, context management, config loading |
| `ragent-cli` | CLI entry point and interactive loop |

### Key Design Patterns

- **Event Channel** (from codex-rs): Agent emits `AgentEvent`s through a channel; UI consumes them. Decouples agent logic from presentation.
- **Trait-per-subsystem** (from zeroclaw): `LLMProvider` and `Tool` are trait contracts. Implementations are injected at the CLI layer.
- **Factory functions**: `create_provider()` and `create_tools()` build implementations from config.
- **Multi-crate workspace**: Cargo enforces dependency direction. `core` cannot depend on `cli`; `tools` cannot depend on `llm`.

## Quick Start

```bash
# Copy and edit config
cp config/config.example.toml config/config.toml
# Set your API key
export RAGENT_API_KEY="sk-..."

# Run interactively
cargo run

# Run with a single prompt
cargo run -- -p "What files are in the current directory?"
```

## Development

```bash
# Build
cargo build

# Test
cargo test

# Run
cargo run
```
