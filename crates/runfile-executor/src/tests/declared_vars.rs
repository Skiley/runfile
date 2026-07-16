use super::*;
use crate::runner::{RunError, run_target};
use runfile_parser::parse_runfile;

/// Read a log file written by the spawned shells and return its trimmed,
/// non-empty lines.
fn read_lines(path: &std::path::Path) -> Vec<String> {
	std::fs::read_to_string(path)
		.unwrap_or_default()
		.lines()
		.map(|l| l.trim().to_string())
		.filter(|l| !l.is_empty())
		.collect()
}

#[test]
fn vars_resolve_in_commands_and_see_env() {
	// A declared var that references the target's own `env` block value
	// (vars are evaluated after env is built) plus a literal var.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("out.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "t": {{
                    "env": {{ "FOO": "envval" }},
                    "vars": {{ "lit": "hello", "fromenv": "{{{{ ENV.FOO }}}}" }},
                    "commands": [
                        "echo {{{{ VAR.lit }}}} >> \"{log_escaped}\"",
                        "echo {{{{ VAR.fromenv }}}} >> \"{log_escaped}\""
                    ]
                }}
            }}
        }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(result.final_status.success());
	assert_eq!(read_lines(&log), vec!["hello", "envval"]);
}

#[test]
fn vars_resolve_from_args() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("out.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "t": {{
                    "vars": {{ "x": "{{{{ ARG.abc }}}}" }},
                    "commands": ["echo {{{{ VAR.x }}}} >> \"{log_escaped}\""]
                }}
            }}
        }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let args = RunArgs::parse(&["--abc".into(), "supplied".into()]);
	let result = run_target("t", &runfile, &shell, &args, dir.path()).unwrap();
	assert!(result.final_status.success());
	assert_eq!(read_lines(&log), vec!["supplied"]);
}

#[test]
fn vars_missing_no_default_errors() {
	// A var value referencing an unsupplied arg with no default must error,
	// just like the same reference anywhere else.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();

	let json = r#"{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "t": {
                    "vars": { "x": "{{ ARG.missing }}" },
                    "commands": ["echo {{ VAR.x }}"]
                }
            }
        }"#;

	let runfile = parse_runfile(json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path());
	assert!(result.is_err(), "missing arg with no default should error");
}

#[test]
fn vars_with_default_falls_back() {
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("out.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "t": {{
                    "vars": {{ "x": "{{{{ ARG.missing ? 'fallback' }}}}" }},
                    "commands": ["echo {{{{ VAR.x }}}} >> \"{log_escaped}\""]
                }}
            }}
        }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(result.final_status.success());
	assert_eq!(read_lines(&log), vec!["fallback"]);
}

#[test]
fn vars_scoped_per_target_no_leak() {
	// Parent declares X=P, calls @child (X=C), then reads X again.
	// Child sees its own X; parent's X is intact after the child returns.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("out.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "parent": {{
                    "vars": {{ "X": "P" }},
                    "commands": [
                        "echo parent-before-{{{{ VAR.X }}}} >> \"{log_escaped}\"",
                        "@child",
                        "echo parent-after-{{{{ VAR.X }}}} >> \"{log_escaped}\""
                    ]
                }},
                "child": {{
                    "vars": {{ "X": "C" }},
                    "commands": ["echo child-{{{{ VAR.X }}}} >> \"{log_escaped}\""]
                }}
            }}
        }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("parent", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(result.final_status.success());
	assert_eq!(read_lines(&log), vec!["parent-before-P", "child-C", "parent-after-P"]);
}

#[test]
fn parent_vars_inherited_by_child_without_own() {
	// A child that declares no X still sees the parent's declared X via the
	// shared VARS map — mirroring how `env` parent values reach a dep.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("out.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "parent": {{
                    "vars": {{ "X": "fromparent" }},
                    "commands": ["@child"]
                }},
                "child": {{
                    "commands": ["echo {{{{ VAR.X }}}} >> \"{log_escaped}\""]
                }}
            }}
        }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("parent", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(result.final_status.success());
	assert_eq!(read_lines(&log), vec!["fromparent"]);
}

#[test]
fn declared_var_overridden_by_runtime_define() {
	// A `define(...)` during command execution shadows the declared var for
	// the rest of the target — last writer wins within the target.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let log = dir.path().join("out.txt");
	let log_escaped = json_escape_path(&log);

	let json = format!(
		r#"{{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {{
                "t": {{
                    "vars": {{ "X": "declared" }},
                    "commands": [
                        "echo {{{{ VAR.X }}}} >> \"{log_escaped}\"",
                        "{{{{ define(X, 'redefined') }}}}",
                        "echo {{{{ VAR.X }}}} >> \"{log_escaped}\""
                    ]
                }}
            }}
        }}"#
	);

	let runfile = parse_runfile(&json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path()).unwrap();
	assert!(result.final_status.success());
	assert_eq!(read_lines(&log), vec!["declared", "redefined"]);
}

#[test]
fn invalid_var_key_rejected_at_parse() {
	let json = r#"{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": {
                "t": {
                    "vars": { "bad key": "x" },
                    "commands": ["echo hi"]
                }
            }
        }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().to_lowercase().contains("variable name"));
}

#[test]
fn missing_var_error_unaffected_by_feature() {
	// Sanity: a target with no declared vars referencing an undefined VARS
	// still errors with the usual missing-var message.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let json = r#"{
            "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
            "targets": { "t": { "commands": ["echo {{ VAR.nope }}"] } }
        }"#;
	let runfile = parse_runfile(json).unwrap();
	let result = run_target("t", &runfile, &shell, &RunArgs::default(), dir.path());
	assert!(matches!(result, Err(RunError::Execute(_)) | Err(RunError::Substitution(_))) || result.is_err());
}
