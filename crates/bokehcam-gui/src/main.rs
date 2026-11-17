//! The bokehcam window: a live preview and every setting.

mod link;
mod settings;

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use bokehcam_core::paths;
use bokehcam_engine::{Preview, State, Status};
use gpui_kit::component::{ActiveTheme, Theme, v_flex};
use gpui_kit::*;
use link::{Link, PREVIEW_SIZE, ToUi};
use settings::{SettingsPanel, Shared};

/// Height of the preview picture in the window, in logical pixels.
const PREVIEW_HEIGHT: f32 = 360.0;

struct MainView {
    from_link: Receiver<ToUi>,
    _link: Link,
    status: Status,
    preview: Option<Arc<RenderImage>>,
    settings: Entity<SettingsPanel>,
}

impl MainView {
    fn new(link: Link, from_link: Receiver<ToUi>, cx: &mut Context<Self>) -> Self {
        // Pull messages from the link thread a few times per frame interval.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                if this.update(cx, |this, cx| this.pump(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        Self { from_link, _link: link, status: Status::default(), preview: None, settings: cx.new(SettingsPanel::new) }
    }

    fn pump(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut newest: Option<Preview> = None;
        while let Ok(message) = self.from_link.try_recv() {
            changed = true;
            match message {
                ToUi::Preview(preview) => newest = Some(preview),
                ToUi::Status(status) => self.status = status,
                ToUi::Config(config) => {
                    cx.global_mut::<Shared>().config = config;
                    self.settings.update(cx, SettingsPanel::config_changed);
                }
                ToUi::Problem(problem) => cx.global_mut::<Shared>().problem = problem.map(Into::into),
            }
        }
        if let Some(preview) = newest {
            let mut pixels = preview.rgba;
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2); // the UI wants BGRA
            }
            if let Some(buffer) = image::RgbaImage::from_raw(preview.width, preview.height, pixels) {
                let new = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                if let Some(old) = self.preview.replace(new) {
                    cx.drop_image(old, None);
                }
            }
        }
        if changed {
            cx.notify();
        }
    }
}

impl Render for MainView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (state_text, state_color) = match &self.status.state {
            State::Starting => ("Starting…".to_string(), theme.muted_foreground),
            State::Idle => ("Ready. The camera is off until an app uses it.".to_string(), theme.muted_foreground),
            State::Streaming => (format!("Live, {:.0} fps", self.status.fps), theme.success),
            State::Problem(problem) => (problem.clone(), theme.danger),
        };
        let details = [self.status.camera.as_str(), self.status.model.as_str(), self.status.graphics.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ");
        let problem = cx.global::<Shared>().problem.clone();

        let picture = match &self.preview {
            Some(image) => img(ImageSource::Render(image.clone()))
                .h(px(PREVIEW_HEIGHT))
                .w(px(PREVIEW_HEIGHT * PREVIEW_SIZE.0 as f32 / PREVIEW_SIZE.1 as f32))
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child("Waiting for the camera…")
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .w_full()
                    .h(px(PREVIEW_HEIGHT))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .overflow_hidden()
                    .bg(gpui_kit::black())
                    .child(picture),
            )
            .child(
                v_flex()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_sm().text_color(state_color).child(state_text))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(details))
                    .children(problem.map(|text| div().text_sm().text_color(theme.danger).child(text))),
            )
            .child(div().flex_1().min_h_0().child(self.settings.clone()))
    }
}

fn main() {
    tracing_subscriber::fmt().with_target(false).compact().init();
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);

        let (link, from_link, config) = match Link::start(paths::config_file()) {
            Ok(started) => started,
            Err(e) => {
                eprintln!("error: {e:#}");
                cx.quit();
                return;
            }
        };
        cx.set_global(Shared { config, to_link: link.to_link.clone(), problem: None });

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(980.), px(900.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("bokehcam".into()), ..Default::default() }),
            app_id: Some("bokehcam".into()),
            ..Default::default()
        };
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            // On Linux the system appearance arrives after the window opens,
            // so reading it once here is not enough.
            Theme::sync_system_appearance(Some(window), cx);
            window.observe_window_appearance(|window, cx| Theme::sync_system_appearance(Some(window), cx)).detach();
            cx.new(|cx| MainView::new(link, from_link, cx))
        });
        if let Err(e) = opened {
            eprintln!("error: cannot open the window: {e:#}");
            cx.quit();
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
