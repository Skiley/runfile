use crate::editorconfig::{serialize_json_with_indent, EditorConfigProps};
use runfile_parser::{is_internal_target_name, CommandSpec, Runfile};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
	/// Labels of stale tasks that looked like ours but no longer correspond to a Runfile target.
	pub removed: Vec<String>,
}

/// Generate Zed tasks for all targets in a Runfile.
///
/// Targets are skipped when their (possibly-globals-merged) metadata sets
/// `excludeFromGenerateCommand: true`, alongside the existing rule that
/// hides internal `_`-prefixed targets.
pub fn generate_zed_tasks(runfile: &Runfile) -> Vec<ZedTask> {
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
			build_zed_task(&label, name, spec)
		})
		.collect()
}

/// Merge generated tasks into an existing task list.
/// Existing tasks with matching labels are updated in place (preserving extra fields).
/// Stale tasks that look like ones we generated (via [`is_zed_task_ours`]) but are no
/// longer in the generated set are pruned, so retired Runfile targets stop leaving zombie
/// entries in `.zed/tasks.json`. User-authored tasks that don't pass the ownership check
/// are left untouched.
/// New tasks are appended.
pub fn merge_zed_tasks(existing: &mut Vec<ZedTask>, generated: Vec<ZedTask>) -> ZedMergeResult {
	let generated_labels: HashSet<&str> = generated.iter().map(|t| t.label.as_str()).collect();

	let mut removed = Vec::new();
	let mut kept = Vec::with_capacity(existing.len());
	for task in existing.drain(..) {
		if is_zed_task_ours(&task) && !generated_labels.contains(task.label.as_str()) {
			removed.push(task.label.clone());
		} else {
			kept.push(task);
		}
	}
	*existing = kept;

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

	ZedMergeResult {
		added,
		updated,
		removed,
	}
}

/// Render the Zed task list to the exact bytes to write to `.zed/tasks.json`, formatted per the
/// resolved EditorConfig properties: indentation drives the JSON pretty-printer, and the remaining
/// settings (line endings, trailing whitespace, final newline, BOM) are applied to the serialized
/// text. With an empty [`EditorConfigProps`] this reproduces the previous output (2-space indent,
/// LF, no trailing newline).
pub fn render_zed_tasks(tasks: &[ZedTask], props: &EditorConfigProps) -> Result<Vec<u8>, serde_json::Error> {
	let indent = props.indent_unit();
	let json = serialize_json_with_indent(tasks, indent.as_deref())?;
	Ok(props.apply(&json))
}

/// Decide whether an existing task is one we'd have generated for the same target.
///
/// Matches both the current invocation shape (`["--stdin-args", "<target>", ...]`) and the
/// pre-`--stdin-args` shape (`["<target>", ...]`) so we recognise our own historical output.
/// Anchored on `command == "run"` plus `label == format!("run {target}")` plus the target
/// name appearing as the expected arg — tight enough that hand-authored tasks with custom
/// commands or mismatched labels won't be flagged as ours.
fn is_zed_task_ours(task: &ZedTask) -> bool {
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

fn build_zed_task(label: &str, target_name: &str, spec: &CommandSpec) -> ZedTask {
	let mut uses_args = false;
	runfile_parser::walk_step_templates(&spec.commands, &mut |t| {
		if t.contains("{{ ARGS }}") || t.contains("{{ ARG.") {
			uses_args = true;
		}
	});

	// `--stdin-args` lets the user fill in any unsupplied {{ ARG.x }} /
	// {{ ENV.X }} / {{ FLAG.x }} value at the Zed terminal prompt. It composes
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
