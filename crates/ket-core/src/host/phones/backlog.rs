//! What a phone asks of a project's backlog: its notes, a change to one,
//! and work started from one.

use ket_remote::proto::{self, envelope::Payload};

use super::requests::{ack, envelope, error, phone_project};
use super::session::Phone;
use crate::host::server::Host;

pub(super) fn backlog_from_phone(
    phone: &Phone<'_>,
    id: u64,
    ask: &proto::GetBacklog,
) -> Option<proto::Envelope> {
    let Some(project) = phone_project(phone, &ask.project_id) else {
        return Some(no_project(id));
    };
    Some(match crate::backlog::Backlog::load(&project) {
        Ok(backlog) => backlog_reply(id, &ask.project_id, backlog),
        Err(why) => error(
            id,
            proto::ErrorCode::Unspecified,
            &format!("could not read the backlog: {why}"),
        ),
    })
}

pub(super) fn edit_from_phone(
    phone: &Phone<'_>,
    id: u64,
    edit: &proto::EditBacklog,
) -> Option<proto::Envelope> {
    let Some(project) = phone_project(phone, &edit.project_id) else {
        return Some(no_project(id));
    };
    let edited = edit_backlog(&project, edit);
    phone.phones.forget_backlogs();
    Some(match edited {
        Ok(backlog) => backlog_reply(id, &edit.project_id, backlog),
        Err((code, why)) => error(id, code, &why),
    })
}

pub(super) fn start_note_from_phone(
    host: &Host,
    phone: &Phone<'_>,
    id: u64,
    start: &proto::StartNote,
) -> Option<proto::Envelope> {
    let Some(project) = phone_project(phone, &start.project_id) else {
        return Some(no_project(id));
    };
    let note = match crate::backlog::Backlog::load(&project) {
        Ok(backlog) => backlog
            .notes
            .into_iter()
            .find(|note| note.id == start.note_id),
        Err(why) => {
            return Some(error(
                id,
                proto::ErrorCode::Unspecified,
                &format!("could not read the backlog: {why}"),
            ));
        }
    };
    let Some(note) = note else {
        return Some(no_note(id));
    };
    if note.done.is_some() {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "that note has already been started or done",
        ));
    }
    // What a window from before `note` existed starts on: the words
    // the desktop's own Start hands the agent, without the files.
    let words: Vec<&str> = [note.title.trim(), note.body.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if words.is_empty() {
        return Some(error(
            id,
            proto::ErrorCode::Unspecified,
            "write what the work is first",
        ));
    }
    let started = host.start_work(
        start.project_id.clone(),
        words.join("\n\n"),
        None,
        Some(note.id),
        crate::host::wire::WorkSource::Phone,
    );
    if started {
        ack(id)
    } else {
        Some(error(
            id,
            proto::ErrorCode::NotFound,
            "open ket on your Mac to start work from here",
        ))
    }
}

pub(super) fn no_project(id: u64) -> proto::Envelope {
    error(
        id,
        proto::ErrorCode::NotFound,
        "no such project on this desktop",
    )
}

fn no_note(id: u64) -> proto::Envelope {
    error(
        id,
        proto::ErrorCode::NotFound,
        "no such note in the backlog",
    )
}

/// A project's backlog, as a phone is sent it.
fn backlog_reply(id: u64, project: &str, backlog: crate::backlog::Backlog) -> proto::Envelope {
    let notes = backlog
        .notes
        .into_iter()
        .map(|note| proto::BacklogNote {
            done_ms: note.done.as_ref().map_or(0, |done| done.at_ms),
            branch: note.done.and_then(|done| done.branch).unwrap_or_default(),
            id: note.id,
            title: note.title,
            body: note.body,
            files: note.attachments,
            created_ms: note.created_ms,
            updated_ms: note.updated_ms,
            priority: wire_priority(note.priority).into(),
        })
        .collect();
    envelope(
        id,
        Payload::Backlog(proto::Backlog {
            project_id: project.to_owned(),
            notes,
        }),
    )
}

/// A note's priority as a phone is sent it.
fn wire_priority(priority: crate::backlog::Priority) -> proto::BacklogPriority {
    use crate::backlog::Priority;
    match priority {
        Priority::Low => proto::BacklogPriority::Low,
        Priority::Medium => proto::BacklogPriority::Medium,
        Priority::High => proto::BacklogPriority::High,
        Priority::Urgent => proto::BacklogPriority::Urgent,
    }
}

/// The priority a phone asked for, or `None` when it named none — an older
/// phone, or a level this desktop does not know.
fn asked_priority(wire: i32) -> Option<crate::backlog::Priority> {
    use crate::backlog::Priority;
    match proto::BacklogPriority::try_from(wire).ok()? {
        proto::BacklogPriority::Unspecified => None,
        proto::BacklogPriority::Low => Some(Priority::Low),
        proto::BacklogPriority::Medium => Some(Priority::Medium),
        proto::BacklogPriority::High => Some(Priority::High),
        proto::BacklogPriority::Urgent => Some(Priority::Urgent),
    }
}

/// Applies one phone's change to a note — see [`proto::EditBacklog`].
fn edit_backlog(
    project: &crate::id::ProjectId,
    edit: &proto::EditBacklog,
) -> std::result::Result<crate::backlog::Backlog, (proto::ErrorCode, String)> {
    use crate::backlog::{Backlog, Done, Note};
    use proto::edit_backlog::Action;

    let failed = |why: crate::KetError| (proto::ErrorCode::Unspecified, why.to_string());
    let saved = Backlog::load(project).map_err(failed)?;
    let action = Action::try_from(edit.action).unwrap_or(Action::Unspecified);
    let exists = saved.notes.iter().any(|note| note.id == edit.note_id);
    // Every change but a new note names one that is there: the library
    // would otherwise do nothing, or — for a save — add it.
    let new = action == Action::Save && edit.note_id.is_empty();
    if !new && !exists {
        return Err((
            proto::ErrorCode::NotFound,
            "no such note in the backlog".to_owned(),
        ));
    }
    match action {
        Action::Save => {
            let title = edit.title.trim();
            let body = edit.body.trim();
            if title.is_empty() && body.is_empty() {
                return Err((
                    proto::ErrorCode::Unspecified,
                    "write what the work is first".to_owned(),
                ));
            }
            // An existing note starts as saved, so what a phone is not sent —
            // its tags — is saved back as it was rather than cleared.
            let mut note = match new {
                true => Note::new(&saved.notes),
                false => saved
                    .notes
                    .iter()
                    .find(|note| note.id == edit.note_id)
                    .cloned()
                    .unwrap_or_default(),
            };
            title.clone_into(&mut note.title);
            body.clone_into(&mut note.body);
            // A phone that names no priority leaves the note's as it was.
            if let Some(priority) = asked_priority(edit.priority) {
                note.priority = priority;
            }
            Backlog::save_note(project, &note).map_err(failed)
        }
        Action::Done => Backlog::mark_done(project, &edit.note_id, Done::by_hand()).map_err(failed),
        Action::Reopen => Backlog::reopen(project, &edit.note_id).map_err(failed),
        Action::Remove => Backlog::remove(project, &edit.note_id).map_err(failed),
        Action::Unspecified => Err((
            proto::ErrorCode::Unspecified,
            "not a change this desktop knows; update ket".to_owned(),
        )),
    }
}
