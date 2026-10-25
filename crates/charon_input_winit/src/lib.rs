//! Physical-pixel desktop input adapter. No Core, renderer or physics dependency.
use charon_input::{Button, InputEvent, Key, keys};
use winit::{
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    keyboard::{KeyCode, PhysicalKey},
};
fn key(code: KeyCode) -> Key {
    use KeyCode::*;
    match code {
        KeyA => Key(65),
        KeyB => Key(66),
        KeyC => Key(67),
        KeyD => Key(68),
        KeyE => Key(69),
        KeyF => Key(70),
        KeyG => Key(71),
        KeyH => Key(72),
        KeyI => Key(73),
        KeyJ => Key(74),
        KeyK => Key(75),
        KeyL => Key(76),
        KeyM => Key(77),
        KeyN => Key(78),
        KeyO => Key(79),
        KeyP => Key(80),
        KeyQ => Key(81),
        KeyR => Key(82),
        KeyS => Key(83),
        KeyT => Key(84),
        KeyU => Key(85),
        KeyV => Key(86),
        KeyW => Key(87),
        KeyX => Key(88),
        KeyY => Key(89),
        KeyZ => Key(90),
        Digit0 => Key(48),
        Digit1 => Key(49),
        Digit2 => Key(50),
        Digit3 => Key(51),
        Digit4 => Key(52),
        Digit5 => Key(53),
        Digit6 => Key(54),
        Digit7 => Key(55),
        Digit8 => Key(56),
        Digit9 => Key(57),
        Enter => keys::ENTER,
        Escape => keys::ESCAPE,
        Tab => keys::TAB,
        Space => keys::SPACE,
        ArrowLeft => keys::LEFT,
        ArrowRight => keys::RIGHT,
        ArrowUp => keys::UP,
        ArrowDown => keys::DOWN,
        ShiftLeft => keys::SHIFT_LEFT,
        ShiftRight => keys::SHIFT_RIGHT,
        ControlLeft => keys::CONTROL_LEFT,
        ControlRight => keys::CONTROL_RIGHT,
        AltLeft => keys::ALT_LEFT,
        AltRight => keys::ALT_RIGHT,
        Backspace => keys::BACKSPACE,
        _ => Key(0x10000 + code as u32),
    }
}
pub fn translate(event: &WindowEvent) -> Option<InputEvent> {
    match event {
        WindowEvent::KeyboardInput { event, .. } => {
            if let PhysicalKey::Code(code) = event.physical_key {
                if event.repeat && event.state == ElementState::Pressed {
                    Some(InputEvent::Repeat(Button::Key(key(code))))
                } else {
                    Some(InputEvent::Button {
                        button: Button::Key(key(code)),
                        pressed: event.state == ElementState::Pressed,
                    })
                }
            } else {
                None
            }
        }
        WindowEvent::MouseInput { state, button, .. } => Some(InputEvent::Button {
            button: Button::Mouse(match button {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
                MouseButton::Back => 3,
                MouseButton::Forward => 4,
                MouseButton::Other(n) => u32::from(*n) + 5,
            }),
            pressed: *state == ElementState::Pressed,
        }),
        WindowEvent::CursorMoved { position, .. } => {
            Some(InputEvent::Pointer([position.x as f32, position.y as f32]))
        }
        WindowEvent::CursorLeft { .. } => Some(InputEvent::PointerLeft),
        WindowEvent::MouseWheel { delta, .. } => Some(InputEvent::Wheel(match delta {
            MouseScrollDelta::LineDelta(x, y) => [*x, *y],
            MouseScrollDelta::PixelDelta(p) => [p.x as f32 / 48., p.y as f32 / 48.],
        })),
        WindowEvent::Focused(f) => Some(InputEvent::Focus(*f)),
        WindowEvent::Resized(s) => Some(InputEvent::Resize([s.width, s.height])),
        _ => None,
    }
}
