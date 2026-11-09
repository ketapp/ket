//! Help → Export Diagnostics: a save panel, then the bundle written off the
//! UI thread and shown in Finder.
//!
//! What goes in the bundle, and what is taken out of it, is
//! `ket_core::diagnostics`'s; this file only asks where it goes and says how
//! it went.

use gpui::Context;
use ket_core::diagnostics;

use crate::Shell;
use crate::ui::toast::Tone;

impl Shell {
    /// Asks where to save a diagnostics bundle, then writes it there.
    pub(crate) fn export_diagnostics(&mut self, cx: &mut Context<Self>) {
        // The Desktop, where a file to drag into an email or an issue is
        // easiest to find; home when there is none.
        let directory = ket_core::paths::home()
            .map(|home| {
                let desktop = home.join("Desktop");
                if desktop.is_dir() { desktop } else { home }
            })
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&directory, Some(diagnostics::DEFAULT_NAME));
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.toast_detail(
                            Tone::Error,
                            "Could not export diagnostics",
                            error.to_string(),
                            cx,
                        );
                    });
                    return;
                }
                Err(_) => return,
            };
            let work = {
                let path = path.clone();
                cx.background_executor()
                    .spawn(async move { diagnostics::export(&path) })
            };
            let result = work.await;
            let _ = shell.update(cx, |shell, cx| match result {
                Ok(()) => {
                    let _ = ket_core::surface::reveal_in_file_manager(&path);
                    shell.toast_detail(
                        Tone::Success,
                        "Diagnostics exported",
                        format!(
                            "Attach {} to an issue or email it to {}",
                            path.file_name().map_or_else(
                                || path.display().to_string(),
                                |name| { name.to_string_lossy().into_owned() }
                            ),
                            diagnostics::SUPPORT_EMAIL
                        ),
                        cx,
                    );
                }
                Err(error) => shell.toast_detail(
                    Tone::Error,
                    "Could not export diagnostics",
                    error.to_string(),
                    cx,
                ),
            });
        })
        .detach();
    }
}
