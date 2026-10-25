//! Optional adapters for CPU results and an independent GPU compositing shader.
use charon_assets::Image;
use charon_lighting::LightField;
use charon_render::{DrawFrame, PassId};
pub const SHADER: &str = include_str!("composite.wgsl");
pub fn image(field: &LightField) -> Result<Image, String> {
    field.geometry.validate()?;
    if field.rgba.len() != field.geometry.len()
        || field
            .rgba
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("invalid light result".into());
    }
    let rgba = field
        .rgba
        .iter()
        .flat_map(|c| {
            [
                (c[0] * 255.).round() as u8,
                (c[1] * 255.).round() as u8,
                (c[2] * 255.).round() as u8,
                255,
            ]
        })
        .collect();
    Image::new(field.geometry.width, field.geometry.height, rgba)
}
/// Select which render packets receive lighting; UI and other passes remain free.
pub fn assign_pass(frame: &mut DrawFrame, pass: PassId, max_layer: i32) {
    for quad in &mut frame.quads {
        if quad.layer <= max_layer {
            quad.pass = pass;
        }
    }
}
