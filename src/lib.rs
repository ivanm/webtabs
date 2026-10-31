//! Shared core exposed to the native macOS frontend through an owned JSON ABI.
#![allow(dead_code)]
mod badge;
mod config;
mod favicon;
mod links;
mod presets;

use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};

fn dispatch(request: Value) -> Result<Value, String> {
    let text = |key: &str| request[key].as_str().unwrap_or("");
    match text("op") {
        "load" => {
            let cfg = config::Config::load_or_create(&config::config_path())?;
            Ok(json!({"config": cfg, "path": config::config_path(), "data": config::data_dir()}))
        }
        "save" => {
            let cfg: config::Config =
                serde_json::from_value(request["config"].clone()).map_err(|e| e.to_string())?;
            let cfg = config::Config::parse(&cfg.to_toml())?;
            cfg.save(&config::config_path())
                .map_err(|e| e.to_string())?;
            Ok(json!(cfg))
        }
        "route" => Ok(json!(
            links::route(text("home"), text("url")) == links::Target::InApp
        )),
        "badge" => {
            let b = badge::Badge::from_title(text("title"));
            Ok(json!({"label": b.label(), "count": b.count(), "unread": b.is_unread()}))
        }
        "presets" => Ok(json!(
            presets::PRESETS
                .iter()
                .map(|p| json!({"name": p.name, "url": p.url}))
                .collect::<Vec<_>>()
        )),
        "session_key" => {
            let taken = request["taken"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            Ok(json!(config::new_session_key(text("name"), &taken)))
        }
        "favicon_script" => Ok(json!(favicon::COLLECT_LINKS_JS)),
        "favicons" => Ok(json!(favicon::candidates(text("url"), text("links")))),
        "user_agent" => {
            let cfg: config::Config =
                serde_json::from_value(request["config"].clone()).map_err(|e| e.to_string())?;
            let tab: config::Tab =
                serde_json::from_value(request["tab"].clone()).map_err(|e| e.to_string())?;
            Ok(json!(cfg.user_agent_for(&tab)))
        }
        _ => Err("Unknown core operation".into()),
    }
}

/// # Safety
/// `request` must be a valid NUL-terminated UTF-8 string, or null.
/// The returned pointer must be released once with `webtabs_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn webtabs_request(request: *const c_char) -> *mut c_char {
    let result = if request.is_null() {
        Err("Null request".to_string())
    } else {
        unsafe { CStr::from_ptr(request) }
            .to_str()
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str(s).map_err(|e| e.to_string()))
            .and_then(dispatch)
    };
    let response = match result {
        Ok(value) => json!({"ok": true, "result": value}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    CString::new(response.to_string())
        .expect("JSON escapes NUL bytes")
        .into_raw()
}

/// # Safety
/// `response` must be null or a live pointer returned by `webtabs_request`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn webtabs_free(response: *mut c_char) {
    if !response.is_null() {
        drop(unsafe { CString::from_raw(response) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_roundtrip_and_invalid_requests() {
        for (input, expected) in [
            (
                r#"{"op":"route","home":"https://mail.google.com","url":"https://accounts.google.com"}"#,
                true,
            ),
            ("{", false),
            (r#"{"op":"missing"}"#, false),
        ] {
            let input = CString::new(input).unwrap();
            unsafe {
                let output = webtabs_request(input.as_ptr());
                let value: Value =
                    serde_json::from_str(CStr::from_ptr(output).to_str().unwrap()).unwrap();
                assert_eq!(value["ok"], expected);
                webtabs_free(output);
            }
        }
    }
}
