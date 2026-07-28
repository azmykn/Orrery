//! Native folder picker via the XDG **FileChooser** portal (`ashpd`).
//!
//! Opens a directory chooser with multi-select enabled when the portal/backend
//! supports it (`directory` + `multiple`). Callers should treat a single-path
//! result as success — some desktops only allow one folder even when asked for
//! many. Cancel returns an empty `Vec`, not an error.

use std::path::PathBuf;

use ashpd::desktop::file_chooser::SelectedFiles;
use ashpd::desktop::ResponseError;
use ashpd::Error as AshpdError;

/// Open a native folder dialog and return the selected absolute paths.
///
/// - **Ok([])** — user cancelled, or accepted with no usable paths.
/// - **Ok(paths)** — one or more directories (multi-select when the portal allows).
/// - **Err** — portal unavailable / request failed.
pub fn pick_folders(title: &str, accept_label: &str) -> Result<Vec<PathBuf>, String> {
    let title = title.to_string();
    let accept_label = accept_label.to_string();
    // Own thread + current-thread runtime: ashpd is built with the async-io
    // zbus backend (same as shortcut/tray) so we must not drive it on the
    // app's multi-thread tokio runtime that other crates may expect.
    std::thread::Builder::new()
        .name("orrery-folder-picker".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("folder picker runtime: {e}"))?;
            rt.block_on(pick_folders_async(&title, &accept_label))
        })
        .map_err(|e| format!("folder picker thread: {e}"))?
        .join()
        .map_err(|_| "folder picker thread panicked".to_string())?
}

async fn pick_folders_async(title: &str, accept_label: &str) -> Result<Vec<PathBuf>, String> {
    let request = SelectedFiles::open_file()
        .title(title)
        .accept_label(accept_label)
        .modal(true)
        .directory(true)
        .multiple(true)
        .send()
        .await
        .map_err(|e| format!("folder picker: {e}"))?;

    let files = match request.response() {
        Ok(files) => files,
        Err(AshpdError::Response(ResponseError::Cancelled)) => return Ok(Vec::new()),
        Err(e) => return Err(format!("folder picker: {e}")),
    };

    let mut paths = Vec::new();
    for uri in files.uris() {
        if let Some(path) = file_uri_to_path(uri.as_str()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Convert a `file://` URI from the portal into a filesystem path.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let enc = if rest.starts_with('/') {
        rest
    } else {
        // file://hostname/path — drop the host segment.
        let slash = rest.find('/')?;
        &rest[slash..]
    };
    Some(PathBuf::from(percent_decode(enc)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{file_uri_to_path, percent_decode};
    use std::path::PathBuf;

    #[test]
    fn decodes_plain_file_uri() {
        assert_eq!(
            file_uri_to_path("file:///home/azmy/Orrery"),
            Some(PathBuf::from("/home/azmy/Orrery"))
        );
    }

    #[test]
    fn decodes_percent_encoded_spaces() {
        assert_eq!(
            percent_decode("/home/azmy/My%20Projects"),
            "/home/azmy/My Projects"
        );
        assert_eq!(
            file_uri_to_path("file:///home/azmy/My%20Projects"),
            Some(PathBuf::from("/home/azmy/My Projects"))
        );
    }

    #[test]
    fn rejects_non_file_uri() {
        assert_eq!(file_uri_to_path("https://example.com"), None);
    }
}
