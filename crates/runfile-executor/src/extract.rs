use crate::args::{check_env_case_duplicates, validate_args, LoopVarGuard, RunArgs, SubstitutionError};
use crate::control_flow::{
	collect_shell_only_leaves, evaluate_if_condition, expand_glob, resolve_match_branch, ControlFlowError,
	ShellLeafContext, ShellLeafFlattenError,
};
use crate::env::{build_env_with_base, EnvFileError};
use crate::executor::join_shell_commands;
use runfile_parser::{
	walk_spec_aux_templates, walk_step_templates, CommandStep, ForStep, Runfile, WhenStep, WORKING_DIRECTORY_DEFAULT,
};
use runfile_shell::ShellKind;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
	#[error("Dependency cycle detected: {0}")]
	CycleDetected(String),

	#[error("Unknown target \"{0}\" referenced via @target")]
	UnknownTarget(String),

	#[error("{0}")]
	Substitution(#[from] SubstitutionError),

	#[error("{0}")]
	EnvFile(#[from] EnvFileError),

	#[error("{0}")]
	ControlFlow(#[from] ControlFlowError),

	#[error("{0}")]
	ShellLeafFlatten(#[from] ShellLeafFlattenError),
}

/// A single extracted command line, ready to be printed.
#[derive(Debug, Clone)]
pub struct ExtractedCommand {
	/// The command string with env vars already substituted.
	pub command: String,
	/// Non-system env vars that should be set for this command.
	pub env_vars: Vec<(String, String)>,
}

/// Extract all commands that would be executed for a target, including dependencies.
/// Returns the commands in execution order, with env vars inlined.
///
/// Defaults the shell-kind for `sameShell` joining to [`ShellKind::Bash`]. Use
/// [`extract_target_with_cwd`] when you need cross-shell-accurate joining.
pub fn extract_target(
	target_name: &str,
	runfile: &Runfile,
	args: &RunArgs,
	working_dir: &Path,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	let synthetic_path = working_dir.join(runfile_parser::RUNFILE_NAME);
	extract_target_with_cwd(
		target_name,
		runfile,
		args,
		&synthetic_path,
		working_dir,
		working_dir,
		&HashMap::new(),
		&HashMap::new(),
		None,
		&ShellKind::Bash,
	)
}

/// Extract all commands for a target with separate runfile dir and caller CWD.
/// `source_dirs` maps target names to their source Runfile's parent directory;
/// `source_files` maps target names to the source Runfile *path* (used to
/// populate `{{ RUN.file }}` per-target during substitution).
///
/// `@target` invocations are recursively expanded — the dep's resolved leaf
/// shell commands appear inline at the call site (with the dep's own env
/// block reflected on each command). This mirrors what the runtime would
/// actually execute, so the output matches a real run instead of leaving
/// aggregator targets (whose body is just `@target` dispatches) printing
/// nothing. Cycles are detected via per-call-stack tracking; calling the
/// same target twice from sibling sites expands twice (matching runtime
/// no-dedup semantics).
#[allow(clippy::too_many_arguments)]
pub fn extract_target_with_cwd<'a>(
	target_name: &str,
	runfile: &'a Runfile,
	args: &RunArgs,
	runfile_path: &'a Path,
	runfile_dir: &'a Path,
	caller_cwd: &'a Path,
	source_dirs: &'a HashMap<String, PathBuf>,
	source_files: &'a HashMap<String, PathBuf>,
	available_private_keys: Option<&'a [String]>,
	shell_kind: &'a ShellKind,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	let all_commands = collect_all_extract_commands(target_name, runfile)?;
	validate_args(args, &all_commands)?;

	// Sync `run_context.namespaces` (and the static cwd/file/parent baselines)
	// from the merged Runfile if the caller didn't already attach matching
	// values. The runner does this via its own `ensure_run_context` before
	// dispatch; we mirror it here so `for in: "namespaces"` and `{{ RUN.* }}`
	// resolve identically in dry-run and real execution. Only allocate when
	// out of sync. Per-target `RUN.file`/`RUN.parent` overrides happen later
	// inside `extract_recursive_inner`.
	let cwd_str = caller_cwd.to_string_lossy();
	let file_str = runfile_path.to_string_lossy();
	let parent_str = runfile_dir.to_string_lossy();
	let needs_clone = args.run_context.namespaces.as_slice() != runfile.namespaces.as_slice()
		|| args.run_context.cwd != cwd_str
		|| args.run_context.file != file_str
		|| args.run_context.parent != parent_str;
	let synced_args;
	let args = if needs_clone {
		let mut owned = args.clone();
		owned.run_context.namespaces = Arc::new(runfile.namespaces.clone());
		owned.run_context.cwd = cwd_str.into_owned();
		owned.run_context.file = file_str.into_owned();
		owned.run_context.parent = parent_str.into_owned();
		synced_args = owned;
		&synced_args
	} else {
		args
	};

	let mut ctx = ExtractContext {
		runfile,
		runfile_path,
		runfile_dir,
		source_dirs,
		source_files,
		available_private_keys,
		shell_kind,
		in_progress: HashSet::new(),
	};
	extract_recursive(&mut ctx, target_name, args, None)
}

struct ExtractContext<'a> {
	runfile: &'a Runfile,
	runfile_path: &'a Path,
	runfile_dir: &'a Path,
	source_dirs: &'a HashMap<String, PathBuf>,
	source_files: &'a HashMap<String, PathBuf>,
	available_private_keys: Option<&'a [String]>,
	/// Shell kind used to pick the correct sequencing operator (`&&` / `;` /
	/// `&`) when joining `sameShell: true` targets into a single command line.
	/// Doesn't affect substitution; only the join separator.
	shell_kind: &'a ShellKind,
	in_progress: HashSet<String>,
}

impl ExtractContext<'_> {
	fn target_dir(&self, target_name: &str) -> &Path {
		self.source_dirs
			.get(target_name)
			.map(|p| p.as_path())
			.unwrap_or(self.runfile_dir)
	}

	fn target_file(&self, target_name: &str) -> &Path {
		self.source_files
			.get(target_name)
			.map(|p| p.as_path())
			.unwrap_or(self.runfile_path)
	}
}

/// Runtime preview, with one narrow static-analysis fallback for the
/// only iterator source we can't resolve without side effects:
/// - `if` blocks evaluate the condition against the same context the runner
///   would see (args + resolved env + loop scope) and emit only the matching
///   branch.
/// - `for in` blocks expand each literal iteration with `{{ LOOP.var }}` resolved
///   (and `for in: "namespaces"` snapshots the merged Runfile's namespace list).
/// - `for glob` blocks expand against the filesystem at extract time — the
///   walker is read-only, so previewing iteration values is safe and lets
///   users see the actual command list a real run would produce.
/// - `for shell` blocks emit the body once with the loop variable bound to
///   a `<var>` placeholder, since running the iterator command would have
///   side effects (process spawn, possibly mutating state).
/// - `@target` calls recurse, inheriting the parent's resolved env as their
///   substitution base.
fn extract_recursive(
	ctx: &mut ExtractContext<'_>,
	target_name: &str,
	args: &RunArgs,
	parent_env: Option<&HashMap<String, String>>,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	// Per-call-stack cycle tracking. We add to `in_progress` on entry and
	// remove on exit (success or error) — sibling calls to the same target
	// must succeed, only ancestor calls indicate a true cycle.
	if !ctx.in_progress.insert(target_name.to_string()) {
		return Err(ExtractError::CycleDetected(target_name.to_string()));
	}
	let result = extract_recursive_inner(ctx, target_name, args, parent_env);
	ctx.in_progress.remove(target_name);
	result
}

fn extract_recursive_inner(
	ctx: &mut ExtractContext<'_>,
	target_name: &str,
	args: &RunArgs,
	parent_env: Option<&HashMap<String, String>>,
) -> Result<Vec<ExtractedCommand>, ExtractError> {
	let spec = ctx
		.runfile
		.targets
		.get(target_name)
		.ok_or_else(|| ExtractError::UnknownTarget(target_name.to_string()))?;

	let target_runfile_dir = ctx.target_dir(target_name);
	let target_runfile_file = ctx.target_file(target_name);

	// Refresh `RUN.file` / `RUN.parent` to reflect *this* target's source
	// Runfile. `RUN.cwd` was already set by the top-level `extract_target_with_cwd`
	// and stays constant. We allocate only when at least one field changed
	// (e.g. when entering an included target whose source differs).
	let file_str = target_runfile_file.to_string_lossy();
	let parent_str = target_runfile_dir.to_string_lossy();
	let needs_clone = args.run_context.file != file_str || args.run_context.parent != parent_str;
	let synced_args;
	let args = if needs_clone {
		let mut owned = args.clone();
		owned.run_context.file = file_str.into_owned();
		owned.run_context.parent = parent_str.into_owned();
		synced_args = owned;
		&synced_args
	} else {
		args
	};

	// `workingDirectory` is a free-form path supporting `{{ ... }}` substitution;
	// default is `{{ RUN.parent }}`. We substitute against the parent_env (env
	// files aren't loaded yet — we need the working dir to load them) and
	// resolve relative paths against the target's source dir.
	let pre_env: HashMap<String, String> = parent_env.cloned().unwrap_or_default();
	let working_directory_template = spec.working_directory.as_deref().unwrap_or(WORKING_DIRECTORY_DEFAULT);
	let resolved_working_directory = args.substitute(working_directory_template, &pre_env)?;
	let effective_working_dir_owned = resolve_working_directory_path(&resolved_working_directory, target_runfile_dir);
	let effective_working_dir: &Path = effective_working_dir_owned.as_path();

	let env = build_env_with_base(
		spec,
		effective_working_dir,
		target_runfile_dir,
		args,
		ctx.available_private_keys,
		parent_env,
		None,
	)?;
	check_env_case_duplicates(&env)?;

	// Show only the spec-defined env keys (not envFiles or system env), but
	// pull the resolved values from the fully-built env so `{{ FLAGS.x }}`,
	// `{{ ARGS.x }}`, `{{ ENV.x }}`, etc. references are substituted instead of
	// printed literally.
	let extra_env: Vec<(String, String)> = if let Some(spec_env) = &spec.env {
		let mut pairs: Vec<(String, String)> = spec_env
			.keys()
			.filter_map(|k| env.get(k).map(|v| (k.clone(), v.clone())))
			.collect();
		pairs.sort_by(|a, b| a.0.cmp(&b.0));
		pairs
	} else {
		Vec::new()
	};

	// `sameShell: true`: take the same flatten path the runtime executor uses
	// (`collect_shell_only_leaves`) so the dry-run output exactly matches what
	// would actually execute — one shell invocation per target. `@target`
	// invocations inside the body surface as `ShellLeafFlattenError` here too,
	// matching the runtime contract.
	if spec.same_shell.unwrap_or(false) {
		let leaves = collect_shell_only_leaves(
			&spec.commands,
			args,
			&env,
			effective_working_dir,
			ShellLeafContext::SameShell,
		)?;
		let filtered: Vec<String> = leaves.into_iter().filter(|l| !l.trim().is_empty()).collect();
		if filtered.is_empty() {
			return Ok(Vec::new());
		}
		let ignore_errors = spec.ignore_errors.unwrap_or(false);
		let joined = join_shell_commands(&filtered, ctx.shell_kind, ignore_errors);
		return Ok(vec![ExtractedCommand {
			command: joined,
			env_vars: extra_env,
		}]);
	}

	let mut out: Vec<ExtractedCommand> = Vec::new();
	walk_extract_steps(
		ctx,
		&spec.commands,
		args,
		&env,
		&extra_env,
		effective_working_dir,
		&mut out,
	)?;

	Ok(out)
}

/// Resolve a substituted `workingDirectory` value to an absolute path.
/// Absolute paths pass through; relative paths are joined onto the
/// target's source Runfile directory (`base_dir`).
fn resolve_working_directory_path(value: &str, base_dir: &Path) -> PathBuf {
	let p = Path::new(value);
	if p.is_absolute() {
		p.to_path_buf()
	} else {
		base_dir.join(p)
	}
}

/// Recursive walker that produces extract output with VARS-based loop scope.
///
/// Iterator source templates (the `in` array elements, `glob` pattern, `shell`
/// command) are NOT emitted as commands — they're metadata. `@target` calls
/// recurse into the dep, inheriting the parent's resolved env as the dep's
/// substitution base; the dep's own env block surfaces as inline assignments
/// on its commands. `for` blocks scope their iteration variable into VARS
/// via [`LoopVarGuard`] (save prior, set per iteration, restore on exit).
///
/// `working_dir` is the target's resolved `workingDirectory` and is forwarded
/// to [`expand_glob`] for `for glob:` previews so matches resolve against the
/// same root the runner would see.
fn walk_extract_steps(
	ctx: &mut ExtractContext<'_>,
	steps: &[CommandStep],
	args: &RunArgs,
	env: &HashMap<String, String>,
	extra_env: &[(String, String)],
	working_dir: &Path,
	out: &mut Vec<ExtractedCommand>,
) -> Result<(), ExtractError> {
	for step in steps {
		match step {
			CommandStep::Shell(template) => {
				let cmd = args.substitute(template, env)?;
				// Match runtime behaviour: a command line that resolves to
				// pure whitespace (e.g. one consisting only of a
				// `{{ define(...) }}` call) is not dispatched to the shell.
				// `define` has already mutated `args.vars` during the
				// `substitute` call above, so dropping the line here matches
				// what a real `--dry-run` should print.
				if cmd.trim().is_empty() {
					continue;
				}
				out.push(ExtractedCommand {
					command: cmd,
					env_vars: extra_env.to_vec(),
				});
			}
			CommandStep::TargetCall(call) => {
				// Substitute the target name first so dynamic patterns like
				// `@{{ VARS.ns }}:dev` resolve to the namespace's concrete target.
				let resolved = args.substitute(&call.target, env)?;

				let canonical = match ctx.runfile.resolve_target(&resolved) {
					Some(n) => n.to_string(),
					None if call.optional => continue,
					None => return Err(ExtractError::UnknownTarget(resolved)),
				};

				// Build the dep's args from the substituted+shlex-split args
				// template — same semantics as the runtime executor in
				// `resolve_target_call_argv`. Preserve the parent's run_context
				// (OS, shell, namespaces) so `{{ RUN.* }}` and `for in: namespaces`
				// keep working inside the dep.
				let argv = if call.args_template.is_empty() {
					Vec::new()
				} else {
					let substituted = args.substitute(&call.args_template, env)?;
					let trimmed = substituted.trim();
					if trimmed.is_empty() {
						Vec::new()
					} else {
						shlex::split(trimmed).unwrap_or_default()
					}
				};
				let child_args = RunArgs::parse(&argv)
					.with_run_context(args.run_context.clone())
					.with_vars(args.vars.clone());

				// Dep gets the parent's resolved env as substitution base
				// (matches runtime `@target` env inheritance).
				let dep_cmds = extract_recursive(ctx, &canonical, &child_args, Some(env))?;
				out.extend(dep_cmds);
			}
			CommandStep::When(WhenStep { commands, .. }) => {
				walk_extract_steps(ctx, commands, args, env, extra_env, working_dir, out)?;
			}
			CommandStep::If(if_step) => {
				// Evaluate the condition against the same context the runner
				// would see (args + resolved env + VARS). Match the runtime
				// branch instead of emitting both branches — output then
				// matches what would actually execute.
				let cond = evaluate_if_condition(if_step, args, env)?;
				let branch: &[CommandStep] = if cond {
					&if_step.then
				} else {
					if_step.r#else.as_deref().unwrap_or(&[])
				};
				walk_extract_steps(ctx, branch, args, env, extra_env, working_dir, out)?;
			}
			CommandStep::For(for_step) => {
				use runfile_parser::ForInValue;
				let ForStep {
					var, r#in, body, glob, ..
				} = for_step;
				let guard = LoopVarGuard::enter(&args.vars, var.as_str());
				match r#in {
					Some(ForInValue::Literal(items)) => {
						for item in items {
							let value = args.substitute(item, env)?;
							guard.set(value);
							walk_extract_steps(ctx, body, args, env, extra_env, working_dir, out)?;
						}
					}
					Some(ForInValue::Namespaces) => {
						// Snapshot the merged Runfile's namespace list — extract
						// expands these to concrete commands the same way as a
						// literal array, so callers see e.g. `@project-1:build`
						// rather than a placeholder.
						let namespaces = args.run_context.namespaces.clone();
						for ns in namespaces.iter() {
							guard.set(ns.clone());
							walk_extract_steps(ctx, body, args, env, extra_env, working_dir, out)?;
						}
					}
					None => {
						if let Some(pattern) = glob {
							// `for glob:` — read-only filesystem walk, safe to
							// run during extract. Reuses the runner's
							// [`expand_glob`] so dry-run iteration order and
							// path normalisation match a real run exactly. Empty
							// match set means the body emits zero commands —
							// same as runtime behaviour.
							let matches = expand_glob(pattern, args, env, working_dir)?;
							for m in matches {
								guard.set(m);
								walk_extract_steps(ctx, body, args, env, extra_env, working_dir, out)?;
							}
						} else {
							// `for shell:` (or invalid — defensive). We deliberately
							// do NOT execute the iterator command during extract:
							// `--dry-run` is a read-only preview and shell
							// iterators can have side effects (process spawn,
							// mutated state, slow I/O). Bind a placeholder so
							// `{{ VARS.<var> }}` references inside the body still
							// resolve, and emit the body once.
							guard.set(format!("<{var}>"));
							walk_extract_steps(ctx, body, args, env, extra_env, working_dir, out)?;
						}
					}
				}
				drop(guard);
			}
			CommandStep::Match(match_step) => {
				// Same approach as `if`: dispatch using the same context the
				// runner would see, then walk only the chosen branch. Surfaces
				// `MatchValueUnresolved` / `MatchNoCase` errors at extract time
				// so dry-run output matches the runtime contract — users see
				// the same case-validation diagnostics in both modes.
				let branch = resolve_match_branch(match_step, args, env)?;
				walk_extract_steps(ctx, branch, args, env, extra_env, working_dir, out)?;
			}
		}
	}
	Ok(())
}

/// Format extracted commands as shell-native lines ready to execute.
pub fn format_extracted_commands(commands: &[ExtractedCommand], shell_kind: &ShellKind) -> Vec<String> {
	commands
		.iter()
		.map(|cmd| format_single_command(cmd, shell_kind))
		.collect()
}

fn format_single_command(cmd: &ExtractedCommand, shell_kind: &ShellKind) -> String {
	if cmd.env_vars.is_empty() {
		return cmd.command.clone();
	}

	match shell_kind {
		ShellKind::Bash | ShellKind::Zsh | ShellKind::Sh => {
			let env_prefix: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format_bash_env_assignment(k, v))
				.collect::<Vec<_>>()
				.join(" ");
			format!("{} {}", env_prefix, cmd.command)
		}
		ShellKind::Fish => {
			let env_prefix: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format!("{}={}", k, shell_quote_fish(v)))
				.collect::<Vec<_>>()
				.join(" ");
			format!("env {} {}", env_prefix, cmd.command)
		}
		ShellKind::PowerShell => {
			let env_stmts: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format!("$env:{}={}", k, shell_quote_powershell(v)))
				.collect::<Vec<_>>()
				.join("; ");
			format!("{}; {}", env_stmts, cmd.command)
		}
		ShellKind::Cmd => {
			let env_stmts: String = cmd
				.env_vars
				.iter()
				.map(|(k, v)| format!("set \"{}={}\"", k, v))
				.collect::<Vec<_>>()
				.join(" && ");
			format!("{} && {}", env_stmts, cmd.command)
		}
	}
}

fn format_bash_env_assignment(key: &str, value: &str) -> String {
	if needs_quoting(value) {
		format!("{}='{}'", key, value.replace('\'', "'\\''"))
	} else {
		format!("{}={}", key, value)
	}
}

fn shell_quote_fish(value: &str) -> String {
	if needs_quoting(value) {
		format!("'{}'", value.replace('\'', "'\\''"))
	} else {
		value.to_string()
	}
}

fn shell_quote_powershell(value: &str) -> String {
	format!("'{}'", value.replace('\'', "''"))
}

fn needs_quoting(value: &str) -> bool {
	value.is_empty() || value.chars().any(|c| " \t\n\"'\\$`!#&|;(){}[]<>?*~".contains(c))
}

fn collect_all_extract_commands(target_name: &str, runfile: &Runfile) -> Result<Vec<String>, ExtractError> {
	let mut commands = Vec::new();
	let mut completed = HashSet::new();
	let mut in_progress = HashSet::new();
	collect_extract_commands_recursive(target_name, runfile, &mut commands, &mut completed, &mut in_progress)?;
	Ok(commands)
}

fn collect_extract_commands_recursive(
	target_name: &str,
	runfile: &Runfile,
	commands: &mut Vec<String>,
	completed: &mut HashSet<String>,
	in_progress: &mut HashSet<String>,
) -> Result<(), ExtractError> {
	if completed.contains(target_name) {
		return Ok(());
	}
	if !in_progress.insert(target_name.to_string()) {
		return Err(ExtractError::CycleDetected(target_name.to_string()));
	}

	let spec = runfile
		.targets
		.get(target_name)
		.ok_or_else(|| ExtractError::UnknownTarget(target_name.to_string()))?;

	walk_step_templates(&spec.commands, &mut |t| commands.push(t.to_string()));
	walk_spec_aux_templates(spec, &mut |t| commands.push(t.to_string()));

	completed.insert(target_name.to_string());
	in_progress.remove(target_name);

	Ok(())
}
