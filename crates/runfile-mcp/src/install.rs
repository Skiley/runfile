use std::path::{Path, PathBuf};

/// Result of an install attempt.
pub enum InstallResult {
	/// Successfully wrote config to a file.
	Installed { path: PathBuf },
	/// Updated existing config in a file.
	Updated { path: PathBuf },
	/// Could not auto-install; showing instructions instead.
	Instructions(String),
}

/// Get the MCP config JSON snippet for embedding in agent config files.
pub fn mcp_config_snippet(runfile_path: Option<&str>) -> serde_json::Value {
	let mut full_args: Vec<String> = Vec::new();
	if let Some(path) = runfile_path {
		full_args.push("-f".to_string());
		full_args.push(path.to_string());
	}
	full_args.push(":mcp".to_string());

	serde_json::json!({
		"command": "run",
		"args": full_args
	})
}

/// Install or show instructions for the given agent.
/// `base_dir` is the project root where config directories are created.
pub fn install_for_agent(agent: Option<&str>, runfile_path: Option<&str>, base_dir: &Path) -> InstallResult {
	let snippet = mcp_config_snippet(runfile_path);
	let snippet_pretty = serde_json::to_string_pretty(&snippet).unwrap();

	match agent {
		Some("claude-code") => write_json_config(&base_dir.join(".claude/settings.local.json"), &snippet),
		Some("cursor") => write_json_config(&base_dir.join(".cursor/mcp.json"), &snippet),
		Some("claude-desktop") => InstallResult::Instructions(instructions_claude_desktop(&snippet_pretty)),
		Some("codex") => InstallResult::Instructions(instructions_codex(&snippet_pretty)),
		Some("junie") => InstallResult::Instructions(instructions_junie(&snippet_pretty)),
		Some(other) => InstallResult::Instructions(instructions_generic(other, &snippet_pretty)),
		None => InstallResult::Instructions(instructions_generic_no_agent(&snippet_pretty)),
	}
}

/// Write an MCP server entry into a JSON config file under "mcpServers".
fn write_json_config(path: &Path, snippet: &serde_json::Value) -> InstallResult {
	let mut config: serde_json::Value = if path.is_file() {
		let contents = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".to_string());
		serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
	} else {
		serde_json::json!({})
	};

	let is_update = config.get("mcpServers").and_then(|s| s.get("runfile")).is_some();

	config
		.as_object_mut()
		.unwrap()
		.entry("mcpServers")
		.or_insert_with(|| serde_json::json!({}))
		.as_object_mut()
		.unwrap()
		.insert("runfile".to_string(), snippet.clone());

	if let Some(parent) = path.parent()
		&& let Err(e) = std::fs::create_dir_all(parent)
	{
		eprintln!("Error creating {}: {e}", parent.display());
		return InstallResult::Instructions(format!(
			"Could not create directory {}. Please create it manually and re-run.",
			parent.display()
		));
	}

	let json = serde_json::to_string_pretty(&config).unwrap();
	if let Err(e) = std::fs::write(path, json) {
		eprintln!("Error writing {}: {e}", path.display());
		return InstallResult::Instructions(format!(
			"Could not write {}. Please create it manually.",
			path.display()
		));
	}

	if is_update {
		InstallResult::Updated {
			path: path.to_path_buf(),
		}
	} else {
		InstallResult::Installed {
			path: path.to_path_buf(),
		}
	}
}

fn instructions_claude_desktop(snippet: &str) -> String {
	let config_path = if cfg!(target_os = "macos") {
		"~/Library/Application Support/Claude/claude_desktop_config.json"
	} else if cfg!(windows) {
		"%APPDATA%\\Claude\\claude_desktop_config.json"
	} else {
		"~/.config/claude/claude_desktop_config.json"
	};

	format!(
		r#"To install the Runfile MCP server for Claude Desktop:

1. Open your config file at:
   {config_path}

2. Add the following under "mcpServers":

   "runfile": {snippet}

3. Restart Claude Desktop."#
	)
}

fn instructions_codex(snippet: &str) -> String {
	format!(
		r#"To install the Runfile MCP server for Codex:

Add the following MCP server configuration:

"runfile": {snippet}

Refer to Codex documentation for the exact config file location."#
	)
}

fn instructions_junie(snippet: &str) -> String {
	format!(
		r#"To install the Runfile MCP server for Junie (JetBrains AI):

Add the following MCP server configuration:

"runfile": {snippet}

Refer to JetBrains AI documentation for the exact config file location."#
	)
}

fn instructions_generic(agent: &str, snippet: &str) -> String {
	format!(
		r#"To install the Runfile MCP server for "{agent}":

Add the following MCP server configuration:

"runfile": {snippet}

Refer to your agent's documentation for the config file location."#
	)
}

fn instructions_generic_no_agent(snippet: &str) -> String {
	format!(
		r#"Runfile MCP Server configuration snippet:

"runfile": {snippet}

Supported agents with auto-install:
  - claude-code    (writes to .claude/settings.local.json)
  - cursor         (writes to .cursor/mcp.json)

Agents with manual instructions:
  - claude-desktop
  - codex
  - junie

Usage:
  run :mcp install claude-code    # Auto-install
  run :mcp install cursor         # Auto-install
  run :mcp install claude-desktop # Show instructions
  run :mcp install <agent>        # Show generic instructions"#
	)
}
