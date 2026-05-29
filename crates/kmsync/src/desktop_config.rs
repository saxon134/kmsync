use std::fs;
use std::path::{Path, PathBuf};

use kmsync_core::{DesktopLayout, DesktopRole};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::local_config;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopConfig {
    pub(crate) role: DesktopRole,
    pub(crate) master_device_id: Option<String>,
    pub(crate) layout: DesktopLayout,
    pub(crate) profile_path: Option<PathBuf>,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            role: DesktopRole::Client,
            master_device_id: None,
            layout: DesktopLayout::default(),
            profile_path: None,
        }
    }
}

impl DesktopConfig {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        Self::from_json(&text)
    }

    pub(crate) fn from_json(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct RawDesktopConfig {
            #[serde(default)]
            role: DesktopRole,
            #[serde(default)]
            master_device_id: Option<String>,
            #[serde(default)]
            layout: DesktopLayout,
            #[serde(default)]
            profile_path: Option<PathBuf>,
        }

        let raw: RawDesktopConfig = serde_json::from_str(text)
            .map_err(|error| format!("failed to parse desktop config: {error}"))?;
        Ok(Self {
            role: raw.role,
            master_device_id: raw.master_device_id,
            layout: raw.layout,
            profile_path: raw.profile_path,
        })
    }
}

pub(crate) fn set_role_in_config_file(
    path: &Path,
    role: DesktopRole,
    master_device_id: Option<&str>,
) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let updated = set_role_in_config_text(&text, role, master_device_id)?;
    local_config::write_text_atomic(path, &updated)
}

pub(crate) fn set_layout_in_config_file(path: &Path, layout: &DesktopLayout) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let updated = set_layout_in_config_text(&text, layout)?;
    local_config::write_text_atomic(path, &updated)
}

pub(crate) fn set_server_endpoint_in_config_file(
    path: &Path,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let updated = set_server_endpoint_in_config_text(&text, host, port)?;
    local_config::write_text_atomic(path, &updated)
}

pub(crate) fn set_current_device_config_in_config_file(
    path: &Path,
    device_name: &str,
    role: DesktopRole,
    master_device_id: Option<&str>,
) -> Result<(), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let updated =
        set_current_device_config_in_config_text(&text, device_name, role, master_device_id)?;
    local_config::write_text_atomic(path, &updated)
}

fn set_role_in_config_text(
    text: &str,
    role: DesktopRole,
    master_device_id: Option<&str>,
) -> Result<String, String> {
    let mut object = parse_config_object(text)?;
    object.insert(
        "role".to_string(),
        serde_json::to_value(role).map_err(|error| error.to_string())?,
    );
    match master_device_id {
        Some(master_device_id) => {
            object.insert(
                "master_device_id".to_string(),
                Value::String(master_device_id.to_string()),
            );
        }
        None => {
            object.insert("master_device_id".to_string(), Value::Null);
        }
    }
    serialize_config_object(object)
}

fn set_current_device_config_in_config_text(
    text: &str,
    device_name: &str,
    role: DesktopRole,
    master_device_id: Option<&str>,
) -> Result<String, String> {
    let mut object = parse_config_object(text)?;
    let device_name = device_name.trim();
    if device_name.is_empty() {
        return Err("device name must not be empty".to_string());
    }
    object.insert(
        "device_name".to_string(),
        Value::String(device_name.to_string()),
    );
    object.insert(
        "role".to_string(),
        serde_json::to_value(role).map_err(|error| error.to_string())?,
    );
    match master_device_id {
        Some(master_device_id) => {
            object.insert(
                "master_device_id".to_string(),
                Value::String(master_device_id.to_string()),
            );
        }
        None => {
            object.insert("master_device_id".to_string(), Value::Null);
        }
    }
    serialize_config_object(object)
}

pub(crate) fn set_layout_in_config_text(
    text: &str,
    layout: &DesktopLayout,
) -> Result<String, String> {
    let mut object = parse_config_object(text)?;
    object.insert(
        "layout".to_string(),
        serde_json::to_value(layout).map_err(|error| error.to_string())?,
    );
    serialize_config_object(object)
}

pub(crate) fn set_server_endpoint_in_config_text(
    text: &str,
    host: &str,
    port: u16,
) -> Result<String, String> {
    let mut object = parse_config_object(text)?;
    let server_url = build_server_url(object.get("server_url"), host, port)?;
    object.insert("server_url".to_string(), Value::String(server_url));
    serialize_config_object(object)
}

fn parse_config_object(text: &str) -> Result<Map<String, Value>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("failed to parse config: {error}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "desktop config must be a JSON object".to_string())
}

fn build_server_url(
    existing_server_url: Option<&Value>,
    host: &str,
    port: u16,
) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("server host must not be empty".to_string());
    }
    if host.contains("://") || host.contains('/') {
        return Err(
            "server host must be an IP address or domain without scheme or path".to_string(),
        );
    }
    if port == 0 {
        return Err("server port must be between 1 and 65535".to_string());
    }
    let scheme = existing_server_url
        .and_then(Value::as_str)
        .and_then(server_url_scheme)
        .unwrap_or("http");
    Ok(format!("{scheme}://{host}:{port}"))
}

fn server_url_scheme(server_url: &str) -> Option<&str> {
    let (scheme, _) = server_url.split_once("://")?;
    if !scheme.is_empty()
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    {
        Some(scheme)
    } else {
        None
    }
}

fn serialize_config_object(object: Map<String, Value>) -> Result<String, String> {
    serde_json::to_string_pretty(&Value::Object(object))
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| format!("failed to encode desktop config: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kmsync_core::{DesktopLayout, DesktopRole};
    use std::path::PathBuf;

    #[test]
    fn desktop_config_defaults_to_client_role_and_empty_layout() {
        let config = DesktopConfig::from_json(
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Development Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15
            }"#,
        )
        .expect("parse desktop config");

        assert_eq!(config.role, DesktopRole::Client);
        assert_eq!(config.master_device_id, None);
        assert_eq!(config.layout, DesktopLayout::default());
        assert_eq!(config.profile_path, None);
    }

    #[test]
    fn desktop_config_parses_master_role_layout_and_profile_path() {
        let config = DesktopConfig::from_json(
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Development Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15,
                "role": "master",
                "master_device_id": null,
                "layout": {
                    "left": "left-device",
                    "right": "right-device",
                    "top": "top-device",
                    "bottom": "bottom-device"
                },
                "profile_path": "profiles/current.profile.json"
            }"#,
        )
        .expect("parse desktop config");

        assert_eq!(config.role, DesktopRole::Master);
        assert_eq!(config.layout.left.as_deref(), Some("left-device"));
        assert_eq!(config.layout.right.as_deref(), Some("right-device"));
        assert_eq!(config.layout.top.as_deref(), Some("top-device"));
        assert_eq!(config.layout.bottom.as_deref(), Some("bottom-device"));
        assert_eq!(
            config.profile_path,
            Some(PathBuf::from("profiles/current.profile.json"))
        );
    }

    #[test]
    fn applying_layout_preserves_other_config_fields() {
        let updated = set_layout_in_config_text(
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Development Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15,
                "role": "master"
            }"#,
            &DesktopLayout {
                left: Some("left-device".to_string()),
                right: None,
                top: None,
                bottom: Some("bottom-device".to_string()),
            },
        )
        .expect("set layout");
        let json: serde_json::Value = serde_json::from_str(&updated).expect("valid json");

        assert_eq!(json["server_url"], "http://127.0.0.1:24888");
        assert_eq!(json["role"], "master");
        assert_eq!(json["layout"]["left"], "left-device");
        assert_eq!(json["layout"]["bottom"], "bottom-device");
        assert!(json["layout"].get("right").is_none());
    }

    #[test]
    fn applying_server_endpoint_updates_server_url_and_preserves_other_fields() {
        let updated = set_server_endpoint_in_config_text(
            r#"{
                "server_url": "https://old.example.com:24888",
                "device_name": "Development Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15,
                "role": "master"
            }"#,
            "203.0.113.10",
            24_889,
        )
        .expect("set server endpoint");
        let json: serde_json::Value = serde_json::from_str(&updated).expect("valid json");

        assert_eq!(json["server_url"], "https://203.0.113.10:24889");
        assert_eq!(json["device_name"], "Development Mac");
        assert_eq!(json["role"], "master");
    }

    #[test]
    fn applying_server_endpoint_rejects_empty_host() {
        let error = set_server_endpoint_in_config_text(
            r#"{
                "server_url": "http://127.0.0.1:24888"
            }"#,
            "  ",
            24_889,
        )
        .expect_err("empty host is invalid");

        assert!(error.contains("server host"));
    }

    #[test]
    fn applying_current_device_config_updates_name_role_and_preserves_server() {
        let updated = set_current_device_config_in_config_text(
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Old Name",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15,
                "role": "client",
                "master_device_id": "master-device"
            }"#,
            "  New Name  ",
            DesktopRole::Master,
            None,
        )
        .expect("set current device config");
        let json: serde_json::Value = serde_json::from_str(&updated).expect("valid json");

        assert_eq!(json["server_url"], "http://127.0.0.1:24888");
        assert_eq!(json["device_name"], "New Name");
        assert_eq!(json["role"], "master");
        assert!(json["master_device_id"].is_null());
    }

    #[test]
    fn applying_current_device_config_rejects_empty_name() {
        let error = set_current_device_config_in_config_text(
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Old Name"
            }"#,
            "   ",
            DesktopRole::Client,
            None,
        )
        .expect_err("empty device name is invalid");

        assert!(error.contains("device name"));
    }
}
