//! `host::wire` — the length-prefixed frame protocol between a ket window
//! and its local host process.

use std::io::Cursor;

use ket_core::activity::Foreground;
use ket_core::host::wire::{self, Frame, PROTOCOL, Request};
use ket_core::terminal::TerminalSize;

#[test]
fn a_json_frame_round_trips() {
    let request = Request::Hello {
        protocol: PROTOCOL,
        build: "abc123".to_owned(),
    };
    let encoded = wire::json(&request).unwrap();

    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    let Frame::Json(body) = frame else {
        panic!("expected a json frame");
    };
    let decoded: Request = serde_json::from_slice(&body).unwrap();
    assert!(matches!(
        decoded,
        Request::Hello { protocol, build } if protocol == PROTOCOL && build == "abc123"
    ));
}

#[test]
fn an_output_frame_round_trips() {
    let encoded = wire::output(42, b"hello from the pty");
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    match frame {
        Frame::Output { id, bytes } => {
            assert_eq!(id, 42);
            assert_eq!(bytes, b"hello from the pty");
        }
        other => panic!("expected output, got {other:?}"),
    }
}

#[test]
fn an_input_frame_round_trips() {
    let encoded = wire::input(7, b"ls -la\n");
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    match frame {
        Frame::Input { id, bytes } => {
            assert_eq!(id, 7);
            assert_eq!(bytes, b"ls -la\n");
        }
        other => panic!("expected input, got {other:?}"),
    }
}

#[test]
fn a_checkpoint_frame_round_trips() {
    let size = TerminalSize::new(120, 40);
    let encoded = wire::checkpoint(9, size, 5000, b"\x1b[2J\x1b[Hsome ansi");
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    match frame {
        Frame::Checkpoint {
            id,
            size: got_size,
            scrollback,
            ansi,
        } => {
            assert_eq!(id, 9);
            assert_eq!(got_size.cols, 120);
            assert_eq!(got_size.rows, 40);
            assert_eq!(scrollback, 5000);
            assert_eq!(ansi, b"\x1b[2J\x1b[Hsome ansi");
        }
        other => panic!("expected checkpoint, got {other:?}"),
    }
}

#[test]
fn an_empty_output_and_checkpoint_body_still_round_trips() {
    let encoded = wire::output(1, b"");
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    assert!(matches!(frame, Frame::Output { id: 1, bytes } if bytes.is_empty()));

    let encoded = wire::checkpoint(1, TerminalSize::new(80, 24), 0, b"");
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    assert!(matches!(
        frame,
        Frame::Checkpoint { id: 1, ansi, .. } if ansi.is_empty()
    ));
}

#[test]
fn two_frames_written_back_to_back_are_read_in_order() {
    let mut bytes = wire::output(1, b"first");
    bytes.extend(wire::output(2, b"second"));

    let mut cursor = Cursor::new(&bytes);
    let a = wire::read(&mut cursor).unwrap();
    let b = wire::read(&mut cursor).unwrap();

    assert!(matches!(a, Frame::Output { id: 1, bytes } if bytes == b"first"));
    assert!(matches!(b, Frame::Output { id: 2, bytes } if bytes == b"second"));
}

#[test]
fn a_zero_length_frame_is_refused() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_be_bytes());

    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn a_frame_past_the_limit_is_refused() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(64 * 1024 * 1024 + 1u32).to_be_bytes());

    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn an_unknown_frame_kind_is_refused() {
    let mut bytes = Vec::new();
    let body = [99u8]; // kind 99, no such kind
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn an_output_frame_too_short_for_its_id_is_refused() {
    let mut bytes = Vec::new();
    let body = [2u8, 1, 2, 3]; // kind 2 (output), only 3 bytes of id
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn a_checkpoint_frame_too_short_for_its_header_is_refused() {
    let mut bytes = Vec::new();
    let mut body = vec![3u8]; // kind 3 (checkpoint)
    body.extend_from_slice(&[0u8; 10]); // short of the 16-byte header
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn a_frame_truncated_mid_length_prefix_is_an_unexpected_eof() {
    let bytes = [0u8, 0]; // only 2 of the 4 length bytes
    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn a_frame_truncated_mid_body_is_an_unexpected_eof() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&10u32.to_be_bytes());
    bytes.extend_from_slice(&[1, 2, 3]); // promised 10 bytes, sent 3

    let err = wire::read(&mut Cursor::new(&bytes)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn send_writes_the_frame_and_flushes() {
    let encoded = wire::output(5, b"payload");
    let mut sink = Vec::new();
    wire::send(&mut sink, &encoded).unwrap();
    assert_eq!(sink, encoded);
}

#[test]
fn random_bytes_never_panic_the_reader() {
    // A tiny fuzz: nothing here should panic, only ever succeed or error.
    let mut state = 0x2545F4914F6CDD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..500 {
        let len = (next() % 40) as usize;
        let mut bytes = Vec::with_capacity(4 + len);
        bytes.extend_from_slice(&(len as u32).to_be_bytes());
        for _ in 0..len {
            bytes.push((next() % 256) as u8);
        }
        let _ = wire::read(&mut Cursor::new(&bytes));
    }
}

#[test]
fn exit_status_round_trips_through_the_wire_type() {
    use ket_core::host::wire::Exit;
    use portable_pty::ExitStatus;

    let coded = ExitStatus::with_exit_code(7);
    let exit: Exit = (&coded).into();
    assert_eq!(exit.code, 7);
    assert_eq!(exit.signal, None);
    let back: ExitStatus = exit.into();
    assert_eq!(back.exit_code(), 7);

    let signalled = ExitStatus::with_signal("KILL");
    let exit: Exit = (&signalled).into();
    assert_eq!(exit.signal.as_deref(), Some("KILL"));
    let back: ExitStatus = exit.into();
    assert_eq!(back.signal(), Some("KILL"));
}

#[test]
fn foreground_round_trips_through_the_wire_type() {
    use ket_core::host::wire::Running;

    let running: Running = Foreground::Shell.into();
    assert_eq!(running, Running::Shell);
    let back: Foreground = running.into();
    assert_eq!(back, Foreground::Shell);

    let running: Running = Foreground::Running("cargo test".to_owned()).into();
    assert_eq!(running, Running::Program("cargo test".to_owned()));
    let back: Foreground = running.into();
    assert_eq!(back, Foreground::Running("cargo test".to_owned()));
}

#[test]
fn request_and_reply_variants_round_trip_through_json() {
    use ket_core::host::wire::Reply;

    let request = Request::Open {
        req: 1,
        spec: Box::new(ket_core::terminal::TerminalSpec::shell("/tmp")),
    };
    let encoded = wire::json(&request).unwrap();
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    let Frame::Json(body) = frame else {
        panic!("expected json");
    };
    let decoded: Request = serde_json::from_slice(&body).unwrap();
    assert!(matches!(decoded, Request::Open { req: 1, .. }));

    let reply = Reply::Welcome {
        protocol: PROTOCOL,
        build: "b".to_owned(),
        live: 3,
    };
    let encoded = wire::json(&reply).unwrap();
    let frame = wire::read(&mut Cursor::new(&encoded)).unwrap();
    let Frame::Json(body) = frame else {
        panic!("expected json");
    };
    let decoded: Reply = serde_json::from_slice(&body).unwrap();
    assert!(matches!(
        decoded,
        Reply::Welcome { protocol, live, .. } if protocol == PROTOCOL && live == 3
    ));
}
