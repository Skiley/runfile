use crate::editorconfig::{EditorConfigProps, serialize_json_with_indent};
use runfile_parser::{CommandSpec, Runfile, is_internal_target_name};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// The top-level VS Code tasks.json structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VsCodeTasksFile {
	pub version: String,
	/// The include-namespaces present in the generated set (the same list `run :list`
	/// groups by), so consumers like the Runfile VS Code extension can bucket tasks by
	/// *real* namespace instead of guessing from colons in target names. Emitted only in
	/// `--stdout` mode; omitted when empty so on-disk `tasks.json` files stay clean.
	#[serde(default, rename = "runfileNamespaces", skip_serializing_if = "Vec::is_empty")]
	pub namespaces: Vec<String>,
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
	/// Human-readable description shown in the Run Task quick-pick (and by clients
	/// that surface tasks, like the Runfile sidebar). Sourced from the target's
	/// `description`; omitted entirely when the target has none.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub detail: Option<String>,
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
	/// Labels of stale tasks that looked like ours but no longer correspond to a Runfile target.
	pub removed: Vec<String>,
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
/// Stale tasks that look like ones we generated (via [`is_vscode_task_ours`]) but are no
/// longer in the generated set are pruned, so retired Runfile targets stop leaving zombie
/// entries in `.vscode/tasks.json`. User-authored tasks that don't pass the ownership
/// check are left untouched.
/// New tasks are appended.
pub fn merge_vscode_tasks(existing: &mut VsCodeTasksFile, generated: Vec<VsCodeTask>) -> VsCodeMergeResult {
	let generated_labels: HashSet<&str> = generated.iter().map(|t| t.label.as_str()).collect();

	let mut removed = Vec::new();
	let mut kept = Vec::with_capacity(existing.tasks.len());
	for task in existing.tasks.drain(..) {
		if is_vscode_task_ours(&task) && !generated_labels.contains(task.label.as_str()) {
			removed.push(task.label.clone());
		} else {
			kept.push(task);
		}
	}
	existing.tasks = kept;

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
			entry.detail = task.detail;
			entry.presentation = task.presentation;
			updated.push(task.label);
		} else {
			added.push(task.label.clone());
			existing.tasks.push(task);
		}
	}

	VsCodeMergeResult {
		added,
		updated,
		removed,
	}
}

/// Render a VS Code tasks file to the exact bytes to write to `.vscode/tasks.json`, formatted
/// per the resolved EditorConfig properties: indentation drives the JSON pretty-printer, and the
/// remaining settings (line endings, trailing whitespace, final newline, BOM) are applied to the
/// serialized text. With an empty [`EditorConfigProps`] this reproduces the previous output
/// (2-space indent, LF, no trailing newline).
pub fn render_vscode_tasks(file: &VsCodeTasksFile, props: &EditorConfigProps) -> Result<Vec<u8>, serde_json::Error> {
	let indent = props.indent_unit();
	let json = serialize_json_with_indent(file, indent.as_deref())?;
	Ok(props.apply(&json))
}

/// Decide whether an existing task is one we'd have generated for the same target.
///
/// Matches both the current invocation shape (`["--stdin-args", "<target>", ...]`) and the
/// pre-`--stdin-args` shape (`["<target>", ...]`) so we recognise our own historical output.
/// The check anchors on `command == "run"` plus `label == format!("run {target}")` plus the
/// target name appearing as the expected arg — tight enough that hand-authored tasks with
/// custom commands or mismatched labels won't be flagged as ours.
fn is_vscode_task_ours(task: &VsCodeTask) -> bool {
	if task.command != "run" {
		return false;
	}
	let Some(target) = task.label.strip_prefix("run ") else {
		return false;
	};
	if task.args.len() >= 2 && task.args[0] == "--stdin-args" && task.args[1] == target {
		return true;
	}
	if !task.args.is_empty() && task.args[0] == target {
		return true;
	}
	false
}

fn build_vscode_task(label: &str, target_name: &str, spec: &CommandSpec) -> VsCodeTask {
	let mut uses_args = false;
	runfile_parser::walk_step_templates(&spec.commands, &mut |t| {
		if t.contains("{{ ARGS }}") || t.contains("{{ ARG.") {
			uses_args = true;
		}
	});

	// `--stdin-args` lets the user fill in any unsupplied {{ ARG.x }} /
	// {{ ENV.X }} / {{ FLAG.x }} value at the integrated terminal prompt. It
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
		detail: spec.description.clone(),
		presentation: Some(VsCodeTaskPresentation {
			reveal: "always".to_string(),
			panel: "shared".to_string(),
		}),
		extra: serde_json::Map::new(),
	}
}
