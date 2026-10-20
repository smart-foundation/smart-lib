use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::queue_capnp;

use super::availability::{status_counts, AvailStatus, QUEUE_AVAILABILITY, QUEUE_SCHEMA_ID};
use super::backend::QueueBackend;

const CONTRACT_METHODS: &[&str] = &[
    "enqueue", "dequeue", "peek", "getQueue", "clearQueue", "deleteQueue", "listQueues", "subscribeQueue", "subscribeWithFilter", "getStatistics", "enqueueAll", "dequeueAck", "ack", "nack", "replay", "setDeliveryMode", "setDeadLetterPolicy", "setOrdering", "setEncryption", "setMessageTtl", "setFlowControl", "listDeadLetters", "retryDeadLetter", "createConsumerGroup", "deleteConsumerGroup", "getConsumerGroup", "listConsumerGroups", "addConsumerToGroup", "removeConsumerFromGroup", "dequeueFromGroup", "deduplicate", "pull", "cancel"
];

struct HonestBackend;
impl QueueBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(QUEUE_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in QUEUE_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in super::availability::claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_queue::"));
    }
}
