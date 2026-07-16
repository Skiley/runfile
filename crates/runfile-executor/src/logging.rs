use runfile_parser::CommandSpec;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// ANSI escape codes — these work in all modern terminals including
// bash, zsh, fish, PowerShell 5.1+, PowerShell 7+, Windows Terminal,
// and cmd.exe on Windows 10+.
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";

// Windows console mode bits we require for our log output. Defined locally
// (rather than imported from `windows-sys`) so the pure mode-update helper
// below compiles and is unit-testable on every platform, not just Windows.
// `allow(dead_code)` on non-Windows because only `console_mode_update` (and
// the tests) reference them there.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

/// Given the current Windows console mode, return the mode that should be set,
/// or `None` if the bits we need are already on (so the caller can skip the
/// `SetConsoleMode` syscall).
///
/// We require BOTH:
/// - `ENABLE_PROCESSED_OUTPUT` — so `\n` line terminators act as newlines
///   instead of being rendered as the CP437 glyph `◙`.
/// - `ENABLE_VIRTUAL_TERMINAL_PROCESSING` — so ANSI color/style escapes render
///   as colors instead of as literal `←[..m` text.
///
/// We only ever OR these two bits in — unrelated mode flags are preserved.
/// Pure bit math, extracted from [`enable_ansi_support`] so the behaviour is
/// unit-testable without touching a real console handle.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn console_mode_update(current: u32) -> Option<u32> {
	let desired = current | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
	if desired == current { None } else { Some(desired) }
}

/// Tracks the global step number across an entire run, so that nested
/// `@target` invocations and `when:` blocks share one continuous
/// `(N/total)` counter instead of restarting per call.
///
/// The total is computed up front by walking the dependency tree (see
/// `count_target_leaves` in `runner.rs`). Conditional `when: failure` /
/// `when: always` blocks inflate the total — actual execution may stop
/// before reaching them, in which case the last shown step number will
/// be lower than the total. That trade-off is acceptable.
///
/// Uses atomics + `Arc` so the counter can be shared across threads when
/// `parallel: true` targets spawn dependency invocations on worker threads.
/// `Clone` is shallow (cloning shares the same atomics) so all clones
/// observe the same `(N, total)` state.
#[derive(Debug, Clone)]
pub struct StepCounter {
	current: Arc<AtomicUsize>,
	total: Arc<AtomicUsize>,
}

impl StepCounter {
	pub fn new(total: usize) -> Self {
		Self {
			current: Arc::new(AtomicUsize::new(0)),
			total: Arc::new(AtomicUsize::new(total)),
		}
	}

	/// Advance the counter and return the (1-indexed step, total) pair
	/// to use in log output. Thread-safe.
	pub fn next_step(&self) -> (usize, usize) {
		let n = self.current.fetch_add(1, Ordering::SeqCst) + 1;
		let t = self.total.load(Ordering::SeqCst);
		(n, t)
	}

	pub fn total(&self) -> usize {
		self.total.load(Ordering::SeqCst)
	}

	/// Bump the total step count. Used at runtime when a `for glob` /
	/// `for shell` iterator expands to more iterations than the planning
	/// pass estimated, or when nested control flow inflates the total
	/// beyond the static estimate. Counts are always monotonically
	/// non-decreasing — this never shrinks the total.
	pub fn add_to_total(&self, n: usize) {
		self.total.fetch_add(n, Ordering::SeqCst);
	}

	/// Reduce the total step count. Called when a shell template was
	/// counted by the static [`crate::control_flow::count_leaves`] pass
	/// but turns out to be a runtime no-op (typically a line that
	/// resolves to whitespace — e.g. one consisting only of
	/// `{{ define(...) }}` calls — which is dropped from execution).
	/// Without this, the visible `(N/total)` ratio would drift because
	/// the current counter never advances for the skipped step while
	/// the total still includes it. Saturating: never underflows.
	pub fn subtract_from_total(&self, n: usize) {
		let _ = self
			.total
			.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |t| Some(t.saturating_sub(n)));
	}
}

/// Determine whether logging is enabled for a command.
/// Defaults to false if not set.
pub fn is_logging_enabled(spec: &CommandSpec) -> bool {
	spec.logging.unwrap_or(false)
}

/// Print a command that is about to be executed, in a formatted style.
/// `step` is the 1-indexed global step number; `total` is the total
/// step count for the entire run.
pub fn log_command(command: &str, step: usize, total: usize) {
	// Enable ANSI support on Windows (needed for older cmd.exe / PowerShell 5)
	#[cfg(windows)]
	enable_ansi_support();

	if total > 1 {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}({step}/{total}){RESET} {BOLD}{command}{RESET}");
	} else {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {BOLD}{command}{RESET}");
	}
}

/// Print a command that is about to be executed in parallel mode.
/// `step` is the 1-indexed global step number; `total` is the total
/// step count for the entire run.
pub fn log_parallel_command(command: &str, step: usize, total: usize) {
	#[cfg(windows)]
	enable_ansi_support();

	if total > 1 {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}({step}/{total}) [parallel]{RESET} {BOLD}{command}{RESET}");
	} else {
		eprintln!("{BOLD}{CYAN}[runfile]{RESET} {DIM}[parallel]{RESET} {BOLD}{command}{RESET}");
	}
}

/// Print a summary of which leaves failed in a parallel batch and with what
/// exit code. Called at the end of `run_parallel_batch` whenever at least
/// one leaf failed — even when `ignoreErrors` is set, because the whole
/// point is to surface failures that would otherwise get swallowed by the
/// interleaved parallel output.
///
/// Each entry is `(label, detail)` where `label` is the leaf identity
/// (raw shell template, or `@target args...` for dispatched targets) and
/// `detail` is a short human-readable phrase describing how it failed
/// (e.g. `exit code 1`, `terminated by signal`, `error: ...`).
pub fn log_parallel_failure_summary(failures: &[(String, String)]) {
	if failures.is_empty() {
		return;
	}
	#[cfg(windows)]
	enable_ansi_support();
	let n = failures.len();
	let plural = if n == 1 { "" } else { "s" };
	eprintln!("{BOLD}{CYAN}[runfile]{RESET} {BOLD}{RED}[parallel] {n} command{plural} failed:{RESET}");
	for (label, detail) in failures {
		eprintln!("  {BOLD}{RED}-{RESET} {BOLD}{label}{RESET} {DIM}—{RESET} {RED}{detail}{RESET}");
	}
}

/// Format a duration for human display.
fn format_duration(d: Duration) -> String {
	let secs = d.as_secs_f64();
	if secs < 1.0 {
		format!("{:.0}ms", d.as_millis())
	} else if secs < 60.0 {
		format!("{secs:.1}s")
	} else {
		let mins = secs as u64 / 60;
		let remaining = secs - (mins as f64 * 60.0);
		format!("{mins}m {remaining:.1}s")
	}
}

/// Print timing information for a single command.
pub fn log_command_timing(duration: Duration) {
	#[cfg(windows)]
	enable_ansi_support();
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} {DIM}completed in {}{RESET}",
		format_duration(duration),
	);
}

/// Print timing information for a target.
pub fn log_target_timing(target_name: &str, duration: Duration) {
	#[cfg(windows)]
	enable_ansi_support();
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} target \"{BOLD}{target_name}{RESET}\" completed in {BOLD}{}{RESET}",
		format_duration(duration),
	);
}

/// Print total timing information.
pub fn log_total_timing(duration: Duration) {
	#[cfg(windows)]
	enable_ansi_support();
	eprintln!(
		"{BOLD}{CYAN}[runfile]{RESET} total: {BOLD}{}{RESET}",
		format_duration(duration),
	);
}

/// On Windows, (re-)assert the console mode our log output depends on:
/// `ENABLE_PROCESSED_OUTPUT` + `ENABLE_VIRTUAL_TERMINAL_PROCESSING`, on BOTH
/// the stdout and stderr console handles.
///
/// This MUST run on every log call, not once. Child processes — notably
/// `wsl.exe` and tools running inside it — clear these flags on the shared
/// console mid-run. After that, our ANSI escapes render as literal `←[..m`
/// and our `\n` line terminators render as the CP437 glyph `◙` until the
/// flags are restored. A `Once`-gated, set-only-VT, stderr-only version (the
/// previous implementation) could never recover from this: the first log line
/// armed it, then WSL disarmed it, and every later call was a no-op.
///
/// Both handles need it: our `[runfile]` log lines go to stderr, but the
/// inherited stdout of sequential commands (e.g. a trailing `echo`) shares
/// the same console and is also affected. Re-asserting per call is cheap (a
/// couple of syscalls) and mirrors the strategy the parallel output writer
/// uses in `parallel_output::write_to_stream_windows`.
#[cfg(windows)]
fn enable_ansi_support() {
	use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
	use windows_sys::Win32::System::Console::{
		GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
	};

	for handle_id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
		unsafe {
			let handle = GetStdHandle(handle_id);
			if handle.is_null() || handle == INVALID_HANDLE_VALUE {
				continue;
			}
			let mut mode: u32 = 0;
			// GetConsoleMode fails for non-console handles (pipes, files, NUL).
			// Those need no fix — the bytes pass through verbatim.
			if GetConsoleMode(handle, &mut mode) == 0 {
				continue;
			}
			if let Some(desired) = console_mode_update(mode) {
				let _ = SetConsoleMode(handle, desired);
			}
		}
	}
}
