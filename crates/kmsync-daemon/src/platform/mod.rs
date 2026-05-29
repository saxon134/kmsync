use std::thread;
use std::time::Duration;

use kmsync_core::{ClipboardText, InputEvent, OsKind};

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, Copy)]
pub struct PlatformCapabilities {
    pub input_capture: bool,
    pub input_injection: bool,
    pub clipboard_text: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    Granted,
    Missing,
    NotApplicable,
    Unknown,
}

impl PermissionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Missing => "missing",
            Self::NotApplicable => "not_applicable",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPermissionCheck {
    pub id: &'static str,
    pub label: &'static str,
    pub status: PermissionStatus,
    pub guidance: &'static str,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardBackendKind {
    NativeApi,
    Unavailable,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardWatchBackend {
    PlatformNotification,
    NativeChangeCounter,
    PollingFallback,
    Unavailable,
}

impl ClipboardWatchBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlatformNotification => "platform-notification",
            Self::NativeChangeCounter => "native-change-counter",
            Self::PollingFallback => "polling-fallback",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub struct RemotePointerState {
    current: Option<PointerPosition>,
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
impl RemotePointerState {
    pub fn set(&mut self, position: PointerPosition) {
        self.current = Some(position);
    }

    #[must_use]
    pub fn current(&self) -> Option<PointerPosition> {
        self.current
    }

    pub fn apply_delta(&mut self, dx: f32, dy: f32) -> Option<PointerPosition> {
        let mut position = self.current?;
        position.x += f64::from(dx);
        position.y += f64::from(dy);
        self.current = Some(position);
        Some(position)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayLayout {
    displays: Vec<DisplayBounds>,
}

impl DisplayLayout {
    #[must_use]
    pub fn new(displays: Vec<DisplayBounds>) -> Self {
        Self { displays }
    }

    #[must_use]
    pub fn from_primary(primary: Option<DisplayBounds>) -> Self {
        Self::new(primary.into_iter().collect())
    }

    #[must_use]
    pub fn virtual_bounds(&self) -> Option<DisplayBounds> {
        let first = *self.displays.first()?;
        let mut left = first.x;
        let mut top = first.y;
        let mut right = first.x + first.width;
        let mut bottom = first.y + first.height;

        for display in &self.displays[1..] {
            left = left.min(display.x);
            top = top.min(display.y);
            right = right.max(display.x + display.width);
            bottom = bottom.max(display.y + display.height);
        }

        Some(DisplayBounds {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CapturedInput {
    pub event: InputEvent,
    pub pointer: Option<PointerPosition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureDecision {
    Continue,
    Suppress,
}

pub trait PlatformAdapter: InputInjector + ClipboardBackend + InputCaptureBackend {
    fn os_kind(&self) -> OsKind;
    fn capabilities(&self) -> PlatformCapabilities;
    fn permission_checks(&self) -> Vec<PlatformPermissionCheck>;
    fn permission_hints(&self) -> &'static [&'static str];
    fn primary_display_bounds(&self) -> Option<DisplayBounds>;
    fn active_application_id(&self) -> Option<String> {
        None
    }

    fn display_layout(&self) -> DisplayLayout {
        DisplayLayout::from_primary(self.primary_display_bounds())
    }
}

pub trait InputInjector {
    fn inject(&mut self, event: InputEvent) -> Result<(), String>;
}

pub trait InputCaptureBackend {
    fn capture_loop<F>(&mut self, callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static;
}

pub trait ClipboardBackend {
    fn get_clipboard_text(&self) -> Result<String, String>;
    fn set_clipboard_text(&mut self, text: &str) -> Result<(), String>;
    fn clipboard_watch_backend(&self) -> ClipboardWatchBackend {
        ClipboardWatchBackend::PollingFallback
    }

    fn get_clipboard_content(&self) -> Result<ClipboardText, String> {
        self.get_clipboard_text()
            .map(|text| ClipboardText::from_local_text(0, 0, text))
    }

    fn set_clipboard_content(&mut self, clipboard: &ClipboardText) -> Result<(), String> {
        self.set_clipboard_text(&clipboard.text)
    }

    fn wait_for_clipboard_change(
        &self,
        previous: &ClipboardText,
        fallback_interval: Duration,
    ) -> Result<ClipboardText, String> {
        let sleep_for = if fallback_interval.is_zero() {
            Duration::from_millis(1)
        } else {
            fallback_interval
        };
        loop {
            let content = self.get_clipboard_content()?;
            if &content != previous {
                return Ok(content);
            }
            thread::sleep(sleep_for);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn current_platform() -> macos::MacOsPlatform {
    macos::MacOsPlatform::new()
}

#[cfg(target_os = "windows")]
pub fn current_platform() -> windows::WindowsPlatform {
    windows::WindowsPlatform::new()
}

#[cfg(target_os = "linux")]
pub fn current_platform() -> linux::LinuxPlatform {
    linux::LinuxPlatform::new()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn current_platform() -> unsupported::UnsupportedPlatform {
    unsupported::UnsupportedPlatform::new()
}

pub fn active_application_id() -> Option<String> {
    current_platform().active_application_id()
}

pub fn hide_local_pointer() {
    #[cfg(target_os = "macos")]
    macos::hide_local_pointer();
    #[cfg(target_os = "windows")]
    windows::hide_local_pointer();
    #[cfg(target_os = "linux")]
    linux::hide_local_pointer();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    unsupported::hide_local_pointer();
}

pub fn restore_local_pointer(position: Option<PointerPosition>) {
    #[cfg(target_os = "macos")]
    macos::restore_local_pointer(position);
    #[cfg(target_os = "windows")]
    windows::restore_local_pointer(position);
    #[cfg(target_os = "linux")]
    linux::restore_local_pointer(position);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    unsupported::restore_local_pointer(position);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_pointer_state_applies_mouse_move_without_reading_current_position() {
        let mut state = RemotePointerState::default();

        assert_eq!(state.apply_delta(1.0, 1.0), None);

        state.set(PointerPosition { x: 10.0, y: 20.0 });

        assert_eq!(
            state.apply_delta(3.5, -2.0),
            Some(PointerPosition { x: 13.5, y: 18.0 })
        );
        assert_eq!(state.current(), Some(PointerPosition { x: 13.5, y: 18.0 }));
    }

    #[test]
    fn linux_platform_detects_display_server_and_degrades_cleanly() {
        assert_eq!(
            linux::LinuxDisplayServer::detect(Some("wayland-0"), Some(":0"), Some("x11")),
            linux::LinuxDisplayServer::Wayland
        );
        assert_eq!(
            linux::LinuxDisplayServer::detect(None, Some(":0"), None),
            linux::LinuxDisplayServer::X11
        );
        assert_eq!(
            linux::LinuxDisplayServer::detect(None, None, None),
            linux::LinuxDisplayServer::Unknown
        );

        let _detected_platform = linux::LinuxPlatform::new();
        let platform = linux::LinuxPlatform::for_display_server(linux::LinuxDisplayServer::X11);

        assert_eq!(platform.os_kind(), OsKind::Linux);
        assert_eq!(platform.display_server(), linux::LinuxDisplayServer::X11);
        assert!(platform.capabilities().input_capture);
        assert!(platform.capabilities().input_injection);
        let checks = platform.permission_checks();
        assert!(checks.iter().any(|check| {
            check.id == "linux.x11.capture_backend"
                && check.status == PermissionStatus::Unknown
                && check.guidance.contains("XInput2")
        }));
        assert!(checks.iter().any(|check| {
            check.id == "linux.x11.xtest_extension"
                && check.status == PermissionStatus::Unknown
                && check.guidance.contains("XTEST")
        }));
        assert!(platform
            .permission_hints()
            .iter()
            .any(|hint| hint.contains("XInput2")));
    }

    #[test]
    fn linux_wayland_reports_structured_capability_downgrade() {
        let mut platform =
            linux::LinuxPlatform::for_display_server(linux::LinuxDisplayServer::Wayland);
        let status = platform.backend_status();

        assert_eq!(status.display_server, linux::LinuxDisplayServer::Wayland);
        assert_eq!(
            status.input_capture,
            linux::LinuxCapabilityStatus::Unavailable("wayland_global_capture_blocked")
        );
        assert_eq!(
            status.input_injection,
            linux::LinuxCapabilityStatus::Unavailable("wayland_input_injection_blocked")
        );
        assert_eq!(
            status.clipboard_text,
            linux::LinuxCapabilityStatus::Unavailable("wayland_clipboard_portal_not_wired")
        );
        assert!(!platform.capabilities().input_capture);
        let checks = platform.permission_checks();
        assert!(checks.iter().any(|check| {
            check.id == "linux.wayland.capture_permission"
                && check.status == PermissionStatus::Missing
                && check.guidance.contains("blocked")
        }));
        assert!(platform
            .permission_hints()
            .iter()
            .any(|hint| hint.contains("xdg-desktop-portal")));

        let error = platform
            .inject(InputEvent::Scroll(kmsync_core::ScrollEvent {
                dx: 0.0,
                dy: 1.0,
            }))
            .expect_err("Wayland injection should degrade");
        assert!(error.contains("wayland_input_injection_blocked"));
    }

    #[test]
    fn linux_x11_reports_xtest_injection_backend() {
        let platform = linux::LinuxPlatform::for_display_server(linux::LinuxDisplayServer::X11);
        let status = platform.backend_status();

        assert_eq!(status.display_server, linux::LinuxDisplayServer::X11);
        assert_eq!(
            status.input_capture,
            linux::LinuxCapabilityStatus::Available("x11_xinput2")
        );
        assert_eq!(
            status.input_injection,
            linux::LinuxCapabilityStatus::Available("x11_xtest")
        );
        assert!(platform.capabilities().input_capture);
        assert!(platform.capabilities().input_injection);
        assert!(platform
            .permission_hints()
            .iter()
            .any(|hint| hint.contains("XTest")));
    }

    #[test]
    fn linux_x11_xtest_maps_input_events_without_heap_context() {
        assert_eq!(linux::x11_evdev_keycode(kmsync_core::Key::A), Some(38));
        assert_eq!(
            linux::x11_evdev_keycode(kmsync_core::Key::RightControl),
            Some(105)
        );
        assert_eq!(
            linux::x11_mouse_button_detail(kmsync_core::MouseButton::Left),
            1
        );
        assert_eq!(
            linux::x11_mouse_button_detail(kmsync_core::MouseButton::Back),
            8
        );
        assert_eq!(
            linux::x11_scroll_button_sequence(kmsync_core::ScrollEvent { dx: -2.0, dy: 1.0 }),
            [Some((4, 1)), Some((6, 2))]
        );
        assert_eq!(linux::x11_relative_motion(3.4, -2.6), (3, -3));
        assert_eq!(
            linux::x11_absolute_position(1.4, -0.5, 1920, 1080),
            (1919, 0)
        );
        assert_eq!(
            linux::x11_key_from_evdev_keycode(38),
            Some(kmsync_core::Key::A)
        );
        assert_eq!(
            linux::x11_button_event(4, kmsync_core::KeyState::Pressed),
            Some(InputEvent::Scroll(kmsync_core::ScrollEvent {
                dx: 0.0,
                dy: 1.0
            }))
        );
        assert_eq!(
            linux::x11_button_event(4, kmsync_core::KeyState::Released),
            None
        );
        assert_eq!(
            linux::x11_raw_motion_delta(&[0b11], &[1.0, -2.5]),
            Some((1.0, -2.5))
        );
    }

    #[test]
    fn linux_desktop_compatibility_matrix_covers_common_desktops() {
        assert_eq!(
            linux::LinuxDesktopEnvironment::detect(Some("GNOME"), Some("gnome")),
            linux::LinuxDesktopEnvironment::Gnome
        );
        assert_eq!(
            linux::LinuxDesktopEnvironment::detect(Some("KDE"), Some("plasma")),
            linux::LinuxDesktopEnvironment::KdePlasma
        );
        assert_eq!(
            linux::LinuxDesktopEnvironment::detect(Some("sway"), Some("sway")),
            linux::LinuxDesktopEnvironment::Wlroots
        );
        assert_eq!(
            linux::LinuxDesktopEnvironment::detect(Some("XFCE"), Some("xfce")),
            linux::LinuxDesktopEnvironment::Xfce
        );

        let matrix = linux::linux_compatibility_matrix();
        assert!(matrix.iter().any(|row| {
            row.desktop_environment == linux::LinuxDesktopEnvironment::Gnome
                && row.display_server == linux::LinuxDisplayServer::Wayland
                && row.input_capture
                    == linux::LinuxCapabilityStatus::Unavailable("wayland_global_capture_blocked")
                && row.input_injection
                    == linux::LinuxCapabilityStatus::Unavailable("wayland_input_injection_blocked")
                && row.notes.contains("portal")
        }));
        assert!(matrix.iter().any(|row| {
            row.desktop_environment == linux::LinuxDesktopEnvironment::Xfce
                && row.display_server == linux::LinuxDisplayServer::X11
                && row.input_capture == linux::LinuxCapabilityStatus::Available("x11_xinput2")
                && row.input_injection == linux::LinuxCapabilityStatus::Available("x11_xtest")
        }));

        let platform = linux::LinuxPlatform::for_environment(
            linux::LinuxDisplayServer::Wayland,
            linux::LinuxDesktopEnvironment::Gnome,
        );
        assert_eq!(
            platform.compatibility().desktop_environment,
            linux::LinuxDesktopEnvironment::Gnome
        );
        assert!(platform
            .permission_hints()
            .iter()
            .any(|hint| hint.contains("GNOME")));
    }
}
