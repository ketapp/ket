use super::*;

#[test]
fn sidebar_resizing_keeps_both_the_tree_and_content_reachable() {
    assert_eq!(
        clamped_sidebar_width(px(10.0), px(1200.0)),
        Some(MIN_SIDEBAR_SIZE)
    );
    // Past the widest the window allows: all of it but the content's
    // least and the divider.
    let widest = px(700.0) - MIN_CONTENT_SIZE - SIDEBAR_DIVIDER_SIZE;
    assert_eq!(clamped_sidebar_width(px(900.0), px(700.0)), Some(widest));
}

#[test]
fn a_window_too_narrow_for_both_sides_keeps_the_previous_width() {
    assert_eq!(clamped_sidebar_width(px(200.0), px(400.0)), None);
}
