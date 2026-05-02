use crate::install::{install_for_agent, mcp_config_snippet, InstallResult};
use crate::tools::{build_tool_defs, inspect_json};
use runfile_parser::{CommandSpec, EnvValue, Runfile, RUNFILE_NAME};
use std::collections::HashMap;

fn make_runfile(targets: Vec<(&str, CommandSpec)>) -> Runfile {
	let mut target_map = HashMap::new();
	for (name, spec) in targets {
		target_map.insert(name.to_string(), spec);
	}
	Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: target_map,
		globals: None,
	}
}

fn simple_spec(commands: Vec<&str>, description: Option<&str>) -> CommandSpec {
	let mut spec = CommandSpec::new_shell(commands.into_iter().map(String::from).collect());
	spec.description = description.map(String::from);
	spec
}

// ── Tool definition tests ─────────────────────────────────────────

#[test]
fn build_tools_basic_target() {
	let runfile = make_runfile(vec![(
		"build",
		simple_spec(vec!["cargo build"], Some("Build the project")),
	)]);
	let tools = build_tool_defs(&runfile);
	assert_eq!(tools.len(), 1);
	assert_eq!(tools[0].name, "build");
	assert_eq!(tools[0].description, "Build the project");
}

#[test]
fn build_tools_no_description_gets_default() {
	let runfile = make_runfile(vec![("test", simple_spec(vec!["cargo test"], None))]);
	let tools = build_tool_defs(&runfile);
	assert_eq!(tools[0].description, "Run the \"test\" target");
}

#[test]
fn build_tools_sorted_alphabetically() {
	let runfile = make_runfile(vec![
		("test", simple_spec(vec!["cargo test"], None)),
		("build", simple_spec(vec!["cargo build"], None)),
		("lint", simple_spec(vec!["cargo clippy"], None)),
	]);
	let tools = build_tool_defs(&runfile);
	let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(names, vec!["build", "lint", "test"]);
}

#[test]
fn build_tools_target_with_positional_args_has_args_array() {
	let runfile = make_runfile(vec![("build", simple_spec(vec!["cargo build $(ARGS)"], None))]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	let args_prop = props.get("args").unwrap();
	assert_eq!(args_prop.get("type").unwrap(), "array");
}

#[test]
fn build_tools_target_with_named_args_has_explicit_properties() {
	let runfile = make_runfile(vec![(
		"deploy",
		simple_spec(
			vec!["deploy --env=$(ARGS.env) --region=$(ARGS.region ? us-east-1)"],
			None,
		),
	)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	// Named args should be explicit string properties
	assert_eq!(props.get("env").unwrap().get("type").unwrap(), "string");
	assert_eq!(props.get("region").unwrap().get("type").unwrap(), "string");
	// No generic "args" array since $(ARGS) is not used
	assert!(props.get("args").is_none());
	// "env" is required (no default), "region" is optional (has default)
	let required = schema.get("required").unwrap().as_array().unwrap();
	assert!(required.contains(&serde_json::json!("env")));
	assert!(!required.contains(&serde_json::json!("region")));
}

#[test]
fn build_tools_target_with_flags_has_boolean_properties() {
	let runfile = make_runfile(vec![(
		"build",
		simple_spec(vec!["cargo build $(FLAGS.release ? --release :)"], None),
	)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	assert_eq!(props.get("release").unwrap().get("type").unwrap(), "boolean");
}

#[test]
fn build_tools_target_with_positional_and_named_args() {
	let runfile = make_runfile(vec![("run", simple_spec(vec!["app --env=$(ARGS.env) $(ARGS)"], None))]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	// Both explicit named property and positional args array
	assert_eq!(props.get("env").unwrap().get("type").unwrap(), "string");
	assert_eq!(props.get("args").unwrap().get("type").unwrap(), "array");
}

#[test]
fn build_tools_target_without_args_has_empty_properties() {
	let runfile = make_runfile(vec![("build", simple_spec(vec!["cargo build"], None))]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap().as_object().unwrap();
	assert!(props.is_empty());
}

#[test]
fn build_tools_args_from_env_values_are_included() {
	let mut spec = simple_spec(vec!["echo $MY_VAR"], None);
	let mut env = HashMap::new();
	env.insert("MY_VAR".to_string(), EnvValue::String("$(ARGS.config)".into()));
	spec.env = Some(env);
	let runfile = make_runfile(vec![("test", spec)]);
	let tools = build_tool_defs(&runfile);
	let schema = &tools[0].input_schema;
	let props = schema.get("properties").unwrap();
	assert_eq!(props.get("config").unwrap().get("type").unwrap(), "string");
}

// ── Security tests: sensitive fields NOT exposed ──────────────────

#[test]
fn tools_do_not_expose_env_files() {
	let mut spec = simple_spec(vec!["cargo build"], Some("Build"));
	spec.env_files = Some(vec![".env".into(), ".env.secret".into()]);
	let runfile = make_runfile(vec![("build", spec)]);
	let json = inspect_json(&runfile);
	assert!(!json.contains(".env"), "env_files must not appear in tool output");
	assert!(
		!json.contains("envFiles"),
		"envFiles key must not appear in tool output"
	);
}

#[test]
fn tools_do_not_expose_env_values() {
	let mut spec = simple_spec(vec!["cargo build"], Some("Build"));
	let mut env = HashMap::new();
	env.insert("SECRET_KEY".to_string(), EnvValue::String("s3cr3t".into()));
	spec.env = Some(env);
	let runfile = make_runfile(vec![("build", spec)]);
	let json = inspect_json(&runfile);
	assert!(!json.contains("s3cr3t"), "env values must not appear in tool output");
	assert!(!json.contains("SECRET_KEY"), "env keys must not appear in tool output");
}

#[test]
fn tools_do_not_expose_commands() {
	let runfile = make_runfile(vec![(
		"build",
		simple_spec(vec!["cargo build --secret-flag"], Some("Build")),
	)]);
	let json = inspect_json(&runfile);
	assert!(
		!json.contains("--secret-flag"),
		"command contents must not appear in tool output"
	);
}

// ── Inspect JSON format tests ─────────────────────────────────────

#[test]
fn inspect_json_is_valid_json_array() {
	let runfile = make_runfile(vec![
		("build", simple_spec(vec!["cargo build"], Some("Build"))),
		("test", simple_spec(vec!["cargo test"], Some("Test"))),
	]);
	let json = inspect_json(&runfile);
	let parsed: serde_json::Value = serde_json::from_str(&json).expect("should be valid JSON");
	assert!(parsed.is_array());
	assert_eq!(parsed.as_array().unwrap().len(), 2);
}

#[test]
fn inspect_json_has_required_fields() {
	let runfile = make_runfile(vec![("build", simple_spec(vec!["cargo build"], Some("Build")))]);
	let json = inspect_json(&runfile);
	let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
	let tool = &parsed[0];
	assert!(tool.get("name").is_some());
	assert!(tool.get("description").is_some());
	assert!(tool.get("inputSchema").is_some());
}

// ── Install snippet tests ─────────────────────────────────────────

#[test]
fn config_snippet_without_runfile_path() {
	let snippet = mcp_config_snippet(None);
	let obj = snippet.as_object().unwrap();
	assert_eq!(obj.get("command").unwrap(), "run");
	let args = obj.get("args").unwrap().as_array().unwrap();
	assert_eq!(args.len(), 1);
	assert_eq!(args[0], ":mcp");
}

#[test]
fn config_snippet_with_runfile_path() {
	let snippet = mcp_config_snippet(Some("ci/Runfile.json"));
	let obj = snippet.as_object().unwrap();
	assert_eq!(obj.get("command").unwrap(), "run");
	let args = obj.get("args").unwrap().as_array().unwrap();
	assert_eq!(args.len(), 3);
	assert_eq!(args[0], "-f");
	assert_eq!(args[1], "ci/Runfile.json");
	assert_eq!(args[2], ":mcp");
}

#[test]
fn install_no_agent_returns_instructions() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(None, None, dir.path());
	assert!(matches!(result, InstallResult::Instructions(_)));
	if let InstallResult::Instructions(text) = result {
		assert!(text.contains("claude-code"));
		assert!(text.contains("cursor"));
	}
}

#[test]
fn install_unknown_agent_returns_instructions() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("unknown-agent"), None, dir.path());
	assert!(matches!(result, InstallResult::Instructions(_)));
	if let InstallResult::Instructions(text) = result {
		assert!(text.contains("unknown-agent"));
	}
}

#[test]
fn install_claude_desktop_returns_instructions() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("claude-desktop"), None, dir.path());
	assert!(matches!(result, InstallResult::Instructions(_)));
	if let InstallResult::Instructions(text) = result {
		assert!(text.contains("Claude Desktop"));
	}
}

#[test]
fn install_claude_code_writes_config() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("claude-code"), None, dir.path());

	assert!(matches!(result, InstallResult::Installed { .. }));
	let config_path = dir.path().join(".claude/settings.local.json");
	assert!(config_path.is_file());
	let contents: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
	assert!(contents.get("mcpServers").unwrap().get("runfile").is_some());
}

#[test]
fn install_cursor_writes_config() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("cursor"), None, dir.path());

	assert!(matches!(result, InstallResult::Installed { .. }));
	let config_path = dir.path().join(".cursor/mcp.json");
	assert!(config_path.is_file());
	let contents: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
	assert!(contents.get("mcpServers").unwrap().get("runfile").is_some());
}

#[test]
fn install_claude_code_updates_existing() {
	let dir = tempfile::TempDir::new().unwrap();

	// First install
	install_for_agent(Some("claude-code"), None, dir.path());

	// Second install should be an update
	let result = install_for_agent(Some("claude-code"), None, dir.path());
	assert!(matches!(result, InstallResult::Updated { .. }));
}

#[test]
fn install_preserves_existing_config_fields() {
	let dir = tempfile::TempDir::new().unwrap();
	let claude_dir = dir.path().join(".claude");
	std::fs::create_dir_all(&claude_dir).unwrap();
	let config_path = claude_dir.join("settings.local.json");

	// Write existing config with other fields
	let existing = serde_json::json!({
		"otherSetting": true,
		"mcpServers": {
			"other-server": { "command": "other" }
		}
	});
	std::fs::write(&config_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

	install_for_agent(Some("claude-code"), None, dir.path());

	let contents: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
	// Our entry was added
	assert!(contents.get("mcpServers").unwrap().get("runfile").is_some());
	// Existing entries preserved
	assert!(contents.get("mcpServers").unwrap().get("other-server").is_some());
	assert_eq!(contents.get("otherSetting").unwrap(), true);
}

// ── MCP Server construction tests ─────────────────────────────────

#[test]
fn server_can_be_constructed() {
	use crate::server::RunfileMcpServer;
	let runfile = make_runfile(vec![
		("build", simple_spec(vec!["cargo build"], Some("Build"))),
		("test", simple_spec(vec!["cargo test $(ARGS)"], Some("Test"))),
	]);
	// Just verify it doesn't panic
	let _server = RunfileMcpServer::new(
		&runfile,
		std::path::PathBuf::from("run"),
		std::path::PathBuf::from(RUNFILE_NAME),
	);
}

#[test]
fn server_empty_runfile() {
	use crate::server::RunfileMcpServer;
	// Need at least one target for a valid Runfile, but let's test the server
	// handles a single target fine
	let runfile = make_runfile(vec![("hello", simple_spec(vec!["echo hello"], None))]);
	let _server = RunfileMcpServer::new(
		&runfile,
		std::path::PathBuf::from("run"),
		std::path::PathBuf::from(RUNFILE_NAME),
	);
}

#[test]
fn build_tools_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", simple_spec(vec!["cargo build"], None)),
		("_setup", simple_spec(vec!["echo internal"], None)),
		("test", simple_spec(vec!["cargo test"], None)),
	]);
	let tools = build_tool_defs(&runfile);
	let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
	assert_eq!(names, vec!["build", "test"]);
	assert!(!names.contains(&"_setup"));
}
