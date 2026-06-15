use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::c_char;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr;
use std::thread;
use std::time::Duration;

use arboard::ImageData;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::mach_port::{CFMachPort, CFMachPortInvalidate, CFMachPortRef};
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGMouseButton, CGScrollEventUnit, CallbackResult, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use core_graphics::sys::{CGEventRef, CGEventSourceRef};
use foreign_types::ForeignType;
use kmsync_core::{
    ClipboardFormat, ClipboardText, InputEvent, Key, KeyEvent, KeyState, Modifiers, MouseButton,
    MouseEvent, OsKind, ScrollEvent,
};

use super::{
    CaptureDecision, CapturedInput, ClipboardBackend, ClipboardWatchBackend, DisplayBounds,
    DisplayLayout, InputCaptureBackend, InputInjector, PermissionStatus, PlatformAdapter,
    PlatformCapabilities, PlatformPermissionCheck, PointerPosition, RemotePointerState,
};

pub struct MacOsPlatform {
    event_source: Option<CGEventSource>,
    remote_pointer: RemotePointerState,
}

impl MacOsPlatform {
    #[must_use]
    pub fn new() -> Self {
        Self {
            event_source: None,
            remote_pointer: RemotePointerState::default(),
        }
    }

    fn event_source(&mut self) -> Result<CGEventSource, String> {
        if self.event_source.is_none() {
            self.event_source = Some(
                CGEventSource::new(CGEventSourceStateID::HIDSystemState)
                    .map_err(|()| "failed to create CGEventSource".to_string())?,
            );
        }
        self.event_source
            .as_ref()
            .cloned()
            .ok_or_else(|| "failed to cache CGEventSource".to_string())
    }
}

pub fn hide_local_pointer() {
    let _ = CGDisplay::main().hide_cursor();
}

pub fn restore_local_pointer(position: Option<PointerPosition>) {
    if let Some(position) = position {
        let _ = CGDisplay::warp_mouse_cursor_position(pointer_to_point(position));
    }
    let _ = CGDisplay::main().show_cursor();
}

pub fn request_platform_permissions() {
    let _ = request_macos_accessibility_permission();
    let _ = request_macos_input_monitoring_permission();
}

impl PlatformAdapter for MacOsPlatform {
    fn os_kind(&self) -> OsKind {
        OsKind::MacOs
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            input_capture: true,
            input_injection: true,
            clipboard_text: true,
        }
    }

    fn permission_checks(&self) -> Vec<PlatformPermissionCheck> {
        vec![
            PlatformPermissionCheck {
                id: "macos.accessibility",
                label: "macOS Accessibility",
                status: permission_status_from_bool(macos_accessibility_trusted()),
                guidance: "Grant Accessibility permission to KMSync so it can inject keyboard and mouse events.",
            },
            PlatformPermissionCheck {
                id: "macos.input_monitoring",
                label: "macOS Input Monitoring",
                status: permission_status_from_bool(macos_input_monitoring_granted()),
                guidance: "Grant Input Monitoring permission to KMSync so it can capture global keyboard and mouse events.",
            },
        ]
    }

    fn permission_hints(&self) -> &'static [&'static str] {
        &[
            "Enable Accessibility for input injection.",
            "Enable Input Monitoring for global keyboard capture.",
        ]
    }

    fn primary_display_bounds(&self) -> Option<DisplayBounds> {
        let bounds = CGDisplay::main().bounds();
        Some(DisplayBounds {
            x: bounds.origin.x,
            y: bounds.origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        })
    }

    fn display_layout(&self) -> DisplayLayout {
        macos_display_layout()
            .unwrap_or_else(|| DisplayLayout::from_primary(self.primary_display_bounds()))
    }
}

const fn permission_status_from_bool(granted: bool) -> PermissionStatus {
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Missing
    }
}

#[allow(unsafe_code)]
fn macos_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

#[allow(unsafe_code)]
fn macos_input_monitoring_granted() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

#[allow(unsafe_code)]
fn request_macos_accessibility_permission() -> bool {
    unsafe {
        let prompt_key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let prompt_value = CFBoolean::true_value();
        let options = CFDictionary::from_CFType_pairs(&[(prompt_key, prompt_value)]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

#[allow(unsafe_code)]
fn request_macos_input_monitoring_permission() -> bool {
    unsafe { CGRequestListenEventAccess() }
}

#[allow(unsafe_code)]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    #[allow(non_upper_case_globals)]
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

fn macos_display_layout() -> Option<DisplayLayout> {
    let displays = CGDisplay::active_displays()
        .ok()?
        .into_iter()
        .map(|display_id| {
            let bounds = CGDisplay::new(display_id).bounds();
            DisplayBounds {
                x: bounds.origin.x,
                y: bounds.origin.y,
                width: bounds.size.width,
                height: bounds.size.height,
            }
        })
        .collect::<Vec<_>>();
    if displays.is_empty() {
        None
    } else {
        Some(DisplayLayout::new(displays))
    }
}

impl InputInjector for MacOsPlatform {
    fn inject(&mut self, event: InputEvent) -> Result<(), String> {
        let source = self.event_source()?;
        match event {
            InputEvent::Key(event) => inject_key(source, event.key, event.state),
            InputEvent::Mouse(MouseEvent::Move { dx, dy }) => {
                inject_mouse_move(source, &mut self.remote_pointer, dx, dy)
            }
            InputEvent::Mouse(MouseEvent::Position { x_ratio, y_ratio }) => {
                inject_mouse_position(source, &mut self.remote_pointer, x_ratio, y_ratio)
            }
            InputEvent::Mouse(MouseEvent::Button { button, state }) => {
                inject_mouse_button(source, &mut self.remote_pointer, button, state)
            }
            InputEvent::Scroll(event) => inject_scroll(source, event),
        }
    }
}

impl InputCaptureBackend for MacOsPlatform {
    fn capture_loop<F>(&mut self, callback: F) -> Result<(), String>
    where
        F: FnMut(CapturedInput) -> CaptureDecision + Send + 'static,
    {
        if !macos_input_monitoring_granted() {
            return Err(macos_input_monitoring_required_error());
        }
        if !macos_accessibility_trusted() {
            return Err(macos_event_tap_required_error());
        }

        let callback = RefCell::new(callback);
        with_enabled_macos_capture_tap(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            macos_capture_event_mask(),
            move |_proxy, event_type, event| {
                if let Some(input_event) = mac_capture_event(event_type, event) {
                    if callback.borrow_mut()(input_event) == CaptureDecision::Suppress {
                        return CallbackResult::Drop;
                    }
                }
                CallbackResult::Keep
            },
            CFRunLoop::run_current,
        )
        .map_err(|()| macos_event_tap_required_error())
    }
}

fn macos_input_monitoring_required_error() -> String {
    "macOS Input Monitoring permission is missing for KMSync.app; open KMSync, click 申请权限, grant Input Monitoring, then restart KMSync".to_string()
}

fn macos_event_tap_required_error() -> String {
    "failed to install macOS event tap; grant Accessibility and Input Monitoring to KMSync.app, make sure the core service is launched from /Applications/KMSync.app, then restart KMSync".to_string()
}

const MACOS_CAPTURE_EVENT_MASK: CGEventMask = event_mask_bit(CGEventType::KeyDown)
    | event_mask_bit(CGEventType::KeyUp)
    | event_mask_bit(CGEventType::FlagsChanged)
    | event_mask_bit(CGEventType::MouseMoved)
    | event_mask_bit(CGEventType::LeftMouseDown)
    | event_mask_bit(CGEventType::LeftMouseUp)
    | event_mask_bit(CGEventType::RightMouseDown)
    | event_mask_bit(CGEventType::RightMouseUp)
    | event_mask_bit(CGEventType::OtherMouseDown)
    | event_mask_bit(CGEventType::OtherMouseUp)
    | event_mask_bit(CGEventType::ScrollWheel);

const fn event_mask_bit(event_type: CGEventType) -> CGEventMask {
    1_u64 << (event_type as CGEventMask)
}

const fn macos_capture_event_mask() -> CGEventMask {
    MACOS_CAPTURE_EVENT_MASK
}

#[allow(unsafe_code)]
fn with_enabled_macos_capture_tap<R>(
    tap: CGEventTapLocation,
    place: CGEventTapPlacement,
    options: CGEventTapOptions,
    event_mask: CGEventMask,
    callback: impl Fn(CGEventTapProxy, CGEventType, &CGEvent) -> CallbackResult + 'static,
    with_fn: impl FnOnce() -> R,
) -> Result<R, ()> {
    let event_tap = MacEventTap::new(tap, place, options, event_mask, callback)?;
    let loop_source = event_tap
        .mach_port()
        .create_runloop_source(0)
        .expect("Runloop source creation failed");
    CFRunLoop::get_current().add_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    event_tap.enable();
    Ok(with_fn())
}

type MacEventTapCallback = Box<dyn Fn(CGEventTapProxy, CGEventType, &CGEvent) -> CallbackResult>;

type MacEventTapCallbackInternal = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    etype: CGEventType,
    event: CGEventRef,
    user_info: *const c_void,
) -> CGEventRef;

struct MacEventTap {
    mach_port: CFMachPort,
    _callback: Box<MacEventTapCallback>,
}

impl MacEventTap {
    #[allow(unsafe_code)]
    fn new(
        tap: CGEventTapLocation,
        place: CGEventTapPlacement,
        options: CGEventTapOptions,
        event_mask: CGEventMask,
        callback: impl Fn(CGEventTapProxy, CGEventType, &CGEvent) -> CallbackResult + 'static,
    ) -> Result<Self, ()> {
        let boxed_callback: Box<MacEventTapCallback> = Box::new(Box::new(callback));
        let callback_ptr = Box::into_raw(boxed_callback);
        let event_tap_ref = unsafe {
            CGEventTapCreate(
                tap,
                place,
                options,
                event_mask,
                mac_event_tap_callback_internal,
                callback_ptr.cast(),
            )
        };

        if !event_tap_ref.is_null() {
            Ok(Self {
                mach_port: unsafe { CFMachPort::wrap_under_create_rule(event_tap_ref) },
                _callback: unsafe { Box::from_raw(callback_ptr) },
            })
        } else {
            let _ = unsafe { Box::from_raw(callback_ptr) };
            Err(())
        }
    }

    fn mach_port(&self) -> &CFMachPort {
        &self.mach_port
    }

    #[allow(unsafe_code)]
    fn enable(&self) {
        unsafe { CGEventTapEnable(self.mach_port.as_concrete_TypeRef(), true) };
    }
}

impl Drop for MacEventTap {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe { CFMachPortInvalidate(self.mach_port.as_concrete_TypeRef()) };
    }
}

#[allow(unsafe_code)]
unsafe extern "C" fn mac_event_tap_callback_internal(
    proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    user_info: *const c_void,
) -> CGEventRef {
    let callback = user_info.cast::<MacEventTapCallback>();
    let event = ManuallyDrop::new(CGEvent::from_ptr(event));
    match (*callback)(proxy, event_type, &event) {
        CallbackResult::Keep => event.as_ptr(),
        CallbackResult::Drop => ptr::null_mut(),
        CallbackResult::Replace(new_event) => ManuallyDrop::new(new_event).as_ptr(),
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: CGEventTapLocation,
        place: CGEventTapPlacement,
        options: CGEventTapOptions,
        events_of_interest: CGEventMask,
        callback: MacEventTapCallbackInternal,
        user_info: *const c_void,
    ) -> CFMachPortRef;

    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

impl ClipboardBackend for MacOsPlatform {
    fn get_clipboard_text(&self) -> Result<String, String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open macOS pasteboard: {error}"))?;
        clipboard
            .get_text()
            .map_err(|error| format!("failed to read macOS pasteboard text: {error}"))
    }

    fn set_clipboard_text(&mut self, text: &str) -> Result<(), String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open macOS pasteboard: {error}"))?;
        clipboard
            .set_text(text.to_string())
            .map_err(|error| format!("failed to write macOS pasteboard text: {error}"))
    }

    fn clipboard_watch_backend(&self) -> ClipboardWatchBackend {
        ClipboardWatchBackend::NativeChangeCounter
    }

    fn get_clipboard_content(&self) -> Result<ClipboardText, String> {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open macOS pasteboard: {error}"))?;
        if let Ok(image) = clipboard.get_image() {
            return clipboard_text_from_image(image, "macOS");
        }
        let html = clipboard.get().html().ok().filter(|html| !html.is_empty());
        let text = clipboard
            .get_text()
            .or_else(|error| html.as_ref().map_or(Err(error), |html| Ok(html.clone())))
            .map_err(|error| format!("failed to read macOS pasteboard text: {error}"))?;
        Ok(match html {
            Some(html) => ClipboardText::html(0, 0, html, text),
            None => ClipboardText::from_local_text(0, 0, text),
        })
    }

    fn set_clipboard_content(&mut self, clipboard: &ClipboardText) -> Result<(), String> {
        let mut backend = arboard::Clipboard::new()
            .map_err(|error| format!("failed to open macOS pasteboard: {error}"))?;
        match (clipboard.format, clipboard.html.as_deref()) {
            (ClipboardFormat::Image, _) => {
                let image = clipboard
                    .image
                    .as_ref()
                    .ok_or_else(|| "macOS pasteboard image is missing pixel data".to_string())?;
                backend
                    .set_image(ImageData {
                        width: usize::try_from(image.width)
                            .map_err(|_| "macOS pasteboard image width overflow".to_string())?,
                        height: usize::try_from(image.height)
                            .map_err(|_| "macOS pasteboard image height overflow".to_string())?,
                        bytes: Cow::Borrowed(image.rgba.as_slice()),
                    })
                    .map_err(|error| format!("failed to write macOS pasteboard image: {error}"))
            }
            (ClipboardFormat::Html, Some(html)) => backend
                .set_html(html, Some(clipboard.text.as_str()))
                .map_err(|error| format!("failed to write macOS pasteboard html: {error}")),
            _ => backend
                .set_text(clipboard.text.clone())
                .map_err(|error| format!("failed to write macOS pasteboard text: {error}")),
        }
    }

    fn wait_for_clipboard_change(
        &self,
        previous: &ClipboardText,
        fallback_interval: Duration,
    ) -> Result<ClipboardText, String> {
        let mut change_count = macos_pasteboard_change_count()?;
        let sleep_for = if fallback_interval.is_zero() {
            Duration::from_millis(50)
        } else {
            fallback_interval.min(Duration::from_millis(250))
        };
        loop {
            let next_count = macos_pasteboard_change_count()?;
            if next_count != change_count {
                change_count = next_count;
                let content = self.get_clipboard_content()?;
                if &content != previous {
                    return Ok(content);
                }
            }
            thread::sleep(sleep_for);
        }
    }
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    #[link_name = "objc_msgSend"]
    fn objc_msg_send();
}

#[allow(unsafe_code)]
fn macos_pasteboard_change_count() -> Result<isize, String> {
    unsafe {
        let class = objc_getClass(c"NSPasteboard".as_ptr());
        if class.is_null() {
            return Err("failed to resolve NSPasteboard class".to_string());
        }
        let send_id: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msg_send as unsafe extern "C" fn());
        let send_isize: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize =
            std::mem::transmute(objc_msg_send as unsafe extern "C" fn());
        let general_pasteboard = sel_registerName(c"generalPasteboard".as_ptr());
        let change_count = sel_registerName(c"changeCount".as_ptr());
        let pasteboard = send_id(class, general_pasteboard);
        if pasteboard.is_null() {
            return Err("failed to access macOS general pasteboard".to_string());
        }
        Ok(send_isize(pasteboard, change_count))
    }
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

fn inject_key(source: CGEventSource, key: Key, state: KeyState) -> Result<(), String> {
    let keycode = mac_keycode(key).ok_or_else(|| format!("unsupported macOS key: {key:?}"))?;
    let event = CGEvent::new_keyboard_event(source, keycode, state == KeyState::Pressed)
        .map_err(|()| "failed to create keyboard event".to_string())?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn inject_mouse_move(
    source: CGEventSource,
    pointer: &mut RemotePointerState,
    dx: f32,
    dy: f32,
) -> Result<(), String> {
    ensure_remote_pointer(&source, pointer)?;
    let current = pointer
        .apply_delta(dx, dy)
        .ok_or_else(|| "failed to update remote pointer".to_string())?;
    let event = CGEvent::new_mouse_event(
        source,
        CGEventType::MouseMoved,
        pointer_to_point(current),
        CGMouseButton::Left,
    )
    .map_err(|()| "failed to create mouse move event".to_string())?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn inject_mouse_position(
    source: CGEventSource,
    pointer: &mut RemotePointerState,
    x_ratio: f32,
    y_ratio: f32,
) -> Result<(), String> {
    let bounds = macos_display_layout()
        .and_then(|layout| layout.virtual_bounds())
        .ok_or_else(|| "failed to read macOS display bounds for pointer position".to_string())?;
    let position = normalized_pointer_position(bounds, x_ratio, y_ratio);
    pointer.set(position);
    let event = CGEvent::new_mouse_event(
        source,
        CGEventType::MouseMoved,
        pointer_to_point(position),
        CGMouseButton::Left,
    )
    .map_err(|()| "failed to create mouse position event".to_string())?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn inject_mouse_button(
    source: CGEventSource,
    pointer: &mut RemotePointerState,
    button: MouseButton,
    state: KeyState,
) -> Result<(), String> {
    let current = ensure_remote_pointer(&source, pointer)?;
    let (event_type, cg_button) = match (button, state) {
        (MouseButton::Left, KeyState::Pressed) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
        (MouseButton::Left, KeyState::Released) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
        (MouseButton::Right, KeyState::Pressed) => {
            (CGEventType::RightMouseDown, CGMouseButton::Right)
        }
        (MouseButton::Right, KeyState::Released) => {
            (CGEventType::RightMouseUp, CGMouseButton::Right)
        }
        (_, KeyState::Pressed) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
        (_, KeyState::Released) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
    };
    let event = CGEvent::new_mouse_event(source, event_type, pointer_to_point(current), cg_button)
        .map_err(|()| "failed to create mouse button event".to_string())?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn ensure_remote_pointer(
    source: &CGEventSource,
    pointer: &mut RemotePointerState,
) -> Result<PointerPosition, String> {
    if let Some(position) = pointer.current() {
        return Ok(position);
    }

    let current = CGEvent::new(source.clone())
        .map_err(|()| "failed to create current mouse event".to_string())?
        .location();
    let position = PointerPosition {
        x: current.x,
        y: current.y,
    };
    pointer.set(position);
    Ok(position)
}

fn pointer_to_point(position: PointerPosition) -> CGPoint {
    CGPoint::new(position.x, position.y)
}

fn normalized_pointer_position(
    bounds: DisplayBounds,
    x_ratio: f32,
    y_ratio: f32,
) -> PointerPosition {
    PointerPosition {
        x: bounds.x + bounds.width * f64::from(x_ratio.clamp(0.0, 1.0)),
        y: bounds.y + bounds.height * f64::from(y_ratio.clamp(0.0, 1.0)),
    }
}

fn inject_scroll(source: CGEventSource, event: ScrollEvent) -> Result<(), String> {
    let scroll = new_scroll_event(
        &source,
        ScrollEventUnit::LINE,
        2,
        event.dy.round() as i32,
        event.dx.round() as i32,
        0,
    )
    .map_err(|()| "failed to create scroll event".to_string())?;
    scroll.post(CGEventTapLocation::HID);
    Ok(())
}

fn mac_capture_event(event_type: CGEventType, event: &CGEvent) -> Option<CapturedInput> {
    let pointer = {
        let location = event.location();
        Some(PointerPosition {
            x: location.x,
            y: location.y,
        })
    };
    let input = match event_type {
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let key = key_from_mac_keycode(u16::try_from(keycode).ok()?)?;
            InputEvent::Key(KeyEvent {
                key,
                state: if matches!(event_type, CGEventType::KeyDown) {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                modifiers: modifiers_from_macos(event.get_flags()),
            })
        }
        CGEventType::FlagsChanged => {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let key = key_from_mac_keycode(u16::try_from(keycode).ok()?)?;
            InputEvent::Key(KeyEvent {
                key,
                state: if modifier_is_active(event.get_flags(), key) {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
                modifiers: modifiers_from_macos(event.get_flags()),
            })
        }
        CGEventType::MouseMoved => InputEvent::Mouse(MouseEvent::Move {
            dx: event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as f32,
            dy: event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as f32,
        }),
        CGEventType::LeftMouseDown => InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Left,
            state: KeyState::Pressed,
        }),
        CGEventType::LeftMouseUp => InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Left,
            state: KeyState::Released,
        }),
        CGEventType::RightMouseDown => InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Right,
            state: KeyState::Pressed,
        }),
        CGEventType::RightMouseUp => InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Right,
            state: KeyState::Released,
        }),
        CGEventType::OtherMouseDown => InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Middle,
            state: KeyState::Pressed,
        }),
        CGEventType::OtherMouseUp => InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Middle,
            state: KeyState::Released,
        }),
        CGEventType::ScrollWheel => InputEvent::Scroll(ScrollEvent {
            dx: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2) as f32,
            dy: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1) as f32,
        }),
        _ => return None,
    };
    Some(CapturedInput {
        event: input,
        pointer,
    })
}

fn modifiers_from_macos(flags: CGEventFlags) -> Modifiers {
    let mut modifiers = Modifiers::NONE;
    if flags.contains(CGEventFlags::CGEventFlagShift) {
        modifiers = modifiers.with(Modifiers::SHIFT);
    }
    if flags.contains(CGEventFlags::CGEventFlagControl) {
        modifiers = modifiers.with(Modifiers::CONTROL);
    }
    if flags.contains(CGEventFlags::CGEventFlagAlternate) {
        modifiers = modifiers.with(Modifiers::ALT);
    }
    if flags.contains(CGEventFlags::CGEventFlagCommand) {
        modifiers = modifiers.with(Modifiers::META);
    }
    modifiers
}

fn modifier_is_active(flags: CGEventFlags, key: Key) -> bool {
    match key {
        Key::LeftShift | Key::RightShift => flags.contains(CGEventFlags::CGEventFlagShift),
        Key::LeftControl | Key::RightControl => flags.contains(CGEventFlags::CGEventFlagControl),
        Key::LeftAlt | Key::RightAlt => flags.contains(CGEventFlags::CGEventFlagAlternate),
        Key::LeftMeta | Key::RightMeta => flags.contains(CGEventFlags::CGEventFlagCommand),
        _ => false,
    }
}

#[allow(unsafe_code)]
fn new_scroll_event(
    source: &CGEventSource,
    units: CGScrollEventUnit,
    wheel_count: u32,
    wheel1: i32,
    wheel2: i32,
    wheel3: i32,
) -> Result<CGEvent, ()> {
    let event_ref = unsafe {
        CGEventCreateScrollWheelEvent2(source.as_ptr(), units, wheel_count, wheel1, wheel2, wheel3)
    };
    if event_ref.is_null() {
        Err(())
    } else {
        Ok(unsafe { CGEvent::from_ptr(event_ref) })
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: CGScrollEventUnit,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
}

fn mac_keycode(key: Key) -> Option<u16> {
    Some(match key {
        Key::A => 0x00,
        Key::S => 0x01,
        Key::D => 0x02,
        Key::F => 0x03,
        Key::H => 0x04,
        Key::G => 0x05,
        Key::Z => 0x06,
        Key::X => 0x07,
        Key::C => 0x08,
        Key::V => 0x09,
        Key::B => 0x0B,
        Key::Q => 0x0C,
        Key::W => 0x0D,
        Key::E => 0x0E,
        Key::R => 0x0F,
        Key::Y => 0x10,
        Key::T => 0x11,
        Key::Num1 => 0x12,
        Key::Num2 => 0x13,
        Key::Num3 => 0x14,
        Key::Num4 => 0x15,
        Key::Num6 => 0x16,
        Key::Num5 => 0x17,
        Key::Equal => 0x18,
        Key::Num9 => 0x19,
        Key::Num7 => 0x1A,
        Key::Minus => 0x1B,
        Key::Num8 => 0x1C,
        Key::Num0 => 0x1D,
        Key::RightBracket => 0x1E,
        Key::O => 0x1F,
        Key::U => 0x20,
        Key::LeftBracket => 0x21,
        Key::I => 0x22,
        Key::P => 0x23,
        Key::Enter => 0x24,
        Key::L => 0x25,
        Key::J => 0x26,
        Key::Quote => 0x27,
        Key::K => 0x28,
        Key::Semicolon => 0x29,
        Key::Backslash => 0x2A,
        Key::Comma => 0x2B,
        Key::Slash => 0x2C,
        Key::N => 0x2D,
        Key::M => 0x2E,
        Key::Dot => 0x2F,
        Key::Tab => 0x30,
        Key::Space => 0x31,
        Key::Grave => 0x32,
        Key::Backspace => 0x33,
        Key::Escape => 0x35,
        Key::LeftMeta => 0x37,
        Key::LeftShift => 0x38,
        Key::CapsLock => 0x39,
        Key::LeftAlt => 0x3A,
        Key::LeftControl => 0x3B,
        Key::RightShift => 0x3C,
        Key::RightAlt => 0x3D,
        Key::RightControl => 0x3E,
        Key::RightMeta => 0x36,
        Key::F1 => 0x7A,
        Key::F2 => 0x78,
        Key::F3 => 0x63,
        Key::F4 => 0x76,
        Key::F5 => 0x60,
        Key::F6 => 0x61,
        Key::F7 => 0x62,
        Key::F8 => 0x64,
        Key::F9 => 0x65,
        Key::F10 => 0x6D,
        Key::F11 => 0x67,
        Key::F12 => 0x6F,
        Key::Eisu => 0x66,
        Key::Kana => 0x68,
        Key::Home => 0x73,
        Key::PageUp => 0x74,
        Key::Delete => 0x75,
        Key::End => 0x77,
        Key::PageDown => 0x79,
        Key::ArrowLeft => 0x7B,
        Key::ArrowRight => 0x7C,
        Key::ArrowDown => 0x7D,
        Key::ArrowUp => 0x7E,
        _ => return None,
    })
}

fn key_from_mac_keycode(keycode: u16) -> Option<Key> {
    Some(match keycode {
        0x00 => Key::A,
        0x01 => Key::S,
        0x02 => Key::D,
        0x03 => Key::F,
        0x04 => Key::H,
        0x05 => Key::G,
        0x06 => Key::Z,
        0x07 => Key::X,
        0x08 => Key::C,
        0x09 => Key::V,
        0x0B => Key::B,
        0x0C => Key::Q,
        0x0D => Key::W,
        0x0E => Key::E,
        0x0F => Key::R,
        0x10 => Key::Y,
        0x11 => Key::T,
        0x12 => Key::Num1,
        0x13 => Key::Num2,
        0x14 => Key::Num3,
        0x15 => Key::Num4,
        0x16 => Key::Num6,
        0x17 => Key::Num5,
        0x18 => Key::Equal,
        0x19 => Key::Num9,
        0x1A => Key::Num7,
        0x1B => Key::Minus,
        0x1C => Key::Num8,
        0x1D => Key::Num0,
        0x1E => Key::RightBracket,
        0x1F => Key::O,
        0x20 => Key::U,
        0x21 => Key::LeftBracket,
        0x22 => Key::I,
        0x23 => Key::P,
        0x24 => Key::Enter,
        0x25 => Key::L,
        0x26 => Key::J,
        0x27 => Key::Quote,
        0x28 => Key::K,
        0x29 => Key::Semicolon,
        0x2A => Key::Backslash,
        0x2B => Key::Comma,
        0x2C => Key::Slash,
        0x2D => Key::N,
        0x2E => Key::M,
        0x2F => Key::Dot,
        0x30 => Key::Tab,
        0x31 => Key::Space,
        0x32 => Key::Grave,
        0x33 => Key::Backspace,
        0x35 => Key::Escape,
        0x36 => Key::RightMeta,
        0x37 => Key::LeftMeta,
        0x38 => Key::LeftShift,
        0x39 => Key::CapsLock,
        0x3A => Key::LeftAlt,
        0x3B => Key::LeftControl,
        0x3C => Key::RightShift,
        0x3D => Key::RightAlt,
        0x3E => Key::RightControl,
        0x7A => Key::F1,
        0x78 => Key::F2,
        0x63 => Key::F3,
        0x76 => Key::F4,
        0x60 => Key::F5,
        0x61 => Key::F6,
        0x62 => Key::F7,
        0x64 => Key::F8,
        0x65 => Key::F9,
        0x6D => Key::F10,
        0x67 => Key::F11,
        0x6F => Key::F12,
        0x66 => Key::Eisu,
        0x68 => Key::Kana,
        0x73 => Key::Home,
        0x74 => Key::PageUp,
        0x75 => Key::Delete,
        0x77 => Key::End,
        0x79 => Key::PageDown,
        0x7B => Key::ArrowLeft,
        0x7C => Key::ArrowRight,
        0x7D => Key::ArrowDown,
        0x7E => Key::ArrowUp,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_CAPTURE_EVENT_TYPES: [CGEventType; 11] = [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::ScrollWheel,
    ];

    #[test]
    fn macos_clipboard_uses_native_api_backend() {
        assert_eq!(
            clipboard_backend_kind(),
            crate::platform::ClipboardBackendKind::NativeApi
        );
    }

    #[test]
    fn macos_clipboard_watch_uses_native_change_counter() {
        assert_eq!(
            MacOsPlatform::new().clipboard_watch_backend(),
            crate::platform::ClipboardWatchBackend::NativeChangeCounter
        );
    }

    #[test]
    fn macos_permission_status_maps_preflight_results() {
        assert_eq!(permission_status_from_bool(true), PermissionStatus::Granted);
        assert_eq!(
            permission_status_from_bool(false),
            PermissionStatus::Missing
        );
    }

    #[test]
    fn macos_input_monitoring_error_names_app_permission() {
        let error = macos_input_monitoring_required_error();

        assert!(error.contains("Input Monitoring"));
        assert!(error.contains("KMSync.app"));
    }

    #[test]
    fn macos_event_tap_error_names_both_privacy_permissions() {
        let error = macos_event_tap_required_error();

        assert!(error.contains("Accessibility"));
        assert!(error.contains("Input Monitoring"));
        assert!(error.contains("restart KMSync"));
    }

    #[test]
    fn macos_capture_event_mask_matches_static_event_set() {
        let expected = EXPECTED_CAPTURE_EVENT_TYPES
            .iter()
            .fold(0, |mask, event_type| mask | (1_u64 << *event_type as u64));

        assert_eq!(macos_capture_event_mask(), expected);
    }

    #[test]
    fn maps_caps_lock_and_jis_input_method_keys_for_capture_and_injection() {
        assert_eq!(mac_keycode(Key::CapsLock), Some(0x39));
        assert_eq!(mac_keycode(Key::Eisu), Some(0x66));
        assert_eq!(mac_keycode(Key::Kana), Some(0x68));
        assert_eq!(key_from_mac_keycode(0x39), Some(Key::CapsLock));
        assert_eq!(key_from_mac_keycode(0x66), Some(Key::Eisu));
        assert_eq!(key_from_mac_keycode(0x68), Some(Key::Kana));
    }
}
