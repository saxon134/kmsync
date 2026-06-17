use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use kmsync_core::{
    DesktopConnectionState, DesktopDeviceState, DesktopLayout, DesktopNetworkState,
    DesktopPeerState, DesktopPermissionState, DesktopRole, DesktopState,
};

use crate::client::DeviceWithPresence;
use crate::desktop_config::DesktopConfig;
use crate::platform::{PermissionStatus, PlatformPermissionCheck};

const PRESENCE_TTL_SECONDS: u64 = 60;

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
    let now = now_seconds();
    let current_item = input.current_device_id.and_then(|current_device_id| {
        input
            .devices
            .iter()
            .find(|item| item.device.id == current_device_id)
    });
    let current_presence = current_item.and_then(|item| item.presence.as_ref());
    let current_sync_relay_status_known = current_item.is_some_and(|item| item.relay.is_some());
    let current_sync_relay_online = current_item
        .and_then(|item| item.relay.as_ref())
        .is_some_and(|relay| relay.rx_online);

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
        display: current_presence.and_then(|presence| presence.display.clone()),
    };

    let effective_role = effective_desktop_role(
        input.current_device_id,
        input.desktop_config.master_device_id.as_deref(),
    );

    let device = DesktopDeviceState {
        id: input.current_device_id.map(str::to_string),
        name: input.device_name.to_string(),
        os: std::env::consts::OS.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        role: effective_role.clone(),
        sync_relay_status_known: current_sync_relay_status_known,
        sync_relay_online: current_sync_relay_online,
    };

    let peer_index = build_peer_state_index(input.current_device_id, input.devices, now);
    let layout = canonicalize_layout_targets(
        &input.desktop_config.layout,
        input.current_device_id,
        &peer_index.canonical_device_ids,
    );
    let master_state = master_connection_state(
        input.current_device_id,
        input.desktop_config.master_device_id.as_deref(),
        &layout,
        input.devices,
        now,
    );
    let master_error = input.master_error.or_else(|| {
        desktop_sync_permission_error(&effective_role, &master_state, &layout, input.permissions)
    });

    DesktopState {
        config_path: Some(input.config_path.display().to_string()),
        device,
        network,
        server_state: input.server_state,
        server_error: input.server_error,
        master_state,
        master_device_id: input.desktop_config.master_device_id.clone(),
        master_error,
        layout,
        devices: peer_index.peers,
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
        sync_runtime: Default::default(),
    }
}

fn desktop_sync_permission_error(
    role: &DesktopRole,
    master_state: &DesktopConnectionState,
    layout: &DesktopLayout,
    permissions: &[PlatformPermissionCheck],
) -> Option<String> {
    let sync_configured = match role {
        DesktopRole::Master => !layout.target_device_ids().is_empty(),
        DesktopRole::Client => !matches!(master_state, DesktopConnectionState::Disconnected),
    };
    if !sync_configured {
        return None;
    }

    let (permission, action) = match role {
        DesktopRole::Master => missing_permission_for(
            permissions,
            &["input_monitoring", "input monitoring", "capture"],
        )
        .or_else(|| missing_permission_for(permissions, &["accessibility"]))
        .map(|permission| (permission, "捕获本机鼠标键盘"))?,
        DesktopRole::Client => missing_permission_for(
            permissions,
            &[
                "accessibility",
                "interactive_desktop",
                "input_injection",
                "injection",
            ],
        )
        .map(|permission| (permission, "接收主电脑输入"))?,
    };

    Some(format!(
        "缺少 {} 权限，无法{}。请在系统设置里给 KMSync.app 开启该权限后重启应用。",
        permission.label, action
    ))
}

fn missing_permission_for<'a>(
    permissions: &'a [PlatformPermissionCheck],
    needles: &[&str],
) -> Option<&'a PlatformPermissionCheck> {
    permissions.iter().find(|permission| {
        permission.status == PermissionStatus::Missing && permission_matches(permission, needles)
    })
}

fn permission_matches(permission: &PlatformPermissionCheck, needles: &[&str]) -> bool {
    let id = permission.id.to_ascii_lowercase();
    let label = permission.label.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| id.contains(needle) || label.contains(needle))
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

fn effective_desktop_role(
    current_device_id: Option<&str>,
    master_device_id: Option<&str>,
) -> DesktopRole {
    crate::desktop_config::role_for_topology(current_device_id, master_device_id)
}

fn master_connection_state(
    current_device_id: Option<&str>,
    master_device_id: Option<&str>,
    _layout: &DesktopLayout,
    devices: &[DeviceWithPresence],
    now: u64,
) -> DesktopConnectionState {
    let Some(master_device_id) = master_device_id else {
        return DesktopConnectionState::Disconnected;
    };

    if current_device_id == Some(master_device_id) {
        return DesktopConnectionState::SelfDevice;
    }

    let master_online = devices
        .iter()
        .find(|item| item.device.id == master_device_id)
        .and_then(|item| item.presence.as_ref())
        .is_some_and(|presence| presence_is_online_at(presence, now));
    if !master_online {
        return DesktopConnectionState::Disconnected;
    }

    DesktopConnectionState::Connecting
}

struct PeerStateIndex {
    peers: Vec<DesktopPeerState>,
    canonical_device_ids: HashMap<String, String>,
}

fn build_peer_state_index(
    current_device_id: Option<&str>,
    devices: &[DeviceWithPresence],
    now: u64,
) -> PeerStateIndex {
    let mut selected: Vec<(String, &DeviceWithPresence)> = Vec::new();
    for item in devices
        .iter()
        .filter(|item| Some(item.device.id.as_str()) != current_device_id)
        .filter(|item| !item.device.disabled)
    {
        let key = peer_dedup_key(item);
        if let Some((_, existing)) = selected
            .iter_mut()
            .find(|(existing_key, _)| *existing_key == key)
        {
            if peer_item_is_better(item, existing, now) {
                *existing = item;
            }
        } else {
            selected.push((key, item));
        }
    }
    let mut canonical_device_ids = HashMap::new();
    for item in devices
        .iter()
        .filter(|item| Some(item.device.id.as_str()) != current_device_id)
        .filter(|item| !item.device.disabled)
    {
        let key = peer_dedup_key(item);
        if let Some((_, selected_item)) = selected
            .iter()
            .find(|(selected_key, _)| *selected_key == key)
        {
            if item.device.id == selected_item.device.id
                || peer_item_is_available(selected_item, now)
            {
                canonical_device_ids
                    .insert(item.device.id.clone(), selected_item.device.id.clone());
            }
        }
    }
    let peers = selected
        .into_iter()
        .map(|(_, item)| peer_state_from_item(item, now))
        .collect();

    PeerStateIndex {
        peers,
        canonical_device_ids,
    }
}

fn canonicalize_layout_targets(
    layout: &DesktopLayout,
    current_device_id: Option<&str>,
    canonical_device_ids: &HashMap<String, String>,
) -> DesktopLayout {
    let mut layout = layout.clone();
    canonicalize_layout_target(&mut layout.left, current_device_id, canonical_device_ids);
    canonicalize_layout_target(&mut layout.right, current_device_id, canonical_device_ids);
    canonicalize_layout_target(&mut layout.top, current_device_id, canonical_device_ids);
    canonicalize_layout_target(&mut layout.bottom, current_device_id, canonical_device_ids);
    layout
}

fn canonicalize_layout_target(
    target: &mut Option<String>,
    current_device_id: Option<&str>,
    canonical_device_ids: &HashMap<String, String>,
) {
    let Some(target_id) = target.as_deref() else {
        return;
    };
    if current_device_id == Some(target_id) {
        return;
    }
    if let Some(canonical_device_id) = canonical_device_ids.get(target_id) {
        *target = Some(canonical_device_id.clone());
    }
}

fn peer_state_from_item(item: &DeviceWithPresence, now: u64) -> DesktopPeerState {
    let presence = item.presence.as_ref();
    let sync_relay_status_known = item.relay.is_some();
    let sync_relay_online = item.relay.as_ref().is_some_and(|relay| relay.rx_online);
    DesktopPeerState {
        id: item.device.id.clone(),
        name: item.device.name.clone(),
        os: item.device.os_type.clone(),
        online: presence.is_some_and(|presence| presence_is_online_at(presence, now)),
        sync_relay_status_known,
        sync_relay_online,
        lan_ips: presence
            .map(|presence| presence.lan_ips.clone())
            .unwrap_or_default(),
        public_ip: presence.map(|presence| presence.public_ip.clone()),
        listen_port: presence.map(|presence| presence.listen_port),
        last_seen_at: presence.map(|presence| presence.last_seen_at),
        display: presence.and_then(|presence| presence.display.clone()),
    }
}

fn peer_dedup_key(item: &DeviceWithPresence) -> String {
    let name = item.device.name.trim().to_ascii_lowercase();
    let os = item.device.os_type.trim().to_ascii_lowercase();
    if let Some(presence) = item.presence.as_ref() {
        let mut lan_ips = presence.lan_ips.clone();
        lan_ips.sort();
        lan_ips.dedup();
        if !lan_ips.is_empty() {
            return format!("name:{name}|os:{os}|lan:{}", lan_ips.join(","));
        }
        if !presence.public_ip.trim().is_empty() {
            return format!(
                "name:{name}|os:{os}|public:{}:{}",
                presence.public_ip, presence.listen_port
            );
        }
    }
    if !item.device.public_key.trim().is_empty() {
        return format!("key:{}", item.device.public_key.trim());
    }
    format!("id:{}", item.device.id)
}

fn peer_item_is_better(
    candidate: &DeviceWithPresence,
    existing: &DeviceWithPresence,
    now: u64,
) -> bool {
    let candidate_online = candidate
        .presence
        .as_ref()
        .is_some_and(|presence| presence_is_online_at(presence, now));
    let existing_online = existing
        .presence
        .as_ref()
        .is_some_and(|presence| presence_is_online_at(presence, now));
    let candidate_relay = candidate
        .relay
        .as_ref()
        .is_some_and(|relay| relay.rx_online);
    let existing_relay = existing.relay.as_ref().is_some_and(|relay| relay.rx_online);
    let candidate_last_seen = candidate
        .presence
        .as_ref()
        .map_or(0, |presence| presence.last_seen_at);
    let existing_last_seen = existing
        .presence
        .as_ref()
        .map_or(0, |presence| presence.last_seen_at);

    (
        candidate_online,
        candidate_relay,
        candidate_last_seen,
        &candidate.device.id,
    ) > (
        existing_online,
        existing_relay,
        existing_last_seen,
        &existing.device.id,
    )
}

fn peer_item_is_available(item: &DeviceWithPresence, now: u64) -> bool {
    item.presence
        .as_ref()
        .is_some_and(|presence| presence_is_online_at(presence, now))
        || item.relay.as_ref().is_some_and(|relay| relay.rx_online)
}

fn presence_is_online_at(presence: &crate::client::Presence, now: u64) -> bool {
    let expires_at = if presence.expires_at == 0 {
        presence.last_seen_at.saturating_add(PRESENCE_TTL_SECONDS)
    } else {
        presence.expires_at
    };
    presence.online && now <= expires_at
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
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
                last_seen_at: now_seconds(),
                expires_at: 0,
                display: None,
            }),
            relay: None,
        }
    }

    #[test]
    fn desktop_state_uses_current_presence_for_public_ip_and_connecting_status() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
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
    fn desktop_state_deduplicates_stale_peer_rows_by_presented_device() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout::default(),
            profile_path: None,
        };
        let mut stale = device_with_presence(
            "stale-device",
            "Studio PC",
            false,
            &["192.168.1.50"],
            "203.0.113.50",
        );
        stale
            .presence
            .as_mut()
            .expect("stale presence")
            .last_seen_at = 10;
        stale.presence.as_mut().expect("stale presence").expires_at = 11;
        let mut live = device_with_presence(
            "live-device",
            "Studio PC",
            true,
            &["192.168.1.50"],
            "203.0.113.50",
        );
        live.presence.as_mut().expect("live presence").last_seen_at = now_seconds();
        live.presence.as_mut().expect("live presence").expires_at = 0;

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "This PC",
            server_url: "https://203.0.113.10:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["192.168.1.20".to_string()],
            desktop_config: &config,
            devices: &[stale, live],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.devices[0].id, "live-device");
        assert!(state.devices[0].online);
    }

    #[test]
    fn desktop_state_canonicalizes_layout_target_to_visible_dedup_peer() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout {
                right: Some("stale-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let mut stale = device_with_presence(
            "stale-device",
            "Studio PC",
            false,
            &["192.168.1.50"],
            "203.0.113.50",
        );
        stale
            .presence
            .as_mut()
            .expect("stale presence")
            .last_seen_at = 10;
        stale.presence.as_mut().expect("stale presence").expires_at = 11;
        let mut live = device_with_presence(
            "live-device",
            "Studio PC",
            true,
            &["192.168.1.50"],
            "203.0.113.50",
        );
        live.presence.as_mut().expect("live presence").last_seen_at = now_seconds();
        live.presence.as_mut().expect("live presence").expires_at = 0;

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "This PC",
            server_url: "https://203.0.113.10:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["192.168.1.20".to_string()],
            desktop_config: &config,
            devices: &[stale, live],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.devices[0].id, "live-device");
        assert_eq!(state.layout.right.as_deref(), Some("live-device"));
    }

    #[test]
    fn desktop_state_preserves_configured_layout_target_when_duplicates_are_offline() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout {
                right: Some("configured-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let mut configured = device_with_presence(
            "configured-device",
            "Studio PC",
            false,
            &["192.168.1.50"],
            "203.0.113.50",
        );
        configured
            .presence
            .as_mut()
            .expect("configured presence")
            .last_seen_at = 10;
        configured
            .presence
            .as_mut()
            .expect("configured presence")
            .expires_at = 11;
        let mut duplicate = device_with_presence(
            "newer-duplicate",
            "Studio PC",
            false,
            &["192.168.1.50"],
            "203.0.113.50",
        );
        duplicate
            .presence
            .as_mut()
            .expect("duplicate presence")
            .last_seen_at = 20;
        duplicate
            .presence
            .as_mut()
            .expect("duplicate presence")
            .expires_at = 21;

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "This PC",
            server_url: "https://203.0.113.10:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["192.168.1.20".to_string()],
            desktop_config: &config,
            devices: &[configured, duplicate],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.devices[0].id, "newer-duplicate");
        assert_eq!(state.layout.right.as_deref(), Some("configured-device"));
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

    #[test]
    fn desktop_state_preserves_peer_relay_receiver_status() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout {
                right: Some("right-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let mut right = device_with_presence(
            "right-device",
            "Right PC",
            true,
            &["10.0.0.5"],
            "203.0.113.45",
        );
        right.relay = Some(crate::client::DeviceRelayStatus { rx_online: true });

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Master Mac",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["10.0.0.4".to_string()],
            desktop_config: &config,
            devices: &[right],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.devices[0].id, "right-device");
        assert!(state.devices[0].sync_relay_status_known);
        assert!(state.devices[0].sync_relay_online);
    }

    #[test]
    fn desktop_state_preserves_current_device_relay_receiver_status() {
        let mut current = device_with_presence(
            "current-device",
            "Current PC",
            true,
            &["10.0.0.8"],
            "203.0.113.46",
        );
        current.relay = Some(crate::client::DeviceRelayStatus { rx_online: false });

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Current PC",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["10.0.0.8".to_string()],
            desktop_config: &DesktopConfig::default(),
            devices: &[current],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert!(state.device.sync_relay_status_known);
        assert!(!state.device.sync_relay_online);
    }

    #[test]
    fn desktop_state_preserves_unknown_peer_relay_receiver_status() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout {
                right: Some("right-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let right = device_with_presence(
            "right-device",
            "Right PC",
            true,
            &["10.0.0.5"],
            "203.0.113.45",
        );

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Master Mac",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["10.0.0.4".to_string()],
            desktop_config: &config,
            devices: &[right],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.devices[0].id, "right-device");
        assert!(!state.devices[0].sync_relay_status_known);
        assert!(!state.devices[0].sync_relay_online);
    }

    #[test]
    fn desktop_state_keeps_client_master_connection_connecting_when_current_device_is_only_in_layout(
    ) {
        let config = DesktopConfig {
            role: DesktopRole::Client,
            master_device_id: Some("master-device".to_string()),
            layout: DesktopLayout {
                right: Some("client-device".to_string()),
                ..DesktopLayout::default()
            },
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

    #[test]
    fn desktop_state_treats_stale_online_master_presence_as_disconnected() {
        let config = DesktopConfig {
            role: DesktopRole::Client,
            master_device_id: Some("master-device".to_string()),
            layout: DesktopLayout {
                right: Some("client-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let mut stale_master = device_with_presence(
            "master-device",
            "Master PC",
            true,
            &["10.0.0.4"],
            "203.0.113.44",
        );
        stale_master
            .presence
            .as_mut()
            .expect("presence")
            .last_seen_at = 1;
        let devices = [stale_master];

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Client PC",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("client-device"),
            local_lan_ips: vec!["10.0.0.5".to_string()],
            desktop_config: &config,
            devices: &devices,
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.master_state, DesktopConnectionState::Disconnected);
        assert_eq!(state.devices[0].id, "master-device");
        assert!(!state.devices[0].online);
    }

    #[test]
    fn desktop_state_treats_expired_server_presence_as_disconnected() {
        let config = DesktopConfig {
            role: DesktopRole::Client,
            master_device_id: Some("master-device".to_string()),
            layout: DesktopLayout::default(),
            profile_path: None,
        };
        let mut expired_master = device_with_presence(
            "master-device",
            "Master PC",
            true,
            &["10.0.0.4"],
            "203.0.113.44",
        );
        expired_master
            .presence
            .as_mut()
            .expect("presence")
            .expires_at = 1;
        let devices = [expired_master];

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Client PC",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("client-device"),
            local_lan_ips: vec!["10.0.0.5".to_string()],
            desktop_config: &config,
            devices: &devices,
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.master_state, DesktopConnectionState::Disconnected);
        assert!(!state.devices[0].online);
    }

    #[test]
    fn desktop_state_does_not_promote_legacy_master_role_without_master_id() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: None,
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
            devices: &[],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.device.role, DesktopRole::Client);
        assert_eq!(state.master_device_id, None);
        assert_eq!(state.master_state, DesktopConnectionState::Disconnected);
    }

    #[test]
    fn desktop_state_marks_current_device_as_master_only_when_topology_points_to_it() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout::default(),
            profile_path: None,
        };

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Master PC",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["10.0.0.4".to_string()],
            desktop_config: &config,
            devices: &[],
            permissions: &[],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        assert_eq!(state.device.role, DesktopRole::Master);
        assert_eq!(state.master_device_id.as_deref(), Some("current-device"));
        assert_eq!(state.master_state, DesktopConnectionState::SelfDevice);
    }

    #[test]
    fn desktop_state_reports_missing_master_capture_permission() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout {
                right: Some("right-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let permission = PlatformPermissionCheck {
            id: "macos.input_monitoring",
            label: "macOS Input Monitoring",
            status: PermissionStatus::Missing,
            guidance: "Grant Input Monitoring permission.",
        };

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Master Mac",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["10.0.0.4".to_string()],
            desktop_config: &config,
            devices: &[device_with_presence(
                "right-device",
                "Right PC",
                true,
                &["10.0.0.5"],
                "203.0.113.45",
            )],
            permissions: &[permission],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        let error = state
            .master_error
            .as_deref()
            .expect("missing capture permission should be surfaced");
        assert!(error.contains("macOS Input Monitoring"));
        assert!(error.contains("无法捕获"));
    }

    #[test]
    fn desktop_state_reports_missing_master_accessibility_permission() {
        let config = DesktopConfig {
            role: DesktopRole::Master,
            master_device_id: Some("current-device".to_string()),
            layout: DesktopLayout {
                right: Some("right-device".to_string()),
                ..DesktopLayout::default()
            },
            profile_path: None,
        };
        let input_monitoring = PlatformPermissionCheck {
            id: "macos.input_monitoring",
            label: "macOS Input Monitoring",
            status: PermissionStatus::Granted,
            guidance: "Grant Input Monitoring permission.",
        };
        let accessibility = PlatformPermissionCheck {
            id: "macos.accessibility",
            label: "macOS Accessibility",
            status: PermissionStatus::Missing,
            guidance: "Grant Accessibility permission.",
        };

        let state = build_desktop_state(DesktopStateBuildInput {
            config_path: Path::new("configs/daemon.example.json"),
            device_name: "Master Mac",
            server_url: "http://kmsync.example.com:24888",
            listen_port: 24_800,
            current_device_id: Some("current-device"),
            local_lan_ips: vec!["10.0.0.4".to_string()],
            desktop_config: &config,
            devices: &[device_with_presence(
                "right-device",
                "Right PC",
                true,
                &["10.0.0.5"],
                "203.0.113.45",
            )],
            permissions: &[input_monitoring, accessibility],
            server_state: DesktopConnectionState::Connected,
            server_error: None,
            master_error: None,
        });

        let error = state
            .master_error
            .as_deref()
            .expect("missing Accessibility should be surfaced for master capture");
        assert!(error.contains("macOS Accessibility"));
        assert!(error.contains("无法捕获"));
    }
}
