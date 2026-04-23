//! `DreamInspectorPanel` — the bottom-dock panel.
//!
//! Render pipeline:
//!  top    — category pill bar + pause/follow controls
//!  middle — virtualized feed rows (compact one-liner: time | type | summary)
//!  bottom — status line (connected · N events · idle | error banner)

use crate::categories::Category;
use crate::feed::{FeedEvent, FeedModel, FeedState};
use crate::http::DreamHttpClient;
use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, actions, div, prelude::*,
    uniform_list,
};
use project::Project;
use ui::{
    Button, Color, Divider, Icon, IconButton, IconName, IconSize, Label, LabelSize, Tooltip,
    h_flex, prelude::*, v_flex,
};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    dream_inspector,
    [
        /// Toggle visibility of the Dream Inspector panel.
        Toggle
    ]
);

pub const PANEL_KEY: &str = "DreamInspectorPanel";

pub struct DreamInspectorPanel {
    feed: Entity<FeedModel>,
    focus_handle: FocusHandle,
    follow: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for DreamInspectorPanel {}

impl Focusable for DreamInspectorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DreamInspectorPanel {
    pub fn new(
        workspace: &Workspace,
        _project: Entity<Project>,
        cx: &mut Context<Self>,
    ) -> Self {
        let http_client = workspace.project().read(cx).client().http_client();
        let base_url = std::env::var("REVERIE_URL").ok();
        let dream_http = DreamHttpClient::new(base_url, http_client);

        let feed = cx.new(|_| FeedModel::new(dream_http));

        // start_polling needs Context<FeedModel>; obtain it via the entity handle.
        {
            let weak = feed.downgrade();
            feed.update(cx, |_, feed_cx| {
                if let Some(entity) = weak.upgrade() {
                    FeedModel::start_polling(&entity, feed_cx);
                }
            });
        }

        let subscription = cx.subscribe(&feed, |_this, _feed, _event: &FeedEvent, cx| {
            cx.notify();
        });

        Self {
            feed,
            focus_handle: cx.focus_handle(),
            follow: true,
            _subscriptions: vec![subscription],
        }
    }

    fn render_pills(&self, cx: &Context<Self>) -> AnyElement {
        let filter = self.feed.read(cx).categories().clone();
        let feed = self.feed.clone();
        h_flex()
            .gap_1()
            .children(Category::ALL.iter().map(|&c| {
                let enabled = filter.is_enabled(c);
                let feed = feed.clone();
                Button::new(c.display_name(), c.display_name())
                    .label_size(LabelSize::Small)
                    .color(if enabled { Color::Success } else { Color::Muted })
                    .on_click(move |_, _, cx| {
                        feed.update(cx, |m, cx| m.toggle_category(c, cx));
                    })
            }))
            .into_any_element()
    }

    fn render_controls(&self, cx: &Context<Self>) -> AnyElement {
        let paused = self.feed.read(cx).is_paused();
        let feed = self.feed.clone();
        h_flex()
            .gap_1()
            .child(
                IconButton::new(
                    "dream-pause",
                    if paused { IconName::PlayFilled } else { IconName::DebugPause },
                )
                .tooltip(Tooltip::text("Pause polling"))
                .on_click(move |_, _, cx| {
                    feed.update(cx, |m, cx| m.set_paused(!m.is_paused(), cx));
                }),
            )
            .into_any_element()
    }

    fn render_feed(&self, cx: &Context<Self>) -> AnyElement {
        let snap: Vec<_> = self
            .feed
            .read(cx)
            .visible()
            .into_iter()
            .cloned()
            .collect();
        let count = snap.len();
        if count == 0 {
            return div()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Label::new("Waiting for events…").color(Color::Muted))
                .into_any_element();
        }
        uniform_list(
            "dream-feed",
            count,
            move |range, _window, _cx| {
                range
                    .map(|i| {
                        let ev = &snap[i];
                        h_flex()
                            .gap_3()
                            .child(
                                Label::new(format_time(ev.ts_ms))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(SharedString::from(ev.type_.clone()))
                                    .size(LabelSize::Small)
                                    .color(color_for_category(&ev.category)),
                            )
                            .child(
                                Label::new(SharedString::from(ev.summary.clone()))
                                    .size(LabelSize::Small),
                            )
                            .into_any_element()
                    })
                    .collect()
            },
        )
        .flex_1()
        .into_any_element()
    }

    fn render_status(&self, cx: &Context<Self>) -> AnyElement {
        let state = self.feed.read(cx).state().clone();
        let text: SharedString = match state {
            FeedState::Idle => "connecting…".into(),
            FeedState::Connected { total } => format!("connected · {total} events").into(),
            FeedState::Error(ref msg) => msg.clone().into(),
        };
        let color = match self.feed.read(cx).state() {
            FeedState::Error(_) => Color::Warning,
            _ => Color::Muted,
        };
        div()
            .p_1()
            .child(Label::new(text).size(LabelSize::Small).color(color))
            .into_any_element()
    }
}

fn format_time(ms: u64) -> SharedString {
    // HH:MM:SS in local time.
    let secs = (ms / 1000) as i64;
    let dt = chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    dt.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
        .into()
}

fn color_for_category(wire: &str) -> Color {
    match wire {
        "memory-io" => Color::Info,
        "dream" => Color::Accent,
        "tx" | "coord" | "gate" | "permission" => Color::Muted,
        _ => Color::Default,
    }
}

impl Render for DreamInspectorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .p_1()
                    .gap_2()
                    .child(self.render_pills(cx))
                    .child(div().flex_1())
                    .child(self.render_controls(cx)),
            )
            .child(Divider::horizontal())
            .child(self.render_feed(cx))
            .child(Divider::horizontal())
            .child(self.render_status(cx))
    }
}
