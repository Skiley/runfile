use super::*;

// ── CLI parse tests ──────────────────────────────────────────────

#[test]
fn cli_parses_secret_keys_add_interactive() {
	let result = try_parse(&["run", ":env", "secret-keys", "add"]);
	assert!(result.is_ok(), "failed to parse ':env secret-keys add': {result:?}");
}

#[test]
fn cli_parses_secret_keys_add_with_key() {
	let result = try_parse(&[
		"run",
		":env",
		"secret-keys",
		"add",
		"--key",
		"deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
	]);
	assert!(
		result.is_ok(),
		"failed to parse ':env secret-keys add --key ...': {result:?}"
	);
}

#[test]
fn cli_parses_mcp_server() {
	let result = try_parse(&["run", ":mcp", "server"]);
	assert!(result.is_ok(), "failed to parse ':mcp server': {result:?}");
}

#[test]
fn cli_parses_mcp_inspect() {
	let result = try_parse(&["run", ":mcp", "inspect"]);
	assert!(result.is_ok(), "failed to parse ':mcp inspect': {result:?}");
}

#[test]
fn cli_parses_mcp_install_with_agent() {
	let result = try_parse(&["run", ":mcp", "install", "claude-code"]);
	assert!(result.is_ok(), "failed to parse ':mcp install claude-code': {result:?}");
}

#[test]
fn cli_parses_mcp_install_without_agent() {
	let result = try_parse(&["run", ":mcp", "install"]);
	assert!(result.is_ok(), "failed to parse ':mcp install' (no agent): {result:?}");
}

#[test]
fn cli_parses_mcp_bare() {
	// `:mcp` with no subcommand should still parse (shows help)
	let result = try_parse(&["run", ":mcp"]);
	assert!(result.is_ok(), "failed to parse bare ':mcp': {result:?}");
}

#[test]
fn cli_parses_mcp_with_file_flag() {
	let result = try_parse(&["run", ":mcp", "-f", "path/to/Runfile.json", "inspect"]);
	assert!(result.is_ok(), "failed to parse ':mcp -f ... inspect': {result:?}");
}

#[test]
fn cli_parses_completions_install() {
	let result = try_parse(&["run", ":completions", "install", "bash"]);
	assert!(
		result.is_ok(),
		"failed to parse ':completions install bash': {result:?}"
	);
}

#[test]
fn cli_parses_completions_uninstall() {
	let result = try_parse(&["run", ":completions", "uninstall", "zsh"]);
	assert!(
		result.is_ok(),
		"failed to parse ':completions uninstall zsh': {result:?}"
	);
}

#[test]
fn cli_parses_completions_output() {
	let result = try_parse(&["run", ":completions", "output", "fish"]);
	assert!(result.is_ok(), "failed to parse ':completions output fish': {result:?}");
}

#[test]
fn cli_rejects_completions_without_subcommand() {
	let result = try_parse(&["run", ":completions"]);
	assert!(result.is_err(), ":completions without subcommand should fail");
}

#[test]
fn cli_rejects_completions_install_without_shell() {
	let result = try_parse(&["run", ":completions", "install"]);
	assert!(result.is_err(), ":completions install without shell should fail");
}

#[test]
fn cli_parses_generate_zed_tasks() {
	let result = try_parse(&["run", ":generate", "zed-tasks"]);
	assert!(result.is_ok(), "failed to parse ':generate zed-tasks': {result:?}");
}

#[test]
fn cli_parses_generate_jetbrains() {
	let result = try_parse(&["run", ":generate", "jetbrains-run-configurations"]);
	assert!(
		result.is_ok(),
		"failed to parse ':generate jetbrains-run-configurations': {result:?}"
	);
}

#[test]
fn cli_parses_generate_jetbrains_with_output_dir() {
	let result = try_parse(&["run", ":generate", "jetbrains-run-configurations", "-o", ".idea"]);
	assert!(
		result.is_ok(),
		"failed to parse ':generate jetbrains-run-configurations -o .idea': {result:?}"
	);
}

#[test]
fn cli_parses_generate_vscode_tasks_stdout() {
	let result = try_parse(&["run", ":generate", "vscode-tasks", "--stdout"]);
	assert!(
		result.is_ok(),
		"failed to parse ':generate vscode-tasks --stdout': {result:?}"
	);
}

#[test]
fn cli_parses_generate_zed_tasks_stdout() {
	let result = try_parse(&["run", ":generate", "zed-tasks", "--stdout"]);
	assert!(
		result.is_ok(),
		"failed to parse ':generate zed-tasks --stdout': {result:?}"
	);
}

#[test]
fn cli_parses_generate_jetbrains_stdout() {
	let result = try_parse(&["run", ":generate", "jetbrains-run-configurations", "--stdout"]);
	assert!(
		result.is_ok(),
		"failed to parse ':generate jetbrains-run-configurations --stdout': {result:?}"
	);
}

#[test]
fn cli_rejects_generate_without_subcommand() {
	let result = try_parse(&["run", ":generate"]);
	assert!(result.is_err(), ":generate without subcommand should fail");
}

#[test]
fn cli_parses_convert_makefile() {
	let result = try_parse(&["run", ":convert", "makefile"]);
	assert!(result.is_ok(), "failed to parse ':convert makefile': {result:?}");
}

#[test]
fn cli_parses_convert_makefile_with_path() {
	let result = try_parse(&["run", ":convert", "makefile", "-p", "custom/Makefile"]);
	assert!(
		result.is_ok(),
		"failed to parse ':convert makefile -p path': {result:?}"
	);
}

#[test]
fn cli_parses_convert_package_json() {
	let result = try_parse(&["run", ":convert", "package-json"]);
	assert!(result.is_ok(), "failed to parse ':convert package-json': {result:?}");
}

#[test]
fn cli_parses_convert_package_json_with_path() {
	let result = try_parse(&["run", ":convert", "package-json", "-p", "custom/package.json"]);
	assert!(
		result.is_ok(),
		"failed to parse ':convert package-json -p path': {result:?}"
	);
}

#[test]
fn cli_rejects_convert_without_subcommand() {
	let result = try_parse(&["run", ":convert"]);
	assert!(result.is_err(), ":convert without subcommand should fail");
}

#[test]
fn cli_parses_list() {
	let result = try_parse(&["run", ":list"]);
	assert!(result.is_ok(), "failed to parse ':list': {result:?}");
}

#[test]
fn cli_parses_init() {
	let result = try_parse(&["run", ":init"]);
	assert!(result.is_ok(), "failed to parse ':init': {result:?}");
}

#[test]
fn cli_parses_init_with_path() {
	let result = try_parse(&["run", ":init", "-p", "custom/Runfile.json"]);
	assert!(result.is_ok(), "failed to parse ':init -p path': {result:?}");
}
