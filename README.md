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
      "env": { "PORT": "{{ ARG.port ? '3000' }}" }
    },
    "build": {
      "description": "Type-checks and builds in parallel",
      "commands": [
        "@type-check",
        "vite build",
        {
          "if": "{{ RUN.os == 'windows' && FLAG.wsl }}",
          "then": "wsl --shell-type login -- vite build"
        }
      ],
      "envFiles": [".env", ".env.{{ ARG.env ? 'development' }}"],
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

```powershell
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

**5. Keep it up to date:**

```bash
$ run :update                     # update to the latest release in place
$ run :update --version v0.19.0   # or pin a specific release tag
```

`:update` re-runs the install script for your platform, replacing the binary where it already lives. npm-managed
installs are handled too: on Linux/macOS `:update` runs `npm install -g @runfile/cli@latest` for you; on Windows it
prints that command to run from a fresh shell (npm can't overwrite the running `run.exe` there).

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
$ run :env init .env.production
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
$ run :env inject .env -- node app.js                     # one file
$ run :env inject .env .env.local -- pnpm dev             # multiple files, last wins; parent env always wins
$ run :env inject .env.production -- ./deploy.sh          # encrypted values auto-decrypted
```

In CI, the file path can be supplied via `RUNFILE_ENV_FILE_TARGET` (set automatically by the
[setup action's `env-file-source` input](.github/actions/setup/action.yml) for open-source
repos that keep their encrypted `.env` in a GitHub secret):

```bash
$ run :env inject -- node app.js                          # uses $RUNFILE_ENV_FILE_TARGET
```

#### Powerful argument substitution

Positional, named, flags, env vars, and runtime context (`{{ RUN.os }}` / `{{ RUN.arch }}` / `{{ RUN.shell }}` /
`{{ RUN.cwd }}` / `{{ RUN.file }}` / `{{ RUN.parent }}`) — with chained fallbacks and required values. The
`{{ ... }}` syntax avoids collisions with shell `$(...)` command substitution.

The source prefixes are `{{ ARG.<name> }}` (named arg), `{{ ENV.<name> }}`, `{{ FLAG.<name> }}`,
`{{ VAR.<name> }}`, and `{{ RUN.<key> }}`. Bare `{{ ARGS }}` expands to **all** positional arguments.

```jsonc
"PORT": "{{ ARG.port ? ENV.PORT ? '3000' }}",
"OPTS": "{{ FLAG.release ? '--release' : }} {{ FLAG.verbose ? '-v' : }}",
"CARGO_TARGET_DIR": "target-{{ RUN.os }}",
// Inline OS/shell branches — the boolean DSL goes inside a single `{{ ... }}` block:
{ "if": "{{ RUN.os == 'windows' }}", "then": ["del /S /Q build"], "else": ["rm -rf build"] },
// User-supplied flags branch through `{{ FLAG.x }}` (resolves to "true" / "false"):
{ "if": "{{ FLAG.debug }}", "then": ["./tool --verbose"], "else": ["./tool"] },
// workingDirectory is a free-form path (defaults to {{ RUN.parent }}):
"workingDirectory": "{{ ARG.workdir ? RUN.cwd }}"
```

Strict format: exactly one space after `{{` and before `}}`, exactly one space around `?` and `:` operators.
**String literals must be wrapped in single quotes** — `'production'`, not `production`. Source references
(`ARG.x`, `VAR.x`, etc.) and function calls remain bare. Use `\{{` / `\}}` to emit a literal `{{` / `}}` in
your output.

#### Boolean conditions inside substitutions

A substitution body containing `==`, `!=`, `&&`, `||`, or unary `!` at top level is evaluated as a boolean
expression — the result is the string `"true"` or `"false"`. This is what powers the new `if`-block syntax,
but you can use it anywhere:

```jsonc
"commands": [
  // Branch on a CLI flag — `if` checks if the substitution resolves to the literal "true":
  { "if": "{{ ARG.env == 'production' }}",
    "then": ["./deploy-prod.sh"],
    "else": ["./deploy-staging.sh"] },

  // Compose AND/OR/NOT freely:
  { "if": "{{ ARG.env != 'development' && ARG.env != 'production' }}",
    "then": ["./deploy-staging.sh"] },

  // Invert any boolean-returning value with unary `!` — including the
  // comparison helpers (`less_than`, `is_number`, …) and `contains` etc.:
  { "if": "{{ !is_number(ARG.port) }}",
    "then": ["echo 'port must be a number' && exit 1"] },

  // FLAG.x works as a bare boolean inside DSL — no `== 'true'` needed:
  { "if": "{{ RUN.os == 'windows' && FLAG.wsl }}",
    "then": "wsl --shell-type login -- vite build" },

  // Inline in a command — useful for passing booleans to other tools:
  "my-command --resolve {{ ARG.env == 'production' }}"
]
```

**Strict boolean rule.** Any value used as a bare boolean — both inside the DSL Truthy check (e.g. `&& FLAG.x`,
`!ARG.y`) and as the entire `if` condition — must resolve to exactly one of:

- `"true"` → truthy (the `then` branch runs / left arm of `&&` continues)
- `"false"` → falsy
- `""` (empty) → falsy

Anything else (`"True"`, `"1"`, `"yes"`, `"hello"`, etc.) errors out with a clear message pointing you toward
the explicit comparison form. So `if: "{{ ARG.x }}"` and `{{ ARG.x && ... }}` only work when `ARG.x` is
exactly `"true"` / `"false"` / empty — for any other check, use a comparison: `{{ ARG.x == 'yes' }}`.

Comparisons (`==` / `!=`) operate on raw strings — *those* don't have the boolean restriction, so
`{{ ARG.env == 'staging' }}` works for any value of `ARG.env`.

#### Functions: transform values inline

Wrap any source or chain expression in a function call. Functions resolve at substitution time, can be nested,
and work as full substitution bodies *or* as chain segments:

```jsonc
"commands": [
  // Built-ins:
  //   case      : to_upper, to_lower, capitalize
  //   trim      : trim, trim_start, trim_end
  //   inspect   : length, starts_with, ends_with, contains, substring
  //   transform : escape, repeat, replace_all, remove_all
  //   regex     : regex_replace, regex_remove, regex_matches, regex_capture, regex_capture_all
  //   build     : concat, join
  //   split     : nth, first, last, count_parts
  //   path      : basename, dirname, extname, stem, join_path
  //   math      : add, subtract, multiply, divide, modulo, power, min, max, abs, round, floor, ceil
  //   compare   : less_than, less_than_or_equal, greater_than, greater_than_or_equal, is_number
  //   validate  : one_of
  //   encoding  : base64_encode, base64_decode, url_encode, url_decode
  //   hashing   : sha256, md5
  //   ids/time  : uuid, now
  //   files     : read_file, write_file, file_exists, temp_file, temp_dir
  //   json      : json_get, json_set
  //   control   : try, error
  //   shell     : shell_quote, capture
  //   variables : define
  //   cwd       : set_cwd
  "echo deploying-{{ to_upper(ARG.env) }}",
  "curl -H \"X-Auth: {{ base64_encode(ENV.TOKEN) }}\" ...",
  "echo {{ concat('hello-', ARG.name, '-2026') }}",
  "echo {{ join(' AND ', 'flag-1', 'flag-2', ARG.extra) }}",
  // Tokenise-and-rejoin-style transforms via replace_all:
  "go test {{ replace_all(ARG.flags, ' ', ' -tag=') }}",
  // Strip every match of a regex (here: collapse whitespace runs to a single space):
  "echo {{ regex_replace(ARG.text, '\\s+', ' ') }}",
  // Pull a capture group out of the first regex match — no `(?s)^.*X(...)X.*$`
  // greedy-replace trick needed. Group 0 is the whole match; group N is the
  // N-th `(...)`. Out-of-bounds returns "" (same convention as `nth`).
  "echo version={{ regex_capture(read_file('app/build.gradle.kts'), 'versionName = \"([^\"]+)\"', '1') }}",
  // Pull group N out of EVERY match and join with a separator (the "all" variant
  // of regex_capture). Here: every quoted dependency name, comma-separated.
  "echo deps={{ regex_capture_all(read_file('deps.txt'), 'name = \"([^\"]+)\"', '1', ',') }}",
  // Split-by-separator scalar accessors — string in, string out, no list type:
  // basename idiom (last segment after `/`); empty string when input ends in `/`.
  "echo basename={{ last(ARG.path, '/') }}",
  // Pull out the N-th comma-separated field; out-of-bounds returns "".
  "echo target={{ nth(ARG.csv, ',', '1') }}",
  // Pair `count_parts` with `nth` to bound-check before indexing:
  // "if": "{{ count_parts(ARG.csv, ',') == '3' }}"
  // Boolean-returning helpers are valid DSL `Truthy` values — use them in `if`:
  // "if": "{{ starts_with(ARG.path, '/usr') }}"
  // "if": "{{ regex_matches(ARG.tag, '^v[0-9]+\\.[0-9]+$') }}"

  // Safely inline arbitrary content (newlines, quotes, JSON) as a CLI arg —
  // `shell_quote` picks the right quoting for the active shell:
  "some-tool --json {{ shell_quote(base64_decode(ENV.SECRET_BASE64)) }}",

  // Slurp shell-command stdout straight into a substitution. `capture` runs
  // the command through the platform's default shell (sh / cmd) and trims
  // the trailing newline. Results are memoized per-target so the same
  // capture in multiple commands runs once. `--dry-run` substitutes a
  // readable placeholder instead of spawning the shell.
  "echo built-at={{ capture('date -u +%Y-%m-%dT%H:%M:%SZ') }}",
  "{{ define(sha, capture('git rev-parse HEAD')) }}",

  // Arithmetic — variadic (2+ args), coerces strings to numbers, errors on
  // non-numeric input. The result is formatted as an integer when whole and
  // as a decimal otherwise (so `add('5', '3')` → "8", `add('5', '1.1')` →
  // "6.1"). Divide-by-zero errors out.
  "echo next-build={{ add(VAR.versionCode, '1') }}",
  "echo half={{ divide(VAR.total, '2') }}",
  // …plus modulo, power, min, max, abs, round, floor, ceil:
  "echo shard={{ modulo(ARG.id, '8') }} clamped={{ max('0', min('100', ARG.pct)) }}",

  // Path helpers (std::path semantics). basename/dirname/extname/stem/join_path:
  "echo name={{ basename(ARG.file) }} ext={{ extname(ARG.file) }}",
  "cp {{ ARG.file }} {{ join_path(ARG.outdir, basename(ARG.file)) }}",

  // Substring (char-indexed; 3rd arg = length, optional → to end). Short SHA:
  "echo short={{ substring(capture('git rev-parse HEAD'), '0', '7') }}",

  // Current UTC time. Formats: unix-timestamp, unix-millis, iso, iso-date,
  // iso-time, rfc3339, year/month/day/hour/minute/second.
  "echo built-at={{ now('iso') }} tag=build-{{ now('unix-timestamp') }}",

  // UUID (v4-shaped) for unique temp / cache names. `--dry-run` → `<uuid>`.
  "echo tmp=/tmp/job-{{ uuid() }}",

  // URL percent-encode / decode (space → %20; symmetric round-trip):
  "curl 'https://api/search?q={{ url_encode(ARG.query) }}'",

  // Numeric comparisons — coerce both args to numbers (same rules as the
  // arithmetic family, so non-numeric input errors) and return "true"/"false",
  // so they work as DSL `Truthy` values in `if` conditions:
  // "if": "{{ greater_than(VAR.count, '50') }}"
  // "if": "{{ less_than_or_equal(ARG.retries, '3') }}"
  // `is_number` tests whether a value parses as a (finite) number — it returns
  // "false" instead of erroring on non-numeric input:
  // "if": "{{ is_number(ARG.port) }}"

  // Validate a value against a fixed allow-list. Returns the value on
  // match; lists every valid option in the error on mismatch. Collapses
  // the four-case `match { major: define(part, 'major'), ... }` boilerplate.
  "{{ define(part, one_of(ARGS, 'major', 'minor', 'patch', 'build')) }}",

  // Cache keys / content fingerprints. `sha256` for security-sensitive use,
  // `md5` for cheap fingerprinting (NOT cryptographically secure).
  "echo cache-key={{ sha256(read_file('package-lock.json')) }}",

  // Branch on file presence without forking a shell. `file_exists` returns
  // the literal "true" / "false", so it doubles as a DSL truthy value.
  // "if": "{{ file_exists('.env.local') }}"

  // Read a file inline — relative paths anchor to the Runfile directory.
  // Use `try(...)` to recover from missing files.
  "echo version={{ try(read_file('VERSION')) ? '0.0.0-dev' }}",

  // Write a file straight from a substitution — pairs with `read_file` for
  // read-modify-write pipelines (version bumps, config edits, …). Returns
  // an empty string, so a line containing only this call is dropped silently
  // (same convention as `define`). Goes through Rust's `std::fs::write`, so
  // it sidesteps the shell entirely — safe for large content, content with
  // quotes, embedded newlines, anything that would break a
  // `printf > file`-style redirect on Windows. `--dry-run` skips the write.
  "{{ write_file('build.gradle.kts', regex_replace(read_file('build.gradle.kts'), 'versionCode = [0-9]+', 'versionCode = 43')) }}",

  // Create a temp file / dir in the OS temp directory, auto-deleted when the
  // CLI exits. `temp_file([content], [extension])` writes `content` (if given)
  // and appends `.<extension>` (leading dot optional); `temp_dir()` makes an
  // empty directory (removed recursively). `--dry-run` → `<temp_file>` /
  // `<temp_dir>` placeholders, nothing created. Capture the path once with
  // `define` and reuse it:
  "{{ define(cfg, temp_file('{\"env\":\"prod\"}', 'json')) }}",
  "tool --config {{ VAR.cfg }}",
  "{{ define(work, temp_dir()) }}",
  "git clone --depth 1 $REPO {{ VAR.work }} && build {{ VAR.work }}",

  // Pull values out of arbitrary JSON without `jq` (works on every shell):
  "echo db_host={{ json_get(read_file('config.json'), 'database.host') }}",

  // Modify a JSON document in place (returns the new compact JSON):
  "echo {{ json_set('{\"port\":3000}', 'env', 'production') }}",

  // `try(expr)` swallows inner errors. Standalone returns "" on failure;
  // chained, the next segment runs as a fallback.
  "echo {{ try(base64_decode(ARG.maybe_b64)) ? ARG.maybe_b64 }}",

  // `error('message')` fails the current command on purpose: prints the
  // message to stderr and marks the step failed — but the failure flows
  // through the normal walker, so `when: failure` / `when: always` steps
  // still run and `ignoreErrors` still suppresses it. Great for guard clauses:
  // { "if": "{{ ARG.env == 'prod' }}", "then": ["{{ error('refusing to deploy to prod from a laptop') }}"] }

  // Nested:
  "echo {{ to_upper(to_lower(ARG.x)) }}",

  // As a chain fallback:
  "echo host={{ ARG.host ? to_lower(ENV.HOST) }}",

  // Capture a value with `define(...)` and read it later via `VAR.<name>`:
  "{{ define(sdk, ENV.SDK ? '/opt/sdk') }}",
  "{{ VAR.sdk }}/bin/build",
  "echo using sdk at {{ VAR.sdk }}",

  // `set_cwd(path)` switches the cwd subsequent commands spawn in — like
  // shell `cd`, but works on every shell / OS without forking. Relative
  // paths chain (matching `cd a; cd b` → `a/b`); absolute paths replace.
  "{{ set_cwd(ARG.subproject ? 'packages/api') }}",
  "npm install",
  "npm run build",

  // Single-quoted strings interpolate nested {{ }} substitutions:
  "{{ define(cmd, 'docker compose -f {{ VAR.compose }} pull') }}",
  "{{ VAR.cmd }}"
]
```

`define(name, value)` returns an empty string and stores `value` in a run-wide map; a command line that resolves
to only whitespace (i.e. one consisting solely of a `{{ define(...) }}` call) is silently skipped instead of
being dispatched to the shell. `define`s in a parent target are visible to `@target` children. The `name` MUST
be a bareword identifier matching `[A-Za-z_][A-Za-z0-9_-]*` — quotes are NOT allowed on the name.

#### Declaring variables up front (`vars`)

Instead of (or alongside) `define(...)`, you can declare variables directly on a target — or on `globals` to
apply them to every target — and read them as `{{ VAR.<key> }}`. This mirrors `env`: each value is a `{{ ... }}`
template resolved **after** the env is built (so it can reference `{{ ENV.* }}`), plus `{{ ARG.* }}`,
`{{ FLAG.* }}`, `{{ RUN.* }}`, and earlier vars. If a reference has no default and isn't supplied, it errors —
exactly like everywhere else.

```jsonc
{
  "globals": {
    "vars": { "appName": "skiley" }      // available to every target
  },
  "targets": {
    "deploy": {
      "env": { "REGION": "us-east-1" },
      "vars": {
        "region": "{{ ENV.REGION }}",     // vars resolve after env
        "retries": "3",                    // numbers/bools are stringified
        "tag": "{{ ARG.tag ? 'latest' }}" // chain fallbacks work
      },
      "commands": ["echo deploying {{ VAR.appName }} to {{ VAR.region }} as {{ VAR.tag }}"]
    }
  }
}
```

Global `vars` are merged into each target's `vars` at parse time (target keys win on conflict). Declared vars are
**scoped per-target like `env`**: a parent's vars are visible inside an `@target` dependency, but a dependency's
own declared vars don't leak back to the parent. A runtime `define(...)` of the same name overrides the declared
value for the rest of the target. Var keys must match `[A-Za-z_][A-Za-z0-9_-]*`.

`set_cwd(path)` is the cwd analog of `define`: returns an empty string and changes the cwd that subsequent shell
commands in the *current target* spawn in. Behaves like shell `cd`, but works uniformly across every shell / OS
(no forked process, no `cd` binary needed). Resolution rules:

- **Absolute path** → fully replaces the current override (matches `cd /abs`).
- **Relative path** → joins onto the existing override, or onto the target's `workingDirectory` if no override
  has been set yet. So `set_cwd('a'); set_cwd('b')` lands subsequent commands in `<workingDirectory>/a/b`,
  matching how `cd a; cd b` chains in a shell.

`set_cwd` is **per-target**: each `@target` invocation starts with a clean override (the dispatched target sees
its own `workingDirectory`, not the caller's `set_cwd` state). Inside `parallel: true` targets each leaf
captures the override at the moment of its substitution, so siblings don't race on the spawn cwd. Like
`define`, the side effect is skipped on the redacted-logging pass so log lines don't double-apply it. With
`sameShell: true`, only the *final* `set_cwd` value applies (everything joins into one shell invocation) — use
shell `cd` directly between leaves there if you need intermediate cwd changes.

Function args are separated by `, ` (comma + exactly one space).

**Quote semantics inside `{{ ... }}`:**

- **Single quotes (`'...'`) — interpolated string**: surrounding quotes stripped, with any nested `{{ ... }}`
  resolved through the regular substitution machinery. Use these for almost every literal — they handle the
  full range of values including embedded substitutions: `concat('a, b', ARG.x)`,
  `'/var/log/{{ RUN.os }}.log'`, `define(cmd, 'docker -f {{ VAR.compose }} pull')`.
- **Double quotes (`"..."`) — fully literal value**: the quote characters are part of the value (so `"test"` is
  the 6-character string `"test"`). No interpolation. Useful when the literal you want to emit really should
  contain `"` chars.
- **Plain barewords are rejected** — `{{ ARG.env ? development }}` is a parse-time error. Wrap in quotes.

#### Conditionals and loops, no shell required

Drop `if` / `for` / `match` blocks straight into your `commands` array. Conditions, loops, and value dispatch are
evaluated by Runfile itself, so the logic works the same on every shell and platform.

```jsonc
"commands": [
  { "if": "{{ ARG.env == 'production' }}",
    "then": ["./deploy-prod.sh"],
    "else": ["./deploy-staging.sh"] },

  { "match": "{{ ARG.tier ? '1' }}",  // chain default; case "1" runs when --tier missing
    "cases": {
      "1": "flutter emulators --launch Tier_1_Android_9",
      "2": "flutter emulators --launch Tier_2_Android_11",
      "3": "flutter emulators --launch Tier_3_Android_14",
      // Case keys wrapped in `/.../` are treated as regex patterns.
      // Literal cases always win over a regex that would also match.
      "/^v\\d+$/": "echo version-tag",
      "/^pr-\\d+$/": "echo preview-build"
    } },                                // unknown values error out, listing the valid cases

  { "for": "service",
    "in": ["api", "web", "worker"],
    "parallel": true,
    // VAR.<name>_index exposes the 0-based iteration counter.
    "do": ["echo [{{ VAR.service_index }}] docker build -t {{ VAR.service }} services/{{ VAR.service }}"] },

  { "for": "file",
    "shell": "git diff --name-only HEAD~1",
    "do": ["clang-format -i {{ VAR.file }}"] }
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
  { "for": "ns", "in": "namespaces", "do": "@?{{ VAR.ns }}:adb-forward" } // skip namespaces without the target
]
```

#### Readable parallel output

When commands run in parallel (`"parallel": true` on a target or `for` block), each branch's stdout/stderr is
line-buffered, prefixed with a colored bracketed label identifying the branch, and stripped of cursor-control
escapes — so progress-bar redraws (`docker compose pull`, `cargo build`, etc.) become chronological append-only
lines instead of corrupting each other's output. The label shows the full resolved `@target` invocation
(`[@dev --port 5000]`) for target-call branches, or the raw command truncated to 12 characters (`[docker compo]`)
for shell branches; each label gets one of six cycling colors so adjacent branches stay distinct.
SGR colors flow through unchanged. The prefix propagates through `@target` invocations too,
so monorepo fan-outs like `{ "for": "ns", "in": "namespaces", "do": "@{{ VAR.ns }}:dev" }` tag every nested shell
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

Pass `--stdout` to any of these to print the generated configuration to standard output instead of
writing it to disk — handy for previewing, piping, or diffing in CI. The output is the freshly
generated config (not merged with any existing file on disk) and nothing is created or modified.
For `jetbrains-run-configurations`, which produces one file per target, each block is delimited by a
`<!-- .run/<file> -->` comment when more than one is emitted.

By default only the local Runfile's own targets are generated. Two independent flags widen the set,
and both compose with `--stdout` and the on-disk writers:

- `--include-namespaces` also generates entries for targets pulled in via `includes` — namespaced
  targets carry their `namespace:` prefixes, exactly as `run :list` shows them (e.g. `run api:build`).
- `--include-globals` also generates entries for the global user-level Runfiles registered with
  `run :config global-files` — the same ones `run :list` folds in. Handy when an editor integration
  should offer your machine-wide targets (e.g. `run c`, `run backup-git`) alongside a project's own.

Skip a target from the generated configs by setting `metadata.excludeFromGenerateCommand: true`. The
`metadata` block on `globals` and on each target is **fully open** — any property of any JSON type
(strings, numbers, booleans, arrays, deeply-nested objects) is accepted and round-trips untouched, so
editor extensions and CI scripts can stash arbitrary tooling-specific fields here.

Every file Runfile writes honors your project's [**`.editorconfig`**](https://editorconfig.org): the
output is formatted to match the settings that apply to each written path — the editor files above
(`.vscode/tasks.json`, `.zed/tasks.json`, `.run/*.run.xml`) as well as the `Runfile.json` produced by
`:init` and `:convert`. `indent_style` / `indent_size` / `tab_width` drive indentation, `end_of_line`
sets the newline sequence, `insert_final_newline` and `trim_trailing_whitespace` are applied, and
`charset = utf-8-bom` prepends a BOM. When no `.editorconfig` applies, the previous defaults are kept.

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

## Development

```bash
run setup          # one-time per clone: activates the committed git hooks (.githooks/)
run build          # debug build
run check          # non-mutating gate: fmt --check + clippy (deny warnings) + cargo check
run lint           # auto-format + clippy
run test           # full workspace test suite
```

---

## License

MIT
