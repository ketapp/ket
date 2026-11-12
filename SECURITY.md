# Security policy

## Supported versions

Security fixes are made on `main` and included in the next release. Only the
latest published release is supported.

## Reporting a vulnerability

Please do not open a public issue. Email hello@ketapp.dev, or use GitHub's
private vulnerability reporting for this repository (`Security` → `Advisories`
→ `Report a vulnerability`).

Include the affected revision, impact, reproduction steps, and any suggested
mitigation. Remove credentials, pairing codes, transcripts, and personal file
paths. You should receive an acknowledgement within seven days. Disclosure
timing will be coordinated after a fix is available.

## Security model

ket launches coding agents that can execute commands with the current user's
authority. A project and its automation must therefore be trusted before its
hooks or post-provision command are enabled. Worktrees isolate Git changes;
they are not operating-system sandboxes.

The desktop host uses owner-only local IPC and validates local peers for
privileged operations. Phone sessions use Noise encryption, pin the host key,
require desktop-approved pairing, and receive an explicit role. Revoking a
device invalidates its grant and active sessions.

Devices reach their Mac through a relay, which forwards Noise ciphertext and
cannot read it. Each host runs its own on the Mac, for the local network and
the tailnet, and it can observe connection metadata: which host id a device
asks for, when sockets open and close, and the size and timing of frames.

ket's own relay, for reaching a Mac from anywhere without Tailscale, is built
but not offered yet: no build links to `relay.ketapp.dev`. When it is, it
will see the same metadata plus each client's IP address, which it holds in
memory for its rate and socket limits and does not log; it logs counts and
reasons only, and Caddy in front of it keeps no access log.

A host registers with a relay elsewhere, ket's or one named in its config,
by proving it holds the static key its id is derived from: an X25519
agreement with a one-time key from the relay, over a fresh nonce. A device
pins its host's key, so a relay that routed it to the wrong host would get
nobody anything but a failed handshake. The proof does not name the relay it
is for, so a relay a host links to could pass on its challenge to another
relay and hold the host's place there, which denies the host's devices that
route but reveals nothing; a host links only to its own relay and the one in
its config.

Residual risks include compromise of the user's account, malicious tools run
by an already-authorized agent, denial of service within configured bounds,
and development deployments that weaken normal transport assumptions.

## Safe research

Use repositories and devices you own, disposable `XDG_DATA_HOME` directories,
and test credentials. Do not access other users' data, degrade shared systems,
or retain sensitive information. Good-faith research following these rules is
welcome.
