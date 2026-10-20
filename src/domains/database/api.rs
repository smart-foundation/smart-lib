use crate::core::Result;
use alloc::vec::Vec;
use super::backend::DatabaseBackend;
use super::types::TransactionId;

pub struct Database<'a, B: DatabaseBackend + ?Sized>(&'a B);

impl<'a, B: DatabaseBackend + ?Sized> Database<'a, B> {
    pub fn new(backend: &'a B) -> Self { Database(backend) }
    pub fn caps(&self) -> Vec<&'static str> { self.0.database_caps() }
    pub fn _tx(&self) -> TransactionId { TransactionId { id: 0 } }
}
