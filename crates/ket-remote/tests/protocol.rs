//! The phone protocol's guarantees, checked end to end over an in-memory pipe:
//! who can pair, who can reconnect, what a tampered, replayed or reordered
//! frame does, and that no input — however malformed — makes either side
//! panic.
//!
//! These test the protocol as ket uses it; they are not an independent
//! security review.

use std::thread;
use std::time::Duration;

use ket_remote::{
    Error, Host, Keypair, MAJOR, MAX_MESSAGE, MemoryPipe, Offer, Peer, Pipe, Session,
    check_version, memory_pipe, pair, proto, resume,
};

const RELAY: &str = "ws://relay.test:7979";
const TTL: Duration = Duration::from_secs(600);

/// What the phone sends first once its handshake returns — its `Hello`, as
/// far as the host is concerned: the host grants nothing until it decrypts.
const HELLO: &[u8] = b"hello";

/// A small deterministic generator, so a fuzz case that fails fails again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max + 1);
        (0..len).map(|_| self.next() as u8).collect()
    }
}

/// Runs the host's `accept` on its own thread against whatever the phone does
/// with its end, and hands back the host and both outcomes.
fn meet<T: Send + 'static>(
    mut host: Host,
    phone: impl FnOnce(MemoryPipe) -> T + Send + 'static,
) -> (Host, ket_remote::Result<(Session<MemoryPipe>, Peer)>, T) {
    let (host_end, phone_end) = memory_pipe();
    let phone = thread::spawn(move || phone(phone_end));
    let accepted = host.accept(host_end);
    let phone = phone.join().expect("the phone's side does not panic");
    (host, accepted, phone)
}

/// Pairs `device` using `offer`, sending the first message on success.
fn pair_phone(
    device: Keypair,
    offer: Offer,
) -> impl FnOnce(MemoryPipe) -> ket_remote::Result<Session<MemoryPipe>> {
    move |pipe| {
        let mut session = pair(&device, &offer, pipe)?;
        session.send(HELLO)?;
        Ok(session)
    }
}

/// Reconnects `device` to a host it pinned, sending the first message.
fn resume_phone(
    device: Keypair,
    host_id: [u8; 16],
    host_public: [u8; 32],
) -> impl FnOnce(MemoryPipe) -> ket_remote::Result<Session<MemoryPipe>> {
    move |pipe| {
        let mut session = resume(&device, &host_id, &host_public, pipe)?;
        session.send(HELLO)?;
        Ok(session)
    }
}

/// Where a code's secret starts: after the version, host id, host key and
/// offer id.
const SECRET_AT: usize = 1 + 16 + 32 + 16;

/// `offer`, as its code would read with one bit changed at byte `at`.
fn tampered(offer: &Offer, at: usize) -> Offer {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let code = offer.to_code();
    let mut bytes = URL_SAFE_NO_PAD
        .decode(code.strip_prefix("ket://pair/").unwrap())
        .unwrap();
    bytes[at] ^= 1;
    Offer::from_code(&format!("ket://pair/{}", URL_SAFE_NO_PAD.encode(&bytes))).unwrap()
}

fn host() -> Host {
    Host::generate().expect("a host identity")
}

fn phone() -> Keypair {
    Keypair::generate().expect("a phone identity")
}

/// A host with `device` already paired to it.
fn paired(device: &Keypair) -> Host {
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let (host, accepted, phone) = meet(host, pair_phone(device.clone(), offer));
    accepted.expect("the host accepts the pairing");
    phone.expect("the phone pairs");
    host
}

// -- pairing ----------------------------------------------------------------

#[test]
fn a_phone_pairs_then_reconnects_and_both_sides_know_each_other() {
    let device = phone();
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let (host_id, host_public) = (host.id, host.public());

    let (host, accepted, phone) = meet(host, pair_phone(device.clone(), offer));
    let (mut host_session, peer) = accepted.expect("paired");
    let mut phone_session = phone.expect("paired");
    assert!(peer.paired_now);
    assert_eq!(peer.device, device.public);
    assert_eq!(peer.first, HELLO);
    assert_eq!(host.grants().len(), 1);
    assert_eq!(
        phone_session.remote_static().as_deref(),
        Some(&host_public[..])
    );

    // Both directions carry messages after the handshake.
    host_session.send(b"welcome").unwrap();
    assert_eq!(phone_session.recv().unwrap(), b"welcome");

    let (host, accepted, phone) = meet(host, resume_phone(device.clone(), host_id, host_public));
    let (_, peer) = accepted.expect("a paired phone reconnects");
    phone.expect("the phone reconnects");
    assert!(!peer.paired_now);
    assert_eq!(peer.device, device.public);
    assert_eq!(host.grants().len(), 1, "reconnecting grants nothing new");
}

#[test]
fn a_pairing_code_pairs_once() {
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let (host, accepted, phone) = meet(host, pair_phone(self::phone(), offer.clone()));
    accepted.unwrap();
    phone.unwrap();

    let (host, accepted, phone) = meet(host, pair_phone(self::phone(), offer));
    assert!(matches!(accepted, Err(Error::OfferGone)));
    assert!(matches!(phone, Err(Error::Refused)));
    assert_eq!(host.grants().len(), 1);
}

#[test]
fn a_wrong_secret_grants_nothing_and_spends_the_code() {
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let guessed = tampered(&offer, SECRET_AT);

    let (host, accepted, phone) = meet(host, pair_phone(self::phone(), guessed));
    assert!(
        phone.is_err(),
        "the phone cannot finish with the wrong secret"
    );
    assert!(
        accepted.is_err(),
        "the host never hears a message it can decrypt"
    );
    assert!(host.grants().is_empty());

    // One guess per code: the real secret is no good after a wrong one.
    let (host, accepted, phone) = meet(host, pair_phone(self::phone(), offer));
    assert!(matches!(accepted, Err(Error::OfferGone)));
    assert!(matches!(phone, Err(Error::Refused)));
    assert!(host.grants().is_empty());
}

#[test]
fn an_expired_code_is_refused_by_both_sides() {
    let mut host = host();
    let offer = host.offer(RELAY, Duration::ZERO);

    // The phone checks before it dials…
    let (host_end, phone_end) = memory_pipe();
    assert!(matches!(
        pair(&phone(), &offer, phone_end),
        Err(Error::OfferGone)
    ));
    drop(host_end);

    // …and the host refuses one that arrives anyway: a phone with its clock
    // behind gets no second chance.
    let mut late = offer.clone();
    late.expires_at = u64::MAX;
    let (host, accepted, phone) = meet(host, pair_phone(self::phone(), late));
    assert!(matches!(accepted, Err(Error::OfferGone)));
    assert!(matches!(phone, Err(Error::Refused)));
    assert!(host.grants().is_empty());
}

#[test]
fn a_code_for_another_host_is_refused() {
    let mut elsewhere = host();
    let offer = elsewhere.offer(RELAY, TTL);
    let (host, accepted, phone) = meet(host(), pair_phone(self::phone(), offer));
    assert!(matches!(accepted, Err(Error::Protocol(_))));
    assert!(matches!(phone, Err(Error::Refused)));
    assert!(host.grants().is_empty());
}

#[test]
fn a_code_with_another_hosts_key_pairs_nothing() {
    // A code whose host id is right but whose key is not: someone relaying a
    // code for a host they do not control. The phone encrypts to the key in
    // the code, which the real host cannot decrypt.
    let mut host = host();
    let mut offer = host.offer(RELAY, TTL);
    offer.host_public = self::host().public();
    let (host, accepted, phone) = meet(host, pair_phone(self::phone(), offer));
    assert!(accepted.is_err());
    assert!(phone.is_err());
    assert!(host.grants().is_empty());
}

// -- reconnecting -----------------------------------------------------------

#[test]
fn a_phone_that_never_paired_is_refused() {
    let host = host();
    let (host_id, host_public) = (host.id, host.public());
    let (host, accepted, phone) = meet(host, resume_phone(self::phone(), host_id, host_public));
    assert!(matches!(accepted, Err(Error::Unauthorized)));
    assert!(matches!(phone, Err(Error::Refused)));
    assert!(host.grants().is_empty());
}

#[test]
fn a_revoked_phone_is_refused() {
    let device = phone();
    let mut host = paired(&device);
    assert!(host.revoke(&device.public));
    assert!(!host.revoke(&device.public), "revoking twice finds nothing");
    let (host_id, host_public) = (host.id, host.public());
    let (_, accepted, phone) = meet(host, resume_phone(device, host_id, host_public));
    assert!(matches!(accepted, Err(Error::Unauthorized)));
    assert!(matches!(phone, Err(Error::Refused)));
}

#[test]
fn an_impostor_host_cannot_read_a_paired_phones_handshake() {
    // The phone pins its host's key. Something else answering for the same
    // host id — the relay, or anyone who registered the id — cannot decrypt
    // the phone's first message, and the phone never completes.
    let device = phone();
    let real = paired(&device);
    let mut impostor = host();
    impostor.id = real.id;
    let (_, accepted, phone) = meet(impostor, resume_phone(device, real.id, real.public()));
    assert!(accepted.is_err());
    assert!(phone.is_err());
}

#[test]
fn a_granted_phone_survives_the_host_restarting() {
    let device = phone();
    let host = paired(&device);
    let restored = Host::restore(host.id, host.keys().clone(), host.grants().to_vec());
    let (host_id, host_public) = (restored.id, restored.public());
    let (_, accepted, phone) = meet(restored, resume_phone(device, host_id, host_public));
    accepted.expect("a restored host still knows the phone");
    phone.unwrap();
}

#[test]
fn offers_do_not_survive_the_host_restarting() {
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let restored = Host::restore(host.id, host.keys().clone(), Vec::new());
    let (_, accepted, phone) = meet(restored, pair_phone(self::phone(), offer));
    assert!(matches!(accepted, Err(Error::OfferGone)));
    assert!(matches!(phone, Err(Error::Refused)));
}

// -- the session ------------------------------------------------------------

/// A paired session's two ends, with the host's first message consumed.
fn sessions() -> (Session<MemoryPipe>, Session<MemoryPipe>) {
    let device = phone();
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let (_, accepted, phone) = meet(host, pair_phone(device, offer));
    (accepted.unwrap().0, phone.unwrap())
}

#[test]
fn a_replayed_frame_ends_the_session() {
    let (mut host, mut phone) = sessions();
    let frame = phone.seal(b"type ls").unwrap().remove(0);
    assert_eq!(host.open(&frame).unwrap().as_deref(), Some(&b"type ls"[..]));
    assert!(host.open(&frame).is_err(), "the same frame a second time");
}

#[test]
fn reordered_frames_end_the_session() {
    let (mut host, mut phone) = sessions();
    let first = phone.seal(b"one").unwrap().remove(0);
    let second = phone.seal(b"two").unwrap().remove(0);
    assert!(host.open(&second).is_err());
    let _ = first;
}

#[test]
fn a_dropped_frame_ends_the_session() {
    let (mut host, mut phone) = sessions();
    let _dropped = phone.seal(b"one").unwrap();
    let next = phone.seal(b"two").unwrap().remove(0);
    assert!(host.open(&next).is_err());
}

#[test]
fn a_flipped_bit_anywhere_in_a_frame_is_caught() {
    let (_, mut phone) = sessions();
    let frame = phone.seal(b"allow once").unwrap().remove(0);
    for at in 0..frame.len() {
        for bit in [0x01u8, 0x80] {
            // A fresh session each time: a failed open may leave one unusable.
            let (mut host, mut phone) = sessions();
            let mut tampered = phone.seal(b"allow once").unwrap().remove(0);
            tampered[at] ^= bit;
            assert!(host.open(&tampered).is_err(), "byte {at} bit {bit:#x}");
        }
    }
}

#[test]
fn a_truncated_or_extended_frame_is_caught() {
    let (mut host, mut phone) = sessions();
    let frame = phone.seal(b"interrupt").unwrap().remove(0);
    assert!(host.open(&frame[..frame.len() - 1]).is_err());
    let (mut host, mut phone) = sessions();
    let mut longer = phone.seal(b"interrupt").unwrap().remove(0);
    longer.push(0);
    assert!(host.open(&longer).is_err());
    assert!(host.open(&[]).is_err());
}

#[test]
fn large_and_empty_messages_survive_the_trip() {
    let (mut host, mut phone) = sessions();
    let mut rng = Rng(7);
    // Several frames' worth, with a ragged last frame.
    let large: Vec<u8> = (0..5 * 1024 * 1024 + 123)
        .map(|_| rng.next() as u8)
        .collect();
    phone.send(&large).unwrap();
    assert_eq!(host.recv().unwrap(), large);
    phone.send(&[]).unwrap();
    assert_eq!(host.recv().unwrap(), Vec::<u8>::new());
    host.send(b"after").unwrap();
    assert_eq!(phone.recv().unwrap(), b"after");
}

#[test]
fn a_message_over_the_limit_is_refused_before_it_is_sent() {
    let (_, mut phone) = sessions();
    let too_large = vec![0u8; MAX_MESSAGE + 1];
    assert!(matches!(phone.seal(&too_large), Err(Error::TooLarge(_))));
}

// -- pairing codes ----------------------------------------------------------

#[test]
fn a_pairing_code_round_trips() {
    let mut host = host();
    let offer = host.offer("ws://Example-MacBook.local:7979", TTL);
    let back = Offer::from_code(&offer.to_code()).unwrap();
    assert_eq!(back.host_id, offer.host_id);
    assert_eq!(back.host_public, offer.host_public);
    assert_eq!(back.id, offer.id);
    // The secret is private to the crate; the code carrying it back intact
    // is the same check.
    assert_eq!(back.to_code(), offer.to_code());
    assert_eq!(back.expires_at, offer.expires_at);
    assert_eq!(back.relay, offer.relay);
    // Surrounding whitespace, as a paste brings, is forgiven.
    assert!(Offer::from_code(&format!("  {}\n", offer.to_code())).is_ok());
}

#[test]
fn every_truncation_of_a_code_is_refused() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let code = host().offer(RELAY, TTL).to_code();
    let bytes = URL_SAFE_NO_PAD
        .decode(code.strip_prefix("ket://pair/").unwrap())
        .unwrap();
    for len in 0..bytes.len() {
        let cut = format!("ket://pair/{}", URL_SAFE_NO_PAD.encode(&bytes[..len]));
        assert!(
            matches!(Offer::from_code(&cut), Err(Error::BadOffer(_))),
            "{len} of {} bytes",
            bytes.len()
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    let longer = format!("ket://pair/{}", URL_SAFE_NO_PAD.encode(&longer));
    assert!(matches!(Offer::from_code(&longer), Err(Error::BadOffer(_))));
}

#[test]
fn malformed_codes_are_refused_with_a_reason() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let code = host().offer(RELAY, TTL).to_code();
    let mut bytes = URL_SAFE_NO_PAD
        .decode(code.strip_prefix("ket://pair/").unwrap())
        .unwrap();
    let encode = |bytes: &[u8]| format!("ket://pair/{}", URL_SAFE_NO_PAD.encode(bytes));

    for bad in [
        "",
        "ket://pair/",
        "https://example.com",
        "ket://pair/!!!",
        "KET://PAIR/AAAA",
    ] {
        assert!(
            matches!(Offer::from_code(bad), Err(Error::BadOffer(_))),
            "{bad:?}"
        );
    }

    let mut version = bytes.clone();
    version[0] = 2;
    assert!(matches!(
        Offer::from_code(&encode(&version)),
        Err(Error::BadOffer(_))
    ));

    // The relay address: not text, and a length past the end.
    let relay_at = bytes.len() - RELAY.len();
    bytes[relay_at] = 0xff;
    assert!(matches!(
        Offer::from_code(&encode(&bytes)),
        Err(Error::BadOffer(_))
    ));
    let mut long = URL_SAFE_NO_PAD
        .decode(code.strip_prefix("ket://pair/").unwrap())
        .unwrap();
    long[relay_at - 2..relay_at].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(matches!(
        Offer::from_code(&encode(&long)),
        Err(Error::BadOffer(_))
    ));
}

#[test]
fn random_codes_never_panic() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut rng = Rng(0x6b65_7421);
    for _ in 0..20_000 {
        let bytes = rng.bytes(200);
        let _ = Offer::from_code(&format!("ket://pair/{}", URL_SAFE_NO_PAD.encode(&bytes)));
        let _ = Offer::from_code(&String::from_utf8_lossy(&bytes));
    }
}

// -- handshakes nobody should send ------------------------------------------

#[test]
fn random_first_messages_never_panic_the_host_or_grant_anything() {
    let mut rng = Rng(0x5eed);
    let mut host = host();
    let _offer = host.offer(RELAY, TTL);
    let id = host.id;
    for round in 0..2_000 {
        let mut frame = rng.bytes(300);
        // Most rounds look like a real handshake's opening, so the fuzz gets
        // past the first checks: the magic, this host's id, a mode.
        if round % 4 != 0 && frame.len() >= 37 {
            frame[..4].copy_from_slice(b"KET1");
            frame[4] = [1, 2, 0xff, rng.next() as u8][round % 4];
            frame[5..21].copy_from_slice(&id);
        }
        let (mut host_end, mut phone_end) = memory_pipe();
        phone_end.send(frame).unwrap();
        drop(phone_end);
        assert!(host.accept(&mut host_end).is_err(), "round {round}");
    }
    assert!(host.grants().is_empty());
}

#[test]
fn a_host_that_answers_nonsense_is_not_trusted_by_the_phone() {
    let mut host = host();
    let offer = host.offer(RELAY, TTL);
    let mut rng = Rng(99);
    for round in 0..500 {
        let (mut host_end, phone_end) = memory_pipe();
        let offer = offer.clone();
        let phone = thread::spawn(move || pair(&phone(), &offer, phone_end).map(|_| ()));
        let _hello = host_end.recv().unwrap();
        host_end.send(rng.bytes(120)).unwrap();
        assert!(phone.join().unwrap().is_err(), "round {round}");
    }
}

// -- versions ---------------------------------------------------------------

#[test]
fn a_different_minor_version_is_understood_and_a_different_major_is_not() {
    let mut envelope = ket_remote::envelope(1, proto::envelope::Payload::Ack(proto::Ack {}));
    envelope.minor += 7;
    assert!(check_version(&envelope).is_ok());
    envelope.major = MAJOR + 1;
    let refusal = check_version(&envelope).unwrap_err();
    assert_eq!(refusal.code, proto::ErrorCode::UnsupportedVersion as i32);
}
