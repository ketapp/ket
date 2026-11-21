//! A JSON POST through `curl`, for the few services ket sends to: feedback
//! and push.
//!
//! ket links no TLS stack of its own, and every Mac has a curl that trusts
//! the system's certificates. The request goes to curl as a config file on
//! stdin rather than as arguments, so the body never shows in `ps`.

use std::process::{Command, Stdio};
use std::time::Duration;

/// Where curl puts the response body when nothing reads it: for
/// [`post_json`] the status is the answer.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Why a POST got no status back.
#[derive(Debug)]
pub(crate) enum PostError {
    /// curl couldn't be started or fed: this Mac, not the network.
    Curl(String),
    /// curl ran and failed: no connection, a name that doesn't resolve, a
    /// timeout. What it printed.
    Network(String),
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Curl(why) | Self::Network(why) => f.write_str(why),
        }
    }
}

/// The config curl reads on stdin to POST `body` to `url`, throwing away
/// what comes back.
pub(crate) fn request_config(url: &str, body: &str) -> String {
    format!("{}output = \"{NULL_DEVICE}\"\n", request(url, body))
}

/// [`request_config`] without its `output`, so the response body comes back
/// on curl's stdout.
///
/// curl's config format: quoted values, with `\` and `"` escaped, so
/// nothing in a body can end a line early.
fn request(url: &str, body: &str) -> String {
    let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "url = \"{}\"\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\n\
         data-binary = \"{}\"\n",
        quote(url),
        quote(body),
    )
}

/// POSTs `body` as JSON to `url`, giving up after `timeout`; the HTTP status
/// back.
pub(crate) fn post_json(url: &str, body: &str, timeout: Duration) -> Result<u16, PostError> {
    run(&request_config(url, body), timeout).map(|(status, _)| status)
}

/// [`post_json`], keeping what the service said as well as its status: the
/// push service puts why it refused something in the body.
pub(crate) fn post_json_answer(
    url: &str,
    body: &str,
    timeout: Duration,
) -> Result<(u16, String), PostError> {
    run(&request(url, body), timeout)
}

/// Runs curl on `config_text`: the status, and whatever body it printed.
fn run(config_text: &str, timeout: Duration) -> Result<(u16, String), PostError> {
    let mut child = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg(format!("{:.1}", timeout.as_secs_f32()))
        .arg("--write-out")
        // On a line of its own, after any body.
        .arg("\n%{http_code}")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PostError::Curl(format!("could not run curl: {e}")))?;
    {
        use std::io::Write;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| PostError::Curl("curl took no stdin".to_owned()))?;
        stdin
            .write_all(config_text.as_bytes())
            .map_err(|e| PostError::Curl(format!("could not configure curl: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| PostError::Curl(format!("curl did not finish: {e}")))?;
    if !output.status.success() {
        return Err(PostError::Network(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let printed = String::from_utf8_lossy(&output.stdout);
    let (body, status) = printed.rsplit_once('\n').unwrap_or(("", &printed));
    let status = status
        .trim()
        .parse()
        .map_err(|_| PostError::Curl("curl reported no status".to_owned()))?;
    Ok((status, body.to_owned()))
}
