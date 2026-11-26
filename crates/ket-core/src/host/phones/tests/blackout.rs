//! Blackout hours, enforced by the host: what a phone is told and refused
//! during one, the sessions one ends as it begins, and being let in.
//!
//! The setting is written the way the window writes it, to `blackout.json`
//! in the sandbox's data directory, and the host's once-a-tick look is
//! called by hand: the rig runs no tick of its own.

use super::*;
use crate::automation::Clock;
use crate::host::blackout::{self as setting, Blackout};

/// A blackout that is on whatever the time: midnight to midnight, every day.
fn all_day() -> Blackout {
    let midnight = Clock::new(0, 0).unwrap();
    Blackout {
        enabled: true,
        from: midnight,
        until: midnight,
        ..Blackout::default()
    }
}

fn now_seconds() -> u64 {
    crate::now_ms() / 1000
}

/// A phone paired and approved: its session and its first snapshot.
fn paired_with_snapshot(rig: &Rig, device: &Keypair) -> (PhoneSession, proto::HostSnapshot) {
    let mut session = pair(rig, device);
    let code = pending(&mut session);
    decide(rig, &code, true);
    assert!(matches!(
        next(&mut session).payload,
        Some(Payload::Welcome(_))
    ));
    match next(&mut session).payload {
        Some(Payload::Snapshot(snapshot)) => (session, snapshot),
        other => panic!("no snapshot followed the welcome: {other:?}"),
    }
}

#[test]
fn a_phone_in_a_blackout_is_told_only_the_blackout_and_refused_what_it_asks() {
    if rerun_sandboxed(
        "blackout::a_phone_in_a_blackout_is_told_only_the_blackout_and_refused_what_it_asks",
    ) {
        return;
    }
    let rig = rig();
    // Something a phone would otherwise be shown.
    let (_, _terminal) = terminal(&rig, "/bin/cat", &[]);
    setting::save(&all_day()).unwrap();
    assert!(rig.phones.watch_blackout().is_some());

    let device = Keypair::generate().unwrap();
    let (mut session, snapshot) = paired_with_snapshot(&rig, &device);
    let blackout = snapshot.blackout.clone().expect("the blackout is said");
    assert!(blackout.until_ms > crate::now_ms());
    assert_eq!(blackout.let_in_until_ms, 0);
    assert_eq!(blackout.repeat, "Every day");
    assert_eq!(blackout.windows.len(), setting::WINDOWS_AHEAD);
    assert_eq!(blackout.windows[0].end_ms, blackout.until_ms);
    assert!(snapshot.projects.is_empty(), "{:?}", snapshot.projects);
    assert!(snapshot.usage.is_empty() && snapshot.agents.is_empty());

    // Reading and changing alike.
    let read = ask(
        &mut session,
        1,
        Payload::GetBacklog(proto::GetBacklog {
            project_id: "anything".into(),
        }),
    );
    assert_eq!(error_code(&read), Some(proto::ErrorCode::Blackout));
    let watch = ask(
        &mut session,
        2,
        Payload::Subscribe(proto::Subscribe { terminal_id: 1 }),
    );
    assert_eq!(error_code(&watch), Some(proto::ErrorCode::Blackout));

    // Where its notifications go is still heard: it says so as it connects.
    let quiet = ask(
        &mut session,
        3,
        Payload::UnregisterPush(proto::UnregisterPush {}),
    );
    assert!(is_ack(&quiet), "{quiet:?}");
}

#[test]
fn a_blackout_beginning_ends_the_sessions_open_then() {
    if rerun_sandboxed("blackout::a_blackout_beginning_ends_the_sessions_open_then") {
        return;
    }
    let rig = rig();
    let device = Keypair::generate().unwrap();
    let (mut session, snapshot) = paired_with_snapshot(&rig, &device);
    assert_eq!(snapshot.blackout, None, "no blackout hours are set");
    assert!(rig.phones.watch_blackout().is_none());

    setting::save(&all_day()).unwrap();
    assert!(rig.phones.watch_blackout().is_some());
    assert!(
        session.recv().is_err(),
        "the session stayed open into the blackout"
    );

    // It may come back, to be told when the blackout ends.
    eventually("the ended session left", || {
        rig.phones.devices().iter().all(|device| !device.connected)
    });
    let mut again = resume(&rig, &device, &hello(0, 0)).expect("a paired phone reconnects");
    assert!(matches!(
        next(&mut again).payload,
        Some(Payload::Welcome(_))
    ));
    match next(&mut again).payload {
        Some(Payload::Snapshot(snapshot)) => {
            assert!(
                snapshot
                    .blackout
                    .is_some_and(|blackout| blackout.until_ms != 0)
            );
        }
        other => panic!("no snapshot followed the welcome: {other:?}"),
    }
    // Still on at the next look: nothing more is ended.
    assert!(rig.phones.watch_blackout().is_some());
    let asked = ask(&mut again, 1, Payload::GetSnippets(proto::GetSnippets {}));
    assert_eq!(error_code(&asked), Some(proto::ErrorCode::Blackout));
}

#[test]
fn the_desktop_lets_devices_in_and_shuts_them_out_again() {
    if rerun_sandboxed("blackout::the_desktop_lets_devices_in_and_shuts_them_out_again") {
        return;
    }
    let rig = rig();
    setting::save(&Blackout {
        let_in_until: now_seconds() + 3600,
        ..all_day()
    })
    .unwrap();
    assert!(rig.phones.watch_blackout().is_none(), "let in for an hour");
    assert!(!rig.phones.shut());

    let device = Keypair::generate().unwrap();
    let (mut session, snapshot) = paired_with_snapshot(&rig, &device);
    let blackout = snapshot.blackout.expect("the hours are still said");
    assert_eq!(blackout.until_ms, 0);
    assert!(blackout.let_in_until_ms > crate::now_ms());
    let asked = ask(&mut session, 1, Payload::GetSnippets(proto::GetSnippets {}));
    assert_ne!(error_code(&asked), Some(proto::ErrorCode::Blackout));

    // The desktop takes it back: shut out at the next look, session and all.
    setting::save(&all_day()).unwrap();
    let shut = rig.phones.watch_blackout().expect("shut out again");
    assert!(shut.hold, "notifications wait unless told otherwise");
    assert!(session.recv().is_err());

    // And turning blackout hours off lets everything through.
    setting::save(&Blackout::default()).unwrap();
    assert!(rig.phones.watch_blackout().is_none());
    assert_eq!(rig.phones.blackout_message(), None);
}

#[test]
fn a_damaged_setting_does_not_lift_a_blackout() {
    if rerun_sandboxed("blackout::a_damaged_setting_does_not_lift_a_blackout") {
        return;
    }
    let rig = rig();
    setting::save(&all_day()).unwrap();
    assert!(rig.phones.watch_blackout().is_some());
    let file = crate::host::identity::dir().unwrap().join("blackout.json");
    std::fs::write(&file, b"{ not json").unwrap();
    assert!(rig.phones.watch_blackout().is_some());
    assert!(rig.phones.shut());
}
