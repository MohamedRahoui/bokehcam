//! The bokehcam window: a live preview and every setting.
//!
//! The controls are generated from the settings schema in `bokehcam-core`,
//! so a setting added there appears here without touching this crate.

mod link;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use bokehcam_core::schema::{self, Kind, Setting};
use bokehcam_core::{Config, paths};
use bokehcam_engine::{Preview, State, Status};
use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_kit::component::{ActiveTheme, Theme, h_flex, v_flex};
use gpui_kit::*;
use link::{Link, PREVIEW_SIZE, ToLink, ToUi};

/// Height of the preview picture in the window, in logical pixels.
const PREVIEW_HEIGHT: f32 = 360.0;
use toml::Value;

/// The configuration as the window sees it, shared by every control.
struct Shared {
    config: Config,
    to_link: Sender<ToLink>,
    /// Why the last change was refused, if it was.
    problem: Option<SharedString>,
}

impl Global for Shared {}

impl Shared {
    fn value(cx: &App, key: &str) -> Value {
        cx.global::<Shared>().config.get(key).unwrap_or(Value::Boolean(false))
    }

    /// Change one setting: validate, remember, and send to the engine.
    fn set(cx: &mut App, key: &str, value: Value) {
        let shared = cx.global_mut::<Shared>();
        match shared.config.with(key, value) {
            Ok(config) => {
                shared.config = config.clone();
                shared.problem = None;
                let _ = shared.to_link.send(ToLink::SetConfig(config));
            }
            Err(e) => shared.problem = Some(e.to_string().into()),
        }
        cx.refresh_windows();
    }
}

fn as_f64(value: &Value) -> f64 {
    match value {
        Value::Float(v) => *v,
        Value::Integer(v) => *v as f64,
        _ => 0.0,
    }
}

fn as_text(value: &Value) -> SharedString {
    value.as_str().unwrap_or_default().to_string().into()
}

struct MainView {
    from_link: Receiver<ToUi>,
    _link: Link,
    status: Status,
    preview: Option<Arc<RenderImage>>,
    pages: Vec<SettingPage>,
    sliders: Vec<(&'static str, Entity<SliderState>)>,
    /// The config changed outside the window; move the sliders to match.
    sliders_stale: bool,
    _subscriptions: Vec<Subscription>,
}

impl MainView {
    fn new(link: Link, from_link: Receiver<ToUi>, cx: &mut Context<Self>) -> Self {
        let mut sliders = Vec::new();
        let mut subscriptions = Vec::new();
        let defaults = Config::default();
        let cameras: Vec<(SharedString, SharedString)> = device_options(bokehcam_media::list_cameras());
        let virtual_cameras = device_options(bokehcam_media::list_virtual_cameras());
        let models: Vec<(SharedString, SharedString)> = std::iter::once(("auto".into(), "Automatic".into()))
            .chain(bokehcam_infer::manifest::all().into_iter().map(|m| {
                let note = if m.installed_path().is_some() { "" } else { " (not downloaded)" };
                (m.name.clone().into(), format!("{}{note}", m.title).into())
            }))
            .collect();

        let mut pages = Vec::new();
        for group in schema::GROUPS {
            let mut items = Vec::new();
            for setting in group.settings {
                let key = setting.key;
                let default = defaults.get(key).expect("schema key exists");
                let field_item = match setting.kind {
                    Kind::Bool => SettingItem::new(
                        setting.label,
                        SettingField::switch(
                            move |cx| Shared::value(cx, key).as_bool().unwrap_or(false),
                            move |on, cx| Shared::set(cx, key, Value::Boolean(on)),
                        )
                        .default_value(default.as_bool().unwrap_or(false)),
                    ),
                    Kind::Int { min, max, step } => SettingItem::new(
                        setting.label,
                        SettingField::number_input(
                            NumberFieldOptions { min: min as f64, max: max as f64, step: step as f64 },
                            move |cx| as_f64(&Shared::value(cx, key)),
                            move |v, cx| Shared::set(cx, key, Value::Integer(v.round() as i64)),
                        )
                        .default_value(as_f64(&default)),
                    ),
                    Kind::Float { min, max, step } => {
                        let state = cx.new(|cx| {
                            let current = as_f64(&Shared::value(cx, key)) as f32;
                            let mut state = SliderState::new()
                                .min(min as f32)
                                .max(max as f32)
                                .step(step as f32)
                                .default_value(current);
                            // A range spanning several orders of magnitude needs a log scale.
                            if min > 0.0 && max / min > 1000.0 {
                                state = state.scale(SliderScale::Logarithmic);
                            }
                            state
                        });
                        subscriptions.push(cx.subscribe(&state, move |_, _, event: &SliderEvent, cx| {
                            let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
                            let stepped = ((value.start() as f64 / step).round() * step).clamp(min, max);
                            Shared::set(cx, key, Value::Float((stepped * 1e6).round() / 1e6));
                        }));
                        sliders.push((key, state.clone()));
                        slider_item(setting, state, as_f64(&default))
                    }
                    Kind::Choice(options) => dropdown_item(
                        setting,
                        options.iter().map(|(v, l)| ((*v).into(), (*l).into())).collect(),
                        &default,
                    ),
                    Kind::Camera => dropdown_item(setting, cameras.clone(), &default),
                    Kind::VirtualCamera => dropdown_item(setting, virtual_cameras.clone(), &default),
                    Kind::Model => dropdown_item(setting, models.clone(), &default),
                    Kind::Text => SettingItem::new(
                        setting.label,
                        SettingField::input(
                            move |cx| as_text(&Shared::value(cx, key)),
                            move |text: SharedString, cx| Shared::set(cx, key, Value::String(text.to_string())),
                        ),
                    ),
                };
                items.push(field_item.description(setting.help));
            }
            pages.push(
                SettingPage::new(group.title)
                    .description(group.help)
                    .default_open(true)
                    .group(SettingGroup::new().items(items)),
            );
        }

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

        Self {
            from_link,
            _link: link,
            status: Status::default(),
            preview: None,
            pages,
            sliders,
            sliders_stale: false,
            _subscriptions: subscriptions,
        }
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
                    self.sliders_stale = true;
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

fn device_options(devices: Vec<bokehcam_media::DeviceInfo>) -> Vec<(SharedString, SharedString)> {
    std::iter::once(("auto".into(), "Automatic".into()))
        .chain(devices.into_iter().map(|d| (d.path.clone().into(), format!("{} ({})", d.name, d.path).into())))
        .collect()
}

fn dropdown_item(setting: &Setting, mut options: Vec<(SharedString, SharedString)>, default: &Value) -> SettingItem {
    let key = setting.key;
    SettingItem::new(
        setting.label,
        SettingField::dropdown(
            std::mem::take(&mut options),
            move |cx| as_text(&Shared::value(cx, key)),
            move |choice: SharedString, cx| Shared::set(cx, key, Value::String(choice.to_string())),
        )
        .default_value(as_text(default)),
    )
}

fn slider_item(setting: &Setting, state: Entity<SliderState>, default: f64) -> SettingItem {
    let key = setting.key;
    let reset_state = state.clone();
    SettingItem::new(
        setting.label,
        SettingField::render(move |_, _, cx| {
            let value = as_f64(&Shared::value(cx, key));
            let text = if value.abs() < 0.01 && value != 0.0 { format!("{value:.5}") } else { format!("{value:.2}") };
            h_flex()
                .gap_3()
                .items_center()
                .child(Slider::new(&state).w(px(220.)))
                .child(div().w(px(64.)).text_sm().text_color(cx.theme().muted_foreground).child(text))
        })
        .on_reset(
            move |cx| (as_f64(&Shared::value(cx, key)) - default).abs() > 1e-9,
            move |window, cx| {
                Shared::set(cx, key, Value::Float(default));
                reset_state.update(cx, |state, cx| state.set_value(default as f32, window, cx));
            },
        ),
    )
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.sliders_stale) {
            for (key, state) in &self.sliders {
                let value = as_f64(&Shared::value(cx, key)) as f32;
                state.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
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
            .child(div().flex_1().min_h_0().child(Settings::new("settings").pages(self.pages.clone())))
    }
}

fn main() {
    tracing_subscriber::fmt().with_target(false).compact().init();
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        Theme::sync_system_appearance(None, cx);

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
        let opened = gpui_kit::open_window(options, cx, move |_, cx| cx.new(|cx| MainView::new(link, from_link, cx)));
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
