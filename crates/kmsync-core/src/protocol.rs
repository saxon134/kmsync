use crate::event::{
    DeviceId, InputEvent, Key, KeyEvent, KeyState, Modifiers, MouseButton, MouseEvent, ScrollEvent,
};

const MAGIC: &[u8; 4] = b"SYN1";
const FRAME_MAGIC: &[u8; 4] = b"SYN2";
pub const CURRENT_PROTOCOL_VERSION: u16 = 2;
const EVENT_KEY: u8 = 1;
const EVENT_MOUSE_MOVE: u8 = 2;
const EVENT_MOUSE_BUTTON: u8 = 3;
const EVENT_SCROLL: u8 = 4;
const EVENT_MOUSE_POSITION: u8 = 5;
const PAYLOAD_INPUT: u8 = 1;
const PAYLOAD_CLIPBOARD_TEXT_LEGACY: u8 = 10;
const PAYLOAD_CLIPBOARD_TEXT: u8 = 11;
const PAYLOAD_CLIPBOARD_CONTENT: u8 = 12;
const PAYLOAD_CLIPBOARD_FILES: u8 = 13;
const PAYLOAD_FILE_CHUNK: u8 = 14;
const PAYLOAD_CONTROL: u8 = 20;
const CONTROL_HEARTBEAT: u8 = 1;
const CONTROL_CAPABILITIES: u8 = 2;
const CONTROL_CONFIG_VERSION: u8 = 3;
const CONTROL_SESSION_STATE: u8 = 4;
const INPUT_METADATA_LEN: usize = 35;
const MAX_INPUT_EVENT_LEN: usize = 9;
const CLIPBOARD_METADATA_LEN: usize = 32;
const CLIPBOARD_CONTENT_METADATA_LEN: usize = 41;
const CLIPBOARD_FILES_METADATA_LEN: usize = 34;
const CLIPBOARD_FILE_ENTRY_METADATA_LEN: usize = 18;
const FILE_CHUNK_METADATA_LEN: usize = 57;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProtocolEvent {
    pub sequence: u64,
    pub timestamp_micros: u64,
    pub event: InputEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    UnknownEventType(u8),
    UnknownInputChannel(u8),
    UnknownClipboardFormat(u8),
    UnknownControlMessage(u8),
    UnknownControlSessionState(u8),
    UnknownKey(u16),
    UnknownButton(u8),
    UnknownState(u8),
    InvalidUtf8,
    LengthOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    BufferTooSmall,
    LengthOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolVersionRange {
    pub min: u16,
    pub max: u16,
}

impl ProtocolVersionRange {
    #[must_use]
    pub const fn new(min: u16, max: u16) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub const fn current() -> Self {
        Self {
            min: CURRENT_PROTOCOL_VERSION,
            max: CURRENT_PROTOCOL_VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolNegotiationError {
    NoCompatibleVersion {
        local: ProtocolVersionRange,
        remote: ProtocolVersionRange,
    },
}

pub fn negotiate_protocol_version(
    local: ProtocolVersionRange,
    remote: ProtocolVersionRange,
) -> Result<u16, ProtocolNegotiationError> {
    let min = local.min.max(remote.min);
    let max = local.max.min(remote.max);
    if min <= max {
        Ok(max)
    } else {
        Err(ProtocolNegotiationError::NoCompatibleVersion { local, remote })
    }
}

impl ProtocolEvent {
    #[must_use]
    pub fn encode(self, out: &mut [u8; 32]) -> usize {
        out[..4].copy_from_slice(MAGIC);
        out[4..12].copy_from_slice(&self.sequence.to_le_bytes());
        out[12..20].copy_from_slice(&self.timestamp_micros.to_le_bytes());

        let mut event_buffer = [0; MAX_INPUT_EVENT_LEN];
        let event_size = encode_input_event(self.event, &mut event_buffer);
        out[20..20 + event_size].copy_from_slice(&event_buffer[..event_size]);
        20 + event_size
    }

    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        if input.len() < 21 {
            return Err(DecodeError::TooShort);
        }
        if &input[..4] != MAGIC {
            return Err(DecodeError::BadMagic);
        }

        let sequence = u64::from_le_bytes(input[4..12].try_into().expect("slice len checked"));
        let timestamp_micros =
            u64::from_le_bytes(input[12..20].try_into().expect("slice len checked"));

        let event = decode_input_event(&input[20..])?;

        Ok(Self {
            sequence,
            timestamp_micros,
            event,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChannel {
    InputUnreliable,
    InputReliable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportLane {
    InputUnreliable,
    InputReliable,
    Clipboard,
    Control,
}

impl InputChannel {
    #[must_use]
    pub const fn for_event(event: InputEvent) -> Self {
        match event {
            InputEvent::Mouse(MouseEvent::Move { .. }) => Self::InputUnreliable,
            InputEvent::Key(_)
            | InputEvent::Mouse(MouseEvent::Position { .. })
            | InputEvent::Mouse(MouseEvent::Button { .. })
            | InputEvent::Scroll(_) => Self::InputReliable,
        }
    }

    #[must_use]
    pub const fn transport_lane(self) -> TransportLane {
        match self {
            Self::InputUnreliable => TransportLane::InputUnreliable,
            Self::InputReliable => TransportLane::InputReliable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputEventEnvelope {
    pub source_device_id: DeviceId,
    pub target_device_id: DeviceId,
    pub protocol_version: u16,
    pub channel: InputChannel,
    pub event: InputEvent,
}

impl InputEventEnvelope {
    #[must_use]
    pub const fn new(
        source_device_id: DeviceId,
        target_device_id: DeviceId,
        protocol_version: u16,
        channel: InputChannel,
        event: InputEvent,
    ) -> Self {
        Self {
            source_device_id,
            target_device_id,
            protocol_version,
            channel,
            event,
        }
    }

    #[must_use]
    pub const fn current(
        source_device_id: DeviceId,
        target_device_id: DeviceId,
        event: InputEvent,
    ) -> Self {
        Self::new(
            source_device_id,
            target_device_id,
            CURRENT_PROTOCOL_VERSION,
            InputChannel::for_event(event),
            event,
        )
    }

    #[must_use]
    pub const fn legacy(event: InputEvent) -> Self {
        Self::new(0, 0, 0, InputChannel::for_event(event), event)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardFormat {
    PlainText,
    Url,
    Html,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardText {
    pub source_id: DeviceId,
    pub version: u64,
    pub content_hash: u64,
    pub format: ClipboardFormat,
    pub text: String,
    pub html: Option<String>,
    pub image: Option<ClipboardImage>,
}

impl ClipboardText {
    #[must_use]
    pub fn new(source_id: DeviceId, version: u64, text: String) -> Self {
        let content_hash = clipboard_text_hash(&text);
        Self {
            source_id,
            version,
            content_hash,
            format: ClipboardFormat::PlainText,
            text,
            html: None,
            image: None,
        }
    }

    #[must_use]
    pub fn legacy(text: String) -> Self {
        Self::new(0, 0, text)
    }

    #[must_use]
    pub fn from_local_text(source_id: DeviceId, version: u64, text: String) -> Self {
        if is_likely_clipboard_url(&text) {
            Self::url(source_id, version, text)
        } else {
            Self::new(source_id, version, text)
        }
    }

    #[must_use]
    pub fn url(source_id: DeviceId, version: u64, text: String) -> Self {
        let content_hash = clipboard_content_hash(ClipboardFormat::Url, &text, None);
        Self {
            source_id,
            version,
            content_hash,
            format: ClipboardFormat::Url,
            text,
            html: None,
            image: None,
        }
    }

    #[must_use]
    pub fn html(source_id: DeviceId, version: u64, html: String, text: String) -> Self {
        let content_hash = clipboard_content_hash(ClipboardFormat::Html, &text, Some(&html));
        Self {
            source_id,
            version,
            content_hash,
            format: ClipboardFormat::Html,
            text,
            html: Some(html),
            image: None,
        }
    }

    #[must_use]
    pub fn image(
        source_id: DeviceId,
        version: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Self {
        let content_hash = clipboard_image_hash(width, height, &rgba);
        Self {
            source_id,
            version,
            content_hash,
            format: ClipboardFormat::Image,
            text: String::new(),
            html: None,
            image: Some(ClipboardImage {
                width,
                height,
                rgba,
            }),
        }
    }

    #[must_use]
    pub fn with_source_version(self, source_id: DeviceId, version: u64) -> Self {
        match self.format {
            ClipboardFormat::PlainText => Self::new(source_id, version, self.text),
            ClipboardFormat::Url => Self::url(source_id, version, self.text),
            ClipboardFormat::Html => {
                Self::html(source_id, version, self.html.unwrap_or_default(), self.text)
            }
            ClipboardFormat::Image => {
                let image = self.image.unwrap_or(ClipboardImage {
                    width: 0,
                    height: 0,
                    rgba: Vec::new(),
                });
                Self::image(source_id, version, image.width, image.height, image.rgba)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardFileMetadata {
    pub name: String,
    pub byte_len: u64,
    pub content_hash: u64,
}

impl ClipboardFileMetadata {
    #[must_use]
    pub fn new(name: String, byte_len: u64, content_hash: u64) -> Self {
        Self {
            name: sanitize_clipboard_file_name(&name),
            byte_len,
            content_hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardFiles {
    pub source_id: DeviceId,
    pub version: u64,
    pub content_hash: u64,
    pub files: Vec<ClipboardFileMetadata>,
}

impl ClipboardFiles {
    #[must_use]
    pub fn new(source_id: DeviceId, version: u64, files: Vec<ClipboardFileMetadata>) -> Self {
        let content_hash = clipboard_files_hash(&files);
        Self {
            source_id,
            version,
            content_hash,
            files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTransferChunk {
    pub transfer_id: u128,
    pub source_id: DeviceId,
    pub file_index: u32,
    pub offset: u64,
    pub total_size: u64,
    pub chunk_index: u32,
    pub is_final: bool,
    pub data: Vec<u8>,
}

impl FileTransferChunk {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transfer_id: u128,
        source_id: DeviceId,
        file_index: u32,
        offset: u64,
        total_size: u64,
        chunk_index: u32,
        is_final: bool,
        data: Vec<u8>,
    ) -> Self {
        Self {
            transfer_id,
            source_id,
            file_index,
            offset,
            total_size,
            chunk_index,
            is_final,
            data,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlChannelFlags {
    pub input_unreliable: bool,
    pub input_reliable: bool,
    pub clipboard: bool,
    pub control: bool,
}

impl ControlChannelFlags {
    #[must_use]
    pub const fn all_current() -> Self {
        Self {
            input_unreliable: true,
            input_reliable: true,
            clipboard: true,
            control: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlSessionState {
    Starting,
    Active,
    Suspended,
    Closing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    Heartbeat {
        source_device_id: DeviceId,
        session_id: u128,
        sequence: u64,
    },
    Capabilities {
        source_device_id: DeviceId,
        protocol: ProtocolVersionRange,
        channels: ControlChannelFlags,
    },
    ConfigVersion {
        source_device_id: DeviceId,
        version: u64,
    },
    SessionState {
        source_device_id: DeviceId,
        session_id: u128,
        state: ControlSessionState,
    },
}

impl ControlMessage {
    #[must_use]
    pub const fn heartbeat(source_device_id: DeviceId, session_id: u128, sequence: u64) -> Self {
        Self::Heartbeat {
            source_device_id,
            session_id,
            sequence,
        }
    }

    #[must_use]
    pub const fn capabilities(source_device_id: DeviceId, protocol: ProtocolVersionRange) -> Self {
        Self::Capabilities {
            source_device_id,
            protocol,
            channels: ControlChannelFlags::all_current(),
        }
    }

    #[must_use]
    pub const fn config_version(source_device_id: DeviceId, version: u64) -> Self {
        Self::ConfigVersion {
            source_device_id,
            version,
        }
    }

    #[must_use]
    pub const fn session_state(
        source_device_id: DeviceId,
        session_id: u128,
        state: ControlSessionState,
    ) -> Self {
        Self::SessionState {
            source_device_id,
            session_id,
            state,
        }
    }
}

#[must_use]
pub fn clipboard_text_hash(text: &str) -> u64 {
    fnv_hash_bytes(FNV_OFFSET, text.as_bytes())
}

#[must_use]
pub fn file_content_hash(bytes: &[u8]) -> u64 {
    fnv_hash_bytes(FNV_OFFSET ^ 0x04, bytes)
}

#[must_use]
pub fn clipboard_files_hash(files: &[ClipboardFileMetadata]) -> u64 {
    let mut hash = (FNV_OFFSET ^ 0x05).wrapping_mul(FNV_PRIME);
    for file in files {
        hash = fnv_hash_bytes(hash, file.name.as_bytes());
        hash = (hash ^ 0xff).wrapping_mul(FNV_PRIME);
        for byte in file.byte_len.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
        }
        for byte in file.content_hash.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

#[must_use]
fn clipboard_content_hash(format: ClipboardFormat, text: &str, html: Option<&str>) -> u64 {
    if format == ClipboardFormat::PlainText && html.is_none() {
        return clipboard_text_hash(text);
    }

    let mut hash =
        (FNV_OFFSET ^ u64::from(encode_clipboard_format(format))).wrapping_mul(FNV_PRIME);
    for byte in text.as_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
    hash = (hash ^ 0xff).wrapping_mul(FNV_PRIME);
    if let Some(html) = html {
        for byte in html.as_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

#[must_use]
fn clipboard_image_hash(width: u32, height: u32, rgba: &[u8]) -> u64 {
    let mut hash = (FNV_OFFSET ^ u64::from(encode_clipboard_format(ClipboardFormat::Image)))
        .wrapping_mul(FNV_PRIME);
    for byte in width.to_le_bytes().into_iter().chain(height.to_le_bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
    }
    for byte in rgba {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

fn fnv_hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

fn sanitize_clipboard_file_name(name: &str) -> String {
    let trimmed = name.trim();
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("unnamed");
    base.to_string()
}

fn is_likely_clipboard_url(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed == text
        && matches!(
            trimmed.split_once(':'),
            Some(("http" | "https" | "file" | "mailto", rest)) if !rest.is_empty()
        )
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolPayload {
    Input(InputEventEnvelope),
    ClipboardText(ClipboardText),
    ClipboardFiles(ClipboardFiles),
    FileChunk(FileTransferChunk),
    Control(ControlMessage),
}

impl ProtocolPayload {
    #[must_use]
    pub const fn transport_lane(&self) -> TransportLane {
        match self {
            Self::Input(input) => input.channel.transport_lane(),
            Self::ClipboardText(_) | Self::ClipboardFiles(_) | Self::FileChunk(_) => {
                TransportLane::Clipboard
            }
            Self::Control(_) => TransportLane::Control,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolFrame {
    pub sequence: u64,
    pub timestamp_micros: u64,
    pub payload: ProtocolPayload,
}

impl ProtocolFrame {
    pub fn encode_into(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        if out.len() < 25 {
            return Err(EncodeError::BufferTooSmall);
        }

        out[..4].copy_from_slice(FRAME_MAGIC);
        out[4..12].copy_from_slice(&self.sequence.to_le_bytes());
        out[12..20].copy_from_slice(&self.timestamp_micros.to_le_bytes());

        match &self.payload {
            ProtocolPayload::Input(input) => {
                let mut event_buffer = [0; MAX_INPUT_EVENT_LEN];
                let event_size = encode_input_event(input.event, &mut event_buffer);
                let payload_len = INPUT_METADATA_LEN + event_size;
                let payload_len =
                    u32::try_from(payload_len).map_err(|_| EncodeError::LengthOverflow)?;
                let required = 25 + INPUT_METADATA_LEN + event_size;
                if out.len() < required {
                    return Err(EncodeError::BufferTooSmall);
                }
                out[20] = PAYLOAD_INPUT;
                out[21..25].copy_from_slice(&payload_len.to_le_bytes());
                out[25..41].copy_from_slice(&input.source_device_id.to_le_bytes());
                out[41..57].copy_from_slice(&input.target_device_id.to_le_bytes());
                out[57..59].copy_from_slice(&input.protocol_version.to_le_bytes());
                out[59] = encode_input_channel(input.channel);
                out[60..required].copy_from_slice(&event_buffer[..event_size]);
                Ok(required)
            }
            ProtocolPayload::ClipboardText(clipboard) => {
                if clipboard_uses_content_frame(clipboard) {
                    return encode_clipboard_content_into(clipboard, out);
                }
                let bytes = clipboard.text.as_bytes();
                let payload_len = u32::try_from(CLIPBOARD_METADATA_LEN + bytes.len())
                    .map_err(|_| EncodeError::LengthOverflow)?;
                let required = 25 + CLIPBOARD_METADATA_LEN + bytes.len();
                if out.len() < required {
                    return Err(EncodeError::BufferTooSmall);
                }
                out[20] = PAYLOAD_CLIPBOARD_TEXT;
                out[21..25].copy_from_slice(&payload_len.to_le_bytes());
                out[25..41].copy_from_slice(&clipboard.source_id.to_le_bytes());
                out[41..49].copy_from_slice(&clipboard.version.to_le_bytes());
                out[49..57].copy_from_slice(&clipboard.content_hash.to_le_bytes());
                out[57..required].copy_from_slice(bytes);
                Ok(required)
            }
            ProtocolPayload::ClipboardFiles(files) => encode_clipboard_files_into(files, out),
            ProtocolPayload::FileChunk(chunk) => encode_file_chunk_into(chunk, out),
            ProtocolPayload::Control(message) => encode_control_into(message, out),
        }
    }

    #[must_use]
    pub fn encode_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(FRAME_MAGIC);
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&self.timestamp_micros.to_le_bytes());

        match &self.payload {
            ProtocolPayload::Input(input) => {
                let mut event_buffer = [0; MAX_INPUT_EVENT_LEN];
                let event_size = encode_input_event(input.event, &mut event_buffer);
                out.push(PAYLOAD_INPUT);
                out.extend_from_slice(
                    &(u32::try_from(INPUT_METADATA_LEN + event_size).unwrap_or(0)).to_le_bytes(),
                );
                out.extend_from_slice(&input.source_device_id.to_le_bytes());
                out.extend_from_slice(&input.target_device_id.to_le_bytes());
                out.extend_from_slice(&input.protocol_version.to_le_bytes());
                out.push(encode_input_channel(input.channel));
                out.extend_from_slice(&event_buffer[..event_size]);
            }
            ProtocolPayload::ClipboardText(clipboard) => {
                if clipboard_uses_content_frame(clipboard) {
                    encode_clipboard_content_vec(clipboard, &mut out);
                    return out;
                }
                out.push(PAYLOAD_CLIPBOARD_TEXT);
                let bytes = clipboard.text.as_bytes();
                out.extend_from_slice(
                    &(u32::try_from(CLIPBOARD_METADATA_LEN + bytes.len()).unwrap_or(0))
                        .to_le_bytes(),
                );
                out.extend_from_slice(&clipboard.source_id.to_le_bytes());
                out.extend_from_slice(&clipboard.version.to_le_bytes());
                out.extend_from_slice(&clipboard.content_hash.to_le_bytes());
                out.extend_from_slice(bytes);
            }
            ProtocolPayload::ClipboardFiles(files) => encode_clipboard_files_vec(files, &mut out),
            ProtocolPayload::FileChunk(chunk) => encode_file_chunk_vec(chunk, &mut out),
            ProtocolPayload::Control(message) => encode_control_vec(message, &mut out),
        }

        out
    }

    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        if input.len() < 25 {
            return Err(DecodeError::TooShort);
        }
        if &input[..4] != FRAME_MAGIC {
            return ProtocolEvent::decode(input).map(|event| Self {
                sequence: event.sequence,
                timestamp_micros: event.timestamp_micros,
                payload: ProtocolPayload::Input(InputEventEnvelope::legacy(event.event)),
            });
        }

        let sequence = u64::from_le_bytes(input[4..12].try_into().expect("slice len checked"));
        let timestamp_micros =
            u64::from_le_bytes(input[12..20].try_into().expect("slice len checked"));
        let payload_type = input[20];
        let len = u32::from_le_bytes(input[21..25].try_into().expect("slice len checked"));
        let len = usize::try_from(len).map_err(|_| DecodeError::LengthOverflow)?;
        if input.len() < 25 + len {
            return Err(DecodeError::TooShort);
        }
        let payload_bytes = &input[25..25 + len];

        let payload = match payload_type {
            0 => ProtocolEvent::decode(payload_bytes)
                .map(|event| ProtocolPayload::Input(InputEventEnvelope::legacy(event.event)))?,
            PAYLOAD_INPUT => {
                if payload_bytes.len() < INPUT_METADATA_LEN + 1 {
                    return Err(DecodeError::TooShort);
                }
                let source_device_id =
                    u128::from_le_bytes(payload_bytes[..16].try_into().expect("slice len"));
                let target_device_id =
                    u128::from_le_bytes(payload_bytes[16..32].try_into().expect("slice len"));
                let protocol_version =
                    u16::from_le_bytes(payload_bytes[32..34].try_into().expect("slice len"));
                let channel = decode_input_channel(payload_bytes[34])?;
                let event = decode_input_event(&payload_bytes[INPUT_METADATA_LEN..])?;
                ProtocolPayload::Input(InputEventEnvelope::new(
                    source_device_id,
                    target_device_id,
                    protocol_version,
                    channel,
                    event,
                ))
            }
            PAYLOAD_CLIPBOARD_TEXT_LEGACY => {
                let text = core::str::from_utf8(payload_bytes)
                    .map_err(|_| DecodeError::InvalidUtf8)?
                    .to_string();
                ProtocolPayload::ClipboardText(ClipboardText::legacy(text))
            }
            PAYLOAD_CLIPBOARD_TEXT => {
                if payload_bytes.len() < CLIPBOARD_METADATA_LEN {
                    return Err(DecodeError::TooShort);
                }
                let source_id =
                    u128::from_le_bytes(payload_bytes[..16].try_into().expect("slice len"));
                let version =
                    u64::from_le_bytes(payload_bytes[16..24].try_into().expect("slice len"));
                let text = core::str::from_utf8(&payload_bytes[CLIPBOARD_METADATA_LEN..])
                    .map_err(|_| DecodeError::InvalidUtf8)?
                    .to_string();
                ProtocolPayload::ClipboardText(ClipboardText::new(source_id, version, text))
            }
            PAYLOAD_CLIPBOARD_CONTENT => {
                if payload_bytes.len() < CLIPBOARD_CONTENT_METADATA_LEN {
                    return Err(DecodeError::TooShort);
                }
                let source_id =
                    u128::from_le_bytes(payload_bytes[..16].try_into().expect("slice len"));
                let version =
                    u64::from_le_bytes(payload_bytes[16..24].try_into().expect("slice len"));
                let format = decode_clipboard_format(payload_bytes[32])?;
                let text_len =
                    u32::from_le_bytes(payload_bytes[33..37].try_into().expect("slice len"));
                let html_len =
                    u32::from_le_bytes(payload_bytes[37..41].try_into().expect("slice len"));
                let text_len =
                    usize::try_from(text_len).map_err(|_| DecodeError::LengthOverflow)?;
                let html_len =
                    usize::try_from(html_len).map_err(|_| DecodeError::LengthOverflow)?;
                let required = CLIPBOARD_CONTENT_METADATA_LEN
                    .checked_add(text_len)
                    .and_then(|size| size.checked_add(html_len))
                    .ok_or(DecodeError::LengthOverflow)?;
                if payload_bytes.len() < required {
                    return Err(DecodeError::TooShort);
                }
                if format == ClipboardFormat::Image {
                    let image_bytes = payload_bytes[CLIPBOARD_CONTENT_METADATA_LEN..].to_vec();
                    return Ok(Self {
                        sequence,
                        timestamp_micros,
                        payload: ProtocolPayload::ClipboardText(ClipboardText::image(
                            source_id,
                            version,
                            u32::try_from(text_len).map_err(|_| DecodeError::LengthOverflow)?,
                            u32::try_from(html_len).map_err(|_| DecodeError::LengthOverflow)?,
                            image_bytes,
                        )),
                    });
                }
                let text_start = CLIPBOARD_CONTENT_METADATA_LEN;
                let text_end = text_start + text_len;
                let html_end = text_end + html_len;
                let text = core::str::from_utf8(&payload_bytes[text_start..text_end])
                    .map_err(|_| DecodeError::InvalidUtf8)?
                    .to_string();
                let html = if html_len == 0 {
                    None
                } else {
                    Some(
                        core::str::from_utf8(&payload_bytes[text_end..html_end])
                            .map_err(|_| DecodeError::InvalidUtf8)?
                            .to_string(),
                    )
                };
                ProtocolPayload::ClipboardText(clipboard_from_parts(
                    source_id, version, format, text, html,
                ))
            }
            PAYLOAD_CLIPBOARD_FILES => {
                ProtocolPayload::ClipboardFiles(decode_clipboard_files(payload_bytes)?)
            }
            PAYLOAD_FILE_CHUNK => ProtocolPayload::FileChunk(decode_file_chunk(payload_bytes)?),
            PAYLOAD_CONTROL => ProtocolPayload::Control(decode_control_message(payload_bytes)?),
            unknown => return Err(DecodeError::UnknownEventType(unknown)),
        };

        Ok(Self {
            sequence,
            timestamp_micros,
            payload,
        })
    }
}

fn clipboard_uses_content_frame(clipboard: &ClipboardText) -> bool {
    clipboard.format != ClipboardFormat::PlainText
        || clipboard.html.is_some()
        || clipboard.image.is_some()
}

fn clipboard_payload_len(clipboard: &ClipboardText) -> Result<usize, EncodeError> {
    CLIPBOARD_CONTENT_METADATA_LEN
        .checked_add(clipboard.text.len())
        .and_then(|size| size.checked_add(clipboard.html.as_deref().unwrap_or("").len()))
        .and_then(|size| {
            size.checked_add(clipboard.image.as_ref().map_or(0, |image| image.rgba.len()))
        })
        .ok_or(EncodeError::LengthOverflow)
}

fn encode_clipboard_content_into(
    clipboard: &ClipboardText,
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    let payload_len = clipboard_payload_len(clipboard)?;
    let required = 25usize
        .checked_add(payload_len)
        .ok_or(EncodeError::LengthOverflow)?;
    if out.len() < required {
        return Err(EncodeError::BufferTooSmall);
    }
    let payload_len = u32::try_from(payload_len).map_err(|_| EncodeError::LengthOverflow)?;
    let text_bytes = clipboard.text.as_bytes();
    let html_bytes = clipboard.html.as_deref().unwrap_or("").as_bytes();
    let image_bytes = clipboard
        .image
        .as_ref()
        .map_or([].as_slice(), |image| image.rgba.as_slice());
    let (first_len, second_len) = clipboard_content_lengths(clipboard, text_bytes, html_bytes)?;

    out[20] = PAYLOAD_CLIPBOARD_CONTENT;
    out[21..25].copy_from_slice(&payload_len.to_le_bytes());
    out[25..41].copy_from_slice(&clipboard.source_id.to_le_bytes());
    out[41..49].copy_from_slice(&clipboard.version.to_le_bytes());
    out[49..57].copy_from_slice(&clipboard.content_hash.to_le_bytes());
    out[57] = encode_clipboard_format(clipboard.format);
    out[58..62].copy_from_slice(&first_len.to_le_bytes());
    out[62..66].copy_from_slice(&second_len.to_le_bytes());
    out[66..66 + text_bytes.len()].copy_from_slice(text_bytes);
    out[66 + text_bytes.len()..66 + text_bytes.len() + html_bytes.len()]
        .copy_from_slice(html_bytes);
    out[66 + text_bytes.len() + html_bytes.len()..required].copy_from_slice(image_bytes);
    Ok(required)
}

fn encode_clipboard_content_vec(clipboard: &ClipboardText, out: &mut Vec<u8>) {
    let text_bytes = clipboard.text.as_bytes();
    let html_bytes = clipboard.html.as_deref().unwrap_or("").as_bytes();
    let image_bytes = clipboard
        .image
        .as_ref()
        .map_or([].as_slice(), |image| image.rgba.as_slice());
    let (first_len, second_len) =
        clipboard_content_lengths(clipboard, text_bytes, html_bytes).unwrap_or((0, 0));
    out.push(PAYLOAD_CLIPBOARD_CONTENT);
    out.extend_from_slice(
        &(u32::try_from(
            CLIPBOARD_CONTENT_METADATA_LEN
                + text_bytes.len()
                + html_bytes.len()
                + image_bytes.len(),
        )
        .unwrap_or(0))
        .to_le_bytes(),
    );
    out.extend_from_slice(&clipboard.source_id.to_le_bytes());
    out.extend_from_slice(&clipboard.version.to_le_bytes());
    out.extend_from_slice(&clipboard.content_hash.to_le_bytes());
    out.push(encode_clipboard_format(clipboard.format));
    out.extend_from_slice(&first_len.to_le_bytes());
    out.extend_from_slice(&second_len.to_le_bytes());
    out.extend_from_slice(text_bytes);
    out.extend_from_slice(html_bytes);
    out.extend_from_slice(image_bytes);
}

fn clipboard_content_lengths(
    clipboard: &ClipboardText,
    text_bytes: &[u8],
    html_bytes: &[u8],
) -> Result<(u32, u32), EncodeError> {
    match clipboard.format {
        ClipboardFormat::Image => {
            let image = clipboard
                .image
                .as_ref()
                .ok_or(EncodeError::LengthOverflow)?;
            Ok((image.width, image.height))
        }
        _ => Ok((
            u32::try_from(text_bytes.len()).map_err(|_| EncodeError::LengthOverflow)?,
            u32::try_from(html_bytes.len()).map_err(|_| EncodeError::LengthOverflow)?,
        )),
    }
}

fn clipboard_files_payload_len(files: &ClipboardFiles) -> Result<usize, EncodeError> {
    let entries_len = files.files.iter().try_fold(0usize, |total, file| {
        total
            .checked_add(CLIPBOARD_FILE_ENTRY_METADATA_LEN)
            .and_then(|size| size.checked_add(file.name.len()))
            .ok_or(EncodeError::LengthOverflow)
    })?;
    CLIPBOARD_FILES_METADATA_LEN
        .checked_add(entries_len)
        .ok_or(EncodeError::LengthOverflow)
}

fn encode_clipboard_files_into(
    files: &ClipboardFiles,
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    let payload_len = clipboard_files_payload_len(files)?;
    let required = 25usize
        .checked_add(payload_len)
        .ok_or(EncodeError::LengthOverflow)?;
    if out.len() < required {
        return Err(EncodeError::BufferTooSmall);
    }
    let payload_len = u32::try_from(payload_len).map_err(|_| EncodeError::LengthOverflow)?;
    let file_count = u16::try_from(files.files.len()).map_err(|_| EncodeError::LengthOverflow)?;

    out[20] = PAYLOAD_CLIPBOARD_FILES;
    out[21..25].copy_from_slice(&payload_len.to_le_bytes());
    out[25..41].copy_from_slice(&files.source_id.to_le_bytes());
    out[41..49].copy_from_slice(&files.version.to_le_bytes());
    out[49..57].copy_from_slice(&files.content_hash.to_le_bytes());
    out[57..59].copy_from_slice(&file_count.to_le_bytes());

    let mut cursor = 59;
    for file in &files.files {
        let name_bytes = file.name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| EncodeError::LengthOverflow)?;
        out[cursor..cursor + 8].copy_from_slice(&file.byte_len.to_le_bytes());
        cursor += 8;
        out[cursor..cursor + 8].copy_from_slice(&file.content_hash.to_le_bytes());
        cursor += 8;
        out[cursor..cursor + 2].copy_from_slice(&name_len.to_le_bytes());
        cursor += 2;
        out[cursor..cursor + name_bytes.len()].copy_from_slice(name_bytes);
        cursor += name_bytes.len();
    }
    Ok(required)
}

fn encode_clipboard_files_vec(files: &ClipboardFiles, out: &mut Vec<u8>) {
    let payload_len = clipboard_files_payload_len(files).unwrap_or(0);
    let file_count = u16::try_from(files.files.len()).unwrap_or(0);
    out.push(PAYLOAD_CLIPBOARD_FILES);
    out.extend_from_slice(&(u32::try_from(payload_len).unwrap_or(0)).to_le_bytes());
    out.extend_from_slice(&files.source_id.to_le_bytes());
    out.extend_from_slice(&files.version.to_le_bytes());
    out.extend_from_slice(&files.content_hash.to_le_bytes());
    out.extend_from_slice(&file_count.to_le_bytes());
    for file in &files.files {
        let name_bytes = file.name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).unwrap_or(0);
        out.extend_from_slice(&file.byte_len.to_le_bytes());
        out.extend_from_slice(&file.content_hash.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name_bytes);
    }
}

fn encode_file_chunk_into(chunk: &FileTransferChunk, out: &mut [u8]) -> Result<usize, EncodeError> {
    let payload_len = FILE_CHUNK_METADATA_LEN
        .checked_add(chunk.data.len())
        .ok_or(EncodeError::LengthOverflow)?;
    let required = 25usize
        .checked_add(payload_len)
        .ok_or(EncodeError::LengthOverflow)?;
    if out.len() < required {
        return Err(EncodeError::BufferTooSmall);
    }
    let payload_len = u32::try_from(payload_len).map_err(|_| EncodeError::LengthOverflow)?;

    out[20] = PAYLOAD_FILE_CHUNK;
    out[21..25].copy_from_slice(&payload_len.to_le_bytes());
    write_file_chunk_metadata(chunk, &mut out[25..82]);
    out[82..required].copy_from_slice(&chunk.data);
    Ok(required)
}

fn encode_file_chunk_vec(chunk: &FileTransferChunk, out: &mut Vec<u8>) {
    out.push(PAYLOAD_FILE_CHUNK);
    out.extend_from_slice(
        &(u32::try_from(FILE_CHUNK_METADATA_LEN + chunk.data.len()).unwrap_or(0)).to_le_bytes(),
    );
    let mut metadata = [0; FILE_CHUNK_METADATA_LEN];
    write_file_chunk_metadata(chunk, &mut metadata);
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&chunk.data);
}

fn write_file_chunk_metadata(chunk: &FileTransferChunk, out: &mut [u8]) {
    out[..16].copy_from_slice(&chunk.transfer_id.to_le_bytes());
    out[16..32].copy_from_slice(&chunk.source_id.to_le_bytes());
    out[32..36].copy_from_slice(&chunk.file_index.to_le_bytes());
    out[36..44].copy_from_slice(&chunk.offset.to_le_bytes());
    out[44..52].copy_from_slice(&chunk.total_size.to_le_bytes());
    out[52..56].copy_from_slice(&chunk.chunk_index.to_le_bytes());
    out[56] = u8::from(chunk.is_final);
}

fn decode_clipboard_files(payload_bytes: &[u8]) -> Result<ClipboardFiles, DecodeError> {
    if payload_bytes.len() < CLIPBOARD_FILES_METADATA_LEN {
        return Err(DecodeError::TooShort);
    }
    let source_id = u128::from_le_bytes(payload_bytes[..16].try_into().expect("slice len"));
    let version = u64::from_le_bytes(payload_bytes[16..24].try_into().expect("slice len"));
    let file_count = u16::from_le_bytes(payload_bytes[32..34].try_into().expect("slice len"));
    let mut cursor = CLIPBOARD_FILES_METADATA_LEN;
    let mut files = Vec::with_capacity(usize::from(file_count));

    for _ in 0..file_count {
        if payload_bytes.len() < cursor + CLIPBOARD_FILE_ENTRY_METADATA_LEN {
            return Err(DecodeError::TooShort);
        }
        let byte_len = u64::from_le_bytes(
            payload_bytes[cursor..cursor + 8]
                .try_into()
                .expect("slice len"),
        );
        cursor += 8;
        let content_hash = u64::from_le_bytes(
            payload_bytes[cursor..cursor + 8]
                .try_into()
                .expect("slice len"),
        );
        cursor += 8;
        let name_len = u16::from_le_bytes(
            payload_bytes[cursor..cursor + 2]
                .try_into()
                .expect("slice len"),
        );
        cursor += 2;
        let name_len = usize::from(name_len);
        if payload_bytes.len() < cursor + name_len {
            return Err(DecodeError::TooShort);
        }
        let name = core::str::from_utf8(&payload_bytes[cursor..cursor + name_len])
            .map_err(|_| DecodeError::InvalidUtf8)?
            .to_string();
        cursor += name_len;
        files.push(ClipboardFileMetadata::new(name, byte_len, content_hash));
    }

    Ok(ClipboardFiles::new(source_id, version, files))
}

fn decode_file_chunk(payload_bytes: &[u8]) -> Result<FileTransferChunk, DecodeError> {
    if payload_bytes.len() < FILE_CHUNK_METADATA_LEN {
        return Err(DecodeError::TooShort);
    }
    let is_final = match payload_bytes[56] {
        0 => false,
        1 => true,
        other => return Err(DecodeError::UnknownState(other)),
    };
    Ok(FileTransferChunk::new(
        u128::from_le_bytes(payload_bytes[..16].try_into().expect("slice len")),
        u128::from_le_bytes(payload_bytes[16..32].try_into().expect("slice len")),
        u32::from_le_bytes(payload_bytes[32..36].try_into().expect("slice len")),
        u64::from_le_bytes(payload_bytes[36..44].try_into().expect("slice len")),
        u64::from_le_bytes(payload_bytes[44..52].try_into().expect("slice len")),
        u32::from_le_bytes(payload_bytes[52..56].try_into().expect("slice len")),
        is_final,
        payload_bytes[FILE_CHUNK_METADATA_LEN..].to_vec(),
    ))
}

fn control_payload_len(message: &ControlMessage) -> usize {
    match message {
        ControlMessage::Heartbeat { .. } => 41,
        ControlMessage::Capabilities { .. } => 22,
        ControlMessage::ConfigVersion { .. } => 25,
        ControlMessage::SessionState { .. } => 34,
    }
}

fn encode_control_into(message: &ControlMessage, out: &mut [u8]) -> Result<usize, EncodeError> {
    let payload_len = control_payload_len(message);
    let required = 25usize
        .checked_add(payload_len)
        .ok_or(EncodeError::LengthOverflow)?;
    if out.len() < required {
        return Err(EncodeError::BufferTooSmall);
    }
    let payload_len = u32::try_from(payload_len).map_err(|_| EncodeError::LengthOverflow)?;
    out[20] = PAYLOAD_CONTROL;
    out[21..25].copy_from_slice(&payload_len.to_le_bytes());
    write_control_payload(message, &mut out[25..required]);
    Ok(required)
}

fn encode_control_vec(message: &ControlMessage, out: &mut Vec<u8>) {
    let payload_len = control_payload_len(message);
    out.push(PAYLOAD_CONTROL);
    out.extend_from_slice(&(u32::try_from(payload_len).unwrap_or(0)).to_le_bytes());
    let start = out.len();
    out.resize(start + payload_len, 0);
    write_control_payload(message, &mut out[start..start + payload_len]);
}

fn write_control_payload(message: &ControlMessage, out: &mut [u8]) {
    match message {
        ControlMessage::Heartbeat {
            source_device_id,
            session_id,
            sequence,
        } => {
            out[0] = CONTROL_HEARTBEAT;
            out[1..17].copy_from_slice(&source_device_id.to_le_bytes());
            out[17..33].copy_from_slice(&session_id.to_le_bytes());
            out[33..41].copy_from_slice(&sequence.to_le_bytes());
        }
        ControlMessage::Capabilities {
            source_device_id,
            protocol,
            channels,
        } => {
            out[0] = CONTROL_CAPABILITIES;
            out[1..17].copy_from_slice(&source_device_id.to_le_bytes());
            out[17..19].copy_from_slice(&protocol.min.to_le_bytes());
            out[19..21].copy_from_slice(&protocol.max.to_le_bytes());
            out[21] = encode_control_channel_flags(*channels);
        }
        ControlMessage::ConfigVersion {
            source_device_id,
            version,
        } => {
            out[0] = CONTROL_CONFIG_VERSION;
            out[1..17].copy_from_slice(&source_device_id.to_le_bytes());
            out[17..25].copy_from_slice(&version.to_le_bytes());
        }
        ControlMessage::SessionState {
            source_device_id,
            session_id,
            state,
        } => {
            out[0] = CONTROL_SESSION_STATE;
            out[1..17].copy_from_slice(&source_device_id.to_le_bytes());
            out[17..33].copy_from_slice(&session_id.to_le_bytes());
            out[33] = encode_control_session_state(*state);
        }
    }
}

fn decode_control_message(payload_bytes: &[u8]) -> Result<ControlMessage, DecodeError> {
    let Some(kind) = payload_bytes.first().copied() else {
        return Err(DecodeError::TooShort);
    };
    match kind {
        CONTROL_HEARTBEAT => {
            if payload_bytes.len() < 41 {
                return Err(DecodeError::TooShort);
            }
            Ok(ControlMessage::heartbeat(
                u128::from_le_bytes(payload_bytes[1..17].try_into().expect("slice len")),
                u128::from_le_bytes(payload_bytes[17..33].try_into().expect("slice len")),
                u64::from_le_bytes(payload_bytes[33..41].try_into().expect("slice len")),
            ))
        }
        CONTROL_CAPABILITIES => {
            if payload_bytes.len() < 22 {
                return Err(DecodeError::TooShort);
            }
            Ok(ControlMessage::Capabilities {
                source_device_id: u128::from_le_bytes(
                    payload_bytes[1..17].try_into().expect("slice len"),
                ),
                protocol: ProtocolVersionRange::new(
                    u16::from_le_bytes(payload_bytes[17..19].try_into().expect("slice len")),
                    u16::from_le_bytes(payload_bytes[19..21].try_into().expect("slice len")),
                ),
                channels: decode_control_channel_flags(payload_bytes[21]),
            })
        }
        CONTROL_CONFIG_VERSION => {
            if payload_bytes.len() < 25 {
                return Err(DecodeError::TooShort);
            }
            Ok(ControlMessage::config_version(
                u128::from_le_bytes(payload_bytes[1..17].try_into().expect("slice len")),
                u64::from_le_bytes(payload_bytes[17..25].try_into().expect("slice len")),
            ))
        }
        CONTROL_SESSION_STATE => {
            if payload_bytes.len() < 34 {
                return Err(DecodeError::TooShort);
            }
            Ok(ControlMessage::session_state(
                u128::from_le_bytes(payload_bytes[1..17].try_into().expect("slice len")),
                u128::from_le_bytes(payload_bytes[17..33].try_into().expect("slice len")),
                decode_control_session_state(payload_bytes[33])?,
            ))
        }
        other => Err(DecodeError::UnknownControlMessage(other)),
    }
}

const fn encode_control_channel_flags(flags: ControlChannelFlags) -> u8 {
    (flags.input_unreliable as u8)
        | ((flags.input_reliable as u8) << 1)
        | ((flags.clipboard as u8) << 2)
        | ((flags.control as u8) << 3)
}

const fn decode_control_channel_flags(value: u8) -> ControlChannelFlags {
    ControlChannelFlags {
        input_unreliable: value & 0b0001 != 0,
        input_reliable: value & 0b0010 != 0,
        clipboard: value & 0b0100 != 0,
        control: value & 0b1000 != 0,
    }
}

const fn encode_control_session_state(state: ControlSessionState) -> u8 {
    match state {
        ControlSessionState::Starting => 1,
        ControlSessionState::Active => 2,
        ControlSessionState::Suspended => 3,
        ControlSessionState::Closing => 4,
    }
}

const fn decode_control_session_state(value: u8) -> Result<ControlSessionState, DecodeError> {
    match value {
        1 => Ok(ControlSessionState::Starting),
        2 => Ok(ControlSessionState::Active),
        3 => Ok(ControlSessionState::Suspended),
        4 => Ok(ControlSessionState::Closing),
        other => Err(DecodeError::UnknownControlSessionState(other)),
    }
}

const fn encode_clipboard_format(format: ClipboardFormat) -> u8 {
    match format {
        ClipboardFormat::PlainText => 0,
        ClipboardFormat::Url => 1,
        ClipboardFormat::Html => 2,
        ClipboardFormat::Image => 3,
    }
}

fn decode_clipboard_format(value: u8) -> Result<ClipboardFormat, DecodeError> {
    match value {
        0 => Ok(ClipboardFormat::PlainText),
        1 => Ok(ClipboardFormat::Url),
        2 => Ok(ClipboardFormat::Html),
        3 => Ok(ClipboardFormat::Image),
        other => Err(DecodeError::UnknownClipboardFormat(other)),
    }
}

fn clipboard_from_parts(
    source_id: DeviceId,
    version: u64,
    format: ClipboardFormat,
    text: String,
    html: Option<String>,
) -> ClipboardText {
    match format {
        ClipboardFormat::PlainText => ClipboardText::new(source_id, version, text),
        ClipboardFormat::Url => ClipboardText::url(source_id, version, text),
        ClipboardFormat::Html => {
            ClipboardText::html(source_id, version, html.unwrap_or_default(), text)
        }
        ClipboardFormat::Image => ClipboardText::image(source_id, version, 0, 0, Vec::new()),
    }
}

fn encode_input_event(event: InputEvent, out: &mut [u8; MAX_INPUT_EVENT_LEN]) -> usize {
    match event {
        InputEvent::Key(event) => {
            out[0] = EVENT_KEY;
            out[1..3].copy_from_slice(&(event.key as u16).to_le_bytes());
            out[3] = encode_key_state(event.state);
            out[4] = event.modifiers.bits();
            5
        }
        InputEvent::Mouse(MouseEvent::Move { dx, dy }) => {
            out[0] = EVENT_MOUSE_MOVE;
            out[1..5].copy_from_slice(&dx.to_le_bytes());
            out[5..9].copy_from_slice(&dy.to_le_bytes());
            9
        }
        InputEvent::Mouse(MouseEvent::Position { x_ratio, y_ratio }) => {
            out[0] = EVENT_MOUSE_POSITION;
            out[1..5].copy_from_slice(&x_ratio.to_le_bytes());
            out[5..9].copy_from_slice(&y_ratio.to_le_bytes());
            9
        }
        InputEvent::Mouse(MouseEvent::Button { button, state }) => {
            out[0] = EVENT_MOUSE_BUTTON;
            out[1] = encode_mouse_button(button);
            out[2] = encode_key_state(state);
            3
        }
        InputEvent::Scroll(event) => {
            out[0] = EVENT_SCROLL;
            out[1..5].copy_from_slice(&event.dx.to_le_bytes());
            out[5..9].copy_from_slice(&event.dy.to_le_bytes());
            9
        }
    }
}

fn decode_input_event(input: &[u8]) -> Result<InputEvent, DecodeError> {
    if input.is_empty() {
        return Err(DecodeError::TooShort);
    }

    match input[0] {
        EVENT_KEY => {
            if input.len() < 5 {
                return Err(DecodeError::TooShort);
            }
            let key_raw = u16::from_le_bytes(input[1..3].try_into().expect("slice len"));
            let key = Key::from_u16(key_raw).ok_or(DecodeError::UnknownKey(key_raw))?;
            Ok(InputEvent::Key(KeyEvent {
                key,
                state: decode_key_state(input[3])?,
                modifiers: Modifiers::from_bits(input[4]),
            }))
        }
        EVENT_MOUSE_MOVE => {
            if input.len() < 9 {
                return Err(DecodeError::TooShort);
            }
            Ok(InputEvent::Mouse(MouseEvent::Move {
                dx: f32::from_le_bytes(input[1..5].try_into().expect("slice len")),
                dy: f32::from_le_bytes(input[5..9].try_into().expect("slice len")),
            }))
        }
        EVENT_MOUSE_POSITION => {
            if input.len() < 9 {
                return Err(DecodeError::TooShort);
            }
            Ok(InputEvent::Mouse(MouseEvent::Position {
                x_ratio: f32::from_le_bytes(input[1..5].try_into().expect("slice len")),
                y_ratio: f32::from_le_bytes(input[5..9].try_into().expect("slice len")),
            }))
        }
        EVENT_MOUSE_BUTTON => {
            if input.len() < 3 {
                return Err(DecodeError::TooShort);
            }
            Ok(InputEvent::Mouse(MouseEvent::Button {
                button: decode_mouse_button(input[1])?,
                state: decode_key_state(input[2])?,
            }))
        }
        EVENT_SCROLL => {
            if input.len() < 9 {
                return Err(DecodeError::TooShort);
            }
            Ok(InputEvent::Scroll(ScrollEvent {
                dx: f32::from_le_bytes(input[1..5].try_into().expect("slice len")),
                dy: f32::from_le_bytes(input[5..9].try_into().expect("slice len")),
            }))
        }
        unknown => Err(DecodeError::UnknownEventType(unknown)),
    }
}

const fn encode_input_channel(channel: InputChannel) -> u8 {
    match channel {
        InputChannel::InputUnreliable => 1,
        InputChannel::InputReliable => 2,
    }
}

const fn decode_input_channel(value: u8) -> Result<InputChannel, DecodeError> {
    match value {
        1 => Ok(InputChannel::InputUnreliable),
        2 => Ok(InputChannel::InputReliable),
        unknown => Err(DecodeError::UnknownInputChannel(unknown)),
    }
}

const fn encode_key_state(state: KeyState) -> u8 {
    match state {
        KeyState::Pressed => 1,
        KeyState::Released => 2,
    }
}

const fn decode_key_state(value: u8) -> Result<KeyState, DecodeError> {
    match value {
        1 => Ok(KeyState::Pressed),
        2 => Ok(KeyState::Released),
        unknown => Err(DecodeError::UnknownState(unknown)),
    }
}

const fn encode_mouse_button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 3,
        MouseButton::Back => 4,
        MouseButton::Forward => 5,
    }
}

const fn decode_mouse_button(value: u8) -> Result<MouseButton, DecodeError> {
    match value {
        1 => Ok(MouseButton::Left),
        2 => Ok(MouseButton::Right),
        3 => Ok(MouseButton::Middle),
        4 => Ok(MouseButton::Back),
        5 => Ok(MouseButton::Forward),
        unknown => Err(DecodeError::UnknownButton(unknown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_key_event() {
        let event = ProtocolEvent {
            sequence: 42,
            timestamp_micros: 7,
            event: InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        assert_eq!(ProtocolEvent::decode(&buffer[..size]), Ok(event));
    }

    #[test]
    fn negotiates_highest_common_protocol_version() {
        let local = ProtocolVersionRange::current();
        let remote = ProtocolVersionRange::new(0, CURRENT_PROTOCOL_VERSION + 2);

        assert_eq!(
            negotiate_protocol_version(local, remote),
            Ok(CURRENT_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn rejects_incompatible_protocol_version_ranges() {
        let local = ProtocolVersionRange::current();
        let remote =
            ProtocolVersionRange::new(CURRENT_PROTOCOL_VERSION + 1, CURRENT_PROTOCOL_VERSION + 3);

        assert_eq!(
            negotiate_protocol_version(local, remote),
            Err(ProtocolNegotiationError::NoCompatibleVersion { local, remote })
        );
    }

    #[test]
    fn round_trips_mouse_move_event() {
        let event = ProtocolEvent {
            sequence: 43,
            timestamp_micros: 8,
            event: InputEvent::Mouse(MouseEvent::Move { dx: 1.5, dy: -2.25 }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        assert_eq!(ProtocolEvent::decode(&buffer[..size]), Ok(event));
    }

    #[test]
    fn round_trips_mouse_button_event() {
        let event = ProtocolEvent {
            sequence: 44,
            timestamp_micros: 9,
            event: InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Forward,
                state: KeyState::Released,
            }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        assert_eq!(ProtocolEvent::decode(&buffer[..size]), Ok(event));
    }

    #[test]
    fn round_trips_scroll_event() {
        let event = ProtocolEvent {
            sequence: 45,
            timestamp_micros: 10,
            event: InputEvent::Scroll(ScrollEvent { dx: -3.0, dy: 4.0 }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        assert_eq!(ProtocolEvent::decode(&buffer[..size]), Ok(event));
    }

    #[test]
    fn round_trips_clipboard_frame() {
        let frame = ProtocolFrame {
            sequence: 9,
            timestamp_micros: 10,
            payload: ProtocolPayload::ClipboardText(ClipboardText::legacy("hello".to_string())),
        };
        let bytes = frame.encode_vec();

        assert_eq!(ProtocolFrame::decode(&bytes), Ok(frame));
    }

    #[test]
    fn round_trips_clipboard_frame_with_source_version_and_hash() {
        let clipboard = ClipboardText::new(42, 7, "hello".to_string());
        let frame = ProtocolFrame {
            sequence: 9,
            timestamp_micros: 10,
            payload: ProtocolPayload::ClipboardText(clipboard.clone()),
        };
        let bytes = frame.encode_vec();

        assert_eq!(clipboard.content_hash, clipboard_text_hash("hello"));
        assert_eq!(ProtocolFrame::decode(&bytes), Ok(frame));
    }

    #[test]
    fn round_trips_url_clipboard_frame_with_content_kind() {
        let clipboard = ClipboardText::url(42, 8, "https://example.com/path?q=1".to_string());
        let frame = ProtocolFrame {
            sequence: 10,
            timestamp_micros: 11,
            payload: ProtocolPayload::ClipboardText(clipboard.clone()),
        };

        let decoded = ProtocolFrame::decode(&frame.encode_vec()).expect("decode url clipboard");

        assert_eq!(clipboard.format, ClipboardFormat::Url);
        assert_eq!(decoded, frame);
    }

    #[test]
    fn round_trips_html_clipboard_frame_with_plain_text_fallback() {
        let clipboard = ClipboardText::html(
            42,
            9,
            "<strong>Hello</strong>".to_string(),
            "Hello".to_string(),
        );
        let frame = ProtocolFrame {
            sequence: 11,
            timestamp_micros: 12,
            payload: ProtocolPayload::ClipboardText(clipboard.clone()),
        };

        let decoded = ProtocolFrame::decode(&frame.encode_vec()).expect("decode html clipboard");

        assert_eq!(clipboard.format, ClipboardFormat::Html);
        assert_eq!(clipboard.html.as_deref(), Some("<strong>Hello</strong>"));
        assert_eq!(decoded, frame);
    }

    #[test]
    fn round_trips_image_clipboard_frame() {
        let pixels = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let clipboard = ClipboardText::image(42, 9, 2, 1, pixels);
        let frame = ProtocolFrame {
            sequence: 12,
            timestamp_micros: 13,
            payload: ProtocolPayload::ClipboardText(clipboard.clone()),
        };

        let decoded = ProtocolFrame::decode(&frame.encode_vec()).expect("decode image clipboard");

        assert_eq!(clipboard.format, ClipboardFormat::Image);
        assert_eq!(clipboard.image.as_ref().expect("image").width, 2);
        assert_eq!(clipboard.image.as_ref().expect("image").height, 1);
        assert_eq!(decoded, frame);
    }

    #[test]
    fn round_trips_file_clipboard_metadata_and_transfer_chunk() {
        let files = ClipboardFiles::new(
            42,
            10,
            vec![ClipboardFileMetadata::new(
                "design.pdf".to_string(),
                2048,
                0xabc,
            )],
        );
        let metadata_frame = ProtocolFrame {
            sequence: 13,
            timestamp_micros: 14,
            payload: ProtocolPayload::ClipboardFiles(files.clone()),
        };
        let chunk = FileTransferChunk::new(
            0xfeed,
            42,
            0,
            1024,
            2048,
            1,
            false,
            b"not logged file bytes".to_vec(),
        );
        let chunk_frame = ProtocolFrame {
            sequence: 14,
            timestamp_micros: 15,
            payload: ProtocolPayload::FileChunk(chunk.clone()),
        };

        assert_eq!(
            ProtocolFrame::decode(&metadata_frame.encode_vec()),
            Ok(metadata_frame)
        );
        assert_eq!(
            ProtocolFrame::decode(&chunk_frame.encode_vec()),
            Ok(chunk_frame)
        );
        assert_eq!(
            ProtocolPayload::ClipboardFiles(files).transport_lane(),
            TransportLane::Clipboard
        );
        assert_eq!(
            ProtocolPayload::FileChunk(chunk).transport_lane(),
            TransportLane::Clipboard
        );
    }

    #[test]
    fn encodes_input_frame_into_stack_buffer() {
        let input = InputEventEnvelope::new(
            100,
            200,
            CURRENT_PROTOCOL_VERSION,
            InputChannel::InputUnreliable,
            InputEvent::Mouse(MouseEvent::Move { dx: 5.0, dy: -6.0 }),
        );
        let frame = ProtocolFrame {
            sequence: 10,
            timestamp_micros: 11,
            payload: ProtocolPayload::Input(input),
        };
        let mut buffer = [0; 96];

        let size = frame.encode_into(&mut buffer).expect("frame should fit");

        assert_eq!(ProtocolFrame::decode(&buffer[..size]), Ok(frame));
    }

    #[test]
    fn round_trips_input_frame_metadata() {
        let frame = ProtocolFrame {
            sequence: 12,
            timestamp_micros: 13,
            payload: ProtocolPayload::Input(InputEventEnvelope::new(
                0x1111,
                0x2222,
                CURRENT_PROTOCOL_VERSION,
                InputChannel::InputReliable,
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
            )),
        };

        let bytes = frame.encode_vec();

        assert_eq!(ProtocolFrame::decode(&bytes), Ok(frame));
    }

    #[test]
    fn protocol_payloads_declare_separate_transport_lanes() {
        let clipboard = ProtocolPayload::ClipboardText(ClipboardText::new(
            42,
            9,
            "large clipboard".to_string(),
        ));
        let key = ProtocolPayload::Input(InputEventEnvelope::current(
            1,
            2,
            InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            }),
        ));
        let mouse_button = ProtocolPayload::Input(InputEventEnvelope::current(
            1,
            2,
            InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Left,
                state: KeyState::Pressed,
            }),
        ));
        let scroll = ProtocolPayload::Input(InputEventEnvelope::current(
            1,
            2,
            InputEvent::Scroll(ScrollEvent { dx: 0.0, dy: 1.0 }),
        ));
        let mouse_move = ProtocolPayload::Input(InputEventEnvelope::current(
            1,
            2,
            InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: -1.0 }),
        ));
        let mouse_position = ProtocolPayload::Input(InputEventEnvelope::current(
            1,
            2,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5,
            }),
        ));
        let control = ProtocolPayload::Control(ControlMessage::heartbeat(1, 2, 3));

        assert_eq!(key.transport_lane(), TransportLane::InputReliable);
        assert_eq!(mouse_button.transport_lane(), TransportLane::InputReliable);
        assert_eq!(scroll.transport_lane(), TransportLane::InputReliable);
        assert_eq!(mouse_move.transport_lane(), TransportLane::InputUnreliable);
        assert_eq!(
            mouse_position.transport_lane(),
            TransportLane::InputReliable
        );
        assert_eq!(clipboard.transport_lane(), TransportLane::Clipboard);
        assert_eq!(control.transport_lane(), TransportLane::Control);
        assert_ne!(key.transport_lane(), clipboard.transport_lane());
        assert_ne!(control.transport_lane(), key.transport_lane());
    }

    #[test]
    fn round_trips_control_channel_messages() {
        let messages = [
            ControlMessage::heartbeat(1, 0xabc, 9),
            ControlMessage::capabilities(1, ProtocolVersionRange::new(1, CURRENT_PROTOCOL_VERSION)),
            ControlMessage::config_version(1, 42),
            ControlMessage::session_state(1, 0xabc, ControlSessionState::Active),
        ];

        for (index, message) in messages.into_iter().enumerate() {
            let frame = ProtocolFrame {
                sequence: u64::try_from(index + 1).expect("sequence"),
                timestamp_micros: 20,
                payload: ProtocolPayload::Control(message.clone()),
            };

            assert_eq!(ProtocolFrame::decode(&frame.encode_vec()), Ok(frame));
            assert_eq!(
                ProtocolPayload::Control(message).transport_lane(),
                TransportLane::Control
            );
        }
    }

    #[test]
    fn round_trips_normalized_mouse_position_input_frame() {
        let frame = ProtocolFrame {
            sequence: 14,
            timestamp_micros: 15,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.625,
                }),
            )),
        };

        assert_eq!(ProtocolFrame::decode(&frame.encode_vec()), Ok(frame));
    }

    #[test]
    fn round_trips_extended_key_input_frame() {
        let frame = ProtocolFrame {
            sequence: 13,
            timestamp_micros: 14,
            payload: ProtocolPayload::Input(InputEventEnvelope::new(
                1,
                2,
                CURRENT_PROTOCOL_VERSION,
                InputChannel::InputReliable,
                InputEvent::Key(KeyEvent {
                    key: Key::VolumeUp,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::NONE,
                }),
            )),
        };

        assert_eq!(ProtocolFrame::decode(&frame.encode_vec()), Ok(frame));
    }

    #[test]
    fn encode_into_rejects_too_small_buffer() {
        let frame = ProtocolFrame {
            sequence: 10,
            timestamp_micros: 11,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                0,
                0,
                InputEvent::Mouse(MouseEvent::Move { dx: 5.0, dy: -6.0 }),
            )),
        };
        let mut buffer = [0; 24];

        assert_eq!(
            frame.encode_into(&mut buffer),
            Err(EncodeError::BufferTooSmall)
        );
    }

    #[test]
    fn decodes_legacy_event_as_input_frame() {
        let event = ProtocolEvent {
            sequence: 46,
            timestamp_micros: 11,
            event: InputEvent::Key(KeyEvent {
                key: Key::Escape,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        assert_eq!(
            ProtocolFrame::decode(&buffer[..size]),
            Ok(ProtocolFrame {
                sequence: event.sequence,
                timestamp_micros: event.timestamp_micros,
                payload: ProtocolPayload::Input(InputEventEnvelope::legacy(event.event)),
            })
        );
    }

    #[test]
    fn legacy_input_frame_decodes_as_protocol_version_zero() {
        let event = ProtocolEvent {
            sequence: 48,
            timestamp_micros: 13,
            event: InputEvent::Key(KeyEvent {
                key: Key::Escape,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        let frame = ProtocolFrame::decode(&buffer[..size]).expect("legacy frame");

        let ProtocolPayload::Input(input) = frame.payload else {
            panic!("expected input payload");
        };
        assert_eq!(input.protocol_version, 0);
        assert_eq!(input.event, event.event);
    }

    #[test]
    fn rejects_truncated_input_event() {
        let event = ProtocolEvent {
            sequence: 47,
            timestamp_micros: 12,
            event: InputEvent::Scroll(ScrollEvent { dx: 1.0, dy: 2.0 }),
        };
        let mut buffer = [0; 32];
        let size = event.encode(&mut buffer);

        assert_eq!(
            ProtocolEvent::decode(&buffer[..size - 1]),
            Err(DecodeError::TooShort)
        );
    }

    #[test]
    fn rejects_unknown_key() {
        let mut bytes = [0; 25];
        bytes[..4].copy_from_slice(b"SYN1");
        bytes[20] = EVENT_KEY;
        bytes[21..23].copy_from_slice(&999_u16.to_le_bytes());
        bytes[23] = 1;

        assert_eq!(
            ProtocolEvent::decode(&bytes),
            Err(DecodeError::UnknownKey(999))
        );
    }

    #[test]
    fn rejects_unknown_payload_type() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"SYN2");
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&2_u64.to_le_bytes());
        bytes.push(99);
        bytes.extend_from_slice(&0_u32.to_le_bytes());

        assert_eq!(
            ProtocolFrame::decode(&bytes),
            Err(DecodeError::UnknownEventType(99))
        );
    }

    #[test]
    fn rejects_invalid_utf8_clipboard_frame() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"SYN2");
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&2_u64.to_le_bytes());
        bytes.push(PAYLOAD_CLIPBOARD_TEXT_LEGACY);
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&[0xff, 0xff]);

        assert_eq!(ProtocolFrame::decode(&bytes), Err(DecodeError::InvalidUtf8));
    }
}
