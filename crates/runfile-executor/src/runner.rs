use crate::args::{validate_args, RunArgs, SubstitutionError};
use crate::control_flow::{collect_detach_leaves, DetachFlattenError};
use crate::env::EnvFileError;
use crate::executor::{
	execute_command_with_counter, execute_detached, execute_parallel_with_counter, DependencyResolver, ExecuteError,
	ExecutionResult,
};
use crate::logging::{log_command, log_target_timing, StepCounter};
use runfile_parser::{
	walk_spec_aux_templates, CommandStep, ForInValue, ForStep, IfStep, Runfile, WhenStep, WORKING_DIRECTORY_CWD,
	WORKING_DIRECTORY_RUNFILE_PARENT,
};
use runfile_shell::{resolve_shell, ResolvedShell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RunError {
	#[error("{0}")]
	Execute(#[from] ExecuteError),

	#[error("Dependency cycle detected: {0}")]
	CycleDetected(String),

	#[error("Unknown target \"{0}\"")]
	UnknownTarget(String),

	#[error("{0}")]
	ShellResolve(#[from] runfile_shell::ShellResolveError),

	#[error("{0}")]
	Substitution(#[from] SubstitutionError),

	#[error("{0}")]
	EnvFile(#[from] EnvFileError),

	#[error("{0}")]
	Detach(#[from] DetachFlattenError),

	#[error(
		"Target \"{0}\" has invalid `workingDirectory` value \"{1}\" — must be \"runfileParent\" or \"cwd\" (after substitution)."
	)]
	InvalidWorkingDirectory(String, String),
}

/// Shared root state for an entire run. Holds immutable references plus
/// thread-safe shared mutable state (the step counter is the only mutable
/// field, backed by atomics). Borrowed by `&` from every dispatch path so
/// worker threads spawned by parallel `@target` calls can share the same
/// counter without lock contention.
///
/// Cycle detection lives on the per-call `chain` parameter (a `Vec<String>`)
/// rather than on `RunRoot`, so two parallel calls to the same target are
/// not mistaken for a cycle — only re-entry within a single call stack is.
struct RunRoot<'a> {
	runfile: &'a Runfile,
	shell: &'a ResolvedShell,
	base_args: &'a RunArgs,
	runfile_dir: &'a Path,
	caller_cwd: &'a Path,
	source_dirs: &'a HashMap<String, PathBuf>,
	timings: bool,
	yes: bool,
	available_private_keys: Option<&'a [String]>,
	step_counter: StepCounter,
}

impl RunRoot<'_> {
	/// Get the source directory for a target, falling back to the main runfile_dir.
	fn target_dir(&self, target_name: &str) -> &Path {
		self.source_dirs
			.get(target_name)
			.map(|p| p.as_path())
			.unwrap_or(self.runfile_dir)
	}
}

/// `DependencyResolver` impl that dispatches `@target` calls back into the
/// runner. Holds a borrow of `RunRoot` so all calls share the same counter
/// and runfile state. Implements `Sync` because all fields are either
/// `Send + Sync` or `Sync` via interior mutability (atomics in
/// `StepCounter`).
struct RunnerDependencyResolver<'a> {
	root: &'a RunRoot<'a>,
}

impl DependencyResolver for RunnerDependencyResolver<'_> {
	fn run_dependency(
		&self,
		target_name: &str,
		args: Vec<String>,
		parent_env: &HashMap<String, String>,
		parent_add_to_path_chain: &[Vec<String>],
		optional: bool,
	) -> Result<ExecutionResult, ExecuteError> {
		// `@?target` opts into "skip when target is missing". The check uses the
		// same lookup the runner itself performs (`targets.get`), so optional
		// dispatch matches non-optional dispatch exactly when the target *does*
		// exist. Aliases via `@target` don't currently round-trip through this
		// resolver path — see runtime semantics in `run_target_inner_body`.
		if optional && !self.root.runfile.targets.contains_key(target_name) {
			return Ok(ExecutionResult {
				commands_run: 0,
				failures: 0,
				final_status: dummy_success_status(),
			});
		}

		// Build a child RunArgs from the tokenized argv. We re-parse so
		// `--key value` / `--key=value` / positional split is consistent with
		// the CLI's parser, then re-attach the parent's run_context (the
		// runner refreshes shell per-target downstream anyway).
		let child_args = RunArgs::parse(&args).with_run_context(self.root.base_args.run_context.clone());

		// Dependency invocations don't dedup — every `@target` call runs.
		// Cycle detection uses the per-call chain.
		let mut chain: Vec<String> = Vec::new();
		match run_target_inner(
			self.root,
			target_name,
			&child_args,
			Some(parent_env),
			parent_add_to_path_chain,
			&mut chain,
		) {
			Ok(result) => Ok(result),
			Err(RunError::Execute(e)) => Err(e),
			Err(e) => Err(ExecuteError::DependencyFailed(target_name.to_string(), e.to_string())),
		}
	}
}

/// Execute a target by name, resolving `@target` dependencies along the way.
pub fn run_target(
	target_name: &str,
	runfile: &Runfile,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
) -> Result<ExecutionResult, RunError> {
	run_target_with_cwd(
		target_name,
		runfile,
		shell,
		args,
		working_dir,
		working_dir,
		&HashMap::new(),
		false,
		false,
		None,
	)
}

/// Execute a target by name, with separate runfile dir and caller CWD.
/// `source_dirs` maps target names to their source Runfile's parent directory
/// (used when targets come from different files, e.g. global files).
///
/// The caller's `args.run_context` is used as the baseline for `$(RUN.*)`
/// resolution; the runner rewrites `run_context.shell` internally if a
/// target's `forceShell` substitution resolves to a different shell than
/// the caller's, so users always see the shell that actually runs their
/// commands.
#[allow(clippy::too_many_arguments)]
pub fn run_target_with_cwd(
	target_name: &str,
	runfile: &Runfile,
	shell: &ResolvedShell,
	args: &RunArgs,
	runfile_dir: &Path,
	caller_cwd: &Path,
	source_dirs: &HashMap<String, PathBuf>,
	timings: bool,
	yes: bool,
	available_private_keys: Option<&[String]>,
) -> Result<ExecutionResult, RunError> {
	// Make sure the run_context is in sync with the resolved shell the caller
	// decided on AND carries the merged Runfile's namespace list (for `for
	// "in": "namespaces"` resolution). Both checks are idempotent: nothing
	// gets cloned when the caller already set them.
	let args_owned = ensure_run_context(args, &shell.kind, &runfile.namespaces);
	let args = args_owned.as_ref().unwrap_or(args);

	// Collect all template strings from the target and its dependencies for
	// arg-usage validation. This includes condition strings, args templates,
	// for-iterator sources, etc. — every place a `$(ARGS.x)` reference could
	// hide. It is NOT used to size the step counter (which would over-count).
	let all_commands = collect_all_commands(target_name, runfile)?;
	validate_args(args, &all_commands)?;

	// Separate count for the step counter: only leaf shell commands and
	// `@target` invocations that will actually run. `if` blocks still count
	// both branches (worst case) and `for-glob`/`for-shell` use a 1-iteration
	// estimate — `count_leaves` semantics, but recursively across `@target`
	// references so the global counter sees the whole reachable tree.
	let total_leaves = count_target_leaves(target_name, runfile)?;

	let root = RunRoot {
		runfile,
		shell,
		base_args: args,
		runfile_dir,
		caller_cwd,
		source_dirs,
		timings,
		yes,
		available_private_keys,
		step_counter: StepCounter::new(total_leaves),
	};
	let mut chain: Vec<String> = Vec::new();
	run_target_inner(&root, target_name, args, None, &[], &mut chain)
}

/// Recursive entry for running a target. Used by both the top-level CLI
/// invocation (sequential, no parent env) and by `@target` dependency calls
/// (potentially from worker threads, parent env passed in).
///
/// `chain` is the per-call call stack used for cycle detection. There is no
/// dedup at this layer — every call runs.
/// `parent_add_to_path_chain` is the accumulated `addToPath` chain from
/// ancestors (outermost first); empty for top-level invocations.
fn run_target_inner(
	root: &RunRoot<'_>,
	target_name: &str,
	args: &RunArgs,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	chain: &mut Vec<String>,
) -> Result<ExecutionResult, RunError> {
	if chain.iter().any(|t| t == target_name) {
		return Err(RunError::CycleDetected(target_name.to_string()));
	}
	chain.push(target_name.to_string());
	let result = run_target_inner_body(root, target_name, args, parent_env, parent_add_to_path_chain, chain);
	chain.pop();
	result
}

fn run_target_inner_body(
	root: &RunRoot<'_>,
	target_name: &str,
	args: &RunArgs,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	_chain: &mut Vec<String>,
) -> Result<ExecutionResult, RunError> {
	let spec = root
		.runfile
		.targets
		.get(target_name)
		.ok_or_else(|| RunError::UnknownTarget(target_name.to_string()))?;

	// Build a thin env for substituting `forceShell` and `workingDirectory`.
	// These fields can carry `$(...)` substitutions; we resolve them BEFORE
	// the full env is built (since the working dir is needed to load env
	// files). `parent_env` (if any) and `args` are enough — substitution of
	// `forceShell` / `workingDirectory` against `$(ENV.X)` references the
	// parent's env, not the target's own envFiles.
	let pre_env: HashMap<String, String> = parent_env.cloned().unwrap_or_default();

	// Substitute and resolve `forceShell`. `forceShell` is target-level
	// config and is NOT inherited from the parent across an `@target` call —
	// each target picks its own shell.
	let resolved_force_shell: Option<String> = match spec.force_shell.as_ref() {
		Some(template) => Some(args.substitute(template, &pre_env)?),
		None => None,
	};
	let effective_shell = match resolved_force_shell.as_deref() {
		Some(shell_name) if Some(shell_name) != spec.force_shell.as_deref() => Some(resolve_shell(shell_name)?),
		_ => None,
	};
	let shell = effective_shell.as_ref().unwrap_or(root.shell);

	// Update args.run_context.shell if the target's effective shell differs
	// from the parent's. This makes `$(RUN.shell)` reflect the shell that
	// actually runs the commands, even when a `forceShell` override applies.
	// The namespace list is unchanged at this point — the top-level call
	// already attached it, so we re-pass the same slice for the in-sync check.
	let target_args_owned = ensure_run_context(args, &shell.kind, &root.runfile.namespaces);
	let target_args: &RunArgs = target_args_owned.as_ref().unwrap_or(args);

	// Substitute and validate `workingDirectory`. The substituted value must
	// be exactly `runfileParent` or `cwd`.
	let resolved_working_directory: Option<String> = match spec.working_directory.as_ref() {
		Some(template) => Some(target_args.substitute(template, &pre_env)?),
		None => None,
	};
	let effective_working_dir = {
		let target_runfile_dir = root.target_dir(target_name);
		match resolved_working_directory.as_deref() {
			Some(WORKING_DIRECTORY_CWD) => root.caller_cwd.to_path_buf(),
			Some(WORKING_DIRECTORY_RUNFILE_PARENT) | None => target_runfile_dir.to_path_buf(),
			Some(other) => {
				return Err(RunError::InvalidWorkingDirectory(
					target_name.to_string(),
					other.to_string(),
				));
			}
		}
	};

	// Handle detached targets: spawn commands in background and return immediately
	if spec.detach.unwrap_or(false) {
		let env = crate::env::build_env_with_base(
			spec,
			&effective_working_dir,
			target_args,
			root.available_private_keys,
			parent_env,
			Some(parent_add_to_path_chain),
		)?;

		// Evaluate `if` / `for` / `when` blocks to a flat list of concrete
		// shell commands. `@target` invocations are rejected (they don't
		// have meaningful "fire and forget" semantics — the dep would
		// itself need orchestration). `when: failure` blocks are skipped
		// because there's no runtime failure tracking in detached mode.
		let resolved_commands = collect_detach_leaves(&spec.commands, target_args, &env, &effective_working_dir)?;

		for cmd in &resolved_commands {
			let (step, total) = root.step_counter.next_step();
			log_command(cmd, step, total);
		}

		eprintln!(
			"[runfile] Detaching: spawning {} command(s) in background (parallel)",
			resolved_commands.len()
		);

		for cmd in &resolved_commands {
			execute_detached(cmd, shell, &env, &effective_working_dir)?;
		}

		return Ok(ExecutionResult {
			commands_run: 0,
			failures: 0,
			final_status: dummy_success_status(),
		});
	}

	// Confirmation prompt (skip in CI or with --yes)
	if let Some(prompt) = &spec.confirm {
		if !root.yes && !is_ci_environment() {
			eprint!("\x1b[1m\x1b[36m[runfile]\x1b[0m {} \x1b[2m(y/N)\x1b[0m ", prompt);
			let _ = std::io::Write::flush(&mut std::io::stderr());
			let mut input = String::new();
			if std::io::stdin().read_line(&mut input).is_err() || !input.trim().eq_ignore_ascii_case("y") {
				eprintln!("\x1b[1m\x1b[36m[runfile]\x1b[0m Aborted.");
				return Ok(ExecutionResult {
					commands_run: 0,
					failures: 0,
					final_status: dummy_success_status(),
				});
			}
		}
	}

	let target_start = Instant::now();
	let resolver = RunnerDependencyResolver { root };

	let main_result = if spec.parallel.unwrap_or(false) {
		execute_parallel_with_counter(
			spec,
			shell,
			target_args,
			&effective_working_dir,
			root.available_private_keys,
			root.timings,
			&root.step_counter,
			&resolver,
			parent_env,
			parent_add_to_path_chain,
		)
	} else {
		execute_command_with_counter(
			spec,
			shell,
			target_args,
			&effective_working_dir,
			root.available_private_keys,
			root.timings,
			&root.step_counter,
			&resolver,
			parent_env,
			parent_add_to_path_chain,
		)
	};

	if root.timings {
		log_target_timing(target_name, target_start.elapsed());
	}

	match main_result {
		Ok(result) => Ok(ExecutionResult {
			commands_run: result.commands_run,
			failures: result.failures,
			final_status: result.final_status,
		}),
		Err(e) => Err(e.into()),
	}
}

/// Count the leaf steps that will actually run (or that *might* run, in the
/// case of `if` branches) across the target and its `@target` dependency
/// tree. Used to size the global step counter so the `(N/total)` indicator
/// stays accurate.
///
/// Counting rules:
/// - `Shell` → 1
/// - `TargetCall` → recurse into the called target (memoized per-target).
/// - `When { commands }` → leaves of `commands`.
/// - `If { then, else }` → leaves of `then` + leaves of `else` (worst case
///   — we don't know which branch will run, so we'd rather over-count).
/// - `For { in: [...], body }` → `items.len() * leaves(body)`.
/// - `For { glob | shell, body }` → `leaves(body)` (1-iteration estimate;
///   the runtime calls `StepCounter::add_to_total` to bump if more
///   iterations actually expand).
///
/// Differences from `collect_all_commands`: condition strings and args
/// templates do NOT count as steps (they're scannable for `$(ARGS.x)` but
/// don't contribute to the (N/total) display).
fn count_target_leaves(target_name: &str, runfile: &Runfile) -> Result<usize, RunError> {
	let mut cache: HashMap<String, usize> = HashMap::new();
	let mut in_progress: HashSet<String> = HashSet::new();
	count_target_leaves_recursive(target_name, runfile, &mut cache, &mut in_progress)
}

fn count_target_leaves_recursive(
	target_name: &str,
	runfile: &Runfile,
	cache: &mut HashMap<String, usize>,
	in_progress: &mut HashSet<String>,
) -> Result<usize, RunError> {
	if let Some(&cached) = cache.get(target_name) {
		return Ok(cached);
	}
	if !in_progress.insert(target_name.to_string()) {
		return Err(RunError::CycleDetected(target_name.to_string()));
	}

	let spec = runfile
		.targets
		.get(target_name)
		.ok_or_else(|| RunError::UnknownTarget(target_name.to_string()))?;

	let count = count_step_leaves_recursive(&spec.commands, runfile, cache, in_progress)?;

	in_progress.remove(target_name);
	cache.insert(target_name.to_string(), count);
	Ok(count)
}

fn count_step_leaves_recursive(
	steps: &[CommandStep],
	runfile: &Runfile,
	cache: &mut HashMap<String, usize>,
	in_progress: &mut HashSet<String>,
) -> Result<usize, RunError> {
	let mut total = 0;
	for step in steps {
		total += match step {
			CommandStep::Shell(_) => 1,
			CommandStep::TargetCall(call) => {
				// Dynamic target names (containing `$(...)`, e.g. `@$(LOOP.ns):build`)
				// resolve at runtime; we can't recurse into them statically. Count
				// the call as 1 leaf and let `StepCounter::add_to_total` bump the
				// total at runtime if the dispatched target exposes more leaves.
				// Optional calls (`@?target`) on a static target name that
				// doesn't exist contribute 0 leaves — they'll be silently
				// skipped at runtime.
				if call.target.contains("$(") {
					1
				} else if call.optional && !runfile.targets.contains_key(&call.target) {
					0
				} else {
					count_target_leaves_recursive(&call.target, runfile, cache, in_progress)?
				}
			}
			CommandStep::When(WhenStep { commands, .. }) => {
				count_step_leaves_recursive(commands, runfile, cache, in_progress)?
			}
			CommandStep::If(IfStep { then, r#else, .. }) => {
				let then_count = count_step_leaves_recursive(then, runfile, cache, in_progress)?;
				let else_count = if let Some(e) = r#else {
					count_step_leaves_recursive(e, runfile, cache, in_progress)?
				} else {
					0
				};
				then_count + else_count
			}
			CommandStep::For(ForStep { r#in, body, .. }) => {
				let body_count = count_step_leaves_recursive(body, runfile, cache, in_progress)?;
				match r#in {
					Some(ForInValue::Literal(items)) => items.len() * body_count,
					Some(ForInValue::Namespaces) => runfile.namespaces.len() * body_count,
					None => body_count, // glob/shell — 1-iteration estimate
				}
			}
		};
	}
	Ok(total)
}

/// Collect all command templates from a target and its dependency tree —
/// across `@target` invocations inside `commands` arrays. Used for
/// arg-usage validation (so `$(ARGS.x)` in a transitively called target's
/// condition / args template / shell command counts as a referenced arg).
/// NOT used for sizing the step counter — see `count_target_leaves` for that.
fn collect_all_commands(target_name: &str, runfile: &Runfile) -> Result<Vec<String>, RunError> {
	let mut commands = Vec::new();
	let mut completed = HashSet::new();
	let mut in_progress = HashSet::new();
	collect_commands_recursive(target_name, runfile, &mut commands, &mut completed, &mut in_progress)?;
	Ok(commands)
}

fn collect_commands_recursive(
	target_name: &str,
	runfile: &Runfile,
	commands: &mut Vec<String>,
	completed: &mut HashSet<String>,
	in_progress: &mut HashSet<String>,
) -> Result<(), RunError> {
	if completed.contains(target_name) {
		return Ok(());
	}
	if !in_progress.insert(target_name.to_string()) {
		return Err(RunError::CycleDetected(target_name.to_string()));
	}

	let spec = runfile
		.targets
		.get(target_name)
		.ok_or_else(|| RunError::UnknownTarget(target_name.to_string()))?;

	collect_step_commands(&spec.commands, runfile, commands, completed, in_progress)?;
	walk_spec_aux_templates(spec, &mut |t| commands.push(t.to_string()));

	completed.insert(target_name.to_string());
	in_progress.remove(target_name);

	Ok(())
}

/// Walk a `commands` array, pushing leaf templates to `commands` and
/// recursing into `@target` invocations (with cycle detection). For
/// arg-usage scanning we also include the `@target` arg-template itself
/// (so `@build $(ARGS.x)` registers `x` as a referenced arg).
fn collect_step_commands(
	steps: &[CommandStep],
	runfile: &Runfile,
	commands: &mut Vec<String>,
	completed: &mut HashSet<String>,
	in_progress: &mut HashSet<String>,
) -> Result<(), RunError> {
	for step in steps {
		match step {
			CommandStep::Shell(s) => commands.push(s.clone()),
			CommandStep::TargetCall(call) => {
				// `call.target` itself participates in arg-usage scanning so
				// `$(ARGS.x)` references inside dynamic target names like
				// `@$(LOOP.ns):build` still register.
				commands.push(call.target.clone());
				if !call.args_template.is_empty() {
					commands.push(call.args_template.clone());
				}
				// Dynamic target names resolve at runtime; we can't recurse
				// into them statically. Their args templates were already
				// captured above. Optional calls (`@?target`) on a static
				// target name that doesn't exist also skip recursion — at
				// runtime they're silently no-ops.
				let is_dynamic = call.target.contains("$(");
				let optional_missing = call.optional && !runfile.targets.contains_key(&call.target);
				if !is_dynamic && !optional_missing {
					// Recurse into the called target's commands (cycle-safe).
					// `completed` makes diamond dependencies count once for
					// sizing — actual runtime invocations still don't dedup.
					collect_commands_recursive(&call.target, runfile, commands, completed, in_progress)?;
				}
			}
			CommandStep::When(WhenStep { commands: inner, .. }) => {
				collect_step_commands(inner, runfile, commands, completed, in_progress)?;
			}
			CommandStep::If(IfStep {
				condition,
				then,
				r#else,
				..
			}) => {
				commands.push(condition.clone());
				collect_step_commands(then, runfile, commands, completed, in_progress)?;
				if let Some(else_steps) = r#else {
					collect_step_commands(else_steps, runfile, commands, completed, in_progress)?;
				}
			}
			CommandStep::For(ForStep {
				r#in,
				glob,
				shell: shell_iter,
				body,
				..
			}) => {
				// Only the literal-array form contributes templates; the magic
				// `"namespaces"` keyword isn't substitutable.
				if let Some(ForInValue::Literal(items)) = r#in {
					for item in items {
						commands.push(item.clone());
					}
				}
				if let Some(g) = glob {
					commands.push(g.clone());
				}
				if let Some(s) = shell_iter {
					commands.push(s.clone());
				}
				collect_step_commands(body, runfile, commands, completed, in_progress)?;
			}
		}
	}
	Ok(())
}

fn is_ci_environment() -> bool {
	std::env::var("CI").is_ok_and(|v| v == "true" || v == "1")
}

/// Return a cloned [`RunArgs`] with `run_context.shell` set to match the
/// active shell AND `run_context.namespaces` populated from the merged
/// Runfile; or `None` if `args.run_context` is already in sync (so the
/// caller can keep the existing borrow). Keeps `$(RUN.shell)` / `$(RUN.os)`
/// substitutions correct even when callers pass args from outside (e.g.
/// tests that use `RunArgs::default()`), and ensures `for "in":
/// "namespaces"` always sees the post-merge namespace list.
///
/// The namespace list is compared by pointer (`Arc::ptr_eq`) so cheap
/// pointer equality keeps the no-op fast-path active whenever the caller
/// already attached the same `Arc`.
fn ensure_run_context(args: &RunArgs, shell_kind: &runfile_shell::ShellKind, namespaces: &[String]) -> Option<RunArgs> {
	let shell_name = shell_kind.name();
	let os = crate::args::detect_current_os();
	let shell_in_sync = args.run_context.shell == shell_name && args.run_context.os == os;
	let namespaces_in_sync = args.run_context.namespaces.as_slice() == namespaces;
	if shell_in_sync && namespaces_in_sync {
		None
	} else {
		let mut owned = args.clone();
		owned.run_context.os = os.to_string();
		owned.run_context.shell = shell_name.to_string();
		if !namespaces_in_sync {
			owned.run_context.namespaces = Arc::new(namespaces.to_vec());
		}
		Some(owned)
	}
}

fn dummy_success_status() -> std::process::ExitStatus {
	#[cfg(unix)]
	{
		use std::os::unix::process::ExitStatusExt;
		std::process::ExitStatus::from_raw(0)
	}
	#[cfg(windows)]
	{
		use std::os::windows::process::ExitStatusExt;
		std::process::ExitStatus::from_raw(0)
	}
}
