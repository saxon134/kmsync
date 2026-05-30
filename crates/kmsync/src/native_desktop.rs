use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use kmsync_core::{DesktopConnectionState, DesktopLayout, DesktopRole, DesktopState};

const NATIVE_CJK_FONT_NAME: &str = "kmsync_cjk";
const NATIVE_WINDOW_SIZE: [f32; 2] = [1120.0, 880.0];
const NATIVE_WINDOW_MIN_SIZE: [f32; 2] = [900.0, 700.0];
const NATIVE_TOP_PANEL_MIN_HEIGHT: f32 = 190.0;
const NATIVE_LOWER_PANEL_MIN_HEIGHT: f32 = 480.0;
const NATIVE_PANEL_SPACING: f32 = 16.0;
const NATIVE_LAYOUT_GRID_MIN_COL_WIDTH: f32 = 96.0;
const NATIVE_LAYOUT_GRID_HORIZONTAL_SPACING: f32 = 12.0;
const NATIVE_DEVICES_GRID_MIN_COL_WIDTH: f32 = 92.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_NAME_MIN_WIDTH: f32 = 86.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_STATUS_MIN_WIDTH: f32 = 70.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_LAN_IP_MIN_WIDTH: f32 = 140.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_PUBLIC_IP_MIN_WIDTH: f32 = 86.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_HORIZONTAL_SPACING: f32 = 12.0;
const NATIVE_ACTION_BUTTON_WIDTH: f32 = 150.0;
const NATIVE_ACTION_BUTTON_HEIGHT: f32 = 34.0;
const NATIVE_LAN_IP_POPUP_VERTICAL_OFFSET: f32 = 6.0;
const NATIVE_LAYOUT_SLOT_HEIGHT: f32 = 112.0;
const NATIVE_LAYOUT_CENTER_HEIGHT: f32 = 118.0;
const NATIVE_DEVICE_ROW_HEIGHT: f32 = 72.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeDesktopViewModel {
    pub(crate) title: String,
    pub(crate) device_name: String,
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
    top_panel_min_height: f32,
    lower_panel_columns: usize,
    layout_panel_min_height: f32,
    devices_panel_min_height: f32,
    devices_grid_min_col_width: f32,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeDeviceGridColumnWidths {
    name: f32,
    status: f32,
    lan_ip: f32,
    public_ip: f32,
}

#[cfg(test)]
impl NativeDeviceGridColumnWidths {
    fn total_width(self, horizontal_spacing: f32) -> f32 {
        self.name + self.status + self.lan_ip + self.public_ip + horizontal_spacing * 3.0
    }

    fn content_width(self) -> f32 {
        self.name + self.status + self.lan_ip + self.public_ip
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeSplitPanelWidths {
    primary: f32,
    secondary: f32,
}

impl NativeSplitPanelWidths {
    #[cfg(test)]
    fn total_width(self, horizontal_spacing: f32) -> f32 {
        self.primary + self.secondary + horizontal_spacing
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeStatusTone {
    Success,
    Danger,
    Warning,
    Info,
    Muted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeLayoutDirection {
    Left,
    Right,
    Top,
    Bottom,
}

impl NativeDesktopViewModel {
    pub(crate) fn from_state(state: &DesktopState) -> Self {
        Self {
            title: "KMSync".to_string(),
            device_name: state.device.name.clone(),
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
        top_panel_min_height: NATIVE_TOP_PANEL_MIN_HEIGHT,
        lower_panel_columns: 2,
        layout_panel_min_height: NATIVE_LOWER_PANEL_MIN_HEIGHT,
        devices_panel_min_height: NATIVE_LOWER_PANEL_MIN_HEIGHT,
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
    device_name: String,
    is_master: bool,
    layout: DesktopLayout,
    status_message: String,
    lan_ip_popup: Option<NativeLanIpPopup>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeLanIpPopup {
    device_name: String,
    lan_ips: Vec<String>,
    position: egui::Pos2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeLanIpSummary {
    primary: String,
    has_more: bool,
    total_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeDeviceListRow {
    name: String,
    detail: String,
    status: String,
    status_tone: NativeStatusTone,
    lan_ips: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeLayoutSlotView {
    device_name: String,
    status_label: String,
    route_hint: String,
    tone: NativeStatusTone,
}

#[derive(Debug, Clone, Copy)]
struct NativeToneColors {
    text: egui::Color32,
    fill: egui::Color32,
    stroke: egui::Color32,
}

impl NativeDesktopApp {
    fn load(config_path: PathBuf) -> Result<Self, String> {
        let state = crate::build_local_desktop_state(&config_path)?;
        let view_model = NativeDesktopViewModel::from_state(&state);
        let status_message = status_message_for_state(&state, "就绪");
        Ok(Self {
            config_path,
            state,
            server_host: view_model.server_host,
            server_port: view_model.server_port,
            device_name: view_model.device_name,
            is_master: view_model.is_master,
            layout: view_model.layout,
            status_message,
            lan_ip_popup: None,
        })
    }

    fn reload_state(&mut self, success_message: &str) {
        match crate::build_local_desktop_state(&self.config_path) {
            Ok(state) => {
                let view_model = NativeDesktopViewModel::from_state(&state);
                let status_message = status_message_for_state(&state, success_message);
                self.state = state;
                self.server_host = view_model.server_host;
                self.server_port = view_model.server_port;
                self.device_name = view_model.device_name;
                self.is_master = view_model.is_master;
                self.layout = view_model.layout;
                self.status_message = status_message;
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
                self.reload_state("服务器配置已保存");
            }
            Err(error) => {
                self.status_message = format!("服务器配置保存失败：{error}");
            }
        }
    }

    fn save_current_device_config(&mut self) {
        let device_name = self.device_name.trim().to_string();
        if device_name.is_empty() {
            self.status_message = "设备名称不能为空".to_string();
            return;
        }
        let role = if self.is_master {
            DesktopRole::Master
        } else {
            DesktopRole::Client
        };
        let master_device_id = if self.is_master {
            self.state.device.id.clone()
        } else {
            self.state.master_device_id.clone()
        };
        match crate::desktop_config::set_current_device_config_in_config_file(
            &self.config_path,
            &device_name,
            role,
            master_device_id.as_deref(),
        ) {
            Ok(()) => {
                match crate::client::ClientConfig::load(&self.config_path).and_then(|config| {
                    crate::client::sync_current_device_name(&config)?;
                    self.sync_topology_to_server(master_device_id.clone())
                }) {
                    Ok(_) => {
                        self.reload_state("当前电脑配置已保存并同步");
                    }
                    Err(error) => {
                        self.reload_state("当前电脑配置已保存");
                        self.status_message =
                            format!("当前电脑配置已保存，设备名称同步失败：{error}");
                    }
                }
            }
            Err(error) => {
                self.status_message = format!("当前电脑配置保存失败：{error}");
            }
        }
    }

    fn save_layout(&mut self) {
        if let Err(error) = self.layout.validate(self.state.device.id.as_deref()) {
            self.status_message = format!("设备位置无效：{error:?}");
            return;
        }
        let master_device_id = if self.is_master {
            self.state.device.id.clone()
        } else {
            self.state.master_device_id.clone()
        };
        match crate::desktop_config::set_topology_in_config_file(
            &self.config_path,
            if self.is_master {
                DesktopRole::Master
            } else {
                DesktopRole::Client
            },
            master_device_id.as_deref(),
            &self.layout,
        ) {
            Ok(()) => match self.sync_topology_to_server(master_device_id) {
                Ok(()) => self.reload_state("设备位置已保存并同步"),
                Err(error) => {
                    self.reload_state("设备位置已保存");
                    self.status_message = format!("设备位置已保存，同步失败：{error}");
                }
            },
            Err(error) => {
                self.status_message = format!("设备位置保存失败：{error}");
            }
        }
    }

    fn sync_topology_to_server(&self, master_device_id: Option<String>) -> Result<(), String> {
        let config = crate::client::ClientConfig::load(&self.config_path)?;
        let client = crate::client::ControlClient::new(config.server_url);
        client
            .upsert_topology(&crate::client::UpsertTopologyRequest {
                master_device_id,
                layout: self.layout.clone(),
            })
            .map(|_| ())
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
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(native_app_background())
                    .inner_margin(egui::Margin::symmetric(18, 18)),
            )
            .show(ctx, |ui| {
                native_app_header(ui, &self.state, &self.status_message);
                ui.add_space(14.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let metrics = native_desktop_layout_metrics();
                        let top_widths = native_top_panel_widths(ui.available_width());
                        ui.horizontal_top(|ui| {
                            native_panel(
                                ui,
                                top_widths.primary,
                                metrics.top_panel_min_height,
                                |ui| {
                                    self.network_section(ui);
                                },
                            );
                            native_panel(
                                ui,
                                top_widths.secondary,
                                metrics.top_panel_min_height,
                                |ui| {
                                    self.current_device_section(ui);
                                },
                            );
                        });
                        ui.add_space(NATIVE_PANEL_SPACING);
                        let lower_widths = native_lower_panel_widths(ui.available_width());
                        ui.horizontal_top(|ui| {
                            native_panel(
                                ui,
                                lower_widths.primary,
                                metrics.layout_panel_min_height,
                                |ui| {
                                    self.layout_section(ui);
                                },
                            );
                            native_panel(
                                ui,
                                lower_widths.secondary,
                                metrics.devices_panel_min_height,
                                |ui| {
                                    self.devices_section(ui);
                                },
                            );
                        });
                    });
            });
        self.lan_ip_popup_window(ctx);
    }
}

impl NativeDesktopApp {
    fn network_section(&mut self, ui: &mut egui::Ui) {
        native_section_title(
            ui,
            "服务器",
            Some((
                connection_state_label(&self.state.server_state),
                connection_state_tone(&self.state.server_state),
            )),
        );
        ui.add_space(12.0);
        ui.horizontal_top(|ui| {
            native_labeled_text_edit(ui, "服务器 IP / 域名", &mut self.server_host, 220.0);
            ui.add_space(8.0);
            native_labeled_text_edit(ui, "端口", &mut self.server_port, 90.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(23.0);
                if native_action_button(ui, "保存服务器配置").clicked() {
                    self.save_server_endpoint();
                }
            });
        });
        ui.add_space(16.0);
        ui.monospace(format!("完整地址：{}", self.server_url_preview()));
        ui.add_space(8.0);
        ui.colored_label(native_muted_text(), "配置保存到本地文件，应用会自动重连。");
    }

    fn current_device_section(&mut self, ui: &mut egui::Ui) {
        native_section_title(
            ui,
            "当前电脑",
            Some((
                if self.is_master {
                    "主电脑"
                } else {
                    "从电脑"
                },
                NativeStatusTone::Info,
            )),
        );
        ui.add_space(12.0);
        ui.horizontal_top(|ui| {
            native_labeled_text_edit(ui, "设备名称", &mut self.device_name, 220.0);
            ui.add_space(8.0);
            native_readonly_field(
                ui,
                "设备 ID",
                self.state.device.id.as_deref().unwrap_or("-"),
                140.0,
            );
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(23.0);
                if native_action_button(ui, "保存当前电脑配置").clicked() {
                    self.save_current_device_config();
                }
            });
        });
        ui.add_space(14.0);
        ui.horizontal_top(|ui| {
            native_metric_card(
                ui,
                native_current_device_fact_labels()[0],
                &empty_dash(&self.state.network.lan_ips.join(", ")),
                NativeStatusTone::Success,
                140.0,
            );
            ui.add_space(8.0);
            native_metric_card(
                ui,
                native_current_device_fact_labels()[1],
                &self
                    .state
                    .network
                    .listen_port
                    .map_or_else(|| "-".to_string(), |port| port.to_string()),
                NativeStatusTone::Muted,
                90.0,
            );
            ui.add_space(8.0);
            native_metric_card(
                ui,
                "主电脑配置",
                &native_master_assignment_text(&self.state),
                if self.is_master {
                    NativeStatusTone::Success
                } else {
                    NativeStatusTone::Muted
                },
                130.0,
            );
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(10.0);
                if native_action_button(ui, "刷新").clicked() {
                    self.reload_state("状态已刷新");
                }
            });
        });
        ui.add_space(8.0);
        ui.checkbox(&mut self.is_master, "将当前电脑作为主电脑");
        ui.colored_label(
            native_muted_text(),
            format!(
                "系统：{}，最近心跳：{}",
                self.state.device.os,
                self.state
                    .network
                    .last_seen_at
                    .map_or_else(|| "-".to_string(), |value| value.to_string())
            ),
        );
    }

    fn layout_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.heading("设备位置");
                ui.colored_label(
                    native_muted_text(),
                    "以主电脑为中心配置上下左右，触碰屏幕边缘时切换控制目标。",
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if native_primary_action_button(ui, "保存设备位置").clicked() {
                    self.save_layout();
                }
            });
        });
        ui.add_space(14.0);
        ui.set_min_height(native_desktop_layout_metrics().layout_panel_min_height);
        let devices = native_layout_device_options(&self.state);
        let center_device_name = native_layout_center_device_name(&self.state, &self.device_name);
        let combo_width = native_layout_combo_width((ui.available_width() - 48.0).max(1.0));
        let mut edited_layout = self.layout.clone();
        let top_view = native_layout_slot_view(&self.state, edited_layout.top.as_deref());
        let left_view = native_layout_slot_view(&self.state, edited_layout.left.as_deref());
        let right_view = native_layout_slot_view(&self.state, edited_layout.right.as_deref());
        let bottom_view = native_layout_slot_view(&self.state, edited_layout.bottom.as_deref());
        native_layout_canvas(ui, |ui| {
            egui::Grid::new("native_layout_grid")
                .num_columns(3)
                .spacing([native_layout_grid_horizontal_spacing(), 12.0])
                .min_col_width(combo_width)
                .show(ui, |ui| {
                    ui.label("");
                    if layout_slot_card(
                        ui,
                        "上方",
                        &mut edited_layout.top,
                        &devices,
                        combo_width,
                        &top_view,
                    ) {
                        native_layout_clear_duplicate_targets(
                            &mut edited_layout,
                            NativeLayoutDirection::Top,
                        );
                    }
                    ui.label("");
                    ui.end_row();
                    if layout_slot_card(
                        ui,
                        "左侧",
                        &mut edited_layout.left,
                        &devices,
                        combo_width,
                        &left_view,
                    ) {
                        native_layout_clear_duplicate_targets(
                            &mut edited_layout,
                            NativeLayoutDirection::Left,
                        );
                    }
                    master_device_cell(ui, &center_device_name, self.is_master, combo_width);
                    if layout_slot_card(
                        ui,
                        "右侧",
                        &mut edited_layout.right,
                        &devices,
                        combo_width,
                        &right_view,
                    ) {
                        native_layout_clear_duplicate_targets(
                            &mut edited_layout,
                            NativeLayoutDirection::Right,
                        );
                    }
                    ui.end_row();
                    ui.label("");
                    if layout_slot_card(
                        ui,
                        "下方",
                        &mut edited_layout.bottom,
                        &devices,
                        combo_width,
                        &bottom_view,
                    ) {
                        native_layout_clear_duplicate_targets(
                            &mut edited_layout,
                            NativeLayoutDirection::Bottom,
                        );
                    }
                    ui.label("");
                    ui.end_row();
                });
        });
        self.layout = edited_layout;
    }

    fn devices_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备列表");
        ui.colored_label(
            native_muted_text(),
            "同账号下设备，名称和 IP 会随心跳刷新。",
        );
        ui.add_space(14.0);
        ui.set_min_height(native_desktop_layout_metrics().devices_panel_min_height);
        let rows = native_device_list_rows(&self.state, &self.device_name);
        let row_width = ui.available_width();
        let mut next_lan_ip_popup = None;
        for row in &rows {
            if let Some(position) = native_device_row_card(ui, row, row_width) {
                next_lan_ip_popup = Some(NativeLanIpPopup {
                    device_name: row.name.clone(),
                    lan_ips: row.lan_ips.clone(),
                    position,
                });
            }
            ui.add_space(10.0);
        }
        native_hint_box(
            ui,
            "提示：离线设备会保留位置配置，重新上线后自动刷新连接信息。",
        );
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if native_action_button(ui, "刷新").clicked() {
                self.reload_state("设备列表已刷新");
            }
        });
        if next_lan_ip_popup.is_some() {
            self.lan_ip_popup = next_lan_ip_popup;
        }
    }

    fn lan_ip_popup_window(&mut self, ctx: &egui::Context) {
        let Some(popup) = self.lan_ip_popup.clone() else {
            return;
        };
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new(format!("{} 的内网 IP", popup.device_name))
            .collapsible(false)
            .resizable(false)
            .fixed_pos(popup.position)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(260.0);
                for ip in &popup.lan_ips {
                    ui.monospace(ip);
                }
                ui.add_space(8.0);
                if native_action_button(ui, "关闭").clicked() {
                    close_clicked = true;
                }
            });
        if !open || close_clicked {
            self.lan_ip_popup = None;
        }
    }
}

fn native_app_header(ui: &mut egui::Ui, state: &DesktopState, status_message: &str) {
    let width = ui.available_width();
    egui::Frame::new()
        .fill(native_panel_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(22, 14))
        .show(ui, |ui| {
            ui.set_min_width((width - 46.0).max(1.0));
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("KMSync")
                            .size(28.0)
                            .strong()
                            .color(native_primary_text()),
                    );
                    ui.colored_label(
                        native_muted_text(),
                        format!("桌面端控制台，状态：{status_message}"),
                    );
                });
                ui.add_space((ui.available_width() - 360.0).max(12.0));
                native_status_chip(
                    ui,
                    &format!(
                        "{} {}",
                        native_top_status_labels()[0],
                        connection_state_label(&state.server_state)
                    ),
                    connection_state_tone(&state.server_state),
                );
                native_status_chip(ui, "LAN 优先", NativeStatusTone::Info);
                native_status_chip(ui, "刚刚刷新", NativeStatusTone::Muted);
            });
        });
}

fn native_panel(
    ui: &mut egui::Ui,
    width: f32,
    min_height: f32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, min_height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::Frame::new()
                .fill(native_panel_background())
                .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(20, 18))
                .show(ui, |ui| {
                    ui.set_min_width((width - 42.0).max(1.0));
                    ui.set_min_height((min_height - 38.0).max(1.0));
                    add_contents(ui);
                });
        },
    );
}

fn native_section_title(
    ui: &mut egui::Ui,
    title: &str,
    trailing_status: Option<(&str, NativeStatusTone)>,
) {
    ui.horizontal(|ui| {
        ui.heading(title);
        if let Some((status, tone)) = trailing_status {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                native_status_chip(ui, status, tone);
            });
        }
    });
}

fn native_labeled_text_edit(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .strong()
                .color(native_muted_text()),
        );
        ui.add_sized(
            egui::vec2(width, 32.0),
            egui::TextEdit::singleline(value).desired_width(width),
        );
    });
}

fn native_readonly_field(ui: &mut egui::Ui, label: &str, value: &str, width: f32) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .strong()
                .color(native_muted_text()),
        );
        egui::Frame::new()
            .fill(native_subtle_background())
            .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width((width - 24.0).max(1.0));
                ui.label(egui::RichText::new(value).color(native_primary_text()));
            });
    });
}

fn native_metric_card(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    tone: NativeStatusTone,
    width: f32,
) {
    let colors = native_tone_colors(tone);
    egui::Frame::new()
        .fill(colors.fill)
        .stroke(egui::Stroke::new(1.0, colors.stroke))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width((width - 24.0).max(1.0));
            ui.label(egui::RichText::new(label).color(native_muted_text()));
            ui.label(egui::RichText::new(value).strong().color(colors.text));
        });
}

fn native_layout_canvas(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(native_canvas_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(24, 24))
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 48.0).max(1.0));
            ui.set_min_height(360.0);
            add_contents(ui);
        });
}

fn native_tinted_card(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    tone: NativeStatusTone,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let colors = native_tone_colors(tone);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::Frame::new()
                .fill(colors.fill)
                .stroke(egui::Stroke::new(1.0, colors.stroke))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_min_width((width - 26.0).max(1.0));
                    ui.set_min_height((height - 22.0).max(1.0));
                    add_contents(ui);
                });
        },
    );
}

fn native_status_chip(ui: &mut egui::Ui, label: &str, tone: NativeStatusTone) {
    let colors = native_tone_colors(tone);
    egui::Frame::new()
        .fill(colors.fill)
        .stroke(egui::Stroke::new(1.0, colors.stroke))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).strong().color(colors.text));
        });
}

fn native_device_row_card(
    ui: &mut egui::Ui,
    row: &NativeDeviceListRow,
    width: f32,
) -> Option<egui::Pos2> {
    let colors = native_tone_colors(row.status_tone);
    let summary = native_lan_ip_summary(&row.lan_ips);
    let mut popup_position = None;
    egui::Frame::new()
        .fill(native_panel_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width((width - 30.0).max(1.0));
            ui.set_min_height(NATIVE_DEVICE_ROW_HEIGHT - 20.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●").size(17.0).color(colors.text));
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&row.name)
                            .strong()
                            .color(native_primary_text()),
                    );
                    ui.colored_label(native_muted_text(), &row.detail);
                });
                ui.add_space((ui.available_width() - 210.0).max(8.0));
                native_status_chip(ui, &row.status, row.status_tone);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(summary.primary.as_str())
                        .strong()
                        .color(native_primary_text()),
                );
                if summary.has_more {
                    let response = ui.small_button("更多");
                    if response.clicked() {
                        popup_position = Some(native_lan_ip_popup_position(response.rect));
                    }
                }
            });
        });
    popup_position
}

fn native_hint_box(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(native_canvas_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.colored_label(native_muted_text(), text);
        });
}

fn layout_slot_card(
    ui: &mut egui::Ui,
    direction: &str,
    selected: &mut Option<String>,
    devices: &[(String, String)],
    width: f32,
    view: &NativeLayoutSlotView,
) -> bool {
    let before = selected.clone();
    let selected_text = selected
        .as_deref()
        .and_then(|selected_id| {
            devices
                .iter()
                .find(|(id, _)| id == selected_id)
                .map(|(_, name)| name.as_str())
        })
        .unwrap_or("未配置");
    native_tinted_card(ui, width, NATIVE_LAYOUT_SLOT_HEIGHT, view.tone, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(direction)
                    .strong()
                    .color(native_muted_text()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                native_status_chip(ui, &view.status_label, view.tone);
            });
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(&view.device_name)
                .strong()
                .color(native_primary_text()),
        );
        ui.colored_label(native_muted_text(), &view.route_hint);
        ui.add_space(6.0);
        egui::ComboBox::from_id_salt(("native_layout_combo", direction))
            .width((width - 24.0).max(1.0))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                ui.selectable_value(selected, None, "未配置");
                for (id, name) in devices {
                    ui.selectable_value(selected, Some(id.clone()), name);
                }
            });
    });
    *selected != before
}

fn native_layout_device_options(state: &DesktopState) -> Vec<(String, String)> {
    let current_device_id = state.device.id.as_deref();
    state
        .devices
        .iter()
        .filter(|device| Some(device.id.as_str()) != current_device_id)
        .map(|device| (device.id.clone(), device.name.clone()))
        .collect()
}

fn native_layout_clear_duplicate_targets(
    layout: &mut DesktopLayout,
    changed_direction: NativeLayoutDirection,
) {
    let Some(selected_device_id) =
        native_layout_direction_value(layout, changed_direction).map(str::to_string)
    else {
        return;
    };
    if changed_direction != NativeLayoutDirection::Left
        && layout.left.as_deref() == Some(selected_device_id.as_str())
    {
        layout.left = None;
    }
    if changed_direction != NativeLayoutDirection::Right
        && layout.right.as_deref() == Some(selected_device_id.as_str())
    {
        layout.right = None;
    }
    if changed_direction != NativeLayoutDirection::Top
        && layout.top.as_deref() == Some(selected_device_id.as_str())
    {
        layout.top = None;
    }
    if changed_direction != NativeLayoutDirection::Bottom
        && layout.bottom.as_deref() == Some(selected_device_id.as_str())
    {
        layout.bottom = None;
    }
}

fn native_layout_direction_value(
    layout: &DesktopLayout,
    direction: NativeLayoutDirection,
) -> Option<&str> {
    match direction {
        NativeLayoutDirection::Left => layout.left.as_deref(),
        NativeLayoutDirection::Right => layout.right.as_deref(),
        NativeLayoutDirection::Top => layout.top.as_deref(),
        NativeLayoutDirection::Bottom => layout.bottom.as_deref(),
    }
}

fn native_layout_center_device_name(state: &DesktopState, edited_device_name: &str) -> String {
    let edited_device_name = edited_device_name.trim();
    if edited_device_name.is_empty() {
        state.device.name.clone()
    } else {
        edited_device_name.to_string()
    }
}

fn native_master_assignment_text(state: &DesktopState) -> String {
    let Some(master_device_id) = state.master_device_id.as_deref() else {
        return "当前未配置主电脑".to_string();
    };
    if state.device.id.as_deref() == Some(master_device_id) {
        return "当前电脑是主电脑".to_string();
    }
    match state
        .devices
        .iter()
        .find(|device| device.id == master_device_id)
    {
        Some(device) => format!(
            "主电脑：{}（{}）",
            device.name,
            if device.online { "在线" } else { "离线" }
        ),
        None => format!("主电脑：{master_device_id}（未知）"),
    }
}

#[cfg(test)]
fn native_layout_slot_status_text(
    state: &DesktopState,
    selected_device_id: Option<&str>,
) -> String {
    let Some(selected_device_id) = selected_device_id else {
        return "未配置".to_string();
    };
    let slot = native_layout_slot_view(state, Some(selected_device_id));
    format!("{}：{}", slot.device_name, slot.status_label)
}

fn native_layout_slot_view(
    state: &DesktopState,
    selected_device_id: Option<&str>,
) -> NativeLayoutSlotView {
    let Some(selected_device_id) = selected_device_id else {
        return NativeLayoutSlotView {
            device_name: "未配置设备".to_string(),
            status_label: "未配置".to_string(),
            route_hint: "该边缘保持本机控制".to_string(),
            tone: NativeStatusTone::Muted,
        };
    };
    match state
        .devices
        .iter()
        .find(|device| device.id == selected_device_id)
    {
        Some(device) if device.online => NativeLayoutSlotView {
            device_name: device.name.clone(),
            status_label: "在线".to_string(),
            route_hint: device
                .lan_ips
                .first()
                .map_or_else(|| "等待 LAN 地址".to_string(), |ip| format!("LAN：{ip}")),
            tone: NativeStatusTone::Success,
        },
        Some(device) => NativeLayoutSlotView {
            device_name: device.name.clone(),
            status_label: "离线".to_string(),
            route_hint: "保留配置，等待心跳".to_string(),
            tone: NativeStatusTone::Muted,
        },
        None => NativeLayoutSlotView {
            device_name: "未知设备".to_string(),
            status_label: "未知".to_string(),
            route_hint: selected_device_id.to_string(),
            tone: NativeStatusTone::Warning,
        },
    }
}

fn native_device_list_rows(
    state: &DesktopState,
    edited_device_name: &str,
) -> Vec<NativeDeviceListRow> {
    let mut rows = vec![NativeDeviceListRow {
        name: native_layout_center_device_name(state, edited_device_name),
        detail: format!("{}，本机", state.device.os),
        status: "当前电脑".to_string(),
        status_tone: NativeStatusTone::Info,
        lan_ips: state.network.lan_ips.clone(),
    }];
    rows.extend(state.devices.iter().map(|device| NativeDeviceListRow {
        name: device.name.clone(),
        detail: native_device_row_detail(&state.layout, device),
        status: if device.online { "在线" } else { "离线" }.to_string(),
        status_tone: if device.online {
            NativeStatusTone::Success
        } else {
            NativeStatusTone::Muted
        },
        lan_ips: device.lan_ips.clone(),
    }));
    rows
}

fn native_device_row_detail(
    layout: &DesktopLayout,
    device: &kmsync_core::DesktopPeerState,
) -> String {
    native_layout_position_for_device(layout, &device.id).map_or_else(
        || device.os.clone(),
        |position| format!("{}，{position}", device.os),
    )
}

fn native_layout_position_for_device(
    layout: &DesktopLayout,
    device_id: &str,
) -> Option<&'static str> {
    if layout.left.as_deref() == Some(device_id) {
        Some("左侧")
    } else if layout.right.as_deref() == Some(device_id) {
        Some("右侧")
    } else if layout.top.as_deref() == Some(device_id) {
        Some("上方")
    } else if layout.bottom.as_deref() == Some(device_id) {
        Some("下方")
    } else {
        None
    }
}

fn master_device_cell(ui: &mut egui::Ui, device_name: &str, is_master: bool, width: f32) {
    native_tinted_card(
        ui,
        width,
        NATIVE_LAYOUT_CENTER_HEIGHT,
        NativeStatusTone::Info,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("主电脑")
                        .strong()
                        .color(native_info_text()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    native_status_chip(
                        ui,
                        if is_master {
                            "当前电脑"
                        } else {
                            "已配置主电脑"
                        },
                        if is_master {
                            NativeStatusTone::Success
                        } else {
                            NativeStatusTone::Info
                        },
                    );
                });
            });
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(device_name)
                    .strong()
                    .color(native_primary_text()),
            );
            ui.colored_label(native_muted_text(), "鼠标键盘从这里出发");
        },
    );
}

fn native_layout_combo_width(available_width: f32) -> f32 {
    native_fit_grid_column_width(
        available_width,
        3,
        native_layout_grid_horizontal_spacing(),
        NATIVE_LAYOUT_GRID_MIN_COL_WIDTH,
    )
}

fn native_top_panel_widths(available_width: f32) -> NativeSplitPanelWidths {
    let content_width = (available_width - NATIVE_PANEL_SPACING).max(1.0);
    let primary = (content_width * 0.47).max(1.0);
    NativeSplitPanelWidths {
        primary,
        secondary: (content_width - primary).max(1.0),
    }
}

fn native_lower_panel_widths(available_width: f32) -> NativeSplitPanelWidths {
    let content_width = (available_width - NATIVE_PANEL_SPACING).max(1.0);
    let secondary = (content_width * 0.36).clamp(320.0, 440.0);
    let primary = (content_width - secondary).max(1.0);
    NativeSplitPanelWidths { primary, secondary }
}

#[cfg(test)]
fn native_devices_grid_column_width(available_width: f32) -> f32 {
    native_fit_grid_column_width(
        available_width,
        4,
        native_devices_grid_horizontal_spacing(),
        NATIVE_DEVICES_GRID_MIN_COL_WIDTH,
    )
}

#[cfg(test)]
fn native_devices_grid_column_widths(available_width: f32) -> NativeDeviceGridColumnWidths {
    let min_widths = NativeDeviceGridColumnWidths {
        name: NATIVE_DEVICES_GRID_NAME_MIN_WIDTH,
        status: NATIVE_DEVICES_GRID_STATUS_MIN_WIDTH,
        lan_ip: NATIVE_DEVICES_GRID_LAN_IP_MIN_WIDTH,
        public_ip: NATIVE_DEVICES_GRID_PUBLIC_IP_MIN_WIDTH,
    };
    let spacing = native_devices_grid_horizontal_spacing();
    let gap_width = spacing * 3.0;
    let available_content_width = (available_width - gap_width).max(4.0);
    let min_content_width = min_widths.content_width();
    if available_content_width < min_content_width {
        let scale = available_content_width / min_content_width;
        return NativeDeviceGridColumnWidths {
            name: (min_widths.name * scale).max(1.0),
            status: (min_widths.status * scale).max(1.0),
            lan_ip: (min_widths.lan_ip * scale).max(1.0),
            public_ip: (min_widths.public_ip * scale).max(1.0),
        };
    }

    let extra = available_content_width - min_content_width;
    NativeDeviceGridColumnWidths {
        name: min_widths.name + extra * 0.25,
        status: min_widths.status + extra * 0.15,
        lan_ip: min_widths.lan_ip + extra * 0.40,
        public_ip: min_widths.public_ip + extra * 0.20,
    }
}

fn native_fit_grid_column_width(
    available_width: f32,
    column_count: usize,
    horizontal_spacing: f32,
    preferred_min_width: f32,
) -> f32 {
    let column_count = column_count.max(1);
    let gap_width = horizontal_spacing * column_count.saturating_sub(1) as f32;
    let fitted_width = ((available_width - gap_width) / column_count as f32).max(1.0);
    if native_grid_total_width(preferred_min_width, column_count, horizontal_spacing)
        <= available_width
    {
        fitted_width.max(preferred_min_width)
    } else {
        fitted_width
    }
}

fn native_grid_total_width(column_width: f32, column_count: usize, horizontal_spacing: f32) -> f32 {
    let column_count = column_count.max(1);
    column_width * column_count as f32 + horizontal_spacing * column_count.saturating_sub(1) as f32
}

fn native_layout_grid_horizontal_spacing() -> f32 {
    NATIVE_LAYOUT_GRID_HORIZONTAL_SPACING
}

#[cfg(test)]
fn native_devices_grid_horizontal_spacing() -> f32 {
    NATIVE_DEVICES_GRID_HORIZONTAL_SPACING
}

fn native_lan_ip_popup_position(button_rect: egui::Rect) -> egui::Pos2 {
    button_rect.left_bottom() + egui::vec2(0.0, NATIVE_LAN_IP_POPUP_VERTICAL_OFFSET)
}

fn native_lan_ip_summary(lan_ips: &[String]) -> NativeLanIpSummary {
    NativeLanIpSummary {
        primary: lan_ips
            .first()
            .map_or_else(|| "-".to_string(), ToString::to_string),
        has_more: lan_ips.len() > 1,
        total_count: lan_ips.len(),
    }
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
        .corner_radius(egui::CornerRadius::same(7))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(147, 197, 253),
        )),
    )
}

fn native_primary_action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        native_action_button_size(),
        egui::Button::new(
            egui::RichText::new(label)
                .strong()
                .color(egui::Color32::from_rgb(246, 249, 255)),
        )
        .fill(egui::Color32::from_rgb(35, 88, 184))
        .corner_radius(egui::CornerRadius::same(7))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(35, 88, 184))),
    )
}

fn native_action_button_size() -> egui::Vec2 {
    egui::vec2(NATIVE_ACTION_BUTTON_WIDTH, NATIVE_ACTION_BUTTON_HEIGHT)
}

#[cfg(test)]
fn native_action_button_labels() -> [&'static str; 5] {
    [
        "保存服务器配置",
        "刷新",
        "保存当前电脑配置",
        "保存设备位置",
        "刷新",
    ]
}

fn status_message_for_state(state: &DesktopState, fallback: &str) -> String {
    if let Some(error) = state.server_error.as_deref() {
        return error.to_string();
    }
    if let Some(error) = state.master_error.as_deref() {
        return error.to_string();
    }
    fallback.to_string()
}

fn native_current_device_fact_labels() -> [&'static str; 3] {
    ["内网 IP", "监听端口", "最近心跳"]
}

fn native_top_status_labels() -> [&'static str; 1] {
    ["服务器"]
}

fn connection_state_label(state: &DesktopConnectionState) -> &'static str {
    match state {
        DesktopConnectionState::Connecting => "连接中",
        DesktopConnectionState::Connected => "已连接",
        DesktopConnectionState::Disconnected => "未连接",
        DesktopConnectionState::Retrying => "正在重试",
        DesktopConnectionState::SelfDevice => "当前电脑",
    }
}

fn connection_state_tone(state: &DesktopConnectionState) -> NativeStatusTone {
    match state {
        DesktopConnectionState::Connected => NativeStatusTone::Success,
        DesktopConnectionState::Connecting => NativeStatusTone::Danger,
        DesktopConnectionState::Retrying => NativeStatusTone::Warning,
        DesktopConnectionState::SelfDevice => NativeStatusTone::Info,
        DesktopConnectionState::Disconnected => NativeStatusTone::Muted,
    }
}

#[cfg(test)]
fn connection_state_color(state: &DesktopConnectionState) -> egui::Color32 {
    match connection_state_tone(state) {
        NativeStatusTone::Success => egui::Color32::from_rgb(21, 128, 61),
        NativeStatusTone::Danger => egui::Color32::from_rgb(185, 28, 28),
        NativeStatusTone::Warning => egui::Color32::from_rgb(180, 83, 9),
        NativeStatusTone::Info => egui::Color32::from_rgb(37, 99, 235),
        NativeStatusTone::Muted => egui::Color32::from_rgb(100, 116, 139),
    }
}

fn native_tone_colors(tone: NativeStatusTone) -> NativeToneColors {
    match tone {
        NativeStatusTone::Success => NativeToneColors {
            text: egui::Color32::from_rgb(20, 114, 74),
            fill: egui::Color32::from_rgb(231, 243, 236),
            stroke: egui::Color32::from_rgb(185, 220, 199),
        },
        NativeStatusTone::Danger => NativeToneColors {
            text: egui::Color32::from_rgb(173, 61, 54),
            fill: egui::Color32::from_rgb(252, 236, 234),
            stroke: egui::Color32::from_rgb(244, 195, 189),
        },
        NativeStatusTone::Warning => NativeToneColors {
            text: egui::Color32::from_rgb(175, 85, 36),
            fill: egui::Color32::from_rgb(255, 240, 227),
            stroke: egui::Color32::from_rgb(240, 201, 168),
        },
        NativeStatusTone::Info => NativeToneColors {
            text: egui::Color32::from_rgb(36, 81, 166),
            fill: egui::Color32::from_rgb(238, 244, 255),
            stroke: egui::Color32::from_rgb(195, 208, 232),
        },
        NativeStatusTone::Muted => NativeToneColors {
            text: egui::Color32::from_rgb(104, 114, 105),
            fill: egui::Color32::from_rgb(238, 241, 236),
            stroke: egui::Color32::from_rgb(213, 220, 209),
        },
    }
}

fn native_app_background() -> egui::Color32 {
    egui::Color32::from_rgb(245, 246, 242)
}

fn native_panel_background() -> egui::Color32 {
    egui::Color32::from_rgb(251, 252, 248)
}

fn native_canvas_background() -> egui::Color32 {
    egui::Color32::from_rgb(247, 248, 244)
}

fn native_subtle_background() -> egui::Color32 {
    egui::Color32::from_rgb(248, 250, 246)
}

fn native_panel_stroke() -> egui::Color32 {
    egui::Color32::from_rgb(221, 227, 218)
}

fn native_primary_text() -> egui::Color32 {
    egui::Color32::from_rgb(31, 41, 35)
}

fn native_muted_text() -> egui::Color32 {
    egui::Color32::from_rgb(105, 115, 108)
}

fn native_info_text() -> egui::Color32 {
    egui::Color32::from_rgb(36, 81, 166)
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
        assert_eq!(view_model.device_name, "This PC");
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

        assert_eq!(metrics.window_size, [1120.0, 880.0]);
        assert_eq!(metrics.min_window_size, [900.0, 700.0]);
        assert!(metrics.top_panel_min_height <= 220.0);
        assert_eq!(metrics.lower_panel_columns, 2);
        assert_eq!(
            metrics.layout_panel_min_height,
            metrics.devices_panel_min_height
        );
        assert!(metrics.layout_panel_min_height >= 280.0);
        assert!(metrics.devices_grid_min_col_width <= 96.0);
    }

    #[test]
    fn native_lower_grids_fit_inside_two_column_min_window() {
        let lower_column_content_width = 420.0;

        let layout_combo_width = native_layout_combo_width(lower_column_content_width);
        assert!(
            native_grid_total_width(
                layout_combo_width,
                3,
                native_layout_grid_horizontal_spacing()
            ) <= lower_column_content_width
        );

        let devices_column_width = native_devices_grid_column_width(lower_column_content_width);
        assert!(
            native_grid_total_width(
                devices_column_width,
                4,
                native_devices_grid_horizontal_spacing()
            ) <= lower_column_content_width
        );

        let device_widths = native_devices_grid_column_widths(lower_column_content_width);
        assert!(device_widths.lan_ip > device_widths.status);
        assert!(device_widths.lan_ip > device_widths.public_ip);
        assert!(
            device_widths.total_width(native_devices_grid_horizontal_spacing())
                <= lower_column_content_width
        );
    }

    #[test]
    fn native_redesign_panel_widths_prioritize_topology_canvas() {
        let lower = native_lower_panel_widths(1080.0);

        assert!(lower.primary > lower.secondary);
        assert!(lower.primary >= 620.0);
        assert!(lower.secondary >= 360.0);
        assert!(lower.total_width(16.0) <= 1080.0);
    }

    #[test]
    fn native_lan_ip_popup_opens_next_to_more_button() {
        let button_rect =
            egui::Rect::from_min_size(egui::pos2(320.0, 180.0), egui::vec2(44.0, 20.0));

        let popup_pos = native_lan_ip_popup_position(button_rect);

        assert_eq!(popup_pos.x, button_rect.min.x);
        assert!(popup_pos.y > button_rect.max.y);
    }

    #[test]
    fn native_current_device_facts_do_not_show_public_ip() {
        assert_eq!(
            native_current_device_fact_labels(),
            ["内网 IP", "监听端口", "最近心跳"]
        );
    }

    #[test]
    fn native_top_status_labels_exclude_master_connection_status() {
        assert_eq!(native_top_status_labels(), ["服务器"]);
    }

    #[test]
    fn native_master_assignment_text_distinguishes_unconfigured_and_offline_master() {
        let unconfigured = DesktopState {
            device: DesktopDeviceState {
                id: Some("client".to_string()),
                name: "Client PC".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Client,
            },
            master_device_id: None,
            ..DesktopState::default()
        };
        assert_eq!(
            native_master_assignment_text(&unconfigured),
            "当前未配置主电脑"
        );

        let offline = DesktopState {
            device: DesktopDeviceState {
                id: Some("client".to_string()),
                name: "Client PC".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Client,
            },
            master_device_id: Some("master".to_string()),
            devices: vec![DesktopPeerState {
                id: "master".to_string(),
                name: "Master PC".to_string(),
                os: "macos".to_string(),
                online: false,
                lan_ips: vec![],
                public_ip: None,
                listen_port: None,
                last_seen_at: None,
            }],
            ..DesktopState::default()
        };

        assert_eq!(
            native_master_assignment_text(&offline),
            "主电脑：Master PC（离线）"
        );
    }

    #[test]
    fn native_layout_slot_status_reports_configured_peer_presence() {
        let state = DesktopState {
            devices: vec![DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                os: "windows".to_string(),
                online: true,
                lan_ips: vec![],
                public_ip: None,
                listen_port: None,
                last_seen_at: None,
            }],
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_slot_status_text(&state, Some("right-device")),
            "Right PC：在线"
        );
        assert_eq!(native_layout_slot_status_text(&state, None), "未配置");
    }

    #[test]
    fn native_layout_slot_views_match_topology_card_states() {
        let state = DesktopState {
            devices: vec![
                DesktopPeerState {
                    id: "right-device".to_string(),
                    name: "Right PC".to_string(),
                    os: "windows".to_string(),
                    online: true,
                    lan_ips: vec!["192.168.1.21".to_string()],
                    public_ip: None,
                    listen_port: Some(24_800),
                    last_seen_at: None,
                },
                DesktopPeerState {
                    id: "bottom-device".to_string(),
                    name: "Bottom PC".to_string(),
                    os: "linux".to_string(),
                    online: false,
                    lan_ips: vec!["192.168.1.30".to_string()],
                    public_ip: None,
                    listen_port: Some(24_800),
                    last_seen_at: None,
                },
            ],
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_slot_view(&state, None),
            NativeLayoutSlotView {
                device_name: "未配置设备".to_string(),
                status_label: "未配置".to_string(),
                route_hint: "该边缘保持本机控制".to_string(),
                tone: NativeStatusTone::Muted,
            }
        );
        assert_eq!(
            native_layout_slot_view(&state, Some("right-device")),
            NativeLayoutSlotView {
                device_name: "Right PC".to_string(),
                status_label: "在线".to_string(),
                route_hint: "LAN：192.168.1.21".to_string(),
                tone: NativeStatusTone::Success,
            }
        );
        assert_eq!(
            native_layout_slot_view(&state, Some("bottom-device")),
            NativeLayoutSlotView {
                device_name: "Bottom PC".to_string(),
                status_label: "离线".to_string(),
                route_hint: "保留配置，等待心跳".to_string(),
                tone: NativeStatusTone::Muted,
            }
        );
        assert_eq!(
            native_layout_slot_view(&state, Some("missing-device")),
            NativeLayoutSlotView {
                device_name: "未知设备".to_string(),
                status_label: "未知".to_string(),
                route_hint: "missing-device".to_string(),
                tone: NativeStatusTone::Warning,
            }
        );
    }

    #[test]
    fn native_layout_options_exclude_current_device() {
        let state = DesktopState {
            device: DesktopDeviceState {
                id: Some("current".to_string()),
                name: "This PC".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Master,
            },
            devices: vec![
                DesktopPeerState {
                    id: "current".to_string(),
                    name: "This PC".to_string(),
                    os: "windows".to_string(),
                    online: true,
                    lan_ips: vec![],
                    public_ip: None,
                    listen_port: None,
                    last_seen_at: None,
                },
                DesktopPeerState {
                    id: "right-device".to_string(),
                    name: "Right PC".to_string(),
                    os: "macos".to_string(),
                    online: true,
                    lan_ips: vec![],
                    public_ip: None,
                    listen_port: None,
                    last_seen_at: None,
                },
            ],
            ..DesktopState::default()
        };

        let options = native_layout_device_options(&state);

        assert_eq!(
            options,
            vec![("right-device".to_string(), "Right PC".to_string())]
        );
    }

    #[test]
    fn native_layout_deduplicates_device_when_new_direction_selects_existing_target() {
        let mut layout = DesktopLayout {
            left: Some("device-a".to_string()),
            right: None,
            top: Some("device-b".to_string()),
            bottom: None,
        };

        layout.right = Some("device-a".to_string());
        native_layout_clear_duplicate_targets(&mut layout, NativeLayoutDirection::Right);

        assert_eq!(layout.left, None);
        assert_eq!(layout.right.as_deref(), Some("device-a"));
        assert_eq!(layout.top.as_deref(), Some("device-b"));

        layout.bottom = Some("device-b".to_string());
        native_layout_clear_duplicate_targets(&mut layout, NativeLayoutDirection::Bottom);

        assert_eq!(layout.top, None);
        assert_eq!(layout.bottom.as_deref(), Some("device-b"));
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
                "刷新",
                "保存当前电脑配置",
                "保存设备位置",
                "刷新"
            ]
        );
        assert_eq!(native_action_button_size(), egui::vec2(150.0, 34.0));
    }

    #[test]
    fn native_lan_ip_summary_shows_one_address_and_more_state() {
        assert_eq!(
            native_lan_ip_summary(&[]),
            NativeLanIpSummary {
                primary: "-".to_string(),
                has_more: false,
                total_count: 0
            }
        );
        assert_eq!(
            native_lan_ip_summary(&["192.168.1.21".to_string()]),
            NativeLanIpSummary {
                primary: "192.168.1.21".to_string(),
                has_more: false,
                total_count: 1
            }
        );
        assert_eq!(
            native_lan_ip_summary(&["192.168.1.21".to_string(), "10.0.0.8".to_string()]),
            NativeLanIpSummary {
                primary: "192.168.1.21".to_string(),
                has_more: true,
                total_count: 2
            }
        );
    }

    #[test]
    fn native_device_rows_include_live_current_device_name() {
        let state = DesktopState {
            device: DesktopDeviceState {
                id: Some("current".to_string()),
                name: "Old Name".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Master,
            },
            network: DesktopNetworkState {
                lan_ips: vec!["192.168.1.20".to_string()],
                listen_port: Some(24_800),
                ..DesktopNetworkState::default()
            },
            devices: vec![DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                os: "macos".to_string(),
                online: true,
                lan_ips: vec!["192.168.1.21".to_string()],
                public_ip: None,
                listen_port: Some(24_800),
                last_seen_at: Some(124),
            }],
            ..DesktopState::default()
        };

        let rows = native_device_list_rows(&state, "Renamed PC");

        assert_eq!(
            rows,
            vec![
                NativeDeviceListRow {
                    name: "Renamed PC".to_string(),
                    detail: "windows，本机".to_string(),
                    status: "当前电脑".to_string(),
                    status_tone: NativeStatusTone::Info,
                    lan_ips: vec!["192.168.1.20".to_string()],
                },
                NativeDeviceListRow {
                    name: "Right PC".to_string(),
                    detail: "macos".to_string(),
                    status: "在线".to_string(),
                    status_tone: NativeStatusTone::Success,
                    lan_ips: vec!["192.168.1.21".to_string()],
                }
            ]
        );
        assert_eq!(
            native_layout_center_device_name(&state, "Renamed PC"),
            "Renamed PC"
        );
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
