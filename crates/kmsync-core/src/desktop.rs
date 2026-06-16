use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopRole {
    Master,
    Client,
}

impl Default for DesktopRole {
    fn default() -> Self {
        Self::Client
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<String>,
}

impl DesktopLayout {
    #[must_use]
    pub fn target_device_ids(&self) -> Vec<&str> {
        [
            self.left.as_deref(),
            self.right.as_deref(),
            self.top.as_deref(),
            self.bottom.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    pub fn validate(&self, current_device_id: Option<&str>) -> Result<(), DesktopLayoutError> {
        let mut seen = BTreeSet::new();
        for device_id in self.target_device_ids() {
            if current_device_id == Some(device_id) {
                return Err(DesktopLayoutError::TargetsCurrentDevice(
                    device_id.to_string(),
                ));
            }
            if !seen.insert(device_id.to_string()) {
                return Err(DesktopLayoutError::DuplicateTarget(device_id.to_string()));
            }
        }
        if seen.len() > 4 {
            return Err(DesktopLayoutError::TooManyTargets(seen.len()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesktopLayoutError {
    DuplicateTarget(String),
    TargetsCurrentDevice(String),
    TooManyTargets(usize),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Retrying,
    SelfDevice,
}

impl Default for DesktopConnectionState {
    fn default() -> Self {
        Self::Disconnected
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopNetworkState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_port: Option<u16>,
    #[serde(default)]
    pub lan_ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopDeviceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub os: String,
    pub app_version: String,
    pub role: DesktopRole,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopPeerState {
    pub id: String,
    pub name: String,
    pub os: String,
    pub online: bool,
    #[serde(default)]
    pub sync_relay_status_known: bool,
    #[serde(default)]
    pub sync_relay_online: bool,
    #[serde(default)]
    pub lan_ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopPermissionState {
    pub key: String,
    pub status: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopSyncRuntimeKind {
    Unknown,
    Idle,
    Listening,
    Armed,
    Failed,
}

impl Default for DesktopSyncRuntimeKind {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopSyncRuntimeState {
    #[serde(default)]
    pub state: DesktopSyncRuntimeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    #[serde(default)]
    pub relay_connected: bool,
    #[serde(default)]
    pub captured_events: u64,
    #[serde(default)]
    pub routed_events: u64,
    #[serde(default)]
    pub sent_events: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sent_at: Option<u64>,
    #[serde(default)]
    pub received_events: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_received_at: Option<u64>,
    #[serde(default)]
    pub injected_events: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_injected_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    pub device: DesktopDeviceState,
    pub network: DesktopNetworkState,
    pub server_state: DesktopConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_error: Option<String>,
    pub master_state: DesktopConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_error: Option<String>,
    pub layout: DesktopLayout,
    #[serde(default)]
    pub devices: Vec<DesktopPeerState>,
    #[serde(default)]
    pub permissions: Vec<DesktopPermissionState>,
    #[serde(default)]
    pub sync_runtime: DesktopSyncRuntimeState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_validation_rejects_duplicate_targets() {
        let layout = DesktopLayout {
            left: Some("device-a".to_string()),
            right: Some("device-a".to_string()),
            top: None,
            bottom: None,
        };

        assert_eq!(
            layout.validate(Some("current-device")),
            Err(DesktopLayoutError::DuplicateTarget("device-a".to_string()))
        );
    }

    #[test]
    fn desktop_state_defaults_missing_sync_runtime_to_unknown() {
        let state: DesktopState = serde_json::from_str(
            r#"{
                "device": {
                    "id": "client-device",
                    "name": "Client",
                    "os": "windows",
                    "app_version": "0.1.0",
                    "role": "client"
                },
                "network": {},
                "server_state": "connected",
                "master_state": "connecting",
                "layout": {}
            }"#,
        )
        .expect("deserialize desktop state");

        assert_eq!(state.sync_runtime.state, DesktopSyncRuntimeKind::Unknown);
        assert_eq!(state.sync_runtime.error, None);
    }

    #[test]
    fn desktop_sync_runtime_serializes_failed_capture_status() {
        let runtime = DesktopSyncRuntimeState {
            state: DesktopSyncRuntimeKind::Failed,
            error: Some("missing Input Monitoring".to_string()),
            targets: vec!["right-device".to_string()],
            updated_at: Some(123),
            ..DesktopSyncRuntimeState::default()
        };

        let json = serde_json::to_string(&runtime).expect("serialize runtime");

        assert!(json.contains(r#""state":"failed""#));
        assert!(json.contains("missing Input Monitoring"));
        assert!(json.contains("right-device"));
    }

    #[test]
    fn desktop_sync_runtime_serializes_input_listener_status() {
        let runtime = DesktopSyncRuntimeState {
            state: DesktopSyncRuntimeKind::Listening,
            error: None,
            targets: Vec::new(),
            updated_at: Some(123),
            ..DesktopSyncRuntimeState::default()
        };

        let json = serde_json::to_string(&runtime).expect("serialize runtime");

        assert!(json.contains(r#""state":"listening""#));
    }

    #[test]
    fn desktop_sync_runtime_serializes_transfer_progress() {
        let runtime = DesktopSyncRuntimeState {
            state: DesktopSyncRuntimeKind::Armed,
            error: None,
            targets: vec!["right-device".to_string()],
            updated_at: Some(123),
            sent_events: 7,
            last_sent_at: Some(120),
            received_events: 5,
            last_received_at: Some(121),
            injected_events: 4,
            last_injected_at: Some(122),
            ..DesktopSyncRuntimeState::default()
        };

        let json = serde_json::to_string(&runtime).expect("serialize runtime");
        let decoded: DesktopSyncRuntimeState =
            serde_json::from_str(&json).expect("deserialize runtime");

        assert_eq!(decoded.sent_events, 7);
        assert_eq!(decoded.last_sent_at, Some(120));
        assert_eq!(decoded.received_events, 5);
        assert_eq!(decoded.last_received_at, Some(121));
        assert_eq!(decoded.injected_events, 4);
        assert_eq!(decoded.last_injected_at, Some(122));
    }

    #[test]
    fn desktop_sync_runtime_serializes_capture_observation_progress() {
        let runtime = DesktopSyncRuntimeState {
            state: DesktopSyncRuntimeKind::Armed,
            error: None,
            targets: vec!["right-device".to_string()],
            updated_at: Some(123),
            captured_events: 12,
            routed_events: 3,
            ..DesktopSyncRuntimeState::default()
        };

        let json = serde_json::to_string(&runtime).expect("serialize runtime");
        let decoded: DesktopSyncRuntimeState =
            serde_json::from_str(&json).expect("deserialize runtime");

        assert_eq!(decoded.captured_events, 12);
        assert_eq!(decoded.routed_events, 3);
    }

    #[test]
    fn layout_validation_rejects_current_device_as_target() {
        let layout = DesktopLayout {
            left: Some("current-device".to_string()),
            right: None,
            top: None,
            bottom: None,
        };

        assert_eq!(
            layout.validate(Some("current-device")),
            Err(DesktopLayoutError::TargetsCurrentDevice(
                "current-device".to_string()
            ))
        );
    }

    #[test]
    fn layout_target_list_keeps_direction_order() {
        let layout = DesktopLayout {
            left: Some("left-device".to_string()),
            right: Some("right-device".to_string()),
            top: Some("top-device".to_string()),
            bottom: Some("bottom-device".to_string()),
        };

        assert_eq!(
            layout.target_device_ids(),
            vec!["left-device", "right-device", "top-device", "bottom-device"]
        );
        assert_eq!(layout.validate(Some("current-device")), Ok(()));
    }
}
