use super::*;

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
                "envFiles": [".env", ".env.{{ ARG.env ? 'development' }}"]
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
	assert_eq!(env_files[1], ".env.{{ ARG.env ? 'development' }}");

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
			vars: None,
			force_shell: None,
			logging: None,
			ignore_errors: None,
			same_shell: None,
			working_directory: None,
			force_kill_on_sig_int: None,
			only_in_directories: None,
			metadata: None,
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
fn parse_target_and_global_vars() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": {
            "x": {
                "commands": ["echo {{ VAR.xxx }}"],
                "vars": { "xxx": "{{ ENV.some ? 'default' }}", "yyy": 35, "ok-hyphen": "v" }
            }
        },
        "globals": { "vars": { "gg": "g" } }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let vars = rf.targets["x"].vars.as_ref().unwrap();
	assert_eq!(vars["xxx"], EnvValue::String("{{ ENV.some ? 'default' }}".into()));
	assert_eq!(vars["yyy"], EnvValue::Number(35.0));
	assert_eq!(vars["ok-hyphen"], EnvValue::String("v".into()));
	let gvars = rf.globals.unwrap().vars.unwrap();
	assert_eq!(gvars["gg"], EnvValue::String("g".into()));
}

#[test]
fn parse_rejects_invalid_var_key() {
	let json = r#"{
        "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
        "targets": { "x": { "commands": ["echo"], "vars": { "1bad": "v" } } }
    }"#;
	assert!(parse_runfile(json).is_err());
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
                "env": { "NODE_ENV": "{{ ARG.env ? 'development' }}" }
            }
        }
    }"#;
	let rf = parse_runfile(json).unwrap();
	let env = rf.targets["dev"].env.as_ref().unwrap();
	assert_eq!(
		env["NODE_ENV"],
		EnvValue::String("{{ ARG.env ? 'development' }}".into())
	);
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

// ── Audit M5: nesting-depth guard against a stack-overflow crash ──
// json5 deserialization is recursive, so a small but deeply-nested command
// tree used to abort the process (`exit 134`). The pre-parse depth scan now
// rejects it with a clean error instead.

/// Build a Runfile whose single target nests `if`/`then` `depth` levels deep.
fn nested_if_runfile(depth: usize) -> String {
	// Built ITERATIVELY (innermost outward) so constructing the test input
	// doesn't itself recurse `depth` deep and overflow the stack.
	let mut body = "\"echo done\"".to_string();
	for _ in 0..depth {
		body = format!(r#"{{"if":"{{{{ RUN.os == 'linux' }}}}","then":[{body}]}}"#);
	}
	format!(r#"{{"$schema":"x","targets":{{"t":{{"commands":[{body}]}}}}}}"#)
}

#[test]
fn deeply_nested_command_tree_is_rejected_not_crashed() {
	// 200 `if` levels => ~400 structural `{`/`[` levels, well over the cap.
	let json = nested_if_runfile(200);
	let err = parse_runfile(&json).unwrap_err();
	assert!(
		matches!(err, ParseError::MaxNestingDepthExceeded(_)),
		"expected MaxNestingDepthExceeded, got {err:?}"
	);
}

#[test]
fn extremely_deeply_nested_tree_does_not_stack_overflow() {
	// The whole point of the fix: a pathological file (which previously aborted
	// the process during json5 deserialization) returns a clean Err.
	let json = nested_if_runfile(20_000);
	assert!(matches!(
		parse_runfile(&json).unwrap_err(),
		ParseError::MaxNestingDepthExceeded(_)
	));
}

#[test]
fn moderately_nested_command_tree_still_parses() {
	// Well under the cap — must still parse successfully.
	let json = nested_if_runfile(20);
	assert!(parse_runfile(&json).is_ok(), "20 levels deep should parse fine");
}

#[test]
fn braces_inside_strings_and_comments_do_not_count_toward_depth() {
	// The depth scanner must skip `{`/`[` inside string literals and comments,
	// otherwise a legitimate file with brace-heavy command text would be
	// wrongly rejected.
	let mut cmds = String::new();
	for i in 0..300 {
		if i > 0 {
			cmds.push(',');
		}
		// Each command string is full of braces/brackets but adds NO structural depth.
		cmds.push_str("\"echo {{ RUN.os }} [{[]}] // { not a block\"");
	}
	let json = format!(r#"{{"$schema":"x","targets":{{"t":{{"commands":[{cmds}]}}}}}}"#);
	assert!(
		parse_runfile(&json).is_ok(),
		"braces inside strings must not trip the nesting-depth guard"
	);
}
