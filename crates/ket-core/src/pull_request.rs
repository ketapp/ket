//! The words a pull request is offered with, before anyone edits them.
//!
//! [`crate::workspace::Workspace::open_pull_request`] takes a title and a
//! body and asks nothing about where they came from. These are where the
//! dialog and the command get their first draft of each: the backlog note
//! the branch was started from, when there is one, because its title is the
//! task in the person's own words; else the branch name, made readable. The
//! body names the issue the branch is named for, so that landing the pull
//! request closes it, which is the convention `gh issue develop` and ket's
//! own "Start in worktree" share.

use crate::backlog::Note;
use crate::config::issue_of;
use crate::worktree::Worktree;

/// The title to offer for `worktree`'s pull request.
///
/// The note started in this worktree wins; failing that, a note whose issue
/// the branch is named for; failing that, the branch itself — see
/// [`title_from_branch`].
pub fn title_for(worktree: &Worktree, notes: &[Note]) -> String {
    let issue = issue_of(&worktree.branch).and_then(|n| n.parse::<u64>().ok());
    let started_here = notes.iter().find(|note| {
        note.done
            .as_ref()
            .is_some_and(|done| done.worktree.as_ref() == Some(&worktree.id))
    });
    let for_issue = || {
        notes.iter().find(|note| {
            note.issue
                .as_ref()
                .is_some_and(|link| Some(link.number) == issue)
        })
    };
    started_here
        .or_else(for_issue)
        .map(|note| note.title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| title_from_branch(&worktree.branch))
}

/// The body to offer: `Closes #142` for a branch named for an issue, so the
/// merge closes it, and nothing for any other branch.
pub fn body_for(branch: &str) -> String {
    issue_of(branch)
        .map(|n| format!("Closes #{n}\n"))
        .unwrap_or_default()
}

/// A branch name as a title: `142-fix-the-login-k3f9` becomes
/// `Fix the login`.
///
/// Three things come off. The issue number the branch opens with, since the
/// body names the issue. The four-character tag ket's own branch names end
/// in, which keeps two alike branches apart and means nothing to a reader —
/// taken off only when it looks like one, four characters of `0-9a-z` with
/// a digit among them, so a last word like `login` or `test` stays. And the
/// hyphens, which become spaces. The first letter is capitalised; nothing
/// else is, since `fix the API` was lowercase on the branch too.
pub fn title_from_branch(branch: &str) -> String {
    let name = branch.rsplit('/').next().unwrap_or(branch);
    let name = match issue_of(name) {
        Some(digits) => name[digits.len()..].trim_start_matches('-'),
        None => name,
    };
    let mut words: Vec<&str> = name.split('-').filter(|w| !w.is_empty()).collect();
    if words.len() > 1 && looks_like_tag(words[words.len() - 1]) {
        words.pop();
    }
    let mut title = words.join(" ");
    if let Some(first) = title.chars().next() {
        let upper: String = first.to_uppercase().collect();
        title.replace_range(..first.len_utf8(), &upper);
    }
    title
}

/// Whether `word` is the tag `branch_for` appends: four of `0-9a-z`, at
/// least one a digit.
fn looks_like_tag(word: &str) -> bool {
    word.len() == 4
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog::Done;
    use crate::github::IssueLink;
    use crate::id::{ProjectId, WorktreeId};

    fn worktree(branch: &str) -> Worktree {
        Worktree {
            id: WorktreeId::new("wt-1"),
            project_id: ProjectId::new("p"),
            branch: branch.to_owned(),
            name: None,
            agent_title: None,
            path: std::path::PathBuf::from("/tmp/wt-1"),
            base: "main".to_owned(),
            base_commit: None,
            agent: None,
            created_at_ms: 0,
            provisioned_at_ms: None,
            provisioned_paths: Vec::new(),
            token_reduction: crate::worktree::default_token_reduction(),
            token_reduction_id: None,
            token_reduction_pack_id: None,
            pinned: false,
            pull_request: None,
        }
    }

    fn note(title: &str) -> Note {
        let mut note = Note::new(&[]);
        note.title = title.to_owned();
        note
    }

    #[test]
    fn a_branch_reads_as_a_title_without_its_issue_number_or_tag() {
        assert_eq!(title_from_branch("142-fix-the-login-k3f9"), "Fix the login");
        assert_eq!(title_from_branch("fix-the-login"), "Fix the login");
        assert_eq!(title_from_branch("me/142-fix-the-login"), "Fix the login");
        assert_eq!(title_from_branch("fix-the-API"), "Fix the API");
    }

    #[test]
    fn a_last_word_that_is_not_a_tag_stays() {
        // `test` and `login` are four letters with no digit: words.
        assert_eq!(title_from_branch("fix-the-test"), "Fix the test");
        assert_eq!(title_from_branch("add-login"), "Add login");
        // A branch that is only a tag-shaped word is still its own title.
        assert_eq!(title_from_branch("k3f9"), "K3f9");
    }

    #[test]
    fn the_body_closes_the_issue_the_branch_is_named_for() {
        assert_eq!(body_for("142-fix-the-login"), "Closes #142\n");
        assert_eq!(body_for("fix-the-login"), "");
    }

    #[test]
    fn the_note_started_in_the_worktree_names_the_pull_request() {
        let mut started = note("Fix the login redirect");
        started.done = Some(Done {
            at_ms: 1,
            worktree: Some(WorktreeId::new("wt-1")),
            branch: Some("fix-the-login-k3f9".to_owned()),
        });
        let other = note("Something else");
        assert_eq!(
            title_for(&worktree("fix-the-login-k3f9"), &[other, started]),
            "Fix the login redirect"
        );
    }

    #[test]
    fn a_note_for_the_branches_issue_names_it_when_none_was_started_here() {
        let mut issue = note("Login redirects to the wrong page");
        issue.issue = Some(IssueLink {
            number: 142,
            url: "https://github.com/o/r/issues/142".to_owned(),
        });
        assert_eq!(
            title_for(&worktree("142-fix-the-login-k3f9"), &[issue]),
            "Login redirects to the wrong page"
        );
    }

    #[test]
    fn with_no_note_the_branch_is_the_title() {
        assert_eq!(
            title_for(&worktree("142-fix-the-login-k3f9"), &[note("Unrelated")]),
            "Fix the login"
        );
    }
}
