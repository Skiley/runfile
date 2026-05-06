use runfile_executor::{
	extract_target_with_cwd, format_extracted_commands, log_total_timing, run_target_with_cwd,
	InteractiveStdinPrompter, RunArgs, RunContext, StdinPrompter,
};
use runfile_parser::{is_internal_target_name, CommandSpec, Runfile, SourceKind};
use runfile_settings::Settings;
use runfile_shell::ResolvedShell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::time::Instant;

use crate::agent_detect;
use crate::runfile_helpers::{local_dir_from_merge, resolve_and_merge};
use crate::shell::{resolve_cli_shell_override, resolve_shell_for_runfile};

/// Common resolved state shared across cmd_run, cmd_dry_run, cmd_watch.
struct ResolvedTarget {
	resolved_name: String,
	runfile: Runfile,
	runfile_dir: PathBuf,
	caller_cwd: PathBuf,
	source_dirs: HashMap<String, PathBuf>,
	settings: Settings,
	shell: ResolvedShell,
	args: RunArgs,
}

/// Resolve merge result, target name, shell, and args — shared setup for run/dry-run/watch.
fn resolve_target_setup(
	target_name: &str,
	extra_args: &[String],
	file: Option<&std::path::Path>,
	cli_shell: Option<&str>,
	stdin_args: bool,
) -> ResolvedTarget {
	let merge_result = resolve_and_merge(file);
	let runfile_dir = local_dir_from_merge(&merge_result);
	check_conflict(target_name, &merge_result);

	let runfile = merge_result.runfile;
	let source_dirs = merge_result.source_dirs;

	let resolved_name = match runfile.resolve_target(target_name) {
		Some(name) => name.to_string(),
		None => {
			eprintln!("Error: unknown target \"{target_name}\"");
			eprintln!("Run `run :list` to see available targets.");
			process::exit(1);
		}
	};

	if is_internal_target_name(&resolved_name) {
		eprintln!("Error: target \"{target_name}\" is internal and cannot be invoked directly.");
		eprintln!("Internal targets (names starting with \"_\") are only callable via `@_name` from another target.");
		process::exit(1);
	}

	let caller_cwd = std::env::current_dir().unwrap_or_else(|_| runfile_dir.clone());
	let settings = Settings::load().unwrap_or_default();

	let spec = &runfile.targets[&resolved_name];
	let shell = if let Some(shell_override) = cli_shell {
		resolve_cli_shell_override(shell_override, &settings)
	} else {
		resolve_shell_for_runfile(spec.force_shell.as_deref(), &settings)
	};

	let prompter: Option<Arc<dyn StdinPrompter>> = if stdin_args {
		Some(Arc::new(InteractiveStdinPrompter::new()))
	} else {
		None
	};
	let args = RunArgs::parse(extra_args)
		.with_run_context(RunContext::new(shell.kind.name()))
		.with_stdin_prompter(prompter);

	ResolvedTarget {
		resolved_name,
		runfile,
		runfile_dir,
		caller_cwd,
		source_dirs,
		settings,
		shell,
		args,
	}
}

pub fn cmd_list(file: Option<&std::path::Path>) {
	let merge_result = resolve_and_merge(file);
	let runfile = &merge_result.runfile;
	let target_sources = &merge_result.target_sources;

	// Group targets by (source file, source kind).
	// Use BTreeMap<String, ...> so target names within each group are sorted.
	let mut groups: Vec<(PathBuf, SourceKind, BTreeMap<&String, &CommandSpec>)> = Vec::new();

	// Collect all unique source files preserving a stable order: Local first, Included, Global.
	let mut seen_files: Vec<(PathBuf, SourceKind)> = Vec::new();

	// First pass: collect files in display order (Global → Included → Local)
	for kind in &[SourceKind::Global, SourceKind::Included, SourceKind::Local] {
		let mut files_for_kind: Vec<&PathBuf> = target_sources
			.values()
			.filter(|(_, k)| k == kind)
			.map(|(p, _)| p)
			.collect();
		files_for_kind.sort();
		files_for_kind.dedup();
		for file_path in files_for_kind {
			if !seen_files.iter().any(|(p, _)| p == file_path) {
				seen_files.push((file_path.clone(), *kind));
			}
		}
	}

	// Second pass: populate groups with sorted targets (excluding internal targets)
	for (file_path, kind) in &seen_files {
		let mut targets_in_group: BTreeMap<&String, &CommandSpec> = BTreeMap::new();
		for (name, (src_file, src_kind)) in target_sources {
			if src_file == file_path && src_kind == kind && !is_internal_target_name(name) {
				if let Some(spec) = runfile.targets.get(name) {
					targets_in_group.insert(name, spec);
				}
			}
		}
		if !targets_in_group.is_empty() {
			groups.push((file_path.clone(), *kind, targets_in_group));
		}
	}

	// If only one source file, use the simpler original header
	let single_source = groups.len() == 1;

	let all_names: Vec<&String> = runfile.targets.keys().filter(|n| !is_internal_target_name(n)).collect();
	let col_width = all_names.iter().map(|n| n.len()).max().unwrap_or(0).max(20) + 1;

	for (i, (file_path, kind, targets_in_group)) in groups.iter().enumerate() {
		if i > 0 {
			println!();
		}

		if single_source {
			println!("Available targets:");
		} else {
			let kind_label = match kind {
				SourceKind::Local => "",
				SourceKind::Included => "included ",
				SourceKind::Global => "global ",
			};
			let display_path = display_file_path(file_path);
			println!("Available targets (from {kind_label}{display_path}):");
		}
		println!();

		for (name, spec) in targets_in_group {
			print_target(name, spec, col_width);
		}
	}

	// Show conflicting targets
	if !merge_result.conflicts.is_empty() {
		let mut conflict_names: Vec<&String> = merge_result.conflicts.keys().collect();
		conflict_names.sort();

		if !groups.is_empty() {
			println!();
		}
		eprintln!("Warning: the following targets are defined in multiple files and cannot be run:");
		eprintln!();
		for name in &conflict_names {
			let sources = &merge_result.conflicts[*name];
			let files: Vec<String> = sources.iter().map(|(p, _)| display_file_path(p)).collect();
			eprintln!("  {name}  (defined in: {})", files.join(", "));
		}
	}
}

/// Format a file path for display: use filename if in current dir, otherwise relative or absolute.
fn display_file_path(path: &std::path::Path) -> String {
	if let Ok(cwd) = std::env::current_dir() {
		if let Ok(rel) = path.strip_prefix(&cwd) {
			let rel_str = rel.to_string_lossy();
			if rel_str.contains('/') || rel_str.contains('\\') {
				return format!("./{}", rel_str.replace('\\', "/"));
			}
			return rel_str.to_string();
		}
	}
	// Try to use ~ for home directory
	if let Some(home) = dirs::home_dir() {
		if let Ok(rel) = path.strip_prefix(&home) {
			return format!("~/{}", rel.to_string_lossy().replace('\\', "/"));
		}
	}
	path.to_string_lossy().to_string()
}

/// Check if a target name is conflicting (defined in multiple files) and exit with an error if so.
fn check_conflict(target_name: &str, merge_result: &runfile_parser::MergeResult) {
	if let Some(sources) = merge_result.conflicts.get(target_name) {
		eprintln!("Error: target \"{target_name}\" is defined in multiple files:");
		for (path, _) in sources {
			eprintln!("  {}", display_file_path(path));
		}
		eprintln!();
		eprintln!("Rename or remove the duplicate target to resolve the conflict.");
		process::exit(1);
	}
}

fn print_target(name: &str, spec: &CommandSpec, col_width: usize) {
	let alias_suffix = if let Some(aliases) = &spec.aliases {
		if aliases.is_empty() {
			String::new()
		} else {
			format!(" (aliases: {})", aliases.join(", "))
		}
	} else {
		String::new()
	};

	if let Some(desc) = &spec.description {
		println!("  {name:<col_width$} {desc}{alias_suffix}");
	} else {
		const MAX_PREVIEW: usize = 60;
		// Flatten command steps to leaf shell templates for the preview.
		// Control-flow blocks (if/for) contribute their inner shell strings.
		let mut leaves: Vec<String> = Vec::new();
		runfile_parser::walk_step_templates(&spec.commands, &mut |t| leaves.push(t.to_string()));
		let full: String = leaves.join("; ");
		if full.len() <= MAX_PREVIEW {
			println!("  {name:<col_width$} {full}{alias_suffix}");
		} else {
			println!("  {name:<col_width$} {}...{alias_suffix}", &full[..MAX_PREVIEW]);
		}
	}
}

pub fn cmd_run(
	target_name: &str,
	extra_args: &[String],
	file: Option<&std::path::Path>,
	cli_shell: Option<&str>,
	timings: bool,
	yes: bool,
	stdin_args: bool,
) {
	let rt = resolve_target_setup(target_name, extra_args, file, cli_shell, stdin_args);

	// If the target defines watch patterns, automatically enter watch mode
	let spec = &rt.runfile.targets[&rt.resolved_name];
	if spec.watch.as_ref().is_some_and(|p| !p.is_empty()) {
		return cmd_watch(target_name, extra_args, file, cli_shell, timings, yes, stdin_args);
	}

	let private_keys = rt.settings.resolve_private_keys();
	let pk_slice: Option<&[String]> = if private_keys.is_empty() {
		None
	} else {
		Some(private_keys.as_slice())
	};

	let total_start = Instant::now();

	match run_target_with_cwd(
		&rt.resolved_name,
		&rt.runfile,
		&rt.shell,
		&rt.args,
		&rt.runfile_dir,
		&rt.caller_cwd,
		&rt.source_dirs,
		timings,
		yes,
		pk_slice,
	) {
		Ok(result) => {
			if timings {
				log_total_timing(total_start.elapsed());
			}
			// `final_status` already accounts for `ignoreErrors` (it's
			// computed in the executor): if any step failed and the target
			// wasn't `ignoreErrors`-ed, status is non-zero. Otherwise it's
			// the last command's status (possibly zero, e.g. when a
			// `when: always` cleanup succeeds).
			let code = result
				.final_status
				.code()
				.unwrap_or(if result.final_status.success() { 0 } else { 1 });
			process::exit(code);
		}
		Err(e) => {
			if timings {
				log_total_timing(total_start.elapsed());
			}
			eprintln!("Error: {e}");
			process::exit(1);
		}
	}
}

pub fn cmd_dry_run(
	target_name: &str,
	extra_args: &[String],
	file: Option<&std::path::Path>,
	cli_shell: Option<&str>,
	stdin_args: bool,
) {
	agent_detect::refuse_if_agent("dry-run a target");

	let rt = resolve_target_setup(target_name, extra_args, file, cli_shell, stdin_args);

	match extract_target_with_cwd(
		&rt.resolved_name,
		&rt.runfile,
		&rt.args,
		&rt.runfile_dir,
		&rt.caller_cwd,
		&rt.source_dirs,
	) {
		Ok(commands) => {
			let lines = format_extracted_commands(&commands, &rt.shell.kind);
			for line in lines {
				println!("{}", line);
			}
		}
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	}
}

pub fn cmd_watch(
	target_name: &str,
	extra_args: &[String],
	file: Option<&std::path::Path>,
	cli_shell: Option<&str>,
	timings: bool,
	yes: bool,
	stdin_args: bool,
) {
	use globset::{Glob, GlobSetBuilder};
	use notify::{Event, EventKind, RecursiveMode, Watcher};
	use std::sync::mpsc;
	use std::time::Duration;

	const RESET: &str = "\x1b[0m";
	const BOLD: &str = "\x1b[1m";
	const CYAN: &str = "\x1b[36m";
	const DIM: &str = "\x1b[2m";

	let rt = resolve_target_setup(target_name, extra_args, file, cli_shell, stdin_args);

	let spec = &rt.runfile.targets[&rt.resolved_name];
	let watch_patterns = match &spec.watch {
		Some(patterns) if !patterns.is_empty() => patterns.clone(),
		_ => {
			eprintln!("Error: target \"{}\" has no watch patterns defined.", rt.resolved_name);
			eprintln!("Add a \"watch\" field with glob patterns, e.g.: \"watch\": [\"src/**/*.rs\"]");
			process::exit(1);
		}
	};

	// Build include/exclude glob sets
	let mut include_builder = GlobSetBuilder::new();
	let mut exclude_builder = GlobSetBuilder::new();
	for pattern in &watch_patterns {
		if let Some(neg) = pattern.strip_prefix('!') {
			match Glob::new(neg) {
				Ok(g) => {
					exclude_builder.add(g);
				}
				Err(e) => {
					eprintln!("Error: invalid exclude pattern \"{neg}\": {e}");
					process::exit(1);
				}
			}
		} else {
			match Glob::new(pattern) {
				Ok(g) => {
					include_builder.add(g);
				}
				Err(e) => {
					eprintln!("Error: invalid watch pattern \"{pattern}\": {e}");
					process::exit(1);
				}
			}
		}
	}
	let include_set = include_builder.build().unwrap();
	let exclude_set = exclude_builder.build().unwrap();

	let private_keys = rt.settings.resolve_private_keys();
	let pk_slice: Option<&[String]> = if private_keys.is_empty() {
		None
	} else {
		Some(private_keys.as_slice())
	};

	// Run the target once initially
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} Running \"{BOLD}{}{RESET}\"...",
		rt.resolved_name
	);
	let _ = run_target_with_cwd(
		&rt.resolved_name,
		&rt.runfile,
		&rt.shell,
		&rt.args,
		&rt.runfile_dir,
		&rt.caller_cwd,
		&rt.source_dirs,
		timings,
		yes,
		pk_slice,
	);

	// Set up file watcher
	let (tx, rx) = mpsc::channel();
	let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
		if let Ok(event) = res {
			let _ = tx.send(event);
		}
	})
	.unwrap_or_else(|e| {
		eprintln!("Error: failed to create file watcher: {e}");
		process::exit(1);
	});

	watcher
		.watch(rt.runfile_dir.as_ref(), RecursiveMode::Recursive)
		.unwrap_or_else(|e| {
			eprintln!("Error: failed to watch {}: {e}", rt.runfile_dir.display());
			process::exit(1);
		});

	eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}Watching for changes... (Ctrl+C to stop){RESET}");

	while let Ok(event) = rx.recv() {
		// Debounce: collect events for 300ms
		let mut changed_paths: Vec<PathBuf> = Vec::new();
		if matches!(
			event.kind,
			EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
		) {
			changed_paths.extend(event.paths);
		}
		let deadline = Instant::now() + Duration::from_millis(300);
		loop {
			match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
				Ok(ev)
					if matches!(
						ev.kind,
						EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
					) =>
				{
					changed_paths.extend(ev.paths);
				}
				_ => break,
			}
		}

		// Filter through glob sets
		let mut unique: Vec<String> = changed_paths
			.iter()
			.filter_map(|p| {
				let rel = p.strip_prefix(&rt.runfile_dir).unwrap_or(p);
				let rel_str = rel.to_string_lossy().replace('\\', "/");
				if include_set.is_match(&rel_str) && !exclude_set.is_match(&rel_str) {
					Some(rel_str.to_string())
				} else {
					None
				}
			})
			.collect();

		if unique.is_empty() {
			continue;
		}
		unique.sort();
		unique.dedup();

		let preview: String = if unique.len() <= 3 {
			unique.join(", ")
		} else {
			format!("{}, ... ({} files)", unique[..3].join(", "), unique.len())
		};

		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}Changed: {preview}{RESET}");
		eprintln!(
			"{BOLD}{CYAN}[runfile]{RESET} Re-running \"{BOLD}{}{RESET}\"...",
			rt.resolved_name
		);

		let _ = run_target_with_cwd(
			&rt.resolved_name,
			&rt.runfile,
			&rt.shell,
			&rt.args,
			&rt.runfile_dir,
			&rt.caller_cwd,
			&rt.source_dirs,
			timings,
			yes,
			pk_slice,
		);

		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}Watching for changes... (Ctrl+C to stop){RESET}");
	}
}
