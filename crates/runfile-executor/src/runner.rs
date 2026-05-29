use crate::args::{validate_args, RunArgs, SubstitutionError};
use crate::control_flow::{collect_detach_leaves, DetachFlattenError};
use crate::env::{EnvFileError, PrivateKeyProvider};
use crate::executor::{
	execute_command_with_counter, execute_detached, execute_parallel_with_counter, execute_same_shell_with_counter,
	join_shell_commands, DependencyResolver, ExecuteError, ExecutionResult,
};
use crate::logging::{log_command, log_target_timing, StepCounter};
use runfile_parser::{
	walk_spec_aux_templates, CommandStep, ForInValue, ForStep, IfStep, MatchStep, Runfile, WhenStep,
	WORKING_DIRECTORY_DEFAULT,
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
	runfile_path: &'a Path,
	runfile_dir: &'a Path,
	caller_cwd: &'a Path,
	source_dirs: &'a HashMap<String, PathBuf>,
	source_files: &'a HashMap<String, PathBuf>,
	timings: bool,
	yes: bool,
	available_private_keys: Option<&'a dyn PrivateKeyProvider>,
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

	/// Get the source file path for a target, falling back to the main
	/// runfile_path. Used to populate `{{ RUN.file }}` per-target so a
	/// dispatched `@target` defined in an included file resolves the path
	/// of that include, not the entrypoint.
	fn target_file(&self, target_name: &str) -> &Path {
		self.source_files
			.get(target_name)
			.map(|p| p.as_path())
			.unwrap_or(self.runfile_path)
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
		output_prefix: Option<&str>,
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
		// runner refreshes shell per-target downstream anyway). The
		// `--stdin-args` prompter (if any) is also propagated by `Arc` clone
		// so prompted answers cached on the parent are reused inside the dep
		// instead of re-asking the user.
		let child_args = RunArgs::parse(&args)
			.with_run_context(self.root.base_args.run_context.clone())
			.with_stdin_prompter(self.root.base_args.stdin_prompter.clone())
			.with_vars(self.root.base_args.vars.clone())
			.with_capture_cache(self.root.base_args.capture_cache.clone());

		// Dependency invocations don't dedup — every `@target` call runs.
		// Cycle detection uses the per-call chain.
		let mut chain: Vec<String> = Vec::new();
		match run_target_inner(
			self.root,
			target_name,
			&child_args,
			Some(parent_env),
			parent_add_to_path_chain,
			output_prefix,
			&mut chain,
		) {
			Ok(result) => {
				// Target-level `ignoreErrors: true` self-contains its failures
				// — symmetric with how block-level `ignoreErrors` works in
				// `for`/`if`/`when`/`match` (see executor.rs handling that
				// returns Ok with no fold-in). Without this, a dep's internal
				// failure count would still surface to the caller's
				// `state.failures`, flip the caller's `failed` flag, and skip
				// every subsequent default-`when: success` sibling. The dep's
				// `ignoreErrors` is meant to opt the dep out of the failure
				// chain entirely, so we present a clean result here.
				let dep_ignores_errors = self
					.root
					.runfile
					.targets
					.get(target_name)
					.and_then(|spec| spec.ignore_errors)
					.unwrap_or(false);
				if dep_ignores_errors {
					Ok(ExecutionResult {
						commands_run: result.commands_run,
						failures: 0,
						final_status: dummy_success_status(),
					})
				} else {
					Ok(result)
				}
			}
			Err(RunError::Execute(e)) => Err(e),
			Err(e) => Err(ExecuteError::DependencyFailed(target_name.to_string(), e.to_string())),
		}
	}
}

/// Execute a target by name, resolving `@target` dependencies along the way.
///
/// `working_dir` is taken to be the parent directory of the entrypoint
/// Runfile and is also used as the caller's CWD; the synthetic entrypoint
/// path is `working_dir/Runfile.json` (used to populate `{{ RUN.file }}`).
/// For full control over `caller_cwd`, source dirs/files, etc., use
/// [`run_target_with_cwd`].
pub fn run_target(
	target_name: &str,
	runfile: &Runfile,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
) -> Result<ExecutionResult, RunError> {
	let synthetic_path = working_dir.join(runfile_parser::RUNFILE_NAME);
	run_target_with_cwd(
		target_name,
		runfile,
		shell,
		args,
		&synthetic_path,
		working_dir,
		working_dir,
		&HashMap::new(),
		&HashMap::new(),
		false,
		false,
		None,
	)
}

/// Execute a target by name, with separate runfile dir and caller CWD.
/// `source_dirs` maps target names to their source Runfile's parent directory,
/// `source_files` maps target names to the source Runfile *path* (used to
/// populate `{{ RUN.file }}` / `{{ RUN.parent }}` per-target). Both matter
/// when targets come from different files (e.g. global files, `includes`).
///
/// The caller's `args.run_context` is used as the baseline for `{{ RUN.* }}`
/// resolution; the runner rewrites `run_context.shell` / `file` / `parent`
/// internally per-target so users always see values that match the
/// currently-executing target.
#[allow(clippy::too_many_arguments)]
pub fn run_target_with_cwd(
	target_name: &str,
	runfile: &Runfile,
	shell: &ResolvedShell,
	args: &RunArgs,
	runfile_path: &Path,
	runfile_dir: &Path,
	caller_cwd: &Path,
	source_dirs: &HashMap<String, PathBuf>,
	source_files: &HashMap<String, PathBuf>,
	timings: bool,
	yes: bool,
	available_private_keys: Option<&dyn PrivateKeyProvider>,
) -> Result<ExecutionResult, RunError> {
	// Make sure the run_context is in sync with the resolved shell the caller
	// decided on AND carries the merged Runfile's namespace list (for `for
	// "in": "namespaces"` resolution), plus baseline `cwd`/`file`/`parent`
	// values for `{{ RUN.* }}` substitution. The runner refreshes
	// `file`/`parent` per-target downstream — these top-level values are the
	// fallback for targets without a `source_files` / `source_dirs` entry.
	let args_owned = ensure_run_context(
		args,
		&shell.kind,
		&runfile.namespaces,
		caller_cwd,
		runfile_path,
		runfile_dir,
	);
	let args = args_owned.as_ref().unwrap_or(args);

	// Collect all template strings from the target and its dependencies for
	// arg-usage validation. This includes condition strings, args templates,
	// for-iterator sources, etc. — every place a `{{ ARG.x }}` reference could
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
		runfile_path,
		runfile_dir,
		caller_cwd,
		source_dirs,
		source_files,
		timings,
		yes,
		available_private_keys,
		step_counter: StepCounter::new(total_leaves),
	};
	let mut chain: Vec<String> = Vec::new();
	run_target_inner(&root, target_name, args, None, &[], None, &mut chain)
}

/// Recursive entry for running a target. Used by both the top-level CLI
/// invocation (sequential, no parent env) and by `@target` dependency calls
/// (potentially from worker threads, parent env passed in).
///
/// `chain` is the per-call call stack used for cycle detection. There is no
/// dedup at this layer — every call runs.
/// `parent_add_to_path_chain` is the accumulated `addToPath` chain from
/// ancestors (outermost first); empty for top-level invocations.
#[allow(clippy::too_many_arguments)]
fn run_target_inner(
	root: &RunRoot<'_>,
	target_name: &str,
	args: &RunArgs,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	output_prefix: Option<&str>,
	chain: &mut Vec<String>,
) -> Result<ExecutionResult, RunError> {
	if chain.iter().any(|t| t == target_name) {
		return Err(RunError::CycleDetected(target_name.to_string()));
	}
	chain.push(target_name.to_string());
	let result = run_target_inner_body(
		root,
		target_name,
		args,
		parent_env,
		parent_add_to_path_chain,
		output_prefix,
		chain,
	);
	chain.pop();
	result
}

#[allow(clippy::too_many_arguments)]
fn run_target_inner_body(
	root: &RunRoot<'_>,
	target_name: &str,
	args: &RunArgs,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	output_prefix: Option<&str>,
	_chain: &mut Vec<String>,
) -> Result<ExecutionResult, RunError> {
	let spec = root
		.runfile
		.targets
		.get(target_name)
		.ok_or_else(|| RunError::UnknownTarget(target_name.to_string()))?;

	// Thin env for substituting `forceShell` (and the cheap path for
	// `workingDirectory` when it references neither `ENV.` nor `VAR.`).
	// `forceShell` is resolved before the shell — and therefore before the
	// target's own env — is known, so it can only see the parent env (the
	// parent's already-resolved env for an `@dep` call; empty at top level).
	// `workingDirectory` builds the target's FULL env on demand below so it can
	// reference globals'/target `env` and declared `vars`.
	let pre_env: HashMap<String, String> = parent_env.cloned().unwrap_or_default();

	// Per-target view of `RUN.file` / `RUN.parent`: the source Runfile of
	// *this* target (relevant when the target came from an included or global
	// file). `RUN.cwd` is the run-wide caller cwd already set by the
	// top-level `ensure_run_context`.
	let target_runfile_dir = root.target_dir(target_name);
	let target_runfile_file = root.target_file(target_name);

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

	// Update args.run_context.shell / file / parent so `{{ RUN.shell }}`,
	// `{{ RUN.file }}`, and `{{ RUN.parent }}` reflect the currently-executing
	// target. The namespace list is unchanged at this point — the top-level
	// call already attached it, so we re-pass the same slice for the in-sync
	// check.
	let target_args_owned = ensure_run_context(
		args,
		&shell.kind,
		&root.runfile.namespaces,
		root.caller_cwd,
		target_runfile_file,
		target_runfile_dir,
	);
	let target_args: &RunArgs = target_args_owned.as_ref().unwrap_or(args);

	// `workingDirectory` is a free-form path that supports `{{ ... }}`
	// substitution. Default (when unset) is `{{ RUN.parent }}` — the target's
	// source Runfile directory. After substitution, relative paths are
	// resolved against that same directory.
	//
	// `{{ ENV.* }}` / `{{ VAR.* }}` inside it must resolve against the target's
	// OWN resolved env (globals' `env` is baked into every target at parse time)
	// and declared `vars`, not merely the parent env. Env VALUES don't depend on
	// the working directory — only relative `addToPath` PATH assembly does, and
	// those entries are baked to absolute at parse time — so we can build the
	// full env up front (using the source Runfile dir as the addToPath/envFiles
	// base) purely to resolve the path; the executor builds the env again for the
	// real run. The build is gated on the template actually referencing
	// `ENV.`/`VAR.` so the common cases (a literal path, the `{{ RUN.parent }}`
	// default, `{{ ARG.* }}` / `{{ RUN.* }}`) stay on the cheap parent-env path.
	let working_directory_template = spec.working_directory.as_deref().unwrap_or(WORKING_DIRECTORY_DEFAULT);
	let resolved_working_directory =
		if working_directory_template.contains("ENV.") || working_directory_template.contains("VAR.") {
			let wd_env = crate::env::build_env_with_base(
				spec,
				target_runfile_dir,
				target_runfile_dir,
				target_args,
				root.available_private_keys,
				parent_env,
				Some(parent_add_to_path_chain),
			)?;
			// Apply declared `vars` so `{{ VAR.* }}` resolves here too. The guard
			// restores prior values when this block ends; the executor re-applies
			// them via its own `DeclaredVarsGuard` for the command walk.
			let _wd_vars_guard = crate::executor::DeclaredVarsGuard::apply(spec, target_args, &wd_env)?;
			target_args.substitute(working_directory_template, &wd_env)?
		} else {
			target_args.substitute(working_directory_template, &pre_env)?
		};
	let effective_working_dir = resolve_working_directory_path(&resolved_working_directory, target_runfile_dir);

	let same_shell = spec.same_shell.unwrap_or(false);

	// Handle detached targets: spawn commands in background and return immediately
	if spec.detach.unwrap_or(false) {
		let env = crate::env::build_env_with_base(
			spec,
			&effective_working_dir,
			target_runfile_dir,
			target_args,
			root.available_private_keys,
			parent_env,
			Some(parent_add_to_path_chain),
		)?;

		// Apply the target's declared `vars` so `{{ VAR.* }}` resolves while we
		// substitute the detach leaves below. The guard restores prior values
		// when it drops at the end of this branch.
		let _vars_guard = crate::executor::DeclaredVarsGuard::apply(spec, target_args, &env)?;

		// Evaluate `if` / `for` / `when` blocks to a flat list of concrete
		// shell commands. `@target` invocations are rejected (they don't
		// have meaningful "fire and forget" semantics — the dep would
		// itself need orchestration). `when: failure` blocks are skipped
		// because there's no runtime failure tracking in detached mode.
		let resolved_commands = collect_detach_leaves(&spec.commands, target_args, &env, &effective_working_dir)?;

		// Apply any `set_cwd(...)` that ran while collecting the detach leaves
		// — `target_args.cwd_override` carries the final state and is what
		// every spawned background process should land in. Bare working_dir
		// is the no-op fallback when `set_cwd` was never called.
		let detach_cwd = target_args.spawn_cwd(&effective_working_dir);

		if same_shell {
			// `detach + sameShell`: join every leaf into a single shell
			// command and spawn it as ONE detached process. State changes
			// (`cd`, exported vars, etc.) persist across leaves because they
			// share the same shell context.
			let ignore_errors = spec.ignore_errors.unwrap_or(false);
			let joined = join_shell_commands(&resolved_commands, &shell.kind, ignore_errors);

			let (step, total) = root.step_counter.next_step();
			for cmd in &resolved_commands {
				log_command(cmd, step, total);
			}
			// Counter accounting matches `execute_same_shell_with_counter`:
			// every leaf was counted by `count_target_leaves_recursive` only
			// when sameShell was false; when true the recursion returned 1.
			// Either way `subtract_from_total` saturates correctly.
			if resolved_commands.len() > 1 {
				root.step_counter.subtract_from_total(resolved_commands.len() - 1);
			}

			eprintln!(
				"[runfile] Detaching: spawning {} step(s) as 1 background process (sameShell)",
				resolved_commands.len()
			);

			execute_detached(&joined, shell, &env, &detach_cwd)?;
		} else {
			for cmd in &resolved_commands {
				let (step, total) = root.step_counter.next_step();
				log_command(cmd, step, total);
			}

			eprintln!(
				"[runfile] Detaching: spawning {} command(s) in background (parallel)",
				resolved_commands.len()
			);

			for cmd in &resolved_commands {
				execute_detached(cmd, shell, &env, &detach_cwd)?;
			}
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

	let main_result = if same_shell {
		if spec.parallel.unwrap_or(false) {
			eprintln!(
				"[runfile] Warning: target \"{target_name}\" sets both `sameShell: true` and `parallel: true`. \
				 sameShell joins all steps into a single shell invocation, which collapses parallel into one \
				 process — running as sameShell."
			);
		}
		execute_same_shell_with_counter(
			spec,
			shell,
			target_args,
			&effective_working_dir,
			target_runfile_dir,
			root.available_private_keys,
			root.timings,
			&root.step_counter,
			parent_env,
			parent_add_to_path_chain,
			output_prefix,
		)
	} else if spec.parallel.unwrap_or(false) {
		execute_parallel_with_counter(
			spec,
			shell,
			target_args,
			&effective_working_dir,
			target_runfile_dir,
			root.available_private_keys,
			root.timings,
			&root.step_counter,
			&resolver,
			parent_env,
			parent_add_to_path_chain,
			output_prefix,
		)
	} else {
		execute_command_with_counter(
			spec,
			shell,
			target_args,
			&effective_working_dir,
			target_runfile_dir,
			root.available_private_keys,
			root.timings,
			&root.step_counter,
			&resolver,
			parent_env,
			parent_add_to_path_chain,
			output_prefix,
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
/// templates do NOT count as steps (they're scannable for `{{ ARG.x }}` but
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

	// `sameShell: true` collapses every leaf into a single shell invocation,
	// so the target counts as one step toward the global counter regardless
	// of how many leaves its `commands` tree contains. Without this, the
	// `(N/total)` ratio would over-estimate the total when sameShell targets
	// are reachable.
	let count = if spec.same_shell.unwrap_or(false) {
		1
	} else {
		count_step_leaves_recursive(&spec.commands, runfile, cache, in_progress)?
	};

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
				// Dynamic target names (containing `{{ ... }}`, e.g. `@{{ VAR.ns }}:build`)
				// resolve at runtime; we can't recurse into them statically. Count
				// the call as 1 leaf and let `StepCounter::add_to_total` bump the
				// total at runtime if the dispatched target exposes more leaves.
				// Optional calls (`@?target`) on a static target name that
				// doesn't exist contribute 0 leaves — they'll be silently
				// skipped at runtime.
				if call.target.contains("{{") {
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
			CommandStep::Match(MatchStep { cases, default, .. }) => {
				let mut total = 0;
				for branch in cases.values() {
					total += count_step_leaves_recursive(branch, runfile, cache, in_progress)?;
				}
				if let Some(default_steps) = default {
					total += count_step_leaves_recursive(default_steps, runfile, cache, in_progress)?;
				}
				total
			}
		};
	}
	Ok(total)
}

/// Collect all command templates from a target and its dependency tree —
/// across `@target` invocations inside `commands` arrays. Used for
/// arg-usage validation (so `{{ ARG.x }}` in a transitively called target's
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
/// (so `@build {{ ARG.x }}` registers `x` as a referenced arg).
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
				// `{{ ARG.x }}` references inside dynamic target names like
				// `@{{ VAR.ns }}:build` still register.
				commands.push(call.target.clone());
				if !call.args_template.is_empty() {
					commands.push(call.args_template.clone());
				}
				// Dynamic target names resolve at runtime; we can't recurse
				// into them statically. Their args templates were already
				// captured above. Optional calls (`@?target`) on a static
				// target name that doesn't exist also skip recursion — at
				// runtime they're silently no-ops.
				let is_dynamic = call.target.contains("{{");
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
			CommandStep::Match(MatchStep {
				r#match,
				cases,
				default,
				..
			}) => {
				// The `match` template participates in arg-usage scanning so
				// `{{ ARG.x }}` references inside it register. The case keys are
				// literal strings (no substitution), so we don't push them.
				commands.push(r#match.clone());
				for branch in cases.values() {
					collect_step_commands(branch, runfile, commands, completed, in_progress)?;
				}
				if let Some(default_steps) = default {
					collect_step_commands(default_steps, runfile, commands, completed, in_progress)?;
				}
			}
		}
	}
	Ok(())
}

fn is_ci_environment() -> bool {
	std::env::var("CI").is_ok_and(|v| v == "true" || v == "1")
}

/// Return a cloned [`RunArgs`] with the run-context fields synced to the
/// active shell, the merged Runfile's namespace list, the caller cwd, and
/// the currently-executing target's source Runfile path / parent dir; or
/// `None` if everything is already in sync (so the caller can keep the
/// existing borrow). Keeps `{{ RUN.* }}` substitutions correct even when
/// callers pass args from outside (e.g. tests that use `RunArgs::default()`).
///
/// The namespace list is compared as a slice — when the caller already
/// attached the same content, no allocation happens.
fn ensure_run_context(
	args: &RunArgs,
	shell_kind: &runfile_shell::ShellKind,
	namespaces: &[String],
	caller_cwd: &Path,
	target_file: &Path,
	target_dir: &Path,
) -> Option<RunArgs> {
	let shell_name = shell_kind.name();
	let os = crate::args::detect_current_os();
	let cwd_str = caller_cwd.to_string_lossy();
	let file_str = target_file.to_string_lossy();
	let parent_str = target_dir.to_string_lossy();

	let shell_in_sync = args.run_context.shell == shell_name && args.run_context.os == os;
	let namespaces_in_sync = args.run_context.namespaces.as_slice() == namespaces;
	let cwd_in_sync = args.run_context.cwd == cwd_str;
	let file_in_sync = args.run_context.file == file_str;
	let parent_in_sync = args.run_context.parent == parent_str;

	if shell_in_sync && namespaces_in_sync && cwd_in_sync && file_in_sync && parent_in_sync {
		None
	} else {
		let mut owned = args.clone();
		owned.run_context.os = os.to_string();
		owned.run_context.shell = shell_name.to_string();
		if !cwd_in_sync {
			owned.run_context.cwd = cwd_str.into_owned();
		}
		if !file_in_sync {
			owned.run_context.file = file_str.into_owned();
		}
		if !parent_in_sync {
			owned.run_context.parent = parent_str.into_owned();
		}
		if !namespaces_in_sync {
			owned.run_context.namespaces = Arc::new(namespaces.to_vec());
		}
		Some(owned)
	}
}

/// Resolve a substituted `workingDirectory` value to an absolute path.
/// Absolute paths pass through; relative paths are joined onto the
/// target's source Runfile directory (`base_dir`), so e.g.
/// `"workingDirectory": "subdir"` works regardless of the caller's CWD.
fn resolve_working_directory_path(value: &str, base_dir: &Path) -> PathBuf {
	let p = Path::new(value);
	if p.is_absolute() {
		p.to_path_buf()
	} else {
		base_dir.join(p)
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
