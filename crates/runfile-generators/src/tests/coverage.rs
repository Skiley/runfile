use super::*;

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
	// User-authored task (different `command`) — must NOT be flagged as ours by
	// `is_zed_task_ours`, so it survives the stale-prune pass and the new task is appended.
	let mut existing = vec![ZedTask {
		label: "user task".into(),
		command: "make".into(),
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
	assert!(result.removed.is_empty());
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
	// {{ ARG.name }} should also trigger custom args
	let runfile = make_runfile(vec![("deploy", vec!["echo {{ ARG.env ? prod }}"])]);
	let tasks = generate_zed_tasks(&runfile);
	assert!(tasks[0].args.contains(&"$ZED_CUSTOM_ARGS".to_string()));
	assert_eq!(tasks[0].allow_concurrent_runs, Some(true));
}
