//! The phone protocol (Epic 5b.2): what a phone and a ket host say to each
//! other, and how nobody in between gets to read it.
//!
//! Three layers, from the outside in:
//!
//! - **A pipe** ([`Pipe`]): something that carries whole frames both ways. In
//!   production that is the relay; `ket-relay-protocol` wraps each frame for
//!   routing and can see nothing inside it. [`memory_pipe`] exists so the
//!   protocol can be exercised without one — it is not a second transport.
//! - **A Noise session** ([`Session`]), from `snow`: `IKpsk2` the first time a
//!   phone pairs, with the one-time secret from the host's QR code as the
//!   pre-shared key, and `IK` on every reconnect after, which the host accepts
//!   only from a phone key it has granted. Every frame after the handshake is
//!   encrypted and authenticated, and its nonce is its position in the
//!   session, so a replayed or reordered frame does not decrypt.
//! - **The application contract** ([`proto`]): Protobuf, generated from
//!   `proto/ket/remote/v1/remote.proto`, the same file the TypeScript app
//!   generates from. A message larger than one Noise frame is split and
//!   reassembled here, end to end.
//!
//! What this crate does not do yet: talk to a relay (5b.3), keep keys in the
//! Keychain, or serve anything real from a host. It is the protocol, whole,
//! with the parts around it still to come.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use prost::Message;

/// The application contract, generated from `remote.proto`.
#[allow(missing_docs, clippy::all)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/ket.remote.v1.rs"));
}

mod pipe;
mod session;

pub use pipe::{MemoryPipe, Pipe, memory_pipe};
pub use session::{MAX_MESSAGE, Session};

/// This build's protocol version. See the header of `remote.proto`.
pub const MAJOR: u32 = 1;
/// See [`MAJOR`].
pub const MINOR: u32 = 10;

/// The Noise pattern a phone pairs with: the pairing secret is mixed in last.
const PAIR: &str = "Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
/// The Noise pattern a paired phone reconnects with.
const RESUME: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// Marks the first frame of a handshake, and the version of its header.
const MAGIC: &[u8; 4] = b"KET1";
const MODE_PAIR: u8 = 1;
const MODE_RESUME: u8 = 2;
/// A host's refusal, sent in the clear instead of the handshake's second
/// message. Unauthenticated, so it can only ever mean "stop": nothing is
/// trusted because of it.
const MODE_REFUSED: u8 = 0xff;
/// Magic, mode, host id, offer id.
const HEADER: usize = 4 + 1 + 16 + 16;

/// The largest Noise message, and the authentication tag each carries. Fixed
/// by the Noise specification; `snow` keeps its own copies private.
pub(crate) const NOISE_MAX: usize = 65535;
/// See [`NOISE_MAX`].
pub(crate) const NOISE_TAG: usize = 16;

/// Why a session could not be had, or went wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Noise session failed: a wrong key, a wrong secret, a tampered or
    /// replayed frame.
    #[error("noise: {0}")]
    Noise(#[from] snow::Error),
    /// The pipe failed.
    #[error("pipe: {0}")]
    Pipe(#[from] std::io::Error),
    /// A message did not decode.
    #[error("decode: {0}")]
    Decode(#[from] prost::DecodeError),
    /// The pairing offer does not parse.
    #[error("not a ket pairing code: {0}")]
    BadOffer(&'static str),
    /// The pairing offer has run out, or this host never made it.
    #[error("that pairing code has expired or was already used")]
    OfferGone,
    /// The phone is not one this host has paired with.
    #[error("this device is not paired with the host")]
    Unauthorized,
    /// The host refused the handshake.
    #[error("the host refused the connection")]
    Refused,
    /// The other side said something the protocol does not allow.
    #[error("protocol: {0}")]
    Protocol(&'static str),
    /// A message larger than [`MAX_MESSAGE`].
    #[error("message of {0} bytes is too large")]
    TooLarge(usize),
}

/// A result from this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Six digits, `"123 456"`, from a handshake hash: what a person compares
/// between the phone and the desktop before approving a pairing. Both sides
/// have the hash only if they took part in the same handshake, so a phone
/// that paired with a photographed code shows digits the desktop's intended
/// phone does not — and the desktop shows the name of the phone that did.
pub fn confirmation_code(hash: &[u8]) -> String {
    let number = hash
        .iter()
        .take(4)
        .fold(0_u32, |acc, byte| (acc << 8) | u32::from(*byte))
        % 1_000_000;
    format!("{:03} {:03}", number / 1000, number % 1000)
}

/// Thirty-two bytes from the operating system's randomness: a secret, such as
/// the key a host claims its own relay with.
pub fn secret() -> [u8; 32] {
    random()
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the operating system has no randomness to give");
    bytes
}

fn builder(pattern: &'static str) -> snow::Builder<'static> {
    snow::Builder::new(
        pattern
            .parse()
            .expect("a Noise pattern ket spells correctly"),
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A static Curve25519 key pair: a host's or a phone's long-term identity.
#[derive(Clone)]
pub struct Keypair {
    private: Vec<u8>,
    /// The public half, which the other side pins.
    pub public: Vec<u8>,
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the private half, not even in a debug log.
        f.debug_struct("Keypair")
            .field("public", &URL_SAFE_NO_PAD.encode(&self.public))
            .finish_non_exhaustive()
    }
}

impl Keypair {
    /// A new, random key pair.
    pub fn generate() -> Result<Self> {
        let pair = builder(RESUME).generate_keypair()?;
        Ok(Self {
            private: pair.private,
            public: pair.public,
        })
    }

    /// A key pair read back from storage.
    pub fn from_parts(private: Vec<u8>, public: Vec<u8>) -> Result<Self> {
        if private.len() != 32 || public.len() != 32 {
            return Err(Error::Protocol("a stored key pair of the wrong size"));
        }
        Ok(Self { private, public })
    }

    /// The private half, for writing to storage and nothing else.
    pub fn private_for_storage(&self) -> &[u8] {
        &self.private
    }
}

/// What a host's QR code carries.
///
/// Everything a phone needs to find the host and pair with it once: which
/// host, its public key to pin, which relay to dial, and a one-time secret
/// that proves the phone saw this code — the relay never sees the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Offer {
    /// The host's id, which the relay routes by.
    pub host_id: [u8; 16],
    /// The host's static public key.
    pub host_public: [u8; 32],
    /// Which offer this is, so the host can find its secret.
    pub id: [u8; 16],
    /// The pre-shared key for the pairing handshake. One use.
    secret: [u8; 32],
    /// Unix seconds after which the host refuses it.
    pub expires_at: u64,
    /// The relay to dial.
    pub relay: String,
}

impl std::fmt::Debug for Offer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offer")
            .field("host_id", &URL_SAFE_NO_PAD.encode(self.host_id))
            .field("expires_at", &self.expires_at)
            .field("relay", &self.relay)
            .finish_non_exhaustive()
    }
}

/// The scheme and prefix of a pairing code.
const OFFER_PREFIX: &str = "ket://pair/";

/// Accept only a WebSocket endpoint with a host and optional numeric port.
/// Pairing codes are bearer artifacts, so a scanned code must not be able to
/// redirect the phone to a credential-bearing or unrelated URL scheme.
fn valid_relay_url(url: &str) -> bool {
    if url.is_empty()
        || url.len() > 2048
        || url
            .chars()
            .any(|c| c.is_ascii_control() || c.is_whitespace())
        || url.contains(['?', '#', '@', '\\'])
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if scheme != "ws" && scheme != "wss" {
        return false;
    }
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('%') {
        return false;
    }
    if authority.starts_with('[') {
        let Some(close) = authority.find(']') else {
            return false;
        };
        if close == 1 {
            return false;
        }
        let suffix = &authority[close + 1..];
        suffix.is_empty() || (suffix.starts_with(':') && suffix[1..].parse::<u16>().is_ok())
    } else {
        match authority.split_once(':') {
            Some((host, port)) => {
                !host.is_empty() && !port.is_empty() && port.parse::<u16>().is_ok()
            }
            None => !authority.is_empty(),
        }
    }
}

impl Offer {
    /// The text the QR code shows.
    pub fn to_code(&self) -> String {
        let relay = &self.relay.as_bytes()[..self.relay.len().min(u16::MAX as usize)];
        let mut bytes = Vec::with_capacity(1 + 16 + 32 + 16 + 32 + 8 + 2 + relay.len());
        bytes.push(1);
        bytes.extend_from_slice(&self.host_id);
        bytes.extend_from_slice(&self.host_public);
        bytes.extend_from_slice(&self.id);
        bytes.extend_from_slice(&self.secret);
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        bytes.extend_from_slice(&(relay.len() as u16).to_be_bytes());
        bytes.extend_from_slice(relay);
        format!("{OFFER_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Reads the text a QR code showed.
    pub fn from_code(code: &str) -> Result<Self> {
        let body = code
            .trim()
            .strip_prefix(OFFER_PREFIX)
            .ok_or(Error::BadOffer("not a ket://pair/ link"))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| Error::BadOffer("not base64"))?;
        let mut reader = Reader(&bytes);
        if reader.take(1)? != [1] {
            return Err(Error::BadOffer("an unknown version"));
        }
        let host_id = reader.array()?;
        let host_public = reader.array()?;
        let id = reader.array()?;
        let secret = reader.array()?;
        let expires_at = u64::from_be_bytes(reader.array()?);
        let relay_len = u16::from_be_bytes(reader.array()?) as usize;
        let relay = String::from_utf8(reader.take(relay_len)?.to_vec())
            .map_err(|_| Error::BadOffer("a relay address that is not text"))?;
        if !valid_relay_url(&relay) {
            return Err(Error::BadOffer("an unsafe relay URL"));
        }
        if !reader.0.is_empty() {
            return Err(Error::BadOffer("trailing bytes"));
        }
        Ok(Self {
            host_id,
            host_public,
            id,
            secret,
            expires_at,
            relay,
        })
    }

    fn expired(&self) -> bool {
        now() >= self.expires_at
    }
}

/// Takes fields off the front of a pairing code.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::BadOffer("truncated"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| Error::BadOffer("truncated"))
    }
}

/// The first bytes of a handshake: what the phone wants, in the clear, and
/// bound into the handshake as its prologue so none of it can be changed on
/// the way without the handshake failing.
fn header(mode: u8, host_id: &[u8; 16], offer: &[u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER);
    out.extend_from_slice(MAGIC);
    out.push(mode);
    out.extend_from_slice(host_id);
    out.extend_from_slice(offer);
    out
}

fn refusal() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.push(MODE_REFUSED);
    out
}

fn is_refusal(frame: &[u8]) -> bool {
    frame.len() == 5 && frame.starts_with(MAGIC) && frame[4] == MODE_REFUSED
}

/// Pairs a phone with a host for the first time, using a scanned [`Offer`].
///
/// The phone speaks first once this returns — its `Hello` — and the host
/// grants it only when that arrives intact.
///
/// The phone's key is sent inside the handshake, encrypted; the host grants
/// it once the handshake completes. A wrong secret — a code that was altered,
/// or guessed — fails the handshake on the phone's side, and the host has by
/// then spent the offer, so there is no second guess.
pub fn pair<P: Pipe>(device: &Keypair, offer: &Offer, mut pipe: P) -> Result<Session<P>> {
    if offer.expired() {
        return Err(Error::OfferGone);
    }
    let header = header(MODE_PAIR, &offer.host_id, &offer.id);
    let mut noise = builder(PAIR)
        .local_private_key(&device.private)?
        .remote_public_key(&offer.host_public)?
        .psk(2, &offer.secret)?
        .prologue(&header)?
        .build_initiator()?;
    initiate(&mut noise, header.clone(), &mut pipe)?;
    Ok(Session::new(pipe, noise.into_transport_mode()?))
}

/// Reconnects a paired phone to its host.
pub fn resume<P: Pipe>(
    device: &Keypair,
    host_id: &[u8; 16],
    host_public: &[u8; 32],
    mut pipe: P,
) -> Result<Session<P>> {
    let header = header(MODE_RESUME, host_id, &[0; 16]);
    let mut noise = builder(RESUME)
        .local_private_key(&device.private)?
        .remote_public_key(host_public)?
        .prologue(&header)?
        .build_initiator()?;
    initiate(&mut noise, header.clone(), &mut pipe)?;
    Ok(Session::new(pipe, noise.into_transport_mode()?))
}

/// The phone's half of either handshake: one message out, one back.
fn initiate(noise: &mut snow::HandshakeState, header: Vec<u8>, pipe: &mut impl Pipe) -> Result<()> {
    let mut frame = header;
    let mut message = vec![0u8; NOISE_MAX];
    let len = noise.write_message(&[], &mut message)?;
    frame.extend_from_slice(&message[..len]);
    pipe.send(frame)?;

    let reply = pipe.recv()?;
    if is_refusal(&reply) {
        return Err(Error::Refused);
    }
    noise.read_message(&reply, &mut message)?;
    if !noise.is_handshake_finished() {
        return Err(Error::Protocol("handshake did not finish"));
    }
    Ok(())
}

/// A device a host has paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The phone's static public key, which identifies it from now on.
    pub device: Vec<u8>,
    /// What the phone calls itself, from its first `Hello`. For a person
    /// reading the device list; never trusted for anything.
    pub name: String,
    /// Unix seconds when it paired.
    pub paired_at: u64,
}

/// A host's side of pairing and reconnecting: its identity, the offers it has
/// out, and the phones it has granted.
///
/// Held in memory for now. Where the key and the grants live between runs —
/// the Keychain, per the plan — comes with the host that serves phones.
#[derive(Debug)]
pub struct Host {
    /// The id the relay routes by.
    pub id: [u8; 16],
    keys: Keypair,
    offers: Vec<Offer>,
    grants: Vec<Grant>,
}

/// Who a handshake turned out to be from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// The phone's static public key.
    pub device: Vec<u8>,
    /// Whether this handshake was the pairing that granted it.
    pub paired_now: bool,
    /// The phone's first message — its `Hello`, by the protocol — which the
    /// host has to read before it can trust the handshake. See
    /// [`Host::accept`].
    pub first: Vec<u8>,
}

impl Host {
    /// A host with a new identity and nothing paired.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            id: random(),
            keys: Keypair::generate()?,
            offers: Vec::new(),
            grants: Vec::new(),
        })
    }

    /// A host read back from storage: its id, its key pair, and the phones
    /// it had granted. Offers are never stored — a restart spends them.
    pub fn restore(id: [u8; 16], keys: Keypair, grants: Vec<Grant>) -> Self {
        Self {
            id,
            keys,
            offers: Vec::new(),
            grants,
        }
    }

    /// The host's key pair, for writing to storage.
    pub fn keys(&self) -> &Keypair {
        &self.keys
    }

    /// Names a granted phone, from what its `Hello` said. `false` if the
    /// phone is not one this host has granted.
    pub fn name_device(&mut self, device: &[u8], name: &str) -> bool {
        match self.grants.iter_mut().find(|grant| grant.device == device) {
            Some(grant) => {
                grant.name = name.chars().filter(|c| !c.is_control()).take(64).collect();
                true
            }
            None => false,
        }
    }

    /// The host's static public key, which a phone pins.
    pub fn public(&self) -> [u8; 32] {
        self.keys
            .public
            .as_slice()
            .try_into()
            .expect("a Curve25519 public key is 32 bytes")
    }

    /// Makes a one-time pairing offer, good for `ttl`.
    pub fn offer(&mut self, relay: &str, ttl: Duration) -> Offer {
        self.offers.retain(|offer| !offer.expired());
        let offer = Offer {
            host_id: self.id,
            host_public: self.public(),
            id: random(),
            secret: random(),
            expires_at: now() + ttl.as_secs(),
            relay: relay.to_owned(),
        };
        self.offers.push(offer.clone());
        offer
    }

    /// The phones this host has paired with.
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }

    /// Stops accepting a phone. Its next reconnect is refused; a session it
    /// has open is the caller's to close.
    pub fn revoke(&mut self, device: &[u8]) -> bool {
        let before = self.grants.len();
        self.grants.retain(|grant| grant.device != device);
        self.grants.len() != before
    }

    /// Answers a phone's handshake on `pipe`: a pairing with one of this
    /// host's offers, or a reconnect from a phone it has granted.
    ///
    /// Returns once the phone's first message has arrived and decrypted,
    /// which is what proves the handshake; see [`Peer::first`]. A phone must
    /// therefore send first after [`pair`] or [`resume`] returns.
    ///
    /// Holds `self` across every wait on the phone. A host serving more than
    /// one phone uses [`Host::respond`], [`Handshake::finish`] and
    /// [`Host::admit`] instead, and holds it only for those two calls.
    pub fn accept<P: Pipe>(&mut self, mut pipe: P) -> Result<(Session<P>, Peer)> {
        let first = pipe.recv()?;
        let handshake = match self.respond(&first) {
            Ok(handshake) => handshake,
            Err(rejection) => {
                if let Some(refusal) = rejection.refusal() {
                    pipe.send(refusal)?;
                }
                return Err(rejection.error);
            }
        };
        let (device, paired_now) = (handshake.device.clone(), handshake.paired_now);
        let mut session = handshake.finish(pipe)?;
        let first = session.recv()?;
        self.admit(&device, paired_now)?;
        Ok((
            session,
            Peer {
                device,
                paired_now,
                first,
            },
        ))
    }

    /// Reads a phone's first handshake frame and answers it, touching nothing
    /// but this host's state: an offer is spent here, whether or not the
    /// handshake goes on to succeed — one guess per code — and a reconnect is
    /// checked against the grants. No waiting: the caller received `first`
    /// and sends the reply.
    pub fn respond(&mut self, first: &[u8]) -> std::result::Result<Handshake, Rejection> {
        let refuse = |error| Rejection {
            error,
            refuse: true,
        };
        let drop = |error| Rejection {
            error,
            refuse: false,
        };
        if first.len() < HEADER || !first.starts_with(MAGIC) {
            return Err(drop(Error::Protocol("not a ket handshake")));
        }
        let (header, message) = first.split_at(HEADER);
        if header[5..21] != self.id {
            return Err(refuse(Error::Protocol("a handshake for another host")));
        }
        let offer_id: [u8; 16] = header[21..37].try_into().expect("sixteen bytes");
        let mode = header[4];

        let mut noise = match mode {
            MODE_PAIR => {
                self.offers.retain(|offer| !offer.expired());
                let Some(at) = self.offers.iter().position(|offer| offer.id == offer_id) else {
                    return Err(refuse(Error::OfferGone));
                };
                let offer = self.offers.remove(at);
                let mut noise = builder(PAIR)
                    .local_private_key(&self.keys.private)
                    .and_then(|b| b.prologue(header))
                    .and_then(snow::Builder::build_responder)
                    .map_err(|e| drop(e.into()))?;
                noise
                    .set_psk(2, &offer.secret)
                    .map_err(|e| drop(e.into()))?;
                noise
            }
            MODE_RESUME => builder(RESUME)
                .local_private_key(&self.keys.private)
                .and_then(|b| b.prologue(header))
                .and_then(snow::Builder::build_responder)
                .map_err(|e| drop(e.into()))?,
            _ => return Err(refuse(Error::Protocol("an unknown handshake mode"))),
        };

        let mut buffer = vec![0u8; NOISE_MAX];
        noise
            .read_message(message, &mut buffer)
            .map_err(|e| drop(e.into()))?;
        let device = noise
            .get_remote_static()
            .ok_or_else(|| drop(Error::Protocol("no device key in the handshake")))?
            .to_vec();

        let paired_now = mode == MODE_PAIR;
        if !paired_now && !self.grants.iter().any(|grant| grant.device == device) {
            return Err(refuse(Error::Unauthorized));
        }

        let len = noise
            .write_message(&[], &mut buffer)
            .map_err(|e| drop(e.into()))?;
        buffer.truncate(len);
        let hash = noise.get_handshake_hash().to_vec();
        Ok(Handshake {
            noise,
            reply: buffer,
            device,
            paired_now,
            hash,
        })
    }

    /// Lets a phone in once its first message has decrypted — the proof the
    /// handshake needs; see [`Handshake::finish`]. A pairing is granted
    /// here. A reconnect is checked again: the phone may have been revoked
    /// while its handshake was in flight, and must not get in on a grant that
    /// is gone. A caller that registers the session somewhere does so under
    /// the same lock as this, so a revocation cannot fall in between.
    pub fn admit(&mut self, device: &[u8], paired_now: bool) -> Result<()> {
        let granted = self.grants.iter().any(|grant| grant.device == device);
        if paired_now && !granted {
            self.grants.push(Grant {
                device: device.to_vec(),
                name: String::new(),
                paired_at: now(),
            });
            return Ok(());
        }
        if granted {
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    }
}

/// A handshake the host has answered, not yet proven: see [`Host::respond`].
pub struct Handshake {
    noise: snow::HandshakeState,
    reply: Vec<u8>,
    /// The phone's static public key.
    pub device: Vec<u8>,
    /// Whether this is a pairing, rather than a reconnect.
    pub paired_now: bool,
    /// The handshake hash, which the phone has too — see
    /// [`confirmation_code`].
    pub hash: Vec<u8>,
}

impl Handshake {
    /// Sends the host's reply and turns to the session. Nothing is granted
    /// yet: in `IKpsk2` the pairing secret is mixed in with this reply, so
    /// until the phone has sent something under the keys that came out of
    /// it, the host has no evidence the phone knew the secret at all. The
    /// caller receives that first message, then [`Host::admit`]s the phone.
    pub fn finish<P: Pipe>(self, mut pipe: P) -> Result<Session<P>> {
        pipe.send(self.reply)?;
        Ok(Session::new(pipe, self.noise.into_transport_mode()?))
    }
}

/// Why [`Host::respond`] turned a handshake away, and whether the phone is
/// owed a refusal — rather than silence, for something that was not a ket
/// handshake at all.
#[derive(Debug)]
pub struct Rejection {
    /// What was wrong.
    pub error: Error,
    refuse: bool,
}

impl Rejection {
    /// The frame to send back, if one is owed.
    pub fn refusal(&self) -> Option<Vec<u8>> {
        self.refuse.then(refusal)
    }
}

/// An envelope from this build, around `payload`.
pub fn envelope(request_id: u64, payload: proto::envelope::Payload) -> proto::Envelope {
    proto::Envelope {
        major: MAJOR,
        minor: MINOR,
        request_id,
        payload: Some(payload),
        ..Default::default()
    }
}

/// Whether an envelope from the other side can be understood.
///
/// A different minor version is fine — proto3 skips fields it does not know.
/// A different major is not, and the answer is the error to send back before
/// anything else.
pub fn check_version(envelope: &proto::Envelope) -> std::result::Result<(), proto::Error> {
    if envelope.major == MAJOR {
        return Ok(());
    }
    Err(proto::Error {
        code: proto::ErrorCode::UnsupportedVersion as i32,
        message: format!(
            "this side speaks protocol {MAJOR}.{MINOR}; the other speaks {}.{} — update whichever is older",
            envelope.major, envelope.minor
        ),
    })
}

impl<P: Pipe> Session<P> {
    /// Sends one envelope.
    pub fn send_envelope(&mut self, envelope: &proto::Envelope) -> Result<()> {
        self.send(&envelope.encode_to_vec())
    }

    /// Receives one envelope.
    pub fn recv_envelope(&mut self) -> Result<proto::Envelope> {
        Ok(proto::Envelope::decode(self.recv()?.as_slice())?)
    }
}
