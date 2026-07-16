#[cfg(unix)]
use crate::force_kill::platform;
use crate::force_kill::{ACTIVE, ForceKillGuard};
use std::sync::Mutex;
use std::sync::atomic::Ordering;

/// Serializes every test that touches the process-global force-kill state
/// (`ACTIVE` / `GUARD_COUNT` / the PID registry), across both the guard tests
/// and the platform tests.
static FORCE_KILL_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn snapshot_tracked() -> Vec<i32> {
	let n = platform::CHILD_COUNT
		.load(Ordering::SeqCst)
		.min(platform::MAX_TRACKED_PIDS);
	platform::CHILD_PIDS
		.iter()
		.take(n)
		.map(|s| s.load(Ordering::SeqCst))
		.collect()
}

#[cfg(unix)]
#[test]
fn record_and_reset_pids() {
	let _g = FORCE_KILL_TEST_LOCK.lock().unwrap();
	platform::setup();
	platform::record_pid(111);
	platform::record_pid(222);
	platform::record_pid(333);
	assert_eq!(snapshot_tracked(), vec![111, 222, 333]);
	platform::teardown();
	assert!(snapshot_tracked().is_empty(), "teardown must reset the tracked count");
}

#[cfg(unix)]
#[test]
fn record_beyond_cap_is_bounded_and_does_not_panic() {
	let _g = FORCE_KILL_TEST_LOCK.lock().unwrap();
	platform::setup();
	for i in 0..(platform::MAX_TRACKED_PIDS + 50) {
		platform::record_pid((i as i32) + 1);
	}
	// The read side clamps to the cap — no out-of-bounds indexing.
	let n = platform::CHILD_COUNT
		.load(Ordering::SeqCst)
		.min(platform::MAX_TRACKED_PIDS);
	assert_eq!(n, platform::MAX_TRACKED_PIDS);
	assert_eq!(snapshot_tracked().len(), platform::MAX_TRACKED_PIDS);
	platform::teardown();
}

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
