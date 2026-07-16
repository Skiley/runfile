use crate::args::{RunArgs, RunContext, SubstitutionError, check_env_case_duplicates, scan_args_usage, validate_args};
use crate::env::{build_env, load_env_files, parse_env_file};
use crate::executor::{execute_command, execute_parallel};
use runfile_parser::{CommandSpec, CommandStep, EnvValue};
use runfile_shell::{ResolvedShell, ShellKind, detect_default_shell};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper: case-insensitive lookup for PATH (Windows uses "Path", Unix uses "PATH").
fn get_path_value(env: &HashMap<String, String>) -> &str {
	env.iter()
		.find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
		.map(|(_, v)| v.as_str())
		.expect("PATH should be present in env")
}

/// Helper: substitute with an empty env map (for existing ARGS-only tests).
trait SubstituteNoEnv {
	fn substitute_no_env(&self, input: &str) -> Result<String, SubstitutionError>;
}
impl SubstituteNoEnv for RunArgs {
	fn substitute_no_env(&self, input: &str) -> Result<String, SubstitutionError> {
		self.substitute(input, &HashMap::new())
	}
}

/// Helper: parse a single-target Runfile JSON and return its (validated) [`CommandSpec`].
fn parse_target(json: &str, target_name: &str) -> CommandSpec {
	let rf = runfile_parser::parse_runfile(json).expect("test runfile must parse");
	rf.targets
		.into_iter()
		.find(|(k, _)| k == target_name)
		.expect("target not found")
		.1
}

// ── RunArgs parsing tests ──────────────────────────────────────────

#[test]
fn parse_empty_args() {
	let args = RunArgs::parse(&[]);
	assert!(args.original.is_empty());
	assert!(args.named.is_empty());
}

#[test]
fn parse_positional_args() {
	let args = RunArgs::parse(&["foo".into(), "bar".into(), "baz".into()]);
	assert_eq!(args.original, vec!["foo", "bar", "baz"]);
	assert!(args.named.is_empty());
}

#[test]
fn parse_named_args_with_equals() {
	let args = RunArgs::parse(&["--env=production".into(), "--port=3000".into()]);
	assert_eq!(args.named["env"], "production");
	assert_eq!(args.named["port"], "3000");
}

#[test]
fn parse_named_args_with_space() {
	let args = RunArgs::parse(&["--env".into(), "production".into()]);
	assert_eq!(args.named["env"], "production");
}

#[test]
fn parse_mixed_args() {
	let args = RunArgs::parse(&[
		"positional".into(),
		"--flag".into(),
		"--env=dev".into(),
		"another".into(),
	]);
	assert_eq!(args.named["env"], "dev");
	assert_eq!(args.named["flag"], "");
}

#[test]
fn parse_flag_without_value() {
	let args = RunArgs::parse(&["--verbose".into()]);
	assert_eq!(args.named["verbose"], "");
}

// ── Substitution tests ─────────────────────────────────────────────

#[test]
fn substitute_args_placeholder() {
	let args = RunArgs::parse(&["release".into(), "fast".into()]);
	let result = args.substitute_no_env("cargo build {{ ARGS }}").unwrap();
	assert_eq!(result, "cargo build release fast");
}

#[test]
fn substitute_args_passes_dashes_through() {
	// "run build --help" → "cargo build --help"
	let args = RunArgs::parse(&["--help".into()]);
	let result = args.substitute_no_env("cargo build {{ ARGS }}").unwrap();
	assert_eq!(result, "cargo build --help");
}

#[test]
fn substitute_args_passes_everything_through() {
	// "run build 333 44 $ARGS" → "cargo build 333 44 $ARGS"
	let args = RunArgs::parse(&["333".into(), "44".into(), "$ARGS".into()]);
	let result = args.substitute_no_env("cargo build {{ ARGS }}").unwrap();
	assert_eq!(result, "cargo build 333 44 $ARGS");
}

#[test]
fn substitute_named_consumed_removed_from_args() {
	// "run build --env dev --help" with "cargo build env:{{ ARG.env ? 'test' }} {{ ARGS }}"
	// → "cargo build env:dev --help"
	let args = RunArgs::parse(&["--env".into(), "dev".into(), "--help".into()]);
	let result = args
		.substitute_no_env("cargo build env:{{ ARG.env ? 'test' }} {{ ARGS }}")
		.unwrap();
	assert_eq!(result, "cargo build env:dev --help");
}

#[test]
fn substitute_named_consumed_equals_form() {
	// "run build --env=dev --help" → consumed --env=dev, {{ ARGS }} = "--help"
	let args = RunArgs::parse(&["--env=dev".into(), "--help".into()]);
	let result = args
		.substitute_no_env("cargo build env:{{ ARG.env ? 'test' }} {{ ARGS }}")
		.unwrap();
	assert_eq!(result, "cargo build env:dev --help");
}

#[test]
fn substitute_named_default_not_consumed_from_args() {
	// "run build --help" with "cargo build env:{{ ARG.env ? 'test' }} {{ ARGS }}"
	// --env was NOT provided, so default "test" is used, and --help stays in {{ ARGS }}
	let args = RunArgs::parse(&["--help".into()]);
	let result = args
		.substitute_no_env("cargo build env:{{ ARG.env ? 'test' }} {{ ARGS }}")
		.unwrap();
	assert_eq!(result, "cargo build env:test --help");
}

#[test]
fn substitute_named_arg() {
	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = args.substitute_no_env("echo {{ ARG.env }}").unwrap();
	assert_eq!(result, "echo staging");
}

#[test]
fn substitute_named_arg_with_default() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo {{ ARG.env ? 'development' }}").unwrap();
	assert_eq!(result, "echo development");
}

#[test]
fn substitute_named_arg_overrides_default() {
	let args = RunArgs::parse(&["--env=production".into()]);
	let result = args.substitute_no_env("echo {{ ARG.env ? 'development' }}").unwrap();
	assert_eq!(result, "echo production");
}

#[test]
fn substitute_preserves_shell_dollar_var() {
	// Bare shell variables like `$HOME` are not Runfile substitutions and
	// must pass through verbatim to the shell.
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $HOME and {{ ARGS }}").unwrap();
	assert_eq!(result, "echo $HOME and ");
}

#[test]
fn substitute_multiple_named_consumed() {
	// Both --port and --host are consumed by {{ ARG.key }}, so {{ ARGS }} should be empty
	let args = RunArgs::parse(&["--port=3000".into(), "--host=localhost".into()]);
	let result = args
		.substitute_no_env("server --port={{ ARG.port ? '8080' }} --host={{ ARG.host ? '0.0.0.0' }}")
		.unwrap();
	assert_eq!(result, "server --port=3000 --host=localhost");
}

#[test]
fn substitute_order_independent() {
	// "run build --help --env dev" → same as "--env dev --help"
	let args = RunArgs::parse(&["--help".into(), "--env".into(), "dev".into()]);
	let result = args
		.substitute_no_env("cargo build env:{{ ARG.env ? 'test' }} {{ ARGS }}")
		.unwrap();
	assert_eq!(result, "cargo build env:dev --help");
}

#[test]
fn substitute_no_args_no_substitution() {
	// "run build" with "cargo build {{ ARGS }}" → "cargo build "
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("cargo build {{ ARGS }}").unwrap();
	assert_eq!(result, "cargo build ");
}

// ── Redacted substitution tests ────────────────────────────────────

#[test]
fn substitute_redacted_hides_env_values() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("DB_PASSWORD".to_string(), "s3cret".to_string());
	let result = args
		.substitute_redacted("connect --password={{ ENV.DB_PASSWORD }}", &env)
		.unwrap();
	assert_eq!(result, "connect --password=***");
	assert!(!result.contains("s3cret"), "secret should be redacted");
}

#[test]
fn substitute_redacted_shows_args_values() {
	let args = RunArgs::parse(&["--env=production".into()]);
	let env = HashMap::new();
	let result = args.substitute_redacted("deploy {{ ARG.env }}", &env).unwrap();
	assert_eq!(result, "deploy production");
}

#[test]
fn substitute_redacted_shows_positional_args() {
	let args = RunArgs::parse(&["hello".into(), "world".into()]);
	let env = HashMap::new();
	let result = args.substitute_redacted("echo {{ ARGS }}", &env).unwrap();
	assert_eq!(result, "echo hello world");
}

#[test]
fn substitute_redacted_mixed_args_and_env() {
	let args = RunArgs::parse(&["--host=example.com".into()]);
	let mut env = HashMap::new();
	env.insert("SECRET_TOKEN".to_string(), "tok_abc123".to_string());
	let result = args
		.substitute_redacted(
			"curl -H 'Authorization: {{ ENV.SECRET_TOKEN }}' https://{{ ARG.host }}/api",
			&env,
		)
		.unwrap();
	assert_eq!(result, "curl -H 'Authorization: ***' https://example.com/api");
}

#[test]
fn substitute_redacted_env_with_default_still_redacts() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("TOKEN".to_string(), "actual_value".to_string());
	let result = args
		.substitute_redacted("echo {{ ENV.TOKEN ? 'fallback' }}", &env)
		.unwrap();
	assert_eq!(result, "echo ***");
}

#[test]
fn substitute_redacted_env_missing_uses_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args
		.substitute_redacted("echo {{ ENV.MISSING ? 'fallback' }}", &env)
		.unwrap();
	assert_eq!(result, "echo fallback");
}

#[test]
fn substitute_redacted_chained_args_then_env() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("DB_HOST".to_string(), "db.internal".to_string());
	// ARG.host not provided, falls through to ENV.DB_HOST → redacted
	let result = args
		.substitute_redacted("connect {{ ARG.host ? ENV.DB_HOST }}", &env)
		.unwrap();
	assert_eq!(result, "connect ***");
}

#[test]
fn substitute_redacted_chained_args_provided() {
	let args = RunArgs::parse(&["--host=localhost".into()]);
	let mut env = HashMap::new();
	env.insert("DB_HOST".to_string(), "db.internal".to_string());
	// ARG.host IS provided → shown as-is (not redacted)
	let result = args
		.substitute_redacted("connect {{ ARG.host ? ENV.DB_HOST }}", &env)
		.unwrap();
	assert_eq!(result, "connect localhost");
}

#[test]
fn substitute_redacted_flags_shown() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let env = HashMap::new();
	let result = args
		.substitute_redacted("cmd {{ FLAG.verbose ? '--verbose' : }}", &env)
		.unwrap();
	assert_eq!(result, "cmd --verbose");
}

#[test]
fn substitute_redacted_preserves_non_placeholder_text() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute_redacted("echo hello world", &env).unwrap();
	assert_eq!(result, "echo hello world");
}

// ── Environment building tests ─────────────────────────────────────

#[test]
fn build_env_with_no_extras() {
	let spec = CommandSpec::new(vec!["echo".into()]);
	let env = build_env(
		&spec,
		&PathBuf::from("."),
		&PathBuf::from("."),
		&RunArgs::default(),
		None,
	)
	.unwrap();
	// Should at least contain system PATH (may be "Path" on Windows)
	assert!(
		env.keys().any(|k| k.eq_ignore_ascii_case("PATH")),
		"env should contain a PATH variable"
	);
}

#[test]
fn build_env_with_global_env() {
	let mut global_env = HashMap::new();
	global_env.insert("MY_VAR".into(), EnvValue::String("hello".into()));

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(global_env);

	let env = build_env(
		&spec,
		&PathBuf::from("."),
		&PathBuf::from("."),
		&RunArgs::default(),
		None,
	)
	.unwrap();
	assert_eq!(env.get("MY_VAR").unwrap(), "hello");
}

#[test]
fn build_env_command_overrides_global() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::Number(5000.0));

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);

	let env = build_env(
		&spec,
		&PathBuf::from("."),
		&PathBuf::from("."),
		&RunArgs::default(),
		None,
	)
	.unwrap();
	assert_eq!(env.get("PORT").unwrap(), "5000");
}

#[test]
fn build_env_add_to_path() {
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.add_to_path = Some(vec!["node_modules/.bin".into()]);

	let working_dir = PathBuf::from("/project");
	let env = build_env(&spec, &working_dir, &working_dir, &RunArgs::default(), None).unwrap();
	let path = get_path_value(&env);

	// The added path should be at the beginning (normalize for cross-platform)
	let normalized = path.replace('\\', "/");
	assert!(
		normalized.starts_with("/project/node_modules/.bin"),
		"PATH should start with added dir, got: {normalized}"
	);
}

#[test]
fn build_env_add_to_path_absolute() {
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.add_to_path = Some(vec!["/usr/local/custom/bin".into()]);

	let env = build_env(
		&spec,
		&PathBuf::from("/project"),
		&PathBuf::from("/project"),
		&RunArgs::default(),
		None,
	)
	.unwrap();
	let path = get_path_value(&env);
	assert!(path.contains("/usr/local/custom/bin"));
}

// ── Env value substitution tests ───────────────────────────────────

#[test]
fn build_env_substitutes_args_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::String("{{ ARG.port ? '3000' }}".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::parse(&["--port=4000".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("PORT").unwrap(), "4000");
}

#[test]
fn build_env_substitutes_args_default_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::String("{{ ARG.port ? '3000' }}".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("PORT").unwrap(), "3000");
}

#[test]
fn build_env_substitutes_flags_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert(
		"NODE_OPTIONS".into(),
		EnvValue::String("{{ FLAG.debug ? '--inspect' : }}".into()),
	);
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::parse(&["--debug".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("NODE_OPTIONS").unwrap(), "--inspect");
}

#[test]
fn build_env_substitutes_flags_false_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("DEBUG".into(), EnvValue::String("{{ FLAG.debug }}".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("DEBUG").unwrap(), "false");
}

#[test]
fn build_env_substitutes_args_in_global_env() {
	let mut global_env = HashMap::new();
	global_env.insert(
		"ENV_NAME".into(),
		EnvValue::String("{{ ARG.env ? 'development' }}".into()),
	);
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(global_env);
	let args = RunArgs::parse(&["--env=staging".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("ENV_NAME").unwrap(), "staging");
}

#[test]
fn build_env_substitutes_flags_in_global_env() {
	let mut global_env = HashMap::new();
	global_env.insert("VERBOSE".into(), EnvValue::String("{{ FLAG.verbose }}".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(global_env);
	let args = RunArgs::parse(&["--verbose".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("VERBOSE").unwrap(), "true");
}

#[test]
fn build_env_env_can_reference_system_env_via_substitution() {
	// Env vars can reference system env vars via {{ ENV.VAR }} substitution
	let mut cmd_env = HashMap::new();
	cmd_env.insert(
		"MY_PATH_COPY".into(),
		EnvValue::String("{{ ENV.PATH ? 'fallback' }}".into()),
	);
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	// PATH should exist in system env, so the substitution should use the real value
	let path_copy = env.get("MY_PATH_COPY").unwrap();
	assert_ne!(
		path_copy, "fallback",
		"Should have resolved {{ ENV.PATH }} from system env"
	);
}

#[test]
fn build_env_non_string_env_values_not_substituted() {
	// Numbers and booleans don't contain {{  }} patterns, so substitution is a no-op
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::Number(8080.0));
	cmd_env.insert("ENABLED".into(), EnvValue::Bool(true));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("PORT").unwrap(), "8080");
	assert_eq!(env.get("ENABLED").unwrap(), "true");
}

// ── Execution tests (integration) ──────────────────────────────────

fn get_test_shell() -> ResolvedShell {
	detect_default_shell().expect("Need a shell for integration tests")
}

#[test]
fn execute_simple_echo() {
	let shell = get_test_shell();
	let spec = CommandSpec::new(vec!["echo hello".into()]);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn execute_multiple_commands() {
	let shell = get_test_shell();
	let spec = CommandSpec::new(vec!["echo first".into(), "echo second".into(), "echo third".into()]);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 3);
	assert!(result.final_status.success());
}

#[test]
fn execute_stops_on_failure() {
	let shell = get_test_shell();

	// Use a command that will fail
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};

	let spec = CommandSpec::new_shell(vec![fail_cmd, "echo should not run".into()]);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	// With the `when`-aware walker, a failure flips the target into
	// "failed" state instead of erroring out. Subsequent default-when
	// (success) commands are skipped, so only the failing command ran.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success(), "target should report non-zero exit");
	assert_eq!(
		result.commands_run, 1,
		"second `when:success` command should be skipped"
	);
	assert_eq!(result.failures, 1);
}

#[test]
fn execute_with_env_vars() {
	let shell = get_test_shell();

	let echo_cmd = if shell.kind == ShellKind::Cmd {
		"echo %MY_TEST_VAR%".to_string()
	} else {
		"echo $MY_TEST_VAR".to_string()
	};

	let mut cmd_env = HashMap::new();
	cmd_env.insert("MY_TEST_VAR".into(), EnvValue::String("it_works".into()));

	let mut spec = CommandSpec::new_shell(vec![echo_cmd]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

// ── Substitution error tests ───────────────────────────────────────

#[test]
fn substitute_empty_default_returns_empty_string() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo {{ ARG.env ? }}").unwrap();
	assert_eq!(result, "echo ");
}

#[test]
fn substitute_missing_arg_without_default_errors() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo {{ ARG.env }}");
	assert!(result.is_err());
	let err = result.unwrap_err();
	assert!(err.to_string().contains("env"));
}

#[test]
fn substitute_present_arg_without_default_works() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args.substitute_no_env("echo {{ ARG.env }}").unwrap();
	assert_eq!(result, "echo prod");
}

// ── ENV substitution tests ────────────────────────────────────────

#[test]
fn substitute_env_basic() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "production".into());
	let result = args.substitute("echo {{ ENV.NODE_ENV }}", &env).unwrap();
	assert_eq!(result, "echo production");
}

#[test]
fn substitute_env_case_insensitive() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "production".into());
	let result = args.substitute("echo {{ ENV.node_env }}", &env).unwrap();
	assert_eq!(result, "echo production");
}

#[test]
fn substitute_env_with_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args
		.substitute("echo {{ ENV.NODE_ENV ? 'development' }}", &env)
		.unwrap();
	assert_eq!(result, "echo development");
}

#[test]
fn substitute_env_with_empty_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo {{ ENV.NODE_ENV ? }}", &env).unwrap();
	assert_eq!(result, "echo ");
}

#[test]
fn substitute_env_missing_errors() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo {{ ENV.NODE_ENV }}", &env);
	assert!(result.is_err());
	assert!(result.unwrap_err().to_string().contains("NODE_ENV"));
}

#[test]
fn substitute_env_present_overrides_default() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "staging".into());
	let result = args
		.substitute("echo {{ ENV.NODE_ENV ? 'development' }}", &env)
		.unwrap();
	assert_eq!(result, "echo staging");
}

#[test]
fn substitute_chain_args_then_env() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("MY_ENV".into(), "from_env".into());
	let result = args
		.substitute("echo {{ ARG.key ? ENV.MY_ENV ? 'fallback' }}", &env)
		.unwrap();
	assert_eq!(result, "echo from_env");
}

#[test]
fn substitute_chain_args_wins_over_env() {
	let args = RunArgs::parse(&["--key=from_args".into()]);
	let mut env = HashMap::new();
	env.insert("MY_ENV".into(), "from_env".into());
	let result = args
		.substitute("echo {{ ARG.key ? ENV.MY_ENV ? 'fallback' }}", &env)
		.unwrap();
	assert_eq!(result, "echo from_args");
}

#[test]
fn substitute_chain_falls_through_to_literal() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args
		.substitute("echo {{ ARG.key ? ENV.MY_ENV ? 'fallback' }}", &env)
		.unwrap();
	assert_eq!(result, "echo fallback");
}

#[test]
fn substitute_chain_env_then_env() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("SECONDARY".into(), "second".into());
	let result = args
		.substitute("echo {{ ENV.PRIMARY ? ENV.SECONDARY ? 'none' }}", &env)
		.unwrap();
	assert_eq!(result, "echo second");
}

#[test]
fn substitute_chain_with_empty_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo {{ ARG.key ? ENV.MISSING ? }}", &env).unwrap();
	assert_eq!(result, "echo ");
}

#[test]
fn substitute_env_and_args_in_same_command() {
	let args = RunArgs::parse(&["--port=9090".into()]);
	let mut env = HashMap::new();
	env.insert("HOST".into(), "localhost".into());
	let result = args
		.substitute("server --host={{ ENV.HOST }} --port={{ ARG.port ? '8080' }}", &env)
		.unwrap();
	assert_eq!(result, "server --host=localhost --port=9090");
}

#[test]
fn check_env_case_duplicates_ok() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "test".into());
	env.insert("PATH".into(), "/usr/bin".into());
	assert!(check_env_case_duplicates(&env).is_ok());
}

#[test]
fn check_env_case_duplicates_detects_conflict() {
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "test".into());
	env.insert("node_env".into(), "prod".into());
	let result = check_env_case_duplicates(&env);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("NODE_ENV") || err.contains("node_env"));
}

// ── ignoreErrors tests ─────────────────────────────────────────────

#[test]
fn execute_ignore_errors_continues_on_failure() {
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};

	let mut spec = CommandSpec::new_shell(vec![fail_cmd, "echo still running".into()]);
	spec.ignore_errors = Some(true);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
	assert_eq!(result.failures, 1);
}

#[test]
fn execute_ignore_errors_from_global() {
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};

	let mut spec = CommandSpec::new_shell(vec![fail_cmd, "echo still running".into()]);
	spec.ignore_errors = Some(true);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
	assert_eq!(result.failures, 1);
}

#[test]
fn execute_ignore_errors_command_overrides_global() {
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};

	let mut spec = CommandSpec::new_shell(vec![fail_cmd, "echo should not run".into()]);
	spec.ignore_errors = Some(false);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	// `ignore_errors: false` means failure should flip target into the
	// "failed" state and skip subsequent `when: success` (default) commands.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success());
	assert_eq!(result.commands_run, 1, "second command should be skipped post-failure");
	assert_eq!(result.failures, 1);
}

#[test]
fn dep_ignore_errors_does_not_abort_parent() {
	// Regression: a target with `ignoreErrors: true` invoked via `@target`
	// from a parent that does NOT have `ignoreErrors` should fully contain
	// its failures. Previously the dep's `failures` count was folded into the
	// parent's `state.failures`, which flipped the parent's `failed` flag and
	// caused subsequent default-`when: success` siblings to be skipped.
	// Symmetric with how `for ... ignoreErrors: true` already works.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let marker = dir.path().join("after_dep");
	let marker_escaped = json_escape_path(&marker);
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{marker_escaped}\\\"")
	} else {
		format!("touch \\\"{marker_escaped}\\\"")
	};
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "_dep": {{ "commands": ["{fail_cmd}", "{fail_cmd}"], "ignoreErrors": true }},
            "parent": {{ "commands": ["@_dep", "{create_marker}"] }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();

	let result = run_target("parent", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(
		marker.exists(),
		"Sibling step after `@_dep` (which had ignoreErrors: true) should have run"
	);
	assert!(
		result.final_status.success(),
		"parent should report success because the dep's failures were contained"
	);
	assert_eq!(
		result.failures, 0,
		"dep's failures should not surface to the caller's failure count"
	);
}

// ── Command-level addToPath tests ──────────────────────────────────

#[test]
fn build_env_command_add_to_path() {
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.add_to_path = Some(vec!["my-tools/bin".into()]);

	let working_dir = PathBuf::from("/project");
	let env = build_env(&spec, &working_dir, &working_dir, &RunArgs::default(), None).unwrap();
	let path = get_path_value(&env);
	let normalized = path.replace('\\', "/");
	assert!(
		normalized.starts_with("/project/my-tools/bin"),
		"PATH should start with command addToPath, got: {normalized}"
	);
}

#[test]
fn build_env_command_add_to_path_before_global() {
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.add_to_path = Some(vec!["cmd/bin".into(), "global/bin".into()]);

	let working_dir = PathBuf::from("/project");
	let env = build_env(&spec, &working_dir, &working_dir, &RunArgs::default(), None).unwrap();
	let path = get_path_value(&env);
	let normalized = path.replace('\\', "/");

	let cmd_pos = normalized.find("/project/cmd/bin").expect("cmd/bin should be in PATH");
	let global_pos = normalized
		.find("/project/global/bin")
		.expect("global/bin should be in PATH");
	assert!(
		cmd_pos < global_pos,
		"Command addToPath should come before global addToPath"
	);
}

// ── Logging tests ──────────────────────────────────────────────────

#[test]
fn logging_disabled_by_default() {
	use crate::logging::is_logging_enabled;

	let spec = CommandSpec::new(vec!["echo".into()]);
	assert!(!is_logging_enabled(&spec));
}

#[test]
fn logging_enabled_by_global() {
	use crate::logging::is_logging_enabled;

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.logging = Some(true);
	assert!(is_logging_enabled(&spec));
}

#[test]
fn logging_command_overrides_global() {
	use crate::logging::is_logging_enabled;

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.logging = Some(false);
	assert!(!is_logging_enabled(&spec));
}

#[test]
fn logging_command_enables_without_global() {
	use crate::logging::is_logging_enabled;

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.logging = Some(true);
	assert!(is_logging_enabled(&spec));
}

#[test]
fn execute_with_args_substitution() {
	let shell = get_test_shell();
	let spec = CommandSpec::new(vec!["echo {{ ARGS }}".into()]);
	let args = RunArgs::parse(&["hello".into(), "world".into()]);
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn execute_set_cwd_changes_spawn_dir() {
	// `set_cwd('subdir')` should make subsequent commands spawn in `dir/subdir`.
	// We verify this by writing a marker file from the spawned shell — its
	// final location tells us which cwd the shell saw.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let subdir = dir.path().join("subdir");
	std::fs::create_dir(&subdir).unwrap();

	let marker_name = "set_cwd_marker.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};

	let spec = CommandSpec::new_shell(vec!["{{ set_cwd('subdir') }}".into(), create_marker]);
	let args = RunArgs::default();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
	// The `set_cwd` line is a side-effect-only substitution that resolves to "" —
	// it must NOT count as a dispatched command.
	assert_eq!(result.commands_run, 1, "set_cwd line should not count as a step");
	assert!(
		subdir.join(marker_name).exists(),
		"marker should land in subdir (cwd switched via set_cwd)"
	);
	assert!(
		!dir.path().join(marker_name).exists(),
		"marker should NOT land in parent dir"
	);
}

#[test]
fn execute_set_cwd_chains_relative() {
	// Two relative `set_cwd` calls should compose: dir/sub1/sub2.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let nested = dir.path().join("sub1").join("sub2");
	std::fs::create_dir_all(&nested).unwrap();

	let marker_name = "chained.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};

	let spec = CommandSpec::new_shell(vec![
		"{{ set_cwd('sub1') }}".into(),
		"{{ set_cwd('sub2') }}".into(),
		create_marker,
	]);
	let args = RunArgs::default();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
	assert!(
		nested.join(marker_name).exists(),
		"marker should land in sub1/sub2 after two relative set_cwd calls"
	);
}

#[test]
fn execute_set_cwd_absolute_path() {
	// An absolute `set_cwd` should fully replace the working_dir.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let other = TempDir::new().unwrap();

	let marker_name = "absolute.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};

	// Embed the absolute path inside a single-quoted literal — backslashes
	// pass through verbatim because Runfile single-quoted strings are not
	// re-escaped.
	let absolute = other.path().display().to_string();
	let set_cwd_line = format!("{{{{ set_cwd('{}') }}}}", absolute.replace('\\', "/"));

	let spec = CommandSpec::new_shell(vec![set_cwd_line, create_marker]);
	let args = RunArgs::default();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
	assert!(
		other.path().join(marker_name).exists(),
		"marker should land in the absolute path passed to set_cwd"
	);
	assert!(
		!dir.path().join(marker_name).exists(),
		"marker should NOT land in the original working_dir"
	);
}

// ── Runner / dependency tests ──────────────────────────────────────

/// Escape backslashes for embedding paths in JSON strings.
fn json_escape_path(path: &std::path::Path) -> String {
	path.display().to_string().replace('\\', "\\\\")
}

#[test]
fn run_target_with_dependency() {
	// Migrated from `before.target` lifecycle hooks to `@target` invocations
	// in `commands` (lifecycle was removed; deps live inline now).
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let marker = dir.path().join("dep_ran");
	let marker_escaped = json_escape_path(&marker);
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{marker_escaped}\\\"")
	} else {
		format!("touch \\\"{marker_escaped}\\\"")
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "setup": {{ "commands": ["{create_marker}"] }},
            "build": {{ "commands": ["@setup", "echo building"] }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();

	let result = run_target("build", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success());
	assert!(marker.exists(), "Dependency 'setup' should have run first");
}

#[test]
fn run_target_cycle_detection() {
	use crate::runner::{RunError, run_target};
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "a": { "commands": ["@b"] },
            "b": { "commands": ["@a"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();

	let result = run_target("a", &runfile, &shell, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err();
	let msg = err.to_string().to_lowercase();
	// Cycle detection now happens inside the executor's `@target` dispatch
	// (since lifecycle was removed) — the error may be wrapped in a
	// DependencyFailed; check the message for "cycle".
	assert!(
		matches!(err, RunError::CycleDetected(_)) || msg.contains("cycle"),
		"Expected cycle detection, got: {err}"
	);
}

mod arg_validation_tests;
mod control_flow_match_parallel;
mod declared_vars;
mod empty_command_skip;
mod env_file_tests;
mod escape_parallel_tests;
mod extract_tests;
mod flags_run_tests;
mod force_kill;
mod functions;
mod logging;
mod parallel_output;
mod prefix_rename;
mod quote_rework;
mod runtime_when_targetcall;
mod same_shell;
mod shell_quoting_tests;
mod stdin_args;
mod stdio_tailer;
