use runfile_parser::{ExtendStdio, StdioStream};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Manages background threads that tail log files and route complete lines
/// to stdout or stderr.
pub(crate) struct StdioTailerSet {
	stop: Arc<AtomicBool>,
	handles: Vec<JoinHandle<()>>,
}

impl StdioTailerSet {
	/// Start tailing all configured log files. Each file gets its own thread.
	/// Files that don't exist yet are polled until they appear or the tailer is stopped.
	/// Only complete lines (terminated by `\n`) are forwarded.
	pub fn start(entries: &[ExtendStdio], working_dir: &Path) -> Self {
		let stop = Arc::new(AtomicBool::new(false));
		let mut handles = Vec::with_capacity(entries.len());

		for entry in entries {
			let path = resolve_path(&entry.from_file, working_dir);
			let stream = entry.stream.clone();
			let stop_flag = Arc::clone(&stop);

			let handle = thread::spawn(move || {
				tail_file(path, stream, stop_flag);
			});
			handles.push(handle);
		}

		Self { stop, handles }
	}

	/// Signal all tailer threads to stop and wait for them to finish.
	/// Each thread does one final read to flush remaining complete lines.
	pub fn stop(self) {
		self.stop.store(true, Ordering::Relaxed);
		for handle in self.handles {
			let _ = handle.join();
		}
	}
}

fn resolve_path(from_file: &str, working_dir: &Path) -> PathBuf {
	// Expand ~ and ~/ to the user's home directory
	let expanded = if from_file == "~" {
		dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
	} else if let Some(rest) = from_file.strip_prefix("~/").or_else(|| from_file.strip_prefix("~\\")) {
		match dirs::home_dir() {
			Some(home) => home.join(rest),
			None => PathBuf::from(from_file),
		}
	} else {
		PathBuf::from(from_file)
	};

	if expanded.is_absolute() {
		expanded
	} else {
		working_dir.join(expanded)
	}
}

fn tail_file(path: PathBuf, stream: StdioStream, stop: Arc<AtomicBool>) {
	// Record the initial file size so we only emit new content.
	let initial_pos = File::open(&path)
		.and_then(|f| f.metadata().map(|m| m.len()))
		.unwrap_or(0);

	let mut pos = initial_pos;
	let mut line_buf = String::new();

	while !stop.load(Ordering::Relaxed) {
		pos = read_new_lines(&path, pos, &mut line_buf, &stream);
		thread::sleep(Duration::from_millis(50));
	}

	// Final flush: read any remaining data one more time
	read_new_lines(&path, pos, &mut line_buf, &stream);

	// If there's a trailing incomplete line after stop, emit it anyway
	// so nothing is silently lost.
	if !line_buf.is_empty() {
		line_buf.push('\n');
		write_to_stream(&stream, &line_buf);
	}
}

/// Read new complete lines from `path` starting at byte offset `pos`.
/// Incomplete lines (no trailing newline) are kept in `line_buf` for the next call.
/// Returns the new file position.
pub(crate) fn read_new_lines(path: &PathBuf, pos: u64, line_buf: &mut String, stream: &StdioStream) -> u64 {
	let file = match File::open(path) {
		Ok(f) => f,
		Err(_) => return pos, // File doesn't exist yet or is inaccessible
	};

	let file_len = match file.metadata() {
		Ok(m) => m.len(),
		Err(_) => return pos,
	};

	// If file was truncated/rotated, reset to beginning
	let seek_pos = if file_len < pos { 0 } else { pos };

	let mut reader = BufReader::new(file);
	if reader.seek(SeekFrom::Start(seek_pos)).is_err() {
		return pos;
	}

	let mut new_pos = seek_pos;
	let mut buf: Vec<u8> = Vec::new();

	loop {
		buf.clear();
		// Read BYTES up to and including the next `\n` (or EOF). Using
		// `read_line` into a `String` returns `Err(InvalidData)` on a non-UTF-8
		// byte, and the previous `Err(_) => break` arm left `new_pos` unadvanced
		// — so every subsequent poll re-hit the same offset and the tailer
		// stalled forever, never emitting later lines. `read_until` is byte-based
		// and never errors on invalid UTF-8; we lossily decode for display. `\n`
		// (0x0A) never appears inside a multi-byte UTF-8 sequence, so a complete
		// line always ends on a character boundary.
		match reader.read_until(b'\n', &mut buf) {
			Ok(0) => break, // EOF
			Ok(n) => {
				new_pos += n as u64;
				let text = String::from_utf8_lossy(&buf);
				if buf.ends_with(b"\n") {
					// Complete line — prepend any buffered partial content
					if !line_buf.is_empty() {
						let mut full = std::mem::take(line_buf);
						full.push_str(&text);
						write_to_stream(stream, &full);
					} else {
						write_to_stream(stream, &text);
					}
				} else {
					// Incomplete line — buffer it
					line_buf.push_str(&text);
				}
			}
			Err(_) => break, // genuine IO error — retry on the next poll
		}
	}

	new_pos
}

fn write_to_stream(stream: &StdioStream, data: &str) {
	match stream {
		StdioStream::Stdout => {
			let mut out = std::io::stdout().lock();
			let _ = out.write_all(data.as_bytes());
			let _ = out.flush();
		}
		StdioStream::Stderr => {
			let mut err = std::io::stderr().lock();
			let _ = err.write_all(data.as_bytes());
			let _ = err.flush();
		}
	}
}
