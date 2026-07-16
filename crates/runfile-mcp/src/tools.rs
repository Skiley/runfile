use runfile_parser::{CommandSpec, EnvValue, Runfile, is_internal_target_name};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A serializable tool definition for --inspect output.
/// This is our own type, decoupled from rmcp's Tool struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
	pub name: String,
	pub description: String,
	#[serde(rename = "inputSchema")]
	pub input_schema: serde_json::Value,
}

/// Result of scanning a target's commands for argument patterns.
struct ArgScan {
	/// True if any command uses `{{ ARGS }}` (bare positional).
	uses_positional: bool,
	/// Keys from `{{ ARG.key }}` patterns (string-valued named arguments).
	arg_keys: HashSet<String>,
	/// Keys from `{{ FLAG.key }}` patterns (boolean flags).
	flag_keys: HashSet<String>,
	/// Keys that appear without a `?` default (required arguments).
	required_keys: HashSet<String>,
}

/// Collect all strings from a CommandSpec that could contain argument placeholders.
/// Walks `commands` (including nested if/for/when/@target) and env value strings.
fn collect_scannable_strings(spec: &CommandSpec) -> Vec<String> {
	let mut strings = Vec::new();
	runfile_parser::walk_step_templates(&spec.commands, &mut |t| strings.push(t.to_string()));
	collect_env_strings(&spec.env, &mut strings);
	strings
}

/// Collect string values from an optional env map.
fn collect_env_strings(env: &Option<std::collections::HashMap<String, EnvValue>>, out: &mut Vec<String>) {
	if let Some(env) = env {
		for val in env.values() {
			if let EnvValue::String(s) = val {
				out.push(s.clone());
			}
		}
	}
}

/// Scan strings for `{{ ARGS }}`, `{{ ARG.key }}`, and `{{ FLAG.key }}` patterns.
fn scan_arg_patterns(strings: &[String]) -> ArgScan {
	let mut scan = ArgScan {
		uses_positional: false,
		arg_keys: HashSet::new(),
		flag_keys: HashSet::new(),
		required_keys: HashSet::new(),
	};

	for s in strings {
		scan_one(s, &mut scan);
	}

	scan
}

fn scan_one(input: &str, scan: &mut ArgScan) {
	let bytes = input.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		// Skip escapes so `\{{` literals don't register as substitutions.
		if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'{') && bytes.get(i + 2) == Some(&b'{') {
			i += 3;
			continue;
		}
		if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'}') && bytes.get(i + 2) == Some(&b'}') {
			i += 3;
			continue;
		}
		if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'{') {
			let body_start = i + 2;
			if let Some(rel_close) = input[body_start..].find("}}") {
				let content = &input[body_start..body_start + rel_close];
				let trimmed = content.trim();
				if trimmed == "ARGS" {
					scan.uses_positional = true;
				}
				// A chain may have multiple `ARG.key` references
				// (e.g. `{{ ARG.a ? ARG.b }}`); collect all of them. Required-vs-default
				// is tracked separately: if the substitution body contains any `?`, every
				// ARGS reference inside it becomes optional (the chain has a fallback).
				let has_default = trimmed.contains('?');
				collect_in_substitution(trimmed, has_default, scan);
				i = body_start + rel_close + 2;
				continue;
			}
			break;
		}
		i += 1;
	}
}

fn collect_in_substitution(inner: &str, has_default: bool, scan: &mut ArgScan) {
	let bytes = inner.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		let s = &inner[i..];
		if let Some(rest) = s.strip_prefix("ARG.") {
			let key: String = rest
				.chars()
				.take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
				.collect();
			if !key.is_empty() {
				scan.arg_keys.insert(key.clone());
				if !has_default {
					scan.required_keys.insert(key);
				}
			}
		} else if let Some(rest) = s.strip_prefix("FLAG.") {
			let key: String = rest
				.chars()
				.take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
				.collect();
			if !key.is_empty() {
				scan.flag_keys.insert(key);
			}
		}
		i += 1;
	}
}

/// Build tool definitions for all targets in a Runfile.
///
/// Security: env_files, env, and other sensitive fields are intentionally
/// excluded from the output.
pub fn build_tool_defs(runfile: &Runfile) -> Vec<ToolDef> {
	let mut target_names: Vec<&String> = runfile.targets.keys().filter(|n| !is_internal_target_name(n)).collect();
	target_names.sort();

	target_names
		.iter()
		.map(|name| {
			let spec = &runfile.targets[*name];

			let description = spec
				.description
				.clone()
				.unwrap_or_else(|| format!("Run the \"{name}\" target"));

			let strings = collect_scannable_strings(spec);
			let scan = scan_arg_patterns(&strings);
			let has_any_args = scan.uses_positional || !scan.arg_keys.is_empty() || !scan.flag_keys.is_empty();

			let input_schema = if !has_any_args {
				serde_json::json!({
					"type": "object",
					"properties": {}
				})
			} else {
				let mut properties = serde_json::Map::new();
				let mut required: Vec<String> = Vec::new();

				// Named string arguments from {{ ARG.key }} patterns
				let mut sorted_args: Vec<&String> = scan.arg_keys.iter().collect();
				sorted_args.sort();
				for key in sorted_args {
					properties.insert(
						key.clone(),
						serde_json::json!({
							"type": "string",
							"description": format!("Value for the --{key} argument")
						}),
					);
					if scan.required_keys.contains(key) {
						required.push(key.clone());
					}
				}

				// Boolean flags from {{ FLAG.key }} patterns (skip if already in arg_keys)
				let mut sorted_flags: Vec<&String> =
					scan.flag_keys.iter().filter(|k| !scan.arg_keys.contains(*k)).collect();
				sorted_flags.sort();
				for key in sorted_flags {
					properties.insert(
						key.clone(),
						serde_json::json!({
							"type": "boolean",
							"description": format!("Enable the --{key} flag")
						}),
					);
				}

				// Positional args array for {{ ARGS }} usage
				if scan.uses_positional {
					properties.insert(
						"args".to_string(),
						serde_json::json!({
							"type": "array",
							"items": { "type": "string" },
							"description": "Additional positional arguments"
						}),
					);
				}

				let mut schema = serde_json::json!({
					"type": "object",
					"properties": serde_json::Value::Object(properties)
				});

				if !required.is_empty() {
					required.sort();
					schema
						.as_object_mut()
						.unwrap()
						.insert("required".to_string(), serde_json::json!(required));
				}

				schema
			};

			ToolDef {
				name: name.to_string(),
				description,
				input_schema,
			}
		})
		.collect()
}

/// Serialize tool definitions as pretty JSON for --inspect output.
pub fn inspect_json(runfile: &Runfile) -> String {
	let tools = build_tool_defs(runfile);
	serde_json::to_string_pretty(&tools).expect("tool defs are always serializable")
}
