//! The byte-stream pump: a blocking transport socket as the async stream
//! the RPC runtime drives.
//!
//! # Why a pump and not `AllowStdIo`
//!
//! capnp RPC reads its messages through an [`AsyncRead`] and writes them
//! through an [`AsyncWrite`]. The tree's transports are *blocking* sockets.
////! Adapting them with a blocking adapter would deadlock: the RPC runtime
//! awaits the next inbound message inside its connection task, and while it
//! waits there, the task that must write the answer never gets polled.
//!
//! So the read half is pumped by its own thread: the thread blocks on the
//! socket (that is what a socket is for) and hands what it read to the
//! executor through a channel. The write half is pumped the same way, so
//! the executor thread never blocks on a full socket buffer either.
//!
//! # What this file does not do
//!
//! It does not frame anything. The bytes that cross here are the foreign
//! protocol's bytes, end to end: the transport's own framing stays off this
//! path (that is why [`crate::transport::channel::Channel::into_byte_stream`]
//! *consumes* the channel).

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::channel::{mpsc, oneshot};
use futures::future::Future;
use futures::io::{AsyncRead, AsyncWrite};
use futures::StreamExt;

use crate::transport::channel::{ByteStream, TransportError};

/// How much the reader thread pulls from the socket at a time.
const READ_CHUNK: usize = 64 * 1024;

/// A transport's byte stream, pumped into the async world.
pub struct PumpedStream {
    reader: ReaderPump,
    writer: WriterPump,
    stream: Arc<dyn ByteStream>,
}

impl PumpedStream {
    /// Pump a transport's byte stream.
    ///
    /// The stream is consumed: from here on, the transport's own framing is
    /// not used on this connection at all.
    pub fn new(stream: Box<dyn ByteStream>) -> Self {
        let reader: Arc<dyn ByteStream> = Arc::from(stream);
        let writer = Arc::clone(&reader);
        Self {
            reader: ReaderPump::spawn(Arc::clone(&reader)),
            writer: WriterPump::spawn(writer),
            stream: reader,
        }
    }

    /// The read half.
    pub fn reader(self) -> ReaderPump {
        self.reader
    }

    /// The write half.
    pub fn writer(self) -> WriterPump {
        self.writer
    }

    /// Both halves, for the RPC runtime (which wants them separately).
    pub fn split(self) -> (ReaderPump, WriterPump) {
        (self.reader, self.writer)
    }

    /// The stream underneath: closing it ends the connection.
    pub fn stream(&self) -> Arc<dyn ByteStream> {
        Arc::clone(&self.stream)
    }
}

/// The read half: a thread that blocks on the socket, an executor side that
/// never blocks.
pub struct ReaderPump {
    receiver: mpsc::UnboundedReceiver<Result<Vec<u8>, TransportError>>,
    chunk: Vec<u8>,
    offset: usize,
    failure: Option<TransportError>,
}

impl ReaderPump {
    /// Start the reader thread.
    fn spawn(stream: Arc<dyn ByteStream>) -> Self {
        let (sender, receiver) = mpsc::unbounded();
        std::thread::Builder::new()
            .name("smart-rpc-reader".to_string())
            .spawn(move || {
                let mut buffer = vec![0u8; READ_CHUNK];
                loop {
                    match stream.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            let chunk = buffer[..count].to_vec();
                            if sender.unbounded_send(Ok(chunk)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = sender.unbounded_send(Err(error));
                            break;
                        }
                    }
                }
            })
            .expect("the reader thread is the only thing that can fail to start here");
        Self {
            receiver,
            chunk: Vec::new(),
            offset: 0,
            failure: None,
        }
    }
}

impl AsyncRead for ReaderPump {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            if this.offset < this.chunk.len() {
                let count = core::cmp::min(buf.len(), this.chunk.len() - this.offset);
                buf[..count].copy_from_slice(&this.chunk[this.offset..this.offset + count]);
                this.offset += count;
                return Poll::Ready(Ok(count));
            }
            if let Some(error) = this.failure.take() {
                return Poll::Ready(Err(io::Error::other(error.detail())));
            }
            match this.receiver.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.chunk = chunk;
                    this.offset = 0;
                }
                Poll::Ready(Some(Err(error))) => this.failure = Some(error),
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// What the writer thread is asked to do.
enum WriteJob {
    /// These bytes go to the socket, in order.
    Data(Vec<u8>),
    /// Everything written so far must be on the socket; answer when it is.
    Flush(oneshot::Sender<io::Result<()>>),
}

/// The write half: an executor side that queues, a thread that writes.
pub struct WriterPump {
    sender: mpsc::UnboundedSender<WriteJob>,
    pending: Option<oneshot::Receiver<io::Result<()>>>,
}

impl WriterPump {
    /// Start the writer thread.
    fn spawn(stream: Arc<dyn ByteStream>) -> Self {
        let (sender, mut receiver) = mpsc::unbounded::<WriteJob>();
        std::thread::Builder::new()
            .name("smart-rpc-writer".to_string())
            .spawn(move || {
                // The writer thread runs its own mini-executor: the RPC
                // executor never blocks on the socket.
                while let Some(job) = futures::executor::block_on(receiver.next()) {
                    match job {
                        WriteJob::Data(bytes) => {
                            if stream.write_all(&bytes).is_err() {
                                break;
                            }
                        }
                        WriteJob::Flush(answer) => {
                            let outcome = stream.flush();
                            let failed = outcome.is_err();
                            let _ = answer.send(outcome.map_err(|error| {
                                io::Error::other(error.detail())
                            }));
                            if failed {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("the writer thread is the only thing that can fail to start here");
        Self {
            sender,
            pending: None,
        }
    }
}

impl AsyncWrite for WriterPump {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let _ = &this.sender;
        // Order matters: a write must not overtake a flush that is still
        // outstanding.
        if this.pending.is_some() {
            match this.poll_pending(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        let count = buf.len();
        this.sender
            .unbounded_send(WriteJob::Data(buf.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the write pump is closed"))?;
        Poll::Ready(Ok(count))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pending.is_none() {
            let (answer, receipt) = oneshot::channel();
            this.sender
                .unbounded_send(WriteJob::Flush(answer))
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the write pump is closed"))?;
            this.pending = Some(receipt);
        }
        this.poll_pending(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl WriterPump {
    /// Await the outstanding flush, if there is one.
    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(receipt) = self.pending.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        match Pin::new(receipt).poll(cx) {
            Poll::Ready(Ok(outcome)) => {
                self.pending = None;
                Poll::Ready(outcome)
            }
            Poll::Ready(Err(_)) => {
                self.pending = None;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the write pump is closed",
                )))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
