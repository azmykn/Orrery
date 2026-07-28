//! The grid-view repo card: layout, spacing, token colors and real
//! lucide/devicon/host icons, with live launchers + favorite toggle.
//!
//! Cards render inside `uniform_list` (a `'static` closure), so every stored
//! handler/hover closure captures owned values — never a borrow of `&Theme`.

use gpui::{
    App, ClickEvent, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use orrery_core::attention::Severity;
use orrery_core::{cache, launch};

use crate::data::Row;
use crate::fleet::{self, FleetOp};
use crate::icon::{brand, langicon, lucide};
use crate::shell::OrreryApp;
use crate::theme::{Theme, devicon_stem, lang_color};

const MONO: &str = "monospace";

/// Shared right-click actions for a repo card, list row, or TREE row.
///
/// Action visibility is gated by repo state (`can_stage` / `can_commit` /
/// `can_push` / `can_update_submodules`) and by `can_generate` (`aiReady` ∧
/// dirty) so AI items stay hidden when the backend is unreachable. When the
/// fleet selection includes this repo, also offers the full bulk Actions menu
/// (same items as the top gear dropdown) over that set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_repo_context_menu(
    menu: PopupMenu,
    app: Entity<OrreryApp>,
    repo_id: SharedString,
    can_stage: bool,
    can_commit: bool,
    can_generate: bool,
    can_push: bool,
    can_update_submodules: bool,
    fleet_targets: Vec<String>,
    fleet_ai_ready: bool,
    fleet_has_dirty: bool,
    fleet_idle: bool,
    host_url: SharedString,
    host: SharedString,
) -> PopupMenu {
    let mut m = menu;
    let (a1, id1) = (app.clone(), repo_id.clone());
    m = m.item(
        PopupMenuItem::new("Open drawer").on_click(move |_, window, cx| {
            a1.update(cx, |this, cx| this.open_drawer(id1.clone(), window, cx));
        }),
    );
    if !host_url.is_empty() {
        let url = host_url;
        let label = crate::data::open_on_host_label(host.as_ref());
        m = m.item(PopupMenuItem::new(label).on_click(move |_, _, _cx| {
            let _ = launch::open(&url);
        }));
    }
    if can_stage {
        let (a, id) = (app.clone(), repo_id.clone());
        m = m.item(PopupMenuItem::new("Stage all").on_click(move |_, _, cx| {
            a.update(cx, |this, cx| {
                this.run_fleet_repos(FleetOp::StageAll, vec![id.to_string()], cx);
            });
        }));
    }
    if can_commit {
        let (a_discard, id_discard) = (app.clone(), repo_id.clone());
        m = m.item(
            PopupMenuItem::new("Revert all changes").on_click(move |_, _, cx| {
                a_discard.update(cx, |this, cx| {
                    this.start_fleet_discard_repos(vec![id_discard.to_string()], cx);
                });
            }),
        );
        let (a_open, id_open) = (app.clone(), repo_id.clone());
        m = m.item(
            PopupMenuItem::new("Commit All…").on_click(move |_, window, cx| {
                a_open.update(cx, |this, cx| this.open_drawer(id_open.clone(), window, cx));
            }),
        );
    }
    if can_generate {
        let (a, id) = (app.clone(), repo_id.clone());
        m = m.item(
            PopupMenuItem::new("Generate & commit").on_click(move |_, window, cx| {
                a.update(cx, |this, cx| {
                    this.repo_generate_and_commit(id.clone(), window, cx);
                });
            }),
        );
    }
    if can_push {
        let (a, id) = (app.clone(), repo_id.clone());
        m = m.item(PopupMenuItem::new("Push").on_click(move |_, _, cx| {
            a.update(cx, |this, cx| {
                this.run_fleet_repos(FleetOp::Push, vec![id.to_string()], cx);
            });
        }));
    }
    let (a_f, id_f) = (app.clone(), repo_id.clone());
    let (a_p, id_p) = (app.clone(), repo_id.clone());
    m = m
        .separator()
        .item(PopupMenuItem::new("Fetch").on_click(move |_, _, cx| {
            a_f.update(cx, |this, cx| {
                this.run_fleet_repos(FleetOp::Fetch, vec![id_f.to_string()], cx);
            });
        }))
        .item(PopupMenuItem::new("Pull").on_click(move |_, _, cx| {
            a_p.update(cx, |this, cx| {
                this.run_fleet_repos(FleetOp::Pull, vec![id_p.to_string()], cx);
            });
        }));
    if can_update_submodules {
        let (a, id) = (app.clone(), repo_id.clone());
        m = m.item(
            PopupMenuItem::new("Update submodules").on_click(move |_, _, cx| {
                a.update(cx, |this, cx| {
                    this.run_fleet_repos(FleetOp::SubmoduleUpdate, vec![id.to_string()], cx);
                });
            }),
        );
    }
    let (a_pr, id_pr) = (app.clone(), repo_id.clone());
    m = m.item(PopupMenuItem::new("Prune").on_click(move |_, _, cx| {
        a_pr.update(cx, |this, cx| {
            this.adopt_fleet_targets(&[id_pr.to_string()]);
            this.start_fleet_prune(cx);
        });
    }));
    let (a_rs, id_rs) = (app.clone(), repo_id.clone());
    m = m.item(PopupMenuItem::new("Reset hard").on_click(move |_, _, cx| {
        a_rs.update(cx, |this, cx| {
            this.adopt_fleet_targets(&[id_rs.to_string()]);
            this.start_fleet_reset(cx);
        });
    }));
    let (a_ide, id_ide) = (app.clone(), repo_id.clone());
    m = m.item(PopupMenuItem::new("Open in IDE").on_click(move |_, _, cx| {
        a_ide.update(cx, |this, cx| {
            this.adopt_fleet_targets(&[id_ide.to_string()]);
            this.launch_selected(cx);
        });
    }));
    // Full fleet Actions when this repo is part of a multi-selection.
    let in_selection = fleet_targets
        .iter()
        .any(|id| id.as_str() == repo_id.as_ref());
    if in_selection && fleet_targets.len() > 1 {
        let n = fleet_targets.len();
        m = m.separator().label(format!("Selection ({n})"));
        m = fleet::fill_fleet_actions_menu(
            m,
            app,
            fleet_targets,
            fleet_ai_ready,
            fleet_has_dirty,
            fleet_idle,
            None,
        );
    }
    m
}

/// Selection-scoped fleet targets for context menus: if the clicked repo is in
/// the multi-selection, return that whole set (grid order); otherwise just the
/// clicked repo.
pub(crate) fn fleet_context_targets(app: &OrreryApp, repo_id: &str) -> Vec<String> {
    if !app.selected.is_empty() && app.selected.iter().any(|id| id.as_ref() == repo_id) {
        app.rows
            .iter()
            .filter(|r| app.selected.contains(&r.id))
            .map(|r| r.id.to_string())
            .collect()
    } else {
        vec![repo_id.to_string()]
    }
}

/// Per-card interactive state, resolved by the caller inside the
/// `uniform_list` render closure (cheap per-row lookups by id — no allocation
/// beyond the small reason-chip vec).
#[derive(Clone)]
pub struct CardState {
    /// A live agent session is running in this repo.
    pub active: bool,
    /// An urgent attention item (`orrery_core::attention::Severity::Urgent`).
    pub urgent: bool,
    /// This repo is in the fleet multi-selection.
    pub selected: bool,
    /// Any selection exists — keeps every card's checkbox visible (not just
    /// hover-revealed) while a selection is being built.
    pub selecting: bool,
    /// Top attention reasons (urgent-first, kind-deduped) for chip labels.
    pub reason_chips: Vec<(SharedString, Severity)>,
    /// Extra unique reasons beyond `reason_chips` (renders as "+N").
    pub reasons_more: usize,
    /// Optional glance line under the path (top item summary · branch detail).
    pub reason_subtitle: Option<SharedString>,
}

/// The fleet multi-select checkbox: filled with a check when selected;
/// otherwise invisible (keeping its layout slot, so nothing shifts) until the
/// card's hover group is hovered or any selection exists. Clicking toggles the
/// repo in the selection without opening the drawer.
fn select_box(
    id: SharedString,
    group: SharedString,
    row_id: SharedString,
    selected: bool,
    selecting: bool,
    t: &Theme,
    app: &Entity<OrreryApp>,
) -> impl IntoElement {
    let app = app.clone();
    let (border_c, bg_c) = if selected {
        (t.primary, t.primary)
    } else {
        (t.border_strong, t.button_bg)
    };
    let hov = t.primary;
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(16.))
        .h(px(16.))
        .rounded(px(t.r_xs))
        .border_1()
        .border_color(rgb(border_c))
        .bg(rgb(bg_c))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov)))
        .on_click(move |_ev, _win, cx| {
            // Selection toggles in place — don't also open the drawer.
            cx.stop_propagation();
            let row_id = row_id.clone();
            app.update(cx, |this, cx| this.toggle_selected(row_id, cx));
        });
    if selected {
        b = b.child(lucide("check", 12., t.page));
    } else if !selecting {
        b = b.invisible().group_hover(group, |s| s.visible());
    }
    b
}

/// Click handler for a card's open-drawer region: Ctrl/Cmd+click toggles the
/// fleet selection, a plain click opens the drawer.
fn open_or_select(
    app: &Entity<OrreryApp>,
    row_id: &SharedString,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let app = app.clone();
    let id = row_id.clone();
    move |ev, window, cx| {
        let id = id.clone();
        if ev.modifiers().secondary() {
            app.update(cx, |this, cx| this.toggle_selected(id, cx));
        } else {
            app.update(cx, |this, cx| this.open_drawer(id, window, cx));
        }
    }
}

/// The language mark: the multicolor devicon when one is bundled, else the
/// brand-color dot (no devicon for this language). Shared with the sidebar's
/// LANGUAGES list.
pub(crate) fn lang_mark(language: &str, t: &Theme) -> gpui::AnyElement {
    if let Some(stem) = devicon_stem(language)
        && crate::assets::has_icon(&format!("devicon/{stem}.svg"))
    {
        return langicon(stem, 16.).into_any_element();
    }
    div()
        .w(px(9.))
        .h(px(9.))
        .rounded_full()
        .bg(rgb(lang_color(language, t.fg3)))
        .into_any_element()
}

/// The urgent-attention mark: a small flat dot in the danger token, shown
/// with the git status indicators when the repo has an Urgent item and no
/// reason chips (chips already tint urgency).
fn urgent_dot(t: &Theme) -> impl IntoElement {
    div().w(px(8.)).h(px(8.)).rounded_full().bg(rgb(t.behind))
}

/// Severity → (bg, fg) for a flat attention-reason chip.
fn reason_chip_colors(severity: Severity, t: &Theme) -> (u32, u32) {
    match severity {
        Severity::Urgent => (t.danger_badge, t.behind),
        Severity::Attention => (t.button_bg, t.dirty),
        Severity::Info => (t.button_bg, t.fg2),
    }
}

/// One flat reason chip ("CI failing", "Not pushed", …).
fn reason_chip(label: SharedString, severity: Severity, t: &Theme) -> impl IntoElement {
    let (bg, fg) = reason_chip_colors(severity, t);
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(if severity == Severity::Urgent {
            t.behind
        } else {
            t.border
        }))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(fg))
        .child(label)
}

/// "+N" overflow chip when a repo has more unique reasons than we show.
fn reason_more_chip(n: usize, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(t.r_xs))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2))
        .child(SharedString::from(format!("+{n}")))
}

/// Leading status-row segment: reason chips (and optional urgent dot fallback).
fn attention_marks(state: &CardState, t: &Theme) -> impl IntoElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .flex_none();
    if state.reason_chips.is_empty() {
        if state.urgent {
            row = row.child(urgent_dot(t));
        }
    } else {
        for (label, severity) in &state.reason_chips {
            row = row.child(reason_chip(label.clone(), *severity, t));
        }
        if state.reasons_more > 0 {
            row = row.child(reason_more_chip(state.reasons_more, t));
        }
    }
    row
}

/// One status segment: a lucide icon + label, both in `color`.
fn seg(icon_name: &str, label: SharedString, color: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .text_color(rgb(color))
        .child(lucide(icon_name, 14., color))
        .child(label)
}

/// A clickable launcher button. `wide` ones flex to fill (IDE/Agent); narrow
/// ones are fixed 38px icon slots (Folder/Host). `on` fires on click.
fn button(
    id: SharedString,
    content: impl IntoElement,
    wide: bool,
    t: &Theme,
    on: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let (hov_border, hov_fg) = (t.border_strong, t.fg0);
    let b = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .py(px(8.))
        .rounded(px(t.r_sm))
        .bg(rgb(t.button_bg))
        .border_1()
        .border_color(rgb(t.border))
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg1))
        .font_family(MONO)
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(hov_border)).text_color(rgb(hov_fg)))
        .on_click(move |_ev, _win, cx| on(cx))
        .child(content);
    if wide {
        b.flex_1().min_w(px(0.))
    } else {
        b.w(px(38.))
    }
}

pub fn card(
    row: &Row,
    idx: usize,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ide_cmd: &str,
    agent_cmd: &str,
    state: CardState,
) -> impl IntoElement {
    // Hover group: reveals the (otherwise invisible) select checkbox.
    let group = SharedString::from(format!("cardg-{idx}"));
    // ── head: select box + language mark + name, and the favorite star ─────
    let fav_star = {
        let app = app.clone();
        let id = row.id.clone();
        let fav = row.favorite;
        div()
            .id(SharedString::from(format!("fav-{idx}")))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(lucide("star", 16., if fav { t.star } else { t.fg3 }))
            .on_click(move |_ev, _win, cx| {
                // Don't let the star toggle also open the drawer.
                cx.stop_propagation();
                let next = !fav;
                let _ = cache::set_favorite(&id, next);
                app.update(cx, |this, cx| {
                    if let Some(r) = this.rows.get_mut(idx) {
                        r.favorite = next;
                    }
                    cx.notify();
                });
            })
    };

    let head = div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(9.))
                .min_w(px(0.))
                .text_size(px(t.text_h3))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(t.fg0))
                .child(select_box(
                    SharedString::from(format!("sel-{idx}")),
                    group.clone(),
                    row.id.clone(),
                    state.selected,
                    state.selecting,
                    t,
                    app,
                ))
                .child(lang_mark(&row.language, t))
                .child(div().min_w(px(0.)).truncate().child(row.name.clone()))
                .children((row.child_count > 0).then(|| {
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(t.r_xs))
                        .bg(rgb(t.button_bg))
                        .font_family(MONO)
                        .text_size(px(t.text_data_sm))
                        .text_color(rgb(t.fg2))
                        .child(SharedString::from(format!("{} sub", row.child_count)))
                }))
                // Live agent session running in this repo.
                .children(
                    state
                        .active
                        .then(|| lucide("square-terminal", 13., t.clean)),
                ),
        )
        .child(fav_star);

    // ── slug · path (+ optional attention subtitle) ───────────────────────
    let slug_line = div()
        .mt(px(6.))
        .truncate()
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2))
        .child(SharedString::from(format!("{} · {}", row.slug, row.path)));
    let has_reason_sub = state.reason_subtitle.is_some();
    let slug_block = if let Some(sub) = &state.reason_subtitle {
        div().flex().flex_col().gap(px(2.)).child(slug_line).child(
            div()
                .truncate()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(if state.urgent { t.behind } else { t.dirty }))
                .child(sub.clone()),
        )
    } else {
        div().child(slug_line)
    };

    // ── description (2-line clamp ≈ 38px; shorter when a reason subtitle
    //    is present so the fixed card height still fits the launcher row) ─
    let desc = div()
        .mt(px(9.))
        .h(px(if has_reason_sub { 22. } else { 38. }))
        .overflow_hidden()
        .text_size(px(t.text_small))
        .line_height(px(19.))
        .text_color(rgb(t.fg1))
        .child(row.description.clone());

    // ── git status row ────────────────────────────────────────────────────
    let mut status = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(13.))
        .mt(px(12.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm));
    // Attention reason chips (or urgent dot fallback) lead the status row.
    status = status.child(attention_marks(&state, t));
    status = status.child(seg("git-branch", row.branch.clone(), t.fg2));
    if row.ahead > 0 || row.behind > 0 {
        let color = if row.behind > 0 { t.behind } else { t.clean };
        status = status.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .text_color(rgb(color))
                .child(lucide("arrow-up", 13., color))
                .child(SharedString::from(row.ahead.to_string()))
                .child(lucide("arrow-down", 13., color))
                .child(SharedString::from(row.behind.to_string())),
        );
    }
    if row.dirty > 0 {
        status = status.child(seg(
            "circle-dot",
            SharedString::from(row.dirty.to_string()),
            t.dirty,
        ));
    }

    // ── host row: private · stars · release · age · host brand ───────────
    let mut host = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .mt(px(9.))
        .font_family(MONO)
        .text_size(px(t.text_data_sm))
        .text_color(rgb(t.fg2));
    if row.private {
        host = host.child(lucide("lock", 13., t.fg3));
    }
    if !row.host.is_empty() {
        host = host.child(seg("star", row.stars.clone(), t.star));
    }
    if !row.release.is_empty() {
        host = host.child(seg("tag", row.release.clone(), t.fg2));
    }
    host = host.child(seg("clock", row.age.clone(), t.fg2));
    if !row.host.is_empty() {
        host = host
            .child(div().flex_1())
            .child(brand(&row.host, 14., t.fg2));
    }

    // ── launchers (live) ─────────────────────────────────────────────────
    let id_ide = SharedString::from(format!("ide-{idx}"));
    let id_agent = SharedString::from(format!("agent-{idx}"));
    let id_folder = SharedString::from(format!("folder-{idx}"));
    let id_host = SharedString::from(format!("host-{idx}"));

    let ide_action = {
        let (path, cmd) = (row.id.clone(), ide_cmd.to_string());
        move |_cx: &mut App| {
            let _ = launch::launch(&cmd, &path);
        }
    };
    let agent_action = {
        let (path, cmd) = (row.id.clone(), agent_cmd.to_string());
        move |_cx: &mut App| {
            let _ = launch::spawn(&cmd, &path);
        }
    };
    let folder_action = {
        let path = row.id.clone();
        move |_cx: &mut App| {
            let _ = launch::open(&path);
        }
    };

    let mut acts = div()
        .flex()
        .flex_row()
        .gap(px(8.))
        .mt(px(14.))
        .child(button(
            id_ide,
            SharedString::from("Open in IDE"),
            true,
            t,
            ide_action,
        ))
        .child(button(
            id_agent,
            SharedString::from("Agent"),
            true,
            t,
            agent_action,
        ))
        .child(button(
            id_folder,
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            folder_action,
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        acts = acts.child(button(
            id_host,
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move |_cx: &mut App| {
                let _ = launch::open(&url);
            },
        ));
    }

    // ── clickable content region → opens the repo drawer ──────────────────
    // Everything except the launcher row opens the drawer on click (Ctrl+click
    // toggles the fleet selection instead); the launchers (and the favorite
    // star / select box, which stop propagation) act in place.
    let mut body = div()
        .id(SharedString::from(format!("open-{idx}")))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(open_or_select(app, &row.id))
        .child(head)
        .child(slug_block)
        .child(desc);

    // AI summary, when present, sits between description and status.
    if !row.ai_summary.is_empty() {
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .mt(px(9.))
                .h(px(17.))
                .overflow_hidden()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.ai))
                .child(lucide("sparkles", 13., t.ai))
                .child(row.ai_summary.clone()),
        );
    }
    body = body.child(status).child(host);

    // ── card shell (hover lift via border/bg; accent border when selected) ─
    let (hov_border, hov_bg) = (t.border_accent, t.surface_hover);
    let app_menu = app.clone();
    let repo_id = row.id.clone();
    let can_stage = row.unstaged > 0;
    let can_commit = row.dirty > 0;
    let ahead = row.ahead;
    let can_update_submodules = row.child_count > 0;
    div()
        .id(SharedString::from(format!("card-{idx}")))
        .group(group)
        .flex()
        .flex_1()
        .flex_col()
        .min_w(px(0.))
        .px(px(15.))
        .py(px(14.))
        .bg(rgb(t.surface))
        .border_1()
        .border_color(rgb(if state.selected {
            t.border_accent
        } else {
            t.border
        }))
        .rounded(px(t.r_md))
        .overflow_hidden()
        .hover(move |s| s.border_color(rgb(hov_border)).bg(rgb(hov_bg)))
        .context_menu(move |menu, _window, cx| {
            let st = app_menu.read(cx);
            let can_generate = can_commit && st.services.ai_ready;
            let can_push = ahead > 0 && !st.is_pull_only(repo_id.as_ref());
            let fleet_ai = st.services.ai_ready;
            let fleet_targets = fleet_context_targets(st, repo_id.as_ref());
            let fleet_has_dirty = st
                .rows
                .iter()
                .any(|r| st.selected.contains(&r.id) && r.dirty > 0);
            let fleet_idle = st.fleet_actions_idle();
            let (host_url, host) = st
                .rows
                .iter()
                .find(|r| r.id.as_ref() == repo_id.as_ref())
                .map(|r| (r.url.clone(), r.host.clone()))
                .unwrap_or_default();
            fill_repo_context_menu(
                menu,
                app_menu.clone(),
                repo_id.clone(),
                can_stage,
                can_commit,
                can_generate,
                can_push,
                can_update_submodules,
                fleet_targets,
                fleet_ai,
                fleet_has_dirty,
                fleet_idle,
                host_url,
                host,
            )
        })
        .child(body)
        .child(acts)
}

/// A compact single-row repo entry for the list layout — the same data and
/// launchers as the grid card, laid out horizontally in one fixed-height row.
pub(crate) fn list_item(
    row: &Row,
    idx: usize,
    t: &Theme,
    app: &Entity<OrreryApp>,
    ide_cmd: &str,
    agent_cmd: &str,
    state: CardState,
) -> impl IntoElement {
    // Hover group: reveals the (otherwise invisible) select checkbox.
    let group = SharedString::from(format!("listg-{idx}"));
    let select = select_box(
        SharedString::from(format!("lsel-{idx}")),
        group.clone(),
        row.id.clone(),
        state.selected,
        state.selecting,
        t,
        app,
    );
    let fav_star = {
        let app = app.clone();
        let id = row.id.clone();
        let fav = row.favorite;
        div()
            .id(SharedString::from(format!("lfav-{idx}")))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(lucide("star", 15., if fav { t.star } else { t.fg3 }))
            .on_click(move |_ev, _win, cx| {
                cx.stop_propagation();
                let next = !fav;
                let _ = cache::set_favorite(&id, next);
                app.update(cx, |this, cx| {
                    if let Some(r) = this.rows.get_mut(idx) {
                        r.favorite = next;
                    }
                    cx.notify();
                });
            })
    };

    // Name + slug·path (+ optional attention subtitle); click opens drawer.
    let name_col = {
        let mut col = div().flex().flex_col().min_w(px(0.)).child(
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(t.text_small))
                .text_color(rgb(t.fg0))
                .child(row.name.clone()),
        );
        col = col.child(
            div()
                .truncate()
                .font_family(MONO)
                .text_size(px(t.text_data_sm))
                .text_color(rgb(t.fg2))
                .child(SharedString::from(format!("{} · {}", row.slug, row.path))),
        );
        if let Some(sub) = &state.reason_subtitle {
            col = col.child(
                div()
                    .truncate()
                    .font_family(MONO)
                    .text_size(px(t.text_data_sm))
                    .text_color(rgb(if state.urgent { t.behind } else { t.dirty }))
                    .child(sub.clone()),
            );
        }
        col
    };
    let open = {
        div()
            .id(SharedString::from(format!("lopen-{idx}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .flex_1()
            .min_w(px(0.))
            .cursor_pointer()
            .on_click(open_or_select(app, &row.id))
            .child(lang_mark(&row.language, t))
            .child(name_col)
            // Live agent session running in this repo.
            .children(
                state
                    .active
                    .then(|| lucide("square-terminal", 13., t.clean)),
            )
    };

    // Status segments (attention reasons / branch / ahead-behind / dirty / …).
    let mut status = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .flex_none()
        .font_family(MONO)
        .text_size(px(t.text_data_sm));
    status = status.child(attention_marks(&state, t));
    status = status.child(seg("git-branch", row.branch.clone(), t.fg2));
    if row.ahead > 0 || row.behind > 0 {
        let color = if row.behind > 0 { t.behind } else { t.clean };
        status = status.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .text_color(rgb(color))
                .child(lucide("arrow-up", 13., color))
                .child(SharedString::from(row.ahead.to_string()))
                .child(lucide("arrow-down", 13., color))
                .child(SharedString::from(row.behind.to_string())),
        );
    }
    if row.dirty > 0 {
        status = status.child(seg(
            "circle-dot",
            SharedString::from(row.dirty.to_string()),
            t.dirty,
        ));
    }
    if !row.host.is_empty() {
        status = status.child(seg("star", row.stars.clone(), t.star));
    }
    status = status.child(seg("clock", row.age.clone(), t.fg2));

    // Launchers — narrow icon buttons.
    let ide_action = {
        let (path, cmd) = (row.id.clone(), ide_cmd.to_string());
        move |_cx: &mut App| {
            let _ = launch::launch(&cmd, &path);
        }
    };
    let agent_action = {
        let (path, cmd) = (row.id.clone(), agent_cmd.to_string());
        move |_cx: &mut App| {
            let _ = launch::spawn(&cmd, &path);
        }
    };
    let folder_action = {
        let path = row.id.clone();
        move |_cx: &mut App| {
            let _ = launch::open(&path);
        }
    };
    let mut acts = div()
        .flex()
        .flex_row()
        .gap(px(6.))
        .flex_none()
        .child(button(
            SharedString::from(format!("lide-{idx}")),
            lucide("code", 15., t.fg1),
            false,
            t,
            ide_action,
        ))
        .child(button(
            SharedString::from(format!("lagent-{idx}")),
            lucide("square-terminal", 15., t.fg1),
            false,
            t,
            agent_action,
        ))
        .child(button(
            SharedString::from(format!("lfolder-{idx}")),
            lucide("folder-open", 15., t.fg1),
            false,
            t,
            folder_action,
        ));
    if !row.url.is_empty() {
        let url = row.url.clone();
        acts = acts.child(button(
            SharedString::from(format!("lhost-{idx}")),
            lucide("external-link", 15., t.fg1),
            false,
            t,
            move |_cx: &mut App| {
                let _ = launch::open(&url);
            },
        ));
    }

    let hov_bg = t.surface_hover;
    let app_menu = app.clone();
    let repo_id = row.id.clone();
    let can_stage = row.unstaged > 0;
    let can_commit = row.dirty > 0;
    let ahead = row.ahead;
    let can_update_submodules = row.child_count > 0;
    let mut shell = div()
        .id(SharedString::from(format!("lrow-{idx}")))
        .group(group)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .w_full()
        .h(px(72.))
        .px(px(16.))
        .border_b_1()
        .border_color(rgb(t.border))
        .hover(move |s| s.bg(rgb(hov_bg)))
        .context_menu(move |menu, _window, cx| {
            let st = app_menu.read(cx);
            let can_generate = can_commit && st.services.ai_ready;
            let can_push = ahead > 0 && !st.is_pull_only(repo_id.as_ref());
            let fleet_ai = st.services.ai_ready;
            let fleet_targets = fleet_context_targets(st, repo_id.as_ref());
            let fleet_has_dirty = st
                .rows
                .iter()
                .any(|r| st.selected.contains(&r.id) && r.dirty > 0);
            let fleet_idle = st.fleet_actions_idle();
            let (host_url, host) = st
                .rows
                .iter()
                .find(|r| r.id.as_ref() == repo_id.as_ref())
                .map(|r| (r.url.clone(), r.host.clone()))
                .unwrap_or_default();
            fill_repo_context_menu(
                menu,
                app_menu.clone(),
                repo_id.clone(),
                can_stage,
                can_commit,
                can_generate,
                can_push,
                can_update_submodules,
                fleet_targets,
                fleet_ai,
                fleet_has_dirty,
                fleet_idle,
                host_url,
                host,
            )
        })
        .child(select)
        .child(fav_star)
        .child(open)
        .child(status)
        .child(acts);
    if state.selected {
        shell = shell.bg(rgb(t.accent_wash));
    }
    shell
}
