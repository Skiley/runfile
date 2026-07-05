use runfile_parser::RUNFILE_NAME;
use std::path::PathBuf;
use std::process;

use crate::runfile_helpers::{load_or_create_runfile, runfile_for_generate, write_runfile};

/// Minimal starter Runfile written by `:init`. A single `hello` target
/// running `echo Hello World` — works identically on every supported shell
/// (bash/zsh/sh/fish/powershell/cmd) and demonstrates the bare-string
/// `commands` sugar so users see the cleanest form by default.
const INIT_TEMPLATE: &str = r#"{
	"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
	"targets": {
		"hello": {
			"description": "Say Hello World",
			"commands": "echo Hello World"
		}
	}
}
"#;

pub fn cmd_init(path: Option<PathBuf>) {
	let output_path = path.unwrap_or_else(|| PathBuf::from(RUNFILE_NAME));

	if output_path.is_file() {
		eprintln!("Error: {} already exists", output_path.display());
		process::exit(1);
	}

	// Format the starter Runfile to match the project's .editorconfig for this path. The template
	// is tab-indented (one tab per level), so `apply_to_tab_indented` retargets the indentation
	// and applies the line/charset settings. With no applicable settings the tab-indented,
	// LF, trailing-newline template is written verbatim.
	let props = runfile_generators::EditorConfigProps::resolve_for_path(&output_path);
	if let Err(e) = std::fs::write(&output_path, props.apply_to_tab_indented(INIT_TEMPLATE)) {
		eprintln!("Error writing {}: {e}", output_path.display());
		process::exit(1);
	}
	println!("Created {}", output_path.display());
}

pub fn cmd_convert_package_json(pkg_path: Option<PathBuf>) {
	let pkg_path = pkg_path.unwrap_or_else(|| PathBuf::from("package.json"));

	if !pkg_path.is_file() {
		eprintln!("Error: {} not found", pkg_path.display());
		process::exit(1);
	}

	let pkg_contents = match std::fs::read_to_string(&pkg_path) {
		Ok(c) => c,
		Err(e) => {
			eprintln!("Error reading {}: {e}", pkg_path.display());
			process::exit(1);
		}
	};

	let pkg_json: serde_json::Value = match runfile_parser::from_json_str(&pkg_contents) {
		Ok(v) => v,
		Err(e) => {
			eprintln!("Error parsing {}: {e}", pkg_path.display());
			process::exit(1);
		}
	};

	let scripts = match pkg_json.get("scripts").and_then(|s| s.as_object()) {
		Some(s) => s,
		None => {
			eprintln!("No \"scripts\" found in {}", pkg_path.display());
			process::exit(1);
		}
	};

	if scripts.is_empty() {
		eprintln!("No scripts found in {}", pkg_path.display());
		process::exit(1);
	}

	let mut runfile = load_or_create_runfile();
	let existing: std::collections::HashSet<String> = runfile.targets.keys().cloned().collect();

	let conversion = runfile_converters::convert_package_json_scripts(scripts, &existing);

	if conversion.targets.is_empty() && conversion.skipped.is_empty() {
		println!("No scripts were converted.");
		return;
	}

	for (name, spec) in &conversion.targets {
		runfile.targets.insert(name.clone(), spec.clone());
	}

	// npm scripts always have node_modules/.bin in PATH — replicate this in globals
	if !conversion.targets.is_empty() {
		let globals = runfile.globals.get_or_insert_with(Default::default);
		let paths = globals.add_to_path.get_or_insert_with(Vec::new);
		if !paths.iter().any(|p| p == "node_modules/.bin") {
			paths.push("node_modules/.bin".to_string());
		}
		write_runfile(&runfile);
	}

	let mut names: Vec<&String> = conversion.targets.keys().collect();
	names.sort_by_key(|a| a.to_lowercase());

	if !names.is_empty() {
		println!(
			"Converted {} script(s) from {} into {RUNFILE_NAME}:",
			names.len(),
			pkg_path.display()
		);
		println!();
		for name in &names {
			println!("  {name}");
		}
	}

	if !conversion.skipped.is_empty() {
		let mut skipped = conversion.skipped.clone();
		skipped.sort_by_key(|a| a.to_lowercase());
		if !names.is_empty() {
			println!();
		}
		eprintln!("Skipped {} script(s) (target already exists):", skipped.len());
		for name in &skipped {
			eprintln!("  {name}");
		}
	}
}

pub fn cmd_convert_makefile(makefile_path: Option<PathBuf>) {
	let makefile_path = makefile_path.unwrap_or_else(|| PathBuf::from("Makefile"));

	if !makefile_path.is_file() {
		eprintln!("Error: {} not found", makefile_path.display());
		process::exit(1);
	}

	let contents = match std::fs::read_to_string(&makefile_path) {
		Ok(c) => c,
		Err(e) => {
			eprintln!("Error reading {}: {e}", makefile_path.display());
			process::exit(1);
		}
	};

	let mut runfile = load_or_create_runfile();
	let existing: std::collections::HashSet<String> = runfile.targets.keys().cloned().collect();

	let conversion = runfile_converters::convert_makefile(&contents, &existing);

	if conversion.targets.is_empty() && conversion.skipped.is_empty() {
		println!("No targets were converted.");
		return;
	}

	for (name, spec) in &conversion.targets {
		runfile.targets.insert(name.clone(), spec.clone());
	}

	// Makefile targets run from CWD by convention
	if !conversion.targets.is_empty() {
		let globals = runfile.globals.get_or_insert_with(Default::default);
		if globals.working_directory.is_none() {
			globals.working_directory = Some("{{ RUN.cwd }}".to_string());
		}
		write_runfile(&runfile);
	}

	let mut names: Vec<&String> = conversion.targets.keys().collect();
	names.sort_by_key(|a| a.to_lowercase());

	if !names.is_empty() {
		println!(
			"Converted {} target(s) from {} into Runfile.json:",
			names.len(),
			makefile_path.display()
		);
		println!();
		for name in &names {
			println!("  {name}");
		}
	}

	if !conversion.skipped.is_empty() {
		let mut skipped = conversion.skipped.clone();
		skipped.sort_by_key(|a| a.to_lowercase());
		if !names.is_empty() {
			println!();
		}
		eprintln!("Skipped {} target(s) (target already exists):", skipped.len());
		for name in &skipped {
			eprintln!("  {name}");
		}
	}
}

/// Write raw generated bytes to stdout for `--stdout` mode (exact bytes — no added newline — so
/// the output matches what would land on disk and pipes/redirects cleanly).
fn write_generated_to_stdout(bytes: &[u8]) {
	use std::io::Write;
	let stdout = std::io::stdout();
	let mut lock = stdout.lock();
	if lock.write_all(bytes).and_then(|_| lock.flush()).is_err() {
		// Broken pipe (e.g. piped into `head`) is a normal, quiet exit.
		process::exit(0);
	}
}

pub fn cmd_generate_vscode_tasks(file: Option<&std::path::Path>, stdout: bool, include_namespaces: bool) {
	use runfile_generators::{
		generate_vscode_tasks, merge_vscode_tasks, render_vscode_tasks, EditorConfigProps, VsCodeTasksFile,
	};

	let runfile = runfile_for_generate(file, include_namespaces);

	let tasks_path = PathBuf::from(".vscode/tasks.json");

	// Format the output to match the project's .editorconfig for this path (indentation, line
	// endings, final newline, trailing whitespace, BOM). Falls back to the historical 2-space /
	// LF output when no applicable settings exist.
	let props = EditorConfigProps::resolve_for_path(&tasks_path);

	if stdout {
		// Emit a freshly generated tasks file — not merged with any on-disk `.vscode/tasks.json` —
		// formatted as it would be written. Nothing is read from or written to disk.
		let generated = VsCodeTasksFile {
			version: "2.0.0".to_string(),
			tasks: generate_vscode_tasks(&runfile),
			extra: serde_json::Map::new(),
		};
		let bytes = render_vscode_tasks(&generated, &props).unwrap_or_else(|e| {
			eprintln!("Error serializing tasks: {e}");
			process::exit(1);
		});
		write_generated_to_stdout(&bytes);
		return;
	}

	let mut existing: VsCodeTasksFile = if tasks_path.is_file() {
		let contents = std::fs::read_to_string(&tasks_path).unwrap_or_else(|e| {
			eprintln!("Error reading {}: {e}", tasks_path.display());
			process::exit(1);
		});
		runfile_parser::from_json_str(&contents).unwrap_or_else(|e| {
			eprintln!("Error parsing {}: {e}", tasks_path.display());
			process::exit(1);
		})
	} else {
		VsCodeTasksFile {
			version: "2.0.0".to_string(),
			tasks: Vec::new(),
			extra: serde_json::Map::new(),
		}
	};

	let generated = generate_vscode_tasks(&runfile);
	let result = merge_vscode_tasks(&mut existing, generated);

	if result.updated.is_empty() && result.added.is_empty() && result.removed.is_empty() {
		println!("No tasks to generate.");
		return;
	}

	if let Some(parent) = tasks_path.parent() {
		std::fs::create_dir_all(parent).unwrap_or_else(|e| {
			eprintln!("Error creating {}: {e}", parent.display());
			process::exit(1);
		});
	}

	let bytes = render_vscode_tasks(&existing, &props).unwrap_or_else(|e| {
		eprintln!("Error serializing tasks: {e}");
		process::exit(1);
	});

	std::fs::write(&tasks_path, &bytes).unwrap_or_else(|e| {
		eprintln!("Error writing {}: {e}", tasks_path.display());
		process::exit(1);
	});

	println!("Generated VS Code tasks in {}:", tasks_path.display());
	if !result.added.is_empty() {
		println!();
		println!("  Added:");
		for label in &result.added {
			println!("    {label}");
		}
	}
	if !result.updated.is_empty() {
		println!();
		println!("  Updated:");
		for label in &result.updated {
			println!("    {label}");
		}
	}
	if !result.removed.is_empty() {
		println!();
		println!("  Removed:");
		for label in &result.removed {
			println!("    {label}");
		}
	}
}

pub fn cmd_generate_zed_tasks(file: Option<&std::path::Path>, stdout: bool, include_namespaces: bool) {
	use runfile_generators::{generate_zed_tasks, merge_zed_tasks, render_zed_tasks, EditorConfigProps, ZedTask};

	let runfile = runfile_for_generate(file, include_namespaces);

	let tasks_path = PathBuf::from(".zed/tasks.json");

	// Format the output to match the project's .editorconfig for this path (indentation, line
	// endings, final newline, trailing whitespace, BOM). Falls back to the historical 2-space /
	// LF output when no applicable settings exist.
	let props = EditorConfigProps::resolve_for_path(&tasks_path);

	if stdout {
		// Emit freshly generated tasks — not merged with any on-disk `.zed/tasks.json` — formatted
		// as they would be written. Nothing is read from or written to disk.
		let bytes = render_zed_tasks(&generate_zed_tasks(&runfile), &props).unwrap_or_else(|e| {
			eprintln!("Error serializing tasks: {e}");
			process::exit(1);
		});
		write_generated_to_stdout(&bytes);
		return;
	}

	let mut existing_tasks: Vec<ZedTask> = if tasks_path.is_file() {
		let contents = std::fs::read_to_string(&tasks_path).unwrap_or_else(|e| {
			eprintln!("Error reading {}: {e}", tasks_path.display());
			process::exit(1);
		});
		runfile_parser::from_json_str(&contents).unwrap_or_else(|e| {
			eprintln!("Error parsing {}: {e}", tasks_path.display());
			process::exit(1);
		})
	} else {
		Vec::new()
	};

	let generated = generate_zed_tasks(&runfile);
	let result = merge_zed_tasks(&mut existing_tasks, generated);

	if result.updated.is_empty() && result.added.is_empty() && result.removed.is_empty() {
		println!("No tasks to generate.");
		return;
	}

	if let Some(parent) = tasks_path.parent() {
		std::fs::create_dir_all(parent).unwrap_or_else(|e| {
			eprintln!("Error creating {}: {e}", parent.display());
			process::exit(1);
		});
	}

	let bytes = render_zed_tasks(&existing_tasks, &props).unwrap_or_else(|e| {
		eprintln!("Error serializing tasks: {e}");
		process::exit(1);
	});

	std::fs::write(&tasks_path, &bytes).unwrap_or_else(|e| {
		eprintln!("Error writing {}: {e}", tasks_path.display());
		process::exit(1);
	});

	println!("Generated Zed tasks in {}:", tasks_path.display());
	if !result.added.is_empty() {
		println!();
		println!("  Added:");
		for label in &result.added {
			println!("    {label}");
		}
	}
	if !result.updated.is_empty() {
		println!();
		println!("  Updated:");
		for label in &result.updated {
			println!("    {label}");
		}
	}
	if !result.removed.is_empty() {
		println!();
		println!("  Removed:");
		for label in &result.removed {
			println!("    {label}");
		}
	}
}

pub fn cmd_generate_jetbrains_run_configs(
	file: Option<&std::path::Path>,
	output_dir: Option<&std::path::Path>,
	stdout: bool,
	include_namespaces: bool,
) {
	use runfile_generators::{
		check_existing_jetbrains_config, generate_jetbrains_configs, is_jetbrains_config_ours, render_jetbrains_config,
		EditorConfigProps, JetBrainsConfigCheck,
	};
	use std::collections::HashSet;

	let runfile = runfile_for_generate(file, include_namespaces);

	let run_dir = output_dir.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".run"));

	let configs = generate_jetbrains_configs(&runfile);

	if stdout {
		// Emit each generated run configuration to stdout — formatted per .editorconfig for the
		// path it would occupy — instead of writing any files (no directory is created, no stale
		// sweep runs). With more than one config, a filename-header comment delimits the blocks; a
		// single config is emitted verbatim so it can be redirected straight into a `.run.xml` file.
		let show_headers = configs.len() > 1;
		for (i, config) in configs.iter().enumerate() {
			let file_path = run_dir.join(&config.file_name);
			let props = EditorConfigProps::resolve_for_path(&file_path);
			let bytes = render_jetbrains_config(&config.config_name, &config.target_name, &props);
			if show_headers {
				if i > 0 {
					write_generated_to_stdout(b"\n");
				}
				// Normalize to forward slashes so the delimiter comment is identical on every
				// platform (on Windows `Path::display` would render `.run\Runfile_*.run.xml`).
				let display_path = file_path.display().to_string().replace('\\', "/");
				write_generated_to_stdout(format!("<!-- {display_path} -->\n").as_bytes());
			}
			write_generated_to_stdout(&bytes);
		}
		return;
	}

	std::fs::create_dir_all(&run_dir).unwrap_or_else(|e| {
		eprintln!("Error creating {}: {e}", run_dir.display());
		process::exit(1);
	});

	let mut added: Vec<String> = Vec::new();
	let mut updated: Vec<String> = Vec::new();
	let mut skipped: Vec<(String, String)> = Vec::new();
	let mut removed: Vec<String> = Vec::new();

	// Sweep stale `Runfile_*.run.xml` files we previously emitted but whose target was
	// removed from the Runfile. We only delete files that pass the structural ownership
	// check, so hand-authored XML in `.run/` (even something that happens to start with
	// `Runfile_`) is left alone.
	let generated_file_names: HashSet<&str> = configs.iter().map(|c| c.file_name.as_str()).collect();
	if let Ok(entries) = std::fs::read_dir(&run_dir) {
		for entry in entries.flatten() {
			let file_name = match entry.file_name().into_string() {
				Ok(n) => n,
				Err(_) => continue,
			};
			if !file_name.starts_with("Runfile_") || !file_name.ends_with(".run.xml") {
				continue;
			}
			if generated_file_names.contains(file_name.as_str()) {
				continue;
			}
			let path = entry.path();
			let contents = match std::fs::read_to_string(&path) {
				Ok(c) => c,
				Err(_) => continue,
			};
			if !is_jetbrains_config_ours(&contents) {
				continue;
			}
			std::fs::remove_file(&path).unwrap_or_else(|e| {
				eprintln!("Error removing {}: {e}", path.display());
				process::exit(1);
			});
			removed.push(file_name);
		}
	}

	for config in &configs {
		let file_path = run_dir.join(&config.file_name);

		if file_path.is_file() {
			let contents = std::fs::read_to_string(&file_path).unwrap_or_default();
			match check_existing_jetbrains_config(&contents, &config.config_name, &config.target_name) {
				JetBrainsConfigCheck::Ours => {
					updated.push(config.config_name.clone());
				}
				JetBrainsConfigCheck::Foreign(reason) => {
					skipped.push((config.file_name.clone(), reason));
					continue;
				}
			}
		} else {
			added.push(config.config_name.clone());
		}

		// Format the output to match the project's .editorconfig for this path (indentation, line
		// endings, final newline, trailing whitespace, BOM). Falls back to the historical 2-space /
		// LF output (matching `config.xml`) when no applicable settings exist.
		let props = EditorConfigProps::resolve_for_path(&file_path);
		let bytes = render_jetbrains_config(&config.config_name, &config.target_name, &props);
		std::fs::write(&file_path, &bytes).unwrap_or_else(|e| {
			eprintln!("Error writing {}: {e}", file_path.display());
			process::exit(1);
		});
	}

	if added.is_empty() && updated.is_empty() && skipped.is_empty() && removed.is_empty() {
		println!("No run configurations to generate.");
		return;
	}

	println!("Generated JetBrains run configurations in {}:", run_dir.display());
	if !added.is_empty() {
		println!();
		println!("  Added:");
		for label in &added {
			println!("    {label}");
		}
	}
	if !updated.is_empty() {
		println!();
		println!("  Updated:");
		for label in &updated {
			println!("    {label}");
		}
	}
	if !removed.is_empty() {
		println!();
		println!("  Removed:");
		for label in &removed {
			println!("    {label}");
		}
	}
	if !skipped.is_empty() {
		println!();
		eprintln!("  Skipped (existing file with different configuration):");
		for (file_name, reason) in &skipped {
			eprintln!("    {file_name}: {reason}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::INIT_TEMPLATE;

	#[test]
	fn init_template_parses_as_runfile() {
		// The template is a literal string, so the type system can't catch
		// typos. Round-trip it through the parser to make sure `:init`
		// always produces a file the rest of the toolchain accepts.
		let runfile = runfile_parser::parse_runfile(INIT_TEMPLATE).expect("init template must parse");
		assert!(runfile.targets.contains_key("hello"));
	}
}
