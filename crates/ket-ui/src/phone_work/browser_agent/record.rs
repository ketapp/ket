//! `ket browser record`: a video of a worktree's browser tab, for an agent
//! to show a person what happened. `ket_chromium::Recorder` makes it here, in
//! the window with the page, and it is kept here from `start` to `stop`,
//! which are two runs of the CLI.
//!
//! - **One recording a worktree at a time**, of the tab `ket browser` acts
//!   on when it starts; `stop` finds it whichever tab is in front by then.
//! - **It stops itself at `--max`.** A look every [`LOOK`] stops it then and
//!   keeps what it made, or why it made nothing, for the next `stop`.
//! - **It goes with its tab.** The same look finds the tab closed or its page
//!   gone, and drops the recorder, which abandons the file; the next `stop`
//!   says the recording was lost with the tab.
//! - **`stop` answers in time.** Finishing the MP4 takes a moment, and a page
//!   that has stopped answering takes longer: past [`STOP_WAIT`] the
//!   terminal is told it is still being finished, and the next `stop` has it.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Context, FutureExt as _, WeakEntity};
use ket_chromium::{RecordOptions, Recorder, Recording};
use ket_core::host::{BrowserAnswer, finish_browser};
use ket_core::id::WorktreeId;

use super::{ANSWER_WAIT, Answer, page_gone, page_of};
use crate::Shell;
use crate::browser::BrowserId;

/// How often a recording is looked at: whether its tab is still there, and
/// whether it has run as long as it may.
const LOOK: Duration = Duration::from_millis(200);

/// How long `stop` waits for the video to be finished before saying it still
/// is: short of the host's twenty seconds, so the terminal hears it from
/// here.
const STOP_WAIT: Duration = Duration::from_secs(15);

/// How often `stop` looks while it waits.
const STOP_LOOK: Duration = Duration::from_millis(50);

/// How many worktrees' recordings are kept; past it, the one finished or lost
/// longest ago goes, uncollected. Its files stay until newer recordings push
/// them out.
const KEPT: usize = 32;

const NOT_RECORDING: &str = "not recording";

const LOST: &str = "the recording was lost with the tab: it closed while it was being recorded";

const STILL_FINISHING: &str =
    "the video is still being finished; `ket browser record stop` again gets it in a moment";

/// Every worktree's recording.
static RECORDINGS: Mutex<Store<Recorder>> = Mutex::new(Store::new());

fn recordings() -> MutexGuard<'static, Store<Recorder>> {
    RECORDINGS.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shell {
    /// `ket browser record start`: records browser `tab`, in `worktree`, for
    /// at most `max_seconds`, and answers request `req` once the page is
    /// being recorded.
    pub(super) fn start_recording(
        &mut self,
        req: u64,
        worktree: WorktreeId,
        tab: BrowserId,
        max_seconds: u32,
        cx: &mut Context<Self>,
    ) {
        let begun = recordings().begin(&worktree, Instant::now());
        let serial = match begun {
            Ok(serial) => serial,
            Err(why) => return finish_browser(req, Err(why)),
        };
        cx.spawn(async move |this: WeakEntity<Shell>, cx| {
            let executor = cx.background_executor().clone();
            let begun = record_page(&this, tab, max_seconds, cx)
                .with_timeout(ANSWER_WAIT, &executor)
                .await
                .unwrap_or_else(|_| Err("the page didn't start recording in time".to_owned()));
            let recorder = match begun {
                Ok(recorder) => recorder,
                Err(why) => {
                    recordings().forget(&worktree, serial);
                    return finish_browser(req, Err(why));
                }
            };
            let limit = Duration::from_secs(u64::from(max_seconds.max(1)));
            let started = recordings().started(&worktree, serial, recorder, limit, Instant::now());
            if !started {
                return finish_browser(req, Err(LOST.to_owned()));
            }
            finish_browser(req, Ok(BrowserAnswer::RecordingStarted));
            watch(&this, &worktree, serial, tab, cx).await;
        })
        .detach();
    }

    /// `ket browser record stop`: stops `worktree`'s recording and answers
    /// request `req` with what it made — or with what it made already, if it
    /// ran as long as it could.
    pub(super) fn stop_recording(
        &mut self,
        req: u64,
        worktree: WorktreeId,
        cx: &mut Context<Self>,
    ) {
        let stopping = recordings().stop(&worktree);
        let serial = match stopping {
            Stopping::Answer(answer) => return finish_browser(req, answer),
            Stopping::Wait(serial) => serial,
            Stopping::Finish(serial, recorder) => {
                // Finished on its own, so a terminal that has stopped waiting
                // does not lose it: the next `stop` collects it.
                let worktree = worktree.clone();
                cx.spawn(async move |_, _| {
                    let made = recorder.stop().await.map_err(|error| error.to_string());
                    recordings().finished(&worktree, serial, made);
                })
                .detach();
                serial
            }
        };
        cx.spawn(async move |_, cx| {
            let deadline = Instant::now() + STOP_WAIT;
            let answer = loop {
                let collected = recordings().collect(&worktree, serial);
                if let Some(answer) = collected {
                    break answer;
                }
                if Instant::now() >= deadline {
                    break Err(STILL_FINISHING.to_owned());
                }
                cx.background_executor().timer(STOP_LOOK).await;
            };
            finish_browser(req, answer);
        })
        .detach();
    }
}

/// Starts recording browser `tab`'s page into a new file in ket's cache.
async fn record_page(
    this: &WeakEntity<Shell>,
    tab: BrowserId,
    max_seconds: u32,
    cx: &mut AsyncApp,
) -> Result<Recorder, String> {
    let executor = cx.background_executor().clone();
    let cdp = page_of(this, tab, &executor, cx)
        .await
        .ok_or_else(page_gone)?;
    let out = executor
        .spawn(async { ket_core::capture::new_recording() })
        .await
        .map_err(|error| error.to_string())?;
    let options = RecordOptions {
        max_seconds,
        ..RecordOptions::default()
    };
    Recorder::start(cdp, out, options)
        .await
        .map_err(|error| error.to_string())
}

/// Looks at recording `serial` of browser `tab` until it is over: stops it
/// once it has run as long as it may, and lets it go if the tab or its page
/// does.
async fn watch(
    this: &WeakEntity<Shell>,
    worktree: &WorktreeId,
    serial: u64,
    tab: BrowserId,
    cx: &mut AsyncApp,
) {
    let executor = cx.background_executor().clone();
    loop {
        // A window that has closed has no tabs.
        let alive = this
            .read_with(cx, |shell, _| shell.chromium_browser(tab).is_some())
            .unwrap_or(false);
        let look = recordings().look(worktree, serial, alive, Instant::now());
        match look {
            Look::Again(left) => executor.timer(left.min(LOOK)).await,
            Look::Stop(recorder) => {
                let made = recorder.stop().await.map_err(|error| error.to_string());
                recordings().finished(worktree, serial, made);
                return;
            }
            Look::Over => return,
        }
    }
}

/// `elapsed` as a clock reads a stopwatch: `0:42`, `12:05`.
fn stopwatch(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What a finished recording is to the terminal: where its files are.
fn answer(made: Result<Recording, String>) -> Answer {
    let made = made?;
    Ok(BrowserAnswer::Recording {
        video: made.video.display().to_string(),
        frames: made
            .frames
            .iter()
            .map(|frame| frame.display().to_string())
            .collect(),
        seconds: made.seconds,
    })
}

/// The recordings, by worktree. `R` is the recorder, which a test stands in
/// for.
struct Store<R> {
    /// Counts recordings, so a look at one since replaced leaves the new one
    /// be.
    serial: u64,
    slots: BTreeMap<WorktreeId, Slot<R>>,
}

/// One worktree's recording.
struct Slot<R> {
    /// Which recording it is.
    serial: u64,
    /// When it was asked for, and then when it started.
    since: Instant,
    state: State<R>,
}

enum State<R> {
    /// Asked for; the page has not started being recorded yet.
    Starting,
    /// Being recorded, for at most `limit`.
    Running { recorder: R, limit: Duration },
    /// Stopped, its file being finished.
    Finishing,
    /// What it made, or why it made nothing, for the next `stop`.
    Done(Result<Recording, String>),
    /// Its tab closed, or its page went, while it was being recorded.
    Lost,
}

/// What a look at a recording finds to do.
#[derive(Debug, PartialEq)]
enum Look<R> {
    /// Nothing yet: it has this long left.
    Again(Duration),
    /// It has run as long as it may: finish it.
    Stop(R),
    /// Nothing more: it was stopped, lost or replaced.
    Over,
}

/// What `stop` finds to do.
#[derive(Debug, PartialEq)]
enum Stopping<R> {
    /// Answer with this, now.
    Answer(Answer),
    /// Finish this recorder's video, recording `serial`, and wait for it.
    Finish(u64, R),
    /// Wait for recording `serial`, already being finished.
    Wait(u64),
}

impl<R> Store<R> {
    const fn new() -> Self {
        Self {
            serial: 0,
            slots: BTreeMap::new(),
        }
    }

    /// Recording `serial` in `worktree`, while it is that one.
    fn slot(&mut self, worktree: &WorktreeId, serial: u64) -> Option<&mut Slot<R>> {
        self.slots
            .get_mut(worktree)
            .filter(|slot| slot.serial == serial)
    }

    /// Makes way for a recording in `worktree`, and says which it is — or
    /// why there cannot be one now. One not collected before goes.
    fn begin(&mut self, worktree: &WorktreeId, now: Instant) -> Result<u64, String> {
        match self
            .slots
            .get(worktree)
            .map(|slot| (&slot.state, slot.since))
        {
            Some((State::Starting | State::Running { .. }, since)) => {
                return Err(format!(
                    "already recording since {}",
                    stopwatch(now.saturating_duration_since(since))
                ));
            }
            Some((State::Finishing, _)) => {
                return Err("the last recording is still being finished; \
                            `ket browser record stop` gets it"
                    .to_owned());
            }
            Some((State::Done(_) | State::Lost, _)) | None => {}
        }
        if !self.slots.contains_key(worktree) && self.slots.len() >= KEPT {
            let oldest = self
                .slots
                .iter()
                .filter(|(_, slot)| matches!(slot.state, State::Done(_) | State::Lost))
                .min_by_key(|(_, slot)| slot.since)
                .map(|(worktree, _)| worktree.clone());
            if let Some(oldest) = oldest {
                self.slots.remove(&oldest);
            }
        }
        self.serial += 1;
        let slot = Slot {
            serial: self.serial,
            since: now,
            state: State::Starting,
        };
        self.slots.insert(worktree.clone(), slot);
        Ok(self.serial)
    }

    /// Recording `serial` has started, at `now`, for at most `limit`. False,
    /// the recorder dropped, if it was let go of meanwhile.
    fn started(
        &mut self,
        worktree: &WorktreeId,
        serial: u64,
        recorder: R,
        limit: Duration,
        now: Instant,
    ) -> bool {
        let Some(slot) = self.slot(worktree, serial) else {
            return false;
        };
        if !matches!(slot.state, State::Starting) {
            return false;
        }
        slot.since = now;
        slot.state = State::Running { recorder, limit };
        true
    }

    /// Recording `serial` could not start: it goes.
    fn forget(&mut self, worktree: &WorktreeId, serial: u64) {
        if self.slot(worktree, serial).is_some() {
            self.slots.remove(worktree);
        }
    }

    /// A look at recording `serial` at `now`, its tab `alive` or not.
    fn look(&mut self, worktree: &WorktreeId, serial: u64, alive: bool, now: Instant) -> Look<R> {
        let Some(slot) = self.slot(worktree, serial) else {
            return Look::Over;
        };
        let State::Running { limit, .. } = slot.state else {
            return Look::Over;
        };
        if !alive {
            // Dropped, the recorder abandons the file.
            slot.state = State::Lost;
            return Look::Over;
        }
        let ran = now.saturating_duration_since(slot.since);
        if ran < limit {
            return Look::Again(limit - ran);
        }
        match std::mem::replace(&mut slot.state, State::Finishing) {
            State::Running { recorder, .. } => Look::Stop(recorder),
            other => {
                slot.state = other;
                Look::Over
            }
        }
    }

    /// What `stop` in `worktree` does: takes the recorder to finish, waits
    /// for one being finished, or answers now.
    fn stop(&mut self, worktree: &WorktreeId) -> Stopping<R> {
        let Some(slot) = self.slots.get_mut(worktree) else {
            return Stopping::Answer(Err(NOT_RECORDING.to_owned()));
        };
        let serial = slot.serial;
        match std::mem::replace(&mut slot.state, State::Finishing) {
            State::Running { recorder, .. } => Stopping::Finish(serial, recorder),
            State::Finishing => Stopping::Wait(serial),
            State::Starting => {
                slot.state = State::Starting;
                Stopping::Answer(Err(
                    "the recording is still starting; try again in a moment".to_owned(),
                ))
            }
            State::Done(made) => {
                self.slots.remove(worktree);
                Stopping::Answer(answer(made))
            }
            State::Lost => {
                self.slots.remove(worktree);
                Stopping::Answer(Err(LOST.to_owned()))
            }
        }
    }

    /// Recording `serial`'s video is finished, or could not be.
    fn finished(&mut self, worktree: &WorktreeId, serial: u64, made: Result<Recording, String>) {
        if let Some(slot) = self.slot(worktree, serial)
            && matches!(slot.state, State::Finishing)
        {
            slot.state = State::Done(made);
        }
    }

    /// What `stop`, waiting on recording `serial`, answers now: what it
    /// made, which it takes, once it is finished; `None` while it is still
    /// being finished.
    fn collect(&mut self, worktree: &WorktreeId, serial: u64) -> Option<Answer> {
        let Some(slot) = self.slot(worktree, serial) else {
            // Another `stop` collected it.
            return Some(Err(NOT_RECORDING.to_owned()));
        };
        if !matches!(slot.state, State::Done(_) | State::Lost) {
            return None;
        }
        match self.slots.remove(worktree).map(|slot| slot.state) {
            Some(State::Done(made)) => Some(answer(made)),
            _ => Some(Err(LOST.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> WorktreeId {
        WorktreeId::new("wt-1")
    }

    fn made() -> Recording {
        Recording {
            video: "/c/recording-1-0.mp4".into(),
            frames: vec!["/c/recording-1-0-1.png".into()],
            seconds: 2.0,
        }
    }

    #[test]
    fn a_recording_stops_at_its_limit_and_the_next_stop_has_it() {
        let mut store = Store::<&str>::new();
        let t0 = Instant::now();
        let serial = store.begin(&worktree(), t0).unwrap();
        // One at a time, starting or not.
        assert!(store.begin(&worktree(), t0).is_err());
        assert!(matches!(store.stop(&worktree()), Stopping::Answer(Err(_))));
        let limit = Duration::from_secs(120);
        assert!(store.started(&worktree(), serial, "recorder", limit, t0));
        assert_eq!(
            store.begin(&worktree(), t0 + Duration::from_secs(42)),
            Err("already recording since 0:42".to_owned())
        );
        let at = |secs| t0 + Duration::from_secs(secs);
        assert_eq!(
            store.look(&worktree(), serial, true, at(100)),
            Look::Again(Duration::from_secs(20))
        );
        assert_eq!(
            store.look(&worktree(), serial, true, at(120)),
            Look::Stop("recorder")
        );
        assert_eq!(store.look(&worktree(), serial, true, at(121)), Look::Over);
        // A stop while the file is finished waits for it.
        assert_eq!(store.stop(&worktree()), Stopping::Wait(serial));
        assert_eq!(store.collect(&worktree(), serial), None);
        store.finished(&worktree(), serial, Ok(made()));
        assert_eq!(
            store.collect(&worktree(), serial),
            Some(Ok(BrowserAnswer::Recording {
                video: "/c/recording-1-0.mp4".to_owned(),
                frames: vec!["/c/recording-1-0-1.png".to_owned()],
                seconds: 2.0,
            }))
        );
        assert_eq!(
            store.stop(&worktree()),
            Stopping::Answer(Err(NOT_RECORDING.to_owned()))
        );
    }

    #[test]
    fn a_stop_takes_the_recorder_and_a_failure_is_its_answer() {
        let mut store = Store::<&str>::new();
        let t0 = Instant::now();
        let serial = store.begin(&worktree(), t0).unwrap();
        store.started(&worktree(), serial, "recorder", Duration::from_secs(60), t0);
        assert_eq!(
            store.stop(&worktree()),
            Stopping::Finish(serial, "recorder")
        );
        // The watch sees it taken.
        assert_eq!(store.look(&worktree(), serial, true, t0), Look::Over);
        let why = "the page didn't paint while it was being recorded".to_owned();
        store.finished(&worktree(), serial, Err(why.clone()));
        assert_eq!(store.stop(&worktree()), Stopping::Answer(Err(why)));
    }

    #[test]
    fn a_recording_whose_tab_goes_is_lost_with_it() {
        let mut store = Store::<&str>::new();
        let t0 = Instant::now();
        let serial = store.begin(&worktree(), t0).unwrap();
        store.started(&worktree(), serial, "recorder", Duration::from_secs(60), t0);
        assert_eq!(store.look(&worktree(), serial, false, t0), Look::Over);
        assert_eq!(
            store.stop(&worktree()),
            Stopping::Answer(Err(LOST.to_owned()))
        );
        // Lost, or collected: the worktree can record again.
        let again = store.begin(&worktree(), t0).unwrap();
        assert_ne!(again, serial);
        // One that could not start goes, and was never recording.
        store.forget(&worktree(), again);
        assert!(!store.started(&worktree(), again, "late", Duration::from_secs(1), t0));
        assert_eq!(
            store.stop(&worktree()),
            Stopping::Answer(Err(NOT_RECORDING.to_owned()))
        );
    }

    #[test]
    fn a_stopwatch_reads_minutes_and_seconds() {
        assert_eq!(stopwatch(Duration::from_millis(42_900)), "0:42");
        assert_eq!(stopwatch(Duration::from_secs(605)), "10:05");
    }
}
