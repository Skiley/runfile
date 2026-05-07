use crate::args::RunArgs;
use runfile_parser::{CommandSpec, EnvValue};
use std::collections::HashMap;
use std::path::Path;

// Re-export the core env types and functions from runfile-env
pub use runfile_env::parse_env_file;
pub use runfile_env::EnvError as EnvFileError;

/// Convert an `Option<HashMap<String, EnvValue>>` to `Option<HashMap<String, String>>`.
pub fn convert_env_map(env: Option<&HashMap<String, EnvValue>>) -> Option<HashMap<String, String>> {
	env.map(|m| m.iter().map(|(k, v)| (k.clone(), v.to_env_string())).collect())
}

/// Create a substitution closure from RunArgs.
fn make_substitute(args: &RunArgs) -> impl Fn(&str, &HashMap<String, String>) -> Result<String, String> + '_ {
	|input: &str, env: &HashMap<String, String>| args.substitute(input, env).map_err(|e| e.to_string())
}

/// Load environment variables from env files, applying `{{ ARGS.* }}` and `{{ ENV.* }}`
/// substitution to file paths. Missing files are silently skipped. Parse errors are returned.
pub fn load_env_files(
	env_files: &[String],
	working_dir: &Path,
	args: &RunArgs,
	current_env: &HashMap<String, String>,
) -> Result<HashMap<String, String>, EnvFileError> {
	let substitute = make_substitute(args);
	runfile_env::load_env_files(env_files, working_dir, &substitute, current_env)
}

/// Build the complete environment variable map for a command execution.
/// Merge order (low → high): envFiles → env → current shell env (always wins) →
/// addToPath chain (prepended to PATH) → decryption.
/// If encrypted values are found, automatically resolves the decryption key via
/// `RUNFILE_ENCRYPTION_KEY` env var or public key matching against `available_private_keys`.
///
/// `working_dir` is the resolved `workingDirectory` (used for relative
/// `addToPath` entries and as the spawn dir). `env_files_base_dir` is the
/// source Runfile's parent directory (`{{ RUN.parent }}`) — relative
/// `envFiles` paths always resolve against this, regardless of
/// `workingDirectory`.
pub fn build_env(
	command_spec: &CommandSpec,
	working_dir: &Path,
	env_files_base_dir: &Path,
	args: &RunArgs,
	available_private_keys: Option<&[String]>,
) -> Result<HashMap<String, String>, EnvFileError> {
	build_env_with_base(
		command_spec,
		working_dir,
		env_files_base_dir,
		args,
		available_private_keys,
		None,
		None,
	)
}

/// Like [`build_env`] but lets the caller pass two pieces of ancestor state
/// for `@target` dependency invocations:
/// - `base_env`: the parent's already-resolved env, used as the substitution
///   base so `{{ ENV.X }}` inside the dep can reference parent contributions.
/// - `parent_add_to_path_chain`: ancestor `addToPath` layers in chain order
///   (outermost first). Re-prepended to PATH after the shell-env overlay so
///   the full chain reaches the dep's commands as
///   `[dep addToPath..., parent..., grandparent..., shell PATH]`.
#[allow(clippy::too_many_arguments)]
pub fn build_env_with_base(
	command_spec: &CommandSpec,
	working_dir: &Path,
	env_files_base_dir: &Path,
	args: &RunArgs,
	available_private_keys: Option<&[String]>,
	base_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: Option<&[Vec<String>]>,
) -> Result<HashMap<String, String>, EnvFileError> {
	let command_env = convert_env_map(command_spec.env.as_ref());

	let params = runfile_env::EnvBuildParams {
		env_files: command_spec.env_files.as_deref(),
		env: command_env.as_ref(),
		add_to_path: command_spec.add_to_path.as_deref(),
		working_dir,
		env_files_base_dir,
		available_private_keys,
		base_env,
		parent_add_to_path_chain,
	};

	let substitute = make_substitute(args);
	runfile_env::build_env(&params, &substitute)
}
