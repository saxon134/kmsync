use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use kmsync_core::{DesktopConnectionState, DesktopLayout, DesktopRole, DesktopState};

const NATIVE_CJK_FONT_NAME: &str = "kmsync_cjk";
const NATIVE_WINDOW_SIZE: [f32; 2] = [1120.0, 820.0];
const NATIVE_WINDOW_MIN_SIZE: [f32; 2] = [900.0, 640.0];
const NATIVE_LAYOUT_PANEL_MIN_HEIGHT: f32 = 180.0;
const NATIVE_DEVICES_PANEL_MIN_HEIGHT: f32 = 220.0;
const NATIVE_DEVICES_GRID_MIN_COL_WIDTH: f32 = 120.0;
const NATIVE_ACTION_BUTTON_WIDTH: f32 = 150.0;
const NATIVE_ACTION_BUTTON_HEIGHT: f32 = 34.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeDesktopViewModel {
    pub(crate) title: String,
    pub(crate) server_host: String,
    pub(crate) server_port: String,
    pub(crate) server_url: String,
    pub(crate) is_master: bool,
    pub(crate) layout: DesktopLayout,
    pub(crate) device_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeDesktopLayoutMetrics {
    window_size: [f32; 2],
    min_window_size: [f32; 2],
    full_width_sections: bool,
    layout_panel_min_height: f32,
    devices_panel_min_height: f32,
    devices_grid_min_col_width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeStatusTone {
    Success,
    Danger,
    Warning,
    Info,
    Muted,
}

impl NativeDesktopViewModel {
    pub(crate) fn from_state(state: &DesktopState) -> Self {
        Self {
            title: "KMSync".to_string(),
            server_host: state.network.server_host.clone().unwrap_or_default(),
            server_port: state
                .network
                .server_port
                .map_or_else(String::new, |port| port.to_string()),
            server_url: state.network.server_url.clone().unwrap_or_default(),
            is_master: state.device.role == DesktopRole::Master,
            layout: state.layout.clone(),
            device_names: state
                .devices
                .iter()
                .map(|device| device.name.clone())
                .collect(),
        }
    }
}

pub(crate) fn run_native_desktop(config_path: &Path) -> Result<(), String> {
    let app = NativeDesktopApp::load(config_path.to_path_buf())?;
    let metrics = native_desktop_layout_metrics();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KMSync")
            .with_inner_size(metrics.window_size)
            .with_min_inner_size(metrics.min_window_size),
        ..Default::default()
    };
    eframe::run_native(
        "KMSync",
        options,
        Box::new(move |cc| {
            install_native_text_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| format!("native desktop window failed: {error}"))
}

fn native_desktop_layout_metrics() -> NativeDesktopLayoutMetrics {
    NativeDesktopLayoutMetrics {
        window_size: NATIVE_WINDOW_SIZE,
        min_window_size: NATIVE_WINDOW_MIN_SIZE,
        full_width_sections: true,
        layout_panel_min_height: NATIVE_LAYOUT_PANEL_MIN_HEIGHT,
        devices_panel_min_height: NATIVE_DEVICES_PANEL_MIN_HEIGHT,
        devices_grid_min_col_width: NATIVE_DEVICES_GRID_MIN_COL_WIDTH,
    }
}

fn install_native_text_fonts(ctx: &egui::Context) {
    if let Ok(fonts) = native_font_definitions_from_candidates(&native_cjk_font_candidates()) {
        ctx.set_fonts(fonts);
    }
}

fn native_cjk_font_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Windows\Fonts\Deng.ttf"),
        PathBuf::from(r"C:\Windows\Fonts\simhei.ttf"),
        PathBuf::from(r"C:\Windows\Fonts\simsunb.ttf"),
        PathBuf::from(r"C:\Windows\Fonts\msyh.ttc"),
        PathBuf::from(r"C:\Windows\Fonts\NotoSansSC-VF.ttf"),
        PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
        PathBuf::from("/System/Library/Fonts/STHeiti Light.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),
    ]
}

fn native_font_definitions_from_candidates(
    candidates: &[PathBuf],
) -> Result<egui::FontDefinitions, String> {
    let Some(path) = candidates.iter().find(|path| path.exists()) else {
        return Err("no native CJK font found".to_string());
    };
    let bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read native CJK font {}: {error}", path.display()))?;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        NATIVE_CJK_FONT_NAME.to_string(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, NATIVE_CJK_FONT_NAME.to_string());
    }
    Ok(fonts)
}

struct NativeDesktopApp {
    config_path: PathBuf,
    state: DesktopState,
    server_host: String,
    server_port: String,
    is_master: bool,
    layout: DesktopLayout,
    status_message: String,
}

impl NativeDesktopApp {
    fn load(config_path: PathBuf) -> Result<Self, String> {
        let state = crate::build_local_desktop_state(&config_path)?;
        let view_model = NativeDesktopViewModel::from_state(&state);
        Ok(Self {
            config_path,
            state,
            server_host: view_model.server_host,
            server_port: view_model.server_port,
            is_master: view_model.is_master,
            layout: view_model.layout,
            status_message: "就绪".to_string(),
        })
    }

    fn reload_state(&mut self) {
        match crate::build_local_desktop_state(&self.config_path) {
            Ok(state) => {
                let view_model = NativeDesktopViewModel::from_state(&state);
                self.state = state;
                self.server_host = view_model.server_host;
                self.server_port = view_model.server_port;
                self.is_master = view_model.is_master;
                self.layout = view_model.layout;
            }
            Err(error) => {
                self.status_message = format!("刷新失败：{error}");
            }
        }
    }

    fn save_server_endpoint(&mut self) {
        let port = match self.server_port.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                self.status_message = "请填写 1-65535 范围内的服务器端口".to_string();
                return;
            }
        };
        match crate::desktop_config::set_server_endpoint_in_config_file(
            &self.config_path,
            &self.server_host,
            port,
        ) {
            Ok(()) => {
                self.status_message = "服务器配置已保存".to_string();
                self.reload_state();
            }
            Err(error) => {
                self.status_message = format!("服务器配置保存失败：{error}");
            }
        }
    }

    fn save_role(&mut self) {
        let role = if self.is_master {
            DesktopRole::Master
        } else {
            DesktopRole::Client
        };
        let master_device_id = if self.is_master {
            None
        } else {
            self.state.master_device_id.as_deref()
        };
        match crate::desktop_config::set_role_in_config_file(
            &self.config_path,
            role,
            master_device_id,
        ) {
            Ok(()) => {
                self.status_message = "当前电脑配置已保存".to_string();
                self.reload_state();
            }
            Err(error) => {
                self.status_message = format!("当前电脑配置保存失败：{error}");
            }
        }
    }

    fn save_layout(&mut self) {
        if let Err(error) = self.layout.validate(None) {
            self.status_message = format!("设备位置无效：{error:?}");
            return;
        }
        match crate::desktop_config::set_layout_in_config_file(&self.config_path, &self.layout) {
            Ok(()) => {
                self.status_message = "设备位置已保存".to_string();
                self.reload_state();
            }
            Err(error) => {
                self.status_message = format!("设备位置保存失败：{error}");
            }
        }
    }

    fn server_url_preview(&self) -> String {
        let scheme = self
            .state
            .network
            .server_url
            .as_deref()
            .and_then(|url| url.split_once("://").map(|(scheme, _)| scheme))
            .unwrap_or("http");
        let host = self.server_host.trim();
        let port = self.server_port.trim();
        if host.is_empty() || port.is_empty() {
            "-".to_string()
        } else {
            format!("{scheme}://{host}:{port}")
        }
    }
}

impl eframe::App for NativeDesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("KMSync");
            ui.horizontal_wrapped(|ui| {
                connection_state_status(ui, "服务器", &self.state.server_state);
                ui.separator();
                connection_state_status(ui, "主电脑", &self.state.master_state);
                ui.separator();
                ui.label(format!("状态：{}", self.status_message));
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                let metrics = native_desktop_layout_metrics();
                ui.columns(2, |columns| {
                    columns[0].group(|ui| {
                        ui.set_min_width(ui.available_width());
                        self.network_section(ui);
                    });
                    columns[1].group(|ui| {
                        ui.set_min_width(ui.available_width());
                        self.current_device_section(ui);
                    });
                });
                ui.add_space(12.0);
                full_width_group(ui, metrics.layout_panel_min_height, |ui| {
                    self.layout_section(ui);
                });
                ui.add_space(12.0);
                full_width_group(ui, metrics.devices_panel_min_height, |ui| {
                    self.devices_section(ui);
                });
            });
        });
    }
}

impl NativeDesktopApp {
    fn network_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Linux 服务器");
        ui.label("服务器 IP/域名");
        ui.text_edit_singleline(&mut self.server_host);
        ui.label("服务器端口");
        ui.text_edit_singleline(&mut self.server_port);
        ui.label(format!("完整地址：{}", self.server_url_preview()));
        if native_action_button(ui, "保存服务器配置").clicked() {
            self.save_server_endpoint();
        }
        ui.separator();
        ui.label(format!(
            "内网 IP：{}",
            empty_dash(&self.state.network.lan_ips.join(", "))
        ));
        ui.label(format!(
            "公网 IP：{}",
            self.state.network.public_ip.as_deref().unwrap_or("-")
        ));
        ui.label(format!(
            "监听端口：{}",
            self.state
                .network
                .listen_port
                .map_or_else(|| "-".to_string(), |port| port.to_string())
        ));
    }

    fn current_device_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("当前电脑");
        ui.checkbox(&mut self.is_master, "将当前电脑作为主电脑");
        ui.label(format!("设备名称：{}", self.state.device.name));
        ui.label(format!(
            "设备 ID：{}",
            self.state.device.id.as_deref().unwrap_or("-")
        ));
        ui.label(format!("系统：{}", self.state.device.os));
        ui.horizontal_wrapped(|ui| {
            if native_action_button(ui, "刷新状态").clicked() {
                self.reload_state();
                self.status_message = "状态已刷新".to_string();
            }
            if native_action_button(ui, "保存当前电脑配置").clicked() {
                self.save_role();
            }
        });
    }

    fn layout_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备位置");
        ui.set_min_height(NATIVE_LAYOUT_PANEL_MIN_HEIGHT);
        let devices = self
            .state
            .devices
            .iter()
            .map(|device| (device.id.clone(), device.name.clone()))
            .collect::<Vec<_>>();
        let combo_width = ((ui.available_width() - 18.0) / 2.0).max(240.0);
        egui::Grid::new("native_layout_grid")
            .num_columns(2)
            .spacing([18.0, 10.0])
            .min_col_width(combo_width)
            .show(ui, |ui| {
                device_combo(ui, "上方电脑", &mut self.layout.top, &devices, combo_width);
                device_combo(
                    ui,
                    "下方电脑",
                    &mut self.layout.bottom,
                    &devices,
                    combo_width,
                );
                ui.end_row();
                device_combo(ui, "左边电脑", &mut self.layout.left, &devices, combo_width);
                device_combo(
                    ui,
                    "右边电脑",
                    &mut self.layout.right,
                    &devices,
                    combo_width,
                );
                ui.end_row();
            });
        ui.add_space(8.0);
        if native_action_button(ui, "保存设备位置").clicked() {
            self.save_layout();
        }
    }

    fn devices_section(&self, ui: &mut egui::Ui) {
        ui.heading("设备列表");
        ui.set_min_height(NATIVE_DEVICES_PANEL_MIN_HEIGHT);
        if self.state.devices.is_empty() {
            ui.label("暂无其他设备");
            return;
        }
        let metrics = native_desktop_layout_metrics();
        let min_col_width =
            ((ui.available_width() - 72.0) / 4.0).max(metrics.devices_grid_min_col_width);
        egui::Grid::new("native_devices_grid")
            .striped(true)
            .spacing([20.0, 8.0])
            .min_col_width(min_col_width)
            .show(ui, |ui| {
                ui.strong("设备");
                ui.strong("状态");
                ui.strong("内网 IP");
                ui.strong("公网 IP");
                ui.end_row();
                for device in &self.state.devices {
                    ui.label(&device.name);
                    ui.label(if device.online { "在线" } else { "离线" });
                    ui.label(empty_dash(&device.lan_ips.join(", ")));
                    ui.label(device.public_ip.as_deref().unwrap_or("-"));
                    ui.end_row();
                }
            });
    }
}

fn device_combo(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut Option<String>,
    devices: &[(String, String)],
    width: f32,
) {
    let selected_text = selected
        .as_deref()
        .and_then(|selected_id| {
            devices
                .iter()
                .find(|(id, _)| id == selected_id)
                .map(|(_, name)| name.as_str())
        })
        .unwrap_or("未配置");
    egui::ComboBox::from_label(label)
        .width(width)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, "未配置");
            for (id, name) in devices {
                ui.selectable_value(selected, Some(id.clone()), name);
            }
        });
}

fn full_width_group<R>(
    ui: &mut egui::Ui,
    min_height: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let width = ui.available_width();
    ui.group(|ui| {
        ui.set_min_width(width);
        ui.set_min_height(min_height);
        add_contents(ui)
    })
}

fn native_action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        native_action_button_size(),
        egui::Button::new(
            egui::RichText::new(label)
                .strong()
                .color(egui::Color32::from_rgb(30, 64, 175)),
        )
        .fill(egui::Color32::from_rgb(239, 246, 255))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(147, 197, 253),
        )),
    )
}

fn native_action_button_size() -> egui::Vec2 {
    egui::vec2(NATIVE_ACTION_BUTTON_WIDTH, NATIVE_ACTION_BUTTON_HEIGHT)
}

#[cfg(test)]
fn native_action_button_labels() -> [&'static str; 4] {
    [
        "保存服务器配置",
        "刷新状态",
        "保存当前电脑配置",
        "保存设备位置",
    ]
}

fn connection_state_status(ui: &mut egui::Ui, label: &str, state: &DesktopConnectionState) {
    ui.horizontal(|ui| {
        ui.label(format!("{label}："));
        ui.label(
            egui::RichText::new(connection_state_label(state))
                .strong()
                .color(connection_state_color(state)),
        );
    });
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

fn connection_state_tone(state: &DesktopConnectionState) -> NativeStatusTone {
    match state {
        DesktopConnectionState::Connected => NativeStatusTone::Success,
        DesktopConnectionState::Connecting => NativeStatusTone::Danger,
        DesktopConnectionState::AuthExpired | DesktopConnectionState::Retrying => {
            NativeStatusTone::Warning
        }
        DesktopConnectionState::SelfDevice => NativeStatusTone::Info,
        DesktopConnectionState::Disconnected => NativeStatusTone::Muted,
    }
}

fn connection_state_color(state: &DesktopConnectionState) -> egui::Color32 {
    match connection_state_tone(state) {
        NativeStatusTone::Success => egui::Color32::from_rgb(21, 128, 61),
        NativeStatusTone::Danger => egui::Color32::from_rgb(185, 28, 28),
        NativeStatusTone::Warning => egui::Color32::from_rgb(180, 83, 9),
        NativeStatusTone::Info => egui::Color32::from_rgb(37, 99, 235),
        NativeStatusTone::Muted => egui::Color32::from_rgb(100, 116, 139),
    }
}

fn empty_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kmsync_core::{DesktopDeviceState, DesktopNetworkState, DesktopPeerState};

    #[test]
    fn native_desktop_view_model_covers_server_role_and_layout_controls() {
        let state = DesktopState {
            device: DesktopDeviceState {
                id: Some("current".to_string()),
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
                public_ip: Some("203.0.113.20".to_string()),
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
                public_ip: Some("203.0.113.21".to_string()),
                listen_port: Some(24_800),
                last_seen_at: Some(124),
            }],
            ..DesktopState::default()
        };

        let view_model = NativeDesktopViewModel::from_state(&state);

        assert_eq!(view_model.title, "KMSync");
        assert_eq!(view_model.server_host, "203.0.113.10");
        assert_eq!(view_model.server_port, "24888");
        assert_eq!(view_model.server_url, "http://203.0.113.10:24888");
        assert!(view_model.is_master);
        assert_eq!(view_model.layout.left.as_deref(), Some("left-device"));
        assert!(view_model.device_names.contains(&"Right PC".to_string()));
    }

    #[test]
    fn native_desktop_layout_uses_wide_full_width_panels() {
        let metrics = native_desktop_layout_metrics();

        assert_eq!(metrics.window_size, [1120.0, 820.0]);
        assert_eq!(metrics.min_window_size, [900.0, 640.0]);
        assert!(metrics.full_width_sections);
        assert!(metrics.layout_panel_min_height >= 180.0);
        assert!(metrics.devices_panel_min_height >= 220.0);
        assert!(metrics.devices_grid_min_col_width >= 120.0);
    }

    #[test]
    fn native_desktop_connection_status_uses_semantic_colors() {
        assert_eq!(
            connection_state_tone(&DesktopConnectionState::Connected),
            NativeStatusTone::Success
        );
        assert_eq!(
            connection_state_tone(&DesktopConnectionState::Connecting),
            NativeStatusTone::Danger
        );
        assert_ne!(
            connection_state_color(&DesktopConnectionState::Connected),
            connection_state_color(&DesktopConnectionState::Connecting)
        );
    }

    #[test]
    fn native_desktop_action_buttons_use_button_treatment() {
        assert_eq!(
            native_action_button_labels(),
            [
                "保存服务器配置",
                "刷新状态",
                "保存当前电脑配置",
                "保存设备位置"
            ]
        );
        assert_eq!(native_action_button_size(), egui::vec2(150.0, 34.0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_font_definitions_prefer_windows_cjk_font_for_chinese_text() {
        let candidate = native_cjk_font_candidates()
            .into_iter()
            .find(|path| path.exists())
            .expect("Windows CJK font should exist");

        let fonts = native_font_definitions_from_candidates(&[candidate])
            .expect("load native CJK font definitions");

        assert!(fonts.font_data.contains_key(NATIVE_CJK_FONT_NAME));
        assert_eq!(
            fonts
                .families
                .get(&egui::FontFamily::Proportional)
                .and_then(|family| family.first())
                .map(String::as_str),
            Some(NATIVE_CJK_FONT_NAME)
        );
    }
}
