use runfile_parser::{is_internal_target_name, CommandSpec, Runfile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The top-level VS Code tasks.json structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VsCodeTasksFile {
	pub version: String,
	pub tasks: Vec<VsCodeTask>,
	/// Catch-all for other top-level fields we don't generate but want to preserve.
	#[serde(flatten)]
	pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A single VS Code task definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VsCodeTask {
	pub label: String,
	#[serde(rename = "type")]
	pub task_type: String,
	pub command: String,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub args: Vec<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub presentation: Option<VsCodeTaskPresentation>,
	/// Catch-all for other fields we don't generate but want to preserve.
	#[serde(flatten)]
	pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Presentation options for a VS Code task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VsCodeTaskPresentation {
	pub reveal: String,
	pub panel: String,
}

/// Result of merging generated tasks into an existing tasks file.
pub struct VsCodeMergeResult {
	pub added: Vec<String>,
	pub updated: Vec<String>,
}

/// Generate VS Code tasks for all targets in a Runfile.
///
/// Targets are skipped when their (possibly-globals-merged) metadata sets
/// `excludeFromGenerateCommand: true`, alongside the existing rule that
/// hides internal `_`-prefixed targets.
pub fn generate_vscode_tasks(runfile: &Runfile) -> Vec<VsCodeTask> {
	let mut target_names: Vec<&String> = runfile
		.targets
		.iter()
		.filter(|(n, spec)| !is_internal_target_name(n) && !spec.is_excluded_from_generate())
		.map(|(n, _)| n)
		.collect();
	target_names.sort();

	target_names
		.iter()
		.map(|name| {
			let spec = &runfile.targets[*name];
			let label = format!("run {name}");
			build_vscode_task(&label, name, spec)
		})
		.collect()
}

/// Merge generated tasks into an existing tasks file.
/// Existing tasks with matching labels are updated in place (preserving extra fields).
/// New tasks are appended.
pub fn merge_vscode_tasks(existing: &mut VsCodeTasksFile, generated: Vec<VsCodeTask>) -> VsCodeMergeResult {
	let mut existing_labels: HashMap<String, usize> = HashMap::new();
	for (i, task) in existing.tasks.iter().enumerate() {
		existing_labels.insert(task.label.clone(), i);
	}

	let mut added = Vec::new();
	let mut updated = Vec::new();

	for task in generated {
		if let Some(&idx) = existing_labels.get(&task.label) {
			let entry = &mut existing.tasks[idx];
			entry.task_type = task.task_type;
			entry.command = task.command;
			entry.args = task.args;
			entry.presentation = task.presentation;
			updated.push(task.label);
		} else {
			added.push(task.label.clone());
			existing.tasks.push(task);
		}
	}

	VsCodeMergeResult { added, updated }
}

fn build_vscode_task(label: &str, target_name: &str, spec: &CommandSpec) -> VsCodeTask {
	let mut uses_args = false;
	runfile_parser::walk_step_templates(&spec.commands, &mut |t| {
		if t.contains("{{ ARGS }}") || t.contains("{{ ARGS.") {
			uses_args = true;
		}
	});

	// `--stdin-args` lets the user fill in any unsupplied {{ ARGS.x }} /
	// {{ ENV.X }} / {{ FLAGS.x }} value at the integrated terminal prompt. It
	// composes with `${input:args}` (which still works for callers who
	// already know what to pass): provided values skip the prompt, missing
	// ones fire it. No-op when nothing's missing.
	let mut args = vec!["--stdin-args".to_string(), target_name.to_string()];
	if uses_args {
		args.push("${input:args}".to_string());
	}

	VsCodeTask {
		label: label.to_string(),
		task_type: "shell".to_string(),
		command: "run".to_string(),
		args,
		presentation: Some(VsCodeTaskPresentation {
			reveal: "always".to_string(),
			panel: "shared".to_string(),
		}),
		extra: serde_json::Map::new(),
	}
}
