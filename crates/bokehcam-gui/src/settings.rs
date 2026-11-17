//! The settings panel: one control per setting.
//!
//! The controls are generated from the settings schema in `bokehcam-core`,
//! so a setting added there appears here without touching this crate.

use std::sync::mpsc::Sender;

use bokehcam_core::Config;
use bokehcam_core::schema::{self, Kind, Setting};
use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_kit::component::{ActiveTheme, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use toml::Value;

use crate::link::ToLink;

/// Width of the list of setting groups until its divider is dragged; narrow
/// enough to leave the controls room in a side panel.
const SIDEBAR_WIDTH: Pixels = px(170.);
/// Width of a slider that sits beside its label.
const SLIDER_WIDTH: Pixels = px(220.);

/// The configuration as the window sees it, shared by every control.
pub struct Shared {
    pub config: Config,
    pub to_link: Sender<ToLink>,
    /// Why the last change was refused, if it was.
    pub problem: Option<SharedString>,
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
                let _ = shared.to_link.send(ToLink::SetConfig(Box::new(config)));
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

/// Every setting as a control, grouped as in the schema.
pub struct SettingsPanel {
    pages: Vec<SettingPage>,
    sliders: Vec<(&'static str, Entity<SliderState>)>,
    /// The config changed outside the window; move the sliders to match.
    sliders_stale: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
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

        Self { pages, sliders, sliders_stale: false, _subscriptions: subscriptions }
    }

    /// The configuration was changed by something other than this panel.
    pub fn config_changed(&mut self, cx: &mut Context<Self>) {
        self.sliders_stale = true;
        cx.notify();
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.sliders_stale) {
            for (key, state) in &self.sliders {
                let value = as_f64(&Shared::value(cx, key)) as f32;
                state.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
        Settings::new("settings").sidebar_width(SIDEBAR_WIDTH).pages(self.pages.clone())
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
        SettingField::render(move |options, _, cx| {
            let value = as_f64(&Shared::value(cx, key));
            let text = if value.abs() < 0.01 && value != 0.0 { format!("{value:.5}") } else { format!("{value:.2}") };
            // Below its label the slider takes the whole row; beside it, a fixed width.
            let below_label = options.layout() == Axis::Vertical;
            let slider = Slider::new(&state);
            h_flex()
                .gap_3()
                .items_center()
                .when(below_label, |row| row.w_full())
                .child(if below_label { slider.flex_1() } else { slider.w(SLIDER_WIDTH) })
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
