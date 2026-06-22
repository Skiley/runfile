use super::*;

#[test]
fn to_upper_with_named_arg() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args
		.substitute("deploy-{{ to_upper(ARG.env) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "deploy-PROD");
}

#[test]
fn to_lower_with_single_quoted_literal() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ to_lower('HELLO') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "hello");
}

#[test]
fn to_lower_with_single_quoted_spaces() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ to_lower('FOO BAR') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "foo bar");
}

#[test]
fn double_quotes_are_literal_chars() {
	// Double-quotes are NOT delimiters — they're part of the value.
	// `"hello"` evaluates to the 7-char string `"hello"` (with quotes).
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ to_lower('\"HELLO\"') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "\"hello\"");
}

#[test]
fn nested_calls_evaluate_inside_out() {
	let args = RunArgs::parse(&["--name=Foo".into()]);
	let result = args
		.substitute("{{ to_upper(to_lower(ARG.name)) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "FOO");
}

#[test]
fn base64_round_trip() {
	let args = RunArgs::parse(&["--payload=hello".into()]);
	let result = args
		.substitute("{{ base64_decode(base64_encode(ARG.payload)) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "hello");
}

#[test]
fn base64_encode_known_value() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ base64_encode('foo') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "Zm9v"); // standard base64 of "foo"
}

#[test]
fn base64_decode_invalid_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ base64_decode('not!base64*') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidBase64(_)));
}

#[test]
fn base64_decode_non_utf8_errors() {
	// 0xFF is invalid as a leading UTF-8 byte. Standard base64 of [0xFF] is "/w==".
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ base64_decode('/w==') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::NonUtf8Decoded));
}

#[test]
fn concat_joins_multiple_args() {
	let args = RunArgs::parse(&["--name=alice".into()]);
	let result = args
		.substitute("{{ concat('hello-', ARG.name, '-2026') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "hello-alice-2026");
}

#[test]
fn concat_single_arg_works() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ concat('only') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "only");
}

#[test]
fn join_with_static_items() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ join(' AND ', 'a', 'b', 'c') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "a AND b AND c");
}

#[test]
fn join_with_args_and_functions() {
	let args = RunArgs::parse(&["--first=alice".into(), "--last=BOB".into()]);
	let result = args
		.substitute("{{ join(', ', ARG.first, to_lower(ARG.last)) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "alice, bob");
}

#[test]
fn join_with_only_separator_returns_empty() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ join('-') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn join_zero_args_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ join() }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::FunctionArity { ref name, .. } if name == "join"));
}

#[test]
fn join_empty_separator() {
	// `join('', a, b)` is equivalent to `concat(a, b)`.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ join('', 'x', 'y', 'z') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "xyz");
}

#[test]
fn replace_all_basic() {
	let args = RunArgs::parse(&["--flags=flag-1 flag-2 flag-3".into()]);
	let result = args
		.substitute("{{ replace_all(ARG.flags, ' ', ' AND ') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "flag-1 AND flag-2 AND flag-3");
}

#[test]
fn replace_all_no_match_returns_original() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ replace_all('hello', 'x', 'Y') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "hello");
}

#[test]
fn replace_all_multiple_occurrences() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ replace_all('aaa', 'a', 'bb') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "bbbbbb");
}

#[test]
fn replace_all_empty_replacement_strips() {
	// Strip a substring entirely.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ replace_all('foo-bar-baz', '-', '') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "foobarbaz");
}

#[test]
fn replace_all_wrong_arity_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ replace_all('a', 'b') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 2, .. } if name == "replace_all"
	));
}

#[test]
fn replace_all_composes_with_other_functions() {
	let args = RunArgs::parse(&["--csv=a,b,c".into()]);
	let result = args
		.substitute("{{ to_upper(replace_all(ARG.csv, ',', ' | ')) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "A | B | C");
}

// ── remove_all ──

#[test]
fn remove_all_strips_every_occurrence() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ remove_all('foo-bar-baz', '-') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "foobarbaz");
}

#[test]
fn remove_all_no_match_returns_input() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ remove_all('hello', 'x') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "hello");
}

#[test]
fn remove_all_wrong_arity_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ remove_all('a') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 1, .. } if name == "remove_all"
	));
}

// ── starts_with / ends_with / contains ──

#[test]
fn starts_with_true_and_false() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ starts_with('hello world', 'hello') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ starts_with('hello world', 'world') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn ends_with_true_and_false() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ ends_with('hello world', 'world') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ ends_with('hello world', 'hello') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn contains_true_and_false() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ contains('hello world', 'lo wo') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ contains('hello world', 'xyz') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn boolean_function_works_as_dsl_truthy() {
	// starts_with returns "true"/"false" so it's a valid DSL Truthy value.
	let args = RunArgs::parse(&["--path=/usr/local/bin".into()]);
	let result = args
		.substitute(
			"{{ starts_with(ARG.path, '/usr') && ends_with(ARG.path, 'bin') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn contains_wrong_arity_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ contains('a') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 1, .. } if name == "contains"
	));
}

// ── capitalize ──

#[test]
fn capitalize_uppercases_first_letter_of_each_word() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ capitalize('hello world') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "Hello World");
}

#[test]
fn capitalize_preserves_internal_capitals() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ capitalize('mcDonald hAs lUnch') }}", &HashMap::new())
		.unwrap();
	// First char of each word uppercased, rest passes through.
	assert_eq!(result, "McDonald HAs LUnch");
}

#[test]
fn capitalize_handles_tabs_and_multiple_spaces() {
	let mut env = HashMap::new();
	env.insert("V".into(), "foo  bar\tbaz".into());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ capitalize(ENV.V) }}", &env).unwrap();
	assert_eq!(result, "Foo  Bar\tBaz");
}

#[test]
fn capitalize_empty_string() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ capitalize('') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

// ── trim / trim_start / trim_end ──

#[test]
fn trim_strips_all_whitespace_both_sides() {
	let mut env = HashMap::new();
	env.insert("V".into(), "  \t hello \n ".into());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ trim(ENV.V) }}", &env).unwrap();
	assert_eq!(result, "hello");
}

#[test]
fn trim_start_only_left() {
	let mut env = HashMap::new();
	env.insert("V".into(), "  hello  ".into());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ trim_start(ENV.V) }}", &env).unwrap();
	assert_eq!(result, "hello  ");
}

#[test]
fn trim_end_only_right() {
	let mut env = HashMap::new();
	env.insert("V".into(), "  hello  ".into());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ trim_end(ENV.V) }}", &env).unwrap();
	assert_eq!(result, "  hello");
}

// ── length ──

#[test]
fn length_counts_unicode_scalars_not_bytes() {
	// "héllo" is 6 bytes (é is 2 bytes in UTF-8) but 5 chars.
	let mut env = HashMap::new();
	env.insert("V".into(), "héllo".into());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ length(ENV.V) }}", &env).unwrap();
	assert_eq!(result, "5");
}

#[test]
fn length_empty_string() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ length('') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "0");
}

// ── escape ──

#[test]
fn escape_backslashes_control_chars_and_double_quotes() {
	// Build the raw value via base64_decode so the source string can carry
	// real control bytes.
	let args = RunArgs::parse(&[]);
	// "a\nb\tc\\d\"e" base64-encodes to: "YQpiCWNcZCJl"
	let result = args
		.substitute("{{ escape(base64_decode('YQpiCWNcZCJl')) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "a\\nb\\tc\\\\d\\\"e");
}

#[test]
fn escape_passes_single_quotes_through() {
	let mut env = HashMap::new();
	env.insert("V".into(), "it's".into());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ escape(ENV.V) }}", &env).unwrap();
	// `'` is NOT escaped — Runfile uses `'...'` heavily, so leaving the
	// quote alone is the less surprising default.
	assert_eq!(result, "it's");
}

#[test]
fn escape_emits_hex_for_other_control_bytes() {
	// Bell = 0x07. base64("\x07") = "Bw==".
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ escape(base64_decode('Bw==')) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "\\x07");
}

// ── repeat ──

#[test]
fn repeat_basic() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ repeat('ab', '3') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "ababab");
}

#[test]
fn repeat_zero_returns_empty() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ repeat('hello', '0') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn repeat_count_can_come_from_args() {
	// Counts naturally come from runtime sources (ARGS / ENV / VARS) — the
	// quoted-literal form is just for static numbers in the Runfile.
	let args = RunArgs::parse(&["--n=4".into()]);
	let result = args.substitute("{{ repeat('ab', ARG.n) }}", &HashMap::new()).unwrap();
	assert_eq!(result, "abababab");
}

#[test]
fn repeat_invalid_count_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ repeat('hi', 'oops') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumber { ref name, .. } if name == "repeat"
	));
}

// ── regex_replace / regex_remove / regex_matches ──

#[test]
fn regex_replace_collapses_whitespace_runs() {
	let mut env = HashMap::new();
	env.insert("V".into(), "hello   world\tagain".into());
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_replace(ENV.V, '\\s+', ' ') }}", &env)
		.unwrap();
	assert_eq!(result, "hello world again");
}

#[test]
fn regex_replace_supports_backreferences() {
	// `(\w+)=(\w+)` → `$2:$1` swaps key=value to value:key.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"{{ regex_replace('foo=bar', '(\\w+)=(\\w+)', '$2:$1') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "bar:foo");
}

#[test]
fn regex_remove_strips_pattern() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_remove('keep-DEL-this-DEL-text', 'DEL-?') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "keep-this-text");
}

#[test]
fn regex_matches_unanchored() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute(
			"{{ regex_matches('v1.2.3', '^v\\d+\\.\\d+\\.\\d+$') }}",
			&HashMap::new()
		)
		.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ regex_matches('v1.2', '^v\\d+\\.\\d+\\.\\d+$') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn regex_invalid_pattern_errors() {
	let args = RunArgs::parse(&[]);
	// Unclosed `(` is a regex compile error.
	let err = args
		.substitute("{{ regex_matches('hello', '(unclosed') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidRegex { ref name, .. } if name == "regex_matches"
	));
}

#[test]
fn regex_matches_works_as_dsl_truthy() {
	// regex_matches returns "true"/"false" → valid DSL Truthy.
	let args = RunArgs::parse(&["--tag=v1.0".into()]);
	let result = args
		.substitute("{{ regex_matches(ARG.tag, '^v[0-9]+\\.[0-9]+$') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

// ── nth / first / last / count_parts ──

#[test]
fn nth_indexes_into_split_result() {
	let args = RunArgs::parse(&["--csv=a,b,c".into()]);
	assert_eq!(
		args.substitute("{{ nth(ARG.csv, ',', '0') }}", &HashMap::new())
			.unwrap(),
		"a"
	);
	assert_eq!(
		args.substitute("{{ nth(ARG.csv, ',', '1') }}", &HashMap::new())
			.unwrap(),
		"b"
	);
	assert_eq!(
		args.substitute("{{ nth(ARG.csv, ',', '2') }}", &HashMap::new())
			.unwrap(),
		"c"
	);
}

#[test]
fn nth_out_of_bounds_returns_empty() {
	// Documented: out-of-bounds is empty string, not an error. Lets
	// users compose without bound-checking when an empty result is
	// acceptable.
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ nth('a,b', ',', '5') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn nth_multi_char_separator() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ nth('one::two::three', '::', '1') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "two");
}

#[test]
fn nth_preserves_empty_parts() {
	// Trailing/leading separator yields empty parts that nth must
	// surface verbatim — this is what makes the "missing slot" vs
	// "empty slot" distinction meaningful when paired with count_parts.
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ nth('a,,c', ',', '1') }}", &HashMap::new()).unwrap(),
		""
	);
	assert_eq!(
		args.substitute("{{ nth(',a', ',', '0') }}", &HashMap::new()).unwrap(),
		""
	);
}

#[test]
fn nth_negative_index_errors_as_invalid_number() {
	// `parse_count` uses `usize::from_str`, so negative indices are
	// rejected — surface as InvalidNumber so the user gets a clear
	// pointer to the bad arg.
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ nth('a,b', ',', '-1') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumber { ref name, .. } if name == "nth"
	));
}

#[test]
fn nth_non_numeric_index_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ nth('a,b', ',', 'one') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumber { ref name, .. } if name == "nth"
	));
}

#[test]
fn nth_wrong_arity_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ nth('a,b', ',') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 2, .. } if name == "nth"
	));
}

#[test]
fn nth_index_from_args() {
	// Index can come from a substituted source — `parse_count` runs on
	// the resolved value, so `--i=2` Just Works.
	let args = RunArgs::parse(&["--csv=a,b,c".into(), "--i=2".into()]);
	let result = args
		.substitute("{{ nth(ARG.csv, ',', ARG.i) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "c");
}

#[test]
fn first_returns_first_part() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ first('a,b,c', ',') }}", &HashMap::new()).unwrap(),
		"a"
	);
}

#[test]
fn first_of_empty_string_is_empty() {
	// `"".split(",")` yields `[""]`, so the first part is `""`.
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ first('', ',') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn first_with_no_separator_present_returns_whole_string() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ first('hello world', ',') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "hello world");
}

#[test]
fn last_returns_last_part() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ last('a,b,c', ',') }}", &HashMap::new()).unwrap(),
		"c"
	);
}

#[test]
fn last_basename_idiom() {
	// `last(path, '/')` is the canonical "basename" use case.
	let args = RunArgs::parse(&["--path=/usr/local/bin/run".into()]);
	let result = args.substitute("{{ last(ARG.path, '/') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "run");
}

#[test]
fn last_trailing_separator_yields_empty() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ last('a,b,', ',') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn count_parts_basic() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ count_parts('a,b,c', ',') }}", &HashMap::new())
			.unwrap(),
		"3"
	);
}

#[test]
fn count_parts_empty_string_is_one() {
	// Rust split semantics: `"".split(",")` → `[""]` (one element).
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ count_parts('', ',') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "1");
}

#[test]
fn count_parts_counts_trailing_empty_part() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ count_parts('a,b,', ',') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "3");
}

#[test]
fn count_parts_no_separator_present_is_one() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ count_parts('hello world', ',') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "1");
}

#[test]
fn count_parts_works_as_dsl_value() {
	// `count_parts` returns a decimal string — usable in `==` checks
	// inside the substitution DSL.
	let args = RunArgs::parse(&["--csv=a,b,c".into()]);
	let result = args
		.substitute("{{ count_parts(ARG.csv, ',') == '3' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

#[test]
fn split_accessors_compose_with_other_functions() {
	// nth + to_upper composition: pull a field out, transform it.
	let args = RunArgs::parse(&["--csv=alice,bob,carol".into()]);
	let result = args
		.substitute("{{ to_upper(nth(ARG.csv, ',', '1')) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "BOB");
}

#[test]
fn split_accessors_work_in_chain_segments() {
	// Function calls are valid chain segments — `nth` can be the
	// fallback or the leading segment.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"host={{ ARG.host ? nth('default,fallback', ',', '0') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "host=default");
}

// ── shell_quote ──

fn args_with_shell(shell: &str) -> RunArgs {
	RunArgs::parse(&[]).with_run_context(RunContext {
		shell: shell.into(),
		..Default::default()
	})
}

#[test]
fn shell_quote_posix_simple() {
	let args = args_with_shell("bash");
	let result = args.substitute("{{ shell_quote('hello') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "'hello'");
}

#[test]
fn shell_quote_posix_with_spaces_and_double_quotes() {
	// POSIX single-quotes preserve double quotes verbatim.
	let args = args_with_shell("bash");
	let result = args
		.substitute(
			"{{ shell_quote('it has \"double quotes\" and spaces') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "'it has \"double quotes\" and spaces'");
}

#[test]
fn shell_quote_posix_escapes_single_quote() {
	// `'` inside the value is escaped via close-escape-reopen idiom.
	// Use ENV to carry the literal value — Runfile syntax has no way to
	// inline a `'` inside a single-quoted literal.
	let args = args_with_shell("zsh");
	let mut env = HashMap::new();
	env.insert("V".to_string(), "it's".to_string());
	let result = args.substitute("{{ shell_quote(ENV.V) }}", &env).unwrap();
	// Value is `it's` → quoted as `'it'\''s'`.
	assert_eq!(result, "'it'\\''s'");
}

#[test]
fn shell_quote_posix_handles_dollar_signs_and_backticks() {
	// Inside POSIX single quotes, `$` and backticks are literal — no
	// command substitution / variable expansion.
	let args = args_with_shell("sh");
	let result = args
		.substitute("{{ shell_quote('$HOME and `whoami`') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "'$HOME and `whoami`'");
}

#[test]
fn shell_quote_posix_handles_newlines() {
	// Build a value containing an actual newline via base64_decode (the
	// Runfile single-quoted form doesn't support escape sequences, so we
	// can't put a literal newline inline). "line1\nline2" is base64
	// "bGluZTEKbGluZTI=".
	let args = args_with_shell("bash");
	let result = args
		.substitute("{{ shell_quote(base64_decode('bGluZTEKbGluZTI=')) }}", &HashMap::new())
		.unwrap();
	// POSIX single-quoted strings can carry literal newlines.
	assert_eq!(result, "'line1\nline2'");
}

#[test]
fn shell_quote_powershell_simple() {
	let args = args_with_shell("powershell");
	let result = args.substitute("{{ shell_quote('hello') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "'hello'");
}

#[test]
fn shell_quote_powershell_escapes_single_quote() {
	// PowerShell escapes `'` as `''`.
	let args = args_with_shell("powershell");
	let mut env = HashMap::new();
	env.insert("V".to_string(), "it's".to_string());
	let result = args.substitute("{{ shell_quote(ENV.V) }}", &env).unwrap();
	assert_eq!(result, "'it''s'");
}

#[test]
fn shell_quote_powershell_keeps_double_quotes_verbatim() {
	let args = args_with_shell("powershell");
	let result = args
		.substitute("{{ shell_quote('say \"hi\"') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "'say \"hi\"'");
}

#[test]
fn shell_quote_cmd_simple() {
	let args = args_with_shell("cmd");
	let result = args.substitute("{{ shell_quote('hello') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "\"hello\"");
}

#[test]
fn shell_quote_cmd_escapes_double_quotes() {
	// cmd uses `""` to embed a `"` inside a `"..."` argument.
	let args = args_with_shell("cmd");
	let result = args
		.substitute("{{ shell_quote('say \"hi\"') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "\"say \"\"hi\"\"\"");
}

#[test]
fn shell_quote_unknown_shell_falls_back_to_posix() {
	// Empty shell name (e.g. tests using RunArgs::default()) defaults to
	// POSIX-style single-quoting — the safest baseline for unix-likes.
	let args = RunArgs::default();
	let result = args.substitute("{{ shell_quote('hello') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "'hello'");
}

#[test]
fn shell_quote_empty_string() {
	// An empty argument is `''` in POSIX, `''` in PowerShell, `""` in cmd.
	let args = args_with_shell("bash");
	let result = args.substitute("{{ shell_quote('') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "''");
}

#[test]
fn shell_quote_composes_with_base64_decode() {
	// The headline use case: decode an env-var payload and inline it
	// safely as a CLI argument. The decoded JSON contains `"`, `:`, `\n`
	// and other shell-special chars — all preserved through the POSIX
	// single-quoted form.
	let mut env = HashMap::new();
	// {"key":"val"} base64-encoded.
	env.insert("PAYLOAD".to_string(), "eyJrZXkiOiJ2YWwifQ==".to_string());
	let args = args_with_shell("bash");
	let result = args
		.substitute("some-tool --json {{ shell_quote(base64_decode(ENV.PAYLOAD)) }}", &env)
		.unwrap();
	assert_eq!(result, "some-tool --json '{\"key\":\"val\"}'");
}

#[test]
fn function_chain_segment_with_fallback() {
	// Function call as a chain segment — used as the fallback when
	// the primary `ARG.host` is missing.
	let mut env = HashMap::new();
	env.insert("HOST".to_string(), "Example.COM".to_string());
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ ARG.host ? to_lower(ENV.HOST) }}", &env).unwrap();
	assert_eq!(result, "example.com");
}

#[test]
fn function_arg_chain_default() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ to_upper(ARG.host ? 'localhost') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "LOCALHOST");
}

#[test]
fn function_arg_run_context() {
	let args = RunArgs::parse(&[]).with_run_context(RunContext {
		os: "linux".into(),
		..Default::default()
	});
	let result = args.substitute("{{ to_upper(RUN.os) }}", &HashMap::new()).unwrap();
	assert_eq!(result, "LINUX");
}

#[test]
fn function_inside_for_loop_var() {
	// For-loops write the iteration variable into VARS — `to_lower(VAR.x)`
	// inside a `for` body resolves to the current iteration value.
	let args = RunArgs::parse(&[]);
	args.vars.lock().unwrap().insert("name".to_string(), "Foo".to_string());
	let result = args.substitute("{{ to_lower(VAR.name) }}", &HashMap::new()).unwrap();
	assert_eq!(result, "foo");
}

#[test]
fn for_loop_writes_vars_and_restores_on_exit() {
	// `for x in [...]` writes VAR.x per iteration; on loop exit, VAR.x
	// reverts to whatever it was before the loop (or stays unset if
	// nothing had defined it).
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let json = format!(
		r#"{{"$schema":"x","targets":{{"t":{{"commands":[
			{{"for":"x","in":["a","b","c"],"do":["{cmd}"]}}
		]}}}}}}"#,
		cmd = match shell.kind {
			ShellKind::Cmd | ShellKind::PowerShell => "echo {{ VAR.x }}",
			_ => "echo {{ VAR.x }}",
		}
	);
	let spec = parse_target(&json, "t");
	let args = RunArgs::default();
	execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// After loop exit, VAR.x is restored (was never set → removed).
	assert!(args.vars.lock().unwrap().get("x").is_none());
}

#[test]
fn for_loop_restores_prior_define() {
	// If `define(x, prior)` ran before a `for x in [...]` block, the loop
	// overwrites VAR.x per iteration but the prior value comes back
	// when the loop ends.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			"{{ define(x, 'prior') }}",
			{"for":"x","in":["a","b"],"do":["echo loop {{ VAR.x }}"]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// Loop ended → VAR.x is back to "prior".
	assert_eq!(args.vars.lock().unwrap().get("x").map(|s| s.as_str()), Some("prior"));
}

#[test]
fn nested_for_loops_with_same_var_compose() {
	// Nested `for x` inside `for x` must work: outer's value is saved
	// when inner enters, restored when inner exits, so outer continues
	// with its own iteration value.
	let shell = get_test_shell();
	let dir = TempDir::new().unwrap();
	let spec = parse_target(
		r#"{"$schema":"x","targets":{"t":{"commands":[
			{"for":"x","in":["outer1","outer2"],"do":[
				{"for":"x","in":["inner1","inner2"],"do":["echo {{ VAR.x }}"]}
			]}
		]}}}"#,
		"t",
	);
	let args = RunArgs::default();
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	// 2 outer * 2 inner = 4 leaves
	assert_eq!(result.commands_run, 4);
	// After all loops exit, VAR.x is restored to nothing.
	assert!(args.vars.lock().unwrap().get("x").is_none());
}

#[test]
fn function_arg_flags_ternary_works() {
	// The full FLAGS ternary form works inside a function argument
	// (chain-segment FLAGS only supports the boolean form).
	let args = RunArgs::parse(&["--debug".into()]);
	let result = args
		.substitute("{{ to_upper(FLAG.debug ? 'on' : 'off') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "ON");
}

#[test]
fn flags_in_chain_returns_boolean() {
	let args = RunArgs::parse(&["--release".into()]);
	let result = args
		.substitute("{{ ARG.x ? FLAG.release ? 'default' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "true");
}

// ── define / VARS ──

#[test]
fn define_writes_var_and_returns_empty() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ define(sdkpath, '/opt/sdk') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
	// Subsequent read sees the value.
	let v = args.substitute("[{{ VAR.sdkpath }}]", &HashMap::new()).unwrap();
	assert_eq!(v, "[/opt/sdk]");
}

#[test]
fn define_with_chain_value() {
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute("{{ define(sdkpath, ENV.SDK ? '/default/sdk') }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.sdkpath }}", &HashMap::new()).unwrap();
	assert_eq!(v, "/default/sdk");
}

#[test]
fn define_with_function_value() {
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute("{{ define(env, to_upper('prod')) }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.env }}", &HashMap::new()).unwrap();
	assert_eq!(v, "PROD");
}

#[test]
fn define_with_single_quoted_name_errors() {
	// Under the new quote-strict rules `define`'s first argument is a
	// bareword identifier — quotes (single or double) are NOT accepted.
	// `'my-var'` is the 7-char string `'my-var'` which fails the
	// identifier regex.
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ define('my-var', 'hello') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidVarName(_)));
}

#[test]
fn define_with_double_quoted_name_errors() {
	// Double-quotes are now part of the literal value, so the name
	// would be `"my-var"` (with quotes) — fails the identifier regex.
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ define(\"my-var\", 'hello') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidVarName(_)));
}

#[test]
fn missing_var_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ VAR.undefined }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MissingVar(_)));
}

#[test]
fn missing_var_with_default_falls_through() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ VAR.undefined ? 'fallback' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "fallback");
}

#[test]
fn define_redefine_last_writer_wins() {
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ define(x, 'first') }}", &HashMap::new()).unwrap();
	let _ = args.substitute("{{ define(x, 'second') }}", &HashMap::new()).unwrap();
	let v = args.substitute("{{ VAR.x }}", &HashMap::new()).unwrap();
	assert_eq!(v, "second");
}

#[test]
fn define_invalid_dynamic_name_errors() {
	// `ARG.name` would resolve at runtime — `define` requires a static label.
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ define(ARG.name, 'value') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidVarName(_)));
}

#[test]
fn define_invalid_name_dotted_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ define(some.thing, 'x') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidVarName(_)));
}

#[test]
fn define_does_not_mutate_during_redacted_pass() {
	// Redacted resolution shouldn't overwrite the VARS map — otherwise
	// the next non-redacted substitute would see redacted-pass values.
	let mut env = HashMap::new();
	env.insert("SECRET".to_string(), "real-secret".to_string());
	let args = RunArgs::parse(&[]);
	// Real pass: x = "real-secret"
	let _ = args.substitute("{{ define(x, ENV.SECRET) }}", &env).unwrap();
	// Redacted pass: would resolve ENV.SECRET to "***" if it ran the
	// define mutation. We require it NOT to.
	let _ = args.substitute_redacted("{{ define(x, ENV.SECRET) }}", &env).unwrap();
	let v = args.substitute("{{ VAR.x }}", &env).unwrap();
	assert_eq!(v, "real-secret", "redacted pass must not overwrite VARS");
}

// ── set_cwd ──

#[test]
fn set_cwd_returns_empty_string() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ set_cwd('subdir') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn set_cwd_relative_path_joins_working_dir() {
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ set_cwd('subdir') }}", &HashMap::new()).unwrap();
	let working_dir = PathBuf::from("/base");
	let resolved = args.spawn_cwd(&working_dir);
	assert_eq!(resolved, PathBuf::from("/base").join("subdir"));
}

#[test]
fn set_cwd_absolute_path_replaces_working_dir() {
	// On Windows, absolute paths look like `C:\foo`; on Unix, `/foo`.
	// `is_absolute` is platform-sensitive — pick a value the platform accepts.
	let absolute = if cfg!(windows) {
		r"C:\absolute\dir"
	} else {
		"/absolute/dir"
	};
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute(
			&format!("{{{{ set_cwd('{}') }}}}", absolute.replace('\\', "\\\\")),
			&HashMap::new(),
		)
		.unwrap();
	let working_dir = PathBuf::from(if cfg!(windows) { r"D:\base" } else { "/base" });
	let resolved = args.spawn_cwd(&working_dir);
	assert_eq!(resolved, PathBuf::from(absolute));
}

#[test]
fn set_cwd_chains_relative_calls() {
	// `set_cwd('a'); set_cwd('b')` joins `a/b` onto working_dir, mirroring
	// shell `cd a; cd b` — relative paths compose.
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ set_cwd('a') }}", &HashMap::new()).unwrap();
	let _ = args.substitute("{{ set_cwd('b') }}", &HashMap::new()).unwrap();
	let working_dir = PathBuf::from("/base");
	let resolved = args.spawn_cwd(&working_dir);
	assert_eq!(resolved, PathBuf::from("/base").join("a").join("b"));
}

#[test]
fn set_cwd_absolute_resets_chain() {
	// An absolute path replaces the override — prior relative calls are
	// discarded. Matches `cd a; cd /abs` shell behaviour.
	let absolute = if cfg!(windows) { r"C:\reset" } else { "/reset" };
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ set_cwd('a') }}", &HashMap::new()).unwrap();
	let _ = args
		.substitute(
			&format!("{{{{ set_cwd('{}') }}}}", absolute.replace('\\', "\\\\")),
			&HashMap::new(),
		)
		.unwrap();
	let working_dir = PathBuf::from(if cfg!(windows) { r"D:\base" } else { "/base" });
	let resolved = args.spawn_cwd(&working_dir);
	assert_eq!(resolved, PathBuf::from(absolute));
}

#[test]
fn set_cwd_with_substituted_arg() {
	let args = RunArgs::parse(&["--target=build".into()]);
	let _ = args
		.substitute("{{ set_cwd(ARG.target ? 'default') }}", &HashMap::new())
		.unwrap();
	let working_dir = PathBuf::from("/base");
	let resolved = args.spawn_cwd(&working_dir);
	assert_eq!(resolved, PathBuf::from("/base").join("build"));
}

#[test]
fn set_cwd_default_no_override() {
	let args = RunArgs::parse(&[]);
	let working_dir = PathBuf::from("/base");
	let resolved = args.spawn_cwd(&working_dir);
	assert_eq!(resolved, PathBuf::from("/base"));
}

#[test]
fn set_cwd_does_not_mutate_during_redacted_pass() {
	// Same guarantee as `define`: the redacted-logging pass must not
	// duplicate the side effect, otherwise the override would be
	// re-applied for every command's log line.
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ set_cwd('first') }}", &HashMap::new()).unwrap();
	let _ = args
		.substitute_redacted("{{ set_cwd('second') }}", &HashMap::new())
		.unwrap();
	let working_dir = PathBuf::from("/base");
	let resolved = args.spawn_cwd(&working_dir);
	// Redacted pass would have chained to `first/second` if it had run.
	assert_eq!(resolved, PathBuf::from("/base").join("first"));
}

#[test]
fn set_cwd_arity_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ set_cwd() }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 0, .. } if name == "set_cwd"
	));
}

#[test]
fn set_cwd_two_args_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ set_cwd('a', 'b') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 2, .. } if name == "set_cwd"
	));
}

#[test]
fn set_cwd_snapshot_captures_state() {
	// Verifies the parallel-collector helper: each leaf carries its own
	// snapshot of `cwd_override` so siblings can't race on the shared mutex.
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ set_cwd('first') }}", &HashMap::new()).unwrap();
	let snap1 = args.snapshot_cwd_override();
	let _ = args.substitute("{{ set_cwd('second') }}", &HashMap::new()).unwrap();
	let snap2 = args.snapshot_cwd_override();
	assert_eq!(snap1, Some(PathBuf::from("first")));
	assert_eq!(snap2, Some(PathBuf::from("first").join("second")));
}

// ── Parser errors ──

#[test]
fn unknown_function_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ frobnicate('x') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::UnknownFunction(ref n) if n == "frobnicate"));
}

#[test]
fn wrong_arity_errors() {
	let args = RunArgs::parse(&["--x=a".into(), "--y=b".into()]);
	let err = args
		.substitute("{{ to_upper(ARG.x, ARG.y) }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 2, .. } if name == "to_upper"
	));
}

#[test]
fn concat_zero_args_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ concat() }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::FunctionArity { ref name, .. } if name == "concat"));
}

#[test]
fn unbalanced_parens_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ to_upper(x }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::UnbalancedParens(_)));
}

#[test]
fn missing_space_after_comma_errors() {
	// `concat('a','b')` — comma not followed by space → arg-list whitespace error.
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ concat('a','b') }}", &HashMap::new()).unwrap_err();
	match err {
		SubstitutionError::MalformedSubstitution(_, ref msg) => {
			assert!(msg.contains("space after `,`"), "got: {}", msg);
		}
		other => panic!("expected MalformedSubstitution, got {:?}", other),
	}
}

#[test]
fn space_before_comma_errors() {
	// `concat('a' , 'b')` — space before comma → arg-list whitespace error.
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ concat('a' , 'b') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MalformedSubstitution(_, _)));
}

#[test]
fn double_space_after_comma_errors() {
	// `concat('a',  'b')` — two spaces after comma → arg-list whitespace error.
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ concat('a',  'b') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MalformedSubstitution(_, _)));
}

#[test]
fn trailing_comma_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ concat('a', ) }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MalformedSubstitution(_, _)));
}

// ── Quoted literal handling ──

#[test]
fn single_quoted_literal_with_comma_inside() {
	// The splitter treats `'` as a grouping boundary so the comma
	// inside doesn't split args; unwrap strips the quotes.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ to_upper('hello, world') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "HELLO, WORLD");
}

#[test]
fn single_quoted_literal_with_parens_inside() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ to_upper('foo(bar)baz') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "FOO(BAR)BAZ");
}

#[test]
fn double_quoted_with_comma_inside_keeps_quotes() {
	// Splitter still groups on `"..."` (so the comma doesn't split),
	// but the value reaches the function with the quotes intact.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ to_upper('\"hello, world\"') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "\"HELLO, WORLD\"");
}

#[test]
fn define_value_with_double_quotes_preserves_them() {
	// Direct from the user spec: `define(images, "test")` must store
	// `"test"` (with quotes) as the value of VAR.images.
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute("{{ define(images, '\"test\"') }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.images }}", &HashMap::new()).unwrap();
	assert_eq!(v, "\"test\"");
}

#[test]
fn define_value_with_single_quotes_strips_them() {
	// Mirror image: single-quoted value gets stripped.
	let args = RunArgs::parse(&[]);
	let _ = args
		.substitute("{{ define(images, 'raw image, image2') }}", &HashMap::new())
		.unwrap();
	let v = args.substitute("{{ VAR.images }}", &HashMap::new()).unwrap();
	assert_eq!(v, "raw image, image2");
}

// ── Chain split paren-awareness ──

#[test]
fn chain_split_skips_question_inside_parens() {
	// ` ? ` inside `to_upper(...)` must not split the chain.
	let mut env = HashMap::new();
	env.insert("HOST".to_string(), "host".to_string());
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ to_upper(ENV.MISSING ? 'fallback') ? 'other' }}", &env)
		.unwrap();
	// The function call resolves to "FALLBACK" — first chain segment wins.
	assert_eq!(result, "FALLBACK");
}

// ── @target propagation ──

#[test]
fn vars_propagate_through_clone() {
	// `RunArgs::clone()` shares the Arc — modifying one side mutates
	// the other. This is the same mechanism that propagates `define`
	// across `@target` invocations.
	let parent = RunArgs::parse(&[]);
	let child = parent.clone();
	let _ = parent
		.substitute("{{ define(shared, 'hello') }}", &HashMap::new())
		.unwrap();
	let v = child.substitute("{{ VAR.shared }}", &HashMap::new()).unwrap();
	assert_eq!(v, "hello");
}

// ── Redacted output ──

#[test]
fn redacted_function_over_env_redacts_input() {
	let mut env = HashMap::new();
	env.insert("SECRET".to_string(), "supersecret".to_string());
	let args = RunArgs::parse(&[]);
	let log = args
		.substitute_redacted("payload={{ to_upper(ENV.SECRET) }}", &env)
		.unwrap();
	// `to_upper("***")` is `"***"`. The real secret is never touched.
	assert_eq!(log, "payload=***");
}

#[test]
fn redacted_var_shows_real_value_known_footgun() {
	// VARS is treated like ARGS — not redacted in logs. Documented
	// footgun: don't put secrets in VARS if you want logs to be safe.
	let mut env = HashMap::new();
	env.insert("PASSWORD".to_string(), "hunter2".to_string());
	let args = RunArgs::parse(&[]);
	let _ = args.substitute("{{ define(pw, ENV.PASSWORD) }}", &env).unwrap();
	let log = args.substitute_redacted("auth={{ VAR.pw }}", &env).unwrap();
	assert_eq!(log, "auth=hunter2");
}

// ── try() ──

#[test]
fn try_swallows_missing_env() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ try(ENV.NOPE) }}", &HashMap::new()).unwrap();
	assert_eq!(result, "");
}

#[test]
fn try_returns_value_when_inner_succeeds() {
	let args = RunArgs::parse(&["--env=prod".into()]);
	let result = args.substitute("{{ try(ARG.env) }}", &HashMap::new()).unwrap();
	assert_eq!(result, "prod");
}

#[test]
fn try_chains_with_default() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ try(ENV.NOPE) ? 'fallback' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "fallback");
}

#[test]
fn try_swallows_decode_error() {
	// base64_decode of garbage normally errors out; wrapping in try
	// reduces it to an empty string.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ try(base64_decode('not!base64*')) }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
}

#[test]
fn try_arity_one() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ try('a', 'b') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::FunctionArity { .. }));
}

#[test]
fn try_failure_does_not_consume_args() {
	// If the inner expression errors, neither --foo nor any other ARGS
	// reference should be marked consumed (so they remain in {{ ARGS }}).
	let args = RunArgs::parse(&["--foo=bar".into(), "--missing".into(), "x".into()]);
	// `try(ARG.missing)` resolves OK ("x"), so --missing IS consumed.
	// `try(ARG.never)` errors and `--foo` stays untouched.
	let result = args
		.substitute("{{ try(ARG.never) ? 'gone' }} {{ ARGS }}", &HashMap::new())
		.unwrap();
	// --foo and --missing/x both still present.
	assert!(result.starts_with("gone "));
	assert!(result.contains("--foo=bar"));
	assert!(result.contains("--missing"));
}

// ── sha256() / md5() ──

#[test]
fn sha256_known_value() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ sha256('hello') }}", &HashMap::new()).unwrap();
	assert_eq!(
		result,
		"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
	);
}

#[test]
fn sha256_empty_string() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ sha256('') }}", &HashMap::new()).unwrap();
	assert_eq!(
		result,
		"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
	);
}

#[test]
fn md5_known_value() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ md5('hello') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "5d41402abc4b2a76b9719d911017c592");
}

#[test]
fn md5_with_arg() {
	let args = RunArgs::parse(&["--data=abc".into()]);
	let result = args.substitute("{{ md5(ARG.data) }}", &HashMap::new()).unwrap();
	assert_eq!(result, "900150983cd24fb0d6963f7d28e17f72");
}

// ── read_file() / file_exists() ──

#[test]
fn read_file_absolute_path() {
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("secret.txt");
	std::fs::write(&p, "the-content\n").unwrap();
	let args = RunArgs::parse(&[]);
	let template = format!("{{{{ read_file('{}') }}}}", p.to_string_lossy().replace('\\', "/"));
	let result = args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(result, "the-content\n");
}

#[test]
fn read_file_relative_uses_run_parent() {
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("data.txt");
	std::fs::write(&p, "abc").unwrap();
	let mut args = RunArgs::parse(&[]);
	args.run_context.parent = dir.path().to_string_lossy().into_owned();
	let result = args.substitute("{{ read_file('data.txt') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "abc");
}

#[test]
fn read_file_missing_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ read_file('/no-such-file-runfile-test-xyz') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::ReadFileError(..)));
}

#[test]
fn read_file_wrapped_in_try_returns_empty_on_missing() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"{{ try(read_file('/no-such-file-runfile-test-xyz')) }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "");
}

#[test]
fn file_exists_true() {
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("present.txt");
	std::fs::write(&p, "x").unwrap();
	let args = RunArgs::parse(&[]);
	let template = format!("{{{{ file_exists('{}') }}}}", p.to_string_lossy().replace('\\', "/"));
	let result = args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(result, "true");
}

#[test]
fn file_exists_false() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ file_exists('/no-such-file-runfile-test-xyz') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "false");
}

// ── write_file() ──

#[test]
fn write_file_absolute_path() {
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("out.txt");
	let args = RunArgs::parse(&[]);
	let template = format!(
		"{{{{ write_file('{}', 'hello world') }}}}",
		p.to_string_lossy().replace('\\', "/")
	);
	let result = args.substitute(&template, &HashMap::new()).unwrap();
	// Returns "" so a line containing only the call is empty-command-skip-eligible.
	assert_eq!(result, "");
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello world");
}

#[test]
fn write_file_relative_uses_run_parent() {
	let dir = tempfile::tempdir().unwrap();
	let mut args = RunArgs::parse(&[]);
	args.run_context.parent = dir.path().to_string_lossy().into_owned();
	let result = args
		.substitute("{{ write_file('data.txt', 'abc') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
	let p = dir.path().join("data.txt");
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "abc");
}

#[test]
fn write_file_overwrites_existing() {
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("out.txt");
	std::fs::write(&p, "old content").unwrap();
	let args = RunArgs::parse(&[]);
	let template = format!(
		"{{{{ write_file('{}', 'new') }}}}",
		p.to_string_lossy().replace('\\', "/")
	);
	args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
}

#[test]
fn write_file_round_trip_large_content_with_apostrophes() {
	// The motivating case: bump-style replace-and-write on a file containing
	// apostrophes. The naive shell `printf %s '...' > file` approach
	// truncates on MSYS-on-Windows past ~5KB; `write_file` bypasses the
	// shell entirely so size and quoting are irrelevant.
	let dir = tempfile::tempdir().unwrap();
	let src = dir.path().join("src.txt");
	let dst = dir.path().join("dst.txt");
	// Big content (~30KB) with apostrophes peppered throughout — the exact
	// shape that breaks the shell-roundtrip path.
	let mut content = String::new();
	for i in 0..1000 {
		content.push_str(&format!(
			"line {} with an 'apostrophe' and a \"quote\" and a $dollar\n",
			i
		));
	}
	std::fs::write(&src, &content).unwrap();
	let args = RunArgs::parse(&[]);
	let template = format!(
		"{{{{ write_file('{}', read_file('{}')) }}}}",
		dst.to_string_lossy().replace('\\', "/"),
		src.to_string_lossy().replace('\\', "/"),
	);
	args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(std::fs::read_to_string(&dst).unwrap(), content);
}

#[test]
fn write_file_dry_run_skips_side_effect() {
	// Dry-run pass must NOT touch the filesystem — same rule `capture`
	// follows. Useful so `--dry-run` is safe on targets that overwrite
	// build artifacts.
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("must-not-exist.txt");
	let args = RunArgs::parse(&[]).with_dry_run(true);
	let template = format!(
		"{{{{ write_file('{}', 'data') }}}}",
		p.to_string_lossy().replace('\\', "/")
	);
	let result = args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(result, "");
	assert!(!p.exists(), "dry-run must not create the file");
}

#[test]
fn write_file_redacted_pass_skips_side_effect() {
	// The executor calls `substitute` (real) and `substitute_redacted` (log)
	// back-to-back on the same template. write_file must skip the second
	// pass so a target's file write doesn't fire twice and the log line
	// stays a no-op.
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("count.txt");
	let args = RunArgs::parse(&[]);
	let template = format!(
		"{{{{ write_file('{}', 'once') }}}}",
		p.to_string_lossy().replace('\\', "/")
	);
	// Real pass: writes the file.
	args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "once");
	// Now simulate the redacted pass by writing different content first and
	// confirming substitute_redacted does NOT overwrite it.
	std::fs::write(&p, "kept").unwrap();
	args.substitute_redacted(&template, &HashMap::new()).unwrap();
	assert_eq!(std::fs::read_to_string(&p).unwrap(), "kept");
}

#[test]
fn write_file_errors_on_bad_path() {
	// Writing into a non-existent directory surfaces as WriteFileError so
	// the user sees what went wrong instead of a silent skip.
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute(
			"{{ write_file('/no/such/dir/xyz-runfile/out.txt', 'x') }}",
			&HashMap::new(),
		)
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::WriteFileError(..)));
}

#[test]
fn write_file_composes_with_read_and_regex() {
	// The bump-target pattern: read → regex_replace → write.
	let dir = tempfile::tempdir().unwrap();
	let p = dir.path().join("gradle.kts");
	std::fs::write(&p, "versionCode = 42\nversionName = \"1.0.0\"\n").unwrap();
	let args = RunArgs::parse(&[]);
	let p_str = p.to_string_lossy().replace('\\', "/");
	let template = format!(
		"{{{{ write_file('{0}', regex_replace(read_file('{0}'), 'versionCode = [0-9]+', 'versionCode = 43')) }}}}",
		p_str
	);
	args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(
		std::fs::read_to_string(&p).unwrap(),
		"versionCode = 43\nversionName = \"1.0.0\"\n"
	);
}

// ── temp_file() / temp_dir() ──

// `temp_file` / `temp_dir` share a process-global cleanup registry, so these
// tests must not run concurrently with one another: one test's
// `cleanup_temp_artifacts()` drain would otherwise delete another test's
// freshly-created artifact in the window between creation and assertion.
// Serialize them with a dedicated lock (poison-resistant so a panic in one
// test doesn't cascade). Other tests never touch the registry, so they're
// unaffected and stay parallel.
static TEMP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp_test_guard() -> std::sync::MutexGuard<'static, ()> {
	TEMP_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn temp_file_creates_empty_file_in_os_temp_dir() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let path = args.substitute("{{ temp_file() }}", &HashMap::new()).unwrap();
	let p = std::path::Path::new(&path);
	assert!(p.is_file(), "temp_file() should create a real file");
	assert!(
		p.starts_with(std::env::temp_dir()),
		"temp_file() should live in the OS temp dir, got {path}"
	);
	assert_eq!(std::fs::read_to_string(p).unwrap(), "", "no-arg temp_file is empty");
	crate::cleanup_temp_artifacts();
	assert!(!p.exists(), "cleanup should delete the file");
}

#[test]
fn temp_file_writes_content() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let path = args
		.substitute("{{ temp_file('hello world') }}", &HashMap::new())
		.unwrap();
	assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
	crate::cleanup_temp_artifacts();
}

#[test]
fn temp_file_extension_suffix_with_and_without_dot() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let a = args
		.substitute("{{ temp_file('x', 'json') }}", &HashMap::new())
		.unwrap();
	let b = args
		.substitute("{{ temp_file('x', '.json') }}", &HashMap::new())
		.unwrap();
	assert!(a.ends_with(".json"), "extension should append, got {a}");
	assert!(b.ends_with(".json"), "leading dot should be stripped, got {b}");
	assert!(!b.ends_with("..json"), "no double dot, got {b}");
	crate::cleanup_temp_artifacts();
}

#[test]
fn temp_file_distinct_calls_distinct_files() {
	// No caching: each occurrence creates its own file (unlike `capture`).
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let a = args.substitute("{{ temp_file() }}", &HashMap::new()).unwrap();
	let b = args.substitute("{{ temp_file() }}", &HashMap::new()).unwrap();
	assert_ne!(a, b, "two temp_file() calls must yield distinct paths");
	crate::cleanup_temp_artifacts();
}

#[test]
fn temp_file_dry_run_creates_nothing() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]).with_dry_run(true);
	let result = args.substitute("cat {{ temp_file('x') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "cat <temp_file>");
}

#[test]
fn temp_file_redacted_pass_creates_nothing() {
	// The executor substitutes the real command then `substitute_redacted` for
	// the log line. temp_file must skip creation on the redacted pass so the
	// log substitution doesn't spawn a second, orphaned temp file.
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let log = args
		.substitute_redacted("cat {{ temp_file('x') }}", &HashMap::new())
		.unwrap();
	assert_eq!(log, "cat <temp_file>");
}

#[test]
fn temp_file_define_reuse_is_single_file() {
	// Canonical pattern: capture the path once, reuse it. One file created;
	// VAR holds the real path so subsequent reads (and logs) see it.
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	args.substitute("{{ define(cfg, temp_file('payload')) }}", &HashMap::new())
		.unwrap();
	let path = args.substitute("{{ VAR.cfg }}", &HashMap::new()).unwrap();
	assert_eq!(std::fs::read_to_string(&path).unwrap(), "payload");
	crate::cleanup_temp_artifacts();
}

#[test]
fn temp_file_too_many_args_errors() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ temp_file('a', 'b', 'c') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::FunctionArity { .. }));
}

#[test]
fn temp_dir_creates_directory_and_cleanup_removes_recursively() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let path = args.substitute("{{ temp_dir() }}", &HashMap::new()).unwrap();
	let p = std::path::Path::new(&path);
	assert!(p.is_dir(), "temp_dir() should create a directory");
	assert!(p.starts_with(std::env::temp_dir()));
	// Drop a file inside to prove cleanup is recursive.
	std::fs::write(p.join("inner.txt"), "data").unwrap();
	crate::cleanup_temp_artifacts();
	assert!(!p.exists(), "cleanup should remove the directory and its contents");
}

#[test]
fn temp_dir_with_arg_errors() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ temp_dir('x') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::FunctionArity { .. }));
}

#[test]
fn temp_dir_dry_run_placeholder() {
	let _guard = temp_test_guard();
	let args = RunArgs::parse(&[]).with_dry_run(true);
	let result = args.substitute("ls {{ temp_dir() }}", &HashMap::new()).unwrap();
	assert_eq!(result, "ls <temp_dir>");
}

// ── json_get() / json_set() ──

#[test]
fn json_get_simple_string() {
	// Inside a single-quoted substitution literal, `"` is itself literal
	// (single quotes don't escape) — so we just write JSON with plain
	// `"` chars without any escaping dance.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_get('{"name":"runfile"}', 'name') }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, "runfile");
}

#[test]
fn json_get_nested() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_get('{"a":{"b":{"c":42}}}', 'a.b.c') }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, "42");
}

#[test]
fn json_get_array_index() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			r#"{{ json_get('{"users":[{"name":"alice"},{"name":"bob"}]}', 'users.1.name') }}"#,
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "bob");
}

#[test]
fn json_get_missing_returns_empty() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_get('{"a":1}', 'b.c.d') }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
}

#[test]
fn json_get_invalid_json_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute(r#"{{ json_get('not json', 'a') }}"#, &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidJson(..)));
}

#[test]
fn json_get_object_returns_compact_json() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_get('{"a":{"b":1}}', 'a') }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, r#"{"b":1}"#);
}

#[test]
fn json_set_simple() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_set('{"a":1}', 'b', '2') }}"#, &HashMap::new())
		.unwrap();
	// 2 parses as JSON number, gets stored as number.
	assert_eq!(result, r#"{"a":1,"b":2}"#);
}

#[test]
fn json_set_string_value_when_not_json() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_set('{}', 'name', 'hello') }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, r#"{"name":"hello"}"#);
}

#[test]
fn json_set_creates_intermediate_objects() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_set('{}', 'a.b.c', 'true') }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, r#"{"a":{"b":{"c":true}}}"#);
}

#[test]
fn json_set_creates_arrays_for_numeric_segments() {
	// The replacement value `"x"` is a fully-double-quoted literal that
	// passes through with the surrounding `"` chars — `json_set` then
	// parses it as a JSON string `"x"` (3 chars including the quotes).
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(r#"{{ json_set('{}', 'items.2', "x") }}"#, &HashMap::new())
		.unwrap();
	assert_eq!(result, r#"{"items":[null,null,"x"]}"#);
}

#[test]
fn json_set_invalid_json_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute(r#"{{ json_set('not json', 'k', 'v') }}"#, &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidJson(..)));
}

#[test]
fn json_path_empty_segment_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute(r#"{{ json_get('{"a":1}', 'a..b') }}"#, &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::InvalidJsonPath(..)));
}

// ── regex_capture ──

#[test]
fn regex_capture_extracts_named_group_via_index() {
	// `regex_capture(haystack, pattern, group_idx)` — common idiom for
	// "pull a substring out of a file" without the greedy
	// `(?s)^.*X(...)X.*$` + `regex_replace` dance.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"{{ regex_capture('versionName = \"3.0.0\"', 'versionName = \"([^\"]+)\"', '1') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "3.0.0");
}

#[test]
fn regex_capture_group_zero_is_whole_match() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_capture('foo=42', '[a-z]+=\\d+', '0') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "foo=42");
}

#[test]
fn regex_capture_returns_empty_on_no_match() {
	// Out-of-bounds convention matches `nth`: missing match / missing
	// group → "". Pair with regex_matches to bound-check if you need to
	// distinguish "not found" from "matched but group is empty".
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_capture('hello', 'world', '0') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
}

#[test]
fn regex_capture_returns_empty_on_out_of_range_group() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_capture('abc', 'a(b)c', '5') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
}

#[test]
fn regex_capture_finds_first_match_only() {
	// `regex_capture` returns the FIRST match's capture, not all of them
	// — keeps the contract single-string in / single-string out.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"{{ regex_capture('a=1 b=2 c=3', '([a-z])=(\\d)', '2') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "1");
}

#[test]
fn regex_capture_bad_pattern_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ regex_capture('hello', '(unclosed', '1') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidRegex { ref name, .. } if name == "regex_capture"
	));
}

#[test]
fn regex_capture_non_numeric_group_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ regex_capture('abc', 'a(b)c', 'oops') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumber { ref name, .. } if name == "regex_capture"
	));
}

// ── regex_capture_all ──

#[test]
fn regex_capture_all_joins_every_match_group() {
	// `regex_capture_all(haystack, pattern, group, separator)` — the all
	// variant of `regex_capture`: group N of every match, joined by sep.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"{{ regex_capture_all('a=1 b=2 c=3', '([a-z])=(\\d)', '2', ',') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "1,2,3");
}

#[test]
fn regex_capture_all_group_zero_is_whole_match() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute(
			"{{ regex_capture_all('a=1 b=2', '[a-z]=\\d', '0', ' ') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "a=1 b=2");
}

#[test]
fn regex_capture_all_returns_empty_on_no_match() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_capture_all('hello', 'world', '0', ',') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "");
}

#[test]
fn regex_capture_all_out_of_range_group_yields_empty_entries() {
	// A group index that never participates contributes "" per match, so
	// two matches joined by "," yield a single separator between empties.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ regex_capture_all('ab ab', 'a(b)', '5', ',') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, ",");
}

#[test]
fn regex_capture_all_bad_pattern_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute(
			"{{ regex_capture_all('hello', '(unclosed', '1', ',') }}",
			&HashMap::new(),
		)
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidRegex { ref name, .. } if name == "regex_capture_all"
	));
}

#[test]
fn regex_capture_all_non_numeric_group_errors() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ regex_capture_all('abc', 'a(b)c', 'oops', ',') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumber { ref name, .. } if name == "regex_capture_all"
	));
}

// ── one_of ──

#[test]
fn one_of_passes_through_matching_value() {
	// The canonical use case: validate an enum-like positional arg
	// against the allow-list, return it untouched, no surrounding
	// `match` block needed.
	let args = RunArgs::parse(&["patch".into()]);
	let result = args
		.substitute(
			"{{ one_of(ARGS, 'major', 'minor', 'patch', 'build') }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "patch");
}

#[test]
fn one_of_rejects_unknown_value_with_options_list() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ one_of('huge', 'small', 'medium', 'large') }}", &HashMap::new())
		.unwrap_err();
	match err {
		SubstitutionError::OneOfNoMatch { value, options } => {
			assert_eq!(value, "huge");
			assert!(options.contains("\"small\""));
			assert!(options.contains("\"medium\""));
			assert!(options.contains("\"large\""));
		}
		other => panic!("expected OneOfNoMatch, got {:?}", other),
	}
}

#[test]
fn one_of_single_option_works() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ one_of('only', 'only') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "only");
}

#[test]
fn one_of_requires_at_least_two_args() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ one_of('value') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 1, .. } if name == "one_of"
	));
}

#[test]
fn one_of_composes_with_define() {
	// The whole point: collapse a 4-case `match { ... define(part, ...) ... }`
	// block down to a single `define(part, one_of(ARGS, ...))` line.
	let args = RunArgs::parse(&["minor".into()]);
	let template = "{{ define(part, one_of(ARGS, 'major', 'minor', 'patch')) }}target={{ VAR.part }}";
	let result = args.substitute(template, &HashMap::new()).unwrap();
	assert_eq!(result, "target=minor");
}

// ── add / subtract / multiply / divide ──

#[test]
fn add_integers_returns_integer_string() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ add('5', '3') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "8");
}

#[test]
fn add_floats_with_whole_result_returns_integer_string() {
	// 5.5 + 2.3 + 1.2 = 9.0 exactly — must format as "9", not "9.0".
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ add('5.5', '2.3', '1.2') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "9");
}

#[test]
fn add_mixed_int_and_float_returns_float() {
	// 5 + 1.1 = 6.1 — fractional result formats with decimals.
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ add('5', '1.1') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "6.1");
}

#[test]
fn add_is_variadic_two_or_more() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ add('1', '2', '3', '4', '5') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "15");
}

#[test]
fn add_rejects_single_argument() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ add('1') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 1, .. } if name == "add"
	));
}

#[test]
fn add_rejects_non_numeric() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ add('5', 'oops') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumeric { ref name, .. } if name == "add"
	));
}

#[test]
fn add_rejects_infinity_string() {
	// `f64::from_str` accepts "inf" / "nan"; we reject them so downstream
	// arithmetic stays well-defined.
	let args = RunArgs::parse(&[]);
	assert!(args.substitute("{{ add('5', 'inf') }}", &HashMap::new()).is_err());
	assert!(args.substitute("{{ add('5', 'nan') }}", &HashMap::new()).is_err());
}

#[test]
fn subtract_left_folds() {
	let args = RunArgs::parse(&[]);
	// 10 - 3 - 2 = 5
	let result = args
		.substitute("{{ subtract('10', '3', '2') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "5");
}

#[test]
fn subtract_can_produce_negative() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ subtract('3', '5') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "-2");
}

#[test]
fn subtract_zero_result_formats_as_zero() {
	// -0.0 must format as "0", not "-0".
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ subtract('5', '5') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "0");
}

#[test]
fn multiply_variadic() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ multiply('2', '3', '4') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "24");
}

#[test]
fn multiply_float_result() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ multiply('2.5', '4') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "10");
}

#[test]
fn divide_integer_result_drops_decimal() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ divide('10', '2') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "5");
}

#[test]
fn divide_fractional_result_keeps_decimal() {
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ divide('10', '4') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "2.5");
}

#[test]
fn divide_left_folds_variadic() {
	// 100 / 5 / 4 = 5
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ divide('100', '5', '4') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "5");
}

#[test]
fn divide_by_zero_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ divide('5', '0') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::DivideByZero));
}

#[test]
fn divide_by_zero_at_any_step_errors() {
	// Zero divisor anywhere in the fold trips the error.
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ divide('100', '5', '0', '2') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(err, SubstitutionError::DivideByZero));
}

#[test]
fn arithmetic_nests_naturally() {
	// add(multiply(a, b), c) — exercises the recursive arg-resolution
	// path used by every nested-function case.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ add(multiply('3', '4'), '5') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "17");
}

#[test]
fn arithmetic_consumes_vars_and_args() {
	// Real-world bump pattern: `add(VAR.current_code, '1')`. Confirms
	// the arithmetic family routes through the normal arg-resolution
	// chain (so ARGS / VARS / chain fallbacks all just work).
	let args = RunArgs::parse(&["--n=10".into()]);
	let template = "{{ define(x, '5') }}sum={{ add(VAR.x, ARG.n) }}";
	let result = args.substitute(template, &HashMap::new()).unwrap();
	assert_eq!(result, "sum=15");
}

#[test]
fn arithmetic_scientific_notation_accepted() {
	// f64::from_str accepts scientific notation — keep that behaviour
	// so `add('1e3', '1')` works as expected.
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ add('1e3', '1') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "1001");
}

#[test]
fn arithmetic_trims_whitespace() {
	// Captures often arrive with whitespace; `parse_numeric` trims so
	// `add(capture('echo 5'), '3')` works without an explicit `trim`.
	let args = RunArgs::parse(&[]);
	let result = args.substitute("{{ add(' 5 ', '3') }}", &HashMap::new()).unwrap();
	assert_eq!(result, "8");
}

// ── numeric comparisons & is_number ──

#[test]
fn less_than_true_and_false() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ less_than('10', '50') }}", &HashMap::new()).unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ less_than('50', '50') }}", &HashMap::new()).unwrap(),
		"false"
	);
	assert_eq!(
		args.substitute("{{ less_than('60', '50') }}", &HashMap::new()).unwrap(),
		"false"
	);
}

#[test]
fn less_than_or_equal_includes_equality() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ less_than_or_equal('50', '50') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ less_than_or_equal('60', '50') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn greater_than_true_and_false() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ greater_than('60', '50') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ greater_than('50', '50') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn greater_than_or_equal_includes_equality() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ greater_than_or_equal('50', '50') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ greater_than_or_equal('40', '50') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
}

#[test]
fn comparison_accepts_floats_and_trims() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ less_than(' 2.5 ', '10') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
}

#[test]
fn comparison_rejects_non_numeric() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ less_than('5', 'oops') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::InvalidNumeric { ref name, .. } if name == "less_than"
	));
}

#[test]
fn comparison_requires_two_args() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ greater_than('5') }}", &HashMap::new()).unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 1, .. } if name == "greater_than"
	));
}

#[test]
fn comparison_works_with_vars() {
	let args = RunArgs::parse(&[]);
	let template = "{{ define(x, '30') }}{{ less_than(VAR.x, '50') }}";
	assert_eq!(args.substitute(template, &HashMap::new()).unwrap(), "true");
}

#[test]
fn comparison_negates_via_dsl_bang() {
	// `less_than` returns a Truthy value, so the DSL `!` operator inverts it.
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ !less_than('10', '50') }}", &HashMap::new())
			.unwrap(),
		"false"
	);
	assert_eq!(
		args.substitute("{{ !less_than('60', '50') }}", &HashMap::new())
			.unwrap(),
		"true"
	);
}

#[test]
fn is_number_detects_numbers() {
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ is_number('42') }}", &HashMap::new()).unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ is_number('3.14') }}", &HashMap::new()).unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ is_number('1e3') }}", &HashMap::new()).unwrap(),
		"true"
	);
	assert_eq!(
		args.substitute("{{ is_number(' 7 ') }}", &HashMap::new()).unwrap(),
		"true"
	);
}

#[test]
fn is_number_rejects_non_numbers_without_erroring() {
	// Unlike the arithmetic family, non-numeric input is `"false"`, not an error.
	let args = RunArgs::parse(&[]);
	assert_eq!(
		args.substitute("{{ is_number('oops') }}", &HashMap::new()).unwrap(),
		"false"
	);
	assert_eq!(
		args.substitute("{{ is_number('') }}", &HashMap::new()).unwrap(),
		"false"
	);
	assert_eq!(
		args.substitute("{{ is_number('inf') }}", &HashMap::new()).unwrap(),
		"false"
	);
	assert_eq!(
		args.substitute("{{ is_number('nan') }}", &HashMap::new()).unwrap(),
		"false"
	);
}

#[test]
fn is_number_requires_one_arg() {
	let args = RunArgs::parse(&[]);
	let err = args
		.substitute("{{ is_number('1', '2') }}", &HashMap::new())
		.unwrap_err();
	assert!(matches!(
		err,
		SubstitutionError::FunctionArity { ref name, got: 2, .. } if name == "is_number"
	));
}

// ── capture ──

#[test]
#[cfg(not(windows))]
fn capture_runs_shell_and_strips_trailing_newline() {
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ capture('printf hello') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "hello");
}

#[test]
#[cfg(not(windows))]
fn capture_strips_only_single_trailing_newline() {
	// Internal newlines stay; only the LAST `\n` (or `\r\n`) is stripped.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ capture('printf \"a\\nb\\n\"') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "a\nb");
}

#[test]
#[cfg(not(windows))]
fn capture_non_zero_exit_errors() {
	let args = RunArgs::parse(&[]);
	let err = args.substitute("{{ capture('exit 7') }}", &HashMap::new()).unwrap_err();
	match err {
		SubstitutionError::CaptureFailed { command, message } => {
			assert_eq!(command, "exit 7");
			assert!(message.contains("7"));
		}
		other => panic!("expected CaptureFailed, got {:?}", other),
	}
}

#[test]
fn capture_dry_run_returns_placeholder() {
	// Dry-run flag suppresses the side effect. The placeholder shows the
	// resolved command verbatim so the user can see what would have run.
	let args = RunArgs::parse(&[]).with_dry_run(true);
	let result = args
		.substitute("{{ capture('rm -rf /important') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "<capture: 'rm -rf /important'>");
}

#[test]
#[cfg(not(windows))]
fn capture_memoizes_repeated_calls() {
	// Cache hit avoids re-running the shell. We verify by routing through
	// a tiny script that increments a counter file: the second `capture`
	// must return the SAME value as the first (because it hits the cache
	// without spawning), even though running the script again would yield
	// a higher number.
	//
	// The script avoids nested single quotes in the Runfile-level
	// `capture('...')` literal, which would otherwise terminate the
	// outer single-quoted string early.
	let dir = tempfile::tempdir().unwrap();
	let counter = dir.path().join("c.txt");
	std::fs::write(&counter, "0").unwrap();
	let script = dir.path().join("incr.sh");
	std::fs::write(
		&script,
		"#!/bin/sh\nn=$(cat \"$1\")\necho $((n+1)) > \"$1\"\ncat \"$1\"\n",
	)
	.unwrap();
	let args = RunArgs::parse(&[]);
	let cmd = format!(
		"sh {} {}",
		script.to_string_lossy().replace('\\', "/"),
		counter.to_string_lossy().replace('\\', "/"),
	);
	let template = format!("{{{{ capture('{}') }}}}", cmd);
	let first = args.substitute(&template, &HashMap::new()).unwrap();
	let second = args.substitute(&template, &HashMap::new()).unwrap();
	assert_eq!(first, "1");
	assert_eq!(second, "1");
	// File reflects a single increment (the second `capture` hit the cache).
	assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "1");
}

#[test]
#[cfg(not(windows))]
fn capture_feeds_into_other_functions() {
	// Composes with the arithmetic family — the motivating use case:
	// "git rev-count + 1" or similar dynamic bump pipelines.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ add(capture('printf 41'), '1') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "42");
}

#[test]
#[cfg(not(windows))]
fn capture_works_with_try_fallback() {
	// `try(capture(...))` should swallow CaptureFailed and fall through.
	let args = RunArgs::parse(&[]);
	let result = args
		.substitute("{{ try(capture('exit 1')) ? 'fallback' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "fallback");
}

// ── path helpers ──────────────────────────────────────────────────

#[test]
fn path_helpers_basename_dirname_extname_stem() {
	let args = RunArgs::parse(&[]);
	let sub = |t: &str| args.substitute(t, &HashMap::new()).unwrap();
	assert_eq!(sub("{{ basename('a/b/c.txt') }}"), "c.txt");
	assert_eq!(sub("{{ dirname('a/b/c.txt') }}").replace('\\', "/"), "a/b");
	assert_eq!(sub("{{ extname('a/b.tar.gz') }}"), "gz");
	assert_eq!(sub("{{ stem('a/b.tar.gz') }}"), "b.tar");
	assert_eq!(sub("{{ extname('noext') }}"), "");
}

#[test]
fn path_join_path_builds_path() {
	let args = RunArgs::parse(&["--dir=src".into()]);
	let result = args
		.substitute("{{ join_path(ARG.dir, 'main.rs') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result.replace('\\', "/"), "src/main.rs");
}

// ── now() ─────────────────────────────────────────────────────────

#[test]
fn now_unix_timestamp_is_numeric() {
	let args = RunArgs::parse(&[]);
	let ts = args.substitute("{{ now('unix-timestamp') }}", &HashMap::new()).unwrap();
	let n: u64 = ts.parse().expect("unix timestamp must be an integer");
	assert!(n > 1_600_000_000, "timestamp should be a plausible recent value: {n}");
}

#[test]
fn now_iso_formats_shape() {
	let args = RunArgs::parse(&[]);
	let date = args.substitute("{{ now('iso-date') }}", &HashMap::new()).unwrap();
	assert!(
		date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-',
		"iso-date should be YYYY-MM-DD, got {date}"
	);
	let iso = args.substitute("{{ now('iso') }}", &HashMap::new()).unwrap();
	assert!(
		iso.ends_with('Z') && iso.contains('T'),
		"iso should be ...T...Z, got {iso}"
	);
}

#[test]
fn now_unknown_format_errors() {
	let args = RunArgs::parse(&[]);
	assert!(matches!(
		args.substitute("{{ now('bogus') }}", &HashMap::new()).unwrap_err(),
		SubstitutionError::InvalidTimeFormat(_)
	));
}

// ── numeric extensions ────────────────────────────────────────────

#[test]
fn numeric_modulo_power() {
	let args = RunArgs::parse(&[]);
	let sub = |t: &str| args.substitute(t, &HashMap::new()).unwrap();
	assert_eq!(sub("{{ modulo('17', '5') }}"), "2");
	assert_eq!(sub("{{ power('2', '10') }}"), "1024");
}

#[test]
fn numeric_modulo_by_zero_errors() {
	let args = RunArgs::parse(&[]);
	assert!(matches!(
		args.substitute("{{ modulo('5', '0') }}", &HashMap::new()).unwrap_err(),
		SubstitutionError::DivideByZero
	));
}

#[test]
fn numeric_min_max_abs_rounding() {
	let args = RunArgs::parse(&[]);
	let sub = |t: &str| args.substitute(t, &HashMap::new()).unwrap();
	assert_eq!(sub("{{ min('3', '1', '2') }}"), "1");
	assert_eq!(sub("{{ max('3', '1', '2') }}"), "3");
	assert_eq!(sub("{{ abs('-7') }}"), "7");
	assert_eq!(sub("{{ round('2.6') }}"), "3");
	assert_eq!(sub("{{ floor('2.9') }}"), "2");
	assert_eq!(sub("{{ ceil('2.1') }}"), "3");
}

// ── substring ─────────────────────────────────────────────────────

#[test]
fn substring_with_and_without_length() {
	let args = RunArgs::parse(&[]);
	let sub = |t: &str| args.substitute(t, &HashMap::new()).unwrap();
	assert_eq!(sub("{{ substring('abcdef', '2') }}"), "cdef");
	assert_eq!(sub("{{ substring('abcdef', '1', '3') }}"), "bcd");
	// Out-of-range start → empty; length past end clamps.
	assert_eq!(sub("{{ substring('abc', '10') }}"), "");
	assert_eq!(sub("{{ substring('abc', '1', '99') }}"), "bc");
}

#[test]
fn substring_unicode_is_char_indexed() {
	let args = RunArgs::parse(&[]);
	// "héllo": chars h é l l o — substring(1,2) → "él"
	let result = args
		.substitute("{{ substring('héllo', '1', '2') }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "él");
}

// ── uuid ──────────────────────────────────────────────────────────

#[test]
fn uuid_has_v4_shape_and_is_unique() {
	let args = RunArgs::parse(&[]);
	let a = args.substitute("{{ uuid() }}", &HashMap::new()).unwrap();
	let b = args.substitute("{{ uuid() }}", &HashMap::new()).unwrap();
	assert_eq!(a.len(), 36, "uuid len: {a}");
	let bytes = a.as_bytes();
	assert!(bytes[8] == b'-' && bytes[13] == b'-' && bytes[18] == b'-' && bytes[23] == b'-');
	assert_eq!(bytes[14], b'4', "version nibble should be 4: {a}");
	assert!(matches!(bytes[19], b'8' | b'9' | b'a' | b'b'), "variant nibble: {a}");
	assert_ne!(a, b, "two uuids should differ");
}

#[test]
fn uuid_dry_run_is_placeholder() {
	let mut args = RunArgs::parse(&[]);
	args.dry_run = true;
	assert_eq!(args.substitute("{{ uuid() }}", &HashMap::new()).unwrap(), "<uuid>");
}

// ── url encode / decode ───────────────────────────────────────────

#[test]
fn url_encode_decode_roundtrip() {
	let args = RunArgs::parse(&[]);
	let sub = |t: &str| args.substitute(t, &HashMap::new()).unwrap();
	assert_eq!(sub("{{ url_encode('a b&c=d') }}"), "a%20b%26c%3Dd");
	assert_eq!(sub("{{ url_decode('a%20b%26c%3Dd') }}"), "a b&c=d");
	// Unreserved chars pass through.
	assert_eq!(sub("{{ url_encode('A-Z_a.z~9') }}"), "A-Z_a.z~9");
}

#[test]
fn url_decode_malformed_errors() {
	let args = RunArgs::parse(&[]);
	assert!(matches!(
		args.substitute("{{ url_decode('bad%2') }}", &HashMap::new())
			.unwrap_err(),
		SubstitutionError::InvalidUrlEncoding(_)
	));
}

// ── error() (substitution side) ───────────────────────────────────

#[test]
fn error_returns_user_error() {
	let args = RunArgs::parse(&[]);
	match args.substitute("{{ error('boom') }}", &HashMap::new()) {
		Err(SubstitutionError::UserError(msg)) => assert_eq!(msg, "boom"),
		other => panic!("expected UserError, got {other:?}"),
	}
}

#[test]
fn error_dry_run_is_placeholder() {
	let mut args = RunArgs::parse(&[]);
	args.dry_run = true;
	assert_eq!(
		args.substitute("{{ error('nope') }}", &HashMap::new()).unwrap(),
		"<error: 'nope'>"
	);
}
