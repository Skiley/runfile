//! Line-buffered, prefixed stdout/stderr for parallel command execution.
//!
//! When multiple commands run concurrently and inherit the parent's stdio,
//! their output interleaves character-by-character — and tools that emit
//! cursor-control escapes (progress bars, in-place line redraws via `\r` or
//! `\x1b[A` / `\x1b[K`) corrupt each other's output. This module pipes each
//! child's stdout/stderr through reader threads (one per stream per child)
//! that:
//!
//! 1. Split the byte stream on `\n`, `\r`, and ANSI cursor-positioning /
//!    erase escapes (so dynamic redraws and TTY-mode "frame" updates become
//!    chronological append-only lines, à la `concurrently` / `npm-run-all`
//!    / `pnpm run --recursive`).
//! 2. Strip non-SGR ANSI escapes (cursor movement, erase-line, OSC) while
//!    preserving SGR color/style codes.
//! 3. Trim layout-padding whitespace from each line (Docker Compose's
//!    progress UI etc. leaves behind alignment spaces once cursor escapes
//!    are stripped).
//! 4. Send each completed prefixed line to a single dedicated writer thread
//!    via an MPSC channel. The writer thread is the only thread that ever
//!    touches the real stdout/stderr — guaranteeing no two lines from
//!    different pumps can interleave at the byte level. Per-write
//!    `std::io::stdout().lock()` mutex turns out to be insufficient on
//!    Windows console output: WriteConsoleW exhibits non-atomicity for
//!    concurrent multi-threaded writes (manifesting as mid-line
//!    character-level interleaving — e.g. one pump's `\r\n` showing up
//!    inside another pump's content, rendered by the terminal as CP437
//!    glyphs `♪◙` because the bytes ended up mid-stream).
//!
//! Lines end with `\r\n` on Windows (avoids "growing-indent staircase" when
//! VT processing is enabled and treats bare `\n` as a strict line-feed
//! without carriage return) and `\n` on Unix.

use std::io::Read;
#[cfg(not(windows))]
use std::io::Write;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};
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
pub(crate) struct LineState {
	pub(crate) line: Vec<u8>,
	/// Whether the previous byte was `\r`. Used to swallow the `\n` of a
	/// `\r\n` pair so we don't emit a spurious empty line after the CR
	/// already triggered emission.
	prev_cr: bool,
}

pub(crate) fn process_chunk(chunk: &[u8], state: &mut LineState, emit: &mut impl FnMut(&[u8])) {
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
	dispatch_emit(stream, out);
}

/// A single line ready to be written to its target stream. Sent over the
/// MPSC channel from any number of pump threads to the dedicated writer
/// thread, which drains the channel and writes one message at a time —
/// guaranteeing that no two lines from different pumps can interleave at
/// the byte level. This is critical on Windows console output, where the
/// per-write `std::io::stdout().lock()` mutex turns out to be insufficient
/// in practice: WriteConsoleW and the Windows console driver have observed
/// non-atomicity for concurrent multi-threaded writes (manifesting as
/// mid-line character-level interleaving — e.g. one pump's `\r\n` showing
/// up inside another pump's content, rendered as CP437 glyphs `♪◙`).
enum OutputMessage {
	Line { stream: OutputStream, bytes: Vec<u8> },
	Flush(mpsc::Sender<()>),
}

fn dispatch_emit(stream: &OutputStream, bytes: Vec<u8>) {
	#[cfg(test)]
	if let OutputStream::Capture(buf) = stream {
		// Capture sinks bypass the writer thread entirely so unit tests can
		// run synchronously and don't have to flush through the global
		// channel. Acquiring the buffer's mutex serializes test writes.
		let mut g = buf.lock().unwrap();
		g.extend_from_slice(&bytes);
		return;
	}
	let _ = writer_sender().send(OutputMessage::Line {
		stream: stream.clone(),
		bytes,
	});
}

/// Block until the writer thread has drained every queued message up to
/// this call. Used by pump threads after they finish reading their stream,
/// so the parent's wait loop sees all output before reporting the child
/// done.
pub(crate) fn flush_writer_thread() {
	let (tx, rx) = mpsc::channel();
	if writer_sender().send(OutputMessage::Flush(tx)).is_ok() {
		let _ = rx.recv();
	}
}

/// One-time-initialized handle to the global writer thread's input channel.
/// The first call spawns the writer thread; all subsequent calls return the
/// same `Sender`. The thread runs for the lifetime of the process — it
/// never terminates because there's no clean shutdown signal that wouldn't
/// race with late-arriving pump output.
fn writer_sender() -> &'static Sender<OutputMessage> {
	static SENDER: OnceLock<Sender<OutputMessage>> = OnceLock::new();
	SENDER.get_or_init(|| {
		let (tx, rx) = mpsc::channel::<OutputMessage>();
		thread::Builder::new()
			.name("runfile-output-writer".to_string())
			.spawn(move || {
				while let Ok(msg) = rx.recv() {
					match msg {
						OutputMessage::Line { stream, bytes } => {
							write_to_stream(&stream, &bytes);
						}
						OutputMessage::Flush(ack) => {
							let _ = ack.send(());
						}
					}
				}
			})
			.expect("spawn output writer thread");
		tx
	})
}

fn write_to_stream(stream: &OutputStream, bytes: &[u8]) {
	#[cfg(test)]
	if let OutputStream::Capture(buf) = stream {
		let mut g = buf.lock().unwrap();
		g.extend_from_slice(bytes);
		return;
	}
	#[cfg(windows)]
	{
		windows_write::write_to_stream_windows(stream, bytes);
	}
	#[cfg(not(windows))]
	{
		match stream {
			OutputStream::Stdout => {
				let stdout = std::io::stdout();
				let mut h = stdout.lock();
				let _ = h.write_all(bytes);
				let _ = h.flush();
			}
			OutputStream::Stderr => {
				let stderr = std::io::stderr();
				let mut h = stderr.lock();
				let _ = h.write_all(bytes);
				let _ = h.flush();
			}
			#[cfg(test)]
			OutputStream::Capture(_) => unreachable!("captured above"),
		}
	}
}

/// Direct Windows console / pipe writes that bypass Rust's `Stdout` /
/// `Stderr` `LineWriter` buffering and re-assert the console mode on every
/// call. This is necessary because:
///
/// 1. Windows children (notably `wsl.exe` and tools running inside it) can
///    flip the parent console's mode flags behind our back —
///    `ENABLE_VIRTUAL_TERMINAL_PROCESSING` and `ENABLE_PROCESSED_OUTPUT`
///    can both end up cleared mid-run, causing subsequent writes to render
///    `\x1b` and `\r\n` as literal CP437 glyphs (`←`, `♪`, `◙`).
///
/// 2. The Rust `Stdout` `LineWriter` introduces an internal byte buffer
///    that, under high write throughput from a single thread to a Windows
///    console handle, has been observed to allow byte-level interleaving
///    between adjacent `write_all` calls — even when serialized through a
///    single writer thread. Going straight to `WriteConsoleW` /
///    `WriteFile` avoids that buffer and lands the entire prefixed line in
///    one OS call.
#[cfg(windows)]
mod windows_write {
	use super::OutputStream;
	use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
	use windows_sys::Win32::Storage::FileSystem::WriteFile;
	use windows_sys::Win32::System::Console::{
		GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode, WriteConsoleW,
	};

	const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
	const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

	pub(super) fn write_to_stream_windows(stream: &OutputStream, bytes: &[u8]) {
		let handle_id = match stream {
			OutputStream::Stdout => STD_OUTPUT_HANDLE,
			OutputStream::Stderr => STD_ERROR_HANDLE,
			#[cfg(test)]
			OutputStream::Capture(_) => return,
		};
		unsafe {
			let handle: HANDLE = GetStdHandle(handle_id);
			if handle.is_null() || handle == INVALID_HANDLE_VALUE {
				return;
			}
			let mut mode: u32 = 0;
			let is_console = GetConsoleMode(handle, &mut mode) != 0;
			if is_console {
				// Some descendant (wsl, docker, etc.) may have flipped these
				// flags off. Re-assert on every write so VT escapes and CRLF
				// keep working. Cheap (one syscall) and correct.
				let desired = mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
				if desired != mode {
					let _ = SetConsoleMode(handle, desired);
				}
				// WriteConsoleW takes UTF-16. ASCII (and our ANSI escapes)
				// encode 1:1; non-ASCII content from children gets a lossy
				// UTF-8 → UTF-16 transcode. The whole line lands in a single
				// `WriteConsoleW` call, atomic w.r.t. other console writes.
				let utf16: Vec<u16> = String::from_utf8_lossy(bytes).encode_utf16().collect();
				let mut written: u32 = 0;
				let _ = WriteConsoleW(
					handle,
					utf16.as_ptr(),
					utf16.len() as u32,
					&mut written,
					std::ptr::null_mut(),
				);
			} else {
				// Not a console — pipe, file, or NUL. WriteFile is atomic for
				// small (< PIPE_BUF) writes which our prefixed lines almost
				// always are.
				let mut written: u32 = 0;
				let _ = WriteFile(
					handle,
					bytes.as_ptr(),
					bytes.len() as u32,
					&mut written,
					std::ptr::null_mut(),
				);
			}
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
pub(crate) fn strip_cursor_escapes(input: &[u8]) -> Vec<u8> {
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

/// Build the prefix string for a parallel leaf's output. `label` is the text
/// shown inside the brackets (a command snippet or resolved `@target` call);
/// `step` only drives the cycling color so adjacent leaves stay visually
/// distinct. Honors `NO_COLOR` for plain output in non-terminal contexts.
pub(crate) fn format_parallel_prefix(step: usize, label: &str) -> String {
	if std::env::var_os("NO_COLOR").is_some() {
		format!("[{label}] ")
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
		format!("{color}[{label}]{RESET} ")
	}
}

/// Build the bracket label for a raw shell leaf: the resolved command
/// truncated to at most 12 characters (Unicode scalars, not bytes), taken
/// from the first non-empty line and with surrounding whitespace trimmed so
/// the prefix stays compact and readable.
pub(crate) fn shell_prefix_label(substituted: &str) -> String {
	let first_line = substituted.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
	first_line.chars().take(12).collect()
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
