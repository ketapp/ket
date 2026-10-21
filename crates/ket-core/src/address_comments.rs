//! Unresolved review comments, handed to the agent in its worktree.
//!
//! "Address comments with agent" — on the desktop's worktree menu, and on a
//! phone — reads the pull request's unresolved review comments, line
//! comments included, and types one message naming each into the worktree's
//! agent. The agent is told to address them and push with `ket pr`, the
//! way Fix checks with agent hands over failed checks.

use std::fmt::Write as _;

use crate::Result;
use crate::github::{self, Checkout, PullRequestStatus, ReviewComment};
use crate::id::WorktreeId;

/// What the agent in `worktree` is told about `status`'s unresolved review
/// comments: the prompt to type, after reading them as `at`'s account.
///
/// Fails only when the comments cannot be read. An empty list still produces
/// a prompt asking the agent to read the conversation, so a "changes
/// requested" review with no line comments is not a dead end.
pub fn hand_over(
    at: &Checkout,
    worktree: &WorktreeId,
    status: &PullRequestStatus,
) -> Result<String> {
    let comments = github::unresolved_review_comments(at, status.number)?;
    Ok(prompt(status.number, worktree, &comments))
}

/// The words the agent is given — see [`hand_over`].
fn prompt(number: u64, worktree: &WorktreeId, comments: &[ReviewComment]) -> String {
    let mut prompt = if comments.is_empty() {
        format!(
            "Reviewers requested changes on pull request #{number}. Read the conversation \
             and the review comments on it."
        )
    } else {
        let mut prompt = format!("Pull request #{number} has unresolved review comments:\n");
        for comment in comments {
            prompt.push('\n');
            match (comment.path.as_str(), comment.line, comment.author.as_str()) {
                (path, Some(line), author) if !path.is_empty() && !author.is_empty() => {
                    let _ = write!(prompt, "{path}:{line} (@{author}):");
                }
                (path, Some(line), _) if !path.is_empty() => {
                    let _ = write!(prompt, "{path}:{line}:");
                }
                (path, None, author) if !path.is_empty() && !author.is_empty() => {
                    let _ = write!(prompt, "{path} (@{author}):");
                }
                (_, _, author) if !author.is_empty() => {
                    let _ = write!(prompt, "(@{author}):");
                }
                _ => prompt.push_str("Comment:"),
            }
            let _ = write!(prompt, "\n{}\n", comment.body.trim());
        }
        prompt
    };
    if comments.is_empty() {
        let _ = write!(
            prompt,
            " Address each one. Commit the fixes, then run `ket pr {worktree}` to push."
        );
    } else {
        let _ = write!(
            prompt,
            "\nAddress each one. Commit the fixes, then run `ket pr {worktree}` to push."
        );
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(author: &str, path: &str, line: Option<u32>, body: &str) -> ReviewComment {
        ReviewComment {
            author: author.to_owned(),
            path: path.to_owned(),
            line,
            body: body.to_owned(),
        }
    }

    #[test]
    fn the_prompt_names_each_comment_and_how_to_push() {
        let prompt = prompt(
            17,
            &WorktreeId::new("wt-1"),
            &[
                comment("alice", "src/lib.rs", Some(42), "please rename this"),
                comment("bob", "README.md", None, "typo in the title"),
            ],
        );
        assert_eq!(
            prompt,
            "Pull request #17 has unresolved review comments:\n\
             \n\
             src/lib.rs:42 (@alice):\n\
             please rename this\n\
             \n\
             README.md (@bob):\n\
             typo in the title\n\
             \n\
             Address each one. Commit the fixes, then run `ket pr wt-1` to push."
        );
    }

    #[test]
    fn no_line_comments_still_asks_the_agent_to_read_the_conversation() {
        let prompt = prompt(9, &WorktreeId::new("wt"), &[]);
        assert_eq!(
            prompt,
            "Reviewers requested changes on pull request #9. Read the conversation \
             and the review comments on it. Address each one. Commit the fixes, then \
             run `ket pr wt` to push."
        );
    }
}
