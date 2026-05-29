use std::time::Duration;

use kmsync_core::{InputEvent, OsKind};

use super::{
    CaptureDecision, CapturedInput, ClipboardBackend, ClipboardWatchBackend, DisplayBounds,
    InputCaptureBackend, InputInjector, PermissionStatus, PlatformAdapter, PlatformCapabilities,
    PlatformPermissionCheck,
};

pub struct UnsupportedPlatform;

impl UnsupportedPlatform {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PlatformAdapter for UnsupportedPlatform {
    fn os_kind(&self) -> OsKind {
        OsKind::Linux
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            input_capture: false,
            input_injection: false,
            clipboard_text: false,
        }
    }

    fn permission_checks(&self) -> Vec<PlatformPermissionCheck> {
        vec![PlatformPermissionCheck {
            id: "platform.unsupported",
            label: "Unsupported platform",
            status: PermissionStatus::NotApplicable,
            guidance: "Use macOS, Windows, or Linux for the current daemon build.",
        }]
    }

    fn permission_hints(&self) -> &'static [&'static str] {
        &["This MVP currently targets macOS and Windows."]
    }

    fn primary_display_bounds(&self) -> Option<DisplayBounds> {
        None
    }
}

impl InputInjector for UnsupportedPlatform {
    fn inject(&mut self, event: InputEvent) -> Result<(), String> {
        Err(format!(
            "input injection is unsupported on this platform: {}",
            input_event_type(event)
        ))
    }
}

pub fn hide_local_pointer() {}

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

impl InputCaptureBackend for UnsupportedPlatform {
    fn capture_loop<F>(&mut self, _callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static,
    {
        Err("input capture is unsupported on this platform".to_string())
    }
}

impl ClipboardBackend for UnsupportedPlatform {
    fn get_clipboard_text(&self) -> Result<String, String> {
        Err("clipboard is unsupported on this platform".to_string())
    }

    fn set_clipboard_text(&mut self, _text: &str) -> Result<(), String> {
        Err("clipboard is unsupported on this platform".to_string())
    }

    fn clipboard_watch_backend(&self) -> ClipboardWatchBackend {
        ClipboardWatchBackend::Unavailable
    }

    fn wait_for_clipboard_change(
        &self,
        _previous: &kmsync_core::ClipboardText,
        _fallback_interval: Duration,
    ) -> Result<kmsync_core::ClipboardText, String> {
        Err("clipboard watch is unsupported on this platform".to_string())
    }
}
