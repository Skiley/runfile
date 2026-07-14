//! EditorConfig support for the file generators.
//!
//! When `:generate vscode-tasks` / `zed-tasks` / `jetbrains-run-configurations` write a
//! file, the output is formatted to match the project's [EditorConfig](https://editorconfig.org)
//! settings for that path: indentation (`indent_style` / `indent_size` / `tab_width`), line
//! endings (`end_of_line`), trailing-whitespace trimming (`trim_trailing_whitespace`), a final
//! newline (`insert_final_newline`), and a UTF-8 BOM (`charset = utf-8-bom`).
//!
//! The implementation is dependency-free and self-contained (mirroring the crate's
//! "hand-rolled parser" style — see `runfile-parser::dsl`). It parses `.editorconfig` files
//! (an INI-ish format), walks the directory tree upward from the target file (honoring
//! `root = true`), matches EditorConfig glob sections against the file path, and merges the
//! resolved properties (closer files and later sections win).
//!
//! Only the standard core properties are interpreted. Unrecognized properties round-trip as
//! no-ops. `charset` values other than `utf-8` / `utf-8-bom` are not re-encoded (the files we
//! emit are UTF-8): the text is still written as UTF-8, only the BOM is honored.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// `indent_style` — tabs vs. spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
	Tab,
	Space,
}

/// `indent_size` — either an explicit width or the special `tab` value (defer to `tab_width`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentSize {
	UseTab,
	Num(usize),
}

/// `end_of_line` — the newline sequence to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfLine {
	Lf,
	Cr,
	CrLf,
}

/// `charset` — only UTF-8 (with/without BOM) is honored; anything else is passed through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charset {
	Utf8,
	Utf8Bom,
	Other(String),
}

/// Resolved EditorConfig properties for a single file path. All fields are optional — an
/// absent field means "no applicable setting", in which case the generator keeps its own
/// default (2-space indent, LF, no BOM, and whatever trailing newline the renderer emits).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorConfigProps {
	pub indent_style: Option<IndentStyle>,
	pub indent_size: Option<IndentSize>,
	pub tab_width: Option<usize>,
	pub end_of_line: Option<EndOfLine>,
	pub charset: Option<Charset>,
	pub trim_trailing_whitespace: Option<bool>,
	pub insert_final_newline: Option<bool>,
}

impl EditorConfigProps {
	/// Resolve the EditorConfig properties that apply to `path`.
	///
	/// Walks up from the file's directory reading every `.editorconfig` it finds, stopping at
	/// the first one that declares `root = true`. Sections whose glob matches `path` contribute
	/// their properties; closer files and later sections take precedence.
	///
	/// Filesystem/parse errors degrade gracefully to "no settings" (an empty result) rather than
	/// failing generation — an unreadable `.editorconfig` should never block writing a task file.
	pub fn resolve_for_path(path: &Path) -> EditorConfigProps {
		let abs = lexical_normalize(&absolutize(path));

		// Collect (config_dir, parsed) pairs, nearest first, stopping at `root = true`.
		let mut collected: Vec<(PathBuf, ParsedEditorConfig)> = Vec::new();
		let mut dir = abs.parent().map(Path::to_path_buf);
		while let Some(current) = dir {
			let ec_path = current.join(".editorconfig");
			if let Ok(contents) = std::fs::read_to_string(&ec_path) {
				let parsed = parse_editorconfig(&contents);
				let is_root = parsed.root;
				collected.push((current.clone(), parsed));
				if is_root {
					break;
				}
			}
			dir = current.parent().map(Path::to_path_buf);
		}

		// Merge farthest-first so nearest wins; within a file, later sections win.
		let mut merged: BTreeMap<String, String> = BTreeMap::new();
		for (config_dir, parsed) in collected.iter().rev() {
			let rel = match abs.strip_prefix(config_dir) {
				Ok(rel) => to_forward_slash(rel),
				Err(_) => continue,
			};
			for section in &parsed.sections {
				if section_matches(&section.pattern, &rel) {
					for (k, v) in &section.props {
						merged.insert(k.clone(), v.clone());
					}
				}
			}
		}

		interpret_props(&merged)
	}

	/// The indentation unit to use for structural indentation (one level), or `None` to keep the
	/// renderer's own default. Tab style yields `"\t"`; space style yields N spaces.
	pub fn indent_unit(&self) -> Option<String> {
		match self.indent_style {
			Some(IndentStyle::Tab) => Some("\t".to_string()),
			Some(IndentStyle::Space) => self.space_width().map(|n| " ".repeat(n)),
			None => match self.indent_size {
				// No explicit style but an explicit size: treat a numeric size as spaces (the common
				// intent) and `indent_size = tab` as tabs.
				Some(IndentSize::Num(n)) if n >= 1 => Some(" ".repeat(n)),
				Some(IndentSize::UseTab) => Some("\t".to_string()),
				_ => None,
			},
		}
	}

	/// Resolve the number of spaces for space-style indentation, if determinable.
	fn space_width(&self) -> Option<usize> {
		let n = match self.indent_size {
			Some(IndentSize::Num(n)) => Some(n),
			// `indent_size = tab` (or unset) with space style falls back to tab_width.
			Some(IndentSize::UseTab) | None => self.tab_width,
		};
		n.filter(|&n| n >= 1)
	}

	/// Like [`EditorConfigProps::apply`], but first re-indents text whose leading indentation is
	/// one tab per level to the configured indent unit. Intended for fixed, tab-indented
	/// templates (e.g. the `:init` starter Runfile) that aren't produced by a serializer we can
	/// hand an indent to. When the resolved indent is tabs (or unset), the tabs are kept as-is.
	pub fn apply_to_tab_indented(&self, text: &str) -> Vec<u8> {
		let reindented = match self.indent_unit() {
			Some(unit) if unit != "\t" => retab_leading(text, &unit),
			_ => text.to_string(),
		};
		self.apply(&reindented)
	}

	/// Apply the line-level and charset settings to already-rendered text, returning the exact
	/// bytes to write: normalizes line endings, optionally trims trailing whitespace, applies the
	/// final-newline policy, and prepends a UTF-8 BOM when requested. Indentation is handled by
	/// the renderer via [`EditorConfigProps::indent_unit`], not here.
	pub fn apply(&self, text: &str) -> Vec<u8> {
		let eol = match self.end_of_line {
			Some(EndOfLine::CrLf) => "\r\n",
			Some(EndOfLine::Cr) => "\r",
			Some(EndOfLine::Lf) | None => "\n",
		};
		let trim = self.trim_trailing_whitespace == Some(true);

		// Split into logical lines on '\n', dropping a trailing '\r' so pre-existing CRLF input
		// normalizes cleanly. A trailing '\n' yields a final empty segment, so rejoining with `eol`
		// naturally preserves (or drops) the terminal newline.
		let segments: Vec<String> = text
			.split('\n')
			.map(|line| {
				let line = line.strip_suffix('\r').unwrap_or(line);
				if trim {
					line.trim_end().to_string()
				} else {
					line.to_string()
				}
			})
			.collect();
		let mut out = segments.join(eol);

		match self.insert_final_newline {
			Some(true) => {
				if !out.ends_with(eol) {
					out.push_str(eol);
				}
			}
			Some(false) => {
				if let Some(stripped) = out.strip_suffix(eol) {
					out = stripped.to_string();
				}
			}
			None => {}
		}

		let mut bytes = Vec::with_capacity(out.len() + 3);
		if self.charset == Some(Charset::Utf8Bom) {
			bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
		}
		bytes.extend_from_slice(out.as_bytes());
		bytes
	}
}

/// Replace each line's leading run of tabs (one tab per indentation level) with `unit`. Content
/// after the leading tabs is untouched. Tabs are single UTF-8 bytes, so the byte split lines up
/// with the counted char run.
fn retab_leading(text: &str, unit: &str) -> String {
	text.split('\n')
		.map(|line| {
			let tabs = line.chars().take_while(|&c| c == '\t').count();
			if tabs == 0 {
				line.to_string()
			} else {
				format!("{}{}", unit.repeat(tabs), &line[tabs..])
			}
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// Serialize a value as pretty JSON using the given indentation unit (defaults to two spaces,
/// matching `serde_json::to_string_pretty`).
pub fn serialize_json_with_indent<T: serde::Serialize + ?Sized>(
	value: &T,
	indent: Option<&str>,
) -> Result<String, serde_json::Error> {
	let unit = indent.unwrap_or("  ");
	let mut buf = Vec::new();
	let formatter = serde_json::ser::PrettyFormatter::with_indent(unit.as_bytes());
	let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
	value.serialize(&mut ser)?;
	// serde_json always emits valid UTF-8.
	Ok(String::from_utf8(buf).expect("serde_json emits valid UTF-8"))
}

// ── .editorconfig parsing ────────────────────────────────────────────────

struct ParsedEditorConfig {
	root: bool,
	sections: Vec<Section>,
}

struct Section {
	pattern: String,
	/// Recognized `key = value` pairs (keys lowercased); values kept raw for late interpretation.
	props: BTreeMap<String, String>,
}

/// The core properties we interpret. Everything else in a section is ignored.
const KNOWN_KEYS: &[&str] = &[
	"indent_style",
	"indent_size",
	"tab_width",
	"end_of_line",
	"charset",
	"trim_trailing_whitespace",
	"insert_final_newline",
];

fn parse_editorconfig(contents: &str) -> ParsedEditorConfig {
	let mut root = false;
	let mut sections: Vec<Section> = Vec::new();
	let mut current: Option<Section> = None;

	for raw in contents.lines() {
		let line = raw.trim();
		if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
			continue;
		}

		if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
			if let Some(section) = current.take() {
				sections.push(section);
			}
			current = Some(Section {
				pattern: inner.to_string(),
				props: BTreeMap::new(),
			});
			continue;
		}

		let Some((key, value)) = line.split_once('=') else {
			continue;
		};
		let key = key.trim().to_ascii_lowercase();
		let value = value.trim();

		match &mut current {
			None => {
				// Preamble: only `root` is meaningful.
				if key == "root" {
					root = value.eq_ignore_ascii_case("true");
				}
			}
			Some(section) => {
				if KNOWN_KEYS.contains(&key.as_str()) {
					section.props.insert(key, value.to_ascii_lowercase());
				}
			}
		}
	}

	if let Some(section) = current.take() {
		sections.push(section);
	}

	ParsedEditorConfig { root, sections }
}

/// Interpret a merged raw property map into typed properties. A value of `unset` clears the
/// property (per the EditorConfig spec) and so resolves to `None`.
fn interpret_props(map: &BTreeMap<String, String>) -> EditorConfigProps {
	let get = |k: &str| map.get(k).map(String::as_str).filter(|v| *v != "unset");

	EditorConfigProps {
		indent_style: get("indent_style").and_then(|v| match v {
			"tab" => Some(IndentStyle::Tab),
			"space" => Some(IndentStyle::Space),
			_ => None,
		}),
		indent_size: get("indent_size").and_then(|v| {
			if v == "tab" {
				Some(IndentSize::UseTab)
			} else {
				v.parse::<usize>().ok().map(IndentSize::Num)
			}
		}),
		tab_width: get("tab_width").and_then(|v| v.parse::<usize>().ok()),
		end_of_line: get("end_of_line").and_then(|v| match v {
			"lf" => Some(EndOfLine::Lf),
			"cr" => Some(EndOfLine::Cr),
			"crlf" => Some(EndOfLine::CrLf),
			_ => None,
		}),
		charset: get("charset").map(|v| match v {
			"utf-8" => Charset::Utf8,
			"utf-8-bom" => Charset::Utf8Bom,
			other => Charset::Other(other.to_string()),
		}),
		trim_trailing_whitespace: get("trim_trailing_whitespace").and_then(parse_bool),
		insert_final_newline: get("insert_final_newline").and_then(parse_bool),
	}
}

fn parse_bool(v: &str) -> Option<bool> {
	match v {
		"true" => Some(true),
		"false" => Some(false),
		_ => None,
	}
}

// ── path helpers ─────────────────────────────────────────────────────────

fn absolutize(path: &Path) -> PathBuf {
	if path.is_absolute() {
		path.to_path_buf()
	} else {
		std::env::current_dir()
			.map(|cwd| cwd.join(path))
			.unwrap_or_else(|_| path.to_path_buf())
	}
}

/// Lexically resolve `.` and `..` components without touching the filesystem (the target file
/// may not exist yet).
fn lexical_normalize(path: &Path) -> PathBuf {
	let mut out = PathBuf::new();
	for comp in path.components() {
		match comp {
			Component::CurDir => {}
			Component::ParentDir => {
				out.pop();
			}
			other => out.push(other.as_os_str()),
		}
	}
	out
}

fn to_forward_slash(path: &Path) -> String {
	path.to_string_lossy().replace('\\', "/")
}

// ── EditorConfig glob matching ───────────────────────────────────────────

/// Match an EditorConfig section glob against a file path (both forward-slash, `path` relative
/// to the `.editorconfig` directory).
///
/// Per the EditorConfig spec, a pattern **without** a `/` matches the file's basename in any
/// subdirectory; a pattern **with** a `/` (leading or interior) is anchored to the config
/// directory and matched against the whole relative path. Brace groups (`{a,b}`, `{1..3}`) are
/// expanded first, then each alternative is matched with `*` (any run of non-`/`), `**` (any run
/// including `/`), `?` (one non-`/`), and `[...]` character classes.
///
/// Note: an explicit `**/foo` in an anchored pattern does not match a top-level `foo` (the `**`
/// requires the following literal `/`). None of the sections our generated files realistically
/// live under (`*`, `*.json`, `*.xml`, `*.{json,xml}`, `.vscode/*`) rely on that zero-directory
/// form, so this is left unimplemented.
pub(crate) fn section_matches(pattern: &str, rel_path: &str) -> bool {
	let has_sep = pattern.contains('/');
	let (base_pattern, target): (&str, &str) = if has_sep {
		(pattern.strip_prefix('/').unwrap_or(pattern), rel_path)
	} else {
		(pattern, rel_path.rsplit('/').next().unwrap_or(rel_path))
	};

	expand_braces(base_pattern)
		.iter()
		.any(|expanded| wildcard_match(expanded, target))
}

/// Maximum number of patterns a single section may expand into, and the maximum
/// recursion depth of brace expansion. Both guard against a crafted
/// `.editorconfig` section causing memory exhaustion (combinatorial blowup from
/// `{a,b}{c,d}…`, i.e. 2ᴺ) or a stack overflow (deeply nested or long chains of
/// groups). Real sections expand into a handful of patterns at depth ≤ a few.
const MAX_BRACE_EXPANSIONS: usize = 1024;
const MAX_BRACE_DEPTH: usize = 32;

/// Expand `{a,b}` alternations and `{n..m}` numeric ranges into concrete patterns. Groups with
/// no top-level comma that aren't numeric ranges (e.g. `{foo}`) are left literal. Bounded by
/// [`MAX_BRACE_EXPANSIONS`] / [`MAX_BRACE_DEPTH`]: on a pathological pattern, expansion stops
/// early (leaving remaining groups un-expanded, which simply won't match our generated
/// filenames) rather than exhausting memory or the stack.
pub(crate) fn expand_braces(pattern: &str) -> Vec<String> {
	let mut out = Vec::new();
	expand_braces_into(pattern, 0, &mut out);
	if out.is_empty() {
		out.push(pattern.to_string());
	}
	out
}

fn expand_braces_into(pattern: &str, depth: usize, out: &mut Vec<String>) {
	if out.len() >= MAX_BRACE_EXPANSIONS {
		return;
	}
	if depth > MAX_BRACE_DEPTH {
		// Too deeply nested — stop recursing. Emit the (still partly-braced)
		// pattern literally; it won't match a real filename, which is the safe
		// outcome for an adversarial section.
		out.push(pattern.to_string());
		return;
	}

	let chars: Vec<char> = pattern.chars().collect();
	let Some((start, end)) = find_expandable_group(&chars) else {
		out.push(pattern.to_string());
		return;
	};

	let prefix: String = chars[..start].iter().collect();
	let inner: String = chars[start + 1..end].iter().collect();
	let suffix: String = chars[end + 1..].iter().collect();

	let alternatives = split_top_level_commas(&inner);
	let items: Vec<String> = if alternatives.len() >= 2 {
		alternatives
	} else if let Some(range) = parse_numeric_range(&inner) {
		range
	} else {
		// Not actually expandable (shouldn't happen — find_expandable_group screens for this).
		out.push(pattern.to_string());
		return;
	};

	for item in items {
		if out.len() >= MAX_BRACE_EXPANSIONS {
			return;
		}
		expand_braces_into(&format!("{prefix}{item}{suffix}"), depth + 1, out);
	}
}

/// Find the first brace group that is genuinely expandable (has a top-level comma or is a numeric
/// range). Literal `{...}` groups are skipped so re-expansion terminates.
fn find_expandable_group(chars: &[char]) -> Option<(usize, usize)> {
	let mut i = 0;
	while i < chars.len() {
		match chars[i] {
			'\\' => i += 2, // skip escaped char
			'{' => {
				if let Some(end) = matching_brace(chars, i) {
					let inner: String = chars[i + 1..end].iter().collect();
					if split_top_level_commas(&inner).len() >= 2 || parse_numeric_range(&inner).is_some() {
						return Some((i, end));
					}
					i = end + 1;
				} else {
					i += 1;
				}
			}
			_ => i += 1,
		}
	}
	None
}

/// Index of the `}` matching the `{` at `open`, respecting nesting and `\` escapes.
fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
	let mut depth = 0usize;
	let mut i = open;
	while i < chars.len() {
		match chars[i] {
			'\\' => i += 1,
			'{' => depth += 1,
			'}' => {
				depth -= 1;
				if depth == 0 {
					return Some(i);
				}
			}
			_ => {}
		}
		i += 1;
	}
	None
}

/// Split `inner` on commas at brace depth 0 (ignoring escaped commas). Returns a single element
/// when there is no top-level comma.
fn split_top_level_commas(inner: &str) -> Vec<String> {
	let chars: Vec<char> = inner.chars().collect();
	let mut parts = Vec::new();
	let mut current = String::new();
	let mut depth = 0usize;
	let mut i = 0;
	while i < chars.len() {
		match chars[i] {
			'\\' => {
				current.push(chars[i]);
				if i + 1 < chars.len() {
					current.push(chars[i + 1]);
					i += 1;
				}
			}
			'{' => {
				depth += 1;
				current.push('{');
			}
			'}' => {
				depth = depth.saturating_sub(1);
				current.push('}');
			}
			',' if depth == 0 => {
				parts.push(std::mem::take(&mut current));
			}
			c => current.push(c),
		}
		i += 1;
	}
	parts.push(current);
	parts
}

/// Parse `{n..m}` bodies (e.g. `1..3`, `-2..2`) into the inclusive integer sequence as strings.
/// Ranges wider than a sanity cap are treated as non-ranges (returns `None`).
fn parse_numeric_range(inner: &str) -> Option<Vec<String>> {
	let (lo, hi) = inner.split_once("..")?;
	let lo: i64 = lo.trim().parse().ok()?;
	let hi: i64 = hi.trim().parse().ok()?;
	let (start, end) = if lo <= hi { (lo, hi) } else { (hi, lo) };
	if end - start > 65_536 {
		return None;
	}
	Some((start..=end).map(|n| n.to_string()).collect())
}

/// Match a brace-free EditorConfig pattern against `text`. Supports `*` (any run of non-`/`),
/// `**` (any run including `/`), `?` (one non-`/`), `[...]` / `[!...]` character classes, and
/// `\`-escaped literals.
fn wildcard_match(pattern: &str, text: &str) -> bool {
	let pat: Vec<char> = pattern.chars().collect();
	let txt: Vec<char> = text.chars().collect();
	glob_match(&pat, &txt)
}

fn glob_match(pat: &[char], txt: &[char]) -> bool {
	if pat.is_empty() {
		return txt.is_empty();
	}

	match pat[0] {
		'*' => {
			if pat.len() >= 2 && pat[1] == '*' {
				// `**` — match any run, including path separators.
				let rest = &pat[2..];
				(0..=txt.len()).any(|i| glob_match(rest, &txt[i..]))
			} else {
				// `*` — match any run of non-separator characters.
				let rest = &pat[1..];
				if glob_match(rest, txt) {
					return true;
				}
				let mut i = 0;
				while i < txt.len() && txt[i] != '/' {
					i += 1;
					if glob_match(rest, &txt[i..]) {
						return true;
					}
				}
				false
			}
		}
		'?' => !txt.is_empty() && txt[0] != '/' && glob_match(&pat[1..], &txt[1..]),
		'[' => match_char_class(pat, txt),
		'\\' => {
			// Escaped literal: the next pattern char must match verbatim.
			if pat.len() >= 2 {
				!txt.is_empty() && txt[0] == pat[1] && glob_match(&pat[2..], &txt[1..])
			} else {
				!txt.is_empty() && txt[0] == '\\' && glob_match(&pat[1..], &txt[1..])
			}
		}
		c => !txt.is_empty() && txt[0] == c && glob_match(&pat[1..], &txt[1..]),
	}
}

/// Match a `[...]` / `[!...]` character class at the head of `pat` against `txt[0]`.
fn match_char_class(pat: &[char], txt: &[char]) -> bool {
	debug_assert_eq!(pat[0], '[');
	let mut i = 1;
	let negated = pat.get(i) == Some(&'!') || pat.get(i) == Some(&'^');
	if negated {
		i += 1;
	}

	// Collect class members until the closing ']'. A ']' immediately after the (optional)
	// negation is a literal member.
	let mut members: Vec<(char, Option<char>)> = Vec::new();
	let mut closed = None;
	while i < pat.len() {
		if pat[i] == ']' && !members.is_empty() {
			closed = Some(i);
			break;
		}
		// Range like a-z.
		if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
			members.push((pat[i], Some(pat[i + 2])));
			i += 3;
		} else {
			members.push((pat[i], None));
			i += 1;
		}
	}

	let Some(close) = closed else {
		// Unterminated class — treat the '[' as a literal.
		return !txt.is_empty() && txt[0] == '[' && glob_match(&pat[1..], &txt[1..]);
	};

	if txt.is_empty() || txt[0] == '/' {
		return false;
	}
	let ch = txt[0];
	let matched = members.iter().any(|(lo, hi)| match hi {
		Some(hi) => *lo <= ch && ch <= *hi,
		None => *lo == ch,
	});

	(matched != negated) && glob_match(&pat[close + 1..], &txt[1..])
}
