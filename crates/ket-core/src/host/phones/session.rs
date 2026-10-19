//! One phone's session: its handshake and whether it is let in, then its
//! requests and the terminals it watches, until it goes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use ket_relay_protocol::Control;
use ket_remote::proto::{self, envelope::Payload};
use prost::Message;

use super::relay::RelayPipe;
use super::requests::{envelope, error, handle};
use super::snapshot::snapshot;
use super::{APPROVAL_TIMEOUT, Approval, Event, Phones, epoch, lock};
use crate::Result;
use crate::host::host_error;
use crate::host::server::Host;
use crate::host::wire::DeviceRole;
use crate::terminal::{LocalTerminal, Since};

/// Counts a phone as connected while it lives.
struct Connected<'a>(&'a AtomicUsize);

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn serve_phone(host: &Arc<Host>, phones: &Phones, pipe: RelayPipe) {
    let connection = pipe.connection;
    let out = pipe.out.clone();
    if let Err(error) = session(host, phones, pipe) {
        tracing::info!(connection, %error, "phone session ended");
    }
    let _ = out.send(Control::Closed.frame(connection));
}

/// One phone's session: its Noise session, which device it is, and its name
/// for leases.
pub(super) struct Phone<'a> {
    pub(super) session: ket_remote::Session<RelayPipe>,
    pub(super) device: Vec<u8>,
    pub(super) holder: u64,
    pub(super) role: DeviceRole,
    pub(super) phones: &'a Phones,
}

impl Phone<'_> {
    /// Sends an envelope, numbered, and keeps it for the phone to resume from.
    fn send(&mut self, mut envelope: proto::Envelope) -> Result<()> {
        let bytes = {
            let mut devices = lock(&self.phones.devices);
            let device = devices.entry(self.device.clone()).or_default();
            device.seq += 1;
            envelope.seq = device.seq;
            let bytes = envelope.encode_to_vec();
            device.remember(envelope.seq, &bytes);
            bytes
        };
        self.send_encoded(&bytes)
    }

    fn send_encoded(&mut self, bytes: &[u8]) -> Result<()> {
        for frame in self.session.seal(bytes).map_err(host_error)? {
            ket_remote::Pipe::send(self.session.pipe_mut(), frame).map_err(host_error)?;
        }
        Ok(())
    }
}

/// Counts a handshake as in flight until it is dropped.
struct Pending<'a>(&'a AtomicUsize);

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn session(host: &Arc<Host>, phones: &Phones, mut pipe: RelayPipe) -> Result<()> {
    let pending = Pending(&phones.pending);
    let feed = pipe.feed.clone();
    let admitted = pipe.admitted.clone();

    // The waits on the phone happen with nothing held: a connection that says
    // nothing used to hold the identity — every other handshake, every
    // pairing code, the device list and revocation — until it timed out.
    let first = ket_remote::Pipe::recv(&mut pipe).map_err(host_error)?;
    let answered = lock(&phones.identity).respond(&first);
    let handshake = match answered {
        Ok(handshake) => handshake,
        Err(rejection) => {
            if let Some(refusal) = rejection.refusal() {
                let _ = ket_remote::Pipe::send(&mut pipe, refusal);
            }
            return Err(host_error(rejection.error));
        }
    };
    let (device, paired_now) = (handshake.device.clone(), handshake.paired_now);
    let code = ket_remote::confirmation_code(&handshake.hash);
    let mut session = handshake.finish(pipe).map_err(host_error)?;
    let first = session.recv().map_err(host_error)?;
    let mut phone = Phone {
        session,
        device,
        holder: phones.next_session.fetch_add(1, Ordering::Relaxed),
        // Replaced below before any request is handled.
        role: DeviceRole::Viewer,
        phones,
    };
    // A pairing is granted only for a phone whose first message is a hello
    // this host understands, not merely one that decrypted.
    let hello = proto::Envelope::decode(first.as_slice()).map_err(host_error)?;
    if let Err(refusal) = ket_remote::check_version(&hello) {
        phone.send(envelope(0, Payload::Error(refusal)))?;
        return Ok(());
    }
    let Some(Payload::Hello(hello)) = hello.payload else {
        return Err(host_error("a phone's first message was not a hello"));
    };
    // A scanned code is not enough: a person approves the phone on the
    // desktop, comparing the code it shows with the phone's.
    if paired_now && approved(phones, &mut phone, &hello.device_name, &code)?.is_none() {
        phone.send(error(
            0,
            proto::ErrorCode::Declined,
            "the pairing was not approved on the desktop",
        ))?;
        return Ok(());
    }
    // Every paired device has full access. The desktop's approval is the
    // trust decision, and the owner chose one Approve over a choice of
    // narrower grants (2026-10-01), so whatever role the approval or the
    // file carries, a device that is let in can do what the desktop can.
    let role = DeviceRole::Administrator;
    phone.role = role;

    // Let in and counted live under one hold of the identity, which is what
    // `Phones::revoke` holds too: a phone revoked while its handshake was in
    // flight finds its grant gone here, and one let in is in `live` before a
    // revocation can look.
    {
        let mut identity = lock(&phones.identity);
        identity
            .admit(&phone.device, paired_now)
            .map_err(host_error)?;
        if paired_now {
            identity.name_device(&phone.device, &hello.device_name);
            lock(&phones.roles).insert(phone.device.clone(), role);
            crate::host::identity::save_with_roles(&identity, &lock(&phones.roles))?;
            tracing::info!(device = %hello.device_name, "paired a phone");
        } else if lock(&phones.roles).get(&phone.device) != Some(&role) {
            // Paired before roles existed, or given a narrower one while they
            // were offered: brought up to full access, and the file says so.
            // After `admit`, so only a device that is still paired is written.
            lock(&phones.roles).insert(phone.device.clone(), role);
            crate::host::identity::save_with_roles(&identity, &lock(&phones.roles))?;
        }
        lock(&phones.live)
            .entry(phone.device.clone())
            .or_default()
            .push((phone.holder, feed.clone()));
    }
    admitted.store(true, Ordering::Release);
    drop(pending);
    phones.connected.fetch_add(1, Ordering::AcqRel);
    let _connected = Connected(&phones.connected);
    // Whatever way this ends — the phone leaving, or an error from here on —
    // its terminals are unfitted, its control released, and it leaves `live`.
    let mut held = Held {
        host,
        phones,
        device: phone.device.clone(),
        holder: phone.holder,
        subscriptions: HashMap::new(),
        leased: HashSet::new(),
    };

    // Resume where the phone was, if this is the same run of the host and
    // what it missed is still kept; otherwise it starts over from a snapshot.
    let missed: Option<Vec<Vec<u8>>> = (hello.resume_epoch == epoch())
        .then(|| {
            let devices = lock(&phones.devices);
            let device = devices.get(&phone.device)?;
            device.can_resume(hello.resume_seq).then(|| {
                device
                    .sent
                    .iter()
                    .filter(|(seq, _)| *seq > hello.resume_seq)
                    .map(|(_, bytes)| bytes.clone())
                    .collect()
            })
        })
        .flatten();
    phone.send(envelope(
        0,
        Payload::Welcome(proto::Welcome {
            host_name: host_name(),
            host_epoch: epoch(),
            resumed: missed.is_some(),
        }),
    ))?;
    for bytes in missed.unwrap_or_default() {
        phone.send_encoded(&bytes)?;
    }
    // A fresh picture either way: while no phone was connected no snapshots
    // were sent, so what a resumed phone missed does not include them.
    let first = snapshot(host, phones);
    let mut shown = first.encode_to_vec();
    phone.send(envelope(0, Payload::Snapshot(first)))?;

    loop {
        let event = phone.session.pipe_mut().events.recv();
        match event {
            Ok(Event::Frame(frame)) => {
                let Some(message) = phone.session.open(&frame).map_err(host_error)? else {
                    continue;
                };
                let request = proto::Envelope::decode(message.as_slice()).map_err(host_error)?;
                let reply = handle(
                    host,
                    &phone,
                    &request,
                    &feed,
                    &mut held.subscriptions,
                    &mut held.leased,
                );
                if let Some(reply) = reply {
                    phone.send(reply)?;
                }
            }
            Ok(Event::Send(envelope)) => phone.send(*envelope)?,
            Ok(Event::Snapshot(snapshot)) => {
                let encoded = snapshot.encode_to_vec();
                if encoded != shown {
                    shown = encoded;
                    phone.send(envelope(0, Payload::Snapshot(*snapshot)))?;
                }
            }
            Ok(Event::Closed) | Err(_) => return Ok(()),
        }
    }
}

/// Waits for a person to approve a phone that has just paired. The phone is
/// told to wait, with the code; the desktop's Devices pane shows the same
/// code and the phone's name, and decides. `false` when declined, when it
/// lapses, or when the phone gives up.
fn approved(
    phones: &Phones,
    phone: &mut Phone<'_>,
    name: &str,
    code: &str,
) -> Result<Option<DeviceRole>> {
    let id = phones.next_approval.fetch_add(1, Ordering::Relaxed);
    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    lock(&phones.approvals).push(Approval {
        id,
        name: if name.trim().is_empty() {
            "A phone".to_owned()
        } else {
            name
        },
        code: code.to_owned(),
        asked: Instant::now(),
        decision: None,
    });
    let forget = |phones: &Phones| lock(&phones.approvals).retain(|a| a.id != id);
    tracing::info!(approval = id, "a phone is waiting for approval");
    if let Err(error) = phone.send(envelope(
        0,
        Payload::PairingPending(proto::PairingPending {
            code: code.to_owned(),
            timeout_seconds: u32::try_from(APPROVAL_TIMEOUT.as_secs()).unwrap_or(u32::MAX),
        }),
    )) {
        forget(phones);
        return Err(error);
    }
    let deadline = Instant::now() + APPROVAL_TIMEOUT;
    loop {
        let decision = lock(&phones.approvals)
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.decision);
        if let Some(Some(decision)) = decision {
            forget(phones);
            return Ok(decision);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            forget(phones);
            return Ok(None);
        }
        // The phone going away ends the wait; anything it sends meanwhile
        // is dropped — it has nothing to ask until it is let in.
        match phone
            .session
            .pipe_mut()
            .events
            .recv_timeout(left.min(Duration::from_millis(250)))
        {
            Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                forget(phones);
                return Ok(None);
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// What a phone's session holds on the host, let go when the session ends
/// however it ends — a malformed frame from a paired phone used to return
/// straight past this and leave a terminal fitted to it for good.
struct Held<'a> {
    host: &'a Arc<Host>,
    phones: &'a Phones,
    device: Vec<u8>,
    holder: u64,
    subscriptions: HashMap<u64, Arc<AtomicBool>>,
    leased: HashSet<u64>,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        for (&terminal, stop) in &self.subscriptions {
            stop.store(true, Ordering::Release);
            self.host.unfit(terminal, self.holder);
        }
        // A phone that has gone does not keep control until its lease lapses.
        for &terminal in &self.leased {
            self.host.release(terminal, self.holder);
        }
        let mut live = lock(&self.phones.live);
        if let Some(sessions) = live.get_mut(&self.device) {
            sessions.retain(|(holder, _)| *holder != self.holder);
            if sessions.is_empty() {
                live.remove(&self.device);
            }
        }
    }
}

/// What the phone calls this Mac: the name it has in System Settings.
///
/// `scutil` rather than a system call, the way ket reaches the platform
/// elsewhere; asked once per run.
fn host_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::process::Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Mac".to_owned())
    })
    .clone()
}

/// Sends one terminal to one phone: a checkpoint, then its output, until
/// the phone lets go or the program ends.
pub(super) fn pump(
    id: u64,
    terminal: &LocalTerminal,
    feed: &mpsc::Sender<Event>,
    stop: &AtomicBool,
) {
    let post = |payload: Payload| {
        let mut envelope = envelope(0, payload);
        envelope.stream_id = id;
        feed.send(Event::Send(Box::new(envelope))).is_ok()
    };
    let checkpoint = || {
        let checkpoint = terminal.checkpoint();
        let posted = post(Payload::Checkpoint(proto::TerminalCheckpoint {
            terminal_id: id,
            epoch: checkpoint.cursor.epoch,
            through_seq: checkpoint.cursor.seq,
            cols: u32::from(checkpoint.size.cols),
            rows: u32::from(checkpoint.size.rows),
            scrollback: checkpoint.scrollback as u32,
            ansi: checkpoint.ansi,
        }));
        posted.then_some(checkpoint.cursor)
    };

    let mut frames = terminal.frames();
    frames.borrow_and_update();
    let Some(mut cursor) = checkpoint() else {
        return;
    };
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        match terminal.output_since(cursor) {
            Since::Output {
                bytes,
                cursor: next,
            } => {
                if !bytes.is_empty()
                    && !post(Payload::Chunk(proto::TerminalChunk {
                        terminal_id: id,
                        epoch: cursor.epoch,
                        seq: cursor.seq,
                        bytes,
                    }))
                {
                    return;
                }
                cursor = next;
            }
            Since::Reset => {
                if !post(Payload::Reset(proto::TerminalReset { terminal_id: id })) {
                    return;
                }
                match checkpoint() {
                    Some(next) => cursor = next,
                    None => return,
                }
            }
        }
        if terminal.is_closed() {
            let status = terminal.exit_status();
            post(Payload::Closed(proto::TerminalClosed {
                terminal_id: id,
                exit_code: status.as_ref().map(portable_pty::ExitStatus::exit_code),
                signal: status
                    .as_ref()
                    .and_then(|s| s.signal().map(str::to_owned))
                    .unwrap_or_default(),
            }));
            return;
        }
        if futures::executor::block_on(frames.changed()).is_err() {
            return;
        }
    }
}
