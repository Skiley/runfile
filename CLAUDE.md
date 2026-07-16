# CLAUDE.md — Project Reference for Runfile

## What is Runfile

Runfile is a cross-platform command runner (a modern Makefile alternative). Users define targets in a `Runfile.json`
file and execute them via the `runfile` CLI. It is written entirely in Rust, compiles to a single
binary, and works on Linux, macOS, and Windows with support for Bash, Zsh, Sh, Fish, PowerShell, and cmd.exe.

## Build & Test

NEVER use `cargo` commands!

ALWAYS use `run <target>`, read `Runfile.json` to see available targets.

```
run setup                  # One-time per clone: activates the committed git hooks
run build                  # Debug build
run check                  # Non-mutating gate: fmt --check + clippy (deny warnings) + cargo check
run install                # Links the debug build to the global "rund" command
run lint                   # Formats, checks and lints the code
run test                   # Runs all tests
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
  runfile-settings/            # Local user settings (~/.config/runfile/) and OS keyring access for secret keys
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
  (`{{ ARG.env == 'prod' }}` resolves to `"true"` or `"false"`); see "DSL inside substitutions" below.
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
  `validate_args` collector) recognise `{{ ARG.x }}` / `{{ FLAG.x }}` references that live outside `commands`. `From<&str>` and
  `From<String>` impls let callers use `"foo".into()` ergonomically; `CommandSpec::new_shell(Vec<String>)` is a convenience
  constructor for string-only command lists.
- DSL parsing (`dsl.rs`): tiny boolean expression language for `if` conditions. Hand-written tokenizer + recursive
  descent parser, no external deps. Grammar: comparisons (`==`, `!=`), logical operators (`&&`, `||`, `!`), parens,
  substitution leaves (`{{ ARG.x }}` / `{{ ENV.X }}` / `{{ FLAG.x }}` / `{{ VAR.x }}`), quoted strings, bare-words. Mixing
  `&&` and `||` in the same expression is a hard parse error — parens are required to disambiguate. Parsing happens
  eagerly during `validate_runfile`, so syntax errors surface at Runfile load time.
- `MatchStep` (multi-way dispatch): `{ "match", "cases", "default"?, "ignoreErrors"?, "when"? }`. Stored on
  `CommandStep::Match`. The `match` field is a substitution template — same pipeline as any other `{{ ... }}`
  reference, so chained fallbacks work (`{{ ARG.tier ? ENV.TIER ? '1' }}`). `cases` is a `BTreeMap<String,
  Vec<CommandStep>>` (sorted; alphabetical iteration order in error messages); each case body accepts the usual
  string-or-array sugar via the same `Value`-based deserializer pattern as `IfStep.then`. `default` is an
  optional branch run when no case matches. Validation lives in `parse.rs::validate_match_step`: empty
  match expression and empty cases-without-default are hard errors. Runtime semantics (in
  `control_flow::resolve_match_branch`): substitute `match`; on substitution failure fall through to `default`
  if set, else surface `ControlFlowError::MatchValueUnresolved` with valid cases listed; on substitution
  success look up the value, run the case if found, else fall through to `default` or surface
  `ControlFlowError::MatchNoCase`. Cases are looked up by exact string equality FIRST; if no exact match
  hits, a second pass tries every case key wrapped in `/.../` as a regex pattern (alphabetical iteration via
  the `BTreeMap`). Literal cases always win over regex-shaped cases — even when both would match — because
  the exact-equality check runs before any regex compilation. Bad regex patterns surface as
  `ControlFlowError::BadRegexCase { key, message }` at runtime (no parse-time validation; the parser crate
  intentionally has no `regex` dep). The regex isn't anchored — wrap with `/^...$/` for full-string match.
  `ignoreErrors`/`when` mirror `IfStep`. `count_leaves` sums all cases + default (worst-case, like
  `if`'s both-branches counting).
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

**Files:** `settings.rs`, `paths.rs`, `keyring_store.rs`, `keyring_keys.rs`, `tests.rs`

- `Settings` struct holds `shell_paths`, `path_aliases`, `global_files` only — secret-key state
  is **not** part of settings.json in any form. Older binaries wrote a `secureKeyFingerprints`
  array; that field is silently ignored on load (Settings doesn't use `deny_unknown_fields`) and
  stripped from disk on the next save. There is no migration of legacy keyring entries — keys
  pre-dating the keyring-only storage layout must be re-added via `run :env secret-keys add`.
- Platform paths: Linux `~/.config/runfile/`, macOS `~/Library/Application Support/runfile/`, Windows
  `%APPDATA%\runfile\`
- Load returns defaults if file doesn't exist; save creates parent dirs automatically.
- **Secret keys live exclusively in the OS credential store.** All private keys for a given install
  are stored as a single keyring entry at `(service="runfile", user="__keystore__")` whose value is
  a JSON object `{ fingerprint -> private_key_hex }`. One source of truth — settings.json never
  tracks fingerprints, so drift between settings.json and the credential store is impossible by
  construction. The `keyring_keys` module is the only path through which the CLI talks to that
  storage:
  - `add(private_key_hex) -> Result<bool>` — `Ok(false)` if a key with the same fingerprint is
    already in the blob.
  - `remove(fingerprint) -> Result<bool>` — exact-match deletion.
  - `resolve_prefix(prefix) -> Result<String>` — prefix-match against stored fingerprints (errors
    on zero or multiple matches). Replaces the old "load all keys, derive fingerprints, match" flow,
    so orphan deletion works even when private-key recovery would fail.
  - `list_fingerprints() -> Result<Vec<String>>` — sorted snapshot.
  - `all_private_keys() -> Vec<String>` — best-effort decryption pool. Merges the env-supplied
    `RUNFILE_PRIVATE_KEYS` pool (newline-separated hex keys, first) with the keyring blob (second),
    deduplicating so `find_private_key_by_public_prefix` doesn't see spurious ambiguity. On keyring
    error returns whatever the env pool gave (possibly empty), matching the executor's "no keys →
    no decryption" contract. The credential-store warning is suppressed when the env pool is
    non-empty — the caller has explicitly opted into env-only key supply (CI runners, ephemeral
    containers) and doesn't need to be nagged about a missing keyring.
- `RUNFILE_PRIVATE_KEYS` (in `keyring_keys::ENV_PRIVATE_KEYS_VAR`): newline-separated 64-char hex
  private keys. Whitespace-only lines are skipped. Designed for environments where round-tripping
  a secret through an OS credential store is wasted work — CI runners in particular, where the
  GitHub setup action exports this from the `secret-keys` input via `$GITHUB_ENV` instead of
  bootstrapping gnome-keyring + dbus. Read pure helpers `parse_env_pool` / `merge_key_sources` are
  exercised by unit tests without mutating process env.
- `keyring_store` is the low-level wrapper: `load_blob`/`store_blob`/`delete_blob` operate on the
  single keystore entry. `is_available()` is the public probe used by callers to surface
  "credential store not running" early.
- **Per-platform backends (and the Linux Secret-Service-with-keyutils-fallback).** Windows uses
  Credential Manager and macOS uses Keychain (both persistent), via keyring-core. On **Linux**,
  `keyring_store` picks a backend once per process (`OnceLock<LinuxBackend>` in `linux_backend()`):
  it prefers the **persistent D-Bus Secret Service** (gnome-keyring / KWallet) and falls back to
  **kernel keyutils** (in-memory, cleared on reboot) when no session bus / secret-service provider /
  default collection is present. The selection **never errors** — a missing Secret Service silently
  degrades to keyutils, so `secret-keys add` keeps working in headless / CI environments exactly as
  before (it just isn't durably persisted there). This fixes the long-standing "keys added on Linux
  never survive a reboot/new session" bug: keyutils was the *only* Linux backend, and it's an
  in-kernel store, not a disk-backed one. `is_available()` is therefore always `true` on Linux (the
  keyutils fallback is always present). The four `keyring_store` entry points dispatch on
  `using_secret_service()`; the keyutils/macOS/Windows paths still go through keyring-core
  (`keyring_core_{load,store,delete}_blob` / `keyring_core_is_available`).
- The Secret Service path lives in `secret_service_store.rs` (`#[cfg(target_os = "linux")]`, declared
  in `lib.rs`). It uses the **`secret-service` crate's blocking API** (`secret-service` v5 with
  `default-features = false, features = ["rt-async-io-crypto-rust"]`) — pure Rust (zbus + RustCrypto,
  **no libdbus / OpenSSL C dependency**) so it links cleanly into the static musl release binaries
  built natively per-arch (`ubuntu-24.04` / `ubuntu-24.04-arm` with `musl-tools`, no `cross`). The
  blob is one item in the user's **default** collection, identified by attributes
  `{ service: "runfile", user: "__keystore__" }` (mirroring the keyring-core `(service, user)` pair),
  stored with `create_item(.., replace = true)` so saves upsert in place. `connect` uses
  `EncryptionType::Dh` so the secret is encrypted in transit over the bus. `with_default_collection`
  centralizes connect + unlock-if-locked and runs per operation (no cached connection — sidesteps the
  self-referential `SecretService`/`Collection` lifetime; key-management calls are infrequent).
  `is_usable()` is the probe `linux_backend()` consults — it connects + resolves the default
  collection but deliberately **does not unlock**, so picking the backend never triggers an
  interactive prompt; an unlock prompt can still fire later on an actual `load`/`store`/`delete` of a
  locked keyring (same as macOS Keychain — and `LazyPrivateKeys` keeps decryption runs from touching
  the store unless an `encrypted:` value is actually present). SS errors map to `keyring_core::Error`
  (`Locked` → `NoStorageAccess`, else `PlatformFailure`) so the dispatch return types stay uniform.
- **CI is unaffected by the backend change.** The GitHub setup action exports keys as
  `RUNFILE_PRIVATE_KEYS` and never touches the credential store, so CI runs don't depend on which
  Linux backend is active. The CI runners (ubuntu-latest) generally have no D-Bus session, so
  `is_usable()` returns false there and the keyutils fallback is used — identical to the old
  behavior.

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
- `EnvBuildParams.available_private_keys`: `Option<&dyn PrivateKeyProvider>` — invoked only when encrypted values are
  detected in the merged env, after which `RUNFILE_ENCRYPTION_PUBLIC_KEY` from the env is matched against the
  provider's keys to pick the right one. There is **no longer** a `RUNFILE_ENCRYPTION_KEY` env-var fallback — that
  shorthand was removed when `RUNFILE_PRIVATE_KEYS` (in `runfile-settings`) subsumed it. Encrypted files MUST carry a
  `RUNFILE_ENCRYPTION_PUBLIC_KEY` header (which `:env init`/`encrypt`/`set` always write); hand-crafted files without
  the header now error with a clear message rather than silently relying on a separate env var. Any
  `T: AsRef<[String]> + Sync` satisfies the trait via a blanket impl (so existing `Some(&vec_of_keys)` call sites Just
  Work). `LazyPrivateKeys::new(loader)` memoizes the loader via `OnceLock<Vec<String>>` — used by the CLI to wrap
  `keyring_keys::all_private_keys` so the OS credential store is never touched for runs whose env has no `encrypted:`
  values, and only once per run when it is needed. This is what keeps the macOS Keychain unlock prompt from firing on
  every invocation regardless of target.
- `check_env_case_duplicates()`: validates no env var keys differ only by casing.
- `collect_runfile_env()`: collects only Runfile-defined env vars (not system), sorted by key. Takes a single
  `Option<&HashMap<String, String>>` (no global/command distinction).
- Re-exports `is_encrypted` and `has_encrypted_values` from `runfile-crypto` for convenience.
- Does NOT depend on `runfile-parser` — receives raw `HashMap<String, String>` and `&[String]` slices. The caller
  converts `EnvValue` types before passing them.

### runfile-executor

**Files:** `args.rs`, `functions.rs`, `dsl_eval.rs`, `control_flow.rs`, `env.rs`, `executor.rs`, `parallel.rs`,
`force_kill.rs`, `logging.rs`, `parallel_output.rs`, `runner.rs`, `stdio_tailer.rs`, `tests/`

- `functions.rs`: the built-in `{{ funcname(...) }}` registry — `evaluate_function` (the dispatch entry point, called
  from the chain resolver in `args.rs`) plus its numeric / json / regex / string / shell-quoting helpers. Extracted
  from `args.rs`; calls back into `crate::args::{evaluate_arg, parse_static_name}` to resolve arguments.
- `dsl_eval.rs`: boolean-DSL evaluation for `{{ ... }}` bodies containing `==`/`!=`/`&&`/`||`/`!` — `looks_like_dsl`
  (detection) + `evaluate_dsl_expression` (evaluation to `"true"`/`"false"`). Leaf values resolve through
  `crate::args::{evaluate_arg, walk_template}`. Extracted from `args.rs`.
- `parallel.rs`: parallel execution — `ParallelLeaf` collection (`collect_leaves_parallel`) and the concurrent batch
  runner (`run_parallel_leaves` → `run_parallel_batch` / `run_sequential_leaves`). Extracted from `executor.rs`;
  borrows `pub(crate)` executor internals (`ExecSetup`, `WalkState`, `ProcessTreeTracker`, `dummy_success_status`,
  etc.). `execute_parallel*` orchestration entry points stay in `executor.rs`.
- Test modules live under `tests/` (one file per topic: `functions`, `control_flow_match_parallel`,
  `extract_tests`, `flags_run_tests`, `parallel`-related, etc.), with shared helpers (`get_test_shell`,
  `json_escape_path`, `parse_target`) in `tests/mod.rs`.

- `RunArgs`: parses CLI args into positional (`{{ ARGS }}`) and named (`--key=value`). Carries a `run_context: RunContext`
  field used to resolve `{{ RUN.* }}` substitutions; populated by the CLI via `RunArgs::parse(...).with_run_context(...)`.
  Also carries an optional `stdin_prompter: Option<Arc<dyn StdinPrompter>>` — when set (top-level CLI flag
  `--stdin-args`), `{{ ARG.* }}` / `{{ ENV.* }}` references with no default (and the bare boolean `{{ FLAG.x }}`
  form) trigger a stdin prompt instead of erroring; references that have a default resolve to it without
  prompting (see the `--stdin-args` design-decision entry below). The prompter trait lives in `args.rs` alongside `InteractiveStdinPrompter` (the default impl that
  reads stdin, writes prompts to stderr, and caches answers in `Mutex<HashMap>`s). `Arc` cloning shares the
  cache, so the prompter propagates through `@target` calls (via `RunnerDependencyResolver::run_dependency`,
  which clones `parent.stdin_prompter`) without re-asking the user. Tests use a mock `StdinPrompter` to script
  scripted answers. The `vars: Arc<Mutex<HashMap<String, String>>>` field is the run-wide store for
  `{{ define(...) }}` side effects — read by `{{ VAR.<name> }}` chain segments, shared via Arc-clone across
  `@target` calls and parallel worker threads.
- **Substitution source prefixes**: `ARG.<key>` (named arg), `ENV.<key>`, `FLAG.<key>`, `VAR.<key>`, `RUN.<key>`.
  Bare `{{ ARGS }}` is the "all positional args" sentinel (no `.<key>`). Any unrecognised prefix
  (`{{ WHATEVER.x }}`) falls through the chain resolver / `evaluate_arg` to `resolve_literal_segment`, which
  rejects it as a bareword (`SubstitutionError::BarewordLiteralNotAllowed`).
- `substitute()` returns `Result` — `{{ ARG.key }}` without `?` errors if arg is missing; `{{ ARG.key ? }}` with empty
  right-side defaults to empty string; `{{ ARG.key ? 'default' }}` uses the default. The substituter walks the
  template once, resolving every `{{ ... }}` block it finds while leaving everything else (including bash
  `$(...)` command substitutions) untouched — so `echo $(date)` passes through verbatim, and
  `$(echo "{{ ARG.env }}")` becomes `$(echo "development")` (the inner `{{ ... }}` resolves; the outer
  `$(...)` is opaque to Runfile). Strict whitespace: exactly one space after `{{` and before `}}`, exactly
  one space around `?` and `:`. Use `\{{` / `\}}` to emit a literal `{{` / `}}` in the output.
  `scan_args_usage` mirrors this so `validate_args` recognises `--env` even when its only reference is
  nested inside a shell `$(...)`.
- `RunContext { os, shell, cwd, file, parent, namespaces }`: static execution context. Resolves all `{{ RUN.* }}`
  keys: `os` (`"windows"` / `"linux"` / `"mac"`), `shell` (`"bash"` / `"zsh"` / `"sh"` / `"fish"` / `"powershell"` /
  `"cmd"`), `cwd` (caller's current working dir, absolute path), `file` (source Runfile path of the
  currently-executing target), `parent` (directory of `file`). Unknown `RUN.<key>` is a hard error.
  Participates in chained fallbacks (`{{ ARG.shell ? RUN.shell }}`). The runner calls `ensure_run_context()`
  per target so `shell` / `file` / `parent` stay accurate when a target-level `forceShell` swaps the effective
  shell, or when a target was defined in an included or global Runfile. `cwd` is captured once at top level and
  doesn't change. `forceShell` and `workingDirectory` themselves go through substitution before resolution —
  e.g. `"forceShell": "{{ ARG.shell ? 'bash' }}"` works.
- `LoopVarGuard`: RAII helper used by the executor and extract walker to scope a `for`-loop iteration variable
  into the run-wide `VARS` map. `enter(&vars, name)` captures the prior value of `VAR.<name>` AND
  `VAR.<name>_index` (if any); each iteration calls `set(value)` to overwrite the value and bump the
  internal counter (mirrored into `VAR.<name>_index` as a 0-based decimal); on drop, both prior values
  are restored (or removed if there were no priors). This gives lexical scoping for free: outer `VAR.x` /
  `VAR.x_index` are preserved while a `for x in [...]` runs, and nested loops with the same variable name
  compose correctly. `{{ VAR.x }}` and `{{ VAR.x_index }}` participate in chained fallbacks
  (`{{ ARG.x ? VAR.y ? 'default' }}`); missing `VARS` refs error like missing `ARGS`. The index counter is
  per-guard (lives in a `std::cell::Cell<usize>`) so each loop starts at 0 even when nested with the same
  variable name as an outer loop.
- **Quote-strict literals**: under the new substitution syntax, every string literal *inside a `{{ ... }}` block*
  must be wrapped in single quotes (`'...'`) — bareword literals are rejected with
  [`SubstitutionError::BarewordLiteralNotAllowed`]. Source references (`ARG.x`, `VAR.x`, etc.) and function
  calls remain bare. Two quote forms exist:
  - **Single quotes (`'...'`) — interpolated string**: stripped at evaluation, with any nested `{{ ... }}` blocks
    inside resolved through the regular substitution machinery. So `'docker -f {{ VAR.compose }} pull'`
    becomes `docker -f services/web.yml pull`. Required if the literal contains `,`/`(`/`)`/`?`. Nested
    `{{ ... }}` blocks are **opaque** to every outer scanner ([`find_substitution_close`],
    [`strip_single_quotes`], [`split_function_args`], [`split_chain_segments`], [`parse_function_call`],
    [`looks_like_dsl`]) — they all skip past a nested block via [`skip_nested_subst`] (which recurses
    through `find_substitution_close`) so the inner block's quotes / parens / commas / `?` operators can't
    bleed into outer state. This means `'system-images;{{ nth(VAR.part, ' ', '0') }}'` is a clean
    single-quoted literal even though the inner `nth(...)` uses its own `' '` separator arg, and
    `concat('a', nth(VAR.x, ',', '0'))` splits args correctly even though the inner `nth(...)` has its
    own commas. The same opacity rule applies in the DSL parser tokenizer
    ([`runfile_parser::dsl::tokenize`]).
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
  `regex_capture(haystack, pattern, group_idx)` (returns the substring captured by group
  `group_idx` of the FIRST match — `0` is the whole match; no match or out-of-range group both
  return `""`, mirroring `nth`'s out-of-bounds convention; non-numeric `group_idx` errors as
  `InvalidNumber`; bad pattern errors as `InvalidRegex`; this is the idiom for "pull a substring
  out of a file" without the `(?s)^.*X(...)X.*$` greedy-replace trick),
  `regex_capture_all(haystack, pattern, group_idx, separator)` (the "all matches" variant of
  `regex_capture` — pulls group `group_idx` from EVERY match via `captures_iter` and joins the
  results with `separator`; a match where the requested group didn't participate contributes `""`
  so entry count stays aligned with match count; no matches → `""`; pattern unanchored; same
  `InvalidRegex` / `InvalidNumber` error contract as `regex_capture`),
  `base64_encode(s)`,
  `base64_decode(s)` (errors on `InvalidBase64` / `NonUtf8Decoded`),
  `sha256(s)` / `md5(s)` (hex-encoded digest of `s`'s UTF-8 bytes; `md5` is non-cryptographic — for cache-key /
  fingerprint use, prefer `sha256` for security-sensitive contexts),
  `read_file(path)` (read the file at `path` and return its UTF-8 contents; relative paths resolve against
  `{{ RUN.parent }}` — same anchor as `envFiles` — so reads stay co-located with the Runfile; pair with
  `try(...)` to recover from missing files; surfaces as [`SubstitutionError::ReadFileError`] on failure),
  `write_file(path, content)` (write `content` to `path`; same path-resolution rules as `read_file`; goes
  through Rust's `std::fs::write` so the shell is never involved — this is the recommended path for
  read-modify-write pipelines because the naive `printf %s {{ shell_quote(VAR.x) }} > file` pattern is
  broken on Windows for payloads larger than ~5KB: when Rust spawns `sh.exe -c <command>` via
  `CreateProcessW`, MSYS's argv-reconstruction logic drops the pipe/redirect operators and stdout leaks
  to the terminal; returns `""` so a `{{ write_file(...) }}`-only line is dropped by the
  empty-command-skip path; side effect is skipped on `args.dry_run` and on the `redact_env` log pass —
  same pattern as `define` / `set_cwd` / `capture`; IO failure surfaces as [`SubstitutionError::WriteFileError`]),
  `file_exists(path)` (`"true"` / `"false"`, same path resolution as `read_file`; permission errors fold to
  `"false"` — use `try(read_file(p))` to distinguish "missing" from "unreadable"),
  `temp_file([content], [extension])` (create a fresh file in the OS temp dir — `std::env::temp_dir()` — named
  `runfile-<uuid>`, write `content` into it if given, and append `.<extension>` if given (a leading dot on the
  extension arg is stripped, so `'json'` and `'.json'` both yield `…​.json`); returns the absolute path. Arity is
  0/1/2 (more → `FunctionArity`). Registered for deletion at CLI exit via the process-global registry in
  `functions.rs` — see "**`temp_file` / `temp_dir` cleanup**" below. Non-deterministic + side-effecting, so it
  follows the `uuid` / `capture` / `write_file` pattern: `args.dry_run` AND the `redact_env` log pass both return the
  placeholder `<temp_file>` and create NOTHING (the redact skip is what keeps the back-to-back log substitution from
  spawning a second, orphaned temp file — there's no per-callsite memoization, so each real substitution pass creates
  exactly one file; reuse a single file by capturing it with `define`). Write failure surfaces as
  [`SubstitutionError::TempFileError`]),
  `temp_dir()` (0 args; create a fresh empty directory `runfile-<uuid>` in the OS temp dir, registered for recursive
  deletion at CLI exit; same `dry_run` / `redact_env` placeholder rule as `temp_file`, returning `<temp_dir>`;
  creation failure surfaces as [`SubstitutionError::TempDirError`]),
  `json_get(json, path)` (parse `json` and extract the value at the dotted `path`; numeric segments are
  array indices, e.g. `users.0.name`; missing paths return `""`; strings come back unquoted, scalars use
  their canonical text repr, objects/arrays serialize back to compact JSON; malformed JSON / paths surface
  as [`SubstitutionError::InvalidJson`] / [`SubstitutionError::InvalidJsonPath`]),
  `json_set(json, path, value)` (set the dotted `path` in `json` to `value` — interpreted as JSON if it
  parses, else as a string; intermediate containers are created on demand: numeric segments → arrays
  extended with `null`, non-numeric → objects; existing nodes of the wrong container type are replaced,
  matching `jq` assignment semantics; returns the modified JSON as a compact string),
  `concat(s1, s2, ...)` (variadic, 1+ args),
  `join(sep, s1, s2, ...)` (variadic, 1+ args; `join(sep)` with no items returns `""`),
  `nth(s, sep, i)` (split `s` by `sep`, return the `i`-th part; out-of-bounds → `""`; non-numeric or negative
  `i` → `InvalidNumber`; follows Rust `str::split` semantics including the empty-`sep` edge case),
  `first(s, sep)` / `last(s, sep)` (sugar for the first / last part of `s.split(sep)` — `last`
  is the canonical "basename" idiom; both return `""` when the input is empty or starts/ends with the
  separator), `count_parts(s, sep)` (number of parts as a decimal string; always ≥ 1, since `"".split(",")`
  yields `[""]` per Rust split semantics — pair with `nth` to bound-check before indexing),
  `shell_quote(s)` (per-shell single-arg quoting via [`quote_for_shell`] dispatching on `RUN.shell` —
  POSIX/fish use `'...'` with `'\''` escape, PowerShell uses `'...'` with `''` escape, cmd uses `"..."` with
  `""` escape; lets users inline arbitrary bytes — newlines, `$`, `"`, `'`, JSON, etc. — into shell commands
  as single argv slots without env-var indirection),
  `capture(shell_cmd)` (run `shell_cmd` through the platform's default shell — `sh -c` on Unix,
  `cmd /C` on Windows, matching `for shell:` iterators — at substitution time and return stdout
  with a single trailing `\n` / `\r\n` stripped; non-zero exit, spawn failure, or non-UTF-8 stdout
  all surface as [`SubstitutionError::CaptureFailed`]; results are memoized in
  `RunArgs.capture_cache` keyed by the resolved command string so the real and redacted
  substitution passes don't double-execute and repeats within a target collapse to a single spawn;
  the cache `Arc` is propagated through `@target` invocations so children reuse the parent's
  captures; during `args.dry_run` the call short-circuits to the placeholder
  `<capture: '<resolved-cmd>'>` instead of spawning, mirroring `for shell:`'s dry-run rule),
  `one_of(value, opt1, opt2, ...)` (variadic ≥2; returns `value` if it matches any option by
  exact string equality, else errors as [`SubstitutionError::OneOfNoMatch`] with every valid
  option listed; designed to collapse the `match { major: define(part, 'major'), ... }`
  boilerplate that's purely value-validation),
  `add(a, b, ...)` / `subtract(a, b, ...)` / `multiply(a, b, ...)` / `divide(a, b, ...)`
  (variadic ≥2; each arg goes through [`parse_numeric`] which accepts decimal integers, decimals,
  and scientific notation via `f64::from_str` and rejects `inf` / `nan` as
  [`SubstitutionError::InvalidNumeric`]; whitespace is trimmed; the fold is left-to-right; result
  formatted via [`format_number`] which prints as an integer when `value.fract() == 0.0` and as
  the shortest round-trip `f64` Display otherwise — so `add('5', '3')` → `"8"`, `add('5.5',
  '2.3', '1.2')` → `"9"`, `add('5', '1.1')` → `"6.1"`, `subtract('5', '5')` → `"0"`; `divide`
  errors as [`SubstitutionError::DivideByZero`] on any zero divisor in the fold),
  `less_than(a, b)` / `less_than_or_equal(a, b)` / `greater_than(a, b)` / `greater_than_or_equal(a, b)`
  (exactly 2 args; both coerced via [`parse_numeric`] — same numeric rules as the arithmetic family, so
  non-numeric / `inf` / `nan` error as `InvalidNumeric`; return the literal `"true"` / `"false"` so they
  double as `Truthy` DSL values; shared body is the `numeric_compare` helper),
  `is_number(s)` (1 arg; returns `"true"` if `s.trim()` parses as a finite `f64`, else `"false"` — unlike
  the arithmetic family it does NOT error on non-numeric input, since detection is the whole point; `inf` /
  `nan` are not numbers, matching `parse_numeric`),
  `modulo(a, b)` / `power(a, b)` (2 args; `modulo` errors `DivideByZero` on `b==0`; `power` errors
  `InvalidNumeric` on non-finite result), `min(a, …)` / `max(a, …)` (variadic ≥1, via `numeric_reduce`),
  `abs(a)` / `round(a)` / `floor(a)` / `ceil(a)` (1 arg) — all share `parse_numeric` + `format_number`,
  `substring(s, start[, len])` (char-indexed slice; `parse_count` on `start`/`len`; out-of-range `start` →
  `""`, `len` clamps; omit `len` for to-end),
  `basename(p)` / `dirname(p)` / `extname(p)` / `stem(p)` / `join_path(a, …)` (path helpers via
  `std::path::Path` — `extname` returns the extension WITHOUT the dot; `join_path` uses `PathBuf::push`),
  `url_encode(s)` / `url_decode(s)` (RFC 3986 percent-encoding; unreserved `A-Za-z0-9-_.~` pass through,
  space → `%20`, symmetric; decode errors `InvalidUrlEncoding` / `NonUtf8Decoded`),
  `uuid()` (0 args; v4-shaped via a dependency-free SplitMix64 PRNG seeded from clock/pid/counter — NOT
  cryptographic; `args.dry_run` → `<uuid>` placeholder),
  `now(format)` (1 arg; current UTC time via `now_formatted` — formats `unix-timestamp`/`unix-millis`/`iso`/
  `iso-date`/`iso-time`/`rfc3339`/`year`/`month`/`day`/`hour`/`minute`/`second`; civil-date conversion is
  dependency-free via Howard Hinnant's algorithm in `civil_parts`; unknown format → `InvalidTimeFormat`),
  `error(msg)` (prints `msg` to stderr on the real pass, returns the [`SubstitutionError::UserError`] sentinel;
  `args.dry_run` → `<error: '…'>` placeholder, no print/fail; see "**`error(msg)` semantics**" below),
  `try(expr)` (special-cased before the bulk arg-eval
  pass so inner errors are catchable; see "**`try(expr)` semantics**" below), `define(name, value)`
  (returns `""`, side effect: sets `VAR.name`), and `set_cwd(path)` (returns `""`, side effect: mutates
  `RunArgs.cwd_override` so every subsequent shell spawn in the current target lands in `path` — see
  "**`set_cwd(path)` semantics**" below).
  Functions resolve as full substitution bodies
  AND as chain segments — `{{ ARG.host ? to_lower(ENV.HOST) }}` is a chain whose first segment is a source lookup
  and second is a function call. Args themselves are chain / quoted-literal expressions, so nested calls
  (`to_upper(to_lower(x))`) and chained args (`to_upper(ARG.x ? 'default')`) work naturally. The chain splitter
  (`split_chain_segments`) is paren / quote aware so ` ? ` inside `(...)` or `'...'`/`"..."` doesn't split.
  Errors: `UnknownFunction`, `FunctionArity { name, expected, got }`, `InvalidBase64`, `NonUtf8Decoded`,
  `InvalidRegex { name, message }`, `InvalidNumber { name, message }`,
  `InvalidNumeric { name, message }` (arithmetic family — non-numeric / non-finite input),
  `DivideByZero` (any divisor in the `divide` fold is `0`),
  `OneOfNoMatch { value, options }` (`one_of` value didn't match the allow-list),
  `CaptureFailed { command, message }` (`capture` shell exited non-zero, failed to spawn, or
  produced non-UTF-8 stdout),
  `ReadFileError(path, msg)`, `WriteFileError(path, msg)` (`write_file` could not write — bad
  directory, permissions, etc.), `TempFileError(msg)` / `TempDirError(msg)` (`temp_file` / `temp_dir`
  could not create the artifact), `InvalidJson(name, msg)`, `InvalidJsonPath(path, msg)`,
  `InvalidTimeFormat(format)` (`now` got an unknown format), `InvalidUrlEncoding(input)` (`url_decode` bad
  `%XX`), `UserError(msg)` (`error(...)` — soft command failure, see below),
  `UnbalancedParens`, `BarewordLiteralNotAllowed`, plus `MalformedSubstitution` for arg-list whitespace violations.
  Bare `ARGS` (no `.key`) is special-cased in [`evaluate_arg`] so it works as a function
  argument too — `one_of(ARGS, 'major', ...)` resolves to the positional-args string at
  evaluation time via `RunArgs::build_remaining_args(...)` (same builder the top-level
  sentinel-replace path uses), so the value snapshots the current `consumed` / `flag_keys` state.
- **`try(expr)` semantics**: catches errors from inner substitutions / function calls and either falls
  through the chain (if there's another segment) or resolves to `""` (if standalone). Implementation: when
  the inner expression errors, `try` returns the internal sentinel [`SubstitutionError::TryFailed`] (wrapping
  the original error in `Box<...>` so the enum stays small). The chain resolver special-cases this variant
  in its function-call branch — instead of propagating, it stores it in `last_error` and continues. If a
  later chain segment succeeds, that value wins; otherwise the loop terminates with `Err(TryFailed)` which
  [`resolve_substitution`] converts to `Ok(String::new())` at the substitution boundary. Net effect:
  `{{ try(X) }}` resolves to `""` when X fails, `{{ try(X) ? 'fallback' }}` falls back to `'fallback'`,
  and `{{ try(X) ? ARG.y ? '' }}` chains naturally. Side-effect tracking (consumed args, flag keys) uses
  scratch sets so a thrown-away inner failure doesn't pollute the caller; on inner success the scratch
  state is committed back. Special-cased BEFORE the bulk arg evaluation in `evaluate_function` (same as
  `define`) so missing `ENV.X` references inside the `try` body don't error out before dispatch.
- **DSL inside substitutions**: a substitution body containing the boolean operators `==`, `!=`, `&&`, `||`, or
  unary `!` at top level (paren / quote / nested-substitution aware — see `looks_like_dsl`) is parsed as a DSL
  expression and evaluated to the literal string `"true"` or `"false"`. Examples:
  `{{ ARG.env == 'prod' }}` → `"true"`/`"false"`,
  `{{ ARG.env != 'development' && ARG.env != 'production' }}` → composite boolean,
  `{{ to_upper(ARG.x) == 'PROD' }}` → function-call values work, `{{ !(VAR.skip == 'yes') }}` → unary negation,
  `{{ RUN.os == 'windows' && FLAG.wsl }}` → bare `FLAG.x` works because it resolves to `"true"`/`"false"`.
  DSL value tokens are evaluated by the same machinery as function args (single-quoted interpolates, double-quoted
  is verbatim, source refs resolve, function calls evaluate, plain barewords error). The DSL evaluator
  ([`eval_dsl_ast`]) reuses the parser's [`runfile_parser::DslExpr`] / [`runfile_parser::DslValue`] AST.
- **Strict DSL truthiness** (aligned with `if`-block rule): a `Truthy` value (anything used as a bare boolean
  inside the DSL — e.g. `FLAG.x`, `ARG.x`, `VAR.x`, or a quoted literal) MUST resolve to `"true"` (truthy),
  `"false"` (falsy), or `""` (falsy). Anything else surfaces as `SubstitutionError::DslValueNotBoolean`. This
  catches patterns like `{{ ARG.env && other }}` where the user expected non-empty-truthiness but `ARG.env`
  is some arbitrary string — the error points them at the explicit comparison form (`{{ ARG.env == 'value' }}`).
  Comparisons (`==` / `!=`) operate on raw strings without the boolean check, so any-string equality still
  works. Short-circuiting (`&&` / `||`) still skips evaluation of later arms when the result is determined.
- **`if` block evaluation** is a thin wrapper over substitution with a strict boolean check: the `condition`
  field is a template string, [`evaluate_if_condition`] substitutes it via `args.substitute(...)`, and the
  resolved value MUST be exactly `"true"` (truthy), `"false"` (falsy), or `""` (falsy). **Anything else
  surfaces as [`ControlFlowError::IfConditionNotBoolean`]** — `"True"`, `"1"`, `"yes"`, `"hello"`, etc. all
  error out instead of being silently coerced. The strict rule catches typos and missing comparisons (someone
  writing `if: "{{ ARG.x }}"` expecting truthiness when `ARG.x` is "yes" gets a clear error pointing them
  toward `{{ ARG.x == 'yes' }}`). The OLD form (`if: "{{ X }} == Y"` with operators outside the `{{ }}`) no
  longer works — migrate to `if: "{{ X == 'Y' }}"`. The if condition is not pre-parsed at Runfile load time;
  errors surface at runtime when the substitution + boolean check run. `IfStep` no longer carries a cached
  `condition_ast` field.
- **`define(name, value)` semantics**: `name` MUST be a bareword identifier matching `[A-Za-z_][A-Za-z0-9_-]*` —
  no quotes allowed. `parse_static_name` rejects substitutions, dotted names, and quoted forms with `InvalidVarName`.
  `value` is resolved through the normal arg pipeline. Resolved value is stored in `RunArgs.vars`
  (`Arc<Mutex<HashMap<String, String>>>`) — read by the `VAR.<name>` chain segment, returns `MissingVar` if not yet
  set (or falls through to the chain default). The Arc is shared across `RunArgs::clone()` so `define`s in a parent
  target are visible to `@target` children (the runner's `RunnerDependencyResolver` and the extract walker both
  thread `with_vars(parent.vars.clone())` into the child `RunArgs`). `evaluate_function` skips the mutation when
  `redact_env` is true so the redacted-pass log substitution doesn't overwrite real values with `***` between the
  real-pass and the next command. Concurrent `define`s from a `parallel: true` block serialise on the lock; relative
  ordering of writes is non-deterministic — last writer wins (documented footgun). `VAR.*` is **not** redacted in
  logs (treated like ARGS) — putting secrets in VARS leaks them to `--logging` output.
- **Declared `vars` (Runfile property)**: the declarative counterpart to `define`. `CommandSpec.vars` /
  `Globals.vars` are `Option<HashMap<String, EnvValue>>` (same value type as `env`). Globals' `vars` are baked
  into each target's `vars` at parse time by `bake_globals_into_target()` (global base, target overrides — same
  merge as `env`). Key validation lives in `parse.rs::validate_var_keys` / `is_valid_var_key`
  (`[A-Za-z_][A-Za-z0-9_-]*`, hyphens allowed — matches the `VAR.<name>` source-key rule so every declared key is
  referenceable; `ParseError::InvalidVarKey`). At runtime, `executor.rs::DeclaredVarsGuard::apply(spec, args, env)`
  runs **after** `build_env` (so values can reference `{{ ENV.* }}`), substitutes each value through the normal
  pipeline (sorted-key order, inserting each before the next so a later var can reference an earlier one), and
  writes them into the shared `RunArgs.vars` map. The guard is RAII: on drop it restores every key it overwrote to
  its prior value (removing keys that were absent). This gives declared vars **per-target scoping like `env`** — a
  parent's vars are visible inside an `@target` dependency (shared `Arc` map), but a dependency's own declared vars
  don't leak back to the parent. A runtime `define(...)` of the same key shadows the declared value for the rest of
  the target (the guard captured the pre-declaration prior, so it restores to *that*, not to the declared value).
  Applied at all three execution entry points: `ExecSetup::new` (holds the guard as a field so it lives for the
  whole sequential/parallel/sameShell walk), the runner's detach branch, and `extract.rs` (`--dry-run`). `apply`
  returns `Result<Option<Self>, SubstitutionError>` so every caller's error type (`ExecuteError` / `RunError` /
  `ExtractError`) absorbs it via `?`. `walk_spec_aux_templates` visits `vars` string values so arg-usage
  validation recognises `{{ ARG.x }}` references that live only inside a var. Like `define`, `VAR.*` is not
  redacted in logs.
- **`set_cwd(path)` semantics**: cwd analog of `define` — universal `cd` for the substitution layer that works on
  every shell / OS without forking a process. `path` is resolved through the normal arg pipeline (substitutions,
  function calls, chained fallbacks). The result is stored on `RunArgs.cwd_override`
  (`Arc<Mutex<Option<PathBuf>>>`) and applied by every spawn site via [`RunArgs::spawn_cwd(working_dir)`]:
  absolute override → use as-is; relative override → `working_dir.join(override)`; no override → `working_dir`
  unchanged. Resolution rules at `set_cwd` call time mirror shell `cd`: an absolute new path REPLACES the
  override entirely; a relative new path JOINS onto the existing override (if any) or falls through to be
  joined with `working_dir` at spawn time. So `set_cwd('a'); set_cwd('b')` lands subsequent commands in
  `working_dir/a/b`, matching `cd a; cd b`. The `Arc` is **NOT** propagated to `@target` children
  (`RunnerDependencyResolver` builds the child via `RunArgs::parse(...)` whose default is `None`), so each
  dispatched target starts fresh from its own `workingDirectory` and the parent's override doesn't leak in.
  Inside `parallel: true` parents, `collect_leaves_parallel*` snapshots `args.cwd_override` after each leaf's
  substitution and stores it on the [`ParallelLeaf::Shell.cwd_snapshot`] — the spawn then resolves cwd from
  that per-leaf snapshot via [`RunArgs::spawn_cwd_from_snapshot`], so siblings don't race on the shared mutex
  at spawn time. Sequential, sameShell, parallel, and detached spawn paths all consult the override
  (sameShell collapses every leaf into one shell invocation, so only the *final* `set_cwd` value applies —
  use shell `cd` directly between sameShell leaves if you need intermediate changes). `for shell:` iterators
  also respect the override (they spawn a shell at planning time). `evaluate_function` skips the mutation
  when `redact_env` is true (same pattern as `define`) so the log-substitute pass doesn't double-apply.
  Returns `""` so a line whose only content is `{{ set_cwd(...) }}` resolves to whitespace and is dropped by
  the empty-command-skip path without consuming a step number.
- **`error(msg)` semantics**: a control-flow function that fails the *current command* without aborting the run.
  In `evaluate_function`, the `error` arm prints `msg` to stderr (only on the real, non-`redact_env` pass — so
  the redacted log pass doesn't double-print) and returns `Err(SubstitutionError::UserError(msg))`. The sequential
  executor's `execute_one_shell` matches that variant specially: it consumes a step, sets `state.last_status =
  failed_status()`, increments `state.failures`, and returns `Ok(())` — so the walker keeps going. Because the
  walker derives `state.failed` from `state.failures` (subject to `ignoreErrors`), this means subsequent
  default-`when: success` steps are skipped, `when: failure` / `when: always` steps still run, and target-level
  `ignoreErrors` swallows it — exactly like a non-zero shell exit. On `args.dry_run` it short-circuits to the
  placeholder `<error: '<msg>'>` (no print, no failure), matching `capture` / `uuid`. **Caveat:** the soft-failure
  handling lives only in `execute_one_shell` (the sequential path). In a `parallel: true` parent or a
  `sameShell: true` target, `error()` fires during *leaf collection* and surfaces as a hard `ExecuteError`
  (the target still fails, but parallel siblings' `when:` partitioning isn't engaged). `error()` is intended for
  sequential command flow — including inside `if` / `match` / `for` / `when` blocks, which all dispatch through
  `execute_one_shell`. Note `try(error('x'))` swallows the failure (try catches the `UserError` sentinel too).
- **Empty-command skip**: when a command line resolves to a whitespace-only string (the typical cause is a line
  consisting only of `{{ define(...) }}`), it is NOT dispatched to the shell — and crucially, NOT counted as a
  step in any way. `execute_one_shell` short-circuits *before* calling `counter.next_step()`, prints no log line,
  and calls [`StepCounter::subtract_from_total(1)`] to roll back the static `count_leaves` estimate so the visible
  `(N/total)` ratio reflects only commands that actually run. `state.commands_run` and `state.last_status` stay
  untouched (a target whose body is purely `define`-only lines reports `commands_run = 0` and `final_status =
  dummy_success_status()` via the standard fallback). `collect_leaves_parallel_with_when` does the equivalent for
  the parallel path — drops empty leaves before they enter the batch *and* calls `subtract_from_total` per drop,
  so `[parallel]` runs don't show inflated totals either; the counter is now threaded through the collector for
  this. `extract_target_with_cwd` skips them in dry-run output so a `define`-only line doesn't show up as a blank
  line.
- **`temp_file` / `temp_dir` cleanup**: artifacts created by these two functions are tracked in a **process-global
  registry** — `static TEMP_ARTIFACTS: Mutex<Vec<TempArtifact>>` in `functions.rs` (each entry is a `path` + an
  `is_dir` flag). A global static (rather than per-`RunArgs` state) is the right shape for two reasons: (1) the CLI
  tears down via `std::process::exit`, which skips destructors, so cleanup must be an *explicit* call — a global is
  the simplest thing the CLI can reach without plumbing the registry back out of the executor; (2) `temp_file` runs
  deep inside substitution, far from any one `RunArgs` lifetime, and the same registry must cover artifacts from
  `@target` children, parallel worker threads, and watch-mode re-runs alike. `pub fn cleanup_temp_artifacts()`
  (re-exported from `lib.rs`) drains the registry and best-effort-deletes every path (`remove_file` /
  `remove_dir_all`; missing/permission errors ignored). The CLI calls it in `cmd_run.rs` right before **both**
  `process::exit` paths (success and the `Err` arm) and **after each watch-mode iteration** (so a long-lived
  `run --watch`-style session doesn't accumulate artifacts). `--dry-run` (`cmd_dry_run`) never wires cleanup because
  the dry-run pass creates nothing. **Caveat:** a hard kill (SIGKILL) or a Ctrl+C that terminates before the run
  completes skips cleanup — those leftover artifacts stay in the OS temp dir for the OS to reclaim. No SIGINT handler
  is installed for this (it would entangle with `force_kill.rs`'s conditional SIGINT machinery and require
  async-signal-safe deletion); normal success/failure exit is the guaranteed cleanup path. Tests that exercise these
  functions serialize on a dedicated lock (`TEMP_TEST_LOCK` in `tests/functions.rs`) because the shared global
  registry means one test's `cleanup_temp_artifacts()` drain would otherwise race a sibling test's create→assert
  window.
- **`FLAG.x` in chain segments and function args**: `resolve_chain_impl` recognises `FLAG.<key>` as a value source
  returning `"true"`/`"false"` (boolean form only — the ternary form's ` : ` would conflict with chain semantics).
  Inside a function arg, `evaluate_arg` routes `FLAG.x [? a [: b]]` to the dedicated FLAGS resolver so the full
  ternary form works there too. Both paths thread `flag_keys` through (so `--key`-token consumption stays correct
  for `{{ ARGS }}` rebuilds).
- `control_flow.rs`: DSL evaluator + `for`-block iterator expansion. `evaluate(&DslExpr, args, env, scope)` walks
  the cached AST against the current substitution context. Truthiness rule: only `""` is falsy — `"false"`, `"0"`,
  etc. are truthy (matches what raw shell commands see). `{{ FLAG.x }}` resolves to `"true"`/`"false"` strings, both
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
  where some namespaces don't define the dispatched target (`@?{{ VAR.ns }}:adb-forward`). The skip only suppresses
  the *missing-target* error; failures *inside* the target's commands are not silenced (use `ignoreErrors` for
  that). At execute time, **both** `target` and `args_template` go through normal substitution (so `{{ ARGS }}` /
  `{{ RUN.* }}` / `{{ ENV.* }}` / `{{ VAR.* }}` resolve), then `args_template` is `shlex`-split into argv before being
  dispatched. Substituting the target name lets dynamic patterns like `@{{ VAR.ns }}:build` (the canonical use
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
  `RunArgs::substitute` as the substitution closure. Re-exports `EnvFileError`, `parse_env_file`, `PrivateKeyProvider`,
  and `LazyPrivateKeys` from `runfile-env`. Threads `available_private_keys: Option<&dyn PrivateKeyProvider>` through
  every executor path so callers control when key resolution actually happens.
- `execute_command()`: walks `spec.commands: Vec<CommandStep>` recursively, executing each leaf shell through the
  resolved shell. `if`/`for` blocks are expanded inline: `if` substitutes its `condition` template through
  [`RunArgs::substitute`] and takes the `then` branch iff the resolved string equals `"true"` exactly. `"false"`
  and `""` (empty) take the `else` branch; **any other value errors out with `IfConditionNotBoolean`**. `for`
  calls `expand_for_iterations`, then for each
  iteration calls `LoopVarGuard::set(value)` to write `VAR.<var>`, recurses into the body, and the guard restores
  the prior `VAR.<var>` value when the loop ends. Behavior on failure
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
  `for in: "namespaces"` + `@{{ VAR.ns }}:dev` get cleanly tagged per-branch output even when the leaves are
  `@target` calls rather than direct shells. Same counter-sharing pattern.
- `parallel_output.rs`: line-buffered, prefixed stdout/stderr for parallel shell children. `spawn_line_pump`
  reads from a `ChildStdout`/`ChildStderr`, splits on `\n` and `\r` (CR-as-soft-break flattens progress-bar
  redraws into chronological append-only lines), strips non-SGR ANSI escapes (cursor movement, erase-line, OSC
  title — SGR `…m` color/style codes are preserved), and writes each completed line prefixed with a bracketed
  label identifying the leaf. The label content comes from the leaf type: a `@target` call shows its FULL
  resolved invocation (`[@build]`, `[@dev --port 5000]`, `[@deploy a b c]`) via `format_target_call_label`; a
  raw shell command shows its resolved text truncated to 12 Unicode scalars via `shell_prefix_label`
  (first non-empty line, trimmed — e.g. `[npm run buil]`). `format_parallel_prefix(step, label)` still uses
  the global step number ONLY to pick one of six cycling ANSI colors so adjacent leaves stay visually
  distinct; the displayed text is the label, not the step number.
  Honors `NO_COLOR` for plain output. `\r\n` is collapsed (the LF after a CR is swallowed) so we
  don't emit spurious empty lines. Each line is one `write_all` to the locked global stdout/stderr handle —
  prefix + content + newline lands as a single atomic write so two children can't interleave mid-line.
  `RUNFILE_NO_LINE_PREFIX=1`/`true` opts out (raw stdio inheritance). Pump threads terminate naturally on EOF
  (i.e. when the child closes its end of the pipe on exit) and are `join`-ed in the wait loop so all output is
  flushed before the function returns.
- Output-prefix inheritance rule: when a parallel batch is reached via an ancestor that already set a prefix
  (i.e. this target was dispatched as `@dep` from an outer parallel parent and `setup.output_prefix.is_some()`),
  every leaf in this batch inherits that prefix verbatim — no per-leaf relabeling. This preserves the
  outer partition identity end-to-end: a `[@web:dev]`-tagged branch keeps that label even when its dispatched
  target is itself `parallel: true`. Nested differentiation isn't applied; only the outermost parallel layer is the
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
  before reaching them, in which case the last shown step number will be lower than the total. `add_to_total(n)`
  bumps the total when `for glob:`/`for shell:` runtime expansion exceeds the static 1-iteration estimate;
  `subtract_from_total(n)` (saturating) shrinks it when a Shell step turns out to be a runtime no-op (empty
  substitution) — both paths keep the visible ratio honest without ever printing a stale `(N/total)`.
- `runner.rs`: high-level `run_target()` function that builds env and dispatches command execution. The CLI calls
  `run_target()` instead of `execute_command()` directly. No globals threading — everything is already on the
  `CommandSpec`. `RunRoot` holds a `StepCounter` initialized to `count_target_leaves(target_name, ...)`, threaded
  through every nested `_with_counter` call so step numbers stay continuous. `forceShell` is substituted against
  the parent env (the parent's already-resolved env for an `@dep` call; empty at top level) before resolution —
  it picks the shell, so the target's own env isn't built yet. `workingDirectory` is a free-form path that
  supports `{{ ... }}` substitution; default (when unset) is `{{ RUN.parent }}` (the target's source Runfile
  directory). After substitution, relative paths are resolved against the target's source Runfile dir via
  `resolve_working_directory_path`. There is no per-value validation — any path string is accepted.
  **`workingDirectory` resolves `{{ ENV.* }}` / `{{ VAR.* }}` against the target's OWN env (globals' `env` is baked
  into every target during merge) and declared `vars`, not just the parent env.** Env VALUES don't depend on the
  working dir — only relative `addToPath` PATH assembly does, and those entries are baked to absolute at parse
  time — so when the `workingDirectory` template contains `ENV.`/`VAR.`, the runner builds the full target env up
  front (via `build_env_with_base` with the source Runfile dir as the addToPath/envFiles base) plus a transient
  `DeclaredVarsGuard`, purely to resolve the path; the executor builds the env again for the actual run. Templates
  with no `ENV.`/`VAR.` reference (a literal path, the `{{ RUN.parent }}` default, `{{ ARG.* }}` / `{{ RUN.* }}`)
  stay on the cheap parent-env path with no extra build. The `--dry-run` extract pipeline mirrors this: it builds
  the env (and applies declared vars) BEFORE resolving `workingDirectory` so dry-run resolves the path identically.
- Has a `windows-sys` dependency (Windows-only) for console ANSI support

### runfile-cli

**Files:** `main.rs`

- Clap-based CLI with colon-prefixed subcommands: `:list`, `:config`, `:env`, `:mcp`, `:completions`, `:generate`,
  `:convert`, `:init`, `:update`. `--dry-run` is a top-level flag (not a subcommand) that prints the resolved leaf shell
  commands to stdout (one per line, no `[runfile]` prefix, no ANSI) instead of executing them — exactly the
  behaviour the removed `:extract` subcommand had. `--dry-run` recursively expands `@target` invocations: the
  dep's resolved leaf shell commands appear inline at the call site (with the dep's own env block reflected on
  each line), so aggregator targets whose body is purely `@target` dispatches (e.g. `for in: namespaces` with
  `@{{ VAR.ns }}:dev`) actually print every nested command rather than printing nothing. `if` blocks are
  evaluated (not flattened) against the same context the runner would see — only the matching branch is
  printed. Cycles are caught at extract time via per-call-stack tracking; sibling calls to the same target
  expand twice (matching runtime no-dedup semantics). Optional calls (`@?target`) silently skip when the target
  is missing. `extract_target_with_cwd` auto-syncs `args.run_context.namespaces` from the merged Runfile (matches
  the runner's `ensure_run_context`) so `for in: "namespaces"` resolves identically in dry-run.
  `for` blocks expand at extract time wherever it's safe to do so: `for in: [...]` substitutes per element,
  `for in: "namespaces"` snapshots the namespace list, and **`for glob:` walks the filesystem** (via
  `control_flow::expand_glob`, threading the target's resolved `working_dir` into [`walk_extract_steps`]) so
  matched paths bind to the loop variable just like at runtime — empty match set yields zero body emissions.
  **`for shell:` is the lone exception**: extract emits the body once with `{{ VAR.<var> }}` bound to a `<var>`
  placeholder, since running an arbitrary shell iterator would have side effects (process spawn, possibly slow
  I/O, possibly stateful). Use `--dry-run` confidently as a read-only preview without worrying about iterator
  commands firing.
- `:update` (`cmd_update.rs`): self-update by re-running the published install script — NOT a bespoke
  download/unpack/swap path. `classify_install(current_exe)` (pure, unit-tested) returns `Npm` when any path
  component is `node_modules` (the npm package extracts to `.../node_modules/@runfile/cli/bin/<platform>/run`),
  else `Standalone`. Npm-managed installs are handled by `update_via_npm`: on Unix it runs `npm install -g
  <spec>` directly (replacing a running binary's file is fine there); on Windows it only PRINTS the command —
  npm would have to overwrite the locked, running `run.exe` and, since npm owns the extraction, the rename-aside
  trick can't be applied, so a mid-reify failure could corrupt the global package. `npm_package_spec(version)`
  (pure, unit-tested) strips a leading `v` from the tag so `--version v0.19.0` and `0.19.0` both yield
  `@runfile/cli@0.19.0`; `None` → `@runfile/cli@latest`. For standalone installs, it sets `RUNFILE_INSTALL_DIR`
  to the running binary's parent dir and shells out to the install script: `sh -c "curl -fsSL <install.sh> | sh
  -s -- <version>"` on Unix; on Windows `powershell -NoProfile -Command "$c=(iwr <install.ps1>
  -UseBasicParsing).Content; if($c -is [byte[]]){$c=[Text.Encoding]::UTF8.GetString($c)}; iex $c"`. **Three
  Windows-specific gotchas, all the hard way:** (1) `-UseBasicParsing` is mandatory — without it Windows
  PowerShell 5.1 pipes the response through the legacy IE DOM engine, which can HANG indefinitely. (2) GitHub
  serves release assets as `application/octet-stream`, so `(iwr ...).Content` is a `byte[]`, not a string —
  decode UTF-8 before `iex` or the bytes stringify to `"36 69 114 ..."` and fail to parse (this was a real
  shipped bug). (3) the version can't ride a positional arg through `iex`, so it's passed via the
  `RUNFILE_VERSION` env var, which `install.ps1` reads as a fallback after `$args[0]`. `install.ps1` also sets
  `$ProgressPreference = 'SilentlyContinue'` so its internal `Invoke-WebRequest -OutFile` of the archive doesn't
  crawl when stdout is redirected (another non-interactive 5.1 slowdown). `--version <tag>` pins a release;
  absent → latest. The install-script URLs are `cfg`-gated per platform so each compile only references
  the one it uses. **Windows self-replacement**: you can't overwrite a running `.exe`, but you can rename it
  (renaming touches only the directory entry, not the locked file data) — `install.ps1` renames any existing
  `run.exe` to a UNIQUE `run.exe.old-<guid>` name before moving the new binary in, so `:update` works while
  `run.exe` is executing. The GUID suffix is deliberate: a fixed `run.exe.old` could already exist and be
  locked (e.g. a prior update's process hadn't exited, or an AV/indexer held a handle), and `Rename-Item`
  cannot overwrite an existing name — so the rename would silently fail, leave `run.exe` in place, and the
  subsequent `Move-Item` would fail with the confusing "Cannot create a file when that file already exists"
  (it can't overwrite the running binary). A fresh GUID name never collides, so the rename always succeeds.
  `install.ps1` then sweeps all `run.exe.old*` aside-files: dead ones from finished prior updates get deleted,
  while the just-created one (the live process image during `:update`) stays locked and its delete is ignored.
  For that locked aside-file, `schedule_old_deletion_at_reboot(exe)` (Windows-only, via `windows-sys`) scans the
  install dir for every `<exe>.old*` and calls `MoveFileExW(.., NULL, MOVEFILE_DELAY_UNTIL_REBOOT)` to register
  each for deletion at the next boot. That registry write needs admin, so on the common per-user install it
  silently no-ops — `install.ps1`'s next-update sweep is the guaranteed fallback either way.
  Unix `mv`-over-running-binary already works, so `install.sh` is unchanged.
- Global flags: `-f`/`--file` (custom Runfile path), `--timings` (print execution times), `-y`/`--yes` (skip
  confirms), `--stdin-args` (prompt for missing `ARG.*`/`ENV.*`/`FLAG.*` instead of erroring),
  `--dry-run` (print resolved leaf shell commands without executing). For inline debug branching, declare
  `{{ FLAG.debug }}` in your target — passing `--debug` (or any other flag) to a target works through the
  standard FLAGS substitution.
- `RUNFILE_TARGET` env var: when `-f`/`--file` is not passed, this env var is used as the default Runfile path before
  falling back to auto-discovery. Defined in `runfile_helpers.rs` (`RUNFILE_TARGET_ENV_VAR`, `runfile_target_env()`,
  private `effective_file()`) and applied by `resolve_runfile_path`, `resolve_and_merge`, `cmd_list_targets` (in
  `completions.rs`), and `cmd_mcp_server`. Empty string is treated as unset. When set, the path is required to exist
  (or be a registered path alias) — no fallback to discovery — so misconfigured CI fails fast. `-f` always wins.
- The `:env` command lives in the `cmd_env/` module: `mod.rs` (shared helpers, `:env init`/`get`/`set`/`inject`,
  and the inline tests), `crypt.rs` (`:env decrypt`/`encrypt`/`rotate`), and `secret_keys.rs` (the `secret-keys`
  subgroup). `mod.rs` re-exports the submodules' `cmd_*` fns so callers keep using `cmd_env::cmd_*`.
- `RUNFILE_ENV_FILE_TARGET` env var: complement of `RUNFILE_TARGET` for `:env` commands. Defined in `cmd_env/mod.rs`
  (`RUNFILE_ENV_FILE_TARGET_ENV_VAR`, `env_file_target()`). Consumed by **`:env inject`** (when no positional file
  paths are given) and **`:env decrypt`** (when no positional source is given). Empty string is treated as unset.
  There is **no implicit `.env` fallback** anymore — `:env inject` without a positional file and without the env var
  is a hard error. `:env get/set/encrypt/rotate` still require an explicit positional file (the env var is ignored for
  them; the secret-supplied file is meant for run/decrypt flows, not for mutating commands). Set by the
  `.github/actions/setup` action's `env-file-source` input, which writes the supplied source to
  `$RUNNER_TEMP/runfile-source/.env` and exports the env var via `$GITHUB_ENV`, so open-source repos can keep the
  encrypted env file in a GitHub secret instead of committing the ciphertext.
- `:env` positional-file convention: every `:env` subcommand takes the file path as the **first positional
  argument**. `:env init [path]` defaults `path` to `.env` (the legacy `-p`/`--path` flag was removed). `:env inject
  [file...]` takes one or more files as positionals before `--` (the legacy `-f`/`--file` flag was removed; multi-file
  semantics — later files override earlier — are unchanged). Parser-level: `Inject.command` uses `last = true` to
  require the `--` separator before the command, and `file: Vec<String>` collects positionals before `--`.
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
- Shell completion scripts (`completions.rs`, one hand-written script per shell: bash/zsh/fish/powershell) fall back
  to the shell's native **file completion** once the cursor is past a subcommand into positional-argument territory
  (e.g. `run :env decrypt <TAB>`, `run :config shell set bash <TAB>`, or any target's args like `run build <TAB>`).
  The detection is a single `run --list-subcommands <dotted.path>` query built from the already-typed non-flag words:
  `cmd_list_subcommands` prints nothing both for a leaf subcommand *and* for a path that runs past the leaf into args
  (its `find_subcommand` walk early-returns) — exactly the two cases where files should be offered — so one query
  distinguishes "still choosing a subcommand" (non-empty children → complete names) from "typing an argument"
  (empty → complete files) at arbitrary depth. A first word that isn't `:`-prefixed is a target name, so its args go
  straight to file completion. Bash needs a `compgen -f` fallback inside its `_run_files` helper because
  bash-completion's `_filedir` returns nothing for an *empty* current word (it quotes `""` into a literal `''`);
  zsh uses `_files`, fish re-enables files via a `__run_needs_files` predicate paired with a `-F` rule (the global
  `complete -c run -f` still disables files everywhere else), and powershell uses
  `[CompletionCompleters]::CompleteFilename`.
- CI detection lives in `ci_detect.rs` (used by `:env secret-keys add --key` to gate non-interactive key
  entry; `is_ci_with(env_lookup)` is the testable pure form, `is_ci()` reads `std::env`). The list of CI vars:
  any non-empty value in `CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `CIRCLECI`, `TRAVIS`, `BUILDKITE`, `JENKINS_URL`,
  `TF_BUILD`, `TEAMCITY_VERSION`, `BITBUCKET_BUILD_NUMBER`. There is no LLM-agent guard — commands that emit
  decrypted secrets, private keys, or resolved env values (`:env get`, `:env decrypt`, `:env secret-keys list`,
  `:env secret-keys get-private`, `--dry-run`) run unconditionally regardless of invocation environment.
- `:env secret-keys add --key <hex>` is the non-interactive path used by CI (e.g. the
  `.github/actions/setup` action's `secret-keys` input). Gated by `ci_detect::is_ci()` — refuses to run on dev
  machines so the private key doesn't end up in shell history. Validates the key is 64-char hex, derives the
  public-key fingerprint, and stores via `keyring_keys::add` (same OS credential store path the interactive
  flow uses). Without `--key`, the existing interactive prompt-based flow runs unchanged. The GitHub Action
  also bootstraps a session D-Bus + `gnome-keyring-daemon` on Linux runners (where no Secret Service is
  present by default) before calling this command.

## Runfile.json Schema Quick Reference

Top-level: `$schema` (required), `targets` (required), `globals` (optional)

Target properties: `commands` (required), `description`, `aliases`, `env`, `vars`, `envFiles`, `forceShell`, `addToPath`,
`logging`, `ignoreErrors`, `parallel`, `detach`, `sameShell`, `workingDirectory`, `confirm`, `forceKillOnSigInt`,
`extendStdio`, `watch`, `onlyInDirectories`, `metadata`

Global properties: `addToPath`, `env`, `vars`, `envFiles`, `forceShell`, `logging`, `ignoreErrors`, `sameShell`,
`forceKillOnSigInt`, `workingDirectory`, `onlyInDirectories`, `metadata`

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
- Substitution (`{{ ARG.key }}` without default) is a hard error, not silent empty string. This catches mistakes. Use
  `{{ ARG.key ? }}` for intentional optional-with-empty-default.
- `--stdin-args` (top-level CLI flag, like `--dry-run`): when set, prompts the user via stdin for
  **genuinely-missing** `{{ ARG.x }}` / `{{ ENV.X }}` / `{{ FLAG.x }}` inputs instead of erroring — i.e. only
  references that have NO default and don't otherwise resolve. **A chain that reaches a literal default
  (`{{ ARG.x ? 'y' }}`, or the empty-string default `{{ ARG.x ? }}`) is resolved to that default WITHOUT
  prompting** — the literal-default branch of `resolve_chain_impl` `return`s before any prompt; the user's
  earlier behavior of "prompt even when a default exists, showing it in `[brackets]`" was removed. The prompt
  key is the FIRST `ARG.*` / `ENV.*` segment in the chain (the user-facing "primary name"). A non-empty answer
  is used; an empty answer (just Enter) surfaces the existing `MissingArg`/`MissingEnv` error. Because prompts
  now fire only for no-default values, the `[brackets]` / `[empty]` default-hint is gone — `prompt_value` no
  longer takes a `default` param and the prompt always shows `(required)`. Bare `{{ ARGS }}` (positional args)
  is still prompted under the same flag: when the sentinel appears in the substituted template and the
  remaining positional args would resolve to an empty string, [`RunArgs::resolve_args_sentinel`] prompts with
  key `"ARGS"` (there's no default mechanism for positional args). This covers targets like `bump` whose
  `match` value is `{{ ARGS }}` — without the prompt, `match` would resolve to `""` and surface `MatchNoCase`
  immediately. The prompter cache (key `"ARGS"`) ensures the user is asked at most once per run; the
  redacted-logging pass reuses the cached answer. `VAR.*` and `RUN.*` are NEVER prompted (they're runtime
  context, not user input). **Flags: only the bare boolean form `{{ FLAG.x }}` (no ternary / value part) is
  prompted** as `pass --x? (y/N)` (accepts `y`/`yes`/`true`/`1` as presence) — the ternary / value forms
  (`{{ FLAG.x ? 'a' : 'b' }}`, `{{ FLAG.x ? 'v' }}`) carry their own default (the false branch / empty) and
  resolve without prompting (gated on `ternary_part.is_none()` in `resolve_flag`). Answers are cached per
  (kind, key) so the same value is asked at most once per run, even across `@target` invocations (the
  `Arc<dyn StdinPrompter>` is propagated through `RunnerDependencyResolver`). Works with `--dry-run` too (the
  dry-run path also goes through `RunArgs::substitute`).
- Runtime context substitutions: `{{ RUN.os }}` / `{{ RUN.arch }}` / `{{ RUN.shell }}` / `{{ RUN.cwd }}` /
  `{{ RUN.file }}` / `{{ RUN.parent }}` expose runtime info so users can write inline `if` conditions for
  cross-platform branching (e.g. `"if": "{{ RUN.os }} == windows"`) or reference paths in env values,
  `workingDirectory`, etc. Keys:
  - `RUN.os` — `"windows"` / `"linux"` / `"mac"`
  - `RUN.arch` — `"x86-64"` / `"arm64"` / `"riscv64"` / `"unknown"` (friendly names mapped from
    `std::env::consts::ARCH`; reflects the binary's compile-time target arch, not the host CPU)
  - `RUN.shell` — `"bash"` / `"zsh"` / `"sh"` / `"fish"` / `"powershell"` / `"cmd"`
  - `RUN.cwd` — caller's current working directory (absolute path; fixed for the whole run)
  - `RUN.file` — path to the source Runfile of the *currently-executing target*
  - `RUN.parent` — directory of the currently-executing target's source Runfile
  Unknown RUN keys are a hard error. RUN values are not redacted by `substitute_redacted` (they aren't secrets).
  `RUN.os`/`RUN.arch` are fixed for the whole run. `RUN.shell`/`RUN.file`/`RUN.parent` are refreshed per-target by the runner's `ensure_run_context`, so a target
  defined in an included file — or a target whose `forceShell` swap changes the active shell — sees values that
  match its own context. `RUN.cwd` is the caller cwd captured once at the top-level CLI invocation.
- `forceShell` and `workingDirectory` accept `{{ ... }}` substitutions. `workingDirectory` is a free-form path
  (absolute or relative) with no per-value validation — anything that resolves at runtime is accepted. Default
  (when unset) is `{{ RUN.parent }}` (target's source Runfile dir). Relative paths are resolved against the
  target's source Runfile directory by `resolve_working_directory_path`. `workingDirectory` can reference the
  target's own `env` (including values inherited from `globals.env`, which are baked into each target during
  merge) and declared `vars` via `{{ ENV.* }}` / `{{ VAR.* }}` — the runner builds the target's full env before
  resolving the path (see the runner section above). `forceShell`, by contrast, is resolved before the shell (and
  thus the target's env) is known, so it only sees the parent/inherited env, not the target's own `env` block.
- Control flow (`if` / `for` / `match` blocks) is evaluated by Runfile itself, not by the shell — same semantics
  on every platform. Conditions use a tiny boolean DSL parsed at Runfile load time (errors fail fast). Truthiness
  rule: only
  the empty string is falsy; every other string (including `"false"` and `"0"`) is truthy. This matches what raw
  shell commands see when they receive a `{{ ... }}` substitution. Because `{{ FLAG.x }}` resolves to `"true"` or
  `"false"` (both non-empty), flag presence checks must use explicit `== true` / `== false` comparisons. Mixing
  `&&` and `||` in the same expression is a parse error — parens are required to disambiguate. `for` blocks accept
  one of `in: [...]` (literal array, each element substituted), `glob: "..."` (filesystem glob, sorted matches),
  or `shell: "..."` (run command at planning time, iterate trimmed non-blank stdout lines). `for shell` failure is
  a hard error regardless of `ignoreErrors`. Loop variables are written into the run-wide `VARS` map (the same
  store `define(...)` populates) and referenced as `{{ VAR.<name> }}`. Scoping is save/restore via
  `LoopVarGuard`: outer `VAR.x` is preserved across an inner `for x in [...]`, and nested loops with the same
  name compose correctly. Loop iteration values are visible to dispatched `@target` children via the shared
  `Arc<Mutex<HashMap>>`, but inside a `parallel: true` parent the children may race on the shared map (the
  iteration value is baked into pre-substituted shell leaves, so direct shell uses are safe; only nested
  `@target` bodies that read `{{ VAR.x }}` as a loop variable can race). `parallel: true` on a `for` block runs
  iterations concurrently, but **outer parallel only**: a nested `for parallel: true` inside an already-parallel
  context is forced sequential (with a warning).
- `ignoreErrors` makes the CLI exit 0 even when commands fail — this is the specified behavior, not a bug. **It
  also contains failures across `@target` boundaries**: when a target with `ignoreErrors: true` is invoked from a
  parent via `@target`, the dep self-reports as success to the caller (`failures: 0`, `final_status: success`),
  symmetric with how block-level `ignoreErrors` on `for`/`if`/`when`/`match` already swallows local failures.
  Without this, the dep's internal failure count would surface to the caller's `state.failures`, flip the
  caller's `failed` flag, and skip every subsequent default-`when: success` sibling. The containment lives in
  `RunnerDependencyResolver::run_dependency` (`runner.rs`) — looking up `spec.ignore_errors` on the called
  target after `run_target_inner` returns. Direct CLI invocation goes through `run_target_with_cwd` (not
  `run_dependency`) so a top-level `run _target_with_ignoreerrors` still sees the raw `ExecutionResult` from
  `execute_command_with_counter` (failure count + `final_status` reflecting the last command's actual exit).
- `parallel` spawns all shell commands simultaneously; their stdout/stderr is piped through line-buffered reader
  threads that prefix every line with a bracketed label — the full resolved `@target` call (`[@dev --port 5000]`)
  or the raw command truncated to 12 chars (`[docker compo]`), colored by a per-step cycling palette — and strip
  non-SGR ANSI cursor-control
  escapes — so progress-bar redraws (`docker compose pull`, etc.) become append-only chronological lines instead
  of corrupting interleaved output. Stdin is inherited. **Failure summary**: when at least one leaf in a parallel
  batch failed, [`log_parallel_failure_summary`] prints a final `[runfile] [parallel] N command(s) failed:` block
  to stderr right after the batch completes, listing each failed leaf by its label (raw shell template, or
  `@target args...` for dispatched targets) and its detail (`exit code N` / `terminated by signal` / `error: ...`).
  This fires **regardless of `ignoreErrors`** (the flag silences the propagated error, not the diagnostic) — so
  silent-failure cases like `parallel: true` aggregator targets that `@dep` into namespaced sub-targets stop
  hiding which branch broke under interleaved output. Helpers: `format_target_call_label` (also reused by the
  per-leaf log line so labels stay consistent), `dep_result_failure_detail`, `execute_error_failure_detail`. **`@target` calls inside a parallel parent propagate
  the parent's prefix through the entire dispatched dependency subtree** (via `DependencyResolver::run_dependency(..., output_prefix)`),
  so a `dev` target that fans out via `for in: "namespaces"` + `@{{ VAR.ns }}:dev` gets every nested shell tagged
  with its parallel branch identity. Set `RUNFILE_NO_LINE_PREFIX=1`/`true` to disable prefixing and inherit raw
  stdio. The target finishes when all commands exit. With `ignoreErrors`, failures are counted but exit is 0.
- `detach` requires `parallel: true` (or `sameShell: true`) when there are multiple commands. With `parallel`,
  commands are spawned in parallel as detached background processes and the CLI exits immediately. With
  `sameShell`, the joined command spawns as one detached process. Without either, the parser rejects multi-command
  detach targets (`ParseError::DetachRequiresParallel`).
- `sameShell` collapses every step into a SINGLE shell invocation. The runtime path is
  [`execute_same_shell_with_counter`] in `runfile-executor`: walk the commands tree via
  [`collect_shell_only_leaves`] (which expands `if`/`for`/`match`/`when` into a flat string list, evaluating
  conditions / iterations against the live substitution context exactly like the regular executor; rejects
  `@target` invocations because they need their own shell context); drop empty-after-substitution leaves and
  bump `subtract_from_total` so the `(N/total)` ratio stays honest; join the remaining leaves with
  [`join_shell_commands`] (`&&` for stop-on-failure, `;` for `ignoreErrors: true`, `&` for cmd.exe ignoreErrors);
  spawn one shell. The runner's `count_target_leaves_recursive` short-circuits to `1` for sameShell targets so
  the global counter is sized correctly from the entry point. `parallel: true` collapses into the single
  invocation (warning printed). `detach: true` joins via the same logic and spawns the joined command via
  `execute_detached`. Inner-block `ignoreErrors` flags are NOT honored — the only knob is the target-level
  `ignoreErrors`, which controls the join separator. Dry-run extract follows the same flatten path
  (`extract_recursive_inner` returns one `ExtractedCommand` per sameShell target with `command` set to the joined
  string) so `--dry-run` output matches what really runs.
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
  See `run_parallel_leaves` in `executor.rs`. `execute_parallel_with_counter` computes its returned `final_status`
  with the SAME invariant as the sequential walker (`run_target_inner_body`): when `state.failed` is set, it uses
  `state.last_status` only if non-success, else synthesizes `failed_status()`. This matters because parallel
  `@target` leaves overwrite `state.last_status` in completion order — a later-iterated dep that succeeds would
  otherwise leave a success status even though an earlier dep failed, and the CLI derives the process exit code
  from `final_status.code()` alone (it does NOT consult `failures`). Note `run_parallel_batch` only sets
  `first_error` (→ returns `Err`) for shell-leaf non-zero exits and for `@target` calls that return `Err`; a
  `@target` returning `Ok(dep_res)` with `dep_res.failures > 0` is recorded into `state.failures` /
  `state.failed` and the failure summary but does NOT become a `first_error` — so the non-zero exit code flows
  through `final_status`, not through a propagated error + generic "Error:" line.
- `match` blocks (`{ "match", "cases", "default"?, "ignoreErrors"?, "when"? }`) provide multi-way dispatch on a
  substituted value with built-in case validation. The `match` template goes through the normal substitution
  pipeline (chained fallbacks supported, e.g. `{{ ARG.tier ? ENV.TIER ? '1' }}`). `cases` keys are compared by exact
  string equality against the resolved value. When no case matches, `default` runs if set; otherwise execution
  errors out and the message lists every valid case so the user knows what values to pass. When the `match`
  substitution itself fails (e.g. `{{ ARG.tier }}` with no `--tier` and no chain default), `default` also runs as
  a fallback for the unresolvable value — only when there's no `default` does the substitution error propagate
  (with the case list appended). Cases are stored in a `BTreeMap` so error-message ordering is deterministic
  (alphabetical). `count_leaves` sums every case + default (worst case, like `if`'s both-branches counting).
- `metadata` (fully open object — accepts ANY property of ANY JSON type): available on `globals` and on each
  target. The `Metadata` struct is intentionally NOT marked `deny_unknown_fields` and uses a
  `#[serde(flatten)] extra: HashMap<String, serde_json::Value>` catch-all, so editor extensions, CI scripts,
  and other tooling can stash arbitrary fields here — strings, numbers, booleans, arrays, deeply-nested
  objects, mixed-type structures — without parser errors. Globals' `metadata` is merged into each target's
  `metadata` at parse time by `bake_globals_into_target()` (the helper is `merge::merge_metadata`):
  for each known key the target value wins; otherwise the global value carries through; arbitrary `extra`
  entries from globals are folded into the target's `extra` map (target keys win on collision).
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
- Encryption key resolution: `RUNFILE_ENCRYPTION_PUBLIC_KEY` from the env is matched against the private-key pool
  returned by `keyring_keys::all_private_keys()`, which itself merges `RUNFILE_PRIVATE_KEYS` (env-supplied,
  newline-separated, ephemeral — meant for CI/CD) with the OS credential store (persistent local registration). No
  match → error. The earlier `RUNFILE_ENCRYPTION_KEY` single-key shortcut is gone — `RUNFILE_PRIVATE_KEYS` covers
  every case it served plus multi-key scenarios.
- The `:env` subcommand operates on `.env` files: `init` (create new, optionally encrypted with `--plain`/`--key`
  flags), `get` (auto-decrypts), `set` (auto-encrypts, `--plain` to skip encryption; `value` arg is optional — when
  omitted, reads from stdin until EOF and strips a single trailing `\n`/`\r\n`, so secrets stay out of shell history
  and shell-special characters like `$`/`!` need no escaping), `decrypt` (file→file), `encrypt` (
  file→file with public key prefix match), `rotate <file> [--delete-current-key]` (generate a new private key,
  decrypt every encrypted value with the old key, re-encrypt with the new key, rewrite the file with the new
  `RUNFILE_ENCRYPTION_PUBLIC_KEY` header — plaintext lines / comments preserved verbatim; the new key is added
  to the OS credential store; the old key stays in place by default so other files encrypted with it keep
  working, and `--delete-current-key` removes it after rewriting — only safe once every file using the old key
  has been rotated, otherwise those files become permanently undecryptable), and `inject` (run a command with
  env vars from one or more `.env` files injected, à la `dotenvx run` — `-f <file>` repeatable, defaults to
  `.env`, encrypted values auto-decrypted in memory, command runs after `--`, `RUNFILE_ENCRYPTION_PUBLIC_KEY` is
  stripped before injection, the child's exit code is propagated). **Parent process env always wins**: after files are merged and decrypted, any key already
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
  `"1"`, or when `--yes`/`-y` flag is passed. The `confirm` string is printed verbatim — `{{ ... }}` substitution is
  NOT applied at the prompt site (see `runner.rs`). The string IS visited by `walk_spec_aux_templates` so static
  arg-usage validation picks up `{{ ARG.x }}` references inside it, but the runtime prompt shows the raw template.
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
- **`--stdout` on all three `:generate` subcommands** (a per-subcommand `bool` flag, default off; in `main.rs`'s
  `GenerateAction` variants) prints the generated config to stdout instead of writing to disk. In this mode the
  handlers (`cmd_generate_*` in `cmd_utilities.rs`) emit the **freshly generated** config — NOT merged with any
  existing on-disk file — formatted per `.editorconfig` for the path it would occupy, and perform **no** disk
  reads/writes (no existing-file read, no `.vscode`/`.zed`/`.run` dir creation, no stale sweep, no summary
  messages). Bytes are written verbatim via the shared `write_generated_to_stdout` helper (exact bytes, no added
  trailing newline, so redirects/pipes match on-disk output; a broken pipe exits 0 quietly). JetBrains produces one
  file per target, so `--stdout` emits each config's XML and — only when there's more than one — prefixes each with
  a `<!-- <run_dir>/<file> -->` comment delimiter (a single config is emitted verbatim for a clean redirect into a
  `.run.xml`). The non-`--stdout` path is unchanged.
- **`--include-namespaces` on all three `:generate` subcommands** (a per-subcommand `bool` flag, default off; in
  `main.rs`'s `GenerateAction` variants, threaded through to each `cmd_generate_*`). By default the generators
  operate on the local Runfile's own targets only (single-file parse). With the flag, they operate on the local
  Runfile *with its `includes` resolved* — namespaced included targets carry their `namespace:` prefixes exactly
  as `run :list` shows them (e.g. `api:build`), and plain (un-namespaced) includes contribute their targets
  verbatim. The resolution goes through the shared `runfile_helpers::runfile_for_generate(file, include_namespaces)`
  helper: it always parses the local Runfile (respecting `-f`/`--file`, path aliases, `RUNFILE_TARGET`); when the
  flag is set it additionally runs `merge_runfiles(Some((runfile, path)), &[], &cwd)` — an **empty** global-file
  slice, so **global user-level Runfiles are deliberately never pulled in** (generated editor configs stay
  scoped to the project) — and returns `MergeResult.runfile`. Conflicting targets (defined in multiple files) are
  dropped by the merge, so they never reach the generators. The generators themselves are unchanged: they iterate
  `runfile.targets` regardless of source, and JetBrains' `sanitize_file_name` already maps the `:` in namespaced
  names to `_` for the on-disk `Runfile_api_deploy.run.xml` filename while the `run --stdin-args api:deploy`
  invocation keeps the prefix. Composes with `--stdout` (each handler resolves the Runfile once, before branching
  on `stdout`). The non-flag path is byte-for-byte unchanged.
- All three editor generators (`vscode`, `zed`, `jetbrains-run-configurations`) inject `--stdin-args` into the
  generated invocation. Editor run configs are static (no per-invocation arg prompt UI built into the IDE), so
  `--stdin-args` is what lets a static config still cover targets that need user input — missing `{{ ARG.x }}` /
  `{{ ENV.X }}` / `{{ FLAG.x }}` values are prompted at run time. JetBrains keeps `EXECUTE_IN_TERMINAL=false` (the
  default Run/Services tool window) — do NOT flip it to `true`. `check_existing_jetbrains_config` accepts both
  the current `run --stdin-args <target>` form and the legacy `run <target>` form so older configs upgrade in
  place.
- **Stale-entry pruning across all three editor generators.** Re-running a `:generate <editor>` command after a
  target is renamed or removed deletes the now-orphan entry/file instead of leaving zombies behind. The
  ownership check is structural so hand-authored entries are never touched:
  - VS Code (`merge_vscode_tasks`) / Zed (`merge_zed_tasks`): the private `is_vscode_task_ours` /
    `is_zed_task_ours` predicate matches `command == "run"` AND
    `label == format!("run {target}")` AND either the current `["--stdin-args", target, ...]` arg shape OR
    the pre-`--stdin-args` legacy shape `[target, ...]`. Any existing task that passes the predicate but isn't
    in the new generated set is removed. The merge result carries a third `removed: Vec<String>` field
    alongside `added` / `updated`; the CLI reports it under a "Removed:" section and the `:generate` "nothing
    to do" early-return now also requires `removed.is_empty()`.
  - JetBrains: public `is_jetbrains_config_ours(contents: &str) -> bool` (in `jetbrains.rs`) anchors on three
    markers our generator always emits together: `type="ShConfigurationType"` + `SCRIPT_TEXT" value="run ` (the
    shared prefix of both current `run --stdin-args …` and legacy `run …` forms) + our distinctive
    `SCRIPT_WORKING_DIRECTORY" value="$PROJECT_DIR$"`. The CLI (`cmd_generate_jetbrains_run_configs`) scans
    `.run/` for `Runfile_*.run.xml` files not in the generated `file_name` set; for each it reads the file and
    calls `is_jetbrains_config_ours` before deleting. User-written XML in `.run/` (even something accidentally
    matching the filename pattern) is left alone unless all three markers are present. The "Removed:"
    section prints alongside "Added" / "Updated" / "Skipped"; the no-op early-return also gates on
    `removed.is_empty()`.
- **EditorConfig-conformant output for every file Runfile writes.** The files `:generate` writes
  (`.vscode/tasks.json`, `.zed/tasks.json`, `.run/*.run.xml`) **and** the `Runfile.json` written by `:init`
  and `:convert` are formatted to match the project's [`.editorconfig`](https://editorconfig.org) settings for
  that path. The support lives in `runfile-generators/src/editorconfig.rs` — a dependency-free, self-contained
  module (mirrors the crate's hand-rolled-parser style; adds only a `tempfile` **dev**-dependency for the
  filesystem resolution tests). It:
  - Parses `.editorconfig` (INI-ish: `[section]` glob headers + `key = value`, `#`/`;` comments, preamble
    `root = true`), walking up from the target file's directory and stopping at the first `root = true`.
  - Matches section globs against the file path with `section_matches` (a hand-rolled EditorConfig glob matcher:
    brace expansion `{a,b}` / `{n..m}` first via `expand_braces`, then a recursive `*` (non-`/`) / `**` (any) /
    `?` / `[...]`/`[!...]` matcher). No-separator patterns match the basename at any depth; patterns with a `/`
    are anchored to the config dir. Known limitation: an explicit `**/foo` doesn't match a top-level `foo` (the
    zero-directory case) — no realistic section for these files needs it.
  - Merges matched sections into `EditorConfigProps` (farthest file first, later sections win; a value of
    `unset` clears the property). Raw values are merged as strings, then interpreted once, so `unset` and
    last-wins compose correctly.
  - `EditorConfigProps::indent_unit()` yields the one-level indent string (`"\t"` or N spaces from
    `indent_size` / `tab_width`, `None` → keep the renderer default). `EditorConfigProps::apply(text)` produces
    the final bytes: normalizes line endings (`end_of_line`), optionally `trim_trailing_whitespace`, applies the
    `insert_final_newline` policy (`None` = preserve whatever the renderer emitted), and prepends a UTF-8 BOM for
    `charset = utf-8-bom` (other charsets are not re-encoded — text is always written UTF-8).
  - Rendering: `render_vscode_tasks` / `render_zed_tasks` serialize JSON via `serialize_json_with_indent`
    (a `serde_json` `PrettyFormatter::with_indent` wrapper) then `apply`; `render_jetbrains_config` rebuilds the
    XML with the resolved indent unit (`build_jetbrains_run_config` takes `indent: Option<&str>`; `i1` = one
    level, `i2` = two) then `apply`. The CLI (`cmd_utilities.rs`) resolves `EditorConfigProps::resolve_for_path`
    per output file and calls these instead of the old `to_string_pretty` / `config.xml` write paths.
  - `:init` / `:convert` (writing `Runfile.json`): `write_runfile_to_path` (in `runfile_helpers.rs`, used by
    both `:convert package-json` and `:convert makefile`) resolves props for the target path and serializes via
    `serialize_json_with_indent` + `apply`. `cmd_init` writes the fixed tab-indented `INIT_TEMPLATE` through
    `EditorConfigProps::apply_to_tab_indented`, which retargets the template's one-tab-per-level indentation to
    the resolved indent unit (keeping tabs when the resolved style is tab or unset, via the private
    `retab_leading`) and then runs `apply`. Using the template + retab (rather than reparsing/reserializing the
    JSON) preserves the template's exact key order (`description` before `commands`), which a `serde_json::Value`
    round-trip would alphabetize.
  - **Backward compatible:** with no applicable `.editorconfig`, `EditorConfigProps::default()` reproduces the
    historical output byte-for-byte (2-space indent, LF, no trailing newline for JSON; the XML keeps its trailing
    newline). `generate_jetbrains_configs` still populates each config's `xml` with the default-indent form (used
    by the ownership-check tests); the CLI just re-renders with resolved props before writing. Resolution errors
    (unreadable `.editorconfig`) degrade to "no settings" rather than failing generation.

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
6. CLI command *behavior* (as opposed to arg parsing) is tested by driving the compiled binary as a subprocess:
   `runfile-cli/tests/generate_and_write_cli.rs` runs `env!("CARGO_BIN_EXE_run")` in an isolated temp dir (with
   `HOME` / `XDG_CONFIG_HOME` / `APPDATA` pointed at an empty dir so user settings and global Runfiles can't leak
   targets in) and asserts on stdout and written files. This is where `:generate --stdout` (prints, touches no
   disk), the `.editorconfig`-aware file output, and `:init` / `:convert` formatting are covered end-to-end —
   the `cmd_*` handlers themselves aren't unit-testable inline (they `process::exit` and use CWD-relative paths).
   Pure formatting logic stays unit-tested in `runfile-generators` (`src/tests/editorconfig.rs`); the subprocess
   tests only assert the wiring.

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
