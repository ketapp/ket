//! The relay as a process of its own, for any host that proves its id:
//! `ket-host` runs the same routing for itself alone when phones are turned
//! on — see the library.
//!
//! ```sh
//! ket-relay 127.0.0.1:7878 --health 127.0.0.1:7879
//! ```
//!
//! Sized for many hosts ([`ket_relay::Limits::SHARED`]), and meant to sit
//! behind a proxy on the same machine that ends TLS — see
//! `deploy/relay/README.md`. `SIGTERM` or Ctrl-C tells every phone and host
//! its connection has ended and exits once their sockets have closed.

use tokio::net::TcpListener;

fn usage() -> ! {
    eprintln!("usage: ket-relay [listen address] [--health address]");
    std::process::exit(2);
}

async fn listen(address: &str) -> TcpListener {
    match TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("ket-relay: cannot listen on {address}: {error}");
            std::process::exit(1);
        }
    }
}

/// Resolves on `SIGTERM` or Ctrl-C.
async fn stopped() {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(error) => {
                eprintln!("ket-relay: cannot watch for SIGTERM: {error}");
                std::process::exit(1);
            }
        };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    tracing::info!("relay stopping");
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let mut address = None;
    let mut health = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--health" => health = Some(args.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            _ if arg.starts_with('-') || address.is_some() => usage(),
            _ => address = Some(arg),
        }
    }
    let address = address.unwrap_or_else(|| "127.0.0.1:7878".to_owned());
    let listener = listen(&address).await;
    let health = match &health {
        Some(health) => Some(listen(health).await),
        None => None,
    };
    tracing::info!(%address, health = health.is_some(), "relay listening");
    ket_relay::serve_until(listener, None, ket_relay::Limits::SHARED, health, stopped()).await;
}
