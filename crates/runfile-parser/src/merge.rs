#![allow(clippy::result_large_err)]
//! `MergeError` transitively contains `ParseError`, which contains a `json5::Error`
//! and a `DslParseError`; the boxed error variants are intentional but Clippy flags
//! the resulting `Result<_, MergeError>` size. The merge errors only happen at
//! startup when parsing — the perf cost is irrelevant.

use crate::parse::parse_runfile_from_path_partial;
use crate::schema::{CommandSpec, CommandStep, Globals, Metadata, Runfile};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MergeError {
	#[error("Failed to parse global file {0}: {1}")]
	ParseGlobalFile(PathBuf, crate::parse::ParseError),

	#[error("No targets found — no local Runfile and no applicable global files")]
	NoTargets,

	#[error("Include cycle detected: {0}")]
	IncludeCycle(String),

	#[error("Included file not found: {0}")]
	IncludeNotFound(PathBuf),

	#[error("Included file escapes the project directory: {0}")]
	IncludePathTraversal(PathBuf),

	#[error("Failed to parse included file {0}: {1}")]
	IncludeParse(PathBuf, crate::parse::ParseError),

	#[error("Invalid include namespace \"{namespace}\" for {path}: {reason}")]
	InvalidIncludeNamespace {
		path: PathBuf,
		namespace: String,
		reason: String,
	},
}

/// Where a target originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
	/// The local Runfile.json (discovered or specified via -f/--file).
	Local,
	/// An included Runfile (via the `includes` field).
	Included,
	/// A global Runfile registered in user settings.
	Global,
}

/// Result of merging local + global Runfiles.
pub struct MergeResult {
	/// The merged Runfile with `globals: None` (globals baked into targets).
	pub runfile: Runfile,
	/// Maps target name to the parent directory of the source Runfile.
	pub source_dirs: HashMap<String, PathBuf>,
	/// Maps target name to (source file path, source kind).
	pub target_sources: HashMap<String, (PathBuf, SourceKind)>,
	/// Targets that are defined in multiple source files (not runnable).
	/// Maps target name to the list of all files that define it.
	pub conflicts: HashMap<String, Vec<(PathBuf, SourceKind)>>,
}

impl MergeResult {
	/// Extract the per-target source-file paths (without source kind). Used
	/// by the runner / extract pipeline to populate `{{ RUN.file }}` per
	/// target — equivalent info to `target_sources`, but reshaped into the
	/// `HashMap<String, PathBuf>` shape those layers consume.
	pub fn source_files(&self) -> HashMap<String, PathBuf> {
		self.target_sources
			.iter()
			.map(|(name, (path, _))| (name.clone(), path.clone()))
			.collect()
	}
}

/// Merge a local Runfile with global files.
///
/// - `local`: optional (parsed Runfile, file path) for the local Runfile.json
/// - `global_file_paths`: registered global file paths from settings
/// - `cwd`: the current working directory (used for `onlyInDirectories` filtering)
///
/// Returns a merged Runfile where each file's globals have been baked into its
/// targets. Targets defined in multiple source files are reported as conflicts
/// and excluded from the runnable targets.
pub fn merge_runfiles(
	local: Option<(Runfile, PathBuf)>,
	global_file_paths: &[PathBuf],
	cwd: &Path,
) -> Result<MergeResult, MergeError> {
	merge_runfiles_inner(local, global_file_paths, cwd, true)
}

/// Silent variant that suppresses stderr warnings (for shell completions).
pub fn merge_runfiles_silent(
	local: Option<(Runfile, PathBuf)>,
	global_file_paths: &[PathBuf],
	cwd: &Path,
) -> Result<MergeResult, MergeError> {
	merge_runfiles_inner(local, global_file_paths, cwd, false)
}

/// Accumulated state for merging multiple Runfiles.
pub(crate) struct MergeState {
	pub(crate) targets: HashMap<String, CommandSpec>,
	pub(crate) source_dirs: HashMap<String, PathBuf>,
	pub(crate) target_sources: HashMap<String, (PathBuf, SourceKind)>,
	/// Track ALL sources for each target name to detect conflicts across files.
	pub(crate) all_sources: HashMap<String, Vec<(PathBuf, SourceKind)>>,
	/// Namespace prefixes that have been applied to this state, in
	/// post-composition form (e.g. `outer:inner` after both `outer` and
	/// `inner` namespaces stacked). Each include with a non-empty namespace
	/// contributes one entry; nested includes prefix existing entries via
	/// [`apply_namespace_to_state`]. Final list is sorted + deduplicated by
	/// [`merge_runfiles_inner`] before being placed on `Runfile.namespaces`.
	pub(crate) namespaces: Vec<String>,
}

impl MergeState {
	pub(crate) fn new() -> Self {
		Self {
			targets: HashMap::new(),
			source_dirs: HashMap::new(),
			target_sources: HashMap::new(),
			all_sources: HashMap::new(),
			namespaces: Vec::new(),
		}
	}

	/// Insert targets from a parsed Runfile into the merge state.
	/// `source_path` is the file the targets came from; `source_dir` is its parent.
	/// `kind` indicates whether this is a local, included, or global source.
	/// First occurrence wins for each target name.
	fn insert_targets(
		&mut self,
		runfile_targets: HashMap<String, CommandSpec>,
		globals: Option<&Globals>,
		source_path: &Path,
		source_dir: &Path,
		kind: SourceKind,
	) {
		for (name, spec) in runfile_targets {
			self.all_sources
				.entry(name.clone())
				.or_default()
				.push((source_path.to_path_buf(), kind));

			if self.targets.contains_key(&name) {
				continue;
			}
			let baked = bake_globals_into_target(spec, globals, source_dir, &name);
			self.source_dirs.insert(name.clone(), source_dir.to_path_buf());
			self.target_sources
				.insert(name.clone(), (source_path.to_path_buf(), kind));
			self.targets.insert(name, baked);
		}
	}

	/// Fold a child sub-state into this state. Used by [`resolve_includes`]
	/// once an include's tree has been fully resolved (and namespaced if
	/// applicable). Conflict semantics mirror [`Self::insert_targets`]:
	/// `all_sources` accumulates every source for each name (driving the
	/// post-merge conflict report); the canonical maps keep the first
	/// occurrence and drop later collisions.
	fn merge_from(&mut self, other: MergeState) {
		for (name, sources) in other.all_sources {
			self.all_sources.entry(name).or_default().extend(sources);
		}
		for (name, spec) in other.targets {
			if self.targets.contains_key(&name) {
				continue;
			}
			if let Some(dir) = other.source_dirs.get(&name) {
				self.source_dirs.insert(name.clone(), dir.clone());
			}
			if let Some(src) = other.target_sources.get(&name) {
				self.target_sources.insert(name.clone(), src.clone());
			}
			self.targets.insert(name, spec);
		}
		// Namespaces accumulate across siblings; sort/dedupe happens once at
		// the top-level merge result.
		self.namespaces.extend(other.namespaces);
	}
}

fn merge_runfiles_inner(
	local: Option<(Runfile, PathBuf)>,
	global_file_paths: &[PathBuf],
	cwd: &Path,
	warn: bool,
) -> Result<MergeResult, MergeError> {
	let mut state = MergeState::new();

	// Process global files first
	for global_path in global_file_paths {
		if !global_path.is_file() {
			if warn {
				eprintln!("[runfile] Warning: global file not found: {}", global_path.display());
			}
			continue;
		}

		let global_runfile = match parse_runfile_from_path_partial(global_path) {
			Ok(r) => r,
			Err(e) => {
				if warn {
					eprintln!(
						"[runfile] Warning: failed to parse global file {}: {e}",
						global_path.display()
					);
				}
				continue;
			}
		};

		let global_dir = global_path.parent().unwrap_or(Path::new(".")).to_path_buf();

		// Check onlyInDirectories filter
		if let Some(globals) = &global_runfile.globals
			&& let Some(only_dirs) = &globals.only_in_directories
			&& !is_cwd_allowed(cwd, &global_dir, only_dirs)
		{
			continue;
		}

		let globals_ref = global_runfile.globals.as_ref();
		state.insert_targets(
			global_runfile.targets,
			globals_ref,
			global_path,
			&global_dir,
			SourceKind::Global,
		);
	}

	// Process local targets
	let schema = if let Some((local_runfile, local_path)) = local {
		let local_dir = local_path.parent().unwrap_or(Path::new(".")).to_path_buf();

		// Resolve includes from the local Runfile
		let canonical_local = std::fs::canonicalize(&local_path).unwrap_or(local_path.clone());
		let mut visited = HashSet::new();
		visited.insert(canonical_local.clone());
		resolve_includes(&local_runfile, &canonical_local, &mut state, &mut visited)?;

		let globals_ref = local_runfile.globals.as_ref();
		state.insert_targets(
			local_runfile.targets,
			globals_ref,
			&local_path,
			&local_dir,
			SourceKind::Local,
		);

		local_runfile.schema
	} else {
		"https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".to_string()
	};

	// Detect conflicts: targets defined in multiple source files
	let mut conflicts: HashMap<String, Vec<(PathBuf, SourceKind)>> = HashMap::new();
	for (name, sources) in &state.all_sources {
		if sources.len() > 1 {
			state.targets.remove(name);
			state.source_dirs.remove(name);
			state.target_sources.remove(name);
			conflicts.insert(name.clone(), sources.clone());
		}
	}

	if state.targets.is_empty() && conflicts.is_empty() {
		return Err(MergeError::NoTargets);
	}

	// Sort + dedupe namespaces so consumers (particularly `for in:
	// "namespaces"`) get deterministic order across runs and don't iterate
	// the same namespace twice when it appears under multiple include paths.
	let mut namespaces = state.namespaces;
	namespaces.sort();
	namespaces.dedup();

	Ok(MergeResult {
		runfile: Runfile {
			schema,
			includes: None,
			targets: state.targets,
			globals: None,
			namespaces,
		},
		source_dirs: state.source_dirs,
		target_sources: state.target_sources,
		conflicts,
	})
}

/// Check if `cwd` is at or under one of the allowed directories.
/// `only_dirs` paths are relative to `base_dir` (the global file's parent).
fn is_cwd_allowed(cwd: &Path, base_dir: &Path, only_dirs: &[String]) -> bool {
	for dir_str in only_dirs {
		let allowed = base_dir.join(dir_str);
		// Canonicalize both for reliable comparison
		let allowed_canon = std::fs::canonicalize(&allowed).unwrap_or(allowed);
		let cwd_canon = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
		if cwd_canon.starts_with(&allowed_canon) {
			return true;
		}
	}
	false
}

/// Bake a file's globals into a target's own fields, plus normalise any
/// path-bearing target fields whose semantics anchor to the source Runfile's
/// directory.
///
/// After baking, the target is self-contained: no globals reference, and
/// every relative path in `addToPath` is resolved against `source_dir` so
/// downstream code never has to know which file the target came from to
/// resolve it. (Globals' addToPath entries get the same treatment for the
/// same reason.) `envFiles` are NOT pre-baked here — they're substitution
/// templates and are resolved at runtime against `env_files_base_dir`
/// (= the target's source dir) inside the env builder.
fn bake_globals_into_target(
	mut spec: CommandSpec,
	globals: Option<&Globals>,
	source_dir: &Path,
	_target_name: &str,
) -> CommandSpec {
	// Always bake target's own addToPath entries to absolute. Relative paths
	// resolve against the source Runfile's directory — same anchor as
	// `{{ RUN.parent }}`, decoupled from the target's runtime
	// `workingDirectory`.
	if let Some(target_paths) = spec.add_to_path.as_mut() {
		for p in target_paths.iter_mut() {
			*p = make_path_absolute(p, source_dir);
		}
	}

	let globals = match globals {
		Some(g) => g,
		None => return spec,
	};

	// env: global as base, target overrides
	if let Some(global_env) = &globals.env {
		let mut merged = global_env.clone();
		if let Some(target_env) = spec.env.take() {
			merged.extend(target_env);
		}
		spec.env = Some(merged);
	}

	// vars: global as base, target overrides (same semantics as env)
	if let Some(global_vars) = &globals.vars {
		let mut merged = global_vars.clone();
		if let Some(target_vars) = spec.vars.take() {
			merged.extend(target_vars);
		}
		spec.vars = Some(merged);
	}

	// envFiles: prepend global (made absolute) before target
	if let Some(global_env_files) = &globals.env_files {
		let absolute_globals: Vec<String> = global_env_files
			.iter()
			.map(|p| make_path_absolute(p, source_dir))
			.collect();
		let mut merged = absolute_globals;
		if let Some(target_files) = spec.env_files.take() {
			merged.extend(target_files);
		}
		spec.env_files = Some(merged);
	}

	// addToPath: prepend global (made absolute) before target (also already
	// made absolute above against the same `source_dir`).
	if let Some(global_paths) = &globals.add_to_path {
		let absolute_globals: Vec<String> = global_paths.iter().map(|p| make_path_absolute(p, source_dir)).collect();
		let mut merged = absolute_globals;
		if let Some(target_paths) = spec.add_to_path.take() {
			merged.extend(target_paths);
		}
		spec.add_to_path = Some(merged);
	}

	// Simple fields: target wins, fall back to global
	if spec.force_shell.is_none() {
		spec.force_shell = globals.force_shell.clone();
	}
	if spec.logging.is_none() {
		spec.logging = globals.logging;
	}
	if spec.ignore_errors.is_none() {
		spec.ignore_errors = globals.ignore_errors;
	}
	if spec.working_directory.is_none() {
		spec.working_directory = globals.working_directory.clone();
	}
	if spec.force_kill_on_sig_int.is_none() {
		spec.force_kill_on_sig_int = globals.force_kill_on_sig_int;
	}
	if spec.same_shell.is_none() {
		spec.same_shell = globals.same_shell;
	}

	// onlyInDirectories: prepend global before target
	if let Some(global_dirs) = &globals.only_in_directories {
		let mut merged = global_dirs.clone();
		if let Some(target_dirs) = spec.only_in_directories.take() {
			merged.extend(target_dirs);
		}
		spec.only_in_directories = Some(merged);
	}

	// metadata: merge globals into target — target keys win.
	if let Some(global_meta) = globals.metadata.as_ref() {
		spec.metadata = Some(merge_metadata(global_meta, spec.metadata.as_ref()));
	}

	spec
}

/// Merge a target's metadata over a global metadata block. Target-defined keys
/// win over global-defined keys; missing target keys fall back to the global
/// value. The resulting [`Metadata`] is returned even when the target had no
/// own metadata (so global defaults reach the target).
pub(crate) fn merge_metadata(global: &Metadata, target: Option<&Metadata>) -> Metadata {
	let mut merged = global.clone();
	if let Some(t) = target {
		if t.exclude_from_generate_command.is_some() {
			merged.exclude_from_generate_command = t.exclude_from_generate_command;
		}
		for (k, v) in &t.extra {
			merged.extra.insert(k.clone(), v.clone());
		}
	}
	merged
}

/// Resolve includes from a parsed Runfile, merging included targets into the merge state.
///
/// `visited` is treated as a *call-stack* set: each include's canonical path
/// is inserted before recursion and removed after. This allows the same file
/// to be loaded twice via sibling include paths (a "diamond"), which is a
/// requirement for namespacing — e.g. including the same template under two
/// different namespaces. Re-entry within a single chain still errors out as a
/// cycle.
///
/// When an [`IncludeEntry`] carries a namespace, every target name, alias and
/// `@target` reference contributed by that include's tree is prefixed with
/// `<namespace>:`. Nesting composes from the innermost include outward, so a
/// child's references that already resolve to its own namespaced targets stay
/// internally consistent when an outer namespace is applied.
pub(crate) fn resolve_includes(
	runfile: &Runfile,
	runfile_path: &Path,
	state: &mut MergeState,
	visited: &mut HashSet<PathBuf>,
) -> Result<(), MergeError> {
	let includes = match &runfile.includes {
		Some(inc) if !inc.is_empty() => inc.clone(),
		_ => return Ok(()),
	};

	let base_dir = runfile_path.parent().unwrap_or(Path::new("."));

	// Compute the canonical root directory to restrict path traversal.
	// All includes must resolve to paths within this directory tree.
	let canonical_base = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.to_path_buf());

	for entry in &includes {
		let include_path = base_dir.join(&entry.path);
		let canonical =
			std::fs::canonicalize(&include_path).map_err(|_| MergeError::IncludeNotFound(include_path.clone()))?;

		if !canonical.is_file() {
			return Err(MergeError::IncludeNotFound(include_path));
		}

		// Security: reject includes that escape the Runfile's directory tree
		if !canonical.starts_with(&canonical_base) {
			return Err(MergeError::IncludePathTraversal(include_path));
		}

		// Validate namespace shape (if any) before doing any I/O work below.
		if let Some(ns) = entry.namespace.as_deref() {
			validate_namespace(ns).map_err(|reason| MergeError::InvalidIncludeNamespace {
				path: include_path.clone(),
				namespace: ns.to_string(),
				reason,
			})?;
		}

		// Cycle detection is per-chain, not per-load: insert before recursing,
		// remove after, so a sibling that re-includes a previously-loaded file
		// is not mistaken for a cycle.
		if !visited.insert(canonical.clone()) {
			return Err(MergeError::IncludeCycle(canonical.to_string_lossy().to_string()));
		}

		let result = (|| -> Result<(), MergeError> {
			let included_runfile = parse_runfile_from_path_partial(&canonical)
				.map_err(|e| MergeError::IncludeParse(canonical.clone(), e))?;

			let include_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();

			// Build a sub-state for this include's whole tree (its own targets
			// + the result of resolving its own includes). Sub-includes get
			// their own namespacing applied first; *this* include's namespace
			// (if any) is then applied to the entire sub-state, so nesting
			// composes from innermost outward.
			let mut child_state = MergeState::new();
			resolve_includes(&included_runfile, &canonical, &mut child_state, visited)?;
			let globals_ref = included_runfile.globals.as_ref();
			child_state.insert_targets(
				included_runfile.targets,
				globals_ref,
				&include_path,
				&include_dir,
				SourceKind::Included,
			);

			if let Some(ns) = entry.namespace.as_deref() {
				apply_namespace_to_state(&mut child_state, ns);
			}

			state.merge_from(child_state);
			Ok(())
		})();

		visited.remove(&canonical);
		result?;
	}

	Ok(())
}

/// Validate an include `namespace` string.
///
/// Empty strings are normalised to `None` at deserialize time, so by the time
/// this runs we have a non-empty value. Disallow characters that would break
/// composition or collide with other syntax: leading `_` (would change
/// internal-ness rules), leading `:` (collides with built-in subcommands),
/// embedded `:` (composition is via nesting, not embedded colons), leading
/// `@` (collides with target-call prefix), and any whitespace.
fn validate_namespace(ns: &str) -> Result<(), String> {
	if ns.is_empty() {
		return Err("namespace must not be empty (omit the field or use \"\" to opt out)".into());
	}
	if ns.contains(char::is_whitespace) {
		return Err("namespace must not contain whitespace".into());
	}
	if ns.contains(':') {
		return Err("namespace must not contain ':' — compose hierarchies via nested includes instead".into());
	}
	let first = ns.chars().next().unwrap();
	if first == '@' {
		return Err("namespace must not start with '@' (reserved for `@target` invocations)".into());
	}
	if first == ':' {
		return Err("namespace must not start with ':' (reserved for built-in CLI subcommands)".into());
	}
	if first == '_' {
		return Err("namespace must not start with '_' (reserved for internal-target naming)".into());
	}
	if ns.contains('?') {
		return Err("namespace must not contain '?' (reserved for the `@?target` optional-call marker)".into());
	}
	Ok(())
}

/// Apply a namespace prefix to every target in `state`: rewrites canonical
/// names, alias entries, all `@target` references inside command trees, and
/// every auxiliary lookup map keyed by target name. Used by
/// [`resolve_includes`] when the include carries a namespace.
fn apply_namespace_to_state(state: &mut MergeState, namespace: &str) {
	let prefix = |name: &str| format!("{namespace}:{name}");

	// Rewrite targets (HashMap key changes) + aliases + @target refs.
	let old_targets = std::mem::take(&mut state.targets);
	for (name, mut spec) in old_targets {
		if let Some(aliases) = spec.aliases.as_mut() {
			for alias in aliases.iter_mut() {
				*alias = prefix(alias);
			}
		}
		rewrite_target_calls_in_steps(&mut spec.commands, namespace);
		state.targets.insert(prefix(&name), spec);
	}

	state.source_dirs = std::mem::take(&mut state.source_dirs)
		.into_iter()
		.map(|(k, v)| (prefix(&k), v))
		.collect();
	state.target_sources = std::mem::take(&mut state.target_sources)
		.into_iter()
		.map(|(k, v)| (prefix(&k), v))
		.collect();
	state.all_sources = std::mem::take(&mut state.all_sources)
		.into_iter()
		.map(|(k, v)| (prefix(&k), v))
		.collect();

	// Compose namespaces: existing entries (from sub-includes) get this
	// include's namespace prepended, then this include's own namespace is
	// appended. So a chain `outer → inner → leaf` ends up tracking `outer`,
	// `outer:inner`, and `outer:inner:leaf` (any inner-leaf descendants
	// already encoded inside the sub-state retain their composition).
	state.namespaces = std::mem::take(&mut state.namespaces)
		.into_iter()
		.map(|ns| prefix(&ns))
		.collect();
	state.namespaces.push(namespace.to_string());
}

/// Recursively rewrite every `@target` reference inside a command-step tree
/// by prepending `<namespace>:` to its target name. Visits the same set of
/// nodes as [`crate::CommandStep::walk_templates`], but mutates target names
/// in `TargetCall` leaves rather than visiting templates.
fn rewrite_target_calls_in_steps(steps: &mut [CommandStep], namespace: &str) {
	for step in steps {
		match step {
			CommandStep::Shell(_) => {}
			CommandStep::TargetCall(call) => {
				call.target = format!("{namespace}:{}", call.target);
			}
			CommandStep::When(w) => rewrite_target_calls_in_steps(&mut w.commands, namespace),
			CommandStep::If(i) => {
				rewrite_target_calls_in_steps(&mut i.then, namespace);
				if let Some(else_branch) = i.r#else.as_mut() {
					rewrite_target_calls_in_steps(else_branch, namespace);
				}
			}
			CommandStep::For(f) => rewrite_target_calls_in_steps(&mut f.body, namespace),
			CommandStep::Match(m) => {
				for steps in m.cases.values_mut() {
					rewrite_target_calls_in_steps(steps, namespace);
				}
				if let Some(default) = m.default.as_mut() {
					rewrite_target_calls_in_steps(default, namespace);
				}
			}
		}
	}
}

/// Convert a relative path to absolute based on a source directory.
/// If already absolute, return as-is.
fn make_path_absolute(path: &str, base_dir: &Path) -> String {
	let p = PathBuf::from(path);
	if p.is_absolute() {
		path.to_string()
	} else {
		base_dir.join(path).to_string_lossy().to_string()
	}
}
