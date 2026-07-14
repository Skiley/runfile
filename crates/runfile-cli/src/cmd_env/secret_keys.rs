use super::*;

/// CI-only path for `:env secret-keys add --key <hex>`.
///
/// Refuses to run outside a CI environment so dev-machine users don't accidentally bake
/// their private key into shell history.
fn add_secret_key_non_interactive(key_hex: &str) {
	if !ci_detect::is_ci() {
		eprintln!(
			"Error: --key is only allowed inside a CI environment.\n\
			 Run `run :env secret-keys add` interactively on dev machines so the key\n\
			 doesn't end up in your shell history."
		);
		process::exit(1);
	}

	let key_hex = key_hex.trim().to_string();
	if key_hex.len() != 64 || hex::decode(&key_hex).is_err() {
		eprintln!("Error: --key must be a 64-character hex string (256-bit AES key).");
		process::exit(1);
	}

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	match keyring_keys::add(&key_hex) {
		Ok(false) => {
			println!("Key already registered (public {public_key}).");
			return;
		}
		Err(e) => {
			eprintln!("Error storing key: {e}");
			process::exit(1);
		}
		Ok(true) => {}
	}

	println!("Private key added (public {public_key}).");
}

/// Add a new private key.
///
/// If `key_arg` is `Some`, the key is added non-interactively. This path is gated to
/// CI environments only — running it on a dev machine would risk leaking the private
/// key into shell history.
///
/// If `key_arg` is `None`, prompts the user to either generate a new key or paste an
/// existing one.
pub fn cmd_secret_keys_add(key_arg: Option<&str>) {
	use std::io::{self, BufRead, Write};

	if let Some(key_hex) = key_arg {
		add_secret_key_non_interactive(key_hex);
		return;
	}

	// Prompt user for choice
	eprintln!("How would you like to add a secret key?");
	eprintln!();
	eprintln!("  1) Generate a new private key");
	eprintln!("  2) Import an existing private key");
	eprintln!();
	eprint!("Enter choice (1 or 2): ");
	io::stderr().flush().unwrap_or(());

	let stdin = io::stdin();
	let mut choice = String::new();
	if stdin.lock().read_line(&mut choice).is_err() {
		eprintln!("Error reading input.");
		process::exit(1);
	}

	let key_hex = match choice.trim() {
		"1" => runfile_crypto::generate_key(),
		"2" => {
			eprint!("Paste your private key (64-character hex string): ");
			io::stderr().flush().unwrap_or(());

			// Read with terminal echo disabled so the 256-bit key isn't shown on
			// screen or left in terminal scrollback / logs (audit L4).
			let key_input = match read_secret_line() {
				Ok(s) => s,
				Err(_) => {
					eprintln!("Error reading input.");
					process::exit(1);
				}
			};
			let k = key_input.trim().to_string();
			if k.len() != 64 || hex::decode(&k).is_err() {
				eprintln!("Error: key must be a 64-character hex string (256-bit AES key).");
				process::exit(1);
			}
			k
		}
		_ => {
			eprintln!("Invalid choice. Please enter 1 or 2.");
			process::exit(1);
		}
	};

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	match keyring_keys::add(&key_hex) {
		Ok(false) => {
			eprintln!("Key already exists.");
			process::exit(1);
		}
		Err(e) => {
			eprintln!("Error storing key: {e}");
			process::exit(1);
		}
		Ok(true) => {}
	}

	println!();
	println!("Private key added.");
	println!("  Stored in: OS credential store");
	println!("  Public:    {public_key}");
	println!();
	println!("Add this to your encrypted .env files:");
	println!("  {}={public_key}", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR);
}

/// List all stored keys showing public key fingerprints.
pub fn cmd_secret_keys_list() {
	let fingerprints = match keyring_keys::list_fingerprints() {
		Ok(fps) => fps,
		Err(e) => {
			eprintln!("Error reading keyring: {e}");
			process::exit(1);
		}
	};

	if fingerprints.is_empty() {
		println!("No secret keys configured.");
		return;
	}

	for fingerprint in &fingerprints {
		println!("  {fingerprint}  (secure: OS credential store)");
	}
}

/// Remove a private key by matching public key prefix.
pub fn cmd_secret_keys_remove(public_prefix: &str) {
	let fingerprint = match keyring_keys::resolve_prefix(public_prefix) {
		Ok(fp) => fp,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	match keyring_keys::remove(&fingerprint) {
		Ok(true) => {}
		Ok(false) => {
			eprintln!("Error: key not found.");
			process::exit(1);
		}
		Err(e) => {
			eprintln!("Error removing key: {e}");
			process::exit(1);
		}
	}

	println!("Key removed (public: {fingerprint}).");
}

/// Print the full private key for sharing with teammates.
/// Takes a public key prefix to identify which key to print.
pub fn cmd_get_private_key(public_prefix: &str) {
	let all_keys = keyring_keys::all_private_keys();
	let matched = match runfile_crypto::find_private_key_by_public_prefix(public_prefix, &all_keys) {
		Ok(k) => k,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	let public_key = runfile_crypto::derive_public_key(&matched).unwrap_or_else(|_| "???".to_string());

	println!("{matched}");
	eprintln!();
	eprintln!("  Public key: {public_key}");
	eprintln!();
	eprintln!("To import this key on another machine:");
	eprintln!("  run :env secret-keys add");
	eprintln!("  (then paste the private key when prompted)");
}

/// Read a line from stdin with terminal echo disabled when stdin is a TTY, so a
/// pasted private key is not echoed to the screen or captured in scrollback.
/// Falls back to a normal (echoing) read when echo cannot be toggled — piped
/// input, a non-interactive stdin, or a platform failure — so scripted use is
/// unaffected. The returned string includes the trailing newline; callers trim.
fn read_secret_line() -> std::io::Result<String> {
	use std::io::BufRead;
	let echo_disabled = EchoGuard::disable();
	let mut line = String::new();
	let res = std::io::stdin().lock().read_line(&mut line);
	if echo_disabled.is_some() {
		// The user's Enter was not echoed; emit a newline so following output
		// starts on a fresh line.
		eprintln!();
	}
	res?;
	Ok(line)
}

/// RAII guard that disables terminal input echo for the lifetime of the guard
/// and restores the prior mode on drop. `disable()` returns `None` when stdin
/// isn't an interactive terminal (so callers transparently fall back to a plain
/// read).
#[cfg(unix)]
struct EchoGuard {
	fd: std::os::unix::io::RawFd,
	prev: libc::termios,
}

#[cfg(unix)]
impl EchoGuard {
	fn disable() -> Option<Self> {
		use std::os::unix::io::AsRawFd;
		let fd = std::io::stdin().as_raw_fd();
		unsafe {
			if libc::isatty(fd) != 1 {
				return None;
			}
			let mut term: libc::termios = std::mem::zeroed();
			if libc::tcgetattr(fd, &mut term) != 0 {
				return None;
			}
			let prev = term;
			term.c_lflag &= !libc::ECHO;
			if libc::tcsetattr(fd, libc::TCSANOW, &term) != 0 {
				return None;
			}
			Some(EchoGuard { fd, prev })
		}
	}
}

#[cfg(unix)]
impl Drop for EchoGuard {
	fn drop(&mut self) {
		unsafe {
			libc::tcsetattr(self.fd, libc::TCSANOW, &self.prev);
		}
	}
}

#[cfg(windows)]
struct EchoGuard {
	handle: windows_sys::Win32::Foundation::HANDLE,
	prev: u32,
}

#[cfg(windows)]
impl EchoGuard {
	fn disable() -> Option<Self> {
		use windows_sys::Win32::System::Console::{
			GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
		};
		unsafe {
			let handle = GetStdHandle(STD_INPUT_HANDLE);
			if handle.is_null() {
				return None;
			}
			let mut mode: u32 = 0;
			// Fails on a non-console handle (e.g. piped stdin) — fall back to a
			// plain read in that case.
			if GetConsoleMode(handle, &mut mode) == 0 {
				return None;
			}
			let prev = mode;
			if SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT) == 0 {
				return None;
			}
			Some(EchoGuard { handle, prev })
		}
	}
}

#[cfg(windows)]
impl Drop for EchoGuard {
	fn drop(&mut self) {
		unsafe {
			windows_sys::Win32::System::Console::SetConsoleMode(self.handle, self.prev);
		}
	}
}

#[cfg(not(any(unix, windows)))]
struct EchoGuard;

#[cfg(not(any(unix, windows)))]
impl EchoGuard {
	fn disable() -> Option<Self> {
		None
	}
}
