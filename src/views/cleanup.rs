use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    switch::Switch,
};
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, field_label, file_row, heading, human_bytes, job_progress, muted, note, page,
    result_section, save_button, saved_note, stat, success_box,
};
use crate::engine::Engine;
use crate::jobs::{Job, Jobs, Tool};

pub struct CleanupView {
    engine: Engine,
    jobs: Entity<Jobs>,
    path: Option<PathBuf>,
    deskew: bool,
    denoise: bool,
    threshold: bool,
    autocrop: bool,
    job: Option<String>,
    original_job: Option<String>,
    preview_job: Option<String>,
    cache_dir: PathBuf,
    revision: u64,
    show_cleaned: bool,
    output_dir: Option<PathBuf>,
    _observe: Subscription,
}

impl CleanupView {
    pub fn new(
        engine: Engine,
        jobs: Entity<Jobs>,
        cx: &mut Context<Self>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        let observe = cx.observe(&jobs, |_, _, cx| cx.notify());
        Self {
            engine,
            jobs,
            path: None,
            deskew: true,
            denoise: true,
            threshold: true,
            autocrop: true,
            job: None,
            original_job: None,
            preview_job: None,
            cache_dir: std::env::temp_dir().join(format!(
                "garage-cleanup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )),
            revision: 0,
            show_cleaned: true,
            output_dir,
            _observe: observe,
        }
    }

    pub(crate) fn set_output_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        self.output_dir = dir;
        cx.notify();
    }

    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a photo or scan".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |view, cx| {
                    if let Some(path) = paths.into_iter().next() {
                        view.load(path, cx);
                    }
                });
            }
        })
        .detach();
    }

    pub(crate) fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if let Some(path) = paths.into_iter().next() {
            self.load(path, cx);
        }
    }

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = Some(path);
        self.job = None;
        self.show_cleaned = true;
        self.load_original_preview(cx);
        self.refresh_preview(cx);
        cx.notify();
    }

    fn load_original_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.original_job.take() {
            self.engine.cancel(&id);
        }
        let Some(path) = &self.path else {
            return;
        };
        self.revision += 1;
        let dst = self
            .cache_dir
            .join(format!("original-{}.png", self.revision));
        let id = self.engine.call(
            "image.preview",
            json!({ "src": path.to_string_lossy(), "dst": dst.to_string_lossy() }),
        );
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Cleanup, "Preparing original preview…");
            cx.notify();
        });
        self.original_job = Some(id);
    }

    fn cleanup_args(&self) -> Option<Value> {
        self.path.as_ref().map(|path| {
            json!({
                "src": path.to_string_lossy(), "deskew": self.deskew, "denoise": self.denoise,
                "threshold": self.threshold, "autocrop": self.autocrop,
                "out_dir": self.output_dir.as_ref().map(|dir| dir.to_string_lossy().to_string()),
            })
        })
    }

    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.preview_job.take() {
            self.engine.cancel(&id);
        }
        self.job = None;
        let Some(mut args) = self.cleanup_args() else {
            return;
        };
        self.revision += 1;
        // Each settings revision gets its own image-cache key.
        args["dst"] = json!(
            self.cache_dir
                .join(format!("cleaned-{}.png", self.revision))
                .to_string_lossy()
        );
        let id = self.engine.call("cleanup.enhance", args);
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Cleanup, "Updating cleaned preview…");
            cx.notify();
        });
        self.preview_job = Some(id);
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let Some(args) = self.cleanup_args() else {
            return;
        };
        self.show_cleaned = true;
        let id = self.engine.call("cleanup.enhance", args);
        self.jobs.update(cx, |jobs, _| {
            jobs.start_export(id.clone(), Tool::Cleanup, "Saving cleaned copy…");
        });
        self.job = Some(id);
        cx.notify();
    }
}

impl Drop for CleanupView {
    fn drop(&mut self) {
        for id in [&self.original_job, &self.preview_job]
            .into_iter()
            .flatten()
        {
            self.engine.cancel(id);
        }
        let _ = std::fs::remove_dir_all(&self.cache_dir);
    }
}

impl Render for CleanupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let job: Option<Job> = self.jobs.read(cx).get(&self.job).cloned();
        let busy = job.as_ref().map(Job::is_running).unwrap_or(false);
        let result = job.as_ref().and_then(|j| j.result()).cloned();
        let preview_job = self.jobs.read(cx).get(&self.preview_job).cloned();
        let original_job = self.jobs.read(cx).get(&self.original_job).cloned();
        let cleaned = preview_job.as_ref().and_then(Job::result);
        let displayed_job = if self.show_cleaned {
            preview_job.as_ref()
        } else {
            original_job.as_ref()
        };
        let preview_path = displayed_job
            .and_then(Job::result)
            .and_then(|value| value.get("dst"))
            .and_then(Value::as_str)
            .map(PathBuf::from);

        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "No image chosen".into());
        let size = self
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);

        let mut body = card(cx)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(field_label("Scan", cx))
                    .child(
                        Button::new("cleanup-pick")
                            .label("Choose image…")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(name),
                            )
                            .child(note(
                                if size > 0 {
                                    human_bytes(size)
                                } else {
                                    "Choose a photo of a document.".into()
                                },
                                cx,
                            )),
                    )
                    .child(muted("Writes .clean.png beside the original", cx)),
            );

        if self.path.is_none() {
            body = body.child(
                dropzone(
                    "Drop a scan here",
                    "or choose an image · phone photos work well",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
            );
        }

        body = body.child(field_label("Steps", cx)).child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(self.toggle("Deskew", ToggleKind::Deskew, cx))
                .child(self.toggle("Denoise", ToggleKind::Denoise, cx))
                .child(self.toggle("Threshold", ToggleKind::Threshold, cx))
                .child(self.toggle("Autocrop", ToggleKind::Autocrop, cx)),
        );

        if self.path.is_some() {
            let mut comparison = super::segmented(cx).flex_wrap();
            for (id, label, show_cleaned) in [
                ("cleanup-original", "Original", false),
                ("cleanup-preview", "Cleaned preview", true),
            ] {
                let button = super::segment(id, label, self.show_cleaned == show_cleaned);
                comparison =
                    comparison.child(button.on_click(cx.listener(move |this, _, _, cx| {
                        this.show_cleaned = show_cleaned;
                        cx.notify();
                    })));
            }
            body = body.child(comparison);
            let height = (f32::from(window.viewport_size().height) - 500.).clamp(220., 560.);
            let mut frame = div()
                .w_full()
                .h(px(height))
                .min_w_0()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .bg(cx.theme().muted.opacity(0.2));
            if let Some(path) = preview_path {
                frame = frame.child(img(path).w_full().h_full().object_fit(ObjectFit::Contain));
            } else {
                frame = frame.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w_full()
                        .max_w(px(480.))
                        .p_5()
                        .child(muted(
                            if displayed_job.and_then(Job::error).is_some() {
                                "Preview unavailable"
                            } else if self.show_cleaned {
                                "Updating cleaned preview…"
                            } else {
                                "Preparing original preview…"
                            },
                            cx,
                        ))
                        .child(job_progress(displayed_job, cx)),
                );
            }
            body = body.child(frame);
            if displayed_job.and_then(Job::error).is_some() {
                body = body.child(
                    Button::new("cleanup-preview-retry")
                        .label("Retry preview")
                        .small()
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.show_cleaned {
                                this.refresh_preview(cx);
                            } else {
                                this.load_original_preview(cx);
                            }
                            cx.notify();
                        })),
                );
            }
            body = body.child(note("Preview updates with your cleanup options. Choose Original to compare; save when you're happy with the result.", cx));
        }

        body = body
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(note(
                        "For signatures: turn off threshold to keep the ink antialiased.",
                        cx,
                    ))
                    .child(
                        Button::new("cleanup-run")
                            .label(if busy {
                                "Saving…"
                            } else {
                                "Save cleaned copy"
                            })
                            .primary()
                            .disabled(busy || cleaned.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .child(job_progress(job.as_ref(), cx));

        if let Some(result) = result {
            let dst = result
                .get("dst")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let angle = result.get("skew").and_then(Value::as_f64).unwrap_or(0.0);
            let bytes = result.get("bytes").and_then(Value::as_u64).unwrap_or(0);
            let width = result.get("width").and_then(Value::as_u64).unwrap_or(0);
            let height = result.get("height").and_then(Value::as_u64).unwrap_or(0);
            let opened = dst.clone();
            body = body.child(
                result_section(cx)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_8()
                            .child(stat("Angle", format!("{angle:.2}°"), cx))
                            .child(stat("Output", format!("{width} × {height}"), cx))
                            .child(stat("Size", human_bytes(bytes), cx)),
                    )
                    .child(file_row(&dst, "written", cx))
                    .child(save_button(
                        "cleanup-save",
                        Some(dst.clone()),
                        self.job.clone(),
                        self.jobs.clone(),
                        self.output_dir.clone(),
                        cx,
                    ))
                    .child(
                        Button::new("cleanup-open")
                            .label("Open result")
                            .small()
                            .secondary()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let _ = &this;
                                cx.open_with_system(std::path::Path::new(&opened));
                            })),
                    ),
            );
            body = body.child(success_box("Done. The original is unchanged.", cx));
            if let Some(note) = saved_note(self.jobs.read(cx).saved_for(&self.job), cx) {
                body = body.child(note);
            }
        }

        page(window)
            .child(heading(
                "Cleanup",
                "Straighten a phone photo of a document, kill the noise, and trim the margins. Signature images stay antialiased if you leave threshold off.",
                cx,
            ))
            .child(body)
    }
}

#[derive(Clone, Copy)]
enum ToggleKind {
    Deskew,
    Denoise,
    Threshold,
    Autocrop,
}

impl CleanupView {
    fn toggle(&self, label: &'static str, kind: ToggleKind, cx: &mut Context<Self>) -> Div {
        let checked = match kind {
            ToggleKind::Deskew => self.deskew,
            ToggleKind::Denoise => self.denoise,
            ToggleKind::Threshold => self.threshold,
            ToggleKind::Autocrop => self.autocrop,
        };
        let description = match kind {
            ToggleKind::Deskew => "Straighten tilted text and page edges.",
            ToggleKind::Denoise => "Remove speckles from scanned pages.",
            ToggleKind::Threshold => "Convert to black and white. Turn off for signatures.",
            ToggleKind::Autocrop => "Trim empty margins around the document.",
        };
        let control = Switch::new(label)
            .checked(checked)
            .disabled(
                self.jobs
                    .read(cx)
                    .get(&self.job)
                    .is_some_and(Job::is_running),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                match kind {
                    ToggleKind::Deskew => this.deskew = !this.deskew,
                    ToggleKind::Denoise => this.denoise = !this.denoise,
                    ToggleKind::Threshold => this.threshold = !this.threshold,
                    ToggleKind::Autocrop => this.autocrop = !this.autocrop,
                }
                this.show_cleaned = true;
                this.refresh_preview(cx);
                cx.notify();
            }));
        super::setting_row(label, description, control, cx)
    }
}
