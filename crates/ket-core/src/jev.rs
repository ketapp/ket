//! Jev, TypeSafe's decision model, for the two calls ket hands it in beta.
//!
//! Jev is not a chat model. It is asked typed questions — pick one of these,
//! score on this scale — about some state, and answers each with a choice
//! and a calibrated confidence, in about a tenth of a second. ket asks it:
//!
//! - **which Economy level** suits a new task, from its prompt, choosing only
//!   among the active pack's own levels ([`economy_for`]);
//! - **how risky** a permission prompt is, from its tool and what it would
//!   run, for the phone to show beside Allow and Deny ([`risk_of`]). It
//!   informs; it never answers a prompt.
//!
//! Off until a key is saved in Settings → Economy. Every failure — no key, no
//! network, a refused key, a slow answer — is `None` to the caller, which
//! carries on as if Jev were not there.
//!
//! The request goes to `curl` the way [`crate::admin`]'s does: URL, key and
//! body on stdin as a curl config, so neither the key nor the prompt is ever
//! on a command line another process can read.

use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::JevConfig;
use crate::{KetError, Result};

/// Where every question goes.
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// How long a new task waits for its level before taking the default.
const ECONOMY_TIMEOUT: Duration = Duration::from_millis(2_500);

/// How long a risk read may take. Nobody waits on it: the phone shows the
/// prompt at once and the badge when it comes.
const RISK_TIMEOUT: Duration = Duration::from_secs(4);

/// Longest prompt or command sent, in characters. The start says what a task
/// is; the rest is detail a level does not turn on.
const STATE_MAX: usize = 4_000;

/// How risky a permission prompt reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Reads, or changes that are easily undone.
    Low,
    /// Changes the project's files.
    Medium,
    /// Destructive, outside the project, or reaching the network or secrets.
    High,
}

impl Risk {
    /// The word the phone shows.
    pub fn word(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        }
    }
}

/// The Economy level Jev picks for a task, and how sure it is — or `None`
/// when it is off, has no prompt to go on, or did not answer in time.
pub fn economy_for(config: &JevConfig, prompt: &str) -> Option<(u8, f32)> {
    if !config.enabled() || !config.auto_economy {
        return None;
    }
    let prompt = clip(prompt.trim());
    if prompt.is_empty() {
        return None;
    }
    let levels = crate::worktree::token_reduction_levels();
    let criteria: serde_json::Map<String, Value> = levels
        .iter()
        .map(|level| {
            (
                level.level.to_string(),
                Value::String(format!("{} — {}", level.label, level.description)),
            )
        })
        .collect();
    let questions = json!({
        "economy": {
            "type": "choice",
            "instructions": "A developer is starting a coding agent on this task. How much \
                token reduction suits it? Routine, well-scoped changes can take heavy \
                reduction; investigation, design and risky refactors need the agent to \
                reason and explain freely.",
            "criteria": criteria,
        }
    });
    let answers = evaluate(
        config,
        json!({ "task": prompt }),
        questions,
        ECONOMY_TIMEOUT,
    )
    .inspect_err(|e| tracing::info!("jev: no Economy level: {e}"))
    .ok()?;
    let answer = answers.get("economy")?;
    let level: u8 = answer.get("choice")?.as_str()?.parse().ok()?;
    let confidence = answer.get("confidence")?.as_f64()? as f32;
    levels
        .iter()
        .any(|l| l.level == level)
        .then_some((level, confidence))
}

/// How risky allowing `tool` to act on `subject` reads, and how sure Jev is.
pub fn risk_of(config: &JevConfig, tool: &str, subject: &str) -> Option<(Risk, f32)> {
    if !config.enabled() || !config.approval_risk {
        return None;
    }
    let questions = json!({
        "risk": {
            "type": "score",
            "instructions": "A coding agent is asking permission to use a tool in a \
                developer's project. How risky is allowing it?",
            "criteria": [
                "Low: reads, lists or searches, or a change that is easily undone",
                "Medium: changes or creates files inside the project",
                "High: deletes or overwrites work, acts outside the project, or reaches \
                 the network, credentials or git history",
            ],
        }
    });
    let state = json!({ "tool": tool, "input": clip(subject) });
    let answers = evaluate(config, state, questions, RISK_TIMEOUT)
        .inspect_err(|e| tracing::info!("jev: no risk read: {e}"))
        .ok()?;
    let answer = answers.get("risk")?;
    let score = answer.get("score")?.as_f64()?;
    let confidence = answer.get("confidence")?.as_f64()? as f32;
    let risk = match score {
        s if s < 0.5 => Risk::Low,
        s if s < 1.5 => Risk::Medium,
        _ => Risk::High,
    };
    Some((risk, confidence))
}

/// Asks `questions` about `state`; the `answers` object back.
fn evaluate(
    config: &JevConfig,
    state: Value,
    questions: Value,
    timeout: Duration,
) -> Result<Value> {
    let body = json!({
        "model": config.model,
        "state": state,
        "questions": questions,
    })
    .to_string();
    // curl's config format: quoted values, with `\` and `"` escaped, so
    // nothing in a prompt can end a line early.
    let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let config_text = format!(
        "url = \"{ENDPOINT}\"\nrequest = \"POST\"\nheader = \"Authorization: Bearer {}\"\n\
         header = \"Content-Type: application/json\"\ndata-binary = \"{}\"\n",
        quote(config.api_key.trim()),
        quote(&body),
    );

    let mut child = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg(format!("{:.1}", timeout.as_secs_f32()))
        .arg("--write-out")
        .arg("\n%{http_code}")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| KetError::Config(format!("could not run curl: {e}")))?;
    {
        use std::io::Write;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| KetError::Config("curl took no stdin".to_owned()))?;
        stdin
            .write_all(config_text.as_bytes())
            .map_err(|e| KetError::Config(format!("could not configure curl: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| KetError::Config(format!("curl did not finish: {e}")))?;
    if !output.status.success() {
        return Err(KetError::Config(format!(
            "Jev could not be reached: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text
        .rsplit_once('\n')
        .ok_or_else(|| KetError::Config("curl reported no status".to_owned()))?;
    let reason = match status.trim() {
        "200" => None,
        "401" => Some("the key was refused"),
        "422" => Some("the question was refused"),
        "429" => Some("rate limited"),
        "529" => Some("overloaded"),
        _ => Some("an unexpected answer"),
    };
    if let Some(reason) = reason {
        return Err(KetError::Config(format!(
            "Jev answered {}: {reason}",
            status.trim()
        )));
    }
    let reply: Value = serde_json::from_str(body)
        .map_err(|e| KetError::Config(format!("Jev's answer could not be read: {e}")))?;
    reply
        .get("answers")
        .cloned()
        .ok_or_else(|| KetError::Config("Jev's answer had no answers".to_owned()))
}

/// `text` cut to [`STATE_MAX`] characters.
fn clip(text: &str) -> String {
    match text.char_indices().nth(STATE_MAX) {
        Some((at, _)) => text[..at].to_owned(),
        None => text.to_owned(),
    }
}
