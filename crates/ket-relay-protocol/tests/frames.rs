//! The relay's outer frame: every input either decodes to what was encoded
//! or is refused with a reason — never a panic, never a frame larger than
//! the relay will hold. The phone protocol's own tests are in
//! `ket-remote/tests/protocol.rs`.

use ket_relay_protocol::{Control, Error, Frame, HEADER, Kind, MAX_FRAME, VERSION};

/// A small deterministic generator, so a fuzz case that fails fails again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
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

#[test]
fn frames_round_trip() {
    for (kind, body) in [
        (Kind::Data, Vec::new()),
        (Kind::Data, b"ciphertext".to_vec()),
        (Kind::Control, vec![3]),
        (Kind::Data, vec![0xab; Frame::MAX_BODY]),
    ] {
        let frame = Frame {
            connection: u64::MAX - 1,
            kind,
            body,
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(bytes.len(), HEADER + frame.body.len());
        assert_eq!(Frame::decode(&bytes).unwrap(), frame);
    }
}

#[test]
fn a_frame_past_the_limit_is_neither_encoded_nor_decoded() {
    let too_big = Frame::data(1, vec![0; Frame::MAX_BODY + 1]);
    assert!(matches!(too_big.encode(), Err(Error::Size(_))));

    let mut bytes = Frame::data(1, vec![0; Frame::MAX_BODY]).encode().unwrap();
    bytes.push(0);
    assert_eq!(bytes.len(), MAX_FRAME + 1);
    assert!(matches!(Frame::decode(&bytes), Err(Error::Size(_))));
}

#[test]
fn a_length_that_disagrees_with_the_body_is_refused() {
    let bytes = Frame::data(9, b"twelve bytes".to_vec()).encode().unwrap();
    // Every truncation, header included.
    for len in 0..bytes.len() {
        assert!(Frame::decode(&bytes[..len]).is_err(), "{len} bytes");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(matches!(Frame::decode(&longer), Err(Error::Size(_))));

    // A header claiming more than the relay would ever hold.
    let mut lying = bytes;
    lying[10..14].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(Frame::decode(&lying), Err(Error::Short)));
}

#[test]
fn an_unknown_version_or_kind_is_refused() {
    let mut bytes = Frame::data(1, vec![1, 2, 3]).encode().unwrap();
    bytes[0] = VERSION + 1;
    assert!(matches!(Frame::decode(&bytes), Err(Error::Version(_))));
    bytes[0] = VERSION;
    for kind in [0u8, 3, 0xff] {
        bytes[1] = kind;
        assert!(matches!(Frame::decode(&bytes), Err(Error::Kind(k)) if k == kind));
    }
}

#[test]
fn every_control_round_trips() {
    let host = [7u8; 16];
    for control in [
        Control::Register { host },
        Control::Connect { host },
        Control::Opened,
        Control::Connected,
        Control::Closed,
        Control::Refused { reason: 2 },
    ] {
        let frame = control.frame(42);
        assert_eq!(frame.kind, Kind::Control);
        assert_eq!(frame.connection, 42);
        let back = Frame::decode(&frame.encode().unwrap()).unwrap();
        assert_eq!(Control::parse(&back.body).unwrap(), control);
    }
}

#[test]
fn a_control_with_the_wrong_shape_is_refused() {
    assert!(matches!(Control::parse(&[]), Err(Error::Short)));
    // A host id one byte short, and one byte long.
    assert!(Control::parse(&[1; 16]).is_err());
    assert!(Control::parse(&[2; 18]).is_err());
    // A bare op that takes nothing, with something after it.
    for op in [3u8, 4, 5] {
        assert!(Control::parse(&[op, 0]).is_err(), "op {op}");
    }
    assert!(Control::parse(&[6]).is_err());
    assert!(Control::parse(&[6, 1, 1]).is_err());
    assert!(matches!(Control::parse(&[0xee]), Err(Error::Kind(0xee))));
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0xf00d);
    for round in 0..50_000 {
        let mut bytes = rng.bytes(64);
        // Half the rounds carry a valid version and kind, so the fuzz reaches
        // the length checks rather than stopping at the first byte.
        if round % 2 == 0 && bytes.len() >= 2 {
            bytes[0] = VERSION;
            bytes[1] = 1 + (round as u8 % 2);
        }
        if let Ok(frame) = Frame::decode(&bytes) {
            assert!(bytes.len() <= MAX_FRAME);
            assert_eq!(frame.encode().unwrap(), bytes, "decoding is exact");
            let _ = Control::parse(&frame.body);
        }
        let _ = Control::parse(&bytes);
    }
}
