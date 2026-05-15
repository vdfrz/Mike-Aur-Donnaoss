use anyhow::Result;

const COST: u32 = 12;

pub fn hash_password(plain: &str) -> Result<String> {
    Ok(bcrypt::hash(plain, COST)?)
}

pub fn verify_password(plain: &str, hash: &str) -> Result<bool> {
    Ok(bcrypt::verify(plain, hash)?)
}
