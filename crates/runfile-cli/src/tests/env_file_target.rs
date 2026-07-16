// ── RUNFILE_ENV_FILE_TARGET env var tests ─────────────────────────
//
// Mirrors the RUNFILE_TARGET pattern: the env var feeds a default into
// `:env inject` / `:env decrypt` when no positional path is given.

use crate::cmd_env::{RUNFILE_ENV_FILE_TARGET_ENV_VAR, env_file_target};
use std::sync::Mutex;

static RUNFILE_ENV_FILE_TARGET_TEST_LOCK: Mutex<()> = Mutex::new(());

fn with_runfile_env_file_target<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
	let _guard = RUNFILE_ENV_FILE_TARGET_TEST_LOCK
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let prev = std::env::var(RUNFILE_ENV_FILE_TARGET_ENV_VAR).ok();
	match value {
		// TODO: Audit that the environment access only happens in single-threaded code.
		Some(v) => unsafe { std::env::set_var(RUNFILE_ENV_FILE_TARGET_ENV_VAR, v) },
		// TODO: Audit that the environment access only happens in single-threaded code.
		None => unsafe { std::env::remove_var(RUNFILE_ENV_FILE_TARGET_ENV_VAR) },
	}
	let result = f();
	match prev {
		// TODO: Audit that the environment access only happens in single-threaded code.
		Some(v) => unsafe { std::env::set_var(RUNFILE_ENV_FILE_TARGET_ENV_VAR, v) },
		// TODO: Audit that the environment access only happens in single-threaded code.
		None => unsafe { std::env::remove_var(RUNFILE_ENV_FILE_TARGET_ENV_VAR) },
	}
	result
}

#[test]
fn env_file_target_returns_none_when_unset() {
	with_runfile_env_file_target(None, || {
		assert!(env_file_target().is_none());
	});
}

#[test]
fn env_file_target_returns_path_when_set() {
	with_runfile_env_file_target(Some("custom/.env"), || {
		assert_eq!(env_file_target().as_deref(), Some("custom/.env"));
	});
}

#[test]
fn env_file_target_returns_none_when_set_empty() {
	with_runfile_env_file_target(Some(""), || {
		assert!(env_file_target().is_none());
	});
}
