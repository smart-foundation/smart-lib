//! The domain command modules. `network` is the one real domain on the new
//! engine; every other domain is the migration queue's data, answered
//! honestly — never faked.

pub mod network;
