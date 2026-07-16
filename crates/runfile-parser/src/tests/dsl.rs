use crate::dsl::*;

#[test]
fn parses_truthy_substitution() {
	let ast = parse_condition("{{ ARG.x }}").unwrap();
	assert_eq!(ast, DslExpr::Truthy(DslValue::Substitution("{{ ARG.x }}".into())));
}

#[test]
fn parses_equality() {
	let ast = parse_condition("{{ ARG.env }} == production").unwrap();
	assert_eq!(
		ast,
		DslExpr::Equality(
			DslValue::Substitution("{{ ARG.env }}".into()),
			DslValue::Literal("production".into()),
		)
	);
}

#[test]
fn parses_inequality() {
	let ast = parse_condition("{{ ARG.env }} != \"staging\"").unwrap();
	assert_eq!(
		ast,
		DslExpr::Inequality(
			DslValue::Substitution("{{ ARG.env }}".into()),
			DslValue::Literal("\"staging\"".into()),
		)
	);
}

#[test]
fn parses_and_chain() {
	let ast = parse_condition("a && b && c").unwrap();
	match ast {
		DslExpr::And(parts) => assert_eq!(parts.len(), 3),
		_ => panic!("expected And"),
	}
}

#[test]
fn parses_or_chain() {
	let ast = parse_condition("a || b || c").unwrap();
	match ast {
		DslExpr::Or(parts) => assert_eq!(parts.len(), 3),
		_ => panic!("expected Or"),
	}
}

#[test]
fn rejects_mixed_and_or() {
	let err = parse_condition("a && b || c").unwrap_err();
	assert_eq!(err, DslParseError::MixedAndOr);
}

#[test]
fn allows_grouped_mix() {
	assert!(parse_condition("(a && b) || c").is_ok());
	assert!(parse_condition("a || (b && c)").is_ok());
}

#[test]
fn parses_negation() {
	let ast = parse_condition("!a").unwrap();
	match ast {
		DslExpr::Not(_) => {}
		_ => panic!(),
	}
}

#[test]
fn parses_double_negation() {
	let ast = parse_condition("!!a").unwrap();
	match ast {
		DslExpr::Not(inner) => match *inner {
			DslExpr::Not(_) => {}
			_ => panic!(),
		},
		_ => panic!(),
	}
}

#[test]
fn parses_quoted_strings() {
	// Under the new tokenizer, value tokens keep their RAW text (including
	// the surrounding quotes). The evaluator (in `runfile-executor::args`)
	// then unwraps single-quoted strings (interpolated) or keeps double-
	// quoted strings verbatim — that decision happens at evaluation time,
	// not parse time.
	let ast = parse_condition("'foo bar' == \"baz\"").unwrap();
	assert_eq!(
		ast,
		DslExpr::Equality(
			DslValue::Literal("'foo bar'".into()),
			DslValue::Literal("\"baz\"".into()),
		)
	);
}

#[test]
fn rejects_empty_condition() {
	assert_eq!(parse_condition(""), Err(DslParseError::EmptyCondition));
	assert_eq!(parse_condition("   "), Err(DslParseError::EmptyCondition));
}

#[test]
fn rejects_unterminated_string() {
	match parse_condition("\"oops").unwrap_err() {
		DslParseError::UnterminatedString(_) => {}
		e => panic!("got {e:?}"),
	}
}

#[test]
fn rejects_unterminated_substitution() {
	match parse_condition("{{ ARG.x").unwrap_err() {
		DslParseError::UnterminatedSubstitution(_) => {}
		e => panic!("got {e:?}"),
	}
}

#[test]
fn rejects_empty_parens() {
	assert_eq!(parse_condition("()"), Err(DslParseError::EmptyParens));
}

#[test]
fn rejects_unbalanced_parens() {
	match parse_condition("(a").unwrap_err() {
		DslParseError::UnbalancedParens | DslParseError::UnexpectedEnd(_) => {}
		e => panic!("got {e:?}"),
	}
}

#[test]
fn deeply_nested_parens_rejected_not_overflow() {
	// Audit M5: `((((…x…))))` must surface a clean MaxDepthExceeded error
	// instead of overflowing the stack in the recursive-descent parser.
	let expr = format!("{}x{}", "(".repeat(500), ")".repeat(500));
	assert_eq!(
		parse_condition(&expr),
		Err(DslParseError::MaxDepthExceeded(MAX_DSL_DEPTH))
	);
}

#[test]
fn deeply_nested_negation_rejected_not_overflow() {
	// `!!!!…x` recurses through `parse_not`; must also be depth-bounded.
	let expr = format!("{}x", "!".repeat(500));
	assert_eq!(
		parse_condition(&expr),
		Err(DslParseError::MaxDepthExceeded(MAX_DSL_DEPTH))
	);
}

#[test]
fn moderately_nested_parens_still_parse() {
	// Comfortably under the cap — must still parse.
	let expr = format!("{}x{}", "(".repeat(10), ")".repeat(10));
	assert!(parse_condition(&expr).is_ok());
}

#[test]
fn parses_flag_ternary_substitution() {
	// `{{ FLAG.x ? on : off }}` — colon inside but the inner content is
	// captured as a single substitution token by the DSL tokenizer.
	let ast = parse_condition("{{ FLAG.x ? on : off }} == on").unwrap();
	assert!(matches!(ast, DslExpr::Equality(..)));
}

#[test]
fn parses_complex_expression() {
	let ast =
		parse_condition("{{ ARG.env }} == production && ({{ FLAG.confirm }} == true || {{ ENV.CI }} == \"true\")")
			.unwrap();
	match ast {
		DslExpr::And(parts) => assert_eq!(parts.len(), 2),
		_ => panic!(),
	}
}

#[test]
fn parses_negated_truthy() {
	let ast = parse_condition("!{{ ARG.skip }}").unwrap();
	match ast {
		DslExpr::Not(inner) => assert!(matches!(*inner, DslExpr::Truthy(_))),
		_ => panic!(),
	}
}

#[test]
fn parses_paths_as_barewords() {
	let ast = parse_condition("{{ ENV.PATH }} != /usr/local/bin").unwrap();
	assert!(matches!(ast, DslExpr::Inequality(..)));
}
