//! DSL evaluation for boolean expressions inside `{{ ... }}` substitutions.
//!
//! A substitution body containing top-level `==`, `!=`, `&&`, `||`, or unary
//! `!` is treated as a boolean DSL expression (detected by [`looks_like_dsl`])
//! and evaluated to the strings `"true"` / `"false"` by
//! [`evaluate_dsl_expression`]. Leaf values resolve through the same machinery
//! as chain segments / function args ([`crate::args::evaluate_arg`] /
//! [`crate::args::walk_template`]), so source refs and function calls work
//! inside conditions exactly as they do elsewhere.

use crate::args::{RunArgs, SubstitutionError, evaluate_arg, skip_nested_subst, walk_template};
use std::collections::{HashMap, HashSet};

/// Whether `inner` (the body of a `{{ ... }}` block, after [`strict_unwrap`])
/// contains DSL boolean operators at top level — `==`, `!=`, `&&`, `||`, or
/// a unary `!`. When true, the body is parsed and evaluated as a boolean
/// expression that resolves to the strings `"true"` or `"false"`.
///
/// Detection is paren / quote / nested-substitution aware so the operators
/// only count when they're at the top level of the body. Operators inside
/// `'...'`, `"..."`, `(...)` (function call args), or nested `{{ ... }}`
/// blocks are ignored. This means `{{ to_upper(ARG.x ? 'foo!') }}` is
/// still a chain (the `!` inside `'foo!'` doesn't count) and
/// `{{ ARG.env == 'prod' }}` is recognised as DSL.
///
/// `!` only counts when it isn't immediately followed by `=` (which would
/// make it the inequality operator `!=`, already caught by the two-char
/// check).
pub(crate) fn looks_like_dsl(inner: &str) -> bool {
	let bytes = inner.as_bytes();
	let mut i = 0usize;
	let mut depth: i32 = 0;
	let mut in_single = false;
	let mut in_double = false;
	while i < bytes.len() {
		let b = bytes[i];
		// Nested `{{ ... }}` is opaque — operators inside it belong to the
		// inner expression. Skip past the matching `}}`. Allowed at top
		// level and inside single-quoted (interpolated) literals; inside
		// `"..."` (verbatim), `{{` is literal text.
		if !in_double && b == b'{' && bytes.get(i + 1) == Some(&b'{') {
			match skip_nested_subst(inner, i) {
				Some(next) => {
					i = next;
					continue;
				}
				// Malformed nested block — bail out conservatively. The
				// downstream parser will surface the real error.
				None => return false,
			}
		}
		if in_single {
			if b == b'\'' {
				in_single = false;
			}
			i += 1;
			continue;
		}
		if in_double {
			if b == b'"' {
				in_double = false;
			}
			i += 1;
			continue;
		}
		match b {
			b'\'' => {
				in_single = true;
				i += 1;
			}
			b'"' => {
				in_double = true;
				i += 1;
			}
			b'(' => {
				depth += 1;
				i += 1;
			}
			b')' => {
				if depth > 0 {
					depth -= 1;
				}
				i += 1;
			}
			_ if depth == 0 => {
				let pair = bytes.get(i..i + 2);
				if pair == Some(b"==") || pair == Some(b"!=") || pair == Some(b"&&") || pair == Some(b"||") {
					return true;
				}
				if b == b'!' && bytes.get(i + 1) != Some(&b'=') {
					return true;
				}
				i += 1;
			}
			_ => i += 1,
		}
	}
	false
}

/// Parse `inner` as a DSL boolean expression and evaluate it against the
/// current substitution context, returning `"true"` or `"false"`. Each
/// DSL value (substitution-body fragment) is resolved through the same
/// machinery as a chain segment / function arg, so source refs (`ARG.x`,
/// `VAR.x`, etc.), function calls, single-quoted, and double-quoted
/// values all work uniformly inside conditions.
pub(crate) fn evaluate_dsl_expression(
	args: &RunArgs,
	inner: &str,
	env: &HashMap<String, String>,
	consumed: &mut HashSet<String>,
	flag_keys: &mut HashSet<String>,
	redact_env: bool,
) -> Result<String, SubstitutionError> {
	use runfile_parser::parse_condition;
	let ast = parse_condition(inner).map_err(|e| SubstitutionError::DslError(format!("{e}: `{inner}`")))?;
	let truth = eval_dsl_ast(args, &ast, env, consumed, flag_keys, redact_env)?;
	Ok(if truth { "true".to_string() } else { "false".to_string() })
}

/// Recursive evaluator for the DSL AST. Reuses the substitution machinery
/// to resolve each leaf value (so source refs and function calls work
/// inside conditions exactly as they do inside a chain or function arg).
fn eval_dsl_ast(
	args: &RunArgs,
	expr: &runfile_parser::DslExpr,
	env: &HashMap<String, String>,
	consumed: &mut HashSet<String>,
	flag_keys: &mut HashSet<String>,
	redact_env: bool,
) -> Result<bool, SubstitutionError> {
	use runfile_parser::DslExpr;
	match expr {
		DslExpr::Truthy(v) => {
			let s = resolve_dsl_value(args, v, env, consumed, flag_keys, redact_env)?;
			// Strict boolean rule (aligned with `if`-block evaluation):
			// - "true" → truthy
			// - "false" or "" (empty) → falsy
			// - anything else → error
			//
			// This makes `FLAG.x` work as a natural boolean inside DSL —
			// `{{ RUN.os == 'windows' && FLAG.wsl }}` Just Works because
			// FLAG.wsl resolves to `"true"` / `"false"`, both valid here.
			// Patterns like `{{ ARG.x && ... }}` where ARG.x is some
			// arbitrary string surface a clear error pointing the user at
			// the explicit comparison form.
			match s.as_str() {
				"true" => Ok(true),
				"false" | "" => Ok(false),
				_ => Err(SubstitutionError::DslValueNotBoolean { value: s }),
			}
		}
		DslExpr::Equality(l, r) => {
			let lhs = resolve_dsl_value(args, l, env, consumed, flag_keys, redact_env)?;
			let rhs = resolve_dsl_value(args, r, env, consumed, flag_keys, redact_env)?;
			Ok(lhs == rhs)
		}
		DslExpr::Inequality(l, r) => {
			let lhs = resolve_dsl_value(args, l, env, consumed, flag_keys, redact_env)?;
			let rhs = resolve_dsl_value(args, r, env, consumed, flag_keys, redact_env)?;
			Ok(lhs != rhs)
		}
		DslExpr::Not(inner) => Ok(!eval_dsl_ast(args, inner, env, consumed, flag_keys, redact_env)?),
		DslExpr::And(parts) => {
			for part in parts {
				if !eval_dsl_ast(args, part, env, consumed, flag_keys, redact_env)? {
					return Ok(false);
				}
			}
			Ok(true)
		}
		DslExpr::Or(parts) => {
			for part in parts {
				if eval_dsl_ast(args, part, env, consumed, flag_keys, redact_env)? {
					return Ok(true);
				}
			}
			Ok(false)
		}
	}
}

/// Resolve a DSL value (a leaf in the boolean-expression AST) to a string.
/// The value text is treated as a substitution-body fragment — it might be
/// a source ref, function call, single-quoted, double-quoted, nested
/// substitution, or a parser-produced [`runfile_parser::DslValue::Substitution`].
fn resolve_dsl_value(
	args: &RunArgs,
	value: &runfile_parser::DslValue,
	env: &HashMap<String, String>,
	consumed: &mut HashSet<String>,
	flag_keys: &mut HashSet<String>,
	redact_env: bool,
) -> Result<String, SubstitutionError> {
	use runfile_parser::DslValue;
	match value {
		DslValue::Substitution(raw) => {
			// Raw includes the `{{ ... }}` delimiters. Run through walk_template
			// so the body is processed by the regular substitution pipeline.
			walk_template(args, raw, env, consumed, flag_keys, redact_env)
		}
		DslValue::Literal(text) => {
			// Under the new quote-strict rules, DSL literals must be:
			// - Single-quoted (interpolated)
			// - Double-quoted (kept verbatim with quotes)
			// - A source ref (ARG.x, VAR.x, etc.) — handled by chain resolver
			// - A function call — handled by chain resolver
			//
			// A raw bareword like `prod` is rejected (must be `'prod'`).
			evaluate_arg(args, text, env, consumed, flag_keys, redact_env)
		}
	}
}
