//! A macOS notification when a worktree's agent stops on a permission prompt
//! while ket is not the app in front.
//!
//! The sidebar already pulses a waiting row; this is for when nobody is
//! looking at the sidebar. Only the move into waiting is announced: a row that
//! stays waiting is not announced again, one that stops and starts again is,
//! and nothing is said while ket is in front, where the row says it already.
//! A notification whose row is no longer waiting is taken back, so
//! Notification Center does not keep a prompt somebody has answered.
//!
//! Banners need an `.app` bundle — `UNUserNotificationCenter` throws in a
//! bare binary such as `target/debug/ket-ui`, bundle identifier or not — so
//! there the Dock icon bounces once instead. Clicking a banner brings ket to
//! the front. Going to the worktree as well would take a notification-centre
//! delegate, an Objective-C class ket would have to define, which is `unsafe`.

use std::collections::HashSet;

use ket_core::activity::Signal;
use ket_core::id::WorktreeId;

use crate::Shell;

/// Which worktrees were waiting at the last look.
#[derive(Debug, Default)]
pub(crate) struct Waiting {
    seen: HashSet<WorktreeId>,
    /// Whether there has been a look. The first only learns what was already
    /// waiting at launch: those prompts are not news.
    primed: bool,
}

/// One notification, in the words it is shown with.
struct Notice {
    /// The worktree's id, which is also the notification's, so a second
    /// window announcing the same prompt replaces the first's banner.
    id: String,
    /// Groups a project's notifications together.
    thread: String,
    title: String,
    body: String,
}

impl Shell {
    /// Announces the worktrees that have started waiting since the last look,
    /// and takes back the notifications of those that stopped. Cheap when
    /// nothing changed: the config is read only when there is news.
    pub(crate) fn announce_waiting(&mut self) {
        let now = ket_core::now_ms();
        let mut waiting = HashSet::new();
        let mut fresh = Vec::new();
        for project in &self.projects {
            for node in &project.worktrees {
                if self.row_signal(node, now) != Signal::Blocked {
                    continue;
                }
                waiting.insert(node.id.clone());
                if self.waiting.seen.contains(&node.id) {
                    continue;
                }
                let agent = self
                    .activity
                    .activity(&node.id, now)
                    .agent()
                    .map(crate::ui::agent::agent_label)
                    .unwrap_or_else(|| "An agent".to_owned());
                let worktree = node.name.as_ref().unwrap_or(&node.branch);
                fresh.push(Notice {
                    id: node.id.as_str().to_owned(),
                    thread: project.id.as_str().to_owned(),
                    title: format!("{} \u{b7} {worktree}", project.name),
                    body: format!("{agent} is waiting on you."),
                });
            }
        }

        let answered: Vec<String> = self
            .waiting
            .seen
            .difference(&waiting)
            .map(|id| id.as_str().to_owned())
            .collect();
        let primed = std::mem::replace(&mut self.waiting.primed, true);
        self.waiting.seen = waiting;

        if !answered.is_empty() {
            platform::withdraw(&answered);
        }
        if !primed || fresh.is_empty() || platform::in_front() {
            return;
        }
        let wanted = ket_core::config::Config::load().map_or(true, |c| c.general.notify_waiting);
        if wanted {
            platform::post(&fresh);
        }
    }
}

/// Asks macOS whether ket may post notifications, once, at launch: the first
/// time it shows the person the question, and afterwards it answers at once.
/// Asked up front because a banner posted before the answer is dropped.
pub(crate) fn prepare() {
    if ket_core::config::Config::load().map_or(true, |c| c.general.notify_waiting) {
        platform::authorize();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
    use objc2_foundation::{MainThreadMarker, NSArray, NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNNotificationSound, UNUserNotificationCenter,
    };

    use super::Notice;

    /// Whether this process runs from an `.app` bundle, without which the
    /// notification centre raises an Objective-C exception. An identifier is
    /// not enough to go on: `build.rs` embeds one in the bare binary for the
    /// microphone, and the centre still finds no bundle to go with it.
    fn bundled() -> bool {
        NSBundle::mainBundle()
            .bundlePath()
            .to_string()
            .ends_with(".app")
    }

    pub(super) fn in_front() -> bool {
        MainThreadMarker::new().is_some_and(|mtm| NSApplication::sharedApplication(mtm).isActive())
    }

    pub(super) fn authorize() {
        if !bundled() {
            return;
        }
        let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
        let answered = RcBlock::new(|granted: Bool, _: *mut NSError| {
            tracing::debug!(granted = granted.as_bool(), "notifications: authorization");
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(options, &answered);
    }

    pub(super) fn post(notices: &[Notice]) {
        if !bundled() {
            // No banner to show: the Dock icon bounces once instead.
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm)
                    .requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
            }
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        for notice in notices {
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(&notice.title));
            content.setBody(&NSString::from_str(&notice.body));
            content.setThreadIdentifier(&NSString::from_str(&notice.thread));
            content.setSound(Some(&UNNotificationSound::defaultSound()));
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                &NSString::from_str(&notice.id),
                &content,
                None,
            );
            center.addNotificationRequest_withCompletionHandler(&request, None);
        }
    }

    pub(super) fn withdraw(ids: &[String]) {
        if !bundled() {
            return;
        }
        let ids: Vec<_> = ids.iter().map(|id| NSString::from_str(id)).collect();
        UNUserNotificationCenter::currentNotificationCenter()
            .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Notice;

    pub(super) fn in_front() -> bool {
        true
    }
    pub(super) fn authorize() {}
    pub(super) fn post(_: &[Notice]) {}
    pub(super) fn withdraw(_: &[String]) {}
}
