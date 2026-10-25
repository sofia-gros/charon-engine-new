//! Backend-neutral 2D translation, zoom and inverse transforms in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    pub fn validate(self) -> Result<Self, String> {
        if [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .iter()
        .any(|v| !v.is_finite())
            || self.width <= 0.
            || self.height <= 0.
        {
            return Err("invalid viewport or world rectangle".into());
        }
        Ok(self)
    }
    pub fn center(self) -> [f32; 2] {
        [self.x + self.width / 2., self.y + self.height / 2.]
    }
    pub fn contains(self, p: [f32; 2]) -> bool {
        p[0] >= self.x
            && p[1] >= self.y
            && p[0] < self.x + self.width
            && p[1] < self.y + self.height
    }
    pub fn array(self) -> [f32; 4] {
        [self.x, self.y, self.width, self.height]
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    center: [f32; 2],
    zoom: f32,
    viewport: Rect,
    limits: [f32; 2],
}
impl Camera2D {
    pub fn new(center: [f32; 2], zoom: f32, viewport: Rect) -> Result<Self, String> {
        viewport.validate()?;
        let mut c = Self {
            center: [0.; 2],
            zoom: 1.,
            viewport,
            limits: [0.25, 16.],
        };
        c.set_center(center)?;
        c.set_zoom(zoom)?;
        Ok(c)
    }
    pub fn center(self) -> [f32; 2] {
        self.center
    }
    pub fn zoom(self) -> f32 {
        self.zoom
    }
    pub fn viewport(self) -> Rect {
        self.viewport
    }
    pub fn set_center(&mut self, p: [f32; 2]) -> Result<(), String> {
        if p.iter().any(|v| !v.is_finite()) {
            return Err("non-finite camera center".into());
        }
        self.center = p;
        Ok(())
    }
    pub fn pan(&mut self, delta: [f32; 2]) -> Result<(), String> {
        self.set_center([self.center[0] + delta[0], self.center[1] + delta[1]])
    }
    pub fn set_viewport(&mut self, viewport: Rect) -> Result<(), String> {
        self.viewport = viewport.validate()?;
        Ok(())
    }
    pub fn set_zoom(&mut self, zoom: f32) -> Result<(), String> {
        if !zoom.is_finite() || zoom <= 0. {
            return Err("invalid camera zoom".into());
        }
        self.zoom = zoom.clamp(self.limits[0], self.limits[1]);
        Ok(())
    }
    pub fn set_zoom_limits(&mut self, min: f32, max: f32) -> Result<(), String> {
        if !min.is_finite() || !max.is_finite() || min <= 0. || max < min {
            return Err("invalid zoom limits".into());
        }
        self.limits = [min, max];
        self.zoom = self.zoom.clamp(min, max);
        Ok(())
    }
    pub fn world_to_screen(self, p: [f32; 2]) -> Result<[f32; 2], String> {
        let v = self.viewport.center();
        let out = [
            v[0] + (p[0] - self.center[0]) * self.zoom,
            v[1] + (p[1] - self.center[1]) * self.zoom,
        ];
        if out.iter().any(|v| !v.is_finite()) {
            return Err("non-finite camera projection".into());
        }
        Ok(out)
    }
    pub fn screen_to_world(self, p: [f32; 2]) -> Result<[f32; 2], String> {
        let v = self.viewport.center();
        let out = [
            self.center[0] + (p[0] - v[0]) / self.zoom,
            self.center[1] + (p[1] - v[1]) / self.zoom,
        ];
        if out.iter().any(|v| !v.is_finite()) {
            return Err("non-finite camera inverse".into());
        }
        Ok(out)
    }
    pub fn pick(self, p: [f32; 2]) -> Option<[f32; 2]> {
        self.viewport
            .contains(p)
            .then(|| self.screen_to_world(p).ok())
            .flatten()
    }
    pub fn zoom_at(&mut self, zoom: f32, anchor: [f32; 2]) -> Result<(), String> {
        let before = self.screen_to_world(anchor)?;
        let mut next = *self;
        next.set_zoom(zoom)?;
        let after = next.screen_to_world(anchor)?;
        next.pan([before[0] - after[0], before[1] - after[1]])?;
        *self = next;
        Ok(())
    }
    pub fn project_rect(self, rect: Rect) -> Result<Rect, String> {
        rect.validate()?;
        let p = self.world_to_screen([rect.x, rect.y])?;
        Rect {
            x: p[0],
            y: p[1],
            width: rect.width * self.zoom,
            height: rect.height * self.zoom,
        }
        .validate()
    }
    pub fn fit(&mut self, world: Rect) -> Result<(), String> {
        world.validate()?;
        let mut next = *self;
        next.set_center(world.center())?;
        next.set_zoom(
            (next.viewport.width / world.width).min(next.viewport.height / world.height),
        )?;
        *self = next;
        Ok(())
    }
}
#[cfg(feature = "plugin")]
pub struct CameraPlugin {
    initial: Camera2D,
    owns: bool,
}
#[cfg(feature = "plugin")]
impl CameraPlugin {
    pub fn new(camera: Camera2D) -> Self {
        Self {
            initial: camera,
            owns: false,
        }
    }
}
#[cfg(feature = "plugin")]
impl charon_core::Plugin for CameraPlugin {
    fn name(&self) -> &'static str {
        "camera"
    }
    fn init(&mut self, w: &mut charon_core::World) -> charon_core::PluginResult {
        if w.get::<Camera2D>().is_some() {
            return Err("Camera2D already exists".into());
        }
        w.insert(self.initial);
        self.owns = true;
        Ok(())
    }
    fn shutdown(&mut self, w: &mut charon_core::World) -> charon_core::PluginResult {
        if self.owns {
            w.remove::<Camera2D>();
            self.owns = false;
        }
        Ok(())
    }
}
