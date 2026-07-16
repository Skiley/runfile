use runfile_parser::{CommandSpec, CommandStep, EnvValue, IfStep};

/// Convert a `Vec<String>` of raw shell command strings into `Vec<CommandStep>`.
fn to_steps(commands: Vec<String>) -> Vec<CommandStep> {
	commands.into_iter().map(CommandStep::Shell).collect()
}
use std::collections::{HashMap, HashSet};

/// Result of converting package.json scripts into Runfile targets.
pub struct PackageJsonConversion {
	/// Converted targets keyed by target name.
	pub targets: HashMap<String, CommandSpec>,
	/// Script names that were skipped because a target with that name already exists.
	pub skipped: Vec<String>,
}

/// npm lifecycle scripts that are automatically triggered by npm and should never
/// be converted into standalone Runfile targets. These run during install, publish,
/// pack, uninstall, or shrinkwrap — not via `npm run <name>`.
const LIFECYCLE_SCRIPTS: &[&str] = &[
	"dependencies",
	"install",
	"postinstall",
	"postpack",
	"postpublish",
	"postshrinkwrap",
	"postuninstall",
	"preinstall",
	"prepack",
	"prepare",
	"prepublish",
	"prepublishOnly",
	"preshrinkwrap",
	"preuninstall",
	"shrinkwrap",
	"uninstall",
];

/// Convert package.json scripts into Runfile targets.
///
/// `scripts` is a map of script_name → script_command from package.json.
/// `existing_targets` is the set of target names already in the Runfile (to avoid collisions).
pub fn convert_package_json_scripts(
	scripts: &serde_json::Map<String, serde_json::Value>,
	existing_targets: &HashSet<String>,
) -> PackageJsonConversion {
	let mut targets = HashMap::new();
	let mut skipped = Vec::new();

	// Collect all convertible script names for cross-referencing
	let all_names: HashSet<&str> = scripts
		.keys()
		.map(|s| s.as_str())
		.filter(|s| !LIFECYCLE_SCRIPTS.contains(s))
		.collect();

	// Identify pre/post hooks: if "pretest" exists and "test" also exists,
	// "pretest" becomes a before step on "test" (not a standalone target).
	let mut pre_hooks: HashMap<String, String> = HashMap::new();
	let mut post_hooks: HashMap<String, String> = HashMap::new();
	let mut hook_names: HashSet<String> = HashSet::new();

	for (name, value) in scripts {
		if LIFECYCLE_SCRIPTS.contains(&name.as_str()) {
			continue;
		}
		let cmd = match value.as_str() {
			Some(s) => s,
			None => continue,
		};

		if let Some(base) = name.strip_prefix("pre") {
			if !base.is_empty() && all_names.contains(base) {
				pre_hooks.insert(base.to_string(), cmd.to_string());
				hook_names.insert(name.clone());
			}
		} else if let Some(base) = name.strip_prefix("post")
			&& !base.is_empty()
			&& all_names.contains(base)
		{
			post_hooks.insert(base.to_string(), cmd.to_string());
			hook_names.insert(name.clone());
		}
	}

	for (script_name, script_value) in scripts {
		if LIFECYCLE_SCRIPTS.contains(&script_name.as_str()) {
			continue;
		}
		if hook_names.contains(script_name) {
			continue;
		}

		let script_cmd = match script_value.as_str() {
			Some(s) => s.to_string(),
			None => continue,
		};

		if existing_targets.contains(script_name) {
			skipped.push(script_name.clone());
			continue;
		}

		let mut spec = convert_npm_script(&script_cmd, script_name, &all_names);

		// Attach pre hook by prepending the cleaned command to the spec's commands list.
		if let Some(pre_cmd) = pre_hooks.get(script_name.as_str()) {
			let mut combined = vec![CommandStep::Shell(clean_npm_command(pre_cmd, &all_names))];
			combined.append(&mut spec.commands);
			spec.commands = combined;
		}

		// Attach post hook by appending the cleaned command (with `when: success`
		// — the default — so it skips on prior failure, same as old after.success).
		if let Some(post_cmd) = post_hooks.get(script_name.as_str()) {
			spec.commands
				.push(CommandStep::Shell(clean_npm_command(post_cmd, &all_names)));
		}

		targets.insert(script_name.clone(), spec);
	}

	PackageJsonConversion { targets, skipped }
}

/// Clean up an npm command: replace package manager invocations with `run`,
/// strip npx and node_modules/.bin/ prefixes.
fn clean_npm_command(cmd: &str, known_scripts: &HashSet<&str>) -> String {
	let trimmed = cmd.trim();

	// npm run X → run X (when X is a known script)
	for prefix in &["npm run ", "yarn run ", "pnpm run ", "bun run "] {
		if let Some(rest) = trimmed.strip_prefix(prefix) {
			let name = rest.split_whitespace().next().unwrap_or("");
			if known_scripts.contains(name) {
				return format!("run {rest}");
			}
		}
	}

	// yarn X → run X (yarn allows running scripts without "run")
	if let Some(rest) = trimmed.strip_prefix("yarn ") {
		let name = rest.split_whitespace().next().unwrap_or("");
		if known_scripts.contains(name) {
			return format!("run {rest}");
		}
	}

	// npm test → run test, npm start → run start
	if trimmed == "npm test" && known_scripts.contains("test") {
		return "run test".to_string();
	}
	if trimmed == "npm start" && known_scripts.contains("start") {
		return "run start".to_string();
	}

	// npx X → X (npx runs from node_modules/.bin, which we add to PATH)
	if let Some(rest) = trimmed.strip_prefix("npx ") {
		return rest.trim().to_string();
	}

	// ./node_modules/.bin/X → X
	if let Some(rest) = trimmed.strip_prefix("./node_modules/.bin/") {
		return rest.to_string();
	}
	if let Some(rest) = trimmed.strip_prefix("node_modules/.bin/") {
		return rest.to_string();
	}

	trimmed.to_string()
}

/// Try to convert a `run-s` / `run-p` / `npm-run-all` script into a CommandSpec
/// with sequential or parallel `run` commands.
fn try_convert_runner_tool(script: &str, script_name: &str, known_scripts: &HashSet<&str>) -> Option<CommandSpec> {
	let trimmed = script.trim();

	let (rest, is_parallel) = if let Some(rest) = trimmed.strip_prefix("run-s ") {
		(rest.trim(), false)
	} else if let Some(rest) = trimmed.strip_prefix("run-p ") {
		(rest.trim(), true)
	} else if let Some(rest) = trimmed.strip_prefix("npm-run-all ") {
		let rest = rest.trim();
		if rest.starts_with("--parallel ") || rest.starts_with("-p ") {
			let r = rest
				.strip_prefix("--parallel ")
				.or_else(|| rest.strip_prefix("-p "))
				.unwrap()
				.trim();
			(r, true)
		} else {
			let r = rest
				.strip_prefix("--sequential ")
				.or_else(|| rest.strip_prefix("-s "))
				.map(|s| s.trim())
				.unwrap_or(rest);
			(r, false)
		}
	} else {
		return None;
	};

	// Parse task names (skip flags, bail on glob patterns)
	let args: Vec<&str> = rest.split_whitespace().filter(|a| !a.starts_with('-')).collect();
	if args.is_empty() || args.iter().any(|a| a.contains('*') || a.contains('{')) {
		return None;
	}

	let commands: Vec<String> = args
		.iter()
		.map(|a| {
			if known_scripts.contains(*a) {
				format!("run {a}")
			} else {
				(*a).to_string()
			}
		})
		.collect();

	let mut spec = CommandSpec::new(to_steps(commands));
	spec.description = Some(format!("Converted from package.json script \"{script_name}\""));
	if is_parallel {
		spec.parallel = Some(true);
	}
	Some(spec)
}

/// Try to convert a `concurrently` script into a parallel CommandSpec.
fn try_convert_concurrently(script: &str, script_name: &str, known_scripts: &HashSet<&str>) -> Option<CommandSpec> {
	let trimmed = script.trim();
	let rest = trimmed.strip_prefix("concurrently ")?;

	// Extract quoted arguments, skipping flags like --kill-others
	let tokens = shell_tokenize(rest);
	let mut commands = Vec::new();
	for token in &tokens {
		if token.starts_with('-') {
			continue;
		}
		let unquoted = token.trim_matches('"').trim_matches('\'');
		let cleaned = clean_npm_command(unquoted, known_scripts);
		if !cleaned.is_empty() {
			commands.push(cleaned);
		}
	}

	if commands.is_empty() {
		return None;
	}

	let mut spec = CommandSpec::new(to_steps(commands));
	spec.description = Some(format!("Converted from package.json script \"{script_name}\""));
	spec.parallel = Some(true);
	Some(spec)
}

/// Try to strip a `dotenvx run` wrapper from a command.
/// Returns `(env_files, inner_command)` on success.
/// Handles: `dotenvx run -- cmd`, `dotenvx run -f .env.local -- cmd`,
/// `dotenvx run -f .env -f .env.prod -- cmd`, `dotenvx run cmd` (no `--`).
fn try_strip_dotenvx(cmd: &str) -> Option<(Vec<String>, String)> {
	let trimmed = cmd.trim();
	let rest = trimmed.strip_prefix("dotenvx run ")?;

	let tokens = shell_tokenize(rest);
	let mut env_files: Vec<String> = Vec::new();
	let mut i = 0;

	while i < tokens.len() {
		let token = &tokens[i];
		if token == "--" {
			// Everything after -- is the command
			let inner = tokens[i + 1..].join(" ");
			return Some((env_files, inner));
		} else if (token == "-f" || token == "--env-file") && i + 1 < tokens.len() {
			env_files.push(tokens[i + 1].trim_matches('"').trim_matches('\'').to_string());
			i += 2;
			continue;
		} else if let Some(path) = token.strip_prefix("--env-file=") {
			env_files.push(path.trim_matches('"').trim_matches('\'').to_string());
		} else if token.starts_with('-') {
			// Skip other flags (e.g. --verbose, --override)
		} else {
			// No -- separator; this token starts the command
			let inner = tokens[i..].join(" ");
			return Some((env_files, inner));
		}
		i += 1;
	}

	None
}

/// Convert a single npm script string into a Runfile CommandSpec.
fn convert_npm_script(script: &str, script_name: &str, known_scripts: &HashSet<&str>) -> CommandSpec {
	// Try specialized tool conversions first
	if let Some(spec) = try_convert_runner_tool(script, script_name, known_scripts) {
		return spec;
	}
	if let Some(spec) = try_convert_concurrently(script, script_name, known_scripts) {
		return spec;
	}

	let raw_parts: Vec<&str> = split_chained_commands(script);

	let mut env_map: HashMap<String, EnvValue> = HashMap::new();
	let mut env_files: Vec<String> = Vec::new();
	let mut commands: Vec<String> = Vec::new();
	let mut is_windows_only = false;

	for part in &raw_parts {
		let trimmed = part.trim();
		if trimmed.is_empty() {
			continue;
		}

		// Detect Windows-only patterns
		if trimmed.starts_with("set ") && trimmed.contains('=') && !trimmed.contains("set -") {
			is_windows_only = true;
		}
		if trimmed.contains('%') {
			let has_win_var = trimmed.match_indices('%').collect::<Vec<_>>().len() >= 2;
			if has_win_var {
				is_windows_only = true;
			}
		}

		// Strip dotenvx run wrapper, extracting env files
		let working = if let Some((files, inner)) = try_strip_dotenvx(trimmed) {
			for f in files {
				if !env_files.contains(&f) {
					env_files.push(f);
				}
			}
			inner
		} else {
			trimmed.to_string()
		};

		let (extracted_env, cleaned_cmd) = extract_env_and_command(&working);

		for (k, v) in extracted_env {
			env_map.insert(k, EnvValue::String(v));
		}

		if !cleaned_cmd.is_empty() {
			commands.push(clean_npm_command(&cleaned_cmd, known_scripts));
		}
	}

	if commands.is_empty() {
		commands.push(script.to_string());
	}

	let env_files = if env_files.is_empty() { None } else { Some(env_files) };
	let description = Some(format!("Converted from package.json script \"{script_name}\""));

	if is_windows_only {
		// Wrap the Windows-only commands in an `if "{{ RUN.os }} == windows"`
		// block; non-Windows runs emit a friendly error and exit 1.
		let win_commands = to_steps(commands);
		let if_step = IfStep {
			condition: "{{ RUN.os == 'windows' }}".to_string(),
			then: win_commands,
			r#else: Some(vec![CommandStep::Shell(
				"echo \"Error: this target is Windows-only\" && exit 1".to_string(),
			)]),
			ignore_errors: None,
			when: None,
		};
		let mut spec = CommandSpec::new(vec![CommandStep::If(if_step)]);
		spec.description = description;
		spec.env_files = env_files;
		if !env_map.is_empty() {
			spec.env = Some(env_map);
		}
		spec
	} else {
		let mut spec = CommandSpec::new(to_steps(commands));
		spec.description = description;
		spec.env_files = env_files;
		if !env_map.is_empty() {
			spec.env = Some(env_map);
		}
		spec
	}
}

/// Split a script on `&&` while respecting quoted strings.
pub(crate) fn split_chained_commands(script: &str) -> Vec<&str> {
	let mut parts = Vec::new();
	let mut start = 0;
	let bytes = script.as_bytes();
	let len = bytes.len();
	let mut i = 0;
	let mut in_single_quote = false;
	let mut in_double_quote = false;

	while i < len {
		let ch = bytes[i] as char;
		match ch {
			'\'' if !in_double_quote => in_single_quote = !in_single_quote,
			'"' if !in_single_quote => in_double_quote = !in_double_quote,
			'&' if !in_single_quote && !in_double_quote && i + 1 < len && bytes[i + 1] == b'&' => {
				parts.push(&script[start..i]);
				i += 2;
				start = i;
				continue;
			}
			_ => {}
		}
		i += 1;
	}
	parts.push(&script[start..]);
	parts
}

/// Extract inline env vars from the beginning of a command.
fn extract_env_and_command(cmd: &str) -> (Vec<(String, String)>, String) {
	let mut envs: Vec<(String, String)> = Vec::new();
	let working = cmd.trim();

	let working = if working.starts_with("cross-env ") {
		working.strip_prefix("cross-env ").unwrap().trim()
	} else {
		working
	};

	let tokens = shell_tokenize(working);
	let mut cmd_start_idx = 0;

	for (i, token) in tokens.iter().enumerate() {
		if let Some(eq_pos) = token.find('=') {
			let key = &token[..eq_pos];
			if !key.is_empty()
				&& key
					.chars()
					.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
			{
				let val = &token[eq_pos + 1..];
				let val = val
					.strip_prefix('"')
					.and_then(|v| v.strip_suffix('"'))
					.or_else(|| val.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
					.unwrap_or(val);
				envs.push((key.to_string(), val.to_string()));
				cmd_start_idx = i + 1;
			} else {
				break;
			}
		} else {
			break;
		}
	}

	let remaining = if cmd_start_idx < tokens.len() {
		tokens[cmd_start_idx..].join(" ")
	} else {
		String::new()
	};

	(envs, remaining)
}

/// Simple shell-like tokenizer that splits on whitespace but respects quoted strings.
pub(crate) fn shell_tokenize(input: &str) -> Vec<String> {
	let mut tokens = Vec::new();
	let mut current = String::new();
	let mut in_single = false;
	let mut in_double = false;

	for ch in input.chars() {
		match ch {
			'\'' if !in_double => {
				in_single = !in_single;
				current.push(ch);
			}
			'"' if !in_single => {
				in_double = !in_double;
				current.push(ch);
			}
			' ' | '\t' if !in_single && !in_double => {
				if !current.is_empty() {
					tokens.push(std::mem::take(&mut current));
				}
			}
			_ => {
				current.push(ch);
			}
		}
	}
	if !current.is_empty() {
		tokens.push(current);
	}
	tokens
}
