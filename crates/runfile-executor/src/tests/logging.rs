use crate::logging::{ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, console_mode_update};

/// Both managed bits OR'd together — the end state we always converge to.
const BOTH: u32 = ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;

// A handful of plausible "unrelated" Windows console mode bits that our
// helper must never touch (ENABLE_WRAP_AT_EOL_OUTPUT, ENABLE_LVB_GRID, an
// arbitrary high bit).
const UNRELATED: u32 = 0x0002 | 0x0008 | 0x8000_0000;

#[test]
fn flag_values_match_win32_constants() {
	// Guard the magic numbers against accidental edits — these are the
	// documented Win32 console mode bits.
	assert_eq!(ENABLE_PROCESSED_OUTPUT, 0x0001);
	assert_eq!(ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0x0004);
}

#[test]
fn enables_both_flags_from_zero() {
	// The exact state after `wsl.exe` clears the console mode: neither flag
	// set. We must turn both back on. This is the core regression case —
	// the bug manifested as both ANSI escapes (`←[..m`) and `\n` (`◙`)
	// rendering literally because both bits were off and never restored.
	assert_eq!(console_mode_update(0), Some(BOTH));
}

#[test]
fn adds_processed_output_when_only_vt_present() {
	// The previous implementation set ONLY VT, leaving `\n` rendering as
	// `◙`. Ensure the missing ENABLE_PROCESSED_OUTPUT bit is added.
	assert_eq!(console_mode_update(ENABLE_VIRTUAL_TERMINAL_PROCESSING), Some(BOTH));
}

#[test]
fn adds_vt_when_only_processed_present() {
	assert_eq!(console_mode_update(ENABLE_PROCESSED_OUTPUT), Some(BOTH));
}

#[test]
fn no_update_when_both_already_set() {
	// Already correct → no SetConsoleMode syscall needed.
	assert_eq!(console_mode_update(BOTH), None);
}

#[test]
fn no_update_when_both_set_alongside_unrelated_bits() {
	assert_eq!(console_mode_update(BOTH | UNRELATED), None);
}

#[test]
fn no_update_when_all_bits_set() {
	// All bits set already includes our two → nothing to do.
	assert_eq!(console_mode_update(u32::MAX), None);
}

#[test]
fn preserves_unrelated_bits_while_adding_ours() {
	// We only OR our two bits in — every other mode flag must survive.
	let got = console_mode_update(UNRELATED).expect("should need an update");
	assert_eq!(got, UNRELATED | BOTH);
	assert_eq!(got & UNRELATED, UNRELATED, "unrelated bits were cleared");
}

#[test]
fn is_idempotent() {
	// Applying the computed mode again must report "no change" — proves the
	// per-call re-assert converges and won't thrash SetConsoleMode.
	let first = console_mode_update(0).expect("first update");
	assert_eq!(console_mode_update(first), None);

	let from_partial = console_mode_update(UNRELATED).expect("update from partial");
	assert_eq!(console_mode_update(from_partial), None);
}

#[test]
fn only_ever_sets_our_two_bits_and_clears_nothing() {
	// Exhaustive-ish sweep: for any input, the delta must be a subset of
	// our two managed bits, and no existing bit may be cleared.
	for current in [
		0u32,
		0x1,
		0x2,
		0x4,
		0x5,
		0x7,
		0xFF,
		0xFF00,
		0x8000_0000,
		UNRELATED,
		u32::MAX,
	] {
		if let Some(desired) = console_mode_update(current) {
			let changed = desired ^ current;
			assert_eq!(changed & !BOTH, 0, "unexpected bits changed for {current:#x}");
			assert_eq!(desired & current, current, "cleared bits for {current:#x}");
			// Whenever an update is returned, both target bits end up set.
			assert_eq!(desired & BOTH, BOTH, "both flags not set for {current:#x}");
		} else {
			// `None` is only valid when both bits were already present.
			assert_eq!(current & BOTH, BOTH, "spurious None for {current:#x}");
		}
	}
}
