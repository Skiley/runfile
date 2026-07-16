use super::*;

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
                    {{ "when": "always", "if": "{{{{ RUN.os == 'windows' }}}}", "then": "echo windows >> \"{log_escaped}\"", "else": "echo unix >> \"{log_escaped}\"" }}
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
	use crate::control_flow::{DetachFlattenError, collect_detach_leaves};
	use runfile_parser::{CommandStep, parse_runfile};
	use std::collections::HashMap;

	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"install": {
				"commands": [
					{
						"if": "{{ RUN.os == 'windows' }}",
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
	let append_arg = format!("echo _{{{{ ARGS }}}}_ >> \\\"{log_escaped}\\\"");

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
	let bump = format!("echo {{{{ ARG.tag }}}} >> \\\"{counter_escaped}\\\"");

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
                    {{ "if": "{{{{ ARG.env == 'prod' }}}}", "then": "@prod", "else": "@dev" }}
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
	// {{ ARGS }} in the args template should expand to the parent's args.
	use crate::runner::run_target;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("log.txt");
	let log_escaped = json_escape_path(&log);

	let _ = shell.kind;
	let echo = format!("echo {{{{ ARGS }}}} > \\\"{log_escaped}\\\"");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "echo": {{ "commands": ["{echo}"] }},
            "fwd": {{ "commands": ["@echo {{{{ ARGS }}}}"] }}
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
                "workingDirectory": "{{{{ RUN.cwd }}}}"
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let runfile_path = runfile_dir.path().join("Runfile.json");
	let result = run_target_with_cwd(
		"test-cwd",
		&runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
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
                "workingDirectory": "{{{{ RUN.cwd }}}}"
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let runfile_path = runfile_dir.path().join("Runfile.json");
	let result = run_target_with_cwd(
		"test-cwd",
		&runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
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

	// Target overrides with `{{ RUN.parent }}` — marker lands in runfile_dir,
	// proving the target override took effect.
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
                "workingDirectory": "{{{{ RUN.parent }}}}"
            }}
        }}
    }}"#
	);

	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let runfile_path = runfile_dir.path().join("Runfile.json");
	let result = run_target_with_cwd(
		"test-override",
		&runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
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
	// `workingDirectory` is a free-form path that supports `{{ ... }}`
	// substitution; chain fallbacks should resolve at runtime.
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

	// `{{ ARG.dir ? RUN.cwd }}` → falls back to RUN.cwd when --dir is missing.
	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": ["{touch}"],
                "workingDirectory": "{{{{ ARG.dir ? RUN.cwd }}}}"
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
fn working_directory_relative_path_resolves_against_runfile_parent() {
	// A bare relative `workingDirectory` path resolves against the target's
	// source Runfile directory, not the caller's CWD.
	use crate::runner::run_target_with_cwd;
	use runfile_parser::Runfile;

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();
	let nested = runfile_dir.path().join("nested");
	std::fs::create_dir(&nested).unwrap();

	let marker = nested.join("relative_marker");
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
            "t": {{
                "commands": ["{create_marker}"],
                "workingDirectory": "nested"
            }}
        }}
    }}"#
	);
	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let runfile_path = runfile_dir.path().join("Runfile.json");
	let result = run_target_with_cwd(
		"t",
		&runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
		&std::collections::HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(result.final_status.success());
	assert!(
		marker.exists(),
		"Relative workingDirectory should resolve against runfile parent, not caller CWD"
	);
}

#[test]
fn working_directory_resolves_env_from_globals() {
	// Regression: `{{ ENV.X }}` inside `workingDirectory` must resolve against
	// the target's OWN resolved env, including vars declared in `globals.env`
	// (which `merge_runfiles` bakes into every target). Previously
	// `workingDirectory` was substituted against only the parent env (empty at
	// top level), so this failed with "environment variable not set". A relative
	// marker proves the command actually ran in the resolved directory.
	use crate::runner::run_target_with_cwd;
	use runfile_parser::{merge_runfiles, parse_runfile};

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();
	let project_dir = TempDir::new().unwrap();

	let marker_name = "globals_env_wd.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};
	// Forward slashes are absolute on Windows too and need no JSON escaping.
	let project_path = project_dir.path().display().to_string().replace('\\', "/");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "globals": {{
            "env": {{ "PROJECT_PATH": "{project_path}" }}
        }},
        "targets": {{
            "build": {{
                "commands": ["{create_marker}"],
                "workingDirectory": "{{{{ ENV.PROJECT_PATH }}}}"
            }}
        }}
    }}"#
	);

	// `globals.env` is baked into each target's `env` during merge (not plain
	// parse), so go through `merge_runfiles` to mirror the real CLI pipeline.
	let runfile_path = runfile_dir.path().join("Runfile.json");
	let parsed = parse_runfile(&json).unwrap();
	let merged = merge_runfiles(Some((parsed, runfile_path.clone())), &[], runfile_dir.path()).unwrap();

	let args = RunArgs::default();
	let result = run_target_with_cwd(
		"build",
		&merged.runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&merged.source_dirs,
		&merged.source_files(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(result.final_status.success());
	assert!(
		project_dir.path().join(marker_name).exists(),
		"command should run in the dir from globals env (ENV.PROJECT_PATH)"
	);
}

#[test]
fn working_directory_resolves_target_env() {
	// `{{ ENV.X }}` inside `workingDirectory` resolves against the target's own
	// `env` block too (not only globals).
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let project_dir = TempDir::new().unwrap();
	let runfile_dir = TempDir::new().unwrap();

	let marker_name = "target_env_wd.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};
	let project_path = project_dir.path().display().to_string().replace('\\', "/");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "build": {{
                "commands": ["{create_marker}"],
                "env": {{ "TARGET_DIR": "{project_path}" }},
                "workingDirectory": "{{{{ ENV.TARGET_DIR }}}}"
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();
	let result = run_target("build", &runfile, &shell, &args, runfile_dir.path()).unwrap();
	assert!(result.final_status.success());
	assert!(
		project_dir.path().join(marker_name).exists(),
		"command should run in the dir from the target's own env (ENV.TARGET_DIR)"
	);
}

#[test]
fn working_directory_resolves_declared_var() {
	// `{{ VAR.X }}` inside `workingDirectory` resolves against the target's
	// declared `vars`, matching the `{{ ENV.X }}` behaviour. The declared vars
	// must be applied BEFORE `workingDirectory` is resolved.
	use crate::runner::run_target;
	use runfile_parser::parse_runfile;

	let shell = get_test_shell();
	let project_dir = TempDir::new().unwrap();
	let runfile_dir = TempDir::new().unwrap();

	let marker_name = "declared_var_wd.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};
	let project_path = project_dir.path().display().to_string().replace('\\', "/");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "build": {{
                "commands": ["{create_marker}"],
                "vars": {{ "PROJECT_DIR": "{project_path}" }},
                "workingDirectory": "{{{{ VAR.PROJECT_DIR }}}}"
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::default();
	let result = run_target("build", &runfile, &shell, &args, runfile_dir.path()).unwrap();
	assert!(result.final_status.success());
	assert!(
		project_dir.path().join(marker_name).exists(),
		"command should run in the dir from the declared var (VAR.PROJECT_DIR)"
	);
}

#[test]
fn working_directory_on_globals_resolves_env_from_globals() {
	// The exact reported scenario: BOTH `workingDirectory` and the env var it
	// references live in `globals`. `merge_runfiles` bakes both into the target,
	// and the baked `workingDirectory` must still resolve `{{ ENV.X }}` against
	// the baked env.
	use crate::runner::run_target_with_cwd;
	use runfile_parser::{merge_runfiles, parse_runfile};

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();
	let project_dir = TempDir::new().unwrap();

	let marker_name = "globals_wd_globals_env.txt";
	let create_marker = if shell.kind == ShellKind::Cmd {
		format!("echo done > {marker_name}")
	} else {
		format!("touch {marker_name}")
	};
	let project_path = project_dir.path().display().to_string().replace('\\', "/");

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "globals": {{
            "env": {{ "PROJECT_PATH": "{project_path}" }},
            "workingDirectory": "{{{{ ENV.PROJECT_PATH }}}}"
        }},
        "targets": {{
            "build": {{ "commands": ["{create_marker}"] }}
        }}
    }}"#
	);

	let runfile_path = runfile_dir.path().join("Runfile.json");
	let parsed = parse_runfile(&json).unwrap();
	let merged = merge_runfiles(Some((parsed, runfile_path.clone())), &[], runfile_dir.path()).unwrap();

	let args = RunArgs::default();
	let result = run_target_with_cwd(
		"build",
		&merged.runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&merged.source_dirs,
		&merged.source_files(),
		false,
		false,
		None,
	)
	.unwrap();
	assert!(result.final_status.success());
	assert!(
		project_dir.path().join(marker_name).exists(),
		"command should run in the dir from globals workingDirectory + globals env"
	);
}

#[test]
fn force_shell_accepts_substitution() {
	// `forceShell` may be a substituted string. We only verify parsing +
	// resolution succeeds — a value of `{{ RUN.shell }}` resolves to the shell
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
                "forceShell": "{{ RUN.shell }}"
            }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let res = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(res.final_status.success());
}

#[test]
fn add_to_path_resolves_relative_to_runfile_parent_not_working_directory() {
	// Regression: when a target sets `workingDirectory` to a subdir, relative
	// `addToPath` entries must STILL resolve against the source Runfile's
	// parent dir — not against the resolved workingDirectory. The parser
	// bakes target-level addToPath against `source_dir` for this reason.
	use crate::runner::run_target_with_cwd;
	use runfile_parser::{merge_runfiles, parse_runfile};
	use std::fs;

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();
	let nested = runfile_dir.path().join("subdir");
	fs::create_dir(&nested).unwrap();
	let tool_dir = runfile_dir.path().join("tools");
	fs::create_dir(&tool_dir).unwrap();

	let marker = nested.join("path-marker");
	let marker_escaped = json_escape_path(&marker);
	let write_path = if shell.kind == ShellKind::Cmd {
		format!("echo %PATH%> \\\"{marker_escaped}\\\"")
	} else {
		format!("echo \\\"$PATH\\\" > \\\"{marker_escaped}\\\"")
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": ["{write_path}"],
                "addToPath": ["tools"],
                "workingDirectory": "subdir"
            }}
        }}
    }}"#
	);
	let runfile_path = runfile_dir.path().join("Runfile.json");
	fs::write(&runfile_path, &json).unwrap();

	// Use the merge pipeline so target-level addToPath gets baked against
	// `source_dir` (= runfile_dir) — same code path the CLI uses.
	let parsed = parse_runfile(&json).unwrap();
	let merge_result = merge_runfiles(Some((parsed, runfile_path.clone())), &[], runfile_dir.path()).unwrap();
	let source_files = merge_result.source_files();
	let runfile = merge_result.runfile;
	let source_dirs = merge_result.source_dirs;

	let args = RunArgs::default();
	run_target_with_cwd(
		"t",
		&runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&source_dirs,
		&source_files,
		false,
		false,
		None,
	)
	.unwrap();

	let written = fs::read_to_string(&marker).expect("path marker should exist in subdir/");
	// Normalise backslashes — some shells (MSYS bash on Windows) emit POSIX-form
	// paths in `$PATH`, so we can't compare against the absolute Windows form
	// directly. The behavior we care about is "addToPath was anchored to the
	// runfile parent": `tools` should appear in PATH as a direct child of the
	// runfile parent, not nested under `subdir/`.
	let normalised = written.replace('\\', "/");
	assert!(
		!normalised.contains("subdir/tools"),
		"addToPath entry `tools` was wrongly resolved as a child of workingDirectory `subdir`. PATH was: {:?}",
		written
	);
	assert!(
		normalised.contains("/tools"),
		"addToPath entry `tools` should appear in PATH (as `<runfile_parent>/tools`). PATH was: {:?}",
		written
	);
}

#[test]
fn env_files_resolve_relative_to_runfile_parent_not_working_directory() {
	// Regression: when a target sets `workingDirectory` to a subdir, relative
	// `envFiles` paths must STILL resolve against the source Runfile's parent
	// dir — not against the resolved workingDirectory. Env files are
	// configuration co-located with the Runfile.
	use crate::runner::run_target_with_cwd;
	use runfile_parser::Runfile;
	use std::fs;

	let shell = get_test_shell();
	let runfile_dir = TempDir::new().unwrap();
	let caller_cwd = TempDir::new().unwrap();
	let nested = runfile_dir.path().join("subdir");
	fs::create_dir(&nested).unwrap();

	// .env.production lives next to the Runfile (NOT inside `subdir/`).
	fs::write(runfile_dir.path().join(".env.production"), "MY_TOKEN=from-envfile\n").unwrap();

	let marker = nested.join("token");
	let marker_escaped = json_escape_path(&marker);
	let write_token = if shell.kind == ShellKind::Cmd {
		format!("echo %MY_TOKEN%> \\\"{marker_escaped}\\\"")
	} else {
		format!("echo $MY_TOKEN > \\\"{marker_escaped}\\\"")
	};

	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "t": {{
                "commands": ["{write_token}"],
                "envFiles": [".env.production"],
                "workingDirectory": "subdir"
            }}
        }}
    }}"#
	);
	let runfile: Runfile = serde_json::from_str(&json).unwrap();
	let args = RunArgs::default();

	let runfile_path = runfile_dir.path().join("Runfile.json");
	run_target_with_cwd(
		"t",
		&runfile,
		&shell,
		&args,
		&runfile_path,
		runfile_dir.path(),
		caller_cwd.path(),
		&std::collections::HashMap::new(),
		&std::collections::HashMap::new(),
		false,
		false,
		None,
	)
	.unwrap();

	let written = fs::read_to_string(&marker).expect("token marker should exist in subdir/");
	assert!(
		written.contains("from-envfile"),
		"envFile from runfile parent should have loaded MY_TOKEN even when workingDirectory is `subdir`. got: {:?}",
		written
	);
}

// ── error() control-flow function ─────────────────────────────────

#[test]
fn error_fails_command_and_runs_when_failure_and_always() {
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
                    "{{{{ error('boom') }}}}",
                    "echo step2 >> \"{log_escaped}\"",
                    {{ "when": "failure", "commands": ["echo onfail >> \"{log_escaped}\""] }},
                    {{ "when": "always", "commands": ["echo cleanup >> \"{log_escaped}\""] }}
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	// The target failed (error() marked the command failed).
	assert!(!result.final_status.success(), "error() should fail the target");

	let lines: Vec<String> = std::fs::read_to_string(&log)
		.unwrap_or_default()
		.lines()
		.map(|l| l.trim().to_string())
		.filter(|l| !l.is_empty())
		.collect();
	assert!(lines.contains(&"step1".to_string()), "step1 should run: {lines:?}");
	assert!(
		!lines.contains(&"step2".to_string()),
		"step2 (default when:success) should be skipped after error(): {lines:?}"
	);
	assert!(
		lines.contains(&"onfail".to_string()),
		"when:failure should run: {lines:?}"
	);
	assert!(
		lines.contains(&"cleanup".to_string()),
		"when:always should run: {lines:?}"
	);
}

#[test]
fn error_swallowed_by_ignore_errors() {
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
                "ignoreErrors": true,
                "commands": [
                    "{{{{ error('non-fatal') }}}}",
                    "echo after >> \"{log_escaped}\""
                ]
            }}
        }}
    }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(result.final_status.success(), "ignoreErrors should swallow error()");
	let log_contents = std::fs::read_to_string(&log).unwrap_or_default();
	assert!(
		log_contents.contains("after"),
		"subsequent step should run when error() is ignored: {log_contents:?}"
	);
}
