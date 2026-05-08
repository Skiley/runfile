use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A step in a `commands` array. Either a raw shell command, a target
/// invocation (`@target args`), a `when`-guarded block, an `if`-block, a
/// `for`-block, or a `match`-block. Backwards compatible with plain-string
/// arrays: any string without a leading `@` deserializes as
/// `CommandStep::Shell`.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandStep {
	/// A raw shell command string (the original Runfile shape).
	Shell(String),
	/// A target invocation — `@target [args...]` from a string entry.
	/// The leading `@` is stripped at parse time.
	TargetCall(TargetCallStep),
	/// `when`-guarded block of commands. Used to run the inner commands only
	/// on success / only on failure / always, depending on the target's
	/// running state.
	When(WhenStep),
	/// Conditional execution.
	If(IfStep),
	/// Iteration over an inline array, glob, or shell output.
	For(ForStep),
	/// Multi-way dispatch on a substituted value. Equivalent to a chain of
	/// `if` / `else if` / `else` blocks but with a clearer error story when
	/// the value doesn't match any case (and no default is configured).
	Match(MatchStep),
}

impl Serialize for CommandStep {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		match self {
			CommandStep::Shell(s) => serializer.serialize_str(s),
			CommandStep::TargetCall(call) => {
				let prefix = if call.optional { "@?" } else { "@" };
				let s = if call.args_template.is_empty() {
					format!("{}{}", prefix, call.target)
				} else {
					format!("{}{} {}", prefix, call.target, call.args_template)
				};
				serializer.serialize_str(&s)
			}
			CommandStep::When(when_step) => when_step.serialize(serializer),
			CommandStep::If(if_step) => if_step.serialize(serializer),
			CommandStep::For(for_step) => for_step.serialize(serializer),
			CommandStep::Match(match_step) => match_step.serialize(serializer),
		}
	}
}

/// Controls when a step executes relative to the target's running state.
///
/// Default is `Success`: a step only runs while no prior step has failed.
/// `Failure` steps run only after a prior step has failed (so they can do
/// cleanup, error reporting, etc.). `Always` steps run regardless.
///
/// State flips to "failed" the first time a `when: Success` step exits
/// non-zero (and isn't `ignoreErrors`'d). Once failed, the state stays
/// failed for the rest of the target — there is no "recovery" by a
/// `Failure` step succeeding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WhenCondition {
	/// Run only if no prior step has failed (default).
	#[default]
	Success,
	/// Run only after a prior step has failed.
	Failure,
	/// Run regardless of state.
	Always,
}

impl WhenCondition {
	/// Whether this condition matches the given "has-the-target-failed-yet" state.
	pub fn matches(self, failed: bool) -> bool {
		match self {
			WhenCondition::Success => !failed,
			WhenCondition::Failure => failed,
			WhenCondition::Always => true,
		}
	}
}

/// A `when`-guarded block of command steps.
///
/// Wraps a list of inner commands so they run only when the target's state
/// matches the configured `when` condition. Use this to express
/// post-failure cleanup, always-runs-after-everything teardown, or
/// success-only follow-ups inline with the rest of `commands`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WhenStep {
	/// State guard for the inner commands. Defaults to `Success` if missing.
	#[serde(default)]
	pub when: WhenCondition,

	/// The guarded command steps. Run sequentially in source order; cannot
	/// itself be a parallel block (the parent's `parallel: true` decides).
	///
	/// Accepts either a single string (sugar for a one-element array) or a
	/// full array of [`CommandStep`]s.
	#[serde(deserialize_with = "deserialize_steps_or_string")]
	pub commands: Vec<CommandStep>,

	/// When true, failures inside this block do not flip the target's success
	/// state. Same semantics as `if.ignoreErrors` / `for.ignoreErrors`.
	#[serde(default, rename = "ignoreErrors", skip_serializing_if = "Option::is_none")]
	pub ignore_errors: Option<bool>,
}

impl<'de> Deserialize<'de> for CommandStep {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		// Manual untagged deserialization that produces clearer error messages
		// than serde's default `untagged` impl (which swallows context).
		use serde_json::Value;
		let value = Value::deserialize(deserializer)?;
		match value {
			Value::String(s) => command_step_from_string(s).map_err(serde::de::Error::custom),
			Value::Object(_) => {
				let map = match value {
					Value::Object(m) => m,
					_ => unreachable!(),
				};
				if map.contains_key("if") {
					let step: IfStep = serde_json::from_value(Value::Object(map)).map_err(serde::de::Error::custom)?;
					Ok(CommandStep::If(step))
				} else if map.contains_key("for") {
					let step: ForStep = serde_json::from_value(Value::Object(map)).map_err(serde::de::Error::custom)?;
					Ok(CommandStep::For(step))
				} else if map.contains_key("match") {
					let step: MatchStep =
						serde_json::from_value(Value::Object(map)).map_err(serde::de::Error::custom)?;
					Ok(CommandStep::Match(step))
				} else if map.contains_key("commands") {
					let step: WhenStep =
						serde_json::from_value(Value::Object(map)).map_err(serde::de::Error::custom)?;
					Ok(CommandStep::When(step))
				} else {
					Err(serde::de::Error::custom(
						"command step object must contain an \"if\", \"for\", \"match\", or \"commands\" key",
					))
				}
			}
			_ => Err(serde::de::Error::custom(
				"command step must be a string or an object with an \"if\", \"for\", \"match\", or \"commands\" key",
			)),
		}
	}
}

impl From<String> for CommandStep {
	fn from(s: String) -> Self {
		CommandStep::Shell(s)
	}
}

impl From<&str> for CommandStep {
	fn from(s: &str) -> Self {
		CommandStep::Shell(s.to_string())
	}
}

// Convenience comparisons used by tests and some scanners. Only `Shell`
// variants compare equal to strings; control-flow variants are never equal
// to a plain string.
impl PartialEq<str> for CommandStep {
	fn eq(&self, other: &str) -> bool {
		matches!(self, CommandStep::Shell(s) if s == other)
	}
}

impl PartialEq<&str> for CommandStep {
	fn eq(&self, other: &&str) -> bool {
		matches!(self, CommandStep::Shell(s) if s == *other)
	}
}

impl PartialEq<String> for CommandStep {
	fn eq(&self, other: &String) -> bool {
		matches!(self, CommandStep::Shell(s) if s == other)
	}
}

impl PartialEq<CommandStep> for &str {
	fn eq(&self, other: &CommandStep) -> bool {
		other == self
	}
}

impl PartialEq<CommandStep> for String {
	fn eq(&self, other: &CommandStep) -> bool {
		other == self
	}
}

impl CommandStep {
	/// Convenience constructor for a raw shell command.
	pub fn shell<S: Into<String>>(s: S) -> Self {
		CommandStep::Shell(s.into())
	}

	/// Convenience constructor for a target invocation (`@target args`).
	pub fn target_call(target: impl Into<String>, args_template: impl Into<String>) -> Self {
		CommandStep::TargetCall(TargetCallStep {
			target: target.into(),
			args_template: args_template.into(),
			optional: false,
		})
	}

	/// Convenience constructor for an optional target invocation (`@?target args`).
	/// At execute time, if the target doesn't resolve, the call is silently
	/// skipped instead of erroring.
	pub fn optional_target_call(target: impl Into<String>, args_template: impl Into<String>) -> Self {
		CommandStep::TargetCall(TargetCallStep {
			target: target.into(),
			args_template: args_template.into(),
			optional: true,
		})
	}

	/// If this step is a [`CommandStep::Shell`], return its string. Returns
	/// `None` for control-flow blocks and target invocations.
	pub fn as_shell_str(&self) -> Option<&str> {
		match self {
			CommandStep::Shell(s) => Some(s.as_str()),
			_ => None,
		}
	}

	/// True if this step is a [`CommandStep::Shell`] whose string contains
	/// the given pattern. Returns false for control-flow blocks. This is a
	/// convenience used by tests and tooling — for full traversal across
	/// nested control flow use [`Self::walk_templates`] instead.
	pub fn contains(&self, pat: &str) -> bool {
		matches!(self, CommandStep::Shell(s) if s.contains(pat))
	}

	/// Walk this step (recursively) and call `visit` on every leaf string
	/// template that participates in substitution: shell command strings,
	/// target-invocation arg templates, `if` condition expressions, `for in`
	/// array elements, and `for glob`/`for shell` iterator sources.
	///
	/// Used for static analysis (arg-usage detection, scanning for `{{ ARGS.x }}`
	/// references in IDE generators and MCP tooling) without needing to
	/// resolve values.
	pub fn walk_templates<'a, F: FnMut(&'a str)>(&'a self, visit: &mut F) {
		match self {
			CommandStep::Shell(s) => visit(s.as_str()),
			CommandStep::TargetCall(call) => {
				if !call.args_template.is_empty() {
					visit(call.args_template.as_str());
				}
			}
			CommandStep::When(WhenStep { commands, .. }) => {
				for step in commands {
					step.walk_templates(visit);
				}
			}
			CommandStep::If(IfStep {
				condition,
				then,
				r#else,
				..
			}) => {
				visit(condition.as_str());
				for step in then {
					step.walk_templates(visit);
				}
				if let Some(else_steps) = r#else {
					for step in else_steps {
						step.walk_templates(visit);
					}
				}
			}
			CommandStep::For(ForStep {
				r#in,
				glob,
				shell,
				body,
				..
			}) => {
				// Only `Literal` entries are templates; the magic `"namespaces"`
				// keyword isn't substitutable.
				if let Some(ForInValue::Literal(items)) = r#in {
					for item in items {
						visit(item.as_str());
					}
				}
				if let Some(g) = glob {
					visit(g.as_str());
				}
				if let Some(s) = shell {
					visit(s.as_str());
				}
				for step in body {
					step.walk_templates(visit);
				}
			}
			CommandStep::Match(MatchStep {
				r#match,
				cases,
				default,
				..
			}) => {
				visit(r#match.as_str());
				for steps in cases.values() {
					for step in steps {
						step.walk_templates(visit);
					}
				}
				if let Some(default_steps) = default {
					for step in default_steps {
						step.walk_templates(visit);
					}
				}
			}
		}
	}

	/// The step's effective `when` condition. Returns `Success` for steps that
	/// don't carry a `when` (plain shells, target calls). For `WhenStep`, `IfStep`,
	/// `ForStep`, and `MatchStep`, returns the configured value (defaulting to
	/// `Success`).
	pub fn effective_when(&self) -> WhenCondition {
		match self {
			CommandStep::Shell(_) | CommandStep::TargetCall(_) => WhenCondition::Success,
			CommandStep::When(w) => w.when,
			CommandStep::If(i) => i.when.unwrap_or_default(),
			CommandStep::For(f) => f.when.unwrap_or_default(),
			CommandStep::Match(m) => m.when.unwrap_or_default(),
		}
	}
}

/// Find the first whitespace that splits `target` from `args` in a target-call
/// string, but skip whitespace inside a `{{ ... }}` substitution block. Lets
/// `@{{ VARS.ns }}:dev` keep its whole substituted name as the target.
fn find_target_args_split(s: &str) -> Option<usize> {
	let bytes = s.as_bytes();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
			// Skip until matching `}}`.
			let close = match s[i + 2..].find("}}") {
				Some(rel) => i + 2 + rel + 2,
				None => return None,
			};
			i = close;
			continue;
		}
		if (bytes[i] as char).is_whitespace() {
			return Some(i);
		}
		i += 1;
	}
	None
}

/// Convert a raw string into a [`CommandStep`]. Strings starting with `@`
/// become `TargetCall`; everything else is a plain `Shell` command. Used
/// both by the manual `Deserialize` impl and by the `then` / `else`
/// string-shorthand path in [`IfStep`] (where serde routes through a
/// custom deserializer that needs the same parsing logic).
pub(crate) fn command_step_from_string(s: String) -> Result<CommandStep, String> {
	if let Some(rest) = s.strip_prefix('@') {
		// `@?` opts into "skip silently when the target doesn't exist". We strip
		// the `?` here so the in-memory `target` field never carries it; the
		// `optional` flag is the source of truth.
		let (rest, optional) = match rest.strip_prefix('?') {
			Some(after) => (after, true),
			None => (rest, false),
		};
		let (target, args) = match find_target_args_split(rest) {
			Some(idx) => (rest[..idx].to_string(), rest[idx..].trim_start().to_string()),
			None => (rest.to_string(), String::new()),
		};
		if target.is_empty() {
			return Err(if optional {
				"target invocation `@?` must be followed by a target name".to_string()
			} else {
				"target invocation `@` must be followed by a target name".to_string()
			});
		}
		Ok(CommandStep::TargetCall(TargetCallStep {
			target,
			args_template: args,
			optional,
		}))
	} else {
		Ok(CommandStep::Shell(s))
	}
}

/// A `@target [args...]` invocation parsed from a string command entry.
/// The leading `@` is stripped at parse time. An optional `?` after the `@`
/// (i.e. `@?target`) marks the call as **optional**: at execute time, if the
/// (substituted) target name doesn't exist in the merged Runfile, the call is
/// silently skipped rather than failing. `args_template` is the raw
/// post-target text (after the first whitespace run). At execute time it goes
/// through the normal substitution pipeline (so `{{ ARGS }}`, `{{ RUN.* }}`, etc.
/// resolve), then is split into argv via shell-style tokenization.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetCallStep {
	/// Target name (without the leading `@` / `@?`). Validated as non-empty at
	/// parse time; existence is checked at runtime against the merged Runfile.
	pub target: String,
	/// Argument template — substituted then shlex-split into argv. Empty if
	/// the user wrote `@target` with no args.
	pub args_template: String,
	/// When true, this call was written as `@?target`: if `target` doesn't
	/// resolve at execute time, the call is silently skipped (no error, no
	/// failure counted) rather than aborting the run. Useful with
	/// `for in: "namespaces"` patterns where some namespaces may not define
	/// a given target.
	#[serde(default, skip_serializing_if = "is_false")]
	pub optional: bool,
}

fn is_false(b: &bool) -> bool {
	!*b
}

/// Walk a slice of [`CommandStep`]s and call `visit` on every leaf template
/// string (see [`CommandStep::walk_templates`]).
pub fn walk_step_templates<'a, F: FnMut(&'a str)>(steps: &'a [CommandStep], visit: &mut F) {
	for step in steps {
		step.walk_templates(visit);
	}
}

/// Walk every non-`commands` template field on a [`CommandSpec`] and call
/// `visit` on each string that participates in `{{ ... }}` substitution.
///
/// Covers: `env` values (string variants only — numbers/bools have no
/// templates), `envFiles` paths, `forceShell`, `addToPath` entries,
/// `workingDirectory`, `confirm`, and `extendStdio.fromFile` paths.
///
/// Used by static analysis (arg-usage scanning) so references like
/// `{{ ARGS.x }}` / `{{ FLAGS.x }}` placed in `env` values, env-file paths, or
/// other auxiliary fields are recognised — without it the validator would
/// only see the `commands` array and reject otherwise-valid CLI args.
pub fn walk_spec_aux_templates<'a, F: FnMut(&'a str)>(spec: &'a CommandSpec, visit: &mut F) {
	if let Some(env) = &spec.env {
		for value in env.values() {
			if let EnvValue::String(s) = value {
				visit(s.as_str());
			}
		}
	}
	if let Some(files) = &spec.env_files {
		for f in files {
			visit(f.as_str());
		}
	}
	if let Some(s) = &spec.force_shell {
		visit(s.as_str());
	}
	if let Some(paths) = &spec.add_to_path {
		for p in paths {
			visit(p.as_str());
		}
	}
	if let Some(s) = &spec.working_directory {
		visit(s.as_str());
	}
	if let Some(s) = &spec.confirm {
		visit(s.as_str());
	}
	if let Some(items) = &spec.extend_stdio {
		for item in items {
			visit(item.from_file.as_str());
		}
	}
}

/// An `if` block within a `commands` array.
///
/// The condition is a substitution template — at runtime the string is
/// fully substituted via the executor's substitution machinery, and the
/// branch is taken when the resolved string is exactly `"true"`. Any other
/// value (including `"false"`, `"True"`, `"1"`, the empty string, etc.)
/// counts as falsy.
///
/// The OLD form `"{{ ARGS.env }} == prod"` (with operators outside the
/// `{{ ... }}` substitution) is no longer parsed at the if-level — that
/// DSL is reachable inside a substitution: `"{{ ARGS.env == 'prod' }}"`
/// resolves to `"true"` or `"false"` and is what the if then compares.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IfStep {
	/// The condition template. Resolved as a substitution string at runtime
	/// and compared against the literal `"true"`.
	#[serde(rename = "if")]
	pub condition: String,

	/// Steps executed when the condition is truthy. Accepts either a single
	/// shell-command string (sugared into a one-element list) or an array
	/// of command steps.
	#[serde(deserialize_with = "deserialize_steps_or_string")]
	pub then: Vec<CommandStep>,

	/// Steps executed when the condition is falsy. Optional. Same string/array
	/// shorthand as `then`.
	#[serde(
		default,
		rename = "else",
		skip_serializing_if = "Option::is_none",
		deserialize_with = "deserialize_optional_steps_or_string"
	)]
	pub r#else: Option<Vec<CommandStep>>,

	/// When true, failures inside this block do not flip the run's success state.
	#[serde(default, rename = "ignoreErrors", skip_serializing_if = "Option::is_none")]
	pub ignore_errors: Option<bool>,

	/// State guard for the entire `if` block. When `Some(Failure)` / `Some(Always)`,
	/// the block only runs after a prior failure / regardless of state. Default
	/// (or `Some(Success)`) means the block runs only while no prior failure.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub when: Option<WhenCondition>,
}

/// Deserialize either a single string (treated as one shell command) or an
/// array of [`CommandStep`]s. Used for the `then` / `else` fields of [`IfStep`]
/// so users can write `"then": "echo hi"` instead of `"then": ["echo hi"]`.
fn deserialize_steps_or_string<'de, D>(deserializer: D) -> Result<Vec<CommandStep>, D::Error>
where
	D: Deserializer<'de>,
{
	use serde::de::{self, SeqAccess, Visitor};
	use std::fmt;

	struct StepsOrString;
	impl<'de> Visitor<'de> for StepsOrString {
		type Value = Vec<CommandStep>;
		fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
			f.write_str("a shell command string or an array of command steps")
		}
		fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
			let step = command_step_from_string(v.to_string()).map_err(de::Error::custom)?;
			Ok(vec![step])
		}
		fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
			let step = command_step_from_string(v).map_err(de::Error::custom)?;
			Ok(vec![step])
		}
		fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
			let mut out = Vec::new();
			while let Some(step) = seq.next_element::<CommandStep>()? {
				out.push(step);
			}
			Ok(out)
		}
	}

	deserializer.deserialize_any(StepsOrString)
}

/// `Option`-aware variant of [`deserialize_steps_or_string`]. Serde only invokes
/// this when the field is present, so a missing `else` still yields `None` via
/// `#[serde(default)]`.
fn deserialize_optional_steps_or_string<'de, D>(deserializer: D) -> Result<Option<Vec<CommandStep>>, D::Error>
where
	D: Deserializer<'de>,
{
	deserialize_steps_or_string(deserializer).map(Some)
}

/// Iterator source for a `for` block's `in` field.
///
/// Either a literal array of values (each substitutable like every other
/// template), or a magic source resolved at execution time. Today the only
/// magic value is `"namespaces"`, which expands to every namespace prefix
/// declared via `includes` (composed across nested includes — a chain of
/// `outer:inner` shows up as both `outer` and `outer:inner`).
#[derive(Debug, Clone, PartialEq)]
pub enum ForInValue {
	/// Iterate over a literal array of strings (each goes through normal
	/// `{{ ... }}` substitution at execution time).
	Literal(Vec<String>),
	/// Iterate over every namespace prefix declared via `includes`.
	/// Resolved against the merged Runfile's `namespaces` list at execution
	/// time — order is alphabetical, deduplicated.
	Namespaces,
}

impl ForInValue {
	/// Magic string that selects [`ForInValue::Namespaces`] when used as the
	/// `in` value (i.e. `"in": "namespaces"`).
	pub const NAMESPACES_KEYWORD: &'static str = "namespaces";
}

impl Serialize for ForInValue {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			ForInValue::Literal(items) => items.serialize(serializer),
			ForInValue::Namespaces => serializer.serialize_str(Self::NAMESPACES_KEYWORD),
		}
	}
}

impl<'de> Deserialize<'de> for ForInValue {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		use serde_json::Value;
		let v = Value::deserialize(deserializer)?;
		match v {
			Value::Array(arr) => {
				let mut items = Vec::with_capacity(arr.len());
				for elem in arr {
					match elem {
						Value::String(s) => items.push(s),
						other => {
							return Err(serde::de::Error::custom(format!(
								"`in` array entries must be strings, got {other}"
							)));
						}
					}
				}
				Ok(ForInValue::Literal(items))
			}
			Value::String(s) if s == Self::NAMESPACES_KEYWORD => Ok(ForInValue::Namespaces),
			Value::String(s) => Err(serde::de::Error::custom(format!(
				"`in` string value must be \"{}\" (got \"{s}\"); for literal iteration use an array",
				Self::NAMESPACES_KEYWORD
			))),
			other => Err(serde::de::Error::custom(format!(
				"`in` must be an array of strings or the magic string \"{}\" (got {other})",
				Self::NAMESPACES_KEYWORD
			))),
		}
	}
}

/// A `for` block within a `commands` array.
///
/// Exactly one of `in`, `glob`, or `shell` must be set (validated at parse time).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ForStep {
	/// The loop variable name (referenced inside the body as `{{ VARS.<var> }}`).
	#[serde(rename = "for")]
	pub var: String,

	/// Iterate over either an explicit array of strings or a magic source
	/// (currently just `"namespaces"`). See [`ForInValue`].
	#[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
	pub r#in: Option<ForInValue>,

	/// Iterate over file paths matching this glob pattern (relative to the working directory).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub glob: Option<String>,

	/// Iterate over the lines of stdout produced by running this shell command.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub shell: Option<String>,

	/// The body steps (executed once per iteration).
	///
	/// Accepts either a single string (sugar for a one-element array) or a
	/// full array of [`CommandStep`]s.
	#[serde(rename = "do", deserialize_with = "deserialize_steps_or_string")]
	pub body: Vec<CommandStep>,

	/// When true, iterations run concurrently. Inner `for` blocks inside an outer
	/// parallel context are forced sequential regardless of this flag.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parallel: Option<bool>,

	/// When true, failures inside the body do not flip the run's success state.
	#[serde(default, rename = "ignoreErrors", skip_serializing_if = "Option::is_none")]
	pub ignore_errors: Option<bool>,

	/// State guard for the entire `for` block. See [`WhenCondition`].
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub when: Option<WhenCondition>,
}

/// A `match`-block within a `commands` array.
///
/// Resolves the `match` template through the normal substitution pipeline
/// (so `{{ ARGS.x }}`, `{{ ENV.X }}`, `{{ VARS.x }}`, chained fallbacks, etc. work),
/// then dispatches on string equality against `cases`. When no case matches
/// and no `default` is set, execution errors out — listing the valid cases
/// in the message — so users learn about valid values rather than silently
/// running through to subsequent steps.
///
/// Cases are stored in a [`BTreeMap`] so iteration order is deterministic
/// (alphabetical by key); error messages list valid values in that order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MatchStep {
	/// The substitution template to evaluate. Goes through the same
	/// substitution pipeline as any other `{{ ... }}` reference, so chained
	/// fallbacks (`{{ ARGS.tier ? ENV.TIER ? 1 }}`) and all source kinds are
	/// supported.
	#[serde(rename = "match")]
	pub r#match: String,

	/// Map from case value (compared against the resolved match value as a
	/// string) to the steps to run for that case. Each value accepts the
	/// usual string-or-array sugar.
	#[serde(deserialize_with = "deserialize_match_cases")]
	pub cases: BTreeMap<String, Vec<CommandStep>>,

	/// Steps run when no case matches the resolved value. Optional — when
	/// absent, an unmatched value is a hard error that lists the valid
	/// cases. Accepts the usual string-or-array sugar.
	#[serde(
		default,
		skip_serializing_if = "Option::is_none",
		deserialize_with = "deserialize_optional_steps_or_string"
	)]
	pub default: Option<Vec<CommandStep>>,

	/// When true, failures inside the chosen branch do not flip the run's
	/// success state. Same semantics as `if.ignoreErrors` /
	/// `for.ignoreErrors` / `when.ignoreErrors`.
	#[serde(default, rename = "ignoreErrors", skip_serializing_if = "Option::is_none")]
	pub ignore_errors: Option<bool>,

	/// State guard for the entire `match` block. Default (or `Some(Success)`)
	/// means the block runs only while no prior failure.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub when: Option<WhenCondition>,
}

/// Deserialize `MatchStep.cases`: a JSON object whose values may each be a
/// single shell-command string or an array of [`CommandStep`]s. Mirrors the
/// `then` / `else` / `do` shorthand of [`IfStep`] and [`ForStep`].
fn deserialize_match_cases<'de, D>(deserializer: D) -> Result<BTreeMap<String, Vec<CommandStep>>, D::Error>
where
	D: Deserializer<'de>,
{
	use serde_json::Value;

	let value = Value::deserialize(deserializer)?;
	let obj = match value {
		Value::Object(m) => m,
		_ => return Err(serde::de::Error::custom("`cases` must be an object")),
	};

	let mut out = BTreeMap::new();
	for (key, val) in obj {
		let steps = match val {
			Value::String(s) => {
				let step = command_step_from_string(s).map_err(serde::de::Error::custom)?;
				vec![step]
			}
			Value::Array(_) => serde_json::from_value::<Vec<CommandStep>>(val).map_err(serde::de::Error::custom)?,
			other => {
				return Err(serde::de::Error::custom(format!(
					"case \"{key}\" must be a shell-command string or array of command steps, got {other}"
				)));
			}
		};
		out.insert(key, steps);
	}
	Ok(out)
}

/// Top-level Runfile specification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Runfile {
	/// Schema version identifier.
	#[serde(rename = "$schema")]
	pub schema: String,

	/// Other Runfile.json files to include (paths relative to this file).
	///
	/// Each entry is either a plain path string (no namespace) or an object
	/// `{ "path": "...", "namespace": "..." }`. When a namespace is set, every
	/// target name and every `@target` reference inside that included file is
	/// prefixed with `<namespace>:`. Children are sealed: a `@target`
	/// reference inside an included file resolves to that file's own targets,
	/// never to the parent's. Nested includes compose left-to-right.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub includes: Option<Vec<IncludeEntry>>,

	/// Named targets that can be invoked.
	pub targets: HashMap<String, CommandSpec>,

	/// Optional global configuration.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub globals: Option<Globals>,

	/// Namespace prefixes declared via `includes`, sorted and deduplicated.
	/// Populated by the merge step — never serialized or deserialized. Used
	/// at execution time to resolve `for "in": "namespaces"` blocks. Composes
	/// across nested includes: a chain of `outer:inner` shows up as both
	/// `outer` and `outer:inner`.
	#[serde(skip)]
	pub namespaces: Vec<String>,
}

/// A single entry in the `includes` array. Accepts either a plain string
/// (path-only, no namespace) or an object `{ "path", "namespace" }`. The
/// object form's `namespace` is optional — an absent or empty namespace
/// behaves identically to the string form.
#[derive(Debug, Clone, PartialEq)]
pub struct IncludeEntry {
	/// Path to the included Runfile, relative to the file that declares it
	/// (or absolute, subject to the same path-traversal restriction as the
	/// string form).
	pub path: String,
	/// Optional namespace prefix applied to every target name and every
	/// `@target` reference inside the included file. `None` (or `Some("")`,
	/// which is normalised to `None` at parse time) means no rewrite — the
	/// include behaves exactly like the historical string-form entry.
	pub namespace: Option<String>,
}

impl Serialize for IncludeEntry {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		use serde::ser::SerializeStruct;
		match &self.namespace {
			None => serializer.serialize_str(&self.path),
			Some(ns) => {
				let mut s = serializer.serialize_struct("IncludeEntry", 2)?;
				s.serialize_field("path", &self.path)?;
				s.serialize_field("namespace", ns)?;
				s.end()
			}
		}
	}
}

impl<'de> Deserialize<'de> for IncludeEntry {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		use serde_json::Value;
		let v = Value::deserialize(deserializer)?;
		match v {
			Value::String(path) => Ok(IncludeEntry { path, namespace: None }),
			Value::Object(map) => {
				#[derive(Deserialize)]
				#[serde(deny_unknown_fields)]
				struct Obj {
					path: String,
					#[serde(default)]
					namespace: Option<String>,
				}
				let obj: Obj = serde_json::from_value(Value::Object(map)).map_err(serde::de::Error::custom)?;
				let ns = obj.namespace.and_then(|s| if s.is_empty() { None } else { Some(s) });
				Ok(IncludeEntry {
					path: obj.path,
					namespace: ns,
				})
			}
			_ => Err(serde::de::Error::custom(
				"include entry must be a string or an object with a \"path\" key",
			)),
		}
	}
}

/// A single command specification (target).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
	/// Optional human-readable description of what this command does.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,

	/// Optional alternative names for this target.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub aliases: Option<Vec<String>>,

	/// List of command steps to execute sequentially. Each entry is either a
	/// raw shell command string or a control-flow block (`if` / `for`).
	///
	/// Accepts either a single string (sugar for a one-element array) or a
	/// full array of [`CommandStep`]s.
	#[serde(deserialize_with = "deserialize_steps_or_string")]
	pub commands: Vec<CommandStep>,

	/// Optional file paths to load environment variables from (loaded before `env`).
	#[serde(default, rename = "envFiles", skip_serializing_if = "Option::is_none")]
	pub env_files: Option<Vec<String>>,

	/// Optional environment variables specific to this command.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub env: Option<HashMap<String, EnvValue>>,

	/// Force using a specific shell for this command (overrides globals.forceShell).
	#[serde(default, rename = "forceShell", skip_serializing_if = "Option::is_none")]
	pub force_shell: Option<String>,

	/// Directories to prepend to PATH for this command (merged with globals.addToPath).
	#[serde(default, rename = "addToPath", skip_serializing_if = "Option::is_none")]
	pub add_to_path: Option<Vec<String>>,

	/// When true, print each command to stderr before executing it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub logging: Option<bool>,

	/// When true, continue executing subsequent commands even if one fails.
	#[serde(default, rename = "ignoreErrors", skip_serializing_if = "Option::is_none")]
	pub ignore_errors: Option<bool>,

	/// When true, spawn the commands as a detached background process and exit immediately.
	/// Requires `parallel: true` when there are multiple commands.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub detach: Option<bool>,

	/// When true, execute all commands in parallel instead of sequentially.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parallel: Option<bool>,

	/// Working directory for command execution — a free-form path that supports
	/// `{{ ... }}` substitution. Defaults to `{{ RUN.parent }}` (the source
	/// Runfile's directory) when unset. Relative paths are resolved against
	/// the target's source Runfile directory.
	#[serde(default, rename = "workingDirectory", skip_serializing_if = "Option::is_none")]
	pub working_directory: Option<String>,

	/// Prompt message shown to the user before executing. Requires y/N confirmation.
	/// Skipped in CI environments or when --yes is passed.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub confirm: Option<String>,

	/// When true, forcefully kill the spawned process tree on SIGINT/CTRL+C.
	/// Useful for GUI-subsystem apps (e.g. Unity) that don't respond to console signals.
	#[serde(default, rename = "forceKillOnSigInt", skip_serializing_if = "Option::is_none")]
	pub force_kill_on_sig_int: Option<bool>,

	/// Log files to tail and route to stdout/stderr during command execution.
	#[serde(default, rename = "extendStdio", skip_serializing_if = "Option::is_none")]
	pub extend_stdio: Option<Vec<ExtendStdio>>,

	/// Glob patterns for watch mode. When present, the target automatically
	/// re-runs whenever matching files change. Use ! prefix to exclude.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub watch: Option<Vec<String>>,

	/// Restrict this target to only be available when the current directory is at
	/// or under one of the specified paths (relative to the Runfile location).
	#[serde(default, rename = "onlyInDirectories", skip_serializing_if = "Option::is_none")]
	pub only_in_directories: Option<Vec<String>>,

	/// Free-form metadata for this target. Merged from globals' `metadata`
	/// at parse time, with target-level keys winning over global keys.
	/// Currently only [`Metadata::exclude_from_generate_command`] is consumed
	/// by built-in tooling (the editor-config generators); other keys round-trip
	/// untouched and can be used by external tools.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub metadata: Option<Metadata>,
}

impl CommandSpec {
	/// Convenience: create a `CommandSpec` from a `Vec<String>` of shell
	/// commands. Each string is wrapped in [`CommandStep::Shell`].
	pub fn new_shell(commands: Vec<String>) -> Self {
		Self::new(commands.into_iter().map(CommandStep::Shell).collect())
	}

	/// Create a new `CommandSpec` with the given command steps and all optional fields set to `None`.
	pub fn new(commands: Vec<CommandStep>) -> Self {
		Self {
			description: None,
			aliases: None,
			commands,
			env_files: None,
			env: None,
			force_shell: None,
			add_to_path: None,
			logging: None,
			ignore_errors: None,
			detach: None,
			parallel: None,
			working_directory: None,
			confirm: None,
			force_kill_on_sig_int: None,
			extend_stdio: None,
			watch: None,
			only_in_directories: None,
			metadata: None,
		}
	}

	/// Whether this target opts out of editor-config generators
	/// (`run :generate vscode-tasks` / `zed-tasks` / `jetbrains-run-configurations`).
	pub fn is_excluded_from_generate(&self) -> bool {
		self.metadata
			.as_ref()
			.and_then(|m| m.exclude_from_generate_command)
			.unwrap_or(false)
	}
}

/// Free-form metadata, attached to globals or to an individual target.
///
/// Globals' `metadata` is merged into each target at parse time
/// (see [`crate::merge::merge_metadata`]): for keys both define, the target
/// value wins; otherwise the global value carries through. Unknown keys are
/// preserved in [`Metadata::extra`] so external tooling can stash its own
/// fields here without parser errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Metadata {
	/// When true, editor-config generators (`run :generate vscode-tasks`,
	/// `zed-tasks`, `jetbrains-run-configurations`) skip this target entirely:
	/// no task/run-configuration is added, and existing entries with a matching
	/// label are NOT updated. Default: false.
	#[serde(
		default,
		rename = "excludeFromGenerateCommand",
		skip_serializing_if = "Option::is_none"
	)]
	pub exclude_from_generate_command: Option<bool>,

	/// Untyped extra fields. Round-trip through serde; used for tool-specific
	/// metadata that Runfile itself doesn't interpret.
	#[serde(flatten)]
	pub extra: HashMap<String, serde_json::Value>,
}

impl Metadata {
	/// True when no field is set (neither known fields nor [`Self::extra`]).
	/// Used by the merge layer to decide whether to attach a `Metadata` block
	/// to a target at all.
	pub fn is_empty(&self) -> bool {
		self.exclude_from_generate_command.is_none() && self.extra.is_empty()
	}
}

/// Internal targets cannot be invoked directly from the CLI and are hidden from
/// `:list`, shell completions, MCP, and editor task generators. They are still
/// fully usable from `@target` invocations inside another target's commands.
///
/// A target is considered internal when the **last** `:`-separated segment of
/// its canonical name starts with `_`. This means `_helper` is internal, and
/// `child:_helper` (a namespaced internal target from an included file) is
/// also internal — internal-ness rides along with the canonical name through
/// namespacing.
pub fn is_internal_target_name(name: &str) -> bool {
	let last = name.rsplit_once(':').map_or(name, |(_, last)| last);
	last.starts_with('_')
}

impl Runfile {
	/// Resolve a target name or alias to the canonical target name.
	/// Returns `None` if neither a target nor an alias matches.
	pub fn resolve_target<'a>(&'a self, name: &'a str) -> Option<&'a str> {
		if self.targets.contains_key(name) {
			return Some(name);
		}
		for (target_name, spec) in &self.targets {
			if let Some(aliases) = &spec.aliases {
				if aliases.iter().any(|a| a == name) {
					return Some(target_name);
				}
			}
		}
		None
	}

	/// Whether the given name (target or alias) refers to an internal target.
	/// Returns `false` for unknown names.
	pub fn is_internal(&self, name: &str) -> bool {
		self.resolve_target(name).is_some_and(is_internal_target_name)
	}

	/// Collect all invocable names: target names + aliases.
	/// Includes internal targets — use [`Self::public_target_names`] to exclude them.
	pub fn all_target_names(&self) -> Vec<&str> {
		let mut names: Vec<&str> = Vec::new();
		for (name, spec) in &self.targets {
			names.push(name);
			if let Some(aliases) = &spec.aliases {
				for alias in aliases {
					names.push(alias);
				}
			}
		}
		names.sort();
		names
	}

	/// Collect public invocable names: target names + aliases, excluding internal targets.
	pub fn public_target_names(&self) -> Vec<&str> {
		let mut names: Vec<&str> = Vec::new();
		for (name, spec) in &self.targets {
			if is_internal_target_name(name) {
				continue;
			}
			names.push(name);
			if let Some(aliases) = &spec.aliases {
				for alias in aliases {
					names.push(alias);
				}
			}
		}
		names.sort();
		names
	}
}

/// Which standard stream to route log file contents to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum StdioStream {
	Stdout,
	Stderr,
}

/// A log file to tail and route to stdout or stderr during execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtendStdio {
	/// Path to the log file to tail. Relative paths are resolved from the working directory.
	#[serde(rename = "fromFile")]
	pub from_file: String,

	/// Which stream to route the file contents to.
	pub stream: StdioStream,
}

/// Default `workingDirectory` value applied when a target (and globals) does
/// not specify one. Resolves to the directory of the currently-executing
/// target's source Runfile.
///
/// `workingDirectory` itself is a free-form path string that supports
/// `{{ ... }}` substitution (typically via `{{ RUN.parent }}` /
/// `{{ RUN.cwd }}` / `{{ RUN.file }}`). After substitution, relative paths
/// are resolved against `RUN.parent` (the target's source dir).
pub const WORKING_DIRECTORY_DEFAULT: &str = "{{ RUN.parent }}";

/// An environment variable value — can be a string or a number.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum EnvValue {
	String(String),
	Number(f64),
	Bool(bool),
}

impl EnvValue {
	/// Convert the value to its string representation for env var assignment.
	pub fn to_env_string(&self) -> String {
		match self {
			EnvValue::String(s) => s.clone(),
			EnvValue::Number(n) => {
				if *n == (*n as i64) as f64 {
					(*n as i64).to_string()
				} else {
					n.to_string()
				}
			}
			EnvValue::Bool(b) => b.to_string(),
		}
	}
}

/// Global configuration that applies to all commands.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Globals {
	/// Directories to prepend to PATH before running commands.
	#[serde(default, rename = "addToPath", skip_serializing_if = "Option::is_none")]
	pub add_to_path: Option<Vec<String>>,

	/// File paths to load environment variables from (loaded before `env`).
	#[serde(default, rename = "envFiles", skip_serializing_if = "Option::is_none")]
	pub env_files: Option<Vec<String>>,

	/// Environment variables to set for all commands.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub env: Option<HashMap<String, EnvValue>>,

	/// Force using a specific shell.
	#[serde(default, rename = "forceShell", skip_serializing_if = "Option::is_none")]
	pub force_shell: Option<String>,

	/// When true, print each command to stderr before executing it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub logging: Option<bool>,

	/// When true, continue executing subsequent commands even if one fails.
	#[serde(default, rename = "ignoreErrors", skip_serializing_if = "Option::is_none")]
	pub ignore_errors: Option<bool>,

	/// Working directory for command execution — a free-form path that supports
	/// `{{ ... }}` substitution. Defaults to `{{ RUN.parent }}` (the source
	/// Runfile's directory) when unset. Relative paths are resolved against
	/// the target's source Runfile directory.
	#[serde(default, rename = "workingDirectory", skip_serializing_if = "Option::is_none")]
	pub working_directory: Option<String>,

	/// When true, forcefully kill the spawned process tree on SIGINT/CTRL+C.
	/// Useful for GUI-subsystem apps (e.g. Unity) that don't respond to console signals.
	#[serde(default, rename = "forceKillOnSigInt", skip_serializing_if = "Option::is_none")]
	pub force_kill_on_sig_int: Option<bool>,

	/// Restrict this Runfile to only be available at the specified directory
	/// paths or their children (relative to the Runfile location).
	#[serde(default, rename = "onlyInDirectories", skip_serializing_if = "Option::is_none")]
	pub only_in_directories: Option<Vec<String>>,

	/// Free-form metadata applied to every target in this Runfile. Merged into
	/// each target's own `metadata` at parse time — target keys win over
	/// global keys, otherwise globals carry through.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub metadata: Option<Metadata>,
}
