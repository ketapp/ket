//! The feedback service over HTTP, in log mode.
//!
//! `serve` runs on a loopback port picked by the OS, and each test speaks
//! plain HTTP/1.1 to it over a `TcpStream`, the way any client would. Log
//! mode sends nothing anywhere: an accepted report is only printed.

use std::io::{Read, Write};
use std::net::TcpStream;

use ket_feedback::{Config, Delivery, serve};

/// A running service and its address. The runtime goes with it.
struct Service {
    address: std::net::SocketAddr,
    _runtime: tokio::runtime::Runtime,
}

impl Service {
    fn start(trust_proxy: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let address = listener.local_addr().unwrap();
        runtime.spawn(serve(
            listener,
            Config {
                delivery: Delivery::Log,
                trust_proxy,
            },
        ));
        Self {
            address,
            _runtime: runtime,
        }
    }

    /// Sends one raw request and returns `(status, headers, body)`.
    fn request(&self, raw: &str) -> (u16, String, String) {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, head.to_owned(), body.to_owned())
    }

    fn post(&self, body: &str, extra_headers: &str) -> (u16, String, String) {
        self.request(&format!(
            "POST /v1/feedback HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n{extra_headers}\r\n{body}",
            body.len()
        ))
    }
}

fn report(install: &str) -> String {
    serde_json::json!({
        "kind": "idea",
        "message": "Show the savings per project.",
        "reply_to": null,
        "app_version": "0.1.0",
        "os": null,
        "install": install,
        "unknown_field": "ignored"
    })
    .to_string()
}

fn install(n: u32) -> String {
    format!("{n:032x}")
}

#[test]
fn the_health_check_answers_ok() {
    let service = Service::start(false);
    let (status, head, body) = service.request("GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status, 200);
    assert_eq!(body, "ok");
    assert!(head.to_ascii_lowercase().contains("text/plain"));

    let (status, head, _) =
        service.request("POST /healthz HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n");
    assert_eq!(status, 405);
    assert!(head.to_ascii_lowercase().contains("allow: get, head"));
}

#[test]
fn a_report_is_accepted_once_and_the_second_from_the_install_waits_a_week() {
    let service = Service::start(false);
    let (status, _, body) = service.post(&report(&install(1)), "");
    assert_eq!(status, 202, "{body}");
    assert_eq!(body, r#"{"ok":true}"#);

    let (status, head, body) = service.post(&report(&install(1)), "");
    assert_eq!(status, 429, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(parsed["error"].as_str().unwrap().contains("7 days"));
    let wait = parsed["retry_after_secs"].as_u64().unwrap();
    assert!(wait > 6 * 24 * 60 * 60, "{wait}");
    assert!(head.to_ascii_lowercase().contains("retry-after:"));
}

#[test]
fn one_network_gets_ten_reports_a_day() {
    let service = Service::start(false);
    for n in 0..10 {
        let (status, _, body) = service.post(&report(&install(100 + n)), "");
        assert_eq!(status, 202, "report {n}: {body}");
    }
    let (status, _, body) = service.post(&report(&install(200)), "");
    assert_eq!(status, 429);
    assert!(body.contains("this network"), "{body}");
}

#[test]
fn behind_a_trusted_proxy_the_forwarded_address_is_counted() {
    let service = Service::start(true);
    for n in 0..10 {
        let (status, _, _) = service.post(
            &report(&install(300 + n)),
            "X-Forwarded-For: 198.51.100.1, 10.0.0.1\r\n",
        );
        assert_eq!(status, 202);
    }
    // Another client behind the same proxy is still let in.
    let (status, _, body) =
        service.post(&report(&install(400)), "X-Forwarded-For: 198.51.100.2\r\n");
    assert_eq!(status, 202, "{body}");
}

#[test]
fn a_report_out_of_contract_is_a_400_with_a_sentence() {
    let service = Service::start(false);
    let cases = [
        ("not json", "could not be read as JSON"),
        (r#"{"kind": "bug"}"#, "The message is empty."),
        (
            &serde_json::json!({"kind": "bug", "message": "x", "app_version": "1",
                                "install": "short"})
            .to_string(),
            "32 lowercase hexadecimal",
        ),
    ];
    for (body, sentence) in cases {
        let (status, _, answer) = service.post(body, "");
        assert_eq!(status, 400, "{body}");
        assert!(answer.contains(sentence), "{body}: {answer}");
    }
}

#[test]
fn a_report_must_be_json_posted_and_small() {
    let service = Service::start(false);

    let body = report(&install(500));
    let (status, _, answer) = service.request(&format!(
        "POST /v1/feedback HTTP/1.1\r\nHost: x\r\nContent-Type: text/plain\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    assert_eq!(status, 400);
    assert!(answer.contains("sent as JSON"), "{answer}");

    let (status, head, _) = service.request("GET /v1/feedback HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status, 405);
    assert!(head.to_ascii_lowercase().contains("allow: post"));

    // Turned away on its declared length, before the body is read.
    let (status, _, answer) = service.request(
        "POST /v1/feedback HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
         Content-Length: 40000\r\n\r\n",
    );
    assert_eq!(status, 413);
    assert!(answer.contains("32 KiB"), "{answer}");

    let (status, _, _) = service.request("GET /elsewhere HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_eq!(status, 404);
}
