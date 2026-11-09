//! A project's backlog of notes.

use clap::Subcommand;
use ket_core::KetError;
use ket_core::backlog::{Backlog, Done, Note, normalised_tags, repo_dir};
use ket_core::command::{self, Category, Invocation, Registry};
use ket_core::id::ProjectId;
use ket_core::workspace::Workspace;

use crate::cwd;

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    registry.register(
        command::Command::new(
            "backlog.start",
            "Start a backlog note in a new worktree, as a tab in the ket window",
            Category::Backlog,
        )
        .arg(command::Arg::optional("project", command::ArgKind::Project))
        .arg(command::Arg::required("note", command::ArgKind::Text))
        .arg(command::Arg::optional("agent", command::ArgKind::Text)),
    )?;

    registry.bind("backlog.start", |invocation, _| {
        backlog_start(
            invocation.text_of("project"),
            invocation.require_text("note")?,
            invocation.text_of("agent"),
        )
    })?;

    registry.bind("backlog.add", |invocation, _| {
        backlog_add(
            invocation.text_of("project"),
            invocation.require_text("title")?,
            invocation.text_of("body"),
            invocation.text_of("tags"),
        )
    })?;

    registry.bind("backlog.list", |invocation, _| {
        backlog_list(
            invocation.text_of("project"),
            invocation.is_set("all"),
            invocation.text_of("tag"),
            invocation.text_of("search"),
        )
    })?;

    registry.bind("backlog.done", |invocation, _| {
        backlog_done(
            invocation.text_of("project"),
            invocation.require_text("note")?,
        )
    })?;

    registry.bind("backlog.reopen", |invocation, _| {
        backlog_reopen(
            invocation.text_of("project"),
            invocation.require_text("note")?,
        )
    })?;

    registry.bind("backlog.remove", |invocation, _| {
        backlog_remove(
            invocation.text_of("project"),
            invocation.require_text("note")?,
        )
    })?;

    registry.bind("backlog.tag", |invocation, _| {
        backlog_tag(
            invocation.text_of("project"),
            invocation.require_text("note")?,
            invocation.require_text("tags")?,
            invocation.is_set("remove"),
        )
    })?;

    registry.bind("backlog.move", |invocation, _| {
        backlog_move(
            invocation.text_of("project"),
            invocation.require_text("note")?,
            invocation.text_of("before"),
        )
    })?;

    registry.bind("backlog.store", |invocation, _| {
        backlog_store(
            invocation.text_of("project"),
            invocation.require_text("location")?,
        )
    })?;

    Ok(())
}

impl BacklogCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            BacklogCmd::Add {
                title,
                body,
                tags,
                project,
            } => Invocation::new("backlog.add")
                .maybe_text("project", project)
                .text("title", title)
                .maybe_text("body", body)
                .maybe_text("tags", (!tags.is_empty()).then(|| lines(&tags))),
            BacklogCmd::List {
                project,
                all,
                tag,
                search,
            } => Invocation::new("backlog.list")
                .maybe_text("project", project)
                .flag("all", all)
                .maybe_text("tag", tag)
                .maybe_text("search", search),
            BacklogCmd::Start {
                note,
                agent,
                project,
            } => Invocation::new("backlog.start")
                .maybe_text("project", project)
                .text("note", note)
                .maybe_text("agent", agent),
            BacklogCmd::Done { note, project } => Invocation::new("backlog.done")
                .maybe_text("project", project)
                .text("note", note),
            BacklogCmd::Reopen { note, project } => Invocation::new("backlog.reopen")
                .maybe_text("project", project)
                .text("note", note),
            BacklogCmd::Rm { note, project } => Invocation::new("backlog.remove")
                .maybe_text("project", project)
                .text("note", note),
            BacklogCmd::Tag {
                note,
                tags,
                remove,
                project,
            } => Invocation::new("backlog.tag")
                .maybe_text("project", project)
                .text("note", note)
                .text("tags", lines(&tags))
                .flag("remove", remove),
            BacklogCmd::Move {
                note,
                before,
                project,
            } => Invocation::new("backlog.move")
                .maybe_text("project", project)
                .text("note", note)
                .maybe_text("before", before),
            BacklogCmd::Store { location, project } => Invocation::new("backlog.store")
                .maybe_text("project", project)
                .text("location", location),
        }
    }
}

#[derive(Subcommand, Debug)]
pub(super) enum BacklogCmd {
    /// Add a note to a project's backlog.
    Add {
        /// What the list shows, and the first line an agent is told.
        title: String,
        /// The description: what an agent is told, after the title.
        #[arg(long, short)]
        body: Option<String>,
        /// A tag to find it by. Repeat it for more than one.
        #[arg(long = "tag", short, value_name = "TAG")]
        tags: Vec<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// List a project's backlog notes, most pressing first.
    List {
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
        /// Include notes already marked done.
        #[arg(long)]
        all: bool,
        /// Only the notes with this tag.
        #[arg(long, short)]
        tag: Option<String>,
        /// Only the notes holding every word of this, in the title, the
        /// description or a tag. `#word` asks for a tag.
        #[arg(long, short)]
        search: Option<String>,
    },
    /// Add tags to a note, or take them off.
    Tag {
        /// Note id, from `ket backlog list`.
        note: String,
        /// The tags. A leading `#` is dropped.
        #[arg(required = true)]
        tags: Vec<String>,
        /// Take these tags off instead.
        #[arg(long)]
        remove: bool,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Move a note in the list: before another, or to the end of its
    /// priority.
    ///
    /// Moved before a note of another priority, it takes that priority.
    Move {
        /// Note id, from `ket backlog list`.
        note: String,
        /// The note to put it before. Without it, the note goes to the end of
        /// its priority.
        #[arg(long)]
        before: Option<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Keep a project's backlog privately or in its repository.
    ///
    /// `repo` keeps it in `.ket/backlog/` in the repository, a file per note,
    /// to be committed and shared with a team; `private` keeps it in ket's
    /// data directory, as a new project does. The notes and their files are
    /// moved across, and a note on both sides keeps its latest edit.
    Store {
        /// `private` or `repo`.
        #[arg(value_parser = ["private", "repo"])]
        location: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Start a note in a new worktree, as the backlog's Start does.
    ///
    /// The open ket window makes a worktree named after the note, opens the
    /// agent there with the note and its files, and marks the note done once
    /// the worktree exists. Needs ket open.
    Start {
        /// Note id, from `ket backlog list`.
        note: String,
        /// Agent to run. Defaults to the project's preferred agent.
        #[arg(long, short)]
        agent: Option<String>,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Mark a note done by hand — for work that happened somewhere else.
    Done {
        /// Note id, from `ket backlog list`.
        note: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Put a done note back among the open ones.
    Reopen {
        /// Note id, from `ket backlog list --all`.
        note: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
    /// Delete a note and its files.
    Rm {
        /// Note id, from `ket backlog list`.
        note: String,
        /// Project id or name. Defaults to the current directory's project.
        #[arg(long, short)]
        project: Option<String>,
    },
}

/// The note `id` refers to, or a clear error — `Backlog`'s own mutators treat
/// an unknown id as a no-op, which would otherwise have the CLI print a
/// success message for nothing happening.
pub(super) fn find_note(project: &ProjectId, id: &str) -> Result<(), KetError> {
    let backlog = Backlog::load(project)?;
    if backlog.notes.iter().any(|note| note.id == id) {
        Ok(())
    } else {
        Err(KetError::Conflict(format!("no backlog note {id}")))
    }
}

/// Several values as one registry argument, a line each — an argument is
/// text or a switch, and a tag never holds a line break.
pub(super) fn lines(values: &[String]) -> String {
    values.join("\n")
}

/// A note's tags as `list` prints them after its title: `  #one #two`, or
/// nothing.
pub(super) fn tag_list(tags: &[String]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let tags: Vec<String> = tags.iter().map(|tag| format!("#{tag}")).collect();
    format!("  {}", tags.join(" "))
}

/// Handles `ket backlog add`.
pub(super) fn backlog_add(
    project: Option<&str>,
    title: &str,
    body: Option<&str>,
    tags: Option<&str>,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let existing = Backlog::load(&resolved.id)?;
    let mut note = Note::new(&existing.notes);
    note.title = title.to_owned();
    note.body = body.unwrap_or_default().to_owned();
    note.tags = tags.map_or_else(Vec::new, |tags| normalised_tags(tags.lines()));
    Backlog::save_note(&resolved.id, &note)?;
    println!("{}  {}{}", note.id, note.title, tag_list(&note.tags));
    Ok(())
}

/// Handles `ket backlog list`.
///
/// In the order the backlog is shown — most pressing first, then as moved —
/// and the done notes after, most recently done first.
pub(super) fn backlog_list(
    project: Option<&str>,
    all: bool,
    tag: Option<&str>,
    search: Option<&str>,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::load(&resolved.id)?;
    // `--tag` is a `#tag` in the search, spelt the way a tag is kept.
    let mut query = search.unwrap_or_default().to_owned();
    for tag in normalised_tags(tag) {
        query.push_str(" #");
        query.push_str(&tag);
    }
    let done = if all {
        backlog.sorted_done()
    } else {
        Vec::new()
    };
    let notes: Vec<&Note> = backlog
        .sorted_open()
        .into_iter()
        .chain(done)
        .filter(|note| note.matches(&query))
        .collect();

    if notes.is_empty() {
        if query.trim().is_empty() {
            eprintln!("ket: no backlog notes; run `ket backlog add \"<title>\"`");
        } else {
            eprintln!("ket: no backlog notes match");
        }
        return Ok(());
    }
    for note in notes {
        let mark = if note.done.is_some() { 'x' } else { ' ' };
        println!(
            "{mark} {:<10} {}{}",
            note.id,
            note.title,
            tag_list(&note.tags)
        );
    }
    Ok(())
}

/// Handles `ket backlog tag`.
pub(super) fn backlog_tag(
    project: Option<&str>,
    note: &str,
    tags: &str,
    remove: bool,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    let given = normalised_tags(tags.lines());
    let backlog = Backlog::update(&resolved.id, |backlog| {
        let Some(saved) = backlog.notes.iter_mut().find(|saved| saved.id == note) else {
            return;
        };
        let tags = if remove {
            let gone: Vec<String> = given.iter().map(|tag| tag.to_lowercase()).collect();
            let mut kept = saved.tags.clone();
            kept.retain(|tag| !gone.contains(&tag.to_lowercase()));
            kept
        } else {
            normalised_tags(saved.tags.iter().chain(&given))
        };
        if tags != saved.tags {
            saved.tags = tags;
            saved.updated_ms = ket_core::now_ms();
        }
    })?;
    let kept = backlog
        .notes
        .iter()
        .find(|saved| saved.id == note)
        .map(|saved| saved.tags.clone())
        .unwrap_or_default();
    if !remove {
        let folded: Vec<String> = kept.iter().map(|tag| tag.to_lowercase()).collect();
        if given
            .iter()
            .any(|tag| !folded.contains(&tag.to_lowercase()))
        {
            eprintln!(
                "ket: a note keeps at most {} tags; the rest were not added",
                ket_core::backlog::MAX_TAGS
            );
        }
    }
    if kept.is_empty() {
        println!("{note}  no tags");
    } else {
        println!("{note}{}", tag_list(&kept));
    }
    Ok(())
}

/// Handles `ket backlog move`.
pub(super) fn backlog_move(
    project: Option<&str>,
    note: &str,
    before: Option<&str>,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::reorder(&resolved.id, note, before)?;
    let priority = backlog
        .notes
        .iter()
        .find(|saved| saved.id == note)
        .map(|saved| saved.priority.name())
        .unwrap_or_default();
    match before {
        Some(other) => println!("moved {note} before {other}, in {priority}"),
        None => println!("moved {note} to the end of {priority}"),
    }
    Ok(())
}

/// Handles `ket backlog store`.
pub(super) fn backlog_store(project: Option<&str>, location: &str) -> Result<(), KetError> {
    let in_repo = match location {
        "repo" => true,
        "private" => false,
        other => {
            return Err(KetError::Config(format!(
                "store must be `private` or `repo`, not {other:?}"
            )));
        }
    };
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::set_location(&resolved.id, in_repo)?;
    let count = backlog.notes.len();
    if in_repo {
        println!(
            "{count} note(s) in {} — commit it to share it",
            repo_dir(&resolved.root).display()
        );
    } else {
        println!("{count} note(s), kept privately");
    }
    Ok(())
}

/// Handles `ket backlog done`.
pub(super) fn backlog_done(project: Option<&str>, note: &str) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    Backlog::mark_done(&resolved.id, note, Done::by_hand())?;
    println!("done {note}");
    Ok(())
}

/// Handles `ket backlog start`: the note's brief to the open window, which
/// builds the work from the note itself, files and all.
pub(super) fn backlog_start(
    project: Option<&str>,
    note: &str,
    agent: Option<&str>,
) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    let backlog = Backlog::load(&resolved.id)?;
    let Some(found) = backlog.notes.iter().find(|candidate| candidate.id == note) else {
        return Err(KetError::Conflict(format!("no backlog note {note}")));
    };
    if found.done.is_some() {
        return Err(KetError::Conflict(format!(
            "{note} is already started or done; `ket backlog reopen {note}` puts it back"
        )));
    }
    let Some(brief) = found.brief() else {
        return Err(KetError::Conflict(format!(
            "{note} says nothing to hand an agent yet"
        )));
    };
    ket_core::host::start_work(resolved.id.as_str(), &brief, agent, Some(note))?;
    println!(
        "starting {note} in {} — `ket worktree list` shows its worktree once ket has made it",
        resolved.name
    );
    Ok(())
}

/// Handles `ket backlog reopen`.
pub(super) fn backlog_reopen(project: Option<&str>, note: &str) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    Backlog::reopen(&resolved.id, note)?;
    println!("reopened {note}");
    Ok(())
}

/// Handles `ket backlog rm`.
pub(super) fn backlog_remove(project: Option<&str>, note: &str) -> Result<(), KetError> {
    let resolved = Workspace::open()?.resolve_project(project, &cwd())?;
    find_note(&resolved.id, note)?;
    Backlog::remove(&resolved.id, note)?;
    println!("removed {note}");
    Ok(())
}

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "backlog", "add", "fix the thing"],
    &[
        "ket",
        "backlog",
        "add",
        "fix the thing",
        "--body",
        "details",
    ],
    &["ket", "backlog", "list"],
    &["ket", "backlog", "list", "--all"],
    &["ket", "backlog", "start", "note-1"],
    &["ket", "backlog", "start", "note-1", "--agent", "codex"],
    &["ket", "backlog", "done", "note-1"],
    &["ket", "backlog", "reopen", "note-1"],
    &["ket", "backlog", "rm", "note-1"],
    &["ket", "backlog", "tag", "note-1", "ui"],
    &["ket", "backlog", "tag", "note-1", "ui", "--remove"],
    &["ket", "backlog", "move", "note-1", "--before", "note-2"],
    &["ket", "backlog", "store", "repo"],
];
