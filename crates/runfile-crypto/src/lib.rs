use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Nonce};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

/// Prefix for encrypted values in env files and env objects.
pub const ENCRYPTED_PREFIX: &str = "encrypted:";

/// Environment variable name that stores the public key in encrypted env files.
pub const ENCRYPTION_PUBLIC_KEY_VAR: &str = "RUNFILE_ENCRYPTION_PUBLIC_KEY";

/// Nonce size for AES-256-GCM (96 bits = 12 bytes).
const NONCE_SIZE: usize = 12;

#[derive(Debug, Error)]
pub enum CryptoError {
	#[error("Invalid encryption key: {0}")]
	InvalidKey(String),

	#[error("Failed to decrypt env var \"{0}\": {1}")]
	DecryptionFailed(String, String),

	#[error("Invalid encrypted value format: {0}")]
	InvalidFormat(String),

	#[error("Failed to encrypt value: {0}")]
	EncryptionFailed(String),
}

/// Generate a new random 256-bit encryption key.
/// Returns a 64-character hex string.
pub fn generate_key() -> String {
	let key = Aes256Gcm::generate_key(OsRng);
	hex::encode(key)
}

/// Decode a hex-encoded key into raw bytes and create a cipher.
///
/// Key material is zeroed from memory after the cipher is constructed.
fn make_cipher(key_hex: &str) -> Result<Aes256Gcm, CryptoError> {
	let mut key_bytes =
		hex::decode(key_hex).map_err(|e| CryptoError::InvalidKey(format!("invalid hex encoding: {e}")))?;
	if key_bytes.len() != 32 {
		key_bytes.zeroize();
		return Err(CryptoError::InvalidKey(format!(
			"expected 32 bytes (64 hex chars), got {} bytes ({} hex chars)",
			key_bytes.len(),
			key_hex.len()
		)));
	}
	let cipher = Aes256Gcm::new_from_slice(&key_bytes)
		.map_err(|_| CryptoError::InvalidKey("failed to initialize cipher".to_string()));
	key_bytes.zeroize();
	cipher
}

/// Encrypt a plaintext string using AES-256-GCM.
///
/// Returns `"encrypted:<base64(nonce || ciphertext || tag)>"`.
pub fn encrypt(plaintext: &str, key_hex: &str) -> Result<String, CryptoError> {
	let cipher = make_cipher(key_hex)?;
	let nonce = Aes256Gcm::generate_nonce(OsRng);
	let ciphertext = cipher
		.encrypt(&nonce, plaintext.as_bytes())
		.map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

	// Concatenate nonce + ciphertext (which includes the tag appended by aes-gcm)
	let mut payload = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
	payload.extend_from_slice(&nonce);
	payload.extend_from_slice(&ciphertext);

	let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
	Ok(format!("{ENCRYPTED_PREFIX}{encoded}"))
}

/// Decrypt an encrypted string produced by [`encrypt`].
///
/// Accepts values with or without the `"encrypted:"` prefix.
pub fn decrypt(encrypted: &str, key_hex: &str) -> Result<String, CryptoError> {
	let b64 = encrypted.strip_prefix(ENCRYPTED_PREFIX).unwrap_or(encrypted);

	let payload = base64::engine::general_purpose::STANDARD
		.decode(b64)
		.map_err(|e| CryptoError::InvalidFormat(format!("invalid base64: {e}")))?;

	if payload.len() < NONCE_SIZE + 1 {
		return Err(CryptoError::InvalidFormat(format!(
			"payload too short ({} bytes, need at least {})",
			payload.len(),
			NONCE_SIZE + 1
		)));
	}

	let (nonce_bytes, ciphertext) = payload.split_at(NONCE_SIZE);
	let nonce = Nonce::from_slice(nonce_bytes);

	let cipher = make_cipher(key_hex)?;
	let mut plaintext_bytes = Zeroizing::new(
		cipher
			.decrypt(nonce, ciphertext)
			.map_err(|_| CryptoError::InvalidFormat("decryption failed (wrong key or corrupted data)".to_string()))?,
	);

	let result = String::from_utf8(plaintext_bytes.to_vec())
		.map_err(|e| CryptoError::InvalidFormat(format!("decrypted value is not valid UTF-8: {e}")));
	plaintext_bytes.zeroize();
	result
}

/// Check whether a value has the `"encrypted:"` prefix.
pub fn is_encrypted(value: &str) -> bool {
	value.starts_with(ENCRYPTED_PREFIX)
}

/// Check whether any value in the map has the `"encrypted:"` prefix.
pub fn has_encrypted_values(env: &HashMap<String, String>) -> bool {
	env.values().any(|v| is_encrypted(v))
}

/// Decrypt all values in the map that have the `"encrypted:"` prefix.
/// Plain values are left unchanged.
pub fn decrypt_env_values(env: &mut HashMap<String, String>, key_hex: &str) -> Result<(), CryptoError> {
	// Collect keys to avoid borrow issues
	let keys_to_decrypt: Vec<String> = env
		.iter()
		.filter(|(_, v)| is_encrypted(v))
		.map(|(k, _)| k.clone())
		.collect();

	for key in keys_to_decrypt {
		let encrypted_value = &env[&key];
		let decrypted = decrypt(encrypted_value, key_hex).map_err(|e| match e {
			CryptoError::InvalidFormat(msg) => CryptoError::DecryptionFailed(key.clone(), msg),
			CryptoError::InvalidKey(msg) => CryptoError::InvalidKey(msg),
			other => CryptoError::DecryptionFailed(key.clone(), other.to_string()),
		})?;
		env.insert(key, decrypted);
	}

	Ok(())
}

/// Derive a public key (fingerprint) from a private key.
///
/// The public key is the SHA-256 hash of the raw private key bytes,
/// returned as a 64-character hex string. This allows identifying which
/// private key was used to encrypt a file without revealing the key itself.
pub fn derive_public_key(private_key_hex: &str) -> Result<String, CryptoError> {
	let mut key_bytes =
		hex::decode(private_key_hex).map_err(|e| CryptoError::InvalidKey(format!("invalid hex encoding: {e}")))?;
	if key_bytes.len() != 32 {
		key_bytes.zeroize();
		return Err(CryptoError::InvalidKey(format!(
			"expected 32 bytes (64 hex chars), got {} bytes ({} hex chars)",
			key_bytes.len(),
			private_key_hex.len()
		)));
	}
	let hash = Sha256::digest(&key_bytes);
	key_bytes.zeroize();
	Ok(hex::encode(hash))
}

/// Find a private key that matches a given public key.
///
/// Derives the public key from each available private key and returns the
/// first one whose public key matches. Uses constant-time comparison to
/// prevent timing side-channel attacks. Returns `None` if no match is found.
pub fn find_matching_private_key(public_key_hex: &str, private_keys: &[String]) -> Option<String> {
	for pk in private_keys {
		if let Ok(derived) = derive_public_key(pk)
			&& derived.as_bytes().ct_eq(public_key_hex.as_bytes()).into()
		{
			return Some(pk.clone());
		}
	}
	None
}

/// Find a private key by matching a prefix of its derived public key.
///
/// Returns `Ok(private_key)` if exactly one stored key has a public key starting with `public_prefix`.
/// Returns `Err` if zero or more than one key matches.
pub fn find_private_key_by_public_prefix(public_prefix: &str, private_keys: &[String]) -> Result<String, CryptoError> {
	let mut matches: Vec<&String> = Vec::new();
	for pk in private_keys {
		if let Ok(derived) = derive_public_key(pk)
			&& derived.starts_with(public_prefix)
		{
			matches.push(pk);
		}
	}
	match matches.len() {
		0 => Err(CryptoError::InvalidKey(format!(
			"no key has a public key starting with \"{public_prefix}\""
		))),
		1 => Ok(matches[0].clone()),
		n => Err(CryptoError::InvalidKey(format!(
			"{n} keys have public keys starting with \"{public_prefix}\" — provide more characters to disambiguate"
		))),
	}
}

#[cfg(test)]
mod tests;
