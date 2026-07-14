use super::*;

/// Decrypt an encrypted env file. Writes to `output` if provided, otherwise prints to stdout.
/// When `source` is `None`, falls back to [`RUNFILE_ENV_FILE_TARGET_ENV_VAR`] — set by the
/// `setup` action's `env-file-source` input so CI can decrypt a secret-supplied file
/// without writing the path into the workflow.
pub fn cmd_decrypt_file(source: Option<&str>, output: Option<&str>) {
	let source_owned = match source {
		Some(s) => s.to_string(),
		None => env_file_target().unwrap_or_else(|| {
			eprintln!(
				"Error: no source file specified.\n\
				 Usage: run :env decrypt <source> [output]\n\
				 (Or set {RUNFILE_ENV_FILE_TARGET_ENV_VAR} to provide one.)"
			);
			process::exit(1);
		}),
	};
	let source = source_owned.as_str();
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
			// Decrypted plaintext to disk — restrict to 0600 (Unix) so the
			// secrets aren't left world-readable on a shared host.
			if let Err(e) = write_secret_file(path, out_content.as_bytes()) {
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

/// Rotate the private key for an encrypted env file: decrypt every value with
/// the old key and re-encrypt with a freshly generated key.
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
