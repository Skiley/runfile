use super::*;

// ── VS Code merge / stale-prune tests ────────────────────────────────

fn vscode_task(label: &str, command: &str, args: Vec<&str>) -> VsCodeTask {
	VsCodeTask {
		label: label.into(),
		task_type: "shell".into(),
		command: command.into(),
		args: args.into_iter().map(String::from).collect(),
		detail: None,
		presentation: None,
		extra: serde_json::Map::new(),
	}
}

fn empty_vscode_tasks_file(tasks: Vec<VsCodeTask>) -> VsCodeTasksFile {
	VsCodeTasksFile {
		version: "2.0.0".into(),
		tasks,
		extra: serde_json::Map::new(),
	}
}

#[test]
fn vscode_merge_prunes_stale_ours_entries() {
	let mut existing = empty_vscode_tasks_file(vec![
		vscode_task("run dead", "run", vec!["--stdin-args", "dead"]),
		vscode_task("run build", "run", vec!["--stdin-args", "build"]),
	]);
	let generated = vec![vscode_task("run build", "run", vec!["--stdin-args", "build"])];

	let result = merge_vscode_tasks(&mut existing, generated);

	assert_eq!(result.removed, vec!["run dead"]);
	assert_eq!(result.updated, vec!["run build"]);
	assert!(result.added.is_empty());
	let labels: Vec<&str> = existing.tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build"]);
}

#[test]
fn vscode_merge_prunes_legacy_ours_entries() {
	// Pre-`--stdin-args` shape was `["<target>", ...]` with no `--stdin-args` arg.
	let mut existing = empty_vscode_tasks_file(vec![vscode_task("run legacy", "run", vec!["legacy"])]);
	let generated = vec![vscode_task("run build", "run", vec!["--stdin-args", "build"])];

	let result = merge_vscode_tasks(&mut existing, generated);

	assert_eq!(result.removed, vec!["run legacy"]);
	assert_eq!(result.added, vec!["run build"]);
	let labels: Vec<&str> = existing.tasks.iter().map(|t| t.label.as_str()).collect();
	assert_eq!(labels, vec!["run build"]);
}

#[test]
fn vscode_merge_preserves_foreign_entries() {
	let mut existing = empty_vscode_tasks_file(vec![
		// Different command — clearly not ours, even though label starts with "run ".
		vscode_task("run thing", "make", vec!["thing"]),
		// Label doesn't match our `"run <target>"` convention — not ours.
		vscode_task("custom task", "run", vec!["--stdin-args", "custom"]),
		// Args[1] doesn't match the label suffix — likely user-edited; not ours.
		vscode_task("run foo", "run", vec!["--stdin-args", "bar"]),
	]);
	let generated = vec![vscode_task("run build", "run", vec!["--stdin-args", "build"])];

	let result = merge_vscode_tasks(&mut existing, generated);

	assert!(result.removed.is_empty());
	assert_eq!(result.added, vec!["run build"]);
	assert_eq!(existing.tasks.len(), 4);
}

#[test]
fn vscode_merge_no_prune_when_everything_still_exists() {
	let mut existing = empty_vscode_tasks_file(vec![
		vscode_task("run build", "run", vec!["--stdin-args", "build"]),
		vscode_task("run test", "run", vec!["--stdin-args", "test"]),
	]);
	let generated = vec![
		vscode_task("run build", "run", vec!["--stdin-args", "build"]),
		vscode_task("run test", "run", vec!["--stdin-args", "test"]),
	];

	let result = merge_vscode_tasks(&mut existing, generated);

	assert!(result.removed.is_empty());
	assert!(result.added.is_empty());
	assert_eq!(result.updated.len(), 2);
}
