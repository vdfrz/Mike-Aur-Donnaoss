/// Biometric authentication — platform-specific.
///
/// Windows: Windows Hello via UserConsentVerifier API.
///   Supports fingerprint, face, iris, or PIN fallback — OS handles everything.
///
/// macOS: TODO — LocalAuthentication framework (objc2-local-authentication).
///
/// The contract is simple: `verify()` returns Ok(true) if the OS accepted
/// the biometric, Ok(false) if the user cancelled or failed, Err if the
/// hardware/API is unavailable.
use anyhow::Result;

pub async fn is_available() -> bool {
    #[cfg(target_os = "windows")]
    return windows_impl::is_available().await;

    #[cfg(target_os = "macos")]
    return macos_impl::is_available().await;

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    return false;
}

/// Prompt the OS biometric dialog with the given reason string.
/// Returns true if the user authenticated successfully.
pub async fn verify(reason: &str) -> Result<bool> {
    #[cfg(target_os = "windows")]
    return windows_impl::verify(reason).await;

    #[cfg(target_os = "macos")]
    return macos_impl::verify(reason).await;

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = reason;
        anyhow::bail!("Biometric authentication is not supported on this platform");
    }
}

// ---------------------------------------------------------------------------
// Windows Hello implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows_impl {
    use anyhow::Result;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier,
        UserConsentVerifierAvailability,
    };
    use windows::core::HSTRING;

    pub async fn is_available() -> bool {
        tokio::task::spawn_blocking(|| {
            tracing::debug!("[biometric] checking Windows Hello availability");
            match UserConsentVerifier::CheckAvailabilityAsync() {
                Ok(op) => match op.get() {
                    Ok(result) => {
                        tracing::info!("[biometric] availability result code: {}", result.0);
                        matches!(result, UserConsentVerifierAvailability::Available)
                    }
                    Err(e) => {
                        tracing::warn!("[biometric] availability op.get() error: {e}");
                        false
                    }
                },
                Err(e) => {
                    tracing::warn!("[biometric] CheckAvailabilityAsync error: {e}");
                    false
                }
            }
        })
        .await
        .unwrap_or(false)
    }

    pub async fn verify(reason: &str) -> Result<bool> {
        let reason = reason.to_string();
        tracing::info!("[biometric] verify() called with reason: '{reason}'");
        let blocking = tokio::task::spawn_blocking(move || {
            tracing::debug!("[biometric] spawn_blocking: calling RequestVerificationAsync");
            let message = HSTRING::from(reason.as_str());
            let op = UserConsentVerifier::RequestVerificationAsync(&message)
                .map_err(|e| anyhow::anyhow!("Windows Hello init error: {e}"))?;
            tracing::debug!("[biometric] RequestVerificationAsync returned op, calling .get()");
            let result = op
                .get()
                .map_err(|e| anyhow::anyhow!("Windows Hello verify error: {e}"))?;
            tracing::info!("[biometric] verify result code: {}", result.0);
            Ok(matches!(result, UserConsentVerificationResult::Verified))
        });

        // Hard timeout so the HTTP request never hangs indefinitely
        match tokio::time::timeout(std::time::Duration::from_secs(60), blocking).await {
            Ok(join) => join.map_err(|e| anyhow::anyhow!("spawn_blocking error: {e}"))?,
            Err(_) => {
                tracing::error!("[biometric] Windows Hello timed out after 60s");
                anyhow::bail!("Windows Hello timed out")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// macOS placeholder
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos_impl {
    use anyhow::Result;

    pub async fn is_available() -> bool {
        // TODO: implement via objc2-local-authentication
        // let context = LAContext::new();
        // context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
        false
    }

    pub async fn verify(_reason: &str) -> Result<bool> {
        // TODO: implement via objc2-local-authentication
        anyhow::bail!("macOS Touch ID not yet implemented")
    }
}
