use crate::keyring_keys::*;

#[test]
fn parse_empty_string_yields_empty_blob() {
	let blob = parse_blob("").unwrap();
	assert!(blob.is_empty());
}

#[test]
fn parse_whitespace_yields_empty_blob() {
	let blob = parse_blob("   \n").unwrap();
	assert!(blob.is_empty());
}

#[test]
fn parse_object_yields_entries() {
	let raw = r#"{"aabb":"deadbeef","ccdd":"feedface"}"#;
	let blob = parse_blob(raw).unwrap();
	assert_eq!(blob.len(), 2);
	assert_eq!(blob.get("aabb").unwrap(), "deadbeef");
	assert_eq!(blob.get("ccdd").unwrap(), "feedface");
}

#[test]
fn parse_invalid_json_errors() {
	let err = parse_blob("not json").unwrap_err();
	assert!(matches!(err, KeyringKeysError::BlobParse(_)));
}

#[test]
fn serialize_is_deterministic_via_btreemap() {
	// BTreeMap iteration order is sorted, so serialization is stable.
	let mut blob = Blob::new();
	blob.insert("ccdd".to_string(), "feedface".to_string());
	blob.insert("aabb".to_string(), "deadbeef".to_string());
	let raw = serialize_blob(&blob).unwrap();
	// `aabb` must come before `ccdd` regardless of insertion order.
	let aa = raw.find("aabb").unwrap();
	let cc = raw.find("ccdd").unwrap();
	assert!(aa < cc, "expected sorted serialization, got {raw}");
}

#[test]
fn parse_env_pool_handles_blank_and_whitespace() {
	let raw = "  aa  \n\n bb\n\n  \ncc";
	assert_eq!(parse_env_pool(raw), vec!["aa", "bb", "cc"]);
}

#[test]
fn parse_env_pool_empty_input_yields_empty() {
	assert!(parse_env_pool("").is_empty());
	assert!(parse_env_pool("   \n\t\n").is_empty());
}

#[test]
fn merge_env_first_then_keyring() {
	let env = vec!["env_key".to_string()];
	let mut blob = Blob::new();
	blob.insert("fp1".to_string(), "ring_key".to_string());
	assert_eq!(merge_key_sources(env, Ok(blob)), vec!["env_key", "ring_key"]);
}

#[test]
fn merge_dedups_overlap() {
	// Same key registered in both pools — must appear only once so
	// `find_private_key_by_public_prefix` doesn't see spurious ambiguity.
	let env = vec!["shared".to_string(), "env_only".to_string()];
	let mut blob = Blob::new();
	blob.insert("fp1".to_string(), "shared".to_string());
	blob.insert("fp2".to_string(), "ring_only".to_string());
	let result = merge_key_sources(env, Ok(blob));
	assert_eq!(result.iter().filter(|k| *k == "shared").count(), 1);
	assert!(result.contains(&"env_only".to_string()));
	assert!(result.contains(&"ring_only".to_string()));
}

#[test]
fn merge_keyring_error_with_env_pool_returns_env() {
	// Keyring unavailable + env pool present → return env keys, no panic.
	let env = vec!["env_key".to_string()];
	let result = merge_key_sources(env, Err(KeyringKeysError::StoreUnavailable));
	assert_eq!(result, vec!["env_key"]);
}

#[test]
fn merge_keyring_error_no_env_returns_empty() {
	let result = merge_key_sources(Vec::new(), Err(KeyringKeysError::StoreUnavailable));
	assert!(result.is_empty());
}

#[test]
fn roundtrip_preserves_entries() {
	let mut blob = Blob::new();
	blob.insert("fp1".to_string(), "key1".to_string());
	blob.insert("fp2".to_string(), "key2".to_string());
	let raw = serialize_blob(&blob).unwrap();
	let parsed = parse_blob(&raw).unwrap();
	assert_eq!(blob, parsed);
}
