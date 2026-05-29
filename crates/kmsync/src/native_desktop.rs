use std::path::{Path, PathBuf};

use eframe::egui;
use kmsync_core::{DesktopConnectionState, DesktopLayout, DesktopRole, DesktopState};

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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("KMSync")
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([760.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("KMSync", options, Box::new(move |_cc| Ok(Box::new(app))))
        .map_err(|error| format!("native desktop window failed: {error}"))
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
                ui.label(format!(
                    "服务器：{}",
                    connection_state_label(&self.state.server_state)
                ));
                ui.separator();
                ui.label(format!(
                    "主电脑：{}",
                    connection_state_label(&self.state.master_state)
                ));
                ui.separator();
                ui.label(format!("状态：{}", self.status_message));
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.columns(2, |columns| {
                    columns[0].group(|ui| self.network_section(ui));
                    columns[1].group(|ui| self.current_device_section(ui));
                });
                ui.add_space(12.0);
                ui.group(|ui| self.layout_section(ui));
                ui.add_space(12.0);
                ui.group(|ui| self.devices_section(ui));
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
        if ui.button("保存服务器配置").clicked() {
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
        if ui.button("保存当前电脑配置").clicked() {
            self.save_role();
        }
        if ui.button("刷新状态").clicked() {
            self.reload_state();
            self.status_message = "状态已刷新".to_string();
        }
    }

    fn layout_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备位置");
        let devices = self
            .state
            .devices
            .iter()
            .map(|device| (device.id.clone(), device.name.clone()))
            .collect::<Vec<_>>();
        device_combo(ui, "上方电脑", &mut self.layout.top, &devices);
        device_combo(ui, "左边电脑", &mut self.layout.left, &devices);
        device_combo(ui, "右边电脑", &mut self.layout.right, &devices);
        device_combo(ui, "下方电脑", &mut self.layout.bottom, &devices);
        if ui.button("保存设备位置").clicked() {
            self.save_layout();
        }
    }

    fn devices_section(&self, ui: &mut egui::Ui) {
        ui.heading("设备列表");
        if self.state.devices.is_empty() {
            ui.label("暂无其他设备");
            return;
        }
        egui::Grid::new("native_devices_grid")
            .striped(true)
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
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, "未配置");
            for (id, name) in devices {
                ui.selectable_value(selected, Some(id.clone()), name);
            }
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
}
