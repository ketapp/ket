//! The hook script the agents run: where it lives, where it spools a report
//! it can't deliver, and how it finds the host.

use std::path::{Path, PathBuf};

use super::home;
use crate::{KetError, Result};

// ---- the script -------------------------------------------------------------

/// Where ket keeps the script agents call.
pub fn script_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("ket-agent-hook.sh")
}

/// The spool a report falls back to when ket is not listening.
pub fn spool_path() -> PathBuf {
    home().join(".ket").join("agent-hooks").join("spool.jsonl")
}

/// Where the terminal host publishes its current hook port and token.
pub fn host_endpoint_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("host-endpoint")
}

/// Publishes the terminal host's current hook endpoint for resumed sessions.
///
/// Codex restores environment variables from the original session when it
/// runs hooks after `codex resume`. The script therefore cannot rely on the
/// port and token it inherited, even though the resumed process itself was
/// launched with the new values. This owner-only file is the live authority;
/// the inherited values remain the fallback for local, host-disabled runs.
pub fn publish_host_endpoint(port: u16, token: &str) -> Result<()> {
    let path = host_endpoint_path();
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("hook endpoint has no directory".to_owned()))?;
    private_dir(dir)?;
    let temporary = dir.join(format!(".host-endpoint-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    // Owner-only from the moment it exists — written and then narrowed, it
    // could be read in between.
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|e| KetError::io(&temporary, e))?;
        file.write_all(format!("{port}\n{token}\n").as_bytes())
            .map_err(|e| KetError::io(&temporary, e))?;
    }
    std::fs::rename(&temporary, &path).map_err(|e| KetError::io(&path, e))
}

/// A token for a hook listener: 32 random bytes, in hex. What a report must
/// carry to be believed, so not something that can be worked out from a
/// process id and the time.
pub fn new_token() -> String {
    ket_remote::secret()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Makes `dir` if it is missing, and keeps it the owner's alone: it holds
/// the host's hook token and the spool of reports.
fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| KetError::io(dir, e))?;
    }
    Ok(())
}

/// Writes the hook script, creating its directory.
///
/// Rewritten on every install rather than only when absent: a ket that has been
/// upgraded should not leave last version's script behind, and the file is the
/// contract between the two.
pub fn install_script() -> Result<PathBuf> {
    let path = script_path();
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("hook script has no directory".to_owned()))?;
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    std::fs::write(&path, SCRIPT).map_err(|e| KetError::io(&path, e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)
            .map_err(|e| KetError::io(&path, e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).map_err(|e| KetError::io(&path, e))?;
    }

    Ok(path)
}

/// The script every agent's hook runs.
///
/// Three properties it must have, and all three are why this is not one line
/// of curl:
///
/// 1. **It can never break the agent.** Whatever happens — no ket, no curl, no
///    network, a full disk — it prints `{}` and exits 0. A hook that fails is a
///    hook that stops a person's session.
/// 2. **It never blocks.** Connect and total timeouts are one second; an agent
///    must not wait on ket to think.
/// 3. **It does not lose reports.** A failed post is appended to the spool
///    instead, for ket to pick up when it next starts — up to 8 MiB, past
///    which a ket that never came back would only fill a disk. A report ket
///    would drop on reading the spool is not written: one from an agent ket
///    did not launch, other than Codex's, which are placed by directory.
///
/// On `SessionStart` it does one more thing: it hands Claude Code back
/// whatever [`crate::worktree::TOKEN_REDUCTION_NOTE_ENV`] holds as
/// `hookSpecificOutput.additionalContext`, telling it how tersely to work for
/// the rest of the session, followed by [`CLI_NOTE_ENV`]'s word on the `ket`
/// command. ket set both variables on the launched process — the script
/// deliberately knows nothing about which level means what, so the table in
/// [`crate::worktree`] stays the only copy. That field is the one place a
/// `SessionStart` hook can talk back; every other event still gets the same
/// inert `{}`.
///
/// On `PostToolUse` for the shell tool it does one more, and only when the
/// launch set [`crate::filter::ENV`]: it hands the payload to that binary's
/// `--filter-hook` and, if anything comes back, answers Claude Code with the
/// replacement output instead of `{}` and tells ket how much the model will
/// read (`shown`, in the envelope). Nothing back — no binary, a failed run,
/// a command with no recipe — is the same `{}` as ever. See
/// [`crate::filter`].
///
/// On `PreToolUse` and `PermissionRequest` it does the same with
/// [`crate::permissions::ENV`]'s `--permission-hook`: a rule's deny or ask
/// before the tool runs, its allow once Claude would ask, carried in the
/// envelope as `ruling`. Questions and plans are never ruled.
pub(super) const SCRIPT: &str = r#"#!/bin/sh
# ket-agent-hook — installed by ket. Reports an agent's lifecycle back to it,
# and on session start, tells Claude how tersely this worktree asked it to work
# and that the `ket` command is there to drive ket with.
#
# Removing ket's entries from the agent's settings is what uninstalls this;
# the file itself is inert without them.

payload=$(cat 2>/dev/null || printf '')

# Nothing to say, and nothing that could go wrong from here on.
if [ -z "$payload" ]; then printf '{}\n'; exit 0; fi

# Grok runs Claude Code's hooks as well as its own, so ket's Claude entry
# fires inside Grok too. Only the entry ket wrote for Grok, which passes
# "grok", reports from there; the other would claim to be Claude.
if [ -n "${GROK_HOOK_EVENT:-}" ] && [ "${1:-}" != "grok" ]; then printf '{}\n'; exit 0; fi
agent="${1:-${KET_AGENT_NAME:-claude}}"

escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# A successful shell command's output, trimmed before the model reads it —
# only when the launch named a ket binary to ask, and only for Claude, whose
# PostToolUse hook can replace a tool's result. Decided before the report
# below, so the report can say how much the model will read. Any failure
# leaves both empty, and the output alone.
shown=
decision=
if [ -n "${KET_COMMAND_FILTER:-}" ] && [ "$agent" = claude ] && [ -x "$KET_COMMAND_FILTER" ]; then
  case "$payload" in
    *'"hook_event_name":"PostToolUse"'*)
      case "$payload" in
        *'"tool_name":"Bash"'*)
          out=$(printf '%s' "$payload" | "$KET_COMMAND_FILTER" --filter-hook 2>/dev/null) || out=
          if [ -n "$out" ]; then
            shown=$(printf '%s\n' "$out" | sed -n '1p')
            decision=$(printf '%s\n' "$out" | sed '1d')
          fi
          ;;
      esac
      ;;
  esac
fi

# A permission rule's ruling on a tool call — only when the launch named a ket
# binary to ask, only for Claude, and never for a question or a plan, which
# are a person's to answer. Decided before the report below, so the report can
# say what ket ruled. Any failure leaves both empty, and the call to Claude.
ruling=
verdict=
if [ -n "${KET_PERMISSION_HOOK:-}" ] && [ "$agent" = claude ] && [ -x "$KET_PERMISSION_HOOK" ]; then
  case "$payload" in
    *'"hook_event_name":"PreToolUse"'*|*'"hook_event_name":"PermissionRequest"'*)
      case "$payload" in
        *'"tool_name":"AskUserQuestion"'*|*'"tool_name":"ExitPlanMode"'*) ;;
        *)
          out=$(printf '%s' "$payload" | "$KET_PERMISSION_HOOK" --permission-hook 2>/dev/null) || out=
          if [ -n "$out" ]; then
            ruling=$(printf '%s\n' "$out" | sed -n '1p')
            verdict=$(printf '%s\n' "$out" | sed '1d')
          fi
          ;;
      esac
      ;;
  esac
fi

envelope() {
  printf '{"agent":"%s","worktree":"%s","pane":"%s","token":"%s","version":"%s","shown":%s,"ruling":%s,"payload":%s}' \
    "$(escape "$agent")" \
    "$(escape "${KET_WORKTREE_ID:-}")" \
    "$(escape "${KET_PANE_KEY:-}")" \
    "$(escape "$1")" \
    "$(escape "${KET_HOOK_VERSION:-1}")" \
    "${shown:-null}" \
    "${ruling:-null}" \
    "$payload"
}

post() {
  [ -n "$1" ] && [ -n "$2" ] && command -v curl >/dev/null 2>&1 || return 1
  envelope "$2" | curl -fsS -o /dev/null \
      --connect-timeout 1 --max-time 1 \
      -H 'Content-Type: application/json' \
      --data-binary @- \
      "http://127.0.0.1:$1/hook" 2>/dev/null
}

delivered=0
endpoint="${HOME:-}/.ket/agent-hooks/host-endpoint"
host_port=
host_token=
if [ -n "${KET_LAUNCH_TOKEN:-}" ] && [ -n "${HOME:-}" ] && [ -r "$endpoint" ]; then
  host_port=$(sed -n '1p' "$endpoint" 2>/dev/null)
  host_token=$(sed -n '2p' "$endpoint" 2>/dev/null)
fi
# A pane-bound launch's own token is already the right one for the life of
# this process; trying the host-endpoint discovery file first would spend a
# round trip on every single report, since that file never carries a
# pane-specific token — only a launch with no pane key depends on it.
#
# `wait_port` and `wait_token` are the host's listener, when the report
# reached it, and what it took: where a question or a plan waits below. The
# host hands each terminal it runs its own port and a token bound to the
# pane, so the launch's port matching the discovery file's is the host —
# and the pane's token is the one it accepts. A window's listener is never
# waited on: phones answer through the host.
wait_port=
wait_token=
if [ -n "${KET_PANE_KEY:-}" ]; then
  if post "${KET_HOOK_PORT:-}" "${KET_LAUNCH_TOKEN:-}"; then
    delivered=1
    if [ -n "$host_port" ] && [ "${KET_HOOK_PORT:-}" = "$host_port" ]; then
      wait_port=$host_port
      wait_token=${KET_LAUNCH_TOKEN:-}
    fi
  elif post "$host_port" "$host_token"; then
    delivered=1
    wait_port=$host_port
    wait_token=$host_token
  fi
elif post "$host_port" "$host_token"; then
  delivered=1
  wait_port=$host_port
  wait_token=$host_token
elif post "${KET_HOOK_PORT:-}" "${KET_LAUNCH_TOKEN:-}"; then
  delivered=1
fi

# A question or a plan, reported to the host: held open there until a phone
# answers it, or until it is answered in the terminal and ket lets go. What
# comes back is Claude's decision, in Claude's own shape. Nothing back — an
# older host, a wait that ran out — leaves the prompt to the terminal, as if
# this had never asked. The wait is a little shorter than the entry's timeout.
if [ -n "$wait_port" ] && [ "$agent" = claude ]; then
  case "$payload" in
    *'"hook_event_name":"PermissionRequest"'*)
      case "$payload" in
        *'"tool_name":"AskUserQuestion"'*|*'"tool_name":"ExitPlanMode"'*)
          decision=$(envelope "$wait_token" | curl -sS \
              --connect-timeout 1 --max-time 3570 \
              -H 'Content-Type: application/json' \
              --data-binary @- \
              "http://127.0.0.1:$wait_port/hook/await" 2>/dev/null)
          if [ -n "$decision" ]; then printf '%s\n' "$decision"; exit 0; fi
          ;;
      esac
      ;;
  esac
fi

# Not delivered: keep it for whenever ket comes back — unless ket would only
# throw it away. An agent ket did not launch names no worktree, and only
# Codex's reports are placed by their directory instead; any other is dropped
# when the spool is read, so writing it would only grow the file.
keep=1
if [ -z "${KET_WORKTREE_ID:-}" ] && [ -z "${KET_LAUNCH_TOKEN:-}" ] && [ "$agent" != codex ]; then
  keep=0
fi
if [ "$delivered" -eq 0 ] && [ "$keep" -eq 1 ] && [ -n "${HOME:-}" ]; then
  envelope=$(envelope "${host_token:-${KET_LAUNCH_TOKEN:-}}")
  spool="${HOME}/.ket/agent-hooks/spool.jsonl"
  # Capped at 8 MiB, for a ket that never comes back to read it: one dragged
  # to the Trash with its hooks still in place. Past the cap a report is lost,
  # which costs a reopened ket only the status it would have shown for a while.
  size=0
  if [ -f "$spool" ]; then
    size=$( { wc -c < "$spool"; } 2>/dev/null | tr -d ' ')
  fi
  if [ "${size:-0}" -lt 8388608 ] 2>/dev/null; then
    # Owner-only: the spool holds the token each report carries.
    ( umask 077
      mkdir -p "${HOME}/.ket/agent-hooks" 2>/dev/null || :
      printf '%s\n' "$envelope" >> "$spool" 2>/dev/null || : )
  fi
fi

# Only a SessionStart hook's output can steer the session, and only a session
# ket handed a note has anything to say — the level-to-sentence table and the
# word on the CLI live in ket, not here, so there is one copy of each. Every
# other event, and no note, gets the same `{}` as before: this is additive,
# not a replacement for the report above.
case "$payload" in
  *'"hook_event_name":"SessionStart"'*)
    # Flattened before it is escaped: `escape` covers the backslashes and
    # quotes that JSON needs, and a newline in the note would otherwise be a
    # literal one inside a string — invalid JSON, and an error the reader sees
    # instead of a session.
    context=$(printf '%s' "${KET_TOKEN_REDUCTION_NOTE:-}" | tr '\n\r\t' '   ')
    cli=$(printf '%s' "${KET_CLI_NOTE:-}" | tr '\n\r\t' '   ')
    if [ -n "$cli" ]; then context="${context:+$context }$cli"; fi
    if [ -n "$context" ]; then
      # Each agent's own documented way of taking context from a hook, chosen
      # by the name ket launched it under. Claude Code reads a JSON object with
      # `additionalContext` in it; Codex adds a hook's plain stdout to the
      # session as developer context, and would have no idea what Claude's
      # envelope meant. Sending either one the other's shape is at best ignored
      # and at worst an error the reader sees instead of a session, so anything
      # ket does not recognise is told nothing at all.
      case "$agent" in
        claude)
          printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$(escape "$context")"
          ;;
        codex)
          printf '%s\n' "$context"
          ;;
        *)
          printf '{}\n'
          ;;
      esac
    else
      printf '{}\n'
    fi
    ;;
  *'"hook_event_name":"PostToolUse"'*)
    # The filter's answer, when there is one; the same `{}` when not.
    if [ -n "$decision" ]; then printf '%s\n' "$decision"; else printf '{}\n'; fi
    ;;
  *'"hook_event_name":"PreToolUse"'*|*'"hook_event_name":"PermissionRequest"'*)
    # A permission rule's answer, when one ruled; the same `{}` when not.
    if [ -n "$verdict" ]; then printf '%s\n' "$verdict"; else printf '{}\n'; fi
    ;;
  *)
    printf '{}\n'
    ;;
esac
exit 0
"#;
