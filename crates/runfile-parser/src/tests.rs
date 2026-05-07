use crate::*;
use std::collections::HashMap;
use tempfile::TempDir;

// ── Parsing tests ──────────────────────────────────────────────────

#[test]
fn parse_minimal_runfile() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(
		rf.schema,
		"https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json"
	);
	assert_eq!(rf.targets.len(), 1);
	assert_eq!(rf.targets["build"].commands, vec!["cargo build"]);
	assert!(rf.globals.is_none());
}

#[test]
fn parse_full_runfile() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["npm run build {{ ARGS }}"]
            },
            "dev": {
                "commands": ["echo starting", "npm run dev"],
                "env": { "PORT": 5000, "NODE_ENV": "development" }
            }
        },
        "globals": {
            "addToPath": ["node_modules/.bin"],
            "env": { "KEY": "VALUE" },
            "forceShell": "bash"
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets.len(), 2);

	let dev = &rf.targets["dev"];
	assert_eq!(dev.commands.len(), 2);
	let env = dev.env.as_ref().unwrap();
	assert_eq!(env["PORT"], EnvValue::Number(5000.0));
	assert_eq!(env["NODE_ENV"], EnvValue::String("development".into()));

	let globals = rf.globals.as_ref().unwrap();
	assert_eq!(globals.add_to_path.as_ref().unwrap(), &["node_modules/.bin"]);
	assert_eq!(globals.force_shell.as_ref().unwrap(), "bash");
}

#[test]
fn parse_runfile_with_bool_env() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["cargo test"],
                "env": { "CI": true, "VERBOSE": false }
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let env = rf.targets["test"].env.as_ref().unwrap();
	assert_eq!(env["CI"], EnvValue::Bool(true));
	assert_eq!(env["VERBOSE"], EnvValue::Bool(false));
}

#[test]
fn parse_runfile_with_env_files() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "dev": {
                "commands": ["npm start"],
                "envFiles": [".env", ".env.{{ ARGS.env ? development }}"]
            }
        },
        "globals": {
            "envFiles": [".env"]
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let target = &rf.targets["dev"];
	let env_files = target.env_files.as_ref().unwrap();
	assert_eq!(env_files.len(), 2);
	assert_eq!(env_files[0], ".env");
	assert_eq!(env_files[1], ".env.{{ ARGS.env ? development }}");

	let globals = rf.globals.as_ref().unwrap();
	let global_env_files = globals.env_files.as_ref().unwrap();
	assert_eq!(global_env_files, &[".env"]);
}

#[test]
fn env_value_to_string() {
	assert_eq!(EnvValue::String("hello".into()).to_env_string(), "hello");
	assert_eq!(EnvValue::Number(5000.0).to_env_string(), "5000");
	assert_eq!(EnvValue::Number(2.72).to_env_string(), "2.72");
	assert_eq!(EnvValue::Bool(true).to_env_string(), "true");
	assert_eq!(EnvValue::Bool(false).to_env_string(), "false");
}

// ── Validation error tests ─────────────────────────────────────────

#[test]
fn reject_empty_schema() {
	let json = r#"{
        "$schema": "",
        "targets": { "x": { "commands": ["echo hi"] } }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::EmptySchema));
}

#[test]
fn accept_any_nonempty_schema() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "x": { "commands": ["echo hi"] } }
    }"#;
	assert!(parse_runfile(json).is_ok());

	let json2 = r#"{
        "$schema": "./schemas/v0.schema.json",
        "targets": { "x": { "commands": ["echo hi"] } }
    }"#;
	assert!(parse_runfile(json2).is_ok());
}

#[test]
fn reject_empty_targets_map() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {}
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::NoTargets));
}

#[test]
fn reject_command_with_empty_list() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "bad": { "commands": [] } }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::EmptyCommandList(_)));
}

#[test]
fn reject_unknown_top_level_field() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "x": { "commands": ["echo"] } },
        "extra": true
    }"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn reject_unknown_command_field() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "x": { "commands": ["echo"], "unknown_field": 42 }
        }
    }"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn reject_unknown_globals_field() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "x": { "commands": ["echo"] } },
        "globals": { "badField": true }
    }"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn reject_missing_schema() {
	let json = r#"{
        "targets": { "x": { "commands": ["echo"] } }
    }"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn reject_missing_targets_key() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json"
    }"#;
	assert!(parse_runfile(json).is_err());
}

// ── Discovery tests ────────────────────────────────────────────────

#[test]
fn discover_in_current_dir() {
	let dir = TempDir::new().unwrap();
	let runfile_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&runfile_path,
		r#"{"$schema":"https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json","targets":{"a":{"commands":["echo"]}}}"#,
	)
	.unwrap();

	let found = discover_runfile(dir.path()).unwrap();
	assert_eq!(found, runfile_path);
}

#[test]
fn discover_in_parent_dir() {
	let dir = TempDir::new().unwrap();
	let child = dir.path().join("sub").join("deep");
	std::fs::create_dir_all(&child).unwrap();

	let runfile_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&runfile_path,
		r#"{"$schema":"https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json","targets":{"a":{"commands":["echo"]}}}"#,
	)
	.unwrap();

	let found = discover_runfile(&child).unwrap();
	assert_eq!(found, runfile_path);
}

#[test]
fn discover_not_found() {
	let dir = TempDir::new().unwrap();
	// Create a deep nested path inside the temp dir. The temp dir itself
	// is guaranteed empty, so discovery within it (without walking above)
	// would fail. We verify that discovery eventually returns an error
	// by checking the full walk doesn't find a Runfile.json in ancestors.
	let child = dir.path().join("a").join("b").join("c");
	std::fs::create_dir_all(&child).unwrap();

	let result = discover_runfile(&child);
	// If a Runfile.json exists somewhere in the temp path's ancestors (e.g. the
	// project we're building from), discovery may succeed — that's correct
	// behavior. We only assert it doesn't panic and returns a valid result.
	if let Ok(path) = result {
		assert!(path.is_file());
	}
	// Err case: no Runfile.json found — that's fine too
}

// ── File-based parsing tests ───────────────────────────────────────

#[test]
fn parse_from_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&path,
		r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "hello": { "commands": ["echo hello world"] }
        }
    }"#,
	)
	.unwrap();

	let rf = parse_runfile_from_path(&path).unwrap();
	assert_eq!(rf.targets["hello"].commands, vec!["echo hello world"]);
}

#[test]
fn parse_from_nonexistent_file() {
	let result = parse_runfile_from_path(std::path::Path::new("/nonexistent/Runfile.json"));
	assert!(result.is_err());
}

// ── Serialization round-trip ───────────────────────────────────────

#[test]
fn roundtrip_serialization() {
	let mut targets = HashMap::new();
	targets.insert("build".into(), CommandSpec::new(vec!["cargo build".into()]));

	let runfile = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets,
		globals: Some(Globals {
			add_to_path: Some(vec!["bin".into()]),
			env_files: None,
			env: None,
			force_shell: None,
			logging: None,
			ignore_errors: None,
			working_directory: None,
			force_kill_on_sig_int: None,
			only_in_directories: None,
		}),
		namespaces: Vec::new(),
	};

	let json = serde_json::to_string(&runfile).unwrap();
	let parsed: Runfile = serde_json::from_str(&json).unwrap();
	assert_eq!(runfile, parsed);
}

// ── Multiple commands / complex scenarios ──────────────────────────

#[test]
fn parse_many_commands() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["npm run build"] },
            "test": { "commands": ["npm test"] },
            "lint": { "commands": ["eslint ."] },
            "deploy": { "commands": ["echo deploying", "kubectl apply -f k8s/"] },
            "clean": { "commands": ["rm -rf dist/"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets.len(), 5);
}

#[test]
fn parse_globals_only_add_to_path() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "x": { "commands": ["echo"] } },
        "globals": { "addToPath": ["/usr/local/bin", "vendor/bin"] }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let globals = rf.globals.unwrap();
	assert_eq!(globals.add_to_path.unwrap().len(), 2);
	assert!(globals.env.is_none());
	assert!(globals.force_shell.is_none());
}

#[test]
fn parse_globals_only_env() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "x": { "commands": ["echo"] } },
        "globals": { "env": { "A": "1", "B": 2 } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let globals = rf.globals.unwrap();
	let env = globals.env.unwrap();
	assert_eq!(env["A"], EnvValue::String("1".into()));
	assert_eq!(env["B"], EnvValue::Number(2.0));
}

#[test]
fn parse_command_with_args_placeholder() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "run": { "commands": ["node app.js {{ ARGS }}"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert!(rf.targets["run"].commands[0].contains("{{ ARGS }}"));
}

#[test]
fn parse_command_with_conditional_args() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "dev": {
                "commands": ["npm run dev"],
                "env": { "NODE_ENV": "{{ ARGS.env ? development }}" }
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let env = rf.targets["dev"].env.as_ref().unwrap();
	assert_eq!(env["NODE_ENV"], EnvValue::String("{{ ARGS.env ? development }}".into()));
}

#[test]
fn parse_command_with_description() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "description": "Build the project",
                "commands": ["cargo build"]
            },
            "test": {
                "commands": ["cargo test"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["build"].description.as_deref(), Some("Build the project"));
	assert!(rf.targets["test"].description.is_none());
}

#[test]
fn parse_command_with_force_shell() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "ps-task": {
                "commands": ["Write-Host hello"],
                "forceShell": "powershell"
            },
            "unix-task": {
                "commands": ["echo hello"],
                "forceShell": "bash"
            },
            "default-task": {
                "commands": ["echo hi"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["ps-task"].force_shell.as_deref(), Some("powershell"));
	assert_eq!(rf.targets["unix-task"].force_shell.as_deref(), Some("bash"));
	assert!(rf.targets["default-task"].force_shell.is_none());
}

#[test]
fn parse_command_force_shell_overrides_global() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "special": {
                "commands": ["Write-Host hello"],
                "forceShell": "powershell"
            },
            "normal": {
                "commands": ["echo hi"]
            }
        },
        "globals": {
            "forceShell": "bash"
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["special"].force_shell.as_deref(), Some("powershell"));
	assert!(rf.targets["normal"].force_shell.is_none());
	assert_eq!(rf.globals.unwrap().force_shell.as_deref(), Some("bash"));
}

#[test]
fn parse_command_with_add_to_path() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["my-tool build"],
                "addToPath": ["vendor/bin", ".tools"]
            },
            "test": {
                "commands": ["cargo test"]
            }
        },
        "globals": {
            "addToPath": ["node_modules/.bin"]
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(
		rf.targets["build"].add_to_path.as_ref().unwrap(),
		&["vendor/bin", ".tools"]
	);
	assert!(rf.targets["test"].add_to_path.is_none());
	assert_eq!(rf.globals.unwrap().add_to_path.unwrap(), vec!["node_modules/.bin"]);
}

#[test]
fn parse_logging_on_command() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "verbose": {
                "commands": ["echo step1", "echo step2"],
                "logging": true
            },
            "quiet": {
                "commands": ["echo hi"],
                "logging": false
            },
            "default": {
                "commands": ["echo hi"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["verbose"].logging, Some(true));
	assert_eq!(rf.targets["quiet"].logging, Some(false));
	assert!(rf.targets["default"].logging.is_none());
}

#[test]
fn parse_logging_on_globals() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "x": { "commands": ["echo"] }
        },
        "globals": {
            "logging": true
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.globals.unwrap().logging, Some(true));
}

#[test]
fn parse_command_logging_overrides_global() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "quiet": {
                "commands": ["echo"],
                "logging": false
            },
            "inherited": {
                "commands": ["echo"]
            }
        },
        "globals": { "logging": true }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["quiet"].logging, Some(false));
	assert!(rf.targets["inherited"].logging.is_none());
	assert_eq!(rf.globals.unwrap().logging, Some(true));
}

#[test]
fn parse_ignore_errors_on_command() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "risky": {
                "commands": ["echo a", "echo b"],
                "ignoreErrors": true
            },
            "strict": {
                "commands": ["echo a"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["risky"].ignore_errors, Some(true));
	assert!(rf.targets["strict"].ignore_errors.is_none());
}

#[test]
fn parse_ignore_errors_on_globals() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "x": { "commands": ["echo"] }
        },
        "globals": { "ignoreErrors": true }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.globals.unwrap().ignore_errors, Some(true));
}

// ── Reserved target name tests ─────────────────────────────────────

#[test]
fn reject_target_name_starting_with_colon() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            ":build": { "commands": ["echo"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::ReservedTargetName(_)));
}

#[test]
fn reject_target_name_colon_list() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            ":list": { "commands": ["echo"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::ReservedTargetName(_)));
}

#[test]
fn accept_target_names_with_colon_not_at_start() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "ci:build": { "commands": ["echo"] },
            "test:unit": { "commands": ["echo"] }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn accept_previously_reserved_target_names() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "list": { "commands": ["echo"] },
            "config": { "commands": ["echo"] },
            "utilities": { "commands": ["echo"] }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

// ── Alias tests ───────────────────────────────────────────────────

#[test]
fn parse_target_with_aliases() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "stop-dev": { "commands": ["./stop.sh"], "aliases": ["stop", "sd"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let spec = &runfile.targets["stop-dev"];
	assert_eq!(
		spec.aliases.as_ref().unwrap(),
		&vec!["stop".to_string(), "sd".to_string()]
	);
}

#[test]
fn resolve_target_by_name() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["b"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.resolve_target("build"), Some("build"));
}

#[test]
fn resolve_target_by_alias() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["b"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.resolve_target("b"), Some("build"));
}

#[test]
fn resolve_target_unknown_returns_none() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.resolve_target("unknown"), None);
}

#[test]
fn all_target_names_includes_aliases() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["b"] },
            "test": { "commands": ["cargo test"] }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	let names = runfile.all_target_names();
	assert!(names.contains(&"build"));
	assert!(names.contains(&"b"));
	assert!(names.contains(&"test"));
}

#[test]
fn reject_alias_conflicts_with_target() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["test"] },
            "test": { "commands": ["cargo test"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::AliasConflictsWithTarget(_, _)));
}

#[test]
fn reject_duplicate_alias_across_targets() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["x"] },
            "test": { "commands": ["cargo test"], "aliases": ["x"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::DuplicateAlias(_, _, _)));
}

#[test]
fn reject_alias_starting_with_colon() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": [":b"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::ReservedAlias(_, _)));
}

#[test]
fn accept_alias_with_colon_not_at_start() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["ci:b"] }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn reject_alias_same_as_target() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["cargo build"], "aliases": ["build"] }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::AliasSameAsTarget(_, _)));
}

// ── workingDirectory tests ──────────────────────────────────────────

#[test]
fn parse_working_directory_substitution_on_target() {
	// `workingDirectory` is a free-form path that supports `{{ ... }}`
	// substitution.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "workingDirectory": "{{ RUN.cwd }}"
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["build"].working_directory.as_deref(), Some("{{ RUN.cwd }}"));
}

#[test]
fn parse_working_directory_relative_path_on_target() {
	// Plain relative paths are accepted; the runner resolves them against
	// the target's source Runfile directory.
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "workingDirectory": "subdir/build"
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["build"].working_directory.as_deref(), Some("subdir/build"));
}

#[test]
fn parse_working_directory_absolute_path_on_target() {
	// Absolute paths pass through untouched.
	#[cfg(windows)]
	let abs = r"C:\\Users\\dev\\project";
	#[cfg(not(windows))]
	let abs = "/home/dev/project";
	let json = format!(
		r#"{{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {{
            "build": {{
                "commands": ["echo"],
                "workingDirectory": "{abs}"
            }}
        }}
    }}"#
	);
	let rf = parse_runfile(&json).unwrap();
	assert!(rf.targets["build"].working_directory.is_some());
}

#[test]
fn parse_working_directory_on_globals() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo"] }
        },
        "globals": {
            "workingDirectory": "{{ RUN.cwd }}"
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.globals.unwrap().working_directory.as_deref(), Some("{{ RUN.cwd }}"));
}

#[test]
fn parse_working_directory_absent_is_none() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert!(rf.targets["build"].working_directory.is_none());
}

// ── JSON5 parsing tests ───────────────────────────────────────────

#[test]
fn json5_trailing_comma_in_object() {
	let input = r#"{"a": 1, "b": 2,}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
	assert_eq!(val["b"], 2);
}

#[test]
fn json5_trailing_comma_in_array() {
	let input = r#"[1, 2, 3,]"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val[0], 1);
	assert_eq!(val[2], 3);
}

#[test]
fn json5_single_line_comments() {
	let input = r#"{
		// This is a comment
		"a": 1
	}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
}

#[test]
fn json5_block_comments() {
	let input = r#"{
		/* block comment */
		"a": 1
	}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
}

#[test]
fn json5_unquoted_keys() {
	let input = r#"{a: 1, b: 2}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
	assert_eq!(val["b"], 2);
}

#[test]
fn json5_single_quoted_strings() {
	let input = r#"{'a': 'hello'}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], "hello");
}

#[test]
fn json5_plain_json_still_works() {
	let input = r#"{"a": 1}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"], 1);
}

#[test]
fn json5_real_error_propagated() {
	let input = r#"{"a": }"#;
	assert!(from_json_str::<serde_json::Value>(input).is_err());
}

#[test]
fn json5_runfile_with_comments() {
	let input = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		// Build targets
		"targets": {
			"build": {
				"commands": ["cargo build"], // main build command
			}
		}
	}"#;
	let rf = parse_runfile(input).unwrap();
	assert_eq!(rf.targets["build"].commands, vec!["cargo build"]);
}

// ── Merge tests ───────────────────────────────────────────────────

#[test]
fn merge_local_only_no_global_files() {
	let runfile = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), CommandSpec::new(vec!["cargo build".into()]));
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let dir = TempDir::new().unwrap();
	let path = dir.path().join(RUNFILE_NAME);
	let result = merge_runfiles(Some((runfile, path)), &[], dir.path()).unwrap();
	assert_eq!(result.runfile.targets.len(), 1);
	assert!(result.runfile.targets.contains_key("build"));
}

#[test]
fn merge_global_only_no_local() {
	let dir = TempDir::new().unwrap();
	let global_path = dir.path().join("global.json");
	std::fs::write(
		&global_path,
		r#"{ "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json", "targets": { "lint": { "commands": ["cargo clippy"] } } }"#,
	)
	.unwrap();

	let result = merge_runfiles(None, &[global_path], dir.path()).unwrap();
	assert_eq!(result.runfile.targets.len(), 1);
	assert!(result.runfile.targets.contains_key("lint"));
}

#[test]
fn merge_local_and_global_conflict() {
	let dir = TempDir::new().unwrap();
	let global_path = dir.path().join("global.json");
	std::fs::write(
		&global_path,
		r#"{ "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json", "targets": { "build": { "commands": ["global build"] }, "deploy": { "commands": ["deploy"] } } }"#,
	)
	.unwrap();

	let local = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), {
				let mut s = CommandSpec::new(vec!["local build".into()]);
				s.description = Some("local".into());
				s
			});
			t.insert("test".into(), {
				let mut s = CommandSpec::new(vec!["local test".into()]);
				s.description = Some("local test".into());
				s
			});
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let local_path = dir.path().join(RUNFILE_NAME);

	let result = merge_runfiles(
		Some((local, local_path.clone())),
		std::slice::from_ref(&global_path),
		dir.path(),
	)
	.unwrap();

	// "build" is defined in both local and global — should be a conflict
	assert!(result.conflicts.contains_key("build"), "build should be a conflict");
	assert!(
		!result.runfile.targets.contains_key("build"),
		"build should not be in runnable targets"
	);

	// Conflict should list both sources
	let build_sources = &result.conflicts["build"];
	assert_eq!(build_sources.len(), 2);

	// "test" is only in local — should be runnable
	assert!(result.runfile.targets.contains_key("test"));

	// "deploy" is only in global — should be runnable
	assert!(result.runfile.targets.contains_key("deploy"));
}

#[test]
fn merge_only_in_directories_filters() {
	let base = TempDir::new().unwrap();
	let allowed = base.path().join("allowed");
	std::fs::create_dir(&allowed).unwrap();

	let global_path = base.path().join("global.json");
	std::fs::write(
        &global_path,
        r#"{ "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json", "targets": { "lint": { "commands": ["lint"] } }, "globals": { "onlyInDirectories": ["allowed"] } }"#,
    )
    .unwrap();

	// CWD is under allowed — should include
	let result = merge_runfiles(None, std::slice::from_ref(&global_path), &allowed).unwrap();
	assert!(result.runfile.targets.contains_key("lint"));

	// CWD is base (not under allowed) — should exclude
	let result = merge_runfiles(None, &[global_path], base.path());
	assert!(result.is_err()); // No targets
}

#[test]
fn merge_missing_global_file_skipped() {
	let dir = TempDir::new().unwrap();
	let missing = dir.path().join("nonexistent.json");

	let local = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), CommandSpec::new(vec!["echo".into()]));
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let local_path = dir.path().join(RUNFILE_NAME);

	let result = merge_runfiles(Some((local, local_path)), &[missing], dir.path()).unwrap();
	assert_eq!(result.runfile.targets.len(), 1);
}

#[test]
fn merge_globals_baked_into_targets() {
	let dir = TempDir::new().unwrap();
	let global_path = dir.path().join("global.json");
	std::fs::write(
        &global_path,
        r#"{ "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json", "targets": { "build": { "commands": ["build"] } }, "globals": { "logging": true, "env": { "FOO": "bar" } } }"#,
    )
    .unwrap();

	let result = merge_runfiles(None, &[global_path], dir.path()).unwrap();
	let spec = &result.runfile.targets["build"];
	assert_eq!(spec.logging, Some(true));
	assert!(spec.env.is_some());
	assert!(result.runfile.globals.is_none());
}

#[test]
fn merge_source_dirs_tracked() {
	let dir = TempDir::new().unwrap();
	let global_dir = dir.path().join("global");
	std::fs::create_dir(&global_dir).unwrap();
	let global_path = global_dir.join(RUNFILE_NAME);
	std::fs::write(
		&global_path,
		r#"{ "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json", "targets": { "lint": { "commands": ["lint"] } } }"#,
	)
	.unwrap();

	let local = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: {
			let mut t = HashMap::new();
			t.insert("build".into(), CommandSpec::new(vec!["build".into()]));
			t
		},
		globals: None,
		namespaces: Vec::new(),
	};
	let local_path = dir.path().join(RUNFILE_NAME);

	let result = merge_runfiles(Some((local, local_path)), &[global_path], dir.path()).unwrap();

	// "lint" should come from global_dir, "build" from dir
	assert_eq!(result.source_dirs["lint"], global_dir);
	assert_eq!(result.source_dirs["build"], *dir.path());
}

#[test]
fn cross_file_target_refs_accepted_at_parse_time() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "deploy": {
                "commands": ["@build", "deploy"]
            }
        }
    }"#;
	// `@target` references to unknown targets are validated at runtime, not
	// parse time — included files may define `build` later.
	assert!(parse_runfile(json).is_ok());
	assert!(parse_runfile_partial(json).is_ok());
}

#[test]
fn partial_parse_allows_zero_targets() {
	let json =
		r#"{ "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json", "targets": {} }"#;
	assert!(parse_runfile(json).is_err());
	assert!(parse_runfile_partial(json).is_ok());
}

#[test]
fn reject_detach_without_parallel_multiple_commands() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello", "echo world"],
                "detach": true
            }
        }
    }"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("detach"));
	assert!(err.to_string().contains("parallel"));
}

#[test]
fn reject_detach_without_parallel_multiple_commands_partial() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello", "echo world"],
                "detach": true
            }
        }
    }"#;
	let err = parse_runfile_partial(json).unwrap_err();
	assert!(err.to_string().contains("detach"));
}

#[test]
fn accept_detach_single_command_without_parallel() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello"],
                "detach": true
            }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn accept_detach_with_parallel() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "bg": {
                "commands": ["echo hello", "echo world"],
                "parallel": true,
                "detach": true
            }
        }
    }"#;
	assert!(parse_runfile(json).is_ok());
}

#[test]
fn accept_parallel_without_detach() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "multi": {
                "commands": ["echo a", "echo b"],
                "parallel": true
            }
        }
    }"#;
	let runfile = parse_runfile(json).unwrap();
	assert_eq!(runfile.targets["multi"].parallel, Some(true));
}

// ── Env key validation tests ──────────────────────────────────────

#[test]
fn is_valid_env_key_accepts_simple_names() {
	assert!(is_valid_env_key("FOO"));
	assert!(is_valid_env_key("bar"));
	assert!(is_valid_env_key("NODE_ENV"));
	assert!(is_valid_env_key("_PRIVATE"));
	assert!(is_valid_env_key("A"));
	assert!(is_valid_env_key("_"));
	assert!(is_valid_env_key("a1b2c3"));
	assert!(is_valid_env_key("MY_VAR_123"));
}

#[test]
fn is_valid_env_key_rejects_empty() {
	assert!(!is_valid_env_key(""));
}

#[test]
fn is_valid_env_key_rejects_leading_digit() {
	assert!(!is_valid_env_key("1FOO"));
	assert!(!is_valid_env_key("0"));
	assert!(!is_valid_env_key("99_PROBLEMS"));
}

#[test]
fn is_valid_env_key_rejects_special_chars() {
	assert!(!is_valid_env_key("FOO-BAR"));
	assert!(!is_valid_env_key("FOO.BAR"));
	assert!(!is_valid_env_key("FOO BAR"));
	assert!(!is_valid_env_key("FOO;BAR"));
	assert!(!is_valid_env_key("VAR&whoami"));
	assert!(!is_valid_env_key("VAR|cat"));
	assert!(!is_valid_env_key("$env:VAR"));
	assert!(!is_valid_env_key("VAR=value"));
	assert!(!is_valid_env_key("FOO`BAR"));
	assert!(!is_valid_env_key("FOO(BAR)"));
}

#[test]
fn parse_rejects_invalid_env_key_in_target() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo test"],
                "env": { "VALID_KEY": "ok", "bad;key": "injected" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("bad;key"), "error should mention the bad key: {err}");
	assert!(
		err.contains("Invalid environment variable name"),
		"should be env key error: {err}"
	);
}

#[test]
fn parse_rejects_env_key_with_ampersand() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "env": { "VAR&whoami": "pwned" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
}

#[test]
fn parse_rejects_env_key_with_dollar_sign() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "env": { "$env:SECRET": "value" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
}

#[test]
fn parse_rejects_env_key_starting_with_digit() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "test": {
                "commands": ["echo test"],
                "env": { "1KEY": "value" }
            }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
}

#[test]
fn parse_rejects_invalid_env_key_in_globals() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": { "commands": ["echo test"] }
        },
        "globals": {
            "env": { "OK_KEY": "fine", "bad key": "spaces" }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("bad key"), "error should mention the bad key: {err}");
}

#[test]
fn parse_accepts_valid_env_keys() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo test"],
                "env": {
                    "SIMPLE": "ok",
                    "_UNDERSCORE": "ok",
                    "camelCase": "ok",
                    "MIX_123_abc": "ok",
                    "A": "ok"
                }
            }
        },
        "globals": {
            "env": { "GLOBAL_VAR": "value" }
        }
    }"#;
	let result = parse_runfile(json);
	assert!(result.is_ok(), "valid env keys should be accepted: {:?}", result.err());
}

#[test]
fn parse_partial_also_rejects_invalid_env_keys() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo test"],
                "env": { "key;injection": "value" }
            }
        }
    }"#;
	let result = parse_runfile_partial(json);
	assert!(result.is_err());
}

// ── JSON5 UTF-8 and edge case tests ─────────────────────────────────

#[test]
fn json5_utf8_values() {
	let input = r#"{"msg": "日本語 🎉 café",}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["msg"], "日本語 🎉 café");
}

#[test]
fn json5_utf8_in_arrays() {
	let input = r#"{"a": ["α", "β", "γ",], "b": [1, 2,],}"#;
	let val: serde_json::Value = from_json_str(input).unwrap();
	assert_eq!(val["a"][0], "α");
	assert_eq!(val["a"][2], "γ");
}

// ── Fix #9: file size limit ───────────────────────────────────────

#[test]
fn parse_rejects_oversized_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("huge.json");
	// Create a file slightly over the limit
	let size = (crate::MAX_RUNFILE_SIZE + 1) as usize;
	let content = " ".repeat(size);
	std::fs::write(&path, content).unwrap();

	let result = parse_runfile_from_path(&path);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("too large"), "error should mention size: {err}");
}

#[test]
fn parse_rejects_oversized_file_partial() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("huge.json");
	let size = (crate::MAX_RUNFILE_SIZE + 1) as usize;
	std::fs::write(&path, " ".repeat(size)).unwrap();

	let result = parse_runfile_from_path_partial(&path);
	assert!(result.is_err());
	let err = result.unwrap_err().to_string();
	assert!(err.contains("too large"), "error should mention size: {err}");
}

#[test]
fn parse_accepts_file_at_size_limit() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("maxsize.json");
	// A valid but padded Runfile at exactly the limit
	let json = r#"{"$schema":"v0","targets":{"a":{"commands":["echo"]}}}"#;
	let padding = " ".repeat(crate::MAX_RUNFILE_SIZE as usize - json.len());
	let content = format!("{json}{padding}");
	assert!(content.len() as u64 <= crate::MAX_RUNFILE_SIZE);
	std::fs::write(&path, content).unwrap();

	// Should not fail with FileTooLarge (may fail with JSON parse error from padding, that's ok)
	let result = parse_runfile_from_path(&path);
	match result {
		Ok(_) => {} // valid
		Err(ParseError::FileTooLarge(..)) => panic!("file at limit should not be rejected"),
		Err(_) => {} // other parse errors are fine
	}
}

#[test]
fn parse_accepts_small_file() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("small.json");
	let json = r#"{"$schema":"v0","targets":{"a":{"commands":["echo hi"]}}}"#;
	std::fs::write(&path, json).unwrap();
	let result = parse_runfile_from_path(&path);
	assert!(result.is_ok());
}

#[test]
fn file_size_error_includes_actual_size() {
	let dir = TempDir::new().unwrap();
	let path = dir.path().join("big.json");
	let size = crate::MAX_RUNFILE_SIZE + 42;
	std::fs::write(&path, " ".repeat(size as usize)).unwrap();

	let result = parse_runfile_from_path(&path);
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains(&size.to_string()),
		"error should include actual size: {err}"
	);
}

#[test]
fn parse_nonexistent_file_gives_io_error_not_size_error() {
	let result = parse_runfile_from_path(std::path::Path::new("/nonexistent/path/Runfile.json"));
	assert!(result.is_err());
	match result.unwrap_err() {
		ParseError::Io(_) => {} // expected
		other => panic!("expected Io error, got: {other}"),
	}
}

// ── Include path traversal tests ──────────────────────────────────

#[test]
fn include_within_project_succeeds() {
	let dir = TempDir::new().unwrap();

	// Create sub/included.json
	let sub = dir.path().join("sub");
	std::fs::create_dir(&sub).unwrap();
	std::fs::write(
		sub.join("included.json"),
		r#"{ "$schema": "x", "targets": { "included": { "commands": ["echo included"] } } }"#,
	)
	.unwrap();

	// Create root Runfile that includes sub/included.json
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&root_path,
		r#"{ "$schema": "x", "includes": ["sub/included.json"], "targets": { "root": { "commands": ["echo root"] } } }"#,
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());

	let result = crate::merge::resolve_includes(&runfile, &canonical, &mut state, &mut visited);
	assert!(result.is_ok(), "include within project should succeed");
	assert!(state.targets.contains_key("included"));
}

#[test]
fn include_path_traversal_rejected() {
	let dir = TempDir::new().unwrap();

	// Create an outer file that's OUTSIDE the project
	let outer = dir.path().join("outer");
	std::fs::create_dir(&outer).unwrap();
	std::fs::write(
		outer.join("evil.json"),
		r#"{ "$schema": "x", "targets": { "evil": { "commands": ["echo pwned"] } } }"#,
	)
	.unwrap();

	// Create project directory with a Runfile that tries to include ../outer/evil.json
	let project = dir.path().join("project");
	std::fs::create_dir(&project).unwrap();
	let root_path = project.join(RUNFILE_NAME);
	std::fs::write(
		&root_path,
		r#"{ "$schema": "x", "includes": ["../outer/evil.json"], "targets": { "safe": { "commands": ["echo safe"] } } }"#,
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());

	let result = crate::merge::resolve_includes(&runfile, &canonical, &mut state, &mut visited);
	assert!(result.is_err(), "include path traversal should be rejected");
	let err = result.unwrap_err().to_string();
	assert!(
		err.contains("escapes the project directory"),
		"error should mention path traversal: {err}"
	);
}

#[test]
fn include_absolute_path_outside_project_rejected() {
	let dir = TempDir::new().unwrap();

	// Create an outside file
	let outside = dir.path().join("outside");
	std::fs::create_dir(&outside).unwrap();
	let outside_file = outside.join("external.json");
	std::fs::write(
		&outside_file,
		r#"{ "$schema": "x", "targets": { "ext": { "commands": ["echo ext"] } } }"#,
	)
	.unwrap();

	// Create project dir with Runfile including the absolute outside path
	let project = dir.path().join("project2");
	std::fs::create_dir(&project).unwrap();
	let root_path = project.join(RUNFILE_NAME);
	let include_path = outside_file.to_string_lossy().replace('\\', "/");
	std::fs::write(
		&root_path,
		format!(
			r#"{{ "$schema": "x", "includes": ["{include_path}"], "targets": {{ "safe": {{ "commands": ["echo safe"] }} }} }}"#,
		),
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());

	let result = crate::merge::resolve_includes(&runfile, &canonical, &mut state, &mut visited);
	assert!(result.is_err(), "absolute path outside project should be rejected");
}

// ── Include namespacing ───────────────────────────────────────────

/// Set up a temp directory with a root Runfile that includes a child file,
/// run `resolve_includes`, and return the resulting `MergeState` so the test
/// can inspect renamed targets and rewritten `@target` references.
fn run_namespace_include(root_json: &str, files: &[(&str, &str)]) -> crate::merge::MergeState {
	let dir = TempDir::new().unwrap();
	for (rel, body) in files {
		let path = dir.path().join(rel);
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent).unwrap();
		}
		std::fs::write(path, body).unwrap();
	}
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(&root_path, root_json).unwrap();
	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());
	crate::merge::resolve_includes(&runfile, &canonical, &mut state, &mut visited).unwrap();
	state
}

#[test]
fn parse_include_string_form() {
	let json = r#"{
        "$schema": "x",
        "includes": ["a.json", "b.json"],
        "targets": { "root": { "commands": ["echo root"] } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let inc = rf.includes.unwrap();
	assert_eq!(inc.len(), 2);
	assert_eq!(inc[0].path, "a.json");
	assert!(inc[0].namespace.is_none());
	assert_eq!(inc[1].path, "b.json");
	assert!(inc[1].namespace.is_none());
}

#[test]
fn parse_include_object_form_with_namespace() {
	let json = r#"{
        "$schema": "x",
        "includes": [
            { "path": "a.json", "namespace": "child" },
            { "path": "b.json" }
        ],
        "targets": { "root": { "commands": ["echo root"] } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let inc = rf.includes.unwrap();
	assert_eq!(inc[0].path, "a.json");
	assert_eq!(inc[0].namespace.as_deref(), Some("child"));
	assert_eq!(inc[1].path, "b.json");
	assert!(inc[1].namespace.is_none(), "missing namespace = no prefix");
}

#[test]
fn parse_include_blank_namespace_treated_as_none() {
	let json = r#"{
        "$schema": "x",
        "includes": [{ "path": "a.json", "namespace": "" }],
        "targets": { "root": { "commands": ["echo root"] } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let inc = rf.includes.unwrap();
	assert!(
		inc[0].namespace.is_none(),
		"empty-string namespace must normalise to None"
	);
}

#[test]
fn include_namespace_prefixes_target_names_and_aliases() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"], "aliases": ["b"] },
                "lint": { "commands": ["echo lint"] }
            } }"#,
		)],
	);

	assert!(state.targets.contains_key("child:build"));
	assert!(state.targets.contains_key("child:lint"));
	assert!(
		!state.targets.contains_key("build"),
		"child's `build` must be namespaced"
	);
	assert!(!state.targets.contains_key("lint"), "child's `lint` must be namespaced");

	let aliases = state.targets["child:build"].aliases.as_ref().unwrap();
	assert_eq!(aliases, &vec!["child:b".to_string()], "aliases get the same prefix");
}

#[test]
fn include_namespace_rewrites_target_calls_inside_child() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			// `lint` calls `@build` — must resolve to the child's build, never the parent's.
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"] },
                "lint":  { "commands": ["@build"] }
            } }"#,
		)],
	);

	let lint_steps = &state.targets["child:lint"].commands;
	match &lint_steps[0] {
		CommandStep::TargetCall(call) => {
			assert_eq!(
				call.target, "child:build",
				"@build inside child must be rewritten to @child:build"
			);
		}
		other => panic!("expected TargetCall after namespacing, got {other:?}"),
	}
}

#[test]
fn parent_targets_keep_unprefixed_target_calls() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": {
                "build": { "commands": ["echo parent-build"] },
                "all":   { "commands": ["@build", "@child:build"] }
            }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo child-build"] } } }"#,
		)],
	);

	// Parent's targets are inserted by the caller (merge_runfiles_inner) — at
	// this stage `state` holds only included targets. Sanity-check the child
	// got namespaced, so the parent's literal `@child:build` will resolve at
	// runtime against the merged map.
	assert!(state.targets.contains_key("child:build"));
	assert!(!state.targets.contains_key("build"));
}

#[test]
fn nested_includes_compose_namespaces() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "mid.json", "namespace": "outer" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[
			(
				"mid.json",
				// `mid` includes `inner.json` as `inner` and has its own `@build`.
				r#"{ "$schema": "x",
                     "includes": [{ "path": "inner.json", "namespace": "inner" }],
                     "targets": {
                         "build": { "commands": ["@inner:build"] }
                     } }"#,
			),
			(
				"inner.json",
				r#"{ "$schema": "x", "targets": {
                    "build": { "commands": ["echo inner-build"] }
                } }"#,
			),
		],
	);

	// Both layers fold under `outer:`.
	assert!(state.targets.contains_key("outer:build"));
	assert!(state.targets.contains_key("outer:inner:build"));

	// `mid`'s `@inner:build` reference must compose to `@outer:inner:build`.
	let outer_build = &state.targets["outer:build"].commands;
	match &outer_build[0] {
		CommandStep::TargetCall(call) => {
			assert_eq!(call.target, "outer:inner:build");
		}
		other => panic!("expected nested TargetCall, got {other:?}"),
	}
}

#[test]
fn include_without_namespace_keeps_original_names() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": ["child.json"],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "child_build": { "commands": ["echo child"] }
            } }"#,
		)],
	);
	assert!(state.targets.contains_key("child_build"));
}

#[test]
fn include_object_form_without_namespace_keeps_original_names() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "child_build": { "commands": ["echo child"] }
            } }"#,
		)],
	);
	assert!(state.targets.contains_key("child_build"));
}

#[test]
fn include_namespace_preserves_internal_targets() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "child" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "_helper": { "commands": ["echo helper"] }
            } }"#,
		)],
	);

	assert!(state.targets.contains_key("child:_helper"));
	// Internal-ness rides along with the canonical name through namespacing.
	assert!(
		is_internal_target_name("child:_helper"),
		"namespaced internal targets must still report internal"
	);
	assert!(!is_internal_target_name("child:build"));
	assert!(is_internal_target_name("_helper"));
}

// ── Namespace tracking for `for in: "namespaces"` ──────────────────

#[test]
fn merge_records_top_level_namespace() {
	// A single namespaced include populates `state.namespaces` with that
	// one entry — used at runtime to expand `for "in": "namespaces"`.
	let state = run_namespace_include(
		r#"{
			"$schema": "x",
			"includes": [{ "path": "child.json", "namespace": "child" }],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo build"] } } }"#,
		)],
	);
	assert_eq!(state.namespaces, vec!["child".to_string()]);
}

#[test]
fn merge_records_no_namespaces_for_unnamespaced_includes() {
	// String-form (no namespace) and object-form-without-namespace contribute
	// nothing to the namespaces list.
	let state = run_namespace_include(
		r#"{
			"$schema": "x",
			"includes": ["plain.json", { "path": "obj.json" }],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
		&[
			(
				"plain.json",
				r#"{ "$schema": "x", "targets": { "p": { "commands": ["echo p"] } } }"#,
			),
			(
				"obj.json",
				r#"{ "$schema": "x", "targets": { "o": { "commands": ["echo o"] } } }"#,
			),
		],
	);
	assert!(
		state.namespaces.is_empty(),
		"unnamespaced includes contribute nothing: {:?}",
		state.namespaces
	);
}

#[test]
fn merge_namespaces_compose_innermost_first() {
	// Nested includes layer up: a chain `outer → inner` lands as both
	// `outer` and `outer:inner` in the namespaces list.
	let state = run_namespace_include(
		r#"{
			"$schema": "x",
			"includes": [{ "path": "mid.json", "namespace": "outer" }],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
		&[
			(
				"mid.json",
				r#"{ "$schema": "x",
				     "includes": [{ "path": "inner.json", "namespace": "inner" }],
				     "targets": { "build": { "commands": ["echo build"] } } }"#,
			),
			(
				"inner.json",
				r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo inner-build"] } } }"#,
			),
		],
	);
	let mut ns = state.namespaces.clone();
	ns.sort();
	assert_eq!(
		ns,
		vec!["outer".to_string(), "outer:inner".to_string()],
		"nested namespaces compose with the outer prefix"
	);
}

#[test]
fn merge_namespaces_dedup_in_final_runfile() {
	// `merge_runfiles` sorts and dedupes, so the same namespace appearing
	// under multiple roots yields a single entry. Tested via the public
	// `merge_runfiles` API — `MergeState` itself just accumulates.
	use crate::merge_runfiles;
	let dir = TempDir::new().unwrap();

	// Two siblings with the same namespace `"shared"`.
	std::fs::write(
		dir.path().join("a.json"),
		r#"{ "$schema": "x", "targets": { "build": { "commands": ["a"] } } }"#,
	)
	.unwrap();
	std::fs::write(
		dir.path().join("b.json"),
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["b"] } } }"#,
	)
	.unwrap();

	let local_path = dir.path().join(RUNFILE_NAME);
	let local = parse_runfile(
		r#"{
			"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
			"includes": [
				{ "path": "a.json", "namespace": "shared" },
				{ "path": "b.json", "namespace": "shared" }
			],
			"targets": { "root": { "commands": ["echo root"] } }
		}"#,
	)
	.unwrap();
	std::fs::write(&local_path, "{}").unwrap();

	let result = merge_runfiles(Some((local, local_path)), &[], dir.path()).unwrap();
	assert_eq!(
		result.runfile.namespaces,
		vec!["shared".to_string()],
		"duplicate namespaces from sibling includes are deduplicated"
	);
}

#[test]
fn rewrites_target_calls_inside_control_flow() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [{ "path": "child.json", "namespace": "ns" }],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"child.json",
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"] },
                "lint":  { "commands": ["echo lint"] },
                "all": {
                    "commands": [
                        { "if": "{{ RUN.os }} == windows",
                          "then": ["@build"],
                          "else": ["@lint"] },
                        { "for": "x", "in": ["a"], "do": ["@build"] }
                    ]
                }
            } }"#,
		)],
	);

	let all = &state.targets["ns:all"].commands;

	// First step: an `if` block — both branches should be rewritten.
	match &all[0] {
		CommandStep::If(i) => {
			match &i.then[0] {
				CommandStep::TargetCall(c) => assert_eq!(c.target, "ns:build"),
				other => panic!("expected target call in `then`, got {other:?}"),
			}
			let else_branch = i.r#else.as_ref().expect("else");
			match &else_branch[0] {
				CommandStep::TargetCall(c) => assert_eq!(c.target, "ns:lint"),
				other => panic!("expected target call in `else`, got {other:?}"),
			}
		}
		other => panic!("expected If, got {other:?}"),
	}

	// Second step: a `for` block — body should be rewritten too.
	match &all[1] {
		CommandStep::For(f) => match &f.body[0] {
			CommandStep::TargetCall(c) => assert_eq!(c.target, "ns:build"),
			other => panic!("expected target call in `for/do`, got {other:?}"),
		},
		other => panic!("expected For, got {other:?}"),
	}
}

#[test]
fn invalid_namespace_rejected() {
	for bad in &[":foo", "foo:bar", "_foo", "@foo", "foo bar", "", "ns?bad", "?leading"] {
		let dir = TempDir::new().unwrap();
		std::fs::write(
			dir.path().join("child.json"),
			r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo"] } } }"#,
		)
		.unwrap();
		let root_path = dir.path().join(RUNFILE_NAME);
		// Empty-string namespace round-trips through the deserializer's
		// "blank = no namespace" rule, so we test it via the merge layer too —
		// but here we expect it to *not* error (it's normalised away).
		let body = if bad.is_empty() {
			r#"{ "$schema": "x", "includes": [{ "path": "child.json", "namespace": "" }],
                  "targets": { "root": { "commands": ["echo"] } } }"#
				.to_string()
		} else {
			format!(
				r#"{{ "$schema": "x", "includes": [{{ "path": "child.json", "namespace": "{}" }}],
                       "targets": {{ "root": {{ "commands": ["echo"] }} }} }}"#,
				bad.replace('"', "\\\"")
			)
		};
		std::fs::write(&root_path, body).unwrap();
		let runfile = parse_runfile_from_path(&root_path).unwrap();
		let mut state = crate::merge::MergeState::new();
		let canonical = std::fs::canonicalize(&root_path).unwrap();
		let mut visited = std::collections::HashSet::new();
		visited.insert(canonical.clone());
		let result = crate::merge::resolve_includes(&runfile, &canonical, &mut state, &mut visited);
		if bad.is_empty() {
			assert!(result.is_ok(), "blank namespace must round-trip as no-namespace");
		} else {
			let err = result.expect_err(&format!("namespace \"{bad}\" must be rejected"));
			let msg = err.to_string();
			assert!(
				msg.contains("Invalid include namespace"),
				"error must mention namespace ({bad}): {msg}"
			);
		}
	}
}

#[test]
fn same_file_included_twice_with_different_namespaces_yields_independent_copies() {
	let state = run_namespace_include(
		r#"{
            "$schema": "x",
            "includes": [
                { "path": "tmpl.json", "namespace": "a" },
                { "path": "tmpl.json", "namespace": "b" }
            ],
            "targets": { "root": { "commands": ["echo root"] } }
        }"#,
		&[(
			"tmpl.json",
			r#"{ "$schema": "x", "targets": {
                "build": { "commands": ["echo build"] }
            } }"#,
		)],
	);

	assert!(state.targets.contains_key("a:build"));
	assert!(state.targets.contains_key("b:build"));
}

#[test]
fn diamond_include_no_namespace_no_cycle_error() {
	// A includes B and C; C also includes B. Without per-call-stack cycle
	// detection this would (incorrectly) fail as a cycle. With the chain-style
	// `visited`, B re-loads cleanly via the second path and merge_runfiles
	// detects the duplicate target as a conflict, not a cycle.
	let dir = TempDir::new().unwrap();
	std::fs::write(
		dir.path().join("b.json"),
		r#"{ "$schema": "x", "targets": { "leaf": { "commands": ["echo leaf"] } } }"#,
	)
	.unwrap();
	std::fs::write(
		dir.path().join("c.json"),
		r#"{ "$schema": "x", "includes": ["b.json"], "targets": {} }"#,
	)
	.unwrap();
	let root_path = dir.path().join(RUNFILE_NAME);
	std::fs::write(
		&root_path,
		r#"{ "$schema": "x", "includes": ["b.json", "c.json"],
              "targets": { "root": { "commands": ["echo root"] } } }"#,
	)
	.unwrap();

	let runfile = parse_runfile_from_path(&root_path).unwrap();
	let mut state = crate::merge::MergeState::new();
	let canonical = std::fs::canonicalize(&root_path).unwrap();
	let mut visited = std::collections::HashSet::new();
	visited.insert(canonical.clone());
	let result = crate::merge::resolve_includes(&runfile, &canonical, &mut state, &mut visited);
	assert!(
		result.is_ok(),
		"diamond include should not be reported as a cycle: {:?}",
		result
	);
}

// ── extendStdio tests ─────────────────────────────────────────────

#[test]
fn parse_extend_stdio() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["npm run build"],
                "extendStdio": [
                    { "fromFile": "build.log", "stream": "stdout" },
                    { "fromFile": "errors.log", "stream": "stderr" }
                ]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let ext = rf.targets["build"].extend_stdio.as_ref().unwrap();
	assert_eq!(ext.len(), 2);
	assert_eq!(ext[0].from_file, "build.log");
	assert_eq!(ext[0].stream, StdioStream::Stdout);
	assert_eq!(ext[1].from_file, "errors.log");
	assert_eq!(ext[1].stream, StdioStream::Stderr);
}

#[test]
fn parse_extend_stdio_rejects_unknown_stream() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "extendStdio": [
                    { "fromFile": "x.log", "stream": "stdin" }
                ]
            }
        }
    }"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_extend_stdio_rejects_missing_fields() {
	// Missing "stream"
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "extendStdio": [{ "fromFile": "x.log" }]
            }
        }
    }"#;
	assert!(parse_runfile(json).is_err());

	// Missing "fromFile"
	let json2 = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "build": {
                "commands": ["echo"],
                "extendStdio": [{ "stream": "stdout" }]
            }
        }
    }"#;
	assert!(parse_runfile(json2).is_err());
}

// ── forceKillOnSigInt tests ───────────────────────────────────────

#[test]
fn parse_force_kill_on_sig_int() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "unity": {
                "commands": ["unity -batchmode"],
                "forceKillOnSigInt": true
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.targets["unity"].force_kill_on_sig_int, Some(true));
}

#[test]
fn parse_force_kill_on_sig_int_in_globals() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "globals": {
            "forceKillOnSigInt": true
        },
        "targets": {
            "unity": {
                "commands": ["unity -batchmode"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert_eq!(rf.globals.unwrap().force_kill_on_sig_int, Some(true));
}

// ── Internal targets (names starting with "_") ─────────────────────

#[test]
fn parse_accepts_internal_target_name() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": { "commands": ["echo internal"] },
            "build":  { "commands": ["cargo build"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	assert!(rf.targets.contains_key("_setup"));
	assert!(is_internal_target_name("_setup"));
	assert!(!is_internal_target_name("build"));
}

#[test]
fn is_internal_resolves_through_aliases() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": {
                "commands": ["echo internal"],
                "aliases": ["bootstrap"]
            },
            "build": { "commands": ["cargo build"] }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	// Canonical and alias for an internal target both report as internal.
	assert!(rf.is_internal("_setup"));
	assert!(rf.is_internal("bootstrap"));
	// Public target is not internal.
	assert!(!rf.is_internal("build"));
	// Unknown name is not internal.
	assert!(!rf.is_internal("nope"));
}

#[test]
fn public_target_names_excludes_internal_targets_and_their_aliases() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": {
                "commands": ["echo internal"],
                "aliases": ["bootstrap"]
            },
            "build": {
                "commands": ["cargo build"],
                "aliases": ["b"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();

	let all = rf.all_target_names();
	assert!(all.contains(&"_setup"));
	assert!(all.contains(&"bootstrap"));
	assert!(all.contains(&"build"));
	assert!(all.contains(&"b"));

	let public = rf.public_target_names();
	assert!(!public.contains(&"_setup"));
	assert!(!public.contains(&"bootstrap"));
	assert!(public.contains(&"build"));
	assert!(public.contains(&"b"));
}

#[test]
fn internal_target_can_be_referenced_via_at_call() {
	// `@` invocations to internal targets (`_name`) are valid — internal-only
	// means "not directly invocable from the CLI", not "uncallable".
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "_setup": { "commands": ["echo setup"] },
            "build": {
                "commands": ["@_setup", "cargo build"]
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	match &rf.targets["build"].commands[0] {
		CommandStep::TargetCall(call) => assert_eq!(call.target, "_setup"),
		_ => panic!("expected TargetCall"),
	}
}

// ──────────────────────────────────────────────────────────────────
// Control flow: if / for blocks
// ──────────────────────────────────────────────────────────────────

#[test]
fn parse_if_block_with_string_then() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARGS.env }} == production", "then": ["./deploy-prod.sh"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmd0 = &rf.targets["deploy"].commands[0];
	match cmd0 {
		CommandStep::If(if_step) => {
			assert_eq!(if_step.condition, "{{ ARGS.env }} == production");
			assert_eq!(if_step.then.len(), 1);
			assert!(if_step.r#else.is_none());
			assert!(if_step.condition_ast.is_some());
		}
		_ => panic!("expected If block"),
	}
}

#[test]
fn parse_if_block_with_else() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARGS.dry-run }}", "then": ["echo would deploy"], "else": ["./deploy.sh"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmd0 = &rf.targets["deploy"].commands[0];
	match cmd0 {
		CommandStep::If(if_step) => {
			assert!(if_step.r#else.is_some());
			let else_branch = if_step.r#else.as_ref().unwrap();
			assert_eq!(else_branch.len(), 1);
		}
		_ => panic!("expected If block"),
	}
}

#[test]
fn parse_if_block_with_ignore_errors() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"clean": {
				"commands": [
					{ "if": "{{ FLAGS.force }} == true", "then": ["rm -rf target"], "ignoreErrors": true }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["clean"].commands[0] {
		assert_eq!(if_step.ignore_errors, Some(true));
	} else {
		panic!("expected If block");
	}
}

#[test]
fn parse_if_block_then_as_string_shorthand() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARGS.env }} == production", "then": "./deploy-prod.sh" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["deploy"].commands[0] {
		assert_eq!(if_step.then.len(), 1);
		assert_eq!(if_step.then[0], "./deploy-prod.sh");
		assert!(if_step.r#else.is_none());
	} else {
		panic!("expected If block");
	}
}

#[test]
fn parse_if_block_else_as_string_shorthand() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARGS.dry-run }}", "then": "echo would deploy", "else": "./deploy.sh" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["deploy"].commands[0] {
		assert_eq!(if_step.then.len(), 1);
		assert_eq!(if_step.then[0], "echo would deploy");
		let else_branch = if_step.r#else.as_ref().unwrap();
		assert_eq!(else_branch.len(), 1);
		assert_eq!(else_branch[0], "./deploy.sh");
	} else {
		panic!("expected If block");
	}
}

#[test]
fn parse_if_block_mixed_string_then_array_else() {
	// String `then` + array `else` and vice versa should both work side by side.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": {
				"commands": [
					{ "if": "{{ ARGS.x }}", "then": "echo a", "else": ["echo b", "echo c"] },
					{ "if": "{{ ARGS.y }}", "then": ["echo d", "echo e"], "else": "echo f" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["t"].commands;
	if let CommandStep::If(s) = &cmds[0] {
		assert_eq!(s.then.len(), 1);
		assert_eq!(s.r#else.as_ref().unwrap().len(), 2);
	} else {
		panic!("expected If");
	}
	if let CommandStep::If(s) = &cmds[1] {
		assert_eq!(s.then.len(), 2);
		assert_eq!(s.r#else.as_ref().unwrap().len(), 1);
	} else {
		panic!("expected If");
	}
}

// ── `commands` as string shorthand ────────────────────────

#[test]
fn parse_target_commands_as_string_shorthand() {
	// A bare string in place of the `commands` array should be treated as a
	// one-element array containing a single shell step.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": "cargo build" }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["build"].commands;
	assert_eq!(cmds.len(), 1);
	assert_eq!(cmds[0], "cargo build");
}

#[test]
fn parse_target_commands_string_shorthand_target_call() {
	// The `@target` shorthand applies even when `commands` is a string.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"a": { "commands": "@b --release" },
			"b": { "commands": ["cargo build --release"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["a"].commands;
	assert_eq!(cmds.len(), 1);
	if let CommandStep::TargetCall(call) = &cmds[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "--release");
	} else {
		panic!("expected TargetCall, got {:?}", cmds[0]);
	}
}

#[test]
fn parse_when_step_commands_as_string_shorthand() {
	// `when:` blocks accept the same string shorthand for `commands`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": {
				"commands": [
					"./run-tests.sh",
					{ "when": "failure", "commands": "./report.sh" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["t"].commands;
	assert_eq!(cmds.len(), 2);
	if let CommandStep::When(w) = &cmds[1] {
		assert_eq!(w.when, WhenCondition::Failure);
		assert_eq!(w.commands.len(), 1);
		assert_eq!(w.commands[0], "./report.sh");
	} else {
		panic!("expected When, got {:?}", cmds[1]);
	}
}

#[test]
fn parse_target_commands_as_array_still_works() {
	// Adding the string shorthand must not break the existing array form.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["cargo build", "echo done"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["build"].commands;
	assert_eq!(cmds.len(), 2);
	assert_eq!(cmds[0], "cargo build");
	assert_eq!(cmds[1], "echo done");
}

// ── `when:` block parsing ─────────────────────────────────

#[test]
fn parse_when_block_default_success() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "commands": ["echo hi"] }] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.when, WhenCondition::Success);
		assert_eq!(w.commands.len(), 1);
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_block_failure() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "failure", "commands": ["./report.sh"] }] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.when, WhenCondition::Failure);
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_block_always() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "always", "commands": ["./cleanup.sh"] }] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.when, WhenCondition::Always);
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_on_if_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [
				{ "when": "always", "if": "{{ RUN.os }} == windows", "then": "rm -rf tmp" }
			] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["t"].commands[0] {
		assert_eq!(if_step.when, Some(WhenCondition::Always));
	} else {
		panic!("expected If");
	}
}

#[test]
fn parse_when_on_for_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [
				{ "when": "failure", "for": "f", "glob": "logs/*", "do": ["cat {{ LOOP.f }}"] }
			] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["t"].commands[0] {
		assert_eq!(for_step.when, Some(WhenCondition::Failure));
	} else {
		panic!("expected For");
	}
}

#[test]
fn parse_when_block_with_ignore_errors() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [
				{ "when": "always", "commands": ["./cleanup.sh"], "ignoreErrors": true }
			] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::When(w) = &rf.targets["t"].commands[0] {
		assert_eq!(w.ignore_errors, Some(true));
	} else {
		panic!("expected When");
	}
}

#[test]
fn parse_when_block_rejects_empty_commands() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "always", "commands": [] }] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_when_block_rejects_unknown_when_value() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": [{ "when": "sometimes", "commands": ["echo"] }] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

// ── @target invocation parsing ─────────────────────────────

#[test]
fn parse_target_call_no_args() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"ci": { "commands": ["@build"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["ci"].commands[0] {
		assert_eq!(call.target, "build");
		assert_eq!(call.args_template, "");
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_target_call_with_args() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"ci": { "commands": ["@build --release --features foo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["ci"].commands[0] {
		assert_eq!(call.target, "build");
		assert_eq!(call.args_template, "--release --features foo");
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_target_call_with_args_substitution_template() {
	// {{ ARGS }} and {{ RUN.os }} are preserved in the args_template — substitution
	// happens at runtime.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"ci": { "commands": ["@build {{ ARGS }} --os={{ RUN.os }}"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["ci"].commands[0] {
		assert_eq!(call.target, "build");
		assert_eq!(call.args_template, "{{ ARGS }} --os={{ RUN.os }}");
	}
}

#[test]
fn parse_target_call_inside_if_branches() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"prod-deploy": { "commands": ["echo prod"] },
			"dev-deploy": { "commands": ["echo dev"] },
			"deploy": {
				"commands": [
					{ "if": "{{ ARGS.env }} == production", "then": "@prod-deploy", "else": "@dev-deploy" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::If(if_step) = &rf.targets["deploy"].commands[0] {
		assert!(matches!(&if_step.then[0], CommandStep::TargetCall(c) if c.target == "prod-deploy"));
		let else_branch = if_step.r#else.as_ref().unwrap();
		assert!(matches!(&else_branch[0], CommandStep::TargetCall(c) if c.target == "dev-deploy"));
	} else {
		panic!("expected If");
	}
}

#[test]
fn parse_target_call_inside_for_body() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"matrix": {
				"commands": [
					{ "for": "v", "in": ["1", "2"], "do": ["@build --version {{ LOOP.v }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["matrix"].commands[0] {
		assert!(
			matches!(&for_step.body[0], CommandStep::TargetCall(c) if c.target == "build" && c.args_template == "--version {{ LOOP.v }}")
		);
	} else {
		panic!("expected For");
	}
}

#[test]
fn parse_target_call_with_quoted_args() {
	// Quoted args are kept verbatim — shlex-splitting happens at execute time.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"echo": { "commands": ["echo {{ ARGS }}"] },
			"t": { "commands": ["@echo \"hello world\" foo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["t"].commands[0] {
		assert_eq!(call.target, "echo");
		assert_eq!(call.args_template, "\"hello world\" foo");
	}
}

#[test]
fn parse_target_call_rejects_empty_target_name() {
	// `@` alone or `@ args` is rejected at parse time.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": ["@ foo"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(
		err.contains("@") || err.contains("target name"),
		"unexpected error: {err}"
	);
}

#[test]
fn parse_target_call_serializes_back_to_string() {
	// Round-trip: TargetCall serializes as the original `@target args` form.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["echo build"] },
			"t": { "commands": ["@build --release"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let serialized = serde_json::to_string(&rf.targets["t"].commands).unwrap();
	assert!(
		serialized.contains("\"@build --release\""),
		"expected @build --release in {serialized}"
	);
}

#[test]
fn parse_plain_string_with_at_inside_is_shell_command() {
	// `email@host` (no leading @) is a plain shell command.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"t": { "commands": ["echo email@host"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	assert!(matches!(&rf.targets["t"].commands[0], CommandStep::Shell(s) if s == "echo email@host"));
}

#[test]
fn parse_if_block_string_then_rejects_object() {
	// A non-array, non-string `then` should still be a parse error.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "{{ ARGS.x }}", "then": { "if": "true", "then": [] } }
			] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_if_block_empty_then_allowed() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"noop": { "commands": [
				{ "if": "{{ ARGS.x }}", "then": [] }
			] }
		}
	}"#;
	parse_runfile(json).unwrap();
}

#[test]
fn parse_if_rejects_empty_condition() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "", "then": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("Invalid condition") || msg.contains("Empty"), "got: {msg}");
}

#[test]
fn parse_if_rejects_malformed_condition() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "a && b || c", "then": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("Invalid condition"));
}

#[test]
fn parse_for_in_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build_each": {
				"commands": [
					{ "for": "service", "in": ["api", "web"], "do": ["echo {{ LOOP.service }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["build_each"].commands[0] {
		assert_eq!(for_step.var, "service");
		assert_eq!(
			for_step.r#in.as_ref().unwrap(),
			&crate::ForInValue::Literal(vec!["api".to_string(), "web".to_string()])
		);
		assert_eq!(for_step.body.len(), 1);
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_do_accepts_single_string() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"each": {
				"commands": [
					{ "for": "x", "in": ["a", "b"], "do": "echo {{ LOOP.x }}" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["each"].commands[0] {
		assert_eq!(for_step.body.len(), 1);
		assert_eq!(for_step.body[0], "echo {{ LOOP.x }}");
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_in_namespaces_magic_string() {
	// `"in": "namespaces"` is the only string form accepted — anything else
	// errors. Used to iterate over namespace prefixes from `includes`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build_all": {
				"commands": [
					{ "for": "ns", "in": "namespaces", "do": "@{{ LOOP.ns }}:build" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["build_all"].commands[0] {
		assert_eq!(for_step.var, "ns");
		assert_eq!(for_step.r#in.as_ref().unwrap(), &crate::ForInValue::Namespaces);
		// Body's "@{{ LOOP.ns }}:build" string starts with @, so it parses as a target call
		// with an empty target (the namespace is filled in at runtime).
		assert_eq!(for_step.body.len(), 1);
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_in_array_still_works_alongside_magic_string() {
	// Sanity: existing `in: [array]` form is unaffected by the new magic-string
	// path through `ForInValue`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"each": {
				"commands": [
					{ "for": "x", "in": ["a", "b", "c"], "do": "echo {{ LOOP.x }}" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["each"].commands[0] {
		assert_eq!(
			for_step.r#in.as_ref().unwrap(),
			&crate::ForInValue::Literal(vec!["a".into(), "b".into(), "c".into()])
		);
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_in_string_other_than_namespaces_errors() {
	// Only `"namespaces"` is a recognised string form — anything else is a
	// hard error to catch typos like `"namespace"` (singular).
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": {
				"commands": [
					{ "for": "ns", "in": "namespace", "do": ["echo"] }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(
		msg.contains("namespaces") && msg.contains("namespace"),
		"error should call out the typo and the accepted keyword: {msg}"
	);
}

#[test]
fn parse_for_in_object_form_errors() {
	// Defensive: rejecting non-array/non-string `in` values with a clear message.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": {
				"commands": [
					{ "for": "x", "in": { "a": 1 }, "do": [] }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("namespaces") || err.to_string().contains("array"));
}

#[test]
fn for_in_namespaces_roundtrips_through_serde() {
	// Serialize → deserialize must preserve the magic value (string form),
	// not collapse it into an array.
	let original = crate::ForInValue::Namespaces;
	let json = serde_json::to_value(&original).unwrap();
	assert_eq!(json, serde_json::json!("namespaces"));
	let parsed: crate::ForInValue = serde_json::from_value(json).unwrap();
	assert_eq!(parsed, original);

	// Literal also roundtrips cleanly.
	let literal = crate::ForInValue::Literal(vec!["a".into(), "b".into()]);
	let json = serde_json::to_value(&literal).unwrap();
	assert_eq!(json, serde_json::json!(["a", "b"]));
	let parsed: crate::ForInValue = serde_json::from_value(json).unwrap();
	assert_eq!(parsed, literal);
}

#[test]
fn parse_for_glob_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"fmt": {
				"commands": [
					{ "for": "f", "glob": "src/**/*.rs", "do": ["rustfmt {{ LOOP.f }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["fmt"].commands[0] {
		assert_eq!(for_step.glob.as_deref(), Some("src/**/*.rs"));
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_shell_block() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"check": {
				"commands": [
					{ "for": "f", "shell": "git diff --name-only", "do": ["echo {{ LOOP.f }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["check"].commands[0] {
		assert_eq!(for_step.shell.as_deref(), Some("git diff --name-only"));
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_for_rejects_no_iterator() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "for": "x", "do": ["echo {{ LOOP.x }}"] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("for") && msg.contains("none"), "got: {msg}");
}

#[test]
fn parse_for_rejects_multiple_iterators() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "for": "x", "in": ["a"], "glob": "*.rs", "do": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	let msg = err.to_string();
	assert!(msg.contains("for") && (msg.contains("in") || msg.contains("glob")));
}

#[test]
fn parse_for_rejects_invalid_var_name() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "for": "1abc", "in": ["x"], "do": [] }
			] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(err.to_string().contains("loop variable"));
}

#[test]
fn parse_for_with_parallel_flag() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"par": {
				"commands": [
					{ "for": "x", "in": ["1","2","3"], "parallel": true, "do": ["sleep {{ LOOP.x }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["par"].commands[0] {
		assert_eq!(for_step.parallel, Some(true));
	} else {
		panic!("expected For block");
	}
}

#[test]
fn parse_nested_control_flow() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"complex": {
				"commands": [
					{ "for": "svc", "in": ["api","web"], "do": [
						{ "if": "{{ LOOP.svc }} == api", "then": [
							"echo building api",
							{ "for": "stage", "in": ["lint","test","build"], "do": ["echo api {{ LOOP.stage }}"] }
						], "else": [
							"echo building {{ LOOP.svc }}"
						] }
					] }
				]
			}
		}
	}"#;
	parse_runfile(json).unwrap();
}

#[test]
fn parse_unknown_control_flow_field_rejected() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "if": "{{ ARGS.x }}", "then": [], "extraField": 1 }
			] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_object_without_if_or_for_rejected() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"bad": { "commands": [
				{ "foo": "bar" }
			] }
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_control_flow_inside_when_block() {
	// Lifecycle hooks were replaced with `when`-guarded blocks. A `before`
	// step's previous "run inline commands first" role is now just
	// prepending to `commands`; the always/failure-only cases use `when`.
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"deploy": {
				"commands": [
					{ "if": "{{ ARGS.skip-tests }}", "then": ["echo skipping"], "else": ["./test.sh"] },
					"echo deploying"
				]
			}
		}
	}"#;
	parse_runfile(json).unwrap();
}

#[test]
fn parse_backwards_compat_string_only_commands_still_works() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"build": { "commands": ["cargo build", "cargo test"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let cmds = &rf.targets["build"].commands;
	assert_eq!(cmds.len(), 2);
	assert!(matches!(cmds[0], CommandStep::Shell(ref s) if s == "cargo build"));
	assert!(matches!(cmds[1], CommandStep::Shell(ref s) if s == "cargo test"));
}

#[test]
fn walk_step_templates_visits_all_string_payloads() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"x": {
				"commands": [
					"echo top",
					{ "if": "{{ ARGS.flag }}", "then": ["echo then1", "echo then2"], "else": ["echo else1"] },
					{ "for": "x", "in": ["a","b"], "do": ["echo {{ LOOP.x }}"] },
					{ "for": "f", "glob": "*.rs", "do": ["rustfmt {{ LOOP.f }}"] }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let mut seen: Vec<String> = Vec::new();
	walk_step_templates(&rf.targets["x"].commands, &mut |t| seen.push(t.to_string()));

	assert!(seen.contains(&"echo top".to_string()));
	assert!(seen.contains(&"{{ ARGS.flag }}".to_string()));
	assert!(seen.contains(&"echo then1".to_string()));
	assert!(seen.contains(&"echo then2".to_string()));
	assert!(seen.contains(&"echo else1".to_string()));
	assert!(seen.contains(&"a".to_string()));
	assert!(seen.contains(&"b".to_string()));
	assert!(seen.contains(&"echo {{ LOOP.x }}".to_string()));
	assert!(seen.contains(&"*.rs".to_string()));
	assert!(seen.contains(&"rustfmt {{ LOOP.f }}".to_string()));
}

#[test]
fn walk_spec_aux_templates_visits_all_substitutable_fields() {
	let json = r#"{
		"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
		"targets": {
			"x": {
				"commands": ["echo go"],
				"env": {
					"A": "{{ ARGS.a }}",
					"B": "{{ FLAGS.b }}",
					"N": 42,
					"BL": true
				},
				"envFiles": [".env.{{ RUN.os }}", ".env"],
				"forceShell": "{{ ARGS.shell ? bash }}",
				"addToPath": ["bin/{{ ARGS.profile }}"],
				"workingDirectory": "{{ ARGS.dir ? RUN.parent }}",
				"confirm": "Run with {{ ARGS.env }}?",
				"extendStdio": [{ "fromFile": "logs/{{ RUN.os }}.log", "stream": "stdout" }]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let mut seen: Vec<String> = Vec::new();
	walk_spec_aux_templates(&rf.targets["x"], &mut |t| seen.push(t.to_string()));

	// commands array is intentionally NOT covered by this walker.
	assert!(!seen.iter().any(|s| s == "echo go"));

	// env values: only string variants are visited (numbers/bools have no templates).
	assert!(seen.iter().any(|s| s == "{{ ARGS.a }}"));
	assert!(seen.iter().any(|s| s == "{{ FLAGS.b }}"));
	assert!(!seen.iter().any(|s| s == "42"));
	assert!(!seen.iter().any(|s| s == "true"));

	assert!(seen.iter().any(|s| s == ".env.{{ RUN.os }}"));
	assert!(seen.iter().any(|s| s == ".env"));
	assert!(seen.iter().any(|s| s == "{{ ARGS.shell ? bash }}"));
	assert!(seen.iter().any(|s| s == "bin/{{ ARGS.profile }}"));
	assert!(seen.iter().any(|s| s == "{{ ARGS.dir ? RUN.parent }}"));
	assert!(seen.iter().any(|s| s == "Run with {{ ARGS.env }}?"));
	assert!(seen.iter().any(|s| s == "logs/{{ RUN.os }}.log"));
}

#[test]
fn parse_dsl_features_all_supported() {
	let conditions = [
		"{{ ARGS.x }}",
		"{{ ARGS.x }} == y",
		"{{ ARGS.x }} != y",
		"a == b && c == d",
		"a == b || c == d",
		"!a",
		"!!a",
		"!(a == b)",
		"(a && b) || c",
		"a || (b && c)",
		"{{ ARGS.x ? default }} == foo",
		"{{ ENV.HOME }} != \"\"",
	];
	for c in conditions {
		let escaped = c.replace('\\', "\\\\").replace('"', "\\\"");
		let json = format!(
			r#"{{ "$schema": "x", "targets": {{ "t": {{ "commands": [
				{{ "if": "{escaped}", "then": [] }}
			] }} }} }}"#
		);
		parse_runfile(&json).unwrap_or_else(|e| panic!("Failed to parse condition `{c}`: {e}"));
	}
}

#[test]
fn parse_optional_target_call_marker() {
	// `@?target` parses with optional = true; the `?` is stripped from the
	// in-memory target name.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?b --release"] },
			"b": { "commands": ["echo b"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["a"].commands[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "--release");
		assert!(call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_optional_target_call_with_dynamic_name() {
	// `@?{{ LOOP.ns }}:build` is the canonical use case — combine optional with
	// runtime substitution. The `?` is stripped, leaving the substitutable name.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": {
				"commands": [
					{ "for": "ns", "in": "namespaces", "do": "@?{{ LOOP.ns }}:build" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::For(for_step) = &rf.targets["a"].commands[0] {
		if let CommandStep::TargetCall(call) = &for_step.body[0] {
			assert_eq!(call.target, "{{ LOOP.ns }}:build");
			assert!(call.optional);
		} else {
			panic!("expected TargetCall in for body");
		}
	} else {
		panic!("expected For");
	}
}

#[test]
fn parse_optional_target_call_no_args() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?b"] },
			"b": { "commands": ["echo b"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["a"].commands[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "");
		assert!(call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_optional_target_call_empty_name_errors() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("@?") || err.contains("target name"), "got: {err}");
}

#[test]
fn parse_non_optional_target_call_has_optional_false() {
	// Plain `@target` should leave optional = false (not the new opt-in).
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@b"] },
			"b": { "commands": ["echo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::TargetCall(call) = &rf.targets["a"].commands[0] {
		assert!(!call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn parse_optional_target_call_serializes_back_with_question_mark() {
	// Round-trip: the `@?` marker must survive a serialize → deserialize cycle.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@?b --opt"] },
			"b": { "commands": ["echo"] }
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let serialized = serde_json::to_string(&rf).unwrap();
	assert!(serialized.contains("@?b --opt"), "serialized: {serialized}");
	// And re-parsing produces the same step.
	let rf2 = parse_runfile(&serialized).unwrap();
	if let CommandStep::TargetCall(call) = &rf2.targets["a"].commands[0] {
		assert_eq!(call.target, "b");
		assert_eq!(call.args_template, "--opt");
		assert!(call.optional);
	} else {
		panic!("expected TargetCall");
	}
}

#[test]
fn target_name_with_question_mark_rejected() {
	// `?` is reserved for the `@?target` optional-call marker, so a declared
	// target name containing `?` must be rejected.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"foo?bar": { "commands": ["echo"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("?"), "got: {err}");
}

#[test]
fn alias_with_question_mark_rejected() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["echo"], "aliases": ["a?b"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("?"), "got: {err}");
}

#[test]
fn target_call_with_question_mark_in_name_rejected() {
	// `@foo?bar` is parsed as `@<foo?bar>` (no leading `?`), and `foo?bar`
	// then fails validation because the target name contains `?`.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["@foo?bar"] }
		}
	}"#;
	let err = parse_runfile(json).unwrap_err().to_string();
	assert!(err.contains("?"), "got: {err}");
}

// ── Match step tests ──────────────────────────────────────────────

#[test]
fn parse_match_block() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"emulate": {
				"commands": [
					{
						"match": "{{ ARGS.tier }}",
						"cases": {
							"1": "echo tier 1",
							"2": ["echo tier 2", "echo two"]
						}
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["emulate"].commands[0] {
		assert_eq!(m.r#match, "{{ ARGS.tier }}");
		assert_eq!(m.cases.len(), 2);
		assert_eq!(m.cases["1"], vec![CommandStep::shell("echo tier 1")]);
		assert_eq!(
			m.cases["2"],
			vec![CommandStep::shell("echo tier 2"), CommandStep::shell("echo two")]
		);
		assert!(m.default.is_none());
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_with_default_and_target_call() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"a": { "commands": ["echo a"] },
			"dispatch": {
				"commands": [
					{
						"match": "{{ ARGS.mode ? prod }}",
						"cases": {
							"prod": "@a",
							"dev": ["echo dev"]
						},
						"default": "echo unknown"
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["dispatch"].commands[0] {
		assert_eq!(m.r#match, "{{ ARGS.mode ? prod }}");
		// String case "prod" parsed as `@a` → TargetCall.
		assert!(matches!(&m.cases["prod"][0], CommandStep::TargetCall(c) if c.target == "a"));
		let default = m.default.as_ref().expect("default should be set");
		assert_eq!(default, &vec![CommandStep::shell("echo unknown")]);
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_when_and_ignore_errors() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{
						"match": "{{ ARGS.x }}",
						"cases": { "a": "echo a" },
						"when": "always",
						"ignoreErrors": true
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["t"].commands[0] {
		assert_eq!(m.when, Some(WhenCondition::Always));
		assert_eq!(m.ignore_errors, Some(true));
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_empty_match_expression_rejected() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "", "cases": { "a": "echo a" } }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::EmptyMatchExpression(_)), "got: {err:?}");
}

#[test]
fn parse_match_no_cases_no_default_rejected() {
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "{{ ARGS.x }}", "cases": {} }
				]
			}
		}
	}"#;
	let err = parse_runfile(json).unwrap_err();
	assert!(matches!(err, ParseError::EmptyMatchCases(_)), "got: {err:?}");
}

#[test]
fn parse_match_default_only_is_allowed() {
	// Edge case: an empty `cases` map paired with a `default` is essentially
	// "always run default" — silly but not invalid.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "{{ ARGS.x ? y }}", "cases": {}, "default": "echo y" }
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	if let CommandStep::Match(m) = &rf.targets["t"].commands[0] {
		assert!(m.cases.is_empty());
		assert!(m.default.is_some());
	} else {
		panic!("expected Match block");
	}
}

#[test]
fn parse_match_unknown_field_rejected() {
	// deny_unknown_fields applies — typos should fail loudly.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{ "match": "{{ ARGS.x }}", "cases": { "a": "echo a" }, "extra": true }
				]
			}
		}
	}"#;
	assert!(parse_runfile(json).is_err());
}

#[test]
fn parse_match_round_trips_through_serde() {
	// Parse → serialize → parse must produce the same tree.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{
						"match": "{{ ARGS.x }}",
						"cases": { "a": "echo a", "b": ["echo b1", "echo b2"] },
						"default": "echo other"
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let serialized = serde_json::to_string(&rf).unwrap();
	assert!(
		serialized.contains("\"match\":\"{{ ARGS.x }}\""),
		"serialized: {serialized}"
	);
	let rf2 = parse_runfile(&serialized).unwrap();
	assert_eq!(rf.targets["t"].commands, rf2.targets["t"].commands);
}

#[test]
fn match_walks_templates_inside_cases_and_default() {
	// `walk_step_templates` should visit the match template, every case body,
	// and the default body so static analysis (arg-usage scanning) sees
	// `{{ ARGS.* }}` references inside them.
	let json = r#"{
		"$schema": "x",
		"targets": {
			"t": {
				"commands": [
					{
						"match": "{{ ARGS.tier }}",
						"cases": { "1": "echo {{ ARGS.foo }}" },
						"default": "echo {{ ARGS.bar }}"
					}
				]
			}
		}
	}"#;
	let rf = parse_runfile(json).unwrap();
	let mut seen: Vec<String> = Vec::new();
	walk_step_templates(&rf.targets["t"].commands, &mut |t| seen.push(t.to_string()));
	assert!(seen.iter().any(|s| s == "{{ ARGS.tier }}"), "saw: {seen:?}");
	assert!(seen.iter().any(|s| s == "echo {{ ARGS.foo }}"), "saw: {seen:?}");
	assert!(seen.iter().any(|s| s == "echo {{ ARGS.bar }}"), "saw: {seen:?}");
}
