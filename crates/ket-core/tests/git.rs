//! `Git`, exercised directly rather than through `Workspace` — the merge
//! probes, remote helpers and ref plumbing that nothing else happens to call.

use std::fs;

use ket_core::git::{Fetch, Git, LocalBaseRefresh, PreparedBase, discover_root};

mod common;
use common::{Sandbox, commit_file, git, init_repo};

#[test]
fn discover_root_finds_the_top_from_a_subdirectory() {
    let sandbox = Sandbox::new("git-discover-root");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("a/b")).unwrap();

    let found = discover_root(&repo.join("a/b")).unwrap();
    assert_eq!(found, repo.canonicalize().unwrap());
}

#[test]
fn discover_root_refuses_a_plain_directory() {
    let sandbox = Sandbox::new("git-discover-root-none");
    let dir = sandbox.path("not-a-repo");
    fs::create_dir_all(&dir).unwrap();
    assert!(discover_root(&dir).is_err());
}

#[test]
fn rev_parse_resolves_head_and_rejects_nonsense() {
    let sandbox = Sandbox::new("git-rev-parse");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    let head = g.rev_parse("HEAD").unwrap();
    assert_eq!(head.len(), 40);
    assert!(g.rev_parse("not-a-thing").is_err());
}

#[test]
fn branch_exists_is_accurate() {
    let sandbox = Sandbox::new("git-branch-exists");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert!(g.branch_exists("main"));
    assert!(!g.branch_exists("nope"));
}

#[test]
fn default_branch_names_the_checked_out_branch() {
    let sandbox = Sandbox::new("git-default-branch");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert_eq!(g.default_branch().unwrap(), "main");
}

#[test]
fn is_dirty_reacts_to_tracked_changes_only() {
    let sandbox = Sandbox::new("git-is-dirty");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert!(!g.is_dirty().unwrap());
    fs::write(repo.join("untracked.txt"), "x").unwrap();
    assert!(
        g.is_dirty().unwrap(),
        "untracked files count too, unlike has_tracked_changes"
    );
}

#[test]
fn current_branch_is_none_when_detached() {
    let sandbox = Sandbox::new("git-current-branch");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert_eq!(g.current_branch().unwrap().as_deref(), Some("main"));

    let head = g.rev_parse("HEAD").unwrap();
    git(&repo, &["checkout", "--quiet", "--detach", &head]);
    assert_eq!(g.current_branch().unwrap(), None);
}

#[test]
fn is_ancestor_is_true_only_in_the_right_direction() {
    let sandbox = Sandbox::new("git-is-ancestor");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "done\n", "feature work");

    assert!(g.is_ancestor("main", "feature"));
    assert!(!g.is_ancestor("feature", "main"));
}

#[test]
fn untracked_lists_only_untracked_files() {
    let sandbox = Sandbox::new("git-untracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    fs::write(repo.join("fresh.txt"), "x").unwrap();
    let found = g.untracked().unwrap();
    assert_eq!(found, vec!["fresh.txt".to_owned()]);
}

#[test]
fn diff_new_file_shows_an_untracked_files_whole_content_as_added() {
    let sandbox = Sandbox::new("git-diff-new-file");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    fs::write(repo.join("fresh.txt"), "hello\n").unwrap();
    let diff = g.diff_new_file("fresh.txt").unwrap();
    assert!(diff.contains("+hello"), "{diff}");
}

#[test]
fn merge_base_finds_the_fork_point() {
    let sandbox = Sandbox::new("git-merge-base");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let fork = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "done\n", "feature work");

    let g = Git::new(&repo);
    assert_eq!(g.merge_base("main", "feature").unwrap(), fork);
}

#[test]
fn line_changes_counts_additions_and_deletions() {
    let sandbox = Sandbox::new("git-line-changes");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("README.md"), "one\ntwo\nthree\n").unwrap();

    let g = Git::new(&repo);
    let (added, removed) = g.line_changes().unwrap();
    assert_eq!(added, 3);
    assert_eq!(
        removed, 1,
        "the original README line was replaced, not kept"
    );
}

#[test]
fn stage_all_and_commit_staged_round_trip() {
    let sandbox = Sandbox::new("git-stage-commit");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert_eq!(g.commit_staged("nothing to commit").unwrap(), None);

    fs::write(repo.join("a.txt"), "a\n").unwrap();
    fs::write(repo.join("b.txt"), "b\n").unwrap();
    assert_eq!(g.stage_all().unwrap(), 2);

    let sha = g.commit_staged("add files").unwrap();
    assert!(sha.is_some());
    assert!(!g.is_dirty().unwrap());
}

#[test]
fn commit_all_reports_how_many_files_changed() {
    let sandbox = Sandbox::new("git-commit-all");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "a\n").unwrap();
    fs::write(repo.join("b.txt"), "b\n").unwrap();

    let g = Git::new(&repo);
    let report = g.commit_all("work").unwrap();
    assert_eq!(report.files, 2);
    assert!(!g.is_dirty().unwrap());
}

#[test]
fn worktree_add_existing_checks_out_an_established_branch() {
    let sandbox = Sandbox::new("git-worktree-add-existing");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "feature"]);

    let g = Git::new(&repo);
    let path = sandbox.path("wt");
    g.worktree_add_existing(&path, "feature").unwrap();
    assert!(path.join("README.md").is_file());
}

#[test]
fn submodule_status_is_empty_without_submodules() {
    let sandbox = Sandbox::new("git-submodule-status");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    assert!(g.submodule_status().unwrap().is_empty());
}

#[test]
fn config_get_reads_a_set_value_and_none_for_unset() {
    let sandbox = Sandbox::new("git-config-get");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert_eq!(
        g.config_get("user.email").as_deref(),
        Some("test@example.com")
    );
    assert_eq!(g.config_get("does.not.exist"), None);
}

#[test]
fn symbolic_ref_resolves_head() {
    let sandbox = Sandbox::new("git-symbolic-ref");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert_eq!(g.symbolic_ref("HEAD").as_deref(), Some("refs/heads/main"));
    assert_eq!(g.symbolic_ref("refs/remotes/origin/HEAD"), None);
}

#[test]
fn commit_of_and_tree_of_resolve_a_rev() {
    let sandbox = Sandbox::new("git-commit-tree-of");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    let head = g.rev_parse("HEAD").unwrap();
    assert_eq!(g.commit_of("HEAD").as_deref(), Some(head.as_str()));
    assert!(g.tree_of("HEAD").is_some());
    assert_eq!(g.commit_of("not-a-thing"), None);
}

#[test]
fn merge_tree_finds_a_clean_merges_tree_and_none_on_conflict() {
    let sandbox = Sandbox::new("git-merge-tree");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "shared.txt", "base\n", "add shared");
    let g = Git::new(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "only-feature.txt", "x\n", "feature work");
    git(&repo, &["checkout", "--quiet", "main"]);

    assert!(g.merge_tree("main", "feature").is_some());

    git(&repo, &["checkout", "--quiet", "feature"]);
    commit_file(
        &repo,
        "shared.txt",
        "from feature\n",
        "feature edits shared",
    );
    git(&repo, &["checkout", "--quiet", "main"]);
    commit_file(&repo, "shared.txt", "from main\n", "main edits shared");

    assert_eq!(g.merge_tree("main", "feature"), None);
}

#[test]
fn cherry_tells_new_commits_from_ones_already_upstream() {
    let sandbox = Sandbox::new("git-cherry");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "done\n", "feature work");

    let g = Git::new(&repo);
    let entries = g.cherry("main", "feature").unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].starts_with('+'), "{:?}", entries[0]);
}

#[test]
fn remotes_and_base_remote_without_any_remote() {
    let sandbox = Sandbox::new("git-remotes-none");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert!(g.remotes().unwrap().is_empty());
    assert_eq!(g.base_remote(), None);
}

#[test]
fn base_remote_prefers_origin_over_other_remotes() {
    let sandbox = Sandbox::new("git-base-remote-origin");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "upstream", "https://example.com/u.git"],
    );
    git(
        &repo,
        &["remote", "add", "origin", "https://example.com/o.git"],
    );

    let g = Git::new(&repo);
    assert_eq!(g.base_remote().as_deref(), Some("origin"));
}

#[test]
fn base_remote_falls_back_to_the_first_remote_without_origin() {
    let sandbox = Sandbox::new("git-base-remote-fallback");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "upstream", "https://example.com/u.git"],
    );

    let g = Git::new(&repo);
    assert_eq!(g.base_remote().as_deref(), Some("upstream"));
}

#[test]
fn ref_exists_checks_fully_qualified_refs() {
    let sandbox = Sandbox::new("git-ref-exists");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert!(g.ref_exists("refs/heads/main"));
    assert!(!g.ref_exists("refs/heads/nope"));
}

#[test]
fn ahead_behind_counts_both_directions() {
    let sandbox = Sandbox::new("git-ahead-behind");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "a.txt", "a\n", "feature commit 1");
    commit_file(&repo, "b.txt", "b\n", "feature commit 2");
    git(&repo, &["checkout", "--quiet", "main"]);
    commit_file(&repo, "c.txt", "c\n", "main commit");

    let g = Git::new(&repo);
    let (ahead, behind) = g.ahead_behind("feature", "main").unwrap();
    assert_eq!(ahead, 2);
    assert_eq!(behind, 1);
}

#[test]
fn checkout_of_finds_the_worktree_with_a_branch_out() {
    let sandbox = Sandbox::new("git-checkout-of");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "feature"]);
    let g = Git::new(&repo);

    assert_eq!(
        g.checkout_of("main").unwrap(),
        Some(repo.canonicalize().unwrap())
    );
    assert_eq!(g.checkout_of("feature").unwrap(), None);
}

#[test]
fn update_ref_moves_a_ref_only_when_it_is_still_at_old() {
    let sandbox = Sandbox::new("git-update-ref");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let start = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    commit_file(&repo, "a.txt", "a\n", "advance main");
    let after = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let g = Git::new(&repo);

    // main is at `after`, not `start`, so a swap expecting `start` is refused.
    assert!(g.update_ref("refs/heads/main", &start, &start).is_err());
    assert_eq!(git(&repo, &["rev-parse", "main"]).trim(), after);

    // Expecting the value it actually holds succeeds.
    assert!(g.update_ref("refs/heads/main", &start, &after).is_ok());
    assert_eq!(git(&repo, &["rev-parse", "main"]).trim(), start);
}

#[test]
fn reset_hard_moves_head_and_discards_changes() {
    let sandbox = Sandbox::new("git-reset-hard");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let start = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    commit_file(&repo, "a.txt", "a\n", "extra commit");
    fs::write(repo.join("README.md"), "dirty\n").unwrap();

    let g = Git::new(&repo);
    g.reset_hard(&start).unwrap();

    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).trim(), start);
    assert!(!g.is_dirty().unwrap());
    assert!(!repo.join("a.txt").exists());
}

// ---- PreparedBase::notice() — pure, no repo needed --------------------------

fn base(fetch: Fetch, refresh: LocalBaseRefresh) -> PreparedBase {
    PreparedBase {
        asked: "main".to_owned(),
        rev: "main".to_owned(),
        fetch,
        refresh,
    }
}

#[test]
fn notice_is_silent_when_nothing_worth_saying_happened() {
    let b = base(Fetch::NoRemote, LocalBaseRefresh::NotApplicable);
    assert_eq!(b.notice(), None);

    let b = base(
        Fetch::Fetched {
            remote: "origin".to_owned(),
        },
        LocalBaseRefresh::UpToDate,
    );
    assert_eq!(b.notice(), None);
}

#[test]
fn notice_reports_a_failed_fetch() {
    let b = base(
        Fetch::Failed {
            remote: "origin".to_owned(),
            why: "timed out".to_owned(),
        },
        LocalBaseRefresh::NotApplicable,
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("could not fetch origin"), "{notice}");
    assert!(notice.contains("timed out"), "{notice}");
}

#[test]
fn notice_reports_a_fast_forward_with_and_without_a_checkout() {
    let b = base(
        Fetch::NoRemote,
        LocalBaseRefresh::Updated {
            behind: 1,
            checkout: None,
        },
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("1 commit behind"), "{notice}");
    assert!(!notice.contains("commits"), "singular for one: {notice}");

    let b = base(
        Fetch::NoRemote,
        LocalBaseRefresh::Updated {
            behind: 3,
            checkout: Some(std::path::PathBuf::from("/tmp/wt")),
        },
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("3 commits behind"), "{notice}");
    assert!(notice.contains("/tmp/wt"), "{notice}");
}

#[test]
fn notice_is_silent_for_local_ahead_with_nothing_behind() {
    let b = base(
        Fetch::NoRemote,
        LocalBaseRefresh::LocalAhead {
            ahead: 2,
            behind: 0,
        },
    );
    assert_eq!(b.notice(), None);
}

#[test]
fn notice_reports_local_ahead_and_behind_together() {
    let b = base(
        Fetch::NoRemote,
        LocalBaseRefresh::LocalAhead {
            ahead: 2,
            behind: 1,
        },
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("2 commits"), "{notice}");
    assert!(notice.contains("1 behind"), "{notice}");
}

#[test]
fn notice_reports_a_dirty_checkout_left_alone() {
    let b = base(
        Fetch::NoRemote,
        LocalBaseRefresh::DirtyCheckout {
            behind: 1,
            checkout: std::path::PathBuf::from("/tmp/wt"),
        },
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("uncommitted changes"), "{notice}");
    assert!(notice.contains("/tmp/wt"), "{notice}");
}

#[test]
fn notice_reports_a_failed_fast_forward() {
    let b = base(
        Fetch::NoRemote,
        LocalBaseRefresh::Failed {
            behind: 4,
            why: "conflict".to_owned(),
        },
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("4 commits behind"), "{notice}");
    assert!(notice.contains("conflict"), "{notice}");
}

#[test]
fn notice_combines_a_failed_fetch_and_a_local_ahead_note() {
    let b = base(
        Fetch::Failed {
            remote: "origin".to_owned(),
            why: "network".to_owned(),
        },
        LocalBaseRefresh::LocalAhead {
            ahead: 1,
            behind: 1,
        },
    );
    let notice = b.notice().unwrap();
    assert!(notice.contains("could not fetch"), "{notice}");
    assert!(notice.contains("has 1 commit"), "{notice}");
}

#[test]
fn diff_since_reports_the_working_tree_against_a_rev() {
    let sandbox = Sandbox::new("git-diff-since");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    let head = g.rev_parse("HEAD").unwrap();

    commit_file(&repo, "changed.txt", "new content\n", "change it");

    let diff = g.diff_since(&head, &[]).unwrap();
    assert!(diff.contains("changed.txt"), "{diff}");
    assert!(diff.contains("new content"), "{diff}");
}

#[test]
fn diff_since_against_the_current_head_is_empty() {
    let sandbox = Sandbox::new("git-diff-since-empty");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);

    assert!(g.diff_since("HEAD", &[]).unwrap().is_empty());
}

#[test]
fn worktree_add_creates_a_new_branch_and_worktree_list_reports_it() {
    let sandbox = Sandbox::new("git-worktree-add");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    let path = sandbox.path("wt");

    g.worktree_add(&path, "feature", "main").unwrap();

    assert!(path.join(".git").exists());
    assert!(g.branch_exists("feature"));

    let list = g.worktree_list().unwrap();
    let entry = list
        .iter()
        .find(|w| w.path == path.canonicalize().unwrap())
        .expect("the new worktree is listed");
    assert_eq!(entry.branch.as_deref(), Some("refs/heads/feature"));
    assert!(!entry.bare);
    assert!(!entry.detached);
}

#[test]
fn worktree_remove_deletes_the_checkout_and_its_registration() {
    let sandbox = Sandbox::new("git-worktree-remove");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    let path = sandbox.path("wt");
    g.worktree_add(&path, "feature", "main").unwrap();

    g.worktree_remove(&path, false).unwrap();

    assert!(!path.exists());
    assert_eq!(
        g.worktree_list().unwrap().len(),
        1,
        "only the primary checkout remains"
    );
}

#[test]
fn worktree_remove_without_force_refuses_a_dirty_checkout() {
    let sandbox = Sandbox::new("git-worktree-remove-dirty");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    let path = sandbox.path("wt");
    g.worktree_add(&path, "feature", "main").unwrap();
    fs::write(path.join("uncommitted.txt"), "work\n").unwrap();

    assert!(g.worktree_remove(&path, false).is_err());
    assert!(path.exists());

    g.worktree_remove(&path, true).unwrap();
    assert!(!path.exists());
}

#[test]
fn worktree_prune_drops_a_registration_whose_directory_is_gone() {
    let sandbox = Sandbox::new("git-worktree-prune");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let g = Git::new(&repo);
    let path = sandbox.path("wt");
    g.worktree_add(&path, "feature", "main").unwrap();
    fs::remove_dir_all(&path).unwrap();

    g.worktree_prune().unwrap();

    assert_eq!(g.worktree_list().unwrap().len(), 1);
}

#[test]
fn branch_delete_removes_a_merged_branch_without_force() {
    let sandbox = Sandbox::new("git-branch-delete");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "merged-already"]);
    let g = Git::new(&repo);

    g.branch_delete("merged-already", false).unwrap();
    assert!(!g.branch_exists("merged-already"));
}

#[test]
fn branch_delete_without_force_refuses_an_unmerged_branch() {
    let sandbox = Sandbox::new("git-branch-delete-unmerged");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let path = sandbox.path("wt");
    let g = Git::new(&repo);
    g.worktree_add(&path, "feature", "main").unwrap();
    commit_file(&path, "work.txt", "done\n", "feature work");
    g.worktree_remove(&path, true).unwrap();

    assert!(g.branch_delete("feature", false).is_err());
    assert!(g.branch_exists("feature"));

    g.branch_delete("feature", true).unwrap();
    assert!(!g.branch_exists("feature"));
}

#[test]
fn local_branches_lists_every_local_branch_and_no_remote_one() {
    let sandbox = Sandbox::new("git-local-branches");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "feature"]);
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/elsewhere", "HEAD"],
    );

    let mut branches = Git::new(&repo).local_branches().unwrap();
    branches.sort();
    assert_eq!(branches, ["feature", "main"]);
}
