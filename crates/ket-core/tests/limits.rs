//! Which of the ways a turn ends badly are a limit reached, in each agent's
//! own words as its CLI has been seen to write them.

use ket_core::limits::{Failure, Hit, Scope, Window, classify};

fn plan(window: Window) -> Option<Hit> {
    Some(Hit {
        scope: Scope::Plan,
        window,
    })
}

#[test]
fn claudes_session_and_weekly_limits_are_the_plan_running_out() {
    let session = Failure::coded(
        "rate_limit",
        "You've hit your session limit · resets 10:40pm (America/New_York)",
    );
    assert_eq!(classify("claude", &session), plan(Window::Session));

    let weekly = Failure::coded(
        "rate_limit",
        "You've hit your weekly limit · resets Oct 19 at 1am (America/New_York)",
    );
    assert_eq!(classify("claude", &weekly), plan(Window::Weekly));
}

#[test]
fn a_typographic_apostrophe_reads_like_a_plain_one() {
    assert_eq!(
        classify(
            "claude",
            &Failure::said("You’ve hit your session limit · resets 3pm")
        ),
        plan(Window::Session)
    );
}

#[test]
fn claudes_limit_on_one_model_is_not_the_plan() {
    let model = Failure::coded(
        "rate_limit",
        "You've reached your Opus limit. Run /usage-credits to continue or switch models \
         with /model.",
    );
    assert_eq!(
        classify("claude", &model),
        Some(Hit {
            scope: Scope::Model,
            window: Window::Unknown,
        })
    );
}

#[test]
fn claudes_other_errors_are_not_limits_whatever_they_say() {
    for (code, said) in [
        (
            "server_error",
            "API Error: Connection lost mid-response. The response above may be incomplete.",
        ),
        ("authentication_failed", "Login expired · Please run /login"),
        // The name decides: an error Claude calls something else is not a
        // limit for mentioning one.
        ("invalid_request", "Rate limit settings could not be read"),
        ("max_output_tokens", "The response hit the output limit"),
    ] {
        assert_eq!(
            classify("claude", &Failure::coded(code, said)),
            None,
            "{code}"
        );
    }
}

#[test]
fn claudes_words_alone_are_enough_when_it_gives_no_name() {
    assert_eq!(
        classify(
            "claude",
            &Failure::said("You've hit your session limit · resets 3pm")
        ),
        plan(Window::Session)
    );
    assert_eq!(
        classify(
            "claude",
            &Failure::said("Claude AI usage limit reached|1760000000")
        ),
        plan(Window::Unknown)
    );
    assert_eq!(
        classify(
            "claude",
            &Failure::said("Login expired · Please run /login")
        ),
        None
    );
    assert_eq!(
        classify(
            "claude",
            &Failure::said("You've reached the end of the file")
        ),
        None
    );
}

#[test]
fn codexs_usage_limit_is_a_limit_and_its_passing_refusals_are_not() {
    assert_eq!(
        classify("codex", &Failure::said("You've hit your usage limit.")),
        plan(Window::Unknown)
    );
    assert_eq!(
        classify(
            "codex",
            &Failure::said(
                "You've hit your usage limit. Upgrade to Pro or try again in 3 days 4 hours."
            )
        ),
        plan(Window::Unknown)
    );
    // As Codex 0.170 records it, apostrophe and all, with its own name for it.
    let recorded = "You’ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), \
                    visit https://chatgpt.com/settings/usage to purchase more credits or try \
                    again at 4:37 PM.";
    assert_eq!(
        classify("codex", &Failure::coded("usage_limit_exceeded", recorded)),
        plan(Window::Unknown)
    );
    assert_eq!(
        classify("codex", &Failure::coded("usage_limit_exceeded", "")),
        plan(Window::Unknown)
    );
    for passing in [
        "stream disconnected",
        "exceeded retry limit, last status: 429 Too Many Requests",
        "Rate limit reached for the model. Please try again in 1.2s.",
        "Your input exceeds the context window of this model.",
    ] {
        assert_eq!(
            classify("codex", &Failure::said(passing)),
            None,
            "{passing}"
        );
    }
}

#[test]
fn groks_exhausted_balance_and_its_429_are_limits() {
    let exhausted = "API error (status 402 Payment Required): Grok Build usage balance exhausted";
    assert_eq!(
        classify("grok", &Failure::said(exhausted)),
        plan(Window::Unknown)
    );
    assert_eq!(
        classify("grok", &Failure::said("HTTP 429 Too Many Requests")),
        plan(Window::Unknown)
    );
    assert_eq!(classify("grok", &Failure::said("HTTP 500")), None);
}

#[test]
fn opencode_is_read_for_what_its_backend_said() {
    for limit in [
        "You exceeded your current quota, please check your plan and billing details.",
        "Your credit balance is too low to access the API.",
        "rate_limit_error: This request would exceed your account's rate limit.",
        "429 Too Many Requests",
        "Weekly limit reached: your usage limit resets on Monday.",
    ] {
        assert!(
            classify("opencode", &Failure::said(limit)).is_some(),
            "{limit}"
        );
    }
    for other in [
        "ConnectionRefused: Unable to connect. Is the computer able to access the url?",
        "OpenCode's step ended on an error.",
        "Step interrupted",
        "The context limit of this model was exceeded.",
        "Used 4290 tokens of 8192",
        "request req_0429abc failed",
    ] {
        assert_eq!(classify("opencode", &Failure::said(other)), None, "{other}");
    }
}

#[test]
fn a_local_agent_is_read_as_the_harness_it_runs() {
    assert_eq!(
        classify(
            "opencode:ollama/qwen3-coder",
            &Failure::said("429 Too Many Requests")
        ),
        plan(Window::Unknown)
    );
    assert_eq!(
        classify(
            "claude:work",
            &Failure::coded("rate_limit", "You've hit your weekly limit")
        ),
        plan(Window::Weekly)
    );
}

#[test]
fn a_named_window_is_kept_for_any_agent() {
    assert_eq!(
        classify(
            "opencode",
            &Failure::said("Weekly limit reached: your usage limit resets on Monday.")
        ),
        plan(Window::Weekly)
    );
    assert_eq!(
        classify(
            "opencode",
            &Failure::said("5-hour limit reached, rate limit resets at 4pm")
        ),
        plan(Window::Session)
    );
}
