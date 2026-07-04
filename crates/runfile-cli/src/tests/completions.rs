use super::*;

// ── completions_install_profile ──────────────────────────────────

#[test]
fn install_profile_appends_marker_and_content() {
	let path = temp_file("install_append");
	fs::write(&path, "existing content\n").unwrap();

	completions_install_profile(
		"eval \"{{ run :completions output bash }}\"",
		&path,
		"# runfile completions",
	);

	let result = fs::read_to_string(&path).unwrap();
	let _ = fs::remove_file(&path);

	assert!(result.contains("existing content"));
	assert!(result.contains("# runfile completions"));
	assert!(result.contains("eval \"{{ run :completions output bash }}\""));
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
		"eval \"{{ run :completions output bash }}\"",
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
fn bash_completion_falls_back_to_files_for_leaf_args() {
	// A shared file-completion helper exists and is invoked when a subcommand
	// path has no further children (leaf) or the first word is a target name.
	assert!(BASH_COMPLETION.contains("_run_files"));
	// The leaf branch (empty children) falls back to files.
	assert!(BASH_COMPLETION.contains("_run_files\n    fi"));
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
fn zsh_completion_falls_back_to_files_for_leaf_args() {
	// The `rest` state uses `_files` when a subcommand path has no children.
	assert!(ZSH_COMPLETION.contains("_files"));
}

#[test]
fn fish_completion_disables_default_file_completions() {
	assert!(FISH_COMPLETION.contains("complete -c run -f"));
	assert!(FISH_COMPLETION.contains("--list-targets"));
	assert!(FISH_COMPLETION.contains("--list-subcommands"));
}

#[test]
fn fish_completion_reenables_files_for_leaf_args() {
	// A predicate function drives the force-files rule for positional args.
	assert!(FISH_COMPLETION.contains("__run_needs_files"));
	assert!(FISH_COMPLETION.contains("complete -c run -n '__run_needs_files' -F"));
}

#[test]
fn powershell_completion_registers_completer() {
	assert!(POWERSHELL_COMPLETION.contains("Register-ArgumentCompleter"));
	assert!(POWERSHELL_COMPLETION.contains("--list-targets"));
	assert!(POWERSHELL_COMPLETION.contains("--list-subcommands"));
}

#[test]
fn powershell_completion_falls_back_to_files_for_leaf_args() {
	// The leaf branch uses PowerShell's built-in filename completer.
	assert!(POWERSHELL_COMPLETION.contains("CompletionCompleters]::CompleteFilename"));
}
