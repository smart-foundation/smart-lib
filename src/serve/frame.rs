//! The dispatch wire format: the envelope every served method travels in.
//!
//! ## The one rule this file obeys
//!
//! **The envelope is transport machinery and never names a domain; the
//! payload is the contract's own bytes.** There is no second wire
//! vocabulary: the params bytes inside a request are exactly the capnp
//! serialisation of the contract's `<method>Params` struct, and the payload
//! bytes inside a response are exactly the capnp serialisation of the
//! contract's `<method>Results` struct (or, for a subscription, the
//! service-minted id plus the contract's event struct). Everything in this
//! module is fixed-offset little-endian framing, zero dependencies, and it
//! would look identical for the `files` domain or any other — the only
//! domain-specific number anywhere in a frame is the `interfaceId`, which is
//! the schema's own compiled id (`network.capnp`'s `@0x…` line), never a
//! domain noun.
//!
//! ## How a frame travels
//!
//! The channel moves capnp messages, so a serve frame is carried as the
//! single segment of a one-segment capnp message (see
//! [`crate::serve`], which does the send/recv). The frame is therefore
//! word-padded at the tail: the lengths in the headers are exact, and the
//! padding is zero filler the reader never looks at. That keeps the channel
//! seam's word-alignment law intact without the envelope needing a schema of
//! its own — a schema for the envelope would be exactly the "second wire
//! vocabulary" this file refuses to be.
//!
//! ## Request frame (little-endian)
//!
//! ```text
//! [0..8]   interfaceId  u64   the domain schema id; any other id is answered
//!                             "unknown-interface" (the service serves one id)
//! [8..10]  methodOrdinal u16  the contract's own ordinal (0..=82 here), or
//!                             one of the subscription pseudo-ordinals
//! [10..12] paramsLen    u16   length of the params bytes that follow
//! [12..16] flags        u32   reserved, MUST be 0 (a peer that sets a flag
//!                             this build does not know fails loudly)
//! [16..16+paramsLen]             params bytes (capnp-serialized param struct)
//! [16+paramsLen..]               attached capability bytes, if any
//! ```
//!
//! ### Subscription pseudo-ordinals
//!
//! The contract's subscription params are *capabilities*
//! (`StatusSubscription` / `CounterSubscription`), and a capability cannot
//! travel as frame bytes — that is the whole point of
//! [`CapabilityTransfer`](crate::transport::channel::CapabilityTransfer). So
//! the wire carries subscription ids in-band through two pseudo-ordinals,
//! minted by the service at `subscribe`:
//!
//! * `SUB_PULL = 0xFFFE` — params `[0..8] subId u64 + [8..12] limit u32`.
//!   `limit` bounds the read (the caller's backpressure); 0 means "one".
//! * `SUB_CANCEL = 0xFFFF` — params `[0..8] subId u64`. This is the same
//!   ordinal the availability matrix gives the domain's `cancelSubscription`
//!   helper, so the wire and the matrix agree.
//!
//! ## Response frame (little-endian)
//!
//! ```text
//! [0..2]   status     u16   0 = ok, 1 = error
//! [2..4]   msgLen     u16   length of the UTF-8 message bytes
//! [4..8]   payloadLen u32   length of the payload bytes
//! [8..8+msgLen]             message bytes (UTF-8, may be empty)
//! [8+msgLen..]              payload bytes (the contract's own bytes)
//! ```
//!
//! An error response's message carries the failure's vocabulary — for an
//! unavailable method the matrix row's `reasonClass` token, then the row's
//! reason; for a capability-class refusal the contract code name
//! `permissionDenied`. The error payload is the serialized contract `T.Error`
//! (code / message / context / retryable), so an error answer is the
//! contract's own struct, not a second error language.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Bytes of a capnp word — the unit the channel's segment law is written in.
pub const WORD_BYTES: usize = 8;

/// Request header length: interfaceId, methodOrdinal, paramsLen, flags.
pub const REQUEST_HEADER_LEN: usize = 16;

/// Response header length: status, msgLen, payloadLen.
pub const RESPONSE_HEADER_LEN: usize = 8;

/// `status` value for a successful response.
pub const STATUS_OK: u16 = 0;

/// `status` value for an error response (its payload is the `T.Error`).
pub const STATUS_ERROR: u16 = 1;

/// Pseudo-ordinal: pull one subscription's next event(s).
pub const SUB_PULL: u16 = 0xFFFE;

/// Pseudo-ordinal: retire a subscription (the matrix's `cancelSubscription`).
pub const SUB_CANCEL: u16 = 0xFFFF;

/// One parsed request frame, borrowing the bytes it was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    /// The domain schema id this call names.
    pub interface_id: u64,
    /// The contract method ordinal (or a subscription pseudo-ordinal).
    pub ordinal: u16,
    /// The reserved flags word (always 0; a non-zero value is refused).
    pub flags: u32,
    /// The capnp-serialized params struct.
    pub params: &'a [u8],
    /// Attached capability bytes, if the caller staged any.
    pub caps: &'a [u8],
}

/// Why a frame could not be parsed. Typed so the answer names what was wrong;
/// a truncated or self-inconsistent frame is a peer protocol violation, not
/// a backend result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// The bytes are not a frame this build understands.
    Malformed {
        /// What was wrong, named.
        detail: String,
    },
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RequestError::Malformed { detail } => write!(f, "bad-frame: {detail}"),
        }
    }
}

/// Parse one request frame. The frame must be exactly long enough for its
/// own declared params length: trailing bytes beyond `paramsLen` are the
/// attached capability bytes, and anything shorter is malformed.
pub fn parse_request(bytes: &[u8]) -> Result<Request<'_>, RequestError> {
    if bytes.len() < REQUEST_HEADER_LEN {
        return Err(RequestError::Malformed {
            detail: format!(
                "request frame is {} bytes; the header alone is {REQUEST_HEADER_LEN}",
                bytes.len()
            ),
        });
    }
    let interface_id = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let ordinal = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
    let params_len = u16::from_le_bytes(bytes[10..12].try_into().unwrap()) as usize;
    let flags = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    if flags != 0 {
        return Err(RequestError::Malformed {
            detail: format!("reserved flags word is {flags:#x}; this build speaks flags 0"),
        });
    }
    let params_start = REQUEST_HEADER_LEN;
    let params_end = params_start
        .checked_add(params_len)
        .ok_or_else(|| RequestError::Malformed {
            detail: "params length overflows the frame".to_string(),
        })?;
    if bytes.len() < params_end {
        return Err(RequestError::Malformed {
            detail: format!(
                "params length {params_len} exceeds the {} bytes the frame carries",
                bytes.len() - REQUEST_HEADER_LEN
            ),
        });
    }
    Ok(Request {
        interface_id,
        ordinal,
        flags,
        params: &bytes[params_start..params_end],
        caps: &bytes[params_end..],
    })
}

/// The parsed subscription id and bound of a `SUB_PULL` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// The service-minted subscription id.
    pub sub_id: u64,
    /// The caller's bound: the most events this response may carry (0 = one).
    pub limit: u32,
}

/// Parse `SUB_PULL` params: `[0..8] subId u64 + [8..12] limit u32`.
pub fn parse_pull_params(bytes: &[u8]) -> Result<PullRequest, RequestError> {
    if bytes.len() < 12 {
        return Err(RequestError::Malformed {
            detail: format!(
                "SUB_PULL params are [subId u64][limit u32] = 12 bytes; got {}",
                bytes.len()
            ),
        });
    }
    Ok(PullRequest {
        sub_id: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        limit: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
    })
}

/// Parse `SUB_CANCEL` params: `[0..8] subId u64`.
pub fn parse_cancel_params(bytes: &[u8]) -> Result<u64, RequestError> {
    if bytes.len() < 8 {
        return Err(RequestError::Malformed {
            detail: format!(
                "SUB_CANCEL params are [subId u64] = 8 bytes; got {}",
                bytes.len()
            ),
        });
    }
    Ok(u64::from_le_bytes(bytes[0..8].try_into().unwrap()))
}

/// One response frame: the status, the message vocabulary, and the contract's
/// own payload bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// 0 ok / 1 error.
    pub status: u16,
    /// UTF-8 message: empty on a plain ok, the failure's vocabulary on error.
    pub message: String,
    /// The contract's own bytes (a result struct, or the `T.Error` on error).
    pub payload: Vec<u8>,
}

impl Response {
    /// A successful response carrying `payload` (the contract's result bytes).
    pub fn ok(payload: Vec<u8>) -> Self {
        Response {
            status: STATUS_OK,
            message: String::new(),
            payload,
        }
    }

    /// An error response with a message and no payload (a frame-level fault:
    /// the request never named a method).
    pub fn error(message: impl Into<String>) -> Self {
        Response {
            status: STATUS_ERROR,
            message: message.into(),
            payload: Vec::new(),
        }
    }

    /// An error response whose payload is the serialized contract `T.Error`
    /// the caller provided in `payload` bytes.
    pub fn error_with_payload(message: impl Into<String>, payload: Vec<u8>) -> Self {
        Response {
            status: STATUS_ERROR,
            message: message.into(),
            payload,
        }
    }

    /// Encode the frame, word-padded so the channel's segment law holds. The
    /// header lengths are exact; the padding is zero filler.
    pub fn encode(&self) -> Vec<u8> {
        let message = self.message.as_bytes();
        let len = RESPONSE_HEADER_LEN + message.len() + self.payload.len();
        let mut out = Vec::with_capacity(padded_len(len));
        out.extend_from_slice(&self.status.to_le_bytes());
        out.extend_from_slice(&(message.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(message);
        out.extend_from_slice(&self.payload);
        out.resize(padded_len(len), 0);
        out
    }
}

/// One parsed response frame, borrowing the bytes it was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseView<'a> {
    /// 0 ok / 1 error.
    pub status: u16,
    /// The message bytes (valid UTF-8 when the peer wrote UTF-8).
    pub message: &'a [u8],
    /// The payload bytes.
    pub payload: &'a [u8],
}

impl<'a> ResponseView<'a> {
    /// The message as a string (lossy: a peer's invalid UTF-8 is a protocol
    /// violation that message validation should have caught; it reads as
    /// replacement characters here rather than fabricating a message).
    pub fn message_text(&self) -> String {
        String::from_utf8_lossy(self.message).into_owned()
    }

    /// True for a successful response.
    pub fn is_ok(&self) -> bool {
        self.status == STATUS_OK
    }
}

/// Parse one response frame (the client side of this format).
pub fn parse_response(bytes: &[u8]) -> Result<ResponseView<'_>, RequestError> {
    if bytes.len() < RESPONSE_HEADER_LEN {
        return Err(RequestError::Malformed {
            detail: format!(
                "response frame is {} bytes; the header alone is {RESPONSE_HEADER_LEN}",
                bytes.len()
            ),
        });
    }
    let status = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
    let msg_len = u16::from_le_bytes(bytes[2..4].try_into().unwrap()) as usize;
    let payload_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let msg_start = RESPONSE_HEADER_LEN;
    let msg_end = msg_start
        .checked_add(msg_len)
        .ok_or_else(|| RequestError::Malformed {
            detail: "message length overflows the frame".to_string(),
        })?;
    let payload_end = msg_end
        .checked_add(payload_len)
        .ok_or_else(|| RequestError::Malformed {
            detail: "payload length overflows the frame".to_string(),
        })?;
    if bytes.len() < payload_end {
        return Err(RequestError::Malformed {
            detail: format!(
                "declared message {msg_len} + payload {payload_len} bytes exceed the {} bytes the frame carries",
                bytes.len() - RESPONSE_HEADER_LEN
            ),
        });
    }
    Ok(ResponseView {
        status,
        message: &bytes[msg_start..msg_end],
        payload: &bytes[msg_end..payload_end],
    })
}

/// Encode a request frame, word-padded like [`Response::encode`]. `params`
/// is the capnp-serialized param struct; `caps` is any attached capability
/// bytes (empty for every method the network surface serves).
pub fn encode_request(interface_id: u64, ordinal: u16, params: &[u8], caps: &[u8]) -> Vec<u8> {
    let len = REQUEST_HEADER_LEN + params.len() + caps.len();
    let mut out = Vec::with_capacity(padded_len(len));
    out.extend_from_slice(&interface_id.to_le_bytes());
    out.extend_from_slice(&ordinal.to_le_bytes());
    out.extend_from_slice(&(params.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(params);
    out.extend_from_slice(caps);
    out.resize(padded_len(len), 0);
    out
}

/// A subscription subscribe request's payload prefix: the service-minted id
/// the first event's bytes follow.
pub const SUB_ID_LEN: usize = 8;

/// Prepend the service-minted subscription id to an event's payload — the
/// shape [`SUB_PULL`] and `subscribe` responses share.
pub fn payload_with_sub_id(sub_id: u64, event: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SUB_ID_LEN + event.len());
    out.extend_from_slice(&sub_id.to_le_bytes());
    out.extend_from_slice(event);
    out
}

/// Read the subscription id back off a subscription payload.
pub fn sub_id_of(payload: &[u8]) -> Option<u64> {
    payload
        .get(..SUB_ID_LEN)
        .map(|head| u64::from_le_bytes(head.try_into().unwrap()))
}

/// `len` rounded up to a whole capnp word (the padding the channel's segment
/// law needs; the frame's own header lengths stay exact).
pub fn padded_len(len: usize) -> usize {
    (len + WORD_BYTES - 1) & !(WORD_BYTES - 1)
}
