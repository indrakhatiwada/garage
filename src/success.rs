//! Shared, short-lived confirmation for published files and completed saves.

use std::{path::PathBuf, time::Duration};

use gpui_kit::component::{ActiveTheme, WindowExt, dialog::DialogEntrance};
use gpui_kit::*;
use serde_json::Value;

use crate::jobs::Jobs;

const VISIBLE_FOR: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub struct Success {
    title: &'static str,
    detail: String,
    location: PathBuf,
}

impl Success {
    pub fn saved(path: PathBuf) -> Self {
        Self {
            title: "Saved successfully",
            detail: "Your file is ready.".into(),
            location: path,
        }
    }

    pub fn exported(result: &Value) -> Option<Self> {
        let files: Vec<&str> = if let Some(files) = result.get("files").and_then(Value::as_array) {
            // A partial batch must not look like an entirely successful export.
            if files.iter().any(|file| file.get("error").is_some()) {
                return None;
            }
            files
                .iter()
                .filter_map(|file| file.as_str().or_else(|| file.get("dst")?.as_str()))
                .filter(|path| !path.is_empty())
                .collect()
        } else {
            result
                .get("dst")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .into_iter()
                .collect()
        };
        let first = PathBuf::from(*files.first()?);
        Some(Self {
            title: "Export complete",
            detail: if files.len() == 1 {
                "Your file is ready.".into()
            } else {
                format!("{} files are ready.", files.len())
            },
            location: if files.len() == 1 {
                first
            } else {
                first.parent().unwrap_or(&first).to_path_buf()
            },
        })
    }
}

pub struct SuccessFeedback {
    generation: u64,
    active: Option<u64>,
    timer: Option<Task<()>>,
    _events: Subscription,
}

impl SuccessFeedback {
    pub fn new(jobs: &Entity<Jobs>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = cx.subscribe_in(jobs, window, |this, _, success: &Success, window, cx| {
            this.show(success.clone(), window, cx);
        });
        Self {
            generation: 0,
            active: None,
            timer: None,
            _events: events,
        }
    }

    fn show(&mut self, success: Success, window: &mut Window, cx: &mut Context<Self>) {
        // Replace rapid completions and restart their lifetime. Old timers cannot
        // dismiss a newer confirmation, including after manual dismissal.
        self.timer = None;
        if self.active.take().is_some() {
            window.close_dialog(cx);
        }
        self.generation += 1;
        let generation = self.generation;
        self.active = Some(generation);
        let feedback = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let feedback = feedback.clone();
            dialog
                .centered()
                .w(px(400.))
                .p(px(28.))
                .rounded(px(16.))
                .entrance(DialogEntrance::FadeSlide)
                .title(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_5()
                        .w_full()
                        .text_center()
                        .child(
                            Checkmark {
                                progress: 1.,
                                color: cx.theme().primary,
                            }
                            .with_animation(
                                ElementId::NamedInteger("success-checkmark".into(), generation),
                                Animation::new(Duration::from_millis(620)),
                                |mut mark, progress| {
                                    mark.progress = progress;
                                    mark
                                },
                            ),
                        )
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(success.title),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_3()
                        .w_full()
                        .text_center()
                        .text_sm()
                        .child(success.detail.clone())
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .bg(cx.theme().muted)
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(success.location.to_string_lossy().to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Closes automatically"),
                        ),
                )
                .on_close(move |_, _, cx| {
                    let _ = feedback.update(cx, |this, _| {
                        if this.active == Some(generation) {
                            this.active = None;
                            this.timer = None;
                        }
                    });
                })
        });
        self.timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(VISIBLE_FOR).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.active == Some(generation) {
                    this.active = None;
                    window.close_dialog(cx);
                }
            });
        }));
    }
}

/// A single authored sequence: the ring seals, then the check is drawn.
/// GPUI resolves non-repeating animations to their final state for reduced motion.
#[derive(IntoElement)]
struct Checkmark {
    progress: f32,
    color: Hsla,
}

impl RenderOnce for Checkmark {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .size(px(80.))
            .flex_shrink_0()
            .rounded_full()
            .bg(self.color.opacity(0.10))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let at = |x, y| bounds.origin + point(px(x), px(y));
                        let ring = (self.progress / 0.65).clamp(0., 1.);
                        if ring > 0. {
                            let mut path = PathBuilder::stroke(px(2.));
                            for i in 0..=80 {
                                let angle = -std::f32::consts::FRAC_PI_2
                                    + std::f32::consts::TAU * ring * i as f32 / 80.;
                                let point = at(40. + 32. * angle.cos(), 40. + 32. * angle.sin());
                                if i == 0 {
                                    path.move_to(point);
                                } else {
                                    path.line_to(point);
                                }
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, self.color);
                            }
                        }
                        let check = ((self.progress - 0.4) / 0.6).clamp(0., 1.);
                        if check > 0. {
                            let a = at(25., 40.);
                            let b = at(36., 51.);
                            let c = at(56., 29.);
                            let mut path = PathBuilder::stroke(px(3.5));
                            path.move_to(a);
                            if check < 0.34 {
                                path.line_to(a + (b - a) * (check / 0.34));
                            } else {
                                path.line_to(b);
                                path.line_to(b + (c - b) * ((check - 0.34) / 0.66));
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, self.color);
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::Success;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn batch_exports_accept_both_engine_result_formats_without_double_counting() {
        for files in [
            json!(["/tmp/pages/1.png", "/tmp/pages/2.png"]),
            json!([{"dst": "/tmp/pages/1.png"}, {"dst": "/tmp/pages/2.png"}]),
        ] {
            let success =
                Success::exported(&json!({"files": files, "dst": "/tmp/pages/1.png"})).unwrap();
            assert_eq!(success.detail, "2 files are ready.");
            assert_eq!(success.location, PathBuf::from("/tmp/pages"));
        }
    }

    #[test]
    fn empty_or_partial_outputs_do_not_claim_success() {
        for result in [
            json!({"pages": 3}),
            json!({"dst": ""}),
            json!({"files": []}),
            json!({"files": [{"dst": "/tmp/a.jpg"}, {"error": "Failed"}]}),
        ] {
            assert!(Success::exported(&result).is_none());
        }
    }
}
