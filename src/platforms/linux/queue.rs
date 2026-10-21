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
    use crate::core::ErrorCode;
    use crate::domains::queue::backend::QueueBackend;

    /// The backend over the engine's OWN queues: this domain keeps its
    /// entries in the process's memory, so a test never touches a host file
    /// and never needs to restore one.
    fn backend() -> LinuxQueueBackend {
        LinuxQueueBackend::new()
    }

    /// `enqueue` stores the bytes and hands back the engine's own id.
    #[test]
    fn enqueue_stores_the_bytes_and_hands_back_an_id() {
        let b = backend();
        let id = b.enqueue("q", b"the real bytes").expect("enqueue");
        // The id is the engine's own counter, and it is not the same one
        // twice.
        let second = b.enqueue("q", b"more").expect("enqueue");
        assert_ne!(id, second, "every entry gets its own id");
        // The bytes are in the queue, in order.
        assert_eq!(b.peek("q").expect("peek"), Some(b"the real bytes".to_vec()));
    }

    /// `dequeue` hands the oldest entry back and takes it out of the queue.
    #[test]
    fn dequeue_hands_the_oldest_entry_back_and_removes_it() {
        let b = backend();
        b.enqueue("q", b"first").expect("enqueue");
        b.enqueue("q", b"second").expect("enqueue");
        assert_eq!(b.dequeue("q").expect("dequeue"), Some(b"first".to_vec()));
        assert_eq!(b.dequeue("q").expect("dequeue"), Some(b"second".to_vec()));
        // An empty queue is the honest empty, never a fabricated entry.
        assert_eq!(b.dequeue("q").expect("dequeue"), None);
        // A queue that was never created is the honest empty too.
        assert_eq!(b.dequeue("nosuchqueue").expect("dequeue"), None);
    }

    /// `peek` reads the oldest entry WITHOUT removing it.
    #[test]
    fn peek_reads_the_oldest_entry_without_removing_it() {
        let b = backend();
        assert_eq!(b.peek("q").expect("peek"), None);
        b.enqueue("q", b"first").expect("enqueue");
        b.enqueue("q", b"second").expect("enqueue");
        assert_eq!(b.peek("q").expect("peek"), Some(b"first".to_vec()));
        // The peek changed nothing: the entry is still there.
        assert_eq!(b.dequeue("q").expect("dequeue"), Some(b"first".to_vec()));
    }

    /// `getQueue` answers with the queue's own name — and a queue that is
    /// not there is `notFound`, never a fabricated name.
    #[test]
    fn get_queue_answers_with_the_queues_own_name() {
        let b = backend();
        b.enqueue("q", b"v").expect("enqueue");
        assert_eq!(b.get_queue("q").expect("get queue"), "q");
        let missing = b.get_queue("nosuchqueue").expect_err("no such queue");
        assert_eq!(missing.code, ErrorCode::NotFound);
    }

    /// `clearQueue` empties the queue and leaves the other queues alone.
    #[test]
    fn clear_queue_empties_the_queue_and_only_the_queue() {
        let b = backend();
        b.enqueue("q", b"a").expect("enqueue");
        b.enqueue("q", b"b").expect("enqueue");
        b.enqueue("other", b"c").expect("enqueue");
        b.clear_queue("q").expect("clear");
        assert_eq!(b.peek("q").expect("peek"), None, "the queue is empty");
        assert_eq!(
            b.peek("other").expect("peek"),
            Some(b"c".to_vec()),
            "another queue is untouched"
        );
        // Clearing a queue that was never created is the honest no-op.
        b.clear_queue("nosuchqueue").expect("clear");
    }

    /// `deleteQueue` takes the whole queue away, and says so.
    #[test]
    fn delete_queue_takes_the_queue_away() {
        let b = backend();
        b.enqueue("q", b"a").expect("enqueue");
        b.delete_queue("q").expect("delete");
        let missing = b.get_queue("q").expect_err("the queue is gone");
        assert_eq!(missing.code, ErrorCode::NotFound);
        assert_eq!(b.peek("q").expect("peek"), None);
        // Deleting a queue that is not there is the honest no-op.
        b.delete_queue("nosuchqueue").expect("delete");
    }

    /// `listQueues` answers with the engine's OWN queue names, sorted.
    #[test]
    fn list_queues_answers_with_the_engines_own_queue_names() {
        let b = backend();
        assert!(b.list_queues().expect("list").is_empty());
        b.enqueue("beta", b"1").expect("enqueue");
        b.enqueue("alpha", b"1").expect("enqueue");
        assert_eq!(
            b.list_queues().expect("list"),
            alloc::vec!["alpha".to_string(), "beta".to_string()]
        );
    }
}
