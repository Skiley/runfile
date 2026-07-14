use super::*;
use crate::editorconfig::*;
use std::collections::BTreeMap;
use std::path::Path;

// ── EditorConfig glob matching ───────────────────────────────────────────

#[test]
fn glob_star_matches_at_any_depth() {
	// `[*]` → matches every file, any depth.
	assert!(section_matches("*", "tasks.json"));
	assert!(section_matches("*", ".vscode/tasks.json"));
	assert!(section_matches("*", ".run/Runfile_build.run.xml"));
}

#[test]
fn glob_extension_matches_basename_at_any_depth() {
	assert!(section_matches("*.json", ".vscode/tasks.json"));
	assert!(section_matches("*.json", "tasks.json"));
	assert!(!section_matches("*.json", ".run/Runfile_build.run.xml"));
	assert!(section_matches("*.xml", ".run/Runfile_build.run.xml"));
}

#[test]
fn glob_brace_alternation_on_extension() {
	assert!(section_matches("*.{json,xml}", ".vscode/tasks.json"));
	assert!(section_matches("*.{json,xml}", ".run/x.run.xml"));
	assert!(!section_matches("*.{json,xml}", "notes.txt"));
}

#[test]
fn glob_brace_alternation_on_dirs() {
	assert!(section_matches("{.vscode,.zed}/tasks.json", ".vscode/tasks.json"));
	assert!(section_matches("{.vscode,.zed}/tasks.json", ".zed/tasks.json"));
	assert!(!section_matches("{.vscode,.zed}/tasks.json", ".run/tasks.json"));
}

#[test]
fn glob_interior_slash_is_anchored() {
	// A pattern with a `/` is anchored to the .editorconfig directory: it must match the whole
	// relative path, not just a suffix.
	assert!(section_matches(".vscode/*.json", ".vscode/tasks.json"));
	assert!(!section_matches(".vscode/*.json", "sub/.vscode/tasks.json"));
}

#[test]
fn glob_leading_slash_anchors_to_root() {
	assert!(section_matches("/tasks.json", "tasks.json"));
	assert!(!section_matches("/tasks.json", "sub/tasks.json"));
}

#[test]
fn glob_double_star_spans_separators() {
	assert!(section_matches(".vscode/**", ".vscode/tasks.json"));
	assert!(section_matches(".vscode/**", ".vscode/nested/deep.json"));
}

#[test]
fn glob_single_star_does_not_span_separators() {
	// `dir/*.json` matches one level down only.
	assert!(section_matches("a/*.json", "a/b.json"));
	assert!(!section_matches("a/*.json", "a/b/c.json"));
}

#[test]
fn glob_question_mark_and_char_class() {
	assert!(section_matches("file?.json", "file1.json"));
	assert!(!section_matches("file?.json", "file10.json"));
	assert!(section_matches("*.[jx]son", "a.json"));
	assert!(section_matches("*.[jx]son", "a.xson"));
	assert!(!section_matches("*.[jx]son", "a.tson"));
}

#[test]
fn glob_negated_char_class() {
	assert!(section_matches("*.[!x]ml", "a.hml"));
	assert!(!section_matches("*.[!x]ml", "a.xml"));
}

// ── Brace expansion ──────────────────────────────────────────────────────

#[test]
fn brace_expand_simple() {
	assert_eq!(expand_braces("a{b,c}d"), vec!["abd", "acd"]);
}

#[test]
fn brace_expand_multiple_groups_cross_product() {
	assert_eq!(expand_braces("{a,b}{1,2}"), vec!["a1", "a2", "b1", "b2"]);
}

#[test]
fn brace_expand_nested() {
	assert_eq!(expand_braces("{a,{b,c}}"), vec!["a", "b", "c"]);
}

#[test]
fn brace_expand_numeric_range() {
	assert_eq!(expand_braces("v{1..3}"), vec!["v1", "v2", "v3"]);
	// Descending ranges normalize to ascending.
	assert_eq!(expand_braces("{3..1}"), vec!["1", "2", "3"]);
}

#[test]
fn brace_literal_single_group_is_not_expanded() {
	// No top-level comma and not a numeric range → left literal (and terminates, no infinite loop).
	assert_eq!(expand_braces("a{foo}b"), vec!["a{foo}b"]);
}

// ── Audit L15: brace-expansion must be bounded (no OOM / stack overflow) ──

#[test]
fn brace_expand_bounds_combinatorial_blowup() {
	// 40 binary groups would be 2^40 patterns. Must be capped, not exhaust memory.
	let pattern = "{a,b}".repeat(40);
	let result = expand_braces(&pattern);
	assert!(
		result.len() <= 1024,
		"combinatorial expansion must be bounded, got {}",
		result.len()
	);
}

#[test]
fn brace_expand_bounds_multiplied_ranges() {
	// Multiplied numeric ranges (the single-range cap doesn't bound their product).
	let pattern = "{0..500}{0..500}";
	let result = expand_braces(pattern);
	assert!(
		result.len() <= 1024,
		"multiplied ranges must be bounded, got {}",
		result.len()
	);
}

#[test]
fn brace_expand_deep_nesting_does_not_overflow() {
	// Deeply nested groups must not overflow the recursive expander.
	let pattern = format!("{}z{}", "{a,".repeat(2000), "}".repeat(2000));
	let result = expand_braces(&pattern);
	assert!(!result.is_empty(), "deep nesting must terminate and return something");
}

#[test]
fn brace_expand_matcher_still_works_after_bounding() {
	// Regression: normal section matching that relies on expansion is unaffected.
	assert!(section_matches("*.{json,xml}", ".vscode/tasks.json"));
	assert!(section_matches("*.{json,xml}", ".run/x.run.xml"));
	assert!(!section_matches("*.{json,xml}", "notes.txt"));
}

// ── indent_unit ──────────────────────────────────────────────────────────

#[test]
fn indent_unit_tab() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Tab),
		..Default::default()
	};
	assert_eq!(props.indent_unit().as_deref(), Some("\t"));
}

#[test]
fn indent_unit_spaces_with_size() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Space),
		indent_size: Some(IndentSize::Num(4)),
		..Default::default()
	};
	assert_eq!(props.indent_unit().as_deref(), Some("    "));
}

#[test]
fn indent_unit_space_style_falls_back_to_tab_width() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Space),
		tab_width: Some(8),
		..Default::default()
	};
	assert_eq!(props.indent_unit().as_deref(), Some("        "));
}

#[test]
fn indent_unit_size_only_is_treated_as_spaces() {
	let props = EditorConfigProps {
		indent_size: Some(IndentSize::Num(3)),
		..Default::default()
	};
	assert_eq!(props.indent_unit().as_deref(), Some("   "));
}

#[test]
fn indent_unit_none_when_unset() {
	assert_eq!(EditorConfigProps::default().indent_unit(), None);
}

// ── apply (line endings / final newline / trailing ws / BOM) ─────────────

#[test]
fn apply_default_is_passthrough() {
	let props = EditorConfigProps::default();
	assert_eq!(props.apply("a\nb"), b"a\nb");
	// A trailing newline is preserved when the policy is unset.
	assert_eq!(props.apply("a\nb\n"), b"a\nb\n");
}

#[test]
fn apply_crlf_line_endings() {
	let props = EditorConfigProps {
		end_of_line: Some(EndOfLine::CrLf),
		..Default::default()
	};
	assert_eq!(props.apply("a\nb\n"), b"a\r\nb\r\n");
}

#[test]
fn apply_insert_final_newline_true() {
	let props = EditorConfigProps {
		insert_final_newline: Some(true),
		..Default::default()
	};
	assert_eq!(props.apply("a\nb"), b"a\nb\n");
	// Idempotent when one already exists.
	assert_eq!(props.apply("a\nb\n"), b"a\nb\n");
}

#[test]
fn apply_insert_final_newline_false_strips() {
	let props = EditorConfigProps {
		insert_final_newline: Some(false),
		..Default::default()
	};
	assert_eq!(props.apply("a\nb\n"), b"a\nb");
}

#[test]
fn apply_trim_trailing_whitespace() {
	let props = EditorConfigProps {
		trim_trailing_whitespace: Some(true),
		..Default::default()
	};
	assert_eq!(props.apply("a   \nb\t\n"), b"a\nb\n");
}

#[test]
fn apply_utf8_bom_prepended() {
	let props = EditorConfigProps {
		charset: Some(Charset::Utf8Bom),
		..Default::default()
	};
	let out = props.apply("a");
	assert_eq!(&out[..3], &[0xEF, 0xBB, 0xBF]);
	assert_eq!(&out[3..], b"a");
}

#[test]
fn apply_combines_crlf_and_final_newline() {
	let props = EditorConfigProps {
		end_of_line: Some(EndOfLine::CrLf),
		insert_final_newline: Some(true),
		..Default::default()
	};
	assert_eq!(props.apply("a\nb"), b"a\r\nb\r\n");
}

// ── apply_to_tab_indented (for the :init template) ───────────────────────

#[test]
fn apply_to_tab_indented_keeps_tabs_by_default() {
	// No settings → tab-indented template written verbatim.
	let props = EditorConfigProps::default();
	assert_eq!(props.apply_to_tab_indented("{\n\t\"a\": 1\n}\n"), b"{\n\t\"a\": 1\n}\n");
}

#[test]
fn apply_to_tab_indented_keeps_tabs_when_style_is_tab() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Tab),
		..Default::default()
	};
	assert_eq!(props.apply_to_tab_indented("{\n\t\"a\": 1\n}"), b"{\n\t\"a\": 1\n}");
}

#[test]
fn apply_to_tab_indented_converts_to_spaces() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Space),
		indent_size: Some(IndentSize::Num(2)),
		..Default::default()
	};
	// One tab → 2 spaces; two tabs → 4 spaces; non-leading tabs untouched.
	assert_eq!(
		props.apply_to_tab_indented("{\n\t\"a\": 1,\n\t\t\"b\": 2\n}"),
		b"{\n  \"a\": 1,\n    \"b\": 2\n}"
	);
}

#[test]
fn apply_to_tab_indented_applies_line_settings_too() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Space),
		indent_size: Some(IndentSize::Num(4)),
		end_of_line: Some(EndOfLine::CrLf),
		insert_final_newline: Some(true),
		..Default::default()
	};
	assert_eq!(
		props.apply_to_tab_indented("{\n\t\"a\": 1\n}"),
		b"{\r\n    \"a\": 1\r\n}\r\n"
	);
}

// ── JSON serialization with indent ───────────────────────────────────────

#[test]
fn json_indent_defaults_to_two_spaces() {
	let mut map = BTreeMap::new();
	map.insert("a", 1);
	let json = serialize_json_with_indent(&map, None).unwrap();
	assert_eq!(json, "{\n  \"a\": 1\n}");
}

#[test]
fn json_indent_tab() {
	let mut map = BTreeMap::new();
	map.insert("a", 1);
	let json = serialize_json_with_indent(&map, Some("\t")).unwrap();
	assert_eq!(json, "{\n\t\"a\": 1\n}");
}

// ── resolve_for_path (filesystem, tempfiles) ─────────────────────────────

fn write(path: &Path, contents: &str) {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).unwrap();
	}
	std::fs::write(path, contents).unwrap();
}

#[test]
fn resolve_reads_matching_section() {
	let dir = tempfile::tempdir().unwrap();
	write(
		&dir.path().join(".editorconfig"),
		"root = true\n\n[*.json]\nindent_style = tab\nend_of_line = crlf\ninsert_final_newline = true\n",
	);

	let props = EditorConfigProps::resolve_for_path(&dir.path().join(".vscode/tasks.json"));
	assert_eq!(props.indent_style, Some(IndentStyle::Tab));
	assert_eq!(props.end_of_line, Some(EndOfLine::CrLf));
	assert_eq!(props.insert_final_newline, Some(true));
}

#[test]
fn resolve_later_section_wins() {
	let dir = tempfile::tempdir().unwrap();
	write(
		&dir.path().join(".editorconfig"),
		"root = true\n\n[*]\nindent_size = 2\n\n[*.json]\nindent_size = 4\n",
	);

	let props = EditorConfigProps::resolve_for_path(&dir.path().join("tasks.json"));
	assert_eq!(props.indent_size, Some(IndentSize::Num(4)));
}

#[test]
fn resolve_unset_clears_property() {
	let dir = tempfile::tempdir().unwrap();
	write(
		&dir.path().join(".editorconfig"),
		"root = true\n\n[*]\nindent_style = space\n\n[*.json]\nindent_style = unset\n",
	);

	let props = EditorConfigProps::resolve_for_path(&dir.path().join("tasks.json"));
	assert_eq!(props.indent_style, None);
}

#[test]
fn resolve_stops_at_root() {
	let dir = tempfile::tempdir().unwrap();
	// Outer file would set tabs, but the inner `root = true` file stops the walk before it.
	write(
		&dir.path().join(".editorconfig"),
		"root = true\n\n[*]\nindent_style = tab\n",
	);
	write(
		&dir.path().join("sub/.editorconfig"),
		"root = true\n\n[*]\nindent_style = space\nindent_size = 2\n",
	);

	let props = EditorConfigProps::resolve_for_path(&dir.path().join("sub/tasks.json"));
	assert_eq!(props.indent_style, Some(IndentStyle::Space));
}

#[test]
fn resolve_nearest_file_overrides_farther() {
	let dir = tempfile::tempdir().unwrap();
	write(
		&dir.path().join(".editorconfig"),
		"root = true\n\n[*]\nindent_size = 2\nend_of_line = lf\n",
	);
	// Not a root; contributes an override for a deeper file, nearest wins on indent_size but
	// end_of_line still comes from the ancestor.
	write(&dir.path().join("sub/.editorconfig"), "[*]\nindent_size = 8\n");

	let props = EditorConfigProps::resolve_for_path(&dir.path().join("sub/tasks.json"));
	assert_eq!(props.indent_size, Some(IndentSize::Num(8)));
	assert_eq!(props.end_of_line, Some(EndOfLine::Lf));
}

// ── End-to-end rendering ─────────────────────────────────────────────────

fn sample_vscode_file() -> VsCodeTasksFile {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	VsCodeTasksFile {
		version: "2.0.0".to_string(),
		tasks: generate_vscode_tasks(&runfile),
		extra: serde_json::Map::new(),
	}
}

#[test]
fn render_vscode_default_matches_legacy_output() {
	let file = sample_vscode_file();
	let bytes = render_vscode_tasks(&file, &EditorConfigProps::default()).unwrap();
	// Legacy behavior: to_string_pretty (2-space, LF, no trailing newline).
	let legacy = serde_json::to_string_pretty(&file).unwrap();
	assert_eq!(bytes, legacy.into_bytes());
}

#[test]
fn render_vscode_honors_tabs_crlf_and_final_newline() {
	let file = sample_vscode_file();
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Tab),
		end_of_line: Some(EndOfLine::CrLf),
		insert_final_newline: Some(true),
		..Default::default()
	};
	let bytes = render_vscode_tasks(&file, &props).unwrap();
	let text = String::from_utf8(bytes).unwrap();
	assert!(text.contains("\r\n"), "expected CRLF line endings");
	assert!(!text.contains("\n  \""), "expected tab indentation, not 2 spaces");
	assert!(text.contains("\n\t"), "expected tab indentation");
	assert!(text.ends_with("\r\n"), "expected a final CRLF newline");
}

#[test]
fn render_zed_honors_four_space_indent() {
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let tasks = generate_zed_tasks(&runfile);
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Space),
		indent_size: Some(IndentSize::Num(4)),
		insert_final_newline: Some(true),
		..Default::default()
	};
	let bytes = render_zed_tasks(&tasks, &props).unwrap();
	let text = String::from_utf8(bytes).unwrap();
	// Array elements sit one level deep (4 spaces), object properties two levels (8 spaces).
	assert!(text.contains("\n    {"), "expected 4-space indent on array elements");
	assert!(
		text.contains("\n        \""),
		"expected 8-space indent on object properties"
	);
	assert!(!text.contains("\n  {"), "should not contain the 2-space indent");
	assert!(text.ends_with('\n'));
}

#[test]
fn render_jetbrains_default_matches_generated_xml() {
	// With no EditorConfig settings, the rendered bytes equal the generator's default XML.
	let bytes = render_jetbrains_config("Build", "build", &EditorConfigProps::default());
	let runfile = make_runfile(vec![("build", vec!["cargo build"])]);
	let expected = &generate_jetbrains_configs(&runfile)[0].xml;
	assert_eq!(bytes, expected.as_bytes());
}

#[test]
fn render_jetbrains_honors_tabs() {
	let props = EditorConfigProps {
		indent_style: Some(IndentStyle::Tab),
		..Default::default()
	};
	let bytes = render_jetbrains_config("Build", "build", &props);
	let text = String::from_utf8(bytes).unwrap();
	// `<configuration>` is one level deep (one tab); `<option>` two levels (two tabs).
	assert!(
		text.contains("\n\t<configuration"),
		"expected one-tab indent on <configuration>"
	);
	assert!(text.contains("\n\t\t<option"), "expected two-tab indent on <option>");
	assert!(
		!text.contains("\n  <configuration"),
		"should not contain the 2-space indent"
	);
}
