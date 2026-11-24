//! The New worktree dialog's branch naming and its agent, model and effort
//! choices.

use super::*;

fn choices(ids: &[(&str, &str)]) -> Vec<Choice> {
    ids.iter()
        .map(|(id, label)| Choice {
            id: (*id).to_owned(),
            label: (*label).to_owned(),
        })
        .collect()
}

#[test]
fn the_branch_comes_from_the_prompt_until_it_is_typed() {
    assert_eq!(
        submitted_branch("Fix the login redirect after sign-in", "", false),
        Ok("fix-the-login-redirect-after".to_owned())
    );
    // Typed over: the typed name, made fit for git, prompt or no prompt.
    assert_eq!(
        submitted_branch("Fix the login redirect", "login redirect", true),
        Ok("login-redirect".to_owned())
    );
    assert_eq!(
        submitted_branch("", "spike sqlite", true),
        Ok("spike-sqlite".to_owned())
    );
}

#[test]
fn a_submit_with_nothing_to_name_the_branch_says_why() {
    assert_eq!(
        submitted_branch("", "", false),
        Err("a worktree needs a prompt or a branch name")
    );
    assert_eq!(
        submitted_branch("...", "", false),
        Err("the prompt needs a word or two to name the branch from")
    );
    // Typed as nothing git would take: the same as nothing typed.
    assert_eq!(
        submitted_branch("", "?*~", true),
        Err("a worktree needs a prompt or a branch name")
    );
}

#[test]
fn a_taken_name_from_the_prompt_gets_a_tag_and_a_typed_one_is_refused() {
    let free = branch_when_taken("fix-login".to_owned(), "fix login", false, false).unwrap();
    assert_eq!(free, "fix-login");

    let tagged = branch_when_taken("fix-login".to_owned(), "fix login", true, false).unwrap();
    assert!(tagged.starts_with("fix-login-"), "{tagged}");
    assert_eq!(tagged.len(), "fix-login-".len() + 4);

    let refused = branch_when_taken("fix-login".to_owned(), "fix login", true, true);
    assert!(matches!(refused, Err(KetError::Conflict(ref why)) if why.contains("fix-login")));
}

#[test]
fn a_choice_menu_leads_with_the_default_and_ticks_the_current() {
    let models = choices(&[("opus", "Opus"), ("sonnet", "Sonnet")]);
    assert_eq!(choice_index(&models, None), 0);
    assert_eq!(choice_index(&models, Some("sonnet")), 2);
    assert_eq!(choice_index(&models, Some("gone")), 0);

    let rows = choice_items(&models, Some("opus"), "What Claude Code starts with");
    assert_eq!(rows.len(), 3);
    let checked: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(at, row)| match row {
            MenuEntry::Item(item) if item.checked => Some(at),
            _ => None,
        })
        .collect();
    assert_eq!(checked, vec![1]);
}

#[test]
fn a_trigger_reads_default_a_pick_or_nothing() {
    let models = choices(&[("opus", "Opus")]);
    assert_eq!(choice_label(&models, None), SharedString::from("Default"));
    assert_eq!(
        choice_label(&models, Some("opus")),
        SharedString::from("Opus")
    );
    assert_eq!(
        choice_label(&models, Some("gone")),
        SharedString::from("Default")
    );
    assert_eq!(choice_label(&[], Some("opus")), SharedString::default());
}

#[test]
fn only_a_switchable_agent_offers_models_and_efforts() {
    let (models, efforts) = agent_choices("");
    assert!(models.is_empty() && efforts.is_empty());

    let (models, efforts) = agent_choices("claude");
    assert!(models.iter().any(|choice| choice.id == "fable"));
    assert_eq!(efforts.len(), 5);

    let (models, efforts) = agent_choices("no-such-agent");
    assert!(models.is_empty() && efforts.is_empty());
}
