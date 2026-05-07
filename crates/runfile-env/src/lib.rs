mod parse;

pub use parse::*;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

// Re-export crypto utilities for convenience
pub use runfile_crypto::has_encrypted_values;
pub use runfile_crypto::is_encrypted;

#[derive(Debug, Error)]
pub enum EnvError {
	#[error("Failed to read env file \"{0}\": {1}")]
	ReadError(String, std::io::Error),

	#[error("Failed to parse env file \"{path}\" at line {line}: {message}")]
	ParseError { path: String, line: usize, message: String },

	#[error("{0}")]
	Substitution(String),

	#[error(
		"Duplicate environment variable with different casing: \"{0}\" and \"{1}\". Use a single consistent casing."
	)]
	DuplicateEnvCasing(String, String),

	#[error("Encryption error: {0}")]
	Encryption(String),
}

/// Input parameters for building the complete environment variable map.
///
/// All env values should already be converted to strings (e.g. via `EnvValue::to_env_string()`).
/// The caller is responsible for converting non-string types before passing them here.
pub struct EnvBuildParams<'a> {
	/// Env file paths to load (in order; later files override earlier).
	pub env_files: Option<&'a [String]>,
	/// Env vars to set (applied after env files).
	pub env: Option<&'a HashMap<String, String>>,
	/// Directories to prepend to PATH. Entries should already be absolute —
	/// the parser bakes target-level relative `addToPath` entries against the
	/// source Runfile's directory in `merge.rs`, mirroring how globals are
	/// baked. The `working_dir` fallback in `apply_add_to_path_chain` only
	/// kicks in for any stray relative entry that bypassed baking.
	pub add_to_path: Option<&'a [String]>,
	/// Working directory the spawned command will run in (= the resolved
	/// `workingDirectory`). Used as a fallback for any relative `addToPath`
	/// entry that wasn't baked at parse time; not used for `envFiles`.
	pub working_dir: &'a Path,
	/// Base directory for resolving relative `envFiles` paths. Always the
	/// source Runfile's parent directory (`{{ RUN.parent }}`), regardless of
	/// `workingDirectory` — env files are configuration files co-located with
	/// the Runfile, so anchoring them to the Runfile dir is what users expect
	/// when they tweak `workingDirectory` for command execution.
	pub env_files_base_dir: &'a Path,
	/// Available private keys for decrypting `encrypted:` prefixed values.
	/// After merging, if encrypted values are detected, the key is resolved by:
	/// 1. `RUNFILE_ENCRYPTION_KEY` env var (for CI/CD)
	/// 2. Auto-matching `RUNFILE_ENCRYPTION_PUBLIC_KEY` against these private keys
	pub available_private_keys: Option<&'a [String]>,
	/// Optional override for the env-var base. When `Some`, this map replaces
	/// the default `std::env::vars()` snapshot as the starting layer of the
	/// merged env. Used to pass a parent target's already-resolved env into a
	/// dependency invocation, so `@dep` sees the parent's env on top of which
	/// it layers its own envFiles/env. When `None` (the default), the process's
	/// environment is used.
	pub base_env: Option<&'a HashMap<String, String>>,
	/// Accumulated `addToPath` contributions from ancestor `@target` callers,
	/// in chain order (outermost first). The current target's own `add_to_path`
	/// is appended internally, then the whole chain is prepended to PATH at the
	/// end so the innermost (this target's) entries end up at the very front:
	/// `[this..., parent..., grandparent..., shell PATH]`. None or empty for
	/// top-level invocations.
	pub parent_add_to_path_chain: Option<&'a [Vec<String>]>,
}

/// Load environment variables from env files, applying substitution to file paths.
/// Missing files are silently skipped. Parse errors are returned.
///
/// The `substitute` function is called on each file path template with the current
/// environment, allowing `{{ ARGS.* }}` and `{{ ENV.* }}` expansion in paths.
#[allow(clippy::type_complexity)]
pub fn load_env_files(
	env_files: &[String],
	working_dir: &Path,
	substitute: &dyn Fn(&str, &HashMap<String, String>) -> Result<String, String>,
	current_env: &HashMap<String, String>,
) -> Result<HashMap<String, String>, EnvError> {
	let mut result = HashMap::new();

	for file_template in env_files {
		// Substitute {{ ARGS.* }} and {{ ENV.* }} in the file path
		let file_path_str = substitute(file_template, current_env).map_err(EnvError::Substitution)?;

		// Resolve relative to working directory
		let file_path = if Path::new(&file_path_str).is_absolute() {
			PathBuf::from(&file_path_str)
		} else {
			working_dir.join(&file_path_str)
		};

		// Skip if file doesn't exist
		if !file_path.exists() {
			continue;
		}

		// Read and parse
		let content =
			fs::read_to_string(&file_path).map_err(|e| EnvError::ReadError(file_path.display().to_string(), e))?;

		let pairs = parse_env_file(&content).map_err(|(_line, message)| EnvError::ParseError {
			path: file_path.display().to_string(),
			line: _line,
			message,
		})?;

		for (key, value) in pairs {
			result.insert(key, value);
		}
	}

	Ok(result)
}

/// Build the complete environment variable map for a command execution.
///
/// Merge order (lowest → highest priority for non-PATH vars):
/// 1. `envFiles` — loaded left-to-right, later files override earlier
/// 2. `env` — with substitution; overrides envFiles per key
/// 3. **Current shell env** — `std::env::vars()` re-overlaid; the inherited shell
///    value ALWAYS beats whatever the Runfile's `envFiles` / `env` set
/// 4. `addToPath` chain — for PATH only, prepended in innermost-first order
///    (`[this target's addToPath..., parent's..., grandparent's..., shell PATH]`)
/// 5. Decryption — `encrypted:` values rewritten in place
///
/// For top-level invocations (`base_env: None`), step 1 starts from
/// `std::env::vars()` so `{{ ENV.X }}` substitution sees the inherited shell
/// values. The system-env re-overlay in step 3 is what actually ENFORCES
/// shell-wins on the final env (Runfile overrides during step 2 are undone for
/// any key the shell defines).
///
/// For dependency invocations (`base_env: Some(parent's resolved env)`),
/// step 1 starts from the parent's resolved env, so the dep inherits parent's
/// Runfile-defined values (those that survived shell-wins in the parent build)
/// and `{{ ENV.X }}` in the dep can reference them. Step 3 still re-overlays
/// `std::env::vars()`, ensuring shell wins over both parent and dep
/// contributions. Step 4 walks `parent_add_to_path_chain` plus this target's
/// `addToPath` so the full chain is re-prepended after step 3 wiped PATH.
///
/// The `substitute` function is called on env values and file paths, allowing
/// `{{ ARGS.* }}`, `{{ FLAGS.* }}`, and `{{ ENV.* }}` expansion.
#[allow(clippy::type_complexity)]
pub fn build_env(
	params: &EnvBuildParams<'_>,
	substitute: &dyn Fn(&str, &HashMap<String, String>) -> Result<String, String>,
) -> Result<HashMap<String, String>, EnvError> {
	let mut env_map: HashMap<String, String> = match params.base_env {
		Some(base) => base.clone(),
		None => env::vars().collect(),
	};

	// Layer envFiles (substitution sees the env_map built so far). Relative
	// envFiles paths resolve against `env_files_base_dir` — the source
	// Runfile's parent — NOT the resolved `workingDirectory`. Env files are
	// configuration co-located with the Runfile.
	if let Some(env_files) = params.env_files {
		let file_vars = load_env_files(env_files, params.env_files_base_dir, substitute, &env_map)?;
		env_map.extend(file_vars);
	}

	// Decrypt encrypted file-loaded values BEFORE the env block runs so that
	// `{{ ENV.SECRET }}` references inside an `env` block see the decrypted
	// plaintext (e.g. so `base64_decode(ENV.X)` works on a value that's both
	// Runfile-encrypted in the file AND base64-encoded). Without this, the
	// env block would see the literal `encrypted:abc...` form and any
	// post-processing would error.
	//
	// `RUNFILE_ENCRYPTION_PUBLIC_KEY` is read from `env_map`, which already
	// contains the system env (base_env), so a key set in the shell env
	// works the same as one set in the env file. Any decrypted value can
	// still be overwritten by `overlay_shell_env` below — the shell wins
	// for keys it defines.
	if runfile_crypto::has_encrypted_values(&env_map) {
		let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
		runfile_crypto::decrypt_env_values(&mut env_map, &key_hex).map_err(|e| EnvError::Encryption(e.to_string()))?;
	}

	// Layer env vars (substitution sees the env_map built so far; same-key
	// values override the file layer at this stage — though shell will win in
	// the next step). At this point any encrypted file values have been
	// decrypted, so substitutions like `{{ base64_decode(ENV.X) }}` work
	// without the user having to think about decryption ordering.
	if let Some(env_vars) = params.env {
		for (key, raw) in env_vars {
			let resolved = substitute(raw, &env_map).map_err(EnvError::Substitution)?;
			env_map.insert(key.clone(), resolved);
		}
	}

	// Re-overlay the current shell env. Any key the shell defines now beats
	// whatever envFiles/env set, restoring the inherited value. PATH is
	// case-aware (Windows uses "Path", Unix "PATH") so we don't end up with
	// two case-different PATH keys.
	overlay_shell_env(&mut env_map);

	// Build the full addToPath chain (parent ancestors + this target) and
	// prepend to PATH. After the shell-env overlay, PATH = shell's PATH (if
	// any), so this re-prepends the entire chain on top.
	apply_add_to_path_chain(
		&mut env_map,
		params.parent_add_to_path_chain,
		params.add_to_path,
		params.working_dir,
	);

	// Final decrypt pass: if the env block (or shell overlay) somehow
	// introduced an `encrypted:...` value — uncommon but possible — make
	// sure it doesn't leak through to the child process.
	if runfile_crypto::has_encrypted_values(&env_map) {
		let key_hex = resolve_decryption_key(&env_map, params.available_private_keys)?;
		runfile_crypto::decrypt_env_values(&mut env_map, &key_hex).map_err(|e| EnvError::Encryption(e.to_string()))?;
	}

	Ok(env_map)
}

/// Re-overlay `std::env::vars()` so the inherited shell env wins per key.
/// Handles PATH's case-insensitive identity on Windows: if env_map already
/// contains a case-insensitive PATH match, the system's PATH value is written
/// to that existing key rather than introducing a duplicate "Path"/"PATH"
/// pair that would later confuse `Command::envs`.
fn overlay_shell_env(env_map: &mut HashMap<String, String>) {
	let existing_path_key = env_map.keys().find(|k| k.eq_ignore_ascii_case("PATH")).cloned();
	for (k, v) in env::vars() {
		if k.eq_ignore_ascii_case("PATH") {
			let target = existing_path_key.clone().unwrap_or(k);
			env_map.insert(target, v);
		} else {
			env_map.insert(k, v);
		}
	}
}

/// Prepend `parent_chain + [this target's add_to_path]` to PATH so the
/// innermost (this target's) entries end up at the very front. Relative paths
/// resolve against `working_dir`. No-op when both inputs are empty.
fn apply_add_to_path_chain(
	env_map: &mut HashMap<String, String>,
	parent_chain: Option<&[Vec<String>]>,
	this_target: Option<&[String]>,
	working_dir: &Path,
) {
	let parent_layers = parent_chain.unwrap_or(&[]);
	let this_layer: &[String] = this_target.unwrap_or(&[]);
	if parent_layers.iter().all(|l| l.is_empty()) && this_layer.is_empty() {
		return;
	}

	let path_key = env_map
		.keys()
		.find(|k| k.eq_ignore_ascii_case("PATH"))
		.cloned()
		.unwrap_or_else(|| "PATH".to_string());
	let current_path = env_map.get(&path_key).cloned().unwrap_or_default();
	let separator = if cfg!(windows) { ";" } else { ":" };

	let resolve = |p: &String| -> String {
		let path = PathBuf::from(p);
		if path.is_absolute() {
			path.to_string_lossy().to_string()
		} else {
			working_dir.join(p).to_string_lossy().to_string()
		}
	};

	// Innermost first (this target), then walk the parent chain in reverse so
	// outer ancestors land further back, closer to shell PATH at the tail.
	let mut new_paths: Vec<String> = this_layer.iter().map(&resolve).collect();
	for layer in parent_layers.iter().rev() {
		new_paths.extend(layer.iter().map(&resolve));
	}
	if !current_path.is_empty() {
		new_paths.push(current_path);
	}

	env_map.insert(path_key, new_paths.join(separator));
}

/// Resolve the private key for decrypting encrypted env values.
///
/// Resolution order:
/// 1. `RUNFILE_ENCRYPTION_KEY` env var in the env map (for CI/CD)
/// 2. `RUNFILE_ENCRYPTION_PUBLIC_KEY` in the env map → match against available private keys
/// 3. Error if no key can be resolved
fn resolve_decryption_key(
	env_map: &HashMap<String, String>,
	available_private_keys: Option<&[String]>,
) -> Result<String, EnvError> {
	// 1. Check RUNFILE_ENCRYPTION_KEY in the merged env (includes system env)
	if let Some(key) = env_map.get("RUNFILE_ENCRYPTION_KEY") {
		if !key.is_empty() {
			// Validate format: must be 64 hex chars
			if key.len() != 64 || hex::decode(key).is_err() {
				return Err(EnvError::Encryption(
					"RUNFILE_ENCRYPTION_KEY must be a 64-character hex string (256-bit AES key).".to_string(),
				));
			}
			// If RUNFILE_ENCRYPTION_PUBLIC_KEY is also present, verify the key matches
			if let Some(public_key) = env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
				if let Ok(derived) = runfile_crypto::derive_public_key(key) {
					if derived != *public_key {
						return Err(EnvError::Encryption(format!(
							"RUNFILE_ENCRYPTION_KEY does not match {}. \
							 The provided key's fingerprint ({}) differs from the expected fingerprint ({}).",
							runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR,
							&derived[..16],
							&public_key[..public_key.len().min(16)],
						)));
					}
				}
			}
			return Ok(key.clone());
		}
	}

	// 2. Check RUNFILE_ENCRYPTION_PUBLIC_KEY and match against available private keys
	if let Some(public_key) = env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		if let Some(private_keys) = available_private_keys {
			if let Some(matched) = runfile_crypto::find_matching_private_key(public_key, private_keys) {
				return Ok(matched);
			}
			return Err(EnvError::Encryption(format!(
				"Found {} in env but no matching private key is configured. \
				 Run `run :env secret-keys add` to add a key.",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			)));
		}
		return Err(EnvError::Encryption(format!(
			"Found {} in env but no private keys are available. \
			 Set RUNFILE_ENCRYPTION_KEY env var or configure keys via `run :env secret-keys add`.",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
		)));
	}

	Err(EnvError::Encryption(
		"Encrypted env values found but no encryption key available. \
		 Set RUNFILE_ENCRYPTION_KEY env var or ensure env files contain RUNFILE_ENCRYPTION_PUBLIC_KEY."
			.to_string(),
	))
}

/// Check for duplicate env var keys with different casing.
/// Returns an error if e.g. both "NODE_ENV" and "node_env" are defined.
pub fn check_env_case_duplicates(env: &HashMap<String, String>) -> Result<(), EnvError> {
	let mut seen: HashMap<String, String> = HashMap::new(); // lowercase -> original
	for key in env.keys() {
		let lower = key.to_lowercase();
		if let Some(existing) = seen.get(&lower) {
			if existing != key {
				return Err(EnvError::DuplicateEnvCasing(existing.clone(), key.clone()));
			}
		} else {
			seen.insert(lower, key.clone());
		}
	}
	Ok(())
}

/// Collect only the env vars explicitly set by the Runfile.
/// Returns them in a deterministic order (sorted by key).
///
/// This does NOT include system env vars — only the vars defined in the Runfile.
pub fn collect_runfile_env(env: Option<&HashMap<String, String>>) -> Vec<(String, String)> {
	let mut pairs: Vec<(String, String)> = match env {
		Some(e) => e.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
		None => Vec::new(),
	};
	pairs.sort_by(|a, b| a.0.cmp(&b.0));
	pairs
}

#[cfg(test)]
mod tests;
