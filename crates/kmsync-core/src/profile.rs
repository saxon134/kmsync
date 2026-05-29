use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::event::{Key, OsKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HabitPreset {
    KeepMacHabit,
    KeepWindowsHabit,
    KeepLinuxHabit,
    TargetOsHabit,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKeyMode {
    Standard,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    Physical,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardLayout {
    UsAnsi,
    Iso,
    Jis,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonEnglishInputStrategy {
    PhysicalFallback,
    ImePassthrough,
    UnicodeText,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSettings {
    pub vertical_multiplier: f32,
    pub horizontal_multiplier: f32,
}

impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            vertical_multiplier: 1.0,
            horizontal_multiplier: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSettings {
    pub speed_multiplier: f32,
}

impl Default for PointerSettings {
    fn default() -> Self {
        Self {
            speed_multiplier: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceLayout {
    pub targets: Vec<DeviceLayoutTarget>,
    pub edge_bindings: EdgeBindings,
}

impl DeviceLayout {
    #[must_use]
    pub fn target_for_edge(&self, edge: ScreenEdge) -> Option<&str> {
        match edge {
            ScreenEdge::Left => self.edge_bindings.left.as_deref(),
            ScreenEdge::Right => self.edge_bindings.right.as_deref(),
            ScreenEdge::Top => self.edge_bindings.top.as_deref(),
            ScreenEdge::Bottom => self.edge_bindings.bottom.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLayoutTarget {
    pub device_id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EdgeBindings {
    pub left: Option<String>,
    pub right: Option<String>,
    pub top: Option<String>,
    pub bottom: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMapping {
    pub from: Key,
    pub to: Key,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub source_os: OsKind,
    pub target_os: OsKind,
    pub preset: HabitPreset,
    pub key_mappings: Vec<KeyMapping>,
    pub function_key_mode: FunctionKeyMode,
    pub keyboard_mode: KeyboardMode,
    pub source_keyboard_layout: KeyboardLayout,
    pub target_keyboard_layout: KeyboardLayout,
    pub non_english_input_strategy: NonEnglishInputStrategy,
    pub scroll: ScrollSettings,
    pub pointer: PointerSettings,
    pub device_layout: DeviceLayout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileConfigError {
    InvalidJson(String),
    UnknownOs(String),
    UnknownPreset(String),
    UnknownFunctionKeyMode(String),
    UnknownKeyboardMode(String),
    UnknownKeyboardLayout(String),
    UnknownNonEnglishInputStrategy(String),
    UnknownKey(String),
    DuplicateLayoutTarget(String),
    UnknownLayoutTarget {
        edge: ScreenEdge,
        target_device_id: String,
    },
}

#[derive(Debug, Deserialize)]
struct ProfileConfigJson {
    source_os: String,
    target_os: String,
    preset: String,
    modifier_mapping: BTreeMap<String, String>,
    #[serde(default)]
    function_key_mode: Option<String>,
    #[serde(default)]
    keyboard_mode: Option<String>,
    #[serde(default)]
    source_keyboard_layout: Option<String>,
    #[serde(default)]
    target_keyboard_layout: Option<String>,
    #[serde(default)]
    non_english_input_strategy: Option<String>,
    scroll: ScrollSettingsJson,
    pointer: PointerSettingsJson,
    #[serde(default)]
    device_layout: DeviceLayoutJson,
}

#[derive(Debug, Deserialize)]
struct ScrollSettingsJson {
    vertical_multiplier: f32,
    horizontal_multiplier: f32,
}

#[derive(Debug, Deserialize)]
struct PointerSettingsJson {
    speed_multiplier: f32,
}

#[derive(Debug, Deserialize, Default)]
struct DeviceLayoutJson {
    #[serde(default)]
    targets: Vec<DeviceLayoutTargetJson>,
    #[serde(default)]
    edges: EdgeBindingsJson,
}

#[derive(Debug, Deserialize)]
struct DeviceLayoutTargetJson {
    device_id: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct EdgeBindingsJson {
    #[serde(default)]
    left: Option<String>,
    #[serde(default)]
    right: Option<String>,
    #[serde(default)]
    top: Option<String>,
    #[serde(default)]
    bottom: Option<String>,
}

impl Profile {
    pub fn from_config_json(text: &str) -> Result<Self, ProfileConfigError> {
        let config: ProfileConfigJson = serde_json::from_str(text)
            .map_err(|error| ProfileConfigError::InvalidJson(error.to_string()))?;
        let key_mappings = config
            .modifier_mapping
            .into_iter()
            .map(|(from, to)| {
                Ok(KeyMapping {
                    from: parse_profile_key(&from)?,
                    to: parse_profile_key(&to)?,
                })
            })
            .collect::<Result<Vec<_>, ProfileConfigError>>()?;

        Ok(Self {
            source_os: parse_os_kind(&config.source_os)?,
            target_os: parse_os_kind(&config.target_os)?,
            preset: parse_habit_preset(&config.preset)?,
            key_mappings,
            function_key_mode: parse_function_key_mode(config.function_key_mode.as_deref())?,
            keyboard_mode: parse_keyboard_mode(config.keyboard_mode.as_deref())?,
            source_keyboard_layout: parse_keyboard_layout(
                config.source_keyboard_layout.as_deref(),
            )?,
            target_keyboard_layout: parse_keyboard_layout(
                config.target_keyboard_layout.as_deref(),
            )?,
            non_english_input_strategy: parse_non_english_input_strategy(
                config.non_english_input_strategy.as_deref(),
            )?,
            scroll: ScrollSettings {
                vertical_multiplier: config.scroll.vertical_multiplier,
                horizontal_multiplier: config.scroll.horizontal_multiplier,
            },
            pointer: PointerSettings {
                speed_multiplier: config.pointer.speed_multiplier,
            },
            device_layout: parse_device_layout(config.device_layout)?,
        })
    }

    #[must_use]
    pub fn mac_to_windows_default() -> Self {
        Self {
            source_os: OsKind::MacOs,
            target_os: OsKind::Windows,
            preset: HabitPreset::KeepMacHabit,
            key_mappings: vec![
                KeyMapping {
                    from: Key::LeftMeta,
                    to: Key::LeftControl,
                },
                KeyMapping {
                    from: Key::RightMeta,
                    to: Key::RightControl,
                },
                KeyMapping {
                    from: Key::LeftAlt,
                    to: Key::LeftAlt,
                },
                KeyMapping {
                    from: Key::RightAlt,
                    to: Key::RightAlt,
                },
            ],
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings {
                vertical_multiplier: -1.0,
                horizontal_multiplier: -1.0,
            },
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        }
    }

    #[must_use]
    pub fn windows_to_macos_default() -> Self {
        Self {
            source_os: OsKind::Windows,
            target_os: OsKind::MacOs,
            preset: HabitPreset::KeepWindowsHabit,
            key_mappings: vec![
                KeyMapping {
                    from: Key::LeftControl,
                    to: Key::LeftMeta,
                },
                KeyMapping {
                    from: Key::RightControl,
                    to: Key::RightMeta,
                },
                KeyMapping {
                    from: Key::LeftAlt,
                    to: Key::LeftAlt,
                },
                KeyMapping {
                    from: Key::RightAlt,
                    to: Key::RightAlt,
                },
                KeyMapping {
                    from: Key::LeftMeta,
                    to: Key::LeftMeta,
                },
                KeyMapping {
                    from: Key::RightMeta,
                    to: Key::RightMeta,
                },
            ],
            function_key_mode: FunctionKeyMode::Standard,
            keyboard_mode: KeyboardMode::Text,
            source_keyboard_layout: KeyboardLayout::UsAnsi,
            target_keyboard_layout: KeyboardLayout::UsAnsi,
            non_english_input_strategy: NonEnglishInputStrategy::ImePassthrough,
            scroll: ScrollSettings {
                vertical_multiplier: -1.0,
                horizontal_multiplier: -1.0,
            },
            pointer: PointerSettings::default(),
            device_layout: DeviceLayout::default(),
        }
    }
}

fn parse_os_kind(value: &str) -> Result<OsKind, ProfileConfigError> {
    match value {
        "macos" => Ok(OsKind::MacOs),
        "windows" => Ok(OsKind::Windows),
        "linux" => Ok(OsKind::Linux),
        other => Err(ProfileConfigError::UnknownOs(other.to_string())),
    }
}

fn parse_habit_preset(value: &str) -> Result<HabitPreset, ProfileConfigError> {
    match value {
        "keep_mac_habit" => Ok(HabitPreset::KeepMacHabit),
        "keep_windows_habit" => Ok(HabitPreset::KeepWindowsHabit),
        "keep_linux_habit" => Ok(HabitPreset::KeepLinuxHabit),
        "target_os_habit" => Ok(HabitPreset::TargetOsHabit),
        "custom" => Ok(HabitPreset::Custom),
        other => Err(ProfileConfigError::UnknownPreset(other.to_string())),
    }
}

fn parse_function_key_mode(value: Option<&str>) -> Result<FunctionKeyMode, ProfileConfigError> {
    match value.unwrap_or("standard") {
        "standard" => Ok(FunctionKeyMode::Standard),
        "media" => Ok(FunctionKeyMode::Media),
        other => Err(ProfileConfigError::UnknownFunctionKeyMode(
            other.to_string(),
        )),
    }
}

fn parse_keyboard_mode(value: Option<&str>) -> Result<KeyboardMode, ProfileConfigError> {
    match value.unwrap_or("text") {
        "physical" => Ok(KeyboardMode::Physical),
        "text" => Ok(KeyboardMode::Text),
        other => Err(ProfileConfigError::UnknownKeyboardMode(other.to_string())),
    }
}

fn parse_keyboard_layout(value: Option<&str>) -> Result<KeyboardLayout, ProfileConfigError> {
    match value.unwrap_or("us_ansi") {
        "us_ansi" | "ansi" | "us" => Ok(KeyboardLayout::UsAnsi),
        "iso" => Ok(KeyboardLayout::Iso),
        "jis" => Ok(KeyboardLayout::Jis),
        "custom" => Ok(KeyboardLayout::Custom),
        other => Err(ProfileConfigError::UnknownKeyboardLayout(other.to_string())),
    }
}

fn parse_non_english_input_strategy(
    value: Option<&str>,
) -> Result<NonEnglishInputStrategy, ProfileConfigError> {
    match value.unwrap_or("ime_passthrough") {
        "physical_fallback" => Ok(NonEnglishInputStrategy::PhysicalFallback),
        "ime_passthrough" => Ok(NonEnglishInputStrategy::ImePassthrough),
        "unicode_text" => Ok(NonEnglishInputStrategy::UnicodeText),
        other => Err(ProfileConfigError::UnknownNonEnglishInputStrategy(
            other.to_string(),
        )),
    }
}

fn parse_profile_key(value: &str) -> Result<Key, ProfileConfigError> {
    Key::from_name(value).ok_or_else(|| ProfileConfigError::UnknownKey(value.to_string()))
}

fn parse_device_layout(config: DeviceLayoutJson) -> Result<DeviceLayout, ProfileConfigError> {
    let mut target_ids = BTreeSet::new();
    let mut targets = Vec::with_capacity(config.targets.len());

    for target in config.targets {
        if !target_ids.insert(target.device_id.clone()) {
            return Err(ProfileConfigError::DuplicateLayoutTarget(target.device_id));
        }
        targets.push(DeviceLayoutTarget {
            device_id: target.device_id,
            display_name: target.display_name,
        });
    }

    Ok(DeviceLayout {
        targets,
        edge_bindings: EdgeBindings {
            left: parse_edge_binding(ScreenEdge::Left, config.edges.left, &target_ids)?,
            right: parse_edge_binding(ScreenEdge::Right, config.edges.right, &target_ids)?,
            top: parse_edge_binding(ScreenEdge::Top, config.edges.top, &target_ids)?,
            bottom: parse_edge_binding(ScreenEdge::Bottom, config.edges.bottom, &target_ids)?,
        },
    })
}

fn parse_edge_binding(
    edge: ScreenEdge,
    target_device_id: Option<String>,
    target_ids: &BTreeSet<String>,
) -> Result<Option<String>, ProfileConfigError> {
    let Some(target_device_id) = target_device_id else {
        return Ok(None);
    };
    if !target_ids.contains(&target_device_id) {
        return Err(ProfileConfigError::UnknownLayoutTarget {
            edge,
            target_device_id,
        });
    }
    Ok(Some(target_device_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mac_to_windows_profile_json() {
        let text = include_str!("../../../configs/mac-to-windows.profile.json");

        let profile = Profile::from_config_json(text).expect("profile config should parse");

        assert_eq!(profile.source_os, OsKind::MacOs);
        assert_eq!(profile.target_os, OsKind::Windows);
        assert_eq!(profile.preset, HabitPreset::KeepMacHabit);
        assert_eq!(profile.keyboard_mode, KeyboardMode::Physical);
        assert_eq!(profile.scroll.vertical_multiplier, -1.0);
        assert_eq!(profile.pointer.speed_multiplier, 1.0);
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::LeftMeta,
            to: Key::LeftControl,
        }));
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::RightMeta,
            to: Key::RightControl,
        }));
    }

    #[test]
    fn parses_windows_to_macos_profile_json() {
        let text = include_str!("../../../configs/windows-to-mac.profile.json");

        let profile = Profile::from_config_json(text).expect("profile config should parse");

        assert_eq!(profile.source_os, OsKind::Windows);
        assert_eq!(profile.target_os, OsKind::MacOs);
        assert_eq!(profile.preset, HabitPreset::KeepWindowsHabit);
        assert_eq!(profile.keyboard_mode, KeyboardMode::Physical);
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::LeftControl,
            to: Key::LeftMeta,
        }));
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::RightControl,
            to: Key::RightMeta,
        }));
    }

    #[test]
    fn rejects_unknown_profile_key_name() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": { "hyper": "left_control" },
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        assert_eq!(
            Profile::from_config_json(text),
            Err(ProfileConfigError::UnknownKey("hyper".to_string()))
        );
    }

    #[test]
    fn parses_extended_profile_key_names() {
        let text = r#"{
            "source_os": "windows",
            "target_os": "macos",
            "preset": "custom",
            "modifier_mapping": {
                "numpad_enter": "f24",
                "volume_up": "media_play_pause"
            },
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        let profile = Profile::from_config_json(text).expect("profile should parse");

        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::NumpadEnter,
            to: Key::F24,
        }));
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::VolumeUp,
            to: Key::MediaPlayPause,
        }));
    }

    #[test]
    fn parses_input_method_and_system_key_names() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {
                "caps_lock": "caps_lock",
                "kana": "ime_on",
                "eisu": "ime_off",
                "print_screen": "print_screen"
            },
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        let profile = Profile::from_config_json(text).expect("profile should parse");

        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::CapsLock,
            to: Key::CapsLock,
        }));
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::Kana,
            to: Key::ImeOn,
        }));
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::Eisu,
            to: Key::ImeOff,
        }));
        assert!(profile.key_mappings.contains(&KeyMapping {
            from: Key::PrintScreen,
            to: Key::PrintScreen,
        }));
    }

    #[test]
    fn parses_function_key_row_mode() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "function_key_mode": "media",
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        let profile = Profile::from_config_json(text).expect("profile should parse");

        assert_eq!(profile.function_key_mode, FunctionKeyMode::Media);
    }

    #[test]
    fn parses_keyboard_mode() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "keyboard_mode": "text",
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        let profile = Profile::from_config_json(text).expect("profile should parse");

        assert_eq!(profile.keyboard_mode, KeyboardMode::Text);
    }

    #[test]
    fn parses_keyboard_layouts_and_non_english_input_strategy() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "keyboard_mode": "text",
            "source_keyboard_layout": "jis",
            "target_keyboard_layout": "iso",
            "non_english_input_strategy": "unicode_text",
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        let profile = Profile::from_config_json(text).expect("profile should parse");

        assert_eq!(profile.source_keyboard_layout, KeyboardLayout::Jis);
        assert_eq!(profile.target_keyboard_layout, KeyboardLayout::Iso);
        assert_eq!(
            profile.non_english_input_strategy,
            NonEnglishInputStrategy::UnicodeText
        );
    }

    #[test]
    fn parses_multi_target_device_layout_and_edge_bindings() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 },
            "device_layout": {
                "targets": [
                    { "device_id": "linux-left", "display_name": "Linux Workstation" },
                    { "device_id": "windows-right", "display_name": "Windows Tower" },
                    { "device_id": "macbook-top" },
                    { "device_id": "studio-bottom" }
                ],
                "edges": {
                    "left": "linux-left",
                    "right": "windows-right",
                    "top": "macbook-top",
                    "bottom": "studio-bottom"
                }
            }
        }"#;

        let profile = Profile::from_config_json(text).expect("profile should parse");

        assert_eq!(profile.device_layout.targets.len(), 4);
        assert_eq!(
            profile.device_layout.targets[0].display_name.as_deref(),
            Some("Linux Workstation")
        );
        assert_eq!(
            profile.device_layout.target_for_edge(ScreenEdge::Left),
            Some("linux-left")
        );
        assert_eq!(
            profile.device_layout.target_for_edge(ScreenEdge::Right),
            Some("windows-right")
        );
        assert_eq!(
            profile.device_layout.target_for_edge(ScreenEdge::Top),
            Some("macbook-top")
        );
        assert_eq!(
            profile.device_layout.target_for_edge(ScreenEdge::Bottom),
            Some("studio-bottom")
        );
    }

    #[test]
    fn rejects_device_layout_edge_binding_to_unknown_target() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 },
            "device_layout": {
                "targets": [
                    { "device_id": "windows-right" }
                ],
                "edges": {
                    "left": "missing-device"
                }
            }
        }"#;

        assert!(matches!(
            Profile::from_config_json(text),
            Err(ProfileConfigError::UnknownLayoutTarget {
                edge: ScreenEdge::Left,
                target_device_id
            }) if target_device_id == "missing-device"
        ));
    }

    #[test]
    fn rejects_duplicate_device_layout_target_ids() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 },
            "device_layout": {
                "targets": [
                    { "device_id": "windows-right" },
                    { "device_id": "windows-right" }
                ],
                "edges": {
                    "right": "windows-right"
                }
            }
        }"#;

        assert_eq!(
            Profile::from_config_json(text),
            Err(ProfileConfigError::DuplicateLayoutTarget(
                "windows-right".to_string()
            ))
        );
    }

    #[test]
    fn rejects_unknown_keyboard_layout_and_non_english_strategy() {
        let bad_layout = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "source_keyboard_layout": "dvorak",
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;
        let bad_strategy = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "non_english_input_strategy": "guess",
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        assert!(matches!(
            Profile::from_config_json(bad_layout),
            Err(ProfileConfigError::UnknownKeyboardLayout(value)) if value == "dvorak"
        ));
        assert!(matches!(
            Profile::from_config_json(bad_strategy),
            Err(ProfileConfigError::UnknownNonEnglishInputStrategy(value)) if value == "guess"
        ));
    }

    #[test]
    fn rejects_unknown_keyboard_mode() {
        let text = r#"{
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": {},
            "keyboard_mode": "phonetic",
            "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        }"#;

        assert!(matches!(
            Profile::from_config_json(text),
            Err(ProfileConfigError::UnknownKeyboardMode(value)) if value == "phonetic"
        ));
    }
}
