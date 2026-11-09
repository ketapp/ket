//! What in a request looks like an identifier or a secret.
//!
//! Flagged, never changed: the request goes as the agent sent it, and the
//! log says what in it looked like a Social Security number, a card, a key.
//! Pattern matching, so it finds what has a shape — not a name, a diagnosis
//! or anything else a person would have to read to recognise.
//!
//! **Only what left this machine.** A request carries the whole conversation,
//! the model's own replies included; those came from the provider. What is
//! scanned is what the person typed and what the agent's tools read and ran,
//! in the part of the request that was new (see [`super::delta`]).
//!
//! **Quiet on what is everywhere.** Loopback and private addresses, `noreply`
//! mail, `git@` remotes and version numbers shaped like an address are not
//! flagged: a session that touches a repository would otherwise be flagged
//! on every turn, and a flag that is always there is one nobody reads.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a flag is for, most serious first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An API key, an access token, a private key.
    Secret,
    /// A US Social Security number, in its `123-45-6789` shape.
    Ssn,
    /// A payment card number that passes the Luhn check.
    Card,
    /// A date beside "DOB", "date of birth" or "born".
    Birth,
    /// A North American number, or one with a country code.
    Phone,
    /// An email address, other than `noreply` and `git@` ones.
    Email,
    /// A public IPv4 address.
    Ip,
}

impl Kind {
    /// What a row's tag calls it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Secret => "key",
            Kind::Ssn => "SSN",
            Kind::Card => "card",
            Kind::Birth => "birth date",
            Kind::Phone => "phone",
            Kind::Email => "email",
            Kind::Ip => "IP",
        }
    }
}

/// One kind found in a request, and how many different values of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    /// What was found.
    pub kind: Kind,
    /// How many different values of it.
    pub count: usize,
}

/// The flags in `kept`, the new part of a request as [`super::delta`] took
/// it, most serious first: what its messages from this machine carry.
pub fn scan(kept: &Value) -> Vec<Flag> {
    let mut texts = Vec::new();
    for message in turns(kept, "messages") {
        if message.get("role").and_then(Value::as_str) == Some("user") {
            collect(message.get("content"), &mut texts);
        }
    }
    for item in turns(kept, "input") {
        if said_by_user(item) {
            collect(item.get("content"), &mut texts);
        } else if is_output(item) {
            collect(item.get("output"), &mut texts);
        }
    }
    let mut found: BTreeMap<Kind, HashSet<&str>> = BTreeMap::new();
    for text in texts {
        for (range, kind) in find(text) {
            found.entry(kind).or_default().insert(&text[range]);
        }
    }
    found
        .into_iter()
        .map(|(kind, values)| Flag {
            kind,
            count: values.len(),
        })
        .collect()
}

/// Where something flagged in a request came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// What the person typed.
    Message,
    /// What the agent added to a message of the person's: its instructions
    /// files, its memory, its reminders.
    Context,
    /// A file a tool read, by the path the agent gave it.
    File(String),
    /// A shell command's output.
    Command(String),
    /// Another tool's output, by the tool's name; empty when the call that
    /// asked for it was sent in an earlier request.
    Tool(String),
}

/// Where what is flagged in `kept` came from, in the order first met: the
/// message that carried it, or the tool call whose output did.
pub fn sources(kept: &Value) -> Vec<Source> {
    let mut found = sources_of_messages(kept);
    for source in sources_of_input(kept) {
        if !found.contains(&source) {
            found.push(source);
        }
    }
    found
}

/// The same, for a Messages API body.
fn sources_of_messages(kept: &Value) -> Vec<Source> {
    let messages: Vec<&Value> = turns(kept, "messages").collect();
    // A tool's result names its call by id; the call says what was read.
    let calls: HashMap<&str, &Value> = messages
        .iter()
        .copied()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("content")?.as_array())
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| Some((block.get("id")?.as_str()?, block)))
        .collect();
    let flagged = |content: Option<&Value>| {
        let mut texts = Vec::new();
        collect(content, &mut texts);
        texts.iter().any(|text| !find(text).is_empty())
    };
    let mut found: Vec<Source> = Vec::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let blocks: Vec<&Value> = match message.get("content") {
            Some(Value::Array(blocks)) => blocks.iter().collect(),
            Some(text @ Value::String(_)) => vec![text],
            _ => Vec::new(),
        };
        for block in blocks {
            let source = match block {
                Value::String(text) if flagged(Some(block)) => Some(said(text)),
                _ => match block.get("type").and_then(Value::as_str) {
                    Some("text") if flagged(block.get("text")) => {
                        Some(said(block["text"].as_str().unwrap_or_default()))
                    }
                    Some("tool_result") if flagged(block.get("content")) => Some(called(
                        block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .and_then(|id| calls.get(id).copied()),
                    )),
                    _ => None,
                },
            };
            if let Some(source) = source
                && !found.contains(&source)
            {
                found.push(source);
            }
        }
    }
    found
}

/// The same, for a Responses API body: its `input`, where a tool's output
/// is an item of its own.
fn sources_of_input(kept: &Value) -> Vec<Source> {
    // An output names its call by id; the call says what was run.
    let calls: HashMap<&str, Value> = turns(kept, "input")
        .filter_map(|item| Some((item.get("call_id")?.as_str()?, as_call(item)?)))
        .collect();
    let flagged = |content: Option<&Value>| {
        let mut texts = Vec::new();
        collect(content, &mut texts);
        texts.iter().any(|text| !find(text).is_empty())
    };
    let mut found: Vec<Source> = Vec::new();
    for item in turns(kept, "input") {
        let source = if said_by_user(item) {
            let mut texts = Vec::new();
            collect(item.get("content"), &mut texts);
            texts
                .into_iter()
                .find(|text| !find(text).is_empty())
                .map(said)
        } else if is_output(item) && flagged(item.get("output")) {
            Some(called(
                item.get("call_id")
                    .and_then(Value::as_str)
                    .and_then(|id| calls.get(id)),
            ))
        } else {
            None
        };
        if let Some(source) = source
            && !found.contains(&source)
        {
            found.push(source);
        }
    }
    found
}

/// `kept`'s conversation, under `key`: `messages` or `input`.
fn turns<'a>(kept: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    kept.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Whether a Responses API item is a message of the person's — the role
/// a plain one has as well, typed or not.
fn said_by_user(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        None | Some("message")
    ) && item.get("role").and_then(Value::as_str) == Some("user")
}

/// Whether a Responses API item is what a tool gave back:
/// `function_call_output`, `custom_tool_call_output` and the like.
fn is_output(item: &Value) -> bool {
    item.get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.ends_with("_output"))
}

/// A Responses API tool call as [`called`] reads one: its name, and what it
/// was asked, from the JSON its arguments are sent as.
fn as_call(item: &Value) -> Option<Value> {
    let kind = item.get("type")?.as_str()?;
    if !kind.ends_with("_call") {
        return None;
    }
    let input = match item.get("arguments") {
        Some(Value::String(arguments)) => serde_json::from_str(arguments).unwrap_or_default(),
        _ => item.get("action").cloned().unwrap_or_default(),
    };
    Some(
        serde_json::json!({ "name": item.get("name").cloned().unwrap_or_default(), "input": input }),
    )
}

/// What an agent adds to a message of the person's opens on one of these:
/// Claude Code's reminders, and Codex's instructions files, environment and
/// skills.
const CONTEXT: [&str; 6] = [
    "<system-reminder>",
    "# AGENTS.md instructions",
    "<environment_context>",
    "<skill>",
    "<subagent_notification>",
    "<turn_aborted>",
];

/// A message's text: the person's, or what their agent added to it.
fn said(text: &str) -> Source {
    let text = text.trim_start();
    if CONTEXT.iter().any(|opening| text.starts_with(opening)) {
        Source::Context
    } else {
        Source::Message
    }
}

/// What a tool call read, from its input.
fn called(call: Option<&Value>) -> Source {
    let Some(call) = call else {
        return Source::Tool(String::new());
    };
    let input = call.get("input");
    let field = |name: &str| input.and_then(|input| input.get(name)?.as_str());
    // `file_path` and `notebook_path` name one file. Not `path`, which is
    // where Grep and Glob looked — a directory, often the whole checkout.
    if let Some(path) = ["file_path", "notebook_path"].into_iter().find_map(field) {
        return Source::File(path.to_owned());
    }
    if let Some(command) = field("command").or_else(|| field("cmd")) {
        return Source::Command(command.to_owned());
    }
    // A command given as its words, `["bash", "-lc", "cat notes.md"]`: the
    // script is the last of them.
    if let Some(command) =
        input.and_then(|input| input.get("command")?.as_array()?.last()?.as_str())
    {
        return Source::Command(command.to_owned());
    }
    Source::Tool(
        call.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

/// Who issued a flagged key, and the page where it is revoked, by the
/// key's prefix. `None` for a token or a private key with no one issuer.
pub fn issuer(key: &str) -> Option<(&'static str, &'static str)> {
    const ISSUERS: [(&str, &str, &str); 14] = [
        (
            "sk-ant-",
            "Anthropic",
            "https://console.anthropic.com/settings/keys",
        ),
        ("ghp_", "GitHub", "https://github.com/settings/tokens"),
        ("gho_", "GitHub", "https://github.com/settings/applications"),
        ("ghu_", "GitHub", "https://github.com/settings/applications"),
        (
            "ghs_",
            "GitHub",
            "https://github.com/settings/installations",
        ),
        ("ghr_", "GitHub", "https://github.com/settings/applications"),
        (
            "github_pat_",
            "GitHub",
            "https://github.com/settings/personal-access-tokens",
        ),
        (
            "glpat-",
            "GitLab",
            "https://gitlab.com/-/user_settings/personal_access_tokens",
        ),
        (
            "AKIA",
            "AWS",
            "https://console.aws.amazon.com/iam/home#/security_credentials",
        ),
        (
            "ASIA",
            "AWS",
            "https://console.aws.amazon.com/iam/home#/security_credentials",
        ),
        ("xox", "Slack", "https://api.slack.com/apps"),
        ("sk_live_", "Stripe", "https://dashboard.stripe.com/apikeys"),
        (
            "AIza",
            "Google",
            "https://console.cloud.google.com/apis/credentials",
        ),
        // After `sk-ant-`, which it would also take.
        ("sk-", "OpenAI", "https://platform.openai.com/api-keys"),
    ];
    ISSUERS
        .iter()
        .find(|(prefix, _, _)| key.starts_with(prefix))
        .map(|&(_, name, page)| (name, page))
}

/// The text in a message's content: a string, or its text blocks and tool
/// results. Images and documents are left out; they are not kept either.
fn collect<'a>(content: Option<&'a Value>, out: &mut Vec<&'a str>) {
    match content {
        Some(Value::String(text)) => out.push(text),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text" | "input_text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            out.push(text);
                        }
                    }
                    Some("tool_result") => collect(block.get("content"), out),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Where in `text` something is flagged, in order, none overlapping. Also
/// reads a body as the log keeps it — JSON, its line breaks escaped — so a
/// value just after a `\n` is found there as it is in the message.
pub fn find(text: &str) -> Vec<(Range<usize>, Kind)> {
    let mut found: Vec<(Range<usize>, Kind)> = Vec::new();
    for detector in DETECTORS.iter() {
        for captures in detector.pattern.captures_iter(text) {
            let Some(matched) = captures.get(detector.group) else {
                continue;
            };
            let range = past_escape(text, matched.range());
            if (detector.valid)(text, range.clone()) {
                found.push((range, detector.kind));
            }
        }
    }
    // Where two overlap, the earlier wins, and of two at once the more
    // serious: a card is not also a phone number.
    found.sort_by_key(|(range, kind)| (range.start, *kind));
    let mut end = 0;
    found.retain(|(range, _)| {
        let keep = range.start >= end;
        if keep {
            end = range.end;
        }
        keep
    });
    found
}

/// `range` without a leading `n`, `t` or `r` that is the second half of a
/// JSON escape: in `\njane@example.com`, the address starts at the `j`.
fn past_escape(text: &str, range: Range<usize>) -> Range<usize> {
    let bytes = text.as_bytes();
    if range.start > 0
        && bytes[range.start - 1] == b'\\'
        && matches!(bytes.get(range.start), Some(b'n' | b't' | b'r'))
        && range.len() > 1
    {
        return range.start + 1..range.end;
    }
    range
}

struct Detector {
    kind: Kind,
    pattern: Regex,
    /// Which capture is the value: 0 for the whole match.
    group: usize,
    /// Whether a match is one, beyond its shape.
    valid: fn(&str, Range<usize>) -> bool,
}

// Digits are `[0-9]`, never `\d`, which takes any script's digits: the
// checks after a match read it as ASCII.
static DETECTORS: LazyLock<Vec<Detector>> = LazyLock::new(|| {
    let detector = |kind, pattern: &str, group, valid: fn(&str, Range<usize>) -> bool| Detector {
        kind,
        pattern: Regex::new(pattern).expect("a flag's pattern compiles"),
        group,
        valid,
    };
    vec![
        detector(
            Kind::Secret,
            concat!(
                r"sk-ant-[A-Za-z0-9_-]{20,}",
                r"|sk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{32,}",
                r"|gh[pousr]_[A-Za-z0-9]{36,}",
                r"|github_pat_[A-Za-z0-9_]{40,}",
                r"|glpat-[A-Za-z0-9_-]{20,}",
                r"|(?:AKIA|ASIA)[0-9A-Z]{16}",
                r"|xox[abprs]-[A-Za-z0-9-]{10,}",
                r"|[sr]k_live_[0-9A-Za-z]{24,}",
                r"|AIza[0-9A-Za-z_-]{35}",
                r"|eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
                r"|-----BEGIN (?:[A-Z]+ )?PRIVATE KEY-----",
            ),
            0,
            secret,
        ),
        detector(Kind::Ssn, r"[0-9]{3}-[0-9]{2}-[0-9]{4}", 0, ssn),
        // As cards are printed: one run, in fours, or Amex's 4-6-5. Not any
        // digits with spaces between, which would run on into the numbers
        // around a card and take it with them.
        detector(
            Kind::Card,
            r"[0-9]{13,19}|[0-9]{4}(?:[ -][0-9]{4}){3}(?:[ -][0-9]{3})?|[0-9]{4}[ -][0-9]{6}[ -][0-9]{5}",
            0,
            card,
        ),
        detector(
            Kind::Birth,
            concat!(
                r"(?i)\b(?:dob|d\.o\.b|date of birth|birth ?date|born)[^0-9\n]{0,12}",
                r"([0-9]{1,2}[/.-][0-9]{1,2}[/.-](?:[0-9]{4}|[0-9]{2})",
                r"|[0-9]{4}-[0-9]{2}-[0-9]{2}",
                r"|(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.? [0-9]{1,2},? [0-9]{4})",
            ),
            1,
            |_, _| true,
        ),
        detector(
            Kind::Phone,
            concat!(
                r"(?:\+1[ .-]?)?(?:\([0-9]{3}\) ?|[0-9]{3}[.-])[0-9]{3}[.-][0-9]{4}",
                r"|\+[2-9][0-9]{0,2}[ .-][0-9]{2,4}[ .-][0-9]{3,4}[ .-][0-9]{3,4}",
            ),
            0,
            phone,
        ),
        detector(
            Kind::Email,
            r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}",
            0,
            email,
        ),
        detector(Kind::Ip, r"[0-9]{1,3}(?:\.[0-9]{1,3}){3}", 0, ip),
    ]
});

/// Whether `range` stands apart in `text`: not part of a longer word or
/// number, with `joins` counted as part of one too. An escape before it
/// (`\n`) is a break, as it is in the text it stands for.
fn standalone(text: &str, range: Range<usize>, joins: &[char]) -> bool {
    let joined = |c: char| c.is_alphanumeric() || joins.contains(&c);
    let before = text[..range.start].chars().next_back();
    let escaped =
        text[..range.start].ends_with(['n', 't', 'r']) && text[..range.start - 1].ends_with('\\');
    let after = text[range.end..].chars().next();
    (escaped || !before.is_some_and(joined)) && !after.is_some_and(joined)
}

fn secret(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    // A key is random, so has a digit somewhere; a long hyphenated name
    // starting `sk-` has none.
    (value.starts_with("-----") || value.bytes().any(|b| b.is_ascii_digit()))
        && standalone(text, range, &['_', '-'])
}

fn ssn(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    let (area, group, serial) = (&value[..3], &value[4..6], &value[7..]);
    standalone(text, range, &['-', '.'])
        && area != "000"
        && area != "666"
        && !area.starts_with('9')
        && group != "00"
        && serial != "0000"
}

fn card(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    if !standalone(text, range, &['-', '.']) || (value.contains(' ') && value.contains('-')) {
        return false;
    }
    let digits: Vec<u32> = value.chars().filter_map(|c| c.to_digit(10)).collect();
    let len = digits.len();
    let prefix = |n: usize| digits[..n].iter().fold(0, |acc, d| acc * 10 + d);
    let issuer = match digits[0] {
        4 => matches!(len, 13 | 16 | 19),
        5 => (51..=55).contains(&prefix(2)) && len == 16,
        2 => (2221..=2720).contains(&prefix(4)) && len == 16,
        3 => matches!(prefix(2), 34 | 37) && len == 15,
        6 => {
            (prefix(4) == 6011 || prefix(2) == 65 || (644..=649).contains(&prefix(3)))
                && (16..=19).contains(&len)
        }
        _ => false,
    };
    issuer && luhn(&digits)
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| match i % 2 {
            0 => d,
            _ if d * 2 > 9 => d * 2 - 9,
            _ => d * 2,
        })
        .sum();
    sum.is_multiple_of(10)
}

fn phone(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    // North American numbers: an area code and exchange that start 2–9.
    let digits: Vec<u32> = value.chars().filter_map(|c| c.to_digit(10)).collect();
    let local = if value.starts_with("+1") {
        &digits[1..]
    } else {
        &digits[..]
    };
    let plausible = (value.starts_with('+') && !value.starts_with("+1"))
        || (local.len() == 10 && local[0] >= 2 && local[3] >= 2);
    plausible && standalone(text, range, &['-', '.'])
}

/// File extensions an `@2x` asset name ends in, which read as a domain.
const NOT_DOMAINS: [&str; 16] = [
    "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "pdf", "js", "mjs", "ts", "tsx", "jsx",
    "css", "json", "md",
];

fn email(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    let Some((local, domain)) = value.rsplit_once('@') else {
        return false;
    };
    let lower = value.to_ascii_lowercase();
    let tld = domain.rsplit('.').next().unwrap_or_default();
    !local.starts_with('.')
        && local != "git"
        && !lower.contains("noreply")
        && !lower.contains("no-reply")
        && !NOT_DOMAINS.contains(&tld.to_ascii_lowercase().as_str())
        && standalone(text, range, &['.', '-', '_', '@'])
}

fn ip(text: &str, range: Range<usize>) -> bool {
    let value = &text[range.clone()];
    let octets: Vec<u32> = value.split('.').filter_map(|o| o.parse().ok()).collect();
    let &[a, b, c, d] = &octets[..] else {
        return false;
    };
    let private = a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || a >= 224;
    // `1.2.3.4` is far more often a version than an address.
    let version = [a, b, c, d].iter().all(|&o| o < 10);
    [a, b, c, d].iter().all(|&o| o <= 255)
        && !private
        && !version
        && standalone(text, range, &['.', '-', '_'])
}
