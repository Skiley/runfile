use std::path::PathBuf;
use std::process;

use runfile_parser::{RUNFILE_NAME, discover_runfile_cwd};

use crate::runfile_helpers::resolve_and_merge;
use crate::runfile_helpers::resolve_runfile_path;
use crate::runfile_helpers::runfile_target_env;

pub fn cmd_mcp_install(file: Option<&std::path::Path>, agent_raw: &str) {
	let agent = if agent_raw.is_empty() { None } else { Some(agent_raw) };
	let runfile_path_str = file.map(|p| p.to_string_lossy().to_string());
	let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
	let result = runfile_mcp::install_for_agent(agent, runfile_path_str.as_deref(), &base_dir);
	match result {
		runfile_mcp::InstallResult::Installed { path } => {
			println!("Installed MCP server config to {}", path.display());
		}
		runfile_mcp::InstallResult::Updated { path } => {
			println!("Updated MCP server config in {}", path.display());
		}
		runfile_mcp::InstallResult::Instructions(text) => {
			println!("{text}");
		}
	}
}

pub fn cmd_mcp_inspect(file: Option<&std::path::Path>) {
	let merge_result = resolve_and_merge(file);
	let runfile = merge_result.runfile;
	println!("{}", runfile_mcp::inspect_json(&runfile));
}

pub fn cmd_mcp_server(file: Option<&std::path::Path>) {
	let merge_result = resolve_and_merge(file);
	let runfile = merge_result.runfile;

	let env_target = if file.is_none() { runfile_target_env() } else { None };
	let runfile_path = if let Some(f) = file.or(env_target.as_deref()) {
		resolve_runfile_path(Some(f))
	} else {
		discover_runfile_cwd().unwrap_or_else(|_| PathBuf::from(RUNFILE_NAME))
	};

	let run_binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("run"));
	let server = runfile_mcp::RunfileMcpServer::new(&runfile, run_binary, runfile_path);

	let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
		eprintln!("Failed to create async runtime: {e}");
		process::exit(1);
	});

	rt.block_on(async {
		if let Err(e) = runfile_mcp::run_server(server).await {
			eprintln!("MCP server error: {e}");
			process::exit(1);
		}
	});
}
