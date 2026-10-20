use crate::core::Result;
use super::backend::AudioBackend;
use super::types::{AudioState, Page, Pagination};

pub struct Audio<'a, B: AudioBackend + ?Sized>(&'a B);

impl<'a, B: AudioBackend + ?Sized> Audio<'a, B> {
    pub fn new(backend: &'a B) -> Self { Audio(backend) }
    pub fn state(&self) -> Result<AudioState> { self.0.get_state() }
    pub fn devices(&self, page: Pagination) -> Result<Page<super::types::AudioDevice>> { self.0.list_devices(page) }
}
