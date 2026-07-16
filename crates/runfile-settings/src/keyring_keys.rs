//! Single source of truth for runfile's secret keys: the OS credential store.
//!
//! All private keys live in one keyring entry as a JSON object
//! `{ fingerprint: private_key_hex, ... }`. This module is the only path
//! through which the CLI talks to that storage. settings.json no longer
//! tracks fingerprints — drift between the two stores is impossible by
//! construction.

use crate::keyring_store;
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

/// Newline-separated list of 64-char hex private keys, merged into
/// [`all_private_keys`] ahead of the keyring blob. Designed for ephemeral
/// environments (CI runners, containers) where bootstrapping an OS credential
/// store just to round-trip secrets that already live in process env is wasted
/// work. Whitespace-only lines are skipped. A non-empty pool also silences the
/// "credential store unavailable" warning, since the caller has explicitly
/// opted out of relying on the keyring.
pub const ENV_PRIVATE_KEYS_VAR: &str = "RUNFILE_PRIVATE_KEYS";

#[derive(Debug, Error)]
pub enum KeyringKeysError {
	#[error("OS credential store error: {0}")]
	Keyring(String),

	#[error(
		"OS credential store is unavailable. Private keys require a working credential store \
		 (Windows Credential Manager / macOS Keychain). On Linux this never triggers — keys fall \
		 back to kernel keyutils when no D-Bus Secret Service is available."
	)]
	StoreUnavailable,

	#[error("invalid private key: {0}")]
	InvalidKey(String),

	#[error("failed to parse keystore blob: {0}")]
	BlobParse(String),

	#[error("failed to serialize keystore blob: {0}")]
	BlobSerialize(String),
}

impl From<keyring_core::Error> for KeyringKeysError {
	fn from(e: keyring_core::Error) -> Self {
		KeyringKeysError::Keyring(e.to_string())
	}
}

/// In-memory map: fingerprint → private key hex. `BTreeMap` so list/serialize
/// orderings are deterministic.
pub(crate) type Blob = BTreeMap<String, String>;

pub(crate) fn parse_blob(raw: &str) -> Result<Blob, KeyringKeysError> {
	if raw.trim().is_empty() {
		return Ok(Blob::new());
	}
	serde_json::from_str(raw).map_err(|e| KeyringKeysError::BlobParse(e.to_string()))
}

pub(crate) fn serialize_blob(blob: &Blob) -> Result<String, KeyringKeysError> {
	serde_json::to_string(blob).map_err(|e| KeyringKeysError::BlobSerialize(e.to_string()))
}

fn read_blob() -> Result<Blob, KeyringKeysError> {
	match keyring_store::load_blob()? {
		Some(raw) => parse_blob(&raw),
		None => Ok(Blob::new()),
	}
}

fn write_blob(blob: &Blob) -> Result<(), KeyringKeysError> {
	if blob.is_empty() {
		// No keys left — remove the entry rather than store an empty object,
		// so the credential manager doesn't show an empty placeholder.
		let _ = keyring_store::delete_blob()?;
		return Ok(());
	}
	let raw = serialize_blob(blob)?;
	keyring_store::store_blob(&raw)?;
	Ok(())
}

/// Add a private key. Returns `Ok(false)` if a key with the same fingerprint
/// is already stored (no-op), `Ok(true)` on insert.
pub fn add(private_key_hex: &str) -> Result<bool, KeyringKeysError> {
	let fingerprint =
		runfile_crypto::derive_public_key(private_key_hex).map_err(|e| KeyringKeysError::InvalidKey(e.to_string()))?;

	if !keyring_store::is_available() {
		return Err(KeyringKeysError::StoreUnavailable);
	}

	let mut blob = read_blob()?;
	if blob.contains_key(&fingerprint) {
		return Ok(false);
	}
	blob.insert(fingerprint, private_key_hex.to_string());
	write_blob(&blob)?;
	Ok(true)
}

/// Remove a key by full fingerprint. `Ok(false)` if no such fingerprint was
/// stored. Match is exact — for prefix matching, callers should resolve
/// `prefix → full fingerprint` first via [`list_fingerprints`].
pub fn remove(fingerprint: &str) -> Result<bool, KeyringKeysError> {
	let mut blob = read_blob()?;
	if blob.remove(fingerprint).is_none() {
		return Ok(false);
	}
	write_blob(&blob)?;
	Ok(true)
}

/// Resolve a fingerprint prefix to a full fingerprint. Errors if zero or
/// more than one stored fingerprint matches.
pub fn resolve_prefix(prefix: &str) -> Result<String, KeyringKeysError> {
	let fps = list_fingerprints()?;
	let matches: Vec<&String> = fps.iter().filter(|fp| fp.starts_with(prefix)).collect();
	match matches.len() {
		0 => Err(KeyringKeysError::InvalidKey(format!(
			"no key has a public key starting with \"{prefix}\""
		))),
		1 => Ok(matches[0].clone()),
		n => Err(KeyringKeysError::InvalidKey(format!(
			"{n} keys have public keys starting with \"{prefix}\" — provide more characters to disambiguate"
		))),
	}
}

/// List every stored fingerprint, sorted lexicographically.
pub fn list_fingerprints() -> Result<Vec<String>, KeyringKeysError> {
	let blob = read_blob()?;
	Ok(blob.into_keys().collect())
}

/// Parse the [`ENV_PRIVATE_KEYS_VAR`] env-var value into a key list:
/// newline-separated, whitespace trimmed, blank lines skipped. No validation
/// of hex / length is performed here — invalid keys simply fail to match
/// downstream (`find_matching_private_key` runs `derive_public_key` and
/// silently ignores anything that can't be parsed as hex).
pub(crate) fn parse_env_pool(raw: &str) -> Vec<String> {
	raw.lines()
		.map(str::trim)
		.filter(|line| !line.is_empty())
		.map(str::to_string)
		.collect()
}

/// Merge the env-supplied key pool with the keyring blob result.
///
/// Env keys come first so they take precedence in any first-match scan
/// (e.g. [`runfile_crypto::find_matching_private_key`]'s linear walk).
/// Duplicates are stripped to keep `find_private_key_by_public_prefix` —
/// which counts matches and errors on >1 — from spuriously reporting
/// ambiguity when a key is registered in both places.
///
/// On keyring failure: if the env pool already supplied keys, the warning is
/// suppressed (the caller has clearly opted into env-only key supply, no need
/// to nag about a missing credential store).
pub(crate) fn merge_key_sources(env_pool: Vec<String>, keyring: Result<Blob, KeyringKeysError>) -> Vec<String> {
	let mut keys = env_pool;
	match keyring {
		Ok(blob) => keys.extend(blob.into_values()),
		Err(e) => {
			if keys.is_empty() {
				eprintln!("Warning: failed to load private keys from credential store: {e}");
			}
		}
	}
	let mut seen: HashSet<String> = HashSet::new();
	keys.retain(|k| seen.insert(k.clone()));
	keys
}

/// Return every available private key (raw hex). Sourced from
/// [`ENV_PRIVATE_KEYS_VAR`] plus the OS credential store blob.
///
/// Used as the available-keys pool for decryption / prefix matching. The
/// executor's env-build path treats "no keys" as "no decryption possible"
/// rather than aborting the run.
pub fn all_private_keys() -> Vec<String> {
	let env_pool = std::env::var(ENV_PRIVATE_KEYS_VAR)
		.map(|raw| parse_env_pool(&raw))
		.unwrap_or_default();
	merge_key_sources(env_pool, read_blob())
}
