//! The relay as a process of its own, for development: `ket-host` runs the
//! same routing itself when phones are turned on — see the library.
//!
//! ```sh
//! cargo run -p ket-relay -- 0.0.0.0:7979
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
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7878".to_owned());
    let listener = match TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("ket-relay: cannot listen on {address}: {error}");
            std::process::exit(1);
        }
    };
    tracing::info!(%address, "relay listening");
    ket_relay::serve(listener, None).await;
}
