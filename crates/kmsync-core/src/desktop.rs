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
