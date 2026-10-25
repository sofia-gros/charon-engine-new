use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Typed state owned by one App on one lane. Use messages or snapshots for
/// cross-thread exchange; do not share a mutable World between workers.
///
/// Events are broadcast buffers: reads do not consume them. The producer must
/// explicitly clear its event type at the beginning of its next update. This
/// permits multiple consumers and makes fixed/frame lifetime a host decision.
#[derive(Default)]
pub struct World {
    resources: HashMap<TypeId, Box<dyn Any>>,
    events: HashMap<TypeId, Box<dyn Any>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: 'static>(&mut self, resource: T) -> Option<T> {
        self.resources
            .insert(TypeId::of::<T>(), Box::new(resource))
            .map(|old| *old.downcast::<T>().expect("resource type matches its key"))
    }

    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.resources.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.resources.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.resources.remove(&TypeId::of::<T>()).map(|value| {
            *value
                .downcast::<T>()
                .expect("resource type matches its key")
        })
    }

    pub fn send<T: 'static>(&mut self, event: T) {
        self.events
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Vec::<T>::new()))
            .downcast_mut::<Vec<T>>()
            .expect("event type matches its key")
            .push(event);
    }

    pub fn events<T: 'static>(&self) -> &[T] {
        self.events
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<Vec<T>>())
            .map_or(&[], Vec::as_slice)
    }

    pub fn clear_events<T: 'static>(&mut self) {
        if let Some(events) = self.events.get_mut(&TypeId::of::<T>()) {
            events
                .downcast_mut::<Vec<T>>()
                .expect("event type matches its key")
                .clear();
        }
    }
}
