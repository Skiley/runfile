use crate::ci_detect;
use runfile_settings::keyring_keys;
use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process;

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

// ══════════════════════════════════════════════════════════════════════
// Secret key management
// ══════════════════════════════════════════════════════════════════════

/// CI-only path for `:env secret-keys add --key <hex>`.
///
/// Refuses to run outside a CI environment so dev-machine users don't accidentally bake
/// their private key into shell history.
fn add_secret_key_non_interactive(key_hex: &str) {
	if !ci_detect::is_ci() {
		eprintln!(
			"Error: --key is only allowed inside a CI environment.\n\
			 Run `run :env secret-keys add` interactively on dev machines so the key\n\
			 doesn't end up in your shell history."
		);
		process::exit(1);
	}

	let key_hex = key_hex.trim().to_string();
	if key_hex.len() != 64 || hex::decode(&key_hex).is_err() {
		eprintln!("Error: --key must be a 64-character hex string (256-bit AES key).");
		process::exit(1);
	}

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	match keyring_keys::add(&key_hex) {
		Ok(false) => {
			println!("Key already registered (public {public_key}).");
			return;
		}
		Err(e) => {
			eprintln!("Error storing key: {e}");
			process::exit(1);
		}
		Ok(true) => {}
	}

	println!("Private key added (public {public_key}).");
}

/// Add a new private key.
///
/// If `key_arg` is `Some`, the key is added non-interactively. This path is gated to
/// CI environments only — running it on a dev machine would risk leaking the private
/// key into shell history.
///
/// If `key_arg` is `None`, prompts the user to either generate a new key or paste an
/// existing one.
pub fn cmd_secret_keys_add(key_arg: Option<&str>) {
	use std::io::{self, BufRead, Write};

	if let Some(key_hex) = key_arg {
		add_secret_key_non_interactive(key_hex);
		return;
	}

	// Prompt user for choice
	eprintln!("How would you like to add a secret key?");
	eprintln!();
	eprintln!("  1) Generate a new private key");
	eprintln!("  2) Import an existing private key");
	eprintln!();
	eprint!("Enter choice (1 or 2): ");
	io::stderr().flush().unwrap_or(());

	let stdin = io::stdin();
	let mut choice = String::new();
	if stdin.lock().read_line(&mut choice).is_err() {
		eprintln!("Error reading input.");
		process::exit(1);
	}

	let key_hex = match choice.trim() {
		"1" => runfile_crypto::generate_key(),
		"2" => {
			eprint!("Paste your private key (64-character hex string): ");
			io::stderr().flush().unwrap_or(());

			let mut key_input = String::new();
			if stdin.lock().read_line(&mut key_input).is_err() {
				eprintln!("Error reading input.");
				process::exit(1);
			}
			let k = key_input.trim().to_string();
			if k.len() != 64 || hex::decode(&k).is_err() {
				eprintln!("Error: key must be a 64-character hex string (256-bit AES key).");
				process::exit(1);
			}
			k
		}
		_ => {
			eprintln!("Invalid choice. Please enter 1 or 2.");
			process::exit(1);
		}
	};

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	match keyring_keys::add(&key_hex) {
		Ok(false) => {
			eprintln!("Key already exists.");
			process::exit(1);
		}
		Err(e) => {
			eprintln!("Error storing key: {e}");
			process::exit(1);
		}
		Ok(true) => {}
	}

	println!();
	println!("Private key added.");
	println!("  Stored in: OS credential store");
	println!("  Public:    {public_key}");
	println!();
	println!("Add this to your encrypted .env files:");
	println!("  {}={public_key}", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR);
}

/// List all stored keys showing public key fingerprints.
pub fn cmd_secret_keys_list() {
	let fingerprints = match keyring_keys::list_fingerprints() {
		Ok(fps) => fps,
		Err(e) => {
			eprintln!("Error reading keyring: {e}");
			process::exit(1);
		}
	};

	if fingerprints.is_empty() {
		println!("No secret keys configured.");
		return;
	}

	for fingerprint in &fingerprints {
		println!("  {fingerprint}  (secure: OS credential store)");
	}
}

/// Remove a private key by matching public key prefix.
pub fn cmd_secret_keys_remove(public_prefix: &str) {
	let fingerprint = match keyring_keys::resolve_prefix(public_prefix) {
		Ok(fp) => fp,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	match keyring_keys::remove(&fingerprint) {
		Ok(true) => {}
		Ok(false) => {
			eprintln!("Error: key not found.");
			process::exit(1);
		}
		Err(e) => {
			eprintln!("Error removing key: {e}");
			process::exit(1);
		}
	}

	println!("Key removed (public: {fingerprint}).");
}

/// Print the full private key for sharing with teammates.
/// Takes a public key prefix to identify which key to print.
pub fn cmd_get_private_key(public_prefix: &str) {
	let all_keys = keyring_keys::all_private_keys();
	let matched = match runfile_crypto::find_private_key_by_public_prefix(public_prefix, &all_keys) {
		Ok(k) => k,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	let public_key = runfile_crypto::derive_public_key(&matched).unwrap_or_else(|_| "???".to_string());

	println!("{matched}");
	eprintln!();
	eprintln!("  Public key: {public_key}");
	eprintln!();
	eprintln!("To import this key on another machine:");
	eprintln!("  run :env secret-keys add");
	eprintln!("  (then paste the private key when prompted)");
}

// ══════════════════════════════════════════════════════════════════════
// File operations: get / set / encrypt / decrypt
// ══════════════════════════════════════════════════════════════════════

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

	if let Err(e) = std::fs::write(path, &new_content) {
		eprintln!("Error writing {file}: {e}");
		process::exit(1);
	}

	println!("{var} set in {file}");
}

/// Decrypt an encrypted env file. Writes to `output` if provided, otherwise prints to stdout.
pub fn cmd_decrypt_file(source: &str, output: Option<&str>) {
	let (pairs, _) = read_env_file(source);
	let env_map: HashMap<String, String> = pairs.iter().cloned().collect();

	let public_key = match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		Some(pk) => pk.clone(),
		None => {
			eprintln!(
				"Error: {source} does not contain {} — not an encrypted file",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			);
			process::exit(1);
		}
	};

	let key_hex = resolve_private_key_by_public(&public_key);

	// Build output content: decrypt encrypted values, skip the public key line
	let content = read_file_content(source);
	let mut out_lines = Vec::new();

	for line in content.lines() {
		let trimmed = line.trim();
		// Skip the public key line
		if trimmed.starts_with(&format!("{}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
			|| trimmed.starts_with(&format!("export {}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
		{
			continue;
		}
		// If it's a key=value line with an encrypted value, decrypt it
		if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("//") {
			if let Some(eq_pos) = trimmed.find('=') {
				let val_part = &trimmed[eq_pos + 1..];
				let val_trimmed = val_part.trim();
				// Strip quotes if present
				let val_unquoted = if (val_trimmed.starts_with('"') && val_trimmed.ends_with('"'))
					|| (val_trimmed.starts_with('\'') && val_trimmed.ends_with('\''))
				{
					&val_trimmed[1..val_trimmed.len() - 1]
				} else {
					val_trimmed
				};
				if runfile_crypto::is_encrypted(val_unquoted) {
					let key_part = &trimmed[..eq_pos];
					match runfile_crypto::decrypt(val_unquoted, &key_hex) {
						Ok(plaintext) => {
							out_lines.push(format!("{key_part}={plaintext}"));
							continue;
						}
						Err(e) => {
							eprintln!("Error decrypting {key_part}: {e}");
							process::exit(1);
						}
					}
				}
			}
		}
		out_lines.push(line.to_string());
	}

	out_lines.retain(|line| !line.trim().is_empty());

	let mut out_content = out_lines.join("\n");
	if !out_content.ends_with('\n') {
		out_content.push('\n');
	}

	match output {
		Some(path) => {
			if let Err(e) = std::fs::write(path, &out_content) {
				eprintln!("Error writing {path}: {e}");
				process::exit(1);
			}
			eprintln!("Decrypted {source} -> {path}");
		}
		None => {
			use std::io::Write;
			let stdout = std::io::stdout();
			let mut handle = stdout.lock();
			if let Err(e) = handle.write_all(out_content.as_bytes()) {
				eprintln!("Error writing to stdout: {e}");
				process::exit(1);
			}
		}
	}
}

/// Encrypt a plaintext env file into a new encrypted file.
pub fn cmd_encrypt_file(source: &str, output: &str, partial_key: &str) {
	// Check output isn't already encrypted
	let out_path = Path::new(output);
	if out_path.exists() {
		let out_content = read_file_content(output);
		if let Ok(pairs) = runfile_env::parse_env_file(&out_content) {
			let has_pub_key = pairs
				.iter()
				.any(|(k, _)| k == runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR);
			if has_pub_key {
				eprintln!(
					"Error: {output} is already encrypted (contains {})",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				);
				process::exit(1);
			}
		}
	}

	let all_keys = keyring_keys::all_private_keys();
	let key_hex = match runfile_crypto::find_private_key_by_public_prefix(partial_key, &all_keys) {
		Ok(k) => k,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	let content = read_file_content(source);
	let mut out_lines = Vec::new();

	// Add public key as first line
	out_lines.push(format!("{}={public_key}", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR));

	for line in content.lines() {
		let trimmed = line.trim();
		// Pass through comments and blank lines
		if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
			out_lines.push(line.to_string());
			continue;
		}
		// Encrypt key=value lines
		if let Some(eq_pos) = trimmed.find('=') {
			let key_part = &trimmed[..eq_pos];
			let val_part = &trimmed[eq_pos + 1..];
			// Strip export prefix for the key
			let clean_key = key_part.strip_prefix("export ").unwrap_or(key_part).trim();
			let has_export = key_part.starts_with("export ");

			// Don't encrypt empty values
			if val_part.trim().is_empty() {
				out_lines.push(line.to_string());
				continue;
			}

			match runfile_crypto::encrypt(val_part.trim(), &key_hex) {
				Ok(encrypted) => {
					if has_export {
						out_lines.push(format!("export {clean_key}={encrypted}"));
					} else {
						out_lines.push(format!("{clean_key}={encrypted}"));
					}
				}
				Err(e) => {
					eprintln!("Error encrypting {clean_key}: {e}");
					process::exit(1);
				}
			}
		} else {
			out_lines.push(line.to_string());
		}
	}

	let mut out_content = out_lines.join("\n");
	if !out_content.ends_with('\n') {
		out_content.push('\n');
	}

	if let Err(e) = std::fs::write(output, &out_content) {
		eprintln!("Error writing {output}: {e}");
		process::exit(1);
	}

	println!("Encrypted {source} -> {output}");
}

// ══════════════════════════════════════════════════════════════════════
// Inject (run a command with env vars from .env file(s))
// ══════════════════════════════════════════════════════════════════════

/// Drops any key from `env_map` that the parent process already defines, so that
/// inherited values reach the child unmodified when `Command::envs(&env_map)` is
/// applied on top of the inherited environment. `parent_lookup` is injected so
/// tests can supply a fake parent env without mutating the real process env
/// (which would race with parallel tests). In production it's `std::env::var_os`,
/// which is case-insensitive on Windows and case-sensitive on Unix.
fn apply_parent_env_override<F>(env_map: &mut HashMap<String, String>, parent_lookup: F)
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
fn effective_inject_path<F>(env_map: &HashMap<String, String>, parent_path_lookup: F) -> String
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

	let user_specified_files = !files.is_empty();
	let files_to_load: Vec<&str> = if user_specified_files {
		files.iter().map(String::as_str).collect()
	} else {
		vec![".env"]
	};

	let mut env_map: HashMap<String, String> = HashMap::new();
	for file in &files_to_load {
		let path = Path::new(file);
		if !path.exists() {
			if user_specified_files {
				eprintln!("Error: file not found: {file}");
				process::exit(1);
			}
			continue;
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
		let key_hex = match std::env::var("RUNFILE_ENCRYPTION_KEY") {
			Ok(k) if !k.is_empty() => k,
			_ => match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
				Some(public_key) => resolve_private_key_by_public(public_key),
				None => {
					eprintln!(
						"Error: encrypted values found but no encryption key available. \
						 Set RUNFILE_ENCRYPTION_KEY or ensure the env file contains {}.",
						runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
					);
					process::exit(1);
				}
			},
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
pub fn cmd_rotate(file: &str, delete_current_key: bool) {
	let content = read_file_content(file);
	let pairs = match runfile_env::parse_env_file(&content) {
		Ok(p) => p,
		Err((line, msg)) => {
			eprintln!("Error parsing {file} at line {line}: {msg}");
			process::exit(1);
		}
	};
	let env_map: HashMap<String, String> = pairs.iter().cloned().collect();

	// Verify this is an encrypted file
	let old_public_key = match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		Some(pk) => pk.clone(),
		None => {
			eprintln!(
				"Error: {file} does not contain {} — not an encrypted file",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			);
			process::exit(1);
		}
	};

	// Resolve old private key
	let old_key_hex = resolve_private_key_by_public(&old_public_key);

	// Generate a new private key and store it
	let new_key_hex = runfile_crypto::generate_key();
	match keyring_keys::add(&new_key_hex) {
		Ok(true) => {}
		Ok(false) => {
			eprintln!("Error: generated key already exists. Try again.");
			process::exit(1);
		}
		Err(e) => {
			eprintln!("Error storing new key: {e}");
			process::exit(1);
		}
	}

	let new_public_key = runfile_crypto::derive_public_key(&new_key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	// Re-encrypt the file: decrypt each value with old key, encrypt with new key
	let mut out_lines = Vec::new();

	for line in content.lines() {
		let trimmed = line.trim();

		// Replace the public key line
		if trimmed.starts_with(&format!("{}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
			|| trimmed.starts_with(&format!("export {}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
		{
			let has_export = trimmed.starts_with("export ");
			if has_export {
				out_lines.push(format!(
					"export {}={new_public_key}",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				));
			} else {
				out_lines.push(format!(
					"{}={new_public_key}",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				));
			}
			continue;
		}

		// Pass through comments and blank lines
		if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
			out_lines.push(line.to_string());
			continue;
		}

		// Re-encrypt key=value lines with encrypted values
		if let Some(eq_pos) = trimmed.find('=') {
			let key_part = &trimmed[..eq_pos];
			let val_part = &trimmed[eq_pos + 1..];
			let val_trimmed = val_part.trim();

			// Strip quotes if present
			let val_unquoted = if (val_trimmed.starts_with('"') && val_trimmed.ends_with('"'))
				|| (val_trimmed.starts_with('\'') && val_trimmed.ends_with('\''))
			{
				&val_trimmed[1..val_trimmed.len() - 1]
			} else {
				val_trimmed
			};

			if runfile_crypto::is_encrypted(val_unquoted) {
				let clean_key = key_part.strip_prefix("export ").unwrap_or(key_part).trim();
				let has_export = key_part.starts_with("export ");

				// Decrypt with old key
				let plaintext = match runfile_crypto::decrypt(val_unquoted, &old_key_hex) {
					Ok(p) => p,
					Err(e) => {
						eprintln!("Error decrypting {clean_key}: {e}");
						process::exit(1);
					}
				};

				// Encrypt with new key
				let encrypted = match runfile_crypto::encrypt(&plaintext, &new_key_hex) {
					Ok(enc) => enc,
					Err(e) => {
						eprintln!("Error encrypting {clean_key}: {e}");
						process::exit(1);
					}
				};

				if has_export {
					out_lines.push(format!("export {clean_key}={encrypted}"));
				} else {
					out_lines.push(format!("{clean_key}={encrypted}"));
				}
				continue;
			}
		}

		// Non-encrypted lines pass through unchanged
		out_lines.push(line.to_string());
	}

	let mut out_content = out_lines.join("\n");
	if !out_content.ends_with('\n') {
		out_content.push('\n');
	}

	if let Err(e) = std::fs::write(file, &out_content) {
		eprintln!("Error writing {file}: {e}");
		process::exit(1);
	}

	// Optionally delete the old key
	if delete_current_key {
		match keyring_keys::remove(&old_public_key) {
			Ok(true) => {}
			Ok(false) => {
				eprintln!("Warning: old key not found in credential store (already removed?).");
			}
			Err(e) => {
				eprintln!("Warning: failed to remove old key: {e}");
			}
		}
	}

	println!("Key rotated for {file}.");
	println!();
	println!("  Old public key: {old_public_key}");
	println!("  New public key: {new_public_key}");

	if delete_current_key {
		println!();
		println!("Old key has been removed from the OS credential store.");
	}

	println!();
	println!("To share the new key with teammates:");
	println!("  run :env secret-keys get-private {}...", &new_public_key[..8]);
}

// ══════════════════════════════════════════════════════════════════════
// Helpers
// ══════════════════════════════════════════════════════════════════════

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
fn resolve_private_key_by_public(public_key: &str) -> String {
	// Check RUNFILE_ENCRYPTION_KEY env var first (for CI)
	if let Ok(key) = std::env::var("RUNFILE_ENCRYPTION_KEY") {
		if !key.is_empty() {
			return key;
		}
	}

	let all_keys = keyring_keys::all_private_keys();
	match runfile_crypto::find_matching_private_key(public_key, &all_keys) {
		Some(key) => key,
		None => {
			eprintln!(
				"Error: no private key matches public key {public_key}.\n\
				 Run `run :env secret-keys add` to add the correct key."
			);
			process::exit(1);
		}
	}
}

/// Replace or append a VAR=value line in env file content.
/// Preserves comments, blank lines, and formatting.
fn set_env_line(content: &str, var: &str, value: &str) -> String {
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn set_env_line_replaces_existing() {
		let content = "FOO=old\nBAR=keep\n";
		let result = set_env_line(content, "FOO", "new");
		assert!(result.contains("FOO=new"));
		assert!(result.contains("BAR=keep"));
		assert!(!result.contains("FOO=old"));
	}

	#[test]
	fn set_env_line_appends_new() {
		let content = "FOO=value\n";
		let result = set_env_line(content, "BAR", "added");
		assert!(result.contains("FOO=value"));
		assert!(result.contains("BAR=added"));
	}

	#[test]
	fn set_env_line_preserves_export() {
		let content = "export SECRET=old\n";
		let result = set_env_line(content, "SECRET", "new");
		assert!(result.contains("export SECRET=new"));
	}

	#[test]
	fn set_env_line_preserves_comments() {
		let content = "# Database config\nDB_HOST=localhost\n# End\n";
		let result = set_env_line(content, "DB_HOST", "remote");
		assert!(result.contains("# Database config"));
		assert!(result.contains("DB_HOST=remote"));
		assert!(result.contains("# End"));
	}

	#[test]
	fn set_env_line_empty_file() {
		let result = set_env_line("", "KEY", "value");
		assert_eq!(result, "KEY=value\n");
	}

	// ══════════════════════════════════════════════════════════════════════
	// `inject` precedence tests
	//
	// The full ordering for `run :env inject -f a -f b -- COMMAND` is:
	//   1. -f files, processed left-to-right (later overrides earlier)
	//   2. parent process env — ALWAYS wins, last layer applied
	// File-loaded values only fill in gaps the parent doesn't already cover.
	// ══════════════════════════════════════════════════════════════════════

	use std::ffi::OsString;

	/// Builds a fake parent-env lookup with case-sensitive matching (Unix-like)
	/// so we can deterministically test the override logic on every platform.
	fn fake_parent<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
		move |k: &str| {
			pairs
				.iter()
				.find(|(name, _)| *name == k)
				.map(|(_, v)| OsString::from(*v))
		}
	}

	#[test]
	fn inject_parent_env_wins_over_file_value() {
		let mut env_map = HashMap::new();
		env_map.insert("DB_HOST".to_string(), "from-file".to_string());
		env_map.insert("OTHER".to_string(), "from-file".to_string());

		apply_parent_env_override(&mut env_map, fake_parent(&[("DB_HOST", "from-parent")]));

		assert!(
			!env_map.contains_key("DB_HOST"),
			"DB_HOST should be removed so parent's value reaches the child unmodified",
		);
		assert_eq!(env_map.get("OTHER").map(String::as_str), Some("from-file"));
	}

	#[test]
	fn inject_file_value_kept_when_parent_unset() {
		let mut env_map = HashMap::new();
		env_map.insert("FOO".to_string(), "from-file".to_string());
		env_map.insert("BAR".to_string(), "from-file".to_string());

		apply_parent_env_override(&mut env_map, fake_parent(&[]));

		assert_eq!(env_map.get("FOO").map(String::as_str), Some("from-file"));
		assert_eq!(env_map.get("BAR").map(String::as_str), Some("from-file"));
	}

	#[test]
	fn inject_empty_env_map_is_a_noop() {
		let mut env_map: HashMap<String, String> = HashMap::new();
		apply_parent_env_override(&mut env_map, fake_parent(&[("ANYTHING", "x")]));
		assert!(env_map.is_empty());
	}

	#[test]
	fn inject_parent_value_does_not_appear_in_env_map() {
		// The retain only removes — it must never add the parent value into env_map
		// (which would break `Command::envs` semantics on Windows where the parent
		// would otherwise appear twice with different casing).
		let mut env_map = HashMap::new();
		env_map.insert("FOO".to_string(), "from-file".to_string());

		apply_parent_env_override(&mut env_map, fake_parent(&[("FOO", "from-parent")]));

		assert!(!env_map.contains_key("FOO"));
		assert!(!env_map.values().any(|v| v == "from-parent"));
	}

	#[test]
	fn inject_later_file_overrides_earlier() {
		// Mimics what cmd_inject does when iterating multiple -f files: each file's
		// pairs are inserted into env_map, with later inserts winning.
		let mut env_map: HashMap<String, String> = HashMap::new();
		for (k, v) in [("FOO", "first"), ("BAR", "first")] {
			env_map.insert(k.to_string(), v.to_string());
		}
		for (k, v) in [("FOO", "second"), ("BAZ", "second")] {
			env_map.insert(k.to_string(), v.to_string());
		}

		assert_eq!(env_map.get("FOO").map(String::as_str), Some("second"));
		assert_eq!(env_map.get("BAR").map(String::as_str), Some("first"));
		assert_eq!(env_map.get("BAZ").map(String::as_str), Some("second"));
	}

	#[test]
	fn inject_full_order_files_then_parent() {
		// End-to-end ordering test against the helper:
		//   file1: FOO=a, BAR=a
		//   file2: FOO=b, BAZ=b           (later file wins → FOO=b)
		//   parent: FOO=parent, QUX=parent (parent wins → FOO removed from env_map)
		// Expected env_map after `apply_parent_env_override`: {BAR=a, BAZ=b}
		// (FOO is dropped so the parent's value reaches the child untouched.
		// QUX was never in env_map; the child inherits it from the parent.)
		let mut env_map: HashMap<String, String> = HashMap::new();
		for (k, v) in [("FOO", "a"), ("BAR", "a")] {
			env_map.insert(k.to_string(), v.to_string());
		}
		for (k, v) in [("FOO", "b"), ("BAZ", "b")] {
			env_map.insert(k.to_string(), v.to_string());
		}

		apply_parent_env_override(&mut env_map, fake_parent(&[("FOO", "parent"), ("QUX", "parent")]));

		assert!(!env_map.contains_key("FOO"), "parent overrides file FOO");
		assert_eq!(env_map.get("BAR").map(String::as_str), Some("a"));
		assert_eq!(env_map.get("BAZ").map(String::as_str), Some("b"));
		assert!(!env_map.contains_key("QUX"), "parent-only key never enters env_map");
		assert_eq!(env_map.len(), 2);
	}

	#[test]
	fn inject_pubkey_marker_is_stripped_independently_of_parent_logic() {
		// In production cmd_inject removes the encryption pubkey marker BEFORE
		// applying the parent override. This test pins the contract that a marker
		// surviving into apply_parent_env_override would be removed only if the
		// parent also defined it — i.e. the helper itself does NOT special-case
		// the marker. (The strip-pubkey step is the caller's responsibility.)
		let mut env_map = HashMap::new();
		env_map.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), "abc".to_string());
		apply_parent_env_override(&mut env_map, fake_parent(&[]));
		assert!(env_map.contains_key(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR));
	}

	#[cfg(windows)]
	#[test]
	fn inject_parent_env_lookup_is_case_insensitive_on_windows() {
		// On Windows, std::env::var_os does case-insensitive lookup. Verify the
		// helper passes through whatever semantics the lookup function provides
		// by giving it a case-insensitive fake.
		fn ci_parent(k: &str) -> Option<OsString> {
			if k.eq_ignore_ascii_case("Path") {
				Some(OsString::from("C:\\real;C:\\windows"))
			} else {
				None
			}
		}
		let mut env_map = HashMap::new();
		env_map.insert("path".to_string(), "C:\\from-file".to_string());
		env_map.insert("PATH".to_string(), "C:\\from-file-upper".to_string());

		apply_parent_env_override(&mut env_map, ci_parent);

		assert!(!env_map.contains_key("path"), "lowercase path should be dropped");
		assert!(!env_map.contains_key("PATH"), "uppercase PATH should be dropped");
	}

	// --- effective_inject_path ---

	#[test]
	fn effective_path_prefers_parent_when_set() {
		let mut env_map = HashMap::new();
		env_map.insert("PATH".to_string(), "/from-file".to_string());

		let resolved = effective_inject_path(&env_map, || Some("/from-parent".to_string()));

		assert_eq!(resolved, "/from-parent");
	}

	#[test]
	fn effective_path_falls_back_to_env_file_when_parent_unset() {
		let mut env_map = HashMap::new();
		env_map.insert("PATH".to_string(), "/from-file".to_string());

		let resolved = effective_inject_path(&env_map, || None);

		assert_eq!(resolved, "/from-file");
	}

	#[test]
	fn effective_path_env_file_lookup_is_case_insensitive() {
		// Mirrors the env-file PATH detection in cmd_inject: a `path=` (lowercase)
		// entry in a .env file should still be picked up as the fallback.
		let mut env_map = HashMap::new();
		env_map.insert("path".to_string(), "/from-file-lower".to_string());

		let resolved = effective_inject_path(&env_map, || None);

		assert_eq!(resolved, "/from-file-lower");
	}

	#[test]
	fn effective_path_empty_when_neither_source_set() {
		let env_map: HashMap<String, String> = HashMap::new();
		let resolved = effective_inject_path(&env_map, || None);
		assert_eq!(resolved, "");
	}

	#[test]
	fn effective_path_parent_wins_even_when_env_file_has_path() {
		// Critical correctness test: with parent-env-always-wins semantics, the
		// child runs with the parent's PATH. `which::which_in` must therefore
		// search the parent's PATH — otherwise it could resolve a binary the
		// child can't actually exec.
		let mut env_map = HashMap::new();
		env_map.insert("PATH".to_string(), "/from-file".to_string());

		let resolved = effective_inject_path(&env_map, || Some("/from-parent".to_string()));

		assert_eq!(resolved, "/from-parent");
		assert!(!resolved.contains("from-file"));
	}
}
