pub const QUEUE_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60030;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass { NoBackend, NoPrivilege, NotApplicable, DisabledByPolicy, Unknown }
impl ReasonClass { pub const fn as_str(self) -> &'static str { match self { ReasonClass::NoBackend => "no-backend", ReasonClass::NoPrivilege => "no-privilege", ReasonClass::NotApplicable => "not-applicable", ReasonClass::DisabledByPolicy => "disabled-by-policy", ReasonClass::Unknown => "unknown" } } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus { Real, Degraded, Unavailable }

#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability { pub name: &'static str, pub ordinal: u16, pub status: AvailStatus, pub reason_class: Option<ReasonClass>, pub reason: &'static str, pub l3_test: Option<&'static str> }
impl MethodAvailability {
  const fn real(name: &'static str, ordinal: u16, l3: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Real, reason_class: None, reason: "", l3_test: Some(l3) } }
  const fn unavailable(name: &'static str, ordinal: u16, class: ReasonClass, reason: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Unavailable, reason_class: Some(class), reason, l3_test: None } }
}

pub const QUEUE_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("enqueue", 0, "l3_linux_queue::core_works"),
    MethodAvailability::real("dequeue", 1, "l3_linux_queue::core_works"),
    MethodAvailability::real("peek", 2, "l3_linux_queue::core_works"),
    MethodAvailability::real("getQueue", 3, "l3_linux_queue::core_works"),
    MethodAvailability::real("clearQueue", 4, "l3_linux_queue::core_works"),
    MethodAvailability::real("deleteQueue", 5, "l3_linux_queue::core_works"),
    MethodAvailability::real("listQueues", 6, "l3_linux_queue::core_works"),
    MethodAvailability::unavailable("subscribeQueue", 7, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("subscribeWithFilter", 8, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("getStatistics", 9, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("enqueueAll", 10, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("dequeueAck", 11, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("ack", 12, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("nack", 13, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("replay", 14, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("setDeliveryMode", 15, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("setDeadLetterPolicy", 16, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("setOrdering", 17, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("setEncryption", 18, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("setMessageTtl", 19, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("setFlowControl", 20, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("listDeadLetters", 21, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("retryDeadLetter", 22, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("createConsumerGroup", 23, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("deleteConsumerGroup", 24, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("getConsumerGroup", 25, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("listConsumerGroups", 26, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("addConsumerToGroup", 27, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("removeConsumerFromGroup", 28, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("dequeueFromGroup", 29, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("deduplicate", 30, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("pull", 31, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
    MethodAvailability::unavailable("cancel", 32, ReasonClass::NoBackend, "no native queue surface for this method on this host"),
];

pub const PLATFORM: &str = "linux";
pub const TRANSPORTS: &[&str] = &["InProcess"];
pub const CAPS: &[&str] = &[];

pub fn status_counts() -> (usize, usize, usize) { let mut r=0;let mut d=0;let mut u=0; for row in QUEUE_AVAILABILITY { match row.status { AvailStatus::Real=>r+=1, AvailStatus::Degraded=>d+=1, AvailStatus::Unavailable=>u+=1 } } (r,d,u) }
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> { QUEUE_AVAILABILITY.iter().filter(|r| r.l3_test.is_some()).map(|r| (r.name, r.l3_test.unwrap())) }
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> { QUEUE_AVAILABILITY.iter().find(|r| r.name == name) }
