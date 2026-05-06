use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubstitutionError {
	#[error("Argument \"{0}\" was not provided and has no default value. Use $(ARGS.{0} ? default) to set a default, or $(ARGS.{0} ?) for empty string.")]
	MissingArg(String),

	#[error("Environment variable \"{0}\" is not set and has no default value. Use $(ENV.{0} ? default) to set a default, or $(ENV.{0} ?) for empty string.")]
	MissingEnv(String),

	#[error("Loop variable \"{0}\" is not in scope. `$(LOOP.{0})` only resolves inside the body of a `for` block whose `for` field is \"{0}\".")]
	MissingLoopVar(String),

	#[error("Unknown $(RUN.{0}) key. Valid keys are: os, shell.")]
	UnknownRunKey(String),

	#[error(
		"Duplicate environment variable with different casing: \"{0}\" and \"{1}\". Use a single consistent casing."
	)]
	DuplicateEnvCasing(String, String),

	#[error("No command in this target accepts arguments, but arguments were provided: {0}")]
	UnexpectedArgs(String),

	#[error("Unknown named argument \"--{0}\". No command uses $(ARGS.{0}).")]
	UnknownNamedArg(String),
}

/// Static execution context — the OS, the resolved shell, and the merged
/// Runfile's namespace list.
///
/// Used to resolve `$(RUN.*)` substitutions inside command templates,
/// `if` conditions, `for-in`/`for-glob`/`for-shell` iterators, and any other
/// place where substitution happens. Also carries the namespace list so
/// `for "in": "namespaces"` resolves without threading another parameter
/// through every executor function.
///
/// Valid `RUN.*` keys:
/// - `RUN.os` — `"windows"`, `"linux"`, or `"mac"`
/// - `RUN.shell` — `"bash"`, `"zsh"`, `"sh"`, `"fish"`, `"powershell"`, or `"cmd"`
#[derive(Debug, Clone, Default)]
pub struct RunContext {
	/// Operating system: `"windows"`, `"linux"`, or `"mac"`.
	pub os: String,
	/// The resolved shell name (lowercase): `"bash"`, `"zsh"`, `"sh"`, `"fish"`,
	/// `"powershell"`, or `"cmd"`.
	pub shell: String,
	/// Namespace prefixes from the merged Runfile, sorted and deduplicated.
	/// Wrapped in `Arc` so cloning a [`RunContext`] (and thus [`RunArgs`])
	/// across `@dep` boundaries and worker threads stays cheap. Empty for
	/// callers that don't have a Runfile (CLI completion, isolated tests).
	pub namespaces: Arc<Vec<String>>,
}

impl RunContext {
	/// Build a context from the current process OS and a shell name. The
	/// namespace list defaults to empty — populate it via
	/// [`RunContext::with_namespaces`] once the merged Runfile is available.
	pub fn new(shell: impl Into<String>) -> Self {
		Self {
			os: detect_current_os().to_string(),
			shell: shell.into(),
			namespaces: Arc::new(Vec::new()),
		}
	}

	/// Builder: attach a namespace list. Overwrites any previously-attached
	/// list. Pass an `Arc` so the same allocation is shared across cloned
	/// contexts.
	pub fn with_namespaces(mut self, namespaces: Arc<Vec<String>>) -> Self {
		self.namespaces = namespaces;
		self
	}
}

/// Detect the current operating system. Returned values are the same strings exposed
/// via `$(RUN.os)`: `"windows"`, `"mac"`, or `"linux"`.
pub(crate) fn detect_current_os() -> &'static str {
	if cfg!(target_os = "windows") {
		"windows"
	} else if cfg!(target_os = "macos") {
		"mac"
	} else {
		"linux"
	}
}

/// A stack of currently-active `for`-loop bindings. Used to resolve
/// `$(LOOP.<name>)` expressions inside the body of a `for` block.
///
/// Innermost bindings are looked up first (lexical scoping).
#[derive(Debug, Clone, Default)]
pub struct LoopScope {
	bindings: Vec<(String, String)>,
}

impl LoopScope {
	/// Empty scope — the default.
	pub fn new() -> Self {
		Self { bindings: Vec::new() }
	}

	/// Push a new binding (innermost).
	pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
		self.bindings.push((name.into(), value.into()));
	}

	/// Pop the most recent binding. No-op if empty.
	pub fn pop(&mut self) {
		self.bindings.pop();
	}

	/// Look up a name. Innermost match wins.
	pub fn get(&self, name: &str) -> Option<&str> {
		self.bindings
			.iter()
			.rev()
			.find_map(|(k, v)| (k == name).then_some(v.as_str()))
	}

	/// Whether this scope is empty (used as a fast path).
	pub fn is_empty(&self) -> bool {
		self.bindings.is_empty()
	}
}

/// Prompts the user for missing substitution values when `--stdin-args` is
/// enabled. The interactive impl reads from stdin; tests substitute a mock.
///
/// Implementations MUST cache their answers internally — `prompt_value` and
/// `prompt_flag` may be called many times for the same key (once per
/// substitution site, plus once per redacted-logging pass), and asking the
/// user the same thing twice is a poor UX. The cache is also what lets a
/// shared prompter (cloned via `Arc`) propagate prompted values into nested
/// `@target` invocations without re-prompting.
pub trait StdinPrompter: Send + Sync + std::fmt::Debug {
	/// Prompt for a substitution value. `key` identifies the source (e.g.
	/// `"ARGS.foo"`, `"ENV.HOST"`); `default`, when `Some`, is shown to the
	/// user and indicates that an empty response should fall through to the
	/// chain default. Returns `Some(value)` if the user supplied a non-empty
	/// answer (overrides the chain), or `None` if the answer was empty
	/// (caller falls through to the next chain segment / default / error).
	fn prompt_value(&self, key: &str, default: Option<&str>) -> Option<String>;

	/// Prompt for a CLI flag (`$(FLAGS.x)`). Returns `true` if the flag
	/// should be considered present, `false` otherwise.
	fn prompt_flag(&self, key: &str) -> bool;
}

/// Default `StdinPrompter` impl: reads from stdin, writes prompts to stderr,
/// caches answers in process memory. Cloning via `Arc` shares the cache —
/// nested `@target` invocations re-use prompted values rather than re-asking.
#[derive(Debug, Default)]
pub struct InteractiveStdinPrompter {
	value_cache: Mutex<HashMap<String, String>>,
	flag_cache: Mutex<HashMap<String, bool>>,
}

impl InteractiveStdinPrompter {
	pub fn new() -> Self {
		Self::default()
	}
}

impl StdinPrompter for InteractiveStdinPrompter {
	fn prompt_value(&self, key: &str, default: Option<&str>) -> Option<String> {
		{
			let cache = self.value_cache.lock().unwrap();
			if let Some(cached) = cache.get(key) {
				return if cached.is_empty() { None } else { Some(cached.clone()) };
			}
		}

		let suffix = match default {
			Some("") => " \x1b[2m[empty]\x1b[0m: ".to_string(),
			Some(d) => format!(" \x1b[2m[{}]\x1b[0m: ", d),
			None => " \x1b[2m(required)\x1b[0m: ".to_string(),
		};
		eprint!("\x1b[1m\x1b[36m[runfile]\x1b[0m enter \x1b[1m{}{}", key, suffix);
		let _ = std::io::stderr().flush();

		let mut input = String::new();
		let _ = std::io::stdin().read_line(&mut input);
		let trimmed = input.trim_end_matches(['\n', '\r']).to_string();

		self.value_cache
			.lock()
			.unwrap()
			.insert(key.to_string(), trimmed.clone());

		if trimmed.is_empty() {
			None
		} else {
			Some(trimmed)
		}
	}

	fn prompt_flag(&self, key: &str) -> bool {
		{
			let cache = self.flag_cache.lock().unwrap();
			if let Some(&cached) = cache.get(key) {
				return cached;
			}
		}

		eprint!(
			"\x1b[1m\x1b[36m[runfile]\x1b[0m pass \x1b[1m{}\x1b[0m? \x1b[2m(y/N)\x1b[0m: ",
			key
		);
		let _ = std::io::stderr().flush();

		let mut input = String::new();
		let _ = std::io::stdin().read_line(&mut input);
		let answer = input.trim().to_lowercase();
		let value = matches!(answer.as_str(), "y" | "yes" | "true" | "1");

		self.flag_cache.lock().unwrap().insert(key.to_string(), value);
		value
	}
}

/// Check for duplicate env var keys with different casing.
/// Delegates to `runfile_env::check_env_case_duplicates` and converts the error type.
pub fn check_env_case_duplicates(env: &HashMap<String, String>) -> Result<(), SubstitutionError> {
	runfile_env::check_env_case_duplicates(env).map_err(|e| match e {
		runfile_env::EnvError::DuplicateEnvCasing(a, b) => SubstitutionError::DuplicateEnvCasing(a, b),
		_ => unreachable!(),
	})
}

/// Parsed user arguments from the CLI invocation.
/// Supports both positional args ($(ARGS)) and named args ($(ARGS.name ? default)).
#[derive(Debug, Clone, Default)]
pub struct RunArgs {
	/// The original arguments exactly as passed, in order.
	pub original: Vec<String>,
	/// Named arguments parsed from `--key=value` or `--key value` pairs.
	pub named: HashMap<String, String>,
	/// Static execution context (OS, shell) used to resolve `$(RUN.*)`
	/// substitutions. The default is `os` set to the current OS and `shell`
	/// empty — callers (the CLI, the runner) overwrite this with the actual
	/// values once the shell has been resolved.
	pub run_context: RunContext,
	/// When `Some`, missing `$(ARGS.*)` / `$(ENV.*)` / `$(FLAGS.*)` references
	/// are prompted via this prompter instead of erroring. Cloning a
	/// `RunArgs` shares the same prompter (and therefore the same answer
	/// cache) — propagated through `@target` invocations so the user is not
	/// re-asked for the same value.
	pub stdin_prompter: Option<Arc<dyn StdinPrompter>>,
}

impl RunArgs {
	/// Parse CLI arguments into RunArgs.
	/// Named args: `--key=value` or `--key value`
	/// Everything is also preserved in `original` for $(ARGS) expansion.
	pub fn parse(args: &[String]) -> Self {
		let original = args.to_vec();
		let mut named = HashMap::new();
		let mut iter = args.iter().peekable();

		while let Some(arg) = iter.next() {
			if let Some(stripped) = arg.strip_prefix("--") {
				if let Some((key, value)) = stripped.split_once('=') {
					named.insert(key.to_string(), value.to_string());
				} else if !stripped.is_empty() {
					if let Some(next) = iter.peek() {
						if !next.starts_with("--") {
							let val = iter.next().unwrap().clone();
							named.insert(stripped.to_string(), val);
						} else {
							named.insert(stripped.to_string(), String::new());
						}
					} else {
						named.insert(stripped.to_string(), String::new());
					}
				}
			}
		}

		RunArgs {
			original,
			named,
			run_context: RunContext {
				os: detect_current_os().to_string(),
				..Default::default()
			},
			stdin_prompter: None,
		}
	}

	/// Builder: attach a [`RunContext`] (used to resolve `$(RUN.*)`).
	/// Returns the modified args by value so it composes with [`RunArgs::parse`].
	pub fn with_run_context(mut self, run_context: RunContext) -> Self {
		self.run_context = run_context;
		self
	}

	/// Builder: attach a [`StdinPrompter`]. Pass `None` to keep the existing
	/// fail-on-missing behaviour. Composes with [`RunArgs::parse`] /
	/// [`RunArgs::with_run_context`].
	pub fn with_stdin_prompter(mut self, prompter: Option<Arc<dyn StdinPrompter>>) -> Self {
		self.stdin_prompter = prompter;
		self
	}

	/// Substitute `$(ARGS)`, `$(ARGS.key)`, `$(ENV.key)`, and chained
	/// fallback expressions in a string.
	///
	/// Syntax:
	/// - `$(ARGS)` — all remaining positional arguments
	/// - `$(ARGS.key)` — named arg, error if missing
	/// - `$(ARGS.key ? default)` — named arg with default
	/// - `$(ENV.key)` — env var, error if missing
	/// - `$(ENV.key ? default)` — env var with default
	/// - `$(LOOP.var)` — `for`-loop binding (errors if not in scope)
	/// - `$(ARGS.key ? ENV.key ? default)` — chained fallback
	///
	/// Environment variable lookups are case-insensitive.
	pub fn substitute(&self, input: &str, env: &HashMap<String, String>) -> Result<String, SubstitutionError> {
		self.substitute_with_loop(input, env, &LoopScope::new())
	}

	/// Like [`substitute`], but produces a redacted version suitable for logging.
	/// `$(ENV.*)` values are replaced with `***` to prevent leaking secrets.
	/// `$(ARGS.*)` and `$(FLAGS.*)` values are shown as-is (they are user-visible CLI input).
	pub fn substitute_redacted(&self, input: &str, env: &HashMap<String, String>) -> Result<String, SubstitutionError> {
		self.substitute_redacted_with_loop(input, env, &LoopScope::new())
	}

	/// [`substitute`] with an explicit loop scope.
	pub fn substitute_with_loop(
		&self,
		input: &str,
		env: &HashMap<String, String>,
		loop_scope: &LoopScope,
	) -> Result<String, SubstitutionError> {
		let mut consumed_keys: HashSet<String> = HashSet::new();
		let mut flag_keys: HashSet<String> = HashSet::new();
		let first_pass =
			self.resolve_placeholders_impl(input, env, loop_scope, &mut consumed_keys, &mut flag_keys, false)?;

		let remaining = self.build_remaining_args(&consumed_keys, &flag_keys);
		let output = first_pass.replace("$(ARGS)", &remaining);

		Ok(output)
	}

	/// [`substitute_redacted`] with an explicit loop scope.
	pub fn substitute_redacted_with_loop(
		&self,
		input: &str,
		env: &HashMap<String, String>,
		loop_scope: &LoopScope,
	) -> Result<String, SubstitutionError> {
		let mut consumed_keys: HashSet<String> = HashSet::new();
		let mut flag_keys: HashSet<String> = HashSet::new();
		let first_pass =
			self.resolve_placeholders_impl(input, env, loop_scope, &mut consumed_keys, &mut flag_keys, true)?;

		let remaining = self.build_remaining_args(&consumed_keys, &flag_keys);
		let output = first_pass.replace("$(ARGS)", &remaining);

		Ok(output)
	}

	fn prompter(&self) -> Option<&dyn StdinPrompter> {
		self.stdin_prompter.as_deref()
	}

	/// Resolve all $(...) placeholders that start with ARGS., FLAGS., ENV., or LOOP.
	/// When `redact_env` is true, resolved `$(ENV.*)` values are replaced with `***`.
	fn resolve_placeholders_impl(
		&self,
		input: &str,
		env: &HashMap<String, String>,
		loop_scope: &LoopScope,
		consumed: &mut HashSet<String>,
		flag_keys: &mut HashSet<String>,
		redact_env: bool,
	) -> Result<String, SubstitutionError> {
		let mut output = String::new();
		let mut chars = input.chars().peekable();

		while let Some(ch) = chars.next() {
			if ch == '$' && chars.peek() == Some(&'(') {
				chars.next(); // consume '('
				let mut expr = String::new();
				let mut depth = 1;
				for c in chars.by_ref() {
					if c == '(' {
						depth += 1;
					} else if c == ')' {
						depth -= 1;
						if depth == 0 {
							break;
						}
					}
					expr.push(c);
				}

				let trimmed = expr.trim();
				if trimmed == "ARGS" {
					// Leave $(ARGS) for the second pass
					output.push_str("$(ARGS)");
				} else if trimmed.starts_with("FLAGS.") {
					let resolved = resolve_flag(trimmed, &self.named, consumed, flag_keys, self.prompter())?;
					output.push_str(&resolved);
				} else if trimmed.starts_with("ARGS.")
					|| trimmed.starts_with("ENV.")
					|| trimmed.starts_with("LOOP.")
					|| trimmed.starts_with("RUN.")
				{
					let resolved = resolve_chain_impl(
						trimmed,
						&self.named,
						env,
						loop_scope,
						&self.run_context,
						consumed,
						redact_env,
						self.prompter(),
					)?;
					output.push_str(&resolved);
				} else {
					// Unknown head (e.g. a shell `$(echo …)` command substitution).
					// Recursively substitute the body so nested known prefixes —
					// `$(ARGS.x)`, `$(ENV.x)`, `$(LOOP.x)`, `$(RUN.x)`, `$(FLAGS.x)` —
					// still resolve, then re-emit the wrapping `$(...)` for the shell.
					let inner =
						self.resolve_placeholders_impl(&expr, env, loop_scope, consumed, flag_keys, redact_env)?;
					output.push('$');
					output.push('(');
					output.push_str(&inner);
					output.push(')');
				}
			} else {
				output.push(ch);
			}
		}

		Ok(output)
	}

	/// Build the $(ARGS) replacement string: all original args minus the tokens
	/// belonging to consumed named keys. Flag-consumed keys only remove the
	/// `--key` token, not the following value token.
	fn build_remaining_args(&self, consumed_keys: &HashSet<String>, flag_keys: &HashSet<String>) -> String {
		// Collect all tokens that should be removed
		let mut remove_tokens: HashSet<usize> = HashSet::new();
		let mut i = 0;
		while i < self.original.len() {
			let arg = &self.original[i];
			if let Some(stripped) = arg.strip_prefix("--") {
				if let Some((key, _)) = stripped.split_once('=') {
					// --key=value (single token)
					if consumed_keys.contains(key) {
						remove_tokens.insert(i);
					}
				} else if !stripped.is_empty() && consumed_keys.contains(stripped) {
					// --key value (two tokens) or --key (flag)
					remove_tokens.insert(i);
					// For flag-consumed keys, only remove the --key token (not the value).
					// For ARGS-consumed keys, also remove the following value token.
					if !flag_keys.contains(stripped)
						&& i + 1 < self.original.len()
						&& !self.original[i + 1].starts_with("--")
						&& self.named.get(stripped).is_some_and(|v| !v.is_empty())
					{
						remove_tokens.insert(i + 1);
						i += 1; // skip the value token
					}
				}
			}
			i += 1;
		}

		self.original
			.iter()
			.enumerate()
			.filter(|(idx, _)| !remove_tokens.contains(idx))
			.map(|(_, s)| s.as_str())
			.collect::<Vec<_>>()
			.join(" ")
	}
}

/// Resolve a chained fallback expression like `ARGS.key ? ENV.key ? LOOP.var ? RUN.os ? default`.
///
/// The chain is split on `?` delimiters. Each segment is tried in order:
/// - `ARGS.key` — look up in named args (marks key as consumed)
/// - `ENV.key` — look up in env map (case-insensitive)
/// - `LOOP.var` — look up in the active loop scope
/// - `RUN.{os|shell}` — look up in the static [`RunContext`]
/// - anything else — literal default value
///
/// If the last segment is a bare `ARGS.key`, `ENV.key`, `LOOP.var`, or
/// `RUN.<unknown>` with no `?` after it, it's an error if the value is missing.
///
/// When `redact_env` is true, resolved ENV values are replaced with `***`.
///
/// When `prompter` is `Some` (the caller passed `--stdin-args`), missing
/// `ARGS.*` / `ENV.*` references trigger a stdin prompt instead of an
/// immediate error. The prompt key is the FIRST `ARGS.*` / `ENV.*` segment
/// in the chain (the user-facing "primary name"); the prompt's default-hint
/// is the literal default segment if any. A non-empty answer overrides the
/// chain; an empty answer falls through to the chain's default (or to the
/// last source's missing-value error). `LOOP.*` and `RUN.*` are never
/// prompted — they are runtime context, not user input.
#[allow(clippy::too_many_arguments)]
fn resolve_chain_impl(
	expr: &str,
	named_args: &HashMap<String, String>,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
	run_context: &RunContext,
	consumed: &mut HashSet<String>,
	redact_env: bool,
	prompter: Option<&dyn StdinPrompter>,
) -> Result<String, SubstitutionError> {
	let segments: Vec<&str> = expr.splitn(usize::MAX, '?').collect();

	let mut last_error: Option<SubstitutionError> = None;
	let mut prompt_key: Option<String> = None;

	for (i, segment) in segments.iter().enumerate() {
		let seg = segment.trim();
		let is_last = i == segments.len() - 1;

		if let Some(key) = seg.strip_prefix("ARGS.") {
			let key = key.trim();
			if let Some(val) = named_args.get(key) {
				consumed.insert(key.to_string());
				return Ok(val.clone());
			}
			if prompter.is_some() && prompt_key.is_none() {
				prompt_key = Some(format!("ARGS.{}", key));
			}
			last_error = Some(SubstitutionError::MissingArg(key.to_string()));
		} else if let Some(key) = seg.strip_prefix("ENV.") {
			let key = key.trim();
			if let Some(val) = env_get_case_insensitive(env, key) {
				return Ok(if redact_env { "***".to_string() } else { val.to_string() });
			}
			if prompter.is_some() && prompt_key.is_none() {
				prompt_key = Some(format!("ENV.{}", key));
			}
			last_error = Some(SubstitutionError::MissingEnv(key.to_string()));
		} else if let Some(key) = seg.strip_prefix("LOOP.") {
			let key = key.trim();
			if let Some(val) = loop_scope.get(key) {
				return Ok(val.to_string());
			}
			if is_last {
				last_error = Some(SubstitutionError::MissingLoopVar(key.to_string()));
			}
		} else if let Some(key) = seg.strip_prefix("RUN.") {
			let key = key.trim();
			match resolve_run_key(key, run_context) {
				Some(val) => return Ok(val),
				None if is_last => {
					last_error = Some(SubstitutionError::UnknownRunKey(key.to_string()));
				}
				None => continue,
			}
		} else {
			// Literal default segment. With `--stdin-args`, prompt FIRST so
			// the user can override the default; an empty answer falls
			// through to the default itself.
			if let (Some(p), Some(pk)) = (prompter, &prompt_key) {
				if let Some(val) = p.prompt_value(pk, Some(seg)) {
					return Ok(val);
				}
			}
			return Ok(seg.to_string());
		}
	}

	// No literal default reached. With `--stdin-args`, give the user one last
	// chance to supply a value; an empty answer falls through to the chain's
	// missing-value error.
	if let (Some(p), Some(pk)) = (prompter, &prompt_key) {
		if let Some(val) = p.prompt_value(pk, None) {
			return Ok(val);
		}
	}

	if let Some(err) = last_error {
		Err(err)
	} else {
		Ok(String::new())
	}
}

/// Resolve a single `RUN.<key>` lookup against the [`RunContext`].
/// Returns `None` for unrecognised keys so the caller can decide whether
/// to fall through to the next chain segment or report an error.
fn resolve_run_key(key: &str, run_context: &RunContext) -> Option<String> {
	match key {
		"os" => Some(run_context.os.clone()),
		"shell" => Some(run_context.shell.clone()),
		_ => None,
	}
}

/// Resolve a `$(FLAGS.key)` or `$(FLAGS.key ? true_val : false_val)` expression.
///
/// FLAGS are boolean: they check whether `--key` was passed on the CLI (presence only,
/// value is ignored). The key is always consumed so it does not appear in `$(ARGS)`.
///
/// Forms:
/// - `$(FLAGS.key)` — returns `"true"` if present, `"false"` if absent
/// - `$(FLAGS.key ? true_val : false_val)` — returns the matching branch
/// - `$(FLAGS.key ? true_val)` — returns `true_val` if present, empty string if absent
fn resolve_flag(
	expr: &str,
	named_args: &HashMap<String, String>,
	consumed: &mut HashSet<String>,
	flag_keys: &mut HashSet<String>,
	prompter: Option<&dyn StdinPrompter>,
) -> Result<String, SubstitutionError> {
	let after_prefix = expr.strip_prefix("FLAGS.").unwrap(); // caller checked prefix

	// Split on first '?' to separate key from ternary
	let (key_part, ternary_part) = match after_prefix.split_once('?') {
		Some((k, rest)) => (k.trim(), Some(rest)),
		None => (after_prefix.trim(), None),
	};

	if key_part.is_empty() {
		// $(FLAGS.) with no key — return literally
		return Ok(format!("$({})", expr));
	}

	let mut is_present = named_args.contains_key(key_part);
	if !is_present {
		if let Some(p) = prompter {
			is_present = p.prompt_flag(&format!("--{}", key_part));
		}
	}
	consumed.insert(key_part.to_string());
	flag_keys.insert(key_part.to_string());

	match ternary_part {
		None => {
			// $(FLAGS.key) → "true" or "false"
			Ok(if is_present {
				"true".to_string()
			} else {
				"false".to_string()
			})
		}
		Some(rest) => {
			// $(FLAGS.key ? true_val : false_val)
			// Split on " : " (spaced colon) to avoid conflicts with URLs/paths.
			// Also handle trailing " :" for empty false branches like $(FLAGS.v ? --verbose :)
			let (true_val, false_val) = if let Some(pos) = rest.find(" : ") {
				(rest[..pos].trim(), rest[pos + 3..].trim())
			} else {
				let trimmed_rest = rest.trim();
				if let Some(before) = trimmed_rest.strip_suffix(" :") {
					// Empty false branch: $(FLAGS.key ? true_val :)
					(before.trim(), "")
				} else {
					// No ternary colon: $(FLAGS.key ? true_val)
					(trimmed_rest, "")
				}
			};
			Ok(if is_present {
				true_val.to_string()
			} else {
				false_val.to_string()
			})
		}
	}
}

/// Look up an env var case-insensitively.
fn env_get_case_insensitive<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
	// Try exact match first (fast path)
	if let Some(val) = env.get(key) {
		return Some(val);
	}
	// Case-insensitive fallback
	let key_lower = key.to_lowercase();
	for (k, v) in env {
		if k.to_lowercase() == key_lower {
			return Some(v);
		}
	}
	None
}

/// Scan a list of command templates and return what ARGS patterns they reference.
/// Returns (uses_positional_args, set_of_named_arg_keys).
/// `uses_positional_args` is true if any template contains `$(ARGS)` (bare positional).
///
/// Accepts already-walked template strings — callers building from a
/// `Vec<CommandStep>` should use [`runfile_parser::walk_step_templates`] to
/// extract the templates first.
pub fn scan_args_usage(commands: &[String]) -> (bool, HashSet<String>) {
	let mut uses_positional = false;
	let mut named_keys = HashSet::new();

	for cmd in commands {
		scan_one_template(cmd, &mut uses_positional, &mut named_keys);
	}

	(uses_positional, named_keys)
}

fn scan_one_template(cmd: &str, uses_positional: &mut bool, named_keys: &mut HashSet<String>) {
	let mut chars = cmd.chars().peekable();
	while let Some(ch) = chars.next() {
		if ch == '$' && chars.peek() == Some(&'(') {
			chars.next(); // consume '('
			let mut expr = String::new();
			let mut depth = 1;
			for c in chars.by_ref() {
				if c == '(' {
					depth += 1;
				} else if c == ')' {
					depth -= 1;
					if depth == 0 {
						break;
					}
				}
				expr.push(c);
			}

			let trimmed = expr.trim();
			if trimmed == "ARGS" {
				*uses_positional = true;
			} else if let Some(rest) = trimmed.strip_prefix("ARGS.") {
				let key = rest.split('?').next().unwrap_or("").trim();
				if !key.is_empty() {
					named_keys.insert(key.to_string());
				}
			} else if let Some(rest) = trimmed.strip_prefix("FLAGS.") {
				let key = rest.split('?').next().unwrap_or("").trim();
				if !key.is_empty() {
					named_keys.insert(key.to_string());
				}
			} else if !trimmed.starts_with("ENV.") && !trimmed.starts_with("LOOP.") && !trimmed.starts_with("RUN.") {
				// Unknown head — the substituter recurses into the body, so
				// scan it for nested `$(ARGS.*)` / `$(FLAGS.*)` references too.
				scan_one_template(&expr, uses_positional, named_keys);
			}
		}
	}
}

/// Validate that the user-provided arguments are accepted by the commands.
/// - If no command uses $(ARGS) or $(ARGS.name), passing any arguments is an error.
/// - If the user passes --name=value but no command uses $(ARGS.name), it's an error.
///   (Unless $(ARGS) is used, which consumes all remaining args including unknown named ones.)
pub fn validate_args(args: &RunArgs, all_commands: &[String]) -> Result<(), SubstitutionError> {
	if args.original.is_empty() {
		return Ok(());
	}

	let (uses_positional, named_keys) = scan_args_usage(all_commands);

	// If no command references $(ARGS) at all, any arguments are unexpected
	if !uses_positional && named_keys.is_empty() {
		return Err(SubstitutionError::UnexpectedArgs(args.original.join(" ")));
	}

	// If $(ARGS) (bare) is used, all args are consumed — no unknown-arg check needed
	if uses_positional {
		return Ok(());
	}

	// Only named keys are referenced. Check that every user-provided named arg is known.
	for key in args.named.keys() {
		if !named_keys.contains(key) {
			return Err(SubstitutionError::UnknownNamedArg(key.clone()));
		}
	}

	// Also check for positional (non-named) args that aren't consumed.
	// If there are non-named tokens and no $(ARGS), those are unexpected.
	let non_named = collect_non_named_tokens(args);
	if !non_named.is_empty() {
		return Err(SubstitutionError::UnexpectedArgs(non_named.join(" ")));
	}

	Ok(())
}

/// Collect tokens from original args that are not part of --key or --key=value pairs.
fn collect_non_named_tokens(args: &RunArgs) -> Vec<String> {
	let mut result = Vec::new();
	let mut i = 0;
	while i < args.original.len() {
		let arg = &args.original[i];
		if let Some(stripped) = arg.strip_prefix("--") {
			if stripped.contains('=') {
				// --key=value — single token, skip
				i += 1;
			} else if !stripped.is_empty() {
				// --key possibly followed by value
				if i + 1 < args.original.len()
					&& !args.original[i + 1].starts_with("--")
					&& args.named.get(stripped).is_some_and(|v| !v.is_empty())
				{
					i += 2; // skip --key and its value
				} else {
					i += 1; // --flag with no value
				}
			} else {
				// bare "--"
				result.push(arg.clone());
				i += 1;
			}
		} else {
			// Positional arg — not consumed by any named pattern
			result.push(arg.clone());
			i += 1;
		}
	}
	result
}
