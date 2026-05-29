use core::fmt;

pub type DeviceId = u128;
pub const KEY_CODE_SPACE: usize = 0x1100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsKind {
    MacOs,
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierSemantic {
    Control,
    Shift,
    Alt,
    Option,
    Command,
    Super,
    Fn,
    Globe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Key {
    A = 4,
    B = 5,
    C = 6,
    D = 7,
    E = 8,
    F = 9,
    G = 10,
    H = 11,
    I = 12,
    J = 13,
    K = 14,
    L = 15,
    M = 16,
    N = 17,
    O = 18,
    P = 19,
    Q = 20,
    R = 21,
    S = 22,
    T = 23,
    U = 24,
    V = 25,
    W = 26,
    X = 27,
    Y = 28,
    Z = 29,
    Num1 = 30,
    Num2 = 31,
    Num3 = 32,
    Num4 = 33,
    Num5 = 34,
    Num6 = 35,
    Num7 = 36,
    Num8 = 37,
    Num9 = 38,
    Num0 = 39,
    Enter = 40,
    Escape = 41,
    Backspace = 42,
    Tab = 43,
    Space = 44,
    Minus = 45,
    Equal = 46,
    LeftBracket = 47,
    RightBracket = 48,
    Backslash = 49,
    Semicolon = 51,
    Quote = 52,
    Grave = 53,
    Comma = 54,
    Dot = 55,
    Slash = 56,
    CapsLock = 57,
    F1 = 58,
    F2 = 59,
    F3 = 60,
    F4 = 61,
    F5 = 62,
    F6 = 63,
    F7 = 64,
    F8 = 65,
    F9 = 66,
    F10 = 67,
    F11 = 68,
    F12 = 69,
    PrintScreen = 70,
    ScrollLock = 71,
    Pause = 72,
    Insert = 73,
    Home = 74,
    PageUp = 75,
    Delete = 76,
    End = 77,
    PageDown = 78,
    ArrowRight = 79,
    ArrowLeft = 80,
    ArrowDown = 81,
    ArrowUp = 82,
    NumLock = 83,
    NumpadDivide = 84,
    NumpadMultiply = 85,
    NumpadSubtract = 86,
    NumpadAdd = 87,
    NumpadEnter = 88,
    Numpad1 = 89,
    Numpad2 = 90,
    Numpad3 = 91,
    Numpad4 = 92,
    Numpad5 = 93,
    Numpad6 = 94,
    Numpad7 = 95,
    Numpad8 = 96,
    Numpad9 = 97,
    Numpad0 = 98,
    NumpadDecimal = 99,
    NumpadEqual = 103,
    F13 = 104,
    F14 = 105,
    F15 = 106,
    F16 = 107,
    F17 = 108,
    F18 = 109,
    F19 = 110,
    F20 = 111,
    F21 = 112,
    F22 = 113,
    F23 = 114,
    F24 = 115,
    Kana = 0x0088,
    Eisu = 0x0089,
    ImeOn = 0x0090,
    ImeOff = 0x0091,
    LeftControl = 224,
    LeftShift = 225,
    LeftAlt = 226,
    LeftMeta = 227,
    RightControl = 228,
    RightShift = 229,
    RightAlt = 230,
    RightMeta = 231,
    BrightnessDown = 0x106f,
    BrightnessUp = 0x1070,
    MediaPlay = 0x10b0,
    MediaPause = 0x10b1,
    MediaRecord = 0x10b2,
    MediaFastForward = 0x10b3,
    MediaRewind = 0x10b4,
    MediaNextTrack = 0x10b5,
    MediaPreviousTrack = 0x10b6,
    MediaStop = 0x10b7,
    MediaPlayPause = 0x10cd,
    VolumeMute = 0x10e2,
    VolumeUp = 0x10e9,
    VolumeDown = 0x10ea,
    Fn = 0x10f0,
    Globe = 0x10f1,
}

impl Key {
    #[must_use]
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            4 => Self::A,
            5 => Self::B,
            6 => Self::C,
            7 => Self::D,
            8 => Self::E,
            9 => Self::F,
            10 => Self::G,
            11 => Self::H,
            12 => Self::I,
            13 => Self::J,
            14 => Self::K,
            15 => Self::L,
            16 => Self::M,
            17 => Self::N,
            18 => Self::O,
            19 => Self::P,
            20 => Self::Q,
            21 => Self::R,
            22 => Self::S,
            23 => Self::T,
            24 => Self::U,
            25 => Self::V,
            26 => Self::W,
            27 => Self::X,
            28 => Self::Y,
            29 => Self::Z,
            30 => Self::Num1,
            31 => Self::Num2,
            32 => Self::Num3,
            33 => Self::Num4,
            34 => Self::Num5,
            35 => Self::Num6,
            36 => Self::Num7,
            37 => Self::Num8,
            38 => Self::Num9,
            39 => Self::Num0,
            40 => Self::Enter,
            41 => Self::Escape,
            42 => Self::Backspace,
            43 => Self::Tab,
            44 => Self::Space,
            45 => Self::Minus,
            46 => Self::Equal,
            47 => Self::LeftBracket,
            48 => Self::RightBracket,
            49 => Self::Backslash,
            51 => Self::Semicolon,
            52 => Self::Quote,
            53 => Self::Grave,
            54 => Self::Comma,
            55 => Self::Dot,
            56 => Self::Slash,
            57 => Self::CapsLock,
            58 => Self::F1,
            59 => Self::F2,
            60 => Self::F3,
            61 => Self::F4,
            62 => Self::F5,
            63 => Self::F6,
            64 => Self::F7,
            65 => Self::F8,
            66 => Self::F9,
            67 => Self::F10,
            68 => Self::F11,
            69 => Self::F12,
            70 => Self::PrintScreen,
            71 => Self::ScrollLock,
            72 => Self::Pause,
            73 => Self::Insert,
            74 => Self::Home,
            75 => Self::PageUp,
            76 => Self::Delete,
            77 => Self::End,
            78 => Self::PageDown,
            79 => Self::ArrowRight,
            80 => Self::ArrowLeft,
            81 => Self::ArrowDown,
            82 => Self::ArrowUp,
            83 => Self::NumLock,
            84 => Self::NumpadDivide,
            85 => Self::NumpadMultiply,
            86 => Self::NumpadSubtract,
            87 => Self::NumpadAdd,
            88 => Self::NumpadEnter,
            89 => Self::Numpad1,
            90 => Self::Numpad2,
            91 => Self::Numpad3,
            92 => Self::Numpad4,
            93 => Self::Numpad5,
            94 => Self::Numpad6,
            95 => Self::Numpad7,
            96 => Self::Numpad8,
            97 => Self::Numpad9,
            98 => Self::Numpad0,
            99 => Self::NumpadDecimal,
            103 => Self::NumpadEqual,
            104 => Self::F13,
            105 => Self::F14,
            106 => Self::F15,
            107 => Self::F16,
            108 => Self::F17,
            109 => Self::F18,
            110 => Self::F19,
            111 => Self::F20,
            112 => Self::F21,
            113 => Self::F22,
            114 => Self::F23,
            115 => Self::F24,
            0x0088 => Self::Kana,
            0x0089 => Self::Eisu,
            0x0090 => Self::ImeOn,
            0x0091 => Self::ImeOff,
            224 => Self::LeftControl,
            225 => Self::LeftShift,
            226 => Self::LeftAlt,
            227 => Self::LeftMeta,
            228 => Self::RightControl,
            229 => Self::RightShift,
            230 => Self::RightAlt,
            231 => Self::RightMeta,
            0x106f => Self::BrightnessDown,
            0x1070 => Self::BrightnessUp,
            0x10b0 => Self::MediaPlay,
            0x10b1 => Self::MediaPause,
            0x10b2 => Self::MediaRecord,
            0x10b3 => Self::MediaFastForward,
            0x10b4 => Self::MediaRewind,
            0x10b5 => Self::MediaNextTrack,
            0x10b6 => Self::MediaPreviousTrack,
            0x10b7 => Self::MediaStop,
            0x10cd => Self::MediaPlayPause,
            0x10e2 => Self::VolumeMute,
            0x10e9 => Self::VolumeUp,
            0x10ea => Self::VolumeDown,
            0x10f0 => Self::Fn,
            0x10f1 => Self::Globe,
            _ => return None,
        })
    }

    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        Some(match value {
            "a" => Self::A,
            "b" => Self::B,
            "c" => Self::C,
            "d" => Self::D,
            "e" => Self::E,
            "f" => Self::F,
            "g" => Self::G,
            "h" => Self::H,
            "i" => Self::I,
            "j" => Self::J,
            "k" => Self::K,
            "l" => Self::L,
            "m" => Self::M,
            "n" => Self::N,
            "o" => Self::O,
            "p" => Self::P,
            "q" => Self::Q,
            "r" => Self::R,
            "s" => Self::S,
            "t" => Self::T,
            "u" => Self::U,
            "v" => Self::V,
            "w" => Self::W,
            "x" => Self::X,
            "y" => Self::Y,
            "z" => Self::Z,
            "0" | "num0" => Self::Num0,
            "1" | "num1" => Self::Num1,
            "2" | "num2" => Self::Num2,
            "3" | "num3" => Self::Num3,
            "4" | "num4" => Self::Num4,
            "5" | "num5" => Self::Num5,
            "6" | "num6" => Self::Num6,
            "7" | "num7" => Self::Num7,
            "8" | "num8" => Self::Num8,
            "9" | "num9" => Self::Num9,
            "enter" | "return" => Self::Enter,
            "escape" | "esc" => Self::Escape,
            "backspace" => Self::Backspace,
            "tab" => Self::Tab,
            "space" => Self::Space,
            "minus" => Self::Minus,
            "equal" => Self::Equal,
            "left_bracket" => Self::LeftBracket,
            "right_bracket" => Self::RightBracket,
            "backslash" => Self::Backslash,
            "semicolon" => Self::Semicolon,
            "quote" => Self::Quote,
            "grave" => Self::Grave,
            "comma" => Self::Comma,
            "dot" | "period" => Self::Dot,
            "slash" => Self::Slash,
            "caps_lock" | "capslock" => Self::CapsLock,
            "f1" => Self::F1,
            "f2" => Self::F2,
            "f3" => Self::F3,
            "f4" => Self::F4,
            "f5" => Self::F5,
            "f6" => Self::F6,
            "f7" => Self::F7,
            "f8" => Self::F8,
            "f9" => Self::F9,
            "f10" => Self::F10,
            "f11" => Self::F11,
            "f12" => Self::F12,
            "f13" => Self::F13,
            "f14" => Self::F14,
            "f15" => Self::F15,
            "f16" => Self::F16,
            "f17" => Self::F17,
            "f18" => Self::F18,
            "f19" => Self::F19,
            "f20" => Self::F20,
            "f21" => Self::F21,
            "f22" => Self::F22,
            "f23" => Self::F23,
            "f24" => Self::F24,
            "kana" | "lang1" => Self::Kana,
            "eisu" | "lang2" => Self::Eisu,
            "ime_on" | "imeon" => Self::ImeOn,
            "ime_off" | "imeoff" => Self::ImeOff,
            "print_screen" | "printscreen" => Self::PrintScreen,
            "scroll_lock" | "scrolllock" => Self::ScrollLock,
            "pause" => Self::Pause,
            "insert" | "ins" => Self::Insert,
            "home" => Self::Home,
            "page_up" | "pageup" | "pgup" => Self::PageUp,
            "delete" | "del" => Self::Delete,
            "end" => Self::End,
            "page_down" | "pagedown" | "pgdn" => Self::PageDown,
            "arrow_right" | "arrowright" | "right" => Self::ArrowRight,
            "arrow_left" | "arrowleft" | "left" => Self::ArrowLeft,
            "arrow_down" | "arrowdown" | "down" => Self::ArrowDown,
            "arrow_up" | "arrowup" | "up" => Self::ArrowUp,
            "num_lock" | "numlock" => Self::NumLock,
            "numpad_divide" | "keypad_divide" => Self::NumpadDivide,
            "numpad_multiply" | "keypad_multiply" => Self::NumpadMultiply,
            "numpad_subtract" | "keypad_subtract" => Self::NumpadSubtract,
            "numpad_add" | "keypad_add" => Self::NumpadAdd,
            "numpad_enter" | "keypad_enter" => Self::NumpadEnter,
            "numpad_0" | "numpad0" | "keypad_0" | "keypad0" => Self::Numpad0,
            "numpad_1" | "numpad1" | "keypad_1" | "keypad1" => Self::Numpad1,
            "numpad_2" | "numpad2" | "keypad_2" | "keypad2" => Self::Numpad2,
            "numpad_3" | "numpad3" | "keypad_3" | "keypad3" => Self::Numpad3,
            "numpad_4" | "numpad4" | "keypad_4" | "keypad4" => Self::Numpad4,
            "numpad_5" | "numpad5" | "keypad_5" | "keypad5" => Self::Numpad5,
            "numpad_6" | "numpad6" | "keypad_6" | "keypad6" => Self::Numpad6,
            "numpad_7" | "numpad7" | "keypad_7" | "keypad7" => Self::Numpad7,
            "numpad_8" | "numpad8" | "keypad_8" | "keypad8" => Self::Numpad8,
            "numpad_9" | "numpad9" | "keypad_9" | "keypad9" => Self::Numpad9,
            "numpad_decimal" | "keypad_decimal" => Self::NumpadDecimal,
            "numpad_equal" | "keypad_equal" => Self::NumpadEqual,
            "left_control" => Self::LeftControl,
            "left_shift" => Self::LeftShift,
            "left_alt" => Self::LeftAlt,
            "left_meta" => Self::LeftMeta,
            "right_control" => Self::RightControl,
            "right_shift" => Self::RightShift,
            "right_alt" => Self::RightAlt,
            "right_meta" => Self::RightMeta,
            "brightness_down" => Self::BrightnessDown,
            "brightness_up" => Self::BrightnessUp,
            "media_play" => Self::MediaPlay,
            "media_pause" => Self::MediaPause,
            "media_record" => Self::MediaRecord,
            "media_fast_forward" => Self::MediaFastForward,
            "media_rewind" => Self::MediaRewind,
            "media_next_track" | "media_next" => Self::MediaNextTrack,
            "media_previous_track" | "media_previous" | "media_prev" => Self::MediaPreviousTrack,
            "media_stop" => Self::MediaStop,
            "media_play_pause" => Self::MediaPlayPause,
            "volume_mute" | "mute" => Self::VolumeMute,
            "volume_up" => Self::VolumeUp,
            "volume_down" => Self::VolumeDown,
            "fn" | "function" => Self::Fn,
            "globe" => Self::Globe,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn modifier_semantic(self, os: OsKind) -> Option<ModifierSemantic> {
        match self {
            Self::LeftControl | Self::RightControl => Some(ModifierSemantic::Control),
            Self::LeftShift | Self::RightShift => Some(ModifierSemantic::Shift),
            Self::LeftAlt | Self::RightAlt => match os {
                OsKind::MacOs => Some(ModifierSemantic::Option),
                OsKind::Windows | OsKind::Linux => Some(ModifierSemantic::Alt),
            },
            Self::LeftMeta | Self::RightMeta => match os {
                OsKind::MacOs => Some(ModifierSemantic::Command),
                OsKind::Windows | OsKind::Linux => Some(ModifierSemantic::Super),
            },
            Self::Fn => Some(ModifierSemantic::Fn),
            Self::Globe => match os {
                OsKind::MacOs => Some(ModifierSemantic::Globe),
                OsKind::Windows | OsKind::Linux => None,
            },
            _ => None,
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::Num1 => "1",
            Self::Num2 => "2",
            Self::Num3 => "3",
            Self::Num4 => "4",
            Self::Num5 => "5",
            Self::Num6 => "6",
            Self::Num7 => "7",
            Self::Num8 => "8",
            Self::Num9 => "9",
            Self::Num0 => "0",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Backspace => "Backspace",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::Minus => "Minus",
            Self::Equal => "Equal",
            Self::LeftBracket => "Left Bracket",
            Self::RightBracket => "Right Bracket",
            Self::Backslash => "Backslash",
            Self::Semicolon => "Semicolon",
            Self::Quote => "Quote",
            Self::Grave => "Grave",
            Self::Comma => "Comma",
            Self::Dot => "Dot",
            Self::Slash => "Slash",
            Self::CapsLock => "Caps Lock",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::PrintScreen => "Print Screen",
            Self::ScrollLock => "Scroll Lock",
            Self::Pause => "Pause",
            Self::Insert => "Insert",
            Self::Home => "Home",
            Self::PageUp => "Page Up",
            Self::Delete => "Delete",
            Self::End => "End",
            Self::PageDown => "Page Down",
            Self::ArrowRight => "Arrow Right",
            Self::ArrowLeft => "Arrow Left",
            Self::ArrowDown => "Arrow Down",
            Self::ArrowUp => "Arrow Up",
            Self::NumLock => "Num Lock",
            Self::NumpadDivide => "Numpad Divide",
            Self::NumpadMultiply => "Numpad Multiply",
            Self::NumpadSubtract => "Numpad Subtract",
            Self::NumpadAdd => "Numpad Add",
            Self::NumpadEnter => "Numpad Enter",
            Self::Numpad1 => "Numpad 1",
            Self::Numpad2 => "Numpad 2",
            Self::Numpad3 => "Numpad 3",
            Self::Numpad4 => "Numpad 4",
            Self::Numpad5 => "Numpad 5",
            Self::Numpad6 => "Numpad 6",
            Self::Numpad7 => "Numpad 7",
            Self::Numpad8 => "Numpad 8",
            Self::Numpad9 => "Numpad 9",
            Self::Numpad0 => "Numpad 0",
            Self::NumpadDecimal => "Numpad Decimal",
            Self::NumpadEqual => "Numpad Equal",
            Self::F13 => "F13",
            Self::F14 => "F14",
            Self::F15 => "F15",
            Self::F16 => "F16",
            Self::F17 => "F17",
            Self::F18 => "F18",
            Self::F19 => "F19",
            Self::F20 => "F20",
            Self::F21 => "F21",
            Self::F22 => "F22",
            Self::F23 => "F23",
            Self::F24 => "F24",
            Self::Kana => "Kana",
            Self::Eisu => "Eisu",
            Self::ImeOn => "IME On",
            Self::ImeOff => "IME Off",
            Self::LeftControl => "Left Control",
            Self::LeftShift => "Left Shift",
            Self::LeftAlt => "Left Alt",
            Self::LeftMeta => "Left Meta",
            Self::RightControl => "Right Control",
            Self::RightShift => "Right Shift",
            Self::RightAlt => "Right Alt",
            Self::RightMeta => "Right Meta",
            Self::BrightnessDown => "Brightness Down",
            Self::BrightnessUp => "Brightness Up",
            Self::MediaPlay => "Media Play",
            Self::MediaPause => "Media Pause",
            Self::MediaRecord => "Media Record",
            Self::MediaFastForward => "Media Fast Forward",
            Self::MediaRewind => "Media Rewind",
            Self::MediaNextTrack => "Media Next Track",
            Self::MediaPreviousTrack => "Media Previous Track",
            Self::MediaStop => "Media Stop",
            Self::MediaPlayPause => "Media Play/Pause",
            Self::VolumeMute => "Volume Mute",
            Self::VolumeUp => "Volume Up",
            Self::VolumeDown => "Volume Down",
            Self::Fn => "Fn",
            Self::Globe => "Globe",
        }
    }
}

impl ModifierSemantic {
    #[must_use]
    pub const fn preferred_key(self, os: OsKind, side: ModifierSide) -> Option<Key> {
        match self {
            Self::Control => Some(side_key(side, Key::LeftControl, Key::RightControl)),
            Self::Shift => Some(side_key(side, Key::LeftShift, Key::RightShift)),
            Self::Alt => Some(side_key(side, Key::LeftAlt, Key::RightAlt)),
            Self::Option => Some(side_key(side, Key::LeftAlt, Key::RightAlt)),
            Self::Command => match os {
                OsKind::MacOs => Some(side_key(side, Key::LeftMeta, Key::RightMeta)),
                OsKind::Windows | OsKind::Linux => {
                    Some(side_key(side, Key::LeftControl, Key::RightControl))
                }
            },
            Self::Super => Some(side_key(side, Key::LeftMeta, Key::RightMeta)),
            Self::Fn => Some(Key::Fn),
            Self::Globe => match os {
                OsKind::MacOs => Some(Key::Globe),
                OsKind::Windows | OsKind::Linux => None,
            },
        }
    }
}

const fn side_key(side: ModifierSide, left: Key, right: Key) -> Key {
    match side {
        ModifierSide::Left => left,
        ModifierSide::Right => right,
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const CONTROL: Self = Self(1 << 0);
    pub const SHIFT: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const META: Self = Self(1 << 3);

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & 0x0f)
    }

    #[must_use]
    pub const fn contains(self, modifier: Self) -> bool {
        self.0 & modifier.0 != 0
    }

    #[must_use]
    pub const fn with(self, modifier: Self) -> Self {
        Self(self.0 | modifier.0)
    }

    #[must_use]
    pub const fn without(self, modifier: Self) -> Self {
        Self(self.0 & !modifier.0)
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = [""; 4];
        let mut count = 0;
        if self.contains(Self::CONTROL) {
            parts[count] = "Control";
            count += 1;
        }
        if self.contains(Self::SHIFT) {
            parts[count] = "Shift";
            count += 1;
        }
        if self.contains(Self::ALT) {
            parts[count] = "Alt";
            count += 1;
        }
        if self.contains(Self::META) {
            parts[count] = "Meta";
            count += 1;
        }
        f.debug_tuple("Modifiers").field(&&parts[..count]).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub state: KeyState,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseEvent {
    Move {
        dx: f32,
        dy: f32,
    },
    Position {
        x_ratio: f32,
        y_ratio: f32,
    },
    Button {
        button: MouseButton,
        state: KeyState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollEvent {
    pub dx: f32,
    pub dy: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Scroll(ScrollEvent),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_model_includes_numpad_function_media_brightness_and_volume_keys() {
        assert_eq!(Key::from_u16(Key::Numpad0 as u16), Some(Key::Numpad0));
        assert_eq!(
            Key::from_u16(Key::NumpadEnter as u16),
            Some(Key::NumpadEnter)
        );
        assert_eq!(Key::from_u16(Key::F13 as u16), Some(Key::F13));
        assert_eq!(Key::from_u16(Key::F24 as u16), Some(Key::F24));
        assert_eq!(
            Key::from_u16(Key::MediaPlayPause as u16),
            Some(Key::MediaPlayPause)
        );
        assert_eq!(
            Key::from_u16(Key::MediaNextTrack as u16),
            Some(Key::MediaNextTrack)
        );
        assert_eq!(
            Key::from_u16(Key::BrightnessUp as u16),
            Some(Key::BrightnessUp)
        );
        assert_eq!(Key::from_u16(Key::VolumeUp as u16), Some(Key::VolumeUp));
    }

    #[test]
    fn key_display_names_distinguish_left_and_right_modifiers() {
        assert_eq!(Key::LeftControl.display_name(), "Left Control");
        assert_eq!(Key::RightControl.display_name(), "Right Control");
        assert_eq!(Key::LeftShift.display_name(), "Left Shift");
        assert_eq!(Key::RightShift.display_name(), "Right Shift");
        assert_eq!(Key::LeftAlt.display_name(), "Left Alt");
        assert_eq!(Key::RightAlt.display_name(), "Right Alt");
        assert_eq!(Key::LeftMeta.display_name(), "Left Meta");
        assert_eq!(Key::RightMeta.display_name(), "Right Meta");
        assert_eq!(Key::MediaPlayPause.display_name(), "Media Play/Pause");
        assert_eq!(format!("{}", Key::RightMeta), "Right Meta");
    }

    #[test]
    fn key_model_includes_macos_fn_and_globe_keys() {
        assert_eq!(Key::from_u16(Key::Fn as u16), Some(Key::Fn));
        assert_eq!(Key::from_u16(Key::Globe as u16), Some(Key::Globe));
        assert_eq!(Key::from_name("fn"), Some(Key::Fn));
        assert_eq!(Key::from_name("globe"), Some(Key::Globe));
        assert_eq!(Key::Fn.display_name(), "Fn");
        assert_eq!(Key::Globe.display_name(), "Globe");
    }

    #[test]
    fn key_model_includes_caps_lock_input_method_and_system_keys() {
        assert_eq!(Key::from_u16(Key::CapsLock as u16), Some(Key::CapsLock));
        assert_eq!(Key::from_u16(Key::Kana as u16), Some(Key::Kana));
        assert_eq!(Key::from_u16(Key::Eisu as u16), Some(Key::Eisu));
        assert_eq!(Key::from_u16(Key::ImeOn as u16), Some(Key::ImeOn));
        assert_eq!(Key::from_u16(Key::ImeOff as u16), Some(Key::ImeOff));
        assert_eq!(
            Key::from_u16(Key::PrintScreen as u16),
            Some(Key::PrintScreen)
        );
        assert_eq!(Key::from_u16(Key::ScrollLock as u16), Some(Key::ScrollLock));
        assert_eq!(Key::from_u16(Key::Pause as u16), Some(Key::Pause));
        assert_eq!(Key::from_name("kana"), Some(Key::Kana));
        assert_eq!(Key::from_name("eisu"), Some(Key::Eisu));
        assert_eq!(Key::from_name("ime_on"), Some(Key::ImeOn));
        assert_eq!(Key::from_name("ime_off"), Some(Key::ImeOff));
        assert_eq!(Key::Kana.display_name(), "Kana");
        assert_eq!(Key::Eisu.display_name(), "Eisu");
        assert_eq!(Key::ImeOn.display_name(), "IME On");
        assert_eq!(Key::ImeOff.display_name(), "IME Off");
    }

    #[test]
    fn modifier_semantics_are_defined_per_operating_system() {
        assert_eq!(
            Key::LeftMeta.modifier_semantic(OsKind::MacOs),
            Some(ModifierSemantic::Command)
        );
        assert_eq!(
            Key::RightMeta.modifier_semantic(OsKind::Windows),
            Some(ModifierSemantic::Super)
        );
        assert_eq!(
            Key::LeftAlt.modifier_semantic(OsKind::MacOs),
            Some(ModifierSemantic::Option)
        );
        assert_eq!(
            Key::RightAlt.modifier_semantic(OsKind::Windows),
            Some(ModifierSemantic::Alt)
        );
        assert_eq!(
            Key::LeftControl.modifier_semantic(OsKind::Linux),
            Some(ModifierSemantic::Control)
        );
        assert_eq!(
            Key::Fn.modifier_semantic(OsKind::MacOs),
            Some(ModifierSemantic::Fn)
        );
        assert_eq!(
            Key::Globe.modifier_semantic(OsKind::MacOs),
            Some(ModifierSemantic::Globe)
        );
        assert_eq!(Key::C.modifier_semantic(OsKind::Windows), None);
    }

    #[test]
    fn modifier_semantics_resolve_to_preferred_platform_keys() {
        assert_eq!(
            ModifierSemantic::Command.preferred_key(OsKind::MacOs, ModifierSide::Left),
            Some(Key::LeftMeta)
        );
        assert_eq!(
            ModifierSemantic::Command.preferred_key(OsKind::Windows, ModifierSide::Left),
            Some(Key::LeftControl)
        );
        assert_eq!(
            ModifierSemantic::Option.preferred_key(OsKind::Windows, ModifierSide::Right),
            Some(Key::RightAlt)
        );
        assert_eq!(
            ModifierSemantic::Super.preferred_key(OsKind::MacOs, ModifierSide::Right),
            Some(Key::RightMeta)
        );
        assert_eq!(
            ModifierSemantic::Globe.preferred_key(OsKind::MacOs, ModifierSide::Left),
            Some(Key::Globe)
        );
        assert_eq!(
            ModifierSemantic::Globe.preferred_key(OsKind::Windows, ModifierSide::Left),
            None
        );
    }
}
