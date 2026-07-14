/// Parse the contents of a .env file into key-value pairs.
///
/// Supports:
/// - `#` and `//` line comments
/// - `KEY=VALUE`, `KEY = VALUE` (spaces around `=`)
/// - Single-quoted, double-quoted, and unquoted values
/// - Multi-line values inside double or single quotes
/// - `KEY=` (empty string value)
/// - Blank lines are ignored
pub fn parse_env_file(content: &str) -> Result<Vec<(String, String)>, (usize, String)> {
	let mut result = Vec::new();
	let lines: Vec<&str> = content.lines().collect();
	let mut i = 0;

	while i < lines.len() {
		let line = lines[i];
		let trimmed = line.trim();

		// Skip blank lines and comments
		if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
			i += 1;
			continue;
		}

		// Find the '=' separator
		let eq_pos = match trimmed.find('=') {
			Some(pos) => pos,
			None => {
				return Err((i + 1, format!("expected KEY=VALUE, got: {trimmed}")));
			}
		};

		let key = trimmed[..eq_pos].trim();
		if key.is_empty() {
			return Err((i + 1, "empty key".to_string()));
		}

		// Strip `export ` prefix if present (common in .env files)
		let key = key.strip_prefix("export ").unwrap_or(key).trim();
		if key.is_empty() {
			return Err((i + 1, "empty key after 'export'".to_string()));
		}

		let raw_value = trimmed[eq_pos + 1..].trim();

		// Check for quoted multi-line values. A value is multi-line ONLY when it
		// opens with a quote that has no matching close on the same line — i.e.
		// `find_closing_quote` returns `None`. A quoted value with trailing
		// content (e.g. `URL="https://x" # comment`) DOES have a closing quote,
		// so it stays on the single-line path and its trailing text/comment is
		// handled there. Keying off "ends with a quote" instead wrongly treated
		// any quoted-value-with-trailing-content as unterminated and swallowed
		// following lines into the value.
		if (raw_value.starts_with('"') && find_closing_quote(&raw_value[1..], '"').is_none())
			|| (raw_value.starts_with('\'') && find_closing_quote(&raw_value[1..], '\'').is_none())
		{
			let quote_char = raw_value.as_bytes()[0] as char;
			let mut value = raw_value[1..].to_string(); // skip opening quote
			i += 1;

			loop {
				if i >= lines.len() {
					return Err((i, format!("unterminated {quote_char}-quoted value for key \"{key}\"")));
				}
				let next_line = lines[i];
				if let Some(end_pos) = find_closing_quote(next_line, quote_char) {
					value.push('\n');
					value.push_str(&next_line[..end_pos]);
					i += 1;
					break;
				} else {
					value.push('\n');
					value.push_str(next_line);
					i += 1;
				}
			}

			if quote_char == '"' {
				value = unescape_double_quoted(&value);
			}
			result.push((key.to_string(), value));
		} else {
			// Single-line value
			let value = parse_single_line_value(raw_value);
			result.push((key.to_string(), value));
			i += 1;
		}
	}

	Ok(result)
}

/// Find the position of a closing quote in a line.
fn find_closing_quote(line: &str, quote: char) -> Option<usize> {
	let bytes = line.as_bytes();
	for j in 0..bytes.len() {
		if bytes[j] == quote as u8 {
			// Check it's not escaped
			let mut backslash_count = 0;
			for k in (0..j).rev() {
				if bytes[k] == b'\\' {
					backslash_count += 1;
				} else {
					break;
				}
			}
			if backslash_count % 2 == 0 {
				return Some(j);
			}
		}
	}
	None
}

/// Parse a single-line value, handling quotes and inline comments.
fn parse_single_line_value(raw: &str) -> String {
	if raw.is_empty() {
		return String::new();
	}

	// Double-quoted value
	if raw.starts_with('"') && raw.len() >= 2 {
		if let Some(end) = find_closing_quote(&raw[1..], '"') {
			let inner = &raw[1..1 + end];
			return unescape_double_quoted(inner);
		}
	}

	// Single-quoted value (no escape processing)
	if raw.starts_with('\'') && raw.len() >= 2 {
		if let Some(end) = find_closing_quote(&raw[1..], '\'') {
			return raw[1..1 + end].to_string();
		}
	}

	// Unquoted value: strip inline comments (# or //)
	let value = if let Some(pos) = raw.find(" #") {
		raw[..pos].trim()
	} else if let Some(pos) = raw.find(" //") {
		raw[..pos].trim()
	} else {
		raw
	};

	value.to_string()
}

/// Process escape sequences in double-quoted strings.
fn unescape_double_quoted(s: &str) -> String {
	let mut result = String::with_capacity(s.len());
	let mut chars = s.chars();
	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.next() {
				Some('n') => result.push('\n'),
				Some('t') => result.push('\t'),
				Some('r') => result.push('\r'),
				Some('"') => result.push('"'),
				Some('\\') => result.push('\\'),
				Some(other) => {
					result.push('\\');
					result.push(other);
				}
				None => result.push('\\'),
			}
		} else {
			result.push(c);
		}
	}
	result
}
