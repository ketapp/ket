//! Exercises the phone protocol end to end over an in-memory pipe (Epic
//! 5b.2): pairing from a QR code, reconnecting, a message too large for one
//! Noise frame, and every way it is meant to refuse — a spent code, an
//! expired one, an altered one, an unknown or revoked phone, a replayed or
//! tampered frame, an incompatible version, an oversized relay frame.
//!
//! ```sh
//! cargo run -p ket-remote --example pair
//! ```

use std::thread;
use std::time::Duration;

use ket_relay_protocol::{Frame, MAX_FRAME};
use ket_remote::proto::envelope::Payload;
use ket_remote::proto::{self, TerminalCheckpoint};
use ket_remote::{Error, Host, Keypair, MemoryPipe, Offer, envelope, memory_pipe};

fn main() {
    let mut failures = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!(
            "{} {name}{}",
            if ok { "ok  " } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        );
        if !ok {
            failures += 1;
        }
    };

    let host = Host::generate().expect("host keys");
    let phone = Keypair::generate().expect("phone keys");
    let relay = "wss://relay.example/";

    // Pair from the text a QR code would show.
    let mut host = host;
    let code = host.offer(relay, Duration::from_secs(120)).to_code();
    let offer = Offer::from_code(&code).expect("the code parses");
    check(
        "the pairing code round-trips",
        offer.relay == relay,
        format!("{} chars", code.len()),
    );

    let (host, result) = handshake(host, |pipe| ket_remote::pair(&phone, &offer, pipe));
    let (mut host, mut phone_session, mut host_session) = match result {
        Ok((phone_session, host_session)) => (host, phone_session, host_session),
        Err(error) => {
            check("pairing", false, error);
            std::process::exit(1);
        }
    };
    check(
        "pairing grants the phone",
        host.grants().iter().any(|g| g.device == phone.public),
        String::new(),
    );

    // The host answers the hello the handshake carried.
    host_session
        .send_envelope(&envelope(
            1,
            Payload::Welcome(proto::Welcome {
                host_name: "spike host".into(),
                host_epoch: 1,
                resumed: false,
            }),
        ))
        .unwrap();
    let welcome = phone_session.recv_envelope().unwrap();
    check(
        "the welcome arrives",
        matches!(&welcome.payload, Some(Payload::Welcome(w)) if w.host_name == "spike host"),
        String::new(),
    );

    // Larger than any one Noise frame: split, and put back together.
    let ansi: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let big = envelope(
        2,
        Payload::Checkpoint(TerminalCheckpoint {
            terminal_id: 7,
            cols: 120,
            rows: 40,
            ansi: ansi.clone(),
            ..Default::default()
        }),
    );
    host_session.send_envelope(&big).unwrap();
    let got = phone_session.recv_envelope().unwrap();
    check(
        "a 3 MiB checkpoint arrives whole",
        matches!(&got.payload, Some(Payload::Checkpoint(c)) if c.ansi == ansi),
        "split into ~49 frames".into(),
    );

    // A frame sent again is refused: its nonce has been used. Intercepted
    // raw, delivered once — it decrypts — and then delivered again.
    phone_session.send(b"first").unwrap();
    match last_frame(&mut host_session) {
        Some(raw) => {
            phone_session.pipe_mut_send(raw.clone());
            let delivered = host_session.recv();
            check(
                "an intercepted frame decrypts once",
                matches!(&delivered, Ok(m) if m == b"first"),
                describe(delivered.err()),
            );
            phone_session.pipe_mut_send(raw);
            let replayed = host_session.recv();
            check(
                "the same frame again is refused",
                replayed.is_err(),
                describe(replayed.err()),
            );
        }
        None => check("intercepting a frame", false, String::new()),
    }
    drop(phone_session);
    drop(host_session);

    // Reconnect as the paired phone.
    let host_id = host.id;
    let host_public = host.public();
    let (next, result) = handshake(host, |pipe| {
        ket_remote::resume(&phone, &host_id, &host_public, pipe)
    });
    host = next;
    check(
        "the paired phone reconnects",
        result.is_ok(),
        describe_str(result.err()),
    );

    // A phone the host never paired with.
    let stranger = Keypair::generate().unwrap();
    let (next, result) = handshake(host, |pipe| {
        ket_remote::resume(&stranger, &host_id, &host_public, pipe)
    });
    host = next;
    check(
        "an unknown phone is refused",
        result.is_err(),
        describe_str(result.err()),
    );

    // The same code a second time.
    let (next, result) = handshake(host, |pipe| ket_remote::pair(&stranger, &offer, pipe));
    host = next;
    check(
        "a spent code is refused",
        result.is_err(),
        describe_str(result.err()),
    );

    // An altered code: the same offer, one bit of its secret flipped. The
    // secret starts after the version, host id, host key and offer id.
    let fresh = host.offer(relay, Duration::from_secs(120));
    let altered = {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let code = fresh.to_code();
        let body = code.strip_prefix("ket://pair/").unwrap();
        let mut bytes = URL_SAFE_NO_PAD.decode(body).unwrap();
        bytes[1 + 16 + 32 + 16] ^= 1;
        Offer::from_code(&format!("ket://pair/{}", URL_SAFE_NO_PAD.encode(bytes))).unwrap()
    };
    let (next, result) = handshake(host, |pipe| ket_remote::pair(&stranger, &altered, pipe));
    host = next;
    check(
        "an altered secret fails the handshake",
        result.is_err(),
        describe_str(result.err()),
    );
    // And the code is spent: the right secret, a moment later, is too late.
    let (next, result) = handshake(host, |pipe| ket_remote::pair(&stranger, &fresh, pipe));
    host = next;
    check(
        "the code is spent after one wrong guess",
        result.is_err(),
        describe_str(result.err()),
    );
    check(
        "no phone that guessed was granted",
        !host.grants().iter().any(|g| g.device == stranger.public),
        String::new(),
    );

    // An expired code.
    let expired = host.offer(relay, Duration::ZERO);
    let result = ket_remote::pair(&stranger, &expired, memory_pipe().0);
    check(
        "an expired code is refused before dialling",
        matches!(result, Err(Error::OfferGone)),
        describe(result.err()),
    );

    // Revoking the paired phone.
    host.revoke(&phone.public);
    let (_, result) = handshake(host, |pipe| {
        ket_remote::resume(&phone, &host_id, &host_public, pipe)
    });
    check(
        "a revoked phone is refused",
        result.is_err(),
        describe_str(result.err()),
    );

    // Versions: a newer minor is fine, another major is not.
    let mut newer = envelope(1, Payload::Ack(proto::Ack {}));
    newer.minor = 9;
    check(
        "a newer minor version is accepted",
        ket_remote::check_version(&newer).is_ok(),
        String::new(),
    );
    newer.major = 2;
    let refused = ket_remote::check_version(&newer);
    check(
        "another major version is refused",
        matches!(&refused, Err(e) if e.code == proto::ErrorCode::UnsupportedVersion as i32),
        refused.err().map(|e| e.message).unwrap_or_default(),
    );

    // The relay's frame: round-trips, and refuses to grow past 1 MiB.
    let frame = Frame::data(42, vec![7; 1000]);
    let decoded = Frame::decode(&frame.encode().unwrap());
    check(
        "a relay frame round-trips",
        decoded.as_ref() == Ok(&frame),
        String::new(),
    );
    let too_big = Frame::data(42, vec![0; MAX_FRAME]).encode();
    check(
        "an oversized relay frame is refused",
        too_big.is_err(),
        format!("{too_big:?}").chars().take(40).collect(),
    );

    println!();
    if failures == 0 {
        println!("all checks passed");
    } else {
        println!("{failures} check(s) failed");
        std::process::exit(1);
    }
}

type Pair = (
    ket_remote::Session<MemoryPipe>,
    ket_remote::Session<MemoryPipe>,
);

fn hello() -> ket_remote::proto::Envelope {
    envelope(
        1,
        Payload::Hello(proto::Hello {
            device_name: "spike phone".into(),
            app_version: "0".into(),
            ..Default::default()
        }),
    )
}

/// Runs the host's `accept` on one end of a pipe and `phone` on the other.
fn handshake(
    mut host: Host,
    phone: impl FnOnce(MemoryPipe) -> ket_remote::Result<ket_remote::Session<MemoryPipe>>,
) -> (Host, Result<Pair, String>) {
    let (phone_end, host_end) = memory_pipe();
    let accepting = thread::spawn(move || {
        let result = host.accept(host_end);
        (host, result)
    });
    // The phone speaks first after the handshake: its hello is what the host
    // needs before it trusts the handshake at all.
    let phone_result = phone(phone_end).and_then(|mut session| {
        session.send_envelope(&hello())?;
        Ok(session)
    });
    let (host, host_result) = accepting.join().expect("the host thread");
    let result = match (phone_result, host_result) {
        (Ok(phone), Ok((host_session, _))) => Ok((phone, host_session)),
        (Err(phone), Err(host)) => Err(format!("phone: {phone}; host: {host}")),
        (Err(phone), Ok(_)) => Err(format!("phone: {phone}; host thought it succeeded")),
        (Ok(_), Err(host)) => Err(format!("host: {host}; phone thought it succeeded")),
    };
    (host, result)
}

/// Takes the next frame off the pipe without decrypting it, so it can be
/// sent again.
fn last_frame(session: &mut ket_remote::Session<MemoryPipe>) -> Option<Vec<u8>> {
    use ket_remote::Pipe;
    session.pipe_mut().recv().ok()
}

trait Inject {
    fn pipe_mut_send(&mut self, frame: Vec<u8>);
}

impl Inject for ket_remote::Session<MemoryPipe> {
    fn pipe_mut_send(&mut self, frame: Vec<u8>) {
        use ket_remote::Pipe;
        let _ = self.pipe_mut().send(frame);
    }
}

fn describe(error: Option<Error>) -> String {
    error.map(|e| e.to_string()).unwrap_or_default()
}

fn describe_str(error: Option<String>) -> String {
    error.unwrap_or_default()
}
