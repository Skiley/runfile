# CLAUDE.md — Project Reference for Runfile

## What is Runfile

Runfile is a cross-platform command runner (a modern Makefile alternative). Users define targets in a `Runfile.json`
file and execute them via the `runfile` CLI. It is written entirely in Rust, compiles to a single
binary, and works on Linux, macOS, and Windows with support for Bash, Zsh, Sh, Fish, PowerShell, and cmd.exe.

## Build & Test

NEVER use `cargo` commands!

ALWAYS use `run <target>`, read `Runfile.json` to see available targets.

```
run build                  # Debug build, both Windows and Linux (via WSL)
run install                # Links the debug build to the global "run" command
run lint                   # Formats, checks and lints the code, both using Windows and Linux (via WSL) due to OS-specific macros
run test                   # Runs all tests, both using Windows and Linux (via WSL)
```

## Project Layout

```
Cargo.toml                     # Workspace root — defines all 5 crates and shared deps
Runfile.json                   # Our own Runfile (self-hosting / bootstrapping)
README.md                      # Public-facing documentation for users
schemas/v0.schema.json         # JSON Schema for Runfile.json (editor autocomplete)

crates/
  runfile-parser/              # Runfile.json discovery and parsing
  runfile-shell/               # Shell detection, types, and resolution
  runfile-settings/            # Local user settings (~/.config/runfile/)
  runfile-crypto/              # AES-256-GCM encryption/decryption for env vars
  runfile-env/                 # Environment variable building (env files, merging, PATH, decryption)
  runfile-executor/            # Command execution, args substitution
  runfile-cli/                 # CLI binary that wires everything together
```

## Crate Responsibilities

### runfile-parser

**Files:** `schema.rs`, `discover.rs`, `parse.rs`, `merge.rs`, `dsl.rs`, `tests.rs`

- Defines the Runfile schema as Rust types: `Runfile`, `CommandSpec`, `Globals`, `EnvValue`, `WhenStep`,
  `WhenCondition`, `ExtendStdio`, `StdioStream`, `CommandStep`, `IfStep`, `ForStep`, `MatchStep`, `TargetCallStep`,
  `IncludeEntry`
- Conditional configuration is expressed
  via `{{ RUN.os }}` / `{{ RUN.shell }}` substitution in scalar fields (env values, `forceShell`,
  `workingDirectory`, etc.) plus `if` / `when` / `@target` composition inside `commands`.
- All structs use `#[serde(deny_unknown_fields)]` to enforce strict parsing
- `discover.rs` walks up from the current directory to find `Runfile.json` or `Runfile.json5` (`.json` takes priority
  over `.json5` within the same directory)
- `parse.rs` uses the `json5` crate for deserialization (JSON5 is a superset of JSON — supports comments, trailing
  commas, unquoted keys, single-quoted strings). After deserialization, runs validation (non-empty schema, at least
  one target, no empty command lists, env keys, aliases, non-empty `WhenStep.commands`, literal `workingDirectory`
  values when not a `{{ ... }}` template). `@target` references are NOT validated at parse time — they are checked at
  runtime, because included files may define targets not yet available. `parse_runfile_partial()` skips the
  `NoTargets` check (used for included files and global settings files).
- The root JSON key is `"targets"` (not `"commands"` — that was renamed). Each target has a `"commands"` array inside
  it.
- `merge.rs`: `bake_globals_into_target()` merges `Globals` into each `CommandSpec` at parse time so the runtime model
  has no globals. Merge semantics: `envFiles`/`addToPath` are prepended, `env` is deep-merged (target overrides same
  keys), scalar fields (forceShell, logging, ignoreErrors, etc.) use target-if-set-else-global. After merging,
  `runfile.globals` is set to `None` — downstream code never sees globals. `merge_runfiles()` handles multi-file
  includes with target conflict resolution. As part of the same pass, target-level relative `addToPath` entries are
  baked to absolute paths against `source_dir` (the source Runfile's parent dir). This gives `addToPath` the same
  "anchor to runfile parent" semantics as `envFiles`, decoupled from the runtime `workingDirectory`. Globals'
  `addToPath` was already baked the same way; the target-side baking just extends the rule. `envFiles` are NOT
  baked (they're substitution templates) — they're resolved at runtime via `EnvBuildParams::env_files_base_dir`,
  which the runner / extract pipeline always sets to the target's source dir.
- `merge.rs` (cont.): include entries (`IncludeEntry`) are either a plain path string or
  `{ path, namespace? }`. When a namespace is set, `apply_namespace_to_state()` rewrites every target name and alias
  in that include's sub-state, plus every `@target` reference inside its command tree (`rewrite_target_calls_in_steps`
  walks `Shell`/`TargetCall`/`When`/`If`/`For` recursively). Rewrites are applied **innermost-first** so nested
  includes compose: child includes `inner` as `inner`, parent includes child as `child` → child's `@inner:build` ends
  up as `@child:inner:build`. `resolve_includes` builds a fresh `MergeState` per include, recurses into its own
  sub-includes (which apply their own namespaces first), inserts the include's own targets, applies *this* level's
  namespace, then folds the sub-state into the parent via `MergeState::merge_from`. Cycle detection in `resolve_includes`
  uses `visited` as a per-call-stack set (insert before recursing, remove after) so sibling-loads of the same file
  ("diamond" includes) work — and the same file can be included twice under different namespaces, yielding two
  independent copies. Empty/absent `namespace` is normalised to `None` at `IncludeEntry` deserialize time and is
  equivalent to the legacy string form. Namespace validation: non-empty, no `:` or whitespace, no leading `@`/`:`/`_`.
- `MergeState.namespaces: Vec<String>` accumulates every namespace that's been applied. `apply_namespace_to_state`
  prefixes the existing entries with the include's own namespace and pushes the new namespace onto the list, so
  a chain `outer → inner → leaf` ends up tracking `outer`, `outer:inner`, and `outer:inner:leaf`.
  `merge_from` extends sibling lists; `merge_runfiles_inner` sorts + dedupes once at the end and places the
  result on `Runfile.namespaces` (a `#[serde(skip)]` field — never serialized; populated only by the merge
  step). The runner attaches this list to `RunArgs.run_context.namespaces` via `Arc<Vec<String>>` so
  `for "in": "namespaces"` resolves at execution time without threading another parameter through the executor.
- Control-flow blocks (`if` / `for` / `match` / `when`) and target calls (`@target`): each entry of a `commands`
  array is a [`CommandStep`] — either `Shell(String)` (raw command),
  `TargetCall(TargetCallStep)` (a string starting with `@`), `When(WhenStep)`, `If(IfStep)`, `For(ForStep)`, or
  `Match(MatchStep)`.
  Backwards-compatible: an existing string entry deserializes as `CommandStep::Shell` unless it starts with `@`.
  `IfStep`'s `condition` is just a substitution template — the if-block evaluator (in
  `runfile-executor::control_flow::evaluate_if_condition`) substitutes the value at runtime and checks if the
  result equals the literal string `"true"`. The DSL form is reachable inside the substitution itself
  (`{{ ARGS.env == 'prod' }}` resolves to `"true"` or `"false"`); see "DSL inside substitutions" below.
  `CommandSpec.commands`, `WhenStep.commands`, `IfStep.then`, `IfStep.else`, and `ForStep.body` all
  accept either a bare string (sugar for a one-element array) or a `Vec<CommandStep>` — custom
  `deserialize_steps_or_string` / `deserialize_optional_steps_or_string` helpers in `schema.rs` handle the shorthand
  at parse time, so the in-memory shape always normalizes to `Vec<CommandStep>`. `ForStep` requires exactly one of
  `in`/`glob`/`shell` (XOR validated at parse time) and has an optional `parallel` flag. `ForStep.in` is a custom
  `ForInValue` enum: `Literal(Vec<String>)` for the array form (each element substitutable), or `Namespaces` for
  the magic string `"in": "namespaces"` which expands at execution time to every namespace prefix declared via
  `includes` — composed across nesting (`outer:inner`), sorted, deduplicated. `ForInValue` has hand-rolled
  `Serialize` / `Deserialize` impls so it round-trips cleanly: literal arrays → JSON array, `Namespaces` → the
  string `"namespaces"`. Any other string value is a hard parse error. Free function `walk_step_templates(steps, &mut visit)` recursively yields every leaf
  template string (used by IDE generators, MCP, args-usage scanning). The companion `walk_spec_aux_templates(spec, &mut visit)`
  yields every other substitutable string on a `CommandSpec` — `env` string values, `envFiles` paths, `forceShell`,
  `addToPath` entries, `workingDirectory`, `confirm`, and `extendStdio.fromFile` — so arg-usage scanners (e.g. the runner's
  `validate_args` collector) recognise `{{ ARGS.x }}` / `{{ FLAGS.x }}` references that live outside `commands`. `From<&str>` and
  `From<String>` impls let callers use `"foo".into()` ergonomically; `CommandSpec::new_shell(Vec<String>)` is a convenience
  constructor for string-only command lists.
- DSL parsing (`dsl.rs`): tiny boolean expression language for `if` conditions. Hand-written tokenizer + recursive
  descent parser, no external deps. Grammar: comparisons (`==`, `!=`), logical operators (`&&`, `||`, `!`), parens,
  substitution leaves (`{{ ARGS.x }}` / `{{ ENV.X }}` / `{{ FLAGS.x }}` / `{{ VARS.x }}`), quoted strings, bare-words. Mixing
  `&&` and `||` in the same expression is a hard parse error — parens are required to disambiguate. Parsing happens
  eagerly during `validate_runfile`, so syntax errors surface at Runfile load time.
- `MatchStep` (multi-way dispatch): `{ "match", "cases", "default"?, "ignoreErrors"?, "when"? }`. Stored on
  `CommandStep::Match`. The `match` field is a substitution template — same pipeline as any other `{{ ... }}`
  reference, so chained fallbacks work (`{{ ARGS.tier ? ENV.TIER ? 1 }}`). `cases` is a `BTreeMap<String,
  Vec<CommandStep>>` (sorted; alphabetical iteration order in error messages); each case body accepts the usual
  string-or-array sugar via the same `Value`-based deserializer pattern as `IfStep.then`. `default` is an
  optional branch run when no case matches. Validation lives in `parse.rs::validate_match_step`: empty
  match expression and empty cases-without-default are hard errors. Runtime semantics (in
  `control_flow::resolve_match_branch`): substitute `match`; on substitution failure fall through to `default`
  if set, else surface `ControlFlowError::MatchValueUnresolved` with valid cases listed; on substitution
  success look up the value, run the case if found, else fall through to `default` or surface
  `ControlFlowError::MatchNoCase`. Cases are looked up by exact string equality. `ignoreErrors`/`when` mirror
  `IfStep`. `count_leaves` sums all cases + default (worst-case, like `if`'s both-branches counting).
  `walk_step_templates` visits the `match` template and every leaf in cases + default for static analysis.
  `collect_leaves_parallel_with_when`, `collect_detach_leaves_inner`, and `walk_extract_steps` all dispatch
  via `resolve_match_branch` so only the chosen branch is collected — same approach as `if`. Namespace
  rewriting (`rewrite_target_calls_in_steps`) recurses into every case body and the default branch.
- Internal targets: `is_internal_target_name(name)` (free function in `schema.rs`) returns true when the **last**
  `:`-separated segment of a target's canonical name starts with `_`. The "last segment" rule is what makes namespaced
  internals (`api:_helper`) keep their internal status when an include applies a namespace.
  `Runfile::is_internal(name)` resolves any name (canonical or alias) to the canonical name and applies the same
  check, so aliases on internal targets also report as internal. `Runfile::public_target_names()` returns target
  names + aliases excluding internals; `all_target_names()` still returns everything. Internal-ness is purely
  name-based — there is no schema field for it. Validation accepts internal target names; only `:`-prefixed names
  are reserved.

### runfile-shell

**Files:** `types.rs`, `detect.rs`, `resolve.rs`, `tests.rs`

- `ShellKind` enum: Bash, Zsh, Sh, Fish, PowerShell, Cmd
- `ResolvedShell`: a kind + path pair, with `exec_args()` that returns the correct flag (`-c`, `-Command`, `/C`)
- `detect.rs`: auto-detects the default shell per platform (checks `$SHELL` on Unix, well-known paths on Windows)
- `resolve.rs`: resolves a shell by name from known paths or `which`. When the requested shell is `sh` and
  resolution fails (no `/bin/sh`, no `sh` on PATH — common on Windows and minimal containers), falls back to
  other sh-compatible shells in order: bash → zsh → fish. The returned `ResolvedShell.kind` is the actual shell
  that was found (e.g. `Bash`), so `{{ RUN.shell }}` reflects what's really running rather than the requested name.
  Targets that hard-code `forceShell: "sh"` for `cp`/`echo`/etc. now Just Work cross-platform without an
  `if RUN.os == windows` branch.

### runfile-settings

**Files:** `settings.rs`, `paths.rs`, `tests.rs`

- `Settings` struct with `shell_paths`, `path_aliases`, `global_files` (HashMap/Vec), and `secret_keys: Vec<String>`
- `secret_keys`: stores hex-encoded AES-256 private keys (not named — matched by public key fingerprint)
- Platform paths: Linux `~/.config/runfile/`, macOS `~/Library/Application Support/runfile/`, Windows
  `%APPDATA%\runfile\`
- Load returns defaults if file doesn't exist; save creates parent dirs automatically

### runfile-crypto

**Files:** `lib.rs`, `tests.rs`

- Standalone crate for AES-256-GCM encryption/decryption of environment variable values
- `generate_key()`: generates a random 256-bit key as a 64-character hex string
- `encrypt()`/`decrypt()`: encrypt/decrypt using `encrypted:<base64(nonce||ciphertext||tag)>` format
- `is_encrypted()`/`has_encrypted_values()`: detect encrypted values by `encrypted:` prefix
- `decrypt_env_values()`: bulk-decrypt all encrypted values in a `HashMap<String, String>`
- `derive_public_key()`: SHA-256 hash of private key bytes → public key fingerprint (64 hex chars)
- `find_matching_private_key()`: matches a public key against a list of private keys
- `find_private_key_by_public_prefix()`: finds a private key by matching its derived public key against a hex prefix
- Dependencies: `aes-gcm`, `base64`, `rand`, `hex`, `sha2`, `thiserror`

### runfile-env

**Files:** `lib.rs`, `parse.rs`, `tests.rs`

- `parse_env_file()`: parses `.env` file contents into key-value pairs. Supports `#`/`//` comments,
  single/double/unquoted values, multiline quoted values, escape sequences in double quotes, `export` prefix, inline
  comments.
- `load_env_files()`: loads multiple env files with substitution in file paths (via a caller-provided closure), relative
  path resolution, and silent skipping of missing files. The base directory for resolving relative paths is the
  caller's `env_files_base_dir` (passed via `EnvBuildParams`) — always the source Runfile's parent dir, never the
  resolved `workingDirectory`.
- `build_env()`: main orchestration via `EnvBuildParams` struct. Merge order (low → high):
  (1) `base_env` (system env for top-level, parent's resolved env for `@dep`) → (2) `envFiles` (substitution sees the
  env_map built so far; later files win per key) → (3) **decrypt encrypted file values** (so the env block sees
  plaintext — see below) → (4) `env` (substituted; wins over envFiles within the Runfile layer) → (5)
  **`overlay_shell_env`** re-applies `std::env::vars()` so the inherited shell env ALWAYS wins over Runfile-defined
  keys (PATH is case-aware on Windows so we don't end up with both `Path` and `PATH`) → (6) `apply_add_to_path_chain`
  prepends `[this target's add_to_path…, parent's add_to_path…, grandparent's…, current PATH]` so the innermost
  `addToPath` ends up at the very front and the chain re-prepends after step 5 wiped PATH → (7) **final decrypt
  pass** as a defensive backstop in case a later step (env block, shell overlay) introduced an `encrypted:...`
  value. Accepts a substitution closure so it stays independent of arg parsing.

  The decrypt-before-env-block ordering matters: it makes `"env": { "X": "{{ base64_decode(ENV.SECRET_BASE64) }}" }`
  work when `SECRET_BASE64` is encrypted in `.env.production` — the env block sees the decrypted base64 string
  (not the literal `encrypted:abc...` form), so `base64_decode` operates on real input. Without this, the env
  block would see the encrypted form and any post-processing (`base64_decode`, `==` comparisons, function calls)
  would error. The system env (`base_env`) supplies `RUNFILE_ENCRYPTION_PUBLIC_KEY` if it's not in the file, so
  shell-set keys still work for the early decryption pass. `EnvBuildParams` has data fields: `env_files`, `env`, `add_to_path`, plus `parent_add_to_path_chain` for
  threading ancestor `addToPath` layers through `@dep` invocations (no global/command distinction — globals are baked
  into each target by the parser). Two distinct path inputs: `working_dir` (the resolved `workingDirectory`, used as
  the spawn dir and as the base for relative `addToPath` entries) and `env_files_base_dir` (the source Runfile's
  parent dir, used as the base for relative `envFiles`). Decoupling these means a target with
  `workingDirectory: "subdir"` still loads `envFiles: [".env"]` from the Runfile dir, not from `subdir/`.
- `apply_add_to_path_chain` is a no-op when both the parent chain (or its layers) and this target's `add_to_path`
  contribute zero entries — so single-target runs and unused chains never touch the `PATH` value or perturb its case.
- Substitution semantics intentionally stay "lexical": within a target's `env` block, a value can reference a key set
  earlier in the same block (via `{{ ENV.X }}`) and gets that lexically-prior value, even if the shell's value will
  ultimately win in step 4. This keeps existing Runfiles working — the only observable change is the final value of
  any key the shell also defines.
- `EnvBuildParams.available_private_keys`: optional list of private keys; when encrypted values are detected, the key is
  auto-resolved via `RUNFILE_ENCRYPTION_KEY` env var or by matching `RUNFILE_ENCRYPTION_PUBLIC_KEY` against the
  available private keys.
- `check_env_case_duplicates()`: validates no env var keys differ only by casing.
- `collect_runfile_env()`: collects only Runfile-defined env vars (not system), sorted by key. Takes a single
  `Option<&HashMap<String, String>>` (no global/command distinction).
- Re-exports `is_encrypted` and `has_encrypted_values` from `runfile-crypto` for convenience.
- Does NOT depend on `runfile-parser` — receives raw `HashMap<String, String>` and `&[String]` slices. The caller
  converts `EnvValue` types before passing them.

### runfile-executor

**Files:** `args.rs`, `control_flow.rs`, `env.rs`, `executor.rs`, `force_kill.rs`, `logging.rs`,
`parallel_output.rs`, `runner.rs`, `stdio_tailer.rs`, `tests.rs`

- `RunArgs`: parses CLI args into positional (`{{ ARGS }}`) and named (`--key=value`). Carries a `run_context: RunContext`
  field used to resolve `{{ RUN.* }}` substitutions; populated by the CLI via `RunArgs::parse(...).with_run_context(...)`.
  Also carries an optional `stdin_prompter: Option<Arc<dyn StdinPrompter>>` — when set (top-level CLI flag
  `--stdin-args`), missing `{{ ARGS.* }}` / `{{ ENV.* }}` / `{{ FLAGS.* }}` references trigger a stdin prompt instead of
  erroring. The prompter trait lives in `args.rs` alongside `InteractiveStdinPrompter` (the default impl that
  reads stdin, writes prompts to stderr, and caches answers in `Mutex<HashMap>`s). `Arc` cloning shares the
  cache, so the prompter propagates through `@target` calls (via `RunnerDependencyResolver::run_dependency`,
  which clones `parent.stdin_prompter`) without re-asking the user. Tests use a mock `StdinPrompter` to script
  scripted answers. The `vars: Arc<Mutex<HashMap<String, String>>>` field is the run-wide store for
  `{{ define(...) }}` side effects — read by `{{ VARS.<name> }}` chain segments, shared via Arc-clone across
  `@target` calls and parallel worker threads.
- `substitute()` returns `Result` — `{{ ARGS.key }}` without `?` errors if arg is missing; `{{ ARGS.key ? }}` with empty
  right-side defaults to empty string; `{{ ARGS.key ? default }}` uses the default. The substituter walks the
  template once, resolving every `{{ ... }}` block it finds while leaving everything else (including bash
  `$(...)` command substitutions) untouched — so `echo $(date)` passes through verbatim, and
  `$(echo "{{ ARGS.env }}")` becomes `$(echo "development")` (the inner `{{ ... }}` resolves; the outer
  `$(...)` is opaque to Runfile). Strict whitespace: exactly one space after `{{` and before `}}`, exactly
  one space around `?` and `:`. Use `\{{` / `\}}` to emit a literal `{{` / `}}` in the output.
  `scan_args_usage` mirrors this so `validate_args` recognises `--env` even when its only reference is
  nested inside a shell `$(...)`.
- `RunContext { os, shell, cwd, file, parent, namespaces }`: static execution context. Resolves all `{{ RUN.* }}`
  keys: `os` (`"windows"` / `"linux"` / `"mac"`), `shell` (`"bash"` / `"zsh"` / `"sh"` / `"fish"` / `"powershell"` /
  `"cmd"`), `cwd` (caller's current working dir, absolute path), `file` (source Runfile path of the
  currently-executing target), `parent` (directory of `file`). Unknown `RUN.<key>` is a hard error.
  Participates in chained fallbacks (`{{ ARGS.shell ? RUN.shell }}`). The runner calls `ensure_run_context()`
  per target so `shell` / `file` / `parent` stay accurate when a target-level `forceShell` swaps the effective
  shell, or when a target was defined in an included or global Runfile. `cwd` is captured once at top level and
  doesn't change. `forceShell` and `workingDirectory` themselves go through substitution before resolution —
  e.g. `"forceShell": "{{ ARGS.shell ? bash }}"` works.
- `LoopVarGuard`: RAII helper used by the executor and extract walker to scope a `for`-loop iteration variable
  into the run-wide `VARS` map. `enter(&vars, name)` captures the prior value of `VARS.<name>` (if any); each
  iteration calls `set(value)` to overwrite; on drop, the prior value is restored (or removed if there was no
  prior). This gives lexical scoping for free: outer `VARS.x` is preserved while a `for x in [...]` runs, and
  nested loops with the same variable name compose correctly. `{{ VARS.x }}` participates in chained fallbacks
  (`{{ ARGS.x ? VARS.y ? default }}`); missing `VARS` refs error like missing `ARGS`.
- **Quote-strict literals**: under the new substitution syntax, every string literal *inside a `{{ ... }}` block*
  must be wrapped in single quotes (`'...'`) — bareword literals are rejected with
  [`SubstitutionError::BarewordLiteralNotAllowed`]. Source references (`ARGS.x`, `VARS.x`, etc.) and function
  calls remain bare. Two quote forms exist:
  - **Single quotes (`'...'`) — interpolated string**: stripped at evaluation, with any nested `{{ ... }}` blocks
    inside resolved through the regular substitution machinery. So `'docker -f {{ VARS.compose }} pull'`
    becomes `docker -f services/web.yml pull`. Required if the literal contains `,`/`(`/`)`/`?`. The
    substitution walker ([`find_substitution_close`]) is quote/depth-aware so the outer `}}` doesn't close on
    a `}}` that's part of a single-quoted nested substitution.
  - **Double quotes (`"..."`) — fully literal**: the quote characters are part of the value (so `"foo"` is the
    5-character string `"foo"`). No interpolation inside. Rare-but-useful when you need the actual `"` chars in
    your output. The splitter still treats `"..."` as a grouping boundary so commas / `?` inside don't split.
  - The ONLY exception to the quote rule is the first argument of `define` — the var-name — which is always a
    bareword identifier. `define('x', ...)` and `define("x", ...)` are both rejected as `InvalidVarName`.
- **Function calls** (`{{ <funcname>(arg1, arg2, ...) }}`): identifier matches `[a-z][a-z0-9_]*` so it can't collide
  with the uppercase source prefixes; `(` immediately follows the name; arguments are separated by `, ` (comma + one
  space — strict whitespace, mirroring the chain `?` and FLAGS `:` rules). Built-in registry (in
  `evaluate_function`): `to_upper(s)`, `to_lower(s)`, `capitalize(s)` (uppercase the first char of every
  whitespace-separated word; via `capitalize_words`, leaves internal capitals alone),
  `trim(s)` / `trim_start(s)` / `trim_end(s)` (strip whitespace per Rust's `str::trim*`,
  i.e. all `char::is_whitespace`), `length(s)` (Unicode scalar count via `chars().count()`, NOT byte count),
  `starts_with(haystack, needle)` / `ends_with(haystack, needle)` /
  `contains(haystack, needle)` (return literal `"true"`/`"false"` so they double as `Truthy` DSL values),
  `escape(s)` (backslash-escape control chars + `"`; via `escape_string`, NOT a full JSON escape — single
  quotes pass through unchanged; non-printable bytes < 0x20 emit `\xNN`),
  `repeat(s, n)` (n must parse as `usize`; surfaces as [`SubstitutionError::InvalidNumber`] otherwise),
  `replace_all(haystack, needle, replacement)` (defers to Rust's `str::replace`; an empty `needle` produces
  the replacement between every char, matching stdlib semantics),
  `remove_all(haystack, needle)` (sugar for `replace_all(s, n, '')`),
  `regex_replace(haystack, pattern, replacement)` /
  `regex_remove(haystack, pattern)` (sugar for `regex_replace(s, p, '')`) /
  `regex_matches(haystack, pattern)` (compile via `compile_regex`; pattern errors surface as
  [`SubstitutionError::InvalidRegex`]; replacement strings honour the `regex` crate's
  `$1`/`${name}` backreferences; `regex_matches` is unanchored, use `^...$` for full-string),
  `base64_encode(s)`,
  `base64_decode(s)` (errors on `InvalidBase64` / `NonUtf8Decoded`), `concat(s1, s2, ...)` (variadic, 1+ args),
  `join(sep, s1, s2, ...)` (variadic, 1+ args; `join(sep)` with no items returns `""`),
  `shell_quote(s)` (per-shell single-arg quoting via [`quote_for_shell`] dispatching on `RUN.shell` —
  POSIX/fish use `'...'` with `'\''` escape, PowerShell uses `'...'` with `''` escape, cmd uses `"..."` with
  `""` escape; lets users inline arbitrary bytes — newlines, `$`, `"`, `'`, JSON, etc. — into shell commands
  as single argv slots without env-var indirection), and `define(name, value)` (returns `""`, side effect:
  sets `VARS.name`). Functions resolve as full substitution bodies
  AND as chain segments — `{{ ARGS.host ? to_lower(ENV.HOST) }}` is a chain whose first segment is a source lookup
  and second is a function call. Args themselves are chain / quoted-literal expressions, so nested calls
  (`to_upper(to_lower(x))`) and chained args (`to_upper(ARGS.x ? 'default')`) work naturally. The chain splitter
  (`split_chain_segments`) is paren / quote aware so ` ? ` inside `(...)` or `'...'`/`"..."` doesn't split.
  Errors: `UnknownFunction`, `FunctionArity { name, expected, got }`, `InvalidBase64`, `NonUtf8Decoded`,
  `InvalidRegex { name, message }`, `InvalidNumber { name, message }`,
  `UnbalancedParens`, `BarewordLiteralNotAllowed`, plus `MalformedSubstitution` for arg-list whitespace violations.
- **DSL inside substitutions**: a substitution body containing the boolean operators `==`, `!=`, `&&`, `||`, or
  unary `!` at top level (paren / quote / nested-substitution aware — see `looks_like_dsl`) is parsed as a DSL
  expression and evaluated to the literal string `"true"` or `"false"`. Examples:
  `{{ ARGS.env == 'prod' }}` → `"true"`/`"false"`,
  `{{ ARGS.env != 'development' && ARGS.env != 'production' }}` → composite boolean,
  `{{ to_upper(ARGS.x) == 'PROD' }}` → function-call values work, `{{ !(VARS.skip == 'yes') }}` → unary negation,
  `{{ RUN.os == 'windows' && FLAGS.wsl }}` → bare `FLAGS.x` works because it resolves to `"true"`/`"false"`.
  DSL value tokens are evaluated by the same machinery as function args (single-quoted interpolates, double-quoted
  is verbatim, source refs resolve, function calls evaluate, plain barewords error). The DSL evaluator
  ([`eval_dsl_ast`]) reuses the parser's [`runfile_parser::DslExpr`] / [`runfile_parser::DslValue`] AST.
- **Strict DSL truthiness** (aligned with `if`-block rule): a `Truthy` value (anything used as a bare boolean
  inside the DSL — e.g. `FLAGS.x`, `ARGS.x`, `VARS.x`, or a quoted literal) MUST resolve to `"true"` (truthy),
  `"false"` (falsy), or `""` (falsy). Anything else surfaces as `SubstitutionError::DslValueNotBoolean`. This
  catches patterns like `{{ ARGS.env && other }}` where the user expected non-empty-truthiness but `ARGS.env`
  is some arbitrary string — the error points them at the explicit comparison form (`{{ ARGS.env == 'value' }}`).
  Comparisons (`==` / `!=`) operate on raw strings without the boolean check, so any-string equality still
  works. Short-circuiting (`&&` / `||`) still skips evaluation of later arms when the result is determined.
- **`if` block evaluation** is a thin wrapper over substitution with a strict boolean check: the `condition`
  field is a template string, [`evaluate_if_condition`] substitutes it via `args.substitute(...)`, and the
  resolved value MUST be exactly `"true"` (truthy), `"false"` (falsy), or `""` (falsy). **Anything else
  surfaces as [`ControlFlowError::IfConditionNotBoolean`]** — `"True"`, `"1"`, `"yes"`, `"hello"`, etc. all
  error out instead of being silently coerced. The strict rule catches typos and missing comparisons (someone
  writing `if: "{{ ARGS.x }}"` expecting truthiness when `ARGS.x` is "yes" gets a clear error pointing them
  toward `{{ ARGS.x == 'yes' }}`). The OLD form (`if: "{{ X }} == Y"` with operators outside the `{{ }}`) no
  longer works — migrate to `if: "{{ X == 'Y' }}"`. The if condition is not pre-parsed at Runfile load time;
  errors surface at runtime when the substitution + boolean check run. `IfStep` no longer carries a cached
  `condition_ast` field.
- **`define(name, value)` semantics**: `name` MUST be a bareword identifier matching `[A-Za-z_][A-Za-z0-9_-]*` —
  no quotes allowed. `parse_static_name` rejects substitutions, dotted names, and quoted forms with `InvalidVarName`.
  `value` is resolved through the normal arg pipeline. Resolved value is stored in `RunArgs.vars`
  (`Arc<Mutex<HashMap<String, String>>>`) — read by the `VARS.<name>` chain segment, returns `MissingVar` if not yet
  set (or falls through to the chain default). The Arc is shared across `RunArgs::clone()` so `define`s in a parent
  target are visible to `@target` children (the runner's `RunnerDependencyResolver` and the extract walker both
  thread `with_vars(parent.vars.clone())` into the child `RunArgs`). `evaluate_function` skips the mutation when
  `redact_env` is true so the redacted-pass log substitution doesn't overwrite real values with `***` between the
  real-pass and the next command. Concurrent `define`s from a `parallel: true` block serialise on the lock; relative
  ordering of writes is non-deterministic — last writer wins (documented footgun). `VARS.*` is **not** redacted in
  logs (treated like ARGS) — putting secrets in VARS leaks them to `--logging` output.
- **Empty-command skip**: when a command line resolves to a whitespace-only string (the typical cause is a line
  consisting only of `{{ define(...) }}`), it is NOT dispatched to the shell. `execute_one_shell` short-circuits
  and counts it as a successful no-op step (preserving `(N/total)` numbering); `collect_leaves_parallel_with_when`
  drops empty leaves before they reach the parallel batch; `extract_target_with_cwd` skips them in dry-run output
  so a `define`-only line doesn't show up as a blank line.
- **`FLAGS.x` in chain segments and function args**: `resolve_chain_impl` recognises `FLAGS.<key>` as a value source
  returning `"true"`/`"false"` (boolean form only — the ternary form's ` : ` would conflict with chain semantics).
  Inside a function arg, `evaluate_arg` routes `FLAGS.x [? a [: b]]` to the dedicated FLAGS resolver so the full
  ternary form works there too. Both paths thread `flag_keys` through (so `--key`-token consumption stays correct
  for `{{ ARGS }}` rebuilds).
- `control_flow.rs`: DSL evaluator + `for`-block iterator expansion. `evaluate(&DslExpr, args, env, scope)` walks
  the cached AST against the current substitution context. Truthiness rule: only `""` is falsy — `"false"`, `"0"`,
  etc. are truthy (matches what raw shell commands see). `{{ FLAGS.x }}` resolves to `"true"`/`"false"` strings, both
  non-empty, so flag presence checks must use explicit `== true`/`== false`. `expand_for_iterations` produces the
  iteration values: `ForInValue::Literal(arr)` is substituted element-wise; `ForInValue::Namespaces` snapshots
  `args.run_context.namespaces` (sorted + deduped at merge time, threaded down via `RunContext`'s
  `Arc<Vec<String>>` field — empty when no namespaced includes are configured); `glob` patterns are expanded
  against the working directory using `globset` (matches normalized to forward-slash relative paths, sorted);
  `shell` iterators run the command at planning time, capture stdout, trim each line, and drop blank lines.
  **`for shell` failure (non-zero exit) is a hard error regardless of `ignoreErrors`** — that flag controls
  *body* failures, not iterator-source failures. `count_leaves` walks a `&[CommandStep]` tree to compute the
  static step-counter total: `Shell` → 1, `TargetCall` → 1 (the runner's `collect_all_commands` recurses into
  the called target separately to size the global counter accurately; locally, each `@target` invocation
  contributes one slot from the parent's POV — and dynamic target names containing `{{ ... }}` always count as 1
  with no recursion), `If` → `then.len() + else.len()` (both branches inflate it because we don't know which
  runs), `For in: [array]` → `array.len() * body_count`, `For in: "namespaces"` → `runfile.namespaces.len() *
  body_count` (resolved at runner-level by `count_target_leaves_recursive`; the local `count_leaves` falls back
  to a 1-iteration estimate since it doesn't see the Runfile), `For glob` / `For shell` → `body_count`
  (1-iteration estimate; runtime calls `StepCounter::add_to_total` to bump the total when actual iterations
  exceed the estimate, so `N` always stays ≤ `total`).
- Target invocations (`@target args...` / `@?target args...`): a string command entry starting with `@`
  deserializes as `CommandStep::TargetCall { target, args_template, optional }`. The `@?` prefix sets
  `optional: true` and is stripped from the in-memory `target` field; the marker round-trips through serde via
  the manual `Serialize` impl on `CommandStep` (re-emits `@?` when `optional`). Optional calls silently skip
  when the (substituted) target isn't found in the merged Runfile — useful with `for in: "namespaces"` patterns
  where some namespaces don't define the dispatched target (`@?{{ VARS.ns }}:adb-forward`). The skip only suppresses
  the *missing-target* error; failures *inside* the target's commands are not silenced (use `ignoreErrors` for
  that). At execute time, **both** `target` and `args_template` go through normal substitution (so `{{ ARGS }}` /
  `{{ RUN.* }}` / `{{ ENV.* }}` / `{{ VARS.* }}` resolve), then `args_template` is `shlex`-split into argv before being
  dispatched. Substituting the target name lets dynamic patterns like `@{{ VARS.ns }}:build` (the canonical use
  case for `for in: "namespaces"`) dispatch to the right namespaced target on each iteration. The `?` character
  is reserved for the optional marker — declared target names, aliases, and `includes` namespaces are rejected
  at parse time if they contain `?` (`ParseError::TargetNameContainsQuestionMark`,
  `ParseError::AliasContainsQuestionMark`, plus the namespace check in `merge::validate_namespace`); a literal
  `?` inside a `@target` reference (e.g. `@foo?bar`) is also rejected via `validate_target_call`. Static
  analysis (the runner's `count_target_leaves_recursive`, `collect_commands_recursive`) treats names containing
  `{{` as opaque — counts as 1 leaf, no recursion into the called target — so the step counter relies on
  `add_to_total` to bump at runtime if the dispatched target exposes more leaves. **Optional calls on a
  statically-missing target contribute 0 leaves and skip recursion** (since they'll be runtime no-ops); optional
  calls on a present target recurse normally. Dynamic optional calls (`@?{{ ... }}`) still count as 1 leaf each
  (the namespace iteration count drives the for-block multiplier), and the slot is "wasted" but harmless when
  the target turns out to be missing at dispatch — total ≥ N is preserved. The executor calls back into the
  runner via the [`DependencyResolver`] trait, which now carries an `optional: bool` parameter on
  `run_dependency`; tests that don't have a runner use `NoOpDependencyResolver` (which errors on `@`).
  `@target` invocations have **no dedup** — calling the same target twice runs it twice — but cycles are still
  rejected via per-call-stack chain tracking on the post-substitution name. Each invocation inherits the
  parent's already-resolved env as a substitution base, then layers its own `envFiles`/`env` on top (dep wins
  per key) — but the **current shell env always wins** over both via the `overlay_shell_env` step. `addToPath`
  is threaded as a separate `parent_add_to_path_chain: Vec<Vec<String>>` (one layer per ancestor in chain order,
  outermost first); each call appends its own `add_to_path` and the full chain is re-prepended to PATH after
  the shell-env overlay, so PATH ends up `[innermost addToPath…, …, outermost addToPath…, shell PATH]`. The
  trait method
  `DependencyResolver::run_dependency(target, args, parent_env, parent_add_to_path_chain, optional)` carries
  all of these pieces of state. `ExecSetup` precomputes
  `add_to_path_chain = parent_chain + this target's spec.add_to_path` and hands *that* slice to every nested
  `@dep` call (innermost-first ordering is enforced by `apply_add_to_path_chain`, not by the chain layout — the
  chain is stored outermost-first as it accumulates). `forceShell` and other target-level config are NOT
  inherited. Inside `parallel: true` parents, target calls run on worker threads via `std::thread::scope` so
  the resolver can borrow runner state (and the chain slice) without `'static` lifetime requirements. The
  optional flag is forwarded to `ParallelLeaf::TargetCall { optional }` and on into the per-thread
  `run_dependency` call, so optional skip semantics work uniformly in sequential, parallel-shell, and
  parallel-`@target` contexts. Nested `parallel: true` deps fan out further (no enforced sequentialization).
- `env.rs`: thin bridge that converts `CommandSpec` (parser type) into raw data for `runfile-env`, wiring
  `RunArgs::substitute` as the substitution closure. Re-exports `EnvFileError` and `parse_env_file` from `runfile-env`.
  Passes `available_private_keys` through for automatic encrypted env decryption.
- `execute_command()`: walks `spec.commands: Vec<CommandStep>` recursively, executing each leaf shell through the
  resolved shell. `if`/`for` blocks are expanded inline: `if` substitutes its `condition` template through
  [`RunArgs::substitute`] and takes the `then` branch iff the resolved string equals `"true"` exactly. `"false"`
  and `""` (empty) take the `else` branch; **any other value errors out with `IfConditionNotBoolean`**. `for`
  calls `expand_for_iterations`, then for each
  iteration calls `LoopVarGuard::set(value)` to write `VARS.<var>`, recurses into the body, and the guard restores
  the prior `VARS.<var>` value when the loop ends. Behavior on failure
  respects `ignoreErrors` at both the target level and the per-block level (`IfStep.ignore_errors`,
  `ForStep.ignore_errors`). Thin wrapper over `execute_command_with_counter()` that creates a fresh step counter
  sized to `count_leaves(&spec.commands)` — used by tests.
- `execute_command_with_counter()`: same as above but takes an externally provided `&StepCounter` (atomics-based,
  cheaply cloneable, thread-safe), an `&dyn DependencyResolver` to handle `@target` calls, and an optional
  `parent_env` (the parent target's resolved env, used as the env-build base when this call is a dependency
  invocation). Used by the runner.
- `execute_parallel()` / `execute_parallel_with_counter()`: collects every leaf (shell command + `@target`
  invocation) with substitution applied by walking the `CommandStep` tree via `collect_leaves_parallel`. Shell
  leaves spawn via `Command::spawn()`; `@target` leaves run on worker threads via `std::thread::scope` so the
  `DependencyResolver` can borrow non-`'static` runner state. All leaves run concurrently; the outer call waits
  for processes + thread joins. Inside a parallel context, nested `for parallel: true` blocks are forced
  sequential (a warning is printed) — only the *outermost* parallel layer actually runs concurrently. Nested
  `parallel: true` *targets* (via `@target`) DO fan out further. Shell leaves use `Stdio::piped()` and route
  output through `parallel_output::spawn_line_pump` (line-buffered, prefixed, ANSI-cursor-stripped) — see
  `parallel_output.rs` below. Stdin is still inherited. **The output prefix propagates through `@target`
  calls**: the parallel batch hands each leaf's prefix to `DependencyResolver::run_dependency` via the
  `output_prefix` parameter, the runner threads it through `run_target_inner` into the dispatched target's
  `ExecSetup.output_prefix`, and from there every shell (sequential `execute_one_shell` and any nested
  parallel batch) tags its piped output with that string. Result: `dev` targets that fan out via
  `for in: "namespaces"` + `@{{ VARS.ns }}:dev` get cleanly tagged per-branch output even when the leaves are
  `@target` calls rather than direct shells. Same counter-sharing pattern.
- `parallel_output.rs`: line-buffered, prefixed stdout/stderr for parallel shell children. `spawn_line_pump`
  reads from a `ChildStdout`/`ChildStderr`, splits on `\n` and `\r` (CR-as-soft-break flattens progress-bar
  redraws into chronological append-only lines), strips non-SGR ANSI escapes (cursor movement, erase-line, OSC
  title — SGR `…m` color/style codes are preserved), and writes each completed line prefixed with `[N] ` where
  N is the global step number. Each prefix uses one of six cycling ANSI colors so adjacent leaves are visually
  distinct; honors `NO_COLOR` for plain output. `\r\n` is collapsed (the LF after a CR is swallowed) so we
  don't emit spurious empty lines. Each line is one `write_all` to the locked global stdout/stderr handle —
  prefix + content + newline lands as a single atomic write so two children can't interleave mid-line.
  `RUNFILE_NO_LINE_PREFIX=1`/`true` opts out (raw stdio inheritance). Pump threads terminate naturally on EOF
  (i.e. when the child closes its end of the pipe on exit) and are `join`-ed in the wait loop so all output is
  flushed before the function returns.
- Output-prefix inheritance rule: when a parallel batch is reached via an ancestor that already set a prefix
  (i.e. this target was dispatched as `@dep` from an outer parallel parent and `setup.output_prefix.is_some()`),
  every leaf in this batch inherits that prefix verbatim — no per-leaf step renumbering. This preserves the
  outer partition identity end-to-end: a `[3]`-tagged branch stays `[3]` even when its dispatched target is
  itself `parallel: true`. Nested differentiation isn't applied; only the outermost parallel layer is the
  source of distinct prefixes. Sequential `execute_one_shell` and the `when:failure` / `when:always`
  fallback path (`run_sequential_leaves`) honor the same inherited prefix.
- `stdio_tailer.rs`: `StdioTailerSet` manages background threads that tail log files and route complete lines to
  stdout or stderr. Used by `execute_command()` and `execute_parallel()` when `extendStdio` is configured. Polls
  files every 50ms, handles files that don't exist yet or get truncated/rotated, and only emits complete lines
  (terminated by `\n`).
- `logging.rs`: ANSI-colored output (`[runfile] (1/3) command`), enables Windows Virtual Terminal Processing for cmd.exe
  compatibility. Defines `StepCounter` — a global step counter shared across the entire run so `(N/total)` stays
  continuous through nested targets and `when:` blocks. Backed by `Arc<AtomicUsize>` so it's `Send + Sync` and
  cheaply cloneable; worker threads spawned by parallel `@target` calls share the same underlying counter. The
  total is computed once via `count_target_leaves` at the entry point (recurses into `@target` references,
  memoized per-target). `when: failure` / `when: always` blocks inflate the total — actual execution may stop
  before reaching them, in which case the last shown step number will be lower than the total.
- `runner.rs`: high-level `run_target()` function that builds env and dispatches command execution. The CLI calls
  `run_target()` instead of `execute_command()` directly. No globals threading — everything is already on the
  `CommandSpec`. `RunRoot` holds a `StepCounter` initialized to `count_target_leaves(target_name, ...)`, threaded
  through every nested `_with_counter` call so step numbers stay continuous. `forceShell` and `workingDirectory`
  are substituted (against args + parent env) before resolution. `workingDirectory` is a free-form path that
  supports `{{ ... }}` substitution; default (when unset) is `{{ RUN.parent }}` (the target's source Runfile
  directory). After substitution, relative paths are resolved against the target's source Runfile dir via
  `resolve_working_directory_path`. There is no per-value validation — any path string is accepted.
- Has a `windows-sys` dependency (Windows-only) for console ANSI support

### runfile-cli

**Files:** `main.rs`

- Clap-based CLI with colon-prefixed subcommands: `:list`, `:config`, `:env`, `:mcp`, `:completions`, `:generate`,
  `:convert`, `:init`. `--dry-run` is a top-level flag (not a subcommand) that prints the resolved leaf shell
  commands to stdout (one per line, no `[runfile]` prefix, no ANSI) instead of executing them — exactly the
  behaviour the removed `:extract` subcommand had. `--dry-run` recursively expands `@target` invocations: the
  dep's resolved leaf shell commands appear inline at the call site (with the dep's own env block reflected on
  each line), so aggregator targets whose body is purely `@target` dispatches (e.g. `for in: namespaces` with
  `@{{ VARS.ns }}:dev`) actually print every nested command rather than printing nothing. `if` blocks are
  evaluated (not flattened) against the same context the runner would see — only the matching branch is
  printed. Cycles are caught at extract time via per-call-stack tracking; sibling calls to the same target
  expand twice (matching runtime no-dedup semantics). Optional calls (`@?target`) silently skip when the target
  is missing. `extract_target_with_cwd` auto-syncs `args.run_context.namespaces` from the merged Runfile (matches
  the runner's `ensure_run_context`) so `for in: "namespaces"` resolves identically in dry-run.
- Global flags: `-f`/`--file` (custom Runfile path), `--timings` (print execution times), `-y`/`--yes` (skip
  confirms), `--stdin-args` (prompt for missing `ARGS.*`/`ENV.*`/`FLAGS.*` instead of erroring),
  `--dry-run` (print resolved leaf shell commands without executing). For inline debug branching, declare
  `{{ FLAGS.debug }}` in your target — passing `--debug` (or any other flag) to a target works through the
  standard FLAGS substitution.
- `RUNFILE_TARGET` env var: when `-f`/`--file` is not passed, this env var is used as the default Runfile path before
  falling back to auto-discovery. Defined in `runfile_helpers.rs` (`RUNFILE_TARGET_ENV_VAR`, `runfile_target_env()`,
  private `effective_file()`) and applied by `resolve_runfile_path`, `resolve_and_merge`, `cmd_list_targets` (in
  `completions.rs`), and `cmd_mcp_server`. Empty string is treated as unset. When set, the path is required to exist
  (or be a registered path alias) — no fallback to discovery — so misconfigured CI fails fast. `-f` always wins.
- Shell resolution priority: target `forceShell` (which may come from globals merge) > auto-detect. There is no
  CLI-level shell override — pin a shell per target via `forceShell` if needed.
- When no subcommand and no args: prints help. When args given: first arg is target name, rest are passed through.
- Target names starting with `:` are reserved for built-in commands. Names like `ci:build` (colon not at start) are
  allowed.
- Target names starting with `_` are *internal*: `resolve_target_setup()` in `cmd_run.rs` rejects direct invocation
  with a friendly error, `cmd_list` filters them out of `:list`, and `cmd_list_targets` (the completion source) calls
  `Runfile::public_target_names()` instead of `all_target_names()`. Internal targets remain callable via `@_name`
  from another target's `commands`. The same exclusion is applied in `runfile-mcp::tools::build_tool_defs` (so AI
  agents don't see them) and in all three IDE generators (`vscode`, `zed`, `jetbrains`). Aliases on internal targets
  are also blocked because resolution always returns the canonical name; internal-ness is determined solely by the
  canonical name starting with `_`.
- Uses `run_target()` from the runner module, not `execute_command()` directly — this ensures the `when:`-aware
  walker and `@target` resolver are always engaged.
- LLM-agent guard (`agent_detect.rs`): `refuse_if_agent(action_description)` aborts with an error if it detects an
  agent invocation. Detection signals (any one triggers): env vars `CLAUDECODE=1`, `LLM_INVOCATION=true`, or
  `AGENT_MODE=1`, or stdin not being a terminal **unless** a CI environment is detected. CI detection lives in
  `ci_detect.rs` (extracted so the same logic gates other CI-only paths) — `is_ci_with(env_lookup)` is the testable
  pure form, `is_ci()` reads `std::env`. The list of CI vars: any non-empty value in `CI`, `GITHUB_ACTIONS`,
  `GITLAB_CI`, `CIRCLECI`, `TRAVIS`, `BUILDKITE`, `JENKINS_URL`, `TF_BUILD`, `TEAMCITY_VERSION`,
  `BITBUCKET_BUILD_NUMBER`. CI suppresses **only** the stdin heuristic in agent detection, so explicit agent env
  vars still trigger regardless. `refuse_if_agent` is applied to commands that emit decrypted secrets or otherwise
  resolved env values: `:env get`, `:env decrypt`, `:env secret-keys list`, `:env secret-keys get-private`,
  and `--dry-run` (`format_extracted_commands` inlines resolved env vars — including decrypted secrets — into the
  printed shell-ready commands). When adding a new command that prints resolved env values, decrypted secrets,
  or private keys, gate it with `refuse_if_agent` too.
- `:env secret-keys add --key <hex>` is the non-interactive path used by CI (e.g. the
  `.github/actions/setup` action's `secret-keys` input). Gated by `ci_detect::is_ci()` — refuses to run on dev
  machines so the private key doesn't end up in shell history. Validates the key is 64-char hex, derives the
  public-key fingerprint, and stores via `Settings::add_secret_key_secure` (same OS credential store path the
  interactive flow uses). Without `--key`, the existing interactive prompt-based flow runs unchanged. The
  GitHub Action also bootstraps a session D-Bus + `gnome-keyring-daemon` on Linux runners (where no Secret
  Service is present by default) before calling this command.

## Runfile.json Schema Quick Reference

Top-level: `$schema` (required), `targets` (required), `globals` (optional)

Target properties: `commands` (required), `description`, `aliases`, `env`, `envFiles`, `forceShell`, `addToPath`,
`logging`, `ignoreErrors`, `parallel`, `detach`, `workingDirectory`, `confirm`, `forceKillOnSigInt`, `extendStdio`,
`watch`, `onlyInDirectories`, `metadata`

Global properties: `addToPath`, `env`, `envFiles`, `forceShell`, `logging`, `ignoreErrors`, `forceKillOnSigInt`,
`workingDirectory`, `onlyInDirectories`, `metadata`

Each entry of a `commands` array is a [`CommandStep`]: a raw shell command string, a target invocation
(`"@target [args...]"` string), a `WhenStep` (`{ when, commands, [ignoreErrors] }`), an `IfStep`, a
`ForStep`, or a `MatchStep` (`{ match, cases, default?, ignoreErrors?, when? }`). `IfStep`, `ForStep`, and
`MatchStep` may carry a top-level `when` field too. See "Key Design Decisions" below for semantics.

Globals are merged into each target's `CommandSpec` at parse time (by `bake_globals_into_target()` in `merge.rs`).
The in-memory `Runfile` has `globals: None` after parsing — all runtime code operates on self-contained targets.
Every global property can also appear on individual targets. Target-level always overrides global.

Env values can be strings, numbers, or booleans (all converted to strings at runtime).

## Key Design Decisions

- Parsing uses the `json5` crate — all Runfiles (`.json` and `.json5`) are parsed as JSON5. JSON5 is a strict superset
  of JSON, so all existing `.json` Runfiles work unchanged. JSON5 adds: `//` and `/* */` comments, trailing commas,
  unquoted keys, single-quoted strings, hex integers, etc.
- Discovery checks `Runfile.json` first, then `Runfile.json5`. `.json` takes priority in the same directory.
- The serde schema uses `deny_unknown_fields` everywhere — any typo or unrecognized property is a hard error. This is
  intentional.
- The `$schema` field accepts any non-empty string (URLs, paths) so editors can point to the JSON Schema file for
  autocomplete.
- Substitution (`{{ ARGS.key }}` without default) is a hard error, not silent empty string. This catches mistakes. Use
  `{{ ARGS.key ? }}` for intentional optional-with-empty-default.
- `--stdin-args` (top-level CLI flag, like `--dry-run`): when set, missing `{{ ARGS.x }}` / `{{ ENV.X }}` / `{{ FLAGS.x }}`
  references prompt the user via stdin instead of erroring. The prompt key is the FIRST `ARGS.*` / `ENV.*` segment in
  the chain (the user-facing "primary name"); the chain's literal default (if any) is shown in `[brackets]`. A
  non-empty answer overrides the chain; an empty answer (just Enter) falls through to the default — or to the
  existing `MissingArg`/`MissingEnv` error if no default exists. `VARS.*` and `RUN.*` are NEVER prompted (they're
  runtime context, not user input). `FLAGS.x` prompts as `pass --x? (y/N)` and accepts `y`/`yes`/`true`/`1` as
  presence. Answers are cached per (kind, key) so the same value is asked at most once per run, even across
  `@target` invocations (the `Arc<dyn StdinPrompter>` is propagated through `RunnerDependencyResolver`). Works with
  `--dry-run` too (the dry-run path also goes through `RunArgs::substitute`).
- Runtime context substitutions: `{{ RUN.os }}` / `{{ RUN.shell }}` / `{{ RUN.cwd }}` / `{{ RUN.file }}` /
  `{{ RUN.parent }}` expose runtime info so users can write inline `if` conditions for cross-platform branching
  (e.g. `"if": "{{ RUN.os }} == windows"`) or reference paths in env values, `workingDirectory`, etc. Keys:
  - `RUN.os` — `"windows"` / `"linux"` / `"mac"`
  - `RUN.shell` — `"bash"` / `"zsh"` / `"sh"` / `"fish"` / `"powershell"` / `"cmd"`
  - `RUN.cwd` — caller's current working directory (absolute path; fixed for the whole run)
  - `RUN.file` — path to the source Runfile of the *currently-executing target*
  - `RUN.parent` — directory of the currently-executing target's source Runfile
  Unknown RUN keys are a hard error. RUN values are not redacted by `substitute_redacted` (they aren't secrets).
  `RUN.shell`/`RUN.file`/`RUN.parent` are refreshed per-target by the runner's `ensure_run_context`, so a target
  defined in an included file — or a target whose `forceShell` swap changes the active shell — sees values that
  match its own context. `RUN.cwd` is the caller cwd captured once at the top-level CLI invocation.
- `forceShell` and `workingDirectory` accept `{{ ... }}` substitutions. `workingDirectory` is a free-form path
  (absolute or relative) with no per-value validation — anything that resolves at runtime is accepted. Default
  (when unset) is `{{ RUN.parent }}` (target's source Runfile dir). Relative paths are resolved against the
  target's source Runfile directory by `resolve_working_directory_path`.
- Control flow (`if` / `for` / `match` blocks) is evaluated by Runfile itself, not by the shell — same semantics
  on every platform. Conditions use a tiny boolean DSL parsed at Runfile load time (errors fail fast). Truthiness
  rule: only
  the empty string is falsy; every other string (including `"false"` and `"0"`) is truthy. This matches what raw
  shell commands see when they receive a `{{ ... }}` substitution. Because `{{ FLAGS.x }}` resolves to `"true"` or
  `"false"` (both non-empty), flag presence checks must use explicit `== true` / `== false` comparisons. Mixing
  `&&` and `||` in the same expression is a parse error — parens are required to disambiguate. `for` blocks accept
  one of `in: [...]` (literal array, each element substituted), `glob: "..."` (filesystem glob, sorted matches),
  or `shell: "..."` (run command at planning time, iterate trimmed non-blank stdout lines). `for shell` failure is
  a hard error regardless of `ignoreErrors`. Loop variables are written into the run-wide `VARS` map (the same
  store `define(...)` populates) and referenced as `{{ VARS.<name> }}`. Scoping is save/restore via
  `LoopVarGuard`: outer `VARS.x` is preserved across an inner `for x in [...]`, and nested loops with the same
  name compose correctly. Loop iteration values are visible to dispatched `@target` children via the shared
  `Arc<Mutex<HashMap>>`, but inside a `parallel: true` parent the children may race on the shared map (the
  iteration value is baked into pre-substituted shell leaves, so direct shell uses are safe; only nested
  `@target` bodies that read `{{ VARS.x }}` as a loop variable can race). `parallel: true` on a `for` block runs
  iterations concurrently, but **outer parallel only**: a nested `for parallel: true` inside an already-parallel
  context is forced sequential (with a warning).
- `ignoreErrors` makes the CLI exit 0 even when commands fail — this is the specified behavior, not a bug.
- `parallel` spawns all shell commands simultaneously; their stdout/stderr is piped through line-buffered reader
  threads that prefix every line with `[N]` (the global step number) and strip non-SGR ANSI cursor-control
  escapes — so progress-bar redraws (`docker compose pull`, etc.) become append-only chronological lines instead
  of corrupting interleaved output. Stdin is inherited. **`@target` calls inside a parallel parent propagate
  the parent's prefix through the entire dispatched dependency subtree** (via `DependencyResolver::run_dependency(..., output_prefix)`),
  so a `dev` target that fans out via `for in: "namespaces"` + `@{{ VARS.ns }}:dev` gets every nested shell tagged
  with its parallel branch identity. Set `RUNFILE_NO_LINE_PREFIX=1`/`true` to disable prefixing and inherit raw
  stdio. The target finishes when all commands exit. With `ignoreErrors`, failures are counted but exit is 0.
- `detach` requires `parallel: true`. When both are set, commands are spawned in parallel as detached background
  processes and the CLI exits immediately.
- Logging goes to stderr so it doesn't interfere with command stdout.
- Conditional configuration uses `{{ RUN.* }}` substitution in scalar fields (env values,
  `forceShell`, `workingDirectory`, etc.) plus `if`/`when`/`@target` composition. The most common pattern
  (`"X": "value-{{ RUN.os }}"`) covers nearly all real cases without any new constructs.
- **No `before` / `after` lifecycle.** Removed in favor of inline `@target` invocations and `WhenStep` blocks
  in `commands`.
- **Include namespacing for monorepos.** `includes` accepts `IncludeEntry` values: either a path string (legacy
  behavior — no rewrite, conflicts on duplicate target names) or `{ "path", "namespace"? }`. With a namespace, every
  target name, alias, and `@target` reference inside that file is prefixed with `<namespace>:` at parse time. Children
  are sealed: `@build` inside a namespaced include resolves to *that file's* `build`, never the parent's, because the
  rewrite happens before merging. Nesting composes innermost-first (`outer:inner:build`). Same-file-twice with
  different namespaces is allowed (independent copies). Empty/absent namespace = legacy behavior. Internal targets
  preserve their `_`-prefix internal status under namespacing (`is_internal_target_name` checks the last `:`-segment).
  Per-target source-dir tracking (`source_dirs` map keyed by post-rewrite name, plus `target_sources` for the
  full file path) ensures `{{ RUN.parent }}` / `{{ RUN.file }}` and relative `envFiles` paths resolve relative
  to the file that *defined* the target, not the merged root. The runner / extract pipelines accept a
  `source_files: HashMap<String, PathBuf>` alongside `source_dirs` for this — the CLI builds it from
  `MergeResult::source_files()`.
- `WhenStep` (`{ when, commands, [ignoreErrors] }`): the wrapper form for guarded blocks. `IfStep` and `ForStep` also
  carry an optional top-level `when` field. Default is `WhenCondition::Success`. The walker's [`WalkState`] tracks a
  `failed: bool` flag — flipped when any `when: success` step exits non-zero (and isn't `ignoreErrors`'d). `failure` /
  `always` blocks enter with `failed = false` locally so their default-success children execute. Block-level
  `ignoreErrors: true` resets the outer state on exit (failures inside don't propagate). Target-level `ignoreErrors`
  swallows everything and exits 0.
- Walker semantics on shell failure: `execute_one_shell` always returns `Ok` and increments `state.failures` on
  non-zero exit. The walker checks `state.failures` after each step and flips `state.failed` when needed —
  execution does NOT abort. This lets `when: failure` / `when: always` blocks at later positions still run.
  `final_status` is set to a synthetic non-zero status (via `failed_status()`) when `state.failed` is true and the
  last command exited 0 (e.g. an `always` cleanup succeeded after a prior failure).
- Parallel parents partition leaves by `when`: `when: success` leaves run as the parallel batch; if any failed,
  `when: failure` leaves run sequentially after; `when: always` leaves always run sequentially after.
  See `run_parallel_leaves` in `executor.rs`.
- `match` blocks (`{ "match", "cases", "default"?, "ignoreErrors"?, "when"? }`) provide multi-way dispatch on a
  substituted value with built-in case validation. The `match` template goes through the normal substitution
  pipeline (chained fallbacks supported, e.g. `{{ ARGS.tier ? ENV.TIER ? 1 }}`). `cases` keys are compared by exact
  string equality against the resolved value. When no case matches, `default` runs if set; otherwise execution
  errors out and the message lists every valid case so the user knows what values to pass. When the `match`
  substitution itself fails (e.g. `{{ ARGS.tier }}` with no `--tier` and no chain default), `default` also runs as
  a fallback for the unresolvable value — only when there's no `default` does the substitution error propagate
  (with the case list appended). Cases are stored in a `BTreeMap` so error-message ordering is deterministic
  (alphabetical). `count_leaves` sums every case + default (worst case, like `if`'s both-branches counting).
- `metadata` (free-form object): available on `globals` and on each target. Globals' `metadata` is merged
  into each target's `metadata` at parse time by `bake_globals_into_target()` (the helper is `merge::merge_metadata`):
  for each known key the target value wins; otherwise the global value carries through. Untyped extra keys are
  preserved on the [`Metadata::extra`] flatten map (round-trips through serde, no validation, no `deny_unknown_fields`).
  Currently the only key Runfile itself interprets is `excludeFromGenerateCommand: bool` — when true (default false),
  `run :generate vscode-tasks` / `zed-tasks` / `jetbrains-run-configurations` skip the target entirely (no task /
  run-configuration is created and existing labelled entries are NOT updated). The merged value is observed via
  `CommandSpec::is_excluded_from_generate()`. The three generators
  call this helper alongside the existing `is_internal_target_name()` filter, so internal targets and
  metadata-excluded targets are both hidden from generators with no extra wiring. Other tooling consuming
  `Runfile` (CLI `:list`, MCP, completions, etc.) is unaffected — `excludeFromGenerateCommand` is *only* a
  generator-output filter.
- Encrypted env vars use AES-256-GCM with the format `encrypted:<base64(nonce||ciphertext||tag)>`. Decryption happens
  in-memory inside `build_env()` — decrypted secrets never touch disk.
- Each encrypted `.env` file contains a `RUNFILE_ENCRYPTION_PUBLIC_KEY` variable — a SHA-256 fingerprint of the private
  key. Private keys are stored in user settings as a `Vec<String>` of 64-char hex strings. Key matching is automatic:
  Runfile derives the public key from each stored private key and matches against the file's public key.
- Encryption key resolution order: `RUNFILE_ENCRYPTION_KEY` env var → auto-match `RUNFILE_ENCRYPTION_PUBLIC_KEY` against
  stored private keys → error. The env var allows CI/CD without local settings.
- The `:env` subcommand operates on `.env` files: `init` (create new, optionally encrypted with `--plain`/`--key`
  flags), `get` (auto-decrypts), `set` (auto-encrypts, `--plain` to skip encryption; `value` arg is optional — when
  omitted, reads from stdin until EOF and strips a single trailing `\n`/`\r\n`, so secrets stay out of shell history
  and shell-special characters like `$`/`!` need no escaping), `decrypt` (file→file), `encrypt` (
  file→file with public key prefix match), and `inject` (run a command with env vars from one or more `.env` files
  injected, à la `dotenvx run` — `-f <file>` repeatable, defaults to `.env`, encrypted values auto-decrypted in
  memory, command runs after `--`, `RUNFILE_ENCRYPTION_PUBLIC_KEY` is stripped before injection, the child's exit
  code is propagated). **Parent process env always wins**: after files are merged and decrypted, any key already
  defined in the parent process is dropped from the file-loaded map (`std::env::var_os` for platform-correct case
  sensitivity), so the inherited value reaches the child unmodified — file-loaded values only fill in gaps. The
  program is resolved via `which::which_in` against the *effective* PATH the child will see (inherited PATH if
  any, otherwise the env-file PATH — case-insensitive on Windows), so PATHEXT-style shims like
  `node_modules/.bin/vite.cmd` are found on Windows. Rust's `Command::new` only appends `.exe` via
  `CreateProcessW`, so without this lookup `npm`-installed shims would fail with "program not found". The `secret-keys` subgroup includes `add`, `list`, `get-private` (print full private key for
  sharing, matched by public key prefix), and `remove` (matched by public key prefix). All key matching throughout
  the CLI uses public key prefixes, never private key prefixes.

- `--timings` flag prints per-command and per-target durations to stderr. Format: `<1s` → ms, `>=1s` → seconds with one
  decimal, `>=60s` → minutes + seconds. Timings are opt-in and have zero overhead when disabled.
- Watch mode is automatic: if a target defines `watch` patterns, running it enters watch mode with no extra flags.
  Uses the `notify` crate v7 for filesystem events and `globset` for pattern matching. The watch loop lives in
  `cmd_run.rs` (CLI layer). Events are debounced at 300ms. Patterns are relative to the Runfile directory; `!` prefix
  excludes.
- Confirmation prompts (`confirm` field) block on stdin before executing. Auto-skipped when `CI` env var is `"true"` or
  `"1"`, or when `--yes`/`-y` flag is passed. The `confirm` string supports `{{ ... }}` substitution.
- `extendStdio` is an optional array of `{ "fromFile": "path", "stream": "stdout"|"stderr" }` objects on `CommandSpec`.
  During execution, background threads tail each log file and route new complete lines (terminated by `\n`) to the
  specified stream. Files that don't exist yet are polled until they appear. Polling interval is 50ms.
  Truncated/rotated files are handled by resetting to the beginning. Tailers start before command execution and stop
  after all commands finish (including a final flush). `fromFile` paths support `{{ ... }}` substitution (e.g.
  `"logs/{{ RUN.os }}.log"`).
- `forceKillOnSigInt` is a boolean on `CommandSpec`. When true, the executor creates a Job
  Object (Windows) or tracks child PIDs (Unix) and installs a CTRL+C/SIGINT handler that forcefully terminates the
  entire process tree. This is essential for GUI-subsystem apps (e.g. Unity Editor) on Windows that don't receive
  console CTRL+C events and would otherwise survive as orphan processes. On Windows, `TerminateJobObject` kills all
  children and grandchildren. On Unix, `SIGKILL` is sent to each child PID. The handler suppresses the default
  SIGINT behavior so the executor can cleanly reap children and report the exit status. Implementation is in
  `force_kill.rs` using global state (static Mutex for the Job Object handle / PID list) since console ctrl handlers
  on Windows require function pointers, not closures.
- VS Code tasks generator (`run :generate vscode-tasks`) follows the same pattern as the Zed generator: generates
  `.vscode/tasks.json`, merges with existing files preserving user-added fields via `#[serde(flatten)]`.
- All three editor generators (`vscode`, `zed`, `jetbrains-run-configurations`) inject `--stdin-args` into the
  generated invocation. Editor run configs are static (no per-invocation arg prompt UI built into the IDE), so
  `--stdin-args` is what lets a static config still cover targets that need user input — missing `{{ ARGS.x }}` /
  `{{ ENV.X }}` / `{{ FLAGS.x }}` values are prompted at run time. JetBrains keeps `EXECUTE_IN_TERMINAL=false` (the
  default Run/Services tool window) — do NOT flip it to `true`. `check_existing_jetbrains_config` accepts both
  the current `run --stdin-args <target>` form and the legacy `run <target>` form so older configs upgrade in
  place.

## Testing Requirements

**Every crate has its own `tests.rs` module.** When making changes:

1. Always run `run test` for the full workspace — not just the crate you changed.
2. New features need tests. New schema fields need parsing tests (valid JSON, rejection of bad values). New executor
   behavior needs integration tests (actually spawning shells).
3. When adding a new field to `CommandSpec` or `Globals`, you MUST add `field_name: None` (or appropriate value) to
   every existing struct literal in test files. The compiler will catch missing fields but it's easy to miss many of
   them.
4. Integration tests (in `runfile-executor`) spawn real shells, so they depend on having at least one shell available on
   the test machine.
5. Cross-platform: test assertions about PATH must normalize backslashes (`path.replace('\\', "/")`).

## README

`README.md` is the public-facing user documentation. If you add or change features (new CLI flags, new Runfile
properties, changed behavior), update the README to match. It covers:

- CLI usage and all flags/subcommands
- Full Runfile.json property reference
- Substitution syntax
- Shell support and override behavior
- Environment variables, PATH manipulation, logging, error handling
- Runfile discovery, local settings paths, JSON Schema

## JSON Schema

`schemas/v0.schema.json` must stay in sync with the Rust types in `runfile-parser/src/schema.rs`. When adding
properties, update both files.

## CLAUDE.md

Always update this file (CLAUDE.md) with any new key decisions, requirements, crates, major code changes, etc.
