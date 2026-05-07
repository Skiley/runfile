use runfile_parser::{
	discover_runfile_cwd, merge_runfiles, parse_runfile_from_path, CommandSpec, MergeResult, Runfile, SourceKind,
	RUNFILE_NAME,
};
use runfile_settings::Settings;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process;

/// Environment variable that overrides the default Runfile path used when
/// `-f`/`--file` is not passed. Useful in CI to point at a non-default Runfile
/// without threading the flag through every invocation.
pub const RUNFILE_TARGET_ENV_VAR: &str = "RUNFILE_TARGET";

/// Read [`RUNFILE_TARGET_ENV_VAR`] and return the path it points to, if set
/// to a non-empty value. Returns `None` otherwise.
pub fn runfile_target_env() -> Option<PathBuf> {
	std::env::var(RUNFILE_TARGET_ENV_VAR)
		.ok()
		.filter(|s| !s.is_empty())
		.map(PathBuf::from)
}

/// Apply the `RUNFILE_TARGET` env var as a default for `-f`/`--file`.
/// If `file` is `Some`, returns it unchanged. Otherwise, returns the env-var
/// path (owned) when set, or `None` to fall through to auto-discovery.
fn effective_file(file: Option<&Path>) -> Option<PathBuf> {
	if let Some(p) = file {
		return Some(p.to_path_buf());
	}
	runfile_target_env()
}

/// Resolve the Runfile path: use explicit path or alias if given, otherwise
/// fall back to the [`RUNFILE_TARGET_ENV_VAR`] env var, otherwise auto-discover.
/// Always returns an absolute path.
pub fn resolve_runfile_path(file: Option<&std::path::Path>) -> PathBuf {
	let env_path = effective_file(file);
	let path = if let Some(path) = env_path.as_deref() {
		if path.is_file() {
			path.to_path_buf()
		} else {
			// Check if it's a path alias
			let alias_name = path.to_string_lossy();
			let settings = Settings::load().unwrap_or_default();
			if let Some(aliased_path) = settings.get_path_alias(&alias_name) {
				if aliased_path.is_file() {
					aliased_path.clone()
				} else {
					eprintln!(
						"Error: path alias \"{alias_name}\" points to {}, which was not found",
						aliased_path.display()
					);
					process::exit(1);
				}
			} else if file.is_none() {
				// Path came from RUNFILE_TARGET — make the source explicit so
				// users aren't confused when no `-f` was on the command line.
				eprintln!(
					"Error: {RUNFILE_TARGET_ENV_VAR} points to {}, which was not found",
					path.display()
				);
				process::exit(1);
			} else {
				eprintln!("Error: specified Runfile not found: {}", path.display());
				eprintln!("(Not a path alias either. Use `run :config path-alias add` to create one.)");
				process::exit(1);
			}
		}
	} else {
		match discover_runfile_cwd() {
			Ok(p) => p,
			Err(e) => {
				eprintln!("Error: {e}");
				process::exit(1);
			}
		}
	};

	canonicalize_clean(&path)
}

/// Resolve local Runfile + global files, returning the merged result.
/// `runfile_dir` is the effective directory of the local Runfile (for working dir resolution).
pub fn resolve_and_merge(file: Option<&std::path::Path>) -> MergeResult {
	let settings = Settings::load().unwrap_or_default();
	let cwd = std::env::current_dir().unwrap_or_default();

	// Resolve local Runfile (optional). When `RUNFILE_TARGET` is set and `-f`
	// is not passed, treat the env var like an explicit `-f` — the file must
	// exist (or be a path alias), with no fallback to auto-discovery.
	let env_path = effective_file(file);
	let local: Option<(Runfile, PathBuf)> = if let Some(path) = env_path.as_deref() {
		let resolved = resolve_runfile_path(Some(path));
		let runfile = match parse_runfile_from_path(&resolved) {
			Ok(r) => r,
			Err(e) => {
				eprintln!("Error parsing {}: {e}", resolved.display());
				process::exit(1);
			}
		};
		Some((runfile, resolved))
	} else {
		match discover_runfile_cwd() {
			Ok(p) => {
				let abs = canonicalize_clean(&p);
				match parse_runfile_from_path(&abs) {
					Ok(r) => Some((r, abs)),
					Err(e) => {
						eprintln!("Error parsing {}: {e}", abs.display());
						process::exit(1);
					}
				}
			}
			Err(_) => None, // No local Runfile — global files may still provide targets
		}
	};

	match merge_runfiles(local, &settings.global_files, &cwd) {
		Ok(result) => result,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	}
}

/// Helper to get the local runfile dir from a MergeResult's target_sources.
pub fn local_dir_from_merge(result: &MergeResult) -> PathBuf {
	// Find any Local source and use its parent dir
	for (path, kind) in result.target_sources.values() {
		if *kind == SourceKind::Local {
			return path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
		}
	}
	std::env::current_dir().unwrap_or_default()
}

/// Helper to get the local runfile *file path* from a MergeResult. Used to
/// populate `{{ RUN.file }}` for targets that come from the local Runfile
/// (i.e. don't have an entry in `target_sources` because they're not in the
/// merged runfile, or for the top-level fallback). Returns the synthetic
/// `<cwd>/Runfile.json` path when no local source is present (e.g. when the
/// run is driven entirely by global files).
pub fn local_file_from_merge(result: &MergeResult) -> PathBuf {
	for (path, kind) in result.target_sources.values() {
		if *kind == SourceKind::Local {
			return path.clone();
		}
	}
	std::env::current_dir()
		.unwrap_or_default()
		.join(runfile_parser::RUNFILE_NAME)
}

/// Canonicalize and strip Windows UNC prefix.
pub fn canonicalize_clean(path: &std::path::Path) -> PathBuf {
	let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
	#[cfg(windows)]
	{
		let s = abs.to_string_lossy();
		if let Some(stripped) = s.strip_prefix(r"\\?\") {
			return PathBuf::from(stripped);
		}
	}
	abs
}

pub fn load_or_create_runfile() -> Runfile {
	let runfile_path = PathBuf::from(RUNFILE_NAME);
	if runfile_path.is_file() {
		let contents = std::fs::read_to_string(&runfile_path).unwrap_or_else(|e| {
			eprintln!("Error reading {RUNFILE_NAME}: {e}");
			process::exit(1);
		});
		runfile_parser::from_json_str(&contents).unwrap_or_else(|e| {
			eprintln!("Error parsing {RUNFILE_NAME}: {e}");
			process::exit(1);
		})
	} else {
		Runfile {
			schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".to_string(),
			includes: None,
			targets: HashMap::new(),
			globals: None,
			namespaces: Vec::new(),
		}
	}
}

pub fn write_runfile(runfile: &Runfile) {
	write_runfile_to_path(runfile, std::path::Path::new(RUNFILE_NAME));
}

pub fn write_runfile_to_path(runfile: &Runfile, path: &std::path::Path) {
	let sorted_targets: std::collections::BTreeMap<&String, &CommandSpec> = runfile.targets.iter().collect();

	let mut map = serde_json::Map::new();
	map.insert("$schema".to_string(), serde_json::Value::String(runfile.schema.clone()));
	if let Some(globals) = &runfile.globals {
		map.insert("globals".to_string(), serde_json::to_value(globals).unwrap());
	}
	map.insert("targets".to_string(), serde_json::to_value(&sorted_targets).unwrap());

	let json = serde_json::to_string_pretty(&map).unwrap_or_else(|e| {
		eprintln!("Error serializing {RUNFILE_NAME}: {e}");
		process::exit(1);
	});

	std::fs::write(path, json.as_bytes()).unwrap_or_else(|e| {
		eprintln!("Error writing {}: {e}", path.display());
		process::exit(1);
	});
}
