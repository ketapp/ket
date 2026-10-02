//! Writes Noise handshake vectors from `snow`, for the TypeScript
//! implementation in `mobile/remote` to reproduce byte for byte.
//!
//! Both patterns ket uses — `IKpsk2` to pair, `IK` to reconnect — with every
//! key fixed, ephemerals included, so the phone's first message and both
//! sides' first transport messages are determined: a TypeScript initiator
//! that gets any byte different is wrong.
//!
//! ```sh
//! cargo run -p ket-remote --example vectors > mobile/remote/test/vectors.json
//! ```

use std::fmt::Write as _;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("randomness");
    bytes
}

struct Case {
    pattern: &'static str,
    psk: Option<[u8; 32]>,
}

fn main() {
    let cases = [
        Case {
            pattern: "Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s",
            psk: Some(random()),
        },
        Case {
            pattern: "Noise_IK_25519_ChaChaPoly_BLAKE2s",
            psk: None,
        },
    ];
    let mut out = Vec::new();
    for case in cases {
        let params: snow::params::NoiseParams = case.pattern.parse().expect("pattern");
        let phone = snow::Builder::new(params.clone())
            .generate_keypair()
            .unwrap();
        let host = snow::Builder::new(params.clone())
            .generate_keypair()
            .unwrap();
        let phone_ephemeral: [u8; 32] = random();
        let host_ephemeral: [u8; 32] = random();
        // The real header shape: magic, mode, host id, offer id.
        let mut prologue = b"KET1".to_vec();
        prologue.push(if case.psk.is_some() { 1 } else { 2 });
        prologue.extend_from_slice(&random::<16>());
        prologue.extend_from_slice(&random::<16>());

        let mut initiator = snow::Builder::new(params.clone())
            .local_private_key(&phone.private)
            .unwrap()
            .remote_public_key(&host.public)
            .unwrap()
            .prologue(&prologue)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&phone_ephemeral);
        let mut responder = snow::Builder::new(params)
            .local_private_key(&host.private)
            .unwrap()
            .prologue(&prologue)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&host_ephemeral);
        if let Some(psk) = case.psk.as_ref() {
            initiator = initiator.psk(2, psk).unwrap();
            responder = responder.psk(2, psk).unwrap();
        }
        let mut initiator = initiator.build_initiator().unwrap();
        let mut responder = responder.build_responder().unwrap();

        let mut buf = vec![0u8; 65535];
        let mut scratch = vec![0u8; 65535];
        let n = initiator.write_message(&[], &mut buf).unwrap();
        let message1 = buf[..n].to_vec();
        responder.read_message(&message1, &mut scratch).unwrap();
        let n = responder.write_message(&[], &mut buf).unwrap();
        let message2 = buf[..n].to_vec();
        initiator.read_message(&message2, &mut scratch).unwrap();

        let mut initiator = initiator.into_transport_mode().unwrap();
        let mut responder = responder.into_transport_mode().unwrap();
        let phone_says = b"\x00hello from the phone";
        let n = initiator.write_message(phone_says, &mut buf).unwrap();
        let transport1 = buf[..n].to_vec();
        let host_says = b"\x00hello from the host";
        let n = responder.write_message(host_says, &mut buf).unwrap();
        let transport2 = buf[..n].to_vec();

        out.push(format!(
            concat!(
                "  {{\n",
                "    \"pattern\": \"{}\",\n",
                "    \"phone_private\": \"{}\",\n",
                "    \"phone_public\": \"{}\",\n",
                "    \"host_private\": \"{}\",\n",
                "    \"host_public\": \"{}\",\n",
                "    \"phone_ephemeral\": \"{}\",\n",
                "    \"host_ephemeral\": \"{}\",\n",
                "    \"psk\": \"{}\",\n",
                "    \"prologue\": \"{}\",\n",
                "    \"message1\": \"{}\",\n",
                "    \"message2\": \"{}\",\n",
                "    \"phone_plaintext\": \"{}\",\n",
                "    \"transport1\": \"{}\",\n",
                "    \"host_plaintext\": \"{}\",\n",
                "    \"transport2\": \"{}\"\n",
                "  }}"
            ),
            case.pattern,
            hex(&phone.private),
            hex(&phone.public),
            hex(&host.private),
            hex(&host.public),
            hex(&phone_ephemeral),
            hex(&host_ephemeral),
            case.psk.map(|p| hex(&p)).unwrap_or_default(),
            hex(&prologue),
            hex(&message1),
            hex(&message2),
            hex(phone_says),
            hex(&transport1),
            hex(host_says),
            hex(&transport2),
        ));
    }
    println!("[\n{}\n]", out.join(",\n"));
}
