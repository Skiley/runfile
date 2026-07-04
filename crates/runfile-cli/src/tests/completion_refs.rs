use super::*;

// ── completion script new-command references ─────────────────────

#[test]
fn bash_completion_detects_colon_subcommands_generically() {
	// The bash script distinguishes ':'-prefixed subcommands from target names
	// via a `:*` glob so any colon-prefixed subcommand is handled generically.
	assert!(BASH_COMPLETION.contains("!= :*"));
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
	// The powershell script distinguishes ':'-prefixed subcommands from target
	// names via a `-notlike ':*'` check so any colon-prefixed subcommand works.
	assert!(POWERSHELL_COMPLETION.contains("-notlike ':*'"));
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
