use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Whether a force-kill guard is currently active.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Number of live `ForceKillGuard`s. `forceKillOnSigInt` targets dispatched
/// concurrently (e.g. as `@dep`s from a `parallel: true` parent) each create a
/// guard; without refcounting they raced on the process-global handler / PID
/// state — the first guard to drop set `ACTIVE=false` and uninstalled the
/// handler while a sibling was still running. Only the FIRST guard installs and
/// only the LAST uninstalls, so concurrent guards compose and share one handler
/// and one PID registry (a Ctrl+C then kills every tracked child, which is the
/// desired behavior).
static GUARD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Serializes every test that touches the process-global force-kill state
/// (`ACTIVE` / `GUARD_COUNT` / the PID registry), across both this module's
/// tests and the platform submodule's.
#[cfg(test)]
static FORCE_KILL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that sets up a CTRL+C / SIGINT handler which forcefully kills
/// all tracked child processes (and their descendants) when triggered.
///
/// On Windows: uses a Job Object — all children and grandchildren are assigned
/// to the job. CTRL+C handler calls `TerminateJobObject` to kill the entire tree.
///
/// On Unix: tracks child PIDs and sends SIGKILL on SIGINT.
pub(crate) struct ForceKillGuard {
	_private: (),
}

impl ForceKillGuard {
	/// Create a new guard and install the signal handler. Refcounted: only the
	/// first live guard performs setup + install (see [`GUARD_COUNT`]).
	pub fn new() -> Self {
		if GUARD_COUNT.fetch_add(1, Ordering::SeqCst) == 0 {
			platform::setup();
			ACTIVE.store(true, Ordering::SeqCst);
			platform::install_handler();
		}
		Self { _private: () }
	}

	/// Register a spawned child so it will be killed on SIGINT.
	pub fn add_child(&self, child: &Child) {
		platform::add_child(child);
	}

	/// Configure `cmd` (before spawning) so the child can be killed together
	/// with its descendants. On Unix this places the child in its OWN process
	/// group (`setpgid`), so the handler's `kill(-pid, SIGKILL)` reaches the
	/// child's grandchildren (the comment's promise, which didn't hold while
	/// children stayed in the parent's group). On Windows the Job Object already
	/// covers descendants, so this is a no-op. MUST be called before `spawn`.
	pub fn prepare_command(&self, cmd: &mut std::process::Command) {
		platform::prepare_command(cmd);
	}
}

impl Drop for ForceKillGuard {
	fn drop(&mut self) {
		// Only the last live guard uninstalls + tears down.
		if GUARD_COUNT.fetch_sub(1, Ordering::SeqCst) == 1 {
			ACTIVE.store(false, Ordering::SeqCst);
			platform::uninstall_handler();
			platform::teardown();
		}
	}
}

// ──── Windows implementation ────

#[cfg(windows)]
mod platform {
	use super::{Mutex, ACTIVE};
	use std::process::Child;
	use std::sync::atomic::Ordering;

	/// Wrapper to make raw HANDLE Send+Sync for use in a static Mutex.
	/// Safety: Job Object handles are safe to use from any thread.
	struct SendHandle(windows_sys::Win32::Foundation::HANDLE);
	unsafe impl Send for SendHandle {}

	static JOB_HANDLE: Mutex<Option<SendHandle>> = Mutex::new(None);

	pub(super) fn setup() {
		let handle =
			unsafe { windows_sys::Win32::System::JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
		if !handle.is_null() {
			*JOB_HANDLE.lock().unwrap() = Some(SendHandle(handle));
		}
	}

	pub(super) fn teardown() {
		if let Some(SendHandle(handle)) = JOB_HANDLE.lock().unwrap().take() {
			unsafe {
				windows_sys::Win32::Foundation::CloseHandle(handle);
			}
		}
	}

	pub(super) fn add_child(child: &Child) {
		if let Some(SendHandle(handle)) = &*JOB_HANDLE.lock().unwrap() {
			use std::os::windows::io::AsRawHandle;
			unsafe {
				windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
					*handle,
					child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
				);
			}
		}
	}

	/// No-op on Windows: the Job Object already tracks descendants.
	pub(super) fn prepare_command(_cmd: &mut std::process::Command) {}

	pub(super) fn install_handler() {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 1);
		}
	}

	pub(super) fn uninstall_handler() {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ctrl_handler), 0);
		}
	}

	unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> i32 {
		if !ACTIVE.load(Ordering::Relaxed) {
			return 0; // Not active, pass to next handler
		}

		// Terminate all processes in the job
		if let Ok(guard) = JOB_HANDLE.lock() {
			if let Some(SendHandle(handle)) = &*guard {
				windows_sys::Win32::System::JobObjects::TerminateJobObject(*handle, 1);
			}
		}

		// Return TRUE to suppress the default handler (which would kill us
		// before we can reap the children and report the result).
		1
	}
}

// ──── Unix implementation ────

#[cfg(unix)]
mod platform {
	use super::{Mutex, ACTIVE};
	use std::process::Child;
	use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

	/// Maximum number of child PIDs tracked for force-kill. A fixed, lock-free
	/// array (rather than a `Mutex<Vec<u32>>`) is REQUIRED because the SIGINT
	/// handler reads it: `Mutex::lock()` is a `pthread_mutex_lock`, which is NOT
	/// async-signal-safe and self-deadlocks if the signal lands while the main
	/// thread already holds the lock inside `add_child`. Atomic loads and
	/// `libc::kill` are async-signal-safe. 1024 comfortably covers the intended
	/// use (GUI-subsystem apps that ignore console CTRL+C); children past the cap
	/// are simply not force-killed (best-effort, same as any untracked process).
	const MAX_TRACKED_PIDS: usize = 1024;
	static CHILD_PIDS: [AtomicI32; MAX_TRACKED_PIDS] = [const { AtomicI32::new(0) }; MAX_TRACKED_PIDS];
	static CHILD_COUNT: AtomicUsize = AtomicUsize::new(0);
	static PREV_HANDLER: Mutex<Option<libc::sighandler_t>> = Mutex::new(None);

	pub(super) fn setup() {
		CHILD_COUNT.store(0, Ordering::SeqCst);
	}

	pub(super) fn teardown() {
		CHILD_COUNT.store(0, Ordering::SeqCst);
	}

	pub(super) fn add_child(child: &Child) {
		record_pid(child.id() as i32);
	}

	/// Place the child in its OWN process group so `kill(-pid, SIGKILL)` in the
	/// SIGINT handler reaches the child AND its descendants. Must be applied
	/// before spawn.
	pub(super) fn prepare_command(cmd: &mut std::process::Command) {
		use std::os::unix::process::CommandExt;
		// SAFETY: `setpgid` is async-signal-safe and only reparents the
		// about-to-exec child into a fresh process group; it touches no shared
		// parent state. A failure (very unlikely) is ignored — worst case is the
		// prior best-effort behavior (direct child killed, group kill a no-op).
		unsafe {
			cmd.pre_exec(|| {
				libc::setpgid(0, 0);
				Ok(())
			});
		}
	}

	/// Lock-free append of a PID into the tracking array. Split out so unit
	/// tests can exercise the tracking logic without spawning real processes.
	fn record_pid(pid: i32) {
		let idx = CHILD_COUNT.fetch_add(1, Ordering::SeqCst);
		if idx < MAX_TRACKED_PIDS {
			CHILD_PIDS[idx].store(pid, Ordering::SeqCst);
		}
	}

	pub(super) fn install_handler() {
		let prev = unsafe { libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t) };
		*PREV_HANDLER.lock().unwrap() = Some(prev);
	}

	pub(super) fn uninstall_handler() {
		if let Some(prev) = PREV_HANDLER.lock().unwrap().take() {
			unsafe {
				libc::signal(libc::SIGINT, prev);
			}
		}
	}

	extern "C" fn sigint_handler(_sig: libc::c_int) {
		if !ACTIVE.load(Ordering::Relaxed) {
			return;
		}

		// Async-signal-safe body: ONLY atomic loads and `libc::kill` — no
		// locking, no allocation. Send SIGKILL to each tracked child process.
		let n = CHILD_COUNT.load(Ordering::SeqCst).min(MAX_TRACKED_PIDS);
		for slot in CHILD_PIDS.iter().take(n) {
			let pid = slot.load(Ordering::SeqCst);
			if pid > 0 {
				unsafe {
					// Kill the process group (negative PID) to catch grandchildren
					libc::kill(-pid, libc::SIGKILL);
					// Also kill the process directly in case it's not a group leader
					libc::kill(pid, libc::SIGKILL);
				}
			}
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;

		fn snapshot_tracked() -> Vec<i32> {
			let n = CHILD_COUNT.load(Ordering::SeqCst).min(MAX_TRACKED_PIDS);
			CHILD_PIDS.iter().take(n).map(|s| s.load(Ordering::SeqCst)).collect()
		}

		#[test]
		fn record_and_reset_pids() {
			let _g = super::super::FORCE_KILL_TEST_LOCK.lock().unwrap();
			setup();
			record_pid(111);
			record_pid(222);
			record_pid(333);
			assert_eq!(snapshot_tracked(), vec![111, 222, 333]);
			teardown();
			assert!(snapshot_tracked().is_empty(), "teardown must reset the tracked count");
		}

		#[test]
		fn record_beyond_cap_is_bounded_and_does_not_panic() {
			let _g = super::super::FORCE_KILL_TEST_LOCK.lock().unwrap();
			setup();
			for i in 0..(MAX_TRACKED_PIDS + 50) {
				record_pid((i as i32) + 1);
			}
			// The read side clamps to the cap — no out-of-bounds indexing.
			let n = CHILD_COUNT.load(Ordering::SeqCst).min(MAX_TRACKED_PIDS);
			assert_eq!(n, MAX_TRACKED_PIDS);
			assert_eq!(snapshot_tracked().len(), MAX_TRACKED_PIDS);
			teardown();
		}
	}
}

#[cfg(test)]
mod guard_tests {
	use super::*;

	// Audit L12: concurrent force-kill targets used to clobber the shared
	// handler/PID state — the first guard to drop deactivated force-kill for a
	// still-running sibling. The refcount keeps `ACTIVE` true until the LAST
	// guard drops.
	#[test]
	fn refcount_keeps_active_until_last_guard_drops() {
		let _g = FORCE_KILL_TEST_LOCK.lock().unwrap();
		assert!(!ACTIVE.load(Ordering::SeqCst), "no guards → inactive");
		let a = ForceKillGuard::new();
		assert!(ACTIVE.load(Ordering::SeqCst), "first guard activates");
		let b = ForceKillGuard::new();
		assert!(ACTIVE.load(Ordering::SeqCst), "still active with two guards");
		drop(a);
		assert!(ACTIVE.load(Ordering::SeqCst), "still active while one guard remains");
		drop(b);
		assert!(!ACTIVE.load(Ordering::SeqCst), "inactive after the last guard drops");
	}

	#[test]
	fn single_guard_lifecycle() {
		let _g = FORCE_KILL_TEST_LOCK.lock().unwrap();
		assert!(!ACTIVE.load(Ordering::SeqCst));
		{
			let _guard = ForceKillGuard::new();
			assert!(ACTIVE.load(Ordering::SeqCst));
		}
		assert!(!ACTIVE.load(Ordering::SeqCst));
	}

	// Audit L13: `prepare_command` adds a `setpgid` pre_exec so the child is its
	// own process-group leader (letting the group-kill reach grandchildren).
	// Verify it doesn't break normal execution.
	#[cfg(unix)]
	#[test]
	fn prepared_command_still_spawns_and_runs() {
		let _g = FORCE_KILL_TEST_LOCK.lock().unwrap();
		let guard = ForceKillGuard::new();
		let mut cmd = std::process::Command::new("true");
		guard.prepare_command(&mut cmd);
		let status = cmd.status().expect("child should spawn");
		assert!(status.success(), "setpgid pre_exec must not break normal execution");
		drop(guard);
	}
}
