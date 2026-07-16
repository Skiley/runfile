use super::*;

// ── RUNFILE_TARGET env var tests ─────────────────────────────────
//
// Env vars are process-global, so all tests in this section serialize via
// `RUNFILE_TARGET_TEST_LOCK` to avoid clobbering each other.

use crate::runfile_helpers::{RUNFILE_TARGET_ENV_VAR, resolve_runfile_path, runfile_target_env};
use std::sync::Mutex;

static RUNFILE_TARGET_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Set or unset `RUNFILE_TARGET` for the duration of the closure. The lock
/// is acquired on entry and released on exit.
fn with_runfile_target<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
	let _guard = RUNFILE_TARGET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let prev = std::env::var(RUNFILE_TARGET_ENV_VAR).ok();
	match value {
		// TODO: Audit that the environment access only happens in single-threaded code.
		Some(v) => unsafe { std::env::set_var(RUNFILE_TARGET_ENV_VAR, v) },
		// TODO: Audit that the environment access only happens in single-threaded code.
		None => unsafe { std::env::remove_var(RUNFILE_TARGET_ENV_VAR) },
	}
	let result = f();
	match prev {
		// TODO: Audit that the environment access only happens in single-threaded code.
		Some(v) => unsafe { std::env::set_var(RUNFILE_TARGET_ENV_VAR, v) },
		// TODO: Audit that the environment access only happens in single-threaded code.
		None => unsafe { std::env::remove_var(RUNFILE_TARGET_ENV_VAR) },
	}
	result
}

#[test]
fn runfile_target_env_returns_none_when_unset() {
	with_runfile_target(None, || {
		assert!(runfile_target_env().is_none());
	});
}

#[test]
fn runfile_target_env_returns_path_when_set() {
	with_runfile_target(Some("custom/Runfile.json"), || {
		let result = runfile_target_env();
		assert_eq!(result.as_deref(), Some(std::path::Path::new("custom/Runfile.json")));
	});
}

#[test]
fn runfile_target_env_returns_none_when_set_empty() {
	// Empty string is treated as unset so users can clear the var without
	// having to unset it shell-wide.
	with_runfile_target(Some(""), || {
		assert!(runfile_target_env().is_none());
	});
}

#[test]
fn resolve_runfile_path_uses_env_var_when_no_flag() {
	let dir = tempfile::tempdir().unwrap();
	let runfile_path = dir.path().join("custom.runfile.json");
	fs::write(
		&runfile_path,
		r#"{"$schema":"v0","targets":{"hello":{"commands":["echo hi"]}}}"#,
	)
	.unwrap();

	with_runfile_target(Some(runfile_path.to_str().unwrap()), || {
		let resolved = resolve_runfile_path(None);
		// Compare canonicalized paths to avoid Windows path-prefix mismatches.
		let expected = std::fs::canonicalize(&runfile_path).unwrap();
		let resolved_canon = std::fs::canonicalize(&resolved).unwrap();
		assert_eq!(resolved_canon, expected);
	});
}

#[test]
fn resolve_runfile_path_explicit_flag_wins_over_env_var() {
	let dir = tempfile::tempdir().unwrap();
	let env_runfile = dir.path().join("env.runfile.json");
	let flag_runfile = dir.path().join("flag.runfile.json");
	fs::write(
		&env_runfile,
		r#"{"$schema":"v0","targets":{"a":{"commands":["echo a"]}}}"#,
	)
	.unwrap();
	fs::write(
		&flag_runfile,
		r#"{"$schema":"v0","targets":{"b":{"commands":["echo b"]}}}"#,
	)
	.unwrap();

	with_runfile_target(Some(env_runfile.to_str().unwrap()), || {
		let resolved = resolve_runfile_path(Some(&flag_runfile));
		let expected = std::fs::canonicalize(&flag_runfile).unwrap();
		let resolved_canon = std::fs::canonicalize(&resolved).unwrap();
		assert_eq!(resolved_canon, expected);
	});
}
