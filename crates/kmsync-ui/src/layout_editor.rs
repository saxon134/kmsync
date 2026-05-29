use kmsync_core::{
    FunctionKeyMode, HabitPreset, KeyboardLayout, KeyboardMode, NonEnglishInputStrategy, OsKind,
    Profile, ScreenEdge,
};

pub fn render_layout_editor(profile_text: &str) -> Result<String, String> {
    let profile = Profile::from_config_json(profile_text)
        .map_err(|error| format!("invalid profile config: {error:?}"))?;
    let profile_json = serde_json::from_str::<serde_json::Value>(profile_text)
        .map_err(|error| format!("invalid profile json: {error}"))?;

    let targets_json = profile
        .device_layout
        .targets
        .iter()
        .map(|target| {
            serde_json::json!({
                "device_id": target.device_id,
                "display_name": target.display_name.as_deref().unwrap_or(&target.device_id),
            })
        })
        .collect::<Vec<_>>();
    let editor_data = serde_json::json!({
        "source_os": os_label(profile.source_os),
        "target_os": os_label(profile.target_os),
        "preset": habit_preset_label(profile.preset),
        "keyboard_mode": keyboard_mode_label(profile.keyboard_mode),
        "function_key_mode": function_key_mode_label(profile.function_key_mode),
        "source_keyboard_layout": keyboard_layout_label(profile.source_keyboard_layout),
        "target_keyboard_layout": keyboard_layout_label(profile.target_keyboard_layout),
        "non_english_input_strategy": input_strategy_label(profile.non_english_input_strategy),
        "pointer_speed": profile.pointer.speed_multiplier,
        "scroll_vertical": profile.scroll.vertical_multiplier,
        "scroll_horizontal": profile.scroll.horizontal_multiplier,
        "targets": targets_json,
        "edges": {
            "left": profile.device_layout.target_for_edge(ScreenEdge::Left),
            "right": profile.device_layout.target_for_edge(ScreenEdge::Right),
            "top": profile.device_layout.target_for_edge(ScreenEdge::Top),
            "bottom": profile.device_layout.target_for_edge(ScreenEdge::Bottom),
        },
        "profile": profile_json,
    });
    let editor_data = serde_json::to_string(&editor_data)
        .map_err(|error| format!("failed to encode layout editor data: {error}"))?
        .replace("</", "<\\/");

    let target_tiles = profile
        .device_layout
        .targets
        .iter()
        .map(|target| {
            let label = target.display_name.as_deref().unwrap_or(&target.device_id);
            format!(
                r#"<button class="device-tile" type="button" draggable="true" data-target-id="{}"><span>{}</span><small>{}</small></button>"#,
                escape_html(&target.device_id),
                escape_html(label),
                escape_html(&target.device_id)
            )
        })
        .collect::<String>();
    let options = render_target_options(&profile);

    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>KMSync Layout Editor</title>
<style>
:root {{
  color-scheme: light;
  --bg: oklch(97% 0.006 235);
  --panel: oklch(99% 0.004 235);
  --panel-2: oklch(94% 0.008 235);
  --ink: oklch(24% 0.018 235);
  --muted: oklch(49% 0.016 235);
  --line: oklch(84% 0.012 235);
  --accent: oklch(52% 0.15 248);
  --accent-soft: oklch(92% 0.04 248);
  --ok: oklch(57% 0.13 158);
  --warn: oklch(63% 0.14 58);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
}}
* {{ box-sizing: border-box; }}
body {{
  margin: 0;
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
  padding: 16px 22px;
  border-bottom: 1px solid var(--line);
  background: var(--panel);
}}
.title-group {{
  min-width: 0;
}}
h1 {{
  margin: 0;
  font-size: 20px;
  line-height: 1.2;
  font-weight: 720;
  letter-spacing: 0;
}}
.summary {{
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 7px;
  color: var(--muted);
}}
.pill {{
  border: 1px solid var(--line);
  background: var(--panel-2);
  border-radius: 999px;
  padding: 3px 9px;
  font-size: 12px;
  white-space: nowrap;
}}
.actions {{
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;
}}
button {{
  border: 1px solid var(--line);
  background: var(--panel);
  color: var(--ink);
  border-radius: 7px;
  min-height: 34px;
  padding: 0 11px;
  font: inherit;
  cursor: pointer;
}}
button:hover {{ border-color: var(--accent); }}
button:focus-visible, select:focus-visible, textarea:focus-visible {{
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}}
.primary {{
  border-color: var(--accent);
  background: var(--accent);
  color: oklch(98% 0.006 248);
}}
.shell {{
  display: grid;
  grid-template-columns: minmax(520px, 1fr) minmax(320px, 420px);
  gap: 18px;
  padding: 18px;
}}
.workspace, .side-panel {{
  border: 1px solid var(--line);
  background: var(--panel);
  border-radius: 8px;
  min-width: 0;
}}
.workspace {{
  display: grid;
  grid-template-rows: auto 1fr auto;
  gap: 14px;
  padding: 16px;
}}
.profile-strip {{
  display: grid;
  grid-template-columns: repeat(4, minmax(120px, 1fr));
  gap: 8px;
}}
.metric {{
  padding: 10px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel-2);
}}
.metric small {{
  display: block;
  color: var(--muted);
  font-size: 11px;
  margin-bottom: 4px;
}}
.layout-map {{
  display: grid;
  grid-template-columns: minmax(150px, 0.9fr) minmax(210px, 1.2fr) minmax(150px, 0.9fr);
  grid-template-rows: minmax(124px, auto) minmax(160px, auto) minmax(124px, auto);
  gap: 10px;
  align-items: stretch;
}}
.edge-zone {{
  border: 1px dashed var(--line);
  border-radius: 8px;
  padding: 12px;
  background: oklch(97% 0.006 235);
  display: grid;
  gap: 10px;
  transition: border-color 160ms ease-out, background-color 160ms ease-out;
}}
.edge-zone[data-bound="true"] {{
  border-style: solid;
  border-color: var(--accent);
  background: var(--accent-soft);
}}
.edge-zone.drag-over {{
  border-color: var(--ok);
  background: oklch(93% 0.04 158);
}}
.edge-top {{ grid-column: 2; grid-row: 1; }}
.edge-left {{ grid-column: 1; grid-row: 2; }}
.edge-right {{ grid-column: 3; grid-row: 2; }}
.edge-bottom {{ grid-column: 2; grid-row: 3; }}
.local-device {{
  grid-column: 2;
  grid-row: 2;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  display: grid;
  place-items: center;
  min-height: 160px;
  text-align: center;
}}
.local-device strong {{
  display: block;
  font-size: 18px;
}}
.local-device span, .edge-label, .bound-label {{
  color: var(--muted);
}}
.edge-heading {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}}
.edge-heading strong {{
  font-size: 13px;
}}
select {{
  width: 100%;
  min-height: 34px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel);
  color: var(--ink);
  font: inherit;
}}
.device-tray {{
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}}
.device-tile {{
  display: grid;
  justify-items: start;
  align-content: center;
  gap: 2px;
  min-width: 150px;
  min-height: 54px;
  text-align: left;
  background: var(--panel);
}}
.device-tile span {{
  font-weight: 650;
}}
.device-tile small {{
  color: var(--muted);
  max-width: 180px;
  overflow-wrap: anywhere;
}}
.side-panel {{
  display: grid;
  grid-template-rows: auto 1fr;
}}
.side-head {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 14px;
  border-bottom: 1px solid var(--line);
}}
.side-head h2 {{
  margin: 0;
  font-size: 15px;
  line-height: 1.2;
}}
textarea {{
  width: 100%;
  height: 100%;
  min-height: 520px;
  resize: none;
  border: 0;
  padding: 14px;
  background: oklch(96% 0.006 235);
  color: var(--ink);
  font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  line-height: 1.55;
  tab-size: 2;
}}
.toast {{
  position: fixed;
  right: 18px;
  bottom: 18px;
  min-width: 180px;
  padding: 10px 12px;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--panel);
  color: var(--ink);
  box-shadow: 0 10px 30px oklch(22% 0.018 235 / 14%);
  opacity: 0;
  transform: translateY(8px);
  transition: opacity 160ms ease-out, transform 160ms ease-out;
}}
.toast.visible {{
  opacity: 1;
  transform: translateY(0);
}}
@media (max-width: 900px) {{
  .topbar, .shell {{ padding: 12px; }}
  .shell {{ grid-template-columns: 1fr; }}
  .profile-strip {{ grid-template-columns: repeat(2, minmax(120px, 1fr)); }}
  .layout-map {{
    grid-template-columns: 1fr;
    grid-template-rows: repeat(5, auto);
  }}
  .edge-top, .edge-left, .edge-right, .edge-bottom, .local-device {{
    grid-column: 1;
    grid-row: auto;
  }}
}}
</style>
</head>
<body>
<main class="app" data-app="layout-editor">
  <header class="topbar">
    <div class="title-group">
      <h1>Device Layout</h1>
      <div class="summary">
        <span class="pill">{} to {}</span>
        <span class="pill">{}</span>
        <span class="pill">{}</span>
      </div>
    </div>
    <div class="actions">
      <button type="button" id="resetLayout">Reset</button>
      <button type="button" class="primary" id="copyJson">Copy JSON</button>
    </div>
  </header>
  <section class="shell">
    <section class="workspace" aria-label="Device layout workspace">
      <div class="profile-strip">
        <div class="metric"><small>Pointer</small><strong>{:.2}x</strong></div>
        <div class="metric"><small>Vertical Scroll</small><strong>{:.2}x</strong></div>
        <div class="metric"><small>Horizontal Scroll</small><strong>{:.2}x</strong></div>
        <div class="metric"><small>Input</small><strong>{}</strong></div>
      </div>
      <div class="layout-map">
        {}
        {}
        {}
        {}
        <div class="local-device" aria-label="Local device">
          <div><strong>This device</strong><span>{}</span></div>
        </div>
      </div>
      <div class="device-tray" aria-label="Available target devices">
        {}
      </div>
    </section>
    <aside class="side-panel">
      <div class="side-head">
        <h2>Profile JSON</h2>
        <span class="bound-label" id="bindingCount"></span>
      </div>
      <textarea id="profileJson" spellcheck="false"></textarea>
    </aside>
  </section>
  <div class="toast" id="toast" role="status" aria-live="polite"></div>
</main>
<script>
const editorData = {};
const originalProfile = structuredClone(editorData.profile);
const targetById = new Map(editorData.targets.map((target) => [target.device_id, target]));
const edges = ["left", "right", "top", "bottom"];
const profileJson = document.getElementById("profileJson");
const bindingCount = document.getElementById("bindingCount");
let profile = structuredClone(editorData.profile);

function labelFor(deviceId) {{
  if (!deviceId) return "Unassigned";
  const target = targetById.get(deviceId);
  return target ? target.display_name : deviceId;
}}

function ensureLayout() {{
  profile.device_layout = profile.device_layout || {{}};
  profile.device_layout.targets = editorData.targets.map((target) => ({{
    device_id: target.device_id,
    display_name: target.display_name
  }}));
  profile.device_layout.edges = profile.device_layout.edges || {{}};
}}

function setEdge(edge, deviceId) {{
  ensureLayout();
  if (deviceId) {{
    profile.device_layout.edges[edge] = deviceId;
  }} else {{
    delete profile.device_layout.edges[edge];
  }}
  renderEdges();
  syncJson();
}}

function renderEdges() {{
  let bound = 0;
  for (const edge of edges) {{
    const zone = document.querySelector(`[data-edge="${{edge}}"]`);
    const select = zone.querySelector("select");
    const value = profile.device_layout?.edges?.[edge] || "";
    const label = zone.querySelector(".bound-label");
    zone.dataset.bound = value ? "true" : "false";
    select.value = value;
    label.textContent = labelFor(value);
    if (value) bound += 1;
  }}
  bindingCount.textContent = `${{bound}} / 4 bound`;
}}

function syncJson() {{
  profileJson.value = JSON.stringify(profile, null, 2);
}}

function showToast(text) {{
  const toast = document.getElementById("toast");
  toast.textContent = text;
  toast.classList.add("visible");
  window.setTimeout(() => toast.classList.remove("visible"), 1400);
}}

for (const tile of document.querySelectorAll("[data-target-id]")) {{
  tile.addEventListener("dragstart", (event) => {{
    event.dataTransfer.setData("text/plain", tile.dataset.targetId);
    event.dataTransfer.effectAllowed = "copy";
  }});
  tile.addEventListener("click", () => {{
    const firstOpenEdge = edges.find((edge) => !profile.device_layout?.edges?.[edge]) || "right";
    setEdge(firstOpenEdge, tile.dataset.targetId);
  }});
}}

for (const zone of document.querySelectorAll("[data-edge]")) {{
  zone.addEventListener("dragover", (event) => {{
    event.preventDefault();
    zone.classList.add("drag-over");
  }});
  zone.addEventListener("dragleave", () => zone.classList.remove("drag-over"));
  zone.addEventListener("drop", (event) => {{
    event.preventDefault();
    zone.classList.remove("drag-over");
    const deviceId = event.dataTransfer.getData("text/plain");
    if (targetById.has(deviceId)) setEdge(zone.dataset.edge, deviceId);
  }});
  zone.querySelector("select").addEventListener("change", (event) => {{
    setEdge(zone.dataset.edge, event.target.value);
  }});
  zone.querySelector("[data-clear]").addEventListener("click", () => {{
    setEdge(zone.dataset.edge, "");
  }});
}}

document.getElementById("resetLayout").addEventListener("click", () => {{
  profile = structuredClone(originalProfile);
  renderEdges();
  syncJson();
}});

document.getElementById("copyJson").addEventListener("click", async () => {{
  await navigator.clipboard.writeText(profileJson.value);
  showToast("Copied");
}});

ensureLayout();
renderEdges();
syncJson();
</script>
</body>
</html>
"#,
        escape_html(os_label(profile.source_os)),
        escape_html(os_label(profile.target_os)),
        escape_html(habit_preset_label(profile.preset)),
        escape_html(function_key_mode_label(profile.function_key_mode)),
        profile.pointer.speed_multiplier,
        profile.scroll.vertical_multiplier,
        profile.scroll.horizontal_multiplier,
        escape_html(keyboard_mode_label(profile.keyboard_mode)),
        render_edge_zone("top", "Top edge", &options),
        render_edge_zone("left", "Left edge", &options),
        render_edge_zone("right", "Right edge", &options),
        render_edge_zone("bottom", "Bottom edge", &options),
        escape_html(os_label(profile.source_os)),
        target_tiles,
        editor_data
    ))
}

fn render_edge_zone(edge: &str, label: &str, options: &str) -> String {
    format!(
        r#"<section class="edge-zone edge-{}" data-edge="{}" data-bound="false">
          <div class="edge-heading"><strong>{}</strong><button type="button" data-clear>Clear</button></div>
          <select aria-label="{} target"><option value="">Unassigned</option>{}</select>
          <span class="bound-label">Unassigned</span>
        </section>"#,
        edge,
        edge,
        escape_html(label),
        escape_html(label),
        options
    )
}

fn render_target_options(profile: &Profile) -> String {
    profile
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

const fn os_label(value: OsKind) -> &'static str {
    match value {
        OsKind::MacOs => "macOS",
        OsKind::Windows => "Windows",
        OsKind::Linux => "Linux",
    }
}

const fn habit_preset_label(value: HabitPreset) -> &'static str {
    match value {
        HabitPreset::KeepMacHabit => "Keep Mac habit",
        HabitPreset::KeepWindowsHabit => "Keep Windows habit",
        HabitPreset::KeepLinuxHabit => "Keep Linux habit",
        HabitPreset::TargetOsHabit => "Target OS habit",
        HabitPreset::Custom => "Custom habit",
    }
}

const fn function_key_mode_label(value: FunctionKeyMode) -> &'static str {
    match value {
        FunctionKeyMode::Standard => "Standard function row",
        FunctionKeyMode::Media => "Media function row",
    }
}

const fn keyboard_mode_label(value: KeyboardMode) -> &'static str {
    match value {
        KeyboardMode::Physical => "Physical",
        KeyboardMode::Text => "Text",
    }
}

const fn keyboard_layout_label(value: KeyboardLayout) -> &'static str {
    match value {
        KeyboardLayout::UsAnsi => "US ANSI",
        KeyboardLayout::Iso => "ISO",
        KeyboardLayout::Jis => "JIS",
        KeyboardLayout::Custom => "Custom",
    }
}

const fn input_strategy_label(value: NonEnglishInputStrategy) -> &'static str {
    match value {
        NonEnglishInputStrategy::PhysicalFallback => "Physical fallback",
        NonEnglishInputStrategy::ImePassthrough => "IME passthrough",
        NonEnglishInputStrategy::UnicodeText => "Unicode text",
    }
}
