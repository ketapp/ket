//! `ket browser scroll`: the mouse wheel turned over the page, or over an
//! element in it — so what scrolls is what a person's wheel would scroll
//! there, a list inside the page included, and a page that loads more as it
//! is scrolled loads it.

use ket_chromium::protocol::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use ket_chromium::protocol::browser_protocol::page::GetLayoutMetricsParams;
use ket_core::host::wire::{BrowserScroll, BrowserTarget};
use serde_json::Value;

use super::act::{Aimed, Point};
use super::{Answer, Page};

/// How far down the page goes, in CSS pixels, as the page measures it.
const PAGE_HEIGHT: &str = "(document.scrollingElement || document.body || {}).scrollHeight || 0";

/// How far down an element's content goes.
const ELEMENT_HEIGHT: &str = "function () { return this.scrollHeight || 0; }";

impl Page<'_> {
    /// Turns the wheel over `target`'s centre, or the page's, as far as
    /// `by` says, and says where the page is.
    pub(super) async fn scroll(&self, target: Option<&BrowserTarget>, by: BrowserScroll) -> Answer {
        let aimed = match target {
            Some(target) => Some(self.aim(target).await?),
            None => None,
        };
        let at = match &aimed {
            Some(aimed) => self.centre(aimed).await?,
            None => {
                let view = self
                    .ask(GetLayoutMetricsParams::default())
                    .await?
                    .css_layout_viewport;
                let (width, height) = (view.client_width as f64, view.client_height as f64);
                Point {
                    x: width / 2.0,
                    y: height / 2.0,
                    height,
                }
            }
        };
        let down = match by {
            BrowserScroll::Down(px) => px.map_or(at.height, f64::from),
            BrowserScroll::Up(px) => -px.map_or(at.height, f64::from),
            BrowserScroll::Bottom => self.scroll_height(aimed.as_ref(), at).await?,
            BrowserScroll::Top => -self.scroll_height(aimed.as_ref(), at).await?,
        };
        self.mouse(
            DispatchMouseEventType::MouseMoved,
            at,
            MouseButton::None,
            0,
            0,
        )
        .await?;
        let mut wheel =
            DispatchMouseEventParams::new(DispatchMouseEventType::MouseWheel, at.x, at.y);
        wheel.delta_x = Some(0.0);
        wheel.delta_y = Some(down);
        self.ask(wheel).await?;
        self.here(None).await
    }

    /// How far `aimed`'s content goes, or the page's: a turn of the wheel
    /// that far reaches the end, wherever it is scrolled to. Never less
    /// than what shows at `at`.
    async fn scroll_height(&self, aimed: Option<&Aimed>, at: Point) -> Result<f64, String> {
        let height = match aimed {
            Some(aimed) => {
                self.call_on(&aimed.object, ELEMENT_HEIGHT, Vec::new())
                    .await?
            }
            None => self.value(PAGE_HEIGHT).await.unwrap_or(Value::Null),
        };
        Ok(height.as_f64().unwrap_or(0.0).max(at.height))
    }
}
