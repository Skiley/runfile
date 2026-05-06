use runfile_parser::{is_internal_target_name, CommandSpec, Runfile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single Zed task definition (subset of fields we care about).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZedTask {
	pub label: String,
	pub command: String,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub args: Vec<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cwd: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none", rename = "allow_concurrent_runs")]
	pub allow_concurrent_runs: Option<bool>,
	/// Catch-all for other fields we don't generate but want to preserve.
	#[serde(flatten)]
	pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Result of merging generated tasks into an existing task list.
pub struct ZedMergeResult {
	pub added: Vec<String>,
	pub updated: Vec<String>,
}

/// Generate Zed tasks for all targets in a Runfile.
pub fn generate_zed_tasks(runfile: &Runfile) -> Vec<ZedTask> {
	let mut target_names: Vec<&String> = runfile.targets.keys().filter(|n| !is_internal_target_name(n)).collect();
	target_names.sort();

	target_names
		.iter()
		.map(|name| {
			let spec = &runfile.targets[*name];
			let label = format!("run {name}");
			build_zed_task(&label, name, spec)
		})
		.collect()
}

/// Merge generated tasks into an existing task list.
/// Existing tasks with matching labels are updated in place (preserving extra fields).
/// New tasks are appended.
pub fn merge_zed_tasks(existing: &mut Vec<ZedTask>, generated: Vec<ZedTask>) -> ZedMergeResult {
	let mut existing_labels: HashMap<String, usize> = HashMap::new();
	for (i, task) in existing.iter().enumerate() {
		existing_labels.insert(task.label.clone(), i);
	}

	let mut added = Vec::new();
	let mut updated = Vec::new();

	for task in generated {
		if let Some(&idx) = existing_labels.get(&task.label) {
			let entry = &mut existing[idx];
			entry.command = task.command;
			entry.args = task.args;
			entry.cwd = task.cwd;
			entry.allow_concurrent_runs = task.allow_concurrent_runs;
			updated.push(task.label);
		} else {
			added.push(task.label.clone());
			existing.push(task);
		}
	}

	ZedMergeResult { added, updated }
}

fn build_zed_task(label: &str, target_name: &str, spec: &CommandSpec) -> ZedTask {
	let mut uses_args = false;
	runfile_parser::walk_step_templates(&spec.commands, &mut |t| {
		if t.contains("$(ARGS)") || t.contains("$(ARGS.") {
			uses_args = true;
		}
	});

	// `--stdin-args` lets the user fill in any unsupplied $(ARGS.x) /
	// $(ENV.X) / $(FLAGS.x) value at the Zed terminal prompt. It composes
	// with `$ZED_CUSTOM_ARGS` (still respected for callers who know what to
	// pass): provided values skip the prompt, missing ones fire it. No-op
	// when nothing's missing.
	let mut args = vec!["--stdin-args".to_string(), target_name.to_string()];
	if uses_args {
		args.push("$ZED_CUSTOM_ARGS".to_string());
	}

	ZedTask {
		label: label.to_string(),
		command: "run".to_string(),
		args,
		cwd: Some("$ZED_WORKTREE_ROOT".to_string()),
		allow_concurrent_runs: if uses_args { Some(true) } else { None },
		extra: serde_json::Map::new(),
	}
}
