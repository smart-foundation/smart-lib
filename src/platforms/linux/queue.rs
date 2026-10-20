extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::queue::backend::QueueBackend;
use crate::domains::queue::types::Pagination;

pub struct LinuxQueueBackend {
    queues: Mutex<HashMap<String, VecDeque<Vec<u8>>>>,
    next_id: Mutex<u64>,
}

impl LinuxQueueBackend {
    pub fn new() -> Self {
        Self { queues: Mutex::new(HashMap::new()), next_id: Mutex::new(1) }
    }
}

impl QueueBackend for LinuxQueueBackend {
    fn enqueue(&self, queue: &str, data: &[u8]) -> Result<u64> {
        let mut queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        let q = queues.entry(queue.to_string()).or_default();
        q.push_back(data.to_vec());
        let mut id = self.next_id.lock().map_err(|_| Error::failed("lock failed"))?;
        let rid = *id;
        *id += 1;
        Ok(rid)
    }
    fn dequeue(&self, queue: &str) -> Result<Option<Vec<u8>>> {
        let mut queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        Ok(queues.get_mut(queue).and_then(|q| q.pop_front()))
    }
    fn peek(&self, queue: &str) -> Result<Option<Vec<u8>>> {
        let queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        Ok(queues.get(queue).and_then(|q| q.front().cloned()))
    }
    fn get_queue(&self, queue: &str) -> Result<String> {
        let queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        if queues.contains_key(queue) { Ok(queue.to_string()) } else { Err(Error::not_found()) }
    }
    fn clear_queue(&self, queue: &str) -> Result<()> {
        let mut queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        if let Some(q) = queues.get_mut(queue) { q.clear(); }
        Ok(())
    }
    fn delete_queue(&self, queue: &str) -> Result<()> {
        let mut queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        queues.remove(queue);
        Ok(())
    }
    fn list_queues(&self) -> Result<Vec<String>> {
        let queues = self.queues.lock().map_err(|_| Error::failed("lock failed"))?;
        let mut names: Vec<String> = queues.keys().cloned().collect();
        names.sort();
        Ok(names)
    }
    fn queue_caps(&self) -> Vec<&'static str> { alloc::vec!["queue.inproc"] }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::domains::queue::backend::QueueBackend;

    #[test]
    fn core_works() {
        let b = LinuxQueueBackend::new();
        let _ = b.enqueue("q", b"v");
        let _ = b.peek("q");
        let _ = b.dequeue("q");
        let _ = b.list_queues();
        let _ = b.clear_queue("q");
        let _ = b.delete_queue("q");
    }
}
