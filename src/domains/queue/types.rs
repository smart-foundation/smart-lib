use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::queue_capnp;

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
pub struct FlowControl {
    pub max_inflight: u32,
    pub rate_limit_per_producer: u32,
    pub backpressure_threshold: u32,
    pub when_full: u8,
}

impl FlowControl {
    pub fn write_to(&self, b: &mut queue_capnp::flow_control::Builder<'_>) {
        b.set_max_inflight(self.max_inflight);
        b.set_rate_limit_per_producer(self.rate_limit_per_producer);
        b.set_backpressure_threshold(self.backpressure_threshold);
        let v = match self.when_full {
            1 => queue_capnp::OverflowPolicy::DropOldest,
            2 => queue_capnp::OverflowPolicy::BlockProducer,
            3 => queue_capnp::OverflowPolicy::Custom,
            _ => queue_capnp::OverflowPolicy::RefuseNewest,
        };
        b.set_when_full(v);
    }
    pub fn from_reader(r: &queue_capnp::flow_control::Reader<'_>) -> Self {
        Self {
            max_inflight: r.get_max_inflight(),
            rate_limit_per_producer: r.get_rate_limit_per_producer(),
            backpressure_threshold: r.get_backpressure_threshold(),
            when_full: match r.get_when_full() {
                Ok(queue_capnp::OverflowPolicy::DropOldest) => 1,
                Ok(queue_capnp::OverflowPolicy::BlockProducer) => 2,
                Ok(queue_capnp::OverflowPolicy::Custom) => 3,
                _ => 0,
            },
        }
    }
}
