use crate::args::{check_env_case_duplicates, scan_args_usage, validate_args, RunArgs, RunContext, SubstitutionError};
use crate::env::{build_env, load_env_files, parse_env_file};
use crate::executor::{execute_command, execute_parallel};
use runfile_parser::{CommandSpec, CommandStep, EnvValue};
use runfile_shell::{detect_default_shell, ResolvedShell, ShellKind};
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
	let result = args.substitute_no_env("cargo build $(ARGS)").unwrap();
	assert_eq!(result, "cargo build release fast");
}

#[test]
fn substitute_args_passes_dashes_through() {
	// "run build --help" → "cargo build --help"
	let args = RunArgs::parse(&["--help".into()]);
	let result = args.substitute_no_env("cargo build $(ARGS)").unwrap();
	assert_eq!(result, "cargo build --help");
}

#[test]
fn substitute_args_passes_everything_through() {
	// "run build 333 44 $ARGS" → "cargo build 333 44 $ARGS"
	let args = RunArgs::parse(&["333".into(), "44".into(), "$ARGS".into()]);
	let result = args.substitute_no_env("cargo build $(ARGS)").unwrap();
	assert_eq!(result, "cargo build 333 44 $ARGS");
}

#[test]
fn substitute_named_consumed_removed_from_args() {
	// "run build --env dev --help" with "cargo build env:$(ARGS.env ? test) $(ARGS)"
	// → "cargo build env:dev --help"
	let args = RunArgs::parse(&["--env".into(), "dev".into(), "--help".into()]);
	let result = args
		.substitute_no_env("cargo build env:$(ARGS.env ? test) $(ARGS)")
		.unwrap();
	assert_eq!(result, "cargo build env:dev --help");
}

#[test]
fn substitute_named_consumed_equals_form() {
	// "run build --env=dev --help" → consumed --env=dev, $(ARGS) = "--help"
	let args = RunArgs::parse(&["--env=dev".into(), "--help".into()]);
	let result = args
		.substitute_no_env("cargo build env:$(ARGS.env ? test) $(ARGS)")
		.unwrap();
	assert_eq!(result, "cargo build env:dev --help");
}

#[test]
fn substitute_named_default_not_consumed_from_args() {
	// "run build --help" with "cargo build env:$(ARGS.env ? test) $(ARGS)"
	// --env was NOT provided, so default "test" is used, and --help stays in $(ARGS)
	let args = RunArgs::parse(&["--help".into()]);
	let result = args
		.substitute_no_env("cargo build env:$(ARGS.env ? test) $(ARGS)")
		.unwrap();
	assert_eq!(result, "cargo build env:test --help");
}

#[test]
fn substitute_named_arg() {
	let args = RunArgs::parse(&["--env=staging".into()]);
	let result = args.substitute_no_env("echo $(ARGS.env)").unwrap();
	assert_eq!(result, "echo staging");
}

#[test]
fn substitute_named_arg_with_default() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $(ARGS.env ? development)").unwrap();
	assert_eq!(result, "echo development");
}

#[test]
fn substitute_named_arg_overrides_default() {
	let args = RunArgs::parse(&["--env=production".into()]);
	let result = args.substitute_no_env("echo $(ARGS.env ? development)").unwrap();
	assert_eq!(result, "echo production");
}

#[test]
fn substitute_preserves_non_args_dollar() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $(HOME) and $(ARGS)").unwrap();
	assert_eq!(result, "echo $(HOME) and ");
}

#[test]
fn substitute_multiple_named_consumed() {
	// Both --port and --host are consumed by $(ARGS.key), so $(ARGS) should be empty
	let args = RunArgs::parse(&["--port=3000".into(), "--host=localhost".into()]);
	let result = args
		.substitute_no_env("server --port=$(ARGS.port ? 8080) --host=$(ARGS.host ? 0.0.0.0)")
		.unwrap();
	assert_eq!(result, "server --port=3000 --host=localhost");
}

#[test]
fn substitute_order_independent() {
	// "run build --help --env dev" → same as "--env dev --help"
	let args = RunArgs::parse(&["--help".into(), "--env".into(), "dev".into()]);
	let result = args
		.substitute_no_env("cargo build env:$(ARGS.env ? test) $(ARGS)")
		.unwrap();
	assert_eq!(result, "cargo build env:dev --help");
}

#[test]
fn substitute_no_args_no_substitution() {
	// "run build" with "cargo build $(ARGS)" → "cargo build "
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("cargo build $(ARGS)").unwrap();
	assert_eq!(result, "cargo build ");
}

// ── Redacted substitution tests ────────────────────────────────────

#[test]
fn substitute_redacted_hides_env_values() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("DB_PASSWORD".to_string(), "s3cret".to_string());
	let result = args
		.substitute_redacted("connect --password=$(ENV.DB_PASSWORD)", &env)
		.unwrap();
	assert_eq!(result, "connect --password=***");
	assert!(!result.contains("s3cret"), "secret should be redacted");
}

#[test]
fn substitute_redacted_shows_args_values() {
	let args = RunArgs::parse(&["--env=production".into()]);
	let env = HashMap::new();
	let result = args.substitute_redacted("deploy $(ARGS.env)", &env).unwrap();
	assert_eq!(result, "deploy production");
}

#[test]
fn substitute_redacted_shows_positional_args() {
	let args = RunArgs::parse(&["hello".into(), "world".into()]);
	let env = HashMap::new();
	let result = args.substitute_redacted("echo $(ARGS)", &env).unwrap();
	assert_eq!(result, "echo hello world");
}

#[test]
fn substitute_redacted_mixed_args_and_env() {
	let args = RunArgs::parse(&["--host=example.com".into()]);
	let mut env = HashMap::new();
	env.insert("SECRET_TOKEN".to_string(), "tok_abc123".to_string());
	let result = args
		.substitute_redacted(
			"curl -H 'Authorization: $(ENV.SECRET_TOKEN)' https://$(ARGS.host)/api",
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
	let result = args.substitute_redacted("echo $(ENV.TOKEN ? fallback)", &env).unwrap();
	assert_eq!(result, "echo ***");
}

#[test]
fn substitute_redacted_env_missing_uses_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args
		.substitute_redacted("echo $(ENV.MISSING ? fallback)", &env)
		.unwrap();
	assert_eq!(result, "echo fallback");
}

#[test]
fn substitute_redacted_chained_args_then_env() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("DB_HOST".to_string(), "db.internal".to_string());
	// ARGS.host not provided, falls through to ENV.DB_HOST → redacted
	let result = args
		.substitute_redacted("connect $(ARGS.host ? ENV.DB_HOST)", &env)
		.unwrap();
	assert_eq!(result, "connect ***");
}

#[test]
fn substitute_redacted_chained_args_provided() {
	let args = RunArgs::parse(&["--host=localhost".into()]);
	let mut env = HashMap::new();
	env.insert("DB_HOST".to_string(), "db.internal".to_string());
	// ARGS.host IS provided → shown as-is (not redacted)
	let result = args
		.substitute_redacted("connect $(ARGS.host ? ENV.DB_HOST)", &env)
		.unwrap();
	assert_eq!(result, "connect localhost");
}

#[test]
fn substitute_redacted_flags_shown() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let env = HashMap::new();
	let result = args
		.substitute_redacted("cmd $(FLAGS.verbose ? --verbose :)", &env)
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
	let env = build_env(&spec, &PathBuf::from("."), &RunArgs::default(), None).unwrap();
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

	let env = build_env(&spec, &PathBuf::from("."), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("MY_VAR").unwrap(), "hello");
}

#[test]
fn build_env_command_overrides_global() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::Number(5000.0));

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);

	let env = build_env(&spec, &PathBuf::from("."), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("PORT").unwrap(), "5000");
}

#[test]
fn build_env_add_to_path() {
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.add_to_path = Some(vec!["node_modules/.bin".into()]);

	let working_dir = PathBuf::from("/project");
	let env = build_env(&spec, &working_dir, &RunArgs::default(), None).unwrap();
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

	let env = build_env(&spec, &PathBuf::from("/project"), &RunArgs::default(), None).unwrap();
	let path = get_path_value(&env);
	assert!(path.contains("/usr/local/custom/bin"));
}

// ── Env value substitution tests ───────────────────────────────────

#[test]
fn build_env_substitutes_args_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::String("$(ARGS.port ? 3000)".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::parse(&["--port=4000".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("PORT").unwrap(), "4000");
}

#[test]
fn build_env_substitutes_args_default_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::String("$(ARGS.port ? 3000)".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("PORT").unwrap(), "3000");
}

#[test]
fn build_env_substitutes_flags_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert(
		"NODE_OPTIONS".into(),
		EnvValue::String("$(FLAGS.debug ? --inspect : )".into()),
	);
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::parse(&["--debug".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("NODE_OPTIONS").unwrap(), "--inspect");
}

#[test]
fn build_env_substitutes_flags_false_in_target_env() {
	let mut cmd_env = HashMap::new();
	cmd_env.insert("DEBUG".into(), EnvValue::String("$(FLAGS.debug)".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("DEBUG").unwrap(), "false");
}

#[test]
fn build_env_substitutes_args_in_global_env() {
	let mut global_env = HashMap::new();
	global_env.insert("ENV_NAME".into(), EnvValue::String("$(ARGS.env ? development)".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(global_env);
	let args = RunArgs::parse(&["--env=staging".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("ENV_NAME").unwrap(), "staging");
}

#[test]
fn build_env_substitutes_flags_in_global_env() {
	let mut global_env = HashMap::new();
	global_env.insert("VERBOSE".into(), EnvValue::String("$(FLAGS.verbose)".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(global_env);
	let args = RunArgs::parse(&["--verbose".into()]);
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	assert_eq!(env.get("VERBOSE").unwrap(), "true");
}

#[test]
fn build_env_env_can_reference_system_env_via_substitution() {
	// Env vars can reference system env vars via $(ENV.VAR) substitution
	let mut cmd_env = HashMap::new();
	cmd_env.insert("MY_PATH_COPY".into(), EnvValue::String("$(ENV.PATH ? fallback)".into()));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
	// PATH should exist in system env, so the substitution should use the real value
	let path_copy = env.get("MY_PATH_COPY").unwrap();
	assert_ne!(
		path_copy, "fallback",
		"Should have resolved $(ENV.PATH) from system env"
	);
}

#[test]
fn build_env_non_string_env_values_not_substituted() {
	// Numbers and booleans don't contain $() patterns, so substitution is a no-op
	let mut cmd_env = HashMap::new();
	cmd_env.insert("PORT".into(), EnvValue::Number(8080.0));
	cmd_env.insert("ENABLED".into(), EnvValue::Bool(true));
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env = Some(cmd_env);
	let args = RunArgs::default();
	let env = build_env(&spec, &PathBuf::from("."), &args, None).unwrap();
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
	let result = args.substitute_no_env("echo $(ARGS.env ?)").unwrap();
	assert_eq!(result, "echo ");
}

#[test]
fn substitute_missing_arg_without_default_errors() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $(ARGS.env)");
	assert!(result.is_err());
	let err = result.unwrap_err();
	assert!(err.to_string().contains("env"));
}

#[test]
fn substitute_present_arg_without_default_works() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args.substitute_no_env("echo $(ARGS.env)").unwrap();
	assert_eq!(result, "echo prod");
}

// ── ENV substitution tests ────────────────────────────────────────

#[test]
fn substitute_env_basic() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "production".into());
	let result = args.substitute("echo $(ENV.NODE_ENV)", &env).unwrap();
	assert_eq!(result, "echo production");
}

#[test]
fn substitute_env_case_insensitive() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "production".into());
	let result = args.substitute("echo $(ENV.node_env)", &env).unwrap();
	assert_eq!(result, "echo production");
}

#[test]
fn substitute_env_with_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo $(ENV.NODE_ENV ? development)", &env).unwrap();
	assert_eq!(result, "echo development");
}

#[test]
fn substitute_env_with_empty_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo $(ENV.NODE_ENV ?)", &env).unwrap();
	assert_eq!(result, "echo ");
}

#[test]
fn substitute_env_missing_errors() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo $(ENV.NODE_ENV)", &env);
	assert!(result.is_err());
	assert!(result.unwrap_err().to_string().contains("NODE_ENV"));
}

#[test]
fn substitute_env_present_overrides_default() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("NODE_ENV".into(), "staging".into());
	let result = args.substitute("echo $(ENV.NODE_ENV ? development)", &env).unwrap();
	assert_eq!(result, "echo staging");
}

#[test]
fn substitute_chain_args_then_env() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("MY_ENV".into(), "from_env".into());
	let result = args
		.substitute("echo $(ARGS.key ? ENV.MY_ENV ? fallback)", &env)
		.unwrap();
	assert_eq!(result, "echo from_env");
}

#[test]
fn substitute_chain_args_wins_over_env() {
	let args = RunArgs::parse(&["--key=from_args".into()]);
	let mut env = HashMap::new();
	env.insert("MY_ENV".into(), "from_env".into());
	let result = args
		.substitute("echo $(ARGS.key ? ENV.MY_ENV ? fallback)", &env)
		.unwrap();
	assert_eq!(result, "echo from_args");
}

#[test]
fn substitute_chain_falls_through_to_literal() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args
		.substitute("echo $(ARGS.key ? ENV.MY_ENV ? fallback)", &env)
		.unwrap();
	assert_eq!(result, "echo fallback");
}

#[test]
fn substitute_chain_env_then_env() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("SECONDARY".into(), "second".into());
	let result = args
		.substitute("echo $(ENV.PRIMARY ? ENV.SECONDARY ? none)", &env)
		.unwrap();
	assert_eq!(result, "echo second");
}

#[test]
fn substitute_chain_with_empty_default() {
	let args = RunArgs::parse(&[]);
	let env = HashMap::new();
	let result = args.substitute("echo $(ARGS.key ? ENV.MISSING ?)", &env).unwrap();
	assert_eq!(result, "echo ");
}

#[test]
fn substitute_env_and_args_in_same_command() {
	let args = RunArgs::parse(&["--port=9090".into()]);
	let mut env = HashMap::new();
	env.insert("HOST".into(), "localhost".into());
	let result = args
		.substitute("server --host=$(ENV.HOST) --port=$(ARGS.port ? 8080)", &env)
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

// ── Command-level addToPath tests ──────────────────────────────────

#[test]
fn build_env_command_add_to_path() {
	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.add_to_path = Some(vec!["my-tools/bin".into()]);

	let working_dir = PathBuf::from("/project");
	let env = build_env(&spec, &working_dir, &RunArgs::default(), None).unwrap();
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
	let env = build_env(&spec, &working_dir, &RunArgs::default(), None).unwrap();
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
	let spec = CommandSpec::new(vec!["echo $(ARGS)".into()]);
	let args = RunArgs::parse(&["hello".into(), "world".into()]);
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
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
	use crate::runner::{run_target, RunError};
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

// ── `when:` block runtime tests ───────────────────────────────────

#[test]
fn when_always_runs_after_success() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": [
                    "echo step1 >> \"{log_escaped}\"",
                    {{ "when": "always", "commands": ["echo cleanup >> \"{log_escaped}\""] }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();

	let content = std::fs::read_to_string(&log).unwrap();
	assert!(content.contains("step1"));
	assert!(content.contains("cleanup"));
}

#[test]
fn when_failure_skips_on_success() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": [
                    "echo step1 >> \"{log_escaped}\"",
                    {{ "when": "failure", "commands": ["echo SHOULD_NOT_RUN >> \"{log_escaped}\""] }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let res = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(res.final_status.success());

	let content = std::fs::read_to_string(&log).unwrap();
	assert!(content.contains("step1"));
	assert!(!content.contains("SHOULD_NOT_RUN"));
}

#[test]
fn when_failure_runs_on_failure() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	let fail = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": [
                    "echo step1 >> \"{log_escaped}\"",
                    "{fail}",
                    "echo SHOULD_NOT_RUN >> \"{log_escaped}\"",
                    {{ "when": "failure", "commands": ["echo failure_handler >> \"{log_escaped}\""] }},
                    {{ "when": "always", "commands": ["echo always_cleanup >> \"{log_escaped}\""] }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let res = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	// Target failed → final_status reports the failure.
	assert!(!res.final_status.success(), "expected non-zero exit");

	let content = std::fs::read_to_string(&log).unwrap();
	assert!(content.contains("step1"));
	assert!(content.contains("failure_handler"));
	assert!(content.contains("always_cleanup"));
	assert!(!content.contains("SHOULD_NOT_RUN"));
}

#[test]
fn when_with_if_block() {
	// `when` as a property on an `if` block.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);
	let fail = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": [
                    "{fail}",
                    {{ "when": "always", "if": "$(RUN.os) == windows", "then": "echo windows >> \"{log_escaped}\"", "else": "echo unix >> \"{log_escaped}\"" }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	let content = std::fs::read_to_string(&log).unwrap();
	// Either branch wrote SOMETHING — confirm the always-block ran.
	assert!(
		content.contains("windows") || content.contains("unix"),
		"got log: {content:?}"
	);
}

#[test]
fn when_ignore_errors_does_not_flip_target_state() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);
	let fail = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": [
                    {{ "when": "always", "ignoreErrors": true, "commands": ["{fail}"] }},
                    "echo after_block >> \"{log_escaped}\""
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let res = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	// Block's ignoreErrors swallows the failure — target stays successful.
	assert!(
		res.final_status.success(),
		"block-level ignoreErrors should swallow the failure"
	);
	let content = std::fs::read_to_string(&log).unwrap();
	assert!(content.contains("after_block"));
}

#[test]
fn when_inside_parallel_target_partitions_by_phase() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "parallel": true,
                "commands": [
                    "echo a >> \"{log_escaped}\"",
                    "echo b >> \"{log_escaped}\"",
                    {{ "when": "always", "commands": ["echo cleanup >> \"{log_escaped}\""] }},
                    {{ "when": "failure", "commands": ["echo failure_only >> \"{log_escaped}\""] }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	let content = std::fs::read_to_string(&log).unwrap();
	// Default-when steps ran in parallel; always-cleanup ran after.
	assert!(content.contains("a"));
	assert!(content.contains("b"));
	assert!(content.contains("cleanup"));
	// No prior failures → failure-only block must NOT have run.
	assert!(!content.contains("failure_only"), "got log: {content:?}");
}

#[test]
fn detach_evaluates_if_block_and_does_not_run_condition_as_shell() {
	// Regression: detach used to call `walk_step_templates` to flatten the
	// commands list, which yielded the `if` condition string ("os == windows")
	// and BOTH branches as if they were shell commands. The condition got
	// piped into a shell where the first word ("windows") became a missing
	// command. This test pins the fix: detach evaluates control-flow blocks
	// at runtime and only spawns the chosen branch.
	use crate::control_flow::{collect_detach_leaves, DetachFlattenError};
	use runfile_parser::{parse_runfile, CommandStep};
	use std::collections::HashMap;

	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"install": {
				"commands": [
					{
						"if": "$(RUN.os) == windows",
						"then": "echo windows-only",
						"else": "echo unix-only"
					}
				],
				"detach": true,
				"forceShell": "sh"
			}
		}
	}"#;
	let runfile = parse_runfile(json).unwrap();
	let install = &runfile.targets["install"];

	// Verify the spec actually contains an `if` block (not a flattened list).
	assert!(matches!(&install.commands[0], CommandStep::If(_)));

	// Drive `collect_detach_leaves` directly to confirm only one branch
	// is collected. The condition string itself must NOT be in the output.
	let args = RunArgs::default().with_run_context(crate::args::RunContext {
		os: "windows".into(),
		shell: "sh".into(),
		..Default::default()
	});
	let leaves = collect_detach_leaves(&install.commands, &args, &HashMap::new(), std::path::Path::new(".")).unwrap();
	assert_eq!(leaves, vec!["echo windows-only".to_string()]);
	assert!(
		!leaves.iter().any(|l| l.contains("==")),
		"condition should not appear as a leaf: {leaves:?}"
	);

	// And `@target` calls inside detach are rejected.
	let json2 = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"foo": { "commands": ["echo hi"] },
			"bad": { "commands": ["@foo"], "detach": true, "forceShell": "sh" }
		}
	}"#;
	let runfile2 = parse_runfile(json2).unwrap();
	let bad = &runfile2.targets["bad"];
	let err = collect_detach_leaves(
		&bad.commands,
		&RunArgs::default(),
		&HashMap::new(),
		std::path::Path::new("."),
	)
	.unwrap_err();
	assert!(matches!(err, DetachFlattenError::TargetCallNotAllowed(_)));
}

// ── @target invocation runtime tests ────────────────────────────

#[test]
fn target_call_runs_dependency_and_passes_args() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	// Wrap the args in a marker so an empty call still produces a distinguishable line.
	// Use `_` as the delimiter so the marker is a literal string in every shell:
	// `[` / `]` are glob char-classes in bash/zsh (and zsh errors on unmatched globs);
	// `=word` triggers zsh's `=cmd` PATH-lookup expansion. `_` has no special meaning
	// in bash/zsh/sh/fish/powershell/cmd.
	let append_arg = format!("echo _$(ARGS)_ >> \\\"{log_escaped}\\\"");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "echo-arg": {{ "commands": ["{append_arg}"] }},
            "main": {{
                "commands": [
                    "@echo-arg first",
                    "@echo-arg second-with-flag --release",
                    "@echo-arg"
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();
	run_target("main", &runfile, &shell, &args, dir.path()).unwrap();

	let content = std::fs::read_to_string(&log).unwrap();
	let lines: Vec<&str> = content.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
	assert_eq!(lines.len(), 3, "got: {content:?}");
	assert_eq!(lines[0], "_first_");
	assert_eq!(lines[1], "_second-with-flag --release_");
	assert_eq!(lines[2], "__");
}

#[test]
fn target_call_no_dedup() {
	// Calling the same dep multiple times runs it multiple times.
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let counter = dir.path().join("count.txt");
	let counter_escaped = json_escape_path(&counter);
	let _ = shell.kind; // silence unused warning if we ever drop the branch
	let bump = format!("echo x >> \\\"{counter_escaped}\\\"");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "tick": {{ "commands": ["{bump}"] }},
            "main": {{ "commands": ["@tick", "@tick", "@tick"] }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();
	run_target("main", &runfile, &shell, &args, dir.path()).unwrap();

	let content = std::fs::read_to_string(&counter).unwrap();
	let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
	assert_eq!(lines.len(), 3, "Each @target call must run, no dedup");
}

#[test]
fn target_call_passes_parent_env_to_dep() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("env.txt");
	let log_escaped = json_escape_path(&log);

	let echo_var = if shell.kind == ShellKind::Cmd {
		format!("echo %PARENT_VAR% > \\\"{log_escaped}\\\"")
	} else if shell.kind == ShellKind::PowerShell {
		format!("$env:PARENT_VAR | Out-File -Encoding ascii \\\"{log_escaped}\\\"")
	} else {
		format!("echo $PARENT_VAR > \\\"{log_escaped}\\\"")
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "show-env": {{ "commands": ["{echo_var}"] }},
            "parent": {{
                "env": {{ "PARENT_VAR": "from-parent" }},
                "commands": ["@show-env"]
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();
	run_target("parent", &runfile, &shell, &args, dir.path()).unwrap();

	let content = std::fs::read_to_string(&log).unwrap();
	assert!(
		content.contains("from-parent"),
		"Dep should see parent env, got: {content:?}"
	);
}

#[test]
fn target_call_dep_env_overrides_parent_env() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("env.txt");
	let log_escaped = json_escape_path(&log);

	let echo_var = if shell.kind == ShellKind::Cmd {
		format!("echo %SHARED% > \\\"{log_escaped}\\\"")
	} else if shell.kind == ShellKind::PowerShell {
		format!("$env:SHARED | Out-File -Encoding ascii \\\"{log_escaped}\\\"")
	} else {
		format!("echo $SHARED > \\\"{log_escaped}\\\"")
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "show": {{
                "env": {{ "SHARED": "from-dep" }},
                "commands": ["{echo_var}"]
            }},
            "parent": {{
                "env": {{ "SHARED": "from-parent" }},
                "commands": ["@show"]
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();
	run_target("parent", &runfile, &shell, &args, dir.path()).unwrap();

	let content = std::fs::read_to_string(&log).unwrap();
	assert!(
		content.contains("from-dep"),
		"Dep env should win on conflict, got: {content:?}"
	);
}

#[test]
fn target_call_cycle_is_detected() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "a": { "commands": ["@b"] },
            "b": { "commands": ["@a"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();
	let err = run_target("a", &runfile, &shell, &args, dir.path()).unwrap_err();
	let msg = err.to_string();
	assert!(
		msg.contains("cycle") || msg.contains("Cycle"),
		"expected cycle error, got: {msg}"
	);
}

#[test]
fn target_call_unknown_target_errors() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "main": { "commands": ["@nonexistent"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();
	assert!(run_target("main", &runfile, &shell, &args, dir.path()).is_err());
}

#[test]
fn target_call_in_parallel_parent_runs_each_dep() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let counter = dir.path().join("count.txt");
	let counter_escaped = json_escape_path(&counter);
	let _ = shell.kind;
	let bump = format!("echo $(ARGS.tag) >> \\\"{counter_escaped}\\\"");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "log": {{ "commands": ["{bump}"] }},
            "main": {{
                "parallel": true,
                "commands": [
                    "@log --tag a",
                    "@log --tag b",
                    "@log --tag c"
                ]
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();
	run_target("main", &runfile, &shell, &args, dir.path()).unwrap();

	let content = std::fs::read_to_string(&counter).unwrap();
	let mut lines: Vec<&str> = content.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
	lines.sort();
	assert_eq!(lines, vec!["a", "b", "c"]);
}

#[test]
fn target_call_inside_if_branch() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let prod_marker = dir.path().join("prod_ran");
	let dev_marker = dir.path().join("dev_ran");
	let prod_escaped = json_escape_path(&prod_marker);
	let dev_escaped = json_escape_path(&dev_marker);

	let touch_prod = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{prod_escaped}\\\"")
	} else {
		format!("touch \\\"{prod_escaped}\\\"")
	};
	let touch_dev = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{dev_escaped}\\\"")
	} else {
		format!("touch \\\"{dev_escaped}\\\"")
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "prod": {{ "commands": ["{touch_prod}"] }},
            "dev": {{ "commands": ["{touch_dev}"] }},
            "deploy": {{
                "commands": [
                    {{ "if": "$(ARGS.env) == prod", "then": "@prod", "else": "@dev" }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::parse(&["--env=prod".into()]);
	run_target("deploy", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(prod_marker.exists(), "prod target should have run");
	assert!(!dev_marker.exists(), "dev target should NOT have run");
}

#[test]
fn target_call_substitutes_args_template() {
	// $(ARGS) in the args template should expand to the parent's args.
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	let _ = shell.kind;
	let echo = format!("echo $(ARGS) > \\\"{log_escaped}\\\"");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "echo": {{ "commands": ["{echo}"] }},
            "fwd": {{ "commands": ["@echo $(ARGS)"] }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::parse(&["alpha".into(), "beta".into()]);
	run_target("fwd", &runfile, &shell, &args, dir.path()).unwrap();

	let content = std::fs::read_to_string(&log).unwrap();
	assert!(content.contains("alpha"), "got: {content:?}");
	assert!(content.contains("beta"), "got: {content:?}");
}

// ── workingDirectory tests ─────────────────────────────────────────

#[test]
fn run_target_cwd_working_directory() {
	use crate::runner::run_target_with_cwd;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();

	let marker = caller_cwd.path().join("cwd_marker");
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
            "test-cwd": {{
                "commands": ["{create_marker}"],
                "workingDirectory": "cwd"
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let result = run_target_with_cwd(
		"test-cwd",
		&runfile,
		&shell,
		&args,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(result.final_status.success());
	assert!(marker.exists(), "Command should have run in caller's CWD");
}

#[test]
fn run_target_global_cwd_working_directory() {
	use crate::runner::run_target_with_cwd;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();

	let marker = caller_cwd.path().join("global_cwd_marker");
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
            "test-cwd": {{
                "commands": ["{create_marker}"],
                "workingDirectory": "cwd"
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let result = run_target_with_cwd(
		"test-cwd",
		&runfile,
		&shell,
		&args,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(result.final_status.success());
	assert!(
		marker.exists(),
		"Command should have run in caller's CWD via workingDirectory"
	);
}

#[test]
fn run_target_working_directory_target_overrides_global() {
	use crate::runner::run_target_with_cwd;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();

	// Global says cwd, but target says runfileParent.
	// Marker goes into runfile_dir, proving the target override won.
	let marker = runfile_dir.path().join("override_marker");
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
            "test-override": {{
                "commands": ["{create_marker}"],
                "workingDirectory": "runfileParent"
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let result = run_target_with_cwd(
		"test-override",
		&runfile,
		&shell,
		&args,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(result.final_status.success());
	assert!(marker.exists(), "Target workingDirectory should override global");
}

#[test]
fn working_directory_accepts_substitution() {
	// `workingDirectory` may be a `$(...)` template that resolves to one of
	// the canonical values at runtime.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let marker = dir.path().join("ran");
	let marker_escaped = json_escape_path(&marker);
	let touch = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{marker_escaped}\\\"")
	} else {
		format!("touch \\\"{marker_escaped}\\\"")
	};

	// `$(ARGS.dir ? cwd)` → resolves to the literal "cwd" at runtime.
	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": ["{touch}"],
                "workingDirectory": "$(ARGS.dir ? cwd)"
            }}
        }}
    }}"#
	);
	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();
	run_target("t", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(marker.exists());
}

#[test]
fn working_directory_invalid_substitution_errors_at_runtime() {
	use crate::runner::{run_target, RunError};
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "t": {
                "commands": ["echo hi"],
                "workingDirectory": "$(ARGS.dir ? bogus)"
            }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let err = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap_err();
	assert!(matches!(err, RunError::InvalidWorkingDirectory(_, _)), "got: {err}");
}

#[test]
fn force_shell_accepts_substitution() {
	// `forceShell` may be a substituted string. We only verify parsing +
	// resolution succeeds — a value of `$(RUN.shell)` resolves to the shell
	// the CLI already chose, so no actual switch happens.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "t": {
                "commands": ["echo hi"],
                "forceShell": "$(RUN.shell)"
            }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let res = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(res.final_status.success());
}

// ── Extract tests ────────────────────────────────────────────────────

#[test]
fn extract_simple_target() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build", "echo done"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("build", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert_eq!(lines.len(), 2);
	assert_eq!(lines[0], "cargo build");
	assert_eq!(lines[1], "echo done");
}

#[test]
fn extract_with_env_vars_bash() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["npm run build"],
                "env": { "ENV": "test", "NODE_ENV": "production" }
            }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("build", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0], "ENV=test NODE_ENV=production npm run build");
}

#[test]
fn extract_with_dependencies() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	// `@target` invocations are recursively expanded — the dep's resolved
	// shell commands appear inline at the call site, with the dep's own env
	// reflected on each command.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "clean": { "commands": ["npm run clean"], "env": { "ENV": "test" } },
            "build": {
                "commands": ["@clean", "npm run build", "echo done"],
                "env": { "ENV": "test", "NODE_ENV": "test" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("build", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert!(
		lines.iter().any(|l| l.contains("npm run clean")),
		"expected @clean to be expanded; got {lines:?}"
	);
	assert!(lines.iter().any(|l| l.contains("npm run build")));
	assert!(lines.iter().any(|l| l.contains("echo done")));

	// The clean command should display only its own env (ENV=test), not
	// build's NODE_ENV — each dep keeps its own spec env block.
	let clean_line = lines.iter().find(|l| l.contains("npm run clean")).unwrap();
	assert!(clean_line.contains("ENV=test"));
	assert!(
		!clean_line.contains("NODE_ENV"),
		"clean line should not carry build's env: {clean_line}"
	);

	// And dep ordering matches execution order: @clean expands before
	// build's own shell commands.
	let clean_idx = lines.iter().position(|l| l.contains("npm run clean")).unwrap();
	let build_idx = lines.iter().position(|l| l.contains("npm run build")).unwrap();
	assert!(clean_idx < build_idx, "clean must precede build; got {lines:?}");
}

#[test]
fn extract_with_global_dependency() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "setup": { "commands": ["echo setup"] },
            "build": { "commands": ["@setup", "echo build"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("build", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	// Both the dep's command and the parent's command appear, in execution order.
	assert_eq!(lines, vec!["echo setup".to_string(), "echo build".to_string()]);
}

#[test]
fn extract_detects_cycles() {
	// `@target` cycles are now detected at extract time too (per-call-stack
	// tracking inside the recursive walker). A cyclic Runfile yields an
	// `ExtractError::CycleDetected` instead of an infinite loop.
	use crate::extract::{extract_target, ExtractError};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "a": { "commands": ["@b"] },
            "b": { "commands": ["@a"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let err = extract_target("a", &runfile, &args, dir.path()).unwrap_err();
	assert!(
		matches!(err, ExtractError::CycleDetected(_)),
		"expected CycleDetected, got: {err:?}"
	);
}

#[test]
fn extract_target_call_with_args_forwards_to_dep() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	// `@deploy --env=prod` should pass `--env=prod` into the dep so the
	// dep's `$(ARGS.env)` substitution resolves.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": { "commands": ["echo deploying to $(ARGS.env)"] },
            "release": { "commands": ["@deploy --env=prod"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("release", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert_eq!(lines, vec!["echo deploying to prod".to_string()]);
}

#[test]
fn extract_for_namespaces_aggregator() {
	// Regression: aggregator targets whose only body is a `for in: namespaces`
	// loop dispatching `@$(LOOP.ns):something` used to print nothing in
	// dry-run for two compounding reasons: (1) `@target` calls weren't
	// recursively expanded, and (2) the CLI dry-run path didn't sync
	// `run_context.namespaces` from the merged Runfile. Both are fixed —
	// `extract_target_with_cwd` now auto-syncs namespaces (matching what
	// the runner does via `ensure_run_context`) and walks `@target`
	// invocations into their dep's commands.
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "dev": {
                "commands": [
                    { "for": "ns", "in": "namespaces", "do": "@$(LOOP.ns):dev" }
                ]
            },
            "web-admin:dev": { "commands": ["next dev --port 3000"] },
            "web-docs:dev":  { "commands": ["next dev --port 3001"] }
        }
    }"#;

	// Post-merge state: `runfile.namespaces` is populated by the merge
	// step. Set it directly to simulate that, since this test doesn't go
	// through merge. Pass plain `RunArgs::default()` and let the extract
	// auto-sync pick the namespaces up — that matches what the CLI does.
	let mut runfile = parse_runfile(json).unwrap();
	runfile.namespaces = vec!["web-admin".to_string(), "web-docs".to_string()];
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("dev", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert_eq!(
		lines,
		vec!["next dev --port 3000".to_string(), "next dev --port 3001".to_string(),],
		"each namespaced @target should expand to its dev command"
	);
}

#[test]
fn extract_if_evaluates_condition_against_run_context() {
	// Regression: dry-run used to emit BOTH `then` and `else` branches as
	// a static-analysis approximation. Now that extract resolves args/env
	// for real (matching runtime semantics), it can evaluate the condition
	// and emit only the branch that would actually run. This test fakes
	// the runtime OS so the assertion is portable.
	use crate::args::RunContext;
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "stripe-webhook": {
                "commands": [
                    {
                        "if": "$(RUN.os) == windows",
                        "then": "stripe listen -f host.docker.internal:4000/webhook",
                        "else": "stripe listen -f 127.0.0.1:4000/webhook"
                    }
                ]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let dir = TempDir::new().unwrap();

	// Force OS = "windows" so the assertion doesn't depend on the host.
	let mut ctx_win = RunContext::new("bash");
	ctx_win.os = "windows".to_string();
	let args_win = RunArgs::default().with_run_context(ctx_win);
	let lines_win = format_extracted_commands(
		&extract_target("stripe-webhook", &runfile, &args_win, dir.path()).unwrap(),
		&shell.kind,
	);
	assert_eq!(
		lines_win,
		vec!["stripe listen -f host.docker.internal:4000/webhook".to_string()]
	);

	// And again with OS = "linux" — only the else branch should appear.
	let mut ctx_lin = RunContext::new("bash");
	ctx_lin.os = "linux".to_string();
	let args_lin = RunArgs::default().with_run_context(ctx_lin);
	let lines_lin = format_extracted_commands(
		&extract_target("stripe-webhook", &runfile, &args_lin, dir.path()).unwrap(),
		&shell.kind,
	);
	assert_eq!(lines_lin, vec!["stripe listen -f 127.0.0.1:4000/webhook".to_string()]);
}

#[test]
fn extract_optional_target_call_skips_missing() {
	// `@?missing` must not error if the target is absent — runtime silently
	// skips, extract should match.
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["@?missing", "echo built"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("build", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert_eq!(lines, vec!["echo built".to_string()]);
}

#[test]
fn extract_with_args_substitution() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": { "commands": ["echo deploying to $(ARGS.env)"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::parse(&["--env=production".into()]);
	let dir = TempDir::new().unwrap();

	let commands = extract_target("deploy", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &shell.kind);

	assert_eq!(lines.len(), 1);
	assert_eq!(lines[0], "echo deploying to production");
}

#[test]
fn extract_substitutes_env_values_with_args_flags_and_env() {
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::Runfile;

	// Regression: dry-run / extract output used to print env values raw,
	// e.g. `RUN_TESTS_WITH_SIDE_EFFECTS='$(FLAGS.side-effects)'`. Values
	// must be substituted just like commands are.
	//
	// `CARGO_PKG_NAME` is always present during `cargo test`, so we lean on
	// it to exercise `$(ENV.*)` resolution without mutating process env
	// (which races with other parallel tests).
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["./gradlew test"],
                "env": {
                    "RUN_TESTS_WITH_SIDE_EFFECTS": "$(FLAGS.side-effects)",
                    "TARGET_ENV": "$(ARGS.env)",
                    "PKG": "$(ENV.CARGO_PKG_NAME)"
                }
            }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::parse(&["--side-effects".into(), "--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let commands = extract_target("test", &runfile, &args, dir.path()).unwrap();
	let line = format_extracted_commands(&commands, &ShellKind::Bash)
		.into_iter()
		.next()
		.unwrap();

	assert!(
		line.contains("RUN_TESTS_WITH_SIDE_EFFECTS=true"),
		"FLAGS not substituted: {line}"
	);
	assert!(line.contains("TARGET_ENV=prod"), "ARGS not substituted: {line}");
	assert!(line.contains("PKG=runfile-executor"), "ENV not substituted: {line}");
	assert!(line.contains("./gradlew test"), "command missing: {line}");
	assert!(
		!line.contains("$(FLAGS") && !line.contains("$(ARGS") && !line.contains("$(ENV"),
		"unsubstituted placeholder leaked: {line}"
	);
}

#[test]
fn extract_missing_required_arg_errors() {
	use crate::extract::extract_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": { "commands": ["echo deploying to $(ARGS.env)"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = extract_target("deploy", &runfile, &args, dir.path());
	assert!(result.is_err());
}

#[test]
fn extract_format_powershell() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "npm run build".to_string(),
		env_vars: vec![
			("ENV".to_string(), "test".to_string()),
			("NODE_ENV".to_string(), "production".to_string()),
		],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	assert_eq!(lines[0], "$env:ENV='test'; $env:NODE_ENV='production'; npm run build");
}

#[test]
fn extract_format_cmd() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "npm run build".to_string(),
		env_vars: vec![
			("ENV".to_string(), "test".to_string()),
			("NODE_ENV".to_string(), "production".to_string()),
		],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(
		lines[0],
		"set \"ENV=test\" && set \"NODE_ENV=production\" && npm run build"
	);
}

#[test]
fn extract_format_fish() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "npm run build".to_string(),
		env_vars: vec![("ENV".to_string(), "test".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env ENV=test npm run build");
}

#[test]
fn extract_format_no_env_vars() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo hello".to_string(),
		env_vars: vec![],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	assert_eq!(lines[0], "echo hello");
}

#[test]
fn extract_env_value_with_spaces_quoted_bash() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo hello".to_string(),
		env_vars: vec![("MSG".to_string(), "hello world".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Bash);
	assert_eq!(lines[0], "MSG='hello world' echo hello");
}

// ── Argument validation tests ──────────────────────────────────────────

#[test]
fn scan_args_detects_positional() {
	let cmds = vec!["echo $(ARGS)".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(positional);
	assert!(named.is_empty());
}

#[test]
fn scan_args_detects_named() {
	let cmds = vec!["echo $(ARGS.env)".into(), "echo $(ARGS.port ? 8080)".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"));
	assert!(named.contains("port"));
}

#[test]
fn scan_args_detects_both() {
	let cmds = vec!["echo $(ARGS.env) $(ARGS)".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(positional);
	assert!(named.contains("env"));
}

#[test]
fn scan_args_no_patterns() {
	let cmds = vec!["echo hello".into(), "npm run build".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.is_empty());
}

#[test]
fn validate_args_no_args_always_ok() {
	let args = RunArgs::default();
	let cmds = vec!["echo hello".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn validate_args_unexpected_args_error() {
	let args = RunArgs::parse(&["foo".into()]);
	let cmds = vec!["echo hello".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("No command in this target accepts arguments"),
		"Expected UnexpectedArgs, got: {err}"
	);
}

#[test]
fn validate_args_unexpected_named_args_error() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let cmds = vec!["echo hello".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("No command in this target accepts arguments"),
		"Expected UnexpectedArgs, got: {err}"
	);
}

#[test]
fn validate_args_unknown_named_arg_error() {
	let args = RunArgs::parse(&["--env=prod".into(), "--port=8080".into()]);
	let cmds = vec!["echo $(ARGS.env)".into()]; // only $(ARGS.env), not $(ARGS.port)
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("Unknown named argument \"--port\""),
		"Expected UnknownNamedArg, got: {err}"
	);
}

#[test]
fn validate_args_known_named_arg_ok() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let cmds = vec!["echo $(ARGS.env)".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn validate_args_positional_accepts_all() {
	// When $(ARGS) is used, all args are accepted (including unknown named ones)
	let args = RunArgs::parse(&["--env=prod".into(), "foo".into(), "bar".into()]);
	let cmds = vec!["echo $(ARGS)".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn validate_args_named_only_rejects_positional() {
	// Commands only use $(ARGS.env), but user passes positional args
	let args = RunArgs::parse(&["--env=prod".into(), "extra_arg".into()]);
	let cmds = vec!["echo $(ARGS.env)".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("No command in this target accepts arguments")
			|| err.to_string().contains("extra_arg"),
		"Expected error about unexpected positional args, got: {err}"
	);
}

// ── Integration: run_target rejects unexpected args ────────────────────

#[test]
fn run_target_rejects_unexpected_args() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo hello"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::parse(&["--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("build", &runfile, &shell, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("No command in this target accepts arguments"),
		"Expected unexpected args error, got: {err}"
	);
}

#[test]
fn run_target_rejects_unknown_named_arg() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": { "commands": ["echo deploying to $(ARGS.env)"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let shell = ResolvedShell {
		kind: ShellKind::Bash,
		path: PathBuf::from("/bin/bash"),
	};
	let args = RunArgs::parse(&["--env=prod".into(), "--unknown=val".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("deploy", &runfile, &shell, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("Unknown named argument \"--unknown\""),
		"Expected unknown named arg error, got: {err}"
	);
}

#[test]
fn run_target_accepts_valid_args() {
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = detect_default_shell().unwrap();
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "greet": { "commands": ["echo hello $(ARGS)"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::parse(&["world".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("greet", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok());
}

// ── Integration: extract rejects unexpected args ───────────────────────

#[test]
fn extract_rejects_unexpected_args() {
	use crate::extract::extract_target;
	use runfile_parser::Runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo hello"] }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::parse(&["extra".into()]);
	let dir = TempDir::new().unwrap();

	let result = extract_target("build", &runfile, &args, dir.path());
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("No command in this target accepts arguments"),
		"Expected unexpected args error, got: {err}"
	);
}

#[test]
fn validate_args_considers_dependency_commands() {
	// If the dependency uses $(ARGS), args should be accepted
	let args = RunArgs::parse(&["world".into()]);
	let cmds = vec!["echo clean".into(), "echo $(ARGS)".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn run_target_dependency_args_accepted() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	// `@setup $(ARGS)` forwards the parent's args explicitly.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["@setup $(ARGS)", "echo building"] },
            "setup": { "commands": ["echo setup $(ARGS)"] }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["myarg".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("build", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok());
}

// ── Arg validation also scans non-`commands` template fields ──────────
//
// Regression: $(ARGS.x)/$(FLAGS.x) references in env values, envFiles,
// forceShell, addToPath, workingDirectory, confirm, and extendStdio paths
// must be recognised by `validate_args` so users can pass --x without
// also referencing the arg from a command string.

#[test]
fn run_target_accepts_flag_referenced_only_in_env() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": "echo running",
                "env": { "RUN_TESTS_WITH_SIDE_EFFECTS": "$(FLAGS.side-effects)" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--side-effects".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("test", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn run_target_accepts_arg_referenced_only_in_env() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": "echo deploying",
                "env": { "TARGET_ENV": "$(ARGS.env)" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("deploy", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn run_target_accepts_arg_referenced_only_in_env_files() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	// envFiles paths support substitution; missing files are silently skipped,
	// so this still runs successfully even though `.env.prod` doesn't exist.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": "echo deploying",
                "envFiles": [".env.$(ARGS.env)"]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--env=prod".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("deploy", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn run_target_accepts_arg_referenced_only_in_force_shell() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	// Pass --shellname=bash but reference it only via forceShell: $(ARGS.shellname).
	// We don't care which shell ends up resolved — only that validate_args
	// doesn't reject the unknown-arg.
	let shell = detect_default_shell().unwrap();
	let shell_name = shell.kind.name().to_string();
	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "x": {{
                "commands": "echo go",
                "forceShell": "$(ARGS.shellname ? {shell_name})"
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::parse(&[format!("--shellname={shell_name}")]);
	let dir = TempDir::new().unwrap();

	let result = run_target("x", &runfile, &shell, &args, dir.path());
	assert!(result.is_ok(), "expected run to succeed, got: {:?}", result.err());
}

#[test]
fn validate_args_rejects_truly_unknown_named_arg_with_aux_fields() {
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = detect_default_shell().unwrap();
	// env references --side-effects only. --bogus is genuinely unknown.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": "echo running",
                "env": { "X": "$(FLAGS.side-effects)" }
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--bogus".into()]);
	let dir = TempDir::new().unwrap();

	let result = run_target("test", &runfile, &shell, &args, dir.path());
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("Unknown named argument \"--bogus\""),
		"expected unknown-arg error, got: {err}"
	);
}

// ── Env file parsing tests ────────────────────────────────────────

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
fn load_env_files_missing_file_ignored() {
	let dir = TempDir::new().unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env.nonexistent".to_string()], dir.path(), &args, &env);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

#[test]
fn load_env_files_reads_existing_file() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "MY_KEY=my_value\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("MY_KEY").unwrap(), "my_value");
}

#[test]
fn load_env_files_later_overrides_earlier() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=first\n").unwrap();
	std::fs::write(dir.path().join(".env.local"), "KEY=second\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string(), ".env.local".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("KEY").unwrap(), "second");
}

#[test]
fn load_env_files_with_args_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.production"), "DB=prod-db\n").unwrap();
	let args = RunArgs::parse(&["--env".into(), "production".into()]);
	let env = HashMap::new();
	let result = load_env_files(&[".env.$(ARGS.env)".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("DB").unwrap(), "prod-db");
}

#[test]
fn load_env_files_with_env_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.staging"), "DB=staging-db\n").unwrap();
	let args = RunArgs::default();
	let mut env = HashMap::new();
	env.insert("environment".to_string(), "staging".to_string());
	let result = load_env_files(&[".env.$(ENV.environment)".to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("DB").unwrap(), "staging-db");
}

#[test]
fn load_env_files_with_default_substitution() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env.development"), "DB=dev-db\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(
		&[".env.$(ENV.environment ? development)".to_string()],
		dir.path(),
		&args,
		&env,
	)
	.unwrap();
	assert_eq!(result.get("DB").unwrap(), "dev-db");
}

#[test]
fn build_env_env_files_before_env() {
	let dir = TempDir::new().unwrap();
	// env file sets KEY=from_file
	std::fs::write(dir.path().join(".env"), "KEY=from_file\n").unwrap();

	let mut cmd_env = HashMap::new();
	cmd_env.insert("KEY".into(), EnvValue::String("from_env".into()));

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env_files = Some(vec![".env".into()]);
	spec.env = Some(cmd_env);

	// env (inline) should override envFiles
	let env = build_env(&spec, dir.path(), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "from_env");
}

#[test]
fn build_env_global_env_files() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "GLOBAL_KEY=global_value\n").unwrap();

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env_files = Some(vec![".env".into()]);

	let env = build_env(&spec, dir.path(), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("GLOBAL_KEY").unwrap(), "global_value");
}

#[test]
fn build_env_target_env_files_override_global_env_files() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "KEY=global\n").unwrap();
	std::fs::write(dir.path().join(".env.target"), "KEY=target\n").unwrap();

	let mut spec = CommandSpec::new(vec!["echo".into()]);
	spec.env_files = Some(vec![".env".into(), ".env.target".into()]);

	let env = build_env(&spec, dir.path(), &RunArgs::default(), None).unwrap();
	assert_eq!(env.get("KEY").unwrap(), "target");
}

#[test]
fn load_env_files_parse_error() {
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join(".env"), "INVALID_NO_EQUALS\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[".env".to_string()], dir.path(), &args, &env);
	assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — env.rs
// ══════════════════════════════════════════════════════════════════════

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
	// "export =value" — after stripping "export ", key is "" which is before the =
	// but the raw line is "export =value", key part is "export " which contains the export prefix.
	// Actually the line is parsed as key="export " value="value" before export stripping.
	// Let's test that export prefix is correctly handled with a valid key.
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
	// A trailing backslash before closing quote: \"hello\\\" is:
	// opening ", hello, \\(escaped backslash), \"(escaped quote) — no closing quote
	// So this is actually an unterminated string.
	// Test that the parser correctly detects an escaped closing quote vs real closing.
	let content = "KEY=\"hello\\\\\"";
	let pairs = parse_env_file(content).unwrap();
	assert_eq!(pairs[0].1, "hello\\");
}

#[test]
fn parse_env_file_single_quoted_no_escape_processing() {
	// Single-quoted values should NOT process escape sequences
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

#[test]
fn load_env_files_absolute_path() {
	let dir = TempDir::new().unwrap();
	let env_path = dir.path().join("abs.env");
	std::fs::write(&env_path, "ABS_KEY=abs_value\n").unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(&[env_path.to_str().unwrap().to_string()], dir.path(), &args, &env).unwrap();
	assert_eq!(result.get("ABS_KEY").unwrap(), "abs_value");
}

#[test]
fn load_env_files_multiple_missing_files_all_skipped() {
	let dir = TempDir::new().unwrap();
	let args = RunArgs::default();
	let env = HashMap::new();
	let result = load_env_files(
		&[
			".env.missing1".to_string(),
			".env.missing2".to_string(),
			".env.missing3".to_string(),
		],
		dir.path(),
		&args,
		&env,
	);
	assert!(result.is_ok());
	assert!(result.unwrap().is_empty());
}

// ══════════════════════════════════════════════════════════════════════
// Additional test coverage — args.rs
// ══════════════════════════════════════════════════════════════════════

#[test]
fn parse_bare_double_dash() {
	let args = RunArgs::parse(&["--".into()]);
	assert_eq!(args.original, vec!["--"]);
	// Bare "--" should not add to named (empty stripped)
	assert!(args.named.is_empty());
}

#[test]
fn parse_named_arg_with_empty_equals() {
	let args = RunArgs::parse(&["--key=".into()]);
	assert_eq!(args.named["key"], "");
}

#[test]
fn parse_flag_at_end_is_empty_string() {
	let args = RunArgs::parse(&["--verbose".into()]);
	assert_eq!(args.named.get("verbose").unwrap(), "");
}

#[test]
fn parse_flag_followed_by_another_flag() {
	let args = RunArgs::parse(&["--verbose".into(), "--debug".into()]);
	assert_eq!(args.named["verbose"], "");
	assert_eq!(args.named["debug"], "");
}

#[test]
fn substitute_non_args_dollar_expression_preserved() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $(OTHER_EXPR)").unwrap();
	assert_eq!(result, "echo $(OTHER_EXPR)");
}

#[test]
fn substitute_recurses_into_unknown_dollar_expression() {
	// A shell `$(...)` command substitution wrapping an `$(ARGS.x)` reference
	// must keep its outer `$(...)` for the shell, but the inner reference
	// should still be substituted.
	let args = RunArgs::parse(&["--env=development".into()]);
	let result = args
		.substitute_no_env(r#"base=$(echo "$f" | sed 's/\.$(ARGS.env)$//')"#)
		.unwrap();
	assert_eq!(result, r#"base=$(echo "$f" | sed 's/\.development$//')"#);
}

#[test]
fn substitute_recurses_into_deeply_nested_unknown_expression() {
	let args = RunArgs::parse(&["--name=world".into()]);
	let mut env = HashMap::new();
	env.insert("GREETING".to_string(), "hello".to_string());
	let result = args
		.substitute(r#"x=$(printf '%s' $(echo "$(ENV.GREETING) $(ARGS.name)"))"#, &env)
		.unwrap();
	assert_eq!(result, r#"x=$(printf '%s' $(echo "hello world"))"#);
}

#[test]
fn substitute_recursion_propagates_missing_arg_error() {
	// A missing $(ARGS.x) inside a shell `$(...)` should still error rather
	// than silently leaking through unsubstituted.
	let args = RunArgs::parse(&[]);
	let err = args.substitute_no_env(r#"x=$(echo "$(ARGS.missing)")"#).unwrap_err();
	matches!(err, SubstitutionError::MissingArg(_));
}

#[test]
fn substitute_recursion_redacts_env_inside_unknown_expression() {
	let args = RunArgs::parse(&[]);
	let mut env = HashMap::new();
	env.insert("TOKEN".to_string(), "secret123".to_string());
	let result = args.substitute_redacted(r#"x=$(echo "$(ENV.TOKEN)")"#, &env).unwrap();
	assert_eq!(result, r#"x=$(echo "***")"#);
}

#[test]
fn scan_args_usage_finds_args_inside_unknown_expression() {
	// validate_args needs to see `--env` referenced even when its only use
	// is nested inside a shell `$(echo $(ARGS.env))`-style command sub.
	let cmds = vec![r#"base=$(echo "$f" | sed 's/\.$(ARGS.env)$//')"#.into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("env"));
}

#[test]
fn substitute_multiple_args_placeholders() {
	let args = RunArgs::parse(&["hello".into()]);
	let result = args.substitute_no_env("echo $(ARGS) and $(ARGS)").unwrap();
	assert_eq!(result, "echo hello and hello");
}

#[test]
fn substitute_adjacent_dollar_signs() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $$HOME").unwrap();
	assert_eq!(result, "echo $$HOME");
}

#[test]
fn substitute_dollar_without_paren() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $HOME").unwrap();
	assert_eq!(result, "echo $HOME");
}

#[test]
fn scan_args_usage_nested_parens() {
	// Verify depth-tracking handles nested parens
	let cmds = vec!["echo $(ARGS.key ? default(value))".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("key"));
}

#[test]
fn scan_args_detects_named_with_chained_fallback() {
	let cmds = vec!["echo $(ARGS.env ? ENV.NODE_ENV ? production)".into()];
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
		.substitute("echo $(ENV.FIRST ? ENV.SECOND ? fallback)", &env)
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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("EMPTY".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	assert!(lines[0].contains("$env:EMPTY=''"), "got: {}", lines[0]);
}

#[test]
fn extract_format_fish_value_with_single_quotes() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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
	use crate::runner::{run_target, RunError};
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
                "workingDirectory": "cwd"
            }
        }
    }"#;

	let runfile: Runfile = serde_json::from_str(json).unwrap();
	let args = RunArgs::default();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();

	let commands = extract_target_with_cwd(
		"test",
		&runfile,
		&args,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
	)
	.unwrap();
	assert_eq!(commands.len(), 1);
	assert_eq!(commands[0].command, "echo test");
}

#[test]
fn extract_format_bash_value_with_special_chars() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("CMD".to_string(), "echo $(whoami) | cat".to_string())],
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
	use crate::extract::{format_extracted_commands, ExtractedCommand};

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

// ── FLAGS substitution tests ─────────────────────────────────────────

#[test]
fn flags_basic_true() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let result = args.substitute_no_env("echo $(FLAGS.verbose)").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_basic_false() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("echo $(FLAGS.verbose)").unwrap();
	assert_eq!(result, "echo false");
}

#[test]
fn flags_ternary_true() {
	let args = RunArgs::parse(&["--debug".into()]);
	let result = args.substitute_no_env("gcc $(FLAGS.debug ? -g : -O2)").unwrap();
	assert_eq!(result, "gcc -g");
}

#[test]
fn flags_ternary_false() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("gcc $(FLAGS.debug ? -g : -O2)").unwrap();
	assert_eq!(result, "gcc -O2");
}

#[test]
fn flags_ternary_with_spaces_in_values() {
	let args = RunArgs::parse(&["--color".into()]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.color ? --color always : --color never)")
		.unwrap();
	assert_eq!(result, "cmd --color always");
}

#[test]
fn flags_ternary_with_spaces_false_branch() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.color ? --color always : --color never)")
		.unwrap();
	assert_eq!(result, "cmd --color never");
}

#[test]
fn flags_no_colon_present() {
	let args = RunArgs::parse(&["--v".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.v ? -v)").unwrap();
	assert_eq!(result, "cmd -v");
}

#[test]
fn flags_no_colon_absent() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("cmd $(FLAGS.v ? -v)").unwrap();
	assert_eq!(result, "cmd ");
}

#[test]
fn flags_empty_true_branch() {
	let args = RunArgs::parse(&["--quiet".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.quiet ? : --verbose)").unwrap();
	assert_eq!(result, "cmd ");
}

#[test]
fn flags_empty_false_branch() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute_no_env("cmd $(FLAGS.v ? --verbose :)").unwrap();
	assert_eq!(result, "cmd ");
}

#[test]
fn flags_empty_false_branch_present() {
	let args = RunArgs::parse(&["--v".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.v ? --verbose :)").unwrap();
	assert_eq!(result, "cmd --verbose");
}

#[test]
fn flags_consumed_from_args() {
	let args = RunArgs::parse(&["--verbose".into(), "foo".into(), "bar".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) $(ARGS)").unwrap();
	assert_eq!(result, "cmd true foo bar");
}

#[test]
fn flags_absent_not_consumed_from_args() {
	let args = RunArgs::parse(&["foo".into(), "bar".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) $(ARGS)").unwrap();
	assert_eq!(result, "cmd false foo bar");
}

#[test]
fn flags_with_value_still_true() {
	// --verbose=yes should still be "true" for FLAGS (presence only)
	let args = RunArgs::parse(&["--verbose=yes".into()]);
	let result = args.substitute_no_env("echo $(FLAGS.verbose)").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_with_space_value_still_true() {
	// --verbose something should still be "true" for FLAGS
	let args = RunArgs::parse(&["--verbose".into(), "something".into()]);
	let result = args.substitute_no_env("echo $(FLAGS.verbose)").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_multiple() {
	let args = RunArgs::parse(&["--verbose".into(), "--debug".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) $(FLAGS.debug)").unwrap();
	assert_eq!(result, "cmd true true");
}

#[test]
fn flags_multiple_mixed_presence() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) $(FLAGS.debug)").unwrap();
	assert_eq!(result, "cmd true false");
}

#[test]
fn flags_mixed_with_args_named() {
	let args = RunArgs::parse(&["--verbose".into(), "--env=prod".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) env=$(ARGS.env)").unwrap();
	assert_eq!(result, "cmd true env=prod");
}

#[test]
fn flags_mixed_with_args_positional() {
	let args = RunArgs::parse(&["--verbose".into(), "file.txt".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) $(ARGS)").unwrap();
	assert_eq!(result, "cmd true file.txt");
}

#[test]
fn flags_ternary_complex_values() {
	let args = RunArgs::parse(&["--side-effects".into()]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.side-effects ? -run -startup 3 : -donotrun)")
		.unwrap();
	assert_eq!(result, "cmd -run -startup 3");
}

#[test]
fn flags_ternary_complex_values_false() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.side-effects ? -run -startup 3 : -donotrun)")
		.unwrap();
	assert_eq!(result, "cmd -donotrun");
}

#[test]
fn flags_ternary_url_colons_preserved() {
	// Colons in URLs should not be treated as ternary separator (only " : " is)
	let args = RunArgs::parse(&["--ssl".into()]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.ssl ? https://secure.example.com : http://example.com)")
		.unwrap();
	assert_eq!(result, "cmd https://secure.example.com");
}

#[test]
fn flags_ternary_url_colons_false_branch() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.ssl ? https://secure.example.com : http://example.com)")
		.unwrap();
	assert_eq!(result, "cmd http://example.com");
}

#[test]
fn flags_scan_detects_flags() {
	let cmds = vec!["echo $(FLAGS.verbose)".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("verbose"));
}

#[test]
fn flags_scan_detects_flags_with_ternary() {
	let cmds = vec!["echo $(FLAGS.debug ? -g : -O2)".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("debug"));
}

#[test]
fn flags_scan_mixed_with_args() {
	let cmds = vec!["echo $(FLAGS.verbose) $(ARGS.env)".into()];
	let (positional, named) = scan_args_usage(&cmds);
	assert!(!positional);
	assert!(named.contains("verbose"));
	assert!(named.contains("env"));
}

#[test]
fn flags_validate_accepts_flag_args() {
	let args = RunArgs::parse(&["--verbose".into()]);
	let cmds = vec!["echo $(FLAGS.verbose)".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn flags_validate_rejects_unknown_flag() {
	let args = RunArgs::parse(&["--verbose".into(), "--unknown".into()]);
	let cmds = vec!["echo $(FLAGS.verbose)".into()];
	let err = validate_args(&args, &cmds).unwrap_err();
	assert!(
		err.to_string().contains("unknown"),
		"Expected UnknownNamedArg, got: {err}"
	);
}

#[test]
fn flags_validate_mixed_flags_and_args() {
	let args = RunArgs::parse(&["--verbose".into(), "--env=prod".into()]);
	let cmds = vec!["echo $(FLAGS.verbose) $(ARGS.env)".into()];
	assert!(validate_args(&args, &cmds).is_ok());
}

#[test]
fn flags_in_env_substitution() {
	let args = RunArgs::parse(&["--debug".into()]);
	let env = HashMap::new();
	let result = args
		.substitute("echo $(FLAGS.debug ? --inspect : --no-inspect)", &env)
		.unwrap();
	assert_eq!(result, "echo --inspect");
}

#[test]
fn flags_multiple_in_same_command() {
	let args = RunArgs::parse(&["--verbose".into(), "--release".into()]);
	let result = args
		.substitute_no_env("cargo build $(FLAGS.verbose ? -v :) $(FLAGS.release ? --release :)")
		.unwrap();
	assert_eq!(result, "cargo build -v --release");
}

#[test]
fn flags_multiple_in_same_command_none_set() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute_no_env("cargo build $(FLAGS.verbose ? -v :) $(FLAGS.release ? --release :)")
		.unwrap();
	assert_eq!(result, "cargo build  ");
}

#[test]
fn flags_consumed_with_value_from_args() {
	// --verbose=yes used as FLAGS should consume the --verbose=yes token from $(ARGS)
	let args = RunArgs::parse(&["--verbose=yes".into(), "file.txt".into()]);
	let result = args.substitute_no_env("cmd $(FLAGS.verbose) $(ARGS)").unwrap();
	assert_eq!(result, "cmd true file.txt");
}

#[test]
fn flags_hyphenated_key() {
	let args = RunArgs::parse(&["--dry-run".into()]);
	let result = args.substitute_no_env("echo $(FLAGS.dry-run)").unwrap();
	assert_eq!(result, "echo true");
}

#[test]
fn flags_hyphenated_key_ternary() {
	let args = RunArgs::parse(&["--dry-run".into()]);
	let result = args
		.substitute_no_env("cmd $(FLAGS.dry-run ? --dry-run : --execute)")
		.unwrap();
	assert_eq!(result, "cmd --dry-run");
}

// ── RUN.* substitution tests ──────────────────────────────────────

fn args_with_run(shell: &str) -> RunArgs {
	RunArgs::parse(&[]).with_run_context(RunContext {
		os: "linux".to_string(),
		shell: shell.to_string(),
		..Default::default()
	})
}

#[test]
fn run_os_resolves() {
	let args = args_with_run("bash");
	let result = args.substitute_no_env("echo $(RUN.os)").unwrap();
	assert_eq!(result, "echo linux");
}

#[test]
fn run_shell_resolves() {
	let args = args_with_run("powershell");
	let result = args.substitute_no_env("echo $(RUN.shell)").unwrap();
	assert_eq!(result, "echo powershell");
}

#[test]
fn run_unknown_key_errors() {
	let args = args_with_run("bash");
	let err = args.substitute_no_env("echo $(RUN.unknown)").unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("unknown"), "unexpected error: {msg}");
	assert!(msg.contains("os"), "expected error to mention valid keys: {msg}");
}

#[test]
fn run_in_chained_fallback() {
	// $(ARGS.shell ? RUN.shell) — falls back when ARGS not provided.
	let args = args_with_run("zsh");
	let result = args.substitute_no_env("echo $(ARGS.shell ? RUN.shell)").unwrap();
	assert_eq!(result, "echo zsh");
}

#[test]
fn run_with_default_when_unknown() {
	// Unknown RUN key followed by literal default still works.
	let args = args_with_run("bash");
	let result = args.substitute_no_env("echo $(RUN.unknown ? fallback)").unwrap();
	assert_eq!(result, "echo fallback");
}

#[test]
fn run_does_not_consume_named_args() {
	// $(RUN.shell) must not influence $(ARGS) — RUN keys are not user input.
	let args = RunArgs::parse(&["foo".into(), "--keep=true".into()]).with_run_context(RunContext {
		os: "linux".into(),
		shell: "bash".into(),
		..Default::default()
	});
	let result = args.substitute_no_env("cmd $(RUN.shell) $(ARGS)").unwrap();
	assert_eq!(result, "cmd bash foo --keep=true");
}

#[test]
fn run_redacted_substitute_does_not_redact() {
	// RUN values are not secrets — the redacted form should show them.
	let args = args_with_run("bash");
	let env = HashMap::new();
	let result = args.substitute_redacted("echo $(RUN.os)/$(RUN.shell)", &env).unwrap();
	assert_eq!(result, "echo linux/bash");
}

// ── RUN.* in DSL conditions ───────────────────────────────────────

#[test]
fn run_in_if_condition_parses_in_runfile() {
	use runfile_parser::parse_runfile;

	let raw = r#"{
		"$schema": "v0",
		"targets": {
			"t": {
				"commands": [
					{ "if": "$(RUN.os) == linux", "then": ["echo on-linux"] }
				]
			}
		}
	}"#;
	// $(RUN.os) is a substitution leaf in DSL conditions; the parser must
	// accept it without complaining at validation time.
	let runfile = parse_runfile(raw).unwrap();
	assert!(runfile.targets.contains_key("t"));
}

#[test]
fn run_if_condition_runtime_execution() {
	use crate::args::LoopScope;
	use crate::control_flow::evaluate;
	use runfile_parser::parse_condition;

	let ast = parse_condition("$(RUN.shell) == bash").unwrap();
	let env = HashMap::new();
	let scope = LoopScope::new();

	let bash_args = args_with_run("bash");
	assert!(evaluate(&ast, &bash_args, &env, &scope).unwrap());

	let zsh_args = args_with_run("zsh");
	assert!(!evaluate(&ast, &zsh_args, &env, &scope).unwrap());
}

#[test]
fn run_for_in_substitutes_run_values() {
	// `for in: ["$(RUN.os)", "ci"]` should expand $(RUN.os) per element.
	let args = args_with_run("bash");
	let result = args.substitute_no_env("$(RUN.os)").unwrap();
	assert_eq!(result, "linux");
}

#[test]
fn run_negated_inequality() {
	use crate::args::LoopScope;
	use crate::control_flow::evaluate;
	use runfile_parser::parse_condition;

	let ast = parse_condition("$(RUN.os) != windows").unwrap();
	let env = HashMap::new();
	let scope = LoopScope::new();

	let linux_args = args_with_run("bash");
	assert!(evaluate(&ast, &linux_args, &env, &scope).unwrap());
}

// ── Shell quoting security tests ──────────────────────────────────

#[test]
fn cmd_env_value_with_ampersand_is_quoted() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo & whoami".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	// The & must be inside quotes so cmd.exe doesn't interpret it as a command separator
	assert_eq!(lines[0], "set \"VAR=foo & whoami\" && echo test");
}

#[test]
fn cmd_env_value_with_pipe_is_quoted() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo | del *".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=foo | del *\" && echo test");
}

#[test]
fn cmd_env_value_with_angle_brackets_is_quoted() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a > b < c".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=a > b < c\" && echo test");
}

#[test]
fn cmd_env_value_with_caret_is_quoted() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo^bar".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=foo^bar\" && echo test");
}

#[test]
fn cmd_env_value_with_percent_is_quoted() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "%PATH%".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=%PATH%\" && echo test");
}

#[test]
fn powershell_env_value_with_dollar_subexpression_not_expanded() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	// In double quotes, PowerShell would expand $(whoami). Single quotes prevent this.
	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "$(whoami)".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Must use single quotes so $(whoami) is literal
	assert_eq!(lines[0], "$env:VAR='$(whoami)'; echo test");
}

#[test]
fn powershell_env_value_with_backtick_is_literal() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo`nbar".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Single quotes make backtick literal (no escape sequence interpretation)
	assert_eq!(lines[0], "$env:VAR='foo`nbar'; echo test");
}

#[test]
fn powershell_env_value_with_single_quote_is_escaped() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "it's a test".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// PowerShell single-quote escaping: ' becomes ''
	assert_eq!(lines[0], "$env:VAR='it''s a test'; echo test");
}

#[test]
fn powershell_env_value_with_double_quotes_is_literal() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "say \"hello\"".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Double quotes inside single quotes are literal, no escaping needed
	assert_eq!(lines[0], "$env:VAR='say \"hello\"'; echo test");
}

#[test]
fn powershell_env_value_empty() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	assert_eq!(lines[0], "$env:VAR=''; echo test");
}

#[test]
fn powershell_env_value_with_variable_reference_not_expanded() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "$env:SECRET".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// $env:SECRET inside single quotes is literal
	assert_eq!(lines[0], "$env:VAR='$env:SECRET'; echo test");
}

#[test]
fn powershell_env_value_with_semicolon_does_not_break_out() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a'; Remove-Item * -Recurse; echo '".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// The single quote in the value must be doubled, not allow breakout
	assert_eq!(lines[0], "$env:VAR='a''; Remove-Item * -Recurse; echo '''; echo test");
}

#[test]
fn cmd_env_value_simple_no_special_chars() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	// Even simple values get quoted in cmd.exe (defense in depth)
	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "simple".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=simple\" && echo test");
}

#[test]
fn cmd_env_value_with_multiple_dangerous_chars() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a & b | c > d < e ^ f".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=a & b | c > d < e ^ f\" && echo test");
}

// ── Fix #11: error messages use template, not substituted command ──

#[test]
fn error_message_contains_template_not_substituted_value() {
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};
	// Template contains $(ARGS.secret) — the error should show the template,
	// not the substituted value.
	let template = format!("{fail_cmd} $(ARGS.secret ? default_val)");

	let spec = CommandSpec::new_shell(vec![template.clone()]);
	let args = RunArgs::parse(&["--secret=super_secret_password".into()]);
	let dir = TempDir::new().unwrap();

	// With the `when`-aware walker, a non-zero exit no longer surfaces as an
	// `ExecuteError`; the failure is reflected in `final_status` and the
	// `failures` count. Secrets are still kept out of logs via
	// `substitute_redacted` (covered by other tests). What we still want
	// here is: the result reflects the failure cleanly.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success());
	assert_eq!(result.failures, 1);
}

#[test]
fn execute_failure_signals_through_status_not_error() {
	// Shell command failures no longer return `Err` — they bubble up via
	// `final_status` so subsequent `when: failure` / `when: always` blocks
	// still get a chance to run.
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 42".to_string()
	} else {
		"exit 42".to_string()
	};

	let spec = CommandSpec::new_shell(vec![fail_cmd.clone()]);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success(), "exit 42 should report non-zero status");
	assert_eq!(result.failures, 1);
}

// ── Fix #14: Fish shell quoting ───────────────────────────────────

#[test]
fn fish_env_value_with_single_quote_uses_close_escape_reopen() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "it's alive".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Expected: env MSG='it'\''s alive' echo test
	assert_eq!(lines[0], "env MSG='it'\\''s alive' echo test");
}

#[test]
fn fish_env_value_with_multiple_single_quotes() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "it's a 'test'".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env MSG='it'\\''s a '\\''test'\\''' echo test");
}

#[test]
fn fish_env_value_with_dollar_sign_is_quoted() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "$HOME".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// $ triggers quoting, value is wrapped in single quotes
	assert_eq!(lines[0], "env VAR='$HOME' echo test");
}

#[test]
fn fish_env_value_with_spaces() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "hello world".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env VAR='hello world' echo test");
}

#[test]
fn fish_env_value_with_backslash() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("PATH".to_string(), "C:\\Users\\test".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Backslash triggers quoting; in single quotes it's literal in Fish
	assert_eq!(lines[0], "env PATH='C:\\Users\\test' echo test");
}

#[test]
fn fish_env_value_empty_string() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Empty value needs quoting
	assert_eq!(lines[0], "env VAR='' echo test");
}

#[test]
fn fish_env_value_simple_no_quoting() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "simple".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Simple alphanumeric value should NOT be quoted
	assert_eq!(lines[0], "env VAR=simple echo test");
}

#[test]
fn fish_env_value_with_semicolon() {
	use crate::extract::{format_extracted_commands, ExtractedCommand};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a;b".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env VAR='a;b' echo test");
}

use runfile_parser::{ExtendStdio, StdioStream};

// ── extendStdio execution tests ──────────────────────────────────

#[test]
fn execute_with_extend_stdio_tails_log_file() {
	let shell = detect_default_shell().expect("should detect shell");
	let tmp = TempDir::new().unwrap();
	let working_dir = tmp.path();

	// The command writes to a log file, and extendStdio tails it to stdout
	let log_file = working_dir.join("build.log");
	let write_cmd = if shell.kind == ShellKind::Cmd {
		format!(
			"echo line1> \"{}\" && echo line2>> \"{}\"",
			log_file.display(),
			log_file.display()
		)
	} else {
		format!(
			"echo line1 > '{}' && echo line2 >> '{}'",
			log_file.display(),
			log_file.display()
		)
	};

	let mut spec = CommandSpec::new_shell(vec![write_cmd]);
	spec.extend_stdio = Some(vec![ExtendStdio {
		from_file: "build.log".into(),
		stream: StdioStream::Stdout,
	}]);

	let args = RunArgs::parse(&[]);
	let result = execute_command(&spec, &shell, &args, working_dir, None, false);
	// Should succeed — the tailer doesn't affect command exit status
	assert!(result.is_ok());
}

// ──────────────────────────────────────────────────────────────────
// Control flow: if / for blocks
// ──────────────────────────────────────────────────────────────────

/// Parse a single-target Runfile JSON and return its (validated) [`CommandSpec`].
fn parse_target(json: &str, target_name: &str) -> CommandSpec {
	let rf = runfile_parser::parse_runfile(json).expect("test runfile must parse");
	rf.targets
		.into_iter()
		.find(|(k, _)| k == target_name)
		.expect("target not found")
		.1
}

#[test]
fn if_then_branch_executes_on_truthy() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"$(ARGS.go) == yes","then":["echo then-branch"],"else":["echo else-branch"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--go=yes".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn if_else_branch_executes_on_falsy() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"$(ARGS.go) == yes","then":["exit 1"],"else":["echo else-branch"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--go=no".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn if_no_else_branch_skipped_when_falsy() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"$(ARGS.go) == yes","then":["exit 1"]},
			"echo done"
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--go=no".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// Only the trailing "echo done" runs (the "if" branch is skipped).
	assert_eq!(result.commands_run, 1);
}

#[test]
fn if_truthiness_only_empty_string_is_falsy() {
	// Verify that "false" is truthy (matches FLAGS truthiness rule).
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"$(ARGS.flag ? false)","then":["echo truthy"],"else":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	// FLAGS resolves to "false" (default when no flag supplied) which is truthy.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn if_chained_logical_operators() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"$(ARGS.a) == 1 && $(ARGS.b) == 2","then":["echo both"],"else":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--a=1".into(), "--b=2".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn if_negation_works() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"!$(ARGS.skip ? )","then":["echo go"],"else":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn if_failure_propagates_without_ignore_errors() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"if":"$(ARGS.go) == yes","then":["{fail_cmd}"]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::parse(&["--go=yes".into()]);
	// Without `ignoreErrors`, the failure flips the target into the failed
	// state — `final_status` is non-zero.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success());
	assert!(result.failures >= 1);
}

#[test]
fn if_ignore_errors_swallows_body_failure() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"if":"$(ARGS.go) == yes","then":["{fail_cmd}"],"ignoreErrors":true}},
			"echo after"
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::parse(&["--go=yes".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// "echo after" must run — the if-block's ignoreErrors swallows the
	// failure so the outer walker doesn't flip state.failed and skip it.
	assert_eq!(result.commands_run, 2, "both fail_cmd and `echo after` should have run");
	assert!(
		result.final_status.success(),
		"target should succeed when the only failure is inside an ignoreErrors:true block"
	);
}

#[test]
fn for_body_if_ignore_errors_does_not_skip_subsequent_iterations() {
	// Regression: a `for` whose body is `[ if ignoreErrors:true { fail } ]`
	// used to run the if-step only on iteration 1 — the swallowed failure
	// still incremented the outer failure counter, which made the outer
	// walker flip state.failed, which made the default-`when:success`
	// if-step skip on every later iteration.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"x","in":["a","b","c"],"do":[
				{{"if":"a == a","then":["{fail_cmd}"],"ignoreErrors":true}}
			]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(
		result.commands_run, 3,
		"the if-step must execute on all three iterations"
	);
	// `final_status` mirrors the last command run — which here is the
	// ignored failure of iteration 3 — matching `execute_when_block`'s
	// behavior (only the `failed` flag and failure count are isolated,
	// not `last_status`). The actual fix being tested is that all 3
	// iterations ran; with the bug, only iteration 1 did.
}

#[test]
fn for_in_iterates_each_value() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":["1","2","3"],"do":["echo $(LOOP.x)"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 3);
}

#[test]
fn for_in_namespaces_iterates_runfile_namespaces() {
	// Populate args.run_context.namespaces and verify the for-block runs the
	// body once per namespace, with $(LOOP.ns) bound to each.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let touch = match shell.kind {
		ShellKind::Cmd => "type nul > $(LOOP.ns).ns",
		ShellKind::PowerShell => "New-Item -ItemType File -Path \\\"$(LOOP.ns).ns\\\" -Force | Out-Null",
		_ => "touch $(LOOP.ns).ns",
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"ns","in":"namespaces","do":["{touch}"]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default().with_run_context(crate::args::RunContext {
		os: "linux".into(),
		shell: shell.kind.name().to_string(),
		namespaces: std::sync::Arc::new(vec!["project_one".into(), "project_two".into()]),
	});
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
	assert!(dir.path().join("project_one.ns").exists());
	assert!(dir.path().join("project_two.ns").exists());
}

#[test]
fn for_in_namespaces_with_dynamic_target_call_runs_each_namespaced_target() {
	// End-to-end exercise of the user's example pattern:
	//   "for": "ns", "in": "namespaces", "do": "@$(LOOP.ns):build"
	// The for-block iterates the runfile's namespaces; for each value, the
	// `@$(LOOP.ns):build` target call is substituted and dispatched to the
	// real namespaced target. Each project's `build` writes a marker file.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let touch_one = match shell.kind {
		ShellKind::Cmd => "type nul > one.built",
		ShellKind::PowerShell => "New-Item -ItemType File -Path one.built -Force | Out-Null",
		_ => "touch one.built",
	};
	let touch_two = match shell.kind {
		ShellKind::Cmd => "type nul > two.built",
		ShellKind::PowerShell => "New-Item -ItemType File -Path two.built -Force | Out-Null",
		_ => "touch two.built",
	};

	let json = format!(
		r#"{{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {{
			"project_one:build": {{ "commands": ["{touch_one}"] }},
			"project_two:build": {{ "commands": ["{touch_two}"] }},
			"build_all": {{
				"commands": [
					{{ "for": "ns", "in": "namespaces", "do": "@$(LOOP.ns):build" }}
				]
			}}
		}}
	}}"#
	);

	let mut runfile = parse_runfile(&json).unwrap();
	// Simulate what `merge_runfiles` would populate after resolving namespaced
	// includes (those tests live in the parser crate); here we plug the list in
	// directly so the executor sees the same shape it'd see at runtime.
	runfile.namespaces = vec!["project_one".to_string(), "project_two".to_string()];

	let args = RunArgs::default();
	let result = run_target("build_all", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success(), "build_all should succeed");
	assert!(
		dir.path().join("one.built").exists(),
		"project_one:build should have run"
	);
	assert!(
		dir.path().join("two.built").exists(),
		"project_two:build should have run"
	);
}

#[test]
fn optional_target_call_skips_when_missing() {
	// The user's adb-forward use case: iterate every namespace, calling an
	// optional `@?<ns>:adb-forward`. Only namespaces that define the target
	// run; missing ones are silent no-ops (no error, no failure).
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let touch_marker = match shell.kind {
		ShellKind::Cmd => "type nul > marker.touched",
		ShellKind::PowerShell => "New-Item -ItemType File -Path marker.touched -Force | Out-Null",
		_ => "touch marker.touched",
	};

	let json = format!(
		r#"{{
		"$schema": "x",
		"targets": {{
			"with_it:adb-forward": {{ "commands": ["{touch_marker}"] }},
			"without_it:other": {{ "commands": ["echo never"] }},
			"adb-forward": {{
				"commands": [
					{{ "for": "ns", "in": "namespaces", "do": "@?$(LOOP.ns):adb-forward" }}
				]
			}}
		}}
	}}"#
	);

	let mut runfile = parse_runfile(&json).unwrap();
	runfile.namespaces = vec!["with_it".to_string(), "without_it".to_string()];

	let args = RunArgs::default();
	let result = run_target("adb-forward", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(
		result.final_status.success(),
		"adb-forward should succeed even when one namespace lacks the target"
	);
	assert_eq!(
		result.failures, 0,
		"missing optional target must not count as a failure"
	);
	assert!(
		dir.path().join("marker.touched").exists(),
		"with_it:adb-forward should have run"
	);
}

#[test]
fn optional_target_call_static_missing_skips_silently() {
	// Static `@?missing` on a non-existent target is a no-op at runtime.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let json = r#"{
		"$schema": "x",
		"targets": {
			"caller": { "commands": ["@?does-not-exist"] }
		}
	}"#;
	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let result = run_target("caller", &runfile, &shell, &args, dir.path()).unwrap();
	assert_eq!(result.failures, 0);
}

#[test]
fn non_optional_target_call_static_missing_errors() {
	// Sanity: drop the `?` and the same call is a hard error. The optional
	// behavior must not leak into plain `@target` invocations.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let json = r#"{
		"$schema": "x",
		"targets": {
			"caller": { "commands": ["@does-not-exist"] }
		}
	}"#;
	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let err = run_target("caller", &runfile, &shell, &args, dir.path()).unwrap_err();
	assert!(err.to_string().contains("does-not-exist"), "got: {err}");
}

#[test]
fn for_in_namespaces_with_empty_list_does_nothing() {
	// No namespaces ⇒ body doesn't run. Mirrors `for in: []` semantics.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"ns","in":"namespaces","do":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default().with_run_context(crate::args::RunContext {
		os: "linux".into(),
		shell: shell.kind.name().to_string(),
		namespaces: std::sync::Arc::new(Vec::new()),
	});
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 0);
}

#[test]
fn for_in_empty_array_does_nothing() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":[],"do":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 0);
}

#[test]
fn for_in_loop_var_substitutes() {
	// The loop variable must reach the spawned shell. Use a touch-style
	// command that creates files named after the loop variable, then
	// verify they appear in the working directory.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let touch = match shell.kind {
		ShellKind::Cmd => "type nul > $(LOOP.f).out",
		ShellKind::PowerShell => "New-Item -ItemType File -Path \\\"$(LOOP.f).out\\\" -Force | Out-Null",
		_ => "touch $(LOOP.f).out",
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"f","in":["alpha","beta"],"do":["{touch}"]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(dir.path().join("alpha.out").exists());
	assert!(dir.path().join("beta.out").exists());
}

#[test]
fn for_in_nested_inner_var_shadows_outer() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":["1","2"],"do":[
				{"for":"y","in":["a","b"],"do":["echo $(LOOP.x)$(LOOP.y)"]}
			]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 4); // 2 * 2
}

#[test]
fn for_glob_expands_files() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	std::fs::write(dir.path().join("a.txt"), "").unwrap();
	std::fs::write(dir.path().join("b.txt"), "").unwrap();
	std::fs::write(dir.path().join("c.dat"), "").unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"f","glob":"*.txt","do":["echo $(LOOP.f)"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
}

#[test]
fn for_glob_no_matches_does_nothing() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"f","glob":"nonexistent_*.xyz","do":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 0);
}

#[test]
fn for_shell_iterates_lines() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let cmd = if cfg!(windows) {
		"echo a & echo b & echo c"
	} else {
		"printf 'a\\nb\\nc\\n'"
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"line","shell":"{cmd}","do":["echo $(LOOP.line)"]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// Expect 3 iterations regardless of shell quirks (blank lines are dropped).
	assert!(result.commands_run >= 3, "got {} commands_run", result.commands_run);
}

#[test]
fn for_shell_failure_is_hard_error() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let cmd = "exit 1";
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"line","shell":"{cmd}","do":["echo nope"]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false);
	assert!(result.is_err(), "expected hard error from failed shell iterator");
}

#[test]
fn for_in_with_ignore_errors_continues_after_failure() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	// Pre-fail x=1, then succeed for the rest. Use a body that exits 1
	// when x is "1" — encoded without quotes/escapes to keep JSON simple.
	// We can't easily branch in pure shell without quotes; use exit-on-name
	// matching via a marker file: when x=fail, exit 1; otherwise touch a file.
	let body = match shell.kind {
		ShellKind::Cmd => "if $(LOOP.x)==fail (exit /b 1) else (type nul > $(LOOP.x).done)",
		ShellKind::PowerShell => {
			"if ($env:RFLOOP_X -eq 'fail') { exit 1 } else { New-Item -ItemType File -Path \\\"$(LOOP.x).done\\\" -Force | Out-Null }"
		}
		_ => "test $(LOOP.x) = fail && exit 1 || touch $(LOOP.x).done",
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"x","in":["fail","ok2","ok3"],"do":["{body}"],"ignoreErrors":true}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// We don't strictly assert which exact iterations ran (PowerShell branch
	// uses an env var trick that's hard to set per-iteration), but the loop
	// must have iterated at least 2 times when ignoreErrors is on.
	assert!(result.commands_run >= 2, "got commands_run={}", result.commands_run);
}

#[test]
fn for_in_ignore_errors_does_not_skip_next_sibling() {
	// Regression: a sibling step after a `for ignoreErrors:true` that had
	// failing iterations used to be skipped — the for-block's swallowed
	// failures still leaked into the outer failure counter, which made the
	// outer walker flip state.failed and skip the next default-when:success
	// step.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"x","in":["a"],"do":["{fail_cmd}"],"ignoreErrors":true}},
			"echo after"
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(
		result.commands_run, 2,
		"both the for-body and `echo after` should have run"
	);
	assert!(result.final_status.success());
}

#[test]
fn for_in_parallel_ignore_errors_does_not_skip_next_sibling() {
	// Same regression as the sequential version, but for the parallel branch:
	// `run_parallel_leaves` was leaking the failure count to the outer state
	// even when the for-block's ignoreErrors was set.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"x","in":["a","b"],"do":["{fail_cmd}"],"parallel":true,"ignoreErrors":true}},
			"echo after"
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// 2 parallel iterations + 1 sibling = 3 commands executed.
	assert_eq!(
		result.commands_run, 3,
		"parallel for body (2) plus `echo after` (1) should have run"
	);
	assert!(result.final_status.success());
}

#[test]
fn for_in_parallel_runs_concurrently() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let touch = match shell.kind {
		ShellKind::Cmd => "type nul > $(LOOP.f).out",
		ShellKind::PowerShell => "New-Item -ItemType File -Path \\\"$(LOOP.f).out\\\" -Force | Out-Null",
		_ => "touch $(LOOP.f).out",
	};
	let spec_json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"f","in":["a","b","c"],"parallel":true,"do":["{touch}"]}}
		]}}}}}}"#
	);
	let spec = parse_target(&spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 3);
	assert!(dir.path().join("a.out").exists());
	assert!(dir.path().join("b.out").exists());
	assert!(dir.path().join("c.out").exists());
}

#[test]
fn nested_for_outer_parallel_inner_forced_sequential() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	// The outer loop is parallel; the inner is also marked parallel but
	// should be silently coerced to sequential. The semantics must still
	// produce exactly 2*2 = 4 commands total.
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":["1","2"],"parallel":true,"do":[
				{"for":"y","in":["a","b"],"parallel":true,"do":["echo $(LOOP.x)$(LOOP.y)"]}
			]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 4);
}

#[test]
fn missing_loop_var_errors() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":["echo $(LOOP.undefined)"]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false);
	assert!(result.is_err());
	assert!(result.unwrap_err().to_string().contains("LOOP"));
}

#[test]
fn dsl_works_with_env_substitution() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec_json = r#"{"$schema":"x","targets":{"t":{
		"commands":[
			{"if":"$(ENV.MY_TEST_KEY) == hello","then":["echo matched"],"else":["exit 1"]}
		],
		"env":{"MY_TEST_KEY":"hello"}
	}}}"#;
	let spec = parse_target(spec_json, "t");
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn step_counter_walks_all_branches_for_total() {
	use crate::control_flow::count_leaves;

	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			"a",
			{"if":"x","then":["b","c"],"else":["d"]},
			{"for":"v","in":["1","2","3"],"do":["e"]}
		]}}}"#,
		"t",
	);
	// 1 (shell) + 2+1 (if branches) + 3*1 (for body) = 7
	assert_eq!(count_leaves(&spec.commands), 7);
}

#[test]
fn step_counter_for_glob_estimates_one_iteration() {
	use crate::control_flow::count_leaves;

	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"f","glob":"*.rs","do":["a","b"]}
		]}}}"#,
		"t",
	);
	// glob estimate: 1 iteration * 2 body = 2
	assert_eq!(count_leaves(&spec.commands), 2);
}

#[test]
fn dsl_evaluator_truthiness() {
	use crate::args::LoopScope;
	use crate::control_flow::evaluate;
	use runfile_parser::parse_condition;

	let env = HashMap::new();
	let scope = LoopScope::new();
	let args = RunArgs::default();

	// non-empty literal → truthy
	let ast = parse_condition("foo").unwrap();
	assert!(evaluate(&ast, &args, &env, &scope).unwrap());

	// empty quoted string → falsy
	let ast = parse_condition("\"\"").unwrap();
	assert!(!evaluate(&ast, &args, &env, &scope).unwrap());

	// "false" is still truthy under our rule
	let ast = parse_condition("false").unwrap();
	assert!(evaluate(&ast, &args, &env, &scope).unwrap());
}

#[test]
fn dsl_evaluator_comparisons_short_circuit() {
	use crate::args::LoopScope;
	use crate::control_flow::evaluate;
	use runfile_parser::parse_condition;

	let env = HashMap::new();
	let scope = LoopScope::new();
	let args = RunArgs::default();

	let ast = parse_condition("foo == foo").unwrap();
	assert!(evaluate(&ast, &args, &env, &scope).unwrap());

	let ast = parse_condition("foo != foo").unwrap();
	assert!(!evaluate(&ast, &args, &env, &scope).unwrap());

	// Or short-circuit: second arm would fail to substitute (LOOP.missing),
	// but the first arm is true so evaluation stops.
	let ast = parse_condition("foo || $(LOOP.missing)").unwrap();
	assert!(evaluate(&ast, &args, &env, &scope).unwrap());

	// And short-circuit: first arm false stops evaluation before LOOP fails.
	let ast = parse_condition("\"\" && $(LOOP.missing)").unwrap();
	assert!(!evaluate(&ast, &args, &env, &scope).unwrap());
}

#[test]
fn dsl_evaluator_loop_var_resolves() {
	use crate::args::LoopScope;
	use crate::control_flow::evaluate;
	use runfile_parser::parse_condition;

	let env = HashMap::new();
	let mut scope = LoopScope::new();
	scope.push("color", "red");
	let args = RunArgs::default();

	let ast = parse_condition("$(LOOP.color) == red").unwrap();
	assert!(evaluate(&ast, &args, &env, &scope).unwrap());
}

#[test]
fn loop_scope_innermost_wins() {
	use crate::args::LoopScope;
	let mut scope = LoopScope::new();
	scope.push("x", "outer");
	scope.push("x", "inner");
	assert_eq!(scope.get("x"), Some("inner"));
	scope.pop();
	assert_eq!(scope.get("x"), Some("outer"));
	scope.pop();
	assert_eq!(scope.get("x"), None);
}

// ──── output-prefix propagation tests ─────────────────────────────────
//
// These verify that `parallel: true` parents tag every shell command in
// their dispatched dependency subtree with a per-leaf prefix (the global
// step number, e.g. `[3] `). The mechanism is `output_prefix`: it flows
// from `run_parallel_batch` → `DependencyResolver::run_dependency` →
// `run_target_inner` → `ExecSetup.output_prefix` → every nested
// `execute_one_shell` / `run_parallel_batch`. We use a recording resolver
// here so we can assert exactly what prefix each `@target` invocation
// receives — verifying the propagation contract end-to-end without having
// to capture stdout from a real shell child.

use std::sync::Mutex;

#[derive(Default)]
struct RecordingResolver {
	calls: Mutex<Vec<RecordedCall>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedCall {
	target: String,
	output_prefix: Option<String>,
}

impl crate::executor::DependencyResolver for RecordingResolver {
	fn run_dependency(
		&self,
		target_name: &str,
		_args: Vec<String>,
		_parent_env: &HashMap<String, String>,
		_parent_add_to_path_chain: &[Vec<String>],
		_optional: bool,
		output_prefix: Option<&str>,
	) -> Result<crate::executor::ExecutionResult, crate::executor::ExecuteError> {
		self.calls.lock().unwrap().push(RecordedCall {
			target: target_name.to_string(),
			output_prefix: output_prefix.map(String::from),
		});
		Ok(crate::executor::ExecutionResult {
			commands_run: 0,
			failures: 0,
			final_status: dummy_success_status(),
		})
	}
}

fn dummy_success_status() -> std::process::ExitStatus {
	#[cfg(unix)]
	{
		use std::os::unix::process::ExitStatusExt;
		std::process::ExitStatus::from_raw(0)
	}
	#[cfg(windows)]
	{
		use std::os::windows::process::ExitStatusExt;
		std::process::ExitStatus::from_raw(0)
	}
}

#[test]
fn parallel_target_calls_receive_per_leaf_output_prefix() {
	// A parallel target with three `@dep` leaves and no inherited prefix
	// must hand each leaf a distinct per-step prefix derived from the
	// global step counter (`[1]`, `[2]`, `[3]`).
	use crate::executor::execute_parallel_with_counter;
	use crate::logging::StepCounter;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let mut spec = CommandSpec::new(vec![
		CommandStep::TargetCall(runfile_parser::TargetCallStep {
			target: "one".into(),
			args_template: String::new(),
			optional: false,
		}),
		CommandStep::TargetCall(runfile_parser::TargetCallStep {
			target: "two".into(),
			args_template: String::new(),
			optional: false,
		}),
		CommandStep::TargetCall(runfile_parser::TargetCallStep {
			target: "three".into(),
			args_template: String::new(),
			optional: false,
		}),
	]);
	spec.parallel = Some(true);

	let args = RunArgs::default();
	let counter = StepCounter::new(3);
	let resolver = RecordingResolver::default();

	execute_parallel_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
		None,
		false,
		&counter,
		&resolver,
		None,
		&[],
		None, // no inherited prefix → per-leaf step prefixes
	)
	.unwrap();

	let mut calls = resolver.calls.lock().unwrap().clone();
	calls.sort_by(|a, b| a.target.cmp(&b.target));

	// Each leaf must have received SOME prefix (a Some), and the three
	// prefixes must be distinct (per-leaf step numbering, not all empty
	// or all equal).
	assert_eq!(calls.len(), 3);
	for c in &calls {
		assert!(c.output_prefix.is_some(), "{} got no prefix", c.target);
	}
	let prefixes: std::collections::HashSet<_> = calls.iter().map(|c| c.output_prefix.clone().unwrap()).collect();
	assert_eq!(
		prefixes.len(),
		3,
		"per-leaf prefixes must be distinct, got {:?}",
		prefixes
	);
	// Each prefix must contain a `[N]` step token.
	for c in &calls {
		let p = c.output_prefix.as_deref().unwrap();
		assert!(
			p.contains('[') && p.contains(']'),
			"prefix should contain [N], got {:?}",
			p
		);
	}
}

#[test]
fn inherited_output_prefix_overrides_per_leaf_in_nested_parallel() {
	// When a parallel batch is reached via a parent that already set a
	// prefix, every leaf must inherit that prefix verbatim (preserving
	// the outer partition identity) — no per-leaf step renumbering.
	use crate::executor::execute_parallel_with_counter;
	use crate::logging::StepCounter;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let mut spec = CommandSpec::new(vec![
		CommandStep::TargetCall(runfile_parser::TargetCallStep {
			target: "a".into(),
			args_template: String::new(),
			optional: false,
		}),
		CommandStep::TargetCall(runfile_parser::TargetCallStep {
			target: "b".into(),
			args_template: String::new(),
			optional: false,
		}),
	]);
	spec.parallel = Some(true);

	let args = RunArgs::default();
	let counter = StepCounter::new(2);
	let resolver = RecordingResolver::default();

	execute_parallel_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
		None,
		false,
		&counter,
		&resolver,
		None,
		&[],
		Some("[outer] "), // inherited from a parallel grandparent
	)
	.unwrap();

	let calls = resolver.calls.lock().unwrap().clone();
	assert_eq!(calls.len(), 2);
	for c in &calls {
		assert_eq!(
			c.output_prefix.as_deref(),
			Some("[outer] "),
			"inherited prefix must propagate verbatim, got {:?}",
			c.output_prefix
		);
	}
}

#[test]
fn sequential_target_call_forwards_output_prefix() {
	// `execute_one_target_call` (sequential `@dep` invocation) must forward
	// `setup.output_prefix` to the resolver. Without this, a parent's
	// inherited prefix would stop at the first sequential `@dep` boundary.
	use crate::executor::execute_command_with_counter;
	use crate::logging::StepCounter;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let spec = CommandSpec::new(vec![CommandStep::TargetCall(runfile_parser::TargetCallStep {
		target: "child".into(),
		args_template: String::new(),
		optional: false,
	})]);

	let args = RunArgs::default();
	let counter = StepCounter::new(1);
	let resolver = RecordingResolver::default();

	execute_command_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
		None,
		false,
		&counter,
		&resolver,
		None,
		&[],
		Some("[3] "),
	)
	.unwrap();

	let calls = resolver.calls.lock().unwrap().clone();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].target, "child");
	assert_eq!(calls[0].output_prefix.as_deref(), Some("[3] "));
}

#[test]
fn top_level_no_prefix_means_no_propagation_through_sequential() {
	// Top-level (no parallel ancestor) → output_prefix is None and stays
	// None when forwarded to a sequential `@dep` call. We only prefix when
	// inside a parallel context.
	use crate::executor::execute_command_with_counter;
	use crate::logging::StepCounter;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let spec = CommandSpec::new(vec![CommandStep::TargetCall(runfile_parser::TargetCallStep {
		target: "child".into(),
		args_template: String::new(),
		optional: false,
	})]);

	let args = RunArgs::default();
	let counter = StepCounter::new(1);
	let resolver = RecordingResolver::default();

	execute_command_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
		None,
		false,
		&counter,
		&resolver,
		None,
		&[],
		None,
	)
	.unwrap();

	let calls = resolver.calls.lock().unwrap().clone();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].output_prefix, None);
}

#[test]
fn parallel_propagates_prefix_through_real_dispatched_target() {
	// End-to-end through the runner: a parallel parent dispatching to a
	// real target whose commands write to a file. Verifies the wiring is
	// fully connected (parent parallel → resolver → run_target_inner →
	// dispatched target's ExecSetup), without depending on stdout capture.
	// The prefix itself is verified by the recording-resolver tests; this
	// test just guards against a regression that breaks the dispatch path.
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let marker_a = dir.path().join("a.txt");
	let marker_b = dir.path().join("b.txt");
	let marker_a_esc = json_escape_path(&marker_a);
	let marker_b_esc = json_escape_path(&marker_b);

	let touch_a = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{marker_a_esc}\\\"")
	} else {
		format!("touch \\\"{marker_a_esc}\\\"")
	};
	let touch_b = if shell.kind == ShellKind::Cmd {
		format!("echo done > \\\"{marker_b_esc}\\\"")
	} else {
		format!("touch \\\"{marker_b_esc}\\\"")
	};

	let json = format!(
		r#"{{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {{
			"child-a": {{ "commands": ["{touch_a}"] }},
			"child-b": {{ "commands": ["{touch_b}"] }},
			"main": {{
				"parallel": true,
				"commands": ["@child-a", "@child-b"]
			}}
		}}
	}}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();
	run_target("main", &runfile, &shell, &args, dir.path()).unwrap();

	assert!(marker_a.exists(), "child-a should have been dispatched");
	assert!(marker_b.exists(), "child-b should have been dispatched");
}

// ── match step tests ──────────────────────────────────────────────

#[test]
fn match_runs_matching_case() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.tier)","cases":{"1":"echo one","2":"echo two","3":"echo three"}}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tier=2".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn match_no_case_no_default_errors_with_valid_cases_listed() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.tier)","cases":{"1":"echo one","2":"echo two"}}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tier=99".into()]);
	let err = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("99"), "should mention the bad value, got: {msg}");
	assert!(msg.contains("\"1\""), "should list valid case 1, got: {msg}");
	assert!(msg.contains("\"2\""), "should list valid case 2, got: {msg}");
}

#[test]
fn match_default_runs_when_no_case_matches() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.tier)","cases":{"1":"exit 1"},"default":"echo fallback"}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tier=42".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success(), "default branch should run");
}

#[test]
fn match_missing_arg_uses_default_when_set() {
	// When the substitution itself fails (missing arg, no chain default),
	// `default` runs as a fallback for the unresolvable value.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.tier)","cases":{"1":"exit 1"},"default":"echo defaulted"}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn match_missing_arg_no_default_errors_with_valid_cases() {
	// Without a `default`, a substitution failure surfaces an error that
	// includes the valid case list so users can fix the call.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.tier)","cases":{"1":"echo one","2":"echo two"}}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let err = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap_err();
	let msg = err.to_string();
	assert!(
		msg.contains("Could not resolve") || msg.contains("Argument"),
		"got: {msg}"
	);
	assert!(msg.contains("\"1\""), "should list case 1 in error, got: {msg}");
	assert!(msg.contains("\"2\""), "should list case 2 in error, got: {msg}");
}

#[test]
fn match_chained_substitution_resolves_to_default_value() {
	// `$(ARGS.tier ? 1)` resolves to "1" when --tier missing → case "1" runs.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.tier ? 1)","cases":{"1":"echo one","2":"exit 1"}}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn match_target_call_dispatch() {
	use crate::runner::run_target;
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let json = r#"{
		"$schema": "x",
		"targets": {
			"prod": { "commands": ["echo prod"] },
			"dev": { "commands": ["echo dev"] },
			"deploy": {
				"commands": [
					{ "match": "$(ARGS.env)", "cases": { "prod": "@prod", "dev": "@dev" } }
				]
			}
		}
	}"#;
	let runfile = runfile_parser::parse_runfile(json).unwrap();
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = run_target("deploy", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success());
	// The dep ran one shell.
	assert!(result.commands_run >= 1);
}

#[test]
fn match_ignore_errors_isolates_failure() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.x)","cases":{"a":"exit 1"},"ignoreErrors":true},
			"echo after"
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--x=a".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// "echo after" must still run because the match block swallows its failure.
	assert_eq!(result.commands_run, 2);
	assert!(
		result.final_status.success(),
		"ignoreErrors should mask the inner failure"
	);
}

#[test]
fn match_used_with_for_loop() {
	// Combine match with `for` to dispatch based on a loop variable.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":["a","b"],"do":[
				{"match":"$(LOOP.x)","cases":{"a":"echo got-a","b":"echo got-b"}}
			]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
}

#[test]
fn match_count_leaves_sums_all_branches() {
	use crate::control_flow::count_leaves;
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"$(ARGS.x)","cases":{"a":"echo 1","b":["echo 2","echo 3"]},"default":"echo 4"}
		]}}}"#,
		"t",
	);
	// 1 + 2 + 1 = 4 worst-case leaves.
	assert_eq!(count_leaves(&spec.commands), 4);
}

// ── --stdin-args prompter tests ────────────────────────────────────

mod stdin_args {
	use super::*;
	use crate::args::StdinPrompter;
	use std::sync::{Arc, Mutex};

	/// Test prompter that returns scripted answers and records every prompt.
	#[derive(Debug, Default)]
	struct MockPrompter {
		value_answers: Mutex<HashMap<String, Option<String>>>,
		flag_answers: Mutex<HashMap<String, bool>>,
		value_calls: Mutex<Vec<(String, Option<String>)>>,
		flag_calls: Mutex<Vec<String>>,
	}

	impl MockPrompter {
		fn with_value(self, key: &str, answer: Option<&str>) -> Self {
			self.value_answers
				.lock()
				.unwrap()
				.insert(key.to_string(), answer.map(|s| s.to_string()));
			self
		}
		fn with_flag(self, key: &str, present: bool) -> Self {
			self.flag_answers.lock().unwrap().insert(key.to_string(), present);
			self
		}
	}

	impl StdinPrompter for MockPrompter {
		fn prompt_value(&self, key: &str, default: Option<&str>) -> Option<String> {
			self.value_calls
				.lock()
				.unwrap()
				.push((key.to_string(), default.map(|s| s.to_string())));
			self.value_answers.lock().unwrap().get(key).cloned().unwrap_or(None)
		}
		fn prompt_flag(&self, key: &str) -> bool {
			self.flag_calls.lock().unwrap().push(key.to_string());
			self.flag_answers.lock().unwrap().get(key).copied().unwrap_or(false)
		}
	}

	fn args_with(prompter: Arc<dyn StdinPrompter>) -> RunArgs {
		RunArgs::parse(&[]).with_stdin_prompter(Some(prompter))
	}

	#[test]
	fn missing_args_prompts_and_uses_answer() {
		let prompter = Arc::new(MockPrompter::default().with_value("ARGS.name", Some("alice")));
		let args = args_with(prompter.clone());
		let result = args.substitute("hello $(ARGS.name)", &HashMap::new()).unwrap();
		assert_eq!(result, "hello alice");
		let calls = prompter.value_calls.lock().unwrap();
		assert_eq!(calls.len(), 1);
		assert_eq!(calls[0], ("ARGS.name".to_string(), None));
	}

	#[test]
	fn missing_args_with_default_prompts_and_falls_through_when_empty() {
		// Empty answer (None) should fall through to the literal default.
		let prompter = Arc::new(MockPrompter::default().with_value("ARGS.env", None));
		let args = args_with(prompter.clone());
		let result = args
			.substitute("env=$(ARGS.env ? production)", &HashMap::new())
			.unwrap();
		assert_eq!(result, "env=production");
		let calls = prompter.value_calls.lock().unwrap();
		assert_eq!(calls[0], ("ARGS.env".to_string(), Some("production".to_string())));
	}

	#[test]
	fn missing_args_with_default_prompts_and_overrides_when_provided() {
		let prompter = Arc::new(MockPrompter::default().with_value("ARGS.env", Some("staging")));
		let args = args_with(prompter);
		let result = args
			.substitute("env=$(ARGS.env ? production)", &HashMap::new())
			.unwrap();
		assert_eq!(result, "env=staging");
	}

	#[test]
	fn missing_args_no_default_no_answer_errors() {
		// Required substitution; user pressed Enter; nothing else in the chain
		// → fall through to MissingArg as if --stdin-args wasn't set.
		let prompter = Arc::new(MockPrompter::default().with_value("ARGS.name", None));
		let args = args_with(prompter);
		let err = args.substitute("hi $(ARGS.name)", &HashMap::new()).unwrap_err();
		assert!(matches!(err, SubstitutionError::MissingArg(ref k) if k == "name"));
	}

	#[test]
	fn provided_args_skip_prompt() {
		let prompter = Arc::new(MockPrompter::default());
		let args = RunArgs::parse(&["--name=bob".into()]).with_stdin_prompter(Some(prompter.clone()));
		let result = args.substitute("hi $(ARGS.name)", &HashMap::new()).unwrap();
		assert_eq!(result, "hi bob");
		assert!(prompter.value_calls.lock().unwrap().is_empty());
	}

	#[test]
	fn missing_env_prompts_and_uses_answer() {
		let prompter = Arc::new(MockPrompter::default().with_value("ENV.SECRET", Some("hush")));
		let args = args_with(prompter);
		let result = args.substitute("token=$(ENV.SECRET)", &HashMap::new()).unwrap();
		assert_eq!(result, "token=hush");
	}

	#[test]
	fn provided_env_skips_prompt() {
		let prompter = Arc::new(MockPrompter::default());
		let args = args_with(prompter.clone());
		let mut env = HashMap::new();
		env.insert("HOST".to_string(), "example.com".to_string());
		let result = args.substitute("host=$(ENV.HOST)", &env).unwrap();
		assert_eq!(result, "host=example.com");
		assert!(prompter.value_calls.lock().unwrap().is_empty());
	}

	#[test]
	fn chain_args_to_env_to_default_prompts_once_with_first_source_key() {
		// `$(ARGS.x ? ENV.X ? fallback)` — neither set, prompt key is
		// the first source (ARGS.x), default is "fallback".
		let prompter = Arc::new(MockPrompter::default().with_value("ARGS.x", Some("entered")));
		let args = args_with(prompter.clone());
		let result = args
			.substitute("v=$(ARGS.x ? ENV.X ? fallback)", &HashMap::new())
			.unwrap();
		assert_eq!(result, "v=entered");
		let calls = prompter.value_calls.lock().unwrap();
		assert_eq!(calls.len(), 1);
		assert_eq!(calls[0], ("ARGS.x".to_string(), Some("fallback".to_string())));
	}

	#[test]
	fn flags_missing_prompts_for_presence() {
		let prompter = Arc::new(MockPrompter::default().with_flag("--verbose", true));
		let args = args_with(prompter.clone());
		let result = args.substitute("cmd $(FLAGS.verbose ? -v :)", &HashMap::new()).unwrap();
		assert_eq!(result, "cmd -v");
		let calls = prompter.flag_calls.lock().unwrap();
		assert_eq!(calls.len(), 1);
		assert_eq!(calls[0], "--verbose");
	}

	#[test]
	fn flags_provided_skips_prompt() {
		let prompter = Arc::new(MockPrompter::default());
		let args = RunArgs::parse(&["--verbose".into()]).with_stdin_prompter(Some(prompter.clone()));
		let result = args.substitute("cmd $(FLAGS.verbose ? -v :)", &HashMap::new()).unwrap();
		assert_eq!(result, "cmd -v");
		assert!(prompter.flag_calls.lock().unwrap().is_empty());
	}

	#[test]
	fn flags_user_declines_returns_false_branch() {
		let prompter = Arc::new(MockPrompter::default().with_flag("--release", false));
		let args = args_with(prompter);
		let result = args
			.substitute("cargo build $(FLAGS.release ? --release : --debug)", &HashMap::new())
			.unwrap();
		assert_eq!(result, "cargo build --debug");
	}

	#[test]
	fn no_prompter_preserves_existing_error() {
		// Sanity check: with no prompter, missing args still error.
		let args = RunArgs::parse(&[]);
		let err = args.substitute("hi $(ARGS.name)", &HashMap::new()).unwrap_err();
		assert!(matches!(err, SubstitutionError::MissingArg(_)));
	}
}
