use crate::args::{check_env_case_duplicates, LoopVarGuard, RunArgs, SubstitutionError};
use crate::control_flow::{
	collect_shell_only_leaves, count_leaves, evaluate_if_condition, expand_for_iterations, resolve_match_branch,
	ControlFlowError, ShellLeafContext, ShellLeafFlattenError,
};
use crate::env::{build_env_with_base, EnvFileError, PrivateKeyProvider};
use crate::force_kill::ForceKillGuard;
use crate::logging::{is_logging_enabled, log_command, log_command_timing, StepCounter};
use crate::parallel::{collect_leaves_parallel, run_parallel_leaves, ParallelLeaf};
use crate::parallel_output::{flush_writer_thread, spawn_line_pump, OutputStream};
use crate::stdio_tailer::StdioTailerSet;
use runfile_parser::{
	CommandSpec, CommandStep, ExtendStdio, ForStep, IfStep, MatchStep, TargetCallStep, WhenCondition, WhenStep,
};
use runfile_shell::{ResolvedShell, ShellKind};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
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

	#[error("{0}")]
	ShellLeafFlatten(#[from] ShellLeafFlattenError),

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
pub(crate) struct ExecSetup {
	pub(crate) env: HashMap<String, String>,
	/// addToPath chain to pass to any `@target` dependency invoked from this
	/// target's commands: parent's chain + this target's own `add_to_path`.
	/// Empty when neither layer contributed any entries.
	pub(crate) add_to_path_chain: Vec<Vec<String>>,
	pub(crate) logging: bool,
	pub(crate) ignore_errors: bool,
	pub(crate) force_kill: bool,
	/// Output prefix inherited from a parallel ancestor. When set, every
	/// shell command in this target spawns with `Stdio::piped()` and routes
	/// its output through the line-prefix muxer using this prefix. Forwarded
	/// verbatim to nested `@target` calls so the partition identity propagates
	/// down the entire dependency tree. `None` at top-level / when no parallel
	/// ancestor has set one.
	pub(crate) output_prefix: Option<String>,
	/// Keeps the target's Runfile-declared `vars` live in the run-wide VARS map
	/// for the duration of this setup (i.e. the whole target execution), and
	/// restores any shadowed prior values when dropped. `None` when the target
	/// declared no vars. Held as a field so it drops at the end of the executor
	/// entry point, after the command walk completes.
	#[allow(dead_code)]
	vars_guard: Option<DeclaredVarsGuard>,
}

impl ExecSetup {
	#[allow(clippy::too_many_arguments)]
	pub(crate) fn new(
		spec: &CommandSpec,
		args: &RunArgs,
		working_dir: &Path,
		env_files_base_dir: &Path,
		available_private_keys: Option<&dyn PrivateKeyProvider>,
		parent_env: Option<&HashMap<String, String>>,
		parent_add_to_path_chain: &[Vec<String>],
		output_prefix: Option<&str>,
	) -> Result<(Self, Option<StdioTailerSet>, Option<ForceKillGuard>), ExecuteError> {
		let env = build_env_with_base(
			spec,
			working_dir,
			env_files_base_dir,
			args,
			available_private_keys,
			parent_env,
			Some(parent_add_to_path_chain),
		)?;
		check_env_case_duplicates(&env)?;

		// Apply the target's Runfile-declared `vars` (globals already merged in)
		// into the run-wide VARS map now that `env` is built — values may
		// reference `{{ ENV.* }}`. The returned guard restores shadowed values
		// when this setup drops.
		let vars_guard = DeclaredVarsGuard::apply(spec, args, &env)?;

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
			vars_guard,
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

/// RAII scope for a target's Runfile-declared `vars`. On [`apply`](Self::apply)
/// it substitutes each declared value (against the built env + `ARGS` / `FLAGS`
/// / `RUN` / already-set `VARS`) and writes it into the run-wide VARS map; on
/// drop it restores every key it overwrote to its prior value (removing keys
/// that didn't previously exist).
///
/// This gives declared vars the same per-target scoping `env` has: a parent's
/// declared vars are visible inside an `@target` dependency (the VARS map is
/// shared by `Arc`), but a dependency's own declared vars do NOT leak back into
/// the parent once the dependency returns. `define(...)` calls made *during*
/// command execution still follow their existing propagation semantics — the
/// guard only manages the keys it set at setup time.
pub(crate) struct DeclaredVarsGuard {
	vars: Arc<Mutex<HashMap<String, String>>>,
	/// `(key, prior)` for each key this guard wrote. `prior == None` means the
	/// key was absent before and should be removed on restore.
	prior: Vec<(String, Option<String>)>,
}

impl DeclaredVarsGuard {
	/// Evaluate `spec.vars` (globals already merged in at parse time) against
	/// `env` and insert each resolved value into the VARS map. Returns `None`
	/// when the target declared no vars (so callers pay nothing).
	///
	/// Keys are processed in sorted order, inserting each before evaluating the
	/// next, so a later var can deterministically reference an earlier one via
	/// `{{ VAR.<earlier> }}`. A value whose substitution has no default and no
	/// resolvable source errors out — same contract as everywhere else.
	pub(crate) fn apply(
		spec: &CommandSpec,
		args: &RunArgs,
		env: &HashMap<String, String>,
	) -> Result<Option<Self>, SubstitutionError> {
		let declared = match &spec.vars {
			Some(v) if !v.is_empty() => v,
			_ => return Ok(None),
		};
		let mut keys: Vec<&String> = declared.keys().collect();
		keys.sort();
		let mut prior: Vec<(String, Option<String>)> = Vec::with_capacity(keys.len());
		for key in keys {
			let raw = declared[key].to_env_string();
			// Substitute BEFORE locking the VARS map: `substitute` itself locks
			// the map for `{{ VAR.* }}` reads, so holding the lock here would
			// deadlock. Earlier keys are already inserted and thus visible.
			let resolved = args.substitute(&raw, env)?;
			let previous = args.vars.lock().unwrap().insert(key.clone(), resolved);
			prior.push((key.clone(), previous));
		}
		Ok(Some(Self {
			vars: args.vars.clone(),
			prior,
		}))
	}
}

impl Drop for DeclaredVarsGuard {
	fn drop(&mut self) {
		let mut map = self.vars.lock().unwrap();
		// Unwind in reverse insertion order so a key written more than once
		// (shouldn't happen for distinct map keys, but cheap insurance) lands
		// back on its true prior value.
		for (key, previous) in self.prior.drain(..).rev() {
			match previous {
				Some(v) => {
					map.insert(key, v);
				}
				None => {
					map.remove(&key);
				}
			}
		}
	}
}

/// Substitute `{{ ARG.* }}` and `{{ ENV.* }}` references in extendStdio fromFile paths.
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
	available_private_keys: Option<&dyn PrivateKeyProvider>,
	timings: bool,
) -> Result<ExecutionResult, ExecuteError> {
	let counter = StepCounter::new(count_leaves(&spec.commands));
	// No separate Runfile context → reuse `working_dir` as the env-files base
	// (matches the legacy single-base behaviour for callers that don't have a
	// distinct Runfile parent dir, e.g. unit tests).
	execute_command_with_counter(
		spec,
		shell,
		args,
		working_dir,
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
	env_files_base_dir: &Path,
	available_private_keys: Option<&dyn PrivateKeyProvider>,
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
		env_files_base_dir,
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

	let walk_result = execute_steps_walk(
		&spec.commands,
		&setup,
		shell,
		args,
		working_dir,
		timings,
		counter,
		&force_kill_guard,
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

/// Join a flat list of shell command leaves with the appropriate sequencing
/// operator for the given shell, honoring `ignore_errors` semantics.
///
/// - `ignore_errors == false` (default): use `&&` so the joined command stops
///   at the first failure. Works on every shell we support — bash/zsh/sh/fish
///   3.0+, PowerShell 7+ (`&&` is the pipeline-chain operator), and cmd.exe.
/// - `ignore_errors == true`: use `;` (or `&` for cmd.exe), so subsequent
///   leaves run regardless of the previous one's exit code.
///
/// A single leaf is returned verbatim — no separator added.
pub fn join_shell_commands(commands: &[String], shell_kind: &ShellKind, ignore_errors: bool) -> String {
	if commands.len() == 1 {
		return commands[0].clone();
	}
	let separator = match (shell_kind, ignore_errors) {
		(ShellKind::Cmd, false) => " && ",
		(ShellKind::Cmd, true) => " & ",
		(_, false) => " && ",
		(_, true) => "; ",
	};
	commands.join(separator)
}

/// Execute a `sameShell: true` target.
///
/// Walks `spec.commands` to flatten control-flow blocks (`if` / `for` /
/// `match` / `when`) into a flat list of resolved shell-command strings, then
/// joins them with [`join_shell_commands`] and dispatches them as **one**
/// shell invocation. State changes (e.g. `cd`, shell variables, `set -e`)
/// persist across steps because they all run in the same process.
///
/// `@target` invocations inside the command list are rejected — they would
/// run in their own shell context, which violates the sameShell invariant.
///
/// Failure semantics:
/// - With `ignoreErrors: false` (default), the leaves are joined with `&&` so
///   the joined command stops at the first failing leaf and returns that
///   exit code.
/// - With `ignoreErrors: true`, the leaves are joined with `;` (or `&` for
///   cmd.exe) so every leaf runs regardless. The shell exit code reflects
///   only the LAST leaf — same as bash's default scripting behaviour.
///
/// Compatibility:
/// - `parallel: true` is meaningful only when there are multiple shell
///   invocations to run; sameShell collapses to one. The parallel flag is
///   silently ignored in this path (a warning is emitted by the runner).
/// - `detach: true` is handled in the runner's detach branch — it joins with
///   the same logic and spawns the joined command as a single detached
///   process.
#[allow(clippy::too_many_arguments)]
pub fn execute_same_shell_with_counter(
	spec: &CommandSpec,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	env_files_base_dir: &Path,
	available_private_keys: Option<&dyn PrivateKeyProvider>,
	timings: bool,
	counter: &StepCounter,
	parent_env: Option<&HashMap<String, String>>,
	parent_add_to_path_chain: &[Vec<String>],
	output_prefix: Option<&str>,
) -> Result<ExecutionResult, ExecuteError> {
	let (setup, tailer, force_kill_guard) = ExecSetup::new(
		spec,
		args,
		working_dir,
		env_files_base_dir,
		available_private_keys,
		parent_env,
		parent_add_to_path_chain,
		output_prefix,
	)?;

	let flatten_result = collect_shell_only_leaves(
		&spec.commands,
		args,
		&setup.env,
		working_dir,
		ShellLeafContext::SameShell,
	);
	let leaves = match flatten_result {
		Ok(l) => l,
		Err(e) => {
			if let Some(t) = tailer {
				t.stop();
			}
			return Err(e.into());
		}
	};

	// Drop empty / whitespace-only leaves the same way the regular executor
	// does (e.g. lines whose only content is `{{ define(...) }}`). The static
	// counter total may have included these — `subtract_from_total` keeps the
	// visible `(N/total)` ratio honest in either case.
	let mut filtered: Vec<String> = Vec::with_capacity(leaves.len());
	let mut dropped = 0usize;
	for leaf in leaves {
		if leaf.trim().is_empty() {
			dropped += 1;
		} else {
			filtered.push(leaf);
		}
	}
	if dropped > 0 {
		counter.subtract_from_total(dropped);
	}

	if filtered.is_empty() {
		if let Some(t) = tailer {
			t.stop();
		}
		return Ok(ExecutionResult {
			commands_run: 0,
			failures: 0,
			final_status: dummy_success_status(),
		});
	}

	let ignore_errors = setup.ignore_errors;
	let joined = join_shell_commands(&filtered, &shell.kind, ignore_errors);

	// Counter accounting: sameShell collapses every leaf into ONE shell
	// invocation. The runner's `count_target_leaves_recursive` already
	// returns 1 for sameShell targets, so the global total is correctly
	// sized from the entry point — but local callers (e.g. tests using
	// `execute_command`) may have sized the counter from the raw step tree,
	// in which case we need to roll back the `(N - 1)` extra slots so the
	// visible ratio stays honest. `subtract_from_total` saturates, so the
	// "already 1" case is a no-op.
	let (step, total) = counter.next_step();
	if filtered.len() > 1 {
		counter.subtract_from_total(filtered.len() - 1);
	}

	if setup.logging {
		// Log each leaf so the user sees what's queued. Step number is shared
		// across all of them — they're a single shell process from the outside.
		for leaf in &filtered {
			log_command(leaf, step, total);
		}
	}

	let shell_args = shell.exec_args(&joined);

	let cmd_start = Instant::now();
	let mut cmd = Command::new(&shell.path);
	let spawn_cwd = args.spawn_cwd(working_dir);
	cmd.args(&shell_args).envs(&setup.env).current_dir(&spawn_cwd);
	// forceKillOnSigInt: put the child in its own process group (before spawn)
	// so a Ctrl+C force-kill reaches its descendants too.
	if let Some(guard) = force_kill_guard.as_ref() {
		guard.prepare_command(&mut cmd);
	}

	let status = if let Some(prefix) = setup.output_prefix.as_deref() {
		// Inherited from a parallel ancestor: pipe stdio + line-prefix the output.
		cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
		let mut child = cmd.spawn()?;
		if let Some(guard) = force_kill_guard.as_ref() {
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
	} else if let Some(guard) = force_kill_guard.as_ref() {
		let mut child = cmd.spawn()?;
		guard.add_child(&child);
		child.wait()?
	} else {
		cmd.status()?
	};

	if timings {
		log_command_timing(cmd_start.elapsed());
	}

	if let Some(t) = tailer {
		t.stop();
	}

	let failures = if status.success() { 0 } else { 1 };

	Ok(ExecutionResult {
		commands_run: 1,
		failures,
		final_status: status,
	})
}

/// Mutable state threaded through the recursive step walker.
pub(crate) struct WalkState {
	pub(crate) commands_run: usize,
	pub(crate) failures: usize,
	pub(crate) last_status: Option<ExitStatus>,
	/// Whether any prior step in this target has failed (without
	/// being suppressed by `ignoreErrors`). Once `true`, subsequent
	/// `when: success` steps are skipped while `when: failure` and
	/// `when: always` steps still run. Stays `true` for the rest of
	/// the target — there is no "recovery" by a `failure` step
	/// succeeding.
	pub(crate) failed: bool,
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
				state,
			)?,
			CommandStep::TargetCall(call) => execute_one_target_call(call, setup, args, deps, state)?,
			CommandStep::When(when_step) => execute_when_block(
				when_step,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
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
				in_parallel,
				deps,
				state,
			)?,
			CommandStep::Match(match_step) => execute_match_block(
				match_step,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
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
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	// Substitute the target name so dynamic patterns like `@{{ VAR.ns }}:build`
	// dispatch to the correct namespaced target. No-op for static names.
	let target = args.substitute(&call.target, &setup.env)?;
	let argv = resolve_target_call_argv(call, args, &setup.env)?;
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

/// Substitute the args template and shlex-split it into argv. `{{ ARGS }}`
/// expansion happens here, so `@build {{ ARGS }}` correctly forwards the
/// caller's positional arguments. The original (unsubstituted) template is
/// surfaced in error messages to avoid leaking secrets.
pub(crate) fn resolve_target_call_argv(
	call: &TargetCallStep,
	args: &RunArgs,
	env: &HashMap<String, String>,
) -> Result<Vec<String>, ExecuteError> {
	if call.args_template.is_empty() {
		return Ok(Vec::new());
	}
	let substituted = args.substitute(&call.args_template, env)?;
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
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let cmd_str = match args.substitute(template, &setup.env) {
		Ok(s) => s,
		Err(SubstitutionError::UserError(_)) => {
			// `error('msg')` fired during substitution. The message was already
			// printed to stderr by the function itself. Treat this line as a
			// failed command — consume a step, record the failure, and return
			// `Ok` so the walker keeps going: subsequent `when: failure` /
			// `when: always` steps still run, and `ignoreErrors` is honored (the
			// walker derives `state.failed` from `state.failures`, same as a
			// non-zero shell exit).
			let (step, total) = counter.next_step();
			if setup.logging {
				log_command("error(...)", step, total);
			}
			state.commands_run += 1;
			state.last_status = Some(failed_status());
			state.failures += 1;
			return Ok(());
		}
		Err(e) => return Err(e.into()),
	};

	// Empty after substitution → true no-op: no shell dispatch, no step
	// number consumed, no log line, no contribution to the global step
	// total. The most common cause is a line whose only content is
	// `{{ define(...) }}` (which resolves to `""`), but any all-whitespace
	// template lands here too. The static `count_leaves` estimate already
	// counted this Shell step toward the total, so we decrement to keep
	// the visible `(N/total)` ratio accurate. Side effects from `define`
	// have already happened during `substitute` above. We deliberately
	// don't touch `state.commands_run`/`state.last_status` either —
	// `final_status` falls back to `dummy_success_status()` when no
	// command set a status, so a target whose body is purely
	// `define`-only lines still reports success.
	if cmd_str.trim().is_empty() {
		counter.subtract_from_total(1);
		return Ok(());
	}

	let (step, total) = counter.next_step();
	if setup.logging {
		let log_str = args.substitute_redacted(template, &setup.env)?;
		log_command(&log_str, step, total);
	}

	let shell_args = shell.exec_args(&cmd_str);

	let cmd_start = Instant::now();
	let mut cmd = Command::new(&shell.path);
	// `set_cwd(...)` may have run during the substitute pass above. Resolve
	// the spawn cwd against the override now (the resolver short-circuits to
	// `working_dir` when no override is set).
	let spawn_cwd = args.spawn_cwd(working_dir);
	cmd.args(&shell_args).envs(&setup.env).current_dir(&spawn_cwd);
	// forceKillOnSigInt: put the child in its own process group (before spawn)
	// so a Ctrl+C force-kill reaches its descendants too.
	if let Some(guard) = force_kill_guard {
		guard.prepare_command(&mut cmd);
	}

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
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let condition_value = evaluate_if_condition(if_step, args, &setup.env)?;

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

/// Execute a `match` block: resolve the match value, dispatch to the
/// matching case (or `default`, or surface a no-match error), and walk the
/// chosen branch. Mirrors [`execute_if_block`]'s isolation semantics:
/// `ignoreErrors: true` fully isolates the chosen branch's failures from
/// the outer state; otherwise the branch's failure count and `failed` flag
/// merge into the outer walker.
#[allow(clippy::too_many_arguments)]
fn execute_match_block(
	match_step: &MatchStep,
	setup: &ExecSetup,
	shell: &ResolvedShell,
	args: &RunArgs,
	working_dir: &Path,
	timings: bool,
	counter: &StepCounter,
	force_kill_guard: &Option<ForceKillGuard>,
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let chosen_branch = resolve_match_branch(match_step, args, &setup.env)?;

	let local_ignore = match_step.ignore_errors.unwrap_or(false);
	let entered_in_failure_mode = match_step.when.unwrap_or_default() != WhenCondition::Success;
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
		in_parallel,
		deps,
		&mut local_state,
	);

	state.commands_run += local_state.commands_run;
	state.last_status = local_state.last_status.or(state.last_status);

	if local_ignore {
		// Swallow everything internal: failure count, failed flag, walk error.
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
	in_parallel: bool,
	deps: &dyn DependencyResolver,
	state: &mut WalkState,
) -> Result<(), ExecuteError> {
	let iterations = expand_for_iterations(for_step, args, &setup.env, working_dir)?;

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

		// Scope the iteration variable into VARS for the duration of the loop;
		// the guard restores any prior `VAR.<var>` value (or removes the entry
		// if none) when it drops at end-of-loop.
		let var_guard = LoopVarGuard::enter(&args.vars, for_step.var.as_str());
		for value in &iterations {
			var_guard.set(value.clone());
			let result = execute_steps_walk(
				&for_step.body,
				setup,
				shell,
				args,
				working_dir,
				timings,
				counter,
				force_kill_guard,
				in_parallel,
				deps,
				&mut local_state,
			);
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
		drop(var_guard);

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

	// Collection is sequential, so the `LoopVarGuard` save/restore semantics
	// work the same as in `execute_for_block`: substitute each iteration's
	// body with `VAR.<var>` set to the current iteration value, then restore
	// at end of collection. Substituted leaves carry the iteration value
	// baked in, so when the parallel batch dispatches later, no race on the
	// shared VARS map matters for shell leaves. (TargetCall leaves dispatch
	// fresh `RunArgs` whose body still reads the parent's VARS Arc — see
	// the limitation note in CLAUDE.md.)
	let var_guard = LoopVarGuard::enter(&args.vars, for_step.var.as_str());
	for value in iterations {
		var_guard.set(value.clone());
		collect_leaves_parallel(&for_step.body, setup, args, working_dir, counter, &mut leaves)?;
	}
	drop(var_guard);

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

/// Build the human-readable label for a `@target` invocation used in
/// log lines and the parallel-failure summary (`@target` or
/// `@?target arg1 arg2`).
pub(crate) fn format_target_call_label(target: &str, argv: &[String], optional: bool) -> String {
	let prefix = if optional { "@?" } else { "@" };
	if argv.is_empty() {
		format!("{prefix}{target}")
	} else {
		format!("{prefix}{target} {}", argv.join(" "))
	}
}

/// Classify a [`ExecutionResult`] from a successful `run_dependency` call into
/// a short failure-detail phrase, when the dep reported internal failures
/// without surfacing them as an `Err`. Returns `None` when the dep had zero
/// failures.
pub(crate) fn dep_result_failure_detail(result: &ExecutionResult) -> Option<String> {
	if result.failures == 0 {
		return None;
	}
	let detail = match result.final_status.code() {
		Some(0) => format!("{} command(s) failed", result.failures),
		Some(code) => format!("exit code {code}"),
		None => "terminated by signal".to_string(),
	};
	Some(detail)
}

/// Classify an `ExecuteError` from a `run_dependency` call into a short
/// failure-detail phrase for the parallel-failure summary.
pub(crate) fn execute_error_failure_detail(err: &ExecuteError) -> String {
	match err {
		ExecuteError::NonZeroExit(_, code) => format!("exit code {code}"),
		ExecuteError::Signal(_) => "terminated by signal".to_string(),
		other => format!("error: {other}"),
	}
}

pub(crate) fn dummy_success_status() -> ExitStatus {
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
pub(crate) fn failed_status() -> ExitStatus {
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
	available_private_keys: Option<&dyn PrivateKeyProvider>,
	timings: bool,
) -> Result<ExecutionResult, ExecuteError> {
	let counter = StepCounter::new(count_leaves(&spec.commands));
	execute_parallel_with_counter(
		spec,
		shell,
		args,
		working_dir,
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
	env_files_base_dir: &Path,
	available_private_keys: Option<&dyn PrivateKeyProvider>,
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
		env_files_base_dir,
		available_private_keys,
		parent_env,
		parent_add_to_path_chain,
		output_prefix,
	)?;

	// Expand if/for blocks to a flat list of leaves (shell or @target).
	// Inner `for` blocks are forced sequential (their parallel flag is moot
	// once we're already in a parallel context).
	let mut leaves: Vec<ParallelLeaf> = Vec::new();
	collect_leaves_parallel(&spec.commands, &setup, args, working_dir, counter, &mut leaves)?;

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

	// Mirror the sequential walker's invariant (see `run_target_inner_body`):
	// when a failure occurred but the last leaf we observed exited 0 (e.g. a
	// later-iterated parallel `@target` succeeded while an earlier one failed),
	// synthesize a non-zero status. The CLI derives the process exit code from
	// `final_status.code()` alone, so without this a failing parallel batch
	// would report success.
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
pub(crate) struct ProcessTreeTracker {
	/// Read end of the sentinel pipe.  Blocks on read until all writers close.
	read_end: Option<std::fs::File>,
	/// Write end — kept open until all children are spawned, then dropped.
	write_end: Option<std::fs::File>,
}

#[cfg(unix)]
impl ProcessTreeTracker {
	pub(crate) fn new() -> Self {
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
	pub(crate) fn prepare_command(&self, _cmd: &mut Command) {
		// The write end FD is inherited because it lacks CLOEXEC.
	}

	/// Close the parent's copy of the write end. After this, only child
	/// processes (and their descendants) hold the write end open.
	pub(crate) fn children_spawned(&mut self) {
		self.write_end.take(); // Drop closes the FD
	}

	/// Block until every process that inherited the sentinel pipe has exited
	/// (the pipe reaches EOF when the last write-end holder closes).
	///
	/// Uses `poll()` in an EINTR-safe loop — the previous bare `libc::read`
	/// ignored its return value, so a signal (e.g. this run's own SIGINT
	/// handler, installed without `SA_RESTART`) could make it return before real
	/// EOF. `RUNFILE_DESCENDANT_WAIT_MS`, when set to a positive integer, caps
	/// the total wait so an inherited long-lived daemon (which keeps the pipe
	/// open indefinitely) can't hang the CLI forever; unset/0 blocks until the
	/// pipe closes (the default, preserving prior behavior).
	pub(crate) fn wait_for_descendants(&self) {
		let Some(ref read_end) = self.read_end else {
			return;
		};
		use std::os::unix::io::AsRawFd;
		let fd = read_end.as_raw_fd();

		let cap_ms: Option<u64> = std::env::var("RUNFILE_DESCENDANT_WAIT_MS")
			.ok()
			.and_then(|s| s.trim().parse::<u64>().ok())
			.filter(|&n| n > 0);

		let start = std::time::Instant::now();
		loop {
			let timeout_ms: i32 = match cap_ms {
				None => -1, // block indefinitely
				Some(cap) => {
					let elapsed = start.elapsed().as_millis() as u64;
					if elapsed >= cap {
						return;
					}
					(cap - elapsed).min(i32::MAX as u64) as i32
				}
			};
			let mut pfd = libc::pollfd {
				fd,
				events: libc::POLLIN,
				revents: 0,
			};
			let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
			if rc < 0 {
				// EINTR → retry (recompute remaining time); any other error →
				// stop waiting rather than spin.
				if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
					continue;
				}
				return;
			}
			if rc == 0 {
				return; // timed out (only reachable when a cap is set)
			}
			// Readable → EOF (all write-end holders closed). Drain and finish.
			let mut buf = [0u8; 1];
			let _ = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
			return;
		}
	}
}

#[cfg(windows)]
pub(crate) struct ProcessTreeTracker {
	job_handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessTreeTracker {
	pub(crate) fn new() -> Self {
		let handle =
			unsafe { windows_sys::Win32::System::JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
		Self { job_handle: handle }
	}

	// KNOWN LIMITATION (audit L14): the child is assigned to the job AFTER
	// `Command::spawn` returns, so a grandchild forked in the window between
	// spawn and assignment escapes the job (and thus `TerminateJobObject` /
	// `wait_for_descendants`). Closing this window requires the child to start
	// INSIDE the job atomically — either spawn `CREATE_SUSPENDED`, assign, then
	// resume the main thread, or create it with `PROC_THREAD_ATTRIBUTE_JOB_LIST`
	// — both of which need raw `CreateProcessW`, not `std::process::Command`.
	// That is a Windows-only rewrite that can't be compiled/tested from this
	// (Linux) environment, so it is intentionally deferred rather than shipped
	// unverified. In practice the window is microseconds and the shell child
	// takes far longer to fork its own children, so the race rarely fires.
	pub(crate) fn add_child(&self, child: &std::process::Child) {
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
	pub(crate) fn wait_for_descendants(&self) {
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
pub(crate) struct IgnoreSigint {
	prev: libc::sighandler_t,
}

#[cfg(unix)]
impl IgnoreSigint {
	pub(crate) fn new() -> Self {
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
pub(crate) struct IgnoreSigint;

#[cfg(windows)]
impl IgnoreSigint {
	pub(crate) fn new() -> Self {
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
