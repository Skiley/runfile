use super::*;

// ── Brace-escape tests ──────────────────────────────────────────────
//
// `\{{` and `\}}` are escapes for emitting a literal `{{` / `}}` in the
// output without triggering substitution. The backslash itself is consumed.
// A bare `\` (not followed by `{{` or `}}`) is preserved as-is.

#[test]
fn substitute_escaped_open_brace_emits_literal() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env(r"echo \{{ literal").unwrap();
	assert_eq!(result, "echo {{ literal");
}

#[test]
fn substitute_escaped_close_brace_emits_literal() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env(r"echo not a sub \}}").unwrap();
	assert_eq!(result, "echo not a sub }}");
}

#[test]
fn substitute_escaped_pair_emits_literal_braces() {
	// A user wanting to print a literal `{{ X }}` token (e.g. when generating
	// templates for Jinja, Handlebars, etc.) writes both halves escaped.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env(r"template uses \{{ name \}} as a placeholder")
		.unwrap();
	assert_eq!(result, "template uses {{ name }} as a placeholder");
}

#[test]
fn substitute_escape_does_not_interfere_with_real_substitution() {
	// An escaped `\{{` followed by a real `{{ ARG.x }}` should leave the
	// escape literal and still resolve the substitution.
	let args = RunArgs::parse(&["--name=alice".into()]);
	let result = args.substitute_no_env(r"prefix \{{ then {{ ARG.name }}").unwrap();
	assert_eq!(result, "prefix {{ then alice");
}

#[test]
fn substitute_lone_backslash_is_preserved() {
	// A backslash that isn't part of `\{{` or `\}}` passes through unchanged
	// — needed so shell commands like `\\\"` and Windows paths still work.
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env(r"echo C:\Windows\System32").unwrap();
	assert_eq!(result, r"echo C:\Windows\System32");
}

#[test]
fn substitute_double_backslash_before_brace_keeps_first_backslash() {
	// `\\{{` is `\` followed by an escaped `{{` — the inner escape consumes
	// `\{{` and emits literal `{{`, leaving the outer `\` intact.
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env(r"\\{{ literal \\}}").unwrap();
	assert_eq!(result, r"\{{ literal \}}");
}

#[test]
fn substitute_escape_inside_shell_command_substitution() {
	// Mixing escapes with shell `$(...)` substitution: the bash `$(...)`
	// passes through verbatim, the escape produces a literal `{{`, and any
	// real `{{ ARG.x }}` inside still resolves.
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args
		.substitute_no_env(r#"sh -c $(echo "got \{{ {{ ARG.env }} \}}")"#)
		.unwrap();
	assert_eq!(result, r#"sh -c $(echo "got {{ prod }}")"#);
}

#[test]
fn scan_args_usage_ignores_escaped_substitution() {
	// An escaped `\{{ ARG.x \}}` is a literal — it should NOT register `x`
	// as a referenced argument (the validator would otherwise accept --x and
	// the runtime would never resolve it).
	let cmds = vec![r"echo \{{ ARG.x \}}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(
		named.is_empty(),
		"escaped substitution must not register arg keys: got {named:?}"
	);
}

#[test]
fn scan_args_usage_default_with_parens() {
	// Literal default values can contain parens — `{{ ... }}` matching is
	// brace-based, so `()` in the default doesn't break the scanner.
	let cmds = vec!["echo {{ ARG.key ? default(value) }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("key"));
}

#[test]
fn scan_args_detects_named_with_chained_fallback() {
	let cmds = vec!["echo {{ ARG.env ? ENV.NODE_ENV ? 'production' }}".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"));
}

#[test]
fn validate_args_empty_commands_rejects_args() {
	let args = RunArgs::parse(&["foo".into()]);
	let cmds: Vec<String> = vec![];
	let result = validate_args(&args, &cmds);
	assert!(result.is_err());
}

#[test]
fn substitute_env_chain_first_wins() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("FIRST".into(), "found".into());
	env.insert("SECOND".into(), "also_found".into());
	let result = args
		.substitute("echo {{ ENV.FIRST ? ENV.SECOND ? 'fallback' }}", &env)
		.unwrap();
	assert_eq!(result, "echo found");
}

#[test]
fn check_env_case_duplicates_same_case_ok() {
	let mut env = HashMap::new();
	env.insert("KEY".into(), "a".into());
	env.insert("KEY".into(), "b".into()); // Overwrites, same key
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_empty_env_ok() {
	let env: HashMap<String, String> = HashMap::new();
	assert!(check_env_case_duplicates(&env).is_ok());
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — extract.rs (shell quoting)
// ══════════════════════════════════════════════════════════════════════

#[test]
fn extract_format_bash_value_with_single_quotes() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "it's alive".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	// Single quote inside value should be escaped with '\'' pattern
	assert!(lines[0].contains("it'\\''s alive"), "got: {}", lines[0]);
}

#[test]
fn extract_format_bash_value_with_dollar() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("PRICE".to_string(), "$100".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	// Dollar sign needs quoting
	assert!(lines[0].contains("'$100'"), "got: {}", lines[0]);
}

#[test]
fn extract_format_bash_empty_value_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("EMPTY".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	// Empty value needs quoting
	assert!(lines[0].contains("EMPTY=''"), "got: {}", lines[0]);
}

#[test]
fn extract_format_powershell_value_with_double_quotes() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "say \"hello\"".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Single-quoted PowerShell strings are verbatim — double quotes need no escaping
	assert_eq!(lines[0], "$env:MSG='say \"hello\"'; echo test");
}

#[test]
fn extract_format_powershell_empty_value() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("EMPTY".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	assert!(lines[0].contains("$env:EMPTY=''"), "got: {}", lines[0]);
}

#[test]
fn extract_format_fish_value_with_single_quotes() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "it's fish".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert!(lines[0].contains("env"), "Fish should use env prefix");
	// Fish quoting: close quote, escaped quote, reopen: 'it'\''s fish'
	assert!(
		lines[0].contains("'\\''"),
		"Fish should escape single quotes with close-escape-reopen, got: {}",
		lines[0]
	);
}

#[test]
fn extract_format_cmd_multiple_env_vars() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![
			("A".to_string(), "1".to_string()),
			("B".to_string(), "2".to_string()),
			("C".to_string(), "3".to_string()),
		],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"A=1\" && set \"B=2\" && set \"C=3\" && echo test");
}

#[test]
fn extract_format_bash_simple_value_not_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("KEY".to_string(), "simple".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	// Simple alphanumeric value should not be quoted
	assert_eq!(lines[0], "KEY=simple echo test");
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — logging.rs
// ══════════════════════════════════════════════════════════════════════

#[test]
fn logging_default_spec_none() {
	use crate::logging::is_logging_enabled;
	let spec = CommandSpec::new(vec!["echo default".into()]);
	assert!(!is_logging_enabled(&spec));
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — runner.rs
// ══════════════════════════════════════════════════════════════════════

#[test]
fn run_target_unknown_target_errors() {
	use crate::runner::{RunError, run_target};
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo build"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = run_target("nonexistent", &runfile, &shell, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err();
	assert!(
		matches!(err, RunError::UnknownTarget(_)),
		"Expected UnknownTarget, got: {err}"
	);
}

#[test]
fn run_target_global_depends_on_skips_self() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	// After globals merge, self-referencing before steps are filtered out.
	// A target with no before should run successfully.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "init": { "commands": ["echo init"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();

	let result = run_target("init", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn extract_unknown_target_errors() {
	use crate::extract::extract_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo build"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = extract_target("nonexistent", &runfile, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("nonexistent"), "Expected unknown target error, got: {err}");
}

#[test]
fn extract_with_working_directory_cwd() {
	use crate::extract::extract_target_with_cwd;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "workingDirectory": "{{ RUN.cwd }}"
            }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();

	let runfile_path = runfile_dir.path().join("Runfile.json");
	let commands = extract_target_with_cwd(
		"test",
		&runfile,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
		&std::collections::HashMap::new(),
		None,
		&ShellKind::Bash,
	)
	.unwrap();
	assert_eq!(commands.len(), 1);
	assert_eq!(commands[0].command, "echo test");
}

// Regression: --dry-run must thread `available_private_keys` into the env-build
// pipeline so that envFiles containing `RUNFILE_ENCRYPTION_PUBLIC_KEY` +
// encrypted values resolve the same way as a real run. Previously the extract
// path hardcoded `None`, surfacing "no private keys are available" even when
// the user had keys registered via `:env secret-keys add`.
#[test]
fn extract_decrypts_envfile_when_private_key_provided() {
	use crate::extract::extract_target_with_cwd;
	use runfile_parser::Runfile;
	use std::fs;

	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	let encrypted = runfile_crypto::encrypt("super-secret-value", &key_hex).unwrap();
	let private_keys = vec![key_hex];

	let env_path = dir.path().join(".env.production");
	fs::write(
		&env_path,
		format!(
			"{}={public_key}\nMY_SECRET={encrypted}\n",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR,
		),
	)
	.unwrap();

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": ["echo {{ ENV.MY_SECRET }}"],
                "envFiles": [".env.production"]
            }
        }
    }"#;
	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();

	let runfile_path = dir.path().join("Runfile.json");

	// Without keys: should hit the same error path the real run would.
	let err = extract_target_with_cwd(
		"deploy",
		&runfile,
		&args,
		&runfile_path,
		dir.path(),
		dir.path(),
		&std::collections::HashMap::new(),
		&std::collections::HashMap::new(),
		None,
		&ShellKind::Bash,
	)
	.unwrap_err()
	.to_string();
	assert!(
		err.contains("no private keys are available"),
		"Expected no-keys error, got: {err}"
	);

	// With keys: decryption succeeds and the substituted command shows the
	// plaintext (this is exactly what `--dry-run` prints).
	let commands = extract_target_with_cwd(
		"deploy",
		&runfile,
		&args,
		&runfile_path,
		dir.path(),
		dir.path(),
		&std::collections::HashMap::new(),
		&std::collections::HashMap::new(),
		Some(&private_keys),
		&ShellKind::Bash,
	)
	.unwrap();
	assert_eq!(commands.len(), 1);
	assert_eq!(commands[0].command, "echo super-secret-value");
}

// Regression: env-block substitutions must see DECRYPTED values from envFiles,
// not the raw `encrypted:...` form. This is what makes patterns like
// `"ENV": { "JSON": "{{ base64_decode(ENV.SECRET_BASE64) }}" }` work when
// `SECRET_BASE64` is encrypted in the env file. Before the build_env reorder,
// the env block ran first and saw `encrypted:abc...`, so any post-processing
// (`base64_decode`, comparisons, function calls) would error or get wrong
// results.
#[test]
fn env_block_sees_decrypted_envfile_values() {
	use crate::env::build_env_with_base;
	use runfile_parser::{CommandSpec, EnvValue};
	use std::fs;

	let dir = TempDir::new().unwrap();
	let key_hex = runfile_crypto::generate_key();
	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap();
	// Encrypt a base64-encoded payload (mirrors the GOOGLE_PLAY_SERVICE_ACCOUNT_JSON_BASE64
	// pattern: file holds an encrypted value whose plaintext is base64).
	let plaintext_b64 = "aGVsbG8gd29ybGQ="; // "hello world"
	let encrypted = runfile_crypto::encrypt(plaintext_b64, &key_hex).unwrap();
	let private_keys = vec![key_hex];

	let env_path = dir.path().join(".env.production");
	fs::write(
		&env_path,
		format!(
			"{}={public_key}\nSECRET_BASE64={encrypted}\n",
			runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR,
		),
	)
	.unwrap();

	// Spec with an env block that base64_decodes the (encrypted) ENV value.
	// If decryption hadn't run before the env block, this would surface
	// `InvalidBase64` because `encrypted:abc...` isn't valid base64.
	let mut spec = CommandSpec::new(vec!["echo $DECODED".into()]);
	let mut env_block = HashMap::new();
	env_block.insert(
		"DECODED".to_string(),
		EnvValue::String("{{ base64_decode(ENV.SECRET_BASE64) }}".to_string()),
	);
	spec.env = Some(env_block);
	spec.env_files = Some(vec![".env.production".to_string()]);

	let args = RunArgs::default();
	let env = build_env_with_base(&spec, dir.path(), dir.path(), &args, Some(&private_keys), None, None).unwrap();

	// `DECODED` should hold the base64-decoded plaintext, NOT an error/empty/encrypted form.
	assert_eq!(env.get("DECODED").map(String::as_str), Some("hello world"));
}

#[test]
fn extract_format_bash_value_with_special_chars() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("CMD".to_string(), "echo {{ whoami }} | cat".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	// Value with $, (, |, ) should all trigger quoting
	assert!(
		lines[0].starts_with("CMD='"),
		"Special chars should trigger quoting, got: {}",
		lines[0]
	);
}

#[test]
fn extract_format_bash_value_with_tab_and_newline() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("TAB".to_string(), "a\tb".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	assert!(
		lines[0].contains("'a\tb'"),
		"Tab should trigger quoting, got: {}",
		lines[0]
	);
}

// ──── parallel execution tests ────

#[test]
fn parallel_all_succeed() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	// Each command creates a file — proves all ran
	let (cmd1, cmd2, cmd3) = if shell.kind == ShellKind::Cmd {
		(
			format!("echo a > \"{}\"", dir.path().join("a.txt").display()),
			format!("echo b > \"{}\"", dir.path().join("b.txt").display()),
			format!("echo c > \"{}\"", dir.path().join("c.txt").display()),
		)
	} else {
		(
			format!("touch '{}'", dir.path().join("a.txt").display()),
			format!("touch '{}'", dir.path().join("b.txt").display()),
			format!("touch '{}'", dir.path().join("c.txt").display()),
		)
	};

	let mut spec = CommandSpec::new_shell(vec![cmd1, cmd2, cmd3]);
	spec.parallel = Some(true);
	let args = RunArgs::default();

	let result = execute_parallel(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 3);
	assert_eq!(result.failures, 0);
	assert!(result.final_status.success());
	assert!(dir.path().join("a.txt").exists());
	assert!(dir.path().join("b.txt").exists());
	assert!(dir.path().join("c.txt").exists());
}

#[test]
fn parallel_one_fails() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};

	let mut spec = CommandSpec::new_shell(vec!["echo ok".into(), fail_cmd]);
	spec.parallel = Some(true);
	let args = RunArgs::default();

	let result = execute_parallel(&spec, &shell, &args, dir.path(), None, false);
	assert!(result.is_err());
}

#[test]
fn parallel_with_ignore_errors() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};

	let mut spec = CommandSpec::new_shell(vec!["echo ok".into(), fail_cmd]);
	spec.ignore_errors = Some(true);
	spec.parallel = Some(true);
	let args = RunArgs::default();

	let result = execute_parallel(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
	assert_eq!(result.failures, 1);
}

// ── parallel-failure summary helpers ─────────────────────────────────

#[test]
fn format_target_call_label_variants() {
	use crate::executor::format_target_call_label;
	assert_eq!(format_target_call_label("build", &[], false), "@build");
	assert_eq!(format_target_call_label("build", &[], true), "@?build");
	assert_eq!(
		format_target_call_label("web-user:build:infrastructure", &[], false),
		"@web-user:build:infrastructure"
	);
	assert_eq!(
		format_target_call_label("dep", &["--env".into(), "prod".into()], false),
		"@dep --env prod"
	);
	assert_eq!(
		format_target_call_label("dep", &["a".into(), "b".into()], true),
		"@?dep a b"
	);
}

#[test]
fn execute_error_failure_detail_classifies() {
	use crate::executor::{ExecuteError, execute_error_failure_detail};
	assert_eq!(
		execute_error_failure_detail(&ExecuteError::NonZeroExit("cmd".into(), 1)),
		"exit code 1"
	);
	assert_eq!(
		execute_error_failure_detail(&ExecuteError::NonZeroExit("cmd".into(), 137)),
		"exit code 137"
	);
	assert_eq!(
		execute_error_failure_detail(&ExecuteError::Signal("cmd".into())),
		"terminated by signal"
	);
	let other = ExecuteError::DependencyFailed("foo".into(), "boom".into());
	assert!(execute_error_failure_detail(&other).starts_with("error: "));
}

#[test]
fn dep_result_failure_detail_returns_none_when_no_failures() {
	use crate::executor::{ExecutionResult, dep_result_failure_detail};
	use std::process::ExitStatus;
	#[cfg(unix)]
	let success = {
		use std::os::unix::process::ExitStatusExt;
		ExitStatus::from_raw(0)
	};
	#[cfg(windows)]
	let success = {
		use std::os::windows::process::ExitStatusExt;
		ExitStatus::from_raw(0)
	};
	let result = ExecutionResult {
		commands_run: 3,
		failures: 0,
		final_status: success,
	};
	assert!(dep_result_failure_detail(&result).is_none());
}

#[test]
fn dep_result_failure_detail_uses_final_exit_code() {
	use crate::executor::{ExecutionResult, dep_result_failure_detail};
	use std::process::ExitStatus;
	#[cfg(unix)]
	let nonzero = {
		use std::os::unix::process::ExitStatusExt;
		ExitStatus::from_raw(2 << 8)
	};
	#[cfg(windows)]
	let nonzero = {
		use std::os::windows::process::ExitStatusExt;
		ExitStatus::from_raw(2)
	};
	let result = ExecutionResult {
		commands_run: 4,
		failures: 1,
		final_status: nonzero,
	};
	assert_eq!(dep_result_failure_detail(&result).as_deref(), Some("exit code 2"));
}
