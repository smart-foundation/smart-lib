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
    fn enqueue(
        self: Rc<Self>,
        _params: que::EnqueueParams,
        _results: que::EnqueueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("enqueue") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn dequeue(
        self: Rc<Self>,
        _params: que::DequeueParams,
        _results: que::DequeueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("dequeue") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn peek(
        self: Rc<Self>,
        _params: que::PeekParams,
        _results: que::PeekResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("peek") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_queue(
        self: Rc<Self>,
        _params: que::GetQueueParams,
        _results: que::GetQueueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getQueue") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn clear_queue(
        self: Rc<Self>,
        _params: que::ClearQueueParams,
        _results: que::ClearQueueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("clearQueue") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete_queue(
        self: Rc<Self>,
        _params: que::DeleteQueueParams,
        _results: que::DeleteQueueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deleteQueue") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_queues(
        self: Rc<Self>,
        _params: que::ListQueuesParams,
        _results: que::ListQueuesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listQueues") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_queue(
        self: Rc<Self>,
        _params: que::SubscribeQueueParams,
        _results: que::SubscribeQueueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeQueue") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe_with_filter(
        self: Rc<Self>,
        _params: que::SubscribeWithFilterParams,
        _results: que::SubscribeWithFilterResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribeWithFilter") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_statistics(
        self: Rc<Self>,
        _params: que::GetStatisticsParams,
        _results: que::GetStatisticsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getStatistics") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn enqueue_all(
        self: Rc<Self>,
        _params: que::EnqueueAllParams,
        _results: que::EnqueueAllResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("enqueueAll") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn dequeue_ack(
        self: Rc<Self>,
        _params: que::DequeueAckParams,
        _results: que::DequeueAckResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("dequeueAck") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn ack(
        self: Rc<Self>,
        _params: que::AckParams,
        _results: que::AckResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("ack") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn nack(
        self: Rc<Self>,
        _params: que::NackParams,
        _results: que::NackResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("nack") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn replay(
        self: Rc<Self>,
        _params: que::ReplayParams,
        _results: que::ReplayResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("replay") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_delivery_mode(
        self: Rc<Self>,
        _params: que::SetDeliveryModeParams,
        _results: que::SetDeliveryModeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setDeliveryMode") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_dead_letter_policy(
        self: Rc<Self>,
        _params: que::SetDeadLetterPolicyParams,
        _results: que::SetDeadLetterPolicyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setDeadLetterPolicy") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_ordering(
        self: Rc<Self>,
        _params: que::SetOrderingParams,
        _results: que::SetOrderingResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setOrdering") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_encryption(
        self: Rc<Self>,
        _params: que::SetEncryptionParams,
        _results: que::SetEncryptionResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setEncryption") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_message_ttl(
        self: Rc<Self>,
        _params: que::SetMessageTtlParams,
        _results: que::SetMessageTtlResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setMessageTtl") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_flow_control(
        self: Rc<Self>,
        _params: que::SetFlowControlParams,
        _results: que::SetFlowControlResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setFlowControl") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_dead_letters(
        self: Rc<Self>,
        _params: que::ListDeadLettersParams,
        _results: que::ListDeadLettersResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listDeadLetters") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn retry_dead_letter(
        self: Rc<Self>,
        _params: que::RetryDeadLetterParams,
        _results: que::RetryDeadLetterResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("retryDeadLetter") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn create_consumer_group(
        self: Rc<Self>,
        _params: que::CreateConsumerGroupParams,
        _results: que::CreateConsumerGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("createConsumerGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete_consumer_group(
        self: Rc<Self>,
        _params: que::DeleteConsumerGroupParams,
        _results: que::DeleteConsumerGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deleteConsumerGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_consumer_group(
        self: Rc<Self>,
        _params: que::GetConsumerGroupParams,
        _results: que::GetConsumerGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getConsumerGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_consumer_groups(
        self: Rc<Self>,
        _params: que::ListConsumerGroupsParams,
        _results: que::ListConsumerGroupsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listConsumerGroups") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_consumer_to_group(
        self: Rc<Self>,
        _params: que::AddConsumerToGroupParams,
        _results: que::AddConsumerToGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addConsumerToGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn remove_consumer_from_group(
        self: Rc<Self>,
        _params: que::RemoveConsumerFromGroupParams,
        _results: que::RemoveConsumerFromGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("removeConsumerFromGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn dequeue_from_group(
        self: Rc<Self>,
        _params: que::DequeueFromGroupParams,
        _results: que::DequeueFromGroupResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("dequeueFromGroup") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn deduplicate(
        self: Rc<Self>,
        _params: que::DeduplicateParams,
        _results: que::DeduplicateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("deduplicate") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
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
