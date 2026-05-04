use crate::args::{check_env_case_duplicates, LoopScope, RunArgs, SubstitutionError};
use crate::control_flow::{count_leaves, evaluate_if_condition, expand_for_iterations, ControlFlowError};
use crate::env::{build_env_with_base, EnvFileError};
use crate::force_kill::ForceKillGuard;
use crate::logging::{is_logging_enabled, log_command, log_command_timing, log_parallel_command, StepCounter};
use crate::parallel_output::{
	flush_writer_thread, format_parallel_prefix, line_prefixing_enabled, spawn_line_pump, OutputStream,
};
use crate::stdio_tailer::StdioTailerSet;
use runfile_parser::{CommandSpec, CommandStep, ExtendStdio, ForStep, IfStep, TargetCallStep, WhenCondition, WhenStep};
use runfile_shell::ResolvedShell;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecuteError {
	#[error("Failed to spawn command: {0}")]
	Spawn(#[from] std::io::Error),

	#[error("Command \"{0}\" exited with status {1}")]
	NonZeroExit(String, i32),

	#[error("Command \"{0}\" was terminated by signal")]
	Signal(String),

	#[error("{0}")]
	Substitution(#[from] SubstitutionError),

	#[error("{0}")]
	EnvFile(#[from] EnvFileError),

	#[error("{0}")]
	ControlFlow(#[from] ControlFlowError),

	#[error("Failed to invoke target dependency `@{0}`: {1}")]
	DependencyFailed(String, String),
}

/// Resolves `@target` invocations encountered during execution. Implementations
/// dispatch into the runner machinery (target lookup, env build, recursive
/// run). The executor doesn't know about the Runfile or the dependency tree —
/// it just calls back into this trait.
///
/// Must be `Sync` because parallel `commands` arrays may run target calls on
/// worker threads. `&self`-based methods + interior mutability inside the
/// implementation (e.g. `Mutex`-wrapped completed/in-progress sets) is the
/// expected pattern.
pub trait DependencyResolver: Sync {
	/// Run the named target with the given pre-tokenized argv.
	/// - `parent_env`: the parent's already-resolved env (used as the substitution
	///   base in the dep's `build_env`).
	/// - `parent_add_to_path_chain`: ancestor `addToPath` layers in chain order
	///   (outermost first), with the parent's own `addToPath` already appended.
	///   The dep extends this chain with its own `addToPath` and re-prepends
	///   the whole stack to PATH after the shell-env overlay.
	/// - `optional`: when true, a missing target is silently skipped (returns
	///   `Ok(empty result)`) rather than producing an `UnknownTarget` error.
	///   Set by `@?target` invocations.
	/// - `output_prefix`: when `Some`, every shell command spawned during the
	///   dep's execution (including transitively through nested `@target`
	///   invocations) is piped + prefixed with this string. Set by parallel
	///   parents so each branch of the parallel fan-out gets a distinct tag
	///   (e.g. `[3] `) and progress-bar redraws / interleaved output stay
	///   readable. `None` for top-level invocations.
	fn run_dependency(
		&self,
		target_name: &str,
		args: Vec<String>,
		parent_env: &HashMap<String, String>,
		parent_add_to_path_chain: &[Vec<String>],
		optional: bool,
		output_prefix: Option<&str>,
	) -> Result<ExecutionResult, ExecuteError>;
}

/// `DependencyResolver` that errors on every call. Used by `execute_command`
/// (the test-friendly entry point) and any caller that doesn't have a real
/// runner to dispatch into. `@target` references will surface as
/// `ExecuteError::DependencyFailed` if encountered.
pub struct NoOpDependencyResolver;

impl DependencyResolver for NoOpDependencyResolver {
	fn run_dependency(
		&self,
		target_name: &str,
		_args: Vec<String>,
		_parent_env: &HashMap<String, String>,
		_parent_add_to_path_chain: &[Vec<String>],
		_optional: bool,
		_output_prefix: Option<&str>,
	) -> Result<ExecutionResult, ExecuteError> {
		Err(ExecuteError::DependencyFailed(
			target_name.to_string(),
			"no dependency resolver wired (this code path is for tests / standalone use)".to_string(),
		))
	}
}

/// Result of executing a command sequence.
#[derive(Debug)]
pub struct ExecutionResult {
	/// The number of commands that were executed.
	pub commands_run: usize,
	/// The number of commands that failed (non-zero exit).
	pub failures: usize,
	/// The exit status of the last command.
	pub final_status: ExitStatus,
}

/// Common setup state for command execution, shared between sequential and parallel modes.
struct ExecSetup {
	env: HashMap<String, String>,
	/// addToPath chain to pass to any `@target` dependency invoked from this
	/// target's commands: parent's chain + this target's own `add_to_path`.
	/// Empty when neither layer contributed any entries.
	add_to_path_chain: Vec<Vec<String>>,
	logging: bool,
	ignore_errors: bool,
	force_kill: bool,
	/// Output prefix inherited from a parallel ancestor. When set, every
	/// shell command in this target spawns with `Stdio::piped()` and routes
	/// its output through the line-prefix muxer using this prefix. Forwarded
	/// verbatim to nested `@target` calls so the partition identity propagates
	/// down the entire dependency tree. `None` at top-level / when no parallel
	/// ancestor has set one.
	output_prefix: Option<String>,
}

impl ExecSetup {
	fn new(
		spec: &CommandSpec,
		args: &RunArgs,
		working_dir: &Path,
		available_private_keys: Option<&[String]>,
		parent_env: Option<&HashMap<String, String>>,
		parent_add_to_path_chain: &[Vec<String>],
		output_prefix: Option<&str>,
	) -> Result<(Self, Option<StdioTailerSet>, Option<ForceKillGuard>), ExecuteError> {
		let env = build_env_with_base(
			spec,
			working_dir,
			args,
			available_private_keys,
			parent_env,
			Some(parent_add_to_path_chain),
		)?;
		check_env_case_duplicates(&env)?;

		// Build the chain we'll hand off to any @dep called from this target:
		// parent's chain with this target's own addToPath appended.
		let mut add_to_path_chain: Vec<Vec<String>> = parent_add_to_path_chain.to_vec();
		if let Some(this_layer) = spec.add_to_path.as_deref() {
			if !this_layer.is_empty() {
				add_to_path_chain.push(this_layer.to_vec());
			}
		}

		let setup = Self {
			logging: is_logging_enabled(spec),
			ignore_errors: spec.ignore_errors.unwrap_or(false),
			force_kill: spec.force_kill_on_sig_int.unwrap_or(false),
			env,
			add_to_path_chain,
			output_prefix: output_prefix.map(String::from),
		};

		let tailer = if let Some(entries) = spec.extend_stdio.as_ref().filter(|e| !e.is_empty()) {
			let substituted = substitute_extend_stdio(entries, args, &setup.env)?;
			Some(StdioTailerSet::start(&substituted, working_dir))
		} else {
			None
		};

		let force_kill_guard = if setup.force_kill {
			Some(ForceKillGuard::new())
		} else {
			None
		};

		Ok((setup, tailer, force_kill_guard))
	}
}

/// Substitute $(ARGS) and $(ENV) references in extendStdio fromFile paths.
fn substitute_extend_stdio(
	entries: &[ExtendStdio],
	args: &RunArgs,
	env: &HashMap<String, String>,
) -> Result<Vec<ExtendStdio>, SubstitutionError> {
	entries
		.iter()
		.map(|entry| {
			Ok(ExtendStdio {
				from_file: args.substitute(&entry.from_file, env)?,
				stream: entry.stream.clone(),
			})
		})
		.collect()
}

/// Execute a command specification using the given shell.
///
/// Runs each step in `spec.commands` sequentially, recursively expanding
/// `if` / `for` blocks. Behavior on failure depends on `ignoreErrors`:
/// when false (default), stops on first non-zero exit; when true,
/// continues and reports all failures at the end.
///
/// This is the simple entry point used by tests — it creates a fresh local
/// step counter sized to the spec's leaf-shell count and uses a
/// [`NoOpDependencyResolver`] (so any `@target` references error). The runner
/// uses `execute_command_with_counter` to share one counter and a real
/// resolver across nested calls.
pub fn execute_command(
	spec: &CommandSpec,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	available_private_keys: Option<&[String]>,
	timings: bool,
) -> Result<ExecutionResult, ExecuteError> {
	let counter = StepCounter::new(count_leaves(&spec.commands));
	execute_command_with_counter(
		spec,
		shell,
		args,
		working_dir,
		available_private_keys,
		timings,
		&counter,
		&NoOpDependencyResolver,
		None,
		&[],
		None,
	)
}

/// Like `execute_command`, but uses an externally provided step counter
/// so the `(N/total)` indicator stays continuous across multiple calls
/// (e.g. nested `@target` dependencies), an externally provided
/// [`DependencyResolver`] so `@target` invocations dispatch into the
/// runner, and an optional `parent_env` (the parent target's already-resolved
/// env, layered below this target's own envFiles/env/addToPath) used when
/// this call is itself a dependency invocation.
#[allow(clippy::too_many_arguments)]
pub fn execute_command_with_counter(
	spec: &CommandSpec,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	available_private_keys: Option<&[String]>,
	timings: bool,
	counter: &StepCounter,
	deps: &dyn DependencyResolver,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	output_prefix: Option<&str>,
) -> Result<ExecutionResult, ExecuteError> {
	let (setup, tailer, force_kill_guard) = ExecSetup::new(
		spec,
		args,
		working_dir,
		available_private_keys,
		parent_env,
		parent_add_to_path_chain,
		output_prefix,
	)?;
	let mut state = WalkState {
		commands_run: 0,
		failures: 0,
		last_status: None,
		failed: false,
	};
	let mut loop_scope = LoopScope::new();

	let walk_result = execute_steps_walk(
		&spec.commands,
		&setup,
		shell,
		args,
		working_dir,
		timings,
		counter,
		&force_kill_guard,
		&mut loop_scope,
		false, // not yet inside a parallel context
		deps,
		&mut state,
	);

	// Stop tailing log files (always, even on error)
	if let Some(t) = tailer {
		t.stop();
	}

	walk_result?;

	// If the target had a real failure (a step exited non-zero with no
	// `ignoreErrors` override at any level), surface a non-zero status to
	// the caller even if a later `when: failure` / `when: always` step
	// happened to exit cleanly.
	let final_status = if state.failed {
		state.last_status.filter(|s| !s.success()).unwrap_or_else(failed_status)
	} else {
		state.last_status.unwrap_or_else(dummy_success_status)
	};

	Ok(ExecutionResult {
		commands_run: state.commands_run,
		failures: state.failures,
		final_status,
	})
}

/// Mutable state threaded through the recursive step walker.
struct WalkState {
	commands_run: usize,
	failures: usize,
	last_status: Option<ExitStatus>,
	/// Whether any prior step in this target has failed (without
	/// being suppressed by `ignoreErrors`). Once `true`, subsequent
	/// `when: success` steps are skipped while `when: failure` and
	/// `when: always` steps still run. Stays `true` for the rest of
	/// the target — there is no "recovery" by a `failure` step
	/// succeeding.
	failed: bool,
}

#[allow(clippy::too_many_arguments)]
fn execute_steps_walk(
	steps: &[CommandStep],
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	loop_scope: &mut LoopScope,
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	for step in steps {
		// `when` gate: skip the step if its condition doesn't match the
		// current target state. Steps that don't carry `when` default to
		// `Success`, preserving the classic "abort on first failure" feel
		// (failed → skip the rest of `success` steps).
		if !step.effective_when().matches(state.failed) {
			continue;
		}

		let pre_failures = state.failures;

		match step {
			CommandStep::Shell(template) => execute_one_shell(
				template,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
				loop_scope,
				state,
			)?,
			CommandStep::TargetCall(call) => execute_one_target_call(call, setup, args, loop_scope, deps, state)?,
			CommandStep::When(when_step) => execute_when_block(
				when_step,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
				loop_scope,
				in_parallel,
				deps,
				state,
			)?,
			CommandStep::If(if_step) => execute_if_block(
				if_step,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
				loop_scope,
				in_parallel,
				deps,
				state,
			)?,
			CommandStep::For(for_step) => execute_for_block(
				for_step,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
				loop_scope,
				in_parallel,
				deps,
				state,
			)?,
		}

		// If anything in this step incremented the failure counter and the
		// target isn't running with `ignoreErrors: true`, flip into the
		// "failed" state so subsequent `when: success` steps are skipped
		// and `when: failure` / `when: always` steps run.
		if state.failures > pre_failures && !setup.ignore_errors {
			state.failed = true;
		}
	}
	Ok(())
}

/// Execute a `when`-guarded block. The outer walker has already gated this
/// step on the current target state, so we know we should run the inner
/// commands. For `when: failure` / `when: always`, the inner walker runs
/// with `state.failed = false` locally so its default `when: success`
/// children execute (we're already in cleanup mode — we want them to run).
/// `ignoreErrors` on the block is honored — failures inside don't flip the
/// outer `failed` flag.
#[allow(clippy::too_many_arguments)]
fn execute_when_block(
	when_step: &WhenStep,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	loop_scope: &mut LoopScope,
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let local_ignore = when_step.ignore_errors.unwrap_or(false);
	let outer_failed = state.failed;
	let pre_failures = state.failures;

	// For `failure` / `always` entries we reset the local failed state so
	// the inner walker doesn't skip every default-`when:success` child.
	// The outer state is restored (or merged) below.
	if when_step.when != WhenCondition::Success {
		state.failed = false;
	}

	let walk = execute_steps_walk(
		&when_step.commands,
		setup,
		shell,
		args,
		working_dir,
		timings,
		counter,
		force_kill_guard,
		loop_scope,
		in_parallel,
		deps,
		state,
	);

	let inner_failed = state.failed;

	if local_ignore {
		// Restore the outer state — failures inside don't flip the target.
		state.failed = outer_failed;
		state.failures = pre_failures;
		let _ = walk;
		Ok(())
	} else {
		// Outer was failed (or the inner generated new failures) → keep failed.
		state.failed = outer_failed || inner_failed;
		walk
	}
}

/// Sequentially run an `@target` invocation against the active resolver.
/// Substitutes the args template, shlex-splits, and dispatches.
///
/// Failures inside the dep (shell commands exiting non-zero) come back
/// through `result.failures` and are folded into the parent's count so the
/// outer walker can flip `failed`. A genuine spawn error (process couldn't
/// start, missing binary, etc.) propagates as `Err`.
fn execute_one_target_call(
	call: &TargetCallStep,
	setup: &ExecSetup,
	args: &RunArgs,
	loop_scope: &LoopScope,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	// Substitute the target name so dynamic patterns like `@$(LOOP.ns):build`
	// dispatch to the correct namespaced target. No-op for static names.
	let target = args.substitute_with_loop(&call.target, &setup.env, loop_scope)?;
	let argv = resolve_target_call_argv(call, args, &setup.env, loop_scope)?;
	let result = deps.run_dependency(
		&target,
		argv,
		&setup.env,
		&setup.add_to_path_chain,
		call.optional,
		setup.output_prefix.as_deref(),
	)?;
	state.commands_run += result.commands_run;
	state.failures += result.failures;
	state.last_status = Some(result.final_status).or(state.last_status);
	Ok(())
}

/// Substitute the args template and shlex-split it into argv. `$(ARGS)`
/// expansion happens here, so `@build $(ARGS)` correctly forwards the
/// caller's positional arguments. The original (unsubstituted) template is
/// surfaced in error messages to avoid leaking secrets.
fn resolve_target_call_argv(
	call: &TargetCallStep,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
) -> Result<Vec<String>, ExecuteError> {
	if call.args_template.is_empty() {
		return Ok(Vec::new());
	}
	let substituted = args.substitute_with_loop(&call.args_template, env, loop_scope)?;
	let trimmed = substituted.trim();
	if trimmed.is_empty() {
		return Ok(Vec::new());
	}
	match shlex::split(trimmed) {
		Some(argv) => Ok(argv),
		None => Err(ExecuteError::DependencyFailed(
			call.target.clone(),
			format!(
				"could not tokenize args template `{}` (unbalanced quotes?)",
				call.args_template
			),
		)),
	}
}

#[allow(clippy::too_many_arguments)]
fn execute_one_shell(
	template: &str,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	loop_scope: &LoopScope,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let cmd_str = args.substitute_with_loop(template, &setup.env, loop_scope)?;

	let (step, total) = counter.next_step();
	if setup.logging {
		let log_str = args.substitute_redacted_with_loop(template, &setup.env, loop_scope)?;
		log_command(&log_str, step, total);
	}

	let shell_args = shell.exec_args(&cmd_str);

	let cmd_start = Instant::now();
	let mut cmd = Command::new(&shell.path);
	cmd.args(&shell_args).envs(&setup.env).current_dir(working_dir);

	let status = if let Some(prefix) = setup.output_prefix.as_deref() {
		// Inherited from a parallel ancestor: pipe stdout/stderr and route
		// through line-prefixed pumps so this dep's output stays tagged with
		// the parent's branch identity (e.g. `[3] `).
		cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
		let mut child = cmd.spawn()?;
		if let Some(guard) = force_kill_guard {
			guard.add_child(&child);
		}
		let mut handles = Vec::new();
		if let Some(out) = child.stdout.take() {
			handles.push(spawn_line_pump(out, prefix.to_string(), OutputStream::Stdout));
		}
		if let Some(err) = child.stderr.take() {
			handles.push(spawn_line_pump(err, prefix.to_string(), OutputStream::Stderr));
		}
		let s = child.wait()?;
		for h in handles {
			let _ = h.join();
		}
		// All pump threads have finished forwarding bytes to the global
		// writer thread; drain the writer's queue so this command's output
		// is fully visible before we return.
		flush_writer_thread();
		s
	} else if let Some(guard) = force_kill_guard {
		// Inherit stdio + track the child for force-kill on Ctrl+C.
		let mut child = cmd.spawn()?;
		guard.add_child(&child);
		child.wait()?
	} else {
		// Fast path: inherit stdio.
		cmd.status()?
	};

	if timings {
		log_command_timing(cmd_start.elapsed());
	}

	state.commands_run += 1;
	state.last_status = Some(status);
	if !status.success() {
		state.failures += 1;
	}
	Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_if_block(
	if_step: &IfStep,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	loop_scope: &mut LoopScope,
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let condition_value = evaluate_if_condition(if_step, args, &setup.env, loop_scope)?;

	let chosen_branch: &[CommandStep] = if condition_value {
		&if_step.then
	} else {
		match &if_step.r#else {
			Some(b) => b.as_slice(),
			None => &[],
		}
	};

	// `ignoreErrors: true` fully isolates the branch's failures from the
	// outer state. The failure counter must NOT propagate either: the
	// outer walker checks `state.failures > pre_failures` to flip
	// `state.failed`, which would skip subsequent default-`when: success`
	// siblings (and, when this `if` lives inside a `for` body, every later
	// iteration's default-success steps). Mirrors `execute_when_block`.
	let local_ignore = if_step.ignore_errors.unwrap_or(false);
	// If this `if` is gated with `when: failure` / `when: always`, the
	// inner branch runs in "fresh" mode (state.failed=false locally) so
	// its default-success children execute.
	let entered_in_failure_mode = if_step.when.unwrap_or_default() != WhenCondition::Success;
	let mut local_state = WalkState {
		commands_run: 0,
		failures: 0,
		last_status: None,
		failed: if entered_in_failure_mode { false } else { state.failed },
	};

	let walk = execute_steps_walk(
		chosen_branch,
		setup,
		shell,
		args,
		working_dir,
		timings,
		counter,
		force_kill_guard,
		loop_scope,
		in_parallel,
		deps,
		&mut local_state,
	);

	state.commands_run += local_state.commands_run;
	state.last_status = local_state.last_status.or(state.last_status);

	if local_ignore {
		// Swallow everything internal: neither the failure count, the
		// failed flag, nor a walk error propagate.
		let _ = walk;
		return Ok(());
	}

	state.failed = state.failed || local_state.failed;
	state.failures += local_state.failures;
	walk
}

#[allow(clippy::too_many_arguments)]
fn execute_for_block(
	for_step: &ForStep,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	loop_scope: &mut LoopScope,
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let iterations = expand_for_iterations(for_step, args, &setup.env, loop_scope, working_dir)?;

	// Inflate the step counter total when actual iteration count exceeds
	// the static estimate (which assumed 1 iteration for glob / shell /
	// namespaces, and the literal count for `in: [...]`).
	let body_count = count_leaves(&for_step.body);
	let estimated_iterations = match &for_step.r#in {
		Some(runfile_parser::ForInValue::Literal(items)) => items.len(),
		Some(runfile_parser::ForInValue::Namespaces) | None => 1,
	};
	if iterations.len() > estimated_iterations {
		let extra = (iterations.len() - estimated_iterations) * body_count;
		counter.add_to_total(extra);
	} else if iterations.len() < estimated_iterations {
		// Too few iterations — nothing we can do about the over-counted total
		// here without complicating the arithmetic. This is documented as a
		// minor display imprecision (final N may be < total).
	}

	let parallel_requested = for_step.parallel.unwrap_or(false);
	let run_parallel = parallel_requested && !in_parallel;
	if parallel_requested && in_parallel {
		eprintln!(
			"[runfile] Warning: nested `for` block has parallel: true but is already inside a parallel context — running iterations sequentially."
		);
	}

	let local_ignore = for_step.ignore_errors.unwrap_or(false);

	if run_parallel {
		execute_for_parallel(
			for_step,
			&iterations,
			setup,
			shell,
			args,
			working_dir,
			timings,
			counter,
			force_kill_guard,
			loop_scope,
			deps,
			local_ignore,
			state,
		)
	} else {
		// Sequential iteration.
		// If this `for` is gated with `when: failure` / `when: always`, the
		// body runs in "fresh" mode locally so default-success children
		// inside execute.
		let entered_in_failure_mode = for_step.when.unwrap_or_default() != WhenCondition::Success;
		let mut local_state = WalkState {
			commands_run: 0,
			failures: 0,
			last_status: None,
			failed: if entered_in_failure_mode { false } else { state.failed },
		};
		let mut iter_error: Option<ExecuteError> = None;

		for value in &iterations {
			loop_scope.push(&for_step.var, value.clone());
			let result = execute_steps_walk(
				&for_step.body,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
				loop_scope,
				in_parallel,
				deps,
				&mut local_state,
			);
			loop_scope.pop();
			// When the for-block has `ignoreErrors: true`, a failure in one
			// iteration must NOT cause subsequent iterations' default-when
			// steps to be skipped. Reset the local `failed` flag at the
			// boundary between iterations so each one starts fresh.
			if local_ignore {
				local_state.failed = false;
			}
			if let Err(e) = result {
				if local_ignore {
					local_state.failures = local_state.failures.max(1);
					continue;
				}
				iter_error = Some(e);
				break;
			}
		}

		state.commands_run += local_state.commands_run;
		state.last_status = local_state.last_status.or(state.last_status);

		// With `ignoreErrors: true`, the per-iteration `result` was always
		// `Ok` (errors are converted to `continue` above), so `iter_error`
		// is `None`. Swallow the failure count and flag entirely so the
		// outer walker doesn't see new failures and flip `state.failed`
		// on the next sibling step.
		if local_ignore {
			return Ok(());
		}

		state.failed = state.failed || local_state.failed;
		state.failures += local_state.failures;

		match iter_error {
			Some(e) => Err(e),
			None => Ok(()),
		}
	}
}

/// Run all iterations of a `for` block in parallel. Each iteration's body
/// is fully expanded to a flat list of leaves (shell commands + target-call
/// dependencies) and all leaves run concurrently — shells as child processes,
/// target calls on worker threads.
#[allow(clippy::too_many_arguments)]
fn execute_for_parallel(
	for_step: &ForStep,
	iterations: &[String],
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	loop_scope: &mut LoopScope,
	deps: &dyn DependencyResolver,
	local_ignore: bool,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	// Expand all iterations into a flat list of leaves (shell or target call),
	// with per-iteration substitution applied. Inner control flow is expanded
	// here — `if` blocks pick a branch using the inner substitution context,
	// and inner `for` blocks are forced sequential (per the outer-parallel-only
	// rule).
	let mut leaves: Vec<ParallelLeaf> = Vec::new();

	for value in iterations {
		loop_scope.push(&for_step.var, value.clone());
		let leaves_result = collect_leaves_parallel(&for_step.body, setup, args, working_dir, loop_scope, &mut leaves);
		loop_scope.pop();
		leaves_result?;
	}

	if leaves.is_empty() {
		return Ok(());
	}

	let _ignore_timings = timings; // parallel never reports per-command timings
	run_parallel_leaves(
		leaves,
		setup,
		shell,
		working_dir,
		counter,
		force_kill_guard,
		deps,
		local_ignore,
		state,
	)
}

/// A flattened leaf collected from a parallel context — either a shell
/// command (substituted, ready to spawn) or a target-call dependency
/// (target name + tokenized argv, ready to dispatch on a worker thread).
/// Each leaf carries its effective `when` so the parallel runner can
/// partition execution into success/failure/always phases.
enum ParallelLeaf {
	Shell {
		template: String,
		substituted: String,
		when: WhenCondition,
	},
	TargetCall {
		target: String,
		argv: Vec<String>,
		when: WhenCondition,
		/// Set when the source step was an `@?target` call. Forwarded to the
		/// resolver so a missing target is silently skipped.
		optional: bool,
	},
}

impl ParallelLeaf {
	fn when(&self) -> WhenCondition {
		match self {
			ParallelLeaf::Shell { when, .. } | ParallelLeaf::TargetCall { when, .. } => *when,
		}
	}
}

/// Combine an outer `when` annotation with an inner one as we descend the
/// step tree. Returns `None` when the result is "never" (e.g. `success`
/// outer, `failure` inner — those nested blocks can never run).
fn intersect_when(outer: WhenCondition, inner: WhenCondition) -> Option<WhenCondition> {
	match (outer, inner) {
		(WhenCondition::Success, WhenCondition::Success) => Some(WhenCondition::Success),
		(WhenCondition::Success, WhenCondition::Failure) => None,
		(WhenCondition::Success, WhenCondition::Always) => Some(WhenCondition::Success),
		(WhenCondition::Failure, WhenCondition::Success) => None,
		(WhenCondition::Failure, WhenCondition::Failure) => Some(WhenCondition::Failure),
		(WhenCondition::Failure, WhenCondition::Always) => Some(WhenCondition::Failure),
		(WhenCondition::Always, inner) => Some(inner),
	}
}

/// Recursive helper used by parallel command expansion. Collects every leaf
/// (shell command and `@target` invocation) into `out`. Inner `if` blocks
/// pick a branch eagerly using the current substitution context. Inner
/// `for` blocks are expanded sequentially (their `parallel` flag is ignored
/// because we are already inside a parallel context).
fn collect_leaves_parallel(
	steps: &[CommandStep],
	setup: &ExecSetup,
	args: &RunArgs,
	working_dir: &Path,
	loop_scope: &mut LoopScope,
	out: &mut Vec<ParallelLeaf>,
) -> Result<(), ExecuteError> {
	collect_leaves_parallel_with_when(steps, setup, args, working_dir, loop_scope, WhenCondition::Success, out)
}

#[allow(clippy::too_many_arguments)]
fn collect_leaves_parallel_with_when(
	steps: &[CommandStep],
	setup: &ExecSetup,
	args: &RunArgs,
	working_dir: &Path,
	loop_scope: &mut LoopScope,
	outer_when: WhenCondition,
	out: &mut Vec<ParallelLeaf>,
) -> Result<(), ExecuteError> {
	for step in steps {
		// Compute the leaf's effective `when` by intersecting the outer
		// gate with this step's own. If it's "never", drop the entire
		// subtree (e.g. `when: failure` inside an outer `when: success`).
		let step_when = step.effective_when();
		let effective = match intersect_when(outer_when, step_when) {
			Some(w) => w,
			None => continue,
		};

		match step {
			CommandStep::Shell(template) => {
				let substituted = args.substitute_with_loop(template, &setup.env, loop_scope)?;
				out.push(ParallelLeaf::Shell {
					template: template.clone(),
					substituted,
					when: effective,
				});
			}
			CommandStep::TargetCall(call) => {
				// Substitute the target name so `@$(LOOP.ns):build`-style
				// dynamic targets dispatch to the right namespaced target.
				let target = args.substitute_with_loop(&call.target, &setup.env, loop_scope)?;
				let argv = resolve_target_call_argv(call, args, &setup.env, loop_scope)?;
				out.push(ParallelLeaf::TargetCall {
					target,
					argv,
					when: effective,
					optional: call.optional,
				});
			}
			CommandStep::When(when_step) => {
				collect_leaves_parallel_with_when(
					&when_step.commands,
					setup,
					args,
					working_dir,
					loop_scope,
					effective,
					out,
				)?;
			}
			CommandStep::If(if_step) => {
				let condition_value = evaluate_if_condition(if_step, args, &setup.env, loop_scope)?;
				let branch: &[CommandStep] = if condition_value {
					&if_step.then
				} else {
					match &if_step.r#else {
						Some(b) => b.as_slice(),
						None => &[],
					}
				};
				collect_leaves_parallel_with_when(branch, setup, args, working_dir, loop_scope, effective, out)?;
			}
			CommandStep::For(for_step) => {
				if for_step.parallel.unwrap_or(false) {
					eprintln!(
						"[runfile] Warning: nested `for` block has parallel: true but is already inside a parallel context — running iterations sequentially."
					);
				}
				let iterations = expand_for_iterations(for_step, args, &setup.env, loop_scope, working_dir)?;
				for value in iterations {
					loop_scope.push(&for_step.var, value);
					let r = collect_leaves_parallel_with_when(
						&for_step.body,
						setup,
						args,
						working_dir,
						loop_scope,
						effective,
						out,
					);
					loop_scope.pop();
					r?;
				}
			}
		}
	}
	Ok(())
}

/// Spawn a flat list of [`ParallelLeaf`]s concurrently and wait for all to
/// finish. Shell leaves spawn as child processes (with the standard
/// process-tree tracker); target-call leaves run on worker threads via
/// [`std::thread::scope`] so the resolver can borrow non-`'static` state.
///
/// Three-phase execution honors per-leaf `when`:
///   1. Run all `when: Success` (default) leaves in the parallel batch.
///   2. If any failed, run `when: Failure` leaves sequentially.
///   3. Always run `when: Always` leaves sequentially after the above.
///
/// `local_ignore` lets the immediate parent (e.g. a `for parallel` block)
/// suppress its own failure propagation while the global `setup.ignore_errors`
/// still controls target-level behaviour.
#[allow(clippy::too_many_arguments)]
fn run_parallel_leaves(
	leaves: Vec<ParallelLeaf>,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	working_dir: &Path,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	deps: &dyn DependencyResolver,
	local_ignore: bool,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	if leaves.is_empty() {
		return Ok(());
	}

	// Partition leaves by when. The `success` partition runs in the parallel
	// batch; `failure` / `always` run sequentially after, gated on whether
	// anything in the success partition failed.
	let mut success_leaves: Vec<ParallelLeaf> = Vec::new();
	let mut failure_leaves: Vec<ParallelLeaf> = Vec::new();
	let mut always_leaves: Vec<ParallelLeaf> = Vec::new();
	for leaf in leaves {
		match leaf.when() {
			WhenCondition::Success => success_leaves.push(leaf),
			WhenCondition::Failure => failure_leaves.push(leaf),
			WhenCondition::Always => always_leaves.push(leaf),
		}
	}

	let pre_failures = state.failures;
	if !success_leaves.is_empty() {
		run_parallel_batch(
			success_leaves,
			setup,
			shell,
			working_dir,
			counter,
			force_kill_guard,
			deps,
			local_ignore,
			state,
		)?;
	}

	let batch_failed = state.failures > pre_failures && !setup.ignore_errors && !local_ignore;
	if batch_failed {
		// Bubble the failure into the surrounding state so subsequent
		// `when: failure` / `when: always` blocks (at outer levels) see
		// the right state.
		state.failed = true;
	}

	if batch_failed && !failure_leaves.is_empty() {
		run_sequential_leaves(
			failure_leaves,
			setup,
			shell,
			working_dir,
			counter,
			force_kill_guard,
			deps,
			state,
		)?;
	}

	if !always_leaves.is_empty() {
		run_sequential_leaves(
			always_leaves,
			setup,
			shell,
			working_dir,
			counter,
			force_kill_guard,
			deps,
			state,
		)?;
	}

	// `local_ignore` (set when this batch is the body of a
	// `for parallel: true` with `ignoreErrors: true`) must fully isolate
	// internal failures from the outer state — otherwise the outer walker
	// sees the failure delta and flips `state.failed`, skipping subsequent
	// default-`when: success` siblings.
	if local_ignore {
		state.failures = pre_failures;
	}

	Ok(())
}

/// Run leaves sequentially as a fallback (used for `when: failure` and
/// `when: always` branches in a parallel context).
#[allow(clippy::too_many_arguments)]
fn run_sequential_leaves(
	leaves: Vec<ParallelLeaf>,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	working_dir: &Path,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let prefix_output = line_prefixing_enabled();
	for leaf in leaves {
		let (step, total) = counter.next_step();
		// In a prefixed parent context, inherit the parent's prefix so
		// output stays tagged. Otherwise no prefix (sequential fallback).
		let leaf_prefix: Option<String> = if !prefix_output {
			None
		} else {
			setup.output_prefix.clone()
		};
		match leaf {
			ParallelLeaf::Shell {
				template, substituted, ..
			} => {
				if setup.logging {
					log_command(&template, step, total);
				}
				let shell_args = shell.exec_args(&substituted);
				let mut cmd = Command::new(&shell.path);
				cmd.args(&shell_args).envs(&setup.env).current_dir(working_dir);
				let status = if let Some(prefix) = leaf_prefix.as_deref() {
					cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
					let mut child = cmd.spawn()?;
					if let Some(guard) = force_kill_guard {
						guard.add_child(&child);
					}
					let mut handles = Vec::new();
					if let Some(out) = child.stdout.take() {
						handles.push(spawn_line_pump(out, prefix.to_string(), OutputStream::Stdout));
					}
					if let Some(err) = child.stderr.take() {
						handles.push(spawn_line_pump(err, prefix.to_string(), OutputStream::Stderr));
					}
					let s = child.wait()?;
					for h in handles {
						let _ = h.join();
					}
					flush_writer_thread();
					s
				} else if let Some(guard) = force_kill_guard {
					let mut child = cmd.spawn()?;
					guard.add_child(&child);
					child.wait()?
				} else {
					cmd.status()?
				};
				state.commands_run += 1;
				state.last_status = Some(status);
				if !status.success() {
					state.failures += 1;
				}
			}
			ParallelLeaf::TargetCall {
				target, argv, optional, ..
			} => {
				if setup.logging {
					let prefix_marker = if optional { "@?" } else { "@" };
					let label = if argv.is_empty() {
						format!("{}{}", prefix_marker, target)
					} else {
						format!("{}{} {}", prefix_marker, target, argv.join(" "))
					};
					log_command(&label, step, total);
				}
				let result = deps.run_dependency(
					&target,
					argv,
					&setup.env,
					&setup.add_to_path_chain,
					optional,
					leaf_prefix.as_deref(),
				)?;
				state.commands_run += result.commands_run;
				state.failures += result.failures;
				state.last_status = Some(result.final_status).or(state.last_status);
			}
		}
	}
	Ok(())
}

/// Run a batch of leaves concurrently — the actual parallel pass that the
/// public `execute_parallel_*` entry points used to do directly. Now used
/// by [`run_parallel_leaves`] for the `when: success` partition.
#[allow(clippy::too_many_arguments)]
fn run_parallel_batch(
	leaves: Vec<ParallelLeaf>,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	working_dir: &Path,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	deps: &dyn DependencyResolver,
	local_ignore: bool,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let total = leaves.len();
	if total == 0 {
		return Ok(());
	}

	// Reserve one counter slot per leaf up-front. Target-call leaves account
	// for themselves as one slot here; the resolver bumps the global total on
	// entry so the dependency's own commands stay below `total`.
	let step_pairs: Vec<(usize, usize)> = (0..total).map(|_| counter.next_step()).collect();
	if setup.logging {
		for (leaf, &(step, total)) in leaves.iter().zip(step_pairs.iter()) {
			let label = match leaf {
				ParallelLeaf::Shell { template, .. } => template.clone(),
				ParallelLeaf::TargetCall {
					target, argv, optional, ..
				} => {
					let prefix = if *optional { "@?" } else { "@" };
					if argv.is_empty() {
						format!("{}{}", prefix, target)
					} else {
						format!("{}{} {}", prefix, target, argv.join(" "))
					}
				}
			};
			log_parallel_command(&label, step, total);
		}
	}

	// Split leaves into shells (spawned as processes) and target calls (run
	// on worker threads). Each leaf carries the prefix that should tag its
	// output. When this batch was reached via an ancestor that already set a
	// prefix, every leaf inherits it verbatim (preserving the outer partition
	// identity); otherwise each leaf gets a per-step prefix `[N]` matching
	// the upfront `(N/total)` log line.
	let prefix_output = line_prefixing_enabled();
	let mut shells: Vec<(String, String, Option<String>)> = Vec::new();
	let mut target_calls: Vec<(String, Vec<String>, bool, Option<String>)> = Vec::new();
	for (leaf, &(step, _total)) in leaves.into_iter().zip(step_pairs.iter()) {
		let leaf_prefix = if !prefix_output {
			None
		} else if let Some(parent) = setup.output_prefix.as_deref() {
			Some(parent.to_string())
		} else {
			Some(format_parallel_prefix(step))
		};
		match leaf {
			ParallelLeaf::Shell {
				template, substituted, ..
			} => shells.push((template, substituted, leaf_prefix)),
			ParallelLeaf::TargetCall {
				target, argv, optional, ..
			} => target_calls.push((target, argv, optional, leaf_prefix)),
		}
	}

	#[allow(unused_mut)]
	let mut tree_tracker = ProcessTreeTracker::new();

	let mut children: Vec<(String, Child, Vec<JoinHandle<()>>)> = Vec::with_capacity(shells.len());
	for (template, substituted, leaf_prefix) in &shells {
		let shell_args = shell.exec_args(substituted);
		let mut cmd = Command::new(&shell.path);
		cmd.args(&shell_args).envs(&setup.env).current_dir(working_dir);

		if leaf_prefix.is_some() {
			cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
		}

		#[cfg(unix)]
		tree_tracker.prepare_command(&mut cmd);

		let mut child = cmd.spawn()?;

		#[cfg(windows)]
		tree_tracker.add_child(&child);

		if let Some(guard) = force_kill_guard {
			guard.add_child(&child);
		}

		let mut pump_handles: Vec<JoinHandle<()>> = Vec::new();
		if let Some(prefix) = leaf_prefix {
			if let Some(out) = child.stdout.take() {
				pump_handles.push(spawn_line_pump(out, prefix.clone(), OutputStream::Stdout));
			}
			if let Some(err) = child.stderr.take() {
				pump_handles.push(spawn_line_pump(err, prefix.clone(), OutputStream::Stderr));
			}
		}

		children.push((template.clone(), child, pump_handles));
	}

	#[cfg(unix)]
	tree_tracker.children_spawned();

	let _sigint_guard = if !setup.force_kill {
		Some(IgnoreSigint::new())
	} else {
		None
	};

	let mut failures = 0usize;
	let mut shells_run = children.len();
	let mut first_error: Option<ExecuteError> = None;
	let mut wait_error: Option<std::io::Error> = None;
	let mut last_status: Option<ExitStatus> = None;
	let parent_env = &setup.env;
	let parent_chain = setup.add_to_path_chain.as_slice();

	// Run target calls on worker threads while children processes run
	// concurrently. `thread::scope` lets the threads borrow `deps`,
	// `parent_env`, and `parent_chain` without requiring `'static`. Each
	// worker forwards its leaf's `output_prefix` so the dispatched dep's
	// transitive shells get tagged with the partition identity (e.g. `[3] `).
	let dep_results: Vec<Result<ExecutionResult, ExecuteError>> = std::thread::scope(|scope| {
		let mut handles = Vec::with_capacity(target_calls.len());
		for (target, argv, optional, leaf_prefix) in target_calls {
			handles.push(scope.spawn(move || {
				deps.run_dependency(
					&target,
					argv,
					parent_env,
					parent_chain,
					optional,
					leaf_prefix.as_deref(),
				)
			}));
		}
		handles
			.into_iter()
			.map(|h| match h.join() {
				Ok(r) => r,
				Err(_) => Err(ExecuteError::DependencyFailed(
					String::new(),
					"worker thread panicked".to_string(),
				)),
			})
			.collect()
	});

	for (template, mut child, pump_handles) in children {
		match child.wait() {
			Ok(status) => {
				last_status = Some(status);
				if !status.success() {
					failures += 1;
					if first_error.is_none() && !setup.ignore_errors && !local_ignore {
						let code = status.code().unwrap_or(-1);
						first_error = Some(if code == -1 {
							ExecuteError::Signal(template)
						} else {
							ExecuteError::NonZeroExit(template, code)
						});
					}
				}
			}
			Err(e) => {
				failures += 1;
				if wait_error.is_none() {
					wait_error = Some(e);
				}
			}
		}
		// The child's pipe ends close on exit, so reader threads see EOF and
		// terminate. Joining ensures all buffered output has been flushed
		// to the global writer thread before we move on.
		for h in pump_handles {
			let _ = h.join();
		}
	}

	// All pump threads have queued their final bytes; block until the
	// global writer thread has actually written them out so this batch's
	// output is fully visible before we proceed to failure/always partitions
	// or return.
	flush_writer_thread();

	for result in dep_results {
		match result {
			Ok(dep_res) => {
				shells_run += dep_res.commands_run;
				failures += dep_res.failures;
				last_status = Some(dep_res.final_status).or(last_status);
			}
			Err(e) => {
				failures += 1;
				if first_error.is_none() && !setup.ignore_errors && !local_ignore {
					first_error = Some(e);
				}
			}
		}
	}

	tree_tracker.wait_for_descendants();
	drop(_sigint_guard);

	state.commands_run += shells_run;
	state.failures += failures;
	state.last_status = last_status.or(state.last_status);

	if let Some(err) = wait_error {
		return Err(ExecuteError::Spawn(err));
	}
	if let Some(err) = first_error {
		return Err(err);
	}
	Ok(())
}

fn dummy_success_status() -> ExitStatus {
	#[cfg(unix)]
	{
		use std::os::unix::process::ExitStatusExt;
		ExitStatus::from_raw(0)
	}
	#[cfg(windows)]
	{
		use std::os::windows::process::ExitStatusExt;
		ExitStatus::from_raw(0)
	}
}

/// Synthetic non-zero `ExitStatus` for the case where the target had a real
/// failure but the last actual command we ran exited 0 (e.g. a `when: always`
/// cleanup step that succeeded after a prior failure).
fn failed_status() -> ExitStatus {
	#[cfg(unix)]
	{
		use std::os::unix::process::ExitStatusExt;
		// Encode exit code 1 in the wait-status format (`code << 8`).
		ExitStatus::from_raw(1 << 8)
	}
	#[cfg(windows)]
	{
		use std::os::windows::process::ExitStatusExt;
		ExitStatus::from_raw(1)
	}
}

/// Execute a command specification with all leaf commands spawned in parallel.
///
/// All commands are started simultaneously after `if`/`for` blocks have been
/// fully expanded into a flat list of leaf shell strings. Stdout and stderr
/// are inherited (not buffered), so output flows through in real time. The
/// function waits for the entire process tree (children AND grandchildren)
/// to exit before returning — this prevents orphaned processes from spilling
/// output after the shell prompt returns.
pub fn execute_parallel(
	spec: &CommandSpec,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	available_private_keys: Option<&[String]>,
	timings: bool,
) -> Result<ExecutionResult, ExecuteError> {
	let counter = StepCounter::new(count_leaves(&spec.commands));
	execute_parallel_with_counter(
		spec,
		shell,
		args,
		working_dir,
		available_private_keys,
		timings,
		&counter,
		&NoOpDependencyResolver,
		None,
		&[],
		None,
	)
}

/// Like `execute_parallel`, but uses an externally provided step counter
/// so the `(N/total)` indicator stays continuous across multiple calls.
#[allow(clippy::too_many_arguments)]
pub fn execute_parallel_with_counter(
	spec: &CommandSpec,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	available_private_keys: Option<&[String]>,
	_timings: bool,
	counter: &StepCounter,
	deps: &dyn DependencyResolver,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	output_prefix: Option<&str>,
) -> Result<ExecutionResult, ExecuteError> {
	let (setup, tailer, force_kill_guard) = ExecSetup::new(
		spec,
		args,
		working_dir,
		available_private_keys,
		parent_env,
		parent_add_to_path_chain,
		output_prefix,
	)?;

	// Expand if/for blocks to a flat list of leaves (shell or @target).
	// Inner `for` blocks are forced sequential (their parallel flag is moot
	// once we're already in a parallel context).
	let mut leaves: Vec<ParallelLeaf> = Vec::new();
	let mut loop_scope = LoopScope::new();
	collect_leaves_parallel(&spec.commands, &setup, args, working_dir, &mut loop_scope, &mut leaves)?;

	let mut state = WalkState {
		commands_run: 0,
		failures: 0,
		last_status: None,
		failed: false,
	};
	let walk = run_parallel_leaves(
		leaves,
		&setup,
		shell,
		working_dir,
		counter,
		&force_kill_guard,
		deps,
		false, // top-level parallel: surface errors normally
		&mut state,
	);

	if let Some(t) = tailer {
		t.stop();
	}

	walk?;

	Ok(ExecutionResult {
		commands_run: state.commands_run,
		failures: state.failures,
		final_status: state.last_status.unwrap_or_else(dummy_success_status),
	})
}

// ──── Process tree tracking ────
//
// `child.wait()` only waits for the direct child process. When that child
// spawns its own children (e.g. `bash → run → bash → node`), those
// grandchildren can outlive the direct child and spill output after the
// shell prompt returns.
//
// Unix:  A sentinel pipe — the write end is inherited by children and all
//        their descendants. When ALL processes holding the write end have
//        exited, a read on the read end returns EOF.
//
// Windows: A Job Object — all child processes (and their descendants) are
//          tracked. We poll `ActiveProcesses` until it reaches zero.

#[cfg(unix)]
struct ProcessTreeTracker {
	/// Read end of the sentinel pipe.  Blocks on read until all writers close.
	read_end: Option<std::fs::File>,
	/// Write end — kept open until all children are spawned, then dropped.
	write_end: Option<std::fs::File>,
}

#[cfg(unix)]
impl ProcessTreeTracker {
	fn new() -> Self {
		let mut fds = [0i32; 2];
		if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
			// If pipe creation fails, degrade gracefully (no descendant waiting)
			return Self {
				read_end: None,
				write_end: None,
			};
		}
		// Set CLOEXEC on the read end — only the parent needs it
		unsafe {
			libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
		}
		// Write end does NOT have CLOEXEC — children inherit it automatically
		use std::os::unix::io::FromRawFd;
		let read_end = unsafe { std::fs::File::from_raw_fd(fds[0]) };
		let write_end = unsafe { std::fs::File::from_raw_fd(fds[1]) };
		Self {
			read_end: Some(read_end),
			write_end: Some(write_end),
		}
	}

	/// No-op on Unix — children inherit the write end automatically via fork.
	fn prepare_command(&self, _cmd: &mut Command) {
		// The write end FD is inherited because it lacks CLOEXEC.
	}

	/// Close the parent's copy of the write end. After this, only child
	/// processes (and their descendants) hold the write end open.
	fn children_spawned(&mut self) {
		self.write_end.take(); // Drop closes the FD
	}

	/// Block until every process that inherited the sentinel pipe has exited.
	fn wait_for_descendants(&self) {
		if let Some(ref read_end) = self.read_end {
			// File::read needs &mut, but we can borrow via the OS fd
			let fd = {
				use std::os::unix::io::AsRawFd;
				read_end.as_raw_fd()
			};
			let mut buf = [0u8; 1];
			// This blocks until EOF (all write-end holders have exited)
			unsafe {
				libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1);
			}
		}
	}
}

#[cfg(windows)]
struct ProcessTreeTracker {
	job_handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessTreeTracker {
	fn new() -> Self {
		let handle =
			unsafe { windows_sys::Win32::System::JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
		Self { job_handle: handle }
	}

	fn add_child(&self, child: &Child) {
		if self.job_handle.is_null() {
			return;
		}
		use std::os::windows::io::AsRawHandle;
		unsafe {
			windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
				self.job_handle,
				child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
			);
		}
	}

	/// Poll until no active processes remain in the job.
	fn wait_for_descendants(&self) {
		if self.job_handle.is_null() {
			return;
		}
		loop {
			let mut info: windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
				unsafe { std::mem::zeroed() };
			let ok = unsafe {
				windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
					self.job_handle,
					windows_sys::Win32::System::JobObjects::JobObjectBasicAccountingInformation,
					&mut info as *mut _ as *mut _,
					std::mem::size_of_val(&info) as u32,
					std::ptr::null_mut(),
				)
			};
			if ok == 0 || info.ActiveProcesses == 0 {
				break;
			}
			std::thread::sleep(std::time::Duration::from_millis(50));
		}
	}
}

#[cfg(windows)]
impl Drop for ProcessTreeTracker {
	fn drop(&mut self) {
		if !self.job_handle.is_null() {
			unsafe {
				windows_sys::Win32::Foundation::CloseHandle(self.job_handle);
			}
		}
	}
}

// ──── SIGINT guard for parallel execution ────
//
// When running commands in parallel, Ctrl-C sends SIGINT to the entire
// foreground process group (parent + children). Without intervention the
// Rust runtime's default handler kills the parent before all `child.wait()`
// calls complete. The guard temporarily ignores SIGINT in the parent so
// only the children react; after all children exit the guard is dropped
// and default SIGINT handling is restored.

/// RAII guard that ignores SIGINT while alive and restores previous handling on drop.
#[cfg(unix)]
struct IgnoreSigint {
	prev: libc::sighandler_t,
}

#[cfg(unix)]
impl IgnoreSigint {
	fn new() -> Self {
		let prev = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
		Self { prev }
	}
}

#[cfg(unix)]
impl Drop for IgnoreSigint {
	fn drop(&mut self) {
		unsafe {
			libc::signal(libc::SIGINT, self.prev);
		}
	}
}

#[cfg(windows)]
struct IgnoreSigint;

#[cfg(windows)]
impl IgnoreSigint {
	fn new() -> Self {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ignore_ctrl_handler), 1);
		}
		Self
	}
}

#[cfg(windows)]
impl Drop for IgnoreSigint {
	fn drop(&mut self) {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ignore_ctrl_handler), 0);
		}
	}
}

#[cfg(windows)]
unsafe extern "system" fn ignore_ctrl_handler(_ctrl_type: u32) -> i32 {
	1 // TRUE = handled (suppress default behaviour)
}

/// Spawn a compound command as a detached background process and return immediately.
/// The spawned process outlives the parent. All commands are joined with ` && `.
pub fn execute_detached(
	compound_command: &str,
	shell: &ResolvedShell,
	env: &HashMap<String, String>,
	working_dir: &Path,
) -> Result<(), ExecuteError> {
	let shell_args = shell.exec_args(compound_command);

	Command::new(&shell.path)
		.args(&shell_args)
		.envs(env)
		.current_dir(working_dir)
		.stdin(Stdio::null())
		.spawn()?;
	Ok(())
}
