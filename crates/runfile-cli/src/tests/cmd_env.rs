use crate::cmd_env::*;
use std::collections::HashMap;

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

// ── Audit M3: secret files written owner-only (0600) ──

#[cfg(unix)]
#[test]
fn write_secret_file_sets_owner_only_permissions() {
	use std::os::unix::fs::PermissionsExt;
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("secret.env");
	write_secret_file(&path, b"TOKEN=s3cret\n").unwrap();
	let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
	assert_eq!(mode, 0o600, "decrypted / secret output must be owner-only (0600)");
	assert_eq!(std::fs::read_to_string(&path).unwrap(), "TOKEN=s3cret\n");
}

#[test]
fn write_secret_file_writes_content() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("out.env");
	write_secret_file(&path, b"A=1\n").unwrap();
	assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=1\n");
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
