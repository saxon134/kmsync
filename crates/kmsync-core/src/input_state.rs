use crate::{
    InputEvent, Key, KeyEvent, KeyState, Modifiers, MouseButton, MouseEvent, KEY_CODE_SPACE,
};

#[derive(Debug, Clone)]
pub struct RemoteInputState {
    pressed_keys: [bool; KEY_CODE_SPACE],
    pressed_buttons: [bool; 5],
}

impl Default for RemoteInputState {
    fn default() -> Self {
        Self {
            pressed_keys: [false; KEY_CODE_SPACE],
            pressed_buttons: [false; 5],
        }
    }
}

impl RemoteInputState {
    pub fn apply(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key(event) => {
                self.pressed_keys[event.key as usize] = event.state == KeyState::Pressed;
            }
            InputEvent::Mouse(MouseEvent::Button { button, state }) => {
                self.pressed_buttons[button_index(button)] = state == KeyState::Pressed;
            }
            InputEvent::Mouse(MouseEvent::Move { .. } | MouseEvent::Position { .. })
            | InputEvent::Scroll(_) => {}
        }
    }

    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let mut releases = Vec::new();
        for index in 0..self.pressed_keys.len() {
            if self.pressed_keys[index] {
                self.pressed_keys[index] = false;
                if let Some(key) = Key::from_u16(index as u16) {
                    releases.push(InputEvent::Key(KeyEvent {
                        key,
                        state: KeyState::Released,
                        modifiers: Modifiers::NONE,
                    }));
                }
            }
        }
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
        ] {
            let index = button_index(button);
            if self.pressed_buttons[index] {
                self.pressed_buttons[index] = false;
                releases.push(InputEvent::Mouse(MouseEvent::Button {
                    button,
                    state: KeyState::Released,
                }));
            }
        }
        releases
    }
}

const fn button_index(button: MouseButton) -> usize {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        InputEvent, Key, KeyEvent, KeyState, Modifiers, MouseButton, MouseEvent, ScrollEvent,
    };

    #[test]
    fn tracks_pressed_keys_and_releases_them() {
        let mut state = RemoteInputState::default();
        state.apply(InputEvent::Key(KeyEvent {
            key: Key::LeftControl,
            state: KeyState::Pressed,
            modifiers: Modifiers::CONTROL,
        }));
        state.apply(InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::CONTROL,
        }));

        let releases = state.release_all();

        assert_eq!(
            releases,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
            ]
        );
        assert!(state.release_all().is_empty());
    }

    #[test]
    fn key_release_removes_key_from_tracked_state() {
        let mut state = RemoteInputState::default();
        state.apply(InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::CONTROL,
        }));
        state.apply(InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Released,
            modifiers: Modifiers::NONE,
        }));

        assert!(state.release_all().is_empty());
    }

    #[test]
    fn tracks_mouse_buttons_and_releases_them() {
        let mut state = RemoteInputState::default();
        state.apply(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Left,
            state: KeyState::Pressed,
        }));
        state.apply(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Forward,
            state: KeyState::Pressed,
        }));

        let releases = state.release_all();

        assert_eq!(
            releases,
            vec![
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Forward,
                    state: KeyState::Released,
                }),
            ]
        );
        assert!(state.release_all().is_empty());
    }

    #[test]
    fn ignores_move_scroll_and_unpressed_button_release() {
        let mut state = RemoteInputState::default();
        state.apply(InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 2.0 }));
        state.apply(InputEvent::Scroll(ScrollEvent { dx: 1.0, dy: 2.0 }));
        state.apply(InputEvent::Mouse(MouseEvent::Button {
            button: MouseButton::Right,
            state: KeyState::Released,
        }));

        assert!(state.release_all().is_empty());
    }

    #[test]
    fn tracks_extended_keys_for_disconnect_release() {
        let mut state = RemoteInputState::default();
        state.apply(InputEvent::Key(KeyEvent {
            key: Key::VolumeUp,
            state: KeyState::Pressed,
            modifiers: Modifiers::NONE,
        }));

        assert_eq!(
            state.release_all(),
            vec![InputEvent::Key(KeyEvent {
                key: Key::VolumeUp,
                state: KeyState::Released,
                modifiers: Modifiers::NONE,
            })]
        );
    }
}
