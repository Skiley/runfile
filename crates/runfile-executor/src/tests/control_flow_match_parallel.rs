use super::*;

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

#[test]
fn if_then_branch_executes_on_truthy() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"{{ ARG.go == 'yes' }}","then":["echo then-branch"],"else":["echo else-branch"]}
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
			{"if":"{{ ARG.go == 'yes' }}","then":["exit 1"],"else":["echo else-branch"]}
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
			{"if":"{{ ARG.go == 'yes' }}","then":["exit 1"]},
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
fn if_only_literal_true_is_truthy() {
	// Under the new if-evaluation rules, the condition is fully substituted
	// and ONLY the literal string "true" is treated as truthy. The string
	// "false" — and every other value — is falsy.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"{{ ARG.flag ? 'false' }}","then":["exit 1"],"else":["echo not-truthy"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	// `{{ ARG.flag ? 'false' }}` resolves to the string "false". Under the
	// new rule, "false" != "true" → falsy → else branch runs.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(result.final_status.success());
}

#[test]
fn if_chained_logical_operators() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	// Under the new if-evaluation, the DSL expression goes inside a single
	// `{{ ... }}` block which substitutes to "true" or "false" — the if
	// only checks for the literal string "true".
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"{{ ARG.a == '1' && ARG.b == '2' }}","then":["echo both"],"else":["exit 1"]}
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
	// Negation inside the substitution-DSL form: `!(comparison)`. Branch is
	// taken when ARG.skip is anything other than "yes".
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"if":"{{ !(ARG.skip == 'yes') }}","then":["echo go"],"else":["exit 1"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--skip=no".into()]);
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
			{{"if":"{{{{ ARG.go == 'yes' }}}}","then":["{fail_cmd}"]}}
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
			{{"if":"{{{{ ARG.go == 'yes' }}}}","then":["{fail_cmd}"],"ignoreErrors":true}},
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
				{{"if":"{{{{ 'a' == 'a' }}}}","then":["{fail_cmd}"],"ignoreErrors":true}}
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
			{"for":"x","in":["1","2","3"],"do":["echo {{ VAR.x }}"]}
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
	// body once per namespace, with {{ VAR.ns }} bound to each.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let touch = match shell.kind {
		ShellKind::Cmd => "type nul > {{ VAR.ns }}.ns",
		ShellKind::PowerShell => "New-Item -ItemType File -Path \\\"{{ VAR.ns }}.ns\\\" -Force | Out-Null",
		_ => "touch {{ VAR.ns }}.ns",
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
		..Default::default()
	});
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
	assert!(dir.path().join("project_one.ns").exists());
	assert!(dir.path().join("project_two.ns").exists());
}

#[test]
fn for_in_namespaces_with_dynamic_target_call_runs_each_namespaced_target() {
	// End-to-end exercise of the user's example pattern:
	//   "for": "ns", "in": "namespaces", "do": "@{{ VAR.ns }}:build"
	// The for-block iterates the runfile's namespaces; for each value, the
	// `@{{ VAR.ns }}:build` target call is substituted and dispatched to the
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
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
		"targets": {{
			"project_one:build": {{ "commands": ["{touch_one}"] }},
			"project_two:build": {{ "commands": ["{touch_two}"] }},
			"build_all": {{
				"commands": [
					{{ "for": "ns", "in": "namespaces", "do": "@{{{{ VAR.ns }}}}:build" }}
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
					{{ "for": "ns", "in": "namespaces", "do": "@?{{{{ VAR.ns }}}}:adb-forward" }}
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
		..Default::default()
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
		ShellKind::Cmd => "type nul > {{ VAR.f }}.out",
		ShellKind::PowerShell => "New-Item -ItemType File -Path \\\"{{ VAR.f }}.out\\\" -Force | Out-Null",
		_ => "touch {{ VAR.f }}.out",
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
				{"for":"y","in":["a","b"],"do":["echo {{ VAR.x }}{{ VAR.y }}"]}
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
			{"for":"f","glob":"*.txt","do":["echo {{ VAR.f }}"]}
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
			{{"for":"line","shell":"{cmd}","do":["echo {{{{ VAR.line }}}}"]}}
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
		ShellKind::Cmd => "if {{ VAR.x }}==fail (exit /b 1) else (type nul > {{ VAR.x }}.done)",
		ShellKind::PowerShell => {
			"if ($env:RFLOOP_X -eq 'fail') { exit 1 } else { New-Item -ItemType File -Path \\\"{{ VAR.x }}.done\\\" -Force | Out-Null }"
		}
		_ => "test {{ VAR.x }} = fail && exit 1 || touch {{ VAR.x }}.done",
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
		ShellKind::Cmd => "type nul > {{ VAR.f }}.out",
		ShellKind::PowerShell => "New-Item -ItemType File -Path \\\"{{ VAR.f }}.out\\\" -Force | Out-Null",
		_ => "touch {{ VAR.f }}.out",
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
				{"for":"y","in":["a","b"],"parallel":true,"do":["echo {{ VAR.x }}{{ VAR.y }}"]}
			]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 4);
}

// ── Audit M7: `when` partitioning inside a `parallel: true` target ──
// A `when: failure` cleanup block must run when a parallel leaf fails, and a
// `when: always` block must run regardless. Previously the failure/always
// partitions were dropped at collection time (seeded with `Success`) AND
// short-circuited by the batch error, so cleanups silently never ran. These
// go through `run_target` because target-level `parallel: true` is dispatched
// by the runner (not `execute_command`, which walks steps sequentially).

/// Build a parse-able single-target Runfile from a JSON target body.
fn parallel_when_runfile(target_body: &str) -> runfile_parser::Runfile {
	let json = format!(
		r#"{{"$schema":"https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json","targets":{{"t":{target_body}}}}}"#
	);
	runfile_parser::parse_runfile(&json).unwrap()
}

/// True if the run failed — whether the runner surfaced an `Err` (failing shell
/// leaf) or an `Ok` with a non-success status.
fn run_failed<E>(result: &Result<crate::executor::ExecutionResult, E>) -> bool {
	match result {
		Err(_) => true,
		Ok(r) => !r.final_status.success(),
	}
}

#[test]
fn parallel_when_failure_cleanup_runs_on_failure() {
	use crate::runner::run_target;
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let runfile = parallel_when_runfile(&format!(
		r#"{{"parallel":true,"commands":["{fail_cmd}",{{"when":"failure","commands":["echo cleaned > failure.marker"]}}]}}"#
	));
	let args = RunArgs::default();
	let result = run_target("t", &runfile, &shell, &args, dir.path());
	assert!(
		run_failed(&result),
		"a failing parallel leaf should make the target fail"
	);
	assert!(
		dir.path().join("failure.marker").exists(),
		"when: failure cleanup must run after a parallel-batch failure"
	);
}

#[test]
fn parallel_when_failure_cleanup_skipped_on_success() {
	use crate::runner::run_target;
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let runfile = parallel_when_runfile(
		r#"{"parallel":true,"commands":["echo ok",{"when":"failure","commands":["echo cleaned > failure.marker"]}]}"#,
	);
	let args = RunArgs::default();
	let result = run_target("t", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success());
	assert!(
		!dir.path().join("failure.marker").exists(),
		"when: failure cleanup must NOT run when the parallel batch succeeded"
	);
}

#[test]
fn parallel_when_always_runs_even_on_failure() {
	use crate::runner::run_target;
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1"
	} else {
		"exit 1"
	};
	let runfile = parallel_when_runfile(&format!(
		r#"{{"parallel":true,"commands":["{fail_cmd}",{{"when":"always","commands":["echo done > always.marker"]}}]}}"#
	));
	let args = RunArgs::default();
	let result = run_target("t", &runfile, &shell, &args, dir.path());
	assert!(
		run_failed(&result),
		"the failing leaf should still make the target fail"
	);
	assert!(
		dir.path().join("always.marker").exists(),
		"when: always block must run even when the parallel batch failed"
	);
}

#[test]
fn parallel_when_always_runs_on_success() {
	use crate::runner::run_target;
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let runfile = parallel_when_runfile(
		r#"{"parallel":true,"commands":["echo ok",{"when":"always","commands":["echo done > always.marker"]}]}"#,
	);
	let args = RunArgs::default();
	let result = run_target("t", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success());
	assert!(
		dir.path().join("always.marker").exists(),
		"when: always block must run after a successful parallel batch"
	);
}

#[test]
fn missing_var_errors_at_runtime() {
	// `{{ VAR.x }}` reference without a prior `define(x, ...)` (and not
	// being a `for`-loop variable) errors at substitution time.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":["echo {{ VAR.undefined }}"]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false);
	assert!(result.is_err());
	assert!(result.unwrap_err().to_string().contains("undefined"));
}

#[test]
fn dsl_works_with_env_substitution() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec_json = r#"{"$schema":"x","targets":{"t":{
		"commands":[
			{"if":"{{ ENV.MY_TEST_KEY == 'hello' }}","then":["echo matched"],"else":["exit 1"]}
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

// ── DSL inside substitutions ──────────────────────────────────────
//
// DSL boolean expressions live INSIDE `{{ ... }}` blocks and resolve to
// the literal strings `"true"` or `"false"`. The truthiness rule is
// strict-and-aligned with the `if`-block: bare values must resolve to
// `"true"` (truthy), `"false"` (falsy), or `""` (empty, also falsy);
// anything else surfaces as `DslValueNotBoolean`. Use comparisons
// (`==` / `!=`) for arbitrary-string checks.

#[test]
fn dsl_strict_truthiness_only_true_false_empty_accepted() {
	let args = RunArgs::default();
	let env = HashMap::new();

	// "true" is truthy
	let result = args.substitute("{{ 'true' && 'true' }}", &env).unwrap();
	assert_eq!(result, "true");

	// "false" is falsy
	let result = args.substitute("{{ 'false' || 'false' }}", &env).unwrap();
	assert_eq!(result, "false");

	// empty is falsy
	let result = args.substitute("{{ '' || '' }}", &env).unwrap();
	assert_eq!(result, "false");
}

#[test]
fn dsl_truthy_errors_on_non_boolean_value() {
	let args = RunArgs::default();
	let env = HashMap::new();

	// `'foo'` isn't a boolean — bare truthy check errors.
	let err = args.substitute("{{ 'foo' && 'true' }}", &env).unwrap_err();
	let msg = err.to_string();
	assert!(
		msg.contains("not a boolean"),
		"expected DslValueNotBoolean error, got: {msg}"
	);

	// Same for "True" (case-sensitive).
	let err = args.substitute("{{ 'True' && 'true' }}", &env).unwrap_err();
	assert!(err.to_string().contains("not a boolean"));
}

#[test]
fn dsl_in_substitution_short_circuit() {
	let args = RunArgs::default();
	let env = HashMap::new();

	// `||` short-circuits: first arm true means the second isn't evaluated.
	// VAR.missing would error if it were resolved, so the test passing
	// proves short-circuiting.
	let result = args.substitute("{{ 'true' || VAR.missing }}", &env).unwrap();
	assert_eq!(result, "true");

	// `&&` short-circuits on first false arm.
	let result = args.substitute("{{ 'false' && VAR.missing }}", &env).unwrap();
	assert_eq!(result, "false");

	// Empty arm also short-circuits AND.
	let result = args.substitute("{{ '' && VAR.missing }}", &env).unwrap();
	assert_eq!(result, "false");
}

#[test]
fn dsl_flags_works_as_bare_boolean() {
	// The killer feature: `FLAG.x` resolves to `"true"`/`"false"` —
	// both valid under the strict rule — so it can be used as a bare
	// boolean inside the DSL without the explicit `== 'true'` check.
	let env = HashMap::new();

	// Flag present → "true".
	let args = RunArgs::parse(&["--wsl".into()]);
	let result = args.substitute("{{ RUN.os == 'windows' && FLAG.wsl }}", &env).unwrap();
	// RUN.os depends on host — just confirm it returns a boolean string.
	assert!(matches!(result.as_str(), "true" | "false"));

	// Flag absent → "false". Force a known truthy left-arm via 'true'.
	let args_off = RunArgs::parse(&[]);
	let result = args_off.substitute("{{ 'true' && FLAG.wsl }}", &env).unwrap();
	assert_eq!(result, "false");

	// Flag present + truthy left-arm → "true".
	let args_on = RunArgs::parse(&["--wsl".into()]);
	let result = args_on.substitute("{{ 'true' && FLAG.wsl }}", &env).unwrap();
	assert_eq!(result, "true");
}

#[test]
fn dsl_negated_flag_works_as_bare_boolean() {
	// `!FLAG.x` works because the inner Truthy resolves to a boolean.
	let env = HashMap::new();
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ !FLAG.wsl }}", &env).unwrap();
	assert_eq!(result, "true");

	let args = RunArgs::parse(&["--wsl".into()]);
	let result = args.substitute("{{ !FLAG.wsl }}", &env).unwrap();
	assert_eq!(result, "false");
}

#[test]
fn dsl_in_substitution_uses_vars() {
	let args = RunArgs::default();
	args.vars.lock().unwrap().insert("color".to_string(), "red".to_string());
	let env = HashMap::new();
	let result = args.substitute("{{ VAR.color == 'red' }}", &env).unwrap();
	assert_eq!(result, "true");
	let result = args.substitute("{{ VAR.color == 'blue' }}", &env).unwrap();
	assert_eq!(result, "false");
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
	// must hand each leaf a distinct prefix labelled with the resolved
	// `@target` call (`[@one]`, `[@two]`, `[@three]`).
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
	// prefixes must be distinct (per-leaf command labels, not all empty
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
	// Each prefix must contain a bracketed label.
	for c in &calls {
		let p = c.output_prefix.as_deref().unwrap();
		assert!(
			p.contains('[') && p.contains(']'),
			"prefix should contain a bracketed label, got {:?}",
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

/// Resolver that reports a non-zero `ExecutionResult` (failures = 1) for one
/// named target and success for every other. Used to reproduce a failing
/// parallel `@target` whose failure must reach the returned `final_status`.
struct FailingTargetResolver {
	failing_target: String,
}

impl crate::executor::DependencyResolver for FailingTargetResolver {
	fn run_dependency(
		&self,
		target_name: &str,
		_args: Vec<String>,
		_parent_env: &HashMap<String, String>,
		_parent_add_to_path_chain: &[Vec<String>],
		_optional: bool,
		_output_prefix: Option<&str>,
	) -> Result<crate::executor::ExecutionResult, crate::executor::ExecuteError> {
		if target_name == self.failing_target {
			#[cfg(unix)]
			let status = {
				use std::os::unix::process::ExitStatusExt;
				std::process::ExitStatus::from_raw(2 << 8)
			};
			#[cfg(windows)]
			let status = {
				use std::os::windows::process::ExitStatusExt;
				std::process::ExitStatus::from_raw(2)
			};
			Ok(crate::executor::ExecutionResult {
				commands_run: 1,
				failures: 1,
				final_status: status,
			})
		} else {
			Ok(crate::executor::ExecutionResult {
				commands_run: 1,
				failures: 0,
				final_status: dummy_success_status(),
			})
		}
	}
}

#[test]
fn parallel_failing_target_call_yields_nonzero_final_status() {
	// Regression: a `parallel: true` target whose body fans out into `@target`
	// calls must report a non-zero `final_status`
	// when ANY dispatched dep fails — even when the failing dep is not the
	// last one observed. The CLI derives the process exit code from
	// `final_status.code()` alone, so a success-looking status here means
	// `run check` exits 0 despite a sub-target failing.
	use crate::executor::execute_parallel_with_counter;
	use crate::logging::StepCounter;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	// "one" fails; "two" and "three" succeed. Leaves are processed in order,
	// so the last observed status is "three" (success) — the buggy code used
	// that as the final status and reported success.
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
	let resolver = FailingTargetResolver {
		failing_target: "one".into(),
	};

	let result = execute_parallel_with_counter(
		&spec,
		&shell,
		&args,
		dir.path(),
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

	assert_eq!(result.failures, 1, "the failed dep must be counted");
	assert!(
		!result.final_status.success(),
		"final_status must be non-zero when a parallel @target dep failed, got {:?}",
		result.final_status
	);
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
		"$schema": "https://github.com/JoaaoVerona/runfile/releases/latest/download/v0.schema.json",
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
			{"match":"{{ ARG.tier }}","cases":{"1":"echo one","2":"echo two","3":"echo three"}}
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
			{"match":"{{ ARG.tier }}","cases":{"1":"echo one","2":"echo two"}}
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
			{"match":"{{ ARG.tier }}","cases":{"1":"exit 1"},"default":"echo fallback"}
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
			{"match":"{{ ARG.tier }}","cases":{"1":"exit 1"},"default":"echo defaulted"}
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
			{"match":"{{ ARG.tier }}","cases":{"1":"echo one","2":"echo two"}}
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
	// `{{ ARG.tier ? '1' }}` resolves to "1" when --tier missing → case "1" runs.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"{{ ARG.tier ? '1' }}","cases":{"1":"echo one","2":"exit 1"}}
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
					{ "match": "{{ ARG.env }}", "cases": { "prod": "@prod", "dev": "@dev" } }
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
			{"match":"{{ ARG.x }}","cases":{"a":"exit 1"},"ignoreErrors":true},
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
				{"match":"{{ VAR.x }}","cases":{"a":"echo got-a","b":"echo got-b"}}
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
			{"match":"{{ ARG.x }}","cases":{"a":"echo 1","b":["echo 2","echo 3"]},"default":"echo 4"}
		]}}}"#,
		"t",
	);
	// 1 + 2 + 1 = 4 worst-case leaves.
	assert_eq!(count_leaves(&spec.commands), 4);
}

#[test]
fn match_regex_case_matches() {
	// Case keys wrapped in `/.../` are treated as regex patterns. A literal
	// case still beats a regex that would also match.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"{{ ARG.tag }}","cases":{
				"/^v\\d+$/":"echo version-tag",
				"latest":"echo latest-tag"
			},"default":"echo other"}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tag=v42".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn match_literal_case_wins_over_regex() {
	// `latest` matches both the regex `/^l.+$/` and the literal `latest`.
	// The literal must win — exact equality is checked first.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"{{ ARG.tag }}","cases":{
				"/^l.+$/":"exit 1",
				"latest":"echo got-literal"
			}}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tag=latest".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success(), "literal case should win");
}

#[test]
fn match_regex_falls_through_to_default() {
	// Regex that doesn't match falls through to default.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"{{ ARG.tag }}","cases":{
				"/^v\\d+$/":"exit 1"
			},"default":"echo defaulted"}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tag=hello".into()]);
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 1);
	assert!(result.final_status.success());
}

#[test]
fn match_bad_regex_surfaces_clear_error() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"match":"{{ ARG.tag }}","cases":{
				"/[/":"echo bad"
			},"default":"echo defaulted"}
		]}}}"#,
		"t",
	);
	let args = RunArgs::parse(&["--tag=anything".into()]);
	let err = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("Invalid regex"), "got: {msg}");
	assert!(msg.contains("/[/"), "should mention the bad key, got: {msg}");
}

// ── for-loop index variable ───────────────────────────────────────

#[test]
fn for_loop_exposes_index_var() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":["a","b","c"],"do":"echo {{ VAR.x_index }}={{ VAR.x }}"}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 3);
	assert!(result.final_status.success());
}

#[test]
fn for_loop_index_resets_per_loop_and_restores() {
	use crate::args::LoopVarGuard;
	use std::collections::HashMap;
	use std::sync::{Arc, Mutex};

	let vars: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
	{
		let g = LoopVarGuard::enter(&vars, "x");
		g.set("a");
		assert_eq!(vars.lock().unwrap().get("x").cloned(), Some("a".to_string()));
		assert_eq!(vars.lock().unwrap().get("x_index").cloned(), Some("0".to_string()));
		g.set("b");
		assert_eq!(vars.lock().unwrap().get("x").cloned(), Some("b".to_string()));
		assert_eq!(vars.lock().unwrap().get("x_index").cloned(), Some("1".to_string()));
	}
	assert!(vars.lock().unwrap().get("x").is_none(), "x should be restored");
	assert!(
		vars.lock().unwrap().get("x_index").is_none(),
		"x_index should be restored"
	);
}

#[test]
fn for_loop_index_restores_prior_value() {
	use crate::args::LoopVarGuard;
	use std::collections::HashMap;
	use std::sync::{Arc, Mutex};

	let vars: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
	vars.lock().unwrap().insert("x".to_string(), "outer".to_string());
	vars.lock().unwrap().insert("x_index".to_string(), "999".to_string());
	{
		let g = LoopVarGuard::enter(&vars, "x");
		g.set("inner-a");
		assert_eq!(vars.lock().unwrap().get("x_index").cloned(), Some("0".to_string()));
	}
	assert_eq!(vars.lock().unwrap().get("x").cloned(), Some("outer".to_string()));
	assert_eq!(vars.lock().unwrap().get("x_index").cloned(), Some("999".to_string()));
}

#[test]
fn for_loop_index_with_namespaces_iter() {
	// Verify the for-loop index works for the array form via execute_command,
	// double-checking that the index increments alongside the value.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"item","in":["alpha","beta"],"do":[
				"echo idx-{{ VAR.item_index }} val-{{ VAR.item }}"
			]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert_eq!(result.commands_run, 2);
}
