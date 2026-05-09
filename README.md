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
      "env": { "PORT": "{{ ARGS.port ? 3000 }}" }
    },
    "build": {
      "description": "Type-checks and builds in parallel",
      "commands": [
        "@type-check",
        "vite build",
        {
          "if": "{{ RUN.os == 'windows' && FLAGS.wsl }}",
          "then": "wsl --shell-type login -- vite build"
        }
      ],
      "envFiles": [".env", ".env.{{ ARGS.env ? development }}"],
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
npm install -g @runfile/cli
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

| Feature                                                             | Runfile | Make | Just | Taskfile |
|---------------------------------------------------------------------|:-------:|:----:|:----:|:--------:|
| Encrypted env vars, built-in (AES-256-GCM)                          |    ✅    |  ❌   |  ❌   |    ❌     |
| MCP server (AI agent integration)                                   |    ✅    |  ❌   |  ❌   |    ❌     |
| `when:` blocks for success/failure/always cleanup                   |    ✅    |  ❌   |  ❌   |    ❌     |
| Inline OS / shell / cwd / paths / debug branching via `{{ RUN.* }}` |    ✅    |  ❌   |  ❌   |    ❌     |
| Detached background execution                                       |    ✅    |  ❌   |  ❌   |    ❌     |
| IDE task generation (VS Code / Zed / JetBrains)                     |    ✅    |  ❌   |  ❌   |    ❌     |
| Migrate from `package.json` / `Makefile`                            |    ✅    |  ❌   |  ❌   |    ❌     |
| Force-kill process tree on SIGINT (GUI apps)                        |    ✅    |  ❌   |  ❌   |    ❌     |
| Stdio log tailers (`extendStdio`)                                   |    ✅    |  ❌   |  ❌   |    ❌     |
| First-class PowerShell / cmd.exe                                    |    ✅    |  ❌   |  ✅   |    ❌     |
| Fish shell support                                                  |    ✅    |  ❌   |  ✅   |    ❌     |
| Per-target shell override                                           |    ✅    |  ❌   |  ✅   |    ❌     |
| Strict parsing (typos are errors)                                   |    ✅    |  ❌   |  ✅   |    ❌     |
| Argument substitution with chained fallbacks                        |    ✅    |  ❌   |  ✅   |    ❌     |
| JSON Schema autocomplete in editors                                 |    ✅    |  ❌   |  ❌   |    ✅     |
| Watch mode, built-in                                                |    ✅    |  ❌   |  ❌   |    ✅     |
| Output grouping / prefixing in parallel mode                        |    ✅    |  ❌   |  ❌   |    ✅     |
| Native Windows (no WSL / msys2 needed)                              |    ✅    |  ❌   |  ✅   |    ✅     |
| Shell completions                                                   |    ✅    |  ❌   |  ✅   |    ✅     |
| Private / internal targets (`_name`)                                |    ✅    |  ❌   |  ✅   |    ✅     |
| Built-in string functions (upper, replace, trim, regex, base64, …)  |    ✅    |  ❌   |  ✅   |    ✅     |
| Parallel execution                                                  |    ✅    |  ✅   |  ❌   |    ✅     |
| Single static binary                                                |    ✅    |  ✅   |  ✅   |    ✅     |
| Pattern rules (`%.o: %.c`)                                          |    ❌    |  ✅   |  ❌   |    ❌     |
| Preconditions / status checks                                       |    ❌    |  ❌   |  ❌   |    ✅     |
| Incremental builds (sources / timestamps / checksums)               |    ❌    |  ✅   |  ❌   |    ✅     |

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
$ run :env set .env.production DB_PASS            # omit value → read from stdin (no shell history, no escaping)
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

Positional, named, flags, env vars, and runtime context (`{{ RUN.os }}` / `{{ RUN.arch }}` / `{{ RUN.shell }}` /
`{{ RUN.cwd }}` / `{{ RUN.file }}` / `{{ RUN.parent }}`) — with chained fallbacks and required values. The
`{{ ... }}` syntax avoids collisions with shell `$(...)` command substitution.

```jsonc
"PORT": "{{ ARGS.port ? ENV.PORT ? '3000' }}",
"OPTS": "{{ FLAGS.release ? '--release' : }} {{ FLAGS.verbose ? '-v' : }}",
"CARGO_TARGET_DIR": "target-{{ RUN.os }}",
// Inline OS/shell branches — the boolean DSL goes inside a single `{{ ... }}` block:
{ "if": "{{ RUN.os == 'windows' }}", "then": ["del /S /Q build"], "else": ["rm -rf build"] },
// User-supplied flags branch through `{{ FLAGS.x }}` (resolves to "true" / "false"):
{ "if": "{{ FLAGS.debug }}", "then": ["./tool --verbose"], "else": ["./tool"] },
// workingDirectory is a free-form path (defaults to {{ RUN.parent }}):
"workingDirectory": "{{ ARGS.workdir ? RUN.cwd }}"
```

Strict format: exactly one space after `{{` and before `}}`, exactly one space around `?` and `:` operators.
**String literals must be wrapped in single quotes** — `'production'`, not `production`. Source references
(`ARGS.x`, `VARS.x`, etc.) and function calls remain bare. Use `\{{` / `\}}` to emit a literal `{{` / `}}` in
your output.

#### Boolean conditions inside substitutions

A substitution body containing `==`, `!=`, `&&`, `||`, or unary `!` at top level is evaluated as a boolean
expression — the result is the string `"true"` or `"false"`. This is what powers the new `if`-block syntax,
but you can use it anywhere:

```jsonc
"commands": [
  // Branch on a CLI flag — `if` checks if the substitution resolves to the literal "true":
  { "if": "{{ ARGS.env == 'production' }}",
    "then": ["./deploy-prod.sh"],
    "else": ["./deploy-staging.sh"] },

  // Compose AND/OR/NOT freely:
  { "if": "{{ ARGS.env != 'development' && ARGS.env != 'production' }}",
    "then": ["./deploy-staging.sh"] },

  // FLAGS.x works as a bare boolean inside DSL — no `== 'true'` needed:
  { "if": "{{ RUN.os == 'windows' && FLAGS.wsl }}",
    "then": "wsl --shell-type login -- vite build" },

  // Inline in a command — useful for passing booleans to other tools:
  "my-command --resolve {{ ARGS.env == 'production' }}"
]
```

**Strict boolean rule.** Any value used as a bare boolean — both inside the DSL Truthy check (e.g. `&& FLAGS.x`,
`!ARGS.y`) and as the entire `if` condition — must resolve to exactly one of:

- `"true"` → truthy (the `then` branch runs / left arm of `&&` continues)
- `"false"` → falsy
- `""` (empty) → falsy

Anything else (`"True"`, `"1"`, `"yes"`, `"hello"`, etc.) errors out with a clear message pointing you toward
the explicit comparison form. So `if: "{{ ARGS.x }}"` and `{{ ARGS.x && ... }}` only work when `ARGS.x` is
exactly `"true"` / `"false"` / empty — for any other check, use a comparison: `{{ ARGS.x == 'yes' }}`.

Comparisons (`==` / `!=`) operate on raw strings — *those* don't have the boolean restriction, so
`{{ ARGS.env == 'staging' }}` works for any value of `ARGS.env`.

#### Functions: transform values inline

Wrap any source or chain expression in a function call. Functions resolve at substitution time, can be nested,
and work as full substitution bodies *or* as chain segments:

```jsonc
"commands": [
  // Built-ins:
  //   case      : to_upper, to_lower, capitalize
  //   trim      : trim, trim_start, trim_end
  //   inspect   : length, starts_with, ends_with, contains
  //   transform : escape, repeat, replace_all, remove_all
  //   regex     : regex_replace, regex_remove, regex_matches
  //   build     : concat, join
  //   split     : nth, first, last, count_parts
  //   encoding  : base64_encode, base64_decode
  //   shell     : shell_quote
  //   variables : define
  "echo deploying-{{ to_upper(ARGS.env) }}",
  "curl -H \"X-Auth: {{ base64_encode(ENV.TOKEN) }}\" ...",
  "echo {{ concat('hello-', ARGS.name, '-2026') }}",
  "echo {{ join(' AND ', flag-1, flag-2, ARGS.extra) }}",
  // Tokenise-and-rejoin-style transforms via replace_all:
  "go test {{ replace_all(ARGS.flags, ' ', ' -tag=') }}",
  // Strip every match of a regex (here: collapse whitespace runs to a single space):
  "echo {{ regex_replace(ARGS.text, '\\s+', ' ') }}",
  // Split-by-separator scalar accessors — string in, string out, no list type:
  // basename idiom (last segment after `/`); empty string when input ends in `/`.
  "echo basename={{ last(ARGS.path, '/') }}",
  // Pull out the N-th comma-separated field; out-of-bounds returns "".
  "echo target={{ nth(ARGS.csv, ',', '1') }}",
  // Pair `count_parts` with `nth` to bound-check before indexing:
  // "if": "{{ count_parts(ARGS.csv, ',') == '3' }}"
  // Boolean-returning helpers are valid DSL `Truthy` values — use them in `if`:
  // "if": "{{ starts_with(ARGS.path, '/usr') }}"
  // "if": "{{ regex_matches(ARGS.tag, '^v[0-9]+\\.[0-9]+$') }}"

  // Safely inline arbitrary content (newlines, quotes, JSON) as a CLI arg —
  // `shell_quote` picks the right quoting for the active shell:
  "some-tool --json {{ shell_quote(base64_decode(ENV.SECRET_BASE64)) }}",

  // Nested:
  "echo {{ to_upper(to_lower(ARGS.x)) }}",

  // As a chain fallback:
  "echo host={{ ARGS.host ? to_lower(ENV.HOST) }}",

  // Capture a value with `define(...)` and read it later via `VARS.<name>`:
  "{{ define(sdk, ENV.SDK ? '/opt/sdk') }}",
  "{{ VARS.sdk }}/bin/build",
  "echo using sdk at {{ VARS.sdk }}",

  // Single-quoted strings interpolate nested {{ }} substitutions:
  "{{ define(cmd, 'docker compose -f {{ VARS.compose }} pull') }}",
  "{{ VARS.cmd }}"
]
```

`define(name, value)` returns an empty string and stores `value` in a run-wide map; a command line that resolves
to only whitespace (i.e. one consisting solely of a `{{ define(...) }}` call) is silently skipped instead of
being dispatched to the shell. `define`s in a parent target are visible to `@target` children. The `name` MUST
be a bareword identifier matching `[A-Za-z_][A-Za-z0-9_-]*` — quotes are NOT allowed on the name.

Function args are separated by `, ` (comma + exactly one space).

**Quote semantics inside `{{ ... }}`:**

- **Single quotes (`'...'`) — interpolated string**: surrounding quotes stripped, with any nested `{{ ... }}`
  resolved through the regular substitution machinery. Use these for almost every literal — they handle the
  full range of values including embedded substitutions: `concat('a, b', ARGS.x)`,
  `'/var/log/{{ RUN.os }}.log'`, `define(cmd, 'docker -f {{ VARS.compose }} pull')`.
- **Double quotes (`"..."`) — fully literal value**: the quote characters are part of the value (so `"test"` is
  the 6-character string `"test"`). No interpolation. Useful when the literal you want to emit really should
  contain `"` chars.
- **Plain barewords are rejected** — `{{ ARGS.env ? development }}` is a parse-time error. Wrap in quotes.

#### Conditionals and loops, no shell required

Drop `if` / `for` / `match` blocks straight into your `commands` array. Conditions, loops, and value dispatch are
evaluated by Runfile itself, so the logic works the same on every shell and platform.

```jsonc
"commands": [
  { "if": "{{ ARGS.env == 'production' }}",
    "then": ["./deploy-prod.sh"],
    "else": ["./deploy-staging.sh"] },

  { "match": "{{ ARGS.tier ? '1' }}",  // chain default; case "1" runs when --tier missing
    "cases": {
      "1": "flutter emulators --launch Tier_1_Android_9",
      "2": "flutter emulators --launch Tier_2_Android_11",
      "3": "flutter emulators --launch Tier_3_Android_14"
    } },                                // unknown values error out, listing the valid cases

  { "for": "service",
    "in": ["api", "web", "worker"],
    "parallel": true,
    "do": ["docker build -t {{ VARS.service }} services/{{ VARS.service }}"] },

  { "for": "file",
    "shell": "git diff --name-only HEAD~1",
    "do": ["clang-format -i {{ VARS.file }}"] }
]
```

#### Call other targets inline with `@target`

Any command string starting with `@` invokes another target. Forward args with `{{ ARGS }}` or pass anything explicit.
Each invocation runs (no dedup), inherits the parent's env, and works inside `if` / `for` blocks. Parallel parents
fan out target calls onto worker threads.

Prefix with `@?target` to mark the call **optional**: if the (substituted) target doesn't exist, it's silently
skipped instead of erroring. Useful with `for in: "namespaces"` when the dispatched target isn't defined in every
namespace.

```jsonc
"commands": [
  "@lint",
  "@test --coverage",
  "@build {{ ARGS }}",
  { "if": "{{ RUN.os == 'windows' }}", "then": "@deploy-win", "else": "@deploy-unix" },
  { "for": "ns", "in": "namespaces", "do": "@?{{ VARS.ns }}:adb-forward" } // skip namespaces without the target
]
```

#### Readable parallel output

When commands run in parallel (`"parallel": true` on a target or `for` block), each branch's stdout/stderr is
line-buffered, prefixed with its step number `[N]`, and stripped of cursor-control escapes — so progress-bar
redraws (`docker compose pull`, `cargo build`, etc.) become chronological append-only lines instead of corrupting
each other's output. SGR colors flow through unchanged. The prefix propagates through `@target` invocations too,
so monorepo fan-outs like `{ "for": "ns", "in": "namespaces", "do": "@{{ VARS.ns }}:dev" }` tag every nested shell
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

#### Single-shell mode with `sameShell`

By default each step runs in its own shell process, so `cd`, exported variables, and other shell state don't carry
over between steps. Set `"sameShell": true` to join every step into a single shell invocation — state changes
persist for free.

```jsonc
{
  "deploy": {
    "sameShell": true,
    "commands": ["cd ci-scripts/", "./ci-deploy.sh"]
  }
}
```

Steps are joined with `&&` so the run stops at the first failure (or `;` / `&` when `ignoreErrors: true`). `if`,
`for`, and `match` blocks are evaluated by Runfile and their chosen branches flow into the same joined invocation.
`@target` calls inside the body are rejected — they have their own shell context and can't share state with the
parent. `sameShell` also composes with `detach: true` (joined command spawns as one detached process) and is
available on `globals` for project-wide defaults.

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
confirmation prompts · `--dry-run` · `--stdin-args` (interactive prompting for missing inputs) · `--timings` ·
force-kill on SIGINT (works for GUI apps like Unity) · stdio tailers · shell completions · MCP server.

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
