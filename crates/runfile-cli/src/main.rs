use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

mod agent_detect;
mod ci_detect;
mod cmd_config;
mod cmd_env;
mod cmd_mcp;
mod cmd_run;
mod cmd_utilities;
mod completions;
mod runfile_helpers;
mod shell;
#[cfg(test)]
mod tests;

#[derive(Parser)]
#[command(
	name = "run",
	about = "Runfile — a modern, cross-platform command runner",
	version,
	disable_help_subcommand = true
)]
pub struct Cli {
	/// Show what would be executed without running anything (like make -n)
	#[arg(long = "dry-run")]
	dry_run: bool,

	/// Prompt for any missing {{ ARGS.x }} / {{ ENV.X }} / {{ FLAGS.x }} values via stdin
	/// instead of failing. Substitutions with defaults are also prompted —
	/// pressing Enter accepts the default; required values without a default
	/// must be supplied or the run fails.
	#[arg(long = "stdin-args")]
	stdin_args: bool,

	/// Path to a specific Runfile to use instead of auto-discovery
	#[arg(short = 'f', long = "file")]
	file: Option<PathBuf>,

	/// Print execution time for each command and target
	#[arg(long = "timings")]
	timings: bool,

	/// Auto-confirm all prompts (skip interactive confirmation)
	#[arg(short = 'y', long = "yes")]
	yes: bool,

	/// List target names for shell completion (one per line)
	#[arg(long = "list-targets", hide = true)]
	list_targets: bool,

	/// List subcommand names for shell completion (one per line, tab-separated with description).
	/// Accepts a dot-separated path to navigate the subcommand tree (e.g. "config.shell").
	#[arg(long = "list-subcommands", hide = true, default_missing_value = "", num_args = 0..=1)]
	list_subcommands: Option<String>,

	#[command(subcommand)]
	subcommand: Option<Commands>,

	/// The target name to run from Runfile.json, followed by arguments
	#[arg(trailing_var_arg = true)]
	args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
	/// Shell completion scripts
	#[command(name = ":completions")]
	Completions {
		#[command(subcommand)]
		action: CompletionsAction,
	},
	/// Manage Runfile local settings
	#[command(name = ":config")]
	Config {
		/// Print the path to the settings file and exit
		#[arg(long)]
		path: bool,

		#[command(subcommand)]
		action: Option<ConfigAction>,
	},
	/// Convert external task definitions into Runfile targets
	#[command(name = ":convert")]
	Convert {
		#[command(subcommand)]
		action: ConvertAction,
	},
	/// Manage encrypted environment variables and secret keys
	#[command(name = ":env")]
	Env {
		#[command(subcommand)]
		action: EnvAction,
	},
	/// Generate editor integration files from Runfile targets
	#[command(name = ":generate")]
	Generate {
		#[command(subcommand)]
		action: GenerateAction,
	},
	/// Create a default Runfile.json in the current directory
	#[command(name = ":init")]
	Init {
		/// Path to write the Runfile.json (defaults to ./Runfile.json)
		#[arg(short = 'p', long = "path")]
		path: Option<PathBuf>,
	},
	/// List all available targets in the current Runfile.json
	#[command(name = ":list")]
	List {
		/// Path to a specific Runfile to use instead of auto-discovery
		#[arg(short = 'f', long = "file")]
		file: Option<PathBuf>,
	},
	/// MCP (Model Context Protocol) server for exposing Runfile targets as tools
	#[command(name = ":mcp")]
	Mcp {
		/// Path to a specific Runfile to use instead of auto-discovery
		#[arg(short = 'f', long = "file")]
		file: Option<PathBuf>,

		#[command(subcommand)]
		action: Option<McpAction>,
	},
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum McpAction {
	/// Output the tool definitions as JSON and exit
	Inspect,
	/// Install the MCP server configuration for an agent
	Install {
		/// Agent name (claude-code, cursor, claude-desktop, codex, junie)
		#[arg(default_value = "")]
		agent: String,
	},
	/// Start the MCP server on stdio
	Server,
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum CompletionsAction {
	/// Install completion scripts for a shell
	Install {
		/// Shell to install completions for (bash, zsh, fish, powershell)
		shell: String,
	},
	/// Output completion script to stdout (for eval or manual installation)
	Output {
		/// Shell to generate completions for (bash, zsh, fish, powershell)
		shell: String,
	},
	/// Remove previously installed completion scripts
	Uninstall {
		/// Shell to uninstall completions for (bash, zsh, fish, powershell)
		shell: String,
	},
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum GenerateAction {
	/// Generate JetBrains (IntelliJ, CLion, etc.) run configurations from Runfile targets
	JetbrainsRunConfigurations {
		/// Path to the Runfile.json (defaults to auto-discovery)
		#[arg(short = 'f', long = "file")]
		file: Option<PathBuf>,
		/// Directory to write .xml run configurations to (defaults to .run)
		#[arg(short = 'o', long = "output-dir")]
		output_dir: Option<PathBuf>,
	},
	/// Generate VS Code tasks from Runfile targets
	VscodeTasks {
		/// Path to the Runfile.json (defaults to auto-discovery)
		#[arg(short = 'f', long = "file")]
		file: Option<PathBuf>,
	},
	/// Generate Zed editor tasks from Runfile targets
	ZedTasks {
		/// Path to the Runfile.json (defaults to auto-discovery)
		#[arg(short = 'f', long = "file")]
		file: Option<PathBuf>,
	},
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum ConvertAction {
	/// Convert targets from a Makefile into Runfile targets
	Makefile {
		/// Path to the Makefile (defaults to ./Makefile)
		#[arg(short = 'p', long = "path")]
		path: Option<PathBuf>,
	},
	/// Convert scripts from a package.json into Runfile targets
	PackageJson {
		/// Path to the package.json file (defaults to ./package.json)
		#[arg(short = 'p', long = "path")]
		path: Option<PathBuf>,
	},
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum ConfigAction {
	/// Manage global Runfile.json files that are always merged with the local Runfile
	GlobalFiles {
		#[command(subcommand)]
		action: GlobalFilesAction,
	},
	/// Manage path aliases for -f/--file
	PathAlias {
		#[command(subcommand)]
		action: PathAliasAction,
	},
	/// Delete the settings file, resetting all configuration to defaults
	Reset,
	/// Manage custom shell paths
	Shell {
		#[command(subcommand)]
		action: ShellAction,
	},
}

#[derive(Subcommand)]
enum GlobalFilesAction {
	/// Register a Runfile.json as a global file
	Add {
		/// Path to the Runfile.json to register
		path: PathBuf,
	},
	/// List all registered global files
	List,
	/// Unregister a global file (supports partial match)
	Remove {
		/// Path (or unique substring) of the global file to remove
		path: String,
	},
}

#[derive(Subcommand)]
enum ShellAction {
	/// List all shells with their resolved paths and availability
	List,
	/// Set a custom shell path in local settings
	Set {
		/// Shell name (bash, zsh, sh, fish, powershell, cmd)
		name: String,
		/// Path to the shell executable
		path: PathBuf,
	},
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum EnvAction {
	/// Decrypt an encrypted env file (prints to stdout if no output path is given)
	Decrypt {
		/// Source encrypted .env file
		source: String,
		/// Output plaintext .env file (omit to print to stdout)
		output: Option<String>,
	},
	/// Encrypt a plaintext env file into a new encrypted file
	Encrypt {
		/// Source plaintext .env file
		source: String,
		/// Output encrypted .env file
		output: String,
		/// Public key (or prefix) to identify which key to encrypt with
		#[arg(name = "PUBLIC_KEY")]
		secret_key: String,
	},
	/// Read a variable from an env file (auto-decrypts if encrypted)
	Get {
		/// Path to the .env file
		file: String,
		/// Variable name to read
		var: String,
	},
	/// Run a command with environment variables loaded from one or more .env files
	Inject {
		/// Path to a .env file (can be specified multiple times; defaults to .env).
		/// Files are merged in order — later files override earlier ones.
		#[arg(short = 'f', long = "file")]
		file: Vec<String>,

		/// The command to run, followed by its arguments. Use `--` to separate from flags.
		#[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
		command: Vec<String>,
	},
	/// Create a new .env file, optionally encrypted
	Init {
		/// Public key (or prefix) to identify which key to encrypt with.
		/// If omitted and encryption is enabled, a new key is generated automatically.
		#[arg(long = "key")]
		key: Option<String>,

		/// Path to the .env file (defaults to .env)
		#[arg(short = 'p', long = "path", default_value = ".env")]
		path: String,

		/// Create a plaintext .env file (no encryption)
		#[arg(long = "plain")]
		plain: bool,
	},
	/// Rotate the encryption key for an encrypted env file
	Rotate {
		/// Path to the encrypted .env file
		file: String,
		/// Also delete the old private key from the OS credential store
		#[arg(long = "delete-current-key")]
		delete_current_key: bool,
	},
	/// Manage private encryption keys stored in user settings
	SecretKeys {
		#[command(subcommand)]
		action: SecretKeysAction,
	},
	/// Set a variable in an env file (auto-encrypts if file is encrypted).
	/// If VALUE is omitted, the value is read from stdin (until EOF) — useful
	/// for keeping secrets out of shell history and for passing values that
	/// contain shell-special characters like `$` or `!` without escaping.
	Set {
		/// Path to the .env file
		file: String,
		/// Variable name
		var: String,
		/// Value to set (omit to read from stdin)
		value: Option<String>,
		/// Store the value as plaintext even if the file is encrypted
		#[arg(long = "plain")]
		plain: bool,
	},
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum SecretKeysAction {
	/// Generate a new key or import an existing private encryption key (interactive,
	/// unless `--key` is given for non-interactive CI use).
	Add {
		/// Add a known private key non-interactively. CI-only — refused on dev machines
		/// to avoid leaking the key into shell history. Detection uses standard CI env
		/// vars (`CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, etc.).
		#[arg(long = "key")]
		key: Option<String>,
	},
	/// Print the full private key for a given public key prefix (for sharing with teammates)
	GetPrivate {
		/// Public key hex prefix (partial match)
		partial: String,
	},
	/// List all stored private keys with their public key fingerprints
	List,
	/// Remove a key by public key prefix
	Remove {
		/// Public key hex prefix (partial match)
		partial: String,
	},
}

#[derive(Subcommand)]
enum PathAliasAction {
	/// Add a path alias for use with -f/--file
	Add {
		/// Alias name (e.g. "root", "globals")
		alias: String,
		/// Path to the Runfile
		path: PathBuf,
	},
	/// List all saved path aliases
	List,
	/// Remove a path alias (supports partial match)
	Remove {
		/// Alias name (or unique substring) to remove
		alias: String,
	},
}

fn main() {
	let cli = Cli::parse();

	// Hidden flag for shell completion scripts — must be handled before subcommands
	if cli.list_targets {
		completions::cmd_list_targets(cli.file.as_deref());
		return;
	}

	if let Some(path) = &cli.list_subcommands {
		completions::cmd_list_subcommands(path);
		return;
	}

	match cli.subcommand {
		Some(Commands::List { file }) => cmd_run::cmd_list(file.as_deref().or(cli.file.as_deref())),
		Some(Commands::Config { path, action }) => {
			if path {
				cmd_config::cmd_config_path();
				return;
			}
			match action {
				Some(ConfigAction::Shell { action }) => match action {
					ShellAction::Set { name, path } => cmd_config::cmd_set_shell(&name, path),
					ShellAction::List => cmd_config::cmd_list_shells(),
				},
				Some(ConfigAction::PathAlias { action }) => match action {
					PathAliasAction::Add { alias, path } => cmd_config::cmd_add_path_alias(&alias, path),
					PathAliasAction::Remove { alias } => cmd_config::cmd_remove_path_alias(&alias),
					PathAliasAction::List => cmd_config::cmd_list_path_aliases(),
				},
				Some(ConfigAction::Reset) => cmd_config::cmd_reset(),
				Some(ConfigAction::GlobalFiles { action }) => match action {
					GlobalFilesAction::Add { path } => cmd_config::cmd_add_global_file(path),
					GlobalFilesAction::Remove { path } => cmd_config::cmd_remove_global_file(&path),
					GlobalFilesAction::List => cmd_config::cmd_list_global_files(),
				},
				None => {
					use clap::CommandFactory;
					let mut cmd = Cli::command();
					for sub in cmd.get_subcommands_mut() {
						if sub.get_name() == ":config" {
							sub.print_help().ok();
							println!();
							process::exit(0);
						}
					}
				}
			}
		}
		Some(Commands::Mcp { file, action }) => {
			let file = file.as_deref().or(cli.file.as_deref());
			match action {
				Some(McpAction::Install { agent }) => cmd_mcp::cmd_mcp_install(file, &agent),
				Some(McpAction::Inspect) => cmd_mcp::cmd_mcp_inspect(file),
				Some(McpAction::Server) => cmd_mcp::cmd_mcp_server(file),
				None => {
					use clap::CommandFactory;
					let mut cmd = Cli::command();
					for sub in cmd.get_subcommands_mut() {
						if sub.get_name() == ":mcp" {
							sub.print_help().ok();
							println!();
							process::exit(0);
						}
					}
				}
			}
		}
		Some(Commands::Completions { action }) => match action {
			CompletionsAction::Install { shell } => completions::cmd_completions_install(&shell),
			CompletionsAction::Uninstall { shell } => completions::cmd_completions_uninstall(&shell),
			CompletionsAction::Output { shell } => completions::cmd_completions_output(&shell),
		},
		Some(Commands::Generate { action }) => match action {
			GenerateAction::ZedTasks { file } => cmd_utilities::cmd_generate_zed_tasks(file.as_deref()),
			GenerateAction::JetbrainsRunConfigurations { file, output_dir } => {
				cmd_utilities::cmd_generate_jetbrains_run_configs(file.as_deref(), output_dir.as_deref())
			}
			GenerateAction::VscodeTasks { file } => cmd_utilities::cmd_generate_vscode_tasks(file.as_deref()),
		},
		Some(Commands::Convert { action }) => match action {
			ConvertAction::Makefile { path } => cmd_utilities::cmd_convert_makefile(path),
			ConvertAction::PackageJson { path } => cmd_utilities::cmd_convert_package_json(path),
		},
		Some(Commands::Env { action }) => match action {
			EnvAction::Init { path, plain, key } => cmd_env::cmd_init(&path, plain, key.as_deref()),
			EnvAction::Inject { file, command } => cmd_env::cmd_inject(&file, &command),
			EnvAction::SecretKeys { action } => match action {
				SecretKeysAction::Add { key } => cmd_env::cmd_secret_keys_add(key.as_deref()),
				SecretKeysAction::List => cmd_env::cmd_secret_keys_list(),
				SecretKeysAction::GetPrivate { partial } => cmd_env::cmd_get_private_key(&partial),
				SecretKeysAction::Remove { partial } => cmd_env::cmd_secret_keys_remove(&partial),
			},
			EnvAction::Get { file, var } => cmd_env::cmd_get(&file, &var),
			EnvAction::Set {
				file,
				var,
				value,
				plain,
			} => cmd_env::cmd_set(&file, &var, value.as_deref(), plain),
			EnvAction::Decrypt { source, output } => cmd_env::cmd_decrypt_file(&source, output.as_deref()),
			EnvAction::Encrypt {
				source,
				output,
				secret_key,
			} => cmd_env::cmd_encrypt_file(&source, &output, &secret_key),
			EnvAction::Rotate {
				file,
				delete_current_key,
			} => cmd_env::cmd_rotate(&file, delete_current_key),
		},
		Some(Commands::Init { path }) => cmd_utilities::cmd_init(path),
		None => {
			if cli.args.is_empty() {
				use clap::CommandFactory;
				Cli::command().print_help().unwrap();
				println!();
				process::exit(0);
			} else {
				let target_name = &cli.args[0];
				let extra_args: Vec<String> = cli.args[1..].to_vec();
				if cli.dry_run {
					cmd_run::cmd_dry_run(target_name, &extra_args, cli.file.as_deref(), cli.stdin_args);
				} else {
					cmd_run::cmd_run(
						target_name,
						&extra_args,
						cli.file.as_deref(),
						cli.timings,
						cli.yes,
						cli.stdin_args,
					);
				}
			}
		}
	}
}
