use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
#[cfg(test)]
use kmsync_core::DesktopPermissionState;
use kmsync_core::{DesktopConnectionState, DesktopLayout, DesktopRole, DesktopState};

const NATIVE_CJK_FONT_NAME: &str = "kmsync_cjk";
const NATIVE_WINDOW_SIZE: [f32; 2] = [1000.0, 700.0];
const NATIVE_WINDOW_MIN_SIZE: [f32; 2] = NATIVE_WINDOW_SIZE;
const NATIVE_PAGE_MARGIN_X: i8 = 28;
const NATIVE_PAGE_MARGIN_Y: i8 = 20;
const NATIVE_HEADER_TOTAL_HEIGHT: f32 = 62.0;
const NATIVE_HEADER_CONTENT_HEIGHT: f32 = 42.0;
const NATIVE_HEADER_LOGO_CONTAINER_SIZE: f32 = 38.0;
const NATIVE_HEADER_LOGO_SIZE: f32 = 28.0;
const NATIVE_HEADER_LOGO_CORNER_RADIUS: u8 = 10;
const NATIVE_HEADER_STATUS_GAP: f32 = 8.0;
const NATIVE_AFTER_HEADER_GAP: f32 = 16.0;
const NATIVE_ROW_GAP: f32 = 20.0;
const NATIVE_TOP_PANEL_MIN_HEIGHT: f32 = 158.0;
const NATIVE_LOWER_PANEL_MIN_HEIGHT: f32 = 384.0;
const NATIVE_PANEL_SPACING: f32 = 14.0;
const NATIVE_LAYOUT_GRID_MIN_COL_WIDTH: f32 = 68.0;
const NATIVE_LAYOUT_GRID_MAX_COL_WIDTH: f32 = 154.0;
const NATIVE_LAYOUT_GRID_HORIZONTAL_SPACING: f32 = 24.0;
const NATIVE_DEVICES_GRID_MIN_COL_WIDTH: f32 = 64.0;
const NATIVE_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const NATIVE_DEVICES_GRID_NAME_MIN_WIDTH: f32 = 60.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_STATUS_MIN_WIDTH: f32 = 49.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_LAN_IP_MIN_WIDTH: f32 = 98.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_PUBLIC_IP_MIN_WIDTH: f32 = 60.0;
#[cfg(test)]
const NATIVE_DEVICES_GRID_HORIZONTAL_SPACING: f32 = 8.0;
const NATIVE_ACTION_BUTTON_WIDTH: f32 = 105.0;
const NATIVE_COMPACT_ACTION_BUTTON_WIDTH: f32 = 84.0;
const NATIVE_ACTION_BUTTON_HEIGHT: f32 = 28.0;
const NATIVE_STATUS_CHIP_HEIGHT: f32 = 26.0;
const NATIVE_LAN_IP_POPUP_VERTICAL_OFFSET: f32 = 4.0;
const NATIVE_LAYOUT_HEADER_HEIGHT: f32 = 44.0;
const NATIVE_LAYOUT_SLOT_HEIGHT: f32 = 82.0;
const NATIVE_LAYOUT_CENTER_HEIGHT: f32 = NATIVE_LAYOUT_SLOT_HEIGHT;
const NATIVE_LAYOUT_CANVAS_CONTENT_HEIGHT: f32 = 260.0;
const NATIVE_DEVICE_ROW_HEIGHT: f32 = 50.0;
const NATIVE_DEVICE_ROW_IP_WIDTH: f32 = 116.0;
const NATIVE_DELETE_DEVICE_BUTTON_WIDTH: f32 = 42.0;
const NATIVE_DELETE_DEVICE_BUTTON_HEIGHT: f32 = 24.0;

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

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeServerFormWidths {
    host: f32,
    port: f32,
    pre_button_gap: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeCurrentDeviceFormWidths {
    name: f32,
    id: f32,
    pre_button_gap: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeCurrentDeviceMetricWidths {
    server: f32,
    metric_gap: f32,
    lan_ip: f32,
    pre_refresh_gap: f32,
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
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("KMSync")
        .with_inner_size(metrics.window_size)
        .with_min_inner_size(metrics.min_window_size)
        .with_max_inner_size(metrics.window_size)
        .with_resizable(false);
    if let Some(icon) = native_logo_icon_data() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        persist_window: native_persist_window_size(),
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

fn native_persist_window_size() -> bool {
    false
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
    logo_texture: Option<egui::TextureHandle>,
    next_auto_refresh_at: Instant,
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
    id: Option<String>,
    name: String,
    detail: String,
    status: String,
    status_tone: NativeStatusTone,
    lan_ips: Vec<String>,
    can_delete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeLayoutSlotView {
    device_name: String,
    status_label: String,
    route_hint: String,
    tone: NativeStatusTone,
}

#[derive(Debug, Clone, Copy)]
enum NativeDeviceRowAction {
    ShowLanIps(egui::Pos2),
    Delete,
}

#[derive(Debug, Clone, Copy)]
struct NativeToneColors {
    text: egui::Color32,
    fill: egui::Color32,
    stroke: egui::Color32,
}

impl NativeDesktopApp {
    fn load_state(config_path: &Path) -> Result<DesktopState, String> {
        let mut state = crate::build_local_desktop_state(config_path)?;
        crate::attach_current_core_service_health_status(&mut state);
        Ok(state)
    }

    fn load(config_path: PathBuf) -> Result<Self, String> {
        let state = Self::load_state(&config_path)?;
        maybe_request_platform_permissions(&state);
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
            logo_texture: None,
            next_auto_refresh_at: Instant::now() + NATIVE_AUTO_REFRESH_INTERVAL,
        })
    }

    fn reload_state(&mut self, success_message: &str) {
        match Self::load_state(&self.config_path) {
            Ok(state) => {
                let status_message = status_message_for_state(&state, success_message);
                maybe_request_platform_permissions(&state);
                self.apply_state(state);
                self.status_message = status_message;
            }
            Err(error) => {
                self.status_message = format!("刷新失败：{error}");
            }
        }
        self.next_auto_refresh_at = Instant::now() + NATIVE_AUTO_REFRESH_INTERVAL;
    }

    fn reload_state_quietly(&mut self) {
        if let Ok(state) = Self::load_state(&self.config_path) {
            self.apply_state(state);
        }
    }

    fn apply_state(&mut self, state: DesktopState) {
        let view_model = NativeDesktopViewModel::from_state(&state);
        self.state = state;
        self.server_host = view_model.server_host;
        self.server_port = view_model.server_port;
        self.device_name = view_model.device_name;
        self.is_master = view_model.is_master;
        self.layout = view_model.layout;
    }

    fn refresh_if_due(&mut self, now: Instant) {
        if now >= self.next_auto_refresh_at {
            self.reload_state_quietly();
            self.next_auto_refresh_at = now + NATIVE_AUTO_REFRESH_INTERVAL;
        }
    }

    fn request_platform_permissions(&mut self) {
        crate::platform::request_platform_permissions();
        self.reload_state("已请求系统权限");
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
                        self.reload_state("本机配置已保存并同步");
                    }
                    Err(error) => {
                        self.reload_state("本机配置已保存");
                        self.status_message = format!("本机配置已保存，设备名称同步失败：{error}");
                    }
                }
            }
            Err(error) => {
                self.status_message = format!("本机配置保存失败：{error}");
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

    fn delete_device(&mut self, device_id: &str) {
        if self.state.device.id.as_deref() == Some(device_id) {
            self.status_message = "不能删除本机".to_string();
            return;
        }
        let config = match crate::client::ClientConfig::load(&self.config_path) {
            Ok(config) => config,
            Err(error) => {
                self.status_message = format!("删除设备失败：{error}");
                return;
            }
        };
        let client = crate::client::ControlClient::new(config.server_url);
        if let Err(error) = client.delete_device(device_id) {
            self.status_message = format!("删除设备失败：{error}");
            return;
        }

        let cleaned_layout = native_layout_without_device(&self.layout, device_id);
        let mut master_device_id = if self.is_master {
            self.state.device.id.clone()
        } else {
            self.state.master_device_id.clone()
        };
        if master_device_id.as_deref() == Some(device_id) {
            master_device_id = None;
        }
        let role = if self.state.device.id.as_deref() == master_device_id.as_deref()
            && master_device_id.is_some()
        {
            DesktopRole::Master
        } else {
            DesktopRole::Client
        };

        match crate::desktop_config::set_topology_in_config_file(
            &self.config_path,
            role,
            master_device_id.as_deref(),
            &cleaned_layout,
        ) {
            Ok(()) => {
                self.layout = cleaned_layout;
                match self.sync_topology_to_server(master_device_id) {
                    Ok(()) => self.reload_state("设备已删除，已断开跟主电脑和服务器的链接"),
                    Err(error) => {
                        self.reload_state("设备已删除");
                        self.status_message =
                            format!("设备已删除，本地位置已清理，同步失败：{error}");
                    }
                }
            }
            Err(error) => {
                self.status_message = format!("设备已删除，本地位置清理失败：{error}");
                self.reload_state("设备已删除");
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

    fn mark_current_device_offline(&self) {
        let result = crate::client::ClientConfig::load(&self.config_path)
            .and_then(|config| crate::client::mark_current_device_offline(&config).map(|_| ()));
        if let Err(error) = result {
            eprintln!("failed to mark current device offline: {error}");
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
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.mark_current_device_offline();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_if_due(Instant::now());
        ctx.request_repaint_after(NATIVE_AUTO_REFRESH_INTERVAL);
        if self.logo_texture.is_none() {
            self.logo_texture = native_logo_texture(ctx);
        }
        let logo_texture = self.logo_texture.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(native_app_background())
                    .inner_margin(egui::Margin::symmetric(
                        NATIVE_PAGE_MARGIN_X,
                        NATIVE_PAGE_MARGIN_Y,
                    )),
            )
            .show(ctx, |ui| {
                native_app_header(ui, &self.state, logo_texture.as_ref(), &self.status_message);
                ui.add_space(NATIVE_AFTER_HEADER_GAP);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let metrics = native_desktop_layout_metrics();
                        let top_widths = native_top_panel_widths(ui.available_width());
                        let previous_spacing = ui.spacing().item_spacing;
                        ui.spacing_mut().item_spacing.x = NATIVE_PANEL_SPACING;
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
                        ui.spacing_mut().item_spacing = previous_spacing;
                        ui.add_space(NATIVE_ROW_GAP);
                        let lower_widths = native_lower_panel_widths(ui.available_width());
                        let previous_spacing = ui.spacing().item_spacing;
                        ui.spacing_mut().item_spacing.x = NATIVE_PANEL_SPACING;
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
                        ui.spacing_mut().item_spacing = previous_spacing;
                    });
            });
        self.lan_ip_popup_window(ctx);
    }
}

fn maybe_request_platform_permissions(state: &DesktopState) {
    if native_should_auto_request_platform_permissions(state) {
        crate::platform::request_platform_permissions();
    }
}

fn native_should_auto_request_platform_permissions(_state: &DesktopState) -> bool {
    false
}

fn native_should_show_permission_request_button(state: &DesktopState) -> bool {
    state.permissions.iter().any(|permission| {
        permission.status == "missing"
            && matches!(
                permission.key.as_str(),
                "macos.accessibility" | "macos.input_monitoring"
            )
    })
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
        ui.add_space(8.0);
        let widths = native_server_form_widths(ui.available_width());
        ui.horizontal_top(|ui| {
            let previous_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.x = 0.0;
            native_labeled_text_edit(ui, "服务器 IP / 域名", &mut self.server_host, widths.host);
            ui.add_space(10.0);
            native_labeled_text_edit(ui, "端口", &mut self.server_port, widths.port);
            ui.add_space(widths.pre_button_gap);
            ui.vertical(|ui| {
                ui.add_space(19.0);
                if native_action_button(ui, "保存服务器配置").clicked() {
                    self.save_server_endpoint();
                }
            });
            ui.spacing_mut().item_spacing = previous_spacing;
        });
        ui.add_space(10.0);
        ui.monospace(format!("完整地址：{}", self.server_url_preview()));
        ui.add_space(5.0);
        ui.colored_label(native_muted_text(), "配置保存到本地文件，应用会自动重连。");
    }

    fn current_device_section(&mut self, ui: &mut egui::Ui) {
        native_section_title(
            ui,
            "本机",
            Some((
                if self.is_master {
                    "主电脑"
                } else {
                    "从电脑"
                },
                NativeStatusTone::Info,
            )),
        );
        ui.add_space(8.0);
        let form_widths = native_current_device_form_widths(ui.available_width());
        ui.horizontal_top(|ui| {
            let previous_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.x = 0.0;
            native_labeled_text_edit(ui, "设备名称", &mut self.device_name, form_widths.name);
            ui.add_space(10.0);
            native_readonly_field(
                ui,
                "设备 ID",
                self.state.device.id.as_deref().unwrap_or("-"),
                form_widths.id,
            );
            ui.add_space(form_widths.pre_button_gap);
            ui.vertical(|ui| {
                ui.add_space(19.0);
                if native_action_button(ui, "保存本机配置").clicked() {
                    self.save_current_device_config();
                }
            });
            ui.spacing_mut().item_spacing = previous_spacing;
        });
        ui.add_space(10.0);
        let metric_widths = native_current_device_metric_widths(ui.available_width());
        let show_permission_request = native_should_show_permission_request_button(&self.state);
        ui.horizontal_top(|ui| {
            let previous_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.x = 0.0;
            native_metric_card(
                ui,
                native_current_device_fact_labels()[0],
                connection_state_label(&self.state.server_state),
                connection_state_tone(&self.state.server_state),
                metric_widths.server,
            );
            ui.add_space(metric_widths.metric_gap);
            native_metric_card(
                ui,
                native_current_device_fact_labels()[1],
                &empty_dash(&self.state.network.lan_ips.join(", ")),
                NativeStatusTone::Success,
                metric_widths.lan_ip,
            );
            ui.add_space(metric_widths.pre_refresh_gap);
            ui.vertical(|ui| {
                ui.add_space(if show_permission_request { 0.0 } else { 8.0 });
                if show_permission_request {
                    if native_compact_action_button(ui, "申请权限").clicked() {
                        self.request_platform_permissions();
                    }
                    ui.add_space(4.0);
                }
                if native_compact_action_button(ui, "刷新").clicked() {
                    self.reload_state("状态已刷新");
                }
            });
            ui.spacing_mut().item_spacing = previous_spacing;
        });
    }

    fn layout_section(&mut self, ui: &mut egui::Ui) {
        let header_width = ui.available_width();
        let header_text_width =
            (header_width - NATIVE_ACTION_BUTTON_WIDTH - 10.0).clamp(1.0, header_width);
        ui.horizontal_top(|ui| {
            let previous_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(
                egui::vec2(header_text_width, NATIVE_LAYOUT_HEADER_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(header_text_width);
                    ui.horizontal(|ui| {
                        ui.heading("设备位置");
                        let (label, tone) = native_layout_section_status(&self.state);
                        native_status_chip(ui, &label, tone);
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "以主电脑为中心配置上下左右，触碰屏幕边缘时切换控制目标。",
                            )
                            .color(native_muted_text()),
                        )
                        .truncate(),
                    );
                },
            );
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(3.0);
                if native_primary_action_button(ui, "保存设备位置").clicked() {
                    self.save_layout();
                }
            });
            ui.spacing_mut().item_spacing = previous_spacing;
        });
        ui.add_space(10.0);
        let devices = native_layout_device_options(&self.state);
        let center_device_name = native_layout_center_device_name(&self.state, &self.device_name);
        let combo_width = native_layout_combo_width((ui.available_width() - 34.0).max(1.0));
        let mut edited_layout = self.layout.clone();
        let top_view = native_layout_slot_view(&self.state, edited_layout.top.as_deref());
        let left_view = native_layout_slot_view(&self.state, edited_layout.left.as_deref());
        let right_view = native_layout_slot_view(&self.state, edited_layout.right.as_deref());
        let bottom_view = native_layout_slot_view(&self.state, edited_layout.bottom.as_deref());
        native_layout_canvas(ui, |ui| {
            let leading_space = native_layout_grid_leading_space(ui.available_width(), combo_width);
            ui.horizontal_top(|ui| {
                ui.add_space(leading_space);
                egui::Grid::new("native_layout_grid")
                    .num_columns(3)
                    .spacing([native_layout_grid_horizontal_spacing(), 6.0])
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
        });
        self.layout = edited_layout;
    }

    fn devices_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("设备列表");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if native_compact_action_button(ui, "刷新").clicked() {
                    self.reload_state("设备列表已刷新");
                }
            });
        });
        ui.colored_label(
            native_muted_text(),
            "同账号下设备，名称和 IP 会随心跳刷新。",
        );
        ui.add_space(10.0);
        let rows = native_device_list_rows(&self.state, &self.device_name);
        let row_width = ui.available_width();
        let mut next_lan_ip_popup = None;
        let mut delete_device_id = None;
        let list_height = native_device_list_scroll_height(ui.available_height());
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in &rows {
                    if let Some(action) = native_device_row_card(ui, row, row_width) {
                        match action {
                            NativeDeviceRowAction::ShowLanIps(position) => {
                                next_lan_ip_popup = Some(NativeLanIpPopup {
                                    device_name: row.name.clone(),
                                    lan_ips: row.lan_ips.clone(),
                                    position,
                                });
                            }
                            NativeDeviceRowAction::Delete => {
                                delete_device_id = row.id.clone();
                            }
                        }
                    }
                    ui.add_space(7.0);
                }
            });
        ui.add_space(8.0);
        native_hint_box(
            ui,
            "提示：离线设备会保留位置配置，重新上线后自动刷新连接信息。",
        );
        if next_lan_ip_popup.is_some() {
            self.lan_ip_popup = next_lan_ip_popup;
        }
        if let Some(device_id) = delete_device_id {
            self.delete_device(&device_id);
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
                ui.set_min_width(180.0);
                for ip in &popup.lan_ips {
                    ui.monospace(ip);
                }
                ui.add_space(6.0);
                if native_action_button(ui, "关闭").clicked() {
                    close_clicked = true;
                }
            });
        if !open || close_clicked {
            self.lan_ip_popup = None;
        }
    }
}

fn native_app_header(
    ui: &mut egui::Ui,
    state: &DesktopState,
    logo_texture: Option<&egui::TextureHandle>,
    _status_message: &str,
) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, native_header_total_height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::Frame::new()
                .fill(native_panel_background())
                .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(15, 10))
                .show(ui, |ui| {
                    let content_width = (width - 32.0).max(1.0);
                    ui.set_min_size(egui::vec2(content_width, native_header_content_height()));
                    ui.set_max_height(native_header_content_height());
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, native_header_content_height()),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            native_header_brand(ui, logo_texture);
                            let labels = native_header_status_labels(state);
                            ui.add_space(
                                (ui.available_width() - native_header_status_row_width(&labels))
                                    .max(8.0),
                            );
                            native_header_status_row(ui, &labels);
                        },
                    );
                });
        },
    );
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
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_min_width((width - 30.0).max(1.0));
                    ui.set_min_height((min_height - 26.0).max(1.0));
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

fn native_header_brand(ui: &mut egui::Ui, logo_texture: Option<&egui::TextureHandle>) {
    if let Some(texture) = logo_texture {
        egui::Frame::new()
            .fill(native_header_logo_background())
            .corner_radius(egui::CornerRadius::same(native_header_logo_corner_radius()))
            .inner_margin(egui::Margin::symmetric(5, 5))
            .show(ui, |ui| {
                ui.set_min_size(egui::vec2(
                    NATIVE_HEADER_LOGO_CONTAINER_SIZE - 10.0,
                    NATIVE_HEADER_LOGO_CONTAINER_SIZE - 10.0,
                ));
                ui.add(
                    egui::Image::new((
                        texture.id(),
                        egui::vec2(NATIVE_HEADER_LOGO_SIZE, NATIVE_HEADER_LOGO_SIZE),
                    ))
                    .fit_to_exact_size(egui::vec2(NATIVE_HEADER_LOGO_SIZE, NATIVE_HEADER_LOGO_SIZE))
                    .corner_radius(egui::CornerRadius::same(native_header_logo_corner_radius())),
                );
            });
        ui.add_space(10.0);
    }
    ui.vertical(|ui| {
        ui.add_space(1.0);
        ui.label(
            egui::RichText::new("KMSync")
                .size(20.0)
                .strong()
                .color(native_primary_text()),
        );
        ui.colored_label(native_muted_text(), "桌面端控制台");
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
            egui::vec2(width, 30.0),
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
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                let content_width = (width - 16.0).max(1.0);
                ui.set_min_width(content_width);
                ui.set_max_width(content_width);
                ui.add(
                    egui::Label::new(egui::RichText::new(value).color(native_primary_text()))
                        .truncate(),
                );
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
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            let content_width = (width - 16.0).max(1.0);
            ui.set_min_width(content_width);
            ui.set_max_width(content_width);
            ui.vertical(|ui| {
                ui.set_max_width(content_width);
                ui.add(
                    egui::Label::new(egui::RichText::new(label).color(native_muted_text()))
                        .truncate(),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(value).strong().color(colors.text))
                        .truncate(),
                );
            });
        });
}

fn native_layout_canvas(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(native_canvas_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(17, 17))
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 34.0).max(1.0));
            ui.set_min_height(NATIVE_LAYOUT_CANVAS_CONTENT_HEIGHT);
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
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_min_width((width - 18.0).max(1.0));
                    ui.set_min_height((height - 10.0).max(1.0));
                    add_contents(ui);
                });
        },
    );
}

fn native_status_chip(ui: &mut egui::Ui, label: &str, tone: NativeStatusTone) {
    let colors = native_tone_colors(tone);
    let (rect, _) = ui.allocate_exact_size(native_status_chip_size(label), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(7),
            colors.fill,
            egui::Stroke::new(1.0, colors.stroke),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.0),
            colors.text,
        );
    }
}

fn native_layout_peer_status_label(ui: &mut egui::Ui, label: &str, tone: NativeStatusTone) {
    let colors = native_tone_colors(tone);
    ui.add(
        egui::Label::new(
            egui::RichText::new(label)
                .size(12.0)
                .strong()
                .color(colors.text),
        )
        .truncate(),
    );
}

#[cfg(test)]
fn native_layout_peer_status_has_border() -> bool {
    false
}

fn native_device_row_ip_width() -> f32 {
    NATIVE_DEVICE_ROW_IP_WIDTH
}

fn native_delete_device_button_size() -> egui::Vec2 {
    egui::vec2(
        NATIVE_DELETE_DEVICE_BUTTON_WIDTH,
        NATIVE_DELETE_DEVICE_BUTTON_HEIGHT,
    )
}

fn native_delete_device_button(ui: &mut egui::Ui) -> egui::Response {
    let colors = native_tone_colors(NativeStatusTone::Danger);
    ui.add_sized(
        native_delete_device_button_size(),
        egui::Button::new(
            egui::RichText::new("删除")
                .size(12.0)
                .strong()
                .color(colors.text),
        )
        .truncate()
        .fill(colors.fill)
        .corner_radius(egui::CornerRadius::same(7))
        .stroke(egui::Stroke::new(1.0, colors.stroke)),
    )
}

fn native_device_row_card(
    ui: &mut egui::Ui,
    row: &NativeDeviceListRow,
    width: f32,
) -> Option<NativeDeviceRowAction> {
    let colors = native_tone_colors(row.status_tone);
    let summary = native_lan_ip_summary(&row.lan_ips);
    let mut action = None;
    egui::Frame::new()
        .fill(native_panel_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            let content_width = (width - 22.0).max(1.0);
            ui.set_min_width(content_width);
            ui.set_max_width(content_width);
            ui.set_min_height(NATIVE_DEVICE_ROW_HEIGHT - 14.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●").size(17.0).color(colors.text));
                ui.add_space(6.0);
                let status_width = native_status_chip_size(&row.status).x;
                let more_width = if summary.has_more { 28.0 } else { 0.0 };
                let delete_width = if row.can_delete {
                    native_delete_device_button_size().x + 6.0
                } else {
                    0.0
                };
                let name_width = (content_width
                    - 17.0
                    - 6.0
                    - status_width
                    - 6.0
                    - native_device_row_ip_width()
                    - more_width
                    - delete_width)
                    .max(64.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(name_width, NATIVE_DEVICE_ROW_HEIGHT - 14.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(name_width);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&row.name)
                                    .strong()
                                    .color(native_primary_text()),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&row.detail).color(native_muted_text()),
                            )
                            .truncate(),
                        );
                    },
                );
                ui.add_space(6.0);
                native_status_chip(ui, &row.status, row.status_tone);
                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        native_device_row_ip_width(),
                        NATIVE_DEVICE_ROW_HEIGHT - 14.0,
                    ),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_max_width(native_device_row_ip_width());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(summary.primary.as_str())
                                    .strong()
                                    .color(native_primary_text()),
                            )
                            .truncate(),
                        );
                    },
                );
                if summary.has_more {
                    let response = ui.small_button("…");
                    if response.clicked() {
                        action = Some(NativeDeviceRowAction::ShowLanIps(
                            native_lan_ip_popup_position(response.rect),
                        ));
                    }
                }
                if row.can_delete {
                    ui.add_space(6.0);
                    if native_delete_device_button(ui).clicked() {
                        action = Some(NativeDeviceRowAction::Delete);
                    }
                }
            });
        });
    action
}

fn native_hint_box(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(native_canvas_background())
        .stroke(egui::Stroke::new(1.0, native_panel_stroke()))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(8, 6))
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
    native_tinted_card(ui, width, NATIVE_LAYOUT_SLOT_HEIGHT, view.tone, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(direction)
                    .strong()
                    .color(native_muted_text()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                native_layout_peer_status_label(ui, &view.status_label, view.tone);
            });
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(egui::RichText::new(&view.route_hint).color(native_muted_text()))
                .truncate(),
        );
        ui.add_space(2.0);
        egui::ComboBox::from_id_salt(("native_layout_combo", direction))
            .width((width - 24.0).max(1.0))
            .selected_text(&view.device_name)
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

fn native_layout_without_device(layout: &DesktopLayout, device_id: &str) -> DesktopLayout {
    DesktopLayout {
        left: (layout.left.as_deref() != Some(device_id))
            .then(|| layout.left.clone())
            .flatten(),
        right: (layout.right.as_deref() != Some(device_id))
            .then(|| layout.right.clone())
            .flatten(),
        top: (layout.top.as_deref() != Some(device_id))
            .then(|| layout.top.clone())
            .flatten(),
        bottom: (layout.bottom.as_deref() != Some(device_id))
            .then(|| layout.bottom.clone())
            .flatten(),
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

fn native_current_device_name(state: &DesktopState, edited_device_name: &str) -> String {
    let edited_device_name = edited_device_name.trim();
    if edited_device_name.is_empty() {
        state.device.name.clone()
    } else {
        edited_device_name.to_string()
    }
}

fn native_layout_center_device_name(state: &DesktopState, edited_device_name: &str) -> String {
    let Some(master_device_id) = state.master_device_id.as_deref() else {
        return "未配置主电脑".to_string();
    };
    if state.device.id.as_deref() == Some(master_device_id) {
        return native_current_device_name(state, edited_device_name);
    }
    state
        .devices
        .iter()
        .find(|device| device.id == master_device_id)
        .map(|device| device.name.clone())
        .unwrap_or_else(|| format!("未知主电脑 {master_device_id}"))
}

fn native_layout_section_status(state: &DesktopState) -> (String, NativeStatusTone) {
    if state.master_error.is_some() {
        return ("同步通道 需处理".to_string(), NativeStatusTone::Danger);
    }
    if let Some((label, tone)) = sync_runtime_status_label(state) {
        return (format!("同步通道 {label}"), tone);
    }
    (
        format!("同步通道 {}", sync_channel_state_label(&state.master_state)),
        sync_channel_state_tone(&state.master_state),
    )
}

fn sync_runtime_status_label(state: &DesktopState) -> Option<(String, NativeStatusTone)> {
    match state.sync_runtime.state {
        kmsync_core::DesktopSyncRuntimeKind::Unknown => None,
        kmsync_core::DesktopSyncRuntimeKind::Idle => {
            Some(("等待设备".to_string(), NativeStatusTone::Muted))
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening
            if state.device.role == DesktopRole::Client
                && state.master_state != DesktopConnectionState::Disconnected =>
        {
            if state.sync_runtime.injected_events > 0 {
                Some((
                    format!("已注入 {}", state.sync_runtime.injected_events),
                    NativeStatusTone::Warning,
                ))
            } else if state.sync_runtime.received_events > 0 {
                Some((
                    format!("已接收 {}", state.sync_runtime.received_events),
                    NativeStatusTone::Warning,
                ))
            } else {
                Some(("等待输入".to_string(), NativeStatusTone::Warning))
            }
        }
        kmsync_core::DesktopSyncRuntimeKind::Listening => None,
        kmsync_core::DesktopSyncRuntimeKind::Armed => {
            if state.sync_runtime.sent_events > 0 {
                Some((
                    format!("已转发 {}", state.sync_runtime.sent_events),
                    NativeStatusTone::Warning,
                ))
            } else {
                Some(("捕获中".to_string(), NativeStatusTone::Warning))
            }
        }
        kmsync_core::DesktopSyncRuntimeKind::Failed => {
            Some(("需处理".to_string(), NativeStatusTone::Danger))
        }
    }
}

#[cfg(test)]
fn native_master_assignment_text(state: &DesktopState) -> String {
    let Some(master_device_id) = state.master_device_id.as_deref() else {
        return "当前未配置主电脑".to_string();
    };
    if state.device.id.as_deref() == Some(master_device_id) {
        return "本机是主电脑".to_string();
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
    slot.status_label
}

fn native_layout_slot_view(
    state: &DesktopState,
    selected_device_id: Option<&str>,
) -> NativeLayoutSlotView {
    let Some(selected_device_id) = selected_device_id else {
        return NativeLayoutSlotView {
            device_name: "未配置".to_string(),
            status_label: "未配置".to_string(),
            route_hint: "该边缘保持本机控制".to_string(),
            tone: NativeStatusTone::Muted,
        };
    };
    if state.device.id.as_deref() == Some(selected_device_id) {
        return NativeLayoutSlotView {
            device_name: state.device.name.clone(),
            status_label: connection_state_label(&state.master_state).to_string(),
            route_hint: format!("{}，本机", state.device.name),
            tone: connection_state_tone(&state.master_state),
        };
    }
    match state
        .devices
        .iter()
        .find(|device| device.id == selected_device_id)
    {
        Some(device) if device.online => NativeLayoutSlotView {
            device_name: device.name.clone(),
            status_label: "在线".to_string(),
            route_hint: "已加入布局，等待同步通道".to_string(),
            tone: NativeStatusTone::Info,
        },
        Some(device) => NativeLayoutSlotView {
            device_name: device.name.clone(),
            status_label: "离线".to_string(),
            route_hint: "从电脑离线".to_string(),
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
        id: state.device.id.clone(),
        name: native_current_device_name(state, edited_device_name),
        detail: format!("{}，本机", state.device.os),
        status: "本机".to_string(),
        status_tone: NativeStatusTone::Info,
        lan_ips: state.network.lan_ips.clone(),
        can_delete: false,
    }];
    rows.extend(state.devices.iter().map(|device| NativeDeviceListRow {
        id: Some(device.id.clone()),
        name: device.name.clone(),
        detail: native_device_row_detail(&state.layout, device),
        status: if device.online { "在线" } else { "离线" }.to_string(),
        status_tone: if device.online {
            NativeStatusTone::Success
        } else {
            NativeStatusTone::Muted
        },
        lan_ips: device.lan_ips.clone(),
        can_delete: true,
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
                            "本机"
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
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(device_name)
                        .strong()
                        .color(native_primary_text()),
                )
                .truncate(),
            );
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new("鼠标键盘从这里出发").color(native_muted_text()),
                )
                .truncate(),
            );
        },
    );
}

fn native_layout_combo_width(available_width: f32) -> f32 {
    if native_grid_total_width(
        NATIVE_LAYOUT_GRID_MAX_COL_WIDTH,
        3,
        native_layout_grid_horizontal_spacing(),
    ) <= available_width
    {
        NATIVE_LAYOUT_GRID_MAX_COL_WIDTH
    } else {
        native_fit_grid_column_width(
            available_width,
            3,
            native_layout_grid_horizontal_spacing(),
            NATIVE_LAYOUT_GRID_MIN_COL_WIDTH,
        )
        .min(NATIVE_LAYOUT_GRID_MAX_COL_WIDTH)
    }
}

fn native_layout_grid_leading_space(available_width: f32, column_width: f32) -> f32 {
    ((available_width
        - native_grid_total_width(column_width, 3, native_layout_grid_horizontal_spacing()))
        / 2.0)
        .max(0.0)
}

fn native_top_panel_widths(available_width: f32) -> NativeSplitPanelWidths {
    let content_width = (available_width - NATIVE_PANEL_SPACING).max(1.0);
    let primary = native_design_split_primary(content_width, 640.0, 700.0);
    NativeSplitPanelWidths {
        primary,
        secondary: (content_width - primary).max(1.0),
    }
}

fn native_lower_panel_widths(available_width: f32) -> NativeSplitPanelWidths {
    let content_width = (available_width - NATIVE_PANEL_SPACING).max(1.0);
    let primary = (content_width / 2.0).max(1.0);
    NativeSplitPanelWidths {
        primary,
        secondary: (content_width - primary).max(1.0),
    }
}

fn native_design_split_primary(
    content_width: f32,
    design_primary: f32,
    design_secondary: f32,
) -> f32 {
    let design_total = (design_primary + design_secondary).max(1.0);
    (content_width * design_primary / design_total).max(1.0)
}

fn native_server_form_widths(available_width: f32) -> NativeServerFormWidths {
    let first_gap = 10.0;
    let min_pre_button_gap = 12.0;
    let button = NATIVE_ACTION_BUTTON_WIDTH;
    let port = if available_width >= 390.0 { 82.0 } else { 64.0 };
    let field_budget = (available_width - button - first_gap - min_pre_button_gap).max(1.0);
    let host = (field_budget - port).clamp(125.0, 185.0);
    let used = host + first_gap + port + button;
    NativeServerFormWidths {
        host,
        port,
        pre_button_gap: (available_width - used).max(min_pre_button_gap),
    }
}

fn native_current_device_form_widths(available_width: f32) -> NativeCurrentDeviceFormWidths {
    let first_gap = 10.0;
    let min_pre_button_gap = 12.0;
    let button = NATIVE_ACTION_BUTTON_WIDTH;
    let id = if available_width >= 430.0 {
        124.0
    } else {
        98.0
    };
    let field_budget = (available_width - button - first_gap - min_pre_button_gap).max(1.0);
    let name = (field_budget - id).clamp(140.0, 180.0);
    let used = name + first_gap + id + button;
    NativeCurrentDeviceFormWidths {
        name,
        id,
        pre_button_gap: (available_width - used).max(min_pre_button_gap),
    }
}

fn native_current_device_metric_widths(available_width: f32) -> NativeCurrentDeviceMetricWidths {
    let min_pre_refresh_gap = 12.0;
    let metric_gap = 10.0;
    let refresh = NATIVE_COMPACT_ACTION_BUTTON_WIDTH;
    let server = if available_width >= 430.0 {
        108.0
    } else {
        96.0
    };
    let lan_ip =
        (available_width - server - metric_gap - refresh - min_pre_refresh_gap).clamp(130.0, 230.0);
    let used = server + metric_gap + lan_ip + refresh;
    NativeCurrentDeviceMetricWidths {
        server,
        metric_gap,
        lan_ip,
        pre_refresh_gap: (available_width - used).max(min_pre_refresh_gap),
    }
}

fn native_device_list_scroll_height(available_height: f32) -> f32 {
    (available_height - 46.0).clamp(150.0, 272.0)
}

fn native_header_total_height() -> f32 {
    NATIVE_HEADER_TOTAL_HEIGHT
}

fn native_header_content_height() -> f32 {
    NATIVE_HEADER_CONTENT_HEIGHT
}

fn native_logo_icon_data() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/kmsync-logo.png")).ok()
}

fn native_logo_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let icon = native_logo_icon_data()?;
    let image = egui::ColorImage::from(&icon);
    Some(ctx.load_texture("kmsync-logo", image, egui::TextureOptions::LINEAR))
}

fn native_header_logo_corner_radius() -> u8 {
    NATIVE_HEADER_LOGO_CORNER_RADIUS
}

fn native_header_logo_background() -> egui::Color32 {
    egui::Color32::from_rgb(238, 244, 255)
}

fn native_header_status_labels(_state: &DesktopState) -> Vec<(String, NativeStatusTone)> {
    vec![
        (
            native_top_status_labels()[0].to_string(),
            NativeStatusTone::Info,
        ),
        (
            native_top_status_labels()[1].to_string(),
            NativeStatusTone::Muted,
        ),
    ]
}

fn native_header_status_row_width(labels: &[(String, NativeStatusTone)]) -> f32 {
    let chips_width = labels
        .iter()
        .map(|(label, _)| native_status_chip_size(label).x)
        .sum::<f32>();
    chips_width + NATIVE_HEADER_STATUS_GAP * labels.len().saturating_sub(1) as f32
}

fn native_header_status_row(ui: &mut egui::Ui, labels: &[(String, NativeStatusTone)]) {
    for (index, (label, tone)) in labels.iter().enumerate() {
        if index > 0 {
            ui.add_space(NATIVE_HEADER_STATUS_GAP);
        }
        native_status_chip(ui, label, *tone);
    }
}

fn native_status_chip_size(label: &str) -> egui::Vec2 {
    let text_width = label
        .chars()
        .map(|ch| if ch.is_ascii() { 7.0 } else { 14.0 })
        .sum::<f32>();
    egui::vec2(
        (text_width + 18.0).clamp(48.0, 142.0),
        NATIVE_STATUS_CHIP_HEIGHT,
    )
}

fn native_compact_action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        egui::vec2(
            NATIVE_COMPACT_ACTION_BUTTON_WIDTH,
            NATIVE_ACTION_BUTTON_HEIGHT,
        ),
        egui::Button::new(
            egui::RichText::new(label)
                .size(12.0)
                .strong()
                .color(egui::Color32::from_rgb(30, 64, 175)),
        )
        .truncate()
        .fill(egui::Color32::from_rgb(239, 246, 255))
        .corner_radius(egui::CornerRadius::same(7))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(147, 197, 253),
        )),
    )
}

#[cfg(test)]
fn native_compact_action_button_size() -> egui::Vec2 {
    egui::vec2(
        NATIVE_COMPACT_ACTION_BUTTON_WIDTH,
        NATIVE_ACTION_BUTTON_HEIGHT,
    )
}

#[cfg(test)]
fn native_page_content_width() -> f32 {
    NATIVE_WINDOW_SIZE[0] - f32::from(NATIVE_PAGE_MARGIN_X) * 2.0
}

#[cfg(test)]
fn native_default_top_panel_widths() -> NativeSplitPanelWidths {
    native_top_panel_widths(native_page_content_width())
}

#[cfg(test)]
fn native_default_lower_panel_widths() -> NativeSplitPanelWidths {
    native_lower_panel_widths(native_page_content_width())
}

#[cfg(test)]
fn native_default_lower_panel_content_widths() -> NativeSplitPanelWidths {
    let lower = native_default_lower_panel_widths();
    NativeSplitPanelWidths {
        primary: (lower.primary - 30.0).max(1.0),
        secondary: (lower.secondary - 30.0).max(1.0),
    }
}

#[cfg(test)]
fn native_default_layout_canvas_content_width() -> f32 {
    let lower_content = native_default_lower_panel_content_widths();
    (lower_content.primary - 34.0).max(1.0)
}

#[cfg(test)]
fn native_default_layout_grid_leading_space() -> f32 {
    let canvas_content = native_default_layout_canvas_content_width();
    let combo_width = native_layout_combo_width(canvas_content);
    native_layout_grid_leading_space(canvas_content, combo_width)
}

#[cfg(test)]
fn native_default_server_form_widths() -> NativeServerFormWidths {
    let top = native_default_top_panel_widths();
    native_server_form_widths((top.primary - 30.0).max(1.0))
}

#[cfg(test)]
fn native_default_current_device_form_widths() -> NativeCurrentDeviceFormWidths {
    let top = native_default_top_panel_widths();
    native_current_device_form_widths((top.secondary - 30.0).max(1.0))
}

#[cfg(test)]
fn native_default_current_device_metric_widths() -> NativeCurrentDeviceMetricWidths {
    let top = native_default_top_panel_widths();
    native_current_device_metric_widths((top.secondary - 30.0).max(1.0))
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
                .size(12.0)
                .strong()
                .color(egui::Color32::from_rgb(30, 64, 175)),
        )
        .truncate()
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
                .size(12.0)
                .strong()
                .color(egui::Color32::from_rgb(246, 249, 255)),
        )
        .truncate()
        .fill(egui::Color32::from_rgb(35, 88, 184))
        .corner_radius(egui::CornerRadius::same(7))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(35, 88, 184))),
    )
}

fn native_action_button_size() -> egui::Vec2 {
    egui::vec2(NATIVE_ACTION_BUTTON_WIDTH, NATIVE_ACTION_BUTTON_HEIGHT)
}

#[cfg(test)]
fn native_action_button_labels() -> [&'static str; 6] {
    [
        "保存服务器配置",
        "刷新",
        "保存本机配置",
        "申请权限",
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

fn native_current_device_fact_labels() -> [&'static str; 2] {
    ["服务器", "内网 IP"]
}

fn native_top_status_labels() -> [&'static str; 2] {
    ["LAN 优先", "自动刷新"]
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

fn sync_channel_state_label(state: &DesktopConnectionState) -> &'static str {
    match state {
        DesktopConnectionState::Connected | DesktopConnectionState::Connecting => "连接中",
        DesktopConnectionState::Disconnected => "未连接",
        DesktopConnectionState::Retrying => "正在重试",
        DesktopConnectionState::SelfDevice => "本机",
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

fn sync_channel_state_tone(state: &DesktopConnectionState) -> NativeStatusTone {
    match state {
        DesktopConnectionState::Connected | DesktopConnectionState::Connecting => {
            NativeStatusTone::Warning
        }
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

        assert_eq!(metrics.window_size, [1000.0, 700.0]);
        assert_eq!(metrics.min_window_size, metrics.window_size);
        assert_eq!(metrics.top_panel_min_height, 158.0);
        assert_eq!(metrics.lower_panel_columns, 2);
        assert_eq!(
            metrics.layout_panel_min_height,
            metrics.devices_panel_min_height
        );
        assert_eq!(metrics.layout_panel_min_height, 384.0);
        assert!(metrics.devices_grid_min_col_width <= 68.0);
        assert!(!native_persist_window_size());
    }

    #[test]
    fn native_lower_grids_fit_inside_two_column_min_window() {
        let lower_column_content_width = 267.0;

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
        assert_eq!(native_layout_combo_width(533.0), 154.0);
    }

    #[test]
    fn native_redesign_panel_widths_prioritize_topology_canvas() {
        let lower = native_lower_panel_widths(native_page_content_width());

        assert_eq!(lower.primary.round(), 465.0);
        assert_eq!(lower.secondary.round(), 465.0);
        assert!(lower.total_width(NATIVE_PANEL_SPACING) <= native_page_content_width());

        let default_top = native_default_top_panel_widths();
        assert_eq!(default_top.primary.round(), 444.0);
        assert_eq!(default_top.secondary.round(), 486.0);
        assert_eq!(
            default_top.total_width(NATIVE_PANEL_SPACING).round(),
            native_page_content_width()
        );

        let default_lower = native_default_lower_panel_widths();
        assert_eq!(default_lower.primary.round(), 465.0);
        assert_eq!(default_lower.secondary.round(), 465.0);
        assert_eq!(
            default_lower.total_width(NATIVE_PANEL_SPACING).round(),
            native_page_content_width()
        );
    }

    #[test]
    fn native_default_design_grid_aligns_buttons_and_topology() {
        assert_eq!(native_page_content_width(), 944.0);

        let server = native_default_server_form_widths();
        assert_eq!(server.host, 185.0);
        assert_eq!(server.port, 82.0);
        assert_eq!(server.pre_button_gap.round(), 32.0);

        let current = native_default_current_device_form_widths();
        assert_eq!(current.name, 180.0);
        assert_eq!(current.id, 124.0);
        assert_eq!(current.pre_button_gap.round(), 37.0);

        let metrics = native_default_current_device_metric_widths();
        assert_eq!(metrics.server, 108.0);
        assert_eq!(metrics.metric_gap, 10.0);
        assert_eq!(metrics.lan_ip, 230.0);
        assert_eq!(metrics.pre_refresh_gap.round(), 24.0);

        assert_eq!(native_default_layout_canvas_content_width().round(), 401.0);
        assert_eq!(native_default_layout_grid_leading_space().round(), 0.0);
        assert_eq!(NATIVE_LAYOUT_CENTER_HEIGHT, NATIVE_LAYOUT_SLOT_HEIGHT);
        assert_eq!(native_compact_action_button_size(), egui::vec2(84.0, 28.0));
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
            native_current_device_fact_labels().as_slice(),
            ["服务器", "内网 IP"]
        );
    }

    #[test]
    fn native_header_status_labels_do_not_mix_connection_states() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::Connected,
            ..DesktopState::default()
        };
        let labels = native_header_status_labels(&state);

        assert_eq!(native_top_status_labels(), ["LAN 优先", "自动刷新"]);
        assert_eq!(
            labels,
            vec![
                ("LAN 优先".to_string(), NativeStatusTone::Info),
                ("自动刷新".to_string(), NativeStatusTone::Muted)
            ]
        );
    }

    #[test]
    fn native_layout_section_status_reports_sync_channel() {
        let state = DesktopState {
            master_state: DesktopConnectionState::Connected,
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 连接中".to_string(), NativeStatusTone::Warning)
        );
    }

    #[test]
    fn native_layout_section_status_surfaces_sync_errors() {
        let state = DesktopState {
            master_state: DesktopConnectionState::SelfDevice,
            master_error: Some("缺少 macOS Input Monitoring 权限".to_string()),
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 需处理".to_string(), NativeStatusTone::Danger)
        );
    }

    #[test]
    fn native_layout_section_status_reports_runtime_capture_state() {
        let state = DesktopState {
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

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 捕获中".to_string(), NativeStatusTone::Warning)
        );
    }

    #[test]
    fn native_layout_section_status_reports_transmit_progress() {
        let state = DesktopState {
            master_state: DesktopConnectionState::SelfDevice,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                sent_events: 3,
                last_sent_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 已转发 3".to_string(), NativeStatusTone::Warning)
        );
    }

    #[test]
    fn native_layout_section_status_reports_client_listener_without_claiming_connected() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 等待输入".to_string(), NativeStatusTone::Warning)
        );
    }

    #[test]
    fn native_layout_section_status_reports_receive_progress() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                received_events: 4,
                last_received_at: Some(456),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 已接收 4".to_string(), NativeStatusTone::Warning)
        );
    }

    #[test]
    fn native_layout_section_status_reports_injection_progress() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            master_state: DesktopConnectionState::Connecting,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                received_events: 4,
                last_received_at: Some(456),
                injected_events: 2,
                last_injected_at: Some(789),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 已注入 2".to_string(), NativeStatusTone::Warning)
        );
    }

    #[test]
    fn native_layout_section_status_keeps_client_disconnected_when_master_is_offline() {
        let state = DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            master_state: DesktopConnectionState::Disconnected,
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Listening,
                error: None,
                targets: Vec::new(),
                updated_at: Some(123),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_section_status(&state),
            ("同步通道 未连接".to_string(), NativeStatusTone::Muted)
        );
    }

    #[test]
    fn native_desktop_shows_permission_button_without_auto_prompting() {
        let state = DesktopState {
            permissions: vec![DesktopPermissionState {
                key: "macos.input_monitoring".to_string(),
                status: "missing".to_string(),
                label: "macOS Input Monitoring".to_string(),
                guidance: None,
            }],
            ..DesktopState::default()
        };

        assert!(native_should_show_permission_request_button(&state));
        assert!(!native_should_auto_request_platform_permissions(&state));
    }

    #[test]
    fn native_desktop_hides_permission_button_when_granted() {
        let state = DesktopState {
            permissions: vec![DesktopPermissionState {
                key: "macos.input_monitoring".to_string(),
                status: "granted".to_string(),
                label: "macOS Input Monitoring".to_string(),
                guidance: None,
            }],
            ..DesktopState::default()
        };

        assert!(!native_should_show_permission_request_button(&state));
        assert!(!native_should_auto_request_platform_permissions(&state));
    }

    #[test]
    fn native_header_uses_fixed_height_status_chips_and_logo() {
        let state = DesktopState {
            server_state: DesktopConnectionState::Connected,
            master_state: DesktopConnectionState::SelfDevice,
            ..DesktopState::default()
        };
        let labels = native_header_status_labels(&state);

        assert_eq!(native_header_total_height(), 62.0);
        assert_eq!(native_header_content_height(), 42.0);
        assert_eq!(labels[0].0, "LAN 优先");
        assert_eq!(labels[1].0, "自动刷新");
        assert_eq!(native_status_chip_size("LAN 优先").y, 26.0);
        assert!(native_header_status_row_width(&labels) < 430.0);
        assert!(native_logo_icon_data().is_some());
        assert_eq!(native_header_logo_corner_radius(), 10);
        assert_ne!(native_header_logo_background(), native_panel_background());
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
            "在线"
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
                device_name: "未配置".to_string(),
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
                route_hint: "已加入布局，等待同步通道".to_string(),
                tone: NativeStatusTone::Info,
            }
        );
        assert_eq!(
            native_layout_slot_view(&state, Some("bottom-device")),
            NativeLayoutSlotView {
                device_name: "Bottom PC".to_string(),
                status_label: "离线".to_string(),
                route_hint: "从电脑离线".to_string(),
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
        assert!(!native_layout_peer_status_has_border());
    }

    #[test]
    fn native_client_topology_shows_master_center_and_current_device_slot() {
        let state = DesktopState {
            device: DesktopDeviceState {
                id: Some("client-device".to_string()),
                name: "Client PC".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: DesktopRole::Client,
            },
            master_device_id: Some("master-device".to_string()),
            master_state: DesktopConnectionState::Connected,
            layout: DesktopLayout {
                right: Some("client-device".to_string()),
                ..DesktopLayout::default()
            },
            devices: vec![DesktopPeerState {
                id: "master-device".to_string(),
                name: "Master PC".to_string(),
                os: "macos".to_string(),
                online: true,
                lan_ips: vec!["192.168.1.10".to_string()],
                public_ip: None,
                listen_port: Some(24_800),
                last_seen_at: None,
            }],
            ..DesktopState::default()
        };

        assert_eq!(
            native_layout_center_device_name(&state, "Client Edited"),
            "Master PC"
        );

        let right_view = native_layout_slot_view(&state, state.layout.right.as_deref());
        assert_eq!(
            right_view.status_label,
            connection_state_label(&DesktopConnectionState::Connected)
        );
        assert!(right_view.route_hint.contains("Client PC"));
        assert_eq!(right_view.tone, NativeStatusTone::Success);
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
                "保存本机配置",
                "申请权限",
                "保存设备位置",
                "刷新"
            ]
        );
        assert_eq!(native_action_button_size(), egui::vec2(105.0, 28.0));
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
                    id: Some("current".to_string()),
                    name: "Renamed PC".to_string(),
                    detail: "windows，本机".to_string(),
                    status: "本机".to_string(),
                    status_tone: NativeStatusTone::Info,
                    lan_ips: vec!["192.168.1.20".to_string()],
                    can_delete: false,
                },
                NativeDeviceListRow {
                    id: Some("right-device".to_string()),
                    name: "Right PC".to_string(),
                    detail: "macos".to_string(),
                    status: "在线".to_string(),
                    status_tone: NativeStatusTone::Success,
                    lan_ips: vec!["192.168.1.21".to_string()],
                    can_delete: true,
                }
            ]
        );
        assert_eq!(
            native_current_device_name(&state, "Renamed PC"),
            "Renamed PC"
        );
    }

    #[test]
    fn native_device_rows_reserve_full_ip_and_delete_button_width() {
        assert!(native_device_row_ip_width() >= 112.0);
        assert_eq!(native_delete_device_button_size(), egui::vec2(42.0, 24.0));
        assert_eq!(native_device_list_scroll_height(400.0), 272.0);
        assert_eq!(native_device_list_scroll_height(160.0), 150.0);
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
