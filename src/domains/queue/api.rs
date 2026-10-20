use crate::core::Result;
use alloc::vec::Vec;
use super::backend::QueueBackend;

pub struct Queue<'a, B: QueueBackend + ?Sized>(&'a B);

impl<'a, B: QueueBackend + ?Sized> Queue<'a, B> {
    pub fn new(backend: &'a B) -> Self { Queue(backend) }
    pub fn caps(&self) -> Vec<&'static str> { self.0.queue_caps() }
}
