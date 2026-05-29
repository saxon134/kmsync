use std::env;
use std::time::Duration;

use kmsync_core::{InputEvent, Key, KeyState, MouseButton, MouseEvent, OsKind, ScrollEvent};

#[cfg(target_os = "linux")]
use kmsync_core::{KeyEvent, Modifiers};

#[cfg(target_os = "linux")]
use x11rb::{
    connection::Connection,
    protocol::{
        xinput::{self, ConnectionExt as XInputConnectionExt},
        xproto::{self, ConnectionExt as XProtoConnectionExt},
        xtest::ConnectionExt as XTestConnectionExt,
        Event as X11Event,
    },
    rust_connection::RustConnection,
};

use super::{
    CaptureDecision, CapturedInput, ClipboardBackend, ClipboardWatchBackend, DisplayBounds,
    InputCaptureBackend, InputInjector, PermissionStatus, PlatformAdapter, PlatformCapabilities,
    PlatformPermissionCheck,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDisplayServer {
    X11,
    Wayland,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDesktopEnvironment {
    Gnome,
    KdePlasma,
    Wlroots,
    Xfce,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxCapabilityStatus {
    Available(&'static str),
    Unavailable(&'static str),
}

impl LinuxCapabilityStatus {
    #[must_use]
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Available(_))
    }

    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Available(reason) | Self::Unavailable(reason) => reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxBackendStatus {
    pub display_server: LinuxDisplayServer,
    pub input_capture: LinuxCapabilityStatus,
    pub input_injection: LinuxCapabilityStatus,
    pub clipboard_text: LinuxCapabilityStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxCompatibilityRow {
    pub desktop_environment: LinuxDesktopEnvironment,
    pub display_server: LinuxDisplayServer,
    pub input_capture: LinuxCapabilityStatus,
    pub input_injection: LinuxCapabilityStatus,
    pub clipboard_text: LinuxCapabilityStatus,
    pub notes: &'static str,
}

impl LinuxDisplayServer {
    #[must_use]
    pub fn detect(
        wayland_display: Option<&str>,
        display: Option<&str>,
        xdg_session_type: Option<&str>,
    ) -> Self {
        if wayland_display.is_some_and(|value| !value.is_empty())
            || matches!(xdg_session_type, Some("wayland"))
        {
            return Self::Wayland;
        }

        if display.is_some_and(|value| !value.is_empty()) || matches!(xdg_session_type, Some("x11"))
        {
            return Self::X11;
        }

        Self::Unknown
    }
}

impl LinuxDesktopEnvironment {
    #[must_use]
    pub fn detect(xdg_current_desktop: Option<&str>, desktop_session: Option<&str>) -> Self {
        if has_desktop_token(xdg_current_desktop, &["GNOME"])
            || has_desktop_token(desktop_session, &["gnome"])
        {
            return Self::Gnome;
        }

        if has_desktop_token(xdg_current_desktop, &["KDE", "PLASMA"])
            || has_desktop_token(desktop_session, &["kde", "plasma"])
        {
            return Self::KdePlasma;
        }

        if has_desktop_token(
            xdg_current_desktop,
            &["SWAY", "WAYFIRE", "RIVER", "HYPRLAND", "LABWC"],
        ) || has_desktop_token(
            desktop_session,
            &["sway", "wayfire", "river", "hyprland", "labwc"],
        ) {
            return Self::Wlroots;
        }

        if has_desktop_token(xdg_current_desktop, &["XFCE", "XFCE4"])
            || has_desktop_token(desktop_session, &["xfce", "xfce4"])
        {
            return Self::Xfce;
        }

        Self::Unknown
    }
}

fn has_desktop_token(value: Option<&str>, needles: &[&str]) -> bool {
    value.is_some_and(|value| {
        value.split([':', ';', ',', ' ']).any(|part| {
            needles
                .iter()
                .any(|needle| part.eq_ignore_ascii_case(needle))
        })
    })
}

pub struct LinuxPlatform {
    display_server: LinuxDisplayServer,
    desktop_environment: LinuxDesktopEnvironment,
    #[cfg(target_os = "linux")]
    x11_injector: Option<X11XTestInjector>,
}

impl LinuxPlatform {
    #[must_use]
    pub fn new() -> Self {
        Self::for_environment(
            LinuxDisplayServer::detect(
                env::var("WAYLAND_DISPLAY").ok().as_deref(),
                env::var("DISPLAY").ok().as_deref(),
                env::var("XDG_SESSION_TYPE").ok().as_deref(),
            ),
            LinuxDesktopEnvironment::detect(
                env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
                env::var("DESKTOP_SESSION").ok().as_deref(),
            ),
        )
    }

    #[cfg(test)]
    #[must_use]
    pub const fn for_display_server(display_server: LinuxDisplayServer) -> Self {
        Self::for_environment(display_server, LinuxDesktopEnvironment::Unknown)
    }

    #[must_use]
    pub const fn for_environment(
        display_server: LinuxDisplayServer,
        desktop_environment: LinuxDesktopEnvironment,
    ) -> Self {
        Self {
            display_server,
            desktop_environment,
            #[cfg(target_os = "linux")]
            x11_injector: None,
        }
    }

    #[must_use]
    pub const fn display_server(&self) -> LinuxDisplayServer {
        self.display_server
    }

    #[must_use]
    pub const fn desktop_environment(&self) -> LinuxDesktopEnvironment {
        self.desktop_environment
    }

    #[must_use]
    pub fn compatibility(&self) -> LinuxCompatibilityRow {
        for row in linux_compatibility_matrix() {
            if row.display_server == self.display_server()
                && row.desktop_environment == self.desktop_environment()
            {
                return *row;
            }
        }
        compatibility_for(self.display_server(), self.desktop_environment())
    }

    #[must_use]
    pub fn backend_status(&self) -> LinuxBackendStatus {
        let compatibility = self.compatibility();
        LinuxBackendStatus {
            display_server: compatibility.display_server,
            input_capture: compatibility.input_capture,
            input_injection: compatibility.input_injection,
            clipboard_text: compatibility.clipboard_text,
        }
    }
}

const LINUX_COMPATIBILITY_MATRIX: [LinuxCompatibilityRow; 8] = [
    compatibility_for(LinuxDisplayServer::Wayland, LinuxDesktopEnvironment::Gnome),
    compatibility_for(
        LinuxDisplayServer::Wayland,
        LinuxDesktopEnvironment::KdePlasma,
    ),
    compatibility_for(
        LinuxDisplayServer::Wayland,
        LinuxDesktopEnvironment::Wlroots,
    ),
    compatibility_for(
        LinuxDisplayServer::Wayland,
        LinuxDesktopEnvironment::Unknown,
    ),
    compatibility_for(LinuxDisplayServer::X11, LinuxDesktopEnvironment::Gnome),
    compatibility_for(LinuxDisplayServer::X11, LinuxDesktopEnvironment::KdePlasma),
    compatibility_for(LinuxDisplayServer::X11, LinuxDesktopEnvironment::Xfce),
    compatibility_for(LinuxDisplayServer::X11, LinuxDesktopEnvironment::Unknown),
];

#[must_use]
pub const fn linux_compatibility_matrix() -> &'static [LinuxCompatibilityRow] {
    &LINUX_COMPATIBILITY_MATRIX
}

const fn compatibility_for(
    display_server: LinuxDisplayServer,
    desktop_environment: LinuxDesktopEnvironment,
) -> LinuxCompatibilityRow {
    match display_server {
        LinuxDisplayServer::X11 => LinuxCompatibilityRow {
            desktop_environment,
            display_server: LinuxDisplayServer::X11,
            input_capture: LinuxCapabilityStatus::Available("x11_xinput2"),
            input_injection: LinuxCapabilityStatus::Available("x11_xtest"),
            clipboard_text: LinuxCapabilityStatus::Unavailable("x11_clipboard_not_wired"),
            notes: x11_compatibility_notes(desktop_environment),
        },
        LinuxDisplayServer::Wayland => LinuxCompatibilityRow {
            desktop_environment,
            display_server: LinuxDisplayServer::Wayland,
            input_capture: LinuxCapabilityStatus::Unavailable("wayland_global_capture_blocked"),
            input_injection: LinuxCapabilityStatus::Unavailable("wayland_input_injection_blocked"),
            clipboard_text: LinuxCapabilityStatus::Unavailable("wayland_clipboard_portal_not_wired"),
            notes: wayland_compatibility_notes(desktop_environment),
        },
        LinuxDisplayServer::Unknown => LinuxCompatibilityRow {
            desktop_environment,
            display_server: LinuxDisplayServer::Unknown,
            input_capture: LinuxCapabilityStatus::Unavailable("linux_display_server_unknown"),
            input_injection: LinuxCapabilityStatus::Unavailable("linux_display_server_unknown"),
            clipboard_text: LinuxCapabilityStatus::Unavailable("linux_display_server_unknown"),
            notes: "Unknown Linux desktop/display server; detect XDG_CURRENT_DESKTOP, DESKTOP_SESSION, WAYLAND_DISPLAY, DISPLAY, and XDG_SESSION_TYPE before enabling backends.",
        },
    }
}

const fn x11_compatibility_notes(desktop_environment: LinuxDesktopEnvironment) -> &'static str {
    match desktop_environment {
        LinuxDesktopEnvironment::Gnome => {
            "GNOME on X11 can use XInput2 raw capture and XTest injection now; X selection clipboard support is still pending."
        }
        LinuxDesktopEnvironment::KdePlasma => {
            "KDE Plasma on X11 can use XInput2 raw capture and XTest injection now; X selection clipboard support is still pending."
        }
        LinuxDesktopEnvironment::Xfce => {
            "XFCE on X11 can use XInput2 raw capture and XTest injection now; X selection clipboard support is still pending."
        }
        LinuxDesktopEnvironment::Wlroots | LinuxDesktopEnvironment::Unknown => {
            "Generic X11 session; XInput2 raw capture and XTest injection are wired when the extensions are present; X selection clipboard support is still pending."
        }
    }
}

const fn wayland_compatibility_notes(desktop_environment: LinuxDesktopEnvironment) -> &'static str {
    match desktop_environment {
        LinuxDesktopEnvironment::Gnome => {
            "GNOME Wayland blocks global capture and synthetic input by default; use portal or Mutter-specific capabilities when available."
        }
        LinuxDesktopEnvironment::KdePlasma => {
            "KDE Plasma Wayland blocks global capture and synthetic input by default; use xdg-desktop-portal or KWin-specific capabilities when available."
        }
        LinuxDesktopEnvironment::Wlroots => {
            "wlroots Wayland compositors vary by implementation; prefer compositor-specific protocols or xdg-desktop-portal where exposed."
        }
        LinuxDesktopEnvironment::Xfce | LinuxDesktopEnvironment::Unknown => {
            "Generic Wayland session; global capture and injection remain blocked until portal or compositor-specific support is wired."
        }
    }
}

impl PlatformAdapter for LinuxPlatform {
    fn os_kind(&self) -> OsKind {
        OsKind::Linux
    }

    fn capabilities(&self) -> PlatformCapabilities {
        let status = self.backend_status();
        PlatformCapabilities {
            input_capture: status.input_capture.is_available(),
            input_injection: status.input_injection.is_available(),
            clipboard_text: status.clipboard_text.is_available(),
        }
    }

    fn permission_checks(&self) -> Vec<PlatformPermissionCheck> {
        match self.display_server() {
            LinuxDisplayServer::X11 => vec![
                PlatformPermissionCheck {
                    id: "linux.x11.capture_backend",
                    label: "Linux X11 capture backend",
                    status: PermissionStatus::Unknown,
                    guidance: "Use an X11 session with the XInput2 extension enabled; KMSync verifies it when capture starts.",
                },
                PlatformPermissionCheck {
                    id: "linux.x11.xtest_extension",
                    label: "Linux X11 XTest extension",
                    status: PermissionStatus::Unknown,
                    guidance: "Use an X11 session with the XTEST extension enabled; KMSync verifies it when the first event is injected.",
                },
            ],
            LinuxDisplayServer::Wayland => vec![
                PlatformPermissionCheck {
                    id: "linux.wayland.capture_permission",
                    label: "Linux Wayland capture permission",
                    status: PermissionStatus::Missing,
                    guidance: "Use a trusted portal or compositor-specific capture backend; global capture is blocked by default.",
                },
                PlatformPermissionCheck {
                    id: "linux.wayland.injection_permission",
                    label: "Linux Wayland injection permission",
                    status: PermissionStatus::Missing,
                    guidance: "Use a trusted portal or compositor-specific injection backend; synthetic input is blocked by default.",
                },
            ],
            LinuxDisplayServer::Unknown => vec![PlatformPermissionCheck {
                id: "linux.display_server",
                label: "Linux display server",
                status: PermissionStatus::Unknown,
                guidance: "Set WAYLAND_DISPLAY, DISPLAY, or XDG_SESSION_TYPE so KMSync can select a backend.",
            }],
        }
    }

    fn permission_hints(&self) -> &'static [&'static str] {
        match (self.display_server(), self.desktop_environment()) {
            (LinuxDisplayServer::X11, LinuxDesktopEnvironment::Gnome) => &[
                "Linux GNOME X11 detected; XInput2 raw capture is enabled when the extension is present.",
                "Linux GNOME X11 detected; XTest injection is enabled when the XTEST extension is present.",
            ],
            (LinuxDisplayServer::X11, LinuxDesktopEnvironment::KdePlasma) => &[
                "Linux KDE Plasma X11 detected; XInput2 raw capture is enabled when the extension is present.",
                "Linux KDE Plasma X11 detected; XTest injection is enabled when the XTEST extension is present.",
            ],
            (LinuxDisplayServer::X11, LinuxDesktopEnvironment::Xfce) => &[
                "Linux XFCE X11 detected; XInput2 raw capture is enabled when the extension is present.",
                "Linux XFCE X11 detected; XTest injection is enabled when the XTEST extension is present.",
            ],
            (LinuxDisplayServer::X11, _) => &[
                "Linux X11 detected; XInput2 raw capture is enabled when the extension is present.",
                "Linux X11 detected; XTest injection is enabled when the XTEST extension is present.",
            ],
            (LinuxDisplayServer::Wayland, LinuxDesktopEnvironment::Gnome) => &[
                "Linux GNOME Wayland detected; global capture is blocked by the compositor security model.",
                "Linux GNOME Wayland detected; input injection is disabled until a portal or Mutter backend is added.",
                "Linux GNOME Wayland detected; xdg-desktop-portal clipboard support is not wired yet.",
            ],
            (LinuxDisplayServer::Wayland, LinuxDesktopEnvironment::KdePlasma) => &[
                "Linux KDE Plasma Wayland detected; global capture is blocked by the compositor security model.",
                "Linux KDE Plasma Wayland detected; input injection is disabled until a portal or KWin backend is added.",
                "Linux KDE Plasma Wayland detected; xdg-desktop-portal clipboard support is not wired yet.",
            ],
            (LinuxDisplayServer::Wayland, LinuxDesktopEnvironment::Wlroots) => &[
                "Linux wlroots Wayland detected; compositor-specific capture support is not wired yet.",
                "Linux wlroots Wayland detected; injection is disabled until compositor-specific support is added.",
                "Linux wlroots Wayland detected; xdg-desktop-portal clipboard support is not wired yet.",
            ],
            (LinuxDisplayServer::Wayland, _) => &[
                "Linux Wayland detected; global capture is blocked by the compositor security model.",
                "Linux Wayland detected; input injection is disabled until a trusted portal or compositor backend is added.",
                "Linux Wayland detected; xdg-desktop-portal clipboard support is not wired yet.",
            ],
            (LinuxDisplayServer::Unknown, _) => &[
                "Linux display server is unknown; set WAYLAND_DISPLAY, DISPLAY, or XDG_SESSION_TYPE.",
                "Input capture and injection are disabled until an X11 or Wayland backend is selected.",
            ],
        }
    }

    fn primary_display_bounds(&self) -> Option<DisplayBounds> {
        None
    }
}

impl InputInjector for LinuxPlatform {
    fn inject(&mut self, event: InputEvent) -> Result<(), String> {
        match self.display_server() {
            LinuxDisplayServer::X11 => self.inject_x11(event),
            LinuxDisplayServer::Wayland | LinuxDisplayServer::Unknown => Err(format!(
                "Linux input injection is unavailable for {} on {:?}: {}",
                input_event_type(event),
                self.display_server(),
                self.backend_status().input_injection.reason()
            )),
        }
    }
}

impl LinuxPlatform {
    #[cfg(target_os = "linux")]
    fn inject_x11(&mut self, event: InputEvent) -> Result<(), String> {
        if self.x11_injector.is_none() {
            self.x11_injector = Some(X11XTestInjector::connect()?);
        }

        let result = match self.x11_injector.as_mut() {
            Some(injector) => injector.inject(event),
            None => Err("Linux X11 XTest injector is not initialized".to_string()),
        };

        if result.is_err() {
            self.x11_injector = None;
        }

        result
    }

    #[cfg(not(target_os = "linux"))]
    fn inject_x11(&mut self, event: InputEvent) -> Result<(), String> {
        Err(format!(
            "Linux X11 XTest injection is unavailable for {} in this test build",
            input_event_type(event)
        ))
    }
}

#[allow(dead_code)]
pub fn hide_local_pointer() {}

#[allow(dead_code)]
pub fn restore_local_pointer(_position: Option<super::PointerPosition>) {}

fn input_event_type(event: InputEvent) -> &'static str {
    match event {
        InputEvent::Key(_) => "key",
        InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. }) => "mouse_move",
        InputEvent::Mouse(kmsync_core::MouseEvent::Position { .. }) => "mouse_position",
        InputEvent::Mouse(kmsync_core::MouseEvent::Button { .. }) => "mouse_button",
        InputEvent::Scroll(_) => "scroll",
    }
}

#[cfg(target_os = "linux")]
struct X11XTestInjector {
    conn: RustConnection,
    screen_num: usize,
    keymap: X11Keymap,
}

#[cfg(target_os = "linux")]
impl X11XTestInjector {
    fn connect() -> Result<Self, String> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|error| format!("failed to connect to X11: {error}"))?;
        conn.xtest_get_version(2, 2)
            .map_err(|error| format!("failed to query XTest extension: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read XTest extension version: {error}"))?;
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let keycode_count = setup
            .max_keycode
            .saturating_sub(min_keycode)
            .saturating_add(1);
        let mapping = conn
            .get_keyboard_mapping(min_keycode, keycode_count)
            .map_err(|error| format!("failed to query X11 keyboard mapping: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read X11 keyboard mapping: {error}"))?;
        Ok(Self {
            conn,
            screen_num,
            keymap: X11Keymap {
                min_keycode,
                keysyms_per_keycode: usize::from(mapping.keysyms_per_keycode),
                keysyms: mapping.keysyms,
            },
        })
    }

    fn inject(&mut self, event: InputEvent) -> Result<(), String> {
        match event {
            InputEvent::Key(event) => self.inject_key(event.key, event.state)?,
            InputEvent::Mouse(MouseEvent::Move { dx, dy }) => {
                self.inject_relative_motion(dx, dy)?
            }
            InputEvent::Mouse(MouseEvent::Position { x_ratio, y_ratio }) => {
                self.inject_absolute_motion(x_ratio, y_ratio)?;
            }
            InputEvent::Mouse(MouseEvent::Button { button, state }) => {
                self.inject_button(x11_mouse_button_detail(button), state)?;
            }
            InputEvent::Scroll(event) => self.inject_scroll(event)?,
        }
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush X11 XTest input: {error}"))
    }

    fn inject_key(&self, key: Key, state: KeyState) -> Result<(), String> {
        let detail = self
            .keymap
            .keycode(key)
            .or_else(|| x11_evdev_keycode(key))
            .ok_or_else(|| format!("unsupported X11 key: {key:?}"))?;
        self.fake_input(key_event_type(state), detail, 0, 0, 0)
    }

    fn inject_button(&self, detail: u8, state: KeyState) -> Result<(), String> {
        self.fake_input(button_event_type(state), detail, 0, 0, 0)
    }

    fn inject_scroll(&self, event: ScrollEvent) -> Result<(), String> {
        for button in x11_scroll_button_sequence(event).into_iter().flatten() {
            for _ in 0..button.1 {
                self.inject_button(button.0, KeyState::Pressed)?;
                self.inject_button(button.0, KeyState::Released)?;
            }
        }
        Ok(())
    }

    fn inject_relative_motion(&self, dx: f32, dy: f32) -> Result<(), String> {
        let (root_x, root_y) = x11_relative_motion(dx, dy);
        if root_x == 0 && root_y == 0 {
            return Ok(());
        }
        self.fake_input(xproto::MOTION_NOTIFY_EVENT, 1, 0, root_x, root_y)
    }

    fn inject_absolute_motion(&self, x_ratio: f32, y_ratio: f32) -> Result<(), String> {
        let screen = self
            .conn
            .setup()
            .roots
            .get(self.screen_num)
            .ok_or_else(|| format!("X11 screen {} is unavailable", self.screen_num))?;
        let (root_x, root_y) = x11_absolute_position(
            x_ratio,
            y_ratio,
            screen.width_in_pixels,
            screen.height_in_pixels,
        );
        self.fake_input(xproto::MOTION_NOTIFY_EVENT, 0, screen.root, root_x, root_y)
    }

    fn fake_input(
        &self,
        event_type: u8,
        detail: u8,
        root: xproto::Window,
        root_x: i16,
        root_y: i16,
    ) -> Result<(), String> {
        self.conn
            .xtest_fake_input(event_type, detail, 0, root, root_x, root_y, 0)
            .map_err(|error| format!("failed to queue X11 XTest input: {error}"))?
            .ignore_error();
        Ok(())
    }
}

#[cfg(target_os = "linux")]
struct X11Keymap {
    min_keycode: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
}

#[cfg(target_os = "linux")]
impl X11Keymap {
    fn keycode(&self, key: Key) -> Option<u8> {
        if self.keysyms_per_keycode == 0 {
            return None;
        }
        let candidates = x11_keysyms_for_key(key);
        if candidates.is_empty() {
            return None;
        }
        self.keysyms
            .chunks(self.keysyms_per_keycode)
            .enumerate()
            .find_map(|(index, keysyms)| {
                if keysyms.iter().any(|keysym| candidates.contains(keysym)) {
                    u8::try_from(index)
                        .ok()
                        .and_then(|offset| self.min_keycode.checked_add(offset))
                } else {
                    None
                }
            })
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct X11CaptureState {
    modifiers: Modifiers,
}

#[cfg(target_os = "linux")]
impl Default for X11CaptureState {
    fn default() -> Self {
        Self {
            modifiers: Modifiers::NONE,
        }
    }
}

#[cfg(target_os = "linux")]
impl X11CaptureState {
    fn captured_input(&mut self, event: X11Event) -> Option<CapturedInput> {
        let input = match event {
            X11Event::XinputRawKeyPress(event) => {
                self.key_event(event.detail, KeyState::Pressed)?
            }
            X11Event::XinputRawKeyRelease(event) => {
                self.key_event(event.detail, KeyState::Released)?
            }
            X11Event::XinputRawButtonPress(event) => {
                x11_button_event(u8::try_from(event.detail).ok()?, KeyState::Pressed)?
            }
            X11Event::XinputRawButtonRelease(event) => {
                x11_button_event(u8::try_from(event.detail).ok()?, KeyState::Released)?
            }
            X11Event::XinputRawMotion(event) => {
                let (dx, dy) =
                    x11_raw_motion_delta_from_fp(&event.valuator_mask, &event.axisvalues_raw)?;
                InputEvent::Mouse(MouseEvent::Move { dx, dy })
            }
            _ => return None,
        };

        Some(CapturedInput {
            event: input,
            pointer: None,
        })
    }

    fn key_event(&mut self, detail: u32, state: KeyState) -> Option<InputEvent> {
        let key = x11_key_from_evdev_keycode(u8::try_from(detail).ok()?)?;
        if let Some(modifier) = x11_modifier_for_key(key) {
            self.modifiers = match state {
                KeyState::Pressed => self.modifiers.with(modifier),
                KeyState::Released => self.modifiers.without(modifier),
            };
        }
        Some(InputEvent::Key(KeyEvent {
            key,
            state,
            modifiers: self.modifiers,
        }))
    }
}

#[cfg(target_os = "linux")]
fn x11_modifier_for_key(key: Key) -> Option<Modifiers> {
    match key {
        Key::LeftControl | Key::RightControl => Some(Modifiers::CONTROL),
        Key::LeftShift | Key::RightShift => Some(Modifiers::SHIFT),
        Key::LeftAlt | Key::RightAlt => Some(Modifiers::ALT),
        Key::LeftMeta | Key::RightMeta => Some(Modifiers::META),
        _ => None,
    }
}

#[must_use]
pub(super) const fn x11_key_from_evdev_keycode(keycode: u8) -> Option<Key> {
    Some(match keycode {
        9 => Key::Escape,
        10 => Key::Num1,
        11 => Key::Num2,
        12 => Key::Num3,
        13 => Key::Num4,
        14 => Key::Num5,
        15 => Key::Num6,
        16 => Key::Num7,
        17 => Key::Num8,
        18 => Key::Num9,
        19 => Key::Num0,
        20 => Key::Minus,
        21 => Key::Equal,
        22 => Key::Backspace,
        23 => Key::Tab,
        24 => Key::Q,
        25 => Key::W,
        26 => Key::E,
        27 => Key::R,
        28 => Key::T,
        29 => Key::Y,
        30 => Key::U,
        31 => Key::I,
        32 => Key::O,
        33 => Key::P,
        34 => Key::LeftBracket,
        35 => Key::RightBracket,
        36 => Key::Enter,
        37 => Key::LeftControl,
        38 => Key::A,
        39 => Key::S,
        40 => Key::D,
        41 => Key::F,
        42 => Key::G,
        43 => Key::H,
        44 => Key::J,
        45 => Key::K,
        46 => Key::L,
        47 => Key::Semicolon,
        48 => Key::Quote,
        49 => Key::Grave,
        50 => Key::LeftShift,
        51 => Key::Backslash,
        52 => Key::Z,
        53 => Key::X,
        54 => Key::C,
        55 => Key::V,
        56 => Key::B,
        57 => Key::N,
        58 => Key::M,
        59 => Key::Comma,
        60 => Key::Dot,
        61 => Key::Slash,
        62 => Key::RightShift,
        63 => Key::NumpadMultiply,
        64 => Key::LeftAlt,
        65 => Key::Space,
        66 => Key::CapsLock,
        67 => Key::F1,
        68 => Key::F2,
        69 => Key::F3,
        70 => Key::F4,
        71 => Key::F5,
        72 => Key::F6,
        73 => Key::F7,
        74 => Key::F8,
        75 => Key::F9,
        76 => Key::F10,
        77 => Key::NumLock,
        78 => Key::ScrollLock,
        79 => Key::Numpad7,
        80 => Key::Numpad8,
        81 => Key::Numpad9,
        82 => Key::NumpadSubtract,
        83 => Key::Numpad4,
        84 => Key::Numpad5,
        85 => Key::Numpad6,
        86 => Key::NumpadAdd,
        87 => Key::Numpad1,
        88 => Key::Numpad2,
        89 => Key::Numpad3,
        90 => Key::Numpad0,
        91 => Key::NumpadDecimal,
        95 => Key::F11,
        96 => Key::F12,
        104 => Key::NumpadEnter,
        105 => Key::RightControl,
        106 => Key::NumpadDivide,
        107 => Key::PrintScreen,
        108 => Key::RightAlt,
        110 => Key::Home,
        111 => Key::ArrowUp,
        112 => Key::PageUp,
        113 => Key::ArrowLeft,
        114 => Key::ArrowRight,
        115 => Key::End,
        116 => Key::ArrowDown,
        117 => Key::PageDown,
        118 => Key::Insert,
        119 => Key::Delete,
        125 => Key::NumpadEqual,
        127 => Key::Pause,
        133 => Key::LeftMeta,
        134 => Key::RightMeta,
        191 => Key::F13,
        192 => Key::F14,
        193 => Key::F15,
        194 => Key::F16,
        195 => Key::F17,
        196 => Key::F18,
        197 => Key::F19,
        198 => Key::F20,
        199 => Key::F21,
        200 => Key::F22,
        201 => Key::F23,
        202 => Key::F24,
        _ => return None,
    })
}

#[must_use]
pub(super) const fn x11_button_event(detail: u8, state: KeyState) -> Option<InputEvent> {
    match (detail, state) {
        (1, state) => Some(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Left,
            state,
        })),
        (2, state) => Some(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Middle,
            state,
        })),
        (3, state) => Some(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Right,
            state,
        })),
        (8, state) => Some(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Back,
            state,
        })),
        (9, state) => Some(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Forward,
            state,
        })),
        (4, KeyState::Pressed) => Some(InputEvent::Scroll(ScrollEvent { dx: 0.0, dy: 1.0 })),
        (5, KeyState::Pressed) => Some(InputEvent::Scroll(ScrollEvent { dx: 0.0, dy: -1.0 })),
        (6, KeyState::Pressed) => Some(InputEvent::Scroll(ScrollEvent { dx: -1.0, dy: 0.0 })),
        (7, KeyState::Pressed) => Some(InputEvent::Scroll(ScrollEvent { dx: 1.0, dy: 0.0 })),
        (4..=7, KeyState::Released) | (_, _) => None,
    }
}

#[must_use]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn x11_raw_motion_delta(
    valuator_mask: &[u32],
    axisvalues: &[f32],
) -> Option<(f32, f32)> {
    let mut dx = None;
    let mut dy = None;
    let mut value_index = 0usize;
    for (word_index, word) in valuator_mask.iter().copied().enumerate() {
        for bit in 0..32 {
            if word & (1 << bit) == 0 {
                continue;
            }
            let value = *axisvalues.get(value_index)?;
            let axis = word_index * 32 + bit;
            match axis {
                0 => dx = Some(value),
                1 => dy = Some(value),
                _ => {}
            }
            value_index += 1;
        }
    }
    match (dx, dy) {
        (Some(dx), Some(dy)) if dx != 0.0 || dy != 0.0 => Some((dx, dy)),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn x11_raw_motion_delta_from_fp(
    valuator_mask: &[u32],
    axisvalues: &[xinput::Fp3232],
) -> Option<(f32, f32)> {
    let mut dx = None;
    let mut dy = None;
    let mut value_index = 0usize;
    for (word_index, word) in valuator_mask.iter().copied().enumerate() {
        for bit in 0..32 {
            if word & (1 << bit) == 0 {
                continue;
            }
            let value = xinput_fp3232_to_f32(*axisvalues.get(value_index)?);
            let axis = word_index * 32 + bit;
            match axis {
                0 => dx = Some(value),
                1 => dy = Some(value),
                _ => {}
            }
            value_index += 1;
        }
    }
    match (dx, dy) {
        (Some(dx), Some(dy)) if dx != 0.0 || dy != 0.0 => Some((dx, dy)),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn xinput_fp3232_to_f32(value: xinput::Fp3232) -> f32 {
    (f64::from(value.integral) + f64::from(value.frac) / 4_294_967_296.0) as f32
}

#[must_use]
pub(super) const fn x11_mouse_button_detail(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    }
}

#[must_use]
pub(super) fn x11_scroll_button_sequence(event: ScrollEvent) -> [Option<(u8, u8)>; 2] {
    [
        scroll_axis_button(event.dy, 4, 5),
        scroll_axis_button(event.dx, 7, 6),
    ]
}

fn scroll_axis_button(value: f32, positive_button: u8, negative_button: u8) -> Option<(u8, u8)> {
    let steps = rounded_i16(value);
    match steps.cmp(&0) {
        std::cmp::Ordering::Greater => Some((positive_button, steps.unsigned_abs().min(255) as u8)),
        std::cmp::Ordering::Less => Some((negative_button, steps.unsigned_abs().min(255) as u8)),
        std::cmp::Ordering::Equal => None,
    }
}

#[must_use]
pub(super) fn x11_relative_motion(dx: f32, dy: f32) -> (i16, i16) {
    (rounded_i16(dx), rounded_i16(dy))
}

#[must_use]
pub(super) fn x11_absolute_position(
    x_ratio: f32,
    y_ratio: f32,
    width: u16,
    height: u16,
) -> (i16, i16) {
    let max_x = width.saturating_sub(1) as f32;
    let max_y = height.saturating_sub(1) as f32;
    (
        rounded_i16(x_ratio.clamp(0.0, 1.0) * max_x),
        rounded_i16(y_ratio.clamp(0.0, 1.0) * max_y),
    )
}

fn rounded_i16(value: f32) -> i16 {
    if value.is_finite() {
        value
            .round()
            .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
    } else {
        0
    }
}

#[cfg(target_os = "linux")]
const fn key_event_type(state: KeyState) -> u8 {
    match state {
        KeyState::Pressed => 2,
        KeyState::Released => 3,
    }
}

#[cfg(target_os = "linux")]
const fn button_event_type(state: KeyState) -> u8 {
    match state {
        KeyState::Pressed => 4,
        KeyState::Released => 5,
    }
}

#[must_use]
pub(super) const fn x11_evdev_keycode(key: Key) -> Option<u8> {
    Some(match key {
        Key::Escape => 9,
        Key::Num1 => 10,
        Key::Num2 => 11,
        Key::Num3 => 12,
        Key::Num4 => 13,
        Key::Num5 => 14,
        Key::Num6 => 15,
        Key::Num7 => 16,
        Key::Num8 => 17,
        Key::Num9 => 18,
        Key::Num0 => 19,
        Key::Minus => 20,
        Key::Equal => 21,
        Key::Backspace => 22,
        Key::Tab => 23,
        Key::Q => 24,
        Key::W => 25,
        Key::E => 26,
        Key::R => 27,
        Key::T => 28,
        Key::Y => 29,
        Key::U => 30,
        Key::I => 31,
        Key::O => 32,
        Key::P => 33,
        Key::LeftBracket => 34,
        Key::RightBracket => 35,
        Key::Enter => 36,
        Key::LeftControl => 37,
        Key::A => 38,
        Key::S => 39,
        Key::D => 40,
        Key::F => 41,
        Key::G => 42,
        Key::H => 43,
        Key::J => 44,
        Key::K => 45,
        Key::L => 46,
        Key::Semicolon => 47,
        Key::Quote => 48,
        Key::Grave => 49,
        Key::LeftShift => 50,
        Key::Backslash => 51,
        Key::Z => 52,
        Key::X => 53,
        Key::C => 54,
        Key::V => 55,
        Key::B => 56,
        Key::N => 57,
        Key::M => 58,
        Key::Comma => 59,
        Key::Dot => 60,
        Key::Slash => 61,
        Key::RightShift => 62,
        Key::NumpadMultiply => 63,
        Key::LeftAlt => 64,
        Key::Space => 65,
        Key::CapsLock => 66,
        Key::F1 => 67,
        Key::F2 => 68,
        Key::F3 => 69,
        Key::F4 => 70,
        Key::F5 => 71,
        Key::F6 => 72,
        Key::F7 => 73,
        Key::F8 => 74,
        Key::F9 => 75,
        Key::F10 => 76,
        Key::NumLock => 77,
        Key::ScrollLock => 78,
        Key::Numpad7 => 79,
        Key::Numpad8 => 80,
        Key::Numpad9 => 81,
        Key::NumpadSubtract => 82,
        Key::Numpad4 => 83,
        Key::Numpad5 => 84,
        Key::Numpad6 => 85,
        Key::NumpadAdd => 86,
        Key::Numpad1 => 87,
        Key::Numpad2 => 88,
        Key::Numpad3 => 89,
        Key::Numpad0 => 90,
        Key::NumpadDecimal => 91,
        Key::F11 => 95,
        Key::F12 => 96,
        Key::NumpadEnter => 104,
        Key::RightControl => 105,
        Key::NumpadDivide => 106,
        Key::PrintScreen => 107,
        Key::RightAlt => 108,
        Key::Home => 110,
        Key::ArrowUp => 111,
        Key::PageUp => 112,
        Key::ArrowLeft => 113,
        Key::ArrowRight => 114,
        Key::End => 115,
        Key::ArrowDown => 116,
        Key::PageDown => 117,
        Key::Insert => 118,
        Key::Delete => 119,
        Key::NumpadEqual => 125,
        Key::Pause => 127,
        Key::LeftMeta => 133,
        Key::RightMeta => 134,
        Key::F13 => 191,
        Key::F14 => 192,
        Key::F15 => 193,
        Key::F16 => 194,
        Key::F17 => 195,
        Key::F18 => 196,
        Key::F19 => 197,
        Key::F20 => 198,
        Key::F21 => 199,
        Key::F22 => 200,
        Key::F23 => 201,
        Key::F24 => 202,
        Key::Kana
        | Key::Eisu
        | Key::ImeOn
        | Key::ImeOff
        | Key::BrightnessDown
        | Key::BrightnessUp
        | Key::MediaPlay
        | Key::MediaPause
        | Key::MediaRecord
        | Key::MediaFastForward
        | Key::MediaRewind
        | Key::MediaNextTrack
        | Key::MediaPreviousTrack
        | Key::MediaStop
        | Key::MediaPlayPause
        | Key::VolumeMute
        | Key::VolumeUp
        | Key::VolumeDown
        | Key::Fn
        | Key::Globe => return None,
    })
}

#[cfg(target_os = "linux")]
fn x11_keysyms_for_key(key: Key) -> &'static [u32] {
    match key {
        Key::A => &[0x0061, 0x0041],
        Key::B => &[0x0062, 0x0042],
        Key::C => &[0x0063, 0x0043],
        Key::D => &[0x0064, 0x0044],
        Key::E => &[0x0065, 0x0045],
        Key::F => &[0x0066, 0x0046],
        Key::G => &[0x0067, 0x0047],
        Key::H => &[0x0068, 0x0048],
        Key::I => &[0x0069, 0x0049],
        Key::J => &[0x006a, 0x004a],
        Key::K => &[0x006b, 0x004b],
        Key::L => &[0x006c, 0x004c],
        Key::M => &[0x006d, 0x004d],
        Key::N => &[0x006e, 0x004e],
        Key::O => &[0x006f, 0x004f],
        Key::P => &[0x0070, 0x0050],
        Key::Q => &[0x0071, 0x0051],
        Key::R => &[0x0072, 0x0052],
        Key::S => &[0x0073, 0x0053],
        Key::T => &[0x0074, 0x0054],
        Key::U => &[0x0075, 0x0055],
        Key::V => &[0x0076, 0x0056],
        Key::W => &[0x0077, 0x0057],
        Key::X => &[0x0078, 0x0058],
        Key::Y => &[0x0079, 0x0059],
        Key::Z => &[0x007a, 0x005a],
        Key::Num0 => &[0x0030],
        Key::Num1 => &[0x0031],
        Key::Num2 => &[0x0032],
        Key::Num3 => &[0x0033],
        Key::Num4 => &[0x0034],
        Key::Num5 => &[0x0035],
        Key::Num6 => &[0x0036],
        Key::Num7 => &[0x0037],
        Key::Num8 => &[0x0038],
        Key::Num9 => &[0x0039],
        Key::Enter => &[0xff0d],
        Key::Escape => &[0xff1b],
        Key::Backspace => &[0xff08],
        Key::Tab => &[0xff09],
        Key::Space => &[0x0020],
        Key::Minus => &[0x002d],
        Key::Equal => &[0x003d],
        Key::LeftBracket => &[0x005b],
        Key::RightBracket => &[0x005d],
        Key::Backslash => &[0x005c],
        Key::Semicolon => &[0x003b],
        Key::Quote => &[0x0027],
        Key::Grave => &[0x0060],
        Key::Comma => &[0x002c],
        Key::Dot => &[0x002e],
        Key::Slash => &[0x002f],
        Key::CapsLock => &[0xffe5],
        Key::F1 => &[0xffbe],
        Key::F2 => &[0xffbf],
        Key::F3 => &[0xffc0],
        Key::F4 => &[0xffc1],
        Key::F5 => &[0xffc2],
        Key::F6 => &[0xffc3],
        Key::F7 => &[0xffc4],
        Key::F8 => &[0xffc5],
        Key::F9 => &[0xffc6],
        Key::F10 => &[0xffc7],
        Key::F11 => &[0xffc8],
        Key::F12 => &[0xffc9],
        Key::F13 => &[0xffca],
        Key::F14 => &[0xffcb],
        Key::F15 => &[0xffcc],
        Key::F16 => &[0xffcd],
        Key::F17 => &[0xffce],
        Key::F18 => &[0xffcf],
        Key::F19 => &[0xffd0],
        Key::F20 => &[0xffd1],
        Key::F21 => &[0xffd2],
        Key::F22 => &[0xffd3],
        Key::F23 => &[0xffd4],
        Key::F24 => &[0xffd5],
        Key::PrintScreen => &[0xff61],
        Key::ScrollLock => &[0xff14],
        Key::Pause => &[0xff13],
        Key::Insert => &[0xff63],
        Key::Home => &[0xff50],
        Key::PageUp => &[0xff55],
        Key::Delete => &[0xffff],
        Key::End => &[0xff57],
        Key::PageDown => &[0xff56],
        Key::ArrowRight => &[0xff53],
        Key::ArrowLeft => &[0xff51],
        Key::ArrowDown => &[0xff54],
        Key::ArrowUp => &[0xff52],
        Key::NumLock => &[0xff7f],
        Key::NumpadDivide => &[0xffaf],
        Key::NumpadMultiply => &[0xffaa],
        Key::NumpadSubtract => &[0xffad],
        Key::NumpadAdd => &[0xffab],
        Key::NumpadEnter => &[0xff8d],
        Key::Numpad0 => &[0xffb0],
        Key::Numpad1 => &[0xffb1],
        Key::Numpad2 => &[0xffb2],
        Key::Numpad3 => &[0xffb3],
        Key::Numpad4 => &[0xffb4],
        Key::Numpad5 => &[0xffb5],
        Key::Numpad6 => &[0xffb6],
        Key::Numpad7 => &[0xffb7],
        Key::Numpad8 => &[0xffb8],
        Key::Numpad9 => &[0xffb9],
        Key::NumpadDecimal => &[0xffae],
        Key::NumpadEqual => &[0xffbd],
        Key::LeftControl => &[0xffe3],
        Key::LeftShift => &[0xffe1],
        Key::LeftAlt => &[0xffe9],
        Key::LeftMeta => &[0xffeb],
        Key::RightControl => &[0xffe4],
        Key::RightShift => &[0xffe2],
        Key::RightAlt => &[0xffea, 0xfe03],
        Key::RightMeta => &[0xffec],
        Key::BrightnessDown => &[0x1008_ff03],
        Key::BrightnessUp => &[0x1008_ff02],
        Key::MediaPlay => &[0x1008_ff14],
        Key::MediaPause => &[0x1008_ff31],
        Key::MediaRecord => &[0x1008_ff1c],
        Key::MediaFastForward => &[0x1008_ff27],
        Key::MediaRewind => &[0x1008_ff3e],
        Key::MediaNextTrack => &[0x1008_ff17],
        Key::MediaPreviousTrack => &[0x1008_ff16],
        Key::MediaStop => &[0x1008_ff15],
        Key::MediaPlayPause => &[0x1008_ff14, 0x1008_ff31],
        Key::VolumeMute => &[0x1008_ff12],
        Key::VolumeUp => &[0x1008_ff13],
        Key::VolumeDown => &[0x1008_ff11],
        Key::Kana | Key::Eisu | Key::ImeOn | Key::ImeOff | Key::Fn | Key::Globe => &[],
    }
}

impl InputCaptureBackend for LinuxPlatform {
    fn capture_loop<F>(&mut self, _callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static,
    {
        match self.display_server() {
            LinuxDisplayServer::X11 => self.capture_x11(_callback),
            LinuxDisplayServer::Wayland | LinuxDisplayServer::Unknown => Err(format!(
                "Linux input capture is unavailable on {:?}: {}",
                self.display_server(),
                self.backend_status().input_capture.reason()
            )),
        }
    }
}

impl LinuxPlatform {
    #[cfg(target_os = "linux")]
    fn capture_x11<F>(&mut self, mut callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static,
    {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|error| format!("failed to connect to X11: {error}"))?;
        conn.xinput_xi_query_version(2, 0)
            .map_err(|error| format!("failed to query XInput2 extension: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read XInput2 extension version: {error}"))?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| format!("X11 screen {screen_num} is unavailable"))?
            .root;
        let mask = xinput::EventMask {
            deviceid: u16::from(xinput::Device::ALL_MASTER),
            mask: vec![
                xinput::XIEventMask::RAW_KEY_PRESS
                    | xinput::XIEventMask::RAW_KEY_RELEASE
                    | xinput::XIEventMask::RAW_BUTTON_PRESS
                    | xinput::XIEventMask::RAW_BUTTON_RELEASE
                    | xinput::XIEventMask::RAW_MOTION,
            ],
        };
        conn.xinput_xi_select_events(root, &[mask])
            .map_err(|error| format!("failed to select XInput2 raw events: {error}"))?
            .check()
            .map_err(|error| format!("XInput2 raw event selection failed: {error}"))?;
        conn.flush()
            .map_err(|error| format!("failed to flush XInput2 capture setup: {error}"))?;

        let mut state = X11CaptureState::default();
        loop {
            let event = conn
                .wait_for_event()
                .map_err(|error| format!("failed to read XInput2 event: {error}"))?;
            if let Some(captured) = state.captured_input(event) {
                let _decision = callback(captured);
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn capture_x11<F>(&mut self, _callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static,
    {
        Err("Linux X11 XInput2 capture is unavailable in this test build".to_string())
    }
}

impl ClipboardBackend for LinuxPlatform {
    fn get_clipboard_text(&self) -> Result<String, String> {
        Err(format!(
            "Linux clipboard text is unavailable on {:?}: {}",
            self.display_server(),
            self.backend_status().clipboard_text.reason()
        ))
    }

    fn set_clipboard_text(&mut self, _text: &str) -> Result<(), String> {
        Err(format!(
            "Linux clipboard text is unavailable on {:?}: {}",
            self.display_server(),
            self.backend_status().clipboard_text.reason()
        ))
    }

    fn clipboard_watch_backend(&self) -> ClipboardWatchBackend {
        ClipboardWatchBackend::Unavailable
    }

    fn wait_for_clipboard_change(
        &self,
        _previous: &kmsync_core::ClipboardText,
        _fallback_interval: Duration,
    ) -> Result<kmsync_core::ClipboardText, String> {
        Err(format!(
            "Linux clipboard watch is unavailable on {:?}: {}",
            self.display_server(),
            self.backend_status().clipboard_text.reason()
        ))
    }
}
