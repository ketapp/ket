//! What carries a session's frames.

use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A duplex carrier of whole frames.
///
/// The relay is the one production implementation: it routes each frame,
/// wrapped in `ket-relay-protocol`'s header, to the other end. Nothing in the
/// protocol above knows or cares what the carrier is.
pub trait Pipe {
    /// Sends one frame.
    fn send(&mut self, frame: Vec<u8>) -> io::Result<()>;
    /// Waits for the next frame.
    fn recv(&mut self) -> io::Result<Vec<u8>>;
}

/// One end of an in-memory pipe. See [`memory_pipe`].
#[derive(Debug)]
pub struct MemoryPipe {
    out: Sender<Vec<u8>>,
    into: Receiver<Vec<u8>>,
}

/// Two connected ends of a pipe in memory.
///
/// For exercising the protocol without a relay. Not a transport: a phone and
/// a host only ever meet through the relay.
pub fn memory_pipe() -> (MemoryPipe, MemoryPipe) {
    let (a_out, b_in) = channel();
    let (b_out, a_in) = channel();
    (
        MemoryPipe {
            out: a_out,
            into: a_in,
        },
        MemoryPipe {
            out: b_out,
            into: b_in,
        },
    )
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the other end has gone")
}

impl Pipe for MemoryPipe {
    fn send(&mut self, frame: Vec<u8>) -> io::Result<()> {
        self.out.send(frame).map_err(|_| closed())
    }

    fn recv(&mut self) -> io::Result<Vec<u8>> {
        self.into.recv().map_err(|_| closed())
    }
}

impl<P: Pipe + ?Sized> Pipe for &mut P {
    fn send(&mut self, frame: Vec<u8>) -> io::Result<()> {
        (**self).send(frame)
    }

    fn recv(&mut self) -> io::Result<Vec<u8>> {
        (**self).recv()
    }
}
