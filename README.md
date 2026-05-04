# Runfile

[Quick start](#quick-start) · [Why Runfile?](#why-runfile) · [Features](#what-makes-it-different) · [Platforms](#platform-support) · [Docs](.github/DOCS.md)

**One JSON file. One binary. Every OS. Every shell.**

A modern command runner that replaces Makefiles, shell scripts, and `npm run` — without the platform headaches.

```bash
$ run dev --port=4000
```

```jsonc
{
  "$schema": "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json",
  "targets": {
    "dev": {
      "description": "Starts the dev server",
      "commands": "vite",
      "env": { "PORT": "$(ARGS.port ? 3000)" }
    },
    "build": {
      "description": "Type-checks and builds in parallel",
      "commands": [
        "@type-check",
        "vite build",
        {
          "if": "$(RUN.os) == windows && $(FLAGS.wsl) == true",
          "then": "wsl --shell-type login -- vite build"
        }
      ],
      "envFiles": [".env", ".env.$(ARGS.env ? development)"]
      "parallel": true
    },
    "type-check": {
      "description": "Type-checks source code",
      "commands": "tsc --noEmit"
    }
  }
}
```

---

## Quick start

**1. Install:**

npm:

```bash
npm install -g @skiley/runfile
```

Linux / macOS:

```bash
curl -fsSL https://github.com/Skiley/runfile/releases/latest/download/install.sh | sh
```

Windows (via PowerShell):

```bash
iwr https://github.com/Skiley/runfile/releases/latest/download/install.ps1 | iex
```

**2. Generate a starter `Runfile.json`** in your project root:

```bash
$ run :init
```

**3. List and run targets:**

```bash
$ run :list
$ run build
$ run test --release
$ run dev --port=4000
```

**4. Add tab completion** (optional):

```bash
$ run :completions install bash # or zsh, fish, powershell
```

---

## Why Runfile?

Other runners assume one platform and one shell. Runfile assumes **none**.

Sorted from where Runfile is most differentiated to where it lags behind.

| Feature                                               | Runfile | Make | Just | Taskfile |
|-------------------------------------------------------|:-------:|:----:|:----:|:--------:|
| Encrypted env vars, built-in (AES-256-GCM)            |    ✅    |  ❌   |  ❌   |    ❌     |
| MCP server (AI agent integration)                     |    ✅    |  ❌   |  ❌   |    ❌     |
| `when:` blocks for success/failure/always cleanup     |    ✅    |  ❌   |  ❌   |    ❌     |
| Inline OS / shell / debug branching via `$(RUN.*)`    |    ✅    |  ❌   |  ❌   |    ❌     |
| Detached background execution                         |    ✅    |  ❌   |  ❌   |    ❌     |
| IDE task generation (VS Code / Zed / JetBrains)       |    ✅    |  ❌   |  ❌   |    ❌     |
| Migrate from `package.json` / `Makefile`              |    ✅    |  ❌   |  ❌   |    ❌     |
| Force-kill process tree on SIGINT (GUI apps)          |    ✅    |  ❌   |  ❌   |    ❌     |
| Stdio log tailers (`extendStdio`)                     |    ✅    |  ❌   |  ❌   |    ❌     |
| First-class PowerShell / cmd.exe                      |    ✅    |  ❌   |  ✅   |    ❌     |
| Fish shell support                                    |    ✅    |  ❌   |  ✅   |    ❌     |
| Per-target shell override                             |    ✅    |  ❌   |  ✅   |    ❌     |
| Strict parsing (typos are errors)                     |    ✅    |  ❌   |  ✅   |    ❌     |
| Argument substitution with chained fallbacks          |    ✅    |  ❌   |  ✅   |    ❌     |
| JSON Schema autocomplete in editors                   |    ✅    |  ❌   |  ❌   |    ✅     |
| Watch mode, built-in                                  |    ✅    |  ❌   |  ❌   |    ✅     |
| Native Windows (no WSL / msys2 needed)                |    ✅    |  ❌   |  ✅   |    ✅     |
| Parallel execution                                    |    ✅    |  ✅   |  ❌   |    ✅     |
| Shell completions                                     |    ✅    |  ❌   |  ✅   |    ✅     |
| Private / internal targets (`_name`)                  |    ✅    |  ❌   |  ✅   |    ✅     |
| Single static binary                                  |    ✅    |  ✅   |  ✅   |    ✅     |
| Pattern rules (`%.o: %.c`)                            |    ❌    |  ✅   |  ❌   |    ❌     |
| Output grouping / prefixing in parallel mode          |    ❌    |  ❌   |  ❌   |    ✅     |
| Preconditions / status checks                         |    ❌    |  ❌   |  ❌   |    ✅     |
| Incremental builds (sources / timestamps / checksums) |    ❌    |  ✅   |  ❌   |    ✅     |
| Built-in string functions (upper, replace, trim, …)   |    ❌    |  ❌   |  ✅   |    ✅     |

---

## What makes it different

#### Cross-shell, not just cross-platform

Auto-detects the shell, or pin one per target. The same Runfile works for a teammate using `bash` on Linux and another
using PowerShell on Windows.

```jsonc
{
  "targets": {
    "ps":   { "commands": ["Write-Host 'hi'"], "forceShell": "powershell" },
    "bash": { "commands": ["echo $BASH_VERSION"], "forceShell": "bash" }
  }
}
```

#### Encrypted env vars, baked in

AES-256-GCM secrets you can commit to git. Auto-decrypted at runtime, never written to disk in plaintext. No external
tool, no wrapper.

```bash
$ run :env init -p .env.production
$ run :env set .env.production DB_PASS "s3cr3t"   # auto-encrypted
```

```bash
RUNFILE_ENCRYPTION_PUBLIC_KEY=9f86d081...
DB_PASS=encrypted:BASE64_CIPHERTEXT...
```

You can also use Runfile as a drop-in `dotenvx`/`dotenv` replacement to inject env vars (encrypted or plaintext) into
any command — no Runfile needed:

```bash
$ run :env inject -- node app.js                          # uses .env by default
$ run :env inject -f .env -f .env.local -- pnpm dev       # multiple files, last wins; parent env always wins
$ run :env inject -f .env.production -- ./deploy.sh       # encrypted values auto-decrypted
```

#### Powerful argument substitution

Positional, named, flags, env vars, and runtime context (`$(RUN.os)` / `$(RUN.shell)`) — with chained fallbacks and
required values.

```jsonc
"PORT": "$(ARGS.port ? ENV.PORT ? 3000)",
"OPTS": "$(FLAGS.release ? --release :) $(FLAGS.verbose ? -v :)",
"CARGO_TARGET_DIR": "target-$(RUN.os)",
// Inline OS/shell branches:
{ "if": "$(RUN.os) == windows", "then": ["del /S /Q build"], "else": ["rm -rf build"] },
// User-supplied flags branch through $(FLAGS.x):
{ "if": "$(FLAGS.debug) == true", "then": ["./tool --verbose"], "else": ["./tool"] }
```

#### Conditionals and loops, no shell required

Drop `if` / `for` blocks straight into your `commands` array. Conditions and loops are evaluated by Runfile itself,
so the logic works the same on every shell and platform.

```jsonc
"commands": [
  { "if": "$(ARGS.env) == production",
    "then": ["./deploy-prod.sh"],
    "else": ["./deploy-staging.sh"] },

  { "for": "service",
    "in": ["api", "web", "worker"],
    "parallel": true,
    "do": ["docker build -t $(LOOP.service) services/$(LOOP.service)"] },

  { "for": "file",
    "shell": "git diff --name-only HEAD~1",
    "do": ["clang-format -i $(LOOP.file)"] }
]
```

#### Call other targets inline with `@target`

Any command string starting with `@` invokes another target. Forward args with `$(ARGS)` or pass anything explicit.
Each invocation runs (no dedup), inherits the parent's env, and works inside `if` / `for` blocks. Parallel parents
fan out target calls onto worker threads.

Prefix with `@?target` to mark the call **optional**: if the (substituted) target doesn't exist, it's silently
skipped instead of erroring. Useful with `for in: "namespaces"` when the dispatched target isn't defined in every
namespace.

```jsonc
"commands": [
  "@lint",
  "@test --coverage",
  "@build $(ARGS)",
  { "if": "$(RUN.os) == windows", "then": "@deploy-win", "else": "@deploy-unix" },
  { "for": "ns", "in": "namespaces", "do": "@?$(LOOP.ns):adb-forward" } // skip namespaces without the target
]
```

#### Readable parallel output

When commands run in parallel (`"parallel": true` on a target or `for` block), each branch's stdout/stderr is
line-buffered, prefixed with its step number `[N]`, and stripped of cursor-control escapes — so progress-bar
redraws (`docker compose pull`, `cargo build`, etc.) become chronological append-only lines instead of corrupting
each other's output. SGR colors flow through unchanged. The prefix propagates through `@target` invocations too,
so monorepo fan-outs like `{ "for": "ns", "in": "namespaces", "do": "@$(LOOP.ns):dev" }` tag every nested shell
with its branch identity. Set `RUNFILE_NO_LINE_PREFIX=1` to opt out and inherit raw stdio.

```text
[runfile] (1/3) [parallel] docker compose pull api
[runfile] (2/3) [parallel] docker compose pull web
[runfile] (3/3) [parallel] docker compose pull worker
[1] Pulling api ... 50%
[2] Pulling web ... 30%
[3] Pulling worker ... 100%
[1] Pulling api ... 100%
[2] Pulling web ... 100%
```

#### Cleanup on failure / always with `when:` blocks

Wrap any commands in a `when:` block to run them only after a failure, or always — interleaved with the rest of
`commands`, no separate `before`/`after` arrays.

```jsonc
"commands": [
  "./run-tests.sh",                                          // failure flips state
  "./report.sh",                                             // skipped on failure (default when:success)
  { "when": "failure", "commands": ["./post-failure.sh"] },  // runs only after a failure
  { "when": "always",  "commands": ["@cleanup"] }            // runs every time
]
```

#### Watch mode, built in

Add a `watch` array to any target. No flags, no extra tooling.

```jsonc
"watch": ["src/**/*.rs", "!target/**"]
```

#### Editor integrations

Generate native tasks for **VS Code**, **Zed**, and **JetBrains** IDEs from your Runfile:

```bash
$ run :generate vscode-tasks
$ run :generate zed-tasks
$ run :generate jetbrains-run-configurations
```

#### Migrate in seconds

```bash
$ run :convert package-json   # turns package.json scripts into targets
$ run :convert makefile       # turns Makefile recipes into targets
```

#### Plus

Parallel & detached execution · file includes with optional namespacing for monorepos
(`pnpm --recursive --parallel`-style aggregation) · cycle detection · global Runfiles · path aliases ·
confirmation prompts · `--dry-run` · `--timings` · force-kill on SIGINT (works for GUI apps like Unity) ·
stdio tailers · shell completions · MCP server.

---

## Platform Support

| Platform              | Status    |
|-----------------------|-----------|
| Linux (x86-64)        | Supported |
| Linux (ARM64)         | Supported |
| macOS (Apple Silicon) | Supported |
| macOS (Intel)         | Supported |
| Windows 10            | Supported |
| Windows 11            | Supported |
| Windows Server        | Supported |

Single static binary. No runtime, no interpreter, no package manager.

---

## Documentation

The full reference — every property, every flag, every subcommand — lives in [DOCS.md](.github/DOCS.md).

- [CLI usage and subcommands](.github/DOCS.md#cli-usage)
- [Runfile.json reference](.github/DOCS.md#runfilejson-reference)
- [Arguments and substitution](.github/DOCS.md#arguments-and-substitution)
- [Encrypted environment variables](.github/DOCS.md#encrypted-environment-variables)
- [Internal targets](.github/DOCS.md#internal-targets)
- [`when:` blocks](.github/DOCS.md#when-guarded-blocks-when)
- [Watch mode](.github/DOCS.md#watch-mode)
- [Editor integration](.github/DOCS.md#editor-integration)

---

## License

MIT
