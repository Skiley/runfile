use crate::jetbrains::*;
use crate::vscode::*;
use crate::zed::*;
use runfile_parser::{CommandSpec, Metadata, Runfile};
use std::collections::HashMap;

fn make_runfile(targets: Vec<(&str, Vec<&str>)>) -> Runfile {
	let mut target_map = HashMap::new();
	for (name, commands) in targets {
		target_map.insert(
			name.to_string(),
			CommandSpec::new_shell(commands.into_iter().map(String::from).collect()),
		);
	}
	Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: target_map,
		globals: None,
		namespaces: Vec::new(),
	}
}

/// Build a runfile where one target has `excludeFromGenerateCommand: true` baked
/// into its `metadata`. Other targets are left as-is. Used to verify each
/// generator skips the flagged target.
fn make_runfile_with_excluded(targets: Vec<(&str, Vec<&str>)>, excluded: &[&str]) -> Runfile {
	let mut rf = make_runfile(targets);
	for name in excluded {
		let spec = rf.targets.get_mut(*name).expect("target exists");
		spec.metadata = Some(Metadata {
			exclude_from_generate_command: Some(true),
			extra: Default::default(),
		});
	}
	rf
}

// ── Zed tests ─────────────────────────────────────────────────────────

#[test]
fn zed_generate_basic_target() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let tasks = generate_zed_tasks(&runfile);
	assert_eq!(tasks.len(), 1);
	assert_eq!(tasks[0].label, "run build");
	assert_eq!(tasks[0].command, "run");
	assert_eq!(tasks[0].args, vec!["--stdin-args", "build"]);
	assert_eq!(tasks[0].cwd, Some("$ZED_WORKTREE_ROOT".to_string()));
	assert!(tasks[0].allow_concurrent_runs.is_none());
}

#[test]
fn zed_target_with_args_gets_custom_args() {
	let runfile = make_runfile(vec![("test", vec!["cargo test {{ ARGS }}"])]);
	let tasks = generate_zed_tasks(&runfile);
	assert_eq!(tasks[0].args, vec!["--stdin-args", "test", "$ZED_CUSTOM_ARGS"]);
	assert_eq!(tasks[0].allow_concurrent_runs, Some(true));
}

#[test]
fn zed_target_with_named_args_gets_custom_args() {
	let runfile = make_runfile(vec![("deploy", vec!["deploy --env={{ ARGS.env }}"])]);
	let tasks = generate_zed_tasks(&runfile);
	assert_eq!(tasks[0].args, vec!["--stdin-args", "deploy", "$ZED_CUSTOM_ARGS"]);
	assert_eq!(tasks[0].allow_concurrent_runs, Some(true));
}

#[test]
fn zed_targets_sorted_alphabetically() {
	let runfile = make_runfile(vec![
		("test", vec!["cargo test"]),
		("build", vec!["cargo build"]),
		("lint", vec!["cargo clippy"]),
	]);
	let tasks = generate_zed_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run lint", "run test"]);
}

#[test]
fn zed_merge_adds_new_tasks() {
	let mut existing: Vec<ZedTask> = Vec::new();
	let generated = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["build".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let result = merge_zed_tasks(&mut existing, generated);
	assert_eq!(result.added, vec!["run build"]);
	assert!(result.updated.is_empty());
	assert_eq!(existing.len(), 1);
}

#[test]
fn zed_merge_updates_existing_preserves_extra() {
	let mut extra = serde_json::Map::new();
	extra.insert("reveal".into(), serde_json::Value::String("always".into()));
	let mut existing = vec![ZedTask {
		label: "run build".into(),
		command: "old-command".into(),
		args: vec!["old-arg".into()],
		cwd: None,
		allow_concurrent_runs: None,
		extra: extra.clone(),
	}];
	let generated = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["build".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let result = merge_zed_tasks(&mut existing, generated);
	assert!(result.added.is_empty());
	assert_eq!(result.updated, vec!["run build"]);
	assert_eq!(existing[0].command, "run");
	assert_eq!(existing[0].args, vec!["build"]);
	assert_eq!(existing[0].cwd, Some("$ZED_WORKTREE_ROOT".into()));
	// Extra fields preserved
	assert_eq!(existing[0].extra, extra);
}

#[test]
fn zed_merge_mixed_add_and_update() {
	let mut existing = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["build".into()],
		cwd: None,
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let generated = vec![
		ZedTask {
			label: "run build".into(),
			command: "run".into(),
			args: vec!["build".into()],
			cwd: Some("$ZED_WORKTREE_ROOT".into()),
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
		ZedTask {
			label: "run test".into(),
			command: "run".into(),
			args: vec!["test".into()],
			cwd: Some("$ZED_WORKTREE_ROOT".into()),
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
	];
	let result = merge_zed_tasks(&mut existing, generated);
	assert_eq!(result.updated, vec!["run build"]);
	assert_eq!(result.added, vec!["run test"]);
	assert_eq!(existing.len(), 2);
}

// ── JetBrains tests ───────────────────────────────────────────────────

#[test]
fn jetbrains_generate_basic_target() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let configs = generate_jetbrains_configs(&runfile);
	assert_eq!(configs.len(), 1);
	assert_eq!(configs[0].config_name, "Build");
	assert_eq!(configs[0].target_name, "build");
	assert_eq!(configs[0].file_name, "Runfile_build.run.xml");
	assert!(configs[0].xml.contains("name=\"Build\""));
	assert!(configs[0].xml.contains("value=\"run --stdin-args build\""));
	assert!(configs[0].xml.contains("ShConfigurationType"));
	assert!(configs[0].xml.contains("$PROJECT_DIR$"));
}

#[test]
fn jetbrains_execute_in_terminal_is_false() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let configs = generate_jetbrains_configs(&runfile);
	assert!(configs[0].xml.contains("name=\"EXECUTE_IN_TERMINAL\" value=\"false\""));
}

#[test]
fn jetbrains_colon_in_target_name_becomes_capitalized_words() {
	let runfile = make_runfile(vec![("build:infrastructure", vec!["cargo build"])]);
	let configs = generate_jetbrains_configs(&runfile);
	assert_eq!(configs[0].config_name, "Build Infrastructure");
	assert_eq!(configs[0].target_name, "build:infrastructure");
	assert_eq!(configs[0].file_name, "Runfile_build_infrastructure.run.xml");
	assert!(configs[0].xml.contains("name=\"Build Infrastructure\""));
	// SCRIPT_TEXT keeps the original target name so `run` resolves it correctly.
	assert!(configs[0]
		.xml
		.contains("value=\"run --stdin-args build:infrastructure\""));
}

#[test]
fn jetbrains_punctuation_variants_become_whitespace() {
	let runfile = make_runfile(vec![
		("test.unit.fast", vec!["cargo test"]),
		("ci/build", vec!["cargo build"]),
		("deploy,prod", vec!["deploy"]),
		("build_release", vec!["cargo build --release"]),
	]);
	let configs = generate_jetbrains_configs(&runfile);
	let names: Vec<&str> = configs.iter().map(|c| c.config_name.as_str()).collect();
	// Sorted alphabetically by target name: "build_release", "ci/build", "deploy,prod", "test.unit.fast"
	assert_eq!(
		names,
		vec!["Build Release", "Ci Build", "Deploy Prod", "Test Unit Fast"]
	);
}

#[test]
fn jetbrains_targets_sorted_alphabetically() {
	let runfile = make_runfile(vec![("test", vec!["cargo test"]), ("build", vec!["cargo build"])]);
	let configs = generate_jetbrains_configs(&runfile);
	assert_eq!(configs[0].config_name, "Build");
	assert_eq!(configs[1].config_name, "Test");
}

#[test]
fn jetbrains_check_ours_current_format() {
	let xml = r#"<configuration default="false" name="Build" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="run --stdin-args build" />"#;
	assert!(matches!(
		check_existing_jetbrains_config(xml, "Build", "build"),
		JetBrainsConfigCheck::Ours
	));
}

#[test]
fn jetbrains_check_ours_pre_stdin_args_format() {
	// Files generated before the `--stdin-args` switch still have the bare
	// `run <target>` form. Treat them as ours so re-running the generator
	// upgrades them in place.
	let xml = r#"<configuration default="false" name="Build" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="run build" />"#;
	assert!(matches!(
		check_existing_jetbrains_config(xml, "Build", "build"),
		JetBrainsConfigCheck::Ours
	));
}

#[test]
fn jetbrains_check_recognizes_legacy_format() {
	// Files generated by even older versions still carry the "Runfile / <target>" prefix; treat them as ours
	// so re-running the generator overwrites them in place rather than skipping them as foreign.
	let xml = r#"<configuration default="false" name="Runfile / build" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="run build" />"#;
	assert!(matches!(
		check_existing_jetbrains_config(xml, "Build", "build"),
		JetBrainsConfigCheck::Ours
	));
}

#[test]
fn jetbrains_check_foreign_name() {
	let xml = r#"<configuration default="false" name="My Custom Build" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="run --stdin-args build" />"#;
	assert!(matches!(
		check_existing_jetbrains_config(xml, "Build", "build"),
		JetBrainsConfigCheck::Foreign(_)
	));
}

#[test]
fn jetbrains_check_foreign_script() {
	let xml = r#"<configuration default="false" name="Build" type="ShConfigurationType">
    <option name="SCRIPT_TEXT" value="make build" />"#;
	assert!(matches!(
		check_existing_jetbrains_config(xml, "Build", "build"),
		JetBrainsConfigCheck::Foreign(_)
	));
}

#[test]
fn jetbrains_xml_is_valid_structure() {
	let runfile = make_runfile(vec![("test", vec!["cargo test"])]);
	let configs = generate_jetbrains_configs(&runfile);
	let xml = &configs[0].xml;
	assert!(xml.starts_with("<component"));
	assert!(xml.trim_end().ends_with("</component>"));
	assert!(xml.contains("<method v=\"2\" />"));
	assert!(xml.contains("<envs />"));
	assert!(xml.contains("EXECUTE_IN_TERMINAL"));
}

// ── Additional coverage tests ────────────────────────────────────────

#[test]
fn zed_no_targets_empty_result() {
	let runfile = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: HashMap::new(),
		globals: None,
		namespaces: Vec::new(),
	};
	let tasks = generate_zed_tasks(&runfile);
	assert!(tasks.is_empty());
}

#[test]
fn zed_task_without_args_no_concurrent() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let tasks = generate_zed_tasks(&runfile);
	assert!(tasks[0].allow_concurrent_runs.is_none());
}

#[test]
fn zed_task_serialization_roundtrip() {
	let task = ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["build".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	};
	let json = serde_json::to_string(&task).unwrap();
	let deserialized: ZedTask = serde_json::from_str(&json).unwrap();
	assert_eq!(deserialized.label, "run build");
	assert_eq!(deserialized.command, "run");
	assert_eq!(deserialized.args, vec!["build"]);
	assert_eq!(deserialized.cwd, Some("$ZED_WORKTREE_ROOT".into()));
	assert!(deserialized.allow_concurrent_runs.is_none());
}

#[test]
fn zed_merge_empty_lists() {
	let mut existing: Vec<ZedTask> = Vec::new();
	let generated: Vec<ZedTask> = Vec::new();
	let result = merge_zed_tasks(&mut existing, generated);
	assert!(result.added.is_empty());
	assert!(result.updated.is_empty());
	assert!(existing.is_empty());
}

#[test]
fn zed_merge_no_overlap() {
	let mut existing = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["build".into()],
		cwd: None,
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let generated = vec![ZedTask {
		label: "run test".into(),
		command: "run".into(),
		args: vec!["test".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let result = merge_zed_tasks(&mut existing, generated);
	assert_eq!(result.added, vec!["run test"]);
	assert!(result.updated.is_empty());
	assert_eq!(existing.len(), 2);
}

#[test]
fn jetbrains_no_targets_empty_result() {
	let runfile = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: HashMap::new(),
		globals: None,
		namespaces: Vec::new(),
	};
	let configs = generate_jetbrains_configs(&runfile);
	assert!(configs.is_empty());
}

#[test]
fn jetbrains_multiple_targets_sorted() {
	let runfile = make_runfile(vec![
		("test", vec!["cargo test"]),
		("build", vec!["cargo build"]),
		("lint", vec!["cargo clippy"]),
	]);
	let configs = generate_jetbrains_configs(&runfile);
	assert_eq!(configs.len(), 3);
	assert_eq!(configs[0].config_name, "Build");
	assert_eq!(configs[1].config_name, "Lint");
	assert_eq!(configs[2].config_name, "Test");
}

#[test]
fn jetbrains_target_with_multiple_colons() {
	let runfile = make_runfile(vec![("test:unit:fast", vec!["cargo test"])]);
	let configs = generate_jetbrains_configs(&runfile);
	assert_eq!(configs[0].config_name, "Test Unit Fast");
	assert_eq!(configs[0].file_name, "Runfile_test_unit_fast.run.xml");
	assert!(configs[0].xml.contains("run --stdin-args test:unit:fast"));
}

#[test]
fn jetbrains_check_completely_different_file() {
	let xml = r#"<configuration default="false" name="Something Else" type="MavenRunConfiguration">
    <option name="WORKING_DIRECTORY" value="$PROJECT_DIR$" />"#;
	assert!(matches!(
		check_existing_jetbrains_config(xml, "Build", "build"),
		JetBrainsConfigCheck::Foreign(_)
	));
}

#[test]
fn zed_task_with_named_args_pattern() {
	// {{ ARGS.name }} should also trigger custom args
	let runfile = make_runfile(vec![("deploy", vec!["echo {{ ARGS.env ? prod }}"])]);
	let tasks = generate_zed_tasks(&runfile);
	assert!(tasks[0].args.contains(&"$ZED_CUSTOM_ARGS".to_string()));
	assert_eq!(tasks[0].allow_concurrent_runs, Some(true));
}

// ── Internal targets are excluded from generators ─────────────────────

#[test]
fn zed_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", vec!["cargo build"]),
		("_setup", vec!["echo internal"]),
		("test", vec!["cargo test"]),
	]);
	let tasks = generate_zed_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

#[test]
fn jetbrains_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", vec!["cargo build"]),
		("_setup", vec!["echo internal"]),
		("test", vec!["cargo test"]),
	]);
	let configs = generate_jetbrains_configs(&runfile);
	let names: Vec<&str> = configs.iter().map(|c| c.config_name.as_str()).collect();
	assert_eq!(names, vec!["Build", "Test"]);
}

#[test]
fn vscode_excludes_internal_targets() {
	let runfile = make_runfile(vec![
		("build", vec!["cargo build"]),
		("_setup", vec!["echo internal"]),
		("test", vec!["cargo test"]),
	]);
	let tasks = generate_vscode_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

// ── --stdin-args insertion across generators ──────────────────────────

#[test]
fn vscode_args_include_stdin_args_flag() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let tasks = generate_vscode_tasks(&runfile);
	assert_eq!(tasks[0].command, "run");
	assert_eq!(tasks[0].args, vec!["--stdin-args", "build"]);
}

#[test]
fn vscode_args_target_using_args_keeps_input_args_after_stdin_args() {
	// `${input:args}` still works for callers who know what to pass; missing
	// values are then prompted in the integrated terminal.
	let runfile = make_runfile(vec![("test", vec!["cargo test {{ ARGS }}"])]);
	let tasks = generate_vscode_tasks(&runfile);
	assert_eq!(tasks[0].args, vec!["--stdin-args", "test", "${input:args}"]);
}

#[test]
fn jetbrains_xml_includes_stdin_args_flag() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let xml = &generate_jetbrains_configs(&runfile)[0].xml;
	assert!(xml.contains("value=\"run --stdin-args build\""));
}

// ── excludeFromGenerateCommand metadata flag ──────────────────────────

#[test]
fn zed_excludes_flagged_targets() {
	let runfile = make_runfile_with_excluded(
		vec![
			("build", vec!["cargo build"]),
			("internal-helper", vec!["echo skip me"]),
			("test", vec!["cargo test"]),
		],
		&["internal-helper"],
	);
	let tasks = generate_zed_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

#[test]
fn jetbrains_excludes_flagged_targets() {
	let runfile = make_runfile_with_excluded(
		vec![
			("build", vec!["cargo build"]),
			("private", vec!["echo skip me"]),
			("test", vec!["cargo test"]),
		],
		&["private"],
	);
	let configs = generate_jetbrains_configs(&runfile);
	let names: Vec<&str> = configs.iter().map(|c| c.config_name.as_str()).collect();
	assert_eq!(names, vec!["Build", "Test"]);
}

#[test]
fn vscode_excludes_flagged_targets() {
	let runfile = make_runfile_with_excluded(
		vec![
			("build", vec!["cargo build"]),
			("private", vec!["echo skip me"]),
			("test", vec!["cargo test"]),
		],
		&["private"],
	);
	let tasks = generate_vscode_tasks(&runfile);
	let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build", "run test"]);
}

#[test]
fn excluded_flag_default_false_keeps_targets() {
	// Metadata present but excludeFromGenerateCommand omitted → not excluded.
	let mut rf = make_runfile(vec![("build", vec!["cargo build"])]);
	rf.targets.get_mut("build").unwrap().metadata = Some(Metadata {
		exclude_from_generate_command: None,
		extra: Default::default(),
	});
	assert_eq!(generate_zed_tasks(&rf).len(), 1);
	assert_eq!(generate_vscode_tasks(&rf).len(), 1);
	assert_eq!(generate_jetbrains_configs(&rf).len(), 1);
}

#[test]
fn excluded_flag_explicit_false_keeps_targets() {
	let mut rf = make_runfile(vec![("build", vec!["cargo build"])]);
	rf.targets.get_mut("build").unwrap().metadata = Some(Metadata {
		exclude_from_generate_command: Some(false),
		extra: Default::default(),
	});
	assert_eq!(generate_zed_tasks(&rf).len(), 1);
	assert_eq!(generate_vscode_tasks(&rf).len(), 1);
	assert_eq!(generate_jetbrains_configs(&rf).len(), 1);
}
