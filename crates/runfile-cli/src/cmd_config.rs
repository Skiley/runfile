use crate::runfile_helpers::canonicalize_clean;
use runfile_settings::Settings;
use runfile_shell::{ShellKind, detect_default_shell, resolve_shell};
use std::path::PathBuf;
use std::process;

fn load_settings() -> Settings {
	match Settings::load() {
		Ok(s) => s,
		Err(e) => {
			eprintln!("Error loading settings: {e}");
			process::exit(1);
		}
	}
}

fn save_settings(settings: &Settings) {
	if let Err(e) = settings.save() {
		eprintln!("Error saving settings: {e}");
		process::exit(1);
	}
}

pub fn cmd_config_path() {
	match runfile_settings::settings_file_path() {
		Some(path) => println!("{}", path.display()),
		None => {
			eprintln!("Error: cannot determine settings directory on this platform");
			process::exit(1);
		}
	}
}

pub fn cmd_set_shell(name: &str, path: PathBuf) {
	let mut settings = load_settings();
	settings.set_shell_path(name, path.clone());
	save_settings(&settings);
	println!("Shell \"{name}\" path set to: {}", path.display());
}

pub fn cmd_add_path_alias(alias: &str, path: PathBuf) {
	let mut settings = load_settings();
	let abs_path = canonicalize_clean(&path);
	settings.set_path_alias(alias, abs_path.clone());
	save_settings(&settings);
	println!("Path alias \"{alias}\" set to: {}", abs_path.display());
	println!("Usage: run -f {alias} <target>");
}

pub fn cmd_remove_path_alias(alias: &str) {
	let mut settings = load_settings();

	// Exact match first
	if settings.remove_path_alias(alias) {
		save_settings(&settings);
		println!("Path alias \"{alias}\" removed.");
		return;
	}

	// Partial match: find aliases containing the query as a substring
	let matches: Vec<String> = settings
		.path_aliases
		.keys()
		.filter(|key| key.contains(alias))
		.cloned()
		.collect();

	match matches.len() {
		0 => {
			eprintln!("Error: no path alias matching \"{alias}\"");
			process::exit(1);
		}
		1 => {
			let matched = &matches[0];
			settings.remove_path_alias(matched);
			save_settings(&settings);
			println!("Path alias \"{matched}\" removed.");
		}
		_ => {
			eprintln!("Error: \"{alias}\" matches multiple aliases:");
			let mut sorted = matches;
			sorted.sort();
			for m in &sorted {
				eprintln!("  {m}");
			}
			process::exit(1);
		}
	}
}

pub fn cmd_list_path_aliases() {
	let settings = load_settings();

	if settings.path_aliases.is_empty() {
		println!("No path aliases configured.");
		println!();
		println!("Add one with: run :config path-alias add <alias> <path>");
		return;
	}

	println!("Path aliases:");
	println!();

	let mut aliases: Vec<(&String, &PathBuf)> = settings.path_aliases.iter().collect();
	aliases.sort_by_key(|(name, _)| name.as_str());

	let col_width = aliases.iter().map(|(n, _)| n.len()).max().unwrap_or(0).max(10) + 2;

	for (alias, path) in aliases {
		println!("  {alias:<col_width$} {}", path.display());
	}
}

pub fn cmd_add_global_file(path: PathBuf) {
	let mut settings = load_settings();

	let abs_path = std::fs::canonicalize(&path).unwrap_or_else(|_| {
		eprintln!("Error: file not found: {}", path.display());
		process::exit(1);
	});
	let abs_path = canonicalize_clean(&abs_path);

	if !settings.add_global_file(abs_path.clone()) {
		println!("Global file already registered: {}", abs_path.display());
		return;
	}

	save_settings(&settings);
	println!("Global file added: {}", abs_path.display());
}

pub fn cmd_remove_global_file(query: &str) {
	let mut settings = load_settings();

	let path = PathBuf::from(query);

	// Try exact match first
	if settings.remove_global_file(&path) {
		save_settings(&settings);
		println!("Global file removed: {}", path.display());
		return;
	}

	// Try canonicalized exact match
	if let Ok(raw_abs) = std::fs::canonicalize(&path) {
		let abs = canonicalize_clean(&raw_abs);
		if settings.remove_global_file(&abs) {
			save_settings(&settings);
			println!("Global file removed: {}", abs.display());
			return;
		}
	}

	// Partial match: find entries whose path string contains the query
	let matches: Vec<PathBuf> = settings
		.global_files
		.iter()
		.filter(|p| p.to_string_lossy().contains(query))
		.cloned()
		.collect();

	match matches.len() {
		0 => {
			eprintln!("Error: no global file matching \"{query}\"");
			process::exit(1);
		}
		1 => {
			let matched = &matches[0];
			settings.remove_global_file(matched);
			save_settings(&settings);
			println!("Global file removed: {}", matched.display());
		}
		_ => {
			eprintln!("Error: \"{query}\" matches multiple global files:");
			for m in &matches {
				eprintln!("  {}", m.display());
			}
			process::exit(1);
		}
	}
}

pub fn cmd_list_global_files() {
	let settings = load_settings();

	if settings.global_files.is_empty() {
		println!("No global files configured.");
		println!();
		println!("Add one with: run :config global-files add <path-to-runfile>");
		return;
	}

	println!("Global files:");
	println!();

	for path in &settings.global_files {
		let exists = path.is_file();
		if exists {
			println!("  {}", path.display());
		} else {
			println!("  {} (not found)", path.display());
		}
	}
}

pub fn cmd_list_shells() {
	let settings = Settings::load().unwrap_or_default();

	let all_shells = [
		ShellKind::Bash,
		ShellKind::Zsh,
		ShellKind::Sh,
		ShellKind::Fish,
		ShellKind::PowerShell,
		ShellKind::Cmd,
	];

	// Detect default shell for annotation
	let default_shell = detect_default_shell().ok();

	println!("Shell availability:");
	println!();

	for kind in &all_shells {
		let name = kind.name();
		let custom_path = settings.get_shell_path(name);

		let (status, is_custom) = if let Some(path) = custom_path {
			if path.exists() {
				(format!("{}", path.display()), true)
			} else {
				(format!("{} (custom path not found)", path.display()), true)
			}
		} else {
			match resolve_shell(name) {
				Ok(resolved) => (format!("{}", resolved.path.display()), false),
				Err(_) => ("not found".to_string(), false),
			}
		};

		let is_default = default_shell.as_ref().is_some_and(|d| d.kind == *kind);

		let suffix = match (is_custom, is_default) {
			(true, true) => " (custom, default)",
			(true, false) => " (custom)",
			(false, true) => " (default)",
			(false, false) => "",
		};

		println!("  {name:<15} {status}{suffix}");
	}
}

pub fn cmd_reset() {
	// Show what will be deleted before doing it
	let path = match runfile_settings::settings_file_path() {
		Some(p) => p,
		None => {
			eprintln!("Error: cannot determine settings directory on this platform");
			process::exit(1);
		}
	};

	match Settings::delete_settings_file() {
		Ok(true) => {
			println!("Settings file deleted: {}", path.display());
			println!("All configuration has been reset to defaults.");
		}
		Ok(false) => {
			println!("No settings file found at: {}", path.display());
			println!("Nothing to reset.");
		}
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	}
}
