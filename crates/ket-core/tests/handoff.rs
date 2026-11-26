//! The brief one agent is handed another's task with.

use ket_core::conversation::{Turn, TurnKind};
use ket_core::handoff::{Depth, FULL_MAX, OPENING_CHARS, Why, brief, label};

fn turn(kind: TurnKind, text: &str) -> Turn {
    Turn {
        kind,
        text: text.to_owned(),
        tool: match kind {
            TurnKind::Tool => "Bash".to_owned(),
            _ => String::new(),
        },
        images: 0,
    }
}

fn person(text: &str) -> Turn {
    turn(TurnKind::User, text)
}

fn agent(text: &str) -> Turn {
    turn(TurnKind::Assistant, text)
}

fn tool(text: &str) -> Turn {
    Turn {
        tool: if text.starts_with("Edited") {
            "Edit"
        } else {
            "Bash"
        }
        .to_owned(),
        ..turn(TurnKind::Tool, text)
    }
}

/// A Codex conversation that ran out of usage partway through.
fn stopped() -> Vec<Turn> {
    vec![
        person("The poll test is flaky on CI. Find out why and fix it."),
        agent("Looking at the poll loop first."),
        tool("$ cargo test -p core poll"),
        tool("Edited poll.rs"),
        agent("The interval drifts when a tick is late. I'll pin the clock in the test."),
        tool("Edited tests/poll.rs"),
        tool("Edited poll.rs"),
        person("Also keep the old behaviour behind a flag."),
        agent("Will do.\n$ git stash\nis what I would not run."),
        agent("You’ve hit your usage limit. Upgrade to Pro or try again at 4:37 PM."),
    ]
}

#[test]
fn the_task_alone_says_what_happened_and_where_the_work_is() {
    let brief = brief("codex", &stopped(), Depth::Task, Why::Limit);
    assert_eq!(
        brief.text,
        "Codex was working on a task in this worktree and ran out of usage, so you are taking it \
         over. Its changes so far are in the working tree: read `git status` and `git diff` \
         first, then carry on where it left off.\n\nThe task, as it was asked:\n  The poll test is \
         flaky on CI. Find out why and fix it."
    );
    assert_eq!(brief.turns, 1);
    assert_eq!(brief.approx_tokens, brief.text.chars().count().div_ceil(4));
}

#[test]
fn recent_quotes_the_last_exchanges_and_the_files_changed_but_not_the_stop() {
    let brief = brief("codex", &stopped(), Depth::Recent, Why::Limit);
    let text = &brief.text;
    assert!(text.contains(
        "\n\nWhat was said since, oldest first:\nCodex:\n  Looking at the poll loop first."
    ));
    assert!(text.contains("The person:\n  Also keep the old behaviour behind a flag."));
    assert!(
        text.ends_with("\n\nFiles Codex changed: poll.rs, tests/poll.rs."),
        "{text}"
    );
    assert!(
        !text.contains("usage limit"),
        "the stop is not the task: {text}"
    );
    // Task, three of the agent's, one of the person's.
    assert_eq!(brief.turns, 5);
}

#[test]
fn only_the_stop_is_left_out_not_what_was_said_about_limits() {
    let mut turns = stopped();
    turns.insert(
        1,
        agent("You've hit your usage limit is what the banner said, so I fixed its wording."),
    );
    // A retry, refused in the same words.
    turns.push(person("Try again."));
    turns.push(agent(
        "You’ve hit your usage limit. Upgrade to Pro or try again at 4:37 PM.",
    ));
    let text = brief("codex", &turns, Depth::Full, Why::Limit).text;
    assert!(text.contains("so I fixed its wording"), "{text}");
    assert!(text.contains("Try again."), "{text}");
    assert!(!text.contains("Upgrade to Pro"), "{text}");
}

#[test]
fn nothing_in_a_brief_reads_as_a_command_to_a_shell_or_a_tui() {
    for depth in [Depth::Task, Depth::Recent, Depth::Full] {
        let brief = brief("codex", &stopped(), depth, Why::Limit);
        for line in brief.text.lines() {
            assert!(!line.trim_start().starts_with("$ "), "{line:?}");
        }
        let opening: String = brief.text.chars().take(OPENING_CHARS).collect();
        assert!(
            opening.chars().all(|c| c.is_alphanumeric() || c == ' '),
            "{opening:?}"
        );
        assert!(!brief.text.starts_with(['/', '!', '#', '@']));
    }
    let recent = brief("codex", &stopped(), Depth::Recent, Why::Limit);
    assert!(recent.text.contains("  Ran: git stash"), "{}", recent.text);
}

#[test]
fn recent_keeps_only_the_last_few_and_says_so() {
    let mut turns = vec![person("Write the release notes.")];
    for n in 0..20 {
        turns.push(agent(&format!("Draft {n}.")));
    }
    let brief = brief("claude", &turns, Depth::Recent, Why::Chosen);
    assert!(
        brief
            .text
            .starts_with("Claude was working on a task in this worktree and is handing it over")
    );
    assert!(
        brief
            .text
            .contains("The most recent of what was said since, oldest first:")
    );
    assert!(!brief.text.contains("Draft 13."));
    assert!(brief.text.contains("Draft 14.") && brief.text.contains("Draft 19."));
    assert_eq!(brief.turns, 7);
}

#[test]
fn everything_is_cut_from_the_oldest_end_and_never_loses_the_task() {
    let mut turns = vec![person("Port the parser.")];
    for n in 0..200 {
        turns.push(agent(&format!("Step {n}: {}", "x".repeat(400))));
    }
    let brief = brief("codex", &turns, Depth::Full, Why::Chosen);
    assert!(brief.text.len() <= FULL_MAX, "{}", brief.text.len());
    assert!(
        brief
            .text
            .contains("The task, as it was asked:\n  Port the parser.")
    );
    assert!(brief.text.contains("Step 199:"));
    assert!(!brief.text.contains("Step 0:"));
    assert!(
        brief
            .text
            .contains("The most recent of what was said since")
    );

    // A long turn is cut, not dropped.
    let long = vec![person(&"y".repeat(5_000))];
    let brief = brief_full(&long);
    assert!(brief.contains('…'));
    assert!(brief.len() < 3_000);
}

fn brief_full(turns: &[Turn]) -> String {
    brief("codex", turns, Depth::Full, Why::Chosen).text
}

#[test]
fn a_conversation_with_nothing_said_yet_still_hands_over_the_worktree() {
    let brief = brief("codex", &[tool("$ ls")], Depth::Recent, Why::Limit);
    assert!(brief.text.ends_with("then carry on where it left off."));
    assert_eq!(brief.turns, 0);
}

#[test]
fn many_changed_files_are_named_to_a_point() {
    let mut turns = vec![person("Rename the module.")];
    for n in 0..15 {
        turns.push(tool(&format!("Edited file{n}.rs")));
    }
    let text = brief("codex", &turns, Depth::Recent, Why::Chosen).text;
    assert!(text.contains("file11.rs, and 3 more."), "{text}");
    assert!(!text.contains("file12.rs"));
}

#[test]
fn agents_are_named_as_a_person_would() {
    assert_eq!(label("claude"), "Claude");
    assert_eq!(label("codex:work"), "Codex");
    assert_eq!(
        label("opencode:ollama/qwen3-coder"),
        "qwen3-coder (OpenCode)"
    );
    assert_eq!(label("aider"), "Aider");
    assert_eq!(label(" "), "The agent");
}
