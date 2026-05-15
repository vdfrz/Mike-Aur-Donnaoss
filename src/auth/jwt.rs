use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

const EXPIRY_SECS: u64 = 60 * 60 * 24 * 30; // 30 days

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,   // user_id
    pub email: String,
    pub exp: u64,
    pub iat: u64,
}

fn secret() -> Result<String> {
    std::env::var("JWT_SECRET").map_err(|_| anyhow!("JWT_SECRET not set"))
}

pub fn sign_token(user_id: &str, email: &str) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        iat: now,
        exp: now + EXPIRY_SECS,
    };
    let key = EncodingKey::from_secret(secret()?.as_bytes());
    Ok(encode(&Header::new(Algorithm::HS256), &claims, &key)?)
}

pub fn verify_token(token: &str) -> Result<Claims> {
    let key = DecodingKey::from_secret(secret()?.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    let data = decode::<Claims>(token, &key, &validation)
        .map_err(|e| anyhow!("Invalid token: {e}"))?;
    Ok(data.claims)
}
