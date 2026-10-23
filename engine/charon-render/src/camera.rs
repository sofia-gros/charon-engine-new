/// Axis-aligned world-space view. UI coordinates are intentionally independent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    origin: [f32; 2],
    size: [f32; 2],
}
impl Camera2D {
    pub fn new(origin: [f32; 2], size: [f32; 2]) -> Result<Self, &'static str> {
        if !origin.into_iter().chain(size).all(f32::is_finite) || size.iter().any(|v| *v <= 0.) {
            return Err("camera origin/size must be finite and size positive");
        }
        Ok(Self { origin, size })
    }
    pub fn origin(self) -> [f32; 2] {
        self.origin
    }
    pub fn size(self) -> [f32; 2] {
        self.size
    }
    pub fn world_to_view(self, point: [f32; 2], viewport: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|i| (point[i] - self.origin[i]) / self.size[i] * viewport[i])
    }
    pub fn view_to_world(self, point: [f32; 2], viewport: [f32; 2]) -> Option<[f32; 2]> {
        if !point.into_iter().chain(viewport).all(f32::is_finite)
            || (0..2).any(|i| viewport[i] <= 0. || point[i] < 0. || point[i] >= viewport[i])
        {
            return None;
        }
        Some(std::array::from_fn(|i| {
            self.origin[i] + point[i] / viewport[i] * self.size[i]
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn world_and_screen_agree_after_pan_zoom_and_resize() {
        let camera = Camera2D::new([50., 25.], [240., 135.]).unwrap();
        for viewport in [[1920., 1080.], [1280., 720.], [640., 480.]] {
            for point in [[50., 25.], [170., 92.5], [289., 159.]] {
                let screen = camera.world_to_view(point, viewport);
                let restored = camera.view_to_world(screen, viewport).unwrap();
                assert!((0..2).all(|i| (restored[i] - point[i]).abs() < 0.0001));
            }
        }
        assert!(camera.view_to_world([-1., 0.], [1920., 1080.]).is_none());
        assert!(camera.view_to_world([1920., 0.], [1920., 1080.]).is_none());
        assert!(camera.view_to_world([0., 0.], [0., 1080.]).is_none());
        assert!(Camera2D::new([f32::NAN, 0.], [1., 1.]).is_err());
        assert!(Camera2D::new([0., 0.], [0., 1.]).is_err());
    }
}
