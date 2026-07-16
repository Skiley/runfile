use crate::cmd_run::truncate_preview;

#[test]
fn short_ascii_preview_is_unchanged() {
	assert_eq!(truncate_preview("echo hi", 60), "echo hi");
}

#[test]
fn long_ascii_preview_is_truncated_with_ellipsis() {
	let full = "a".repeat(100);
	let out = truncate_preview(&full, 60);
	assert_eq!(out.chars().count(), 63); // 60 + "..."
	assert!(out.ends_with("..."));
}

#[test]
fn multibyte_preview_does_not_panic_and_truncates_by_char() {
	// Audit L3: a multi-byte character straddling the byte boundary used to
	// panic under `&full[..60]`. Each `é` is 2 bytes, so byte index 60 lands
	// mid-character. Char-safe truncation must not panic.
	let full = "é".repeat(100); // 200 bytes, 100 chars
	let out = truncate_preview(&full, 60);
	assert_eq!(out.chars().count(), 63);
	assert!(out.ends_with("..."));
}

#[test]
fn exactly_max_chars_not_truncated() {
	let full = "x".repeat(60);
	assert_eq!(truncate_preview(&full, 60), full);
}
