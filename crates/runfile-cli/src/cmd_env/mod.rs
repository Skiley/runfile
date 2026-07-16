use crate::ci_detect;
use runfile_settings::keyring_keys;
use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process;

/// Environment variable that supplies a default env file path for `:env inject`
/// and `:env decrypt` when no positional path is given. Set by the `setup`
/// GitHub Action when `env-file-source` is passed, so open-source repos can
/// keep their encrypted `.env` in a secret instead of committing the ciphertext.
pub const RUNFILE_ENV_FILE_TARGET_ENV_VAR: &str = "RUNFILE_ENV_FILE_TARGET";

/// Read [`RUNFILE_ENV_FILE_TARGET_ENV_VAR`] and return the path it points to,
/// if set to a non-empty value. Returns `None` otherwise.
pub fn env_file_target() -> Option<String> {
	std::env::var(RUNFILE_ENV_FILE_TARGET_ENV_VAR)
		.ok()
		.filter(|s| !s.is_empty())
}

// ══════════════════════════════════════════════════════════════════════
// Init
// ══════════════════════════════════════════════════════════════════════

/// Create a new .env file, optionally encrypted.
pub fn cmd_init(path: &str, plain: bool, key_partial: Option<&str>) {
	// Validate flag combination
	if plain && key_partial.is_some() {
		eprintln!("Error: --plain and --key cannot be used together.");
		process::exit(1);
	}

	let file_path = Path::new(path);
	if file_path.exists() {
		eprintln!("Error: file already exists: {path}");
		process::exit(1);
	}

	if plain {
		// Create a plain .env file
		let content = "# Environment variables\n\n";
		if let Err(e) = std::fs::write(file_path, content) {
			eprintln!("Error writing {path}: {e}");
			process::exit(1);
		}
		println!("Created {path} (plaintext, not encrypted).");
		return;
	}

	// Encrypted mode
	let auto_generated;
	let key_hex;

	if let Some(partial) = key_partial {
		// Match against existing keys by public key prefix
		let all_keys = keyring_keys::all_private_keys();
		key_hex = match runfile_crypto::find_private_key_by_public_prefix(partial, &all_keys) {
			Ok(k) => k,
			Err(e) => {
				eprintln!("Error: {e}");
				process::exit(1);
			}
		};
		auto_generated = false;
	} else {
		// Generate a new key
		key_hex = runfile_crypto::generate_key();
		match keyring_keys::add(&key_hex) {
			Ok(false) => {
				// Extremely unlikely: generated key already exists
				eprintln!("Error: generated key already exists. Try again.");
				process::exit(1);
			}
			Err(e) => {
				eprintln!("Error storing key: {e}");
				process::exit(1);
			}
			Ok(true) => {}
		}
		auto_generated = true;
	}

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	// Write the encrypted .env file with the public key header
	let content = format!(
		"{}={public_key}\n\n# Add your variables below\n\n",
		runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
	);
	if let Err(e) = std::fs::write(file_path, &content) {
		eprintln!("Error writing {path}: {e}");
		process::exit(1);
	}

	println!("Created {path} (encrypted).");
	println!();
	println!("  Public key: {public_key}");

	if auto_generated {
		println!();
		println!("A new private key was generated and added to your local settings.");
		println!();
		println!("To share this env file with teammates, they must import the same");
		println!("private key before they can decrypt or use it:");
		println!();
		println!("  1. Share the private key securely:");
		println!("     run :env secret-keys get-private {}...", &public_key[..8]);
		println!();
		println!("  2. They import it on their machine:");
		println!("     run :env secret-keys add");
		println!("     (then paste the private key when prompted)");
	}
}

mod crypt;
mod secret_keys;
pub use crypt::*;
pub use secret_keys::*;

/// Read a variable from an env file. Auto-detects encryption and decrypts if needed.
pub fn cmd_get(file: &str, var: &str) {
	let (pairs, _) = read_env_file(file);
	let env_map: HashMap<String, String> = pairs.iter().cloned().collect();

	let value = match env_map.get(var) {
		Some(v) => v.clone(),
		None => {
			eprintln!("Error: variable \"{var}\" not found in {file}");
			process::exit(1);
		}
	};

	if runfile_crypto::is_encrypted(&value) {
		// File is encrypted — resolve key and decrypt
		let key_hex = resolve_private_key_for_file(&env_map);
		match runfile_crypto::decrypt(&value, &key_hex) {
			Ok(plaintext) => println!("{plaintext}"),
			Err(e) => {
				eprintln!("Error decrypting {var}: {e}");
				process::exit(1);
			}
		}
	} else {
		println!("{value}");
	}
}

/// Read a value from stdin (until EOF), stripping a single trailing newline.
/// When stdin is a TTY, prints a usage hint to stderr first so users know the
/// terminator is Ctrl+D (Unix) / Ctrl+Z then Enter (Windows).
fn read_value_from_stdin() -> String {
	let mut stdin = std::io::stdin();
	if stdin.is_terminal() {
		eprintln!("Enter value, then press Ctrl+D (Unix) or Ctrl+Z then Enter (Windows):");
	}
	let mut buf = String::new();
	if let Err(e) = stdin.read_to_string(&mut buf) {
		eprintln!("Error reading value from stdin: {e}");
		process::exit(1);
	}
	if let Some(stripped) = buf.strip_suffix("\r\n") {
		buf.truncate(stripped.len());
	} else if let Some(stripped) = buf.strip_suffix('\n') {
		buf.truncate(stripped.len());
	}
	buf
}

/// Set a variable in an env file. Auto-detects encryption and encrypts if needed.
/// When `plain` is true, the value is stored as plaintext even if the file is encrypted.
/// When `value` is `None`, the value is read from stdin (until EOF), with a single
/// trailing newline stripped — useful to keep secrets out of shell history and to
/// pass values containing shell-special characters without escaping.
pub fn cmd_set(file: &str, var: &str, value: Option<&str>, plain: bool) {
	let stdin_value;
	let value = match value {
		Some(v) => v,
		None => {
			stdin_value = read_value_from_stdin();
			&stdin_value
		}
	};

	let path = Path::new(file);
	let content = if path.exists() {
		read_file_content(file)
	} else {
		String::new()
	};

	// Parse to check for RUNFILE_ENCRYPTION_PUBLIC_KEY
	let pairs = match runfile_env::parse_env_file(&content) {
		Ok(p) => p,
		Err((line, msg)) => {
			eprintln!("Error parsing {file} at line {line}: {msg}");
			process::exit(1);
		}
	};
	let env_map: HashMap<String, String> = pairs.into_iter().collect();

	let final_value = if !plain && env_map.contains_key(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		// File is encrypted — encrypt the value
		let key_hex = resolve_private_key_for_file(&env_map);
		match runfile_crypto::encrypt(value, &key_hex) {
			Ok(encrypted) => encrypted,
			Err(e) => {
				eprintln!("Error encrypting value: {e}");
				process::exit(1);
			}
		}
	} else {
		value.to_string()
	};

	let new_content = set_env_line(&content, var, &final_value);

	// 0600 on Unix — a `.env` written by `:env set` may hold plaintext secrets
	// (especially with `--plain`), so don't leave it group/other-readable.
	if let Err(e) = write_secret_file(path, new_content.as_bytes()) {
		eprintln!("Error writing {file}: {e}");
		process::exit(1);
	}

	println!("{var} set in {file}");
}

/// Write `content` to `path`, restricting the file to owner read/write only
/// (mode 0600) on Unix. Used for files that may hold secrets — decrypted `.env`
/// output and `:env set` rewrites — so they are not left group/other-readable
/// (the default `0644` under a typical umask) on a shared host. On non-Unix
/// platforms this is a plain write. The permission is set after the write so a
/// freshly-created file never has a wider-permission window.
pub(crate) fn write_secret_file(path: impl AsRef<Path>, content: &[u8]) -> std::io::Result<()> {
	let path = path.as_ref();
	std::fs::write(path, content)?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
	}
	Ok(())
}

/// Drops any key from `env_map` that the parent process already defines, so that
/// inherited values reach the child unmodified when `Command::envs(&env_map)` is
/// applied on top of the inherited environment. `parent_lookup` is injected so
/// tests can supply a fake parent env without mutating the real process env.
pub(crate) fn apply_parent_env_override<F>(env_map: &mut HashMap<String, String>, parent_lookup: F)
where
	F: Fn(&str) -> Option<std::ffi::OsString>,
{
	env_map.retain(|k, _| parent_lookup(k).is_none());
}

/// Resolves the PATH that `which::which_in` should search to mirror what the child
/// will actually see at exec time. With "parent env always wins" semantics, this
/// means: inherited PATH if the parent has one, otherwise fall back to the env
/// file's PATH (case-insensitive lookup so `path=` in a file is honored on
/// Windows). PATHEXT-aware lookup is required on Windows because Rust's
/// `Command::new` only appends `.exe` via `CreateProcessW` — `.cmd`/`.bat`/`.ps1`
/// shims like `node_modules/.bin/vite.cmd` need explicit resolution.
pub(crate) fn effective_inject_path<F>(env_map: &HashMap<String, String>, parent_path_lookup: F) -> String
where
	F: FnOnce() -> Option<String>,
{
	parent_path_lookup()
		.or_else(|| {
			env_map
				.iter()
				.find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
				.map(|(_, v)| v.clone())
		})
		.unwrap_or_default()
}

/// Run a command with environment variables loaded from one or more .env files,
/// auto-decrypting encrypted values.
///
/// Precedence: later files override earlier ones, but the parent process
/// environment ALWAYS wins — any var inherited from the parent shell shadows
/// the file-loaded value of the same name.
pub fn cmd_inject(files: &[String], command_args: &[String]) {
	if command_args.is_empty() {
		eprintln!("Error: no command provided.");
		eprintln!("Usage: run :env inject [-f <file>]... -- <command> [args...]");
		process::exit(1);
	}

	// File resolution order:
	//   1. Explicit positional paths (one or more)
	//   2. RUNFILE_ENV_FILE_TARGET — set by the setup action's `env-file-source` so
	//      open-source repos can keep their encrypted .env in a secret
	//   3. Error — there's no implicit `.env` fallback; the user must opt in
	//
	// Once a source is chosen, missing files are a hard error (no silent skipping).
	let env_target;
	let files_to_load: Vec<&str> = if !files.is_empty() {
		files.iter().map(String::as_str).collect()
	} else {
		match env_file_target() {
			Some(t) => {
				env_target = t;
				vec![env_target.as_str()]
			}
			None => {
				eprintln!(
					"Error: no env file specified.\n\
					 Usage: run :env inject <file>... -- <command> [args...]\n\
					 (Or set {RUNFILE_ENV_FILE_TARGET_ENV_VAR} to provide one.)"
				);
				process::exit(1);
			}
		}
	};

	let mut env_map: HashMap<String, String> = HashMap::new();
	for file in &files_to_load {
		let path = Path::new(file);
		if !path.exists() {
			eprintln!("Error: file not found: {file}");
			process::exit(1);
		}
		let content = read_file_content(file);
		let pairs = match runfile_env::parse_env_file(&content) {
			Ok(p) => p,
			Err((line, msg)) => {
				eprintln!("Error parsing {file} at line {line}: {msg}");
				process::exit(1);
			}
		};
		for (k, v) in pairs {
			env_map.insert(k, v);
		}
	}

	if runfile_crypto::has_encrypted_values(&env_map) {
		let key_hex = match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
			Some(public_key) => resolve_private_key_by_public(public_key),
			None => {
				eprintln!(
					"Error: encrypted values found but {0} is missing. \
					 Re-create the file via `run :env init` / `run :env encrypt`, or add a {0} line above the encrypted values.",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				);
				process::exit(1);
			}
		};
		if let Err(e) = runfile_crypto::decrypt_env_values(&mut env_map, &key_hex) {
			eprintln!("Error decrypting env values: {e}");
			process::exit(1);
		}
	}

	env_map.remove(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR);

	apply_parent_env_override(&mut env_map, |k| std::env::var_os(k));

	let program = &command_args[0];
	let args = &command_args[1..];

	let effective_path = effective_inject_path(&env_map, || std::env::var("PATH").ok());
	let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
	let resolved = which::which_in(program, Some(&effective_path), &cwd);

	let mut cmd = match resolved {
		Ok(path) => std::process::Command::new(path),
		Err(_) => std::process::Command::new(program),
	};
	cmd.args(args);
	cmd.envs(&env_map);

	let status = match cmd.status() {
		Ok(s) => s,
		Err(e) => {
			eprintln!("Error running {program}: {e}");
			process::exit(127);
		}
	};

	process::exit(status.code().unwrap_or(1));
}

// ══════════════════════════════════════════════════════════════════════
// Key rotation
// ══════════════════════════════════════════════════════════════════════

/// Rotate the encryption key for an encrypted env file.
/// Generates a new key, decrypts all values with the old key, re-encrypts with the new key,
/// and updates the file in place. Optionally deletes the old key from the OS credential store.
fn read_file_content(file: &str) -> String {
	match std::fs::read_to_string(file) {
		Ok(c) => c,
		Err(e) => {
			eprintln!("Error reading {file}: {e}");
			process::exit(1);
		}
	}
}

fn read_env_file(file: &str) -> (Vec<(String, String)>, String) {
	let path = Path::new(file);
	if !path.exists() {
		eprintln!("Error: file not found: {file}");
		process::exit(1);
	}
	let content = read_file_content(file);
	let pairs = match runfile_env::parse_env_file(&content) {
		Ok(p) => p,
		Err((line, msg)) => {
			eprintln!("Error parsing {file} at line {line}: {msg}");
			process::exit(1);
		}
	};
	(pairs, content)
}

/// Resolve the private key for an env file by its RUNFILE_ENCRYPTION_PUBLIC_KEY.
fn resolve_private_key_for_file(env_map: &HashMap<String, String>) -> String {
	match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		Some(public_key) => resolve_private_key_by_public(public_key),
		None => {
			eprintln!(
				"Error: file does not contain {}",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			);
			process::exit(1);
		}
	}
}

/// Find a private key that matches the given public key.
///
/// Pulls from `keyring_keys::all_private_keys()` — which already merges
/// `RUNFILE_PRIVATE_KEYS` (env-supplied pool) with the OS credential store —
/// so no extra CI-specific branching is needed here.
fn resolve_private_key_by_public(public_key: &str) -> String {
	let all_keys = keyring_keys::all_private_keys();
	match runfile_crypto::find_matching_private_key(public_key, &all_keys) {
		Some(key) => key,
		None => {
			eprintln!(
				"Error: no private key matches public key {public_key}.\n\
				 Set RUNFILE_PRIVATE_KEYS or run `run :env secret-keys add` to add the correct key."
			);
			process::exit(1);
		}
	}
}

/// Replace or append a VAR=value line in env file content.
/// Preserves comments, blank lines, and formatting.
pub(crate) fn set_env_line(content: &str, var: &str, value: &str) -> String {
	let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
	let prefix_plain = format!("{var}=");
	let prefix_export = format!("export {var}=");

	let mut found = false;
	for line in &mut lines {
		let trimmed = line.trim();
		if trimmed.starts_with(&prefix_plain) || trimmed.starts_with(&prefix_export) {
			let has_export = trimmed.starts_with("export ");
			if has_export {
				*line = format!("export {var}={value}");
			} else {
				*line = format!("{var}={value}");
			}
			found = true;
			break;
		}
	}

	if !found {
		if !lines.is_empty() && !lines.last().is_none_or(|l| l.is_empty()) {
			lines.push(String::new());
		}
		lines.push(format!("{var}={value}"));
	}

	let mut result = lines.join("\n");
	if !result.ends_with('\n') {
		result.push('\n');
	}
	result
}
