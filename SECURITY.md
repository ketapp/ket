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
require desktop-approved pairing, and receive an explicit role. The local relay
can observe connection metadata and ciphertext sizes, but not application
plaintext. Revoking a device invalidates its grant and active sessions.

Residual risks include compromise of the user's account, malicious tools run
by an already-authorized agent, denial of service within configured bounds,
and development deployments that weaken normal transport assumptions.

## Safe research

Use repositories and devices you own, disposable `XDG_DATA_HOME` directories,
and test credentials. Do not access other users' data, degrade shared systems,
or retain sensitive information. Good-faith research following these rules is
welcome.
