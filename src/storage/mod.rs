use anyhow::Result;
use std::path::PathBuf;
use tokio::fs;

/// Unified storage trait — local filesystem or S3/R2 backend.
/// Switch via env: if STORAGE_PATH is set → LocalStorage, else → S3Storage.
#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    async fn put(&self, key: &str, data: &[u8], content_type: &str) -> Result<()>;
    async fn get(&self, key: &str) -> Result<Vec<u8>>;
    async fn delete(&self, key: &str) -> Result<()>;
    /// Returns a URL or file:// path usable for download
    async fn public_url(&self, key: &str) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Local filesystem implementation
// ---------------------------------------------------------------------------

pub struct LocalStorage {
    base: PathBuf,
}

impl LocalStorage {
    pub fn new() -> Result<Self> {
        let base = PathBuf::from(
            std::env::var("STORAGE_PATH").unwrap_or_else(|_| "./data/storage".to_string()),
        );
        std::fs::create_dir_all(&base)?;
        Ok(Self { base })
    }

    fn full_path(&self, key: &str) -> PathBuf {
        // Sanitize: strip leading slashes, no path traversal
        let safe = key.trim_start_matches('/').replace("..", "");
        self.base.join(safe)
    }
}

#[async_trait::async_trait]
impl Storage for LocalStorage {
    async fn put(&self, key: &str, data: &[u8], _content_type: &str) -> Result<()> {
        let path = self.full_path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, data).await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        Ok(fs::read(self.full_path(key)).await?)
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let path = self.full_path(key);
        if path.exists() {
            fs::remove_file(path).await?;
        }
        Ok(())
    }

    async fn public_url(&self, key: &str) -> Result<String> {
        // In local mode serve via /download/:key endpoint
        let api_base = std::env::var("API_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:3001".to_string());
        Ok(format!("{api_base}/download/{key}"))
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

pub fn make_storage() -> Result<Box<dyn Storage>> {
    // If STORAGE_PATH is set, use local filesystem.
    // R2/S3 implementation can be added behind the s3-storage feature flag.
    Ok(Box::new(LocalStorage::new()?))
}
