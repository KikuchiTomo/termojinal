//! Command signing and verification using Ed25519.
//!
//! Official termojinal commands are signed with a known public key embedded in the
//! binary.  Third-party commands can optionally be signed with their own keys.
//! The UI shows a verified badge for signed commands and a warning for unsigned ones.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// The termojinal official public key (Ed25519).
/// This is embedded in the binary at compile time.
/// To generate a new keypair, use `generate_keypair()`.
const TERMOJINAL_PUBLIC_KEY: [u8; 32] = [
    // TODO: Replace with actual public key bytes after first key generation
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Result of signature verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyResult {
    /// Signed with the official termojinal key.
    OfficialVerified,
    /// Signed with a recognized third-party key.
    ThirdPartyVerified,
    /// Signature present but invalid.
    InvalidSignature,
    /// No signature present.
    Unsigned,
}

impl VerifyResult {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::OfficialVerified | Self::ThirdPartyVerified)
    }

    pub fn is_unsigned(&self) -> bool {
        matches!(self, Self::Unsigned)
    }
}

/// Verify a command's signature.
///
/// The signature is computed over the TOML content of `command.toml`
/// (excluding the `signature` field itself). The signature string in
/// the TOML is hex-encoded Ed25519 signature bytes.
///
/// # Arguments
/// * `toml_content` - The full content of command.toml
/// * `signature_hex` - The hex-encoded signature from the `signature` field (or None if unsigned)
pub fn verify_command(toml_content: &str, signature_hex: Option<&str>) -> VerifyResult {
    let sig_hex = match signature_hex {
        Some(s) if !s.is_empty() => s,
        _ => return VerifyResult::Unsigned,
    };

    // Parse the signature from hex
    let sig_bytes = match hex_decode(sig_hex) {
        Some(b) if b.len() == 64 => b,
        _ => return VerifyResult::InvalidSignature,
    };
    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => return VerifyResult::InvalidSignature,
    };

    // The message to verify is the TOML content with the signature field removed.
    let message = strip_signature_field(toml_content);

    // Try official key first
    if let Ok(official_key) = VerifyingKey::from_bytes(&TERMOJINAL_PUBLIC_KEY) {
        if official_key.verify(message.as_bytes(), &signature).is_ok() {
            return VerifyResult::OfficialVerified;
        }
    }

    // Could check third-party keys from a trust store here in the future.
    VerifyResult::InvalidSignature
}

/// Verify a command's signature against a specific public key.
///
/// This is useful for testing and for third-party key verification.
pub fn verify_command_with_key(
    toml_content: &str,
    signature_hex: Option<&str>,
    public_key: &VerifyingKey,
) -> VerifyResult {
    let sig_hex = match signature_hex {
        Some(s) if !s.is_empty() => s,
        _ => return VerifyResult::Unsigned,
    };

    let sig_bytes = match hex_decode(sig_hex) {
        Some(b) if b.len() == 64 => b,
        _ => return VerifyResult::InvalidSignature,
    };
    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => return VerifyResult::InvalidSignature,
    };

    let message = strip_signature_field(toml_content);

    if public_key.verify(message.as_bytes(), &signature).is_ok() {
        return VerifyResult::ThirdPartyVerified;
    }

    VerifyResult::InvalidSignature
}

/// Sign a command.toml content with a signing key.
///
/// Returns the hex-encoded signature string.
pub fn sign_command(toml_content: &str, signing_key: &SigningKey) -> String {
    let message = strip_signature_field(toml_content);
    let signature = signing_key.sign(message.as_bytes());
    hex_encode(signature.to_bytes().as_ref())
}

/// Generate a new Ed25519 keypair.
///
/// Returns (signing_key, verifying_key_bytes) where verifying_key_bytes
/// is the 32-byte public key that should be embedded in the binary.
pub fn generate_keypair() -> (SigningKey, [u8; 32]) {
    let mut rng = rand::thread_rng();
    let signing_key = SigningKey::generate(&mut rng);
    let verifying_key = signing_key.verifying_key();
    (signing_key, verifying_key.to_bytes())
}

/// Remove the `signature = "..."` line from TOML content for verification.
fn strip_signature_field(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("signature") || !trimmed.contains('=')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Hex encode bytes to string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Hex decode string to bytes.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_unsigned() {
        let toml_content = r#"
[command]
name = "Test"
description = "A test command"
run = "./run.sh"
"#;
        assert_eq!(verify_command(toml_content, None), VerifyResult::Unsigned);
        assert_eq!(
            verify_command(toml_content, Some("")),
            VerifyResult::Unsigned
        );
    }

    #[test]
    fn test_verify_invalid_signature() {
        let toml_content = r#"
[command]
name = "Test"
description = "A test command"
run = "./run.sh"
signature = "not_valid_hex!"
"#;
        assert_eq!(
            verify_command(toml_content, Some("not_valid_hex!")),
            VerifyResult::InvalidSignature
        );

        // Valid hex but wrong length
        assert_eq!(
            verify_command(toml_content, Some("abcd")),
            VerifyResult::InvalidSignature
        );

        // Valid hex, correct length (128 hex chars = 64 bytes), but wrong signature
        let fake_sig = "a".repeat(128);
        assert_eq!(
            verify_command(toml_content, Some(&fake_sig)),
            VerifyResult::InvalidSignature
        );
    }

    #[test]
    fn test_sign_and_verify() {
        // Generate a keypair for testing
        let (signing_key, pub_key_bytes) = generate_keypair();
        let verifying_key = VerifyingKey::from_bytes(&pub_key_bytes).unwrap();

        let toml_content = r#"
[command]
name = "Test"
description = "A test command"
run = "./run.sh"
"#;

        // Sign the content
        let signature = sign_command(toml_content, &signing_key);

        // Verify with the matching key (uses third-party path since it won't match TERMOJINAL_PUBLIC_KEY)
        let result = verify_command_with_key(toml_content, Some(&signature), &verifying_key);
        assert_eq!(result, VerifyResult::ThirdPartyVerified);
        assert!(result.is_verified());

        // Verify that the official verify_command returns InvalidSignature
        // (because the keypair doesn't match the placeholder TERMOJINAL_PUBLIC_KEY)
        let result = verify_command(toml_content, Some(&signature));
        assert_eq!(result, VerifyResult::InvalidSignature);
    }

    #[test]
    fn test_sign_and_verify_with_signature_field_in_toml() {
        // Simulate a TOML that already has a signature field (as it would on disk)
        let (signing_key, pub_key_bytes) = generate_keypair();
        let verifying_key = VerifyingKey::from_bytes(&pub_key_bytes).unwrap();

        let toml_without_sig = r#"
[command]
name = "Test"
description = "A test command"
run = "./run.sh"
"#;

        let signature = sign_command(toml_without_sig, &signing_key);

        // Now create the TOML as it would appear on disk (with signature field)
        let toml_with_sig = format!(
            r#"
[command]
name = "Test"
description = "A test command"
run = "./run.sh"
signature = "{}"
"#,
            signature
        );

        // Verification should still work because strip_signature_field removes the sig line
        let result = verify_command_with_key(&toml_with_sig, Some(&signature), &verifying_key);
        assert_eq!(result, VerifyResult::ThirdPartyVerified);
    }

    #[test]
    fn test_strip_signature_field() {
        let content = r#"[command]
name = "Test"
signature = "abc123"
run = "./run.sh""#;

        let stripped = strip_signature_field(content);
        assert!(!stripped.contains("signature"));
        assert!(stripped.contains("name"));
        assert!(stripped.contains("run"));
    }

    #[test]
    fn test_strip_signature_field_no_signature() {
        let content = r#"[command]
name = "Test"
run = "./run.sh""#;

        let stripped = strip_signature_field(content);
        assert_eq!(stripped, content);
    }

    #[test]
    fn test_hex_encode_decode() {
        let original = vec![0u8, 1, 15, 16, 255, 128];
        let encoded = hex_encode(&original);
        assert_eq!(encoded, "00010f10ff80");

        let decoded = hex_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_hex_decode_empty() {
        let decoded = hex_decode("").unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert!(hex_decode("abc").is_none());
    }

    #[test]
    fn test_hex_decode_invalid_chars() {
        assert!(hex_decode("zzzz").is_none());
    }

    #[test]
    fn test_generate_keypair() {
        let (signing_key, pub_key_bytes) = generate_keypair();

        // Public key should be 32 bytes (it's a fixed-size array, so this is guaranteed)
        assert_eq!(pub_key_bytes.len(), 32);

        // Should be able to create a verifying key from the bytes
        let verifying_key = VerifyingKey::from_bytes(&pub_key_bytes);
        assert!(verifying_key.is_ok());

        // The verifying key should match the signing key's verifying key
        assert_eq!(signing_key.verifying_key().to_bytes(), pub_key_bytes);
    }

    #[test]
    fn test_verify_result_is_unsigned() {
        assert!(VerifyResult::Unsigned.is_unsigned());
        assert!(!VerifyResult::OfficialVerified.is_unsigned());
        assert!(!VerifyResult::ThirdPartyVerified.is_unsigned());
        assert!(!VerifyResult::InvalidSignature.is_unsigned());
    }

    #[test]
    fn test_verify_result_is_verified() {
        assert!(VerifyResult::OfficialVerified.is_verified());
        assert!(VerifyResult::ThirdPartyVerified.is_verified());
        assert!(!VerifyResult::Unsigned.is_verified());
        assert!(!VerifyResult::InvalidSignature.is_verified());
    }
}
