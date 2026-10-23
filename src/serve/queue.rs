#![allow(missing_docs)]
//! The queue domain, served through the contract's generated `Server` trait.
extern crate std;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::rc::Rc;
use std::sync::Arc;
use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;
use crate::core::ErrorCode;
use crate::domains::queue::availability;
use crate::domains::queue::backend::QueueBackend;
use crate::schema::queue_capnp::queue as que;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct QueueService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for QueueService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> QueueService<B>
where
    B: QueueBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's queue state, and this connection holds the read-only class"),
    ))
}

fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a Queue contract method"),
        )),
    }
}

fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

impl<B> que::Server for QueueService<B>
where
    B: QueueBackend + 'static,
{
}

pub struct QueueDomain<B> {
    backend: Arc<B>,
}

impl<B> QueueDomain<B>
where
    B: QueueBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for QueueDomain<B>
where
    B: QueueBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        que::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::QUEUE_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "queue"
    }

    fn category(&self) -> &'static str {
        "queue"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "queue enqueue",
                "Queue",
                "enqueue",
                "enqueue",
                0,
                true,
            )
            .with_row(matrix_row("enqueue")),            DomainCommand::new(
                "queue dequeue",
                "Queue",
                "dequeue",
                "dequeue",
                1,
                true,
            )
            .with_row(matrix_row("dequeue")),            DomainCommand::new(
                "queue peek",
                "Queue",
                "peek",
                "peek",
                2,
                true,
            )
            .with_row(matrix_row("peek")),            DomainCommand::new(
                "queue peek",
                "Queue",
                "getQueue",
                "getQueue",
                3,
                true,
            )
            .with_row(matrix_row("getQueue")),            DomainCommand::new(
                "queue clear",
                "Queue",
                "clearQueue",
                "clearQueue",
                4,
                true,
            )
            .with_row(matrix_row("clearQueue")),            DomainCommand::new(
                "queue delete",
                "Queue",
                "deleteQueue",
                "deleteQueue",
                5,
                true,
            )
            .with_row(matrix_row("deleteQueue")),            DomainCommand::new(
                "queue list-queues",
                "Queue",
                "listQueues",
                "listQueues",
                6,
                true,
            )
            .with_row(matrix_row("listQueues")),            DomainCommand::new(
                "queue subscribe-queue",
                "Queue",
                "subscribeQueue",
                "subscribeQueue",
                7,
                true,
            )
            .with_row(matrix_row("subscribeQueue")),            DomainCommand::new(
                "queue subscribe-with-filter",
                "Queue",
                "subscribeWithFilter",
                "subscribeWithFilter",
                8,
                true,
            )
            .with_row(matrix_row("subscribeWithFilter")),            DomainCommand::new(
                "queue get-statistics",
                "Queue",
                "getStatistics",
                "getStatistics",
                9,
                true,
            )
            .with_row(matrix_row("getStatistics")),            DomainCommand::new(
                "queue enqueue-all",
                "Queue",
                "enqueueAll",
                "enqueueAll",
                10,
                true,
            )
            .with_row(matrix_row("enqueueAll")),            DomainCommand::new(
                "queue dequeue-ack",
                "Queue",
                "dequeueAck",
                "dequeueAck",
                11,
                true,
            )
            .with_row(matrix_row("dequeueAck")),            DomainCommand::new(
                "queue ack",
                "Queue",
                "ack",
                "ack",
                12,
                true,
            )
            .with_row(matrix_row("ack")),            DomainCommand::new(
                "queue nack",
                "Queue",
                "nack",
                "nack",
                13,
                true,
            )
            .with_row(matrix_row("nack")),            DomainCommand::new(
                "queue replay",
                "Queue",
                "replay",
                "replay",
                14,
                true,
            )
            .with_row(matrix_row("replay")),            DomainCommand::new(
                "queue set-delivery-mode",
                "Queue",
                "setDeliveryMode",
                "setDeliveryMode",
                15,
                true,
            )
            .with_row(matrix_row("setDeliveryMode")),            DomainCommand::new(
                "queue set-dead-letter-policy",
                "Queue",
                "setDeadLetterPolicy",
                "setDeadLetterPolicy",
                16,
                true,
            )
            .with_row(matrix_row("setDeadLetterPolicy")),            DomainCommand::new(
                "queue set-ordering",
                "Queue",
                "setOrdering",
                "setOrdering",
                17,
                true,
            )
            .with_row(matrix_row("setOrdering")),            DomainCommand::new(
                "queue set-encryption",
                "Queue",
                "setEncryption",
                "setEncryption",
                18,
                true,
            )
            .with_row(matrix_row("setEncryption")),            DomainCommand::new(
                "queue set-message-ttl",
                "Queue",
                "setMessageTtl",
                "setMessageTtl",
                19,
                true,
            )
            .with_row(matrix_row("setMessageTtl")),            DomainCommand::new(
                "queue set-flow-control",
                "Queue",
                "setFlowControl",
                "setFlowControl",
                20,
                true,
            )
            .with_row(matrix_row("setFlowControl")),            DomainCommand::new(
                "queue list-dead-letters",
                "Queue",
                "listDeadLetters",
                "listDeadLetters",
                21,
                true,
            )
            .with_row(matrix_row("listDeadLetters")),            DomainCommand::new(
                "queue retry-dead-letter",
                "Queue",
                "retryDeadLetter",
                "retryDeadLetter",
                22,
                true,
            )
            .with_row(matrix_row("retryDeadLetter")),            DomainCommand::new(
                "queue create-consumer-group",
                "Queue",
                "createConsumerGroup",
                "createConsumerGroup",
                23,
                true,
            )
            .with_row(matrix_row("createConsumerGroup")),            DomainCommand::new(
                "queue delete-consumer-group",
                "Queue",
                "deleteConsumerGroup",
                "deleteConsumerGroup",
                24,
                true,
            )
            .with_row(matrix_row("deleteConsumerGroup")),            DomainCommand::new(
                "queue get-consumer-group",
                "Queue",
                "getConsumerGroup",
                "getConsumerGroup",
                25,
                true,
            )
            .with_row(matrix_row("getConsumerGroup")),            DomainCommand::new(
                "queue list-consumer-groups",
                "Queue",
                "listConsumerGroups",
                "listConsumerGroups",
                26,
                true,
            )
            .with_row(matrix_row("listConsumerGroups")),            DomainCommand::new(
                "queue add-consumer-to-group",
                "Queue",
                "addConsumerToGroup",
                "addConsumerToGroup",
                27,
                true,
            )
            .with_row(matrix_row("addConsumerToGroup")),            DomainCommand::new(
                "queue remove-consumer-from-group",
                "Queue",
                "removeConsumerFromGroup",
                "removeConsumerFromGroup",
                28,
                true,
            )
            .with_row(matrix_row("removeConsumerFromGroup")),            DomainCommand::new(
                "queue dequeue-from-group",
                "Queue",
                "dequeueFromGroup",
                "dequeueFromGroup",
                29,
                true,
            )
            .with_row(matrix_row("dequeueFromGroup")),            DomainCommand::new(
                "queue deduplicate",
                "Queue",
                "deduplicate",
                "deduplicate",
                30,
                true,
            )
            .with_row(matrix_row("deduplicate")),            DomainCommand::new(
                "queue pull",
                "Queue",
                "pull",
                "pull",
                31,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "queue cancel",
                "Queue",
                "cancel",
                "cancel",
                32,
                true,
            )
            .with_row(matrix_row("cancel")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/queue.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = QueueService::new(Arc::clone(&self.backend), class);
        let client: que::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
