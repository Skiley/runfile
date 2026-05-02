use crate::dsl::{parse_condition, DslParseError};
use crate::schema::{
	CommandStep, ForStep, IfStep, Runfile, TargetCallStep, WhenStep, WORKING_DIRECTORY_CWD,
	WORKING_DIRECTORY_RUNFILE_PARENT,
};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
	#[error("Failed to read Runfile: {0}")]
	Io(#[from] std::io::Error),

	#[error("Failed to parse Runfile: {0}")]
	Json(#[from] json5::Error),

	#[error("Empty $schema field — set it to \"https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json\" or a schema URL")]
	EmptySchema,

	#[error("Target name \"{0}\" must not start with ':' (reserved for built-in commands)")]
	ReservedTargetName(String),

	#[error("Runfile must define at least one target")]
	NoTargets,

	#[error("Target \"{0}\" has an empty commands list")]
	EmptyCommandList(String),

	#[error("Alias \"{0}\" in target \"{1}\" conflicts with existing target name")]
	AliasConflictsWithTarget(String, String),

	#[error("Alias \"{0}\" is used by both target \"{1}\" and target \"{2}\"")]
	DuplicateAlias(String, String, String),

	#[error("Alias \"{0}\" in target \"{1}\" must not start with ':' (reserved for built-in commands)")]
	ReservedAlias(String, String),

	#[error("Alias \"{0}\" in target \"{1}\" is the same as the target name")]
	AliasSameAsTarget(String, String),

	#[error("Target \"{0}\" has detach: true with multiple commands but parallel is not enabled. Set parallel: true to use detach with multiple commands.")]
	DetachRequiresParallel(String),

	#[error("Invalid environment variable name \"{0}\" in {1}. Names must match [A-Za-z_][A-Za-z0-9_]*.")]
	InvalidEnvKey(String, String),

	#[error("Runfile is too large ({0} bytes, maximum is {1} bytes)")]
	FileTooLarge(u64, u64),

	#[error("Invalid condition in {context}: {error}\n  → {condition}")]
	InvalidCondition {
		context: String,
		condition: String,
		error: DslParseError,
	},

	#[error("Invalid `for` block in {0}: exactly one of \"in\", \"glob\", or \"shell\" must be specified (got {1})")]
	InvalidForIterator(String, String),

	#[error("Invalid `for` loop variable name \"{0}\" in {1}. Names must match [A-Za-z_][A-Za-z0-9_]*.")]
	InvalidForVarName(String, String),

	#[error("Invalid target invocation in {context}: {reason}")]
	InvalidTargetCall { context: String, reason: String },

	#[error("`when` block in {0} has an empty commands list")]
	EmptyWhenCommands(String),

	#[error(
		"Invalid `workingDirectory` value \"{1}\" in {0} — must be \"runfileParent\" or \"cwd\" (or a `$(...)` substitution)."
	)]
	InvalidWorkingDirectoryLiteral(String, String),
}

/// Whether a string carries a `$(...)` substitution that defers its actual
/// value to runtime. Used to skip parse-time literal validation for fields
/// like `forceShell` / `workingDirectory` that accept substitution.
fn is_substitution_template(s: &str) -> bool {
	s.contains("$(")
}

/// Validate a `workingDirectory` field. Substitution templates are accepted
/// (they're checked at runtime); pure literals must be one of the canonical
/// values.
fn validate_working_directory(value: &Option<String>, context: &str) -> Result<(), ParseError> {
	if let Some(s) = value {
		if !is_substitution_template(s) && s != WORKING_DIRECTORY_RUNFILE_PARENT && s != WORKING_DIRECTORY_CWD {
			return Err(ParseError::InvalidWorkingDirectoryLiteral(
				context.to_string(),
				s.clone(),
			));
		}
	}
	Ok(())
}

/// Maximum Runfile size in bytes (10 MiB). Prevents denial-of-service via
/// huge files that would consume excessive memory during parsing.
pub const MAX_RUNFILE_SIZE: u64 = 10 * 1024 * 1024;

/// Parse a Runfile from a JSON string.
///
/// Validates structural constraints (non-empty schema, at least one target,
/// non-empty command lists, env keys, aliases, `for`-step iterator XOR,
/// literal `workingDirectory` values). `@target` references inside `commands`
/// are NOT validated here — they are checked at runtime, because included
/// files may define targets not yet available.
///
/// As part of validation, every `if` condition is parsed eagerly into a
/// [`crate::DslExpr`] AST and cached on the [`crate::IfStep`] so that
/// runtime evaluation does not need to re-tokenize. Syntax errors in
/// conditions surface at load time.
pub fn parse_runfile(json: &str) -> Result<Runfile, ParseError> {
	let mut runfile: Runfile = crate::json::from_json_str(json)?;
	validate_runfile(&mut runfile, true)?;
	Ok(runfile)
}

/// Parse a Runfile from a file path.
///
/// Rejects files larger than [`MAX_RUNFILE_SIZE`] to prevent denial-of-service.
pub fn parse_runfile_from_path(path: &Path) -> Result<Runfile, ParseError> {
	check_file_size(path)?;
	let content = std::fs::read_to_string(path)?;
	parse_runfile(&content)
}

/// Parse a partial Runfile (included file or global settings file).
///
/// Same validation as [`parse_runfile`] but allows zero targets, since included
/// files and global settings may not define any targets of their own.
pub fn parse_runfile_partial(json: &str) -> Result<Runfile, ParseError> {
	let mut runfile: Runfile = crate::json::from_json_str(json)?;
	validate_runfile(&mut runfile, false)?;
	Ok(runfile)
}

/// Parse a partial Runfile from a file path.
///
/// Rejects files larger than [`MAX_RUNFILE_SIZE`] to prevent denial-of-service.
pub fn parse_runfile_from_path_partial(path: &Path) -> Result<Runfile, ParseError> {
	check_file_size(path)?;
	let content = std::fs::read_to_string(path)?;
	parse_runfile_partial(&content)
}

/// Check that a file does not exceed the maximum allowed size.
fn check_file_size(path: &Path) -> Result<(), ParseError> {
	let metadata = std::fs::metadata(path)?;
	let size = metadata.len();
	if size > MAX_RUNFILE_SIZE {
		return Err(ParseError::FileTooLarge(size, MAX_RUNFILE_SIZE));
	}
	Ok(())
}

/// Target names starting with ':' are reserved for built-in CLI commands.
fn is_reserved_name(name: &str) -> bool {
	name.starts_with(':')
}

/// Check whether a string is a valid environment variable name.
///
/// Valid names match `[A-Za-z_][A-Za-z0-9_]*`. This prevents shell injection
/// when env keys are interpolated into shell commands (e.g. `$env:KEY=...` in
/// PowerShell or `set KEY=...` in cmd.exe).
pub fn is_valid_env_key(key: &str) -> bool {
	if key.is_empty() {
		return false;
	}
	let mut chars = key.chars();
	let first = chars.next().unwrap();
	if !first.is_ascii_alphabetic() && first != '_' {
		return false;
	}
	chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate all env keys in an optional env map.
fn validate_env_keys(
	env: &Option<std::collections::HashMap<String, crate::schema::EnvValue>>,
	context: &str,
) -> Result<(), ParseError> {
	if let Some(map) = env {
		for key in map.keys() {
			if !is_valid_env_key(key) {
				return Err(ParseError::InvalidEnvKey(key.clone(), context.to_string()));
			}
		}
	}
	Ok(())
}

/// Recursively validate a slice of [`CommandStep`]s. For [`IfStep`]s, the
/// condition is parsed and the resulting AST is cached. For [`ForStep`]s,
/// the iterator-source XOR and variable-name regex are checked. For
/// [`WhenStep`]s, the inner commands list is validated. Nested blocks are
/// visited.
pub(crate) fn validate_command_steps(steps: &mut [CommandStep], context: &str) -> Result<(), ParseError> {
	for step in steps {
		match step {
			CommandStep::Shell(_) => {}
			CommandStep::TargetCall(call) => {
				validate_target_call(call, context)?;
			}
			CommandStep::When(when_step) => {
				validate_when_step(when_step, context)?;
			}
			CommandStep::If(if_step) => {
				validate_if_step(if_step, context)?;
			}
			CommandStep::For(for_step) => {
				validate_for_step(for_step, context)?;
			}
		}
	}
	Ok(())
}

fn validate_when_step(step: &mut WhenStep, context: &str) -> Result<(), ParseError> {
	if step.commands.is_empty() {
		return Err(ParseError::EmptyWhenCommands(context.to_string()));
	}
	let inner_ctx = format!("{context} > when");
	validate_command_steps(&mut step.commands, &inner_ctx)
}

fn validate_target_call(call: &TargetCallStep, context: &str) -> Result<(), ParseError> {
	if call.target.is_empty() {
		return Err(ParseError::InvalidTargetCall {
			context: context.to_string(),
			reason: "target name is empty".to_string(),
		});
	}
	if call.target.contains(char::is_whitespace) {
		return Err(ParseError::InvalidTargetCall {
			context: context.to_string(),
			reason: format!("target name \"{}\" contains whitespace", call.target),
		});
	}
	Ok(())
}

fn validate_if_step(step: &mut IfStep, context: &str) -> Result<(), ParseError> {
	let trimmed = step.condition.trim();
	if trimmed.is_empty() {
		return Err(ParseError::InvalidCondition {
			context: context.to_string(),
			condition: step.condition.clone(),
			error: DslParseError::EmptyCondition,
		});
	}
	let ast = parse_condition(&step.condition).map_err(|error| ParseError::InvalidCondition {
		context: context.to_string(),
		condition: step.condition.clone(),
		error,
	})?;
	step.condition_ast = Some(ast);

	// Recurse into branches.
	let then_ctx = format!("{context} > if/then");
	validate_command_steps(&mut step.then, &then_ctx)?;
	if let Some(else_branch) = step.r#else.as_mut() {
		let else_ctx = format!("{context} > if/else");
		validate_command_steps(else_branch, &else_ctx)?;
	}
	Ok(())
}

fn validate_for_step(step: &mut ForStep, context: &str) -> Result<(), ParseError> {
	if !is_valid_loop_var(&step.var) {
		return Err(ParseError::InvalidForVarName(step.var.clone(), context.to_string()));
	}

	let count = (step.r#in.is_some() as u8) + (step.glob.is_some() as u8) + (step.shell.is_some() as u8);
	if count != 1 {
		let detail = if count == 0 {
			"none".to_string()
		} else {
			let mut names: Vec<&str> = Vec::new();
			if step.r#in.is_some() {
				names.push("in");
			}
			if step.glob.is_some() {
				names.push("glob");
			}
			if step.shell.is_some() {
				names.push("shell");
			}
			names.join(" and ")
		};
		return Err(ParseError::InvalidForIterator(context.to_string(), detail));
	}

	let body_ctx = format!("{context} > for/do");
	validate_command_steps(&mut step.body, &body_ctx)?;
	Ok(())
}

fn is_valid_loop_var(name: &str) -> bool {
	if name.is_empty() {
		return false;
	}
	let mut chars = name.chars();
	let first = chars.next().unwrap();
	if !first.is_ascii_alphabetic() && first != '_' {
		return false;
	}
	chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate structural constraints beyond what serde can enforce.
///
/// When `require_targets` is true, at least one target must be defined (used for
/// the root Runfile). When false, zero targets are allowed (used for included
/// files and global settings files).
///
/// Target references in lifecycle steps are never validated here — they are
/// checked at runtime, because included files may define targets that aren't
/// available at parse time.
fn validate_runfile(runfile: &mut Runfile, require_targets: bool) -> Result<(), ParseError> {
	if runfile.schema.is_empty() {
		return Err(ParseError::EmptySchema);
	}

	if require_targets && runfile.targets.is_empty() {
		return Err(ParseError::NoTargets);
	}

	for (name, spec) in runfile.targets.iter_mut() {
		if is_reserved_name(name) {
			return Err(ParseError::ReservedTargetName(name.clone()));
		}

		if spec.commands.is_empty() {
			return Err(ParseError::EmptyCommandList(name.clone()));
		}

		// Validate the command steps (recursively expands if/for/when blocks).
		validate_command_steps(&mut spec.commands, &format!("target \"{name}\""))?;

		// Validate detach requires parallel when there are multiple commands
		if spec.detach.unwrap_or(false) && !spec.parallel.unwrap_or(false) && spec.commands.len() > 1 {
			return Err(ParseError::DetachRequiresParallel(name.clone()));
		}

		// Validate workingDirectory literal values (templates pass through).
		validate_working_directory(&spec.working_directory, &format!("target \"{name}\""))?;

		// Validate env key names to prevent shell injection
		validate_env_keys(&spec.env, &format!("target \"{name}\""))?;
	}

	// Validate aliases
	{
		let mut alias_owners: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
		for (name, spec) in &runfile.targets {
			if let Some(aliases) = &spec.aliases {
				for alias in aliases {
					if alias == name {
						return Err(ParseError::AliasSameAsTarget(alias.clone(), name.clone()));
					}
					if is_reserved_name(alias) {
						return Err(ParseError::ReservedAlias(alias.clone(), name.clone()));
					}
					if runfile.targets.contains_key(alias) {
						return Err(ParseError::AliasConflictsWithTarget(alias.clone(), name.clone()));
					}
					if let Some(other) = alias_owners.get(alias.as_str()) {
						return Err(ParseError::DuplicateAlias(
							alias.clone(),
							other.to_string(),
							name.clone(),
						));
					}
					alias_owners.insert(alias, name);
				}
			}
		}
	}

	// Validate globals
	if let Some(globals) = runfile.globals.as_mut() {
		validate_working_directory(&globals.working_directory, "(globals)")?;

		// Validate global env key names
		validate_env_keys(&globals.env, "(globals)")?;
	}

	Ok(())
}
