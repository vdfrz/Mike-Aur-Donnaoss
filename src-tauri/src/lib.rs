use tauri::Manager;
use tokio::sync::mpsc;

/// Entry point called by main.rs.
/// Starts the axum server as a background tokio task, then launches Tauri.
pub fn run() {
    dotenvy::dotenv().ok();

    // Init tracing once
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "mike=debug,tower_http=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    // Biometric channel: axum sends requests, Tauri processes them with HWND
    let (bio_tx, mut bio_rx) = mpsc::channel::<mike::BiometricRequest>(4);

    // Spawn the axum server on a background tokio runtime
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let port: u16 = std::env::var("PORT")
                .unwrap_or_else(|_| "3001".into())
                .parse()
                .unwrap_or(3001);
            if let Err(e) = mike::run_server_with_bio_tx(port, Some(bio_tx)).await {
                tracing::error!("axum server error: {e}");
            }
        });
    });

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            open_external_url,
            open_file_in_word,
            open_ecourts_verify_window
        ])
        .setup(move |app| {
            // When running as an installed bundle there is no libs/pdfium next
            // to the executable, so point pdfium at the copy bundled under the
            // app's resource dir. Harmless in dev: if the dir is absent the
            // loader (src/pdf/mod.rs) falls back to its other search paths.
            if std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
                if let Ok(res_dir) = app.path().resource_dir() {
                    let pdfium_dir = res_dir.join("pdfium");
                    if pdfium_dir.is_dir() {
                        // SAFETY: set once during setup. pdfium is loaded lazily
                        // (only when a PDF is processed, long after startup), so
                        // no other thread reads this var concurrently here.
                        unsafe {
                            std::env::set_var("PDFIUM_DYNAMIC_LIB_PATH", &pdfium_dir);
                        }
                        tracing::info!("[pdf] using bundled pdfium at {}", pdfium_dir.display());
                    }
                }
            }

            #[cfg(debug_assertions)]
            app.get_webview_window("main")
                .expect("main window")
                .open_devtools();

            // Spawn task that handles biometric requests using the Tauri window HWND
            let window = app.get_webview_window("main").expect("main window");
            tauri::async_runtime::spawn(async move {
                tracing::info!("[tauri-bio] biometric channel listener started");
                while let Some((reason, reply)) = bio_rx.recv().await {
                    tracing::info!("[tauri-bio] received request: '{reason}'");
                    let result = verify_with_window(&window, &reason);
                    tracing::info!("[tauri-bio] verify_with_window result: {:?}", result);
                    let _ = reply.send(result);
                }
                tracing::warn!("[tauri-bio] biometric channel closed");
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Call Windows Hello from the Tauri task context.
///
/// Uses the COM interop API `IUserConsentVerifierInterop::RequestVerificationForWindowAsync`
/// so the OS dialog is parented to our Tauri window — without this, the dialog
/// can appear behind the app window and the user may miss it.
#[cfg(target_os = "windows")]
fn verify_with_window(
    window: &tauri::WebviewWindow,
    reason: &str,
) -> Result<bool, String> {
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier,
        UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use windows::core::HSTRING;
    use windows_future::IAsyncOperation;

    tracing::info!("[tauri-bio] verify_with_window: checking availability");
    let avail = UserConsentVerifier::CheckAvailabilityAsync()
        .map_err(|e: windows::core::Error| { tracing::error!("[tauri-bio] CheckAvailabilityAsync error: {e}"); e.to_string() })?
        .get()
        .map_err(|e: windows::core::Error| { tracing::error!("[tauri-bio] availability .get() error: {e}"); e.to_string() })?;

    tracing::info!("[tauri-bio] availability value: {}", avail.0);
    if !matches!(avail, UserConsentVerifierAvailability::Available) {
        return Err(format!("Windows Hello not available (code {})", avail.0));
    }

    // Bring our Tauri window to the foreground so the OS-level dialog inherits
    // focus from a visible parent. Best-effort — failure isn't fatal.
    let _ = window.set_focus();
    let _ = window.unminimize();

    let raw_hwnd = window
        .hwnd()
        .map_err(|e| { tracing::error!("[tauri-bio] window.hwnd() error: {e}"); format!("hwnd error: {e}") })?;
    let hwnd = HWND(raw_hwnd.0 as *mut core::ffi::c_void);
    tracing::info!("[tauri-bio] obtained HWND: {:?}", hwnd.0);

    let interop: IUserConsentVerifierInterop =
        windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(|e: windows::core::Error| {
                tracing::error!("[tauri-bio] interop factory error: {e}");
                format!("interop factory error: {e}")
            })?;

    let message = HSTRING::from(reason);
    tracing::info!("[tauri-bio] calling RequestVerificationForWindowAsync('{reason}')");
    let op: IAsyncOperation<UserConsentVerificationResult> = unsafe {
        interop
            .RequestVerificationForWindowAsync(hwnd, &message)
            .map_err(|e: windows::core::Error| { tracing::error!("[tauri-bio] interop call error: {e}"); e.to_string() })?
    };
    let result: UserConsentVerificationResult = op
        .get()
        .map_err(|e: windows::core::Error| { tracing::error!("[tauri-bio] .get() error: {e}"); e.to_string() })?;

    tracing::info!("[tauri-bio] verification result code: {}", result.0);
    Ok(matches!(result, UserConsentVerificationResult::Verified))
}

#[cfg(not(target_os = "windows"))]
fn verify_with_window(
    _window: &tauri::WebviewWindow,
    _reason: &str,
) -> Result<bool, String> {
    Err("Biometric not supported on this platform".into())
}

/// Open a URL in the system default browser.
///
/// Tauri's WebView intercepts plain `<a target="_blank">` clicks and
/// opens them inside the same WebView, which makes the in-app shell
/// behave like a mini-browser. Routing through the OS launcher (via
/// the `open` crate) hands the URL to whatever the user's default
/// browser is, which is what they expect when clicking "Open on
/// Indian Kanoon".
///
/// Validates the scheme — only `http://` and `https://` URLs are
/// accepted, so a malicious payload from a tool result can't
/// trigger a `file://` or `mailto:` action through this command.
#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("rejected non-http(s) URL: {url}"));
    }
    open::that(&url).map_err(|e| e.to_string())
}

/// Open a .docx file in Microsoft Word via the system launcher.
/// Used by the DocPanel "Open in Word" button. Accepts a full
/// filesystem path — for RAG KB citations the file already exists
/// on disk at the `kbPath`.
#[tauri::command]
fn open_file_in_word(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("File not found: {}", p.display()));
    }
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .args(["-a", "Microsoft Word", &path])
        .spawn()
        .map_err(|e| format!("Failed to launch Word: {e}"))?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/c", "start", "winword.exe", &path])
        .spawn()
        .map_err(|e| format!("Failed to launch Word: {e}"))?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Err("Opening files in Word is only supported on macOS and Windows".into());
    Ok(())
}

#[tauri::command]
async fn open_ecourts_verify_window(
    app: tauri::AppHandle,
    url: String,
    window_title: String,
    init_script: String,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    if !url.starts_with("https://judgments.ecourts.gov.in/")
        && !url.starts_with("https://ecourts.gov.in/")
        && !url.starts_with("https://scr.sci.gov.in")
    {
        return Err(format!(
            "rejected non-eCourts URL: {url} (this command is only for the official eCourts/SCR portal)"
        ));
    }

    let parsed = url
        .parse::<tauri::Url>()
        .map_err(|e| format!("invalid URL: {e}"))?;

    let label = format!(
        "ecourts-verify-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title(window_title)
        .inner_size(1100.0, 800.0)
        .initialization_script(&init_script)
        .build()
        .map(|_| ())
        .map_err(|e| format!("WebviewWindow build failed: {e}"))
}
