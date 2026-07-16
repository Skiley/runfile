use runfile_settings::Settings;
use runfile_shell::{ResolvedShell, detect_default_shell, resolve_shell, resolve_shell_from_path};
use std::process;

pub fn resolve_shell_for_runfile(command_shell: Option<&str>, settings: &Settings) -> ResolvedShell {
	let force_shell = command_shell;

	if let Some(shell_name) = force_shell {
		if let Some(custom_path) = settings.get_shell_path(shell_name) {
			match resolve_shell_from_path(shell_name, custom_path.clone()) {
				Ok(shell) => return shell,
				Err(e) => {
					eprintln!("Warning: custom shell path failed ({e}), trying default locations...");
				}
			}
		}

		match resolve_shell(shell_name) {
			Ok(shell) => return shell,
			Err(e) => {
				eprintln!("Error: {e}");
				eprintln!("Use `run :config shell set {shell_name} /path/to/shell` to configure the shell path.");
				process::exit(1);
			}
		}
	}

	match detect_default_shell() {
		Ok(shell) => shell,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	}
}
