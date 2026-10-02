use std::sync::Mutex;

/// Cold-start URL drain (spec §1, per tauri.app/learn/mobile-file-associations).
/// On macOS, `RunEvent::Opened` can fire before the frontend has attached its
/// `listen("opened", ...)` handler -- the OS hands the app a file to open at
/// launch, not after. URLs are stashed here and drained once by the
/// `opened_urls` command on boot; anything that arrives after boot reaches the
/// frontend via the `opened` event instead, emitted from the same RunEvent
/// handler.
#[derive(Default)]
pub struct OpenedUrls(pub Mutex<Vec<tauri::Url>>);

/// Converts a `file://` URL to a plain filesystem path, exactly as needed by
/// `open_model`'s `path: String` -- the webview never sees a raw URL, only
/// plain paths, matching every other entry point.
pub fn url_to_path(url: &tauri::Url) -> Option<String> {
    url.to_file_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn opened_urls(app: tauri::AppHandle) -> Vec<String> {
    use tauri::Manager;
    let state = app.state::<OpenedUrls>();
    let mut guard = state.0.lock().unwrap();
    let paths = guard.iter().filter_map(url_to_path).collect();
    guard.clear();
    paths
}
