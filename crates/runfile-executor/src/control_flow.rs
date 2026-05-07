//! Runtime support for `if` / `for` blocks inside the `commands` array.
//!
//! - [`evaluate`] turns a parsed [`DslExpr`] into a boolean against the
//!   current substitution context (CLI args, env, loop scope).
//! - [`expand_for_iterations`] computes the iteration values for a `for`
//!   block (in-array / glob / shell capture). Side-effecting work (running
//!   the `shell` iterator) happens here.
//! - [`count_leaves`] recursively counts leaf shell commands for the global
//!   step counter. `if` inflates by both branches; `for in` multiplies by
//!   the literal length; `for glob` / `for shell` start with a
//!   1-iteration estimate (the runtime adjusts the total dynamically as
//!   iterators are expanded).

use crate::args::{LoopScope, RunArgs, SubstitutionError};
use globset::{Glob, GlobSetBuilder};
use runfile_parser::{
	walk_step_templates, CommandStep, DslExpr, DslValue, ForStep, IfStep, MatchStep, WhenCondition, WhenStep,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControlFlowError {
	#[error("{0}")]
	Substitution(#[from] SubstitutionError),

	#[error("Internal error: `if` condition was not pre-parsed (this is a bug)")]
	UnparsedCondition,

	#[error("Failed to expand `for glob` pattern \"{0}\": {1}")]
	BadGlob(String, String),

	#[error("`for shell` iterator failed: command \"{0}\" exited with status {1}")]
	ShellIteratorFailed(String, i32),

	#[error("Failed to spawn `for shell` iterator command: {0}")]
	ShellIteratorSpawn(#[from] std::io::Error),

	#[error("Failed to walk filesystem for `for glob` pattern \"{0}\": {1}")]
	GlobWalk(String, String),

	#[error("Could not resolve value for `match` \"{match_expr}\": {source}\n  Valid cases: {valid_cases}")]
	MatchValueUnresolved {
		match_expr: String,
		valid_cases: String,
		#[source]
		source: SubstitutionError,
	},

	#[error("No case matched value \"{value}\" for `match` \"{match_expr}\"\n  Valid cases: {valid_cases}")]
	MatchNoCase {
		match_expr: String,
		value: String,
		valid_cases: String,
	},
}

/// Resolve a [`DslValue`] to a string against the current substitution context.
/// `Substitution` payloads are run through the existing substitutor, so all
/// chained-fallback semantics work inside conditions just like in commands.
fn resolve_value(
	value: &DslValue,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
) -> Result<String, SubstitutionError> {
	match value {
		DslValue::Substitution(raw) => args.substitute_with_loop(raw, env, loop_scope),
		DslValue::Literal(s) => Ok(s.clone()),
	}
}

/// Truthiness rule: only the empty string is falsy. Every other string is
/// truthy. This matches the values that raw shell commands see when they
/// receive a `$(...)` substitution. In particular, `$(FLAGS.x)` resolves
/// to either `"true"` or `"false"` — both non-empty — so users must compare
/// flags explicitly with `== true` / `== false`.
fn is_truthy(s: &str) -> bool {
	!s.is_empty()
}

/// Evaluate a parsed condition AST against the current substitution context.
pub fn evaluate(
	expr: &DslExpr,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
) -> Result<bool, ControlFlowError> {
	match expr {
		DslExpr::Truthy(v) => {
			let s = resolve_value(v, args, env, loop_scope)?;
			Ok(is_truthy(&s))
		}
		DslExpr::Equality(l, r) => {
			let lhs = resolve_value(l, args, env, loop_scope)?;
			let rhs = resolve_value(r, args, env, loop_scope)?;
			Ok(lhs == rhs)
		}
		DslExpr::Inequality(l, r) => {
			let lhs = resolve_value(l, args, env, loop_scope)?;
			let rhs = resolve_value(r, args, env, loop_scope)?;
			Ok(lhs != rhs)
		}
		DslExpr::Not(inner) => Ok(!evaluate(inner, args, env, loop_scope)?),
		DslExpr::And(parts) => {
			for part in parts {
				if !evaluate(part, args, env, loop_scope)? {
					return Ok(false);
				}
			}
			Ok(true)
		}
		DslExpr::Or(parts) => {
			for part in parts {
				if evaluate(part, args, env, loop_scope)? {
					return Ok(true);
				}
			}
			Ok(false)
		}
	}
}

/// Pre-parse a condition that arrived without a cached AST (defensive fallback).
/// In practice `validate_runfile` always caches the AST, so this is effectively
/// dead code unless something constructs an `IfStep` programmatically.
pub fn evaluate_if_condition(
	if_step: &IfStep,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
) -> Result<bool, ControlFlowError> {
	if let Some(ast) = &if_step.condition_ast {
		evaluate(ast, args, env, loop_scope)
	} else {
		Err(ControlFlowError::UnparsedCondition)
	}
}

/// Expand a `for` block to a concrete list of iteration values, applying
/// substitution to the iterator source where appropriate.
pub fn expand_for_iterations(
	for_step: &ForStep,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
	working_dir: &Path,
) -> Result<Vec<String>, ControlFlowError> {
	use runfile_parser::ForInValue;

	match &for_step.r#in {
		Some(ForInValue::Literal(items)) => {
			// Substitute every element. Outer-loop variables ARE visible (this
			// resolves at the time the `for` block is entered).
			let mut result = Vec::with_capacity(items.len());
			for item in items {
				result.push(args.substitute_with_loop(item, env, loop_scope)?);
			}
			Ok(result)
		}
		Some(ForInValue::Namespaces) => {
			// Snapshot the merged Runfile's namespace list — no substitution,
			// no working-dir lookup. Empty when no namespaced includes are
			// configured (the body simply doesn't run).
			Ok(args.run_context.namespaces.iter().cloned().collect())
		}
		None => {
			if let Some(pattern) = &for_step.glob {
				expand_glob(pattern, args, env, loop_scope, working_dir)
			} else if let Some(cmd) = &for_step.shell {
				expand_shell(cmd, args, env, loop_scope, working_dir)
			} else {
				// Validation should already have rejected this. Defensive empty result.
				Ok(Vec::new())
			}
		}
	}
}

fn expand_glob(
	pattern: &str,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
	working_dir: &Path,
) -> Result<Vec<String>, ControlFlowError> {
	let resolved = args.substitute_with_loop(pattern, env, loop_scope)?;

	// The glob is rooted at the working directory. We walk the filesystem
	// and collect any path matching the pattern. Matches are returned as
	// paths relative to the working directory (stable across machines).
	let glob = Glob::new(&resolved).map_err(|e| ControlFlowError::BadGlob(resolved.clone(), e.to_string()))?;
	let mut builder = GlobSetBuilder::new();
	builder.add(glob);
	let set = builder
		.build()
		.map_err(|e| ControlFlowError::BadGlob(resolved.clone(), e.to_string()))?;

	let mut results = Vec::new();
	walk_dir(working_dir, working_dir, &set, &mut results)
		.map_err(|e| ControlFlowError::GlobWalk(resolved.clone(), e.to_string()))?;
	results.sort();
	Ok(results)
}

fn walk_dir(root: &Path, dir: &Path, set: &globset::GlobSet, out: &mut Vec<String>) -> std::io::Result<()> {
	let read = match std::fs::read_dir(dir) {
		Ok(r) => r,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
		Err(e) => return Err(e),
	};
	for entry in read {
		let entry = entry?;
		let path = entry.path();
		let file_type = entry.file_type()?;

		if file_type.is_dir() {
			walk_dir(root, &path, set, out)?;
			continue;
		}

		let rel = path.strip_prefix(root).unwrap_or(&path);
		let rel_str = rel.to_string_lossy().replace('\\', "/");
		if set.is_match(&rel_str) {
			out.push(rel_str);
		}
	}
	Ok(())
}

fn expand_shell(
	cmd: &str,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
	working_dir: &Path,
) -> Result<Vec<String>, ControlFlowError> {
	let resolved = args.substitute_with_loop(cmd, env, loop_scope)?;

	// We deliberately go through the platform's default shell here so users
	// can write shell pipelines (`git diff --name-only | sort`, etc.) inside
	// the iterator. We use the same shell convention runfile-shell uses for
	// command execution.
	#[cfg(windows)]
	let mut command = {
		let mut c = Command::new("cmd");
		c.arg("/C").arg(&resolved);
		c
	};
	#[cfg(not(windows))]
	let mut command = {
		let mut c = Command::new("sh");
		c.arg("-c").arg(&resolved);
		c
	};

	let output = command
		.envs(env)
		.current_dir(working_dir)
		.stdout(Stdio::piped())
		.stderr(Stdio::inherit())
		.output()?;

	if !output.status.success() {
		let code = output.status.code().unwrap_or(-1);
		return Err(ControlFlowError::ShellIteratorFailed(resolved, code));
	}

	let stdout = String::from_utf8_lossy(&output.stdout);
	let lines: Vec<String> = stdout
		.lines()
		.map(|l| l.trim().to_string())
		.filter(|l| !l.is_empty())
		.collect();
	Ok(lines)
}

/// Format the keys of a match step's `cases` as a comma-separated list.
/// Used in error messages so the user knows what the valid values are.
/// Cases are stored in a `BTreeMap`, so ordering is alphabetical and stable.
fn format_match_cases(step: &MatchStep) -> String {
	if step.cases.is_empty() {
		return "(none)".to_string();
	}
	step.cases
		.keys()
		.map(|k| format!("\"{k}\""))
		.collect::<Vec<_>>()
		.join(", ")
}

/// Resolve which branch of a `match` step to execute.
///
/// 1. Substitute the `match` template against the current substitution
///    context. Substitution failure is surfaced as
///    [`ControlFlowError::MatchValueUnresolved`] with the list of valid
///    cases attached so the user can fix the missing value or correct the
///    substitution chain.
/// 2. Look up the resolved value in `cases`. A match runs the case body.
/// 3. Otherwise, run `default` (if set), or surface
///    [`ControlFlowError::MatchNoCase`].
///
/// Returns `Some(branch)` when a branch was chosen (case match or default),
/// or `None` when the step has nothing to do (no case matched and no
/// default set — in which case the caller already produced the error).
pub fn resolve_match_branch<'a>(
	step: &'a MatchStep,
	args: &RunArgs,
	env: &HashMap<String, String>,
	loop_scope: &LoopScope,
) -> Result<&'a [CommandStep], ControlFlowError> {
	let resolved = match args.substitute_with_loop(&step.r#match, env, loop_scope) {
		Ok(v) => v,
		Err(e) => {
			// Substitution failed — fall through to default if present so
			// users can write a `default` branch that handles the missing
			// case explicitly. Otherwise surface a richer error that lists
			// the valid cases.
			if let Some(default) = step.default.as_deref() {
				return Ok(default);
			}
			return Err(ControlFlowError::MatchValueUnresolved {
				match_expr: step.r#match.clone(),
				valid_cases: format_match_cases(step),
				source: e,
			});
		}
	};

	if let Some(branch) = step.cases.get(&resolved) {
		return Ok(branch.as_slice());
	}
	if let Some(default) = step.default.as_deref() {
		return Ok(default);
	}
	Err(ControlFlowError::MatchNoCase {
		match_expr: step.r#match.clone(),
		value: resolved,
		valid_cases: format_match_cases(step),
	})
}

/// Static count of leaf shell commands inside a slice of [`CommandStep`]s.
///
/// - `Shell` → 1
/// - `If` → `then.len() + else.len()` (recursive). Both branches inflate
///   the total because we don't know which branch will execute.
/// - `For in` → `in.len() * body_count`
/// - `For glob` / `For shell` → `body_count` (1-iteration estimate; the
///   runtime calls [`crate::logging::StepCounter::add_to_total`] to bump
///   the total dynamically when actual iterations exceed the estimate).
/// - `Match` → `sum(case.len()) + default.len()` (worst case — only one
///   branch runs but we don't know which).
pub fn count_leaves(steps: &[CommandStep]) -> usize {
	let mut total = 0;
	for step in steps {
		total += count_leaves_one(step);
	}
	total
}

fn count_leaves_one(step: &CommandStep) -> usize {
	match step {
		CommandStep::Shell(_) => 1,
		// A target invocation is *one* counter slot from the parent's POV.
		// The runner tops up the counter total at runtime by adding the
		// dependency's own static leaf count via `StepCounter::add_to_total`,
		// so `(N/total)` stays accurate without the parser needing access
		// to the full Runfile.
		CommandStep::TargetCall(_) => 1,
		// A `when`-guarded block contributes the inner block's leaf count —
		// not all branches will run, but we count every step that *might*
		// execute (the same trade-off as `if`'s both-branches counting).
		CommandStep::When(WhenStep { commands, .. }) => count_leaves(commands),
		CommandStep::If(IfStep { then, r#else, .. }) => {
			let mut t = count_leaves(then);
			if let Some(else_steps) = r#else {
				t += count_leaves(else_steps);
			}
			t
		}
		CommandStep::For(ForStep { r#in, body, .. }) => {
			let body_count = count_leaves(body);
			match r#in {
				Some(runfile_parser::ForInValue::Literal(items)) => items.len() * body_count,
				// `Namespaces` resolves against the merged Runfile at runtime,
				// which this counter doesn't see — fall back to the same
				// 1-iteration estimate used for glob / shell. The runner's
				// `count_target_leaves` (which DOES see the Runfile) gives an
				// accurate count and `StepCounter::add_to_total` bumps the
				// shared total at runtime if more iterations actually expand.
				Some(runfile_parser::ForInValue::Namespaces) | None => body_count,
			}
		}
		CommandStep::Match(MatchStep { cases, default, .. }) => {
			let mut total = 0;
			for case in cases.values() {
				total += count_leaves(case);
			}
			if let Some(default_steps) = default {
				total += count_leaves(default_steps);
			}
			total
		}
	}
}

/// Walk the leaf templates of every step, calling `visit` on each. Convenience
/// re-export at the executor crate level (the parser crate already exposes
/// [`runfile_parser::walk_step_templates`]).
pub fn walk_templates<F: FnMut(&str)>(steps: &[CommandStep], mut visit: F) {
	walk_step_templates(steps, &mut |t| visit(t));
}

/// Convert a relative path against the working directory. Public so callers
/// outside this module can reuse the same conversion when expanding globs
/// for iteration.
pub fn make_absolute(p: &str, working_dir: &Path) -> PathBuf {
	let pb = PathBuf::from(p);
	if pb.is_absolute() {
		pb
	} else {
		working_dir.join(pb)
	}
}

/// Errors that can surface while flattening a target's `commands` for
/// detached spawning.
#[derive(Debug, Error)]
pub enum DetachFlattenError {
	#[error("{0}")]
	Substitution(#[from] SubstitutionError),

	#[error("{0}")]
	ControlFlow(#[from] ControlFlowError),

	#[error(
		"`@target` invocations are not supported inside `detach: true` targets — \
		 detach is meant for fire-and-forget shell commands. Found `@{0}` in detached target."
	)]
	TargetCallNotAllowed(String),
}

/// Walk a `commands` array as if executing it, but only collect the resulting
/// shell strings (with substitution applied) into a flat `Vec<String>`. Used
/// by detached targets where we want to spawn each leaf as an independent
/// background process.
///
/// Differences from regular execution:
/// - `if` blocks are evaluated and only the chosen branch is collected.
/// - `for` blocks are expanded; every iteration of the body is collected.
/// - `when: success` (default) and `when: always` blocks are flattened
///   (their inner steps are included). `when: failure` blocks are skipped
///   — there is no runtime "failed" tracking in detached mode, so a
///   failure-only block can never run.
/// - `@target` invocations are rejected: they can't be meaningfully
///   detached without recursing into another target's lifecycle.
pub fn collect_detach_leaves(
	steps: &[CommandStep],
	args: &RunArgs,
	env: &HashMap<String, String>,
	working_dir: &Path,
) -> Result<Vec<String>, DetachFlattenError> {
	let mut out: Vec<String> = Vec::new();
	let mut loop_scope = LoopScope::new();
	collect_detach_leaves_inner(steps, args, env, working_dir, &mut loop_scope, &mut out)?;
	Ok(out)
}

fn collect_detach_leaves_inner(
	steps: &[CommandStep],
	args: &RunArgs,
	env: &HashMap<String, String>,
	working_dir: &Path,
	loop_scope: &mut LoopScope,
	out: &mut Vec<String>,
) -> Result<(), DetachFlattenError> {
	for step in steps {
		// Skip blocks that are gated on a failure that can never occur in detach.
		if step.effective_when() == WhenCondition::Failure {
			continue;
		}

		match step {
			CommandStep::Shell(template) => {
				let substituted = args.substitute_with_loop(template, env, loop_scope)?;
				out.push(substituted);
			}
			CommandStep::TargetCall(call) => {
				return Err(DetachFlattenError::TargetCallNotAllowed(call.target.clone()));
			}
			CommandStep::When(WhenStep { commands, .. }) => {
				collect_detach_leaves_inner(commands, args, env, working_dir, loop_scope, out)?;
			}
			CommandStep::If(if_step) => {
				let cond = evaluate_if_condition(if_step, args, env, loop_scope)?;
				let branch: &[CommandStep] = if cond {
					&if_step.then
				} else {
					if_step.r#else.as_deref().unwrap_or(&[])
				};
				collect_detach_leaves_inner(branch, args, env, working_dir, loop_scope, out)?;
			}
			CommandStep::For(for_step) => {
				let iterations = expand_for_iterations(for_step, args, env, loop_scope, working_dir)?;
				for value in iterations {
					loop_scope.push(&for_step.var, value);
					let r = collect_detach_leaves_inner(&for_step.body, args, env, working_dir, loop_scope, out);
					loop_scope.pop();
					r?;
				}
			}
			CommandStep::Match(match_step) => {
				// Same approach as `if`: pick the matching branch using the
				// current substitution context and only collect that one.
				let branch = resolve_match_branch(match_step, args, env, loop_scope)?;
				collect_detach_leaves_inner(branch, args, env, working_dir, loop_scope, out)?;
			}
		}
	}
	Ok(())
}
