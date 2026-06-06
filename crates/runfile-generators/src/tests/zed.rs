use super::*;

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
	let runfile = make_runfile(vec![("deploy", vec!["deploy --env={{ ARG.env }}"])]);
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
fn zed_merge_prunes_stale_ours_entries() {
	let mut existing = vec![
		ZedTask {
			label: "run dead".into(),
			command: "run".into(),
			args: vec!["--stdin-args".into(), "dead".into()],
			cwd: Some("$ZED_WORKTREE_ROOT".into()),
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
		ZedTask {
			label: "run build".into(),
			command: "run".into(),
			args: vec!["--stdin-args".into(), "build".into()],
			cwd: Some("$ZED_WORKTREE_ROOT".into()),
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
	];
	let generated = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["--stdin-args".into(), "build".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let result = merge_zed_tasks(&mut existing, generated);
	assert_eq!(result.removed, vec!["run dead"]);
	assert_eq!(result.updated, vec!["run build"]);
	assert!(result.added.is_empty());
	let labels: Vec<&str> = existing.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build"]);
}

#[test]
fn zed_merge_prunes_legacy_ours_entries() {
	let mut existing = vec![ZedTask {
		label: "run legacy".into(),
		command: "run".into(),
		args: vec!["legacy".into()],
		cwd: None,
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let generated = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["--stdin-args".into(), "build".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let result = merge_zed_tasks(&mut existing, generated);
	assert_eq!(result.removed, vec!["run legacy"]);
	assert_eq!(result.added, vec!["run build"]);
}

#[test]
fn zed_merge_preserves_foreign_entries() {
	let mut existing = vec![
		// Different command — not ours.
		ZedTask {
			label: "run thing".into(),
			command: "make".into(),
			args: vec!["thing".into()],
			cwd: None,
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
		// Label doesn't fit our `run <target>` convention.
		ZedTask {
			label: "custom".into(),
			command: "run".into(),
			args: vec!["--stdin-args".into(), "custom".into()],
			cwd: None,
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
		// Args[1] != label suffix — likely user-edited; not ours.
		ZedTask {
			label: "run foo".into(),
			command: "run".into(),
			args: vec!["--stdin-args".into(), "bar".into()],
			cwd: None,
			allow_concurrent_runs: None,
			extra: serde_json::Map::new(),
		},
	];
	let generated = vec![ZedTask {
		label: "run build".into(),
		command: "run".into(),
		args: vec!["--stdin-args".into(), "build".into()],
		cwd: Some("$ZED_WORKTREE_ROOT".into()),
		allow_concurrent_runs: None,
		extra: serde_json::Map::new(),
	}];
	let result = merge_zed_tasks(&mut existing, generated);
	assert!(result.removed.is_empty());
	assert_eq!(result.added, vec!["run build"]);
	assert_eq!(existing.len(), 4);
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
