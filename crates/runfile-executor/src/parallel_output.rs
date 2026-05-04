//! Line-buffered, prefixed stdout/stderr for parallel command execution.
//!
//! When multiple commands run concurrently and inherit the parent's stdio,
//! their output interleaves character-by-character — and tools that emit
//! cursor-control escapes (progress bars, in-place line redraws via `\r` or
//! `\x1b[A` / `\x1b[K`) corrupt each other's output. This module pipes each
//! child's stdout/stderr through reader threads that:
//!
//! 1. Split the byte stream on `\n` and `\r` (so dynamic redraws become
//!    chronological append-only lines, à la `concurrently` / `npm-run-all`
//!    / `pnpm run --recursive`).
//! 2. Strip non-SGR ANSI escapes (cursor movement, erase-line, OSC) while
//!    preserving SGR color/style codes.
//! 3. Write each completed line prefixed with a per-leaf tag (e.g. `[3] `)
//!    via the standard stdout/stderr lock so prefix+content+newline lands as
//!    one atomic write.

use std::io::{Read, Write};
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// Which standard stream a pump should write to. The `Capture` variant is
/// only available in tests and routes prefixed output to a shared buffer
/// instead of the real stdout/stderr — used by the end-to-end pump tests
/// in this module to assert on the exact bytes that would have been printed.
#[derive(Clone)]
pub(crate) enum OutputStream {
	Stdout,
	Stderr,
	#[cfg(test)]
	Capture(Arc<Mutex<Vec<u8>>>),
}

/// Spawn a thread that reads from `reader`, splits its bytes into lines on
/// `\n` and `\r`, sanitizes ANSI cursor-control escapes, and writes each
/// completed line prefixed with `prefix` to `stream`.
///
/// Returns when the reader hits EOF (i.e. the child closes its end of the
/// pipe, normally on exit). Caller must `join` the handle to ensure all
/// output has been flushed before reporting the command done.
pub(crate) fn spawn_line_pump<R>(reader: R, prefix: String, stream: OutputStream) -> JoinHandle<()>
where
	R: Read + Send + 'static,
{
	thread::spawn(move || pump(reader, &prefix, &stream))
}

fn pump<R: Read>(mut reader: R, prefix: &str, stream: &OutputStream) {
	let mut buf = [0u8; 4096];
	let mut state = LineState::default();
	let mut emit = |line: &[u8]| emit_line(prefix, line, stream);

	loop {
		match reader.read(&mut buf) {
			Ok(0) => break,
			Ok(n) => process_chunk(&buf[..n], &mut state, &mut emit),
			Err(_) => break,
		}
	}

	// Flush a trailing partial line so nothing is silently lost when a child
	// exits without a final newline.
	if !state.line.is_empty() {
		emit(&state.line);
	}
}

#[derive(Default)]
struct LineState {
	line: Vec<u8>,
	/// Whether the previous byte was `\r`. Used to swallow the `\n` of a
	/// `\r\n` pair so we don't emit a spurious empty line after the CR
	/// already triggered emission.
	prev_cr: bool,
}

fn process_chunk(chunk: &[u8], state: &mut LineState, emit: &mut impl FnMut(&[u8])) {
	for &byte in chunk {
		match byte {
			b'\n' => {
				if state.prev_cr {
					// Tail of a `\r\n` pair — the CR already emitted.
					state.prev_cr = false;
				} else {
					emit(&state.line);
					state.line.clear();
				}
			}
			b'\r' => {
				// Treat CR as a soft line break so progress-bar redraws
				// become append-only lines instead of corrupting interleaved
				// output. Suppress emission when the line is empty to avoid
				// blank prefix-only lines from runs of CRs.
				if !state.line.is_empty() {
					emit(&state.line);
					state.line.clear();
				}
				state.prev_cr = true;
			}
			_ => {
				state.line.push(byte);
				state.prev_cr = false;
			}
		}
	}
}

fn emit_line(prefix: &str, content: &[u8], stream: &OutputStream) {
	// Split at cursor-positioning / erase escapes. Tools running in
	// TTY-mode (Docker Compose's `auto`/`tty` progress UI, fancy CLI
	// loaders) often emit a single "frame" as one logical line containing
	// many `\x1b[<row>;<col>H<text>` segments — each segment is a row of
	// content the user would see on a separate line in a real terminal.
	// Without splitting, all the row contents end up concatenated into one
	// giant prefixed line. Splitting at every cursor-move/erase escape
	// gives us "one prefixed line per virtual row", matching the user's
	// mental model. SGR sequences (color/style) DO NOT split — they're
	// preserved inline so colored text stays colored.
	for segment in split_at_cursor_escapes(content) {
		emit_segment(prefix, segment, stream);
	}
}

fn emit_segment(prefix: &str, segment: &[u8], stream: &OutputStream) {
	let cleaned = strip_cursor_escapes(segment);
	// Trim ASCII whitespace from both ends. Tools that emit terminal-redraw
	// frames (Docker Compose's progress UI, fancy CLI loaders, etc.) often
	// leave behind layout-padding whitespace once their cursor-positioning
	// escapes are stripped — producing wildly indented or right-padded
	// content that's hard to read when interleaved across parallel streams.
	// Trimming reclaims readability at the cost of intra-line indentation
	// in pretty-printed output (rare in parallel contexts).
	let body = trim_ascii_whitespace(&cleaned);
	if body.is_empty() {
		// Drop pure-whitespace segments — they're almost always redraw-frame
		// artifacts and add visual noise without conveying information.
		return;
	}
	// Windows-specific: emit `\r\n` instead of `\n`. With VT processing
	// enabled (Windows Terminal does this automatically as soon as it sees
	// an ANSI escape — and our color prefix triggers this), `\n` is treated
	// as a bare line-feed without carriage return. The cursor moves down
	// but stays in the same column, producing a "growing indent staircase"
	// where each subsequent prefixed line starts at the column where the
	// previous one ended. CRLF restores the canonical "next line, column 0"
	// behavior. On Unix terminals CRLF is harmless (CR alone moves to col
	// 0, LF moves down — same end state as bare LF).
	let line_ending: &[u8] = if cfg!(windows) { b"\r\n" } else { b"\n" };
	let mut out: Vec<u8> = Vec::with_capacity(prefix.len() + body.len() + line_ending.len());
	out.extend_from_slice(prefix.as_bytes());
	out.extend_from_slice(body);
	out.extend_from_slice(line_ending);
	match stream {
		OutputStream::Stdout => {
			let stdout = std::io::stdout();
			let mut h = stdout.lock();
			let _ = h.write_all(&out);
			let _ = h.flush();
		}
		OutputStream::Stderr => {
			let stderr = std::io::stderr();
			let mut h = stderr.lock();
			let _ = h.write_all(&out);
			let _ = h.flush();
		}
		#[cfg(test)]
		OutputStream::Capture(buf) => {
			let mut g = buf.lock().unwrap();
			g.extend_from_slice(&out);
		}
	}
}

/// Split a byte slice at every cursor-positioning / erase ANSI escape, so
/// each "virtual row" of a TTY-mode redraw frame becomes its own segment.
/// SGR escapes (color/style) are NOT split — they stay inline within their
/// segment so colored content keeps its color. OSC and other 2-byte ESC
/// sequences also don't split.
///
/// Returns segments as slices into the original buffer. Each segment may
/// still contain SGR escapes (those are stripped later in `emit_segment`'s
/// call to `strip_cursor_escapes`, which is idempotent on already-clean
/// content but also handles any non-SGR escapes we didn't split on).
fn split_at_cursor_escapes(input: &[u8]) -> Vec<&[u8]> {
	let mut segments = Vec::new();
	let mut start = 0;
	let mut i = 0;
	while i < input.len() {
		if input[i] == 0x1b && i + 1 < input.len() && input[i + 1] == b'[' {
			// Find the CSI final byte.
			let mut j = i + 2;
			while j < input.len() {
				let c = input[j];
				if (0x40..=0x7e).contains(&c) {
					// Final byte. Decide whether this CSI is a "line
					// boundary" escape.
					let final_byte = c;
					if is_line_boundary_csi(final_byte) {
						// Emit segment up to (but not including) this escape.
						segments.push(&input[start..i]);
						// Skip the entire CSI sequence.
						start = j + 1;
					}
					i = j + 1;
					break;
				}
				j += 1;
			}
			if j >= input.len() {
				// Truncated CSI — bail out and let the rest fall through
				// to a single trailing segment.
				i = input.len();
			}
			continue;
		}
		i += 1;
	}
	segments.push(&input[start..]);
	segments
}

/// Whether a CSI final byte indicates a cursor-positioning or erase
/// operation that should be treated as a logical line boundary in our
/// line-splitting model. SGR (`m`) is excluded — it stays inline. Other
/// finals (DEC private mode set/reset `h`/`l`, designate character set
/// `?`, etc.) are also excluded since they don't represent visible
/// "frame element" boundaries.
fn is_line_boundary_csi(final_byte: u8) -> bool {
	matches!(
		final_byte,
		b'A' // CUU - cursor up
		| b'B' // CUD - cursor down
		| b'C' // CUF - cursor forward
		| b'D' // CUB - cursor back
		| b'E' // CNL - cursor next line
		| b'F' // CPL - cursor previous line
		| b'G' // CHA - cursor horizontal absolute
		| b'H' // CUP - cursor position
		| b'J' // ED - erase display
		| b'K' // EL - erase line
		| b'S' // SU - scroll up
		| b'T' // SD - scroll down
		| b'd' // VPA - vertical position absolute
		| b'f' // HVP - horizontal vertical position
	)
}

/// Trim ASCII whitespace from both ends of a byte slice. Used to clean up
/// layout-padding whitespace that survives ANSI escape stripping (e.g.
/// Docker Compose's progress UI emits cursor-positioning escapes followed
/// by literal spaces for column alignment).
fn trim_ascii_whitespace(s: &[u8]) -> &[u8] {
	let start = s.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(s.len());
	let end = s
		.iter()
		.rposition(|b| !b.is_ascii_whitespace())
		.map_or(start, |i| i + 1);
	&s[start..end]
}

/// Strip ANSI cursor-movement and erase escapes (CSI sequences whose final
/// byte is anything other than `m`). SGR sequences (`…m`) are preserved so
/// colored output flows through untouched. OSC (`\x1b]…BEL` / `…ST`) and
/// other ESC-prefixed escapes are dropped entirely.
fn strip_cursor_escapes(input: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(input.len());
	let mut i = 0;
	while i < input.len() {
		let b = input[i];
		if b == 0x1b && i + 1 < input.len() {
			match input[i + 1] {
				b'[' => {
					// CSI: \x1b[ <params>* <intermediates>* <final>
					// final byte is in 0x40..=0x7e. Keep only when final == 'm'
					// (SGR), drop everything else (cursor moves, erase, etc.).
					let mut j = i + 2;
					while j < input.len() {
						let c = input[j];
						if (0x40..=0x7e).contains(&c) {
							if c == b'm' {
								out.extend_from_slice(&input[i..=j]);
							}
							i = j + 1;
							break;
						}
						j += 1;
					}
					if j >= input.len() {
						i = input.len();
					}
					continue;
				}
				b']' => {
					// OSC: \x1b] … BEL  or  … \x1b\\
					let mut j = i + 2;
					while j < input.len() {
						if input[j] == 0x07 {
							i = j + 1;
							break;
						}
						if input[j] == 0x1b && j + 1 < input.len() && input[j + 1] == b'\\' {
							i = j + 2;
							break;
						}
						j += 1;
					}
					if j >= input.len() {
						i = input.len();
					}
					continue;
				}
				_ => {
					// Other 2-byte ESC sequence (e.g. \x1bM = reverse line
					// feed). Drop both bytes.
					i += 2;
					continue;
				}
			}
		}
		out.push(b);
		i += 1;
	}
	out
}

/// Build the prefix string for a parallel leaf's output, given its global
/// step number. Honors `NO_COLOR` for plain output in non-terminal contexts.
pub(crate) fn format_parallel_prefix(step: usize) -> String {
	if std::env::var_os("NO_COLOR").is_some() {
		format!("[{step}] ")
	} else {
		// Cycle through a small palette so adjacent parallel leaves are
		// visually distinct. Matches the spirit of `concurrently` / `pnpm`.
		const COLORS: [&str; 6] = [
			"\x1b[36m", // cyan
			"\x1b[33m", // yellow
			"\x1b[35m", // magenta
			"\x1b[32m", // green
			"\x1b[34m", // blue
			"\x1b[31m", // red
		];
		const RESET: &str = "\x1b[0m";
		let color = COLORS[step.saturating_sub(1) % COLORS.len()];
		format!("{color}[{step}]{RESET} ")
	}
}

/// Whether parallel children should pipe + prefix their output. Disabled by
/// `RUNFILE_NO_LINE_PREFIX=1`/`true` so users can opt out (e.g. when piping
/// runfile's own stdout through a tool that already structures the output).
pub(crate) fn line_prefixing_enabled() -> bool {
	match std::env::var("RUNFILE_NO_LINE_PREFIX") {
		Ok(v) => !(v == "1" || v.eq_ignore_ascii_case("true")),
		Err(_) => true,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn strip_keeps_sgr_color() {
		let input = b"\x1b[31mred\x1b[0m";
		assert_eq!(strip_cursor_escapes(input), input);
	}

	#[test]
	fn strip_removes_cursor_up() {
		let input = b"hello\x1b[2Aworld";
		assert_eq!(strip_cursor_escapes(input), b"helloworld");
	}

	#[test]
	fn strip_removes_erase_line() {
		let input = b"foo\x1b[Kbar";
		assert_eq!(strip_cursor_escapes(input), b"foobar");
	}

	#[test]
	fn strip_removes_osc_title() {
		let input = b"a\x1b]0;title\x07b";
		assert_eq!(strip_cursor_escapes(input), b"ab");
	}

	#[test]
	fn strip_handles_truncated_csi() {
		// Should not panic on a half-finished escape sequence.
		let input = b"foo\x1b[12";
		let _ = strip_cursor_escapes(input);
	}

	fn drive(data: &[u8]) -> Vec<String> {
		let mut state = LineState::default();
		let mut out: Vec<String> = Vec::new();
		let mut emit = |line: &[u8]| out.push(String::from_utf8_lossy(line).to_string());
		process_chunk(data, &mut state, &mut emit);
		if !state.line.is_empty() {
			out.push(String::from_utf8_lossy(&state.line).to_string());
		}
		out
	}

	#[test]
	fn process_chunk_splits_on_lf() {
		assert_eq!(drive(b"a\nb\nc"), vec!["a", "b", "c"]);
	}

	#[test]
	fn process_chunk_splits_on_cr() {
		// Bare CR (progress-bar redraw style) becomes a line break.
		assert_eq!(drive(b"frame1\rframe2\rframe3"), vec!["frame1", "frame2", "frame3"]);
	}

	#[test]
	fn process_chunk_collapses_crlf() {
		// `\r\n` must emit exactly once, not "line\r" + "" from the LF.
		assert_eq!(drive(b"line1\r\nline2\r\n"), vec!["line1", "line2"]);
	}

	#[test]
	fn process_chunk_preserves_blank_lf_lines() {
		// Genuine blank lines (LF-only) are preserved.
		assert_eq!(drive(b"a\n\nb\n"), vec!["a", "", "b"]);
	}

	#[test]
	fn process_chunk_handles_mixed_terminators() {
		assert_eq!(
			drive(b"line1\nline2\rline3\r\nline4"),
			vec!["line1", "line2", "line3", "line4"]
		);
	}

	#[test]
	fn prefix_contains_step_number() {
		// Always contains the step number, regardless of NO_COLOR setting.
		let p = format_parallel_prefix(7);
		assert!(p.contains("[7]"));
		assert!(p.ends_with(' '));
	}

	// ── End-to-end pump tests via OutputStream::Capture ───────────────

	use std::io::Cursor;

	fn run_pump(input: &[u8], prefix: &str) -> String {
		let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
		let stream = OutputStream::Capture(buf.clone());
		let h = spawn_line_pump(Cursor::new(input.to_vec()), prefix.to_string(), stream);
		h.join().unwrap();
		let bytes: Vec<u8> = buf.lock().unwrap().clone();
		// Normalize line endings so tests work on both Windows (where the
		// pump emits CRLF for terminal compatibility) and Unix (LF only).
		String::from_utf8(bytes).unwrap().replace("\r\n", "\n")
	}

	#[test]
	fn pump_prefixes_each_line() {
		let out = run_pump(b"line1\nline2\nline3\n", "[1] ");
		assert_eq!(out, "[1] line1\n[1] line2\n[1] line3\n");
	}

	#[test]
	fn pump_flattens_cr_progress_redraw() {
		// `\r`-driven progress redraws become append-only chronological lines.
		let out = run_pump(b"frame1\rframe2\rframe3\n", "[X] ");
		assert_eq!(out, "[X] frame1\n[X] frame2\n[X] frame3\n");
	}

	#[test]
	fn pump_preserves_sgr_colors() {
		let out = run_pump(b"\x1b[31mred\x1b[0m text\n", "[2] ");
		assert_eq!(out, "[2] \x1b[31mred\x1b[0m text\n");
	}

	#[test]
	fn pump_treats_cursor_movement_escapes_as_line_boundaries() {
		// `\x1b[1A` (cursor up) and `\x1b[K` (erase line) split the stream
		// into separate prefixed lines — each represents a "virtual row" of
		// a TTY-mode redraw frame.
		let out = run_pump(b"a\x1b[1Ab\x1b[Kc\n", "[7] ");
		assert_eq!(out, "[7] a\n[7] b\n[7] c\n");
	}

	#[test]
	fn pump_handles_crlf_without_blank_lines() {
		// `\r\n` must not emit a spurious empty prefixed line for the LF.
		let out = run_pump(b"line1\r\nline2\r\n", "[1] ");
		assert_eq!(out, "[1] line1\n[1] line2\n");
	}

	#[test]
	fn pump_flushes_trailing_partial_line() {
		// A child exiting without a final newline must still surface its
		// output (otherwise short-lived commands could go silent).
		let out = run_pump(b"no-newline-at-end", "[9] ");
		assert_eq!(out, "[9] no-newline-at-end\n");
	}

	#[test]
	fn pump_drops_blank_lines() {
		// Pure-whitespace / empty lines are dropped — they're almost always
		// redraw-frame artifacts and add visual noise in parallel output.
		let out = run_pump(b"a\n\nb\n", "[1] ");
		assert_eq!(out, "[1] a\n[1] b\n");
	}

	#[test]
	fn pump_trims_leading_and_trailing_whitespace() {
		// Layout-padding whitespace (left over after stripping cursor
		// positioning escapes) is trimmed from each line.
		let out = run_pump(b"      Image foo Pulling          \n", "[2] ");
		assert_eq!(out, "[2] Image foo Pulling\n");
	}

	#[test]
	fn pump_drops_pure_whitespace_lines() {
		// A line that's only spaces / tabs (after escape stripping) is
		// dropped entirely — common in Docker Compose-style progress UIs
		// where redraw frames leave behind whitespace-only "rows".
		let out = run_pump(b"real line\n          \n\t\t\nanother\n", "[3] ");
		assert_eq!(out, "[3] real line\n[3] another\n");
	}

	#[test]
	fn pump_splits_tty_frame_at_cursor_position() {
		// TTY-mode tools emit a single "frame" with cursor-position escapes
		// between virtual rows of content, no `\n` between rows. Each row
		// should emerge as its own prefixed line.
		let frame = b"\x1b[1;1HRow A\x1b[2;1HRow B\x1b[3;1HRow C\n";
		let out = run_pump(frame, "[1] ");
		assert_eq!(out, "[1] Row A\n[1] Row B\n[1] Row C\n");
	}

	#[test]
	fn pump_splits_at_cursor_up_then_text() {
		// Docker Compose's redraw pattern: cursor up + content (overwrites
		// previous row). Each content piece should become its own line.
		let frame = b"first\x1b[1Asecond\x1b[1Athird\n";
		let out = run_pump(frame, "[2] ");
		assert_eq!(out, "[2] first\n[2] second\n[2] third\n");
	}

	#[test]
	fn pump_splits_at_erase_line() {
		// `\x1b[K` (erase line) typically marks a redraw boundary too.
		let frame = b"old content\x1b[Knew content\n";
		let out = run_pump(frame, "[3] ");
		assert_eq!(out, "[3] old content\n[3] new content\n");
	}

	#[test]
	fn pump_does_not_split_at_sgr() {
		// SGR (color/style) escapes must NOT split lines — they stay inline
		// within the segment so colored text keeps its color.
		let line = b"\x1b[31mred\x1b[0m and \x1b[32mgreen\x1b[0m\n";
		let out = run_pump(line, "[4] ");
		assert_eq!(out, "[4] \x1b[31mred\x1b[0m and \x1b[32mgreen\x1b[0m\n");
	}
}
