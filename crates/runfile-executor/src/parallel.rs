//! Parallel command execution: leaf collection from the command tree and the
//! concurrent batch runner.
//!
//! A `parallel: true` target (or `for` block) is flattened into a list of
//! [`ParallelLeaf`]s (shell commands + `@target` invocations) by
//! [`collect_leaves_parallel`], then dispatched by [`run_parallel_leaves`],
//! which partitions leaves into success/failure/always phases. Shell leaves
//! spawn as child processes with prefixed line-muxed output; `@target` leaves
//! run on worker threads so the resolver can borrow non-`'static` runner state.

use crate::args::{LoopVarGuard, RunArgs};
use crate::control_flow::{evaluate_if_condition, expand_for_iterations, resolve_match_branch};
use crate::executor::{
	dep_result_failure_detail, execute_error_failure_detail, format_target_call_label, resolve_target_call_argv,
	DependencyResolver, ExecSetup, ExecuteError, ExecutionResult, IgnoreSigint, ProcessTreeTracker, WalkState,
};
use crate::force_kill::ForceKillGuard;
use crate::logging::{log_command, log_parallel_command, log_parallel_failure_summary, StepCounter};
use crate::parallel_output::{
	flush_writer_thread, format_parallel_prefix, line_prefixing_enabled, shell_prefix_label, spawn_line_pump,
	OutputStream,
};
use runfile_parser::{CommandStep, WhenCondition};
use runfile_shell::ResolvedShell;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

/// A flattened leaf collected from a parallel context — either a shell
/// command (substituted, ready to spawn) or a target-call dependency
/// (target name + tokenized argv, ready to dispatch on a worker thread).
/// Each leaf carries its effective `when` so the parallel runner can
/// partition execution into success/failure/always phases.
pub(crate) enum ParallelLeaf {
	Shell {
		template: String,
		substituted: String,
		when: WhenCondition,
		/// Snapshot of `args.cwd_override` taken right after this leaf's
		/// substitution, so the parallel spawn uses the cwd that was
		/// effective for *this* leaf — not whatever later leaves wrote
		/// into the shared override during the rest of the collect pass.
		/// `None` when no `set_cwd` had been called yet.
		cwd_snapshot: Option<PathBuf>,
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
pub(crate) fn collect_leaves_parallel(
	steps: &[CommandStep],
	setup: &ExecSetup,
	args: &RunArgs,
	working_dir: &Path,
	counter: &StepCounter,
	out: &mut Vec<ParallelLeaf>,
) -> Result<(), ExecuteError> {
	// `None` = the target's top level, where there is no enclosing `when` gate.
	// A bare leaf here lands in the default (success/concurrent) batch, while a
	// top-level `when: failure` / `when: always` block maps to its own
	// partition. Seeding with a concrete `WhenCondition` was the M7 bug: with
	// `Success`, `intersect_when(Success, Failure) == None` dropped every
	// top-level `when: failure` cleanup block; and bare commands INSIDE a
	// `when: failure` block (which default to `when: success`) were dropped by
	// `intersect_when(Failure, Success) == None`. Making the gate `Option` and
	// having bare leaves inherit the enclosing gate fixes both.
	collect_leaves_parallel_with_when(steps, setup, args, working_dir, None, counter, out)
}

/// The step's OWN explicit `when`, or `None` when it carries no gate of its own
/// — a bare shell / `@target`, or an `if`/`for`/`match` with no `when`. Such a
/// step INHERITS the enclosing gate rather than being independently re-gated.
fn step_explicit_when(step: &CommandStep) -> Option<WhenCondition> {
	match step {
		CommandStep::Shell(_) | CommandStep::TargetCall(_) => None,
		CommandStep::When(w) => Some(w.when),
		CommandStep::If(i) => i.when,
		CommandStep::For(f) => f.when,
		CommandStep::Match(m) => m.when,
	}
}

#[allow(clippy::too_many_arguments)]
fn collect_leaves_parallel_with_when(
	steps: &[CommandStep],
	setup: &ExecSetup,
	args: &RunArgs,
	working_dir: &Path,
	outer: Option<WhenCondition>,
	counter: &StepCounter,
	out: &mut Vec<ParallelLeaf>,
) -> Result<(), ExecuteError> {
	for step in steps {
		// Compose the enclosing gate with this step's OWN explicit `when`.
		// A step with no gate of its own inherits the enclosing gate; an
		// explicit `when` composes via `intersect_when` and may prune the whole
		// subtree (e.g. `when: success` nested inside a `when: failure` block can
		// never run → `continue`).
		let gate: Option<WhenCondition> = match (outer, step_explicit_when(step)) {
			(o, None) => o,
			(None, Some(w)) => Some(w),
			(Some(o), Some(w)) => match intersect_when(o, w) {
				Some(e) => Some(e),
				None => continue,
			},
		};
		// Recursion carries the composed gate; a leaf uses its concrete
		// partition (an unconstrained gate → the default success batch).
		let effective = gate;
		let partition = gate.unwrap_or(WhenCondition::Success);

		match step {
			CommandStep::Shell(template) => {
				let substituted = args.substitute(template, &setup.env)?;
				// Drop empty leaves (typically `{{ define(...) }}` or
				// `{{ set_cwd(...) }}` lines that resolve to ""). Side-effecting
				// substitutions like `define` / `set_cwd` have already executed
				// during `substitute`; a no-op shell leaf would just bloat the
				// parallel batch and inflate the visible `(N/total)` ratio. The
				// static `count_leaves` estimate already counted this Shell step
				// toward the total, so we decrement to match what will actually
				// run.
				if substituted.trim().is_empty() {
					counter.subtract_from_total(1);
					continue;
				}
				// Snapshot the cwd override AFTER substitution so this leaf
				// captures whatever cwd a `{{ set_cwd(...) }}` in `template`
				// (or a prior leaf's template) just wrote. Without this snapshot,
				// every leaf would race on the shared override at spawn time
				// and only the last writer's value would apply.
				let cwd_snapshot = args.snapshot_cwd_override();
				out.push(ParallelLeaf::Shell {
					template: template.clone(),
					substituted,
					when: partition,
					cwd_snapshot,
				});
			}
			CommandStep::TargetCall(call) => {
				// Substitute the target name so `@{{ VAR.ns }}:build`-style
				// dynamic targets dispatch to the right namespaced target.
				let target = args.substitute(&call.target, &setup.env)?;
				let argv = resolve_target_call_argv(call, args, &setup.env)?;
				out.push(ParallelLeaf::TargetCall {
					target,
					argv,
					when: partition,
					optional: call.optional,
				});
			}
			CommandStep::When(when_step) => {
				collect_leaves_parallel_with_when(
					&when_step.commands,
					setup,
					args,
					working_dir,
					effective,
					counter,
					out,
				)?;
			}
			CommandStep::If(if_step) => {
				let condition_value = evaluate_if_condition(if_step, args, &setup.env)?;
				let branch: &[CommandStep] = if condition_value {
					&if_step.then
				} else {
					match &if_step.r#else {
						Some(b) => b.as_slice(),
						None => &[],
					}
				};
				collect_leaves_parallel_with_when(branch, setup, args, working_dir, effective, counter, out)?;
			}
			CommandStep::For(for_step) => {
				if for_step.parallel.unwrap_or(false) {
					eprintln!(
						"[runfile] Warning: nested `for` block has parallel: true but is already inside a parallel context — running iterations sequentially."
					);
				}
				let iterations = expand_for_iterations(for_step, args, &setup.env, working_dir)?;
				let guard = LoopVarGuard::enter(&args.vars, for_step.var.as_str());
				for value in iterations {
					guard.set(value);
					collect_leaves_parallel_with_when(
						&for_step.body,
						setup,
						args,
						working_dir,
						effective,
						counter,
						out,
					)?;
				}
				drop(guard);
			}
			CommandStep::Match(match_step) => {
				// Like `if`, pick the chosen branch eagerly using the current
				// substitution context and only collect that branch's leaves.
				let branch = resolve_match_branch(match_step, args, &setup.env)?;
				collect_leaves_parallel_with_when(branch, setup, args, working_dir, effective, counter, out)?;
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
pub(crate) fn run_parallel_leaves(
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
	// Capture the success-batch result instead of propagating it immediately.
	// A failing shell leaf makes `run_parallel_batch` return `Err`, but the
	// `when: failure` / `when: always` partitions below must still run — the
	// whole point of a failure/always cleanup block. The batch error (if any)
	// is re-propagated at the very end, AFTER the cleanup partitions have run,
	// so the target still fails while the cleanup is honored (matching the
	// sequential walker's behavior).
	let batch_result = if !success_leaves.is_empty() {
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
		)
	} else {
		Ok(())
	};

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

	// Re-propagate the success-batch error now that the failure/always cleanup
	// partitions have run.
	batch_result
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
				template,
				substituted,
				cwd_snapshot,
				..
			} => {
				if setup.logging {
					log_command(&template, step, total);
				}
				let shell_args = shell.exec_args(&substituted);
				let mut cmd = Command::new(&shell.path);
				let spawn_cwd = RunArgs::spawn_cwd_from_snapshot(cwd_snapshot.as_deref(), working_dir);
				cmd.args(&shell_args).envs(&setup.env).current_dir(&spawn_cwd);
				// forceKillOnSigInt: new process group before spawn (see force_kill).
				if let Some(guard) = force_kill_guard {
					guard.prepare_command(&mut cmd);
				}
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
				} => format_target_call_label(target, argv, *optional),
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
	let mut shells: Vec<(String, String, Option<String>, Option<PathBuf>)> = Vec::new();
	let mut target_calls: Vec<(String, Vec<String>, bool, Option<String>)> = Vec::new();
	// Pre-computed labels for each target call, parallel to `target_calls`.
	// Captured before the worker threads consume the call data so we can still
	// build the failure summary after the fact (e.g. `@web-user:build:infrastructure`).
	let mut target_labels: Vec<String> = Vec::new();
	for (leaf, &(step, _total)) in leaves.into_iter().zip(step_pairs.iter()) {
		// The bracket label reflects what's running: a resolved `@target` call
		// (shown in full) or a raw shell command (truncated to 12 chars).
		let label = match &leaf {
			ParallelLeaf::Shell { substituted, .. } => shell_prefix_label(substituted),
			ParallelLeaf::TargetCall {
				target, argv, optional, ..
			} => format_target_call_label(target, argv, *optional),
		};
		let leaf_prefix = if !prefix_output {
			None
		} else if let Some(parent) = setup.output_prefix.as_deref() {
			Some(parent.to_string())
		} else {
			Some(format_parallel_prefix(step, &label))
		};
		match leaf {
			ParallelLeaf::Shell {
				template,
				substituted,
				cwd_snapshot,
				..
			} => shells.push((template, substituted, leaf_prefix, cwd_snapshot)),
			ParallelLeaf::TargetCall {
				target, argv, optional, ..
			} => {
				target_labels.push(format_target_call_label(&target, &argv, optional));
				target_calls.push((target, argv, optional, leaf_prefix));
			}
		}
	}

	#[allow(unused_mut)]
	let mut tree_tracker = ProcessTreeTracker::new();

	let mut children: Vec<(String, Child, Vec<JoinHandle<()>>)> = Vec::with_capacity(shells.len());
	for (template, substituted, leaf_prefix, cwd_snapshot) in &shells {
		let shell_args = shell.exec_args(substituted);
		let mut cmd = Command::new(&shell.path);
		// Per-leaf cwd: each parallel leaf captured the override at the moment
		// of its substitution so siblings can't race on the shared mutex.
		let spawn_cwd = RunArgs::spawn_cwd_from_snapshot(cwd_snapshot.as_deref(), working_dir);
		cmd.args(&shell_args).envs(&setup.env).current_dir(&spawn_cwd);

		if leaf_prefix.is_some() {
			cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
		}

		#[cfg(unix)]
		tree_tracker.prepare_command(&mut cmd);
		// forceKillOnSigInt: new process group before spawn (see force_kill).
		if let Some(guard) = force_kill_guard {
			guard.prepare_command(&mut cmd);
		}

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
	// `(label, detail)` for every leaf that failed in this batch. Printed
	// at the end so the user can clearly see which parallel branch broke
	// and with what exit code, even when interleaved output buried it.
	let mut failure_summary: Vec<(String, String)> = Vec::new();
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
					let detail = match status.code() {
						Some(code) => format!("exit code {code}"),
						None => "terminated by signal".to_string(),
					};
					failure_summary.push((template.clone(), detail));
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
				failure_summary.push((template.clone(), format!("wait failed: {e}")));
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

	for (label, result) in target_labels.into_iter().zip(dep_results) {
		match result {
			Ok(dep_res) => {
				shells_run += dep_res.commands_run;
				failures += dep_res.failures;
				last_status = Some(dep_res.final_status).or(last_status);
				if let Some(detail) = dep_result_failure_detail(&dep_res) {
					failure_summary.push((label, detail));
				}
			}
			Err(e) => {
				failures += 1;
				failure_summary.push((label, execute_error_failure_detail(&e)));
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

	// Surface a summary of every leaf that failed in this batch — even when
	// the outer error path is going to bubble up `first_error`, the parallel
	// log lines and interleaved output can make it hard to tell which branch
	// broke. Printed regardless of `ignore_errors` because that flag silences
	// the propagated error, not the diagnostic.
	if !failure_summary.is_empty() {
		log_parallel_failure_summary(&failure_summary);
	}

	if let Some(err) = wait_error {
		return Err(ExecuteError::Spawn(err));
	}
	if let Some(err) = first_error {
		return Err(err);
	}
	Ok(())
}
