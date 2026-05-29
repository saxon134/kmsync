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
.badge[data-state="disconnected"], .badge[data-state="auth_expired"] {{ color: var(--danger); }}
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
}}
.devices th, .devices td {{
  border-bottom: 1px solid var(--line);
  padding: 9px 8px;
  text-align: left;
  vertical-align: top;
}}
.devices th {{
  color: var(--muted);
  font-size: 12px;
}}
@media (max-width: 820px) {{
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
        <h2>Linux 服务器</h2>
        <div class="body">
          <label class="field"><span>服务器 IP/域名</span><input id="serverHost" name="server_host" value="{server_host}" placeholder="1.2.3.4"></label>
          <label class="field"><span>服务器端口</span><input id="serverPort" name="server_port" type="number" min="1" max="65535" value="{server_port}" placeholder="24888"></label>
          <div class="fact"><span>完整地址</span><strong id="serverUrlPreview" data-scheme="{server_scheme}">{server_url}</strong></div>
        </div>
      </section>
      <section class="panel">
        <h2>网络状态</h2>
        <div class="body facts">
          <div class="fact"><span>内网 IP</span><strong>{lan_ips}</strong></div>
          <div class="fact"><span>公网 IP</span><strong>{public_ip}</strong></div>
          <div class="fact"><span>监听端口</span><strong>{listen_port}</strong></div>
          <div class="fact"><span>最近心跳</span><strong>{last_seen_at}</strong></div>
        </div>
      </section>
      <section class="panel">
        <h2>当前电脑</h2>
        <div class="body">
          <label class="role"><input type="checkbox" {master_checked}> 将当前电脑作为主电脑</label>
          <div class="fact"><span>设备 ID</span><strong>{device_id}</strong></div>
          <div class="fact"><span>系统</span><strong>{device_os}</strong></div>
          <button type="button" class="primary">保存配置</button>
        </div>
      </section>
    </section>
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
        master_state_key = connection_state_key(&state.master_state),
        master_state = connection_state_label(&state.master_state),
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
        public_ip = escape_html(state.network.public_ip.as_deref().unwrap_or("-")),
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
    for device in &state.devices {
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
                if device.online { "在线" } else { "离线" },
                escape_html(&empty_dash(&device.lan_ips.join(", "))),
                escape_html(device.public_ip.as_deref().unwrap_or("-"))
            )
        })
        .collect()
}

fn device_label<'a>(state: &'a DesktopState, device_id: &str) -> Option<&'a str> {
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
        DesktopConnectionState::AuthExpired => "auth_expired",
        DesktopConnectionState::Retrying => "retrying",
        DesktopConnectionState::SelfDevice => "self_device",
    }
}

fn connection_state_label(state: &DesktopConnectionState) -> &'static str {
    match state {
        DesktopConnectionState::Connecting => "连接中",
        DesktopConnectionState::Connected => "已连接",
        DesktopConnectionState::Disconnected => "未连接",
        DesktopConnectionState::AuthExpired => "登录失效",
        DesktopConnectionState::Retrying => "正在重试",
        DesktopConnectionState::SelfDevice => "当前电脑",
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
            },
            network: DesktopNetworkState {
                server_url: Some("http://203.0.113.10:24888".to_string()),
                server_host: Some("203.0.113.10".to_string()),
                server_port: Some(24_888),
                lan_ips: vec!["192.168.1.20".to_string()],
                public_ip: Some("203.0.113.10".to_string()),
                listen_port: Some(24_800),
                last_seen_at: Some(123),
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
                lan_ips: vec!["192.168.1.21".to_string()],
                public_ip: Some("203.0.113.11".to_string()),
                listen_port: Some(24_800),
                last_seen_at: Some(456),
            }],
            ..DesktopState::default()
        };

        let html = render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("Linux 服务器"));
        assert!(html.contains("服务器 IP/域名"));
        assert!(html.contains("服务器端口"));
        assert!(html.contains("serverHost"));
        assert!(html.contains("serverPort"));
        assert!(html.contains("http://203.0.113.10:24888"));
        assert!(html.contains("内网 IP"));
        assert!(html.contains("公网 IP"));
        assert!(html.contains("连接中"));
        assert!(html.contains("将当前电脑作为主电脑"));
        assert!(html.contains("左边电脑"));
        assert!(html.contains("右边电脑"));
        assert!(html.contains("192.168.1.20"));
        assert!(html.contains("203.0.113.10"));
        assert!(html.contains("Right PC"));
        assert!(!html.contains("legacy daemon"));
    }
}
