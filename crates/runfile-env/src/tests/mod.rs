use crate::{
	EnvBuildParams, build_env, check_env_case_duplicates, collect_runfile_env, load_env_files, parse_env_file,
};
use std::collections::HashMap;
use tempfile::TempDir;

/// A no-op substitution function that returns the input unchanged.
fn no_substitute(input: &str, _env: &HashMap<String, String>) -> Result<String, String> {
	Ok(input.to_string())
}

/// Helper: case-insensitive lookup for PATH (Windows uses "Path", Unix uses "PATH").
fn get_path_value(env: &HashMap<String, String>) -> &str {
	env.iter()
		.find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
		.map(|(_, v)| v.as_str())
		.expect("PATH should be present in env")
}

// ══════════════════════════════════════════════════════════════════════
// parse_env_file tests
// ══════════════════════════════════════════════════════════════════════

mod build;
mod encryption;
mod load;
mod parse;
mod path;
