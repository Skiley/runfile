use crate::Cli;
use crate::completions::{
	BASH_COMPLETION, FISH_COMPLETION, POWERSHELL_COMPLETION, ZSH_COMPLETION, completions_install_profile,
	completions_uninstall_profile,
};
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

/// Test helper: find a subcommand by name on a clap Command, panicking if absent.
fn find_subcommand<'a>(cmd: &'a clap::Command, name: &str) -> &'a clap::Command {
	cmd.get_subcommands()
		.find(|s| s.get_name() == name)
		.unwrap_or_else(|| panic!("subcommand '{name}' not found"))
}

mod ci_detect;
mod cli_parse;
mod cli_structure;
mod cmd_env;
mod cmd_run;
mod cmd_update;
mod cmd_utilities;
mod completion_refs;
mod completions;
mod env_file_target;
mod env_parsing;
mod help;
mod list_subcommands;
mod runfile_target;
