//! Convenience sets. Enabling a Cargo feature makes a set available; it never installs it.
use crate::{AppBuilder, CoreError};
pub trait PluginSet {
    fn install(self, app: AppBuilder) -> Result<AppBuilder, CoreError>;
}
#[cfg(feature = "assets")]
pub struct Assets;
#[cfg(feature = "assets")]
impl PluginSet for Assets {
    fn install(self, app: AppBuilder) -> Result<AppBuilder, CoreError> {
        app.plugin(crate::assets::AssetsPlugin::default())
    }
}
/// CPU assets, input and camera only: no renderer, world, sand, or lighting.
/// InputClient is installed as a resource and is also available through `client()`.
#[cfg(feature = "interaction")]
pub struct Interaction2D {
    input: crate::input::InputPlugin,
    client: crate::input::InputClient,
    camera: crate::camera::Camera2D,
}
#[cfg(feature = "interaction")]
impl Interaction2D {
    pub fn new(camera: crate::camera::Camera2D, capacity: usize) -> Result<Self, String> {
        let (input, client) = crate::input::InputPlugin::new(capacity)?;
        Ok(Self {
            input,
            client,
            camera,
        })
    }
    pub fn client(&self) -> crate::input::InputClient {
        self.client.clone()
    }
}
#[cfg(feature = "interaction")]
impl PluginSet for Interaction2D {
    fn install(self, app: AppBuilder) -> Result<AppBuilder, CoreError> {
        app.bundle(Assets)?
            .plugin(self.input)?
            .plugin(crate::camera::CameraPlugin::new(self.camera))
            .map(|app| app.resource(self.client))
    }
}
