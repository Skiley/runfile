use crate::{
	build_env, check_env_case_duplicates, collect_runfile_env, load_env_files, parse_env_file, EnvBuildParams,
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

#[test]
fn parse_env_file_simple() {
	let content = "KEY=value\nANOTHER=hello world\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(
		pairs,
		vec![
			("KEY".to_string(), "value".to_string()),
			("ANOTHER".to_string(), "hello world".to_string()),
		]
	);
}

#[test]
fn parse_env_file_with_comments() {
	let content = "# This is a comment\nKEY=value\n// Another comment\nFOO=bar\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 2);
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
}

#[test]
fn parse_env_file_blank_lines() {
	let content = "\n\nKEY=value\n\n\nFOO=bar\n\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 2);
}

#[test]
fn parse_env_file_spaces_around_equals() {
	let content = "KEY = value\nFOO =bar\nBAZ= baz\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
	assert_eq!(pairs[2], ("BAZ".to_string(), "baz".to_string()));
}

#[test]
fn parse_env_file_double_quoted() {
	let content = r#"KEY="hello world""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "hello world".to_string()));
}

#[test]
fn parse_env_file_single_quoted() {
	let content = "KEY='hello world'";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "hello world".to_string()));
}

#[test]
fn parse_env_file_multiline_double_quoted() {
	let content = "KEY=\"line1\nline2\nline3\"";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "line1\nline2\nline3".to_string()));
}

#[test]
fn parse_env_file_multiline_single_quoted() {
	let content = "KEY='line1\nline2'";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "line1\nline2".to_string()));
}

#[test]
fn parse_env_file_empty_value() {
	let content = "KEY=";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "".to_string()));
}

#[test]
fn parse_env_file_escape_sequences() {
	let content = r#"KEY="hello\nworld\ttab""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "hello\nworld\ttab".to_string()));
}

#[test]
fn parse_env_file_inline_comments() {
	let content = "KEY=value # this is a comment\nFOO=bar // another";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
}

#[test]
fn parse_env_file_export_prefix() {
	let content = "export KEY=value\nexport FOO=bar";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "value".to_string()));
	assert_eq!(pairs[1], ("FOO".to_string(), "bar".to_string()));
}

#[test]
fn parse_env_file_error_no_equals() {
	let content = "INVALID_LINE";
	let err = parse_env_file(content);
	assert!(err.is_err());
}

#[test]
fn parse_env_file_empty_key_errors() {
	let content = "=value";
	let err = parse_env_file(content);
	assert!(err.is_err());
	let (line, msg) = err.unwrap_err();
	assert_eq!(line, 1);
	assert!(msg.contains("empty key"), "got: {msg}");
}

#[test]
fn parse_env_file_export_prefix_stripped() {
	let content = "export MY_KEY=my_value";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("MY_KEY".to_string(), "my_value".to_string()));
}

#[test]
fn parse_env_file_unterminated_double_quote() {
	let content = "KEY=\"this is never closed\n";
	let err = parse_env_file(content);
	assert!(err.is_err());
	let (_, msg) = err.unwrap_err();
	assert!(msg.contains("unterminated"), "got: {msg}");
}

#[test]
fn parse_env_file_unterminated_single_quote() {
	let content = "KEY='this is never closed\n";
	let err = parse_env_file(content);
	assert!(err.is_err());
	let (_, msg) = err.unwrap_err();
	assert!(msg.contains("unterminated"), "got: {msg}");
}

#[test]
fn parse_env_file_escaped_double_quote_in_value() {
	let content = r#"KEY="say \"hello\"""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, r#"say "hello""#);
}

#[test]
fn parse_env_file_escaped_backslash() {
	let content = r#"KEY="path\\to\\file""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, r"path\to\file");
}

#[test]
fn parse_env_file_carriage_return_escape() {
	let content = r#"KEY="line\r""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "line\r");
}

#[test]
fn parse_env_file_unknown_escape_preserved() {
	let content = r#"KEY="hello\x""#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "hello\\x");
}

#[test]
fn parse_env_file_trailing_backslash_in_double_quotes() {
	let content = "KEY=\"hello\\\\\"";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "hello\\");
}

#[test]
fn parse_env_file_single_quoted_no_escape_processing() {
	let content = r#"KEY='hello\nworld'"#;
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, r"hello\nworld");
}

#[test]
fn parse_env_file_multiple_entries() {
	let content = "A=1\nB=2\nC=3\n";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs.len(), 3);
	assert_eq!(pairs[2], ("C".to_string(), "3".to_string()));
}

#[test]
fn parse_env_file_value_with_equals() {
	let content = "KEY=abc=def";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0], ("KEY".to_string(), "abc=def".to_string()));
}

#[test]
fn parse_env_file_only_comments_and_blanks() {
	let content = "# comment\n\n// another\n\n";
	let pairs = parse_env_file(content).unwrap();
	assert!(pairs.is_empty());
}

#[test]
fn parse_env_file_empty_content() {
	let pairs = parse_env_file("").unwrap();
	assert!(pairs.is_empty());
}

// ══════════════════════════════════════════════════════════════════════
// load_env_files tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn load_env_files_missing_file_ignored() {
	let dir = TempDir::new().unwrap();
	let env = HashMap::new();
	let result = load_env_files(&[".env.nonexistent".to_string()], dir.path(), &no_substitute, &env);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

#[test]
fn load_env_files_reads_existing_file() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "MY_KEY=my_value\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &no_substitute, &env).unwrap();
	assert_eq!(result.get("MY_KEY").unwrap(), "my_value");
}

#[test]
fn load_env_files_later_overrides_earlier() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=first\n").unwrap();
	std::fs::write(dir.path().join(".env.local"), "KEY=second\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(
		&[".env".to_string(), ".env.local".to_string()],
		dir.path(),
		&no_substitute,
		&env,
	)
	.unwrap();
	assert_eq!(result.get("KEY").unwrap(), "second");
}

#[test]
fn load_env_files_parse_error() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "INVALID_NO_EQUALS\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &no_substitute, &env);
	assert!(result.is_err());
}

#[test]
fn load_env_files_absolute_path() {
	let dir = TempDir::new().unwrap();
	let env_path = dir.path().join("abs.env");
	std::fs::write(&env_path, "ABS_KEY=abs_value\n").unwrap();
	let env = HashMap::new();
	let result = load_env_files(
		&[env_path.to_str().unwrap().to_string()],
		dir.path(),
		&no_substitute,
		&env,
	)
	.unwrap();
	assert_eq!(result.get("ABS_KEY").unwrap(), "abs_value");
}

#[test]
fn load_env_files_multiple_missing_files_all_skipped() {
	let dir = TempDir::new().unwrap();
	let env = HashMap::new();
	let result = load_env_files(
		&[
			".env.missing1".to_string(),
			".env.missing2".to_string(),
			".env.missing3".to_string(),
		],
		dir.path(),
		&no_substitute,
		&env,
	);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

#[test]
fn load_env_files_with_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.production"), "DB=prod-db\n").unwrap();
	let env = HashMap::new();
	// Simulate substitution that replaces {{ MYVAR }} with "production"
	let substitute = |input: &str, _env: &HashMap<String, String>| -> Result<String, String> {
		Ok(input.replace("{{ MYVAR }}", "production"))
	};
	let result = load_env_files(&[".env.{{ MYVAR }}".to_string()], dir.path(), &substitute, &env).unwrap();
	assert_eq!(result.get("DB").unwrap(), "prod-db");
}

// ══════════════════════════════════════════════════════════════════════
// build_env tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn build_env_with_no_extras() {
	let dir = TempDir::new().unwrap();
	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	// Should contain system env vars
	assert!(!env.is_empty());
	// PATH should be present
	assert!(env.iter().any(|(k, _)| k.eq_ignore_ascii_case("PATH")));
}

#[test]
fn build_env_with_env() {
	let dir = TempDir::new().unwrap();
	let mut global_env = HashMap::new();
	global_env.insert("MY_GLOBAL".to_string(), "global_value".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&global_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("MY_GLOBAL").unwrap(), "global_value");
}

#[test]
fn build_env_with_env_value() {
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".to_string(), "command".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "command");
}

#[test]
fn build_env_add_to_path() {
	let dir = TempDir::new().unwrap();
	let paths = vec!["global_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let expected = dir.path().join("global_bin").to_string_lossy().replace('\\', "/");
	assert!(path.contains(&expected), "PATH should contain global_bin: {path}");
}

#[test]
fn build_env_add_to_path_multiple() {
	let dir = TempDir::new().unwrap();
	let paths = vec!["cmd_bin".to_string(), "global_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let cmd_expected = dir.path().join("cmd_bin").to_string_lossy().replace('\\', "/");
	let global_expected = dir.path().join("global_bin").to_string_lossy().replace('\\', "/");
	assert!(path.contains(&cmd_expected), "cmd_bin should be in PATH");
	assert!(path.contains(&global_expected), "global_bin should be in PATH");
}

#[test]
fn build_env_env_files_before_env() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=from_file\n").unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".to_string(), "from_env".to_string());

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	// env (inline) should override envFiles
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "from_env");
}

#[test]
fn build_env_env_files_load() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "GLOBAL_KEY=global_value\n").unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("GLOBAL_KEY").unwrap(), "global_value");
}

#[test]
fn build_env_env_files() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.target"), "KEY=target\n").unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env.target".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "target");
}

#[test]
fn build_env_substitution_in_env_values() {
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("GREETING".to_string(), "hello {{ NAME }}".to_string());

	// Substitute that replaces {{ NAME }} with "world"
	let substitute = |input: &str, _env: &HashMap<String, String>| -> Result<String, String> {
		Ok(input.replace("{{ NAME }}", "world"))
	};

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &substitute).unwrap();
	assert_eq!(env.get("GREETING").unwrap(), "hello world");
}

#[test]
fn build_env_substitution_error_propagated() {
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".to_string(), "{{ MISSING }}".to_string());

	let substitute = |input: &str, _env: &HashMap<String, String>| -> Result<String, String> {
		if input.contains("{{ MISSING }}") {
			Err("missing variable".to_string())
		} else {
			Ok(input.to_string())
		}
	};

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &substitute);
	assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════
// check_env_case_duplicates tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn check_env_case_duplicates_ok() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".to_string(), "production".to_string());
	env.insert("OTHER_VAR".to_string(), "value".to_string());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_detects_conflict() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".to_string(), "production".to_string());
	env.insert("node_env".to_string(), "development".to_string());
	let result = check_env_case_duplicates(&env);
	assert!(result.is_err());
}

#[test]
fn check_env_case_duplicates_same_case_ok() {
	let mut env = HashMap::new();
	env.insert("KEY".to_string(), "value".to_string());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_empty_env_ok() {
	let env: HashMap<String, String> = HashMap::new();
	assert!(check_env_case_duplicates(&env).is_ok());
}

// ══════════════════════════════════════════════════════════════════════
// collect_runfile_env tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn collect_runfile_env_empty() {
	let result = collect_runfile_env(None);
	assert!(result.is_empty());
}

#[test]
fn collect_runfile_env_with_values() {
	let mut env = HashMap::new();
	env.insert("A".to_string(), "1".to_string());
	let result = collect_runfile_env(Some(&env));
	assert_eq!(result, vec![("A".to_string(), "1".to_string())]);
}

#[test]
fn collect_runfile_env_sorted() {
	let mut env = HashMap::new();
	env.insert("Z".to_string(), "last".to_string());
	env.insert("A".to_string(), "first".to_string());
	let result = collect_runfile_env(Some(&env));
	assert_eq!(result[0].0, "A");
	assert_eq!(result[1].0, "Z");
}

// ══════════════════════════════════════════════════════════════════════
// Encrypted env tests
// ══════════════════════════════════════════════════════════════════════

#[test]
fn build_env_decrypts_via_public_key_matching() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret_password", &key_hex).unwrap();
	let private_keys = vec![key_hex];

	let mut cmd_env = HashMap::new();
	cmd_env.insert("DB_PASS".to_string(), encrypted);
	cmd_env.insert("PLAIN_VAR".to_string(), "plain_value".to_string());
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&private_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("DB_PASS").unwrap(), "secret_password");
	assert_eq!(env.get("PLAIN_VAR").unwrap(), "plain_value");
}

#[test]
fn build_env_encrypted_no_public_key_no_keys_errors() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("secret", &key_hex).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	// No RUNFILE_ENCRYPTION_PUBLIC_KEY, no RUNFILE_ENCRYPTION_KEY

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("Encryption error"), "got: {err}");
}

#[test]
fn build_env_encrypted_no_matching_private_key_errors() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let wrong_key = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("secret", &key_hex).unwrap();
	let wrong_keys = vec![wrong_key];

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&wrong_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("no matching private key"), "got: {err}");
}

#[test]
fn build_env_decrypts_env_file_with_public_key() {
	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("from_file_secret", &key_hex).unwrap();
	let private_keys = vec![key_hex];

	std::fs::write(
		dir.path().join(".env"),
		format!(
			"{}={public_key}\nFILE_SECRET={encrypted}\n",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
		),
	)
	.unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: Some(&private_keys),
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("FILE_SECRET").unwrap(), "from_file_secret");
}

// ══════════════════════════════════════════════════════════════════════
// RUNFILE_ENCRYPTION_KEY validation tests
// ══════════════════════════════════════════════════════════════════════

/// These tests exercise `resolve_decryption_key` indirectly via `build_env`.
/// Since `build_env` collects system env vars first, we inject `RUNFILE_ENCRYPTION_KEY`
/// via the command env to avoid race conditions with parallel tests mutating `std::env`.

#[test]
fn encryption_key_env_var_must_be_valid_hex() {
	let dir = TempDir::new().unwrap();
	let key = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("secret", &key).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	// Inject an invalid RUNFILE_ENCRYPTION_KEY via command env
	cmd_env.insert("RUNFILE_ENCRYPTION_KEY".to_string(), "not-valid-hex-at-all".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);

	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("64-character hex"),
		"should mention format requirement: {err}"
	);
}

#[test]
fn encryption_key_env_var_validated_against_public_key() {
	let dir = TempDir::new().unwrap();
	let correct_key = runfile_crypto::generate_key();
	let wrong_key = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&correct_key).unwrap();
	let encrypted = runfile_crypto::encrypt("secret", &correct_key).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);
	// Inject a valid but WRONG key via command env
	cmd_env.insert("RUNFILE_ENCRYPTION_KEY".to_string(), wrong_key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);

	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("does not match"), "should report key mismatch: {err}");
}

#[test]
fn encryption_key_env_var_matching_public_key_succeeds() {
	let dir = TempDir::new().unwrap();
	let key = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key).unwrap();
	let encrypted = runfile_crypto::encrypt("my-secret", &key).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	cmd_env.insert(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR.to_string(), public_key);
	// Inject the correct key via command env
	cmd_env.insert("RUNFILE_ENCRYPTION_KEY".to_string(), key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);

	assert!(result.is_ok());
	let env = result.unwrap();
	assert_eq!(env.get("SECRET").unwrap(), "my-secret");
}

#[test]
fn encryption_key_env_var_too_short_rejected() {
	let dir = TempDir::new().unwrap();
	let key = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("secret", &key).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("VAR".to_string(), encrypted);
	// 32 hex chars (only 128-bit, not 256-bit)
	cmd_env.insert(
		"RUNFILE_ENCRYPTION_KEY".to_string(),
		"aabbccddaabbccddaabbccddaabbccdd".to_string(),
	);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);

	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("64-character hex"), "should reject short key: {err}");
}

#[test]
fn encryption_key_without_public_key_still_works() {
	// When RUNFILE_ENCRYPTION_KEY is set but no RUNFILE_ENCRYPTION_PUBLIC_KEY,
	// the key should be used directly (no fingerprint verification).
	let dir = TempDir::new().unwrap();
	let key = runfile_crypto::generate_key();
	let encrypted = runfile_crypto::encrypt("value", &key).unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("SECRET".to_string(), encrypted);
	cmd_env.insert("RUNFILE_ENCRYPTION_KEY".to_string(), key);

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let result = build_env(&params, &no_substitute);

	assert!(result.is_ok());
	assert_eq!(result.unwrap().get("SECRET").unwrap(), "value");
}

// ══════════════════════════════════════════════════════════════════════
// Priority order tests
//
// Final ordering for `build_env` (low → high):
//   1. envFiles (later file wins per key)
//   2. env (substituted; wins over envFiles within the Runfile layer)
//   3. **current shell env always wins** — re-overlays Runfile-defined keys
//   4. addToPath chain — for PATH only, prepended innermost-first
//      (`[this target's addToPath..., parent's..., ..., shell PATH]`)
//   5. decryption
//
// PATH (from std::env::vars()) is the only system var we can rely on being
// present cross-platform without mutating the test process env.
// ══════════════════════════════════════════════════════════════════════

#[test]
fn shell_path_beats_runfile_env_path_override() {
	// User tries to set PATH via Runfile env; shell's PATH must win.
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PATH".to_string(), "/should/be/wiped/by/shell".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();

	assert!(
		!path.contains("/should/be/wiped/by/shell"),
		"shell PATH must win over the Runfile-set PATH; got {path}"
	);
	assert_eq!(
		path, shell_path,
		"with no addToPath, final PATH should equal shell PATH"
	);
}

#[test]
fn shell_path_beats_runfile_envfile_path_override() {
	// Same idea but PATH coming from an env file.
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "PATH=/from/file/should/be/wiped\n").unwrap();

	let params = EnvBuildParams {
		env_files: Some(&[".env".to_string()]),
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);

	assert!(
		!path.contains("/from/file/should/be/wiped"),
		"shell PATH must win over envFile-set PATH; got {path}"
	);
}

#[test]
fn runfile_env_kept_for_keys_not_in_shell() {
	// Keys that don't conflict with shell vars survive untouched.
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("RUNFILE_TEST_UNIQUE_KEY_42".to_string(), "runfile_kept".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(env.get("RUNFILE_TEST_UNIQUE_KEY_42").unwrap(), "runfile_kept");
}

#[test]
fn add_to_path_prepends_to_shell_path_after_overlay() {
	// addToPath is applied AFTER the shell-env overlay, so it always lands at
	// the front of PATH — never gets wiped by the overlay.
	let dir = TempDir::new().unwrap();
	let paths = vec!["custom_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let resolved = dir.path().join("custom_bin").to_string_lossy().replace('\\', "/");
	let shell_path = std::env::var("PATH").unwrap_or_default().replace('\\', "/");

	let separator = if cfg!(windows) { ";" } else { ":" };
	let expected_prefix = format!("{resolved}{separator}");
	assert!(
		path.starts_with(&expected_prefix),
		"addToPath entry should be at the front of PATH; got {path}"
	);
	assert!(
		path.ends_with(&shell_path),
		"shell PATH should be preserved at the tail; got {path}"
	);
}

#[test]
fn add_to_path_wins_even_when_runfile_env_tries_to_replace_path() {
	// User sets PATH via Runfile env AND has addToPath. Shell wipes the
	// Runfile-set PATH (step 3), then addToPath prepends to shell PATH (step 4).
	let dir = TempDir::new().unwrap();
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PATH".to_string(), "/should/be/wiped".to_string());
	let paths = vec!["my_bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&cmd_env),
		add_to_path: Some(&paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let resolved = dir.path().join("my_bin").to_string_lossy().replace('\\', "/");

	assert!(path.contains(&resolved), "addToPath entry should be in PATH");
	assert!(
		!path.contains("/should/be/wiped"),
		"the Runfile-set PATH should never reach the final env"
	);
}

#[test]
fn parent_add_to_path_chain_innermost_wins() {
	// Simulates A → @B → @C: the chain handed to C is [A_addToPath, B_addToPath]
	// (outermost first). C's own addToPath is the innermost. Final PATH order:
	// [C_paths, B_paths, A_paths, shell PATH].
	//
	// Use TempDir-derived absolute paths so the test is portable: on Windows
	// `/abs/...` isn't absolute (no drive letter) and would get resolved
	// against the working_dir, mangling the prefix string.
	let dir = TempDir::new().unwrap();
	let grand_path = dir.path().join("grand_bin").to_string_lossy().to_string();
	let parent_path = dir.path().join("parent_bin").to_string_lossy().to_string();
	let dep_path = dir.path().join("dep_bin").to_string_lossy().to_string();

	let parent_chain: Vec<Vec<String>> = vec![vec![grand_path.clone()], vec![parent_path.clone()]];
	let dep_paths = vec![dep_path.clone()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&dep_paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let separator = if cfg!(windows) { ";" } else { ":" };

	let dep_idx = path.find(&dep_path).expect("dep entry missing");
	let parent_idx = path.find(&parent_path).expect("parent entry missing");
	let grand_idx = path.find(&grand_path).expect("grand entry missing");
	assert!(
		dep_idx < parent_idx && parent_idx < grand_idx,
		"order must be dep < parent < grand (innermost first); got {path}"
	);

	let expected_prefix = format!("{dep_path}{separator}{parent_path}{separator}{grand_path}{separator}");
	assert!(
		path.starts_with(&expected_prefix),
		"PATH should start with full chain in innermost-first order; got {path}"
	);
}

#[test]
fn parent_chain_re_prepended_after_shell_overlay_wipes_parent_resolved_path() {
	// Realistic @dep flow: parent's resolved env (passed as base_env) carries
	// a stale PATH that already had parent's addToPath baked in. The shell
	// overlay wipes that, then we re-prepend the chain.
	let dir = TempDir::new().unwrap();
	let mut parent_resolved = HashMap::new();
	parent_resolved.insert("PATH".to_string(), "/abs/parent/bin:/old/system/snapshot".to_string());
	parent_resolved.insert("PARENT_KEPT".to_string(), "from_parent".to_string());

	let parent_chain: Vec<Vec<String>> = vec![vec!["/abs/parent/bin".to_string()]];
	let dep_paths = vec!["/abs/dep/bin".to_string()];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: Some(&dep_paths),
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&parent_resolved),
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env).replace('\\', "/");
	let shell_path = std::env::var("PATH").unwrap_or_default().replace('\\', "/");

	assert!(
		!path.contains("/old/system/snapshot"),
		"stale parent-resolved PATH must be wiped by the shell overlay; got {path}"
	);
	assert!(path.contains("/abs/parent/bin"), "parent chain entry should be in PATH");
	assert!(path.contains("/abs/dep/bin"), "dep addToPath should be in PATH");
	assert!(
		path.ends_with(&shell_path),
		"shell PATH should be at the tail (after the chain); got {path}"
	);
	let dep_idx = path.find("/abs/dep/bin").unwrap();
	let parent_idx = path.find("/abs/parent/bin").unwrap();
	assert!(dep_idx < parent_idx, "dep should precede parent in PATH");

	// Non-PATH parent contribution survives because shell doesn't define PARENT_KEPT.
	assert_eq!(env.get("PARENT_KEPT").unwrap(), "from_parent");
}

#[test]
fn dep_runfile_env_beats_parent_runfile_env_when_shell_does_not_have_key() {
	// For keys not in shell, the dep's env layer wins over parent's because it's
	// applied later. Use a unique key shell can't possibly have.
	let dir = TempDir::new().unwrap();
	let mut parent_resolved = HashMap::new();
	parent_resolved.insert(
		"RUNFILE_TEST_DEP_BEATS_PARENT_99".to_string(),
		"from_parent".to_string(),
	);

	let mut dep_env = HashMap::new();
	dep_env.insert("RUNFILE_TEST_DEP_BEATS_PARENT_99".to_string(), "from_dep".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&dep_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&parent_resolved),
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	assert_eq!(
		env.get("RUNFILE_TEST_DEP_BEATS_PARENT_99").unwrap(),
		"from_dep",
		"dep's later layer should win over parent's value when shell doesn't define the key"
	);
}

#[test]
fn shell_beats_dep_runfile_env_too_for_keys_in_shell() {
	// Shell-wins applies to dep contributions, not just top-level. Setting PATH
	// in dep's env doesn't survive — shell overlay wipes it.
	let dir = TempDir::new().unwrap();
	let mut parent_resolved = HashMap::new();
	parent_resolved.insert("PATH".to_string(), "/parent/baked/path".to_string());

	let mut dep_env = HashMap::new();
	dep_env.insert("PATH".to_string(), "/dep/tries/to/win".to_string());

	let params = EnvBuildParams {
		env_files: None,
		env: Some(&dep_env),
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: Some(&parent_resolved),
		parent_add_to_path_chain: None,
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();

	assert!(!path.contains("/dep/tries/to/win"));
	assert!(!path.contains("/parent/baked/path"));
	assert_eq!(path, shell_path);
}

#[test]
fn empty_chain_with_no_local_add_to_path_leaves_path_untouched() {
	// Sanity: when neither chain nor local target contributes anything, PATH
	// equals shell PATH exactly (no separator/empty-entry edge cases).
	let dir = TempDir::new().unwrap();
	let parent_chain: Vec<Vec<String>> = vec![];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();
	assert_eq!(path, shell_path);
}

#[test]
fn empty_inner_chain_layer_does_not_emit_stray_separator() {
	// Defensive: a chain entry that's an empty Vec (e.g. ancestor had
	// `addToPath: []`) shouldn't add a stray separator that turns into an
	// empty PATH segment.
	let dir = TempDir::new().unwrap();
	let parent_chain: Vec<Vec<String>> = vec![vec![]];

	let params = EnvBuildParams {
		env_files: None,
		env: None,
		add_to_path: None,
		working_dir: dir.path(),
		env_files_base_dir: dir.path(),
		available_private_keys: None,
		base_env: None,
		parent_add_to_path_chain: Some(&parent_chain),
	};
	let env = build_env(&params, &no_substitute).unwrap();
	let path = get_path_value(&env);
	let shell_path = std::env::var("PATH").unwrap_or_default();
	assert_eq!(path, shell_path, "empty chain layer should not perturb PATH");
}
