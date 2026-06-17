use kmsync_core::{DesktopConnectionState, DesktopLayout, DesktopRole, DesktopState};

pub fn render_desktop_panel(state: &DesktopState) -> Result<String, String> {
    let devices_json = serde_json::to_string(&state.devices)
        .map_err(|error| format!("failed to encode desktop devices: {error}"))?
        .replace("</", "<\\/");
    Ok(format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>KMSync</title>
<style>
:root {{
  color-scheme: light;
  --bg: #f6f7f9;
  --panel: #ffffff;
  --ink: #20242b;
  --muted: #667085;
  --line: #d8dde6;
  --accent: #1463ff;
  --ok: #0f8a4b;
  --warn: #b56b00;
  --danger: #c43131;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
}}
* {{ box-sizing: border-box; }}
body {{
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--ink);
  font-size: 14px;
}}
.app {{
  min-height: 100vh;
  display: grid;
  grid-template-rows: auto 1fr;
}}
.topbar {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 20px;
  border-bottom: 1px solid var(--line);
  background: var(--panel);
}}
.brand h1 {{
  margin: 0;
  font-size: 20px;
  line-height: 1.2;
  letter-spacing: 0;
}}
.brand p {{
  margin: 4px 0 0;
  color: var(--muted);
}}
.badges {{
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  justify-content: flex-end;
}}
.badge {{
  min-height: 32px;
  display: inline-flex;
  align-items: center;
  border: 1px solid var(--line);
  border-radius: 7px;
  padding: 0 10px;
  background: #fbfcfe;
}}
.badge[data-state="connected"], .badge[data-state="self_device"] {{ color: var(--ok); }}
.badge[data-state="connecting"], .badge[data-state="retrying"] {{ color: var(--warn); }}
.badge[data-state="disconnected"] {{ color: var(--danger); }}
.content {{
  padding: 18px;
  display: grid;
  gap: 14px;
  max-width: 1180px;
  width: 100%;
}}
.grid {{
  display: grid;
  grid-template-columns: minmax(260px, 0.9fr) minmax(360px, 1.1fr);
  gap: 14px;
  align-items: start;
}}
.secondary-grid {{
  grid-template-columns: minmax(420px, 1fr) minmax(420px, 1fr);
}}
.panel {{
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  min-width: 0;
}}
.panel h2 {{
  margin: 0;
  padding: 13px 14px;
  border-bottom: 1px solid var(--line);
  font-size: 15px;
  letter-spacing: 0;
}}
.body {{
  padding: 14px;
  display: grid;
  gap: 12px;
}}
.facts {{
  display: grid;
  gap: 10px;
}}
.fact {{
  display: grid;
  gap: 4px;
}}
.fact span {{
  color: var(--muted);
  font-size: 12px;
}}
.fact strong {{
  overflow-wrap: anywhere;
}}
.role {{
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 36px;
}}
.layout {{
  display: grid;
  grid-template-columns: minmax(120px, 1fr) minmax(150px, 1fr) minmax(120px, 1fr);
  grid-template-rows: repeat(3, minmax(86px, auto));
  gap: 10px;
}}
.slot {{
  border: 1px dashed var(--line);
  border-radius: 8px;
  background: #fbfcfe;
  padding: 10px;
  display: grid;
  gap: 7px;
}}
.slot strong, .local strong {{ font-size: 13px; }}
.slot span, .local span {{ color: var(--muted); overflow-wrap: anywhere; }}
.top {{ grid-column: 2; grid-row: 1; }}
.left {{ grid-column: 1; grid-row: 2; }}
.local {{ grid-column: 2; grid-row: 2; border: 1px solid var(--accent); border-radius: 8px; padding: 10px; background: #eef4ff; display: grid; gap: 7px; }}
.right {{ grid-column: 3; grid-row: 2; }}
.bottom {{ grid-column: 2; grid-row: 3; }}
select, input, button {{
  min-height: 34px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel);
  color: var(--ink);
  padding: 0 9px;
  font: inherit;
}}
select {{ width: 100%; min-width: 0; }}
input {{ width: 100%; }}
.field {{
  display: grid;
  gap: 6px;
}}
.field span {{
  color: var(--muted);
  font-size: 12px;
}}
button.primary {{
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}}
.devices {{
  width: 100%;
  border-collapse: collapse;
  table-layout: fixed;
}}
.devices th, .devices td {{
  border-bottom: 1px solid var(--line);
  padding: 9px 8px;
  text-align: left;
  vertical-align: top;
  overflow-wrap: anywhere;
}}
.devices th {{
  color: var(--muted);
  font-size: 12px;
}}
@media (max-width: 920px) {{
  .topbar, .grid {{ grid-template-columns: 1fr; display: grid; }}
  .badges {{ justify-content: flex-start; }}
  .layout {{ grid-template-columns: 1fr; grid-template-rows: none; }}
  .top, .left, .local, .right, .bottom {{ grid-column: auto; grid-row: auto; }}
}}
</style>
</head>
<body>
<div class="app" data-app="desktop-panel">
  <header class="topbar">
    <div class="brand">
      <h1>KMSync</h1>
      <p>{device_name} · {device_role}</p>
    </div>
    <div class="badges">
      <div class="badge" data-state="{server_state_key}">服务器：{server_state}</div>
      <div class="badge" data-state="{master_state_key}">主电脑：{master_state}</div>
    </div>
  </header>
  <main class="content">
    <section class="grid">
      <section class="panel">
        <h2>服务器</h2>
        <div class="body">
          <label class="field"><span>服务器 IP/域名</span><input id="serverHost" name="server_host" value="{server_host}" placeholder="1.2.3.4"></label>
          <label class="field"><span>服务器端口</span><input id="serverPort" name="server_port" type="number" min="1" max="65535" value="{server_port}" placeholder="24888"></label>
          <div class="fact"><span>完整地址</span><strong id="serverUrlPreview" data-scheme="{server_scheme}">{server_url}</strong></div>
        </div>
      </section>
      <section class="panel">
        <h2>本机</h2>
        <div class="body">
          <label class="role"><input type="checkbox" {master_checked}> 将本机作为主电脑</label>
          <div class="fact"><span>设备 ID</span><strong>{device_id}</strong></div>
          <div class="fact"><span>系统</span><strong>{device_os}</strong></div>
          <div class="fact"><span>内网 IP</span><strong>{lan_ips}</strong></div>
          <div class="fact"><span>监听端口</span><strong>{listen_port}</strong></div>
          <div class="fact"><span>最近心跳</span><strong>{last_seen_at}</strong></div>
          <button type="button" class="primary">保存配置</button>
        </div>
      </section>
    </section>
    <section class="grid secondary-grid">
      <section class="panel">
        <h2>设备位置</h2>
        <div class="body">
          <div class="layout">
            {layout_slots}
            <div class="local"><strong>主电脑</strong><span>{device_name}</span></div>
          </div>
        </div>
      </section>
      <section class="panel">
        <h2>设备列表</h2>
        <div class="body">
          <table class="devices">
            <thead><tr><th>设备</th><th>状态</th><th>内网 IP</th><th>公网 IP</th></tr></thead>
            <tbody>{device_rows}</tbody>
          </table>
        </div>
      </section>
    </section>
  </main>
</div>
<script type="application/json" id="kmsync-devices">{devices_json}</script>
<script>
const serverHostInput = document.getElementById("serverHost");
const serverPortInput = document.getElementById("serverPort");
const serverUrlPreview = document.getElementById("serverUrlPreview");
function updateServerUrlPreview() {{
  const scheme = serverUrlPreview.dataset.scheme || "http";
  const host = serverHostInput.value.trim();
  const port = serverPortInput.value.trim();
  serverUrlPreview.textContent = host && port ? `${{scheme}}://${{host}}:${{port}}` : "-";
}}
serverHostInput.addEventListener("input", updateServerUrlPreview);
serverPortInput.addEventListener("input", updateServerUrlPreview);
</script>
</body>
</html>
"#,
        device_name = escape_html(&state.device.name),
        device_role = role_label(&state.device.role),
        server_state_key = connection_state_key(&state.server_state),
        server_state = connection_state_label(&state.server_state),
        master_state_key = master_connection_state_key(state),
        master_state = master_connection_state_label(state),
        server_host = escape_html(state.network.server_host.as_deref().unwrap_or("")),
        server_port = state
            .network
            .server_port
            .map_or_else(String::new, |port| port.to_string()),
        server_url = escape_html(state.network.server_url.as_deref().unwrap_or("-")),
        server_scheme = escape_html(&server_url_scheme(
            state.network.server_url.as_deref().unwrap_or("http://")
        )),
        lan_ips = escape_html(&empty_dash(&state.network.lan_ips.join(", "))),
        listen_port = state
            .network
            .listen_port
            .map_or_else(|| "-".to_string(), |port| port.to_string()),
        last_seen_at = state
            .network
            .last_seen_at
            .map_or_else(|| "-".to_string(), |value| value.to_string()),
        master_checked = if state.device.role == DesktopRole::Master {
            "checked"
        } else {
            ""
        },
        device_id = escape_html(state.device.id.as_deref().unwrap_or("-")),
        device_os = escape_html(&state.device.os),
        layout_slots = render_layout_slots(&state.layout, state),
        device_rows = render_device_rows(state),
        devices_json = devices_json,
    ))
}

fn render_layout_slots(layout: &DesktopLayout, state: &DesktopState) -> String {
    [
        ("top", "上方电脑", layout.top.as_deref()),
        ("left", "左边电脑", layout.left.as_deref()),
        ("right", "右边电脑", layout.right.as_deref()),
        ("bottom", "下方电脑", layout.bottom.as_deref()),
    ]
    .into_iter()
    .map(|(class_name, label, selected)| {
        format!(
            r#"<div class="slot {class_name}"><strong>{label}</strong><select>{}</select><span>{}</span></div>"#,
            render_device_options(state, selected),
            escape_html(selected.and_then(|id| device_label(state, id)).unwrap_or("未配置"))
        )
    })
    .collect()
}

fn render_device_options(state: &DesktopState, selected: Option<&str>) -> String {
    let mut options = String::from(r#"<option value="">未配置</option>"#);
    let current_device_id = state.device.id.as_deref();
    if let Some(current_device_id) = current_device_id {
        let selected_attr = if Some(current_device_id) == selected {
            " selected"
        } else {
            ""
        };
        options.push_str(&format!(
            r#"<option value="{}"{}>{}</option>"#,
            escape_html(current_device_id),
            selected_attr,
            escape_html(&state.device.name)
        ));
    }
    for device in state
        .devices
        .iter()
        .filter(|device| Some(device.id.as_str()) != current_device_id)
    {
        let selected_attr = if Some(device.id.as_str()) == selected {
            " selected"
        } else {
            ""
        };
        options.push_str(&format!(
            r#"<option value="{}"{}>{}</option>"#,
            escape_html(&device.id),
            selected_attr,
            escape_html(&device.name)
        ));
    }
    options
}

fn render_device_rows(state: &DesktopState) -> String {
    if state.devices.is_empty() {
        return r#"<tr><td colspan="4">暂无其他设备</td></tr>"#.to_string();
    }

    state
        .devices
        .iter()
        .map(|device| {
            format!(
                "<tr><td>{}<br><small>{}</small></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&device.name),
                escape_html(&device.os),
                desktop_peer_sync_status_label(device),
                escape_html(&empty_dash(&device.lan_ips.join(", "))),
                escape_html(device.public_ip.as_deref().unwrap_or("-"))
            )
        })
        .collect()
}

fn desktop_peer_sync_status_label(device: &kmsync_core::DesktopPeerState) -> &'static str {
    if !device.online {
        "离线"
    } else if !device.sync_relay_status_known {
        "同步接收待验证"
    } else if device.sync_relay_online {
        "同步接收在线"
    } else {
        "同步接收未连接"
    }
}

fn device_label<'a>(state: &'a DesktopState, device_id: &str) -> Option<&'a str> {
    if state.device.id.as_deref() == Some(device_id) {
        return Some(state.device.name.as_str());
    }
    state
        .devices
        .iter()
        .find(|device| device.id == device_id)
        .map(|device| device.name.as_str())
}

fn connection_state_key(state: &DesktopConnectionState) -> &'static str {
    match state {
        DesktopConnectionState::Connecting => "connecting",
        DesktopConnectionState::Connected => "connected",
        DesktopConnectionState::Disconnected => "disconnected",
        DesktopConnectionState::Retrying => "retrying",
        DesktopConnectionState::SelfDevice => "self_device",
    }
}

fn connection_state_label(state: &DesktopConnectionState) -> &'static str {
    match state {
        DesktopConnectionState::Connecting => "连接中",
        DesktopConnectionState::Connected => "已连接",
        DesktopConnectionState::Disconnected => "未连接",
        DesktopConnectionState::Retrying => "正在重试",
        DesktopConnectionState::SelfDevice => "本机",
    }
}

fn master_connection_state_key(state: &DesktopState) -> &'static str {
    if state.master_error.is_some() {
        return "disconnected";
    }
    match state.sync_runtime.state {
        kmsync_core::DesktopSyncRuntimeKind::Armed if should_show_master_runtime(state) => {
            return "retrying";
        }
        kmsync_core::DesktopSyncRuntimeKind::Idle if should_show_master_runtime(state) => {
            return "disconnected";
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening
            if should_show_client_runtime(state)
                && state.master_state != DesktopConnectionState::Disconnected =>
        {
            return "retrying";
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening
            if should_show_client_runtime(state)
                && state.master_state == DesktopConnectionState::Disconnected
                && state.sync_runtime.relay_connected =>
        {
            return "retrying";
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening => {}
        kmsync_core::DesktopSyncRuntimeKind::Idle | kmsync_core::DesktopSyncRuntimeKind::Armed => {}
        kmsync_core::DesktopSyncRuntimeKind::Failed => return "disconnected",
        kmsync_core::DesktopSyncRuntimeKind::Unknown => {}
    }
    if master_layout_has_non_relay_target_outside_local_subnet(state)
        || master_layout_has_unknown_sync_receiver(state)
        || master_layout_waiting_for_sync_receiver(state)
    {
        return "retrying";
    }
    match state.master_state {
        DesktopConnectionState::Connected | DesktopConnectionState::Connecting => "connecting",
        DesktopConnectionState::Disconnected => "disconnected",
        DesktopConnectionState::Retrying => "retrying",
        DesktopConnectionState::SelfDevice => "self_device",
    }
}

fn master_connection_state_label(state: &DesktopState) -> String {
    if state.master_error.is_some() {
        return "需处理".to_string();
    }
    match state.sync_runtime.state {
        kmsync_core::DesktopSyncRuntimeKind::Armed if should_show_master_runtime(state) => {
            return if state.sync_runtime.sent_events > 0 {
                format!("已转发 {}", state.sync_runtime.sent_events)
            } else if master_layout_has_non_relay_target_outside_local_subnet(state) {
                "网络不可直连".to_string()
            } else if master_layout_has_unknown_sync_receiver(state) {
                "服务器待更新".to_string()
            } else if master_layout_waiting_for_sync_receiver(state) {
                "等待从电脑接收端".to_string()
            } else if state.sync_runtime.routed_events > 0 {
                format!("已路由 {}", state.sync_runtime.routed_events)
            } else if state.sync_runtime.captured_events > 0 {
                format!("已捕获 {}", state.sync_runtime.captured_events)
            } else {
                "捕获中".to_string()
            };
        }
        kmsync_core::DesktopSyncRuntimeKind::Idle if should_show_master_runtime(state) => {
            return "等待设备".to_string();
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening
            if should_show_client_runtime(state) && client_relay_receiver_not_online(state) =>
        {
            return "中继未上线".to_string();
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening
            if should_show_client_runtime(state)
                && state.master_state == DesktopConnectionState::Disconnected
                && state.sync_runtime.relay_connected =>
        {
            return "等待主电脑".to_string();
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening
            if should_show_client_runtime(state)
                && state.master_state != DesktopConnectionState::Disconnected =>
        {
            return if state.sync_runtime.injected_events > 0 {
                format!("已注入 {}", state.sync_runtime.injected_events)
            } else if state.sync_runtime.received_events > 0 {
                format!("已接收 {}", state.sync_runtime.received_events)
            } else if !state.sync_runtime.relay_connected {
                "中继连接中".to_string()
            } else {
                "等待输入".to_string()
            };
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening => {}
        kmsync_core::DesktopSyncRuntimeKind::Idle | kmsync_core::DesktopSyncRuntimeKind::Armed => {}
        kmsync_core::DesktopSyncRuntimeKind::Failed => return "需处理".to_string(),
        kmsync_core::DesktopSyncRuntimeKind::Unknown => {}
    }
    if master_layout_has_non_relay_target_outside_local_subnet(state) {
        return "网络不可直连".to_string();
    }
    if master_layout_has_unknown_sync_receiver(state) {
        return "服务器待更新".to_string();
    }
    if master_layout_waiting_for_sync_receiver(state) {
        return "等待从电脑接收端".to_string();
    }
    match state.master_state {
        DesktopConnectionState::Connected | DesktopConnectionState::Connecting => {
            "连接中".to_string()
        }
        DesktopConnectionState::Disconnected => "未连接".to_string(),
        DesktopConnectionState::Retrying => "正在重试".to_string(),
        DesktopConnectionState::SelfDevice => "本机".to_string(),
    }
}

fn should_show_master_runtime(state: &DesktopState) -> bool {
    state.device.role == DesktopRole::Master
        || state.master_state == DesktopConnectionState::SelfDevice
}

fn should_show_client_runtime(state: &DesktopState) -> bool {
    state.device.role == DesktopRole::Client
}

fn client_relay_receiver_not_online(state: &DesktopState) -> bool {
    state.device.role == DesktopRole::Client
        && state.device.sync_relay_status_known
        && !state.device.sync_relay_online
}

fn master_layout_waiting_for_sync_receiver(state: &DesktopState) -> bool {
    if state.device.role != DesktopRole::Master {
        return false;
    }
    state
        .layout
        .target_device_ids()
        .into_iter()
        .any(|target_id| {
            state
                .devices
                .iter()
                .find(|device| device.id == target_id)
                .is_some_and(|device| {
                    device.online && device.sync_relay_status_known && !device.sync_relay_online
                })
        })
}

fn master_layout_has_unknown_sync_receiver(state: &DesktopState) -> bool {
    if state.device.role != DesktopRole::Master {
        return false;
    }
    state
        .layout
        .target_device_ids()
        .into_iter()
        .any(|target_id| {
            state
                .devices
                .iter()
                .find(|device| device.id == target_id)
                .is_some_and(|device| device.online && !device.sync_relay_status_known)
        })
}

fn master_layout_has_non_relay_target_outside_local_subnet(state: &DesktopState) -> bool {
    if state.device.role != DesktopRole::Master || state.network.lan_ips.is_empty() {
        return false;
    }
    state
        .layout
        .target_device_ids()
        .into_iter()
        .filter_map(|target_id| state.devices.iter().find(|device| device.id == target_id))
        .filter(|device| {
            device.online
                && !device.lan_ips.is_empty()
                && (!device.sync_relay_status_known || !device.sync_relay_online)
        })
        .any(|device| {
            device.lan_ips.iter().all(|target_ip| {
                state
                    .network
                    .lan_ips
                    .iter()
                    .all(|local_ip| !same_lan_subnet(local_ip, target_ip))
            })
        })
}

fn same_lan_subnet(left: &str, right: &str) -> bool {
    let Ok(left) = left.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(right) = right.parse::<std::net::IpAddr>() else {
        return false;
    };
    match (left, right) {
        (std::net::IpAddr::V4(left), std::net::IpAddr::V4(right)) => {
            let left = left.octets();
            let right = right.octets();
            left[..3] == right[..3]
        }
        (std::net::IpAddr::V6(left), std::net::IpAddr::V6(right)) => {
            let left = left.segments();
            let right = right.segments();
            left[..4] == right[..4]
        }
        _ => false,
    }
}

fn role_label(role: &DesktopRole) -> &'static str {
    match role {
        DesktopRole::Master => "主电脑",
        DesktopRole::Client => "普通电脑",
    }
}

fn server_url_scheme(server_url: &str) -> String {
    server_url
        .split_once("://")
        .map_or("http", |(scheme, _)| scheme)
        .to_string()
}

fn empty_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use kmsync_core::{
        DesktopConnectionState, DesktopDeviceState, DesktopLayout, DesktopNetworkState,
        DesktopPeerState, DesktopRole, DesktopState,
    };

    #[test]
    fn desktop_panel_renders_status_network_role_and_layout() {
        let state = DesktopState {
            device: DesktopDeviceState {
                id: Some("current-device".to_string()),
                name: "This PC".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Master,
                sync_relay_status_known: false,
                sync_relay_online: false,
            },
            network: DesktopNetworkState {
                server_url: Some("http://203.0.113.10:24888".to_string()),
                server_host: Some("203.0.113.10".to_string()),
                server_port: Some(24_888),
                lan_ips: vec!["192.168.1.20".to_string()],
                public_ip: Some("203.0.113.10".to_string()),
                listen_port: Some(24_800),
                last_seen_at: Some(123),
                display: None,
            },
            server_state: DesktopConnectionState::Connecting,
            master_state: DesktopConnectionState::SelfDevice,
            layout: DesktopLayout {
                left: Some("left-device".to_string()),
                right: Some("right-device".to_string()),
                top: None,
                bottom: None,
            },
            devices: vec![DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                os: "macos".to_string(),
                online: true,
                sync_relay_status_known: true,
                sync_relay_online: true,
                lan_ips: vec!["192.168.1.21".to_string()],
                public_ip: Some("203.0.113.11".to_string()),
                listen_port: Some(24_800),
                last_seen_at: Some(456),
                display: None,
            }],
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("<h2>服务器</h2>"));
        assert!(!html.contains("Linux 服务器"));
        assert!(!html.contains("<h2>网络状态</h2>"));
        assert!(html.contains("服务器 IP/域名"));
        assert!(html.contains("服务器端口"));
        assert!(html.contains("serverHost"));
        assert!(html.contains("serverPort"));
        assert!(html.contains("http://203.0.113.10:24888"));
        assert!(html.contains("内网 IP"));
        assert!(!html.contains("<div class=\"fact\"><span>公网 IP</span>"));
        assert!(html.contains("连接中"));
        assert!(html.contains("将本机作为主电脑"));
        assert!(html.contains("左边电脑"));
        assert!(html.contains("右边电脑"));
        assert!(html.contains("192.168.1.20"));
        assert!(html.contains("203.0.113.10"));
        assert!(html.contains("Right PC"));
        assert!(!html.contains("legacy daemon"));

        let current_device_section = html
            .split("<h2>本机</h2>")
            .nth(1)
            .expect("current computer section");
        assert!(current_device_section.contains("内网 IP"));
        assert!(!current_device_section.contains("<span>公网 IP</span>"));
        assert!(current_device_section.contains("监听端口"));

        let layout_pair = html
            .split("<section class=\"grid secondary-grid\">")
            .nth(1)
            .expect("secondary grid");
        assert!(layout_pair.contains("<h2>设备位置</h2>"));
        assert!(layout_pair.contains("<h2>设备列表</h2>"));
        assert!(html.contains(".secondary-grid {"));
        assert!(html.contains("grid-template-columns: minmax(420px, 1fr) minmax(420px, 1fr);"));
        assert!(html.contains("@media (max-width: 920px)"));
    }

    #[test]
    fn desktop_panel_layout_options_include_current_device() {
        let state = DesktopState {
            device: DesktopDeviceState {
                id: Some("current-device".to_string()),
                name: "This PC".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Master,
                sync_relay_status_known: false,
                sync_relay_online: false,
            },
            devices: vec![
                DesktopPeerState {
                    id: "current-device".to_string(),
                    name: "This PC".to_string(),
                    os: "windows".to_string(),
                    online: true,
                    sync_relay_status_known: true,
                    sync_relay_online: true,
                    lan_ips: vec![],
                    public_ip: None,
                    listen_port: None,
                    last_seen_at: None,
                    display: None,
                },
                DesktopPeerState {
                    id: "right-device".to_string(),
                    name: "Right PC".to_string(),
                    os: "macos".to_string(),
                    online: true,
                    sync_relay_status_known: true,
                    sync_relay_online: true,
                    lan_ips: vec![],
                    public_ip: None,
                    listen_port: None,
                    last_seen_at: None,
                    display: None,
                },
            ],
            ..DesktopState::default()
        };

        let options = render_device_options(&state, None);

        assert!(options.contains(r#"value="current-device""#));
        assert!(options.contains("This PC"));
        assert!(options.contains("right-device"));
    }

    #[test]
    fn desktop_panel_does_not_claim_master_channel_connected_without_session_signal() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connected,
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("服务器：已连接"));
        assert!(html.contains("主电脑：连接中"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_reports_runtime_capture_state() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                error: None,
                targets: vec!["right-device".to_string()],
                updated_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：捕获中"));
    }

    #[test]
    fn desktop_panel_reports_runtime_captured_without_route_progress() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                captured_events: 4,
                routed_events: 0,
                sent_events: 0,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：已捕获 4"));
        assert!(!html.contains("主电脑：捕获中"));
    }

    #[test]
    fn desktop_panel_reports_runtime_routed_without_send_progress() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                captured_events: 6,
                routed_events: 2,
                sent_events: 0,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：已路由 2"));
        assert!(!html.contains("主电脑：捕获中"));
    }

    #[test]
    fn desktop_panel_waits_for_target_sync_receiver() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                role: DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                online: true,
                sync_relay_status_known: true,
                sync_relay_online: false,
                ..kmsync_core::DesktopPeerState::default()
            }],
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                error: None,
                targets: vec!["right-device".to_string()],
                updated_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：等待从电脑接收端"));
        assert!(!html.contains("主电脑：捕获中"));
    }

    #[test]
    fn desktop_panel_marks_unknown_target_sync_receiver() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                role: DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                online: true,
                sync_relay_status_known: false,
                sync_relay_online: false,
                ..kmsync_core::DesktopPeerState::default()
            }],
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                error: None,
                targets: vec!["right-device".to_string()],
                updated_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：服务器待更新"));
        assert!(!html.contains("主电脑：捕获中"));
    }

    #[test]
    fn desktop_panel_marks_unknown_target_sync_receiver_before_runtime_probe() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                role: DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                online: true,
                sync_relay_status_known: false,
                sync_relay_online: false,
                ..kmsync_core::DesktopPeerState::default()
            }],
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：服务器待更新"));
        assert!(!html.contains("主电脑：连接中"));
    }

    #[test]
    fn desktop_panel_marks_target_lan_mismatch_before_capture_state() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                role: DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            network: kmsync_core::DesktopNetworkState {
                lan_ips: vec!["192.168.50.226".to_string()],
                ..kmsync_core::DesktopNetworkState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                online: true,
                sync_relay_status_known: false,
                sync_relay_online: false,
                lan_ips: vec!["192.168.30.99".to_string()],
                ..kmsync_core::DesktopPeerState::default()
            }],
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                error: None,
                targets: vec!["right-device".to_string()],
                updated_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：网络不可直连"));
        assert!(!html.contains("主电脑：捕获中"));
        assert!(!html.contains("主电脑：接收端待验证"));
    }

    #[test]
    fn desktop_peer_sync_status_label_marks_unknown_relay_status() {
        let device = kmsync_core::DesktopPeerState {
            online: true,
            sync_relay_status_known: false,
            sync_relay_online: false,
            ..kmsync_core::DesktopPeerState::default()
        };

        assert_eq!(desktop_peer_sync_status_label(&device), "同步接收待验证");
    }

    #[test]
    fn desktop_panel_reports_runtime_transmit_progress() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                sent_events: 3,
                last_sent_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：已转发 3"));
    }

    #[test]
    fn desktop_panel_reports_client_listener_without_claiming_connected() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                relay_connected: true,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：等待输入"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_trusts_server_relay_status_for_client_receiver() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                sync_relay_status_known: true,
                sync_relay_online: false,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                relay_connected: true,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：中继未上线"));
        assert!(!html.contains("主电脑：等待输入"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_reports_client_relay_connecting() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                relay_connected: false,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：中继连接中"));
        assert!(!html.contains("主电脑：等待输入"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_reports_runtime_receive_progress() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                received_events: 4,
                last_received_at: Some(456),
                relay_connected: true,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：已接收 4"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_reports_runtime_injection_progress() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                received_events: 4,
                last_received_at: Some(456),
                injected_events: 2,
                last_injected_at: Some(789),
                relay_connected: true,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：已注入 2"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_keeps_client_disconnected_when_master_is_offline() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Disconnected,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                relay_connected: false,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：未连接"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_waits_for_master_when_client_relay_is_connected() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Disconnected,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                relay_connected: true,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：等待主电脑"));
        assert!(!html.contains("主电脑：等待输入"));
        assert!(!html.contains("主电脑：已连接"));
    }

    #[test]
    fn desktop_panel_ignores_stale_master_runtime_on_client() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Disconnected,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                targets: vec!["right-device".to_string()],
                updated_at: Some(123),
                captured_events: 5,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：未连接"));
        assert!(!html.contains("主电脑：已捕获"));
        assert!(!html.contains("主电脑：捕获中"));
    }
}
