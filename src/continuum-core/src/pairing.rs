//! Pairing code generation and validation.

use crate::ContinuumResult;
use rand::Rng;

/// Pairing code alphabet (no ambiguous chars: i, l, o, u)
const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Generate a random pairing code of the given length.
///
/// # Examples
///
/// ```
/// use continuum_core::pairing::generate_pairing_code;
///
/// let code = generate_pairing_code(8);
/// assert_eq!(code.len(), 9); // 8 chars + 1 dash
/// assert!(code.contains('-'));
/// ```
pub fn generate_pairing_code(length: u8) -> String {
    let mut rng = rand::thread_rng();
    let mut code = String::with_capacity(length as usize + 1);

    for i in 0..length {
        let idx = rng.gen_range(0..ALPHABET.len());
        code.push(ALPHABET[idx] as char);

        // Insert dash after 4th character
        if i == 3 {
            code.push('-');
        }
    }

    code
}

/// Validate a pairing code format.
///
/// Returns true if the code matches the expected format:
/// - XXXX-XXXX (8 chars + dash)
/// - All characters from the pairing alphabet
pub fn validate_pairing_code(code: &str) -> bool {
    // Check length (9 = 8 chars + 1 dash)
    if code.len() != 9 {
        return false;
    }

    // Check dash position
    if code.as_bytes()[4] != b'-' {
        return false;
    }

    // Check all characters are from alphabet
    for (i, c) in code.chars().enumerate() {
        if i == 4 {
            continue; // skip dash
        }
        if !ALPHABET.contains(&(c as u8)) {
            return false;
        }
    }

    true
}

/// Parse a pairing code (remove dash, normalize case).
pub fn parse_pairing_code(code: &str) -> Option<String> {
    if !validate_pairing_code(code) {
        return None;
    }
    Some(code.replace('-', "").to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_pairing_code_format() {
        for _ in 0..100 {
            let code = generate_pairing_code(8);
            assert!(validate_pairing_code(&code), "Invalid code: {}", code);
        }
    }

    #[test]
    fn test_generate_pairing_code_uniqueness() {
        use std::collections::HashSet;
        let mut codes = HashSet::new();

        for _ in 0..1000 {
            let code = generate_pairing_code(8);
            assert!(codes.insert(code), "Duplicate code generated");
        }
    }

    #[test]
    fn test_validate_pairing_code_valid() {
        assert!(validate_pairing_code("a3f7-e8c1"));
        assert!(validate_pairing_code("1234-5678"));
        assert!(validate_pairing_code("abcd-efgh"));
    }

    #[test]
    fn test_validate_pairing_code_invalid() {
        // Too short
        assert!(!validate_pairing_code("a3f7-e8c"));
        // Too long
        assert!(!validate_pairing_code("a3f7-e8c12"));
        // No dash
        assert!(!validate_pairing_code("a3f7e8c1"));
        // Wrong dash position
        assert!(!validate_pairing_code("a3f-7e8c1"));
        // Invalid chars
        assert!(!validate_pairing_code("i3f7-e8c1")); // 'i' not in alphabet
        assert!(!validate_pairing_code("o3f7-e8c1")); // 'o' not in alphabet
    }

    #[test]
    fn test_parse_pairing_code() {
        assert_eq!(parse_pairing_code("a3f7-e8c1"), Some("a3f7e8c1".to_string()));
        assert_eq!(parse_pairing_code("invalid"), None);
    }
}
