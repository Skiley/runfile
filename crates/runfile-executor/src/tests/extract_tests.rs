use super::*;

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
	// dep's `{{ ARG.env }}` substitution resolves.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": { "commands": ["echo deploying to {{ ARG.env }}"] },
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
	// loop dispatching `@{{ VAR.ns }}:something` used to print nothing in
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
                    { "for": "ns", "in": "namespaces", "do": "@{{ VAR.ns }}:dev" }
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
fn extract_for_in_literal_array_expands_per_iteration() {
	// `for in: [...]` already expanded at extract time; this is a regression
	// guard so the for-block refactor that added glob expansion didn't break
	// the literal-array path.
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build-all": {
                "commands": [
                    { "for": "tier", "in": ["api", "web"], "do": "echo build {{ VAR.tier }}" }
                ]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("build-all", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &ShellKind::Bash);

	assert_eq!(lines, vec!["echo build api".to_string(), "echo build web".to_string()]);
}

#[test]
fn extract_for_glob_walks_filesystem() {
	// `for glob:` is read-only and side-effect-free, so dry-run expands it
	// against the actual working directory — the user gets the same command
	// list a real run would produce, with concrete paths bound to the loop
	// variable on each iteration.
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;
	use std::fs;

	let dir = TempDir::new().unwrap();
	// `expand_glob` walks recursively; pick names that are deterministic
	// across platforms and won't collide with anything else in the temp
	// dir.
	fs::write(dir.path().join("alpha.txt"), b"a").unwrap();
	fs::write(dir.path().join("beta.txt"), b"b").unwrap();
	fs::write(dir.path().join("ignore.md"), b"c").unwrap();

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "lint": {
                "commands": [
                    { "for": "f", "glob": "*.txt", "do": "lint {{ VAR.f }}" }
                ]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();

	let commands = extract_target("lint", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &ShellKind::Bash);

	// `expand_glob` returns matches sorted alphabetically with forward
	// slashes — `extract_target` plumbs `working_dir` into the walker so
	// matches resolve against the same root the runner would use.
	assert_eq!(lines, vec!["lint alpha.txt".to_string(), "lint beta.txt".to_string()]);
}

#[test]
fn extract_for_glob_with_no_matches_emits_no_commands() {
	// Empty match set → body emits zero commands. Mirrors runtime behaviour
	// (an empty iteration list runs the body zero times).
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "lint": {
                "commands": [
                    { "for": "f", "glob": "*.nonesuch", "do": "lint {{ VAR.f }}" }
                ]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("lint", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &ShellKind::Bash);

	assert!(lines.is_empty(), "no matches → no commands; got {:?}", lines);
}

#[test]
fn extract_for_shell_emits_placeholder_without_running_iterator() {
	// `for shell:` is deliberately NOT executed during extract — running
	// arbitrary shell commands during a read-only preview would have side
	// effects (process spawn, possibly slow I/O, possibly stateful). The
	// iterator command here exits non-zero on purpose: if extract ran it,
	// `expand_shell` would surface the failure as a `ControlFlowError` and
	// the unwrap below would panic. Seeing the placeholder in the output
	// proves extract bypassed the iterator.
	use crate::extract::{extract_target, format_extracted_commands};
	use runfile_parser::parse_runfile;

	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "process-files": {
                "commands": [
                    { "for": "line", "shell": "exit 1", "do": "echo {{ VAR.line }}" }
                ]
            }
        }
    }"#;

	let runfile = parse_runfile(json).unwrap();
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let commands = extract_target("process-files", &runfile, &args, dir.path()).unwrap();
	let lines = format_extracted_commands(&commands, &ShellKind::Bash);

	// Body emits exactly once with the loop var bound to `<line>`.
	assert_eq!(lines, vec!["echo <line>".to_string()]);
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
                        "if": "{{ RUN.os == 'windows' }}",
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
            "deploy": { "commands": ["echo deploying to {{ ARG.env }}"] }
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
	// e.g. `RUN_TESTS_WITH_SIDE_EFFECTS='{{ FLAG.side-effects }}'`. Values
	// must be substituted just like commands are.
	//
	// `CARGO_PKG_NAME` is always present during `cargo test`, so we lean on
	// it to exercise `{{ ENV.* }}` resolution without mutating process env
	// (which races with other parallel tests).
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["./gradlew test"],
                "env": {
                    "RUN_TESTS_WITH_SIDE_EFFECTS": "{{ FLAG.side-effects }}",
                    "TARGET_ENV": "{{ ARG.env }}",
                    "PKG": "{{ ENV.CARGO_PKG_NAME }}"
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
		!line.contains("{{ FLAGS") && !line.contains("{{ ARGS") && !line.contains("{{ ENV"),
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
            "deploy": { "commands": ["echo deploying to {{ ARG.env }}"] }
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

#[test]
fn extract_working_directory_resolves_env_from_globals() {
	// Regression (dry-run): `{{ ENV.X }}` inside `workingDirectory` must resolve
	// against the target's own env (globals' `env` is baked into each target
	// during merge), not just the parent env. Previously `--dry-run` errored with
	// "environment variable not set" before printing any command.
	use crate::extract::extract_target;
	use runfile_parser::{merge_runfiles, parse_runfile};

	let dir = TempDir::new().unwrap();
	let runfile_path = dir.path().join("Runfile.json");
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "globals": {
            "env": { "PROJECT_PATH": "/some/project" }
        },
        "targets": {
            "build": {
                "commands": ["echo hi"],
                "workingDirectory": "{{ ENV.PROJECT_PATH }}"
            }
        }
    }"#;

	// `globals.env` is baked into each target's `env` during merge, so go through
	// `merge_runfiles` rather than a plain parse.
	let parsed = parse_runfile(json).unwrap();
	let merged = merge_runfiles(Some((parsed, runfile_path)), &[], dir.path()).unwrap();
	let args = RunArgs::default();

	let commands = extract_target("build", &merged.runfile, &args, dir.path()).unwrap();
	assert_eq!(commands.len(), 1);
	assert_eq!(commands[0].command, "echo hi");
}
