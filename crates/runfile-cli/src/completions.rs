use clap::CommandFactory;
use runfile_parser::{discover_runfile_cwd, merge_runfiles_silent, parse_runfile_from_path};
use runfile_settings::Settings;
use std::path::PathBuf;
use std::process;

use crate::runfile_helpers::runfile_target_env;
use crate::Cli;

/// Print target names (one per line) for shell completion scripts.
/// Never writes to stderr or exits with non-zero — errors are silently ignored.
pub fn cmd_list_targets(file: Option<&std::path::Path>) {
	let settings = Settings::load().unwrap_or_default();
	let cwd = std::env::current_dir().unwrap_or_default();

	// Resolve local Runfile silently. When `-f` is absent, fall back to the
	// `RUNFILE_TARGET` env var before auto-discovery so completions match the
	// runtime resolution.
	let env_target = if file.is_none() { runfile_target_env() } else { None };
	let effective: Option<&std::path::Path> = file.or(env_target.as_deref());

	let local = if let Some(f) = effective {
		let path = if std::path::Path::new(f).is_file() {
			f.to_path_buf()
		} else {
			let alias_name = f.to_string_lossy();
			match settings.get_path_alias(&alias_name) {
				Some(p) if p.is_file() => p.clone(),
				_ => return,
			}
		};
		match parse_runfile_from_path(&path) {
			Ok(r) => Some((r, path)),
			Err(_) => return,
		}
	} else {
		match discover_runfile_cwd() {
			Ok(p) => match parse_runfile_from_path(&p) {
				Ok(r) => Some((r, p)),
				Err(_) => None,
			},
			Err(_) => None,
		}
	};

	let merged = match merge_runfiles_silent(local, &settings.global_files, &cwd) {
		Ok(m) => m,
		Err(_) => return,
	};

	let names = merged.runfile.public_target_names();
	for name in names {
		// Exclude conflicting targets from completions
		if merged.conflicts.contains_key(name) {
			continue;
		}
		println!("{name}");
	}
}

pub fn cmd_list_subcommands(path: &str) {
	let cmd = Cli::command();
	let target = if path.is_empty() {
		&cmd
	} else {
		let mut current = &cmd;
		for part in path.split('.') {
			match current.find_subcommand(part) {
				Some(sub) => current = sub,
				None => return,
			}
		}
		current
	};
	for sub in target.get_subcommands() {
		if sub.is_hide_set() {
			continue;
		}
		let name = sub.get_name();
		if let Some(about) = sub.get_about() {
			println!("{name}\t{about}");
		} else {
			println!("{name}");
		}
	}
}

fn validate_shell(shell: &str) {
	let key = shell.to_lowercase();
	if !matches!(
		key.as_str(),
		"bash" | "zsh" | "fish" | "powershell" | "pwsh" | "cmd" | "cmd.exe"
	) {
		eprintln!("Unknown shell \"{shell}\". Supported: bash, zsh, fish, powershell");
		process::exit(1);
	}
	if matches!(key.as_str(), "cmd" | "cmd.exe") {
		eprintln!("cmd.exe does not support programmable completions.");
		eprintln!("Use PowerShell instead: run :completions install powershell");
		process::exit(1);
	}
}

fn completion_script(shell: &str) -> &str {
	match shell.to_lowercase().as_str() {
		"zsh" => ZSH_COMPLETION,
		"bash" => BASH_COMPLETION,
		"fish" => FISH_COMPLETION,
		"powershell" | "pwsh" => POWERSHELL_COMPLETION,
		_ => unreachable!(),
	}
}

pub fn cmd_completions_install(shell: &str) {
	validate_shell(shell);
	match shell.to_lowercase().as_str() {
		"bash" => completions_install_bash(),
		"zsh" => completions_install_zsh(),
		"fish" => completions_install_fish(completion_script(shell)),
		"powershell" | "pwsh" => completions_install_profile(
			completion_script(shell),
			&completions_detect_powershell_profile(),
			"# runfile completions",
		),
		_ => unreachable!(),
	}
}

pub fn cmd_completions_uninstall(shell: &str) {
	validate_shell(shell);
	completions_uninstall(shell);
}

pub fn cmd_completions_output(shell: &str) {
	validate_shell(shell);
	print!("{}", completion_script(shell));
}

fn completions_install_bash() {
	let home = home_dir_string();
	let bashrc = PathBuf::from(format!("{}/.bashrc", home));
	completions_install_profile(
		"eval \"$(run :completions output bash)\"",
		&bashrc,
		"# runfile completions",
	);
	println!("Completions installed to {}", bashrc.display());
}

fn completions_install_zsh() {
	let home = home_dir_string();
	let zshrc = PathBuf::from(format!("{}/.zshrc", home));
	completions_install_profile(
		"eval \"$(run :completions output zsh)\"",
		&zshrc,
		"# runfile completions",
	);
	println!("Completions installed to {}", zshrc.display());
	println!("Restart your shell or run: exec zsh");
}

fn completions_install_fish(script: &str) {
	let config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
		let home = home_dir_string();
		format!("{}/.config", home)
	});
	let dir_path = PathBuf::from(format!("{}/fish/completions", config));
	let _ = std::fs::create_dir_all(&dir_path);

	let file_path = dir_path.join("run.fish");
	if let Err(e) = std::fs::write(&file_path, script) {
		eprintln!("Error writing {}: {e}", file_path.display());
		process::exit(1);
	}

	println!("Completions installed to {}", file_path.display());
}

/// Append a block to a shell profile file (bashrc, PowerShell $PROFILE, etc).
pub fn completions_install_profile(content: &str, profile_path: &std::path::Path, marker: &str) {
	let existing = std::fs::read_to_string(profile_path).unwrap_or_default();

	if existing.contains(marker) {
		println!("Completions already installed in {}", profile_path.display());
		println!("Use `run :completions uninstall` first to reinstall.");
		return;
	}

	if let Some(parent) = profile_path.parent() {
		let _ = std::fs::create_dir_all(parent);
	}

	let mut out = existing;
	if !out.is_empty() && !out.ends_with('\n') {
		out.push('\n');
	}
	out.push('\n');
	out.push_str(marker);
	out.push('\n');
	out.push_str(content);
	if !content.ends_with('\n') {
		out.push('\n');
	}

	if let Err(e) = std::fs::write(profile_path, out) {
		eprintln!("Error writing {}: {e}", profile_path.display());
		process::exit(1);
	}
}

fn completions_detect_powershell_profile() -> PathBuf {
	if let Ok(p) = std::env::var("PROFILE") {
		return PathBuf::from(p);
	}
	if cfg!(windows) {
		if let Some(docs) = dirs::document_dir() {
			return docs.join("PowerShell").join("Microsoft.PowerShell_profile.ps1");
		}
	} else if let Some(home) = dirs::home_dir() {
		return home.join(".config/powershell/profile.ps1");
	}
	eprintln!("Cannot determine PowerShell profile path.");
	eprintln!("Print the script manually: run :completions output powershell");
	process::exit(1);
}

fn completions_uninstall(shell: &str) {
	match shell.to_lowercase().as_str() {
		"bash" => completions_uninstall_bash(),
		"zsh" => {
			let home = home_dir_string();
			let zshrc = PathBuf::from(format!("{}/.zshrc", home));
			completions_uninstall_profile(&zshrc, "# runfile completions");
		}
		"fish" => {
			let config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
				let home = home_dir_string();
				format!("{}/.config", home)
			});
			completions_uninstall_file(&PathBuf::from(format!("{}/fish/completions/run.fish", config)));
		}
		"powershell" | "pwsh" => {
			let profile = completions_detect_powershell_profile();
			completions_uninstall_profile(&profile, "# runfile completions");
		}
		_ => {
			eprintln!("Unknown shell \"{shell}\"");
			process::exit(1);
		}
	}
}

fn completions_uninstall_bash() {
	let home = home_dir_string();
	let bashrc = PathBuf::from(format!("{}/.bashrc", home));
	completions_uninstall_profile(&bashrc, "# runfile completions");
}

fn completions_uninstall_file(path: &std::path::Path) {
	if path.is_file() {
		if let Err(e) = std::fs::remove_file(path) {
			eprintln!("Error removing {}: {e}", path.display());
			process::exit(1);
		}
		println!("Completions removed from {}", path.display());
	} else {
		println!("No completions found at {}", path.display());
	}
}

/// Remove a marked block from a profile file (bashrc, PowerShell $PROFILE, etc).
pub fn completions_uninstall_profile(profile: &std::path::Path, marker: &str) {
	let content = match std::fs::read_to_string(profile) {
		Ok(c) => c,
		Err(_) => {
			println!("No completions found (no profile at {})", profile.display());
			return;
		}
	};

	if !content.contains(marker) {
		println!("No runfile completions found in {}", profile.display());
		return;
	}

	// Remove from marker line to end of block (everything from marker onward)
	let lines: Vec<&str> = content.lines().collect();
	let mut start = None;
	for (i, line) in lines.iter().enumerate() {
		if line.contains(marker) {
			// Include blank line before marker if present
			start = if i > 0 && lines[i - 1].trim().is_empty() {
				Some(i - 1)
			} else {
				Some(i)
			};
			break;
		}
	}

	if let Some(s) = start {
		let kept: Vec<&str> = lines[..s].to_vec();
		let new_content = kept.join("\n");
		let final_content = if new_content.is_empty() {
			new_content
		} else {
			new_content + "\n"
		};
		if let Err(e) = std::fs::write(profile, final_content) {
			eprintln!("Error writing {}: {e}", profile.display());
			process::exit(1);
		}
		println!("Completions removed from {}", profile.display());
	}
}

pub fn home_dir_string() -> String {
	dirs::home_dir()
		.map(|p| p.to_string_lossy().to_string())
		.unwrap_or_else(|| {
			std::env::var("HOME")
				.or_else(|_| std::env::var("USERPROFILE"))
				.unwrap_or_else(|_| "~".to_string())
		})
}

// ── Completion Scripts ───────────────────────────────────────────────

pub const BASH_COMPLETION: &str = r#"# Remove ':' from COMP_WORDBREAKS so that readline treats colon-prefixed tokens
# like ":config" as single words.  This is set once at script load — the same
# approach used by npm, nvm, and other tools whose completions contain colons.
COMP_WORDBREAKS="${COMP_WORDBREAKS//:/}"

_run_completions() {
    local cur prev words cword
    if declare -F _init_completion >/dev/null 2>&1; then
        _init_completion || return
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"
        words=("${COMP_WORDS[@]}")
        cword=$COMP_CWORD
    fi

    # Helper: get subcommand names from --list-subcommands (strips descriptions)
    _run_subcmds() {
        run --list-subcommands "$1" 2>/dev/null | cut -f1
    }
    # Helper: fall back to the shell's default file/directory completion.
    # Prefer bash-completion's _filedir when present, but some versions return
    # nothing for an empty current word — fall back to compgen in that case so
    # `run :env decrypt <TAB>` still lists files.
    _run_files() {
        COMPREPLY=()
        if declare -F _filedir >/dev/null 2>&1; then
            _filedir
        fi
        if [[ ${#COMPREPLY[@]} -eq 0 ]]; then
            local IFS=$'\n'
            COMPREPLY=($(compgen -f -- "$cur"))
            compopt -o filenames 2>/dev/null
        fi
    }

    # Complete flag values
    case "$prev" in
        -f|--file)
            _run_files
            return ;;
    esac

    # Complete flags
    if [[ "$cur" == -* ]]; then
        COMPREPLY=($(compgen -W "-f --file -h --help --version" -- "$cur"))
        return
    fi

    # Collect the non-flag words already typed (the subcommand path so far),
    # skipping flags and the value of -f/--file.
    local -a consumed=()
    local i
    for ((i=1; i < cword; i++)); do
        case "${words[i]}" in
            -f|--file) ((i++)); continue ;;
            -*) continue ;;
            *) consumed+=("${words[i]}") ;;
        esac
    done

    # Top level (nothing consumed yet): subcommands + dynamic targets
    if [[ ${#consumed[@]} -eq 0 ]]; then
        local targets subcmds
        targets=$(run --list-targets 2>/dev/null)
        subcmds=$(_run_subcmds "")
        COMPREPLY=($(compgen -W "$subcmds $targets" -- "$cur"))
        return
    fi

    # A target name (not a ':' subcommand): its arguments are positional →
    # fall back to file completion.
    if [[ "${consumed[0]}" != :* ]]; then
        _run_files
        return
    fi

    # Build the dotted subcommand path (e.g. ":env decrypt" → ":env.decrypt")
    # and ask for its children. An empty result means we've reached a leaf
    # subcommand (or moved past it into positional args) — complete files.
    local path
    printf -v path '%s.' "${consumed[@]}"
    path="${path%.}"

    local kids
    kids=$(_run_subcmds "$path")
    if [[ -n "$kids" ]]; then
        COMPREPLY=($(compgen -W "$kids" -- "$cur"))
    else
        _run_files
    fi
}
complete -F _run_completions run
"#;

pub const ZSH_COMPLETION: &str = r#"# Helper: convert "name\tdesc" output from --list-subcommands into zsh "name:desc" array.
# Colons inside the name portion must be backslash-escaped for _describe to parse
# correctly (e.g. ":config" becomes "\:config").
_run_subcmds() {
    local -a result
    local line
    while IFS=$'\t' read -r name desc; do
        local escaped="${name//:/\\:}"
        if [[ -n "$desc" ]]; then
            result+=("${escaped}:${desc}")
        elif [[ -n "$name" ]]; then
            result+=("$escaped")
        fi
    done < <(run --list-subcommands "$1" 2>/dev/null)
    echo "${(F)result}"
}

_run() {
    _arguments -C \
        '-f[Path to Runfile]:file:_files' \
        '--file=[Path to Runfile]:file:_files' \
        '-h[Print help]' \
        '--help[Print help]' \
        '--version[Print version]' \
        '1:command:->first_arg' \
        '*::arg:->rest' && return

    case $state in
        first_arg)
            local -a subcommands dyn_targets
            subcommands=(${(f)"$(_run_subcmds)"})
            # Escape colons in target names so _describe doesn't misparse them
            # (e.g. "ci:build" would otherwise be read as name="ci" desc="build")
            local name
            while IFS= read -r name; do
                [[ -n "$name" ]] && dyn_targets+=("${name//:/\\:}")
            done < <(run --list-targets 2>/dev/null)
            _describe 'subcommand' subcommands -- dyn_targets && return
            ;;
        rest)
            local first="$words[1]"
            # A target name (not a ':' subcommand): its args are positional →
            # complete files.
            if [[ "$first" != :* ]]; then
                _files
                return
            fi
            # Build the dotted subcommand path from every non-flag word typed
            # before the cursor (e.g. ":env decrypt" → ":env.decrypt").
            local -a parts
            local idx
            for (( idx=1; idx < CURRENT; idx++ )); do
                [[ "$words[idx]" == -* ]] && continue
                parts+=("$words[idx]")
            done
            local path="${(j:.:)parts}"
            # Ask for the children of that path. Empty output means we've hit a
            # leaf subcommand (or moved past it into positional args) → files.
            local out
            out="$(_run_subcmds "$path")"
            if [[ -n "$out" ]]; then
                local -a cmds
                cmds=(${(f)out})
                _describe "$path command" cmds
            else
                _files
            fi
            ;;
    esac
}

# Ensure the completion system is initialised, then register.
autoload -Uz compinit && compinit -C 2>/dev/null
compdef _run run
"#;

pub const FISH_COMPLETION: &str = r#"# Disable file completions by default
complete -c run -f

# Returns success (→ re-enable file completion) when the cursor is at a
# positional-argument position: either a leaf subcommand that takes no further
# sub-subcommands (e.g. `:env decrypt <file>`) or a target's arguments.
function __run_needs_files
    set -l tokens (commandline -opc)
    set -l parts
    set -l skip 0
    for t in $tokens[2..-1]
        if test $skip -eq 1
            set skip 0
            continue
        end
        switch $t
            case '-f' '--file'
                set skip 1
            case '-*'
                # ignore other flags
            case '*'
                set -a parts $t
        end
    end
    # Nothing consumed yet → top level (subcommands + targets), no files
    test (count $parts) -gt 0; or return 1
    # A target name (not a ':' subcommand) → positional args → files
    string match -q -- ':*' $parts[1]; or return 0
    # Leaf subcommand (no children) → files; nodes with children → no files
    set -l kids (run --list-subcommands (string join '.' $parts) 2>/dev/null)
    test -z "$kids"
end

# Re-enable file completion for positional arguments
complete -c run -n '__run_needs_files' -F

# Subcommands (only when no subcommand is given yet)
complete -c run -n '__fish_use_subcommand' -a '(run --list-subcommands 2>/dev/null)'

# Dynamic targets (only when no subcommand is given yet)
complete -c run -n '__fish_use_subcommand' -a '(run --list-targets 2>/dev/null)'

# Global flags
complete -c run -s f -l file -d 'Path to Runfile' -r -F
complete -c run -l shell -d 'Override shell' -r -a 'bash zsh sh fish powershell cmd'
complete -c run -s h -l help -d 'Print help'
complete -c run -l version -d 'Print version'

# :config sub-subcommands
complete -c run -n '__fish_seen_subcommand_from :config; and not __fish_seen_subcommand_from shell path-alias reset global-files' -a '(run --list-subcommands :config 2>/dev/null)'

# :config sub-sub-subcommands (shell, path-alias, global-files)
complete -c run -n '__fish_seen_subcommand_from shell' -a '(run --list-subcommands :config.shell 2>/dev/null)'
complete -c run -n '__fish_seen_subcommand_from path-alias' -a '(run --list-subcommands :config.path-alias 2>/dev/null)'
complete -c run -n '__fish_seen_subcommand_from global-files' -a '(run --list-subcommands :config.global-files 2>/dev/null)'

# :mcp sub-subcommands
complete -c run -n '__fish_seen_subcommand_from :mcp; and not __fish_seen_subcommand_from server inspect install' -a '(run --list-subcommands :mcp 2>/dev/null)'

# :completions sub-subcommands
complete -c run -n '__fish_seen_subcommand_from :completions; and not __fish_seen_subcommand_from install uninstall output' -a '(run --list-subcommands :completions 2>/dev/null)'

# :generate sub-subcommands
complete -c run -n '__fish_seen_subcommand_from :generate; and not __fish_seen_subcommand_from zed-tasks jetbrains-run-configurations' -a '(run --list-subcommands :generate 2>/dev/null)'

# :convert sub-subcommands
complete -c run -n '__fish_seen_subcommand_from :convert; and not __fish_seen_subcommand_from makefile package-json' -a '(run --list-subcommands :convert 2>/dev/null)'

# :env sub-subcommands
complete -c run -n '__fish_seen_subcommand_from :env; and not __fish_seen_subcommand_from init secret-keys get get-private set decrypt encrypt' -a '(run --list-subcommands :env 2>/dev/null)'

# :env sub-sub-subcommands (secret-keys)
complete -c run -n '__fish_seen_subcommand_from secret-keys' -a '(run --list-subcommands :env.secret-keys 2>/dev/null)'
"#;

pub const POWERSHELL_COMPLETION: &str = r#"Register-ArgumentCompleter -CommandName run -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    # Helper: get subcommand names from --list-subcommands (strips descriptions)
    function Get-RunSubcmds($path) {
        $output = if ($path) { run --list-subcommands $path 2>$null } else { run --list-subcommands 2>$null }
        if ($output) { $output -split "`n" | ForEach-Object { ($_ -split "`t")[0] } | Where-Object { $_ } }
    }

    # Tokens typed before the cursor, excluding the command name ('run') and the
    # word currently being completed.
    $line = $commandAst.ToString()
    if ($cursorPosition -lt $line.Length) { $line = $line.Substring(0, $cursorPosition) }
    $tokens = @($line -split '\s+' | Where-Object { $_ -ne '' })
    # Drop the command name ('run').
    if ($tokens.Count -gt 0) { $tokens = @($tokens | Select-Object -Skip 1) }
    # If the cursor sits mid-word, drop that partial word from the consumed set.
    if (-not $line.EndsWith(' ') -and $tokens.Count -gt 0) {
        $tokens = @($tokens | Select-Object -SkipLast 1)
    }

    # Keep only non-flag words (the subcommand path so far), skipping -f/--file's value.
    $consumed = @()
    for ($i = 0; $i -lt $tokens.Count; $i++) {
        $t = $tokens[$i]
        if ($t -in '-f', '--file') { $i++; continue }
        if ($t -match '^-') { continue }
        $consumed += $t
    }

    # Top level: subcommands + dynamic targets
    if ($consumed.Count -eq 0) {
        $completions = @(Get-RunSubcmds)
        $targets = run --list-targets 2>$null
        if ($targets) { $completions += $targets -split "`n" }
        return $completions |
            Where-Object { $_ -like "$wordToComplete*" } |
            ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }
    }

    # A target name (not a ':' subcommand): its arguments are positional → files
    if ($consumed[0] -notlike ':*') {
        return [System.Management.Automation.CompletionCompleters]::CompleteFilename($wordToComplete)
    }

    # Build the dotted subcommand path and complete its children. No children
    # means a leaf subcommand (or past it into positional args) → files.
    $kids = @(Get-RunSubcmds ($consumed -join '.'))
    if ($kids.Count -gt 0) {
        return $kids |
            Where-Object { $_ -like "$wordToComplete*" } |
            ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }
    }

    return [System.Management.Automation.CompletionCompleters]::CompleteFilename($wordToComplete)
}
"#;
