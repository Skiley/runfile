use runfile_parser::{CommandSpec, CommandStep, EnvValue};
use std::collections::HashMap;

/// Result of converting a Makefile into Runfile targets.
pub struct MakefileConversion {
	/// Converted targets keyed by target name.
	pub targets: HashMap<String, CommandSpec>,
	/// Target names that were skipped because a target with that name already exists.
	pub skipped: Vec<String>,
}

/// A raw command parsed from a Makefile recipe, with prefix metadata.
struct RawCommand {
	text: String,
	was_silent: bool,
	was_ignore_error: bool,
}

/// A parsed Makefile target before conversion.
struct MakeTarget {
	name: String,
	aliases: Vec<String>,
	commands: Vec<RawCommand>,
	deps: Vec<String>,
	is_phony: bool,
}

/// Convert Makefile content into Runfile targets.
///
/// `makefile_content` is the raw text of the Makefile.
/// `existing_targets` is the set of target names already in the Runfile (to avoid collisions).
pub fn convert_makefile(
	makefile_content: &str,
	existing_targets: &std::collections::HashSet<String>,
) -> MakefileConversion {
	let make_targets = parse_makefile(makefile_content);
	let phony_targets = collect_phony_targets(&make_targets);

	let mut targets = HashMap::new();
	let mut skipped = Vec::new();

	for mut target in make_targets {
		// Mark phony
		target.is_phony = phony_targets.contains(&target.name);

		// Skip special/internal targets
		if should_skip_target(&target.name) {
			continue;
		}

		// Skip file targets (non-phony targets with no commands are likely file targets)
		if !target.is_phony && target.commands.is_empty() {
			continue;
		}

		if existing_targets.contains(&target.name) {
			skipped.push(target.name.clone());
			continue;
		}

		let spec = make_target_to_spec(&target);
		targets.insert(target.name.clone(), spec);
	}

	MakefileConversion { targets, skipped }
}

/// Parse Makefile text into a list of targets with their commands and dependencies.
fn parse_makefile(content: &str) -> Vec<MakeTarget> {
	let mut targets: Vec<MakeTarget> = Vec::new();
	let mut current_target: Option<MakeTarget> = None;
	let mut variables: HashMap<String, String> = HashMap::new();

	let mut continuation = String::new();
	let mut in_define = false;

	for line in content.lines() {
		// Handle define/endef blocks (multi-line variable definitions)
		if in_define {
			if line.trim() == "endef" {
				in_define = false;
			}
			continue;
		}

		// Handle line continuation (backslash at end of line)
		if let Some(without_backslash) = line.strip_suffix('\\') {
			continuation.push_str(without_backslash);
			continuation.push(' ');
			continue;
		}

		// Join with any accumulated continuation
		let full_line = if continuation.is_empty() {
			line.to_string()
		} else {
			continuation.push_str(line);
			std::mem::take(&mut continuation)
		};
		let line = full_line.as_str();

		// Recipe line (starts with tab) — must be checked before comment/blank skipping
		// since recipe lines can contain shell comments like `# comment`
		if line.starts_with('\t') {
			if let Some(ref mut target) = current_target {
				let cmd = line.trim_start_matches('\t');
				let cmd = cmd.trim();
				if !cmd.is_empty() {
					let (cmd, was_silent, was_ignore_error) = strip_recipe_prefixes(cmd);
					if !cmd.is_empty() {
						let expanded = expand_variables(cmd, &variables);
						target.commands.push(RawCommand {
							text: expanded,
							was_silent,
							was_ignore_error,
						});
					}
				}
			}
			continue;
		}

		// Skip comments (non-recipe lines)
		if line.trim_start().starts_with('#') {
			continue;
		}

		// Skip empty lines
		if line.trim().is_empty() {
			continue;
		}

		// Skip Make directives
		if is_make_directive(line.trim()) {
			if line.trim().starts_with("define") {
				in_define = true;
			}
			continue;
		}

		// Variable assignment: VAR = value, VAR := value, VAR ?= value, VAR += value
		// Also handles "export VAR = value" (strip export prefix)
		if let Some(var) = try_parse_variable(line) {
			variables.insert(var.0, var.1);
			continue;
		}

		// Target line: name: deps
		if let Some(target_def) = try_parse_target_line(line) {
			// Save previous target
			if let Some(prev) = current_target.take() {
				targets.push(prev);
			}
			current_target = Some(target_def);
		}
	}

	// Save last target
	if let Some(target) = current_target {
		targets.push(target);
	}

	targets
}

/// Strip leading `@` (silent) and/or `-` (ignore error) prefixes from a recipe line.
/// Returns (cleaned_command, was_silent, was_ignore_error). Handles any order and combinations.
fn strip_recipe_prefixes(cmd: &str) -> (&str, bool, bool) {
	let mut was_silent = false;
	let mut was_ignore_error = false;
	let mut rest = cmd;

	loop {
		if let Some(r) = rest.strip_prefix('@') {
			was_silent = true;
			rest = r.trim_start();
		} else if let Some(r) = rest.strip_prefix('-') {
			was_ignore_error = true;
			rest = r.trim_start();
		} else {
			break;
		}
	}

	(rest, was_silent, was_ignore_error)
}

/// Check if a non-recipe line is a Make directive that should be skipped.
fn is_make_directive(line: &str) -> bool {
	let first_word = line.split_whitespace().next().unwrap_or("");
	matches!(
		first_word,
		"ifeq"
			| "ifneq" | "ifdef"
			| "ifndef"
			| "else" | "endif"
			| "define"
			| "endef" | "override"
			| "include"
			| "-include"
			| "sinclude"
			| "unexport"
			| "vpath"
	)
}

/// Try to parse a variable assignment line.
fn try_parse_variable(line: &str) -> Option<(String, String)> {
	// Must not start with tab (that's a recipe line)
	if line.starts_with('\t') {
		return None;
	}

	let trimmed = line.trim();

	// Handle "export VAR = value" (strip export prefix)
	let trimmed = if let Some(rest) = trimmed.strip_prefix("export ") {
		let rest = rest.trim();
		// "export VAR" without "=" is just an export directive, skip it
		if !rest.contains('=') {
			return None;
		}
		rest
	} else {
		trimmed
	};

	// Check for various assignment operators
	for op in &[":=", "?=", "+=", "="] {
		if let Some(pos) = trimmed.find(op) {
			let key = trimmed[..pos].trim();
			let val = trimmed[pos + op.len()..].trim();

			// Validate key looks like a variable name (no colons, spaces)
			if !key.is_empty() && !key.contains(':') && !key.contains(' ') && !key.contains('\t') {
				return Some((key.to_string(), val.to_string()));
			}
		}
	}
	None
}

/// Try to parse a target definition line (name: deps).
fn try_parse_target_line(line: &str) -> Option<MakeTarget> {
	if line.starts_with('\t') {
		return None;
	}

	let trimmed = line.trim();

	// Must contain a colon, but not start with it (that would be a weird edge case)
	let colon_pos = trimmed.find(':')?;
	if colon_pos == 0 {
		return None;
	}

	// The part before the colon is the target name(s)
	// Skip pattern rules (contain %)
	let name_part = trimmed[..colon_pos].trim();
	if name_part.contains('%') {
		return None;
	}

	// Skip if it looks like a variable assignment with := etc
	let after_colon = &trimmed[colon_pos + 1..];
	if after_colon.starts_with('=') || after_colon.starts_with(':') {
		return None;
	}

	// Multiple target names (space-separated) — take the first name.
	// In Make, `a b: deps` means both `a` and `b` share the same recipe.
	// We use the first name; the rest become aliases.
	let names: Vec<&str> = name_part.split_whitespace().collect();
	let name = names[0].to_string();
	let aliases: Vec<String> = names[1..].iter().map(|s| s.to_string()).collect();
	let deps: Vec<String> = after_colon
		.split_whitespace()
		.filter(|d| !d.is_empty() && !d.starts_with('|'))
		.map(|d| d.to_string())
		.collect();

	Some(MakeTarget {
		name,
		aliases,
		commands: Vec::new(),
		deps,
		is_phony: false,
	})
}

/// Collect all phony target names from .PHONY declarations.
fn collect_phony_targets(targets: &[MakeTarget]) -> std::collections::HashSet<String> {
	let mut phony = std::collections::HashSet::new();
	for target in targets {
		if target.name == ".PHONY" {
			for dep in &target.deps {
				phony.insert(dep.clone());
			}
		}
	}
	phony
}

/// Expand simple $(VAR) and ${VAR} references in a string.
pub(crate) fn expand_variables(input: &str, vars: &HashMap<String, String>) -> String {
	// Accumulate BYTES, not chars: the previous `result.push(bytes[i] as char)`
	// cast each byte of a multi-byte UTF-8 sequence to its own `char`, producing
	// mojibake (`é` → `Ã©`). All the markers we scan for (`$`, `(`, `)`, `{`,
	// `}`) are ASCII and can never appear inside a UTF-8 continuation byte, so
	// byte-scanning stays correct while byte-appending preserves the original
	// UTF-8. Every pushed run comes from a valid `&str`, so the buffer is always
	// valid UTF-8.
	let mut result: Vec<u8> = Vec::with_capacity(input.len());
	let bytes = input.as_bytes();
	let len = bytes.len();
	let mut i = 0;

	while i < len {
		if bytes[i] == b'$' && i + 1 < len {
			let (open, close) = if bytes[i + 1] == b'(' {
				(b'(', b')')
			} else if bytes[i + 1] == b'{' {
				(b'{', b'}')
			} else {
				result.push(b'$');
				i += 1;
				continue;
			};

			let start = i + 2;
			if let Some(end) = bytes[start..].iter().position(|&b| b == close) {
				let var_name = &input[start..start + end];
				// Only expand simple variable names (not function calls like $(shell ...) )
				if !var_name.contains(' ') && !var_name.contains(',') {
					if let Some(val) = vars.get(var_name) {
						result.extend_from_slice(val.as_bytes());
					} else {
						// Keep as-is if not in our variable map
						result.push(b'$');
						result.push(open);
						result.extend_from_slice(var_name.as_bytes());
						result.push(close);
					}
				} else {
					// Keep function calls as-is
					result.push(b'$');
					result.push(open);
					result.extend_from_slice(var_name.as_bytes());
					result.push(close);
				}
				i = start + end + 1;
			} else {
				result.push(b'$');
				i += 1;
			}
		} else {
			result.push(bytes[i]);
			i += 1;
		}
	}

	// Always valid UTF-8 by construction; lossy conversion never alters it and
	// avoids any panic risk.
	String::from_utf8_lossy(&result).into_owned()
}

/// Make a string safe to embed inside a double-quoted shell `echo`. Keeps
/// alphanumerics and a small set of harmless punctuation; every other character
/// (including `"`, `` ` ``, `$`, `\`, `;`, `|`, `&`, `<`, `>`, newlines) becomes
/// `_`. Sanitizing rather than shell-escaping keeps this correct across every
/// shell the converted Runfile might run under (sh/bash/zsh/fish/pwsh/cmd).
pub(crate) fn sanitize_echo_text(s: &str) -> String {
	s.chars()
		.map(|c| {
			if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | ':' | '.' | '/') {
				c
			} else {
				'_'
			}
		})
		.collect()
}

/// Check if a target should be skipped (special Make targets, internal names).
fn should_skip_target(name: &str) -> bool {
	matches!(
		name,
		".PHONY"
			| ".DEFAULT"
			| ".PRECIOUS"
			| ".INTERMEDIATE"
			| ".SECONDARY"
			| ".SECONDEXPANSION"
			| ".DELETE_ON_ERROR"
			| ".IGNORE"
			| ".LOW_RESOLUTION_TIME"
			| ".SILENT"
			| ".EXPORT_ALL_VARIABLES"
			| ".NOTPARALLEL"
			| ".ONESHELL"
			| ".POSIX"
			| ".SUFFIXES"
	)
}

/// Convert a `$(MAKE) target` or `${MAKE} target` or `make target` command
/// into `run target`. Returns None if the command doesn't match this pattern.
fn try_convert_make_call(cmd: &str) -> Option<String> {
	let trimmed = cmd.trim();
	let rest = trimmed
		.strip_prefix("$(MAKE)")
		.or_else(|| trimmed.strip_prefix("${MAKE}"))
		.map(|r| r.trim_start())
		.or_else(|| trimmed.strip_prefix("make "))?;
	let rest = rest.trim();
	if rest.is_empty() {
		return None;
	}
	// Extract the first non-flag word as the target name
	let target = rest.split_whitespace().find(|w| !w.starts_with('-'))?;
	// Must look like a target name, not a file path
	if target.contains('/') || target.is_empty() {
		return None;
	}
	// Reconstruct: replace the make invocation with `run`
	// Preserve any trailing arguments after the target name
	let remaining_args: Vec<&str> = rest
		.split_whitespace()
		.skip_while(|w| w.starts_with('-'))
		.skip(1) // skip the target name itself
		.collect();
	if remaining_args.is_empty() {
		Some(format!("run {target}"))
	} else {
		Some(format!("run {target} {}", remaining_args.join(" ")))
	}
}

/// Convert a parsed MakeTarget into a Runfile CommandSpec.
fn make_target_to_spec(target: &MakeTarget) -> CommandSpec {
	// Convert Make deps to leading `@target` invocations in `commands`
	// (only phony-like targets, not file deps).
	let dep_calls: Vec<CommandStep> = target
		.deps
		.iter()
		.filter(|d| !d.contains('/') && !d.contains('.'))
		.map(|d| CommandStep::target_call(d.clone(), ""))
		.collect();

	// Check if all commands were silent (@) or ignore-error (-)
	let all_silent = !target.commands.is_empty() && target.commands.iter().all(|c| c.was_silent);
	let all_ignore = !target.commands.is_empty() && target.commands.iter().all(|c| c.was_ignore_error);

	// Extract env vars from commands that are just VAR=value assignments,
	// and convert $(MAKE) calls to `run` invocations
	let mut env_map: HashMap<String, EnvValue> = HashMap::new();
	let mut real_commands: Vec<String> = Vec::new();

	for cmd in &target.commands {
		if let Some((key, val)) = try_parse_export(&cmd.text) {
			env_map.insert(key, EnvValue::String(val));
		} else if let Some(run_cmd) = try_convert_make_call(&cmd.text) {
			real_commands.push(run_cmd);
		} else {
			real_commands.push(cmd.text.clone());
		}
	}

	if real_commands.is_empty() && !env_map.is_empty() {
		real_commands.push("echo \"(no commands — env-only target)\"".to_string());
	} else if real_commands.is_empty() {
		// Sanitize the name before embedding it in a shell `echo`: a crafted
		// phony target name (e.g. `x";rm -rf ~;echo "`) would otherwise inject
		// commands when the converted target is run. Keep only characters that
		// are safe in any shell; replace the rest with `_`. This introduces no
		// injection point the source Makefile didn't already have.
		real_commands.push(format!("echo \"Target: {}\"", sanitize_echo_text(&target.name)));
	}

	let aliases = if target.aliases.is_empty() {
		None
	} else {
		Some(target.aliases.clone())
	};

	// Prepend the dependency `@target` invocations before the actual commands.
	let mut combined: Vec<CommandStep> = dep_calls;
	combined.extend(real_commands.into_iter().map(CommandStep::Shell));

	let mut spec = CommandSpec::new(combined);
	spec.description = Some(format!("Converted from Makefile target \"{}\"", target.name));
	spec.aliases = aliases;
	if !env_map.is_empty() {
		spec.env = Some(env_map);
	}
	if all_silent {
		spec.logging = Some(false);
	}
	if all_ignore {
		spec.ignore_errors = Some(true);
	}
	spec
}

/// Try to parse `export VAR=value` as an env assignment.
/// Only matches explicit `export` statements — bare `VAR=value cmd` is shell
/// inline-env syntax and should be kept as a command.
fn try_parse_export(cmd: &str) -> Option<(String, String)> {
	let trimmed = cmd.trim();
	let work = trimmed.strip_prefix("export ")?.trim();

	if let Some(eq_pos) = work.find('=') {
		let key = work[..eq_pos].trim();
		let val = work[eq_pos + 1..].trim();

		// Must look like a simple env assignment, not a command
		if !key.is_empty()
			&& !key.contains(' ')
			&& key
				.chars()
				.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
			&& !val.contains("$(")
			&& !val.contains("${")
		{
			let val = val.trim_matches('"').trim_matches('\'');
			return Some((key.to_string(), val.to_string()));
		}
	}
	None
}
