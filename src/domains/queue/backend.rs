use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{Page, Pagination, SubscriptionId};

pub trait QueueBackend {
    fn enqueue(&self, _queue: &str, _data: &[u8]) -> Result<u64> {
        Err(Error::not_supported("no-backend: queue enqueue is not served on this backend"))
    }
    fn dequeue(&self, _queue: &str) -> Result<Option<Vec<u8>>> {
        Err(Error::not_supported("no-backend: queue dequeue is not served on this backend"))
    }
    fn peek(&self, _queue: &str) -> Result<Option<Vec<u8>>> {
        Err(Error::not_supported("no-backend: queue peek is not served on this backend"))
    }
    fn get_queue(&self, _queue: &str) -> Result<String> {
        Err(Error::not_supported("no-backend: queue getQueue is not served on this backend"))
    }
    fn clear_queue(&self, _queue: &str) -> Result<()> {
        Err(Error::not_supported("no-backend: queue clearQueue is not served on this backend"))
    }
    fn delete_queue(&self, _queue: &str) -> Result<()> {
        Err(Error::not_supported("no-backend: queue deleteQueue is not served on this backend"))
    }
    fn list_queues(&self) -> Result<Vec<String>> {
        Err(Error::not_supported("no-backend: queue listQueues is not served on this backend"))
    }
    fn queue_caps(&self) -> Vec<&'static str> { alloc::vec!["queue.inproc"] }
}
