//! Playback for audio files opened in the file viewer.
//!
//! The device is deliberately created only after the reader presses Play.
//! Opening an audio tab therefore has neither an audio thread nor a device
//! handle as a side effect, which keeps the ordinary text-and-terminal path
//! free of media work.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::time::Duration;

use gpui::{AnyElement, Context, ElementId, SharedString, Window, div, prelude::*, px, relative};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::tabs::{Tab, TabKind};
use crate::ui::button::icon_button;
use crate::ui::icon::Icon;

/// File suffixes routed to the audio viewer.
///
/// These match the deliberately small Rodio feature set in the workspace:
/// WAV, MP3, FLAC, Vorbis-in-Ogg, and AAC-in-MP4/M4A. The decoder still probes
/// the actual bytes before playback; a suffix never makes malformed data safe.
pub(crate) fn is_audio_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("wav" | "mp3" | "flac" | "ogg" | "m4a")
    )
}

/// One process-wide output stream and its single active track.
///
/// A file viewer should never surprise someone with two unrelated tracks
/// playing at once. Keeping one player also bounds decoder, I/O and mixer work
/// independently of how many tabs are open.
#[derive(Default)]
pub(crate) struct AudioEngine {
    sink: Option<MixerDeviceSink>,
    player: Option<Player>,
    active: Option<SharedString>,
    duration: Option<Duration>,
    errors: HashMap<SharedString, SharedString>,
}

impl AudioEngine {
    /// Stops output and releases the device. Used when the preference is off.
    pub(crate) fn disable(&mut self) {
        self.player = None;
        self.active = None;
        self.duration = None;
        self.sink = None;
    }

    /// Stops the selected track but retains a lazily opened device for a fast
    /// subsequent Play.
    pub(crate) fn stop_if_active(&mut self, key: &str) {
        if self.is_active(key) {
            self.player = None;
            self.active = None;
            self.duration = None;
        }
    }

    fn any_playing(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|player| !player.is_paused() && !player.empty())
    }

    fn is_active(&self, key: &str) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.as_ref() == key)
    }

    fn is_playing(&self, key: &str) -> bool {
        self.is_active(key)
            && self
                .player
                .as_ref()
                .is_some_and(|player| !player.is_paused() && !player.empty())
    }

    fn position(&self, key: &str) -> Duration {
        self.is_active(key)
            .then(|| self.player.as_ref().map(Player::get_pos))
            .flatten()
            .unwrap_or(Duration::ZERO)
    }

    fn duration(&self, key: &str) -> Option<Duration> {
        self.is_active(key).then_some(self.duration).flatten()
    }

    fn error(&self, key: &str) -> Option<&SharedString> {
        self.errors.get(key)
    }

    fn start(&mut self, key: SharedString, path: &Path) -> Result<(), String> {
        let sink = match self.sink.as_ref() {
            Some(sink) => sink,
            None => {
                let sink = DeviceSinkBuilder::open_default_sink()
                    .map_err(|error| format!("could not open the default audio output: {error}"))?;
                self.sink = Some(sink);
                self.sink.as_ref().expect("audio sink was just stored")
            }
        };
        let file = File::open(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let decoder = Decoder::try_from(file)
            .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
        let duration = decoder.total_duration();
        let player = Player::connect_new(sink.mixer());
        player.pause();
        player.append(decoder);
        player.play();

        self.active = Some(key.clone());
        self.player = Some(player);
        self.duration = duration;
        self.errors.remove(&key);
        Ok(())
    }

    fn toggle(&mut self, key: SharedString, path: &Path) {
        if self.is_active(key.as_ref())
            && let Some(player) = self.player.as_ref()
            && !player.empty()
        {
            if player.is_paused() {
                player.play();
            } else {
                player.pause();
            }
            return;
        }

        if let Err(error) = self.start(key.clone(), path) {
            self.errors.insert(key, error.into());
        }
    }

    fn seek(&mut self, key: &str, offset: Duration, forward: bool) {
        if !self.is_active(key) {
            return;
        }
        let Some(player) = self.player.as_ref() else {
            return;
        };
        let current = player.get_pos();
        let next = if forward {
            current.saturating_add(offset)
        } else {
            current.saturating_sub(offset)
        };
        if let Err(error) = player.try_seek(next) {
            self.errors.insert(
                key.to_owned().into(),
                format!("could not seek: {error}").into(),
            );
        }
    }
}

/// Formats a duration as the compact clock a player control needs.
fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The mark above the transport: a waveform rather than album art, since a
/// file being auditioned has no cover to show. Eleven bars, each a fixed
/// fraction of the tile's height, is a shape rather than a real analysis of
/// the decoded samples — the file has no fixed shape to draw at all until
/// decoding it fully up front, which playback is deliberately not doing.
const WAVEFORM_BARS: [f32; 11] = [
    0.22, 0.46, 0.78, 0.38, 0.96, 0.58, 0.84, 0.34, 0.64, 0.28, 0.50,
];

/// How far a bar has breathed from its resting height, in `[0.55, 1.0]`.
///
/// Only while playing: a paused file's waveform should sit still, the same
/// way its position does. `seconds` is the track's own position rather than
/// a wall clock, so the animation halts the instant playback does with no
/// extra state to keep in sync.
fn bar_breath(playing: bool, seconds: f32, index: usize) -> f32 {
    if !playing {
        return 1.0;
    }
    const PERIOD_SECS: f32 = 1.1;
    let phase = index as f32 * 0.57;
    let wave = (seconds / PERIOD_SECS * std::f32::consts::TAU + phase).sin();
    0.55 + 0.45 * (0.5 + 0.5 * wave)
}

/// The player's format caption: `"MP3 · 2:03"`, from the extension a reader
/// already sees in the tab and the duration once the decoder has reported it.
fn format_caption(path: &Path, duration: Option<Duration>) -> String {
    let format = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "AUDIO".to_owned());
    match duration {
        Some(duration) => format!("{format} · {}", clock(duration)),
        None => format,
    }
}

impl Shell {
    /// The absolute path in the active audio tab, if the focused pane has one.
    fn active_audio_path(&self) -> Option<SharedString> {
        match self.space()?.active_tab()?.kind.clone() {
            TabKind::Audio(path) => Some(path),
            _ => None,
        }
    }

    /// Opens or focuses the dedicated viewer for an audio file.
    pub(crate) fn open_audio_tab(&mut self, path: std::path::PathBuf) {
        let Some(worktree_id) = self.selected_id() else {
            return;
        };
        let key: SharedString = path.display().to_string().into();
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audio".to_owned());
        self.spaces.entry(worktree_id).or_default().open_tab(Tab {
            title: title.into(),
            kind: TabKind::Audio(key),
            renamed: false,
            pinned: false,
        });
        self.persist_layout();
    }

    /// Handles player controls while an audio tab is active.
    pub(crate) fn audio_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.active_audio_path() else {
            return false;
        };
        match event.keystroke.key.as_str() {
            "space" => self.toggle_audio(path, cx),
            "left" => self.seek_audio(path.as_ref(), false),
            "right" => self.seek_audio(path.as_ref(), true),
            _ => return false,
        }
        true
    }

    /// Starts, pauses, or restarts the track for `path`.
    pub(crate) fn toggle_audio(&mut self, path: SharedString, cx: &mut Context<Self>) {
        if self.audio_enabled {
            self.audio.toggle(path.clone(), Path::new(path.as_ref()));
            self.wake_audio_clock(cx);
        }
    }

    /// Moves the active track by a small, keyboard-friendly increment.
    pub(crate) fn seek_audio(&mut self, path: &str, forward: bool) {
        if self.audio_enabled {
            self.audio.seek(path, Duration::from_secs(5), forward);
        }
    }

    /// Starts a low-frequency repaint loop only while audio is advancing.
    fn wake_audio_clock(&mut self, cx: &mut Context<Self>) {
        if self.audio_tick.is_some() || !self.audio.any_playing() {
            return;
        }
        self.audio_tick = Some(cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep = this
                    .update(cx, |shell, cx| {
                        let keep = shell.audio.any_playing();
                        if keep {
                            cx.notify();
                        } else {
                            shell.audio_tick = None;
                        }
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    return;
                }
            }
        }));
    }

    /// Renders one audio tab. No decoder or output device is opened here.
    pub(crate) fn audio_pane(
        &self,
        path: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let title = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());

        if !self.audio_enabled {
            return div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(paint(t.text.dim))
                .child("Audio previews are disabled in Settings.")
                .into_any_element();
        }

        let playing = self.audio.is_playing(path);
        let position = self.audio.position(path);
        let duration = self.audio.duration(path);
        let progress = duration
            .filter(|duration| !duration.is_zero())
            .map(|duration| (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let key: SharedString = path.to_owned().into();
        let back_key = key.clone();
        let forward_key = key.clone();
        let forward_id = key.clone();

        // A thumb only once there is somewhere for it to sit: at 0:00 with no
        // duration read yet, a dot at the left edge would look like a stray
        // mark rather than a scrubber with nothing to show.
        let show_thumb = duration.is_some();
        let caption = format_caption(Path::new(path), duration);

        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .text_color(paint(t.text.primary))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .w_full()
                    .max_w(px(340.0))
                    .gap(px(24.0))
                    .px(px(24.0))
                    .child(
                        // The waveform stand-in for cover art: eleven bars
                        // that breathe while the track is playing and sit
                        // still, dim, while it is not.
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(3.0))
                            .size(px(132.0))
                            .px(px(20.0))
                            .rounded(crate::ui::RADIUS_LG)
                            .border_1()
                            .border_color(paint(t.border))
                            .children(WAVEFORM_BARS.iter().enumerate().map(|(index, base)| {
                                let breath = bar_breath(playing, position.as_secs_f32(), index);
                                div()
                                    .w(px(3.0))
                                    .h(px((base * breath * 100.0).max(6.0)))
                                    .rounded(px(2.0))
                                    .bg(if playing {
                                        paint(t.accent)
                                    } else {
                                        alpha(paint(t.text.primary), 0.18)
                                    })
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .text_size(px(17.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(paint(t.text.dim))
                                    .child(caption),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .gap(px(9.0))
                            .child(
                                div()
                                    .id((ElementId::from("audio-track"), key.clone()))
                                    .relative()
                                    .w_full()
                                    .h(px(5.0))
                                    .rounded_full()
                                    .cursor_pointer()
                                    // Not `sunken`: every surface token is the
                                    // same colour in this theme, so a track
                                    // painted in one of them is a track
                                    // painted in the ground it sits on.
                                    .bg(alpha(paint(t.text.primary), 0.14))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(progress))
                                            .rounded_full()
                                            .bg(paint(t.accent))
                                            .children(show_thumb.then(|| {
                                                div()
                                                    .absolute()
                                                    .top(px(-4.0))
                                                    .right(px(-6.0))
                                                    .size(px(13.0))
                                                    .rounded_full()
                                                    .bg(paint(t.accent))
                                                    .border_2()
                                                    .border_color(paint(t.surface))
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .justify_between()
                                    .text_size(px(12.0))
                                    .child(
                                        div()
                                            .text_color(paint(t.text.primary))
                                            .child(clock(position)),
                                    )
                                    .child(div().text_color(paint(t.text.dim)).child(
                                        duration.map(clock).unwrap_or_else(|| "--:--".to_owned()),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(22.0))
                            .child(step_button(
                                &t,
                                (ElementId::from("audio-back"), key.clone()),
                                Icon::RotateCcw,
                                cx.listener(move |this, _, _, cx| {
                                    this.seek_audio(back_key.as_ref(), false);
                                    cx.notify();
                                }),
                            ))
                            .child(play_button(
                                &t,
                                (ElementId::from("audio-play"), key.clone()),
                                playing,
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_audio(key.clone(), cx);
                                    cx.notify();
                                }),
                            ))
                            .child(step_button(
                                &t,
                                (ElementId::from("audio-forward"), forward_id),
                                Icon::RotateCw,
                                cx.listener(move |this, _, _, cx| {
                                    this.seek_audio(forward_key.as_ref(), true);
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(10.5))
                            .text_color(paint(t.text.dim))
                            .child(crate::ui::chip::kbd(
                                "Space",
                                self.chrome_family.clone(),
                                &t,
                            ))
                            .child("play/pause")
                            .child(div().w(px(6.0)))
                            .child(crate::ui::chip::kbd("←", self.chrome_family.clone(), &t))
                            .child(crate::ui::chip::kbd("→", self.chrome_family.clone(), &t))
                            .child("seek"),
                    )
                    .children(self.audio.error(path).map(|error| {
                        div()
                            .flex()
                            .justify_center()
                            .text_size(px(12.0))
                            .text_color(paint(t.status.failed))
                            .child(error.clone())
                    })),
            )
            .into_any_element()
    }
}

/// One of the two small transport nudges either side of Play: a circular
/// arrow with the increment it steps by, the way a podcast player marks its
/// skip-back and skip-forward controls.
fn step_button(
    t: &ket_core::theme::Theme,
    id: impl Into<ElementId>,
    which: Icon,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    icon_button(id, which)
        .bare()
        .circle()
        .sized(px(40.0), px(22.0))
        .overlay("5")
        .render(t)
        .on_click(on_click)
}

/// The one action the surface exists for: a filled circle that swaps its
/// glyph between [`Icon::Play`] and [`Icon::Pause`] rather than its label,
/// the way a real transport control does.
fn play_button(
    t: &ket_core::theme::Theme,
    id: impl Into<ElementId>,
    playing: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    icon_button(id, if playing { Icon::Pause } else { Icon::Play })
        .circle()
        .primary()
        .sized(px(60.0), px(22.0))
        .render(t)
        .on_click(on_click)
}
