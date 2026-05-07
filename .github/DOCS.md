# Runfile

**A modern, cross-platform command runner.** Define your project's tasks in a single `Runfile.json` and run them
anywhere — on any OS, in any shell.

Runfile is a replacement for Makefiles, shell scripts, and npm scripts. It's a single binary with no dependencies, built
in Rust for speed and portability.

```
$ run :list

Available targets (from Runfile.json):

  build                Build the project
  dev                  Start development server
  test                 Run all tests
  deploy               Deploy to production
```

```
$ run build --release
```

---

## Table of Contents

- [Why Runfile?](#why-runfile)
- [Quick Start](#quick-start)
- [CLI Usage](#cli-usage)
- [Runfile.json Reference](#runfilejson-reference)
- [Arguments and Substitution](#arguments-and-substitution)
- [Control Flow](#control-flow)
- [Shell Support](#shell-support)
- [Command Execution Model](#command-execution-model)
- [Environment Variables](#environment-variables)
- [Encrypted Environment Variables](#encrypted-environment-variables)
- [PATH Manipulation](#path-manipulation)
- [Command Logging](#command-logging)
- [Aliases](#aliases)
- [Internal Targets](#internal-targets)
- [When-guarded blocks](#when-guarded-blocks-when)
- [Dry Run](#dry-run)
- [File Includes](#file-includes)
- [Error Handling](#error-handling)
- [Parallel Execution](#parallel-execution)
- [Detached Execution](#detached-execution)
- [extendStdio](#extendstdio)
- [Force-kill on Ctrl+C](#force-kill-on-ctrlc)
- [Runfile Discovery](#runfile-discovery)
	- [Global Files](#global-files)
- [Local Settings](#local-settings)
- [Path Aliases](#path-aliases)
- [JSON Schema](#json-schema)
- [Shell Completions](#shell-completions)
- [Watch Mode](#watch-mode)
- [Confirmation Prompts](#confirmation-prompts)
- [MCP Server (AI Agents)](#mcp-server-ai-agents)
- [Bootstrapping a New Project](#bootstrapping-a-new-project)
- [Editor Integration](#editor-integration)
- [Full Example](#full-example)
- [Platform Support](#platform-support)

---

## Why Runfile?

- **Cross-platform** — Works on Linux, macOS, and Windows. No more `#!/bin/bash` scripts that break on Windows, or`.bat`
  files that break everywhere else.
- **Cross-shell** — Supports Bash, Zsh, Sh, Fish, PowerShell, and cmd.exe. You can run from any shell, and commands will
  be spawned in the appropriate shell. Automatically detects your shell, or lets you force a specific one.
- **Simple format** — `Runfile.json` is plain JSON. Your editor already has syntax highlighting, validation, and
  autocomplete (with the included JSON Schema).
- **No dependencies** — Single binary. No runtime, no interpreter, no package manager.
- **Argument passing** — Built-in substitution syntax for positional and named arguments with defaults.

---

## Quick Start

Create a `Runfile.json` in your project root:

```json
{
	"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
	"targets": {
		"build": {
			"description": "Build the project",
			"commands": [
				"cargo build --release"
			]
		},
		"test": {
			"description": "Run tests",
			"commands": [
				"cargo test"
			]
		},
		"dev": {
			"description": "Start dev server",
			"commands": [
				"echo Starting development server...",
				"npm run dev"
			],
			"env": {
				"PORT": 3000,
				"NODE_ENV": "development"
			}
		}
	}
}
```

Run a target:

```
$ run build
$ run test
$ run dev
```

Run with no arguments or `run :list` to see all available targets.

---

## CLI Usage

```
run [OPTIONS] [TARGET] [ARGS...]
run [SUBCOMMAND]
```

### Running targets

```
$ run build                     # Run the "build" target
$ run build --release           # Pass arguments to the target
$ run dev --port=4000           # Named arguments
```

### Subcommands

| Command                                               | Description                                                                                        |
|-------------------------------------------------------|----------------------------------------------------------------------------------------------------|
| `run :list`                                           | List all targets with their descriptions                                                           |
| `run :init [-p path]`                                 | Create a default `Runfile.json` in the current directory                                           |
| `run --dry-run <target> [args...]`                    | Print the resolved shell commands for a target without running them                                |
| `run :config shell set <name> <path>`                 | Save a custom shell executable path to local settings                                              |
| `run :config shell list`                              | Show all shells with their resolved paths and availability                                         |
| `run :config path-alias add <alias> <path>`           | Save a path alias for use with `-f`                                                                |
| `run :config path-alias remove <alias>`               | Remove a path alias (supports partial match)                                                       |
| `run :config path-alias list`                         | List all saved path aliases                                                                        |
| `run :config global-files add <path>`                 | Register a Runfile as a global file (always merged in)                                             |
| `run :config global-files remove <path>`              | Remove a registered global file (supports partial match)                                           |
| `run :config global-files list`                       | List all registered global files                                                                   |
| `run :config reset`                                   | Delete the settings file, resetting all configuration to defaults                                  |
| `run :convert package-json`                           | Convert `package.json` scripts into Runfile targets                                                |
| `run :convert makefile`                               | Convert Makefile targets into Runfile targets                                                      |
| `run :generate zed-tasks`                             | Generate Zed editor tasks from Runfile targets                                                     |
| `run :generate vscode-tasks`                          | Generate VS Code tasks from Runfile targets                                                        |
| `run :generate jetbrains-run-configurations`          | Generate JetBrains IDE run configurations from Runfile targets                                     |
| `run :mcp inspect`                                    | Print the MCP tool definitions as JSON and exit                                                    |
| `run :mcp install [<agent>]`                          | Install the MCP server config for an agent (claude-code, cursor, claude-desktop, codex, junie)     |
| `run :mcp server`                                     | Start the MCP server on stdio                                                                      |
| `run :completions install <shell>`                    | Install the completion script for a shell (bash, zsh, fish, powershell)                            |
| `run :completions output <shell>`                     | Print the completion script to stdout (for `eval` or manual install)                               |
| `run :completions uninstall <shell>`                  | Remove a previously installed completion script                                                    |
| `run :env init [-p path] [--plain] [--key prefix]`    | Create a new `.env` file, optionally encrypted                                                     |
| `run :env inject [-f file]... -- <command> [args...]` | Run a command with env vars loaded from one or more `.env` files (encrypted values auto-decrypted) |
| `run :env rotate <file> [--delete-current-key]`       | Rotate the encryption key for an encrypted `.env` file                                             |
| `run :env secret-keys add`                            | Interactively generate a new key or import an existing one                                         |
| `run :env secret-keys list`                           | List the public key fingerprints of all stored keys                                                |
| `run :env secret-keys get-private <public-prefix>`    | Print the full private key for sharing with teammates                                              |
| `run :env secret-keys remove <public-prefix>`         | Remove a key by public key prefix                                                                  |
| `run :env get <file> <var>`                           | Read a variable (auto-decrypts if file is encrypted)                                               |
| `run :env set <file> <var> <value>`                   | Set a variable (auto-encrypts if file is encrypted)                                                |
| `run :env decrypt <source> [output]`                  | Decrypt an encrypted `.env` file (omit `output` to print to stdout)                                |
| `run :env encrypt <source> <output> <public-prefix>`  | Encrypt a plaintext `.env` file (key matched by public prefix)                                     |

### Flags

| Flag                     | Description                                                                                                        |
|--------------------------|--------------------------------------------------------------------------------------------------------------------|
| `-f`, `--file <path>`    | Use a specific Runfile instead of auto-discovering `Runfile.json` (also settable via the `RUNFILE_TARGET` env var) |
| `--shell <name-or-path>` | Override the shell used for execution, ignoring any `forceShell` in the Runfile                                    |
| `--timings`              | Print per-target and per-command execution times to stderr                                                         |
| `-y`, `--yes`            | Skip confirmation prompts (same as CI auto-skip)                                                                   |
| `--dry-run`              | Show what would be executed without running anything (like `make -n`)                                              |
| `--stdin-args`           | Prompt via stdin for any missing `$(ARGS.x)` / `$(ENV.X)` / `$(FLAGS.x)` values instead of failing                 |
| `--version`              | Print version                                                                                                      |
| `--help`                 | Print help                                                                                                         |

The `-f` flag works with all subcommands:

```
$ run -f deploy/Runfile.json list
$ run -f ci.runfile.json test
```

The `--shell` flag accepts either a shell name or a direct path to a shell executable. It takes highest priority,
overriding both target-level and global `forceShell`:

```
$ run --shell bash build          # Use Bash regardless of Runfile settings
$ run --shell powershell test     # Use PowerShell
$ run --shell /usr/local/bin/zsh dev   # Use a specific shell binary
```

The `--dry-run` flag prints the resolved leaf shell commands for the invoked target without running anything. Only that
target's own shell commands are shown — `@target` invocations and the dependency targets they would dispatch to are not
expanded inline.

```
$ run --dry-run deploy
[runfile] Dry run for target "deploy":
[runfile] ── Target: deploy
[runfile]   (1/2) scp target/release/app server:/opt/
[runfile]   (2/2) echo Deploy complete.
```

---

## Runfile.json Reference

### Structure

```json
{
	"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
	"includes": [
		"./shared/ci.runfile.json"
	],
	"targets": {
		...
	},
	"globals": {
		...
	}
}
```

- `$schema` (required): Schema version identifier.
- `includes` (optional): Array of [include entries](#file-includes) — each is either a path string or
  `{ "path": "...", "namespace": "..." }`. Paths are relative to this file. Included targets are merged — local targets
  win on conflict. Supports recursive includes with cycle detection. With `namespace`, every included target name and
  every `@target` reference inside that file is prefixed with `<namespace>:`.
- `targets` (required): Named targets to run.
- `globals` (optional): Settings applied to all targets.

- `$schema` (string, required) — Schema identifier. Use
  `"https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json"` for now, or a path/URL to the JSON
  Schema file for editor autocomplete.
- `targets` (object, required) — One or more named targets. At least one target must be defined.
- `globals` (object, optional) — Settings that apply to all targets.

### Target Properties

Each target is an object under `targets`:

```json
{
	"targets": {
		"my-target": {
			"description": "What this target does",
			"commands": [
				"echo step 1",
				"echo step 2"
			],
			"env": {
				"KEY": "value"
			},
			"addToPath": [
				"node_modules/.bin"
			],
			"forceShell": "bash",
			"logging": true,
			"ignoreErrors": false
		}
	}
}
```

| Property            | Type                      | Required | Description                                                                                                                                                 |
|---------------------|---------------------------|----------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `commands`          | `(string \| if \| for)[]` | Yes      | Command steps to run sequentially. Each entry is either a shell command string or a control-flow object (`if` / `for`). See [Control Flow](#control-flow).  |
| `description`       | `string`                  | No       | Shown in `run :list` output.                                                                                                                                |
| `envFiles`          | `string[]`                | No       | File paths to load environment variables from. Supports `$(ARGS)` and `$(ENV)` substitution. Loaded before `env`.                                           |
| `env`               | `object`                  | No       | Environment variables for this target. Values can be strings, numbers, or booleans.                                                                         |
| `addToPath`         | `string[]`                | No       | Directories to prepend to `PATH`. Relative paths resolve from the Runfile.json location.                                                                    |
| `forceShell`        | `string`                  | No       | Force a specific shell for this target.                                                                                                                     |
| `logging`           | `boolean`                 | No       | Print each command before running it.                                                                                                                       |
| `ignoreErrors`      | `boolean`                 | No       | Continue running commands even if one fails, and exit with code 0.                                                                                          |
| `parallel`          | `boolean`                 | No       | Execute all commands in parallel instead of sequentially. All commands spawn at once; the target finishes when all exit.                                    |
| `detach`            | `boolean`                 | No       | Spawn commands as detached background processes and exit immediately. Requires `parallel: true`.                                                            |
| `workingDirectory`  | `string`                  | No       | `"runfileParent"` (default) or `"cwd"`. Controls whether commands run in the Runfile.json directory or the caller's current working directory.              |
| `aliases`           | `string[]`                | No       | Alternative names for this target.                                                                                                                          |
| `confirm`           | `string`                  | No       | Prompt message shown before executing. Requires `y/N` confirmation. Skipped in CI or with `--yes`. The string is shown verbatim — no `$(...)` substitution. |
| `forceKillOnSigInt` | `boolean`                 | No       | When true, forcefully kill the entire spawned process tree on SIGINT/CTRL+C. See [Force-kill on Ctrl+C](#force-kill-on-ctrlc).                              |
| `extendStdio`       | `object[]`                | No       | Tail one or more log files during execution and route new lines to `stdout` or `stderr`. See [extendStdio](#extendstdio).                                   |
| `watch`             | `string[]`                | No       | Glob patterns for watch mode. When present, the target automatically re-runs on matching file changes. Use `!` prefix to exclude.                           |
| `onlyInDirectories` | `string[]`                | No       | Restrict this target to only be invocable when the current working directory is at or under one of the listed paths (relative to the Runfile location).     |

### Global Properties

Everything in `globals` applies to all targets. Target-level settings always take priority.

```json
{
	"globals": {
		"addToPath": [
			"node_modules/.bin"
		],
		"env": {
			"CI": true
		},
		"forceShell": "bash",
		"logging": false,
		"ignoreErrors": false
	}
}
```

| Property            | Type       | Description                                                                                                                                                          |
|---------------------|------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `addToPath`         | `string[]` | Directories prepended to `PATH` for every target. Target-level `addToPath` entries go before global ones.                                                            |
| `envFiles`          | `string[]` | File paths to load environment variables from for all targets. Supports `$(ARGS)` and `$(ENV)` substitution.                                                         |
| `env`               | `object`   | Environment variables for all targets. Target-level `env` overrides these.                                                                                           |
| `forceShell`        | `string`   | Default shell for all targets. Overridden by target-level `forceShell`.                                                                                              |
| `logging`           | `boolean`  | Enable command logging globally. Overridden per-target.                                                                                                              |
| `ignoreErrors`      | `boolean`  | Ignore command failures globally. Overridden per-target.                                                                                                             |
| `workingDirectory`  | `string`   | `"runfileParent"` (default) or `"cwd"`. Sets the default working directory for all targets. Overridden per-target.                                                   |
| `forceKillOnSigInt` | `boolean`  | Default for [`forceKillOnSigInt`](#force-kill-on-ctrlc). Overridden per-target.                                                                                      |
| `onlyInDirectories` | `string[]` | Restrict this Runfile's targets to only be available when the current working directory is under one of the listed directories (relative to the Runfile's location). |

---

## Arguments and Substitution

Runfile has built-in support for passing arguments from the command line into your commands.

### Positional arguments — `$(ARGS)`

Everything after the target name that isn't a `--flag` is a positional argument:

```json
{
	"targets": {
		"build": {
			"commands": [
				"cargo build $(ARGS)"
			]
		}
	}
}
```

```
$ run build --release --features=serde
# Executes: cargo build --release --features=serde
```

### Named arguments — `$(ARGS.key ? default)`

Named arguments are extracted from `--key=value` or `--key value` pairs:

```json
{
	"targets": {
		"dev": {
			"commands": [
				"npm run dev"
			],
			"env": {
				"PORT": "$(ARGS.port ? 3000)",
				"NODE_ENV": "$(ARGS.env ? development)"
			}
		}
	}
}
```

```
$ run dev                         # PORT=3000, NODE_ENV=development
$ run dev --port=4000             # PORT=4000, NODE_ENV=development
$ run dev --env=production        # PORT=3000, NODE_ENV=production
```

### Environment variables — `$(ENV.key ? default)`

Reference environment variables (including those defined in the target's `env` property) with `$(ENV.key)`. Lookups are
case-insensitive:

```json
{
	"targets": {
		"greet": {
			"commands": [
				"echo Hello $(ENV.USER ? world)"
			]
		}
	}
}
```

```
$ run greet                       # echo Hello alice (if $USER is set)
$ USER= run greet                 # echo Hello world
```

### Chained fallbacks

Fallback values can themselves be `ARGS.key` or `ENV.key` references, forming a chain that is tried left to right:

```json
{
	"targets": {
		"server": {
			"commands": [
				"node server.js"
			],
			"env": {
				"NODE_ENV": "$(ARGS.env ? ENV.NODE_ENV ? development)"
			}
		}
	}
}
```

```
$ run server --env=staging        # NODE_ENV=staging (from ARGS)
$ NODE_ENV=production run server  # NODE_ENV=production (from ENV)
$ run server                      # NODE_ENV=development (literal default)
```

### Interactive prompts — `--stdin-args`

The `--stdin-args` flag (placed before the target name, like `--dry-run`) prompts via stdin for any missing
`$(ARGS.x)` / `$(ENV.X)` / `$(FLAGS.x)` reference instead of failing. Substitutions with a literal default are
also prompted: pressing Enter accepts the default; required values without a default must be supplied or the
run errors as it would without the flag.

```
$ run --stdin-args server
[runfile] enter ARGS.env [development]:        # Enter → uses "development"
[runfile] pass --release? (y/N): y             # toggles $(FLAGS.release) to true
```

Resolution order with `--stdin-args` set:

1. CLI args / env / FLAGS that were actually provided still win (no prompt).
2. If nothing in the chain resolves, the user is prompted with the chain's literal default shown in
   `[brackets]` (or `(required)` if no default exists).
3. A non-empty answer overrides the chain.
4. An empty answer falls through to the chain's default — or surfaces the existing
   `MissingArg` / `MissingEnv` error if no default exists.

`LOOP.*` and `RUN.*` are never prompted (they're runtime context). Answers are cached per `(kind, key)` so the
same value is asked at most once per run, even across `@target` invocations. Works with `--dry-run` too — the
dry-run path goes through the same substitution layer.

### Boolean flags — `$(FLAGS.key)`

Flags are boolean toggles. They check whether `--key` was passed on the command line (the value, if any, is ignored).
Flags are always optional — they resolve to `"false"` when absent.

```json
{
	"targets": {
		"build": {
			"commands": [
				"cargo build $(FLAGS.release ? --release :) $(FLAGS.verbose ? -v :)"
			]
		}
	}
}
```

```
$ run build                           # cargo build
$ run build --release                 # cargo build --release
$ run build --verbose --release       # cargo build --release -v
```

The raw form `$(FLAGS.key)` returns the string `"true"` or `"false"`:

```json
{
	"targets": {
		"test": {
			"commands": [
				"echo dry_run=$(FLAGS.dry-run)"
			]
		}
	}
}
```

```
$ run test --dry-run                  # echo dry_run=true
$ run test                            # echo dry_run=false
```

The ternary form `$(FLAGS.key ? true_val : false_val)` uses ` : ` (colon with spaces) as the separator, so colons inside
URLs and paths work naturally:

```json
{
	"targets": {
		"serve": {
			"commands": [
				"curl $(FLAGS.ssl ? https://localhost:3443 : http://localhost:3000)"
			]
		}
	}
}
```

The shorthand `$(FLAGS.key ? value)` (no ` : `) returns `value` when the flag is present and an empty string when
absent.

Flags referenced by `$(FLAGS.key)` are consumed and will not appear in `$(ARGS)`.

### Runtime context — `$(RUN.os)`, `$(RUN.shell)`

Reference the active execution context to write conditional commands without
duplicating logic into multiple targets. Two runtime values are exposed as substitutions:

| Substitution   | Resolves to                                                                                                                               |
|----------------|-------------------------------------------------------------------------------------------------------------------------------------------|
| `$(RUN.os)`    | `"windows"`, `"linux"`, or `"mac"`.                                                                                                       |
| `$(RUN.shell)` | `"bash"`, `"zsh"`, `"sh"`, `"fish"`, `"powershell"`, or `"cmd"` — the shell that will run the commands (after any `forceShell` override). |

These plug straight into `if` conditions, `for` iterators, command bodies, and
`env` values:

```jsonc
{
	"targets": {
		"clean": {
			"commands": [
				{ "if": "$(RUN.os) == windows", "then": ["del /S /Q build"], "else": ["rm -rf build"] }
			]
		},
		"smoke": {
			"commands": [
				{ "if": "$(RUN.shell) == powershell", "then": ["Write-Host hello"], "else": ["echo hello"] }
			]
		},
		"trace": {
			"commands": [
				{ "if": "$(FLAGS.debug) == true", "then": ["./tool --verbose"], "else": ["./tool"] }
			]
		}
	}
}
```

Unknown `$(RUN.<key>)` references are an error at substitution time. The valid
keys are `os` and `shell`. `RUN.*` participates in chained fallbacks just like
ARGS/ENV: `$(ARGS.shell ? RUN.shell)`.

All substitution syntax (`$(ARGS)`, `$(FLAGS)`, `$(ENV)`, `$(RUN)`) works in `env` values too, both at the target and
global level:

```json
{
	"targets": {
		"dev": {
			"commands": [
				"node server.js"
			],
			"env": {
				"PORT": "$(ARGS.port ? 3000)",
				"NODE_OPTIONS": "$(FLAGS.debug ? --inspect : )"
			}
		}
	}
}
```

```
$ run dev --debug --port=9229    # PORT=9229, NODE_OPTIONS=--inspect
$ run dev                        # PORT=3000, NODE_OPTIONS=
```

### Substitution Syntax

| Syntax                    | Behavior                                                         |
|---------------------------|------------------------------------------------------------------|
| `$(ARGS)`                 | All positional arguments, joined by spaces.                      |
| `$(ARGS.key ? default)`   | Named argument `--key`, or `default` if not provided.            |
| `$(ARGS.key ?)`           | Named argument `--key`, or empty string if not provided.         |
| `$(ARGS.key)`             | Named argument `--key`. **Error** if not provided.               |
| `$(FLAGS.key)`            | `"true"` if `--key` passed, `"false"` otherwise.                 |
| `$(FLAGS.key ? a : b)`    | `a` if `--key` passed, `b` otherwise (` : ` separator).          |
| `$(FLAGS.key ? a)`        | `a` if `--key` passed, empty string otherwise.                   |
| `$(ENV.key ? default)`    | Environment variable, or `default` if not set.                   |
| `$(ENV.key ?)`            | Environment variable, or empty string if not set.                |
| `$(ENV.key)`              | Environment variable. **Error** if not set.                      |
| `$(RUN.os)`               | `"windows"`, `"linux"`, or `"mac"`.                              |
| `$(RUN.shell)`            | `"bash"`, `"zsh"`, `"sh"`, `"fish"`, `"powershell"`, or `"cmd"`. |
| `$(ARGS.a ? ENV.b ? val)` | Chained: try ARGS.a, then ENV.b, then literal `val`.             |

Environment variable lookups are **case-insensitive**. If the target's `env` property defines the same key with
different casing (e.g. both `NODE_ENV` and `node_env`), Runfile exits with an error.

The `$(ARGS.key)` and `$(ENV.key)` forms (no `?`) are useful when a value is required:

```json
{
	"targets": {
		"deploy": {
			"commands": [
				"./deploy.sh --env=$(ARGS.env)"
			]
		}
	}
}
```

```
$ run deploy --env=staging        # Works
$ run deploy                      # Error: Argument "env" was not provided
```

---

## Control Flow

Each entry in a `commands` array can be either:

- a **shell command string** — runs through the resolved shell, same as today, or
- a **target invocation** — a string starting with `@` (see [Target invocations](#target-invocations--target-args)), or
- an **`if` block** — conditional execution, or
- a **`for` block** — iteration, or
- a **`match` block** — multi-way dispatch on a substituted value (with built-in case validation).

These blocks are evaluated by Runfile itself, so they are cross-platform and cross-shell by default. Conditions are
parsed at Runfile load time, so syntax errors fail fast.

### Target invocations — `@target args...`

Any string command entry that starts with `@` invokes another target as a step. The text after `@` is split into
the target name (first token) and an args template (everything after the first whitespace run).

Prefix the target name with `?` (`@?target`) to mark the call **optional**: at execute time, if the (substituted)
target doesn't exist in the merged Runfile, the call is silently skipped instead of producing an `UnknownTarget`
error. This is the recommended pattern for `for in: "namespaces"` blocks where some namespaces may not define a
given target. Note that `@?target` only suppresses the *missing-target* error — it does not silence runtime
failures from the target's own commands.

```jsonc
"commands": [
  "echo running pipeline",
  "@build",                       // call `build` with no args
  "@build --release",             // call `build` with explicit args
  "@build $(ARGS)",               // forward the parent's positional args
  "@deploy --env=$(ENV.STAGE)",   // any substitution works in the args template
  "@?nightly-cleanup",            // silently skipped if `nightly-cleanup` isn't defined
  // common pattern: per-namespace target that's only defined in some namespaces
  { "for": "ns", "in": "namespaces", "do": "@?$(LOOP.ns):adb-forward" }
]
```

`?` is reserved for this marker — target names, aliases, and `includes` namespaces are rejected at parse time if
they contain `?`.

| Behavior              | Detail                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
|-----------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Argument parsing      | The args template is **substituted first** (all `$(ARGS)` / `$(ENV.*)` / `$(RUN.*)` / `$(LOOP.*)` / `$(FLAGS.*)` resolve), then **shlex-split** into argv. Quoted args (`"hello world"`) are kept intact.                                                                                                                                                                                                                                                                                                                     |
| No dedup              | Calling `@build` three times runs `build` three times, with their respective args. There is no diamond-dependency dedup at this level.                                                                                                                                                                                                                                                                                                                                                                                        |
| Cycle detection       | A target that (transitively) calls itself errors at runtime: `Dependency cycle detected: foo`.                                                                                                                                                                                                                                                                                                                                                                                                                                |
| Env propagation       | The parent target's resolved env is passed to the dependency as a substitution base. The dep's own `envFiles` / `env` layer on top (dep wins per key), then the **current shell env always wins** over both. For PATH, the dep's `addToPath` ends up at the very front, then the parent's, then the shell `PATH` (`[dep_addToPath…, parent_addToPath…, …, shell PATH]`). Chains compose recursively — a grandchild's `addToPath` lands further forward than its parent's, which lands further forward than the grandparent's. |
| Target-level config   | `forceShell`, `workingDirectory`, `parallel`, `confirm`, etc. are **not inherited** — each target picks its own. Only env flows.                                                                                                                                                                                                                                                                                                                                                                                              |
| Inside `if` / `for`   | `@target` is a normal command step — works inside `then` / `else` / `for` body. `if` shorthand `"then": "@deploy"` is the same as `"then": ["@deploy"]`.                                                                                                                                                                                                                                                                                                                                                                      |
| Parallel parents      | When the parent has `parallel: true`, `@target` invocations run on worker threads alongside the sibling shell commands. Nested `parallel: true` deps fan out further (no enforced sequentialization).                                                                                                                                                                                                                                                                                                                         |
| Optional calls        | `@?target` silently skips when the target doesn't exist (no error, no failure counted). Static analysis treats statically-missing optional calls as 0 leaves; dynamic optional calls (`@?$(...)`) still reserve 1 counter slot per dispatch and skip at runtime. Failures *inside* a present target's commands are not silenced — use `ignoreErrors` for that.                                                                                                                                                                |
| Plain `@` not allowed | `"@"` or `"@ "` (no target name) is a parse error. Same for `"@?"` / `"@? "`. To run a shell command starting literally with `@`, prefix with a space or wrap in `sh -c`.                                                                                                                                                                                                                                                                                                                                                     |

For an exhaustive table of target invocation semantics (no dedup, env layering, cycle detection), see
[Target invocations](#target-invocations--target-args).

### `if` blocks

```jsonc
{
  "if": "$(ARGS.env) == production && $(FLAGS.confirm) == true",
  "then": ["./deploy-prod.sh"],
  "else": ["./deploy-staging.sh"]
}

// Single-command shorthand: `then` and `else` can be a bare string instead of a one-element array.
{ "if": "$(RUN.os) == windows", "then": "del /S /Q build", "else": "rm -rf build" }
```

| Field          | Type                                 | Required | Description                                                                                                       |
|----------------|--------------------------------------|----------|-------------------------------------------------------------------------------------------------------------------|
| `if`           | `string`                             | Yes      | Boolean condition expression. See [Condition expressions](#condition-expressions).                                |
| `then`         | `string \| commandStep[]`            | Yes      | Steps run when the condition is truthy. A bare string is sugar for a one-element array. May be empty.             |
| `else`         | `string \| commandStep[]`            | No       | Steps run when the condition is falsy. Same string-shorthand as `then`.                                           |
| `ignoreErrors` | `boolean`                            | No       | When true, failures inside this block do not flip the run's success state.                                        |
| `when`         | `"success" \| "failure" \| "always"` | No       | State guard for the entire `if` block. Default `"success"`. See [When-guarded blocks](#when-guarded-blocks-when). |

### `for` blocks

```jsonc
// Inline list:
{ "for": "service", "in": ["api", "web"], "do": ["docker build $(LOOP.service)"] }

// File glob (relative to the working directory):
{ "for": "f", "glob": "src/**/*.rs", "do": ["rustfmt $(LOOP.f)"] }

// Lines of stdout from a shell command (run once, at planning time):
{ "for": "f", "shell": "git diff --name-only", "do": ["clang-format -i $(LOOP.f)"] }

// Iterate over every namespace prefix declared via `includes` — handy for
// monorepo-style "build everything" targets that compose namespaced subprojects.
// `@$(LOOP.ns):build` substitutes the loop var into the target name and
// dispatches to the matching namespaced target on each iteration.
{ "for": "ns", "in": "namespaces", "do": "@$(LOOP.ns):build" }

// Concurrent iterations:
{ "for": "x", "in": ["a","b","c"], "parallel": true, "do": ["./worker.sh $(LOOP.x)"] }
```

| Field          | Type                         | Required             | Description                                                                                                                                                                                                                                                                                                                                               |
|----------------|------------------------------|----------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `for`          | `string`                     | Yes                  | Loop variable name. Matches `[A-Za-z_][A-Za-z0-9_]*`. Reference inside the body as `$(LOOP.<name>)`.                                                                                                                                                                                                                                                      |
| `in`           | `string[]` or `"namespaces"` | One of in/glob/shell | Iterate over each element of an explicit array (each element is substituted, so `$(ARGS.x)` etc. work), OR pass the magic string `"namespaces"` to iterate over every namespace prefix declared via `includes` (alphabetically sorted, deduplicated, composed across nested includes — a chain `outer:inner` shows up as both `outer` and `outer:inner`). |
| `glob`         | `string`                     | One of in/glob/shell | Iterate over file paths matching the pattern, relative to the working directory.                                                                                                                                                                                                                                                                          |
| `shell`        | `string`                     | One of in/glob/shell | Iterate over each non-empty line of the command's stdout. Lines are trimmed; blank lines are dropped. The iterator runs once at planning time. **A non-zero exit is a hard error.**                                                                                                                                                                       |
| `do`           | `string \| commandStep[]`    | Yes                  | Body steps run once per iteration. May be empty. Accepts either a single shell-command string (sugar for a one-element array) or an array of command steps.                                                                                                                                                                                               |
| `parallel`     | `boolean`                    | No                   | Run iterations concurrently. **Outer parallel only:** an inner `for` block is forced sequential when its parent context is already parallel (a warning is printed).                                                                                                                                                                                       |
| `ignoreErrors` | `boolean`                    | No                   | When true, body failures do not flip the run's success state and do not stop iteration.                                                                                                                                                                                                                                                                   |

> **Dynamic target names.** `@target` invocations now substitute their target
> name at dispatch time, so patterns like `@$(LOOP.ns):build` (or
> `@$(ARGS.target)`) resolve to a concrete target at runtime. Static analysis
> (the `(N/total)` step counter, arg-usage scanning, cycle detection of
> *known* names) treats dynamic names as a single leaf with no recursion —
> the runtime counter bumps the total via `add_to_total` if the dispatched
> target turns out to expose more steps.

### `match` blocks

Multi-way dispatch on a substituted value. Equivalent to a chain of `if` / `else if` / `else` blocks but with a
clearer error story when the value doesn't match any case (and no `default` is configured).

```jsonc
"commands": [
  {
    "match": "$(ARGS.tier ? 1)",  // chained substitution → defaults to "1" when --tier missing
    "cases": {
      "1": "flutter emulators --launch Tier_1_Android_9_SDK_28_1GB",
      "2": "flutter emulators --launch Tier_2_Android_11_SDK_30_2GB",
      "3": ["echo bringing up tier 3", "flutter emulators --launch Tier_3_Android_14_SDK_34_4GB"]
    }
  },
  "adb wait-for-device",
  "flutter run"
]
```

| Field          | Type                                 | Required | Description                                                                                                                                                                                                                                          |
|----------------|--------------------------------------|----------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `match`        | `string`                             | Yes      | The substitution template whose resolved value selects a case. Goes through the same substitution pipeline as any other `$(...)` reference, so chained fallbacks (`$(ARGS.tier ? ENV.TIER ? 1)`) and all source kinds (`ARGS`, `ENV`, `LOOP`, `RUN`) work. |
| `cases`        | `object<string, string \| commandStep[]>` | Yes (or `default`) | Map from case value (compared as a string against the resolved match value) to the steps to run. Each value is either a single shell-command string (sugar for a one-element array) or an array of command steps.                                                                                       |
| `default`      | `string \| commandStep[]`            | No       | Steps run when no case matches the resolved value. Also runs when the `match` substitution itself fails (e.g. missing arg with no chain default). When omitted, an unmatched value is a hard error that lists the valid cases.                       |
| `ignoreErrors` | `boolean`                            | No       | When true, failures inside the chosen branch do not flip the run's success state.                                                                                                                                                                    |
| `when`         | `"success" \| "failure" \| "always"` | No       | State guard for the entire `match` block. Default `"success"`. See [When-guarded blocks](#when-guarded-blocks-when).                                                                                                                                 |

**Match semantics.** The `match` template is substituted, then the resolved string is looked up in `cases` by exact
equality. A case match runs that case's steps. Otherwise, `default` runs if set. Without `default`, an unmatched
value surfaces an error like:

```
No case matched value "5" for `match` "$(ARGS.tier)"
  Valid cases: "1", "2", "3", "4"
```

When the substitution itself fails (e.g. `$(ARGS.tier)` with no `--tier` flag and no chain default), `default` runs
if present; otherwise the error message includes the valid cases too — so users always know what values they can
pass:

```
Could not resolve value for `match` "$(ARGS.tier)": Argument "tier" was not provided …
  Valid cases: "1", "2", "3", "4"
```

Use chained substitution in `match` for a default *value* (`$(ARGS.tier ? 1)`) and `default` for a fallback
*branch* — they compose. Cases iterate in alphabetical order in error messages (because internally they're stored
in a sorted map).

### `$(LOOP.var)` — loop variable substitution

Inside a `for` block's body (and in nested blocks), reference the loop variable as `$(LOOP.<var>)`:

```jsonc
{ "for": "stage", "in": ["lint", "test", "build"], "do": [
  { "for": "service", "in": ["api", "web"], "do": [
    "echo running $(LOOP.stage) on $(LOOP.service)"
  ] }
] }
```

Inner loop variables shadow outer ones with the same name. Referencing a loop variable outside its scope is a hard
error. `$(LOOP.x)` participates in chained fallbacks just like ARGS/ENV: `$(LOOP.x ? default)`.

### Condition expressions

Conditions in `if` blocks use a tiny boolean DSL.

**Operators (in evaluation order):**

| Form             | Meaning                                                        |
|------------------|----------------------------------------------------------------|
| `value`          | Truthy: the empty string is falsy; any other string is truthy. |
| `value == value` | Case-sensitive string equality.                                |
| `value != value` | Case-sensitive string inequality.                              |
| `!expr`          | Logical NOT.                                                   |
| `expr && expr`   | Logical AND (short-circuits).                                  |
| `expr \|\| expr` | Logical OR (short-circuits).                                   |
| `(expr)`         | Grouping. **Required when mixing `&&` and `\|\|`.**            |

**Values** can be:

- A substitution: `$(ARGS.x)`, `$(ENV.X)`, `$(FLAGS.x)`, `$(LOOP.x)`, `$(RUN.os)` / `$(RUN.shell)`, with chained
  fallbacks.
- A quoted string: `"foo bar"` or `'foo bar'`. No escapes in v1.
- A bare word: `production`, `path/to/file`, `1.2.3`.

**Truthiness rule.** Only the empty string is falsy. Every other string — including `"false"`, `"0"`, and `"no"` — is
truthy. This matches what the shell sees when given a `$(...)` substitution. In particular, `$(FLAGS.x)` resolves to
`"true"` when present and `"false"` when absent — *both non-empty* — so flag presence checks must use explicit
comparison:

```jsonc
{ "if": "$(FLAGS.confirm) == true", "then": [...] }   // flag set
{ "if": "$(FLAGS.confirm) == false", "then": [...] }  // flag absent
```

**Mixing `&&` and `||`.** The DSL refuses to assume operator precedence. If you mix the two operators in the same
expression, parentheses are required:

```jsonc
"a == b && (c == d || e == f)"   // OK
"a && b || c"                    // Error: cannot mix without parentheses
```

### Step counter and logging

Each leaf shell command consumes one slot in the `(N/total)` step indicator. The total is precomputed by walking the
command tree statically:

- `if` inflates the total by `then.len() + else.len()` (whichever branch is skipped leaves slots unused — final `N` may
  end below `total`, same trade-off as `when: failure` lifecycle steps).
- `for in` multiplies the body's leaf count by the literal array length.
- `for glob` and `for shell` start with a 1-iteration estimate. When the iterator expands to more iterations, the
  total is bumped at runtime so `N` always stays ≤ `total`.

Log lines:

```
[runfile] (3/12) docker build api
[runfile] (4/12) [parallel] worker.sh a
```

Internal control-flow nodes are not logged separately — only the leaf shell commands they expand to.

---

## Shell Support

Runfile supports six shells:

| Shell        | Platforms                        | Command flag |
|--------------|----------------------------------|--------------|
| `bash`       | Linux, macOS, Windows (Git Bash) | `-c`         |
| `zsh`        | Linux, macOS                     | `-c`         |
| `sh`         | Linux, macOS                     | `-c`         |
| `fish`       | Linux, macOS                     | `-c`         |
| `powershell` | Windows, Linux, macOS            | `-Command`   |
| `cmd`        | Windows                          | `/C`         |

### Automatic detection

By default, Runfile auto-detects the best available shell:

- **Linux/macOS** — Uses the `$SHELL` environment variable when it points to a recognised shell, then falls back through
  `/bin/bash`, `/bin/zsh`, `/bin/sh`, and finally `fish` (`/usr/bin/fish`, `/usr/local/bin/fish`).
- **Windows** — Looks for Git Bash in the standard install locations (`%ProgramFiles%\Git\bin\bash.exe`,
  `%ProgramFiles(x86)%\Git\bin\bash.exe`, `%LOCALAPPDATA%\Programs\Git\bin\bash.exe`, `C:\Git\bin\bash.exe`), then
  PowerShell (`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`), then cmd.exe (
  `%SystemRoot%\System32\cmd.exe`). If none of those exist, it falls back to a `which`/`where` lookup for `bash`, then
  `powershell`, then `cmd`.

WSL's `bash` (`C:\Windows\System32\bash.exe`) is intentionally **not** picked up — it runs inside a separate Linux
environment and cannot see Windows-side binaries from `PATH`.

### `sh` fallback

`sh` is rarely installed on Windows and may be missing on minimal Linux containers. When `sh` is requested
(via `forceShell`, `--shell`, or any other path) and cannot be found, Runfile falls back to other
sh-compatible shells in order: **bash → zsh → fish**. `$(RUN.shell)` then reflects whichever shell actually
ran (e.g. `bash`), so existing `if $(RUN.shell) == bash` branches keep working. This lets you write
`forceShell: "sh"` for portable `cp`/`echo`/`mkdir` snippets without an `if RUN.os == windows` branch.

### Forcing a shell

Use `forceShell` to pin a specific shell. This is useful for cross-platform teams or for commands that require a
particular shell's syntax:

```json
{
	"targets": {
		"ps-task": {
			"commands": [
				"Write-Host 'Hello from PowerShell'"
			],
			"forceShell": "powershell"
		},
		"bash-task": {
			"commands": [
				"echo $BASH_VERSION"
			],
			"forceShell": "bash"
		}
	}
}
```

Set it globally if your whole project assumes one shell:

```json
{
	"globals": {
		"forceShell": "bash"
	}
}
```

### CLI shell override

Use the `--shell` flag to override any `forceShell` in the Runfile. This accepts a shell name or a direct path:

```
$ run --shell powershell build
$ run --shell "C:\tools\git\bin\bash.exe" deploy
```

The priority order for shell selection is: **`--shell` flag** > **target `forceShell`** > **global `forceShell`** > *
*auto-detected shell**.

### Custom shell paths

If Runfile can't find a shell automatically (e.g. Bash installed in a non-standard location on Windows), register it
once:

```
$ run :config shell set bash "C:\tools\git\bin\bash.exe"
```

This saves the path to your local settings file and is remembered across runs.

---

## Command Execution Model

Each entry in a target's `commands` array is executed as a **separate shell process**. This means state like the current
directory, shell variables, or aliases does not carry over between commands.

For example, this does **not** work as expected:

```json
{
	"targets": {
		"build": {
			"commands": [
				"cd build-dir",
				"make"
			]
		}
	}
}
```

The `cd` runs in one shell process and exits. Then `make` runs in a fresh shell process back in the original working
directory.

To chain commands that share state, put them in a single string:

```json
{
	"targets": {
		"build": {
			"commands": [
				"cd build-dir && make"
			]
		}
	}
}
```

This runs both in one shell invocation, so the `cd` carries through to `make`.

As a general rule: use separate entries in `commands` for independent steps, and use `&&` (or `;`) within a single entry
for steps that depend on shared shell state.

---

## Environment Variables

### Per-target

```json
{
	"targets": {
		"dev": {
			"commands": [
				"npm start"
			],
			"env": {
				"PORT": 3000,
				"DEBUG": true,
				"NODE_ENV": "development"
			}
		}
	}
}
```

Values can be strings, numbers, or booleans. Numbers and booleans are converted to strings when set in the environment.

### Global

```json
{
	"globals": {
		"env": {
			"APP_NAME": "my-app",
			"CI": false
		}
	}
}
```

Target-level `env` values override global ones for the same key.

### Environment Files

Load environment variables from `.env` files using `envFiles`:

```json
{
	"globals": {
		"envFiles": [
			".env"
		]
	},
	"targets": {
		"dev": {
			"commands": [
				"npm start"
			],
			"envFiles": [
				".env",
				".env.local"
			]
		}
	}
}
```

File paths support `$(ARGS)` and `$(ENV)` substitution for dynamic selection:

```json
{
	"globals": {
		"envFiles": [
			".env",
			".env.$(ENV.environment ? development)"
		]
	},
	"targets": {
		"deploy": {
			"commands": [
				"./deploy.sh"
			],
			"envFiles": [
				".env",
				".env.$(ARGS.env)"
			]
		}
	}
}
```

With the second example, running `run deploy --env production` would load `.env` and `.env.production`.

**Env file format:**

```bash
# Comments start with # or //
KEY=value
KEY = value           # spaces around = are trimmed
KEY="double quoted"   # quotes are stripped
KEY='single quoted'   # quotes are stripped
KEY="multi
line
value"                # newlines preserved inside quotes
KEY=                  # empty string
export KEY=value      # export prefix is accepted
```

**Behavior:**

- File paths are relative to the working directory (Runfile.json location by default, or CWD if `workingDirectory` is
  `"cwd"`).
- **Missing files are silently ignored.** This allows patterns like `.env.local` that only exist on some machines.
- **Unparseable files produce an error.**
- `envFiles` are loaded **before** the inline `env` object, so `env` values override file values.
- Within `envFiles`, later files override earlier ones for the same key.
- The current shell environment **always wins** over Runfile-defined values. After `envFiles` and `env` are merged,
  Runfile re-overlays `std::env::vars()`, so any var the shell defines reaches the child unmodified. Setting e.g.
  `"env": { "PATH": "/foo" }` will be silently overridden by the inherited shell `PATH` (use `addToPath` to extend`PATH`
  instead — it's applied *after* the overlay).

**Priority order (highest precedence first):** **shell env** > **target `env`** > **global `env`** > **target `envFiles`
** > **global `envFiles`**. (Because globals are baked into each target at parse time, the runtime applies a single
merged `env` map — with target values winning over global ones — *after* all `envFiles` have been loaded, then the shell
overlay wins over the merged result.)

---

## Encrypted Environment Variables

Runfile supports encrypted environment variable values, similar to [dotenvx](https://dotenvx.com/). Encrypted values can
be safely committed to version control — they're decrypted in-memory at runtime and passed to child processes via
`Command::envs()`, never touching disk in plaintext.

### How it works

Each encrypted `.env` file contains a `RUNFILE_ENCRYPTION_PUBLIC_KEY` — a SHA-256 fingerprint of the private key used to
encrypt the values. Private keys are stored in your platform's OS credential store (Windows Credential Manager, macOS
Keychain, or Linux Secret Service); the local settings file only records the public-key fingerprints. When Runfile loads
the file, it automatically matches the file's public key against your stored private keys (or the
`RUNFILE_ENCRYPTION_KEY` env var) to find the correct decryption key. No key names or manual configuration needed in
`Runfile.json`.

### Setup

**1. Create an encrypted `.env` file:**

The easiest way is to use `run :env init`, which generates a new private key and creates an encrypted `.env` file in one
step:

```
$ run :env init -p .env.production
Created .env.production (encrypted).

  Public key: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08

A new private key was generated and added to your local settings.

To share this env file with teammates, they must import the same
private key before they can decrypt or use it:

  1. Share the private key securely:
     run :env secret-keys get-private 9f86d081...

  2. They import it on their machine:
     run :env secret-keys add
     (then paste the private key when prompted)
```

The private key is stored in your platform's OS credential store (Windows Credential Manager / macOS Keychain / Linux
Secret Service via `keyring`); only its public key fingerprint is recorded in the local settings file. Neither is
committed to version control.

You can also use an existing key with `--key <prefix>`, or create a plaintext file with `--plain`. If you omit `-p`, the
file defaults to `.env`.

**Alternatively**, you can encrypt an existing plaintext `.env` file:

```
$ run :env secret-keys add            # Generate a key first (if you don't have one)
$ run :env encrypt .env.local .env.production 9f86
Encrypted .env.local -> .env.production
```

The resulting encrypted file contains the public key and encrypted values:

```bash
RUNFILE_ENCRYPTION_PUBLIC_KEY=9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
DB_PASS=encrypted:BASE64_CIPHERTEXT...
API_KEY=encrypted:BASE64_CIPHERTEXT...
# Comments are preserved
PLAIN_VAR=
```

**2. Use the encrypted file** in your Runfile:

```json
{
	"targets": {
		"deploy": {
			"envFiles": [
				".env.production"
			],
			"commands": [
				"./deploy.sh"
			]
		}
	}
}
```

When `run deploy` executes, Runfile automatically finds the matching private key, decrypts `DB_PASS` and `API_KEY`, and
passes them to the child process. `PLAIN_VAR` is passed through unchanged.

### File helpers

Set a variable in an encrypted `.env` file (auto-encrypts if the file contains `RUNFILE_ENCRYPTION_PUBLIC_KEY`):

```
$ run :env set .env.production DB_PASS "new-password"
DB_PASS set in .env.production
```

Use `--plain` to store a value as plaintext even in an encrypted file:

```
$ run :env set .env.production APP_NAME "my-app" --plain
APP_NAME set in .env.production
```

Read and decrypt a variable (auto-detects encryption):

```
$ run :env get .env.production DB_PASS
new-password
```

Decrypt an entire file to plaintext:

```
$ run :env decrypt .env.production .env.local
Decrypted .env.production -> .env.local
```

Or print the decrypted contents to stdout (handy for piping):

```
$ run :env decrypt .env.production
DATABASE_URL=postgres://...
API_KEY=...
```

### Key management

```
$ run :env secret-keys list                  # Show stored public-key fingerprints
$ run :env secret-keys get-private 9f86      # Print full private key (for sharing)
$ run :env secret-keys remove 9f86           # Remove by public key prefix
```

All key matching uses **public key prefixes** — if a public key starts with `9f86d081` you can reference it as `9f86` as
long as the prefix is unambiguous among your stored keys.

Use `run :env secret-keys get-private <public-prefix>` to print the full private key so you can securely share it with
teammates who need to decrypt the same `.env` files. They import it by running `run :env secret-keys add` and choosing *
*Import an existing private key** when prompted, then pasting the 64-character hex string.

### Rotating a key

`run :env rotate <file>` generates a new private key, **decrypts every encrypted value with the old key, re-encrypts it
with the new key**, and rewrites the file with the new `RUNFILE_ENCRYPTION_PUBLIC_KEY` header. Plaintext values,
comments, and blank lines are preserved verbatim. The new key is added to your OS credential store; the old key is left
in place by default so other files encrypted with it keep working.

```
$ run :env rotate .env.production
Key rotated for .env.production.

  Old public key: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
  New public key: 7c5a3e29c5f1d51ec6da6d3e1c40e88b34f2a7c6e9b8b5c5d4e3f2a1b0c9d8e7

To share the new key with teammates:
  run :env secret-keys get-private 7c5a3e29...
```

Pass `--delete-current-key` to also remove the previous private key from the OS credential store after rewriting the
file. **Do this only after every encrypted file that used the old key has been rotated** — otherwise those files become
permanently undecryptable.

### Running an arbitrary command with an encrypted env

`run :env inject` is a `dotenvx run`-style helper: it loads one or more `.env` files (decrypting any encrypted values in
memory), then `exec`s a child command with those variables in its environment. Use it for tools that aren't invoked via
Runfile targets but still need the same secrets.

```
$ run :env inject -- node scripts/seed.js
$ run :env inject -f .env -f .env.production -- npx prisma migrate deploy
$ run :env inject -f .env.production -- bash -c 'echo $DATABASE_URL'
```

- `-f <file>` is repeatable. If omitted, defaults to a single `.env` in the working directory.
- Files are merged in order — later `-f` files override earlier ones for the same key.
- The `--` separator is required: everything after `--` is the command + args, passed through verbatim (so flags like
  `-v` aren't intercepted by Runfile).
- `RUNFILE_ENCRYPTION_PUBLIC_KEY` is **stripped** from the env before injection — child processes never see the key
  fingerprint.
- The child's exit code is propagated as Runfile's exit code.

### CI/CD

In CI/CD, set the `RUNFILE_ENCRYPTION_KEY` environment variable instead of using local settings:

```yaml
env:
	RUNFILE_ENCRYPTION_KEY: ${{ secrets.RUNFILE_KEY }}
steps:
	-   run: run deploy
```

The env var takes priority over the local settings lookup. The value should be the full 64-character hex-encoded private
key.

---

## PATH Manipulation

Add directories to `PATH` so your commands can find project-local binaries:

```json
{
	"globals": {
		"addToPath": [
			"node_modules/.bin"
		]
	},
	"targets": {
		"lint": {
			"commands": [
				"eslint src/"
			],
			"addToPath": [
				"vendor/bin"
			]
		}
	}
}
```

- Relative paths are resolved from the directory containing `Runfile.json`.
- Global `addToPath` entries come first, then target-level entries, then the existing system `PATH`. The final `PATH`
  looks like `<global-entries>:<target-entries>:<system-PATH>`.
- Because `PATH` lookups are left-to-right, this means a global entry shadows a target entry that resolves to the same
  binary name; both shadow the system `PATH`.

Priority order (left wins): **global addToPath** → **target addToPath** → **system PATH**.

---

## Command Logging

Enable `logging` to see each command printed to stderr before it runs. Useful for debugging multi-step targets:

```json
{
	"targets": {
		"deploy": {
			"commands": [
				"npm run build",
				"npm run test",
				"npm run deploy"
			],
			"logging": true
		}
	}
}
```

Output:

```
[runfile] (1/3) npm run build
...
[runfile] (2/3) npm run test
...
[runfile] (3/3) npm run deploy
...
```

The `[runfile]` prefix uses bold cyan text with ANSI colors, compatible with all supported shells and terminals,
including cmd.exe and PowerShell on Windows.

---

## Aliases

Define alternative names for a target with `aliases`:

```json
{
	"targets": {
		"stop-dev": {
			"commands": [
				"./stop.sh"
			],
			"aliases": [
				"stop",
				"sd"
			]
		}
	}
}
```

```
$ run stop-dev                    # All three invoke the same target
$ run stop
$ run sd
```

Aliases appear in `run :list` and in shell completions. They must be unique — no alias can collide with another target
name or another alias. Names starting with `:` are reserved for built-in commands.

---

## Internal Targets

Targets whose **canonical name starts with `_`** are *internal*. They are hidden from external interfaces but remain
fully usable from within the Runfile itself — perfect for shared setup steps, private helpers, or any target you want to
call from a lifecycle hook without exposing it as part of the public CLI.

```json
{
	"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
	"targets": {
		"_setup": {
			"description": "Shared bootstrap step",
			"commands": [
				"echo 'preparing environment...'"
			]
		},
		"build": {
			"commands": [
				"@_setup",
				"cargo build --release"
			]
		},
		"test": {
			"commands": [
				"@_setup",
				"cargo test"
			]
		}
	}
}
```

```
$ run :list
Available targets:

  build              cargo build --release
  test               cargo test

$ run _setup
Error: target "_setup" is internal and cannot be invoked directly.
Internal targets (names starting with "_") are only callable via `@_name` from another target.

$ run build
preparing environment...
```

### What "internal" means in practice

Internal targets are **excluded** from:

- `run :list` output
- Shell completion suggestions (`run --list-targets`)
- The MCP server's tool list (`run :mcp inspect` / `run :mcp server`) — AI agents won't see them
- Generated editor tasks (`run :generate vscode-tasks`, `zed-tasks`, `jetbrains-run-configurations`)

Internal targets are **rejected** when used directly:

- `run _setup` exits with an error explaining that the target is internal

Internal targets are **fully available** when:

- Invoked from another target's `commands` via `@_setup [args...]` (
  see [Target invocations](#target-invocations--target-args))
- Invoked from another internal target's `commands` (you can chain them)
- Pulled in via includes or globals — visibility is purely a function of the canonical name

### Aliases on internal targets

If an internal target has aliases, they are also blocked from direct invocation and hidden from `:list`/completions,
because resolution always happens through the canonical name. For example, `_setup` with alias `bootstrap` rejects both
`run _setup` and `run bootstrap`.

The internal flag is determined solely by the **canonical** name. Adding an alias that starts with `_` to a public
target does not make that target internal.

### Internal targets in namespaced includes

Internal-ness is checked against the **last** `:`-separated segment of the
canonical name, so a target named `_helper` defined inside a file included
under namespace `api` becomes `api:_helper` and is still treated as
internal — hidden from `:list` and rejected as `run api:_helper`, but
callable via `@api:_helper` from another target's commands and via `@_helper`
from inside the included file itself (which gets rewritten to `@api:_helper`
during the namespacing pass).

---

## When-guarded blocks (`when:`)

Every step in a `commands` array has an effective `when` condition that decides whether it runs based on the target's
running state. The default is `"success"` — the classic "abort on first failure" feel: a failing step flips the target
into the "failed" state and subsequent default-when steps are skipped. `"failure"` and `"always"` blocks let you wire
cleanup, error reporting, or unconditional teardown right inside the same `commands` array.

### Wrapper form: `{ when, commands }`

Wrap a list of inner commands so they only run under the chosen condition:

```jsonc
"commands": [
  "echo step 1",
  "exit 1",                                                  // failure flips state
  "echo skipped",                                            // when:success → skipped after failure
  { "when": "failure", "commands": ["./report-error.sh"] },  // runs only after a failure
  { "when": "always",  "commands": ["./cleanup.sh"] }        // runs regardless
]
```

| Field          | Type                                 | Required | Description                                                                     |
|----------------|--------------------------------------|----------|---------------------------------------------------------------------------------|
| `when`         | `"success" \| "failure" \| "always"` | No       | State guard. Default `"success"`.                                               |
| `commands`     | `commandStep[]`                      | Yes      | The guarded steps. Run sequentially in source order. May not be empty.          |
| `ignoreErrors` | `boolean`                            | No       | When true, failures inside the block do **not** flip the target's failed state. |

### Property form: `when` on `if` / `for` blocks

`when` is also a top-level field on `if` and `for` blocks, so you can guard a control-flow construct without an extra
wrapper:

```jsonc
{ "when": "always", "if": "$(RUN.os) == windows", "then": "rm -rf ./tmp_data", "else": "rm -rf /tmp/data" }
{ "when": "failure", "for": "f", "glob": "logs/*", "do": ["cat $(LOOP.f)"] }
```

### Semantics in detail

| Current state | `when: success` | `when: failure` | `when: always` |
|---------------|-----------------|-----------------|----------------|
| Running OK    | run             | skip            | run            |
| After failure | skip            | run             | run            |

- A `when: success` step that exits non-zero (and isn't `ignoreErrors`'d) flips the state to **failed**. From that
  point, default-`when:success` steps are skipped while `failure` and `always` steps run.
- The state stays failed for the rest of the target — there's no "recovery" by a failure-handler exiting cleanly. The
  target's exit code reflects the failure.
- Inside a `when: failure` or `when: always` block, the inner steps run *as if state were Success* (so default-when
  children execute). New failures inside the block do flip the outer state again unless `ignoreErrors: true` on the
  block.
- Target-level `"ignoreErrors": true` swallows failures entirely — the target exits 0 even if steps failed.
  `when: failure` blocks still don't run in that case (since "failure" wasn't observed).
- Nested combinations like `when: success` outside and `when: failure` inside collapse to "never runs" (the inner is
  unreachable from the outer's gate). Parser keeps it; runtime simply skips the dead path.

### Parallel parents

When the parent target is `parallel: true`, the partition by `when` runs as three sequential phases:

1. All `when: success` (default) leaves run **concurrently** as the parallel batch.
2. After the batch, if anything failed: `when: failure` leaves run sequentially.
3. `when: always` leaves run sequentially after the above.

Concretely, parallelism applies *within* each phase; the phases themselves are ordered.

### Replacing the old `before` / `after` lifecycle

`before` and `after` no longer exist. The mappings:

| Old shape                                              | New shape                                                                                    |
|--------------------------------------------------------|----------------------------------------------------------------------------------------------|
| `"before": [{ "commands": ["X"] }]`                    | Prepend `"X"` to `commands`.                                                                 |
| `"before": [{ "target": "foo" }]`                      | Prepend `"@foo"` to `commands` (see [Target invocations](#target-invocations--target-args)). |
| `"after": [{ "commands": ["X"], "when": "success" }]`  | Append `"X"` to `commands` (default `when: success`).                                        |
| `"after": [{ "commands": ["X"], "when": "failure" }]`  | Append `{ "when": "failure", "commands": ["X"] }`.                                           |
| `"after": [{ "commands": ["X"], "when": "always" }]`   | Append `{ "when": "always", "commands": ["X"] }`.                                            |
| `"after": [{ "target": "cleanup", "when": "always" }]` | Append `{ "when": "always", "commands": ["@cleanup"] }`.                                     |

---

## Dry Run

Use `--dry-run` to print the resolved leaf shell commands for a target without running anything. Output goes to **stdout
**, one command per line, with no `[runfile]` prefix, no ANSI colours, and no `(N/total)` step indicator — so it pipes
cleanly into other tools or diffs across branches:

```
$ run --dry-run deploy --release
scp target/release/app server:/opt/
echo Deploy complete.
```

Substitutions (`$(ARGS.x)`, `$(ENV.x)`, `$(RUN.os)`, etc.) are fully resolved against the current invocation, so the
printed lines are the exact commands that would be sent to the shell. `if` blocks are evaluated against the same
context the runner would see (args + resolved env + loop scope) and only the matching branch is printed.
`@target` invocations are recursively expanded: each dep's resolved shell commands appear inline at the call site,
with the dep's own `env` block reflected on each line. Aggregator targets whose body is purely `@target` dispatches
(for example a `for in: "namespaces"` loop running `@$(LOOP.ns):dev` for every namespaced subproject) print every
nested command, not nothing. Cycles are detected at extract time; optional calls (`@?target`) silently skip when the
dispatched target is absent.

> **Restricted to interactive use.** Because the resolved output inlines env-var values (including decrypted secrets) as
> shell-ready assignments, `--dry-run` refuses to execute when an LLM-agent invocation is detected. Run it from your own
> terminal.

The global `-f`/`--file` and `--shell` flags work with `--dry-run` the same way they do with normal target execution.

---

## File Includes

Use `includes` to pull targets from other Runfile.json files. Each entry is
either a plain path string or an object with a `path` and an optional
`namespace`:

```json
{
	"$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
	"includes": [
		"./shared/ci.runfile.json",
		{
			"path": "./packages/api/Runfile.json",
			"namespace": "api"
		},
		{
			"path": "./packages/web/Runfile.json",
			"namespace": "web"
		}
	],
	"targets": {
		"build": {
			"commands": [
				"@api:build",
				"@web:build"
			],
			"parallel": true
		}
	}
}
```

- Include paths are relative to the including file's directory.
- Local targets always override included targets with the same name.
- Among included files, the first one wins on name conflict.
- Included files can themselves include other files (recursive includes supported).
- Cycle detection prevents circular includes.
- Each included file's `globals` are baked into its own targets only — they don't leak into the including file's
  targets.

### Namespacing

When you set a `namespace` on an include, every target name, alias, and
`@target` reference inside that file is prefixed with `<namespace>:` at parse
time:

- A target `build` in the included file becomes `api:build` in the merged Runfile.
- An alias `b` becomes `api:b`.
- A `@target` reference inside that file's commands gets the same prefix —
  e.g. an included file's `@build` becomes `@api:build` automatically. This
  means **included files are sealed**: `@target` references inside them
  resolve only against the included file's own targets, never against the
  parent's. The same is true for nested includes — the prefix composes
  outward, so an included file that itself includes another file under
  namespace `inner` ends up with targets like `api:inner:build`.

This makes monorepo-style layouts where each package owns its own Runfile
straightforward: aggregate targets in the root call into each package by
namespace, and `parallel: true` fans them out concurrently — similar to
`pnpm --recursive --parallel`.

**Namespace rules.** A namespace must be non-empty and may not contain `:`
or whitespace, or start with `@`, `:`, or `_`. An empty or omitted
`namespace` field is equivalent to the path-string form (no rewrite applied).
The same file may be included twice under different namespaces — the targets
end up as independent copies (e.g. `api:build` and `web:build` both come
from a shared template).

**Internal targets stay internal.** The `_`-prefix rule (see [Internal
targets](#internal-targets)) is applied to the **last** `:`-separated
segment of the canonical name, so `api:_helper` is still treated as
internal — hidden from `:list`, completions, and direct CLI invocation, but
still callable via `@api:_helper` from another target's commands.

**Working directory.** A namespaced target's `workingDirectory: "runfileParent"`
resolves to the directory of the file that defined it, not the root's. Same
for relative `envFiles` paths. This lets each package's commands run with
their own cwd without extra plumbing.

---

## Error Handling

By default, Runfile stops execution on the first command that exits with a non-zero code, and propagates that exit code.

Set `ignoreErrors` to continue through failures:

```json
{
	"targets": {
		"cleanup": {
			"description": "Best-effort cleanup",
			"commands": [
				"rm -rf dist/",
				"rm -rf .cache/",
				"rm -rf tmp/"
			],
			"ignoreErrors": true
		}
	}
}
```

When `ignoreErrors` is `true`:

- All commands run regardless of individual failures.
- The CLI exits with code 0 even if some commands failed.

Set it globally if you want this behavior for all targets, and override per-target where strictness matters.

---

## Parallel Execution

Set `parallel: true` on a target to run all of its commands simultaneously instead of sequentially:

```json
{
	"targets": {
		"dev": {
			"description": "Start all dev services",
			"commands": [
				"npm run watch:css",
				"npm run watch:js",
				"npm run serve"
			],
			"parallel": true
		}
	}
}
```

Running `run dev` will spawn all three commands at the same time. Stdout and stderr from all commands flow through in
real time (not buffered). The target finishes when **all** commands have exited.

When `parallel: true` and `ignoreErrors: true` are both set, all commands run to completion and failures are counted,
but the CLI exits with code 0.

`parallel` is a **target-only** property (not available in `globals`). To make it conditional per-platform, dispatch
into specialized targets via `if "$(RUN.os) == ..."` + `@target`.

---

## Detached Execution

Set `detach: true` (along with `parallel: true`) on a target to spawn commands as background processes that outlive the
Runfile CLI:

```json
{
	"targets": {
		"serve": {
			"commands": [
				"npm start"
			],
			"parallel": true,
			"detach": true
		}
	}
}
```

Running `run serve` will spawn the commands in the background and exit immediately. The spawned processes continue
running independently.

`detach` requires `parallel: true`. Both are **target-only** properties (not available in `globals`). To make them
conditional per-platform, dispatch into specialized targets via `if "$(RUN.os) == ..."` + `@target`.

---

## extendStdio

`extendStdio` lets a target tail one or more log files during execution and route new lines into Runfile's own `stdout`
or `stderr`. Useful when a launched process writes its logs to disk instead of inheriting the console (Unity Editor,
Docker daemons, IDE-launched servers, etc.).

```json
{
	"targets": {
		"unity": {
			"commands": [
				"unity-editor.exe -batchmode -projectPath ."
			],
			"extendStdio": [
				{
					"fromFile": "Logs/Editor.log",
					"stream": "stdout"
				},
				{
					"fromFile": "Logs/Editor.err",
					"stream": "stderr"
				}
			]
		}
	}
}
```

Each entry is `{ "fromFile": <path>, "stream": "stdout" | "stderr" }`.

| Property   | Description                                                                                                                                          |
|------------|------------------------------------------------------------------------------------------------------------------------------------------------------|
| `fromFile` | Path to the file to tail. Relative paths resolve from the target's working directory. Supports `$(...)` substitution (e.g. `"logs/$(RUN.os).log"`).  |
| `stream`   | Either `"stdout"` or `"stderr"`. New lines from the file are written to that stream, prefixed with nothing — they appear inline with command output. |

Behavior:

- A background thread is spawned per `extendStdio` entry **before** the first command runs. Threads continue tailing
  until **after** all commands in the target have exited; a final flush is performed when stopping.
- Files are polled every **50ms**. Files that don't exist yet are tolerated — the tailer waits silently until they
  appear.
- Only **complete lines** (terminated by `\n`) are emitted. A partial trailing line is buffered until the newline
  arrives or the tailer stops (in which case it is flushed).
- If a file is **truncated or rotated** mid-run (size shrinks below the last-read offset), the tailer resets to the
  beginning of the file rather than skipping content.
- `extendStdio` is a **target-only** property (not available in `globals`).

---

## Force-kill on Ctrl+C

Set `forceKillOnSigInt: true` on a target (or globally) when the spawned process tree won't terminate cleanly on console
interrupts. The classic case is **GUI-subsystem applications on Windows** (Unity Editor, Electron-launched dev servers,
etc.) — they don't receive the console `CTRL+C` event the way CLI processes do, and would otherwise survive as orphan
processes when you press Ctrl+C in the terminal where you ran `run`.

```json
{
	"targets": {
		"unity": {
			"commands": [
				"unity-editor.exe -projectPath ."
			],
			"forceKillOnSigInt": true
		}
	}
}
```

Behavior:

- **Windows.** Runfile creates a Windows **Job Object** and assigns the spawned children to it. On `CTRL+C` (or any
  signal that reaches the console handler), `TerminateJobObject` kills every process in the job — direct children **and
  ** all transitive grandchildren — before Runfile exits.
- **Unix.** Runfile records the PID of each spawned child and, on `SIGINT`, sends `SIGKILL` to each before exiting. (The
  default-handler `SIGINT` propagation is also suppressed so Runfile can reap children cleanly and report the exit
  status.)
- The flag has no effect on processes that **do** handle `CTRL+C` correctly — they receive the signal first and exit on
  their own.
- Available on **both targets and `globals`**. Target-level value wins when both are set.

This is opt-in for two reasons: a heavier teardown path is unnecessary for normal CLI tools, and `SIGKILL`/
`TerminateJobObject` give the child no chance to flush state. Don't enable it for processes that need a graceful
shutdown (databases, message queues, etc.).

---

## Runfile Discovery

When you run `run <target>`, it searches for `Runfile.json` starting in the current directory and walking up through
parent directories — similar to how `git` finds `.git/` or `npm` finds `package.json`. This means you can run targets
from any subdirectory of your project. The `run :list` output shows the absolute path of the Runfile being used.

Use `-f` to bypass discovery and point to any file:

```
$ run -f ../other-project/Runfile.json build
```

### `RUNFILE_TARGET` env var

`RUNFILE_TARGET` overrides the default file used when `-f`/`--file` is not passed. This is useful in CI to point at a
non-default Runfile without threading `-f` through every invocation.

```
$ export RUNFILE_TARGET=ci/Runfile.json
$ run build      # equivalent to: run -f ci/Runfile.json build
$ run :list      # also picks up RUNFILE_TARGET
```

When `-f` is provided, it always wins — `RUNFILE_TARGET` is ignored. The env var accepts both file paths and path
aliases (registered via `run :config path-alias add`). If it points to a file that does not exist (and is not an alias),
the command exits with an error rather than falling back to auto-discovery, so misconfigured CI fails fast. Setting
`RUNFILE_TARGET` to an empty string is treated as unset.

### Global Files

Global files are Runfiles that are always merged in alongside the local `Runfile.json`, regardless of which directory
you are in. This is useful for machine-wide or user-wide utility targets you want available everywhere.

Register a global file:

```
$ run :config global-files add ~/.config/runfile/global.json
```

Once registered, its targets are always available when you run `run :list` or `run <target>`, merged with any local
`Runfile.json`. If a local target and a global target share the same name, the **local target takes precedence**.

Global files are processed in registration order. If two global files define the same target, the **first-registered
file wins**.

To see all registered global files:

```
$ run :config global-files list
```

To remove a global file:

```
$ run :config global-files remove ~/.config/runfile/global.json
```

#### globals scope

The `globals` property in a Runfile applies **only to targets in that same file** — it does not bleed across global
files or into the local Runfile. Each file's globals are baked into its own targets at load time.

#### onlyInDirectories

A global Runfile can restrict itself to only be active when the current working directory is under a specific set of
directories. Add `onlyInDirectories` to its `globals` block:

```json
{
	"$schema": "...",
	"globals": {
		"onlyInDirectories": [
			"projects/work",
			"projects/clients"
		]
	},
	"targets": {
		"deploy": {
			"commands": [
				"./deploy.sh"
			]
		}
	}
}
```

Paths in `onlyInDirectories` are relative to the global file's own directory. If the CWD is not under any of the listed
directories, the entire file is skipped (its targets won't appear in `run :list` and can't be invoked).

If `onlyInDirectories` is omitted, the file is active everywhere.

---

## Local Settings

Runfile stores user-level settings (like custom shell paths) in a platform-appropriate location:

| Platform | Path                                                  |
|----------|-------------------------------------------------------|
| Linux    | `~/.config/runfile/settings.json`                     |
| macOS    | `~/Library/Application Support/runfile/settings.json` |
| Windows  | `%APPDATA%\runfile\settings.json`                     |

Settings are created automatically when you use `run :config shell set`, `run :config path-alias add`, or
`run :env secret-keys add`. You don't need to create or edit this file manually.

For the [encrypted environment variables](#encrypted-environment-variables) feature, the settings file stores only *
*public-key fingerprints** in a `secureKeyFingerprints` array — the actual private keys live in your platform's OS
credential store (Windows Credential Manager / macOS Keychain / Linux Secret Service). Each fingerprint (SHA-256 hash of
the private key) is used to automatically match against encrypted `.env` files and look up the corresponding key from
the credential store.

---

## Path Aliases

If you frequently use `-f` with long paths, create an alias:

```
$ run :config path-alias add globals ~/.config/dev/Runfile-globals.json
```

Then use the alias instead of the full path:

```
$ run -f globals check
$ run -f globals list
```

Remove an alias with:

```
$ run :config path-alias remove globals
```

Alias paths are canonicalized to absolute paths when saved, so they work from any directory.

---

## JSON Schema

A JSON Schema file is included at `schemas/v0.schema.json`. Point your `$schema` to it for editor autocomplete and
validation:

```json
{
	"$schema": "./schemas/v0.schema.json",
	"targets": {
		...
	}
}
```

This gives you property suggestions, type checking, and inline documentation in editors like VS Code, IntelliJ, Zed, and
others that support JSON Schema.

---

## Shell Completions

Runfile supports tab completion for targets, subcommands, and flags in Bash, Zsh, Fish, and PowerShell.

### Quick Install

The easiest way is to use the `install` subcommand, which writes the completion script to the standard per-user
directory:

```bash
run :completions install bash
run :completions install zsh
run :completions install fish
run :completions install powershell
```

To remove installed completions:

```bash
run :completions uninstall bash
```

### Manual Setup

You can also print the completion script to stdout and source it yourself:

**Bash** (add to `~/.bashrc`):

```bash
eval "$(run :completions output bash)"
```

**Zsh** (add to `~/.zshrc`):

```zsh
eval "$(run :completions output zsh)"
```

**Fish** (run once):

```fish
run :completions output fish > ~/.config/fish/completions/run.fish
```

**PowerShell** (add to `$PROFILE`):

```powershell
run :completions output powershell | Invoke-Expression
```

### What Gets Completed

- **Targets**: all target names from the current Runfile.json (excluding internal targets)
- **Subcommands**: `:list`, `:init`, `:config`, `:mcp`, `:completions`, `:generate`, `:convert`, `:env`
- **Sub-subcommands**: `:config shell set`, `:config path-alias add`, `:completions install`, `:generate zed-tasks`,
  `:env secret-keys add`, etc.
- **Flags**: `-f`, `--file`, `--shell`, `--timings`, `-y`, `--yes`, `--dry-run`, `--help`, `--version`
- **Shell names**: after `--shell`, completes with `bash`, `zsh`, `sh`, `fish`, `powershell`, `cmd`

---

## Watch Mode

Targets can define `watch` patterns to automatically re-run when files change:

```json
{
	"targets": {
		"dev": {
			"description": "Build on file changes",
			"commands": [
				"cargo build"
			],
			"watch": [
				"src/**/*.rs",
				"!target/**"
			]
		}
	}
}
```

```
$ run dev
[runfile] Running "dev"...
[runfile] Watching for changes... (Ctrl+C to stop)
[runfile] Changed: src/main.rs
[runfile] Re-running "dev"...
```

Watch patterns use glob syntax relative to the Runfile directory. Prefix a pattern with `!` to exclude matching files.

When a target has `watch` patterns defined, running it automatically enters watch mode — no extra flags needed. The
target runs once immediately, then re-runs whenever a matching file changes.

---

## Confirmation Prompts

Targets can require user confirmation before running:

```json
{
	"targets": {
		"deploy": {
			"description": "Deploy to production",
			"commands": [
				"./deploy.sh"
			],
			"confirm": "Deploy to production?"
		}
	}
}
```

```
$ run deploy
[runfile] Deploy to production? (y/N) y
```

Typing anything other than `y` or `Y` aborts execution. Confirmation is automatically skipped:

- In CI environments (when the `CI` environment variable is `"true"` or `"1"`)
- When the `--yes` (`-y`) flag is passed

The `confirm` prompt is shown verbatim — it is not run through `$(...)` substitution, so dynamic values like
`$(ARGS.env)` would appear literally in the prompt.

---

## MCP Server (AI Agents)

Runfile ships a built-in [Model Context Protocol](https://modelcontextprotocol.io) server that exposes every public
target as a callable tool, so AI coding agents (Claude Code, Cursor, Claude Desktop, Codex, Junie, …) can list and
invoke them with arguments.

### Inspect

`run :mcp inspect` prints the JSON tool definitions that the server would advertise — useful for debugging which targets
and arguments are visible to an agent. Internal targets (names starting with `_`) are excluded; named arguments are
inferred from `$(ARGS.x)` and `$(FLAGS.x)` references in the target's commands.

```
$ run :mcp inspect
{
  "tools": [ ... ]
}
```

### Server

`run :mcp server` starts the MCP server on **stdio**, intended to be spawned by an agent rather than run by hand. The
Runfile path is captured at startup (respecting `-f` / `RUNFILE_TARGET` / auto-discovery) so tool calls execute against
the same Runfile they were enumerated from.

### Install

`run :mcp install <agent>` writes (or updates) the MCP-server snippet for a given agent, so you don't have to hand-edit
JSON config files.

| Agent            | Effect                                                                      |
|------------------|-----------------------------------------------------------------------------|
| `claude-code`    | Writes `.claude/settings.local.json` in the current directory.              |
| `cursor`         | Writes `.cursor/mcp.json` in the current directory.                         |
| `claude-desktop` | Prints instructions + the snippet to paste into the Claude Desktop config.  |
| `codex`          | Prints instructions + the snippet for Codex.                                |
| `junie`          | Prints instructions + the snippet for Junie (JetBrains AI).                 |
| *(any other)*    | Prints generic instructions + the snippet to paste into the agent's config. |
| *(no argument)*  | Lists the supported agents and prints the generic snippet.                  |

For agents that auto-install, an existing `runfile` entry under `mcpServers` is updated in-place; other entries are
preserved.

---

## Bootstrapping a New Project

`run :init` creates a starter `Runfile.json` in the current directory (or at `-p <path>`) with a couple of example
targets so you can start filling in your own. The command refuses to overwrite an existing file.

```
$ run :init
Created Runfile.json

$ run :init -p ci/Runfile.json
Created ci/Runfile.json
```

The generated file uses `if "$(RUN.shell) == ..."` to print a hello-world greeting in whichever shell ends up running
it (PowerShell, cmd, fish, or POSIX), demonstrating how to write a single target that works across platforms.

Use `run :convert package-json` or `run :convert makefile` instead when you already have a `package.json` or `Makefile`
to import targets from.

---

## Editor Integration

Runfile can generate run configurations for your editor so targets appear as clickable tasks.

### VS Code

```
$ run :generate vscode-tasks
```

Generates (or updates) `.vscode/tasks.json` with one task per Runfile target. Existing user-added fields are preserved
on update. Targets that use `$(ARGS)` patterns get an `${input:args}` variable appended so VS Code prompts for
arguments. Every generated task is invoked with `--stdin-args` so any unsupplied `$(ARGS.x)` / `$(ENV.X)` /
`$(FLAGS.x)` value is prompted in the integrated terminal at run time.

### Zed

```
$ run :generate zed-tasks
```

Generates (or updates) `.zed/tasks.json` with one task per Runfile target. Existing user-added fields are preserved on
update. Targets that use `$(ARGS)` get `$ZED_CUSTOM_ARGS` appended so Zed prompts for arguments. Every generated task
is invoked with `--stdin-args` so any unsupplied `$(ARGS.x)` / `$(ENV.X)` / `$(FLAGS.x)` value is prompted in the Zed
terminal at run time.

### JetBrains (IntelliJ, CLion, RustRover, WebStorm, etc.)

```
$ run :generate jetbrains-run-configurations
```

Generates Shell Script run configurations (one `.xml` file per target) in the `.run/` directory. Each configuration runs
`run --stdin-args <target>` with the working directory set to `$PROJECT_DIR$` — JetBrains run configs are static (no
per-invocation parameter UI), so `--stdin-args` covers targets that need user input by prompting at run time for any
unsupplied `$(ARGS.x)` / `$(ENV.X)` / `$(FLAGS.x)` value. Re-generating upgrades configs created by older Runfile
versions in place.

Options:

| Flag                        | Description                                                     |
|-----------------------------|-----------------------------------------------------------------|
| `-f`, `--file <path>`       | Use a specific Runfile instead of auto-discovery                |
| `-o`, `--output-dir <path>` | Write configurations to a custom directory (defaults to `.run`) |

Re-running the command updates existing configurations that were generated by Runfile. If a file already exists but has
a different configuration name or runs a different command, it is skipped with a warning.

---

## Full Example

```json
{
	"$schema": "./schema/v0.schema.json",
	"targets": {
		"build": {
			"description": "Build the project",
			"commands": [
				"npm run build $(ARGS)"
			]
		},
		"dev": {
			"description": "Start development server",
			"commands": [
				"echo Starting dev server...",
				"npm run dev"
			],
			"env": {
				"PORT": "$(ARGS.port ? 5000)",
				"NODE_ENV": "$(ARGS.env ? development)"
			}
		},
		"dev:debug": {
			"description": "Start dev server with debug logging",
			"commands": [
				"echo Starting debug server...",
				"node debug-server.js"
			],
			"env": {
				"PORT": 5000,
				"NODE_ENV": "development",
				"DEBUG": "*"
			}
		},
		"test": {
			"description": "Run test suite",
			"commands": [
				"npm test $(ARGS)"
			],
			"env": {
				"CI": true
			}
		},
		"lint": {
			"description": "Lint all source files",
			"commands": [
				"eslint src/",
				"prettier --check src/"
			],
			"addToPath": [
				"node_modules/.bin"
			],
			"ignoreErrors": true
		},
		"deploy": {
			"description": "Deploy to an environment (requires --env)",
			"commands": [
				"echo Deploying to $(ARGS.env)...",
				"./scripts/deploy.sh --target=$(ARGS.env)"
			],
			"forceShell": "bash",
			"logging": true
		},
		"clean": {
			"description": "Remove all build artifacts",
			"commands": [
				"rm -rf dist/",
				"rm -rf .cache/",
				"rm -rf node_modules/"
			],
			"ignoreErrors": true
		}
	},
	"globals": {
		"addToPath": [
			"node_modules/.bin"
		],
		"env": {
			"APP_NAME": "my-app"
		}
	}
}
```

---

## Platform Support

| Platform                     | Status    |
|------------------------------|-----------|
| Linux (x86_64, aarch64)      | Supported |
| macOS (Intel, Apple Silicon) | Supported |
| Windows 10/11                | Supported |

Runfile is built with Rust and compiles to a native binary on all platforms. No runtime or interpreter required.

---

## License

MIT
