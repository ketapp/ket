//! OpenCode's hooks: a plugin ket writes into OpenCode's plugin folder,
//! which posts the session's events itself.

use std::path::{Path, PathBuf};

use super::places::Place;
use super::{HookInstaller, home};
use crate::{KetError, Result};

// ---- opencode ---------------------------------------------------------------

/// OpenCode: a plugin of ket's own in `~/.config/opencode/plugin/`.
///
/// OpenCode has no hook commands; it has plugins, loaded from that directory
/// with no entry in its config, so ket's is a file of its own and the
/// person's `opencode.json` is never touched. It posts the same envelope as
/// [`SCRIPT`](super::script::SCRIPT), with a payload in Claude Code's shape — `hook_event_name`,
/// `session_id`, `tool_name`, `tool_input` — so everything that reads a
/// report reads OpenCode's unchanged. Checked against OpenCode 1.18.33's and
/// 2.0.24's source, and every path below driven end to end against 2.0.24:
///
/// - `chat.message` (OpenCode 1) and the `prompt` hook (OpenCode 2) are
///   `UserPromptSubmit`; `tool.execute.before` / `after` are `PreToolUse` /
///   `PostToolUse`, with OpenCode's tool names spelled as Claude's so rows
///   read the same. OpenCode 2 renamed the tools — `bash` became `shell`,
///   `task` became `subagent` — so each half of the plugin carries its own
///   map. OpenCode 2's after-hook also says whether the tool failed, which
///   OpenCode 1's never did, so it can report `PostToolUseFailure`.
/// - A turn's end is where the versions differ most. OpenCode 1 had
///   `session.status` going idle and `session.error`; OpenCode 2 has
///   `session.execution.succeeded` / `failed` / `interrupted`, which are
///   `Stop` / `StopFailure` / `Interrupt`. (`session.status` is matched
///   too, for a build that publishes it.)
/// - The `permission.asked` event is `PermissionRequest`. There is no hook
///   for the answer, so `permission.replied` is reported as the tool
///   starting — otherwise a prompt answered on the desktop would stay open
///   on the phone until the tool finished. A rejection ends the turn.
///   OpenCode 2's permission carries an action and resources rather than
///   the metadata 1's did, which is where the command a shell wants and the
///   file an edit wants come from.
/// - OpenCode 1's question tool raised `question.asked`; OpenCode 2 asks as
///   a form. Either way it is a `Notification` of a person being needed,
///   with no question a phone could answer, and the reply is the tool going
///   on. A rejected question ends the turn, as a rejected permission does.
/// - `Stop` carries the text the session last said, as Claude's
///   `last_assistant_message`, so a finished row has a closing line.
/// - A subagent's session (one with a parent) is left out altogether: its
///   prompt's Reject opens a feedback box rather than rejecting, so a phone
///   must not answer it, and its tools are not the pane's.
///
/// **Where the plugin runs is the pane's server, and OpenCode 2 moved it.**
/// OpenCode 1 was one process: the terminal UI *was* the server, and the
/// plugin saw the environment ket had injected into the pane. OpenCode 2
/// splits them: a TUI is a client of one shared background service, and
/// plugins run in the service — whose environment is a snapshot of whoever
/// started it, not of any one pane. Reports would name the wrong pane, or
/// none at all. So the pane's launch line passes `--standalone`
/// (see [`crate::config::default_agents`]), giving the pane a private server
/// and the plugin the pane's environment back. `opencode acp`, which the
/// sidebar's sessions speak, already runs a private server per process.
///
/// Inert outside a terminal ket opened: without ket's launch token in the
/// environment it reports nothing.
pub struct OpenCodeHooks {
    /// The plugin file, overridable so this is exercisable without a home.
    plugin: PathBuf,
}

impl Default for OpenCodeHooks {
    fn default() -> Self {
        // OpenCode reads `$XDG_CONFIG_HOME/opencode`, else `~/.config/opencode`.
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home().join(".config"), PathBuf::from);
        Self {
            plugin: config.join("opencode").join("plugin").join("ket.js"),
        }
    }
}

impl OpenCodeHooks {
    /// The plugin written to a specific file.
    pub fn at(plugin: PathBuf) -> Self {
        Self { plugin }
    }
}

impl HookInstaller for OpenCodeHooks {
    fn agent(&self) -> &str {
        "opencode"
    }

    fn is_installed(&self) -> Result<bool> {
        match std::fs::read_to_string(&self.plugin) {
            Ok(text) => Ok(text == OPENCODE_PLUGIN),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KetError::io(&self.plugin, e)),
        }
    }

    /// Writes the plugin. The hook script is not used: the plugin posts
    /// itself.
    fn install(&self, _script: &Path) -> Result<()> {
        if let Some(dir) = self.plugin.parent() {
            std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
        }
        std::fs::write(&self.plugin, OPENCODE_PLUGIN).map_err(|e| KetError::io(&self.plugin, e))
    }

    fn place(&self) -> Place {
        Place::new("opencode", &self.plugin)
    }

    fn has_any(&self) -> Result<bool> {
        match std::fs::read_to_string(&self.plugin) {
            Ok(text) => Ok(text.starts_with(OPENCODE_PLUGIN_MARK)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KetError::io(&self.plugin, e)),
        }
    }

    /// Deletes the plugin — only if it is ket's.
    fn remove(&self) -> Result<()> {
        match std::fs::read_to_string(&self.plugin) {
            Ok(text) if text.starts_with(OPENCODE_PLUGIN_MARK) => {
                std::fs::remove_file(&self.plugin).map_err(|e| KetError::io(&self.plugin, e))
            }
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KetError::io(&self.plugin, e)),
        }
    }
}

/// The plugin's first line, which says a file is ket's to remove.
const OPENCODE_PLUGIN_MARK: &str = "// ket-opencode-plugin";

/// OpenCode's plugin — see [`OpenCodeHooks`]. One file, two plugin APIs:
/// OpenCode 1 calls the default export's `server`, OpenCode 2 its `setup`,
/// and neither reads the other's half, so one install serves both. OpenCode
/// 1 requires `server` to be a function and ignores members it does not
/// know; OpenCode 2 requires `id` and `setup` (or `effect`) and ignores
/// `server`. Every post is queued, so reports keep their order, and none is
/// awaited by a hook OpenCode waits on.
const OPENCODE_PLUGIN: &str = r#"// ket-opencode-plugin — installed by ket. Reports an OpenCode session's
// lifecycle back to ket, so its sidebar and a paired phone see what it is
// doing. Inert outside a terminal ket opened. Deleting this file uninstalls it.
//
// One file, two plugin APIs: OpenCode 1 calls `server`, OpenCode 2 calls
// `setup`, and neither reads the other's half. Both share the wire — the
// envelope, where it is posted, and the queue that keeps reports in order.
import { appendFileSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

// Claude Code's names for OpenCode 1's tools, so a row reads the same
// whichever agent it is. OpenCode 2 renamed the tools and has a map of its
// own, inside `setup`.
const TOOLS = {
  bash: "Bash", edit: "Edit", write: "Write", read: "Read", glob: "Glob",
  grep: "Grep", list: "LS", webfetch: "WebFetch", websearch: "WebSearch",
  task: "Task", todowrite: "TodoWrite",
};
// Only the fields a report reads: a file's contents would be the body.
const pick = (args) => {
  const a = args || {};
  const out = {};
  for (const key of ["command", "description", "pattern", "path", "url", "query"]) {
    if (typeof a[key] === "string") out[key] = a[key];
  }
  if (typeof a.filePath === "string") out.file_path = a.filePath;
  return out;
};

// Everything that posts a report, bound to one project directory. `null`
// when ket did not launch this OpenCode — without the launch token there is
// nothing to report and nowhere the report would be believed.
const wire = (directory) => {
  const env = process.env;
  const token = env.KET_LAUNCH_TOKEN;
  if (!token) return null;
  const home = env.HOME || "";
  const hooks = join(home, ".ket", "agent-hooks");

  const envelope = (to, payload) => JSON.stringify({
    agent: env.KET_AGENT_NAME || "opencode",
    worktree: env.KET_WORKTREE_ID || "",
    pane: env.KET_PANE_KEY || "",
    token: to,
    version: env.KET_HOOK_VERSION || "1",
    payload,
  });
  const hostEndpoint = () => {
    try {
      const [port, hostToken] = readFileSync(join(hooks, "host-endpoint"), "utf8").split("\n");
      return port && hostToken ? { port, token: hostToken } : null;
    } catch {
      return null;
    }
  };
  const post = async (target, payload) => {
    if (!target || !target.port || !target.token) return false;
    try {
      const response = await fetch(`http://127.0.0.1:${target.port}/hook`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: envelope(target.token, payload),
        signal: AbortSignal.timeout(1000),
      });
      return response.ok;
    } catch {
      return false;
    }
  };
  // A prompt held open at the host until a phone answers it, or until it is
  // answered here and ket lets go: what comes back is the decision, in
  // Claude's own shape, or nothing. Not queued — it waits for a person.
  const ask = async (payload) => {
    const port = env.KET_HOOK_PORT;
    if (!port || !token) return "";
    try {
      const response = await fetch(`http://127.0.0.1:${port}/hook/await`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: envelope(token, { hook_event_name: "PermissionRequest", cwd: directory, ...payload }),
        signal: AbortSignal.timeout(3570 * 1000),
      });
      return response.ok ? await response.text() : "";
    } catch {
      return "";
    }
  };
  // In order, the host first, then the window; kept for later if neither.
  // A pane-bound launch's own token is already correct, so it goes first —
  // the discovery file never carries a pane-specific token and would only
  // cost a round trip that never validates for one.
  let queue = Promise.resolve();
  const report = (event, fields) => {
    const payload = { hook_event_name: event, cwd: directory, ...fields };
    queue = queue.then(async () => {
      const inherited = { port: env.KET_HOOK_PORT, token };
      const host = hostEndpoint();
      const [first, second] = env.KET_PANE_KEY ? [inherited, host] : [host, inherited];
      if (await post(first, payload)) return;
      if (await post(second, payload)) return;
      // Capped at 8 MiB, as the hook script's is: a ket dragged to the Trash
      // with this plugin still in place never comes back to read it.
      try {
        mkdirSync(hooks, { recursive: true, mode: 0o700 });
        const spool = join(hooks, "spool.jsonl");
        let size = 0;
        try { size = statSync(spool).size; } catch {}
        if (size < 8 * 1024 * 1024) {
          appendFileSync(spool, envelope(host ? host.token : token, payload) + "\n", { mode: 0o600 });
        }
      } catch {}
    });
  };
  return { report, ask };
};

// The question tool's form as Claude's AskUserQuestion asks it, so the phone
// draws the same card: one question per field, its options by label. A
// field with nothing to pick from — free text, a browser step — can't be
// answered from a card, so a form with one is left to the terminal.
const questionsOf = (form) => {
  const questions = [];
  for (const field of form.fields || []) {
    const question = field.title || field.key;
    if (!question) return null;
    if (field.type === "boolean") {
      questions.push({ question, header: field.key || "", multiSelect: false,
        options: [{ label: "Yes", description: "" }, { label: "No", description: "" }] });
      continue;
    }
    const options = (field.options || [])
      .map((o) => ({ label: String(o.label ?? o.value ?? ""), description: String(o.description || "") }))
      .filter((o) => o.label);
    if (options.length === 0) return null;
    questions.push({ question, header: field.key || "", options, multiSelect: field.type === "multiselect" });
  }
  return questions.length > 0 ? questions : null;
};

// The phone's answers — each question's text to what was picked, a
// multi-select's picks joined with ", " — back as the form's answer: each
// field's key to its option's value, or the text itself where it is the
// person's own.
const answerOf = (form, answers) => {
  const answer = {};
  for (const field of form.fields || []) {
    const said = answers[field.title || field.key];
    if (typeof said !== "string") return null;
    const valueOf = (label) => {
      const option = (field.options || []).find((o) => String(o.label ?? o.value) === label);
      return option ? option.value : label;
    };
    if (field.type === "boolean") answer[field.key] = said === "Yes";
    else if (field.type === "multiselect") answer[field.key] = said.split(", ").map(valueOf);
    else answer[field.key] = valueOf(said);
  }
  return answer;
};

export default {
  id: "ket",

  // ---- OpenCode 1 ----------------------------------------------------------
  async server({ directory }) {
    const w = wire(directory);
    if (!w) return {};
    const { report } = w;

    const children = new Set();
    const own = (session) => session && !children.has(session);
    const asked = new Map();
    // What each session last said, for its Stop to carry: the assistant's
    // messages by id, then their text parts as they are updated.
    const said = new Set();
    const last = new Map();
    const stop = (session) => {
      report("Stop", { session_id: session, last_assistant_message: last.get(session) });
      last.delete(session);
    };
    const question = (p) => {
      const m = p.metadata || {};
      const tool_input = {};
      if (typeof m.command === "string") tool_input.command = m.command;
      if (typeof m.filepath === "string") tool_input.file_path = m.filepath;
      if (typeof m.url === "string") tool_input.url = m.url;
      const pattern = (p.patterns || [])[0];
      if (!Object.keys(tool_input).length && pattern) tool_input.command = pattern;
      return { tool_name: TOOLS[p.permission] || p.permission, tool_input };
    };

    return {
      event: async ({ event }) => {
        const p = event.properties || {};
        switch (event.type) {
          case "session.created":
            if (p.info && p.info.parentID) children.add(p.info.id || p.sessionID);
            else report("SessionStart", { session_id: p.sessionID, source: "startup" });
            break;
          case "session.status":
            if (p.status && p.status.type === "idle" && own(p.sessionID)) stop(p.sessionID);
            break;
          case "session.error":
            if (own(p.sessionID)) stop(p.sessionID);
            break;
          // The end of a compaction; its start is the hook below.
          case "session.compacted":
            if (own(p.sessionID)) report("PostCompact", { session_id: p.sessionID });
            break;
          case "message.updated":
            if (p.info && p.info.role === "assistant" && own(p.sessionID)) {
              if (said.size > 1000) said.clear();
              said.add(p.info.id);
            }
            break;
          case "message.part.updated": {
            const part = p.part || {};
            if (part.type === "text" && !part.synthetic && said.has(part.messageID) && part.text) {
              last.set(p.sessionID, part.text);
            }
            break;
          }
          // The question tool waits on a person without a permission prompt:
          // a wait, not something a phone can allow.
          case "question.asked":
            if (own(p.sessionID)) {
              report("Notification", { session_id: p.sessionID, notification_type: "agent_needs_input" });
            }
            break;
          case "question.replied":
            if (own(p.sessionID)) report("PreToolUse", { session_id: p.sessionID, tool_name: "AskUserQuestion" });
            break;
          case "permission.asked":
            if (own(p.sessionID)) {
              const q = question(p);
              asked.set(p.id, q);
              report("PermissionRequest", { session_id: p.sessionID, ...q });
            }
            break;
          case "permission.replied": {
            const q = asked.get(p.requestID);
            asked.delete(p.requestID);
            if (q && p.reply !== "reject") report("PreToolUse", { session_id: p.sessionID, ...q });
            break;
          }
        }
      },
      "chat.message": async (input) => {
        if (own(input.sessionID)) report("UserPromptSubmit", { session_id: input.sessionID });
      },
      "experimental.session.compacting": async (input) => {
        if (own(input.sessionID)) report("PreCompact", { session_id: input.sessionID });
      },
      "tool.execute.before": async (input, output) => {
        if (!own(input.sessionID)) return;
        report("PreToolUse", {
          session_id: input.sessionID,
          tool_use_id: input.callID,
          tool_name: TOOLS[input.tool] || input.tool,
          tool_input: pick(output && output.args),
        });
      },
      "tool.execute.after": async (input, output) => {
        if (!own(input.sessionID)) return;
        // The output's size, never the output: how much a command cost the
        // context is worth a number, and the body would be the post.
        const text = output && output.output;
        report("PostToolUse", {
          session_id: input.sessionID,
          tool_use_id: input.callID,
          tool_name: TOOLS[input.tool] || input.tool,
          tool_input: pick(input.args),
          tool_response_bytes: typeof text === "string" ? text.length : undefined,
        });
      },
    };
  },

  // ---- OpenCode 2 ----------------------------------------------------------
  async setup(ctx) {
    const w = wire(ctx.location && ctx.location.directory);
    if (!w) return;
    const { report, ask } = w;
    // Where a form is answered: the SDK's session.form, wherever this
    // version of OpenCode hangs it on the context.
    const forms = () =>
      (ctx.client && ctx.client.session && ctx.client.session.form) ||
      (ctx.session && ctx.session.form) ||
      ctx.form ||
      null;

    // OpenCode 2's own tool names, which renamed the tools: `bash` became
    // `shell`, `task` became `subagent`. Claude's names, so a row reads the
    // same whichever agent it is. A permission action is the tool's name,
    // with `write` and `patch` both folding into `edit`.
    const TOOLS = {
      shell: "Bash", edit: "Edit", patch: "Edit", write: "Write", read: "Read",
      glob: "Glob", grep: "Grep", webfetch: "WebFetch", websearch: "WebSearch",
      subagent: "Task", question: "AskUserQuestion",
    };

    const children = new Set();
    const own = (session) => session && !children.has(session);
    const asked = new Map();
    // What each session last said, for its Stop to carry.
    const last = new Map();
    const stop = (session) => {
      report("Stop", { session_id: session, last_assistant_message: last.get(session) });
      last.delete(session);
    };
    // What a permission is about, from its action and resources: the
    // command a shell wants to run, the file an edit wants to touch.
    const question = (d) => {
      const name = TOOLS[d.action] || d.action;
      const first = (d.resources || [])[0];
      const input = {};
      if (typeof first === "string" && first) {
        if (name === "Bash") input.command = first;
        else input.file_path = first;
      }
      return { tool_name: name, tool_input: input };
    };

    await ctx.session.hook("prompt", (event) => {
      if (own(event.sessionID)) report("UserPromptSubmit", { session_id: event.sessionID });
    });
    await ctx.tool.hook("execute.before", (event) => {
      if (!own(event.sessionID)) return;
      report("PreToolUse", {
        session_id: event.sessionID,
        tool_use_id: event.id,
        tool_name: TOOLS[event.tool] || event.tool,
        tool_input: pick(event.input),
      });
    });
    await ctx.tool.hook("execute.after", (event) => {
      if (!own(event.sessionID)) return;
      // The output's size, never the output: how much a command cost the
      // context is worth a number, and the body would be the post.
      const content = event.status === "completed" && event.result ? event.result.content : undefined;
      report(event.status === "error" ? "PostToolUseFailure" : "PostToolUse", {
        session_id: event.sessionID,
        tool_use_id: event.id,
        tool_name: TOOLS[event.tool] || event.tool,
        tool_input: pick(event.input),
        tool_response_bytes: typeof content === "string" ? content.length
          : content ? JSON.stringify(content).length : undefined,
      });
    });

    // The bus: sessions, turns, prompts and questions, in OpenCode 2's
    // names. Everything the hooks above cannot see.
    const controller = new AbortController();
    void (async () => {
      for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
        const d = event.data || {};
        switch (event.type) {
          case "session.created":
            if (d.parentID) children.add(d.sessionID);
            else report("SessionStart", { session_id: d.sessionID, source: "startup" });
            break;
          // The turn's own begin and end. OpenCode 2 has no `session.status`:
          // `execution.succeeded` is the turn that finished, which is Stop.
          case "session.execution.succeeded":
            if (own(d.sessionID)) stop(d.sessionID);
            break;
          case "session.execution.failed":
            if (own(d.sessionID)) {
              report("StopFailure", { session_id: d.sessionID, error: d.error });
              last.delete(d.sessionID);
            }
            break;
          case "session.execution.interrupted":
            if (own(d.sessionID)) {
              report("Interrupt", { session_id: d.sessionID });
              last.delete(d.sessionID);
            }
            break;
          case "session.status":
            if (d.status && d.status.type === "idle" && own(d.sessionID)) stop(d.sessionID);
            break;
          // A compaction, from its start to its end either way.
          case "session.compaction.started":
            if (own(d.sessionID)) report("PreCompact", { session_id: d.sessionID });
            break;
          case "session.compaction.ended":
          case "session.compaction.failed":
            if (own(d.sessionID)) report("PostCompact", { session_id: d.sessionID });
            break;
          // What the session last said. OpenCode 2 ends each of the
          // assistant's text parts with its full value, which is all of it.
          case "session.text.ended":
            if (own(d.sessionID) && typeof d.text === "string") last.set(d.sessionID, d.text);
            break;
          // The answer is reported as the tool starting — otherwise a prompt
          // answered on the desktop would stay open on the phone until the
          // tool finished.
          case "permission.asked":
            if (own(d.sessionID)) {
              const q = question(d);
              asked.set(d.id, q);
              report("PermissionRequest", { session_id: d.sessionID, ...q });
            }
            break;
          case "permission.replied": {
            const q = asked.get(d.requestID);
            asked.delete(d.requestID);
            if (q && d.reply !== "reject") report("PreToolUse", { session_id: d.sessionID, ...q });
            break;
          }
          // The question tool asks as a form. A form whose owner is not a
          // session — `"global"`, an MCP elicitation — is nobody's pane. One
          // a card can answer goes to the host as Claude's question would,
          // and is held there until a phone answers it: the answer comes
          // back and replies to the form. Nothing back — answered here, or
          // the wait ran out — leaves the form as it was.
          case "form.created": {
            const f = d.form || {};
            if (!own(f.sessionID) || f.sessionID === "global") break;
            const questions = questionsOf(f);
            if (!questions) {
              report("Notification", { session_id: f.sessionID, notification_type: "agent_needs_input" });
              break;
            }
            const asked = { session_id: f.sessionID, tool_name: "AskUserQuestion", tool_input: { questions } };
            report("PermissionRequest", asked);
            void (async () => {
              let decided;
              try { decided = JSON.parse(await ask(asked)); } catch { return; }
              const output = decided && decided.hookSpecificOutput;
              const decision = output && output.decision;
              const answers = decision && decision.behavior === "allow" && decision.updatedInput && decision.updatedInput.answers;
              if (!answers) return;
              const answer = answerOf(f, answers);
              const api = forms();
              if (!answer || !api || typeof api.reply !== "function") return;
              try { await api.reply({ sessionID: f.sessionID, formID: f.id, answer }); } catch {}
            })();
            break;
          }
          case "form.replied":
            if (own(d.sessionID) && d.sessionID !== "global") {
              report("PreToolUse", { session_id: d.sessionID, tool_name: "AskUserQuestion" });
            }
            break;
        }
      }
    })().catch(() => {});

    return () => controller.abort();
  },
};
"#;
