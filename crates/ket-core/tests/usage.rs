//! What an agent reports consuming, and what happens when it reports nothing.
//!
//! The distinction these tests exist to hold: a transport that *cannot* measure
//! usage must read as unknown, never as zero. A status bar showing "0 tokens"
//! beside a working agent is worse than one showing nothing at all, and zero is
//! the value a careless implementation reaches for.

use std::sync::Arc;

use ket_core::agent::{SessionContext, cancellation};
use ket_core::config::{AgentTimeouts, PermissionConfig};
use ket_core::event::{Envelope, Event, EventBus, Money, SessionUsage};
use ket_core::id::SessionId;

/// A context with a bus attached, as a transport would be handed one.
fn context() -> (SessionContext, tokio::sync::broadcast::Receiver<Envelope>) {
    let bus = Arc::new(EventBus::default());
    let events = bus.subscribe();
    let (_handle, signal) = cancellation();

    let ctx = SessionContext::new(
        SessionId::new("session-under-test"),
        std::env::temp_dir(),
        bus,
        AgentTimeouts::default(),
        PermissionConfig::default(),
        signal,
    );

    (ctx, events)
}

#[test]
fn a_session_that_never_reported_usage_reads_as_unknown() {
    // The PTY transport is exactly this: it cannot measure and never will.
    let (ctx, _events) = context();
    assert_eq!(ctx.usage(), None);
}

#[test]
fn a_reading_is_kept_and_published() {
    let (ctx, mut events) = context();

    ctx.record_usage(SessionUsage {
        used: 12_000,
        size: 200_000,
        cost: None,
    });

    let latest = ctx.usage().expect("recorded");
    assert_eq!(latest.used, 12_000);
    assert_eq!(latest.size, 200_000);

    let envelope = events.try_recv().expect("published");
    let Event::AgentUsage { usage, .. } = envelope.event else {
        panic!("expected a usage event");
    };
    assert_eq!(usage.used, 12_000);
}

#[test]
fn the_latest_reading_replaces_the_one_before_it() {
    // Latest value rather than a history: usage arrives every turn, and keeping
    // each reading would grow without bound for the length of a session.
    let (ctx, _events) = context();

    for used in [1_000, 5_000, 9_000] {
        ctx.record_usage(SessionUsage {
            used,
            size: 200_000,
            cost: None,
        });
    }

    assert_eq!(ctx.usage().expect("recorded").used, 9_000);
}

#[test]
fn a_reading_without_a_cost_still_carries_its_token_counts() {
    // Not every agent reports money, and losing the tokens because the cost was
    // absent would be the wrong trade.
    let (ctx, _events) = context();

    ctx.record_usage(SessionUsage {
        used: 400,
        size: 8_000,
        cost: None,
    });

    let latest = ctx.usage().expect("recorded");
    assert_eq!(latest.used, 400);
    assert!(latest.cost.is_none());
}

#[test]
fn cost_survives_as_an_integer_rather_than_a_float() {
    // The protocol sends an f64. Money in binary floating point is a bug waiting
    // for a total, so it is stored in millionths.
    let money = Money::from_amount(1.23, "USD");

    assert_eq!(money.micros, 1_230_000);
    assert_eq!(money.currency, "USD");
}

#[test]
fn a_fractional_cost_rounds_rather_than_truncating() {
    // 0.0000005 lands exactly between two micros; truncating would silently lose
    // every sub-micro charge in a long session.
    assert_eq!(Money::from_amount(0.000_000_5, "USD").micros, 1);
    assert_eq!(Money::from_amount(0.000_000_4, "USD").micros, 0);
}

#[test]
fn the_context_fraction_is_a_ratio_of_used_to_size() {
    let usage = SessionUsage {
        used: 50_000,
        size: 200_000,
        cost: None,
    };

    assert_eq!(usage.fraction(), Some(0.25));
}

#[test]
fn an_unknown_window_size_yields_no_fraction_rather_than_zero() {
    // A progress bar drawn at 0% says "plenty of room", which is the opposite of
    // "we do not know how much room there is".
    let usage = SessionUsage {
        used: 900,
        size: 0,
        cost: None,
    };

    assert_eq!(usage.fraction(), None);
}

#[test]
fn a_usage_event_keeps_its_wire_shape() {
    // `ket events --follow` emits these as JSON lines and people build on that
    // shape, so it is an interface rather than an internal.
    let event = Event::AgentUsage {
        session_id: SessionId::new("s1"),
        usage: SessionUsage {
            used: 10,
            size: 100,
            cost: Some(Money {
                micros: 2_500_000,
                currency: "USD".to_owned(),
            }),
        },
    };

    let json = serde_json::to_value(&event).expect("serialises");

    assert_eq!(json["type"], "agentUsage");
    assert_eq!(json["sessionId"], "s1");
    assert_eq!(json["usage"]["used"], 10);
    assert_eq!(json["usage"]["size"], 100);
    assert_eq!(json["usage"]["cost"]["micros"], 2_500_000);
    assert_eq!(json["usage"]["cost"]["currency"], "USD");
}
