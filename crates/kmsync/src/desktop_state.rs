use std::path::Path;

use kmsync_core::{
    DesktopConnectionState, DesktopDeviceState, DesktopNetworkState, DesktopPeerState,
    DesktopPermissionState, DesktopRole, DesktopState,
};

use crate::client::DeviceWithPresence;
use crate::desktop_config::DesktopConfig;
use crate::platform::PlatformPermissionCheck;

pub(crate) struct DesktopStateBuildInput<'a> {
    pub(crate) config_path: &'a Path,
    pub(crate) device_name: &'a str,
    pub(crate) server_url: &'a str,
    pub(crate) listen_port: u16,
    pub(crate) current_device_id: Option<&'a str>,
    pub(crate) local_lan_ips: Vec<String>,
    pub(crate) desktop_config: &'a DesktopConfig,
    pub(crate) devices: &'a [DeviceWithPresence],
    pub(crate) permissions: &'a [PlatformPermissionCheck],
    pub(crate) server_state: DesktopConnectionState,
    pub(crate) server_error: Option<String>,
    pub(crate) master_error: Option<String>,
}

pub(crate) fn build_desktop_state(input: DesktopStateBuildInput<'_>) -> DesktopState {
    let current_presence = input
        .current_device_id
        .and_then(|current_device_id| {
            input
                .devices
                .iter()
                .find(|item| item.device.id == current_device_id)
        })
        .and_then(|item| item.presence.as_ref());

    let (server_host, server_port) = server_endpoint_parts(input.server_url);
    let network = DesktopNetworkState {
        server_url: Some(input.server_url.to_string()),
        server_host,
        server_port,
        lan_ips: current_presence
            .map(|presence| presence.lan_ips.clone())
            .filter(|ips| !ips.is_empty())
            .unwrap_or(input.local_lan_ips),
        public_ip: current_presence.map(|presence| presence.public_ip.clone()),
        listen_port: Some(
            current_presence.map_or(input.listen_port, |presence| presence.listen_port),
        ),
        last_seen_at: current_presence.map(|presence| presence.last_seen_at),
    };

    let device = DesktopDeviceState {
        id: input.current_device_id.map(str::to_string),
        name: input.device_name.to_string(),
        os: std::env::consts::OS.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        role: input.desktop_config.role.clone(),
    };

    let master_state = master_connection_state(
        input.desktop_config.role.clone(),
        input.desktop_config.master_device_id.as_deref(),
        input.devices,
    );

    DesktopState {
        config_path: Some(input.config_path.display().to_string()),
        device,
        network,
        server_state: input.server_state,
        server_error: input.server_error,
        master_state,
        master_device_id: input.desktop_config.master_device_id.clone(),
        master_error: input.master_error,
        layout: input.desktop_config.layout.clone(),
        devices: peer_states(input.current_device_id, input.devices),
        permissions: input
            .permissions
            .iter()
            .map(|permission| DesktopPermissionState {
                key: permission.id.to_string(),
                status: permission.status.as_str().to_string(),
                label: permission.label.to_string(),
                guidance: if permission.guidance.is_empty() {
                    None
                } else {
                    Some(permission.guidance.to_string())
                },
            })
            .collect(),
    }
}

fn server_endpoint_parts(server_url: &str) -> (Option<String>, Option<u16>) {
    let authority = server_url
        .split_once("://")
        .map_or(server_url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim();
    if authority.is_empty() {
        return (None, None);
    }
    if let Some(after_bracket) = authority.strip_prefix('[') {
        if let Some((host, rest)) = after_bracket.split_once(']') {
            let port = rest.strip_prefix(':').and_then(parse_port);
            return (Some(host.to_string()), port);
        }
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => (Some(host.to_string()), parse_port(port)),
        _ => (Some(authority.to_string()), None),
    }
}

fn parse_port(port: &str) -> Option<u16> {
    port.parse::<u16>().ok().filter(|port| *port > 0)
}

fn master_connection_state(
    role: DesktopRole,
    master_device_id: Option<&str>,
    devices: &[DeviceWithPresence],
) -> DesktopConnectionState {
    if role == DesktopRole::Master {
        return DesktopConnectionState::SelfDevice;
    }

    let Some(master_device_id) = master_device_id else {
        return DesktopConnectionState::Disconnected;
    };

    devices
        .iter()
        .find(|item| item.device.id == master_device_id)
        .and_then(|item| item.presence.as_ref())
        .filter(|presence| presence.online)
        .map_or(DesktopConnectionState::Disconnected, |_| {
            DesktopConnectionState::Connecting
        })
}

fn peer_states(
    current_device_id: Option<&str>,
    devices: &[DeviceWithPresence],
) -> Vec<DesktopPeerState> {
    devices
        .iter()
        .filter(|item| Some(item.device.id.as_str()) != current_device_id)
        .map(|item| {
            let presence = item.presence.as_ref();
            DesktopPeerState {
                id: item.device.id.clone(),
                name: item.device.name.clone(),
                os: item.device.os_type.clone(),
                online: presence.is_some_and(|presence| presence.online),
                lan_ips: presence
                    .map(|presence| presence.lan_ips.clone())
                    .unwrap_or_default(),
                public_ip: presence.map(|presence| presence.public_ip.clone()),
                listen_port: presence.map(|presence| presence.listen_port),
                last_seen_at: presence.map(|presence| presence.last_seen_at),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Device, DeviceWithPresence, Presence};
    use crate::desktop_config::DesktopConfig;
    use crate::platform::{PermissionStatus, PlatformPermissionCheck};
    use kmsync_core::{DesktopConnectionState, DesktopLayout, DesktopRole};
    use std::path::Path;

    fn device_with_presence(
        id: &str,
        name: &str,
        online: bool,
        lan_ips: &[&str],
        public_ip: &str,
    ) -> DeviceWithPresence {
        DeviceWithPresence {
            device: Device {
                id: id.to_string(),
                name: name.to_string(),
                os_type: "windows".to_string(),
                os_version: "unknown".to_string(),
                app_version: "0.1.0".to_string(),
                public_key: "ed25519:abc".to_string(),
                disabled: false,
            },
            presence: Some(Presence {
                online,
                lan_ips: lan_ips.iter().map(|ip| (*ip).to_string()).collect(),
                public_ip: public_ip.to_string(),
                listen_port: 24_800,
                nat_type: "unknown".to_string(),
                last_seen_at: 123,
            }),
        }
    }

    #[test]
    fn desktop_state_uses_current_presence_for_public_ip_and_connecting_status() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: None,
            layout: DesktopLayout {
                right: Some("right-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let permission = PlatformPermissionCheck {
            id: "windows.interactive_desktop",
            label: "Windows interactive desktop",
            status: PermissionStatus::Granted,
            guidance: "Run as interactive user.",
        };

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "This PC",
            server_url: "https://203.0.113.10:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["192.168.1.20".to_string()],
            desktop_config: &config,
            devices: &[
                device_with_presence(
                    "current-device",
                    "This PC",
                    true,
                    &["192.168.1.20"],
                    "203.0.113.10",
                ),
                device_with_presence(
                    "right-device",
                    "Right PC",
                    false,
                    &["192.168.1.30"],
                    "203.0.113.30",
                ),
            ],
            permissions: &[permission],
            server_state: DesktopConnectionState::Connecting,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.device.name, "This PC");
        assert_eq!(state.device.role, DesktopRole::Master);
        assert_eq!(state.server_state, DesktopConnectionState::Connecting);
        assert_eq!(state.master_state, DesktopConnectionState::SelfDevice);
        assert_eq!(state.network.lan_ips, vec!["192.168.1.20"]);
        assert_eq!(state.network.public_ip.as_deref(), Some("203.0.113.10"));
        assert_eq!(
            state.network.server_url.as_deref(),
            Some("https://203.0.113.10:24888")
        );
        assert_eq!(state.network.server_host.as_deref(), Some("203.0.113.10"));
        assert_eq!(state.network.server_port, Some(24_888));
        assert_eq!(state.layout.right.as_deref(), Some("right-device"));
        assert_eq!(state.devices.len(), 1);
        assert!(!state.devices[0].online);
        assert_eq!(state.permissions[0].status, "granted");
    }

    #[test]
    fn desktop_state_marks_client_master_connection_connecting_when_master_is_online() {
        let config = DesktopConfig {
            role: DesktopRole::Client,
            master_device_id: Some("master-device".to_string()),
            layout: DesktopLayout::default(),
            profile_path: None,
        };

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Client PC",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("client-device"),
            local_lan_ips: vec!["10.0.0.5".to_string()],
            desktop_config: &config,
            devices: &[device_with_presence(
                "master-device",
                "Master PC",
                true,
                &["10.0.0.4"],
                "203.0.113.44",
            )],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.master_device_id.as_deref(), Some("master-device"));
        assert_eq!(state.master_state, DesktopConnectionState::Connecting);
    }
}
