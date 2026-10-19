//! What phones type into a terminal, in the order they asked — see
//! [`Typing`] — and a message with files handed to the agent in its turn.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::lock;
use crate::Result;
use crate::terminal::LocalTerminal;

/// What phones have asked to be typed into each terminal, in the order they
/// asked.
///
/// A message with files takes seconds to hand over: its images pasted, then
/// a wait while the agent shows them, then its words and Enter. Anything
/// typed meanwhile would land beside the images, and its Enter would send
/// them with the wrong words; a second message's images would mix with the
/// first's, and either wait could be met by the other's. So while a hand-over
/// is under way, what follows for that terminal waits behind it and goes in
/// turn, on that terminal's own worker. With nothing under way, keys go
/// straight in, as fast as before.
#[derive(Clone, Default)]
pub(super) struct Typing(Arc<Mutex<HashMap<u64, VecDeque<Typed>>>>);

/// One thing to type into a terminal, in its turn.
pub(super) enum Typed {
    Keys(Vec<u8>),
    /// A message with files — see [`hand_over`] — and who hears how it went.
    Files {
        text: String,
        files: Vec<PathBuf>,
        done: Box<dyn FnOnce(Result<()>) + Send>,
    },
}

impl Typing {
    /// Types `keys` into terminal `id` now, or after whatever is waiting.
    pub(super) fn keys(&self, id: u64, terminal: &LocalTerminal, keys: Vec<u8>) {
        let mut waiting = lock(&self.0);
        match waiting.get_mut(&id) {
            Some(queue) => queue.push_back(Typed::Keys(keys)),
            // Under the lock, so a hand-over queued next cannot start first.
            None => {
                let _ = terminal.send_input(keys);
            }
        }
    }

    /// Hands a message with files to terminal `id` after whatever is waiting,
    /// starting the terminal's worker if none is running.
    pub(super) fn files(
        &self,
        id: u64,
        terminal: Arc<LocalTerminal>,
        job: Typed,
    ) -> std::io::Result<()> {
        let mut waiting = lock(&self.0);
        if let Some(queue) = waiting.get_mut(&id) {
            queue.push_back(job);
            return Ok(());
        }
        waiting.insert(id, VecDeque::from([job]));
        drop(waiting);
        let typing = self.clone();
        let started = std::thread::Builder::new()
            .name("ket-host-phone-typing".into())
            .spawn(move || typing.work(id, &terminal));
        if started.is_err() {
            lock(&self.0).remove(&id);
        }
        started.map(drop)
    }

    /// Types what waits for terminal `id`, in order, until nothing does. The
    /// queue stays in the map while its worker runs — empty, while it types
    /// the last thing taken — which is what sends later keys to the back.
    fn work(&self, id: u64, terminal: &LocalTerminal) {
        loop {
            let next = {
                let mut waiting = lock(&self.0);
                match waiting.get_mut(&id).and_then(VecDeque::pop_front) {
                    Some(next) => next,
                    None => {
                        waiting.remove(&id);
                        return;
                    }
                }
            };
            match next {
                Typed::Keys(keys) => {
                    let _ = terminal.send_input(keys);
                }
                Typed::Files { text, files, done } => done(hand_over(terminal, &text, &files)),
            }
        }
    }
}

/// Hands a phone's message with files to the agent in `terminal`, as the
/// desktop's quick prompt does: the images first, each as a bracketed paste
/// of its path; then, once the agent shows them attached or
/// [`attachments::ATTACH_WAIT`] has gone by, the words, with any other file
/// named under them; and Enter a beat after.
///
/// [`attachments::ATTACH_WAIT`]: crate::attachments::ATTACH_WAIT
fn hand_over(terminal: &LocalTerminal, text: &str, files: &[PathBuf]) -> Result<()> {
    use crate::attachments::{self, ATTACH_POLL, ATTACH_SETTLE, ATTACH_WAIT, SUBMIT_DELAY};
    use alacritty_terminal::term::TermMode;

    let (bracketed, before) = terminal.with_term(|term| {
        (
            term.mode().contains(TermMode::BRACKETED_PASTE),
            attachments::images_shown(term),
        )
    });
    let (pastes, pasted, left) = attachments::image_pastes(files, bracketed);
    if pasted > 0 {
        terminal.send_input(pastes)?;
        let since = Instant::now();
        while terminal.with_term(|term| attachments::images_shown(term)) < before + pasted
            && since.elapsed() < ATTACH_WAIT
        {
            std::thread::sleep(ATTACH_POLL);
        }
        std::thread::sleep(ATTACH_SETTLE);
    }
    let prompt = attachments::with_files(text.trim().to_owned(), &left);
    if !prompt.is_empty() {
        terminal.send_input(attachments::paste(&prompt, bracketed))?;
        std::thread::sleep(SUBMIT_DELAY);
    }
    terminal.send_input(b"\r".to_vec())
}
