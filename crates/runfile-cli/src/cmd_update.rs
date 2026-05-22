//! `run :update` — self-update by re-running the published install script.
//!
//! Rather than re-implement download / unpack / binary-swap logic in Rust,
//! `:update` invokes the exact same install one-liner documented in the
//! README, scoped (via `RUNFILE_INSTALL_DIR`) to the directory the running
//! binary already lives in. This means there is a single source of truth for
//! "how runfile gets onto a machine" — the install scripts — and the updater
//! inherits every platform/arch detail they already handle.
//!
//! npm-managed installs are refused: their binary lives inside `node_modules`
//! and is owned by the package manager, so the right update path is
//! `npm install -g @runfile/cli@latest`, not a sideways overwrite.

use std::path::Path;
use std::process::{self, Command};

#[cfg(not(windows))]
const INSTALL_SH_URL: &str = "https://github.com/Skiley/runfile/releases/latest/download/install.sh";
#[cfg(windows)]
const INSTALL_PS1_URL: &str = "https://github.com/Skiley/runfile/releases/latest/download/install.ps1";

/// How the running binary was installed — determines whether `:update` can
/// manage it.
#[derive(Debug, PartialEq, Eq)]
enum InstallKind {
	/// Binary lives inside a `node_modules` tree → owned by a JS package
	/// manager. We refuse and defer to it.
	Npm,
	/// Installed via the install scripts (or any other standalone copy) →
	/// safe to overwrite in place.
	Standalone,
}

/// Classify an install purely from the executable path. Pure (no I/O) so it's
/// unit-testable. The `node_modules` component is the signal that a JS package
/// manager owns the binary (the npm package extracts to
/// `.../node_modules/@runfile/cli/bin/<platform>/run`).
fn classify_install(exe: &Path) -> InstallKind {
	let in_node_modules = exe.components().any(|c| c.as_os_str() == "node_modules");
	if in_node_modules {
		InstallKind::Npm
	} else {
		InstallKind::Standalone
	}
}

pub fn cmd_update(version: Option<&str>) {
	let exe = match std::env::current_exe() {
		Ok(p) => p,
		Err(e) => {
			eprintln!("Error: could not locate the running executable: {e}");
			process::exit(1);
		}
	};

	if classify_install(&exe) == InstallKind::Npm {
		return update_via_npm(version);
	}

	let install_dir = match exe.parent() {
		Some(dir) => dir.to_path_buf(),
		None => {
			eprintln!(
				"Error: could not determine the install directory from {}",
				exe.display()
			);
			process::exit(1);
		}
	};

	let current = env!("CARGO_PKG_VERSION");
	let requested = version.unwrap_or("latest");
	eprintln!(
		"Updating runfile (current v{current} → {requested}) in {}...",
		install_dir.display()
	);

	let status = run_install_script(&install_dir, version);

	match status {
		Ok(s) if s.success() => {
			// On Windows the old binary was renamed aside to `<exe>.old-<guid>`
			// because it's the running process and can't be overwritten. It's
			// still locked now (we're executing from it), so the soonest it can
			// go away is the next reboot — schedule that. Best-effort; see the
			// fn doc.
			#[cfg(windows)]
			schedule_old_deletion_at_reboot(&exe);
		}
		Ok(s) => {
			eprintln!("Error: the install script exited with status {s}.");
			process::exit(s.code().unwrap_or(1));
		}
		Err(e) => {
			eprintln!("Error: failed to run the install script: {e}");
			process::exit(1);
		}
	}
}

/// Schedule every `<exe>.old-<guid>` aside-file (a previous binary renamed
/// aside by `install.ps1` because it was the running process) for deletion at
/// the next reboot via `MoveFileExW(.., NULL, MOVEFILE_DELAY_UNTIL_REBOOT)`.
///
/// `install.ps1` renames the running binary to a GUID-suffixed name so the
/// rename can never collide with a still-locked leftover, which means there
/// can be more than one aside-file present — we scan the install dir and
/// schedule each.
///
/// The flag records the path in the registry for the Session Manager to delete
/// at boot, so it doesn't matter that the file is currently locked. Best-effort
/// only: the underlying registry write requires administrator rights, so on the
/// common per-user install this silently no-ops — `install.ps1` sweeps any
/// stale aside-files on the next update regardless, so nothing is orphaned
/// permanently in practice.
#[cfg(windows)]
fn schedule_old_deletion_at_reboot(exe: &Path) {
	use std::os::windows::ffi::OsStrExt;
	use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};

	let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|n| n.to_str())) else {
		return;
	};
	let prefix = format!("{name}.old");

	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let fname = entry.file_name();
		let Some(fname) = fname.to_str() else { continue };
		if !fname.starts_with(&prefix) {
			continue;
		}

		let wide: Vec<u16> = entry
			.path()
			.as_os_str()
			.encode_wide()
			.chain(std::iter::once(0))
			.collect();

		// SAFETY: `wide` is a NUL-terminated UTF-16 string living for the
		// duration of the call; a null destination requests deletion rather
		// than a move. The call only registers a pending operation and returns
		// immediately. A zero return means failure (typically
		// ERROR_ACCESS_DENIED without admin), which we intentionally ignore —
		// the next-update sweep is the fallback.
		unsafe {
			MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
		}
	}
}

/// Build the `npm install -g` package spec. npm uses bare semver, but our
/// release tags carry a leading `v` — strip it so `--version v0.19.0` and
/// `--version 0.19.0` both map to `@runfile/cli@0.19.0`. `None` → `@latest`.
fn npm_package_spec(version: Option<&str>) -> String {
	match version {
		Some(v) => format!("@runfile/cli@{}", v.trim_start_matches('v')),
		None => "@runfile/cli@latest".to_string(),
	}
}

/// Update an npm-managed install.
///
/// On Unix we run `npm install -g <spec>` directly — replacing a running
/// binary's file is fine there. On Windows we can't: npm would have to
/// overwrite the currently-running `run.exe` (locked by this process), and
/// since npm owns the extraction we can't apply the rename-aside trick the
/// standalone path uses — a failed overwrite mid-reify can corrupt the global
/// package. So on Windows we print the command for the user to run in a fresh
/// shell, where no `run.exe` is executing.
fn update_via_npm(version: Option<&str>) {
	let spec = npm_package_spec(version);

	#[cfg(windows)]
	{
		eprintln!("run is managed by npm. Update it from a fresh shell with:");
		eprintln!();
		eprintln!("  npm install -g {spec}");
		eprintln!();
		eprintln!("(Auto-update isn't safe here: npm must overwrite the running run.exe, which Windows locks.)");
		process::exit(1);
	}

	#[cfg(not(windows))]
	{
		eprintln!("run is managed by npm — running: npm install -g {spec}");
		match Command::new("npm").args(["install", "-g", &spec]).status() {
			Ok(s) if s.success() => {}
			Ok(s) => {
				eprintln!("Error: `npm install -g {spec}` exited with status {s}.");
				process::exit(s.code().unwrap_or(1));
			}
			Err(e) => {
				eprintln!("Error: failed to run npm: {e}");
				eprintln!("Update manually with: npm install -g {spec}");
				process::exit(1);
			}
		}
	}
}

/// Spawn the platform install script, scoped to `install_dir` via
/// `RUNFILE_INSTALL_DIR`, optionally pinning `version` (a release tag like
/// `v0.19.0`; `None` installs the latest release). Mirrors the README
/// one-liners: `curl … | sh` on Unix, `iwr … | iex` on Windows.
fn run_install_script(install_dir: &Path, version: Option<&str>) -> std::io::Result<process::ExitStatus> {
	#[cfg(windows)]
	{
		// Fetch the install script and run it in-process via `iex` (bypasses
		// execution policy, like the documented installer). Two PS 5.1 gotchas
		// are handled explicitly:
		//   1. `-UseBasicParsing` — without it, Windows PowerShell pipes the
		//      response through the legacy IE DOM engine, which can HANG.
		//   2. `.Content` is a `byte[]` here — GitHub serves release assets as
		//      application/octet-stream — so we decode UTF-8 before `iex`,
		//      otherwise the bytes stringify to "36 69 114 ..." and won't parse.
		// The version is pinned via the RUNFILE_VERSION env var (install.ps1
		// reads it) since this form can't pass positional args; the dir is
		// scoped via RUNFILE_INSTALL_DIR.
		let ps_cmd = format!(
			"$c=(iwr '{INSTALL_PS1_URL}' -UseBasicParsing).Content; \
			 if($c -is [byte[]]){{$c=[Text.Encoding]::UTF8.GetString($c)}}; iex $c"
		);
		let mut cmd = Command::new("powershell");
		cmd.args(["-NoProfile", "-Command", &ps_cmd])
			.env("RUNFILE_INSTALL_DIR", install_dir);
		if let Some(v) = version {
			cmd.env("RUNFILE_VERSION", v);
		}
		cmd.status()
	}

	#[cfg(not(windows))]
	{
		// `sh -s -- <version>` feeds the piped script its positional args;
		// an empty <version> leaves $1 unset so install.sh defaults to latest.
		let ver = version.unwrap_or("");
		let sh_cmd = format!("curl -fsSL {INSTALL_SH_URL} | sh -s -- {ver}");
		Command::new("sh")
			.arg("-c")
			.arg(&sh_cmd)
			.env("RUNFILE_INSTALL_DIR", install_dir)
			.status()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
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
}
