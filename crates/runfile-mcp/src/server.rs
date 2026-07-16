use crate::tools::build_tool_defs;
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::model::*;
use rmcp::service::ServiceExt;
use rmcp::{ErrorData as McpError, handler::server::ServerHandler};
use runfile_parser::Runfile;
use std::path::PathBuf;

/// MCP server that exposes Runfile targets as tools.
pub struct RunfileMcpServer {
	tool_router: ToolRouter<Self>,
}

impl RunfileMcpServer {
	/// Create a new MCP server from a parsed Runfile.
	///
	/// `run_binary` is the path to the `run` executable used to invoke targets.
	/// `runfile_path` is the path to the Runfile.json (passed via `-f` to the binary).
	pub fn new(runfile: &Runfile, run_binary: PathBuf, runfile_path: PathBuf) -> Self {
		let mut router = ToolRouter::new();
		let tool_defs = build_tool_defs(runfile);

		for def in tool_defs {
			let tool = Tool::new(
				def.name.clone(),
				def.description.clone(),
				serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(def.input_schema.clone())
					.unwrap_or_default(),
			);

			let target_name = def.name.clone();
			let bin = run_binary.clone();
			let rf_path = runfile_path.clone();

			let route = ToolRoute::new_dyn(tool, move |context| {
				let target = target_name.clone();
				let bin = bin.clone();
				let rf_path = rf_path.clone();

				// Extract args from the tool call arguments.
				// Named string properties become --key=value, boolean true becomes --key,
				// and the "args" array provides additional positional arguments.
				let mut cmd_args: Vec<String> =
					vec!["-f".to_string(), rf_path.to_string_lossy().to_string(), target.clone()];

				if let Some(arguments) = &context.arguments {
					// Process named properties first (sorted for determinism)
					let mut keys: Vec<&String> = arguments.keys().filter(|k| *k != "args").collect();
					keys.sort();
					for key in keys {
						match &arguments[key] {
							serde_json::Value::String(s) => {
								cmd_args.push(format!("--{key}={s}"));
							}
							serde_json::Value::Bool(true) => {
								cmd_args.push(format!("--{key}"));
							}
							serde_json::Value::Number(n) => {
								cmd_args.push(format!("--{key}={n}"));
							}
							_ => {} // Bool(false) and others are skipped
						}
					}

					// Then positional args from the "args" array
					if let Some(serde_json::Value::Array(arr)) = arguments.get("args") {
						for item in arr {
							if let serde_json::Value::String(s) = item {
								cmd_args.push(s.clone());
							}
						}
					}
				}

				Box::pin(async move { execute_target(&bin, cmd_args).await })
			});

			router.add_route(route);
		}

		Self { tool_router: router }
	}
}

async fn execute_target(run_binary: &std::path::Path, cmd_args: Vec<String>) -> Result<CallToolResult, McpError> {
	let result = tokio::process::Command::new(run_binary).args(&cmd_args).output().await;

	match result {
		Ok(output) => {
			let stdout = String::from_utf8_lossy(&output.stdout);
			let stderr = String::from_utf8_lossy(&output.stderr);

			let mut text = String::new();
			if !stdout.is_empty() {
				text.push_str(&stdout);
			}
			if !stderr.is_empty() {
				if !text.is_empty() {
					text.push('\n');
				}
				text.push_str(&stderr);
			}
			if text.is_empty() {
				text = "(no output)".to_string();
			}

			if output.status.success() {
				Ok(CallToolResult::success(vec![Content::text(text)]))
			} else {
				let code = output.status.code().unwrap_or(-1);
				Ok(CallToolResult::error(vec![Content::text(format!(
					"Command failed with exit code {code}\n{text}"
				))]))
			}
		}
		Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
			"Failed to execute: {e}"
		))])),
	}
}

#[rmcp::tool_handler(router = self.tool_router)]
impl ServerHandler for RunfileMcpServer {
	fn get_info(&self) -> ServerInfo {
		ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
			.with_instructions("Runfile MCP Server — execute targets defined in Runfile.json")
	}
}

/// Start the MCP server on stdio.
pub async fn run_server(server: RunfileMcpServer) -> Result<(), Box<dyn std::error::Error>> {
	let (stdin, stdout) = rmcp::transport::io::stdio();
	let service = server.serve((stdin, stdout)).await?;
	service.waiting().await?;
	Ok(())
}
