use alloc::vec::Vec;
use crate::core::Result;
use super::backend::MediaBackend;
use super::types::{MediaPlayer, MediaPlayerId, Pagination};

pub struct Media<'a, B: MediaBackend + ?Sized>(&'a B);

impl<'a, B: MediaBackend + ?Sized> Media<'a, B> {
    pub fn new(backend: &'a B) -> Self { Media(backend) }
    pub fn players(&self, page: Pagination) -> Result<Vec<MediaPlayer>> { self.0.list_players(page) }
}
