use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::database_capnp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
    pub limit: u32,
}

impl Pagination {
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 { 1 } else { self.limit }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransactionId {
    pub id: u64,
}

impl TransactionId {
    pub fn write_to(&self, b: &mut database_capnp::transaction_id::Builder<'_>) {
        b.set_id(self.id);
    }
    pub fn from_reader(r: &database_capnp::transaction_id::Reader<'_>) -> Self {
        Self { id: r.get_id() }
    }
}
