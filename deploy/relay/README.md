# ket's relay

What a phone or iPad uses to reach its Mac when it is on neither the Mac's
network nor its tailnet. The relay forwards Noise ciphertext between a device
and a ket host. It cannot read it, and it keeps no record of what it
forwarded: it logs counts and connection events, never ids or addresses.

A host only registers here by proving it holds the key its id comes from, so
nobody can take a Mac's place. Devices pin their Mac's key, so a relay that
routed wrongly would get nobody anything but a failed handshake.

## What runs where

- `ket-relay` listens on `127.0.0.1:7878` for WebSockets, and answers a health
  check on `127.0.0.1:7879`, both on loopback only.
- Caddy listens on 443 for `relay.ketapp.dev`, ends TLS and passes WebSockets
  to the relay, writing `X-Forwarded-For` so the relay's per-address limits
  count each client rather than Caddy.

Any small Linux VPS does. Sockets are cheap, and the relay holds at most
512 MiB of queued frames for everyone together
(`ket_relay::Limits::SHARED`).

## First start

1. Point `relay.ketapp.dev` at the VPS: an `A` record, and `AAAA` if it has
   IPv6. Open ports 80 and 443; Caddy needs 80 to get its certificate.
2. Install Caddy from its own packages (caddyserver.com/docs/install).
3. Build the relay on the VPS from a checkout of ket:

   ```sh
   cargo build --release --locked -p ket-relay
   sudo install -m 755 target/release/ket-relay /usr/local/bin/ket-relay
   ```

4. Install the unit and the Caddyfile, and start both:

   ```sh
   sudo cp deploy/relay/ket-relay.service /etc/systemd/system/
   sudo cp deploy/relay/Caddyfile /etc/caddy/Caddyfile
   sudo systemctl daemon-reload
   sudo systemctl enable --now ket-relay
   sudo systemctl reload caddy
   ```

5. Check it:

   ```sh
   curl -s http://127.0.0.1:7879/    # ok hosts=0 phones=0
   journalctl -u ket-relay -f
   ```

   Then on a Mac, turn on Settings › Devices › Reach it from anywhere. The
   row says Connected once the Mac has registered, and the health check
   counts it.

## Rolling a release

Build the new binary, install it over the old one, and restart:

```sh
cargo build --release --locked -p ket-relay
sudo install -m 755 target/release/ket-relay /usr/local/bin/ket-relay
sudo systemctl restart ket-relay
```

A restart tells every device and host its connection has ended. Hosts dial
back within their backoff, at most 30 seconds, and devices reconnect and
resume where they were.
