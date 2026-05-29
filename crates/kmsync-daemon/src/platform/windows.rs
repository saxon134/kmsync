use std::borrow::Cow;
use std::cell::RefCell;
use std::mem::size_of;
use std::ptr;
use std::time::Duration;

use arboard::ImageData;
use kmsync_core::{
    ClipboardFormat, ClipboardText, InputEvent, Key, KeyEvent, KeyState, Modifiers, MouseButton,
    MouseEvent, OsKind, ScrollEvent,
};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardSequenceNumber, RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow,
    GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, RegisterClassW, SetCursorPos,
    SetWindowsHookExW, ShowCursor, UnhookWindowsHookEx, HWND_MESSAGE, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CYSCREEN, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_CLIPBOARDUPDATE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
};

const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
const KMSYNC_INJECTED_EVENT_MARKER: usize = 0x4B4D_5359_4E43;

use super::{
    CaptureDecision, CapturedInput, ClipboardBackend, ClipboardWatchBackend, DisplayBounds,
    DisplayLayout, InputCaptureBackend, InputInjector, PermissionStatus, PlatformAdapter,
    PlatformCapabilities, PlatformPermissionCheck, PointerPosition,
};

type CaptureCallback = Box<dyn FnMut(CapturedInput) -> CaptureDecision + Send>;

thread_local! {
    static CAPTURE_CALLBACK: RefCell<Option<CaptureCallback>> = RefCell::new(None);
    static LAST_MOUSE_POS: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
}

pub struct WindowsPlatform;

impl WindowsPlatform {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PlatformAdapter for WindowsPlatform {
    fn os_kind(&self) -> OsKind {
        OsKind::Windows
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            input_capture: true,
            input_injection: true,
            clipboard_text: true,
        }
    }

    fn permission_checks(&self) -> Vec<PlatformPermissionCheck> {
        vec![PlatformPermissionCheck {
            id: "windows.interactive_desktop",
            label: "Windows interactive desktop",
            status: windows_interactive_desktop_status(),
            guidance: "Run KMSync in the active interactive user session; services need a user-mode companion for secure desktop boundaries.",
        }]
    }

    fn permission_hints(&self) -> &'static [&'static str] {
        &[
            "Run as the interactive desktop user for normal SendInput injection.",
            "Use the Windows Service as the system anchor and the user-mode companion for interactive desktop input.",
        ]
    }

    #[allow(unsafe_code)]
    fn primary_display_bounds(&self) -> Option<DisplayBounds> {
        Some(DisplayBounds {
            x: 0.0,
            y: 0.0,
            width: f64::from(unsafe { GetSystemMetrics(SM_CXSCREEN) }),
            height: f64::from(unsafe { GetSystemMetrics(SM_CYSCREEN) }),
        })
    }

    fn display_layout(&self) -> DisplayLayout {
        windows_display_layout()
            .unwrap_or_else(|| DisplayLayout::from_primary(self.primary_display_bounds()))
    }

    fn active_application_id(&self) -> Option<String> {
        windows_active_application_id()
    }
}

#[cfg(not(test))]
#[allow(unsafe_code)]
fn windows_interactive_desktop_status() -> PermissionStatus {
    use windows_sys::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_SWITCHDESKTOP,
    };

    let desktop = unsafe { OpenInputDesktop(Default::default(), 0, DESKTOP_SWITCHDESKTOP) };
    if desktop.is_null() {
        PermissionStatus::Missing
    } else {
        unsafe {
            CloseDesktop(desktop);
        }
        PermissionStatus::Granted
    }
}

#[cfg(test)]
fn windows_interactive_desktop_status() -> PermissionStatus {
    PermissionStatus::Granted
}

#[allow(unsafe_code)]
pub fn hide_local_pointer() {
    unsafe { while ShowCursor(0) >= 0 {} }
}

#[allow(unsafe_code)]
pub fn restore_local_pointer(position: Option<PointerPosition>) {
    if let Some(position) = position {
        unsafe {
            SetCursorPos(position.x.round() as i32, position.y.round() as i32);
        }
    }
    unsafe { while ShowCursor(1) < 0 {} }
}

#[allow(unsafe_code)]
fn windows_active_application_id() -> Option<String> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }

    let mut process_id = 0_u32;
    unsafe {
        GetWindowThreadProcessId(window, &mut process_id);
    }
    if process_id == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }

    let image_name = query_process_image_name(process).and_then(|path| {
        process_image_file_name(&path).or_else(|| (!path.is_empty()).then_some(path))
    });
    unsafe {
        CloseHandle(process);
    }
    image_name
}

#[allow(unsafe_code)]
fn query_process_image_name(process: *mut std::ffi::c_void) -> Option<String> {
    let mut buffer = [0_u16; 32_768];
    let mut len = u32::try_from(buffer.len()).ok()?;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len) };
    if ok == 0 || len == 0 {
        return None;
    }
    String::from_utf16(&buffer[..usize::try_from(len).ok()?]).ok()
}

fn process_image_file_name(path: &str) -> Option<String> {
    path.rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .map(ToString::to_string)
}

#[allow(unsafe_code)]
fn windows_display_layout() -> Option<DisplayLayout> {
    let mut displays = Vec::new();
    let displays_ptr = &mut displays as *mut Vec<DisplayBounds>;
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor_bounds),
            displays_ptr as LPARAM,
        )
    };
    if ok == 0 || displays.is_empty() {
        None
    } else {
        Some(DisplayLayout::new(displays))
    }
}

#[allow(unsafe_code)]
unsafe extern "system" fn collect_monitor_bounds(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> i32 {
    let Some(displays) = (data as *mut Vec<DisplayBounds>).as_mut() else {
        return 0;
    };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
        ..MONITORINFO::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        displays.push(rect_to_display_bounds(info.rcMonitor));
    }
    1
}

fn rect_to_display_bounds(rect: RECT) -> DisplayBounds {
    DisplayBounds {
        x: f64::from(rect.left),
        y: f64::from(rect.top),
        width: f64::from(rect.right - rect.left),
        height: f64::from(rect.bottom - rect.top),
    }
}

impl InputInjector for WindowsPlatform {
    fn inject(&mut self, event: InputEvent) -> Result<(), String> {
        match event {
            InputEvent::Key(event) => inject_key(event.key, event.state),
            InputEvent::Mouse(MouseEvent::Move { dx, dy }) => inject_mouse_move(dx, dy),
            InputEvent::Mouse(MouseEvent::Position { x_ratio, y_ratio }) => {
                inject_mouse_position(x_ratio, y_ratio)
            }
            InputEvent::Mouse(MouseEvent::Button { button, state }) => {
                inject_mouse_button(button, state)
            }
            InputEvent::Scroll(event) => inject_scroll(event),
        }
    }
}

impl InputCaptureBackend for WindowsPlatform {
    #[allow(unsafe_code)]
    fn capture_loop<F>(&mut self, callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static,
    {
        set_capture_callback_for_current_thread(Box::new(callback));
        clear_last_mouse_position_for_current_thread();

        let keyboard_hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                std::ptr::null_mut(),
                0,
            )
        };
        if keyboard_hook.is_null() {
            clear_capture_callback_for_current_thread();
            return Err("failed to install Windows keyboard hook".to_string());
        }
        let mouse_hook = unsafe {
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), std::ptr::null_mut(), 0)
        };
        if mouse_hook.is_null() {
            unsafe {
                UnhookWindowsHookEx(keyboard_hook);
            }
            clear_capture_callback_for_current_thread();
            return Err("failed to install Windows mouse hook".to_string());
        }

        let mut message = MSG::default();
        loop {
            let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
            if result <= 0 {
                unsafe {
                    UnhookWindowsHookEx(keyboard_hook);
                    UnhookWindowsHookEx(mouse_hook);
                }
                clear_capture_callback_for_current_thread();
                return Ok(());
            }
        }
    }
}

impl ClipboardBackend for WindowsPlatform {
    fn get_clipboard_text(&self) -> Result<String, String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open Windows clipboard: {error}"))?;
        clipboard
            .get_text()
            .map_err(|error| format!("failed to read Windows clipboard text: {error}"))
    }

    fn set_clipboard_text(&mut self, text: &str) -> Result<(), String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open Windows clipboard: {error}"))?;
        clipboard
            .set_text(text.to_string())
            .map_err(|error| format!("failed to write Windows clipboard text: {error}"))
    }

    fn clipboard_watch_backend(&self) -> ClipboardWatchBackend {
        ClipboardWatchBackend::PlatformNotification
    }

    fn get_clipboard_content(&self) -> Result<ClipboardText, String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open Windows clipboard: {error}"))?;
        if let Ok(image) = clipboard.get_image() {
            return clipboard_text_from_image(image, "Windows");
        }
        let html = clipboard.get().html().ok().filter(|html| !html.is_empty());
        let text = clipboard
            .get_text()
            .or_else(|error| html.as_ref().map_or(Err(error), |html| Ok(html.clone())))
            .map_err(|error| format!("failed to read Windows clipboard text: {error}"))?;
        Ok(match html {
            Some(html) => ClipboardText::html(0, 0, html, text),
            None => ClipboardText::from_local_text(0, 0, text),
        })
    }

    fn set_clipboard_content(&mut self, clipboard: &ClipboardText) -> Result<(), String> {
        let mut backend = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open Windows clipboard: {error}"))?;
        match (clipboard.format, clipboard.html.as_deref()) {
            (ClipboardFormat::Image, _) => {
                let image = clipboard
                    .image
                    .as_ref()
                    .ok_or_else(|| "Windows clipboard image is missing pixel data".to_string())?;
                backend
                    .set_image(ImageData {
                        width: usize::try_from(image.width)
                            .map_err(|_| "Windows clipboard image width overflow".to_string())?,
                        height: usize::try_from(image.height)
                            .map_err(|_| "Windows clipboard image height overflow".to_string())?,
                        bytes: Cow::Borrowed(image.rgba.as_slice()),
                    })
                    .map_err(|error| format!("failed to write Windows clipboard image: {error}"))
            }
            (ClipboardFormat::Html, Some(html)) => backend
                .set_html(html, Some(clipboard.text.as_str()))
                .map_err(|error| format!("failed to write Windows clipboard html: {error}")),
            _ => backend
                .set_text(clipboard.text.clone())
                .map_err(|error| format!("failed to write Windows clipboard text: {error}")),
        }
    }

    fn wait_for_clipboard_change(
        &self,
        previous: &ClipboardText,
        _fallback_interval: Duration,
    ) -> Result<ClipboardText, String> {
        let mut sequence = windows_clipboard_sequence_number();
        loop {
            windows_wait_for_clipboard_notification(&mut sequence)?;
            let content = self.get_clipboard_content()?;
            if &content != previous {
                return Ok(content);
            }
        }
    }
}

#[allow(unsafe_code)]
fn windows_clipboard_sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

#[allow(unsafe_code)]
fn windows_wait_for_clipboard_notification(sequence: &mut u32) -> Result<(), String> {
    let class_name = windows_wide_null("KMSyncClipboardUpdateWindow");
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(clipboard_notification_window_proc),
        lpszClassName: class_name.as_ptr(),
        ..WNDCLASSW::default()
    };

    unsafe {
        RegisterClassW(&window_class);
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err("failed to create Windows clipboard notification window".to_string());
        }

        let _listener = ClipboardNotificationWindow::attach(hwnd)?;
        let current_sequence = windows_clipboard_sequence_number();
        if current_sequence != *sequence {
            *sequence = current_sequence;
            return Ok(());
        }

        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, hwnd, 0, 0);
            if result < 0 {
                return Err("failed to read Windows clipboard notification message".to_string());
            }
            if result == 0 {
                return Err("Windows clipboard notification window stopped".to_string());
            }
            if message.message == WM_CLIPBOARDUPDATE {
                *sequence = windows_clipboard_sequence_number();
                return Ok(());
            }
        }
    }
}

struct ClipboardNotificationWindow(HWND);

impl ClipboardNotificationWindow {
    #[allow(unsafe_code)]
    unsafe fn attach(hwnd: HWND) -> Result<Self, String> {
        if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
            unsafe {
                DestroyWindow(hwnd);
            }
            return Err("failed to attach Windows clipboard format listener".to_string());
        }
        Ok(Self(hwnd))
    }
}

impl Drop for ClipboardNotificationWindow {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            RemoveClipboardFormatListener(self.0);
            DestroyWindow(self.0);
        }
    }
}

#[allow(unsafe_code)]
unsafe extern "system" fn clipboard_notification_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn windows_wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn clipboard_text_from_image(
    image: ImageData<'static>,
    platform: &str,
) -> Result<ClipboardText, String> {
    let width = u32::try_from(image.width)
        .map_err(|_| format!("{platform} clipboard image width overflow"))?;
    let height = u32::try_from(image.height)
        .map_err(|_| format!("{platform} clipboard image height overflow"))?;
    Ok(ClipboardText::image(
        0,
        0,
        width,
        height,
        image.bytes.into_owned(),
    ))
}

#[cfg(test)]
const fn clipboard_backend_kind() -> crate::platform::ClipboardBackendKind {
    crate::platform::ClipboardBackendKind::NativeApi
}

fn inject_key(key: Key, state: KeyState) -> Result<(), String> {
    send_input(&[keyboard_input(key, state)?])
}

fn keyboard_input(key: Key, state: KeyState) -> Result<INPUT, String> {
    let scan = windows_scan_code(key).ok_or_else(|| format!("unsupported Windows key: {key:?}"))?;
    let mut flags = KEYEVENTF_SCANCODE;
    if scan.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if state == KeyState::Released {
        flags |= KEYEVENTF_KEYUP;
    }

    Ok(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan.code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: KMSYNC_INJECTED_EVENT_MARKER,
            },
        },
    })
}

fn inject_mouse_move(dx: f32, dy: f32) -> Result<(), String> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: dx.round() as i32,
                dy: dy.round() as i32,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: KMSYNC_INJECTED_EVENT_MARKER,
            },
        },
    };
    send_input(&[input])
}

fn inject_mouse_position(x_ratio: f32, y_ratio: f32) -> Result<(), String> {
    send_input(&[absolute_mouse_position_input(x_ratio, y_ratio)])
}

fn absolute_mouse_position_input(x_ratio: f32, y_ratio: f32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: normalized_absolute_coordinate(x_ratio),
                dy: normalized_absolute_coordinate(y_ratio),
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: KMSYNC_INJECTED_EVENT_MARKER,
            },
        },
    }
}

fn normalized_absolute_coordinate(ratio: f32) -> i32 {
    (ratio.clamp(0.0, 1.0) * 65_535.0).round() as i32
}

fn inject_mouse_button(button: MouseButton, state: KeyState) -> Result<(), String> {
    let (flags, data) = match (button, state) {
        (MouseButton::Left, KeyState::Pressed) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, KeyState::Released) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, KeyState::Pressed) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, KeyState::Released) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, KeyState::Pressed) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, KeyState::Released) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, KeyState::Pressed) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (MouseButton::Back, KeyState::Released) => (MOUSEEVENTF_XUP, XBUTTON1),
        (MouseButton::Forward, KeyState::Pressed) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (MouseButton::Forward, KeyState::Released) => (MOUSEEVENTF_XUP, XBUTTON2),
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: KMSYNC_INJECTED_EVENT_MARKER,
            },
        },
    };
    send_input(&[input])
}

fn inject_scroll(event: ScrollEvent) -> Result<(), String> {
    let mut inputs = [empty_mouse_input(), empty_mouse_input()];
    let count = build_scroll_inputs(event, &mut inputs);
    send_input(&inputs[..count])
}

fn build_scroll_inputs(event: ScrollEvent, inputs: &mut [INPUT; 2]) -> usize {
    const WHEEL_DELTA: i32 = 120;
    let mut count = 0;
    if event.dy != 0.0 {
        inputs[count] = mouse_input(
            (event.dy.round() as i32 * WHEEL_DELTA) as u32,
            MOUSEEVENTF_WHEEL,
        );
        count += 1;
    }
    if event.dx != 0.0 {
        inputs[count] = mouse_input(
            (event.dx.round() as i32 * WHEEL_DELTA) as u32,
            MOUSEEVENTF_HWHEEL,
        );
        count += 1;
    }
    count
}

fn empty_mouse_input() -> INPUT {
    mouse_input(0, 0)
}

fn mouse_input(mouse_data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: KMSYNC_INJECTED_EVENT_MARKER,
            },
        },
    }
}

#[allow(unsafe_code)]
fn send_input(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            i32::try_from(size_of::<INPUT>()).map_err(|_| "INPUT size overflow".to_string())?,
        )
    };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(format!("SendInput sent {sent}/{} events", inputs.len()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsScanCode {
    code: u16,
    extended: bool,
}

const fn scan(code: u16) -> WindowsScanCode {
    WindowsScanCode {
        code,
        extended: false,
    }
}

const fn extended_scan(code: u16) -> WindowsScanCode {
    WindowsScanCode {
        code,
        extended: true,
    }
}

fn windows_scan_code(key: Key) -> Option<WindowsScanCode> {
    Some(match key {
        Key::A => scan(0x1E),
        Key::B => scan(0x30),
        Key::C => scan(0x2E),
        Key::D => scan(0x20),
        Key::E => scan(0x12),
        Key::F => scan(0x21),
        Key::G => scan(0x22),
        Key::H => scan(0x23),
        Key::I => scan(0x17),
        Key::J => scan(0x24),
        Key::K => scan(0x25),
        Key::L => scan(0x26),
        Key::M => scan(0x32),
        Key::N => scan(0x31),
        Key::O => scan(0x18),
        Key::P => scan(0x19),
        Key::Q => scan(0x10),
        Key::R => scan(0x13),
        Key::S => scan(0x1F),
        Key::T => scan(0x14),
        Key::U => scan(0x16),
        Key::V => scan(0x2F),
        Key::W => scan(0x11),
        Key::X => scan(0x2D),
        Key::Y => scan(0x15),
        Key::Z => scan(0x2C),
        Key::Num0 => scan(0x0B),
        Key::Num1 => scan(0x02),
        Key::Num2 => scan(0x03),
        Key::Num3 => scan(0x04),
        Key::Num4 => scan(0x05),
        Key::Num5 => scan(0x06),
        Key::Num6 => scan(0x07),
        Key::Num7 => scan(0x08),
        Key::Num8 => scan(0x09),
        Key::Num9 => scan(0x0A),
        Key::Enter => scan(0x1C),
        Key::Escape => scan(0x01),
        Key::Backspace => scan(0x0E),
        Key::Tab => scan(0x0F),
        Key::Space => scan(0x39),
        Key::CapsLock => scan(0x3A),
        Key::PrintScreen => extended_scan(0x37),
        Key::ScrollLock => scan(0x46),
        Key::Pause => scan(0x45),
        Key::NumLock => extended_scan(0x45),
        Key::F1 => scan(0x3B),
        Key::F2 => scan(0x3C),
        Key::F3 => scan(0x3D),
        Key::F4 => scan(0x3E),
        Key::F5 => scan(0x3F),
        Key::F6 => scan(0x40),
        Key::F7 => scan(0x41),
        Key::F8 => scan(0x42),
        Key::F9 => scan(0x43),
        Key::F10 => scan(0x44),
        Key::F11 => scan(0x57),
        Key::F12 => scan(0x58),
        Key::Insert => extended_scan(0x52),
        Key::Home => extended_scan(0x47),
        Key::PageUp => extended_scan(0x49),
        Key::Delete => extended_scan(0x53),
        Key::End => extended_scan(0x4F),
        Key::PageDown => extended_scan(0x51),
        Key::ArrowRight => extended_scan(0x4D),
        Key::ArrowLeft => extended_scan(0x4B),
        Key::ArrowDown => extended_scan(0x50),
        Key::ArrowUp => extended_scan(0x48),
        Key::LeftControl => scan(0x1D),
        Key::RightControl => extended_scan(0x1D),
        Key::LeftShift => scan(0x2A),
        Key::RightShift => scan(0x36),
        Key::LeftAlt => scan(0x38),
        Key::RightAlt => extended_scan(0x38),
        Key::LeftMeta => extended_scan(0x5B),
        Key::RightMeta => extended_scan(0x5C),
        _ => return None,
    })
}

#[allow(unsafe_code)]
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let keyboard = &*(lparam as *const KBDLLHOOKSTRUCT);
        if is_self_injected_event(keyboard.dwExtraInfo) {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        }
        if let Some(key) = key_from_windows_hook(keyboard.vkCode, keyboard.scanCode, keyboard.flags)
        {
            let state = match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyState::Pressed),
                WM_KEYUP | WM_SYSKEYUP => Some(KeyState::Released),
                _ => None,
            };
            if let Some(state) = state {
                if emit_capture(CapturedInput {
                    event: InputEvent::Key(KeyEvent {
                        key,
                        state,
                        modifiers: windows_modifiers(),
                    }),
                    pointer: None,
                }) == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

#[allow(unsafe_code)]
unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let mouse = &*(lparam as *const MSLLHOOKSTRUCT);
        if is_self_injected_event(mouse.dwExtraInfo) {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        }
        match wparam as u32 {
            WM_MOUSEMOVE => {
                let current = (mouse.pt.x, mouse.pt.y);
                if let Some((dx, dy)) = record_mouse_position_delta(current) {
                    if dx != 0 || dy != 0 {
                        if emit_capture(CapturedInput {
                            event: InputEvent::Mouse(MouseEvent::Move {
                                dx: dx as f32,
                                dy: dy as f32,
                            }),
                            pointer: Some(PointerPosition {
                                x: f64::from(current.0),
                                y: f64::from(current.1),
                            }),
                        }) == CaptureDecision::Suppress
                        {
                            return 1;
                        }
                    }
                }
            }
            WM_LBUTTONDOWN => {
                if emit_mouse_capture(mouse, mouse_button(MouseButton::Left, KeyState::Pressed))
                    == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_LBUTTONUP => {
                if emit_mouse_capture(mouse, mouse_button(MouseButton::Left, KeyState::Released))
                    == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_RBUTTONDOWN => {
                if emit_mouse_capture(mouse, mouse_button(MouseButton::Right, KeyState::Pressed))
                    == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_RBUTTONUP => {
                if emit_mouse_capture(mouse, mouse_button(MouseButton::Right, KeyState::Released))
                    == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_MBUTTONDOWN => {
                if emit_mouse_capture(mouse, mouse_button(MouseButton::Middle, KeyState::Pressed))
                    == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_MBUTTONUP => {
                if emit_mouse_capture(mouse, mouse_button(MouseButton::Middle, KeyState::Released))
                    == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_XBUTTONDOWN => {
                if emit_mouse_capture(
                    mouse,
                    mouse_button(xbutton_from_data(mouse.mouseData), KeyState::Pressed),
                ) == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_XBUTTONUP => {
                if emit_mouse_capture(
                    mouse,
                    mouse_button(xbutton_from_data(mouse.mouseData), KeyState::Released),
                ) == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_MOUSEWHEEL => {
                if emit_mouse_capture(
                    mouse,
                    InputEvent::Scroll(ScrollEvent {
                        dx: 0.0,
                        dy: wheel_delta(mouse.mouseData),
                    }),
                ) == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            WM_MOUSEHWHEEL => {
                if emit_mouse_capture(
                    mouse,
                    InputEvent::Scroll(ScrollEvent {
                        dx: wheel_delta(mouse.mouseData),
                        dy: 0.0,
                    }),
                ) == CaptureDecision::Suppress
                {
                    return 1;
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

fn emit_mouse_capture(mouse: &MSLLHOOKSTRUCT, event: InputEvent) -> CaptureDecision {
    emit_capture(CapturedInput {
        event,
        pointer: Some(PointerPosition {
            x: f64::from(mouse.pt.x),
            y: f64::from(mouse.pt.y),
        }),
    })
}

const fn is_self_injected_event(extra_info: usize) -> bool {
    extra_info == KMSYNC_INJECTED_EVENT_MARKER
}

fn emit_capture(event: CapturedInput) -> CaptureDecision {
    CAPTURE_CALLBACK.with_borrow_mut(|slot| {
        if let Some(callback) = slot.as_mut() {
            callback(event)
        } else {
            CaptureDecision::Continue
        }
    })
}

fn set_capture_callback_for_current_thread(callback: CaptureCallback) {
    CAPTURE_CALLBACK.with_borrow_mut(|slot| {
        *slot = Some(callback);
    });
}

fn clear_capture_callback_for_current_thread() {
    CAPTURE_CALLBACK.with_borrow_mut(|slot| {
        *slot = None;
    });
}

fn record_mouse_position_delta(current: (i32, i32)) -> Option<(i32, i32)> {
    LAST_MOUSE_POS.with_borrow_mut(|last| {
        let delta = last.map(|previous| (current.0 - previous.0, current.1 - previous.1));
        *last = Some(current);
        delta
    })
}

fn clear_last_mouse_position_for_current_thread() {
    LAST_MOUSE_POS.with_borrow_mut(|last| {
        *last = None;
    });
}

const fn mouse_button(button: MouseButton, state: KeyState) -> InputEvent {
    InputEvent::Mouse(MouseEvent::Button { button, state })
}

fn wheel_delta(mouse_data: u32) -> f32 {
    let high_word = ((mouse_data >> 16) & 0xffff) as u16;
    let signed = i16::from_ne_bytes(high_word.to_ne_bytes());
    f32::from(signed) / 120.0
}

fn xbutton_from_data(mouse_data: u32) -> MouseButton {
    let high_word = (mouse_data >> 16) & 0xffff;
    if high_word == XBUTTON2 {
        MouseButton::Forward
    } else {
        MouseButton::Back
    }
}

#[allow(unsafe_code)]
fn windows_modifiers() -> Modifiers {
    let mut modifiers = Modifiers::NONE;
    if unsafe { GetKeyState(0x10) } < 0 {
        modifiers = modifiers.with(Modifiers::SHIFT);
    }
    if unsafe { GetKeyState(0x11) } < 0 {
        modifiers = modifiers.with(Modifiers::CONTROL);
    }
    if unsafe { GetKeyState(0x12) } < 0 {
        modifiers = modifiers.with(Modifiers::ALT);
    }
    if unsafe { GetKeyState(0x5B) } < 0 || unsafe { GetKeyState(0x5C) } < 0 {
        modifiers = modifiers.with(Modifiers::META);
    }
    modifiers
}

fn key_from_windows_hook(vk: u32, scan_code: u32, flags: u32) -> Option<Key> {
    const VK_SHIFT: u32 = 0x10;
    const VK_CONTROL: u32 = 0x11;
    const VK_MENU: u32 = 0x12;
    const VK_LSHIFT: u32 = 0xA0;
    const VK_RSHIFT: u32 = 0xA1;
    const VK_LCONTROL: u32 = 0xA2;
    const VK_RCONTROL: u32 = 0xA3;
    const VK_LMENU: u32 = 0xA4;
    const VK_RMENU: u32 = 0xA5;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    const SCAN_RIGHT_SHIFT: u32 = 0x36;

    match vk {
        VK_LSHIFT => Some(Key::LeftShift),
        VK_RSHIFT => Some(Key::RightShift),
        VK_LCONTROL => Some(Key::LeftControl),
        VK_RCONTROL => Some(Key::RightControl),
        VK_LMENU => Some(Key::LeftAlt),
        VK_RMENU => Some(Key::RightAlt),
        VK_LWIN => Some(Key::LeftMeta),
        VK_RWIN => Some(Key::RightMeta),
        VK_SHIFT if scan_code == SCAN_RIGHT_SHIFT => Some(Key::RightShift),
        VK_SHIFT => Some(Key::LeftShift),
        VK_CONTROL if flags & LLKHF_EXTENDED != 0 => Some(Key::RightControl),
        VK_CONTROL => Some(Key::LeftControl),
        VK_MENU if flags & LLKHF_EXTENDED != 0 => Some(Key::RightAlt),
        VK_MENU => Some(Key::LeftAlt),
        _ => key_from_windows_vk(vk),
    }
}

fn key_from_windows_vk(vk: u32) -> Option<Key> {
    Some(match vk {
        0x41 => Key::A,
        0x42 => Key::B,
        0x43 => Key::C,
        0x44 => Key::D,
        0x45 => Key::E,
        0x46 => Key::F,
        0x47 => Key::G,
        0x48 => Key::H,
        0x49 => Key::I,
        0x4A => Key::J,
        0x4B => Key::K,
        0x4C => Key::L,
        0x4D => Key::M,
        0x4E => Key::N,
        0x4F => Key::O,
        0x50 => Key::P,
        0x51 => Key::Q,
        0x52 => Key::R,
        0x53 => Key::S,
        0x54 => Key::T,
        0x55 => Key::U,
        0x56 => Key::V,
        0x57 => Key::W,
        0x58 => Key::X,
        0x59 => Key::Y,
        0x5A => Key::Z,
        0x30 => Key::Num0,
        0x31 => Key::Num1,
        0x32 => Key::Num2,
        0x33 => Key::Num3,
        0x34 => Key::Num4,
        0x35 => Key::Num5,
        0x36 => Key::Num6,
        0x37 => Key::Num7,
        0x38 => Key::Num8,
        0x39 => Key::Num9,
        0x0D => Key::Enter,
        0x1B => Key::Escape,
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x20 => Key::Space,
        0x14 => Key::CapsLock,
        0x15 => Key::Kana,
        0x16 => Key::ImeOn,
        0x1A => Key::ImeOff,
        0x1D => Key::Eisu,
        0x2C => Key::PrintScreen,
        0x90 => Key::NumLock,
        0x91 => Key::ScrollLock,
        0x13 => Key::Pause,
        0x70 => Key::F1,
        0x71 => Key::F2,
        0x72 => Key::F3,
        0x73 => Key::F4,
        0x74 => Key::F5,
        0x75 => Key::F6,
        0x76 => Key::F7,
        0x77 => Key::F8,
        0x78 => Key::F9,
        0x79 => Key::F10,
        0x7A => Key::F11,
        0x7B => Key::F12,
        0x2D => Key::Insert,
        0x24 => Key::Home,
        0x21 => Key::PageUp,
        0x2E => Key::Delete,
        0x23 => Key::End,
        0x22 => Key::PageDown,
        0x27 => Key::ArrowRight,
        0x25 => Key::ArrowLeft,
        0x28 => Key::ArrowDown,
        0x26 => Key::ArrowUp,
        0x11 => Key::LeftControl,
        0x10 => Key::LeftShift,
        0x12 => Key::LeftAlt,
        0x5B => Key::LeftMeta,
        0x5C => Key::RightMeta,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_clipboard_uses_native_api_backend() {
        assert_eq!(
            clipboard_backend_kind(),
            crate::platform::ClipboardBackendKind::NativeApi
        );
    }

    #[test]
    fn windows_clipboard_watch_uses_platform_notification() {
        assert_eq!(
            WindowsPlatform::new().clipboard_watch_backend(),
            crate::platform::ClipboardWatchBackend::PlatformNotification
        );
    }

    #[test]
    fn windows_permission_checks_cover_interactive_desktop() {
        let checks = WindowsPlatform::new().permission_checks();

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, "windows.interactive_desktop");
        assert_eq!(checks[0].status, PermissionStatus::Granted);
        assert!(checks[0].guidance.contains("interactive user session"));
        assert!(checks[0].guidance.contains("user-mode companion"));
    }

    #[test]
    #[allow(unsafe_code)]
    fn builds_vertical_and_horizontal_scroll_inputs_without_heap_buffer() {
        let mut inputs = [empty_mouse_input(), empty_mouse_input()];

        let count = build_scroll_inputs(ScrollEvent { dx: -2.0, dy: 1.0 }, &mut inputs);

        assert_eq!(count, 2);
        assert_eq!(unsafe { inputs[0].Anonymous.mi.dwFlags }, MOUSEEVENTF_WHEEL);
        assert_eq!(unsafe { inputs[0].Anonymous.mi.mouseData }, 120_u32);
        assert_eq!(
            unsafe { inputs[1].Anonymous.mi.dwFlags },
            MOUSEEVENTF_HWHEEL
        );
        assert_eq!(
            unsafe { inputs[1].Anonymous.mi.mouseData },
            (-240_i32) as u32
        );
    }

    #[test]
    fn ignores_zero_scroll_axes() {
        let mut inputs = [empty_mouse_input(), empty_mouse_input()];

        let count = build_scroll_inputs(ScrollEvent { dx: 0.0, dy: 0.0 }, &mut inputs);

        assert_eq!(count, 0);
    }

    #[test]
    fn capture_callback_dispatches_from_thread_local_slot() {
        set_capture_callback_for_current_thread(Box::new(|event| {
            if matches!(event.event, InputEvent::Key(_)) {
                CaptureDecision::Suppress
            } else {
                CaptureDecision::Continue
            }
        }));

        let decision = emit_capture(CapturedInput {
            event: InputEvent::Key(KeyEvent {
                key: Key::A,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            }),
            pointer: None,
        });

        clear_capture_callback_for_current_thread();
        assert_eq!(decision, CaptureDecision::Suppress);
    }

    #[test]
    fn mouse_move_delta_uses_thread_local_state() {
        clear_last_mouse_position_for_current_thread();

        assert_eq!(record_mouse_position_delta((10, 20)), None);
        assert_eq!(record_mouse_position_delta((13, 18)), Some((3, -2)));

        clear_last_mouse_position_for_current_thread();
        assert_eq!(record_mouse_position_delta((13, 18)), None);
    }

    #[test]
    fn distinguishes_left_and_right_windows_modifier_keys() {
        assert_eq!(key_from_windows_hook(0x11, 0x1d, 0), Some(Key::LeftControl));
        assert_eq!(
            key_from_windows_hook(0x11, 0x1d, LLKHF_EXTENDED),
            Some(Key::RightControl)
        );
        assert_eq!(key_from_windows_hook(0x10, 0x2a, 0), Some(Key::LeftShift));
        assert_eq!(key_from_windows_hook(0x10, 0x36, 0), Some(Key::RightShift));
        assert_eq!(key_from_windows_hook(0x12, 0x38, 0), Some(Key::LeftAlt));
        assert_eq!(
            key_from_windows_hook(0x12, 0x38, LLKHF_EXTENDED),
            Some(Key::RightAlt)
        );
        assert_eq!(key_from_windows_hook(0x5b, 0x5b, 0), Some(Key::LeftMeta));
        assert_eq!(key_from_windows_hook(0x5c, 0x5c, 0), Some(Key::RightMeta));
    }

    #[test]
    fn maps_caps_lock_ime_and_system_keys_for_capture_and_injection() {
        assert_eq!(key_from_windows_vk(0x14), Some(Key::CapsLock));
        assert_eq!(key_from_windows_vk(0x15), Some(Key::Kana));
        assert_eq!(key_from_windows_vk(0x16), Some(Key::ImeOn));
        assert_eq!(key_from_windows_vk(0x1A), Some(Key::ImeOff));
        assert_eq!(key_from_windows_vk(0x2C), Some(Key::PrintScreen));
        assert_eq!(key_from_windows_vk(0x91), Some(Key::ScrollLock));
        assert_eq!(key_from_windows_vk(0x13), Some(Key::Pause));
        assert_eq!(key_from_windows_vk(0x90), Some(Key::NumLock));
        assert_eq!(windows_scan_code(Key::CapsLock), Some(scan(0x3A)));
        assert_eq!(windows_scan_code(Key::ScrollLock), Some(scan(0x46)));
        assert_eq!(windows_scan_code(Key::NumLock), Some(extended_scan(0x45)));
    }

    #[test]
    #[allow(unsafe_code)]
    fn builds_keyboard_input_with_scan_code_for_physical_key_accuracy() {
        let input = keyboard_input(Key::A, KeyState::Pressed).expect("keyboard input");
        let keyboard = unsafe { input.Anonymous.ki };

        assert_eq!(input.r#type, INPUT_KEYBOARD);
        assert_eq!(keyboard.wVk, 0);
        assert_eq!(keyboard.wScan, 0x1e);
        assert_eq!(keyboard.dwFlags & KEYEVENTF_SCANCODE, KEYEVENTF_SCANCODE);
        assert_eq!(keyboard.dwFlags & KEYEVENTF_KEYUP, 0);
    }

    #[test]
    #[allow(unsafe_code)]
    fn builds_extended_scan_code_for_right_modifier_release() {
        let input = keyboard_input(Key::RightControl, KeyState::Released).expect("keyboard input");
        let keyboard = unsafe { input.Anonymous.ki };

        assert_eq!(keyboard.wVk, 0);
        assert_eq!(keyboard.wScan, 0x1d);
        assert_eq!(keyboard.dwFlags & KEYEVENTF_SCANCODE, KEYEVENTF_SCANCODE);
        assert_eq!(
            keyboard.dwFlags & KEYEVENTF_EXTENDEDKEY,
            KEYEVENTF_EXTENDEDKEY
        );
        assert_eq!(keyboard.dwFlags & KEYEVENTF_KEYUP, KEYEVENTF_KEYUP);
    }

    #[test]
    #[allow(unsafe_code)]
    fn injected_keyboard_and_mouse_inputs_are_marked() {
        let keyboard = keyboard_input(Key::A, KeyState::Pressed).expect("keyboard input");
        let mouse = mouse_input(0, MOUSEEVENTF_MOVE);

        assert_eq!(
            unsafe { keyboard.Anonymous.ki.dwExtraInfo },
            KMSYNC_INJECTED_EVENT_MARKER
        );
        assert_eq!(
            unsafe { mouse.Anonymous.mi.dwExtraInfo },
            KMSYNC_INJECTED_EVENT_MARKER
        );
    }

    #[test]
    fn filters_self_injected_hook_events() {
        assert!(is_self_injected_event(KMSYNC_INJECTED_EVENT_MARKER));
        assert!(!is_self_injected_event(KMSYNC_INJECTED_EVENT_MARKER + 1));
    }

    #[test]
    fn extracts_foreground_process_name_for_clipboard_policy() {
        assert_eq!(
            process_image_file_name(r"C:\Program Files\Bitwarden\Bitwarden.exe").as_deref(),
            Some("Bitwarden.exe")
        );
        assert_eq!(
            process_image_file_name("/Applications/1Password.app").as_deref(),
            Some("1Password.app")
        );
        assert_eq!(process_image_file_name(""), None);
    }
}
