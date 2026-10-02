//! The feedback receiver as a process — see the library for what it does and
//! the environment it reads.
//!
//! ```sh
//! cargo run -p ket-feedback                  # log mode on 127.0.0.1:7980
//! cargo run -p ket-feedback -- 0.0.0.0:7980  # listening for the network
//! ```

use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let config = match ket_feedback::Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ket-feedback: {error}");
            std::process::exit(2);
        }
    };
    let address = std::env::args()
        .nth(1)
        .or_else(|| {
            std::env::var("KET_FEEDBACK_LISTEN")
                .ok()
                .filter(|address| !address.trim().is_empty())
        })
        .unwrap_or_else(|| ket_feedback::DEFAULT_LISTEN.to_owned());
    let listener = match TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("ket-feedback: cannot listen on {address}: {error}");
            std::process::exit(1);
        }
    };
    tracing::info!("listening on {address}; {}", config.summary());
    tokio::select! {
        () = ket_feedback::serve(listener, config) => {}
        () = stopped() => tracing::info!("stopping"),
    }
}

/// Resolves on Ctrl-C, or on SIGTERM where there is one. A container's main
/// process gets no default handling for SIGTERM, so without this `docker
/// stop` would wait out its grace period and kill it.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
