//! The bokehcam window: a live preview and every setting.

mod layout;
mod link;
mod settings;

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use bokehcam_core::paths;
use bokehcam_engine::{Preview, State, Status};
use std::ops::Range;

use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::{ActiveTheme, Theme, h_flex, v_flex};
use gpui_kit::*;
use layout::Arrangement;
use link::{Link, ToUi};
use settings::{SettingsPanel, Shared};

/// How narrow and how wide the divider may make the settings panel.
const SETTINGS_WIDTH_RANGE: Range<Pixels> = px(340.)..px(900.);
/// The settings below the preview cannot be made shorter than this.
const STACKED_SETTINGS_MIN: Pixels = px(160.);

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

impl MainView {
    /// The state in a few words, and the colour of its indicator.
    fn state_label(&self, cx: &App) -> (SharedString, Hsla) {
        let theme = cx.theme();
        match &self.status.state {
            State::Starting => ("Starting…".into(), theme.muted_foreground),
            State::Idle => ("Camera off".into(), theme.muted_foreground),
            State::Streaming => (format!("Live · {:.0} fps", self.status.fps).into(), theme.success),
            State::Problem(_) => ("Not working".into(), theme.danger),
        }
    }

    /// What to say in place of the picture while there is none.
    fn waiting_text(&self) -> SharedString {
        match &self.status.state {
            State::Idle => "The camera is off until an app uses it.".into(),
            State::Problem(problem) => problem.clone().into(),
            State::Starting | State::Streaming => "Waiting for the camera…".into(),
        }
    }

    /// The picture, as large as its pane allows, with the state over it.
    fn render_picture(&self, cx: &App) -> impl IntoElement + use<> {
        let output = &cx.global::<Shared>().config.output;
        let proportions = (output.width, output.height);
        let image = self.preview.clone();
        let (label, indicator) = self.state_label(cx);
        let waiting = self.waiting_text();

        container_query(move |available, _, cx| {
            let theme = cx.theme();
            let (width, height) = layout::fit(proportions, (available.width.as_f32(), available.height.as_f32()));
            let picture = match image {
                Some(image) => img(ImageSource::Render(image)).size_full().rounded(theme.radius_lg).into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(waiting)
                    .into_any_element(),
            };
            let state = h_flex()
                .absolute()
                .top_3()
                .left_3()
                .gap_2()
                .px_3()
                .py_1()
                .rounded(theme.radius_full())
                .bg(black().opacity(0.6))
                .text_xs()
                .text_color(white())
                .child(div().size_2().rounded(theme.radius_full()).bg(indicator))
                .child(label);
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().relative().w(px(width)).h(px(height)).child(picture).child(state))
        })
    }

    /// The preview pane: the picture above a line of details.
    fn render_preview(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let details = [self.status.camera.as_str(), self.status.model.as_str(), self.status.graphics.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ");
        // A stale picture can still be on screen, so a problem is also said here.
        let engine_problem = match (&self.status.state, &self.preview) {
            (State::Problem(problem), Some(_)) => Some(SharedString::from(problem.clone())),
            _ => None,
        };
        let refused = cx.global::<Shared>().problem.clone();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(div().flex_1().min_h_0().p_4().child(self.render_picture(cx)))
            .child(
                v_flex()
                    .flex_none()
                    .px_4()
                    .pb_3()
                    .gap_1()
                    .text_xs()
                    .children(
                        engine_problem
                            .into_iter()
                            .chain(refused)
                            .map(|text| div().text_color(theme.danger).child(text)),
                    )
                    .child(div().truncate().text_color(theme.muted_foreground).child(details)),
            )
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let output = &cx.global::<Shared>().config.output;
        let arrangement =
            Arrangement::for_window((viewport.width.as_f32(), viewport.height.as_f32()), (output.width, output.height));
        let preview = resizable_panel().child(self.render_preview(cx));
        // Only the preview grows and shrinks with the window.
        let settings = resizable_panel().flex_none().child(self.settings.clone());

        // Each arrangement keeps its own divider position.
        let panes = match arrangement {
            Arrangement::SideBySide => h_resizable("side-by-side")
                .child(preview)
                .child(settings.size(px(layout::SETTINGS_WIDTH)).size_range(SETTINGS_WIDTH_RANGE)),
            Arrangement::Stacked => v_resizable("stacked").child(preview).child(
                settings
                    .size(viewport.height * layout::STACKED_SETTINGS_SHARE)
                    .size_range(STACKED_SETTINGS_MIN..Pixels::MAX),
            ),
        };
        div().size_full().bg(cx.theme().background).text_color(cx.theme().foreground).child(panes)
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
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1360.), px(800.)), cx))),
            window_min_size: Some(size(px(480.), px(420.))),
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
