/// PIN hashing via Argon2id — suitable for short PINs (4-8 digits).
/// Argon2id is memory-hard which makes brute-force of short PINs expensive
/// even if the SQLite file is stolen.
use anyhow::Result;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core_06::OsRng;

pub fn hash_pin(pin: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(pin.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("PIN hash error: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_pin(pin: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| anyhow::anyhow!("PIN hash parse error: {e}"))?;
    Ok(Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok())
}

pub fn validate_pin_format(pin: &str) -> bool {
    let len = pin.len();
    len >= 4 && len <= 8 && pin.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_4_to_8_digit_pins() {
        assert!(validate_pin_format("1234"));
        assert!(validate_pin_format("12345678"));
        assert!(validate_pin_format("000000"));
    }

    #[test]
    fn rejects_short_long_or_non_digit_pins() {
        assert!(!validate_pin_format("123"));
        assert!(!validate_pin_format("123456789"));
        assert!(!validate_pin_format("abcd"));
        assert!(!validate_pin_format("12a4"));
        assert!(!validate_pin_format(""));
        assert!(!validate_pin_format(" 1234"));
    }

    #[test]
    fn hash_verify_roundtrip() {
        let hash = hash_pin("139042").unwrap();
        assert!(verify_pin("139042", &hash).unwrap());
        assert!(!verify_pin("139043", &hash).unwrap());
    }

    #[test]
    fn hash_is_salted_and_differs_each_call() {
        let h1 = hash_pin("12345").unwrap();
        let h2 = hash_pin("12345").unwrap();
        assert_ne!(h1, h2, "argon2id with random salt produces different hashes");
        // …but both must verify against the same PIN.
        assert!(verify_pin("12345", &h1).unwrap());
        assert!(verify_pin("12345", &h2).unwrap());
    }

    #[test]
    fn verify_returns_err_on_malformed_hash() {
        assert!(verify_pin("1234", "not-a-hash").is_err());
    }
}
