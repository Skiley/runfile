use super::*;
use crate::executor::execute_command;

#[test]
fn define_only_line_does_not_dispatch_to_shell() {
	let dir = TempDir::new().unwrap();
	let shell = get_test_shell();
	// Two commands: a `define`, then a real command that prints the
	// captured value. If `define` were dispatched as a literal empty
	// shell command, some shells would error or print something —
	// the runtime detects the empty substitution and skips dispatch
	// entirely. Empty commands are also NOT counted toward
	// `commands_run` and do NOT consume a step number, so a target
	// whose body is `[define, echo]` reports a single executed
	// command instead of two.
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			"{{ define(greeting, 'hello') }}",
			"echo {{ VAR.greeting }}"
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1, "only the real `echo` should count");
	assert_eq!(result.failures, 0);
}

#[test]
fn define_only_target_reports_success_and_zero_commands() {
	// Edge case: a target whose entire body is `define`-only lines
	// (no actual shell dispatch). With the empty-skip rule, the
	// run completes without invoking any shell, `commands_run` is
	// 0, and the final status is success — `final_status` falls
	// back to `dummy_success_status()` when no command set a status.
	let dir = TempDir::new().unwrap();
	let shell = get_test_shell();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			"{{ define(a, 'x') }}",
			"{{ define(b, 'y') }}"
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 0);
	assert_eq!(result.failures, 0);
	assert!(result.final_status.success());
}

#[test]
fn empty_command_decrements_step_counter_total() {
	// Regression for the "empty commands inflate (N/total)" bug. The
	// static `count_leaves` pass counts every Shell step, but a step
	// whose template resolves to whitespace is a runtime no-op — we
	// decrement the counter total at the moment of the skip so the
	// visible `(N/total)` ratio reflects only commands that will
	// actually run. Verifying via the externally-provided counter
	// (rather than via stderr) makes the test deterministic.
	use crate::executor::{NoOpDependencyResolver, execute_command_with_counter};
	use crate::logging::StepCounter;

	let dir = TempDir::new().unwrap();
	let shell = get_test_shell();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			"{{ define(a, 'x') }}",
			"echo step-2",
			"{{ define(b, 'y') }}",
			"echo step-4"
		]}}}"#,
		"t",
	);
	// Static count: 4 Shell leaves. Two of them resolve to "" so
	// the total should be decremented to 2 by the time the run
	// finishes.
	let counter = StepCounter::new(4);
	assert_eq!(counter.total(), 4, "sanity: planning total starts at 4");

	let args = RunArgs::default();
	let _ = execute_command_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
		dir.path(),
		None,
		false,
		&counter,
		&NoOpDependencyResolver,
		None,
		&[],
		None,
	)
	.unwrap();

	assert_eq!(
		counter.total(),
		2,
		"both define-only lines should have decremented the total"
	);
}

#[test]
fn empty_command_in_parallel_decrements_total() {
	// Same contract as the sequential case: the parallel collector
	// drops empty leaves at planning time, and the step counter
	// total must shrink to match — otherwise the user sees
	// `(2/4) [parallel]` for what is genuinely a 2-leaf batch.
	use crate::executor::{NoOpDependencyResolver, execute_parallel_with_counter};
	use crate::logging::StepCounter;

	let dir = TempDir::new().unwrap();
	let shell = get_test_shell();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			"{{ define(a, 'x') }}",
			"echo p1",
			"{{ define(b, 'y') }}",
			"echo p2"
		], "parallel": true}}}"#,
		"t",
	);
	let counter = StepCounter::new(4);
	let args = RunArgs::default();
	let _ = execute_parallel_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
		dir.path(),
		None,
		false,
		&counter,
		&NoOpDependencyResolver,
		None,
		&[],
		None,
	)
	.unwrap();

	assert_eq!(
		counter.total(),
		2,
		"parallel collector must shrink total to actual leaf count"
	);
}

#[test]
fn dry_run_skips_empty_define_only_lines() {
	// Dry-run already drops empty commands at the walker level. This
	// test pins the contract: a target with a `define`-only line
	// followed by a real command should print exactly one line —
	// the real command — never an empty line for the `define`.
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "t": {
                    "commands": [
                        "{{ define(x, 'foo') }}",
                        "echo {{ VAR.x }}",
                        "{{ define(y, 'bar') }}"
                    ]
                }
            }
        }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("t", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &ShellKind::Bash);

	assert_eq!(lines, vec!["echo foo".to_string()]);
	assert!(
		!lines.iter().any(|line| line.trim().is_empty()),
		"dry-run output must not contain empty lines"
	);
}
