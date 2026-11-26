//! Shared UI primitives. Every colour comes from the theme
//! so light and dark both look native.

pub mod cleanup;
pub mod dates;
pub mod images;
pub mod ocr;
pub mod pages;
pub mod pdf;
pub mod redact;

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
    label::Label,
    progress::Progress,
};
use gpui_kit::*;

use crate::jobs::{Job, Jobs};

/// The empty state used by every file-based tool. Files can be dropped on any
/// part of the window; this is the visual affordance for that.
pub fn dropzone(title: &str, subtitle: &str, cx: &App) -> Stateful<Div> {
    div()
        .id("file-dropzone")
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(cx.theme().primary.opacity(0.04))
                .border_color(cx.theme().primary.opacity(0.6))
        })
        .relative()
        .overflow_hidden()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .py_8()
        .min_h(px(190.))
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .text_center()
        .rounded(px(12.))
        .border_1()
        .border_dashed()
        .border_color(cx.theme().border.opacity(0.9))
        .bg(cx.theme().muted.opacity(0.22))
        .child(
            img(if cx.theme().is_dark() {
                "garage/paper-dark.webp"
            } else {
                "garage/paper-light.webp"
            })
            .absolute()
            .inset_0()
            .size_full()
            .object_fit(ObjectFit::Cover)
            .opacity(0.32),
        )
        .child(
            div()
                .relative()
                .w(px(44.))
                .h(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .bg(cx.theme().primary.opacity(0.09))
                .text_color(cx.theme().primary)
                .child(Icon::default().path("garage/upload.svg").size(px(22.))),
        )
        .child(
            div()
                .relative()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .child(title.to_string()),
        )
        .child(
            div()
                .relative()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(subtitle.to_string()),
        )
}

/// Save a result somewhere the user chooses. Copies bytes locally — the input
/// is never overwritten, and an existing file is never replaced.
pub fn save_button<T: 'static>(
    id: &'static str,
    src: Option<String>,
    job_id: Option<String>,
    jobs: Entity<Jobs>,
    default_dir: Option<PathBuf>,
    cx: &mut Context<T>,
) -> impl IntoElement {
    save_button_labeled(id, src, job_id, jobs, "Save as…", false, default_dir, cx)
}

/// The same picker with an explicit label and emphasis, for flows where saving
/// is the main action rather than an afterthought.
#[allow(clippy::too_many_arguments)]
pub fn save_button_labeled<T: 'static>(
    id: &'static str,
    src: Option<String>,
    job_id: Option<String>,
    jobs: Entity<Jobs>,
    label: &'static str,
    primary: bool,
    default_dir: Option<PathBuf>,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let view = cx.entity().downgrade();
    let button = Button::new(id).label(label).small();
    let button = if primary {
        button.primary()
    } else {
        button.secondary()
    };
    button
        .disabled(src.is_none())
        .on_click(cx.listener(move |_, _, window, app| {
            let Some(source) = src.clone() else {
                return;
            };
            let source_path = PathBuf::from(&source);
            let dir = default_dir.clone().unwrap_or_else(|| {
                source_path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(PathBuf::from)
                            .map(|home| home.join("Documents"))
                    })
                    .unwrap_or_else(|| PathBuf::from("."))
            });
            let name = source_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let receiver = app.prompt_for_new_path(&dir, Some(&name));
            let jobs = jobs.clone();
            let job_id = job_id.clone();
            let view = view.clone();
            window
                .spawn(app, async move |cx| {
                    if let Ok(Ok(Some(destination))) = receiver.await {
                        if destination == source_path || destination.exists() {
                            return;
                        }
                        if std::fs::copy(&source_path, &destination).is_ok() {
                            let path = destination.to_string_lossy().to_string();
                            if let Some(id) = job_id {
                                jobs.update(cx, |jobs, _| jobs.mark_saved(&id, path));
                            }
                            let _ = view.update(cx, |_, cx| cx.notify());
                        }
                    }
                })
                .detach();
        }))
}

pub fn saved_note(path: Option<&str>, cx: &App) -> Option<Div> {
    path.map(|path| {
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(cx.theme().success.opacity(0.1))
            .text_xs()
            .text_color(cx.theme().success)
            .child("Saved to")
            .child(path.to_string())
    })
}

/// The same content measure and window-size breakpoints apply to every tool.
pub fn page(window: &Window) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_5()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .max_w(px(1120.))
        .mx_auto()
        .p(if window.viewport_size().width < px(960.) {
            px(20.)
        } else {
            px(32.)
        })
}

/// Width available inside a panel, including the sidebar and page gutters.
pub fn panel_width(window: &Window) -> f32 {
    let width = f32::from(window.viewport_size().width);
    let sidebar = if width < 960. { 88. } else { 232. };
    let gutters = if width < 960. { 40. } else { 64. };
    (width - sidebar).min(1120.) - gutters - 42.
}

pub fn card(cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .p_5()
        .rounded(px(12.))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
}

/// Results stay part of the task instead of nesting another card inside it.
pub fn result_section(cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .min_w_0()
        .w_full()
        .flex_shrink_0()
        .mt_2()
        .pt_5()
        .border_t_1()
        .border_color(cx.theme().border)
}

pub fn heading(title: &str, subtitle: &str, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .min_w_0()
        .pt_1()
        .pb_1()
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(subtitle.to_string()),
        )
}

pub fn field_label(text: &str, cx: &App) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().foreground)
        .child(text.to_string())
}

pub fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn tiny(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn stat(label: &str, value: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(field_label(label, cx))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::MEDIUM)
                .child(value.into()),
        )
}

pub fn pill(text: impl Into<SharedString>, color: Hsla, _cx: &App) -> Div {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(color.opacity(0.14))
        .border_1()
        .border_color(color.opacity(0.25))
        .text_xs()
        .text_color(color)
        .child(text.into())
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn error_box(message: &str, cx: &App) -> Div {
    div()
        .flex()
        .gap_2()
        .p_3()
        .rounded_md()
        .bg(cx.theme().danger.opacity(0.12))
        .border_1()
        .border_color(cx.theme().danger.opacity(0.35))
        .text_sm()
        .text_color(cx.theme().danger)
        .child(message.to_string())
}

pub fn success_box(message: &str, cx: &App) -> Div {
    div()
        .flex()
        .gap_2()
        .p_3()
        .rounded_md()
        .bg(cx.theme().success.opacity(0.12))
        .border_1()
        .border_color(cx.theme().success.opacity(0.35))
        .text_sm()
        .text_color(cx.theme().success)
        .child(message.to_string())
}

pub fn job_progress(job: Option<&Job>, cx: &App) -> Div {
    let mut element = div().flex().flex_col().gap_2();
    let Some(job) = job else {
        return element;
    };
    if job.is_running() {
        element = element
            .child(
                Progress::new(SharedString::from(job.id.clone()))
                    .value(job.pct)
                    .accessibility_label("Progress"),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if job.note.is_empty() {
                        "working".to_string()
                    } else {
                        job.note.clone()
                    })
                    .child(format!("{:.0}%", job.pct)),
            );
    }
    if let Some(error) = job.error() {
        element = element.child(error_box(error, cx));
    }
    element
}

pub fn file_row(name: &str, detail: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .justify_between()
        .items_center()
        .w_full()
        .min_w_0()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(cx.theme().muted.opacity(0.35))
        .child(
            div()
                .text_sm()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(name.to_string()),
        )
        .child(
            Label::new(detail.into())
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground),
        )
}

/// Diminishing subtitle used under a result.
pub fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
