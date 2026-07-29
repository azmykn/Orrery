//! Activity log view — recent in-app events (fleet ops, push/pull, scans,
//! soft CI notices). Backed by the in-memory ring in `activity_log`.

use gpui::{
    Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px, rgb,
};

use crate::activity_log::{LogEntry, LogLevel};
use crate::icon::lucide;
use crate::shell::OrreryApp;
use crate::theme::Theme;

/// Render the Log / Activity view.
pub fn render(entries: &[LogEntry], t: &Theme, app: &Entity<OrreryApp>) -> impl IntoElement {
    let body = if entries.is_empty() {
        super::note(
            "No activity yet — scans, pull/push, and fleet ops will show up here.",
            t,
        )
        .into_any_element()
    } else {
        list(entries, t).into_any_element()
    };

    // Custom chrome: Clear instead of Refresh (nothing to reload).
    let app_clear = app.clone();
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(t.page))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .h(px(52.))
                .px(px(20.))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(t.text_h3))
                        .text_color(rgb(t.fg0))
                        .child("Log"),
                )
                .child(super::muted_mono(format!("{} events", entries.len()), t))
                .child(div().flex_1())
                .child(
                    div()
                        .id("log-clear")
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(t.r_sm))
                        .border_1()
                        .border_color(rgb(t.border))
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(t.surface_hover)).text_color(rgb(t.fg0)))
                        .child("Clear")
                        .on_click(move |_ev, _win, cx| {
                            app_clear.update(cx, |this, cx| {
                                this.activity_log.clear();
                                cx.notify();
                            });
                        }),
                ),
        )
        .child(
            div()
                .id("log-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p(px(20.))
                .child(body),
        )
}

fn list(entries: &[LogEntry], t: &Theme) -> impl IntoElement {
    let mut col = div().flex().flex_col().gap(px(4.));
    for e in entries {
        col = col.child(row(e, t));
    }
    col
}

fn row(e: &LogEntry, t: &Theme) -> impl IntoElement {
    let (icon, color) = match e.level {
        LogLevel::Info => ("bell", t.fg2),
        LogLevel::Warn => ("circle-alert", t.dirty),
        LogLevel::Error => ("circle-alert", t.behind),
    };
    div()
        .id(SharedString::from(format!("log-{}", e.id)))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(t.r_sm))
        .border_1()
        .border_color(rgb(t.border))
        .bg(rgb(t.surface))
        .child(lucide(icon, 14., color))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_family("monospace")
                                .text_size(px(t.text_data_sm))
                                .text_color(rgb(t.fg3))
                                .child(SharedString::from(format_time(e.at))),
                        )
                        .child(super::tag(e.level.label(), color, t)),
                )
                .child(
                    div()
                        .text_size(px(t.text_small))
                        .text_color(rgb(t.fg1))
                        .child(e.message.clone()),
                ),
        )
}

/// Compact local-ish clock from unix seconds (HH:MM:SS).
fn format_time(at: i64) -> String {
    let secs = at.rem_euclid(86_400) as u32;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
