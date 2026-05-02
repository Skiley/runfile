use runfile_parser::{
	parse_runfile_from_path, CommandSpec, CommandStep, Globals, IfStep, Runfile, RUNFILE_NAME, WORKING_DIRECTORY_CWD,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process;

use crate::runfile_helpers::{load_or_create_runfile, resolve_runfile_path, write_runfile, write_runfile_to_path};

pub fn cmd_init(path: Option<PathBuf>) {
	let output_path = path.unwrap_or_else(|| PathBuf::from(RUNFILE_NAME));

	if output_path.is_file() {
		eprintln!("Error: {} already exists", output_path.display());
		process::exit(1);
	}

	// Generate a per-shell hello world using `if "$(RUN.shell) == ..."`
	// dispatching. Default branch covers POSIX shells; Windows shells get
	// their idiomatic hello.
	let if_powershell = IfStep {
		condition: "$(RUN.shell) == powershell".to_string(),
		then: vec![CommandStep::Shell("Write-Host 'Hello World'".to_string())],
		r#else: Some(vec![CommandStep::If(IfStep {
			condition: "$(RUN.shell) == cmd".to_string(),
			then: vec![CommandStep::Shell("echo Hello World".to_string())],
			r#else: Some(vec![CommandStep::If(IfStep {
				condition: "$(RUN.shell) == fish".to_string(),
				then: vec![CommandStep::Shell("echo 'Hello World'".to_string())],
				r#else: Some(vec![CommandStep::Shell("echo \"Hello World\"".to_string())]),
				ignore_errors: None,
				when: None,
				condition_ast: None,
			})]),
			ignore_errors: None,
			when: None,
			condition_ast: None,
		})]),
		ignore_errors: None,
		when: None,
		condition_ast: None,
	};
	let mut hello_spec = CommandSpec::new(vec![CommandStep::If(if_powershell)]);
	hello_spec.description = Some("Say Hello World".to_string());

	let mut targets = HashMap::new();
	targets.insert("hello".to_string(), hello_spec);

	let runfile = Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".to_string(),
		includes: None,
		targets,
		globals: Some(Globals::default()),
	};

	write_runfile_to_path(&runfile, &output_path);
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
			globals.working_directory = Some(WORKING_DIRECTORY_CWD.to_string());
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

pub fn cmd_generate_vscode_tasks(file: Option<&std::path::Path>) {
	use runfile_generators::{generate_vscode_tasks, merge_vscode_tasks, VsCodeTasksFile};

	let runfile_path = resolve_runfile_path(file);

	let runfile = match parse_runfile_from_path(&runfile_path) {
		Ok(r) => r,
		Err(e) => {
			eprintln!("Error parsing {}: {e}", runfile_path.display());
			process::exit(1);
		}
	};

	let tasks_path = PathBuf::from(".vscode/tasks.json");

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

	if result.updated.is_empty() && result.added.is_empty() {
		println!("No tasks to generate.");
		return;
	}

	if let Some(parent) = tasks_path.parent() {
		std::fs::create_dir_all(parent).unwrap_or_else(|e| {
			eprintln!("Error creating {}: {e}", parent.display());
			process::exit(1);
		});
	}

	let json = serde_json::to_string_pretty(&existing).unwrap_or_else(|e| {
		eprintln!("Error serializing tasks: {e}");
		process::exit(1);
	});

	std::fs::write(&tasks_path, json.as_bytes()).unwrap_or_else(|e| {
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
}

pub fn cmd_generate_zed_tasks(file: Option<&std::path::Path>) {
	use runfile_generators::{generate_zed_tasks, merge_zed_tasks, ZedTask};

	let runfile_path = resolve_runfile_path(file);

	let runfile = match parse_runfile_from_path(&runfile_path) {
		Ok(r) => r,
		Err(e) => {
			eprintln!("Error parsing {}: {e}", runfile_path.display());
			process::exit(1);
		}
	};

	let tasks_path = PathBuf::from(".zed/tasks.json");

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

	if result.updated.is_empty() && result.added.is_empty() {
		println!("No tasks to generate.");
		return;
	}

	if let Some(parent) = tasks_path.parent() {
		std::fs::create_dir_all(parent).unwrap_or_else(|e| {
			eprintln!("Error creating {}: {e}", parent.display());
			process::exit(1);
		});
	}

	let json = serde_json::to_string_pretty(&existing_tasks).unwrap_or_else(|e| {
		eprintln!("Error serializing tasks: {e}");
		process::exit(1);
	});

	std::fs::write(&tasks_path, json.as_bytes()).unwrap_or_else(|e| {
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
}

pub fn cmd_generate_jetbrains_run_configs(file: Option<&std::path::Path>, output_dir: Option<&std::path::Path>) {
	use runfile_generators::{check_existing_jetbrains_config, generate_jetbrains_configs, JetBrainsConfigCheck};

	let runfile_path = resolve_runfile_path(file);

	let runfile = match parse_runfile_from_path(&runfile_path) {
		Ok(r) => r,
		Err(e) => {
			eprintln!("Error parsing {}: {e}", runfile_path.display());
			process::exit(1);
		}
	};

	let run_dir = output_dir.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".run"));

	std::fs::create_dir_all(&run_dir).unwrap_or_else(|e| {
		eprintln!("Error creating {}: {e}", run_dir.display());
		process::exit(1);
	});

	let configs = generate_jetbrains_configs(&runfile);

	let mut added: Vec<String> = Vec::new();
	let mut updated: Vec<String> = Vec::new();
	let mut skipped: Vec<(String, String)> = Vec::new();

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

		std::fs::write(&file_path, config.xml.as_bytes()).unwrap_or_else(|e| {
			eprintln!("Error writing {}: {e}", file_path.display());
			process::exit(1);
		});
	}

	if added.is_empty() && updated.is_empty() && skipped.is_empty() {
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
	if !skipped.is_empty() {
		println!();
		eprintln!("  Skipped (existing file with different configuration):");
		for (file_name, reason) in &skipped {
			eprintln!("    {file_name}: {reason}");
		}
	}
}
