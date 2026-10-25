use glam::Vec2;
use std::collections::HashSet;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

pub struct InputState {
    pub keys_down: HashSet<KeyCode>,
    pub keys_just_pressed: HashSet<KeyCode>,
    pub mouse_pos: Vec2,
    pub mouse_delta: Vec2,
    prev_mouse_pos: Vec2,
    pub mouse_buttons: [bool; 3],
    pub mouse_just_pressed: [bool; 3],
    pub mouse_just_released: [bool; 3],
    pub scroll_delta: f32,
    pub shift_held: bool,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            keys_down: HashSet::new(),
            keys_just_pressed: HashSet::new(),
            mouse_pos: Vec2::ZERO,
            mouse_delta: Vec2::ZERO,
            prev_mouse_pos: Vec2::ZERO,
            mouse_buttons: [false; 3],
            mouse_just_pressed: [false; 3],
            mouse_just_released: [false; 3],
            scroll_delta: 0.0,
            shift_held: false,
        }
    }

    /// Compute deltas at the start of the frame, before input is consumed.
    pub fn prepare_frame(&mut self) {
        self.mouse_delta = self.mouse_pos - self.prev_mouse_pos;
        self.prev_mouse_pos = self.mouse_pos;
    }

    /// Clear one-shot events at the end of the frame, after input is consumed.
    pub fn end_frame(&mut self) {
        self.keys_just_pressed.clear();
        self.mouse_just_pressed = [false; 3];
        self.mouse_just_released = [false; 3];
        self.scroll_delta = 0.0;
    }

    pub fn handle_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if self.keys_down.insert(code) {
                                self.keys_just_pressed.insert(code);
                            }
                        }
                        ElementState::Released => {
                            self.keys_down.remove(&code);
                        }
                    }
                    self.shift_held = self.keys_down.contains(&KeyCode::ShiftLeft)
                        || self.keys_down.contains(&KeyCode::ShiftRight);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = Vec2::new(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let idx = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                match state {
                    ElementState::Pressed => {
                        self.mouse_buttons[idx] = true;
                        self.mouse_just_pressed[idx] = true;
                    }
                    ElementState::Released => {
                        self.mouse_buttons[idx] = false;
                        self.mouse_just_released[idx] = true;
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.scroll_delta += match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 30.0,
                };
            }
            _ => {}
        }
    }

    pub fn just_pressed(&self, key: KeyCode) -> bool {
        self.keys_just_pressed.contains(&key)
    }

    pub fn left_down(&self) -> bool {
        self.mouse_buttons[0]
    }
    pub fn right_down(&self) -> bool {
        self.mouse_buttons[1]
    }
    pub fn left_just_pressed(&self) -> bool {
        self.mouse_just_pressed[0]
    }
    pub fn left_just_released(&self) -> bool {
        self.mouse_just_released[0]
    }
}
