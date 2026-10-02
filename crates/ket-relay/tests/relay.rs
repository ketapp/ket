//! The relay end to end, over real sockets on loopback: who it routes for,
//! what it routes where, and when it lets a socket go. The frame format
//! itself is covered by `ket-relay-protocol/tests/frames.rs`.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use ket_relay::{Own, serve};
use ket_relay_protocol::{
    Control, Frame, Kind, MAX_FRAME, REFUSED_BACKPRESSURE, REFUSED_HOST_OFFLINE, REFUSED_PROTOCOL,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Longest any one step waits before the test calls the relay stuck.
const WAIT: Duration = Duration::from_secs(5);

const HOST: [u8; 16] = [1; 16];
const OTHER: [u8; 16] = [2; 16];
const KEY: [u8; 32] = [9; 32];

/// A relay on a free loopback port, for the rest of the test.
async fn relay(only: Option<Own>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, only));
    address
}

/// One end of a socket to the relay: a host or a phone.
struct End {
    tx: SplitSink<Socket, Message>,
    rx: SplitStream<Socket>,
}

impl End {
    async fn open(relay: SocketAddr) -> Self {
        let (socket, _) = connect_async(format!("ws://{relay}")).await.unwrap();
        let (tx, rx) = socket.split();
        Self { tx, rx }
    }

    async fn send(&mut self, frame: &Frame) {
        self.raw(frame.encode().unwrap()).await;
    }

    async fn raw(&mut self, bytes: Vec<u8>) {
        self.tx.send(Message::binary(bytes)).await.unwrap();
    }

    async fn control(&mut self, control: Control, connection: u64) {
        self.send(&control.frame(connection)).await;
    }

    async fn data(&mut self, connection: u64, body: &[u8]) {
        self.send(&Frame::data(connection, body.to_vec())).await;
    }

    /// The next frame, or `None` once the relay has closed the socket.
    async fn next(&mut self) -> Option<Frame> {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.rx.next().await? {
                    Ok(Message::Binary(bytes)) => return Some(Frame::decode(&bytes).unwrap()),
                    Ok(Message::Close(_)) | Err(_) => return None,
                    Ok(_) => {}
                }
            }
        })
        .await
        .expect("the relay went quiet")
    }

    /// The next frame, which has to be control, and its connection.
    async fn expect_control(&mut self) -> (Control, u64) {
        let frame = self.next().await.expect("the socket ended");
        assert_eq!(frame.kind, Kind::Control, "{frame:?}");
        (Control::parse(&frame.body).unwrap(), frame.connection)
    }

    /// The next frame, which has to be data, and its connection.
    async fn expect_data(&mut self) -> (Vec<u8>, u64) {
        let frame = self.next().await.expect("the socket ended");
        assert_eq!(frame.kind, Kind::Data, "{frame:?}");
        (frame.body, frame.connection)
    }

    async fn expect_refused(&mut self, reason: u8) {
        assert_eq!(self.expect_control().await.0, Control::Refused { reason });
    }

    async fn expect_ended(&mut self) {
        assert_eq!(self.next().await, None);
    }
}

/// A host registered with a development relay.
async fn host(relay: SocketAddr, id: [u8; 16]) -> End {
    let mut host = End::open(relay).await;
    host.control(Control::Register { host: id }, 0).await;
    host
}

/// A phone connected to `id`, and its connection. The relay does not
/// acknowledge a registration, so a phone that arrives first is refused;
/// this tries again until the host is there.
async fn phone(relay: SocketAddr, id: [u8; 16]) -> (End, u64) {
    let deadline = Instant::now() + WAIT;
    loop {
        let mut phone = End::open(relay).await;
        phone.control(Control::Connect { host: id }, 0).await;
        match phone.expect_control().await {
            (Control::Connected, connection) => return (phone, connection),
            (
                Control::Refused {
                    reason: REFUSED_HOST_OFFLINE,
                },
                _,
            ) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            other => panic!("{other:?}"),
        }
    }
}

/// A phone connected to `host`, which has been told about it.
async fn phone_of(relay: SocketAddr, id: [u8; 16], host: &mut End) -> (End, u64) {
    let (phone, connection) = phone(relay, id).await;
    assert_eq!(host.expect_control().await, (Control::Opened, connection));
    (phone, connection)
}

#[tokio::test]
async fn a_phone_and_its_host_exchange_data() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;
    let (mut phone, connection) = phone_of(relay, HOST, &mut host).await;
    assert_ne!(connection, 0);

    phone.data(connection, b"keystroke").await;
    assert_eq!(
        host.expect_data().await,
        (b"keystroke".to_vec(), connection)
    );

    host.data(connection, b"echo").await;
    assert_eq!(phone.expect_data().await, (b"echo".to_vec(), connection));

    // Empty and full-size bodies pass through untouched.
    phone.data(connection, &[]).await;
    assert_eq!(host.expect_data().await, (Vec::new(), connection));
    let full = vec![0xab; Frame::MAX_BODY];
    host.data(connection, &full).await;
    assert_eq!(phone.expect_data().await, (full, connection));
}

#[tokio::test]
async fn a_phone_is_routed_by_the_connection_the_relay_gave_it() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;
    let (mut first, one) = phone_of(relay, HOST, &mut host).await;
    let (mut second, two) = phone_of(relay, HOST, &mut host).await;
    assert_ne!(one, two);

    // Written as the other phone's connection, arriving as its own.
    first.data(two, b"from the first").await;
    assert_eq!(host.expect_data().await, (b"from the first".to_vec(), one));
    second.data(one, b"from the second").await;
    assert_eq!(host.expect_data().await, (b"from the second".to_vec(), two));

    // And the host reaches each phone only on its own connection.
    host.data(two, b"to the second").await;
    host.data(one, b"to the first").await;
    assert_eq!(first.expect_data().await, (b"to the first".to_vec(), one));
    assert_eq!(second.expect_data().await, (b"to the second".to_vec(), two));
}

#[tokio::test]
async fn a_phone_asking_for_an_absent_host_is_refused() {
    let relay = relay(None).await;
    let _host = host(relay, HOST).await;
    let mut phone = End::open(relay).await;
    phone.control(Control::Connect { host: OTHER }, 0).await;
    phone.expect_refused(REFUSED_HOST_OFFLINE).await;
    phone.expect_ended().await;
}

#[tokio::test]
async fn a_socket_that_opens_with_anything_else_is_refused() {
    let relay = relay(None).await;
    let openings = [
        Frame::data(0, b"ciphertext".to_vec()).encode().unwrap(),
        Control::Opened.frame(0).encode().unwrap(),
        Control::Connected.frame(0).encode().unwrap(),
        Control::Closed.frame(1).encode().unwrap(),
        // A claim means nothing to a development relay.
        Control::Claim {
            host: HOST,
            key: KEY,
        }
        .frame(0)
        .encode()
        .unwrap(),
        // Not a frame at all.
        b"GET / HTTP/1.1".to_vec(),
        // A control frame whose body is no control.
        Frame {
            connection: 0,
            kind: Kind::Control,
            body: vec![0xee],
        }
        .encode()
        .unwrap(),
    ];
    for opening in openings {
        let mut socket = End::open(relay).await;
        socket.raw(opening).await;
        socket.expect_refused(REFUSED_PROTOCOL).await;
        socket.expect_ended().await;
    }
}

#[tokio::test]
async fn a_phone_leaving_closes_its_connection_on_the_host() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;
    let (phone, connection) = phone_of(relay, HOST, &mut host).await;
    drop(phone);
    assert_eq!(host.expect_control().await, (Control::Closed, connection));
}

#[tokio::test]
async fn a_host_closing_a_connection_lets_the_phone_go() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;
    let (mut phone, connection) = phone_of(relay, HOST, &mut host).await;

    host.control(Control::Closed, connection).await;
    assert_eq!(phone.expect_control().await, (Control::Closed, connection));
    // Both sides are told, the one that asked included.
    assert_eq!(host.expect_control().await, (Control::Closed, connection));

    // Whatever the phone sends after that goes nowhere, and ends its socket.
    phone.data(connection, b"too late").await;
    phone.expect_ended().await;
    let (_again, next) = phone_of(relay, HOST, &mut host).await;
    assert_ne!(next, connection);
}

#[tokio::test]
async fn a_host_leaving_closes_its_phones_and_its_registration() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;
    let (mut first, one) = phone_of(relay, HOST, &mut host).await;
    let (mut second, two) = phone_of(relay, HOST, &mut host).await;

    drop(host);
    assert_eq!(first.expect_control().await, (Control::Closed, one));
    assert_eq!(second.expect_control().await, (Control::Closed, two));

    // The registration went before the phones were told.
    let mut late = End::open(relay).await;
    late.control(Control::Connect { host: HOST }, 0).await;
    late.expect_refused(REFUSED_HOST_OFFLINE).await;
}

#[tokio::test]
async fn a_host_cannot_reach_or_close_another_hosts_phones() {
    let relay = relay(None).await;
    let mut ours = host(relay, HOST).await;
    let mut theirs = host(relay, OTHER).await;
    let (mut our_phone, mine) = phone_of(relay, HOST, &mut ours).await;
    let (mut their_phone, target) = phone_of(relay, OTHER, &mut theirs).await;

    ours.data(target, b"not yours").await;
    ours.control(Control::Closed, target).await;
    // The relay reads a host's frames in order: once this one has arrived,
    // the two before it have been dealt with.
    ours.data(mine, b"marker").await;
    assert_eq!(our_phone.expect_data().await, (b"marker".to_vec(), mine));

    theirs.data(target, b"yours").await;
    assert_eq!(their_phone.expect_data().await, (b"yours".to_vec(), target));
}

#[tokio::test]
async fn a_reregistered_host_keeps_its_place_when_the_old_socket_goes() {
    let relay = relay(None).await;
    let mut old = host(relay, HOST).await;
    let (mut old_phone, old_connection) = phone_of(relay, HOST, &mut old).await;

    let mut new = host(relay, HOST).await;
    // Nothing says when the new registration has landed: probe with phones
    // until one reaches it.
    let deadline = Instant::now() + WAIT;
    let (mut new_phone, new_connection) = loop {
        let (probe, connection) = phone(relay, HOST).await;
        tokio::select! {
            opened = new.expect_control() => {
                assert_eq!(opened, (Control::Opened, connection));
                break (probe, connection);
            }
            opened = old.expect_control() => {
                assert_eq!(opened, (Control::Opened, connection));
                drop(probe);
                assert_eq!(old.expect_control().await, (Control::Closed, connection));
                assert!(Instant::now() < deadline, "the new host never took over");
            }
        }
    };

    // The old host's phone stays with it until it goes, and then goes too.
    old.data(old_connection, b"still here").await;
    assert_eq!(
        old_phone.expect_data().await,
        (b"still here".to_vec(), old_connection)
    );
    drop(old);
    assert_eq!(
        old_phone.expect_control().await,
        (Control::Closed, old_connection)
    );

    // The new registration outlives the old socket, and its phones with it.
    new.data(new_connection, b"new").await;
    assert_eq!(
        new_phone.expect_data().await,
        (b"new".to_vec(), new_connection)
    );
    phone_of(relay, HOST, &mut new).await;
}

#[tokio::test]
async fn a_hosts_own_relay_takes_only_its_claim() {
    let relay = relay(Some(Own {
        host: HOST,
        key: KEY,
    }))
    .await;
    let refused = [
        Control::Register { host: HOST },
        Control::Register { host: OTHER },
        Control::Claim {
            host: HOST,
            key: [8; 32],
        },
        Control::Claim {
            host: OTHER,
            key: KEY,
        },
    ];
    for opening in refused {
        let mut socket = End::open(relay).await;
        socket.control(opening, 0).await;
        socket.expect_refused(REFUSED_PROTOCOL).await;
        socket.expect_ended().await;
    }

    // Another host, to a phone, is one that is not here.
    let mut phone_for_other = End::open(relay).await;
    phone_for_other
        .control(Control::Connect { host: OTHER }, 0)
        .await;
    phone_for_other.expect_refused(REFUSED_HOST_OFFLINE).await;

    let mut host = End::open(relay).await;
    host.control(
        Control::Claim {
            host: HOST,
            key: KEY,
        },
        0,
    )
    .await;
    let (mut phone, connection) = phone_of(relay, HOST, &mut host).await;
    phone.data(connection, b"hello").await;
    assert_eq!(host.expect_data().await, (b"hello".to_vec(), connection));
}

#[tokio::test]
async fn a_hosts_own_relay_keeps_its_claim_while_the_host_lives() {
    let own = Own {
        host: HOST,
        key: KEY,
    };
    let relay = relay(Some(own)).await;
    let claim = Control::Claim {
        host: HOST,
        key: KEY,
    };

    let mut host = End::open(relay).await;
    host.control(claim.clone(), 0).await;
    let (mut phone, connection) = phone_of(relay, HOST, &mut host).await;

    // Even with the right key, a second claim would take the first's phones.
    let mut usurper = End::open(relay).await;
    usurper.control(claim.clone(), 0).await;
    usurper.expect_refused(REFUSED_PROTOCOL).await;
    usurper.expect_ended().await;
    host.data(connection, b"undisturbed").await;
    assert_eq!(
        phone.expect_data().await,
        (b"undisturbed".to_vec(), connection)
    );

    // Once it has gone, the host can claim it again.
    drop(host);
    assert_eq!(phone.expect_control().await, (Control::Closed, connection));
    let mut back = End::open(relay).await;
    back.control(claim, 0).await;
    phone_of(relay, HOST, &mut back).await;
}

#[tokio::test]
async fn a_phone_that_sends_nonsense_is_let_go() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;

    let (mut garbled, one) = phone_of(relay, HOST, &mut host).await;
    garbled.raw(vec![0xff; 32]).await;
    assert_eq!(host.expect_control().await, (Control::Closed, one));

    // A message past the frame limit is cut off by the socket itself.
    let (mut oversized, two) = phone_of(relay, HOST, &mut host).await;
    let _ = oversized
        .tx
        .send(Message::binary(vec![0; MAX_FRAME + 1]))
        .await;
    assert_eq!(host.expect_control().await, (Control::Closed, two));
}

#[tokio::test]
async fn a_phone_that_falls_behind_is_cut_off() {
    let relay = relay(None).await;
    let mut host = host(relay, HOST).await;
    let (mut phone, connection) = phone_of(relay, HOST, &mut host).await;

    // The phone reads nothing while the host sends far more than the relay
    // will queue for it: the host is told the connection is over.
    let chunk = Frame::data(connection, vec![7; Frame::MAX_BODY])
        .encode()
        .unwrap();
    let mut sent = 0;
    let closed = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            tokio::select! {
                frame = host.rx.next() => break frame,
                sending = host.tx.send(Message::binary(chunk.clone())), if sent < 256 => {
                    sending.unwrap();
                    sent += 1;
                }
            }
        }
    })
    .await
    .expect("the relay queued without bound");
    let Some(Ok(Message::Binary(closed))) = closed else {
        panic!("{closed:?}");
    };
    let closed = Frame::decode(&closed).unwrap();
    assert_eq!(closed, Control::Closed.frame(connection));

    // The phone gets what was queued before the cut, then why, then nothing.
    loop {
        let frame = phone.next().await.expect("ended without saying why");
        if frame.kind == Kind::Control {
            assert_eq!(
                Control::parse(&frame.body).unwrap(),
                Control::Refused {
                    reason: REFUSED_BACKPRESSURE
                }
            );
            break;
        }
    }
    phone.expect_ended().await;
}

#[tokio::test]
async fn one_address_holds_at_most_32_sockets() {
    let relay = relay(None).await;
    // Sockets that never finish opening still count.
    let mut held = Vec::new();
    for _ in 0..32 {
        held.push(TcpStream::connect(relay).await.unwrap());
    }
    assert!(connect_async(format!("ws://{relay}")).await.is_err());

    // One going frees its place, once the relay has noticed.
    drop(held.pop());
    let deadline = Instant::now() + WAIT;
    while connect_async(format!("ws://{relay}")).await.is_err() {
        assert!(Instant::now() < deadline, "the place was never freed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
