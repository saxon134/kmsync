pub mod desktop;
pub mod event;
pub mod input_state;
pub mod keymap;
pub mod local_ipc;
pub mod profile;
pub mod protocol;

pub use desktop::{
    DesktopConnectionState, DesktopDeviceState, DesktopDisplayState, DesktopLayout,
    DesktopNetworkState, DesktopPeerState, DesktopPermissionState, DesktopRole, DesktopState,
    DesktopSyncRuntimeKind, DesktopSyncRuntimeState,
};
pub use event::{
    DeviceId, InputEvent, Key, KeyEvent, KeyState, ModifierSemantic, ModifierSide, Modifiers,
    MouseButton, MouseEvent, OsKind, ScrollEvent, KEY_CODE_SPACE,
};
pub use input_state::RemoteInputState;
pub use keymap::{CompiledProfile, MappingError, ShortcutAction};
pub use profile::{
    DeviceLayout, DeviceLayoutTarget, EdgeBindings, FunctionKeyMode, HabitPreset, KeyMapping,
    KeyboardLayout, KeyboardMode, NonEnglishInputStrategy, PointerSettings, Profile,
    ProfileConfigError, ScreenEdge, ScrollSettings,
};
pub use protocol::{
    clipboard_files_hash, clipboard_text_hash, file_content_hash, ClipboardFileMetadata,
    ClipboardFiles, ClipboardFormat, ClipboardImage, ClipboardText, ControlChannelFlags,
    ControlMessage, ControlSessionState, DecodeError, EncodeError, FileTransferChunk, InputChannel,
    InputEventEnvelope, ProtocolEvent, ProtocolFrame, ProtocolNegotiationError, ProtocolPayload,
    ProtocolVersionRange, TransportLane, CURRENT_PROTOCOL_VERSION,
};
