//! The `queue` fixture: a scripted backend over the engine's OWNED types.
//!
//! The host is never touched (charter law #8): every read is served from the
//! script, and every mutating verb is RECORDED so a test can prove the shell
//! issued the right engine call — and that a refused write reaches no engine
//! at all.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use smart_lib::core::{Error, Result};

use smart_lib::domains::queue::QueueBackend;

/// A scripted FIFO store: named queues of byte payloads.
#[derive(Default)]
pub struct MockQueue {
    queues: RefCell<HashMap<String, VecDeque<Vec<u8>>>>,
    /// The engine calls a mutating verb made, in order.
    pub writes: RefCell<Vec<String>>,
    next_id: RefCell<u64>,
}

impl MockQueue {
    /// A store with one queue holding two payloads.
    pub fn seeded() -> Self {
        let mut payloads = VecDeque::new();
        payloads.push_back(b"first".to_vec());
        payloads.push_back(b"second".to_vec());
        let mut queues = HashMap::new();
        queues.insert("jobs".to_string(), payloads);
        MockQueue {
            queues: RefCell::new(queues),
            writes: RefCell::new(Vec::new()),
            next_id: RefCell::new(1),
        }
    }

    /// An empty store.
    pub fn empty() -> Self {
        MockQueue::default()
    }
}

impl QueueBackend for MockQueue {
    fn enqueue(&self, queue: &str, data: &[u8]) -> Result<u64> {
        self.writes
            .borrow_mut()
            .push(format!("enqueue {queue}"));
        let mut next = self.next_id.borrow_mut();
        let id = *next;
        *next += 1;
        self.queues
            .borrow_mut()
            .entry(queue.to_string())
            .or_default()
            .push_back(data.to_vec());
        Ok(id)
    }

    fn dequeue(&self, queue: &str) -> Result<Option<Vec<u8>>> {
        self.writes.borrow_mut().push(format!("dequeue {queue}"));
        // The ENGINE's own answer: a queue that is not there yields nothing,
        // not an error.
        let mut queues = self.queues.borrow_mut();
        Ok(queues.get_mut(queue).and_then(|payloads| payloads.pop_front()))
    }

    fn peek(&self, queue: &str) -> Result<Option<Vec<u8>>> {
        let queues = self.queues.borrow();
        Ok(queues.get(queue).and_then(|payloads| payloads.front().cloned()))
    }

    fn get_queue(&self, queue: &str) -> Result<String> {
        let queues = self.queues.borrow();
        if queues.contains_key(queue) {
            Ok(queue.to_string())
        } else {
            Err(Error::not_found())
        }
    }

    fn clear_queue(&self, queue: &str) -> Result<()> {
        self.writes.borrow_mut().push(format!("clear {queue}"));
        let mut queues = self.queues.borrow_mut();
        if let Some(payloads) = queues.get_mut(queue) {
            payloads.clear();
        }
        Ok(())
    }

    fn delete_queue(&self, queue: &str) -> Result<()> {
        self.writes
            .borrow_mut()
            .push(format!("delete {queue}"));
        self.queues.borrow_mut().remove(queue);
        Ok(())
    }

    fn list_queues(&self) -> Result<Vec<String>> {
        let queues = self.queues.borrow();
        let mut names: Vec<String> = queues.keys().cloned().collect();
        names.sort();
        Ok(names)
    }
}
