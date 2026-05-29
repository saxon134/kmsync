use crate::event::{
    InputEvent, Key, KeyEvent, KeyState, Modifiers, OsKind, ScrollEvent, KEY_CODE_SPACE,
};
use crate::profile::{
    FunctionKeyMode, KeyboardLayout, KeyboardMode, NonEnglishInputStrategy, Profile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingError {
    DuplicateSourceKey(Key),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    Copy,
    Paste,
    Cut,
    Undo,
    Redo,
    SelectAll,
    Find,
    SwitchApp,
}

impl ShortcutAction {
    #[must_use]
    pub fn from_key_event(source_os: OsKind, event: KeyEvent) -> Option<Self> {
        match source_os {
            OsKind::MacOs => Self::from_macos_key_event(event),
            OsKind::Windows | OsKind::Linux => Self::from_windows_like_key_event(event),
        }
    }

    #[must_use]
    pub const fn key_event_for_os(self, target_os: OsKind, state: KeyState) -> KeyEvent {
        let (key, modifiers) = match self {
            Self::Copy => (Key::C, primary_shortcut_modifier(target_os)),
            Self::Paste => (Key::V, primary_shortcut_modifier(target_os)),
            Self::Cut => (Key::X, primary_shortcut_modifier(target_os)),
            Self::Undo => (Key::Z, primary_shortcut_modifier(target_os)),
            Self::Redo => redo_shortcut(target_os),
            Self::SelectAll => (Key::A, primary_shortcut_modifier(target_os)),
            Self::Find => (Key::F, primary_shortcut_modifier(target_os)),
            Self::SwitchApp => switch_app_shortcut(target_os),
        };
        KeyEvent {
            key,
            state,
            modifiers,
        }
    }

    fn from_macos_key_event(event: KeyEvent) -> Option<Self> {
        if event.key == Key::Tab && event.modifiers == Modifiers::META {
            return Some(Self::SwitchApp);
        }
        if event.key == Key::Z && event.modifiers == Modifiers::META.with(Modifiers::SHIFT) {
            return Some(Self::Redo);
        }
        if event.modifiers != Modifiers::META {
            return None;
        }
        Self::from_primary_shortcut_key(event.key)
    }

    fn from_windows_like_key_event(event: KeyEvent) -> Option<Self> {
        if event.key == Key::Tab && event.modifiers == Modifiers::ALT {
            return Some(Self::SwitchApp);
        }
        if event.key == Key::Y && event.modifiers == Modifiers::CONTROL {
            return Some(Self::Redo);
        }
        if event.modifiers != Modifiers::CONTROL {
            return None;
        }
        Self::from_primary_shortcut_key(event.key)
    }

    const fn from_primary_shortcut_key(key: Key) -> Option<Self> {
        match key {
            Key::C => Some(Self::Copy),
            Key::V => Some(Self::Paste),
            Key::X => Some(Self::Cut),
            Key::Z => Some(Self::Undo),
            Key::A => Some(Self::SelectAll),
            Key::F => Some(Self::Find),
            _ => None,
        }
    }
}

const fn primary_shortcut_modifier(os: OsKind) -> Modifiers {
    match os {
        OsKind::MacOs => Modifiers::META,
        OsKind::Windows | OsKind::Linux => Modifiers::CONTROL,
    }
}

const fn redo_shortcut(os: OsKind) -> (Key, Modifiers) {
    match os {
        OsKind::MacOs => (Key::Z, Modifiers::META.with(Modifiers::SHIFT)),
        OsKind::Windows | OsKind::Linux => (Key::Y, Modifiers::CONTROL),
    }
}

const fn switch_app_shortcut(os: OsKind) -> (Key, Modifiers) {
    match os {
        OsKind::MacOs => (Key::Tab, Modifiers::META),
        OsKind::Windows | OsKind::Linux => (Key::Tab, Modifiers::ALT),
    }
}

#[derive(Debug, Clone)]
pub struct CompiledProfile {
    source_os: OsKind,
    target_os: OsKind,
    function_key_mode: FunctionKeyMode,
    keyboard_mode: KeyboardMode,
    source_keyboard_layout: KeyboardLayout,
    target_keyboard_layout: KeyboardLayout,
    non_english_input_strategy: NonEnglishInputStrategy,
    key_map: [Key; KEY_CODE_SPACE],
    scroll_y_multiplier: f32,
    scroll_x_multiplier: f32,
    pointer_multiplier: f32,
}

impl CompiledProfile {
    pub fn compile(profile: &Profile) -> Result<Self, MappingError> {
        let mut key_map = [Key::A; KEY_CODE_SPACE];
        for index in 0..KEY_CODE_SPACE {
            if let Some(key) = Key::from_u16(index as u16) {
                key_map[index] = key;
            }
        }

        let mut seen = [false; KEY_CODE_SPACE];
        for mapping in &profile.key_mappings {
            let from_index = mapping.from as usize;
            if seen[from_index] {
                return Err(MappingError::DuplicateSourceKey(mapping.from));
            }
            seen[from_index] = true;
            key_map[from_index] = mapping.to;
        }

        Ok(Self {
            source_os: profile.source_os,
            target_os: profile.target_os,
            function_key_mode: profile.function_key_mode,
            keyboard_mode: profile.keyboard_mode,
            source_keyboard_layout: profile.source_keyboard_layout,
            target_keyboard_layout: profile.target_keyboard_layout,
            non_english_input_strategy: profile.non_english_input_strategy,
            key_map,
            scroll_y_multiplier: profile.scroll.vertical_multiplier,
            scroll_x_multiplier: profile.scroll.horizontal_multiplier,
            pointer_multiplier: profile.pointer.speed_multiplier,
        })
    }

    #[must_use]
    pub fn transform(&self, event: InputEvent) -> InputEvent {
        match event {
            InputEvent::Key(key_event) => InputEvent::Key(self.transform_key_event(key_event)),
            InputEvent::Scroll(scroll_event) => InputEvent::Scroll(ScrollEvent {
                dx: scroll_event.dx * self.scroll_x_multiplier,
                dy: scroll_event.dy * self.scroll_y_multiplier,
            }),
            InputEvent::Mouse(crate::event::MouseEvent::Move { dx, dy }) => {
                InputEvent::Mouse(crate::event::MouseEvent::Move {
                    dx: dx * self.pointer_multiplier,
                    dy: dy * self.pointer_multiplier,
                })
            }
            other => other,
        }
    }

    #[must_use]
    pub fn transform_key(&self, key: Key) -> Key {
        if self.function_key_mode == FunctionKeyMode::Media {
            if let Some(media_key) = function_row_media_key(key) {
                return media_key;
            }
        }

        self.key_map[key as usize]
    }

    #[must_use]
    pub fn transform_modifiers(&self, modifiers: Modifiers) -> Modifiers {
        let mut output = Modifiers::NONE;

        if modifiers.contains(Modifiers::SHIFT) {
            output = output.with(Modifiers::SHIFT);
        }
        if modifiers.contains(Modifiers::CONTROL) {
            output = output.with(Self::modifier_for_key(self.transform_key(Key::LeftControl)));
        }
        if modifiers.contains(Modifiers::ALT) {
            output = output.with(Self::modifier_for_key(self.transform_key(Key::LeftAlt)));
        }
        if modifiers.contains(Modifiers::META) {
            output = output.with(Self::modifier_for_key(self.transform_key(Key::LeftMeta)));
        }

        output
    }

    #[must_use]
    pub fn transform_key_event(&self, event: KeyEvent) -> KeyEvent {
        if self.keyboard_mode == KeyboardMode::Text {
            if let Some(action) = ShortcutAction::from_key_event(self.source_os, event) {
                return action.key_event_for_os(self.target_os, event.state);
            }
        }

        if event.modifiers == Modifiers::NONE && self.function_key_mode == FunctionKeyMode::Media {
            if let Some(key) = function_row_media_key(event.key) {
                return KeyEvent { key, ..event };
            }
        }

        KeyEvent {
            key: self.transform_key(event.key),
            state: event.state,
            modifiers: self.transform_modifiers(event.modifiers),
        }
    }

    #[must_use]
    pub const fn source_keyboard_layout(&self) -> KeyboardLayout {
        self.source_keyboard_layout
    }

    #[must_use]
    pub const fn target_keyboard_layout(&self) -> KeyboardLayout {
        self.target_keyboard_layout
    }

    #[must_use]
    pub const fn non_english_input_strategy(&self) -> NonEnglishInputStrategy {
        self.non_english_input_strategy
    }

    #[must_use]
    pub const fn prefers_unicode_text_input(&self) -> bool {
        matches!(self.keyboard_mode, KeyboardMode::Text)
            && matches!(
                self.non_english_input_strategy,
                NonEnglishInputStrategy::UnicodeText
            )
    }

    const fn modifier_for_key(key: Key) -> Modifiers {
        match key {
            Key::LeftControl | Key::RightControl => Modifiers::CONTROL,
            Key::LeftShift | Key::RightShift => Modifiers::SHIFT,
            Key::LeftAlt | Key::RightAlt => Modifiers::ALT,
            Key::LeftMeta | Key::RightMeta => Modifiers::META,
            _ => Modifiers::NONE,
        }
    }
}

const fn function_row_media_key(key: Key) -> Option<Key> {
    match key {
        Key::F1 => Some(Key::BrightnessDown),
        Key::F2 => Some(Key::BrightnessUp),
        Key::F7 => Some(Key::MediaPreviousTrack),
        Key::F8 => Some(Key::MediaPlayPause),
        Key::F9 => Some(Key::MediaNextTrack),
        Key::F10 => Some(Key::VolumeMute),
        Key::F11 => Some(Key::VolumeDown),
        Key::F12 => Some(Key::VolumeUp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{
        DeviceLayout, HabitPreset, KeyMapping, KeyboardMode, PointerSettings, ScrollSettings,
    };

    #[test]
    fn maps_mac_command_to_windows_control() {
        let profile = Profile::mac_to_windows_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let event = InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::META,
        });

        let InputEvent::Key(mapped) = compiled.transform(event) else {
            panic!("expected key event");
        };

        assert_eq!(mapped.key, Key::C);
        assert!(mapped.modifiers.contains(Modifiers::CONTROL));
        assert!(!mapped.modifiers.contains(Modifiers::META));
    }

    #[test]
    fn maps_right_mac_command_to_right_windows_control() {
        let profile = Profile::mac_to_windows_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform_key_event(KeyEvent {
            key: Key::RightMeta,
            state: KeyState::Pressed,
            modifiers: Modifiers::META,
        });

        assert_eq!(mapped.key, Key::RightControl);
        assert!(mapped.modifiers.contains(Modifiers::CONTROL));
        assert!(!mapped.modifiers.contains(Modifiers::META));
    }

    #[test]
    fn maps_windows_control_to_macos_command() {
        let profile = Profile::windows_to_macos_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let event = InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::CONTROL,
        });

        let InputEvent::Key(mapped) = compiled.transform(event) else {
            panic!("expected key event");
        };

        assert_eq!(mapped.key, Key::C);
        assert!(mapped.modifiers.contains(Modifiers::META));
        assert!(!mapped.modifiers.contains(Modifiers::CONTROL));
    }

    #[test]
    fn maps_right_windows_control_to_right_macos_command() {
        let profile = Profile::windows_to_macos_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform_key_event(KeyEvent {
            key: Key::RightControl,
            state: KeyState::Pressed,
            modifiers: Modifiers::CONTROL,
        });

        assert_eq!(mapped.key, Key::RightMeta);
        assert!(mapped.modifiers.contains(Modifiers::META));
        assert!(!mapped.modifiers.contains(Modifiers::CONTROL));
    }

    #[test]
    fn preserves_shift_while_mapping_command_to_control() {
        let profile = Profile::mac_to_windows_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform_key_event(KeyEvent {
            key: Key::P,
            state: KeyState::Pressed,
            modifiers: Modifiers::SHIFT.with(Modifiers::META),
        });

        assert_eq!(mapped.key, Key::P);
        assert!(mapped.modifiers.contains(Modifiers::SHIFT));
        assert!(mapped.modifiers.contains(Modifiers::CONTROL));
        assert!(!mapped.modifiers.contains(Modifiers::META));
    }

    #[test]
    fn maps_scroll_direction() {
        let profile = Profile::mac_to_windows_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform(InputEvent::Scroll(ScrollEvent { dx: 1.0, dy: 2.0 }));

        assert_eq!(
            mapped,
            InputEvent::Scroll(ScrollEvent { dx: -1.0, dy: -2.0 })
        );
    }

    #[test]
    fn maps_horizontal_and_vertical_scroll_independently() {
        let profile = Profile {
            source_os: OsKind::MacOs,
            target_os: OsKind::Windows,
            preset: HabitPreset::Custom,
            key_mappings: Vec::new(),
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings {
                horizontal_multiplier: 2.0,
                vertical_multiplier: -0.5,
            },
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform(InputEvent::Scroll(ScrollEvent { dx: 3.0, dy: 4.0 }));

        assert_eq!(
            mapped,
            InputEvent::Scroll(ScrollEvent { dx: 6.0, dy: -2.0 })
        );
    }

    #[test]
    fn maps_pointer_speed_multiplier() {
        let profile = Profile {
            source_os: OsKind::Windows,
            target_os: OsKind::MacOs,
            preset: HabitPreset::Custom,
            key_mappings: Vec::new(),
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings::default(),
            pointer: PointerSettings {
                speed_multiplier: 1.5,
            },
            device_layout: DeviceLayout::default(),
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform(InputEvent::Mouse(crate::event::MouseEvent::Move {
            dx: 2.0,
            dy: -4.0,
        }));

        assert_eq!(
            mapped,
            InputEvent::Mouse(crate::event::MouseEvent::Move { dx: 3.0, dy: -6.0 })
        );
    }

    #[test]
    fn rejects_duplicate_source_key() {
        let profile = Profile {
            source_os: OsKind::MacOs,
            target_os: OsKind::Windows,
            preset: HabitPreset::Custom,
            key_mappings: vec![
                KeyMapping {
                    from: Key::LeftMeta,
                    to: Key::LeftControl,
                },
                KeyMapping {
                    from: Key::LeftMeta,
                    to: Key::LeftAlt,
                },
            ],
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings::default(),
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        };

        assert!(matches!(
            CompiledProfile::compile(&profile),
            Err(MappingError::DuplicateSourceKey(Key::LeftMeta))
        ));
    }

    #[test]
    fn preserves_extended_key_model_entries() {
        let profile = Profile {
            source_os: OsKind::Windows,
            target_os: OsKind::MacOs,
            preset: HabitPreset::Custom,
            key_mappings: Vec::new(),
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings::default(),
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        assert_eq!(compiled.transform_key(Key::NumpadEnter), Key::NumpadEnter);
        assert_eq!(compiled.transform_key(Key::F24), Key::F24);
        assert_eq!(compiled.transform_key(Key::VolumeUp), Key::VolumeUp);
        assert_eq!(
            compiled.transform_key(Key::MediaPlayPause),
            Key::MediaPlayPause
        );
    }

    #[test]
    fn standard_function_key_row_keeps_f_keys() {
        let profile = Profile {
            source_os: OsKind::MacOs,
            target_os: OsKind::Windows,
            preset: HabitPreset::Custom,
            key_mappings: Vec::new(),
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings::default(),
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        assert_eq!(compiled.transform_key(Key::F1), Key::F1);
        assert_eq!(compiled.transform_key(Key::F8), Key::F8);
        assert_eq!(compiled.transform_key(Key::F12), Key::F12);
    }

    #[test]
    fn media_function_key_row_maps_to_system_media_keys() {
        let profile = Profile {
            source_os: OsKind::MacOs,
            target_os: OsKind::Windows,
            preset: HabitPreset::Custom,
            key_mappings: Vec::new(),
            function_key_mode: FunctionKeyMode::Media,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings::default(),
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        for (source_key, target_key) in [
            (Key::F1, Key::BrightnessDown),
            (Key::F2, Key::BrightnessUp),
            (Key::F7, Key::MediaPreviousTrack),
            (Key::F8, Key::MediaPlayPause),
            (Key::F9, Key::MediaNextTrack),
            (Key::F10, Key::VolumeMute),
            (Key::F11, Key::VolumeDown),
            (Key::F12, Key::VolumeUp),
        ] {
            let mapped = compiled.transform_key_event(KeyEvent {
                key: source_key,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            });

            assert_eq!(mapped.key, target_key);
            assert_eq!(mapped.modifiers, Modifiers::NONE);
        }
    }

    #[test]
    fn maps_mac_shortcut_semantics_to_windows() {
        let profile = Profile::mac_to_windows_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        for (source_key, source_modifiers, target_key, target_modifiers) in [
            (Key::C, Modifiers::META, Key::C, Modifiers::CONTROL),
            (Key::V, Modifiers::META, Key::V, Modifiers::CONTROL),
            (Key::X, Modifiers::META, Key::X, Modifiers::CONTROL),
            (Key::Z, Modifiers::META, Key::Z, Modifiers::CONTROL),
            (
                Key::Z,
                Modifiers::META.with(Modifiers::SHIFT),
                Key::Y,
                Modifiers::CONTROL,
            ),
            (Key::A, Modifiers::META, Key::A, Modifiers::CONTROL),
            (Key::F, Modifiers::META, Key::F, Modifiers::CONTROL),
            (Key::Tab, Modifiers::META, Key::Tab, Modifiers::ALT),
        ] {
            let mapped = compiled.transform_key_event(KeyEvent {
                key: source_key,
                state: KeyState::Pressed,
                modifiers: source_modifiers,
            });

            assert_eq!(mapped.key, target_key);
            assert_eq!(mapped.modifiers, target_modifiers);
        }
    }

    #[test]
    fn physical_keyboard_mode_preserves_physical_redo_chord() {
        let profile = Profile {
            keyboard_mode: KeyboardMode::Physical,
            ..Profile::mac_to_windows_default()
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform_key_event(KeyEvent {
            key: Key::Z,
            state: KeyState::Pressed,
            modifiers: Modifiers::META.with(Modifiers::SHIFT),
        });

        assert_eq!(mapped.key, Key::Z);
        assert_eq!(mapped.modifiers, Modifiers::CONTROL.with(Modifiers::SHIFT));
    }

    #[test]
    fn text_keyboard_mode_maps_redo_shortcut_semantics() {
        let profile = Profile {
            keyboard_mode: KeyboardMode::Text,
            ..Profile::mac_to_windows_default()
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        let mapped = compiled.transform_key_event(KeyEvent {
            key: Key::Z,
            state: KeyState::Pressed,
            modifiers: Modifiers::META.with(Modifiers::SHIFT),
        });

        assert_eq!(mapped.key, Key::Y);
        assert_eq!(mapped.modifiers, Modifiers::CONTROL);
    }

    #[test]
    fn compiled_profile_exposes_keyboard_layout_and_text_input_strategy() {
        let profile = Profile {
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::Jis,
            target_keyboard_layout: KeyboardLayout::Iso,
            non_english_input_strategy: NonEnglishInputStrategy::UnicodeText,
            ..Profile::mac_to_windows_default()
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        assert_eq!(compiled.source_keyboard_layout(), KeyboardLayout::Jis);
        assert_eq!(compiled.target_keyboard_layout(), KeyboardLayout::Iso);
        assert_eq!(
            compiled.non_english_input_strategy(),
            NonEnglishInputStrategy::UnicodeText
        );
        assert!(compiled.prefers_unicode_text_input());
    }

    #[test]
    fn maps_windows_shortcut_semantics_to_macos() {
        let profile = Profile::windows_to_macos_default();
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        for (source_key, source_modifiers, target_key, target_modifiers) in [
            (Key::C, Modifiers::CONTROL, Key::C, Modifiers::META),
            (Key::V, Modifiers::CONTROL, Key::V, Modifiers::META),
            (Key::X, Modifiers::CONTROL, Key::X, Modifiers::META),
            (Key::Z, Modifiers::CONTROL, Key::Z, Modifiers::META),
            (
                Key::Y,
                Modifiers::CONTROL,
                Key::Z,
                Modifiers::META.with(Modifiers::SHIFT),
            ),
            (Key::A, Modifiers::CONTROL, Key::A, Modifiers::META),
            (Key::F, Modifiers::CONTROL, Key::F, Modifiers::META),
            (Key::Tab, Modifiers::ALT, Key::Tab, Modifiers::META),
        ] {
            let mapped = compiled.transform_key_event(KeyEvent {
                key: source_key,
                state: KeyState::Pressed,
                modifiers: source_modifiers,
            });

            assert_eq!(mapped.key, target_key);
            assert_eq!(mapped.modifiers, target_modifiers);
        }
    }

    #[test]
    fn preserves_caps_lock_and_input_method_keys_as_physical_controls() {
        let profile = Profile {
            keyboard_mode: KeyboardMode::Text,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            ..Profile::mac_to_windows_default()
        };
        let compiled = CompiledProfile::compile(&profile).expect("profile should compile");

        for key in [Key::CapsLock, Key::Kana, Key::Eisu, Key::ImeOn, Key::ImeOff] {
            let mapped = compiled.transform_key_event(KeyEvent {
                key,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            });

            assert_eq!(mapped.key, key);
            assert_eq!(mapped.modifiers, Modifiers::NONE);
        }
    }
}
