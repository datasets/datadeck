use std::path::Path;
use std::time::Duration;
use url::Url;

pub fn web_url(text: &str) -> Option<Url> {
    let text = text.trim();
    if text.chars().any(char::is_whitespace) {
        return None;
    }
    Url::parse(text)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

pub fn read_webloc(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("The internet shortcut is too large.".into());
    }
    let data = plist::Value::from_reader(&mut file).map_err(|e| e.to_string())?;
    data.as_dictionary()
        .and_then(|dict| dict.get("URL"))
        .and_then(|v| v.as_string())
        .and_then(web_url)
        .map(Into::into)
        .ok_or_else(|| "This internet shortcut has no HTTP or HTTPS URL.".into())
}

pub fn looks_like_table(url: &Url) -> bool {
    Path::new(url.path())
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| {
            matches!(
                s.to_ascii_lowercase().as_str(),
                "csv" | "tsv" | "xls" | "xlsx" | "parquet"
            )
        })
}

// Probe only an ambiguous Open URL or CLI argument. Browser drops save directly,
// and the dataset reader streams known file formats without a preliminary request.
pub fn is_page(url: &Url) -> bool {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(6)))
        .build()
        .new_agent();
    let Ok(mut response) = agent
        .get(url.as_str())
        .header("Range", "bytes=0-511")
        .call()
    else {
        return false;
    };
    let mime = response
        .body()
        .mime_type()
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(mime.as_str(), "text/html" | "application/xhtml+xml") {
        return true;
    }
    if matches!(mime.as_str(), "text/csv" | "text/tab-separated-values") {
        return false;
    }
    use std::io::Read;
    let mut prefix = Vec::new();
    let _ = response
        .body_mut()
        .as_reader()
        .take(512)
        .read_to_end(&mut prefix);
    let text = String::from_utf8_lossy(&prefix);
    let text = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    text.starts_with("<!doctype html") || text.starts_with("<html")
}

#[cfg(target_os = "macos")]
pub fn browser_drops(window: &gpui::Window) -> Option<async_channel::Receiver<String>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::{CStr, c_char, c_void};
    unsafe extern "C" {
        fn dd_install_web_drop(
            view: *mut c_void,
            context: *mut c_void,
            receive: extern "C" fn(*mut c_void, *const c_char),
            release: extern "C" fn(*mut c_void),
        );
    }
    extern "C" fn receive(context: *mut c_void, link: *const c_char) {
        // The native view owns the sender until deallocation. NSString keeps
        // its UTF-8 buffer alive throughout this synchronous callback.
        unsafe {
            let sender = &*context.cast::<async_channel::Sender<String>>();
            if let Ok(text) = CStr::from_ptr(link).to_str()
                && let Some(url) = web_url(text)
            {
                let _ = sender.try_send(url.into());
            }
        }
    }
    extern "C" fn release(context: *mut c_void) {
        unsafe {
            drop(Box::from_raw(
                context.cast::<async_channel::Sender<String>>(),
            ))
        };
    }
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
    else {
        return None;
    };
    let (tx, rx) = async_channel::unbounded::<String>();
    unsafe {
        dd_install_web_drop(
            handle.ns_view.as_ptr(),
            Box::into_raw(Box::new(tx)).cast(),
            receive,
            release,
        )
    };
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_records_stay_in_the_grid_and_single_links_enter_the_library() {
        assert!(web_url("site\turl\nFen\thttps://example.org").is_none());
        assert!(web_url("file:///etc/hosts").is_none());
        assert!(looks_like_table(
            &web_url(" https://example.org/a.CSV?rev=4 ").unwrap()
        ));
        assert!(!looks_like_table(
            &web_url("https://example.org/story").unwrap()
        ));
    }

    #[test]
    fn finder_internet_shortcuts_support_xml_and_binary_plists() {
        let mut dict = plist::Dictionary::new();
        dict.insert("URL".into(), "https://example.org/chart".into());
        let value = plist::Value::Dictionary(dict);
        for binary in [false, true] {
            let path = tempfile::NamedTempFile::new().unwrap();
            if binary {
                value.to_file_binary(path.path()).unwrap()
            } else {
                value.to_file_xml(path.path()).unwrap()
            }
            assert_eq!(
                read_webloc(path.path()).unwrap(),
                "https://example.org/chart"
            );
        }
    }
}
