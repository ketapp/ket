//! An established Noise session, and the splitting of large messages across
//! its frames.

use snow::TransportState;

use crate::{Error, NOISE_MAX as MAXMSGLEN, NOISE_TAG as TAGLEN, Pipe, Result};

/// Largest application message a session carries: a checkpoint of a full
/// scrollback in which every cell has its own colour is under 4 MB.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Plaintext bytes per frame: a Noise message's limit, less the tag, less the
/// one byte that says whether more of the message follows.
const CHUNK: usize = MAXMSGLEN - TAGLEN - 1;

const LAST: u8 = 0;
const MORE: u8 = 1;

/// A session after the handshake: every frame encrypted, authenticated, and
/// numbered by its position, so one that is replayed, dropped or reordered
/// fails to decrypt and ends the session.
pub struct Session<P> {
    pipe: P,
    noise: TransportState,
    /// A message being reassembled by [`Session::open`].
    partial: Vec<u8>,
}

impl<P> std::fmt::Debug for Session<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl<P: Pipe> Session<P> {
    pub(crate) fn new(pipe: P, noise: TransportState) -> Self {
        Self {
            pipe,
            noise,
            partial: Vec::new(),
        }
    }

    /// The other side's static public key, as the handshake proved it.
    pub fn remote_static(&self) -> Option<Vec<u8>> {
        self.noise.get_remote_static().map(<[u8]>::to_vec)
    }

    /// Encrypts one message into the frames that carry it, without sending
    /// them — for a caller that does its own I/O, like a host multiplexing a
    /// phone's requests with output it is pushing. Frames must go out in the
    /// order given, and in the order `seal` was called.
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>> {
        if message.len() > MAX_MESSAGE {
            return Err(Error::TooLarge(message.len()));
        }
        let mut frames = Vec::with_capacity(message.len() / CHUNK + 1);
        let mut chunks = message.chunks(CHUNK).peekable();
        // An empty message is still one frame.
        if chunks.peek().is_none() {
            frames.push(self.seal_chunk(LAST, &[])?);
        }
        while let Some(chunk) = chunks.next() {
            let flag = if chunks.peek().is_some() { MORE } else { LAST };
            frames.push(self.seal_chunk(flag, chunk)?);
        }
        Ok(frames)
    }

    fn seal_chunk(&mut self, flag: u8, chunk: &[u8]) -> Result<Vec<u8>> {
        let mut plain = Vec::with_capacity(1 + chunk.len());
        plain.push(flag);
        plain.extend_from_slice(chunk);
        let mut frame = vec![0u8; plain.len() + TAGLEN];
        let len = self.noise.write_message(&plain, &mut frame)?;
        frame.truncate(len);
        Ok(frame)
    }

    /// Decrypts one frame: the whole message once its last frame is in,
    /// `None` while more are to come. The counterpart of [`Session::seal`].
    pub fn open(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut plain = vec![0u8; MAXMSGLEN];
        let len = self.noise.read_message(frame, &mut plain)?;
        let Some((&flag, chunk)) = plain[..len].split_first() else {
            return Err(Error::Protocol("an empty frame"));
        };
        if self.partial.len() + chunk.len() > MAX_MESSAGE {
            return Err(Error::TooLarge(self.partial.len() + chunk.len()));
        }
        self.partial.extend_from_slice(chunk);
        match flag {
            LAST => Ok(Some(std::mem::take(&mut self.partial))),
            MORE => Ok(None),
            _ => Err(Error::Protocol("a frame with an unknown flag")),
        }
    }

    /// Sends one message, split across as many frames as it takes.
    pub fn send(&mut self, message: &[u8]) -> Result<()> {
        for frame in self.seal(message)? {
            self.pipe.send(frame)?;
        }
        Ok(())
    }

    /// Receives one message, reassembled from its frames.
    pub fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            let frame = self.pipe.recv()?;
            if let Some(message) = self.open(&frame)? {
                return Ok(message);
            }
        }
    }

    /// The pipe underneath, for a caller that has to reach past the session —
    /// to close it, or, in a test of the protocol, to tamper with it.
    pub fn pipe_mut(&mut self) -> &mut P {
        &mut self.pipe
    }
}
