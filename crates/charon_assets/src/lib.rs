//! CPU assets. Handles do not own GPU resources; unloading invalidates stale handles.
use charon_core::{Plugin, PluginResult, World};
use std::{marker::PhantomData, path::Path, sync::Arc};

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Handle<T> {
    slot: usize,
    generation: u64,
    marker: PhantomData<fn() -> T>,
}
impl<T> Copy for Handle<T> {}
impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetEvent {
    Loaded { slot: usize },
    Unloaded { slot: usize },
    Failed { path: String, message: String },
}
struct Entry<T> {
    generation: u64,
    value: Option<Arc<T>>,
}
pub struct Assets<T> {
    entries: Vec<Entry<T>>,
    events: Vec<AssetEvent>,
}
impl<T> Default for Assets<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            events: Vec::new(),
        }
    }
}
impl<T> Assets<T> {
    pub fn insert(&mut self, value: T) -> Handle<T> {
        let slot = self
            .entries
            .iter()
            .position(|e| e.value.is_none() && e.generation < u64::MAX)
            .unwrap_or(self.entries.len());
        if slot == self.entries.len() {
            self.entries.push(Entry {
                generation: 0,
                value: None,
            });
        }
        let entry = &mut self.entries[slot];
        entry.value = Some(Arc::new(value));
        self.events.push(AssetEvent::Loaded { slot });
        Handle {
            slot,
            generation: entry.generation,
            marker: PhantomData,
        }
    }
    pub fn get(&self, handle: Handle<T>) -> Option<Arc<T>> {
        self.entries
            .get(handle.slot)
            .filter(|e| e.generation == handle.generation)?
            .value
            .clone()
    }
    /// Existing Arc readers may finish; this handle can never access a replacement.
    pub fn unload(&mut self, handle: Handle<T>) -> bool {
        let Some(entry) = self
            .entries
            .get_mut(handle.slot)
            .filter(|e| e.generation == handle.generation && e.value.is_some())
        else {
            return false;
        };
        entry.value = None;
        entry.generation = entry.generation.saturating_add(1);
        self.events.push(AssetEvent::Unloaded { slot: handle.slot });
        true
    }
    pub fn load(
        &mut self,
        path: impl AsRef<Path>,
        decode: impl FnOnce(&[u8]) -> Result<T, String>,
    ) -> Result<Handle<T>, String> {
        let path = path.as_ref();
        match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode(&bytes))
        {
            Ok(value) => Ok(self.insert(value)),
            Err(message) => {
                self.events.push(AssetEvent::Failed {
                    path: path.display().to_string(),
                    message: message.clone(),
                });
                Err(message)
            }
        }
    }
    pub fn drain_events(&mut self) -> impl Iterator<Item = AssetEvent> + '_ {
        self.events.drain(..)
    }
}
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, String> {
        let size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(4));
        if width == 0 || height == 0 || size != Some(rgba.len()) {
            return Err("invalid RGBA dimensions".into());
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let image = image::load_from_memory(bytes)
            .map_err(|e| e.to_string())?
            .into_rgba8();
        Self::new(image.width(), image.height(), image.into_raw())
    }
}
#[derive(Clone, Debug)]
pub struct ShaderSource(pub String);
impl ShaderSource {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        String::from_utf8(bytes.to_vec())
            .map(Self)
            .map_err(|e| e.to_string())
    }
}
#[derive(Default)]
pub struct AssetsPlugin {
    owns: bool,
}
impl Plugin for AssetsPlugin {
    fn name(&self) -> &'static str {
        "assets"
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        if world.get::<Assets<Image>>().is_some() || world.get::<Assets<ShaderSource>>().is_some() {
            return Err("assets already installed".into());
        }
        world.insert(Assets::<Image>::default());
        world.insert(Assets::<ShaderSource>::default());
        self.owns = true;
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        if self.owns {
            world.remove::<Assets<Image>>();
            world.remove::<Assets<ShaderSource>>();
            self.owns = false;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_handle_cannot_alias_replacement() {
        let mut assets = Assets::default();
        let a = assets.insert(4);
        let reader = assets.get(a).unwrap();
        assert!(assets.unload(a));
        let b = assets.insert(7);
        assert!(assets.get(a).is_none());
        assert_eq!(*assets.get(b).unwrap(), 7);
        assert_eq!(*reader, 4);
        assert!(!assets.unload(a));
    }
    #[test]
    fn failed_read_emits_diagnostic_without_asset() {
        let mut assets = Assets::<ShaderSource>::default();
        assert!(
            assets
                .load("missing-m3-shader.wgsl", ShaderSource::decode)
                .is_err()
        );
        assert!(matches!(
            assets.drain_events().next(),
            Some(AssetEvent::Failed { .. })
        ));
        assert!(Image::new(u32::MAX, u32::MAX, vec![]).is_err());
    }
}
