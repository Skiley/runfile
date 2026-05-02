#![allow(clippy::result_large_err)]
//! `MergeError` transitively contains `ParseError`, which contains a `json5::Error`
//! and a `DslParseError`; the boxed error variants are intentional but Clippy flags
//! the resulting `Result<_, MergeError>` size. The merge errors only happen at
//! startup when parsing — the perf cost is irrelevant.

use crate::parse::parse_runfile_from_path_partial;
use crate::schema::{CommandSpec, Globals, Runfile};
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
}

impl MergeState {
	pub(crate) fn new() -> Self {
		Self {
			targets: HashMap::new(),
			source_dirs: HashMap::new(),
			target_sources: HashMap::new(),
			all_sources: HashMap::new(),
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
		if let Some(globals) = &global_runfile.globals {
			if let Some(only_dirs) = &globals.only_in_directories {
				if !is_cwd_allowed(cwd, &global_dir, only_dirs) {
					continue;
				}
			}
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

	Ok(MergeResult {
		runfile: Runfile {
			schema,
			includes: None,
			targets: state.targets,
			globals: None,
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

/// Bake a file's globals into a target's own fields.
/// After baking, the target is self-contained and doesn't need its source globals.
fn bake_globals_into_target(
	mut spec: CommandSpec,
	globals: Option<&Globals>,
	source_dir: &Path,
	_target_name: &str,
) -> CommandSpec {
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

	// addToPath: prepend global (made absolute) before target
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

	// onlyInDirectories: prepend global before target
	if let Some(global_dirs) = &globals.only_in_directories {
		let mut merged = global_dirs.clone();
		if let Some(target_dirs) = spec.only_in_directories.take() {
			merged.extend(target_dirs);
		}
		spec.only_in_directories = Some(merged);
	}

	spec
}

/// Resolve includes from a parsed Runfile, merging included targets into the merge state.
/// `visited` tracks canonicalized paths for cycle detection.
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

	for include_path_str in &includes {
		let include_path = base_dir.join(include_path_str);
		let canonical =
			std::fs::canonicalize(&include_path).map_err(|_| MergeError::IncludeNotFound(include_path.clone()))?;

		if !canonical.is_file() {
			return Err(MergeError::IncludeNotFound(include_path));
		}

		// Security: reject includes that escape the Runfile's directory tree
		if !canonical.starts_with(&canonical_base) {
			return Err(MergeError::IncludePathTraversal(include_path));
		}

		if !visited.insert(canonical.clone()) {
			return Err(MergeError::IncludeCycle(canonical.to_string_lossy().to_string()));
		}

		let included_runfile =
			parse_runfile_from_path_partial(&canonical).map_err(|e| MergeError::IncludeParse(canonical.clone(), e))?;

		let include_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();

		// Recursively resolve nested includes first
		resolve_includes(&included_runfile, &canonical, state, visited)?;

		// Merge included targets
		let globals_ref = included_runfile.globals.as_ref();
		state.insert_targets(
			included_runfile.targets,
			globals_ref,
			&include_path,
			&include_dir,
			SourceKind::Included,
		);
	}

	Ok(())
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
