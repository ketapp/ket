//! The relay's outer frame (Epic 5b.2).
//!
//! Everything the relay handles, and everything it can see: which connection
//! a frame belongs to, whether it is relay control or data, and how long it
//! is. The data is ciphertext from an end-to-end Noise session between a
//! phone and a host (`ket-remote`), and this crate cannot name its types —
//! there is no dependency to name them through. That boundary is the design:
//! a relay that could decode what it forwards would be a relay that could
//! leak it.
//!
//! ```text
//! version u8 | kind u8 | connection u64 | length u32 | body
//! ```
//!
//! Big-endian, 14 bytes of header, and at most [`MAX_FRAME`] bytes in all. A
//! larger application message is split by the sender and reassembled by the
//! receiver, end to end, never by the relay.

use std::fmt;

/// This frame format's version. A relay rejects frames of any other.
pub const VERSION: u8 = 1;

/// Bytes of header before the body.
pub const HEADER: usize = 14;

/// Largest frame, header included: 1 MiB.
pub const MAX_FRAME: usize = 1024 * 1024;

/// What a frame is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Between an endpoint and the relay itself: admission, routing.
    Control,
    /// Between the two endpoints, through the relay: opaque ciphertext.
    Data,
}

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Which end-to-end connection the relay should route it on.
    pub connection: u64,
    /// See [`Kind`].
    pub kind: Kind,
    /// The payload. For [`Kind::Data`], ciphertext.
    pub body: Vec<u8>,
}

/// Why bytes are not a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Fewer bytes than a header, or than the header says.
    Short,
    /// A frame format this build does not speak.
    Version(u8),
    /// A kind byte that means nothing.
    Kind(u8),
    /// More than [`MAX_FRAME`], or bytes after the body.
    Size(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Short => write!(f, "frame is truncated"),
            Self::Version(v) => write!(f, "relay frame version {v}, expected {VERSION}"),
            Self::Kind(k) => write!(f, "relay frame kind {k}"),
            Self::Size(n) => write!(f, "relay frame of {n} bytes"),
        }
    }
}

impl std::error::Error for Error {}

impl Frame {
    /// A data frame.
    pub fn data(connection: u64, body: Vec<u8>) -> Self {
        Self {
            connection,
            kind: Kind::Data,
            body,
        }
    }

    /// The largest body a frame can carry.
    pub const MAX_BODY: usize = MAX_FRAME - HEADER;

    /// Encodes the frame, or refuses one that is too large.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let size = HEADER + self.body.len();
        if size > MAX_FRAME {
            return Err(Error::Size(size));
        }
        let mut out = Vec::with_capacity(size);
        out.push(VERSION);
        out.push(match self.kind {
            Kind::Control => 1,
            Kind::Data => 2,
        });
        out.extend_from_slice(&self.connection.to_be_bytes());
        out.extend_from_slice(&(self.body.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.body);
        Ok(out)
    }

    /// Decodes exactly one frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER {
            return Err(Error::Short);
        }
        if bytes.len() > MAX_FRAME {
            return Err(Error::Size(bytes.len()));
        }
        if bytes[0] != VERSION {
            return Err(Error::Version(bytes[0]));
        }
        let kind = match bytes[1] {
            1 => Kind::Control,
            2 => Kind::Data,
            other => return Err(Error::Kind(other)),
        };
        let mut connection = [0u8; 8];
        connection.copy_from_slice(&bytes[2..10]);
        let mut len = [0u8; 4];
        len.copy_from_slice(&bytes[10..14]);
        let len = u32::from_be_bytes(len) as usize;
        let body = &bytes[HEADER..];
        if body.len() < len {
            return Err(Error::Short);
        }
        if body.len() > len {
            return Err(Error::Size(bytes.len()));
        }
        Ok(Self {
            connection: u64::from_be_bytes(connection),
            kind,
            body: body.to_vec(),
        })
    }
}

/// What a control frame says. Its [`Frame::connection`] names the connection
/// it is about, where it is about one.
///
/// ```text
/// op u8 | payload
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Host to relay: route phones asking for this host id to me.
    Register {
        /// The host id from the host's pairing codes.
        host: [u8; 16],
    },
    /// Phone to relay: connect me to this host.
    Connect {
        /// The host id from the pairing code.
        host: [u8; 16],
    },
    /// Relay to host: a phone has connected, on the frame's connection.
    Opened,
    /// Relay to phone: you are connected, on the frame's connection.
    Connected,
    /// Either way: the frame's connection has ended.
    Closed,
    /// Relay to either: a request was refused.
    Refused {
        /// Why. See the `REFUSED_*` constants.
        reason: u8,
    },
    /// Host to its own relay: route phones asking for this host id to me,
    /// and here is the key that shows I am the host that started you. A
    /// relay running inside a host takes only this, never [`Self::Register`],
    /// so nothing else that can reach the port — even on the same Mac — can
    /// take the host's place.
    Claim {
        /// The host id from the host's pairing codes.
        host: [u8; 16],
        /// The secret the host gave its relay when it started it.
        key: [u8; 32],
    },
}

/// The host a phone asked for is not connected to the relay.
pub const REFUSED_HOST_OFFLINE: u8 = 1;
/// The first frame on a socket was not a registration or a connection.
pub const REFUSED_PROTOCOL: u8 = 2;
/// The socket fell too far behind reading what was sent to it.
pub const REFUSED_BACKPRESSURE: u8 = 3;

impl Control {
    /// This control message as a frame on `connection`.
    pub fn frame(&self, connection: u64) -> Frame {
        let mut body = Vec::with_capacity(17);
        match self {
            Self::Register { host } => {
                body.push(1);
                body.extend_from_slice(host);
            }
            Self::Connect { host } => {
                body.push(2);
                body.extend_from_slice(host);
            }
            Self::Opened => body.push(3),
            Self::Connected => body.push(4),
            Self::Closed => body.push(5),
            Self::Refused { reason } => {
                body.push(6);
                body.push(*reason);
            }
            Self::Claim { host, key } => {
                body.push(7);
                body.extend_from_slice(host);
                body.extend_from_slice(key);
            }
        }
        Frame {
            connection,
            kind: Kind::Control,
            body,
        }
    }

    /// Reads a control frame's body.
    pub fn parse(body: &[u8]) -> Result<Self, Error> {
        let host =
            |rest: &[u8]| -> Result<[u8; 16], Error> { rest.try_into().map_err(|_| Error::Short) };
        match body.split_first() {
            Some((1, rest)) => Ok(Self::Register { host: host(rest)? }),
            Some((2, rest)) => Ok(Self::Connect { host: host(rest)? }),
            Some((3, [])) => Ok(Self::Opened),
            Some((4, [])) => Ok(Self::Connected),
            Some((5, [])) => Ok(Self::Closed),
            Some((6, [reason])) => Ok(Self::Refused { reason: *reason }),
            Some((7, rest)) if rest.len() == 48 => Ok(Self::Claim {
                host: host(&rest[..16])?,
                key: rest[16..].try_into().map_err(|_| Error::Short)?,
            }),
            Some((7, _)) => Err(Error::Short),
            Some((op, _)) => Err(Error::Kind(*op)),
            None => Err(Error::Short),
        }
    }
}
