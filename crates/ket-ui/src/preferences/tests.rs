use super::{FONT_SIZE_RANGE, McpState, font_size_typed, stepped};
use ket_core::id::WorktreeId;

fn empty_mcp_report() -> ket_core::mcp::Report {
    ket_core::mcp::Report {
        claude: ket_core::mcp::Check::Servers(Vec::new()),
        codex: ket_core::mcp::Check::Servers(Vec::new()),
    }
}

#[test]
fn mcp_starts_unchecked_and_needs_a_selection_to_show_a_result() {
    let mut state = McpState::default();
    let worktree = WorktreeId::new("one");
    assert!(!state.is_checking(&worktree));
    assert!(state.report_for(Some(&worktree)).is_none());
    assert!(state.report_for(None).is_none());

    assert!(state.begin(worktree.clone()));
    assert!(state.finish(worktree.clone(), empty_mcp_report()));
    assert!(state.report_for(None).is_none());
    assert!(state.report_for(Some(&worktree)).is_some());
}

#[test]
fn mcp_result_does_not_follow_a_different_worktree() {
    let mut state = McpState::default();
    let first = WorktreeId::new("one");
    let second = WorktreeId::new("two");
    assert!(state.begin(first.clone()));
    assert!(!state.begin(second.clone()));
    assert!(!state.finish(second.clone(), empty_mcp_report()));
    assert!(state.is_checking(&first));
    assert!(state.finish(first.clone(), empty_mcp_report()));
    assert!(state.report_for(Some(&second)).is_none());
    assert!(state.report_for(Some(&first)).is_some());

    assert!(state.begin(second.clone()));
    assert!(state.report_for(Some(&first)).is_none());
    assert!(state.finish(second.clone(), empty_mcp_report()));
    assert!(state.report_for(Some(&second)).is_some());
}

#[test]
fn a_step_stays_within_the_sizes_a_font_can_be() {
    assert_eq!(stepped(12, 1), 13);
    assert_eq!(stepped(12, -1), 11);
    // ⌘= at the top stays at the top rather than refusing.
    assert_eq!(stepped(*FONT_SIZE_RANGE.end(), 1), *FONT_SIZE_RANGE.end());
    assert_eq!(
        stepped(*FONT_SIZE_RANGE.start(), -1),
        *FONT_SIZE_RANGE.start()
    );
}

#[test]
fn a_cleared_size_box_means_the_shipped_size() {
    assert_eq!(font_size_typed("", 15, "Interface font size"), Ok(15));
    assert_eq!(font_size_typed("   ", 12, "Code font size"), Ok(12));
}

#[test]
fn a_typed_size_is_read_as_written() {
    assert_eq!(font_size_typed(" 14 ", 15, "Interface font size"), Ok(14));
}

#[test]
fn a_size_that_is_not_a_whole_number_names_its_box() {
    let err = font_size_typed("12.5", 12, "Code font size").unwrap_err();
    assert!(err.starts_with("Code font size must be"), "{err}");
    let err = font_size_typed("-3", 15, "Interface font size").unwrap_err();
    assert!(err.starts_with("Interface font size must be"), "{err}");
}

#[test]
fn a_size_too_large_to_be_a_size_is_still_refused_as_a_number() {
    // 70000 overflows a u16 and parses as nothing; the message is the
    // same one a word gets, which is right — the file's own check says
    // what the range is for the numbers that do parse.
    assert!(font_size_typed("70000", 12, "Code font size").is_err());
}
