use kmsync_core::{FunctionKeyMode, HabitPreset, KeyboardMode, Profile, ScreenEdge};

pub fn render_control_panel(profile_text: &str) -> Result<String, String> {
    let profile = Profile::from_config_json(profile_text)
        .map_err(|error| format!("invalid profile config: {error:?}"))?;
    let profile_json = serde_json::from_str::<serde_json::Value>(profile_text)
        .map_err(|error| format!("invalid profile json: {error}"))?;
    let data = serde_json::json!({
        "profile": profile_json,
        "profile_summary": {
            "source_os": os_label(profile.source_os),
            "target_os": os_label(profile.target_os),
            "preset": preset_value(profile.preset),
            "keyboard_mode": keyboard_mode_value(profile.keyboard_mode),
            "function_key_mode": function_key_mode_value(profile.function_key_mode),
            "pointer_speed": profile.pointer.speed_multiplier,
            "scroll_vertical": profile.scroll.vertical_multiplier,
            "scroll_horizontal": profile.scroll.horizontal_multiplier,
        },
        "layout_edges": {
            "left": profile.device_layout.target_for_edge(ScreenEdge::Left),
            "right": profile.device_layout.target_for_edge(ScreenEdge::Right),
            "top": profile.device_layout.target_for_edge(ScreenEdge::Top),
            "bottom": profile.device_layout.target_for_edge(ScreenEdge::Bottom),
        },
        "targets": profile.device_layout.targets.iter().map(|target| {
            serde_json::json!({
                "device_id": target.device_id,
                "display_name": target.display_name.as_deref().unwrap_or(&target.device_id),
            })
        }).collect::<Vec<_>>(),
    });
    let data = serde_json::to_string(&data)
        .map_err(|error| format!("failed to encode control panel data: {error}"))?
        .replace("</", "<\\/");
    let target_options = profile
        .device_layout
        .targets
        .iter()
        .map(|target| {
            let label = target.display_name.as_deref().unwrap_or(&target.device_id);
            format!(
                r#"<option value="{}">{}</option>"#,
                escape_html(&target.device_id),
                escape_html(label)
            )
        })
        .collect::<String>();

    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>KMSync Control Panel</title>
<style>
:root {{
  color-scheme: light;
  --bg: oklch(97% 0.006 232);
  --panel: oklch(99% 0.004 232);
  --panel-2: oklch(94% 0.009 232);
  --ink: oklch(24% 0.018 232);
  --muted: oklch(50% 0.016 232);
  --line: oklch(84% 0.012 232);
  --accent: oklch(52% 0.15 248);
  --accent-soft: oklch(93% 0.035 248);
  --ok: oklch(56% 0.13 154);
  --warn: oklch(62% 0.15 62);
  --danger: oklch(55% 0.16 28);
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
  grid-template-columns: 236px minmax(0, 1fr);
}}
.rail {{
  padding: 16px 12px;
  border-right: 1px solid var(--line);
  background: var(--panel);
}}
.brand {{
  display: grid;
  gap: 4px;
  padding: 4px 8px 14px;
}}
.brand strong {{
  font-size: 18px;
  letter-spacing: 0;
}}
.brand span {{
  color: var(--muted);
  font-size: 12px;
}}
.nav {{
  display: grid;
  gap: 4px;
}}
.nav button {{
  justify-content: flex-start;
  width: 100%;
  border-color: transparent;
  background: transparent;
}}
.nav button[aria-current="page"] {{
  background: var(--accent-soft);
  border-color: oklch(84% 0.04 248);
  color: var(--accent);
}}
main {{
  min-width: 0;
  display: grid;
  grid-template-rows: auto 1fr;
}}
.topbar {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 14px 20px;
  border-bottom: 1px solid var(--line);
  background: var(--panel);
}}
h1 {{
  margin: 0;
  font-size: 20px;
  line-height: 1.2;
  font-weight: 720;
  letter-spacing: 0;
}}
.context {{
  color: var(--muted);
  font-size: 12px;
}}
.content {{
  padding: 18px;
}}
.view {{
  display: none;
  max-width: 1180px;
}}
.view.active {{
  display: grid;
  gap: 14px;
}}
.grid-2 {{
  display: grid;
  grid-template-columns: minmax(280px, 0.8fr) minmax(360px, 1.2fr);
  gap: 14px;
  align-items: start;
}}
.section {{
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  min-width: 0;
}}
.section-head {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 13px 14px;
  border-bottom: 1px solid var(--line);
}}
.section-head h2 {{
  margin: 0;
  font-size: 15px;
  line-height: 1.2;
}}
.section-body {{
  padding: 14px;
  display: grid;
  gap: 12px;
}}
.form-grid {{
  display: grid;
  grid-template-columns: repeat(2, minmax(160px, 1fr));
  gap: 12px;
}}
label {{
  display: grid;
  gap: 6px;
  color: var(--muted);
  font-size: 12px;
}}
input, select, textarea {{
  width: 100%;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel);
  color: var(--ink);
  min-height: 34px;
  padding: 7px 9px;
  font: inherit;
}}
textarea {{
  min-height: 320px;
  resize: vertical;
  font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  line-height: 1.55;
  background: oklch(96% 0.006 232);
}}
button {{
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  min-height: 34px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel);
  color: var(--ink);
  padding: 0 11px;
  font: inherit;
  cursor: pointer;
}}
button:hover {{ border-color: var(--accent); }}
button:focus-visible, input:focus-visible, select:focus-visible, textarea:focus-visible {{
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}}
.primary {{
  background: var(--accent);
  border-color: var(--accent);
  color: oklch(98% 0.006 248);
}}
.actions {{
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}}
.status {{
  min-height: 34px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel-2);
  padding: 8px 10px;
  color: var(--muted);
  overflow-wrap: anywhere;
}}
.status[data-tone="ok"] {{ color: var(--ok); }}
.status[data-tone="warn"] {{ color: var(--warn); }}
.table {{
  width: 100%;
  border-collapse: collapse;
}}
.table th, .table td {{
  border-bottom: 1px solid var(--line);
  padding: 9px 8px;
  text-align: left;
  vertical-align: top;
}}
.table th {{
  color: var(--muted);
  font-size: 12px;
  font-weight: 620;
}}
.edge-grid {{
  display: grid;
  grid-template-columns: repeat(2, minmax(160px, 1fr));
  gap: 10px;
}}
.permission-list {{
  display: grid;
  gap: 8px;
}}
.permission-item {{
  display: grid;
  gap: 3px;
  border: 1px solid var(--line);
  border-radius: 7px;
  padding: 10px;
  background: var(--panel-2);
}}
.permission-item code, .command {{
  font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}}
.command {{
  padding: 10px;
  border-radius: 7px;
  background: oklch(26% 0.018 232);
  color: oklch(95% 0.01 232);
  overflow-x: auto;
}}
@media (max-width: 920px) {{
  .app {{ grid-template-columns: 1fr; }}
  .rail {{
    border-right: 0;
    border-bottom: 1px solid var(--line);
  }}
  .nav {{
    grid-template-columns: repeat(2, minmax(120px, 1fr));
  }}
  .grid-2, .form-grid, .edge-grid {{
    grid-template-columns: 1fr;
  }}
}}
</style>
</head>
<body>
<div class="app" data-app="control-panel">
  <aside class="rail">
    <div class="brand"><strong>KMSync</strong><span>Local control panel</span></div>
    <nav class="nav" aria-label="Control panel sections">
      <button type="button" data-nav="login" aria-current="page">Login</button>
      <button type="button" data-nav="devices">Devices</button>
      <button type="button" data-nav="layout">Layout</button>
      <button type="button" data-nav="habits">Habits</button>
      <button type="button" data-nav="clipboard">Clipboard</button>
      <button type="button" data-nav="network">Network</button>
      <button type="button" data-nav="permissions">Permissions</button>
    </nav>
  </aside>
  <main>
    <header class="topbar">
      <div>
        <h1 id="viewTitle">Login</h1>
        <div class="context">{} to {}, {} mode</div>
      </div>
      <button type="button" class="primary" id="copyProfile">Copy Profile JSON</button>
    </header>
    <div class="content">
      <section class="view active" data-view="login">
        <div class="grid-2">
          <section class="section">
            <div class="section-head"><h2>Email login</h2></div>
            <div class="section-body">
              <label>Server URL<input id="serverUrl" value="http://127.0.0.1:24888"></label>
              <label>Email<input id="email" type="email" value="dev@example.com"></label>
              <div class="actions">
                <button type="button" id="startLogin">Send code</button>
                <button type="button" class="primary" id="verifyLogin">Verify</button>
              </div>
              <label>Code<input id="emailCode" autocomplete="one-time-code"></label>
              <div class="status" id="authStatus">Not signed in</div>
            </div>
          </section>
          <section class="section">
            <div class="section-head"><h2>Session</h2></div>
            <div class="section-body">
              <label>Access token<textarea id="accessToken" spellcheck="false"></textarea></label>
              <label>Refresh token<textarea id="refreshToken" spellcheck="false"></textarea></label>
            </div>
          </section>
        </div>
      </section>
      <section class="view" data-view="devices">
        <section class="section">
          <div class="section-head"><h2>Device list</h2><button type="button" id="loadDevices">Refresh</button></div>
          <div class="section-body">
            <table class="table" aria-label="Devices"><thead><tr><th>Name</th><th>OS</th><th>Presence</th><th>Device ID</th></tr></thead><tbody id="deviceRows"></tbody></table>
            <div class="status" id="deviceStatus">Sign in, then refresh devices.</div>
          </div>
        </section>
      </section>
      <section class="view" data-view="layout">
        <div class="grid-2">
          <section class="section">
            <div class="section-head"><h2>Edge bindings</h2></div>
            <div class="section-body edge-grid">
              {}
            </div>
          </section>
          <section class="section">
            <div class="section-head"><h2>Profile JSON</h2></div>
            <div class="section-body"><textarea id="profileJson" spellcheck="false"></textarea></div>
          </section>
        </div>
      </section>
      <section class="view" data-view="habits">
        <section class="section">
          <div class="section-head"><h2>Keyboard and pointer habits</h2></div>
          <div class="section-body form-grid">
            <label>Preset<select id="preset" data-profile-path="preset"><option value="keep_mac_habit">Keep Mac habit</option><option value="keep_windows_habit">Keep Windows habit</option><option value="keep_linux_habit">Keep Linux habit</option><option value="target_os_habit">Target OS habit</option><option value="custom">Custom</option></select></label>
            <label>keyboard_mode<select id="keyboardMode" data-profile-path="keyboard_mode"><option value="physical">Physical</option><option value="text">Text</option></select></label>
            <label>function_key_mode<select id="functionKeyMode" data-profile-path="function_key_mode"><option value="standard">Standard F1-F12</option><option value="media">System media</option></select></label>
            <label>Pointer speed<input id="pointerSpeed" type="number" step="0.05" data-profile-path="pointer.speed_multiplier"></label>
            <label>Vertical scroll<input id="scrollVertical" type="number" step="0.05" data-profile-path="scroll.vertical_multiplier"></label>
            <label>Horizontal scroll<input id="scrollHorizontal" type="number" step="0.05" data-profile-path="scroll.horizontal_multiplier"></label>
          </div>
        </section>
      </section>
      <section class="view" data-view="clipboard">
        <section class="section">
          <div class="section-head"><h2>Clipboard sync</h2></div>
          <div class="section-body form-grid">
            <label>Sync<select id="clipboardEnabled"><option value="enabled">Enabled</option><option value="disabled">Disabled</option></select></label>
            <label>Max bytes<input id="clipboardMaxBytes" type="number" value="1048576"></label>
            <label>Expiry seconds<input id="clipboardTtlSeconds" type="number" value="300"></label>
            <label>Sensitive apps<input id="clipboardSensitiveApps" value="OnePassword,Bitwarden,KeePassXC"></label>
          </div>
          <div class="section-body">
            <div class="command" id="clipboardCommand">kmsync clip-watch &lt;target-ip&gt;:24800 1 1048576 enabled 300 OnePassword,Bitwarden,KeePassXC</div>
          </div>
        </section>
      </section>
      <section class="view" data-view="network">
        <div class="grid-2">
          <section class="section">
            <div class="section-head"><h2>Network diagnostics</h2><button type="button" id="checkHealth">Check server</button></div>
            <div class="section-body">
              <div class="status" id="networkStatus">Ready</div>
              <div class="command">kmsync connection-diagnostics configs/daemon.example.json &lt;target_device_id&gt;</div>
              <div class="command">kmsync self-test mac-to-windows</div>
            </div>
          </section>
          <section class="section">
            <div class="section-head"><h2>Local IPC</h2></div>
            <div class="section-body">
              <div class="command">kmsync status</div>
              <div class="command">kmsync ipc-ping</div>
            </div>
          </section>
        </div>
      </section>
      <section class="view" data-view="permissions">
        <section class="section">
          <div class="section-head"><h2>Permission guide</h2></div>
          <div class="section-body permission-list">
            <div class="permission-item"><strong>macos.accessibility</strong><span>Grant Accessibility so remote keyboard and mouse injection can run.</span></div>
            <div class="permission-item"><strong>macos.input_monitoring</strong><span>Grant Input Monitoring so global capture can run.</span></div>
            <div class="permission-item"><strong>windows.interactive_desktop</strong><span>Run the companion in the signed-in user session for hooks and SendInput.</span></div>
            <div class="permission-item"><strong>linux.x11_backend</strong><span>X11 capture uses XInput2 raw events and injection uses XTest when both extensions are available.</span></div>
          </div>
        </section>
      </section>
    </div>
  </main>
</div>
<script>
const controlData = {};
let profile = structuredClone(controlData.profile);
const targets = controlData.targets;
const edgeNames = ["left", "right", "top", "bottom"];
const tokenBox = document.getElementById("accessToken");
const refreshBox = document.getElementById("refreshToken");
const profileJson = document.getElementById("profileJson");

function serverUrl() {{
  return document.getElementById("serverUrl").value.replace(/\/$/, "");
}}

function setStatus(id, text, tone) {{
  const element = document.getElementById(id);
  element.textContent = text;
  element.dataset.tone = tone || "";
}}

async function api(path, options = {{}}) {{
  const headers = Object.assign({{ "content-type": "application/json" }}, options.headers || {{}});
  const token = tokenBox.value.trim();
  if (token) headers.authorization = `Bearer ${{token}}`;
  const response = await fetch(`${{serverUrl()}}${{path}}`, Object.assign({{}}, options, {{ headers }}));
  const body = await response.json().catch(() => ({{}}));
  if (!response.ok) throw new Error(body.error || `HTTP ${{response.status}}`);
  return body;
}}

function syncProfileJson() {{
  profileJson.value = JSON.stringify(profile, null, 2);
}}

function setPath(path, value) {{
  const parts = path.split(".");
  let target = profile;
  while (parts.length > 1) {{
    const part = parts.shift();
    target[part] = target[part] || {{}};
    target = target[part];
  }}
  target[parts[0]] = value;
  syncProfileJson();
}}

function updateClipboardCommand() {{
  const bytes = document.getElementById("clipboardMaxBytes").value;
  const enabled = document.getElementById("clipboardEnabled").value;
  const ttl = document.getElementById("clipboardTtlSeconds").value;
  const apps = document.getElementById("clipboardSensitiveApps").value;
  document.getElementById("clipboardCommand").textContent = `kmsync clip-watch <target-ip>:24800 1 ${{bytes}} ${{enabled}} ${{ttl}} ${{apps}}`;
}}

document.querySelectorAll("[data-nav]").forEach((button) => {{
  button.addEventListener("click", () => {{
    document.querySelectorAll("[data-nav]").forEach((item) => item.removeAttribute("aria-current"));
    button.setAttribute("aria-current", "page");
    document.querySelectorAll("[data-view]").forEach((view) => view.classList.toggle("active", view.dataset.view === button.dataset.nav));
    document.getElementById("viewTitle").textContent = button.textContent;
  }});
}});

document.getElementById("startLogin").addEventListener("click", async () => {{
  try {{
    const body = await api("/v1/auth/email/start", {{
      method: "POST",
      body: JSON.stringify({{ email: document.getElementById("email").value }})
    }});
    setStatus("authStatus", `Code sent, expires_at=${{body.expires_at}}`, "ok");
  }} catch (error) {{
    setStatus("authStatus", error.message, "warn");
  }}
}});

document.getElementById("verifyLogin").addEventListener("click", async () => {{
  try {{
    const body = await api("/v1/auth/email/verify", {{
      method: "POST",
      body: JSON.stringify({{ email: document.getElementById("email").value, code: document.getElementById("emailCode").value }})
    }});
    tokenBox.value = body.access_token || "";
    refreshBox.value = body.refresh_token || "";
    setStatus("authStatus", `Signed in as ${{body.user_id}}`, "ok");
  }} catch (error) {{
    setStatus("authStatus", error.message, "warn");
  }}
}});

document.getElementById("loadDevices").addEventListener("click", async () => {{
  try {{
    const devices = await api("/v1/devices", {{ method: "GET", headers: {{}} }});
    const rows = devices.map((item) => {{
      const device = item.device;
      const presence = item.presence;
      const online = presence && presence.online ? `online ${{presence.lan_ips.join(", ")}}:${{presence.listen_port}}` : "offline";
      return `<tr><td>${{device.name}}</td><td>${{device.os_type}} ${{device.os_version}}</td><td>${{online}}</td><td>${{device.id}}</td></tr>`;
    }}).join("");
    document.getElementById("deviceRows").innerHTML = rows || `<tr><td colspan="4">No devices registered</td></tr>`;
    setStatus("deviceStatus", `${{devices.length}} devices loaded`, "ok");
  }} catch (error) {{
    setStatus("deviceStatus", error.message, "warn");
  }}
}});

document.getElementById("checkHealth").addEventListener("click", async () => {{
  try {{
    const response = await fetch(`${{serverUrl()}}/health`);
    setStatus("networkStatus", response.ok ? "server health ok" : `server returned ${{response.status}}`, response.ok ? "ok" : "warn");
  }} catch (error) {{
    setStatus("networkStatus", error.message, "warn");
  }}
}});

document.querySelectorAll("[data-profile-path]").forEach((input) => {{
  input.addEventListener("change", () => {{
    const numeric = input.type === "number";
    setPath(input.dataset.profilePath, numeric ? Number(input.value) : input.value);
  }});
}});

for (const edge of edgeNames) {{
  document.getElementById(`edge-${{edge}}`).addEventListener("change", (event) => {{
    profile.device_layout = profile.device_layout || {{}};
    profile.device_layout.edges = profile.device_layout.edges || {{}};
    if (event.target.value) profile.device_layout.edges[edge] = event.target.value;
    else delete profile.device_layout.edges[edge];
    syncProfileJson();
  }});
}}

["clipboardMaxBytes", "clipboardEnabled", "clipboardTtlSeconds", "clipboardSensitiveApps"].forEach((id) => {{
  document.getElementById(id).addEventListener("input", updateClipboardCommand);
}});

document.getElementById("copyProfile").addEventListener("click", async () => {{
  await navigator.clipboard.writeText(profileJson.value);
}});

document.getElementById("preset").value = controlData.profile_summary.preset;
document.getElementById("keyboardMode").value = controlData.profile_summary.keyboard_mode;
document.getElementById("functionKeyMode").value = controlData.profile_summary.function_key_mode;
document.getElementById("pointerSpeed").value = controlData.profile_summary.pointer_speed;
document.getElementById("scrollVertical").value = controlData.profile_summary.scroll_vertical;
document.getElementById("scrollHorizontal").value = controlData.profile_summary.scroll_horizontal;
for (const edge of edgeNames) {{
  document.getElementById(`edge-${{edge}}`).value = controlData.layout_edges[edge] || "";
}}
syncProfileJson();
updateClipboardCommand();
</script>
</body>
</html>
"#,
        escape_html(os_label(profile.source_os)),
        escape_html(os_label(profile.target_os)),
        escape_html(keyboard_mode_label(profile.keyboard_mode)),
        render_edge_selects(&target_options),
        data
    ))
}

fn render_edge_selects(options: &str) -> String {
    ["left", "right", "top", "bottom"]
        .iter()
        .map(|edge| {
            format!(
                r#"<label>{edge} edge<select id="edge-{edge}"><option value="">Local only</option>{options}</select></label>"#
            )
        })
        .collect()
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

const fn os_label(value: kmsync_core::OsKind) -> &'static str {
    match value {
        kmsync_core::OsKind::MacOs => "macOS",
        kmsync_core::OsKind::Windows => "Windows",
        kmsync_core::OsKind::Linux => "Linux",
    }
}

const fn preset_value(value: HabitPreset) -> &'static str {
    match value {
        HabitPreset::KeepMacHabit => "keep_mac_habit",
        HabitPreset::KeepWindowsHabit => "keep_windows_habit",
        HabitPreset::KeepLinuxHabit => "keep_linux_habit",
        HabitPreset::TargetOsHabit => "target_os_habit",
        HabitPreset::Custom => "custom",
    }
}

const fn keyboard_mode_value(value: KeyboardMode) -> &'static str {
    match value {
        KeyboardMode::Physical => "physical",
        KeyboardMode::Text => "text",
    }
}

const fn function_key_mode_value(value: FunctionKeyMode) -> &'static str {
    match value {
        FunctionKeyMode::Standard => "standard",
        FunctionKeyMode::Media => "media",
    }
}

const fn keyboard_mode_label(value: KeyboardMode) -> &'static str {
    match value {
        KeyboardMode::Physical => "physical",
        KeyboardMode::Text => "text",
    }
}
