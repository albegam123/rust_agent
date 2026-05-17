use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::tool::Tool;
use ragent_types::config::ToolsConfig;
use ragent_types::tool::ToolResult;
use ragent_types::tool::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::debug;
use tracing::info;
use tracing::warn;

#[derive(Debug, Deserialize)]
struct McpConfigFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct McpServerConfig {
    #[serde(rename = "type")]
    connection_type: Option<String>,
    command: Option<String>,
    args: Vec<String>,
    env: HashMap<String, String>,
    disabled: bool,
    connect_timeout_secs: Option<u64>,
    execute_timeout_secs: Option<u64>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            connection_type: None,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            disabled: false,
            connect_timeout_secs: None,
            execute_timeout_secs: None,
        }
    }
}

pub async fn load_mcp_tools(config: &ToolsConfig) -> Result<Vec<Box<dyn Tool>>> {
    if !config.enable_mcp {
        return Ok(Vec::new());
    }

    let config_path = config
        .mcp_config_path
        .as_deref()
        .unwrap_or_else(|| Path::new("mcp.json"));
    if !config_path.exists() {
        warn!(path = %config_path.display(), "MCP enabled but config file was not found");
        return Ok(Vec::new());
    }

    let config_text = tokio::fs::read_to_string(config_path)
        .await
        .with_context(|| format!("failed to read MCP config {}", config_path.display()))?;
    let mcp_config: McpConfigFile = serde_json::from_str(&config_text)
        .with_context(|| format!("failed to parse MCP config {}", config_path.display()))?;

    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    for (server_name, server_config) in mcp_config.mcp_servers {
        if server_config.disabled {
            debug!(server = %server_name, "skipping disabled MCP server");
            continue;
        }

        let connection_type = server_config
            .connection_type
            .as_deref()
            .unwrap_or("stdio")
            .to_ascii_lowercase();
        if connection_type != "stdio" {
            warn!(
                server = %server_name,
                connection_type,
                "unsupported MCP transport; only stdio is currently supported"
            );
            continue;
        }

        let Some(command) = server_config.command.as_deref() else {
            warn!(server = %server_name, "skipping MCP stdio server without command");
            continue;
        };

        let connect_timeout = Duration::from_secs(
            server_config
                .connect_timeout_secs
                .unwrap_or(config.mcp.connect_timeout_secs),
        );
        let execute_timeout = Duration::from_secs(
            server_config
                .execute_timeout_secs
                .unwrap_or(config.mcp.execute_timeout_secs),
        );

        match StdioMcpClient::connect(
            server_name.clone(),
            command,
            &server_config.args,
            &server_config.env,
            connect_timeout,
        )
        .await
        {
            Ok(client) => {
                let client = Arc::new(client);
                let specs = client.list_tools(connect_timeout).await?;
                info!(
                    server = %server_name,
                    count = specs.len(),
                    "loaded MCP tools"
                );
                for remote_spec in specs {
                    tools.push(Box::new(McpTool::new(
                        server_name.clone(),
                        remote_spec,
                        Arc::clone(&client),
                        execute_timeout,
                    )));
                }
            }
            Err(error) => {
                warn!(server = %server_name, "failed to connect MCP server: {error:#}");
            }
        }
    }

    Ok(tools)
}

#[derive(Debug, Clone)]
struct RemoteToolSpec {
    name: String,
    description: String,
    parameters: Value,
}

struct McpTool {
    exposed_name: String,
    remote_name: String,
    description: String,
    parameters: Value,
    client: Arc<StdioMcpClient>,
    execute_timeout: Duration,
}

impl McpTool {
    fn new(
        server_name: String,
        remote_spec: RemoteToolSpec,
        client: Arc<StdioMcpClient>,
        execute_timeout: Duration,
    ) -> Self {
        let exposed_name = exposed_tool_name(&server_name, &remote_spec.name);
        let description = if remote_spec.description.is_empty() {
            format!(
                "MCP tool `{}` from server `{server_name}`",
                remote_spec.name
            )
        } else {
            format!(
                "MCP tool `{}` from server `{server_name}`. {}",
                remote_spec.name, remote_spec.description
            )
        };

        Self {
            exposed_name,
            remote_name: remote_spec.name,
            description,
            parameters: remote_spec.parameters,
            client,
            execute_timeout,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            self.exposed_name.clone(),
            self.description.clone(),
            self.parameters.clone(),
        )
    }

    async fn call(&self, args: Value) -> Result<ToolResult> {
        Ok(self
            .client
            .call_tool(&self.remote_name, args, self.execute_timeout)
            .await)
    }
}

struct StdioMcpClient {
    server_name: String,
    state: Mutex<StdioMcpState>,
}

struct StdioMcpState {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl StdioMcpClient {
    async fn connect(
        server_name: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn MCP server `{server_name}`"))?;

        if let Some(stderr) = child.stderr.take() {
            let server_name_for_stderr = server_name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            info!(server = %server_name_for_stderr, "MCP server stderr: {line}");
                        }
                        Ok(None) => break,
                        Err(error) => {
                            warn!(server = %server_name_for_stderr, "failed to read MCP stderr: {error}");
                            break;
                        }
                    }
                }
            });
        }

        let stdin = child
            .stdin
            .take()
            .with_context(|| format!("MCP server `{server_name}` did not expose stdin"))?;
        let stdout = child
            .stdout
            .take()
            .with_context(|| format!("MCP server `{server_name}` did not expose stdout"))?;

        let client = Self {
            server_name,
            state: Mutex::new(StdioMcpState {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 1,
            }),
        };

        tokio::time::timeout(timeout, client.initialize())
            .await
            .map_err(|_| anyhow::anyhow!("timed out initializing MCP server"))??;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "ragent",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await?;
        self.notification("notifications/initialized", Value::Null)
            .await?;
        Ok(())
    }

    async fn list_tools(&self, timeout: Duration) -> Result<Vec<RemoteToolSpec>> {
        let result =
            tokio::time::timeout(timeout, self.request("tools/list", serde_json::json!({})))
                .await
                .map_err(|_| anyhow::anyhow!("timed out listing MCP tools"))??;

        let Some(tools) = result.get("tools").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };

        Ok(tools
            .iter()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?.to_string();
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let parameters = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                Some(RemoteToolSpec {
                    name,
                    description,
                    parameters,
                })
            })
            .collect())
    }

    async fn call_tool(&self, name: &str, arguments: Value, timeout: Duration) -> ToolResult {
        let call = self.request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": arguments,
            }),
        );

        match tokio::time::timeout(timeout, call).await {
            Ok(Ok(result)) => tool_result_from_mcp_response(result),
            Ok(Err(error)) => ToolResult::failure(format!("MCP tool execution failed: {error:#}")),
            Err(_) => ToolResult::failure(format!(
                "MCP tool execution timed out after {} seconds",
                timeout.as_secs()
            )),
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let mut state = self.state.lock().await;
        let id = state.next_id;
        state.next_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        write_json_line(&mut state.stdin, &request).await?;

        loop {
            let response = read_json_line(&mut state.stdout, &self.server_name).await?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }

            if let Some(error) = response.get("error") {
                return Err(anyhow::anyhow!("MCP request `{method}` failed: {error}"));
            }

            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn notification(&self, method: &str, params: Value) -> Result<()> {
        let mut state = self.state.lock().await;
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        write_json_line(&mut state.stdin, &notification).await
    }
}

impl Drop for StdioMcpState {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn write_json_line(stdin: &mut ChildStdin, value: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_json_line(stdout: &mut BufReader<ChildStdout>, server_name: &str) -> Result<Value> {
    let mut line = String::new();
    let bytes = stdout.read_line(&mut line).await?;
    if bytes == 0 {
        anyhow::bail!("MCP server `{server_name}` closed stdout");
    }
    serde_json::from_str(line.trim_end())
        .with_context(|| format!("MCP server `{server_name}` returned invalid JSON-RPC message"))
}

fn tool_result_from_mcp_response(result: Value) -> ToolResult {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let output = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(content_item_to_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| result.to_string());

    if is_error {
        ToolResult::failure(if output.is_empty() {
            "MCP tool returned an error".to_string()
        } else {
            output
        })
    } else {
        ToolResult::success(output)
    }
}

fn content_item_to_text(item: &Value) -> String {
    item.get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| item.to_string())
}

fn exposed_tool_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_tool_name_part(server_name),
        sanitize_tool_name_part(tool_name)
    )
}

fn sanitize_tool_name_part(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::process::Command as StdCommand;
    use tempfile::tempdir;

    #[test]
    fn exposed_tool_name_sanitizes_server_and_tool_name() {
        assert_eq!(
            exposed_tool_name("web search", "search.query"),
            "mcp__web_search__search_query"
        );
    }

    #[test]
    fn text_content_items_are_joined() {
        let result = tool_result_from_mcp_response(serde_json::json!({
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "text", "text": "world"}
            ]
        }));

        assert_eq!(result, ToolResult::success("hello\nworld"));
    }

    #[test]
    fn error_tool_result_uses_content_as_error() {
        let result = tool_result_from_mcp_response(serde_json::json!({
            "isError": true,
            "content": [{"type": "text", "text": "bad args"}]
        }));

        assert_eq!(result, ToolResult::failure("bad args"));
    }

    #[tokio::test]
    async fn load_mcp_tools_from_stdio_server() {
        if StdCommand::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let temp = tempdir().unwrap();
        let server_path = temp.path().join("server.py");
        fs::write(
            &server_path,
            r#"
import json
import sys

for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "fake", "version": "1"}}}), flush=True)
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": [{"name": "echo", "description": "Echo input", "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"]}}]}}), flush=True)
    elif method == "tools/call":
        value = msg["params"]["arguments"]["value"]
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {"content": [{"type": "text", "text": value}]}}), flush=True)
"#,
        )
        .unwrap();
        let config_path = temp.path().join("mcp.json");
        fs::write(
            &config_path,
            serde_json::json!({
                "mcpServers": {
                    "fake-server": {
                        "type": "stdio",
                        "command": "python3",
                        "args": [server_path],
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let config = ToolsConfig {
            enable_mcp: true,
            mcp_config_path: Some(config_path),
            ..ToolsConfig::default()
        };

        let tools = load_mcp_tools(&config).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].spec().callable_name().unwrap(),
            "mcp__fake-server__echo"
        );
        let result = tools[0]
            .call(serde_json::json!({"value": "hello"}))
            .await
            .unwrap();

        assert_eq!(result, ToolResult::success("hello"));
    }
}
