//! Built-in substitution functions: the `{{ funcname(...) }}` registry plus its
//! numeric / json / regex / string / shell-quoting helpers.
//!
//! [`evaluate_function`] is the dispatch entry point, called from the chain
//! resolver in [`crate::args`]. It resolves each argument via
//! [`crate::args::evaluate_arg`] (so nested calls and chained-fallback args
//! work), then dispatches by function name. The side-effecting `define` /
//! `set_cwd` / `try` functions are handled before the bulk arg-eval pass.

use crate::args::{FuncCall, RunArgs, SubstitutionError, evaluate_arg, parse_static_name};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Evaluate a parsed function call against the substitution context.
///
/// `define(name, value)` is special-cased: its first argument is a static
/// label (validated by [`parse_static_name`]) and is NOT resolved as an
/// expression; the second argument is resolved normally and stored in the
/// run-wide VAR map. Returns `""` so a command line containing only a
/// `{{ define(...) }}` resolves to an empty string and is skipped by the
/// shell-dispatch path.
///
/// Every other function resolves all of its arguments first (so nested
/// `to_upper(to_lower(ARG.x))` works naturally), then dispatches by name.
pub(crate) fn evaluate_function(
	args: &RunArgs,
	call: &FuncCall,
	env: &HashMap<String, String>,
	consumed: &mut HashSet<String>,
	flag_keys: &mut HashSet<String>,
	redact_env: bool,
) -> Result<String, SubstitutionError> {
	// `try(expr)` MUST be handled before the bulk arg-evaluation pass so that
	// errors from evaluating its single argument are caught rather than
	// propagated. If we let the regular path pre-resolve all args, a missing
	// `ENV.X` inside the `try` body would surface before we ever reached the
	// dispatch arm and `try` would be useless.
	//
	// On inner failure we return [`SubstitutionError::TryFailed`] (an internal
	// sentinel). The chain resolver treats it as a fall-through so
	// `{{ try(X) ? 'fallback' }}` works as expected; the top-level
	// substitution boundary catches it and converts to `""` so a standalone
	// `{{ try(X) }}` still resolves cleanly when X errors.
	if call.name == "try" {
		if call.args.len() != 1 {
			return Err(SubstitutionError::FunctionArity {
				name: "try".into(),
				expected: "1".into(),
				got: call.args.len(),
			});
		}
		// Use scratch sets for `consumed` / `flag_keys` so a thrown-away
		// failed evaluation doesn't pollute the parent's args-tracking — if
		// the inner expression *succeeds* we still propagate them, matching
		// what a normal substitution would do.
		let mut scratch_consumed = consumed.clone();
		let mut scratch_flags = flag_keys.clone();
		match evaluate_arg(
			args,
			&call.args[0],
			env,
			&mut scratch_consumed,
			&mut scratch_flags,
			redact_env,
		) {
			Ok(v) => {
				*consumed = scratch_consumed;
				*flag_keys = scratch_flags;
				return Ok(v);
			}
			Err(e) => {
				return Err(SubstitutionError::TryFailed { source: Box::new(e) });
			}
		}
	}

	if call.name == "define" {
		if call.args.len() != 2 {
			return Err(SubstitutionError::FunctionArity {
				name: "define".into(),
				expected: "2".into(),
				got: call.args.len(),
			});
		}
		let name = parse_static_name(&call.args[0])?;
		let value = evaluate_arg(args, &call.args[1], env, consumed, flag_keys, redact_env)?;
		// Only mutate the run-wide store on the non-redacted path. The
		// executor calls `substitute_redacted` after each real
		// `substitute` to produce a log line; if the redacted pass also
		// wrote to `vars`, it would overwrite each prior real value with
		// `***` (whenever `value` came from an `ENV.*` segment), breaking
		// subsequent `{{ VAR.x }}` lookups.
		if !redact_env {
			args.vars.lock().unwrap().insert(name, value);
		}
		return Ok(String::new());
	}

	let resolved: Vec<String> = call
		.args
		.iter()
		.map(|a| evaluate_arg(args, a, env, consumed, flag_keys, redact_env))
		.collect::<Result<Vec<_>, _>>()?;

	match call.name.as_str() {
		"to_upper" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(resolved[0].to_uppercase())
		}
		"to_lower" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(resolved[0].to_lowercase())
		}
		"base64_encode" => {
			expect_arity(&call.name, &resolved, 1)?;
			use base64::Engine;
			Ok(base64::engine::general_purpose::STANDARD.encode(resolved[0].as_bytes()))
		}
		"base64_decode" => {
			expect_arity(&call.name, &resolved, 1)?;
			use base64::Engine;
			let bytes = base64::engine::general_purpose::STANDARD
				.decode(resolved[0].as_bytes())
				.map_err(|e| SubstitutionError::InvalidBase64(e.to_string()))?;
			String::from_utf8(bytes).map_err(|_| SubstitutionError::NonUtf8Decoded)
		}
		"concat" => {
			if resolved.is_empty() {
				return Err(SubstitutionError::FunctionArity {
					name: "concat".into(),
					expected: "1 or more".into(),
					got: 0,
				});
			}
			Ok(resolved.join(""))
		}
		"join" => {
			// `join(sep, s1, s2, ...)` — first arg is the separator, the rest
			// are joined with it. Calling with just the separator (no items)
			// returns "" — matches Python's `",".join([])` and most other
			// languages' behaviour for joining an empty list.
			if resolved.is_empty() {
				return Err(SubstitutionError::FunctionArity {
					name: "join".into(),
					expected: "1 or more (separator, then 0+ items)".into(),
					got: 0,
				});
			}
			let (sep, items) = resolved.split_first().unwrap();
			Ok(items.join(sep.as_str()))
		}
		"replace_all" => {
			// `replace_all(haystack, needle, replacement)` — every occurrence
			// of `needle` in `haystack` becomes `replacement`. Defers to
			// Rust's `str::replace`, which means an empty `needle` produces
			// "X" between every char (`replace_all("abc", "", "X")` →
			// `"XaXbXcX"`). That's a footgun if you weren't expecting it,
			// but matches Rust stdlib semantics; document and move on.
			expect_arity(&call.name, &resolved, 3)?;
			Ok(resolved[0].replace(resolved[1].as_str(), resolved[2].as_str()))
		}
		"remove_all" => {
			// `remove_all(haystack, needle)` — strip every occurrence of
			// `needle` from `haystack`. Sugar for `replace_all(s, n, '')`.
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0].replace(resolved[1].as_str(), ""))
		}
		"starts_with" => {
			// Returns the literal string `"true"` or `"false"` so the result
			// works seamlessly as a `Truthy` value inside DSL expressions
			// (`if: "{{ starts_with(ARG.path, '/usr') }}"`).
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0].starts_with(resolved[1].as_str()).to_string())
		}
		"ends_with" => {
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0].ends_with(resolved[1].as_str()).to_string())
		}
		"contains" => {
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0].contains(resolved[1].as_str()).to_string())
		}
		"capitalize" => {
			// Uppercase the first character of every whitespace-separated
			// "word" (per `char::is_whitespace`), leaving the rest of each
			// word untouched. Matches the common "title case-ish" intuition
			// — `capitalize("hello world")` → `"Hello World"` — without
			// dropping or normalising existing internal capitals.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(capitalize_words(&resolved[0]))
		}
		"trim" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(resolved[0].trim().to_string())
		}
		"trim_start" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(resolved[0].trim_start().to_string())
		}
		"trim_end" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(resolved[0].trim_end().to_string())
		}
		"length" => {
			// Character count, NOT byte count — matches user intuition
			// (`length("héllo")` → 5, not 6). Counts Unicode scalar values
			// via `chars().count()`.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(resolved[0].chars().count().to_string())
		}
		"escape" => {
			// Replace control characters and double-quotes with their
			// backslash-escape equivalents — useful for embedding arbitrary
			// strings into log lines, JSON-ish payloads, or anywhere a
			// printable single-line representation is wanted. NOT a full
			// JSON escape (no `\uXXXX`); single quotes are passed through
			// unchanged because Runfile syntax leans on `'...'` heavily.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(escape_string(&resolved[0]))
		}
		"repeat" => {
			// `repeat(s, n)` — repeat `s` exactly `n` times. `n` must parse
			// as a non-negative integer; `repeat(s, 0)` returns the empty
			// string.
			expect_arity(&call.name, &resolved, 2)?;
			let count = parse_count(&call.name, "count", &resolved[1])?;
			// Bound the allocation: an arg-supplied `count`
			// (`repeat('x', ARG.n)` with `--n 99999999999`) would otherwise
			// panic in `str::repeat` ("capacity overflow") or exhaust memory.
			const MAX_REPEAT_BYTES: usize = 8 * 1024 * 1024; // 8 MiB
			if resolved[0]
				.len()
				.checked_mul(count)
				.is_none_or(|n| n > MAX_REPEAT_BYTES)
			{
				return Err(SubstitutionError::InvalidNumber {
					name: call.name.clone(),
					message: format!(
						"repeat would produce more than {MAX_REPEAT_BYTES} bytes ({} × {count})",
						resolved[0].len()
					),
				});
			}
			Ok(resolved[0].repeat(count))
		}
		"regex_replace" => {
			// `regex_replace(haystack, pattern, replacement)` — every match
			// of the regex `pattern` in `haystack` becomes `replacement`.
			// Supports the regex crate's standard `$1` / `${name}`
			// backreferences in the replacement string. Pattern compile
			// errors surface as [`SubstitutionError::InvalidRegex`].
			expect_arity(&call.name, &resolved, 3)?;
			let re = compile_regex(&call.name, &resolved[1])?;
			Ok(re.replace_all(&resolved[0], resolved[2].as_str()).to_string())
		}
		"regex_remove" => {
			// `regex_remove(haystack, pattern)` — strip every regex match
			// from `haystack`. Sugar for `regex_replace(s, p, '')`.
			expect_arity(&call.name, &resolved, 2)?;
			let re = compile_regex(&call.name, &resolved[1])?;
			Ok(re.replace_all(&resolved[0], "").to_string())
		}
		"regex_matches" => {
			// `regex_matches(haystack, pattern)` — `"true"` if the regex
			// finds any match in `haystack`, else `"false"`. Pattern is
			// unanchored; use `^...$` for full-string match.
			expect_arity(&call.name, &resolved, 2)?;
			let re = compile_regex(&call.name, &resolved[1])?;
			Ok(re.is_match(&resolved[0]).to_string())
		}
		"regex_capture" => {
			// `regex_capture(haystack, pattern, group)` — find the first
			// match of `pattern` in `haystack` and return the substring
			// captured by group `group` (decimal index; `0` is the whole
			// match). No match, or a valid group index that didn't
			// participate in the match, both return `""` — same out-of-bounds
			// convention as `nth`. Bad regex → `InvalidRegex`; non-numeric
			// group index → `InvalidNumber`.
			expect_arity(&call.name, &resolved, 3)?;
			let re = compile_regex(&call.name, &resolved[1])?;
			let group = parse_count(&call.name, "group", &resolved[2])?;
			let captured = re
				.captures(&resolved[0])
				.and_then(|caps| caps.get(group))
				.map(|m| m.as_str().to_string())
				.unwrap_or_default();
			Ok(captured)
		}
		"regex_capture_all" => {
			// `regex_capture_all(haystack, pattern, group, separator)` — the
			// "all matches" variant of `regex_capture`. Find EVERY match of
			// `pattern` in `haystack`, pull the substring captured by group
			// `group` (decimal index; `0` is the whole match) from each, and
			// join the results with `separator`. A match where the requested
			// group didn't participate contributes `""` (same per-match
			// out-of-bounds convention as `regex_capture`), so entry count
			// stays aligned with match count. No matches → `""`. Pattern is
			// unanchored. Bad regex → `InvalidRegex`; non-numeric group index
			// → `InvalidNumber`.
			expect_arity(&call.name, &resolved, 4)?;
			let re = compile_regex(&call.name, &resolved[1])?;
			let group = parse_count(&call.name, "group", &resolved[2])?;
			let parts: Vec<&str> = re
				.captures_iter(&resolved[0])
				.map(|caps| caps.get(group).map(|m| m.as_str()).unwrap_or(""))
				.collect();
			Ok(parts.join(&resolved[3]))
		}
		// ── split accessors (string + separator → string; no list type involved) ──
		"nth" => {
			// `nth(s, sep, i)` — split `s` by `sep` and return the `i`-th
			// part (0-based). Out-of-bounds returns `""`; non-numeric or
			// negative indices error as [`SubstitutionError::InvalidNumber`].
			// Follows Rust's [`str::split`] semantics — including the
			// empty-`sep` edge case (`nth("abc", "", 0)` → `""` because the
			// split yields `["", "a", "b", "c", ""]`). Pair with
			// `count_parts(s, sep)` to bound-check before indexing if you
			// need to distinguish a missing slot from a legitimately empty
			// part.
			expect_arity(&call.name, &resolved, 3)?;
			let index = parse_count(&call.name, "index", &resolved[2])?;
			Ok(resolved[0]
				.split(resolved[1].as_str())
				.nth(index)
				.unwrap_or("")
				.to_string())
		}
		"first" => {
			// `first(s, sep)` — first part of `s.split(sep)`. Equivalent to
			// `nth(s, sep, 0)`; the first element always exists (`"".split(",")`
			// → `[""]`), so this only returns `""` when the input itself starts
			// with the separator (or is empty).
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0].split(resolved[1].as_str()).next().unwrap_or("").to_string())
		}
		"last" => {
			// `last(s, sep)` — last part of `s.split(sep)`. Useful for
			// "basename"-style extraction (`last(ARG.path, '/')`) when the
			// part count is unknown.
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0]
				.rsplit(resolved[1].as_str())
				.next()
				.unwrap_or("")
				.to_string())
		}
		"count_parts" => {
			// `count_parts(s, sep)` — number of parts that `s.split(sep)`
			// would yield, returned as a decimal string. Always >= 1
			// (Rust split semantics: `"".split(",")` → `[""]`, count 1).
			// Pair with `nth` to bound-check before indexing.
			expect_arity(&call.name, &resolved, 2)?;
			Ok(resolved[0].split(resolved[1].as_str()).count().to_string())
		}
		"set_cwd" => {
			// `set_cwd(path)` — change the cwd that subsequent shell commands
			// in this target spawn into. Behaves like `cd` but works on every
			// shell / OS without a forked process: stores the path on
			// `RunArgs.cwd_override` and the executor reads it at every
			// `Command::current_dir` site.
			//
			// Resolution semantics (mirroring shell `cd`):
			// - Absolute path → replaces the current override entirely.
			// - Relative path → joined onto the existing override (or, if no
			//   override is set yet, joined onto the target's resolved
			//   `workingDirectory` at spawn time).
			//
			// Returns `""` so a command line whose only content is
			// `{{ set_cwd(...) }}` resolves to a whitespace-only string and
			// is dropped by `execute_one_shell` / `collect_leaves_parallel*`
			// without dispatching a shell. Side effect-only function, just
			// like `define`.
			//
			// Skipped on the redacted-logging pass so the override isn't
			// written twice (once for the real substitute, once for the log
			// substitute) — same pattern as `define`.
			expect_arity(&call.name, &resolved, 1)?;
			if !redact_env {
				let new_path = std::path::PathBuf::from(&resolved[0]);
				let mut override_lock = args.cwd_override.lock().unwrap();
				*override_lock = if new_path.is_absolute() {
					Some(new_path)
				} else {
					match override_lock.as_ref() {
						Some(prev) => Some(prev.join(&new_path)),
						None => Some(new_path),
					}
				};
			}
			Ok(String::new())
		}
		"sha256" => {
			// `sha256(s)` — hex-encoded SHA-256 of the UTF-8 bytes of `s`.
			// Useful for cache-key generation in build targets and content
			// hashing in dependency-trigger scripts.
			expect_arity(&call.name, &resolved, 1)?;
			use sha2::{Digest, Sha256};
			let mut hasher = Sha256::new();
			hasher.update(resolved[0].as_bytes());
			Ok(hex::encode(hasher.finalize()))
		}
		"md5" => {
			// `md5(s)` — hex-encoded MD5 of the UTF-8 bytes of `s`. NOT
			// cryptographically secure; offered for compatibility with tools
			// that use it as a content fingerprint (e.g. `etag`-style cache
			// busting). Use `sha256` for any security-sensitive purpose.
			expect_arity(&call.name, &resolved, 1)?;
			use md5::{Digest, Md5};
			let mut hasher = Md5::new();
			hasher.update(resolved[0].as_bytes());
			Ok(hex::encode(hasher.finalize()))
		}
		"read_file" => {
			// `read_file(path)` — read the file at `path` and return its
			// contents as a UTF-8 string. Relative paths resolve against the
			// currently-executing target's source Runfile directory
			// (`{{ RUN.parent }}` — same anchor as `envFiles`); absolute paths
			// are used verbatim. Errors out cleanly on missing files / bad
			// UTF-8 — pair with `try(...)` to recover (`try(read_file(...))`).
			expect_arity(&call.name, &resolved, 1)?;
			// On the redacted logging pass, return a placeholder instead of the
			// file contents — a `read_file` result is frequently a secret
			// (`echo {{ read_file('.env') }}`) and log redaction otherwise only
			// masks `ENV.*`, leaking file contents verbatim into `--logging`
			// output. The real substitution pass still reads normally.
			if redact_env {
				return Ok(format!("<read_file: '{}'>", resolved[0]));
			}
			let resolved_path = resolve_substitution_path(args, &resolved[0]);
			std::fs::read_to_string(&resolved_path)
				.map_err(|e| SubstitutionError::ReadFileError(resolved[0].clone(), e.to_string()))
		}
		"write_file" => {
			// `write_file(path, content)` — write `content` to `path`, truncating
			// or creating the file as needed. The companion to `read_file`:
			// relative paths resolve against `{{ RUN.parent }}` and absolute
			// paths are used verbatim. Returns `""` so a command line whose only
			// content is `{{ write_file(...) }}` is dropped by the
			// empty-command-skip path without dispatching a shell — same
			// convention as `define` / `set_cwd`.
			//
			// This is the recommended way to overwrite arbitrary file content
			// from a target. The naive `printf %s {{ shell_quote(...) }} > file`
			// pattern works only for small payloads on Windows: when Rust spawns
			// `sh.exe -c <command>` via `CreateProcessW`, MSYS's argv-
			// reconstruction logic mishandles command lines past ~5KB, so the
			// pipe/redirect operators effectively get lost and stdout leaks to
			// the terminal. Going straight through Rust's `std::fs::write`
			// sidesteps the shell entirely.
			//
			// Skipped on `args.dry_run` (no write happens; placeholder return)
			// and on the redacted-logging pass (`redact_env`) so a target that
			// writes the same file from a single substitution doesn't end up
			// writing twice.
			expect_arity(&call.name, &resolved, 2)?;
			if args.dry_run || redact_env {
				return Ok(String::new());
			}
			let resolved_path = resolve_substitution_path(args, &resolved[0]);
			std::fs::write(&resolved_path, resolved[1].as_bytes())
				.map_err(|e| SubstitutionError::WriteFileError(resolved[0].clone(), e.to_string()))?;
			Ok(String::new())
		}
		"file_exists" => {
			// `file_exists(path)` — `"true"` if a filesystem entry exists at
			// `path` (file, directory, or symlink), else `"false"`. Same path
			// resolution rules as `read_file`. Returns "false" on permission
			// errors rather than failing — use `try(read_file(path))` if you
			// need to distinguish "missing" from "unreadable".
			expect_arity(&call.name, &resolved, 1)?;
			let resolved_path = resolve_substitution_path(args, &resolved[0]);
			Ok(resolved_path.try_exists().unwrap_or(false).to_string())
		}
		"json_get" => {
			// `json_get(json, path)` — parse `json` and extract the value at
			// the dotted `path`. Path syntax: dot-separated keys, with numeric
			// segments treated as array indices (so `users.0.name` reads index
			// 0's `name`). An empty path returns the top-level value as-is.
			//
			// Returns the *string* form of the located value:
			//   - JSON strings → the raw string (no quoting)
			//   - JSON numbers / booleans / null → their canonical text repr
			//   - JSON objects / arrays → compact JSON serialization
			//
			// A missing path segment returns "" (use `try(...)` if you need
			// to distinguish "absent" from "explicitly empty"). Hard errors
			// only on malformed JSON or malformed path syntax.
			expect_arity(&call.name, &resolved, 2)?;
			let value: serde_json::Value = serde_json::from_str(&resolved[0])
				.map_err(|e| SubstitutionError::InvalidJson("json_get".into(), e.to_string()))?;
			let segments = parse_json_path(&resolved[1])?;
			Ok(json_get_value(&value, &segments)
				.map(json_value_to_string)
				.unwrap_or_default())
		}
		"json_set" => {
			// `json_set(json, path, value)` — parse `json`, set the dotted
			// `path` to `value` (interpreted as JSON if it parses, else as a
			// string), and return the modified JSON as a compact string.
			// Intermediate object/array containers are created on demand:
			// numeric path segments create arrays (extending with `null` to
			// reach the index), non-numeric segments create objects.
			//
			// To insert a literal string that *looks* like JSON (e.g.
			// `"true"` as the four-character string), wrap it explicitly:
			// `json_set(j, 'k', '\"true\"')`. The "value parses as JSON" rule
			// covers the common case of injecting numbers, booleans, arrays,
			// or nested objects without an extra escape dance.
			expect_arity(&call.name, &resolved, 3)?;
			let mut value: serde_json::Value = serde_json::from_str(&resolved[0])
				.map_err(|e| SubstitutionError::InvalidJson("json_set".into(), e.to_string()))?;
			let segments = parse_json_path(&resolved[1])?;
			let new_value: serde_json::Value =
				serde_json::from_str(&resolved[2]).unwrap_or_else(|_| serde_json::Value::String(resolved[2].clone()));
			json_set_value(&mut value, &segments, new_value)?;
			Ok(serde_json::to_string(&value).unwrap_or_default())
		}
		"shell_quote" => {
			// `shell_quote(s)` — produce a quoted form of `s` that the
			// currently-active shell will parse as a single argument with
			// the original byte sequence. Lets users safely inline arbitrary
			// values (decoded JSON, paths with spaces, multi-line text, etc.)
			// into shell commands without the env-var-indirection dance.
			//
			// Quoting strategy is shell-aware via `RUN.shell`. Each form uses
			// the shell's "literal string" syntax where the only character
			// needing escape is the delimiting quote itself. Inside that
			// literal: newlines, backslashes, dollar signs, double quotes,
			// etc. all pass through verbatim with no further interpretation.
			//
			// Caveat for `cmd.exe`: arbitrary binary content (especially
			// embedded newlines and `%`) is fundamentally hard to pass via
			// command line. We do best-effort `""`-escape, but if you need
			// rock-solid safety on cmd, prefer the env-var indirection
			// pattern (`{{ ENV.foo }}` set via the `env` block, then
			// `%foo%` in the command) over `shell_quote`.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(quote_for_shell(&resolved[0], args.run_context.shell.as_str()))
		}
		"capture" => {
			// `capture(shell_cmd)` — run `shell_cmd` through the platform's
			// default shell (sh on Unix, cmd.exe on Windows — same convention
			// as `for shell:` iterators) and return its stdout with the
			// trailing newline stripped. Non-zero exit surfaces as
			// `CaptureFailed`.
			//
			// During `--dry-run`, the side effect is suppressed: the call
			// resolves to a readable placeholder `<capture: '<resolved-cmd>'>`
			// instead of spawning a process. This keeps dry-run pure
			// (matching `for shell:`'s placeholder rule) — the user can still
			// preview every other resolved template without arbitrary
			// commands firing.
			//
			// Results are memoized in `args.capture_cache` keyed by the
			// resolved command string. This means (a) the real and redacted
			// substitution passes don't double-execute the shell command,
			// and (b) repeated `capture('expensive-cmd')` calls within a
			// target run the command exactly once.
			expect_arity(&call.name, &resolved, 1)?;
			if args.dry_run {
				return Ok(format!("<capture: '{}'>", resolved[0]));
			}
			// On the redacted logging pass, NEVER spawn the command. The real
			// pass has already executed it; re-running here would (a) execute
			// the shell command a second time and (b) — because ENV.* refs in
			// the command are redacted to `***` on this pass — run a DIFFERENT
			// command whose args are `***`, and a non-zero exit would abort the
			// target after the real command already succeeded. Returning the
			// placeholder also keeps captured output (potentially a secret) out
			// of logs. Mirrors the `redact_env` guard on `write_file` / `define`.
			if redact_env {
				return Ok(format!("<capture: '{}'>", resolved[0]));
			}
			{
				let cache = args.capture_cache.lock().unwrap();
				if let Some(cached) = cache.get(&resolved[0]) {
					return Ok(cached.clone());
				}
			}
			let value = run_capture(&resolved[0])?;
			args.capture_cache
				.lock()
				.unwrap()
				.insert(resolved[0].clone(), value.clone());
			Ok(value)
		}
		"one_of" => {
			// `one_of(value, opt1, opt2, ...)` — validates that `value`
			// equals one of the listed options (string equality). Returns
			// `value` on success; on mismatch, surfaces a clear
			// `OneOfNoMatch` error listing every valid option so the user
			// knows what to pass.
			//
			// Designed for the "validate a positional arg / variable
			// against a fixed allow-list" pattern that otherwise needs a
			// four-case `match` block whose only purpose is rebinding the
			// same value back to itself.
			if resolved.len() < 2 {
				return Err(SubstitutionError::FunctionArity {
					name: "one_of".into(),
					expected: "2 or more (value, then 1+ allowed options)".into(),
					got: resolved.len(),
				});
			}
			let (value, options) = resolved.split_first().unwrap();
			if options.iter().any(|opt| opt == value) {
				Ok(value.clone())
			} else {
				let formatted = options
					.iter()
					.map(|o| format!("\"{}\"", o))
					.collect::<Vec<_>>()
					.join(", ");
				Err(SubstitutionError::OneOfNoMatch {
					value: value.clone(),
					options: formatted,
				})
			}
		}
		"add" => arithmetic_variadic(&call.name, &resolved, |acc, n| acc + n),
		"subtract" => arithmetic_variadic(&call.name, &resolved, |acc, n| acc - n),
		"multiply" => arithmetic_variadic(&call.name, &resolved, |acc, n| acc * n),
		"divide" => {
			// `divide(a, b, ...)` — left-fold division. Divisor of zero
			// at any step surfaces as `DivideByZero`. Other numeric rules
			// (string coercion, integer-when-whole formatting) match the
			// rest of the arithmetic family.
			if resolved.len() < 2 {
				return Err(SubstitutionError::FunctionArity {
					name: "divide".into(),
					expected: "2 or more".into(),
					got: resolved.len(),
				});
			}
			let mut iter = resolved.iter();
			let mut acc = parse_numeric(&call.name, iter.next().unwrap())?;
			for next in iter {
				let n = parse_numeric(&call.name, next)?;
				if n == 0.0 {
					return Err(SubstitutionError::DivideByZero);
				}
				acc /= n;
			}
			Ok(format_number(acc))
		}
		"less_than" => numeric_compare(&call.name, &resolved, |a, b| a < b),
		"less_than_or_equal" => numeric_compare(&call.name, &resolved, |a, b| a <= b),
		"greater_than" => numeric_compare(&call.name, &resolved, |a, b| a > b),
		"greater_than_or_equal" => numeric_compare(&call.name, &resolved, |a, b| a >= b),
		"is_number" => {
			// Returns the literal string `"true"` or `"false"` so it works as a
			// `Truthy` value inside DSL expressions. Unlike the arithmetic family,
			// a non-numeric argument is NOT an error — that's the whole point.
			// "Number" follows the same rule as [`parse_numeric`]: a finite `f64`
			// (so `"inf"` / `"nan"` are not numbers).
			expect_arity(&call.name, &resolved, 1)?;
			let is_num = resolved[0]
				.trim()
				.parse::<f64>()
				.map(|n| n.is_finite())
				.unwrap_or(false);
			Ok(is_num.to_string())
		}
		// ── path helpers (std::path semantics: `/` everywhere, plus `\` on Windows) ──
		"basename" => {
			// Final path component (filename). `basename('a/b/c.txt')` → `c.txt`.
			// Trailing-slash / root paths with no filename → `""`.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(std::path::Path::new(&resolved[0])
				.file_name()
				.map(|s| s.to_string_lossy().into_owned())
				.unwrap_or_default())
		}
		"dirname" => {
			// Everything before the final component. `dirname('a/b/c.txt')` → `a/b`.
			// A bare filename → `""` (no parent).
			expect_arity(&call.name, &resolved, 1)?;
			Ok(std::path::Path::new(&resolved[0])
				.parent()
				.map(|s| s.to_string_lossy().into_owned())
				.unwrap_or_default())
		}
		"extname" => {
			// File extension WITHOUT the leading dot (matches Rust's
			// `Path::extension`). `extname('a/b.tar.gz')` → `gz`; no extension → `""`.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(std::path::Path::new(&resolved[0])
				.extension()
				.map(|s| s.to_string_lossy().into_owned())
				.unwrap_or_default())
		}
		"stem" => {
			// Filename without its final extension. `stem('a/b.tar.gz')` → `b.tar`.
			expect_arity(&call.name, &resolved, 1)?;
			Ok(std::path::Path::new(&resolved[0])
				.file_stem()
				.map(|s| s.to_string_lossy().into_owned())
				.unwrap_or_default())
		}
		"join_path" => {
			// Join path components using the platform separator. Absolute later
			// components replace earlier ones (Rust `PathBuf::push` semantics).
			if resolved.is_empty() {
				return Err(SubstitutionError::FunctionArity {
					name: "join_path".into(),
					expected: "1 or more".into(),
					got: 0,
				});
			}
			let mut p = std::path::PathBuf::from(&resolved[0]);
			for seg in &resolved[1..] {
				p.push(seg);
			}
			Ok(p.to_string_lossy().into_owned())
		}
		// ── date / time ──
		"now" => {
			// `now(format)` — current UTC time in a named format. See
			// `now_formatted` for the full list of valid format strings.
			expect_arity(&call.name, &resolved, 1)?;
			now_formatted(&resolved[0])
		}
		// ── numeric (extends add/subtract/multiply/divide + comparisons) ──
		"modulo" => {
			expect_arity(&call.name, &resolved, 2)?;
			let a = parse_numeric(&call.name, &resolved[0])?;
			let b = parse_numeric(&call.name, &resolved[1])?;
			if b == 0.0 {
				return Err(SubstitutionError::DivideByZero);
			}
			Ok(format_number(a % b))
		}
		"power" => {
			expect_arity(&call.name, &resolved, 2)?;
			let a = parse_numeric(&call.name, &resolved[0])?;
			let b = parse_numeric(&call.name, &resolved[1])?;
			let r = a.powf(b);
			if !r.is_finite() {
				return Err(SubstitutionError::InvalidNumeric {
					name: "power".into(),
					message: "result is not a finite number".into(),
				});
			}
			Ok(format_number(r))
		}
		"min" => numeric_reduce(&call.name, &resolved, f64::min),
		"max" => numeric_reduce(&call.name, &resolved, f64::max),
		"abs" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(format_number(parse_numeric(&call.name, &resolved[0])?.abs()))
		}
		"round" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(format_number(parse_numeric(&call.name, &resolved[0])?.round()))
		}
		"floor" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(format_number(parse_numeric(&call.name, &resolved[0])?.floor()))
		}
		"ceil" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(format_number(parse_numeric(&call.name, &resolved[0])?.ceil()))
		}
		// ── substring (char-indexed, optional length) ──
		"substring" => {
			// `substring(s, start)` → from `start` to end; `substring(s, start, len)`
			// → at most `len` chars from `start`. Indices are Unicode scalar
			// positions (like `length`); out-of-range start → `""`, len past the
			// end clamps. Negative/non-numeric → InvalidNumber.
			if resolved.len() != 2 && resolved.len() != 3 {
				return Err(SubstitutionError::FunctionArity {
					name: "substring".into(),
					expected: "2 or 3".into(),
					got: resolved.len(),
				});
			}
			let start = parse_count(&call.name, "start", &resolved[1])?;
			let chars = resolved[0].chars();
			let out: String = if resolved.len() == 3 {
				let len = parse_count(&call.name, "length", &resolved[2])?;
				chars.skip(start).take(len).collect()
			} else {
				chars.skip(start).collect()
			};
			Ok(out)
		}
		// ── uuid (v4-shaped; non-deterministic → dry-run placeholder) ──
		"uuid" => {
			expect_arity(&call.name, &resolved, 0)?;
			if args.dry_run {
				return Ok("<uuid>".to_string());
			}
			Ok(generate_uuid())
		}
		// ── temp_file / temp_dir (create OS temp artifacts; auto-removed at CLI exit) ──
		"temp_file" => {
			// `temp_file([content], [extension])` — create a fresh file in the
			// OS temp directory and return its absolute path. With one arg,
			// `content` is written into the file; with a second arg, `extension`
			// sets the file suffix (a leading dot is optional, so both `'json'`
			// and `'.json'` yield `runfile-<uuid>.json`). Every created file is
			// registered for deletion when the `runfile` CLI exits (see
			// [`cleanup_temp_artifacts`]).
			//
			// Non-deterministic and side-effecting, so it mirrors `uuid` /
			// `capture` / `write_file`: on `--dry-run` it returns the readable
			// placeholder `<temp_file>` and creates nothing, and on the
			// redacted-logging pass it ALSO skips creation (returning the
			// placeholder) so the back-to-back log substitution never spawns a
			// second, orphaned temp file. Because each real substitution pass
			// runs once per leaf, a bare inline `temp_file()` creates exactly one
			// file. The canonical pattern is to capture the path once and reuse
			// it (the log then shows the real path via `VAR`):
			//   "{{ define(cfg, temp_file()) }}"
			//   "echo '...' > {{ VAR.cfg }}"
			//   "tool --config {{ VAR.cfg }}"
			if resolved.len() > 2 {
				return Err(SubstitutionError::FunctionArity {
					name: "temp_file".into(),
					expected: "0, 1, or 2".into(),
					got: resolved.len(),
				});
			}
			if args.dry_run || redact_env {
				return Ok("<temp_file>".to_string());
			}
			let content = resolved.first().map(String::as_str).unwrap_or("");
			let ext = resolved
				.get(1)
				.map(|e| e.trim_start_matches('.'))
				.filter(|e| !e.is_empty());
			// Create the file atomically and EXCLUSIVELY (O_CREAT|O_EXCL via
			// `create_new`): in the world-writable OS temp dir this refuses to
			// follow a pre-planted symlink or to clobber/truncate an existing
			// file — the previous `std::fs::write` used O_CREAT|O_TRUNC, which
			// does both. On the (astronomically unlikely) name collision, retry
			// with a fresh uuid a bounded number of times.
			use std::io::Write as _;
			let dir = std::env::temp_dir();
			let mut last_err: Option<std::io::Error> = None;
			let mut created: Option<std::path::PathBuf> = None;
			for _ in 0..8 {
				let mut name = format!("runfile-{}", generate_uuid());
				if let Some(ext) = ext {
					name.push('.');
					name.push_str(ext);
				}
				let path = dir.join(&name);
				match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
					Ok(mut file) => {
						file.write_all(content.as_bytes())
							.map_err(|e| SubstitutionError::TempFileError(e.to_string()))?;
						created = Some(path);
						break;
					}
					Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
						last_err = Some(e);
						continue;
					}
					Err(e) => return Err(SubstitutionError::TempFileError(e.to_string())),
				}
			}
			let path = created.ok_or_else(|| {
				SubstitutionError::TempFileError(
					last_err
						.map(|e| e.to_string())
						.unwrap_or_else(|| "could not create a unique temp file".to_string()),
				)
			})?;
			register_temp_artifact(path.clone(), false);
			Ok(path.to_string_lossy().into_owned())
		}
		"temp_dir" => {
			// `temp_dir()` — create a fresh empty directory in the OS temp
			// directory and return its absolute path. Registered for recursive
			// deletion at CLI exit. Same dry-run / redacted-pass placeholder
			// behavior as `temp_file` (returns `<temp_dir>` without creating
			// anything).
			expect_arity(&call.name, &resolved, 0)?;
			if args.dry_run || redact_env {
				return Ok("<temp_dir>".to_string());
			}
			let path = std::env::temp_dir().join(format!("runfile-{}", generate_uuid()));
			std::fs::create_dir(&path).map_err(|e| SubstitutionError::TempDirError(e.to_string()))?;
			register_temp_artifact(path.clone(), true);
			Ok(path.to_string_lossy().into_owned())
		}
		// ── url percent-encoding (RFC 3986; space → %20, symmetric) ──
		"url_encode" => {
			expect_arity(&call.name, &resolved, 1)?;
			Ok(url_encode_str(&resolved[0]))
		}
		"url_decode" => {
			expect_arity(&call.name, &resolved, 1)?;
			url_decode_str(&resolved[0])
		}
		// ── error: fail the current command with a message (see executor) ──
		"error" => {
			// Prints `message` to stderr and fails the current command, but the
			// failure flows through the normal walker so `when: failure` /
			// `when: always` steps still run (subject to `ignoreErrors`). On
			// dry-run it's a no-op placeholder; on the redacted log pass it does
			// not re-print (the real pass already did).
			expect_arity(&call.name, &resolved, 1)?;
			if args.dry_run {
				return Ok(format!("<error: '{}'>", resolved[0]));
			}
			if !redact_env {
				eprintln!("{}", resolved[0]);
			}
			Err(SubstitutionError::UserError(resolved[0].clone()))
		}
		_ => Err(SubstitutionError::UnknownFunction(call.name.clone())),
	}
}

/// Numeric comparison helper for `less_than` / `less_than_or_equal` /
/// `greater_than` / `greater_than_or_equal`. Coerces both arguments through
/// [`parse_numeric`] (so non-numeric input is an error, same as the arithmetic
/// family) and returns the literal string `"true"` / `"false"` so the result
/// doubles as a `Truthy` DSL value.
fn numeric_compare<F: Fn(f64, f64) -> bool>(
	name: &str,
	resolved: &[String],
	op: F,
) -> Result<String, SubstitutionError> {
	expect_arity(name, resolved, 2)?;
	let a = parse_numeric(name, &resolved[0])?;
	let b = parse_numeric(name, &resolved[1])?;
	Ok(op(a, b).to_string())
}

/// Shared body for the left-folded variadic arithmetic functions
/// (`add` / `subtract` / `multiply`). Coerces every argument through
/// [`parse_numeric`] and formats the result via [`format_number`] so the
/// integer-when-whole rule applies uniformly.
fn arithmetic_variadic<F: Fn(f64, f64) -> f64>(
	name: &str,
	resolved: &[String],
	op: F,
) -> Result<String, SubstitutionError> {
	if resolved.len() < 2 {
		return Err(SubstitutionError::FunctionArity {
			name: name.to_string(),
			expected: "2 or more".into(),
			got: resolved.len(),
		});
	}
	let mut iter = resolved.iter();
	let mut acc = parse_numeric(name, iter.next().unwrap())?;
	for next in iter {
		let n = parse_numeric(name, next)?;
		acc = op(acc, n);
	}
	Ok(format_number(acc))
}

/// Parse a numeric argument for the arithmetic family (`add`, `subtract`,
/// `multiply`, `divide`). Accepts any string that round-trips through
/// `f64::from_str` — decimal integers, decimal fractions, and scientific
/// notation. Whitespace is trimmed. NaN and infinity (including the literal
/// `"inf"` / `"nan"` strings that `f64::from_str` accepts) are rejected so
/// downstream arithmetic always lands on a finite, formattable number.
fn parse_numeric(fn_name: &str, value: &str) -> Result<f64, SubstitutionError> {
	let trimmed = value.trim();
	let n: f64 = trimmed.parse().map_err(|_| SubstitutionError::InvalidNumeric {
		name: fn_name.to_string(),
		message: format!("argument `{}` is not a number", value),
	})?;
	if !n.is_finite() {
		return Err(SubstitutionError::InvalidNumeric {
			name: fn_name.to_string(),
			message: format!("argument `{}` resolved to a non-finite number", value),
		});
	}
	Ok(n)
}

/// Format the result of an arithmetic function. Returns an integer string
/// when the value has no fractional part and fits in `i64`; otherwise uses
/// `f64`'s shortest round-trip formatting (`{}`), which yields `"0.1"` for
/// 0.1, `"0.3333333333333333"` for `1/3`, etc.
///
/// The integer-when-whole rule matches user-stated expectations:
/// `add("5", "3") → "8"`, `add("5.5", "2.3", "1.2") → "9"`,
/// `add("5", "1.1") → "6.1"`. We funnel through `f64` for the math itself —
/// no separate integer code path — so accumulated rounding error is the
/// usual IEEE-754 risk; users mixing very large integers with floats should
/// expect float semantics.
fn format_number(value: f64) -> String {
	if value.fract() == 0.0 && value.abs() < (i64::MAX as f64) {
		// `as i64` saturates at the i64 boundary on out-of-range values;
		// the `abs < i64::MAX` guard above keeps us inside the safe range.
		// `-0.0` formats as `"0"` (since `(-0.0 as i64) == 0`), which is the
		// correct answer for `subtract("5", "5")`.
		(value as i64).to_string()
	} else {
		format!("{}", value)
	}
}

/// Shared body for the variadic reducers `min` / `max`. Coerces every argument
/// through [`parse_numeric`] and folds with `op`. Requires at least one arg.
fn numeric_reduce<F: Fn(f64, f64) -> f64>(name: &str, resolved: &[String], op: F) -> Result<String, SubstitutionError> {
	if resolved.is_empty() {
		return Err(SubstitutionError::FunctionArity {
			name: name.to_string(),
			expected: "1 or more".into(),
			got: 0,
		});
	}
	let mut iter = resolved.iter();
	let mut acc = parse_numeric(name, iter.next().unwrap())?;
	for next in iter {
		acc = op(acc, parse_numeric(name, next)?);
	}
	Ok(format_number(acc))
}

/// Resolve `now(format)` to the current UTC time in the requested format.
///
/// Supported formats: `unix` / `unix-timestamp` (seconds), `unix-ms` /
/// `unix-millis` (milliseconds), `iso` / `iso-8601` / `rfc3339`
/// (`YYYY-MM-DDTHH:MM:SSZ`), `iso-date` / `date` (`YYYY-MM-DD`), `iso-time` /
/// `time` (`HH:MM:SS`), and the individual components `year` / `month` / `day`
/// / `hour` / `minute` / `second`. Unknown formats error.
fn now_formatted(format: &str) -> Result<String, SubstitutionError> {
	use std::time::{SystemTime, UNIX_EPOCH};
	let dur = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map_err(|_| SubstitutionError::InvalidTimeFormat("system clock is before the Unix epoch".into()))?;
	match format {
		"unix" | "unix-timestamp" => return Ok(dur.as_secs().to_string()),
		"unix-ms" | "unix-millis" => return Ok(dur.as_millis().to_string()),
		_ => {}
	}
	let (y, mo, d, h, mi, s) = civil_parts(dur.as_secs() as i64);
	match format {
		"iso" | "iso-8601" | "rfc3339" => Ok(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")),
		"iso-date" | "date" => Ok(format!("{y:04}-{mo:02}-{d:02}")),
		"iso-time" | "time" => Ok(format!("{h:02}:{mi:02}:{s:02}")),
		"year" => Ok(format!("{y:04}")),
		"month" => Ok(format!("{mo:02}")),
		"day" => Ok(format!("{d:02}")),
		"hour" => Ok(format!("{h:02}")),
		"minute" => Ok(format!("{mi:02}")),
		"second" => Ok(format!("{s:02}")),
		other => Err(SubstitutionError::InvalidTimeFormat(other.to_string())),
	}
}

/// Split a Unix timestamp (seconds, UTC) into `(year, month, day, hour, minute,
/// second)`. Date conversion uses Howard Hinnant's `civil_from_days` algorithm
/// so we stay dependency-free (no `chrono` / `time`).
fn civil_parts(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
	let days = secs.div_euclid(86_400);
	let tod = secs.rem_euclid(86_400);
	let z = days + 719_468;
	let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
	let doe = z - era * 146_097; // [0, 146096]
	let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
	let year = yoe + era * 400;
	let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
	let mp = (5 * doy + 2) / 153; // [0, 11]
	let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
	let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
	let year = year + if month <= 2 { 1 } else { 0 };
	let h = (tod / 3600) as u32;
	let mi = ((tod % 3600) / 60) as u32;
	let s = (tod % 60) as u32;
	(year, month, day, h, mi, s)
}

/// Generate a version-4-shaped UUID string. Randomness comes from a SplitMix64
/// PRNG seeded from the wall clock, process id, and a per-process counter — not
/// cryptographically strong, but unique enough for temp names / cache keys
/// (which is the intended use). Dependency-free (no `uuid` / `rand` crate).
fn generate_uuid() -> String {
	use std::sync::atomic::{AtomicU64, Ordering};
	use std::time::{SystemTime, UNIX_EPOCH};
	static COUNTER: AtomicU64 = AtomicU64::new(0);
	let nanos = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_nanos() as u64)
		.unwrap_or(0);
	let pid = std::process::id() as u64;
	let count = COUNTER.fetch_add(1, Ordering::Relaxed);
	let mut state = nanos ^ pid.wrapping_shl(32) ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
	let mut next = || {
		state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
		let mut z = state;
		z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
		z ^ (z >> 31)
	};
	let mut bytes = [0u8; 16];
	bytes[..8].copy_from_slice(&next().to_le_bytes());
	bytes[8..].copy_from_slice(&next().to_le_bytes());
	bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
	bytes[8] = (bytes[8] & 0x3F) | 0x80; // variant 1
	let h = |b: u8| format!("{b:02x}");
	format!(
		"{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}",
		h(bytes[0]),
		h(bytes[1]),
		h(bytes[2]),
		h(bytes[3]),
		h(bytes[4]),
		h(bytes[5]),
		h(bytes[6]),
		h(bytes[7]),
		h(bytes[8]),
		h(bytes[9]),
		h(bytes[10]),
		h(bytes[11]),
		h(bytes[12]),
		h(bytes[13]),
		h(bytes[14]),
		h(bytes[15]),
	)
}

/// Percent-encode `s` per RFC 3986: unreserved characters (`A-Za-z0-9-_.~`)
/// pass through; every other byte becomes `%XX` (uppercase hex). Spaces become
/// `%20` (not `+`), so `url_decode(url_encode(s)) == s`.
fn url_encode_str(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for &b in s.as_bytes() {
		match b {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
			_ => out.push_str(&format!("%{b:02X}")),
		}
	}
	out
}

/// Decode percent-escapes (`%XX`) in `s`. `+` is left literal (symmetric with
/// [`url_encode_str`], which emits `%20` for spaces). Malformed escapes error
/// as `InvalidUrlEncoding`; non-UTF-8 results as `NonUtf8Decoded`.
fn url_decode_str(s: &str) -> Result<String, SubstitutionError> {
	let bytes = s.as_bytes();
	let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'%' {
			let hi = bytes.get(i + 1).copied().and_then(hex_val);
			let lo = bytes.get(i + 2).copied().and_then(hex_val);
			match (hi, lo) {
				(Some(hi), Some(lo)) => {
					out.push(hi * 16 + lo);
					i += 3;
				}
				_ => return Err(SubstitutionError::InvalidUrlEncoding(s.to_string())),
			}
		} else {
			out.push(bytes[i]);
			i += 1;
		}
	}
	String::from_utf8(out).map_err(|_| SubstitutionError::NonUtf8Decoded)
}

fn hex_val(b: u8) -> Option<u8> {
	match b {
		b'0'..=b'9' => Some(b - b'0'),
		b'a'..=b'f' => Some(b - b'a' + 10),
		b'A'..=b'F' => Some(b - b'A' + 10),
		_ => None,
	}
}

/// Run a shell command via the platform's default shell and capture its
/// stdout. Mirrors the shell convention used by `for shell:` iterators —
/// `cmd /C` on Windows, `sh -c` everywhere else — so the user's intuition
/// transfers between the two surfaces.
///
/// Trailing `\n` (or `\r\n`) is stripped from stdout (single trailing newline
/// only — internal newlines pass through). Non-zero exit, spawn failure, or
/// non-UTF-8 stdout all surface as [`SubstitutionError::CaptureFailed`] with
/// the resolved command string embedded for debugability.
///
/// Skipped on the dry-run pass — the caller short-circuits to a placeholder
/// before reaching this helper.
fn run_capture(command: &str) -> Result<String, SubstitutionError> {
	use std::process::Command;
	#[cfg(windows)]
	let output = Command::new("cmd").arg("/C").arg(command).output();
	#[cfg(not(windows))]
	let output = Command::new("sh").arg("-c").arg(command).output();

	let output = output.map_err(|e| SubstitutionError::CaptureFailed {
		command: command.to_string(),
		message: format!("failed to spawn shell: {}", e),
	})?;
	if !output.status.success() {
		let code = output
			.status
			.code()
			.map(|c| c.to_string())
			.unwrap_or_else(|| "signal".into());
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		let detail = if stderr.is_empty() {
			format!("shell exited with code {}", code)
		} else {
			format!("shell exited with code {}: {}", code, stderr)
		};
		return Err(SubstitutionError::CaptureFailed {
			command: command.to_string(),
			message: detail,
		});
	}
	let stdout = String::from_utf8(output.stdout).map_err(|_| SubstitutionError::CaptureFailed {
		command: command.to_string(),
		message: "stdout was not valid UTF-8".into(),
	})?;
	// Strip a single trailing newline (LF or CRLF) — the common case is
	// `capture('git rev-parse HEAD')` returning a SHA followed by a stray
	// newline, and users almost never want that newline in interpolation.
	let trimmed = if let Some(s) = stdout.strip_suffix("\r\n") {
		s
	} else if let Some(s) = stdout.strip_suffix('\n') {
		s
	} else {
		&stdout
	};
	Ok(trimmed.to_string())
}

/// One temp artifact created by `temp_file` / `temp_dir`, tracked for deletion
/// at CLI exit.
struct TempArtifact {
	path: PathBuf,
	is_dir: bool,
}

/// Process-global registry of temp files/dirs created via `temp_file` /
/// `temp_dir`, drained and deleted by [`cleanup_temp_artifacts`].
///
/// A process-global static (rather than per-`RunArgs` state) for two reasons:
/// (1) the CLI tears down via `std::process::exit`, which skips destructors, so
/// cleanup is an explicit call the CLI makes before every exit — a global is the
/// simplest thing it can reach without threading the registry back out of the
/// executor; (2) `temp_file` runs deep inside substitution, far from any one
/// `RunArgs` lifetime, and the same registry must cover artifacts created by
/// `@target` children, parallel worker threads, and watch-mode re-runs alike.
/// `Mutex::new` / `Vec::new` are both `const`, so the static needs no lazy init.
static TEMP_ARTIFACTS: std::sync::Mutex<Vec<TempArtifact>> = std::sync::Mutex::new(Vec::new());

/// Record a freshly-created temp artifact for deletion at CLI exit.
fn register_temp_artifact(path: PathBuf, is_dir: bool) {
	if let Ok(mut guard) = TEMP_ARTIFACTS.lock() {
		guard.push(TempArtifact { path, is_dir });
	}
}

/// Delete every temp file/dir created by `temp_file` / `temp_dir` so far and
/// clear the registry. Best-effort: a missing/already-deleted path or a
/// permission error is silently ignored. The CLI calls this before each
/// `process::exit` and between watch-mode iterations.
///
/// Note: a hard kill (SIGKILL, power loss) skips this entirely — those leftover
/// artifacts stay in the OS temp directory for the OS to reclaim. SIGINT
/// (Ctrl+C) likewise terminates before cleanup unless the run completes
/// normally, so long-running interrupted targets may leave artifacts behind.
pub fn cleanup_temp_artifacts() {
	if let Ok(mut guard) = TEMP_ARTIFACTS.lock() {
		for artifact in guard.drain(..) {
			if artifact.is_dir {
				let _ = std::fs::remove_dir_all(&artifact.path);
			} else {
				let _ = std::fs::remove_file(&artifact.path);
			}
		}
	}
}

/// Resolve a path argument passed to `read_file` / `file_exists`. Absolute
/// paths are used verbatim; relative paths are joined onto the
/// currently-executing target's source Runfile directory (`{{ RUN.parent }}`).
/// This matches the anchor used for `envFiles` and the default
/// `workingDirectory` — relative reads "just work" alongside the Runfile.
///
/// When `RUN.parent` is empty (e.g. unit tests that don't go through the
/// runner), the path is used as-is and the OS resolves it against the
/// process cwd.
fn resolve_substitution_path(args: &RunArgs, path: &str) -> PathBuf {
	let p = PathBuf::from(path);
	if p.is_absolute() {
		return p;
	}
	let parent = args.run_context.parent.as_str();
	if parent.is_empty() {
		p
	} else {
		PathBuf::from(parent).join(p)
	}
}

/// One segment of a parsed JSON path: an object key or an array index.
/// `parse_json_path` produces a `Vec<JsonPathSegment>` from a dotted-path
/// expression like `users.0.name`.
#[derive(Debug, Clone)]
enum JsonPathSegment {
	Key(String),
	Index(usize),
}

/// Parse a dotted JSON path: segments are separated by `.` and a numeric
/// segment is treated as an array index (segments are tried as numbers first,
/// then as object keys). An empty path returns an empty `Vec` — callers treat
/// that as "the whole document". Empty segments (`a..b`) are a hard error.
fn parse_json_path(path: &str) -> Result<Vec<JsonPathSegment>, SubstitutionError> {
	if path.is_empty() {
		return Ok(Vec::new());
	}
	let mut out = Vec::new();
	for seg in path.split('.') {
		if seg.is_empty() {
			return Err(SubstitutionError::InvalidJsonPath(
				path.to_string(),
				"empty segment (consecutive `.` or leading/trailing `.`)".to_string(),
			));
		}
		match seg.parse::<usize>() {
			Ok(n) => out.push(JsonPathSegment::Index(n)),
			Err(_) => out.push(JsonPathSegment::Key(seg.to_string())),
		}
	}
	Ok(out)
}

/// Walk `value` along `path`, returning the located node or `None` if any
/// segment misses (wrong type for the segment, missing key, out-of-bounds
/// index). An empty `path` returns the root.
fn json_get_value<'a>(value: &'a serde_json::Value, path: &[JsonPathSegment]) -> Option<&'a serde_json::Value> {
	let mut cur = value;
	for seg in path {
		cur = match (cur, seg) {
			(serde_json::Value::Object(map), JsonPathSegment::Key(k)) => map.get(k)?,
			(serde_json::Value::Array(arr), JsonPathSegment::Index(i)) => arr.get(*i)?,
			// Numeric path segment against an object or non-numeric against an
			// array: try the alternate interpretation as a fallback so
			// `users.0` works even if `0` happens to be an object key.
			(serde_json::Value::Object(map), JsonPathSegment::Index(i)) => map.get(&i.to_string())?,
			_ => return None,
		};
	}
	Some(cur)
}

/// Set `value` at `path` to `new_value`, creating intermediate containers as
/// needed. Numeric segments create arrays (extended with `null` to reach the
/// index); non-numeric segments create objects. Existing nodes of the wrong
/// type are replaced — `json_set` is destructive on container-type
/// mismatches, mirroring `jq`'s assignment semantics.
/// Largest array index `json_set` will materialize. A numeric path segment
/// creates/extends an array with `null`s up to the index, so an arg-supplied
/// index (`json_set(j, ARG.idx, v)` with `--idx 999999999`) would otherwise
/// allocate ~1e9 elements and exhaust memory.
const MAX_JSON_SET_INDEX: usize = 1_000_000;

fn json_set_value(
	value: &mut serde_json::Value,
	path: &[JsonPathSegment],
	new_value: serde_json::Value,
) -> Result<(), SubstitutionError> {
	if path.is_empty() {
		*value = new_value;
		return Ok(());
	}
	let (head, rest) = path.split_first().unwrap();
	match head {
		JsonPathSegment::Key(k) => {
			if !value.is_object() {
				*value = serde_json::Value::Object(serde_json::Map::new());
			}
			let map = value.as_object_mut().unwrap();
			let entry = map.entry(k.clone()).or_insert(serde_json::Value::Null);
			json_set_value(entry, rest, new_value)
		}
		JsonPathSegment::Index(i) => {
			if *i > MAX_JSON_SET_INDEX {
				return Err(SubstitutionError::InvalidJsonPath(
					i.to_string(),
					format!("array index {i} exceeds the maximum of {MAX_JSON_SET_INDEX}"),
				));
			}
			if !value.is_array() {
				*value = serde_json::Value::Array(Vec::new());
			}
			let arr = value.as_array_mut().unwrap();
			while arr.len() <= *i {
				arr.push(serde_json::Value::Null);
			}
			json_set_value(&mut arr[*i], rest, new_value)
		}
	}
}

/// Convert a JSON value to its string form for `json_get` output. Strings are
/// returned unquoted (so `json_get('"hi"', '')` yields `hi`, not `"hi"`);
/// other scalars use their canonical text repr; objects and arrays serialize
/// to compact JSON so the output round-trips through `json_get` again.
fn json_value_to_string(value: &serde_json::Value) -> String {
	match value {
		serde_json::Value::String(s) => s.clone(),
		serde_json::Value::Null => "null".to_string(),
		serde_json::Value::Bool(b) => b.to_string(),
		serde_json::Value::Number(n) => n.to_string(),
		serde_json::Value::Array(_) | serde_json::Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
	}
}

fn expect_arity(name: &str, args: &[String], expected: usize) -> Result<(), SubstitutionError> {
	if args.len() != expected {
		return Err(SubstitutionError::FunctionArity {
			name: name.to_string(),
			expected: expected.to_string(),
			got: args.len(),
		});
	}
	Ok(())
}

/// Uppercase the first character of every whitespace-separated word.
/// Word boundaries are detected via `char::is_whitespace`, so tabs / newlines
/// / NBSP all count. Subsequent characters within a word are passed through
/// unchanged so existing internal capitals (`McDonald`) survive intact.
fn capitalize_words(input: &str) -> String {
	let mut out = String::with_capacity(input.len());
	let mut at_word_start = true;
	for ch in input.chars() {
		if ch.is_whitespace() {
			out.push(ch);
			at_word_start = true;
		} else if at_word_start {
			for upper in ch.to_uppercase() {
				out.push(upper);
			}
			at_word_start = false;
		} else {
			out.push(ch);
		}
	}
	out
}

/// Escape control characters and double quotes for embedding in single-line
/// log / display contexts. Single quotes are NOT escaped (Runfile syntax uses
/// `'...'` extensively; double-escaping would surprise users).
fn escape_string(input: &str) -> String {
	let mut out = String::with_capacity(input.len());
	for ch in input.chars() {
		match ch {
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			'\0' => out.push_str("\\0"),
			'"' => out.push_str("\\\""),
			c if (c as u32) < 0x20 => {
				use std::fmt::Write;
				let _ = write!(out, "\\x{:02x}", c as u32);
			}
			c => out.push(c),
		}
	}
	out
}

/// Parse a non-negative integer argument for a function that needs a count
/// (e.g. `repeat`). Surfaces as [`SubstitutionError::InvalidNumber`] on
/// non-numeric or negative input.
fn parse_count(fn_name: &str, arg_name: &str, value: &str) -> Result<usize, SubstitutionError> {
	value
		.trim()
		.parse::<usize>()
		.map_err(|_| SubstitutionError::InvalidNumber {
			name: fn_name.to_string(),
			message: format!("argument `{}` got `{}`", arg_name, value),
		})
}

/// Compile a regex pattern, surfacing compile failures as a clear
/// [`SubstitutionError::InvalidRegex`] tagged with the function name so the
/// user knows which call produced the bad pattern.
fn compile_regex(fn_name: &str, pattern: &str) -> Result<regex::Regex, SubstitutionError> {
	regex::Regex::new(pattern).map_err(|e| SubstitutionError::InvalidRegex {
		name: fn_name.to_string(),
		message: format!("`{}`: {}", pattern, e),
	})
}

/// Produce a quoted form of `value` that the named shell will parse as a
/// single argument carrying the original byte sequence. Used by the
/// `shell_quote` substitution function.
///
/// Strategy per shell:
/// - **POSIX (`bash` / `zsh` / `sh` / `fish`)**: wrap in `'...'`. Inside
///   single quotes EVERY byte is literal — newlines, backslashes, double
///   quotes, dollar signs, the lot. The only escape needed is for the
///   single quote itself, via the close-escape-reopen idiom: `'` →
///   `'\''` (close the literal, escape a `'`, reopen).
/// - **PowerShell**: wrap in `'...'`. PowerShell's literal-string form
///   treats `''` as an escaped single quote; everything else is literal.
/// - **`cmd.exe`**: wrap in `"..."` and escape `"` as `""`. cmd's quoting
///   is genuinely cursed — newlines and `%` can still cause problems
///   depending on how the command is invoked. For arbitrary binary
///   content on cmd, prefer the env-var-indirection pattern.
/// - **Unknown / empty shell**: default to POSIX. Safest baseline; the
///   handful of test cases that build a [`RunArgs`] without setting the
///   shell name still get reasonable behaviour.
fn quote_for_shell(value: &str, shell: &str) -> String {
	match shell {
		"powershell" => {
			let escaped = value.replace('\'', "''");
			format!("'{}'", escaped)
		}
		"cmd" => {
			// NOTE: `cmd /C` performs `%VAR%` (and, under delayed expansion,
			// `!VAR!`) substitution even inside double quotes, and there is no
			// reliable way to escape `%` on the command line — so a value
			// containing `%FOO%` may still be expanded by cmd. This is an
			// inherent cmd.exe limitation, not something quoting can fully
			// neutralize; only `"` is escaped here (to `""`).
			let escaped = value.replace('"', "\"\"");
			format!("\"{}\"", escaped)
		}
		"fish" => {
			// fish single quotes treat `\` and `'` as escape sequences (unlike
			// POSIX, where both are literal inside `'...'`). Escape backslashes
			// FIRST, then single quotes, so a value ending in `\` — which the
			// POSIX branch would render as `'a\'` and fish would read as an
			// unterminated string — is quoted correctly.
			let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
			format!("'{}'", escaped)
		}
		// bash / zsh / sh / unknown — POSIX-style single-quoting.
		_ => {
			let escaped = value.replace('\'', "'\\''");
			format!("'{}'", escaped)
		}
	}
}
