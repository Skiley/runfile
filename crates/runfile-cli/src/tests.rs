use crate::completions::{
	completions_install_profile, completions_uninstall_profile, BASH_COMPLETION, FISH_COMPLETION,
	POWERSHELL_COMPLETION, ZSH_COMPLETION,
};
use crate::Cli;
use clap::{CommandFactory, Parser};
use std::fs;

/// Helper: try parsing CLI args, returning only the error for debug printing
fn try_parse(args: &[&str]) -> Result<(), clap::Error> {
	Cli::try_parse_from(args).map(|_| ())
}
use std::path::PathBuf;

fn temp_file(name: &str) -> PathBuf {
	std::env::temp_dir().join(format!("runfile_test_{name}_{}", std::process::id()))
}

// ── completions_install_profile ──────────────────────────────────

#[test]
fn install_profile_appends_marker_and_content() {
	let path = temp_file("install_append");
	fs::write(&path, "existing content\n").unwrap();

	completions_install_profile(
		"eval \"$(run :completions output bash)\"",
		&path,
		"# runfile completions",
	);

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	assert!(result.contains("existing content"));
	assert!(result.contains("# runfile completions"));
	assert!(result.contains("eval \"$(run :completions output bash)\""));
	// marker comes after existing content
	assert!(result.find("existing content") < result.find("# runfile completions"));
}

#[test]
fn install_profile_creates_file_if_missing() {
	let path = temp_file("install_create");
	// Ensure it doesn't exist
	let _ = fs::remove_file(&path);

	completions_install_profile("my_content", &path, "# runfile completions");

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	assert!(result.contains("# runfile completions"));
	assert!(result.contains("my_content"));
}

#[test]
fn install_profile_idempotent_when_marker_present() {
	let path = temp_file("install_idempotent");
	fs::write(&path, "# runfile completions\neval stuff\n").unwrap();

	// Call again — should not duplicate
	completions_install_profile("eval stuff", &path, "# runfile completions");

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	// Marker appears exactly once
	assert_eq!(result.matches("# runfile completions").count(), 1);
}

#[test]
fn install_profile_adds_newline_before_marker_when_file_not_empty() {
	let path = temp_file("install_newline");
	fs::write(&path, "line1").unwrap(); // no trailing newline

	completions_install_profile("content", &path, "# marker");

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	// Must have a blank separator before marker
	assert!(result.contains("line1\n\n# marker"));
}

// ── completions_uninstall_profile ────────────────────────────────

#[test]
fn uninstall_profile_removes_marker_block() {
	let path = temp_file("uninstall_remove");
	fs::write(&path, "before\n\n# runfile completions\neval stuff\nmore stuff\n").unwrap();

	completions_uninstall_profile(&path, "# runfile completions");

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	assert!(result.contains("before"));
	assert!(!result.contains("# runfile completions"));
	assert!(!result.contains("eval stuff"));
}

#[test]
fn uninstall_profile_no_op_when_marker_absent() {
	let path = temp_file("uninstall_noop");
	fs::write(&path, "some content\n").unwrap();

	completions_uninstall_profile(&path, "# runfile completions");

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	assert_eq!(result, "some content\n");
}

#[test]
fn install_then_uninstall_roundtrip() {
	let path = temp_file("roundtrip");
	fs::write(&path, "pre-existing\n").unwrap();

	completions_install_profile(
		"eval \"$(run :completions output bash)\"",
		&path,
		"# runfile completions",
	);
	completions_uninstall_profile(&path, "# runfile completions");

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	assert!(result.contains("pre-existing"));
	assert!(!result.contains("# runfile completions"));
}

// ── completion script content sanity checks ──────────────────────

#[test]
fn bash_completion_registers_function() {
	assert!(BASH_COMPLETION.contains("_run_completions"));
	assert!(BASH_COMPLETION.contains("complete -F _run_completions run"));
}

#[test]
fn bash_completion_handles_filedir_fallback() {
	assert!(BASH_COMPLETION.contains("declare -F _filedir"));
	assert!(BASH_COMPLETION.contains("compgen -f"));
}

#[test]
fn bash_completion_lists_targets_and_subcommands() {
	assert!(BASH_COMPLETION.contains("--list-targets"));
	assert!(BASH_COMPLETION.contains("--list-subcommands"));
}

#[test]
fn zsh_completion_defines_compdef() {
	// eval variant: initialises compinit and registers via compdef
	assert!(ZSH_COMPLETION.contains("compdef _run run"));
	assert!(ZSH_COMPLETION.contains("compinit"));
	assert!(ZSH_COMPLETION.contains("_run_subcmds"));
	assert!(ZSH_COMPLETION.contains("--list-targets"));

	// eval variant includes compdef registration
	assert!(ZSH_COMPLETION.contains("compdef _run run"));
}

#[test]
fn fish_completion_disables_default_file_completions() {
	assert!(FISH_COMPLETION.contains("complete -c run -f"));
	assert!(FISH_COMPLETION.contains("--list-targets"));
	assert!(FISH_COMPLETION.contains("--list-subcommands"));
}

#[test]
fn powershell_completion_registers_completer() {
	assert!(POWERSHELL_COMPLETION.contains("Register-ArgumentCompleter"));
	assert!(POWERSHELL_COMPLETION.contains("--list-targets"));
	assert!(POWERSHELL_COMPLETION.contains("--list-subcommands"));
}

// ── CLI structure tests ──────────────────────────────────────────

fn find_subcommand<'a>(cmd: &'a clap::Command, name: &str) -> &'a clap::Command {
	cmd.get_subcommands()
		.find(|s| s.get_name() == name)
		.unwrap_or_else(|| panic!("subcommand '{name}' not found"))
}

#[test]
fn cli_has_all_top_level_subcommands() {
	let cmd = Cli::command();
	let names: Vec<&str> = cmd
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();

	assert!(names.contains(&":config"), "missing :config");
	assert!(names.contains(&":list"), "missing :list");
	assert!(names.contains(&":init"), "missing :init");
	assert!(names.contains(&":mcp"), "missing :mcp");
	assert!(names.contains(&":completions"), "missing :completions");
	assert!(names.contains(&":generate"), "missing :generate");
	assert!(names.contains(&":convert"), "missing :convert");
	assert!(names.contains(&":env"), "missing :env");
}

#[test]
fn cli_does_not_have_utilities_subcommand() {
	let cmd = Cli::command();
	let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
	assert!(!names.contains(&":utilities"), ":utilities should no longer exist");
}

#[test]
fn mcp_has_subcommands() {
	let cmd = Cli::command();
	let mcp = find_subcommand(&cmd, ":mcp");
	let names: Vec<&str> = mcp.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"server"), "missing mcp server");
	assert!(names.contains(&"inspect"), "missing mcp inspect");
	assert!(names.contains(&"install"), "missing mcp install");
	assert_eq!(names.len(), 3, "unexpected mcp subcommands: {names:?}");
}

#[test]
fn mcp_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let mcp = find_subcommand(&cmd, ":mcp");
	for sub in mcp.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"mcp subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn completions_has_subcommands() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let names: Vec<&str> = completions.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"install"), "missing completions install");
	assert!(names.contains(&"uninstall"), "missing completions uninstall");
	assert!(names.contains(&"output"), "missing completions output");
	assert_eq!(names.len(), 3, "unexpected completions subcommands: {names:?}");
}

#[test]
fn completions_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	for sub in completions.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"completions subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn completions_install_requires_shell_arg() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let install = find_subcommand(completions, "install");
	let args: Vec<&str> = install.get_arguments().map(|a| a.get_id().as_str()).collect();
	assert!(args.contains(&"shell"), "completions install missing shell arg");
}

#[test]
fn completions_uninstall_requires_shell_arg() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let uninstall = find_subcommand(completions, "uninstall");
	let args: Vec<&str> = uninstall.get_arguments().map(|a| a.get_id().as_str()).collect();
	assert!(args.contains(&"shell"), "completions uninstall missing shell arg");
}

#[test]
fn completions_output_requires_shell_arg() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let output = find_subcommand(completions, "output");
	let args: Vec<&str> = output.get_arguments().map(|a| a.get_id().as_str()).collect();
	assert!(args.contains(&"shell"), "completions output missing shell arg");
}

#[test]
fn generate_has_subcommands() {
	let cmd = Cli::command();
	let generate = find_subcommand(&cmd, ":generate");
	let names: Vec<&str> = generate.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"zed-tasks"), "missing generate zed-tasks");
	assert!(
		names.contains(&"jetbrains-run-configurations"),
		"missing generate jetbrains-run-configurations"
	);
	assert!(names.contains(&"vscode-tasks"), "missing generate vscode-tasks");
	assert_eq!(names.len(), 3, "unexpected generate subcommands: {names:?}");
}

#[test]
fn generate_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let generate = find_subcommand(&cmd, ":generate");
	for sub in generate.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"generate subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn convert_has_subcommands() {
	let cmd = Cli::command();
	let convert = find_subcommand(&cmd, ":convert");
	let names: Vec<&str> = convert.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"makefile"), "missing convert makefile");
	assert!(names.contains(&"package-json"), "missing convert package-json");
	assert_eq!(names.len(), 2, "unexpected convert subcommands: {names:?}");
}

#[test]
fn convert_subcommands_have_descriptions() {
	let cmd = Cli::command();
	let convert = find_subcommand(&cmd, ":convert");
	for sub in convert.get_subcommands() {
		assert!(
			sub.get_about().is_some(),
			"convert subcommand '{}' missing description",
			sub.get_name()
		);
	}
}

#[test]
fn config_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let names: Vec<&str> = config.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"path-alias"), "missing config path-alias");
	assert!(names.contains(&"reset"), "missing config reset");
	assert!(names.contains(&"shell"), "missing config shell");
	assert!(names.contains(&"global-files"), "missing config global-files");
}

#[test]
fn config_shell_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let shell = find_subcommand(config, "shell");
	let names: Vec<&str> = shell.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"list"), "missing config shell list");
	assert!(names.contains(&"set"), "missing config shell set");
}

#[test]
fn config_path_alias_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let pa = find_subcommand(config, "path-alias");
	let names: Vec<&str> = pa.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"add"), "missing config path-alias add");
	assert!(names.contains(&"list"), "missing config path-alias list");
	assert!(names.contains(&"remove"), "missing config path-alias remove");
}

#[test]
fn config_global_files_has_subcommands() {
	let cmd = Cli::command();
	let config = find_subcommand(&cmd, ":config");
	let gf = find_subcommand(config, "global-files");
	let names: Vec<&str> = gf.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"add"), "missing config global-files add");
	assert!(names.contains(&"list"), "missing config global-files list");
	assert!(names.contains(&"remove"), "missing config global-files remove");
}

// ── CLI parse tests ──────────────────────────────────────────────

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

// ── help-text tests ──────────────────────────────────────────────

#[test]
fn top_level_help_does_not_panic() {
	let mut cmd = Cli::command();
	let mut buf = Vec::new();
	cmd.write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains(":config"));
	assert!(help.contains(":mcp"));
	assert!(help.contains(":completions"));
	assert!(help.contains(":generate"));
	assert!(help.contains(":convert"));
	assert!(!help.contains(":utilities"));
}

#[test]
fn mcp_help_shows_subcommands() {
	let cmd = Cli::command();
	let mcp = find_subcommand(&cmd, ":mcp");
	let mut buf = Vec::new();
	mcp.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("server"), "mcp help missing 'server'");
	assert!(help.contains("inspect"), "mcp help missing 'inspect'");
	assert!(help.contains("install"), "mcp help missing 'install'");
}

#[test]
fn completions_help_shows_subcommands() {
	let cmd = Cli::command();
	let completions = find_subcommand(&cmd, ":completions");
	let mut buf = Vec::new();
	completions.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("install"), "completions help missing 'install'");
	assert!(help.contains("uninstall"), "completions help missing 'uninstall'");
	assert!(help.contains("output"), "completions help missing 'output'");
}

#[test]
fn generate_help_shows_subcommands() {
	let cmd = Cli::command();
	let generate = find_subcommand(&cmd, ":generate");
	let mut buf = Vec::new();
	generate.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("zed-tasks"), "generate help missing 'zed-tasks'");
	assert!(
		help.contains("jetbrains-run-configurations"),
		"generate help missing 'jetbrains-run-configurations'"
	);
}

#[test]
fn convert_help_shows_subcommands() {
	let cmd = Cli::command();
	let convert = find_subcommand(&cmd, ":convert");
	let mut buf = Vec::new();
	convert.clone().write_help(&mut buf).unwrap();
	let help = String::from_utf8(buf).unwrap();
	assert!(help.contains("makefile"), "convert help missing 'makefile'");
	assert!(help.contains("package-json"), "convert help missing 'package-json'");
}

// ── list-subcommands tests ───────────────────────────────────────
// These test the tree navigation used by shell completion scripts

#[test]
fn list_subcommands_navigation_finds_mcp_subcommands() {
	let cmd = Cli::command();
	let mcp = cmd.find_subcommand(":mcp").unwrap();
	let names: Vec<&str> = mcp
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"server"));
	assert!(names.contains(&"inspect"));
	assert!(names.contains(&"install"));
}

#[test]
fn list_subcommands_navigation_finds_completions_subcommands() {
	let cmd = Cli::command();
	let completions = cmd.find_subcommand(":completions").unwrap();
	let names: Vec<&str> = completions
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"install"));
	assert!(names.contains(&"uninstall"));
	assert!(names.contains(&"output"));
}

#[test]
fn list_subcommands_navigation_finds_generate_subcommands() {
	let cmd = Cli::command();
	let generate = cmd.find_subcommand(":generate").unwrap();
	let names: Vec<&str> = generate
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"zed-tasks"));
	assert!(names.contains(&"jetbrains-run-configurations"));
}

#[test]
fn list_subcommands_navigation_finds_convert_subcommands() {
	let cmd = Cli::command();
	let convert = cmd.find_subcommand(":convert").unwrap();
	let names: Vec<&str> = convert
		.get_subcommands()
		.filter(|s| !s.is_hide_set())
		.map(|s| s.get_name())
		.collect();
	assert!(names.contains(&"makefile"));
	assert!(names.contains(&"package-json"));
}

#[test]
fn list_subcommands_navigation_config_shell_set() {
	// Verify 3-level navigation: :config -> shell -> set
	let cmd = Cli::command();
	let config = cmd.find_subcommand(":config").unwrap();
	let shell = config.find_subcommand("shell").unwrap();
	let set = shell.find_subcommand("set");
	assert!(set.is_some(), ":config.shell.set not found");
}

// ── completion script new-command references ─────────────────────

#[test]
fn bash_completion_detects_colon_subcommands_generically() {
	// The new bash script uses `:*) subcmd=...` to detect any colon-prefixed subcommand
	assert!(BASH_COMPLETION.contains(":*)"));
}

#[test]
fn fish_completion_references_mcp_subcommands() {
	assert!(FISH_COMPLETION.contains(":mcp"));
	assert!(FISH_COMPLETION.contains("server inspect install"));
}

#[test]
fn fish_completion_references_completions_subcommands() {
	assert!(FISH_COMPLETION.contains(":completions"));
	assert!(FISH_COMPLETION.contains("install uninstall output"));
}

#[test]
fn fish_completion_references_generate_subcommands() {
	assert!(FISH_COMPLETION.contains(":generate"));
	assert!(FISH_COMPLETION.contains("zed-tasks jetbrains-run-configurations"));
}

#[test]
fn fish_completion_references_convert_subcommands() {
	assert!(FISH_COMPLETION.contains(":convert"));
	assert!(FISH_COMPLETION.contains("makefile package-json"));
}

#[test]
fn fish_completion_does_not_reference_utilities() {
	assert!(
		!FISH_COMPLETION.contains(":utilities"),
		"fish completion should not reference removed :utilities"
	);
}

#[test]
fn bash_completion_does_not_reference_utilities() {
	assert!(
		!BASH_COMPLETION.contains(":utilities"),
		"bash completion should not reference removed :utilities"
	);
}

#[test]
fn powershell_completion_does_not_reference_utilities() {
	assert!(
		!POWERSHELL_COMPLETION.contains(":utilities"),
		"powershell completion should not reference removed :utilities"
	);
}

#[test]
fn powershell_completion_detects_colon_subcommands_generically() {
	// The new powershell script uses regex '^:' to detect colon-prefixed subcommands
	assert!(POWERSHELL_COMPLETION.contains("'^:'"));
}

#[test]
fn zsh_completion_does_not_reference_utilities() {
	assert!(
		!ZSH_COMPLETION.contains(":utilities"),
		"zsh completion should not reference removed :utilities"
	);
}

#[test]
fn zsh_completion_handles_subcommands_generically() {
	// The new zsh script uses `:*` pattern to handle any colon-prefixed subcommand
	assert!(ZSH_COMPLETION.contains(":*"));
}

// ── :env subcommand parsing ──────────────────────────────────────

#[test]
fn env_has_subcommands() {
	let cmd = Cli::command();
	let env = find_subcommand(&cmd, ":env");
	let names: Vec<&str> = env.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"init"), "missing env init");
	assert!(names.contains(&"inject"), "missing env inject");
	assert!(names.contains(&"secret-keys"), "missing env secret-keys");
	assert!(names.contains(&"get"), "missing env get");
	assert!(names.contains(&"set"), "missing env set");
	assert!(names.contains(&"decrypt"), "missing env decrypt");
	assert!(names.contains(&"encrypt"), "missing env encrypt");
}

#[test]
fn cli_parses_env_inject_default_file() {
	let cli = Cli::try_parse_from(["run", ":env", "inject", "--", "echo", "hello"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Inject { file, command },
		}) => {
			assert!(file.is_empty());
			assert_eq!(command, vec!["echo".to_string(), "hello".to_string()]);
		}
		_ => panic!("expected Env Inject"),
	}
}

#[test]
fn cli_parses_env_inject_with_files() {
	let cli = Cli::try_parse_from([
		"run",
		":env",
		"inject",
		"-f",
		".env",
		"-f",
		".env.local",
		"--",
		"node",
		"app.js",
	])
	.unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Inject { file, command },
		}) => {
			assert_eq!(file, vec![".env".to_string(), ".env.local".to_string()]);
			assert_eq!(command, vec!["node".to_string(), "app.js".to_string()]);
		}
		_ => panic!("expected Env Inject"),
	}
}

#[test]
fn cli_parses_env_inject_with_command_flags_after_dashdash() {
	// After `--`, hyphen-prefixed args belong to the command, not to runfile
	let cli = Cli::try_parse_from(["run", ":env", "inject", "--", "node", "--version"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Inject { command, .. },
		}) => {
			assert_eq!(command, vec!["node".to_string(), "--version".to_string()]);
		}
		_ => panic!("expected Env Inject"),
	}
}

#[test]
fn cli_rejects_env_inject_without_command() {
	assert!(try_parse(&["run", ":env", "inject"]).is_err());
	assert!(try_parse(&["run", ":env", "inject", "-f", ".env"]).is_err());
}

#[test]
fn cli_parses_env_init_defaults() {
	let cli = Cli::try_parse_from(["run", ":env", "init"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { path, plain, key },
		}) => {
			assert_eq!(path, ".env");
			assert!(!plain);
			assert!(key.is_none());
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_init_with_path() {
	let cli = Cli::try_parse_from(["run", ":env", "init", "-p", ".env.production"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { path, .. },
		}) => {
			assert_eq!(path, ".env.production");
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_init_plain() {
	let cli = Cli::try_parse_from(["run", ":env", "init", "--plain"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { plain, key, .. },
		}) => {
			assert!(plain);
			assert!(key.is_none());
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_init_with_key() {
	let cli = Cli::try_parse_from(["run", ":env", "init", "--key", "abc123"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { plain, key, .. },
		}) => {
			assert!(!plain);
			assert_eq!(key.as_deref(), Some("abc123"));
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_secret_keys_get_private() {
	let cli = Cli::try_parse_from(["run", ":env", "secret-keys", "get-private", "a1b2"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::SecretKeys {
				action: crate::SecretKeysAction::GetPrivate { partial },
			},
		}) => {
			assert_eq!(partial, "a1b2");
		}
		_ => panic!("expected Env SecretKeys GetPrivate"),
	}
}

#[test]
fn cli_rejects_env_secret_keys_get_private_without_arg() {
	assert!(try_parse(&["run", ":env", "secret-keys", "get-private"]).is_err());
}

#[test]
fn cli_parses_env_set_plain() {
	let cli = Cli::try_parse_from(["run", ":env", "set", "file", "VAR", "val", "--plain"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Set {
				file,
				var,
				value,
				plain,
			},
		}) => {
			assert_eq!(file, "file");
			assert_eq!(var, "VAR");
			assert_eq!(value.as_deref(), Some("val"));
			assert!(plain);
		}
		_ => panic!("expected Env Set"),
	}
}

#[test]
fn cli_parses_env_set_without_value() {
	// When VALUE is omitted, it parses as None and the runtime reads from stdin.
	let cli = Cli::try_parse_from(["run", ":env", "set", "file", "VAR"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Set {
				file,
				var,
				value,
				plain,
			},
		}) => {
			assert_eq!(file, "file");
			assert_eq!(var, "VAR");
			assert_eq!(value, None);
			assert!(!plain);
		}
		_ => panic!("expected Env Set"),
	}
}

// ── RUNFILE_TARGET env var tests ─────────────────────────────────
//
// Env vars are process-global, so all tests in this section serialize via
// `RUNFILE_TARGET_TEST_LOCK` to avoid clobbering each other.

use crate::runfile_helpers::{resolve_runfile_path, runfile_target_env, RUNFILE_TARGET_ENV_VAR};
use std::sync::Mutex;

static RUNFILE_TARGET_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Set or unset `RUNFILE_TARGET` for the duration of the closure. The lock
/// is acquired on entry and released on exit.
fn with_runfile_target<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
	let _guard = RUNFILE_TARGET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let prev = std::env::var(RUNFILE_TARGET_ENV_VAR).ok();
	match value {
		Some(v) => std::env::set_var(RUNFILE_TARGET_ENV_VAR, v),
		None => std::env::remove_var(RUNFILE_TARGET_ENV_VAR),
	}
	let result = f();
	match prev {
		Some(v) => std::env::set_var(RUNFILE_TARGET_ENV_VAR, v),
		None => std::env::remove_var(RUNFILE_TARGET_ENV_VAR),
	}
	result
}

#[test]
fn runfile_target_env_returns_none_when_unset() {
	with_runfile_target(None, || {
		assert!(runfile_target_env().is_none());
	});
}

#[test]
fn runfile_target_env_returns_path_when_set() {
	with_runfile_target(Some("custom/Runfile.json"), || {
		let result = runfile_target_env();
		assert_eq!(result.as_deref(), Some(std::path::Path::new("custom/Runfile.json")));
	});
}

#[test]
fn runfile_target_env_returns_none_when_set_empty() {
	// Empty string is treated as unset so users can clear the var without
	// having to unset it shell-wide.
	with_runfile_target(Some(""), || {
		assert!(runfile_target_env().is_none());
	});
}

#[test]
fn resolve_runfile_path_uses_env_var_when_no_flag() {
	let dir = tempfile::tempdir().unwrap();
	let runfile_path = dir.path().join("custom.runfile.json");
	fs::write(
		&runfile_path,
		r#"{"$schema":"v0","targets":{"hello":{"commands":["echo hi"]}}}"#,
	)
	.unwrap();

	with_runfile_target(Some(runfile_path.to_str().unwrap()), || {
		let resolved = resolve_runfile_path(None);
		// Compare canonicalized paths to avoid Windows path-prefix mismatches.
		let expected = std::fs::canonicalize(&runfile_path).unwrap();
		let resolved_canon = std::fs::canonicalize(&resolved).unwrap();
		assert_eq!(resolved_canon, expected);
	});
}

#[test]
fn resolve_runfile_path_explicit_flag_wins_over_env_var() {
	let dir = tempfile::tempdir().unwrap();
	let env_runfile = dir.path().join("env.runfile.json");
	let flag_runfile = dir.path().join("flag.runfile.json");
	fs::write(
		&env_runfile,
		r#"{"$schema":"v0","targets":{"a":{"commands":["echo a"]}}}"#,
	)
	.unwrap();
	fs::write(
		&flag_runfile,
		r#"{"$schema":"v0","targets":{"b":{"commands":["echo b"]}}}"#,
	)
	.unwrap();

	with_runfile_target(Some(env_runfile.to_str().unwrap()), || {
		let resolved = resolve_runfile_path(Some(&flag_runfile));
		let expected = std::fs::canonicalize(&flag_runfile).unwrap();
		let resolved_canon = std::fs::canonicalize(&resolved).unwrap();
		assert_eq!(resolved_canon, expected);
	});
}
