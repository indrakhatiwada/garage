use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
    switch::Switch,
};
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, error_box, field_label, file_row, heading, human_bytes, job_progress, muted,
    note, page, pill, result_section, save_button, saved_note, stat, success_box,
};
use crate::engine::Engine;
use crate::jobs::{Job, Jobs, Tool};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Preset {
    Screen,
    Balanced,
    Print,
}

impl Preset {
    fn id(self) -> &'static str {
        match self {
            Preset::Screen => "pdf-screen",
            Preset::Balanced => "pdf-balanced",
            Preset::Print => "pdf-print",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Preset::Screen => "Screen · 120dpi",
            Preset::Balanced => "Balanced · 150dpi",
            Preset::Print => "Print · 300dpi",
        }
    }

    fn dpi(self) -> u32 {
        match self {
            Preset::Screen => 120,
            Preset::Balanced => 150,
            Preset::Print => 300,
        }
    }

    fn quality(self) -> u32 {
        match self {
            Preset::Screen => 60,
            Preset::Balanced => 78,
            Preset::Print => 88,
        }
    }
}

pub struct PdfView {
    engine: Engine,
    jobs: Entity<Jobs>,
    path: Option<PathBuf>,
    info: Option<Value>,
    preset: Preset,
    grayscale: bool,
    ghostscript: bool,
    subset_fonts: bool,
    job: Option<String>,
    info_job: Option<String>,
    preview_page: u32,
    preview_job: Option<String>,
    preview_path: Option<PathBuf>,
    preview_dims: Option<(u32, u32)>,
    output_dir: Option<PathBuf>,
    _observe: Subscription,
}

impl PdfView {
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
            info: None,
            preset: Preset::Balanced,
            grayscale: false,
            ghostscript: true,
            subset_fonts: true,
            job: None,
            info_job: None,
            preview_page: 1,
            preview_job: None,
            preview_path: None,
            preview_dims: None,
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
            prompt: Some("Choose a PDF".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |view, cx| view.load(path, cx));
            }
        })
        .detach();
    }

    pub(crate) fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let pdf = paths.into_iter().find(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false)
        });
        if let Some(path) = pdf {
            self.load(path, cx);
        }
    }

    fn render_preview(&mut self, path: &Path, page: u32, cx: &mut Context<Self>) {
        let dst =
            std::env::temp_dir().join(format!("garage-preview-{}-{page}.png", std::process::id()));
        let id = self.engine.call(
            "pdf.render",
            json!({
                "src": path.to_string_lossy().to_string(),
                "dst": dst.to_string_lossy().to_string(),
                "page": page,
                "dpi": 110,
            }),
        );
        self.jobs.update(cx, |jobs, _| {
            jobs.start(id.clone(), Tool::Pdf, "rendering page");
        });
        self.preview_job = Some(id);
        self.preview_page = page;
        self.preview_path = None;
        cx.notify();
    }

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = Some(path.clone());
        self.info = None;
        let id = self.engine.call(
            "pdf.info",
            json!({ "path": path.to_string_lossy().to_string() }),
        );
        self.jobs.update(cx, |jobs, _| {
            jobs.start(id.clone(), Tool::Pdf, "reading");
        });
        self.info_job = Some(id);
        cx.notify();
    }

    pub fn output_dir(&self) -> Option<PathBuf> {
        self.output_dir.clone()
    }

    fn compress(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let id = self.engine.call(
            "pdf.compress",
            json!({
                "src": path.to_string_lossy().to_string(),
                "dpi": self.preset.dpi(),
                "quality": self.preset.quality(),
                "grayscale": self.grayscale,
                "subset_fonts": self.subset_fonts,
                "ghostscript": if self.ghostscript { "auto" } else { "off" },
                "out_dir": self.output_dir.as_ref().map(|dir| dir.to_string_lossy().to_string()),
            }),
        );
        self.jobs.update(cx, |jobs, _| {
            jobs.start(id.clone(), Tool::Pdf, "opening");
        });
        self.job = Some(id);
        cx.notify();
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let info_job: Option<Job> = self.jobs.read(cx).get(&self.info_job).cloned();
        let job: Option<Job> = self.jobs.read(cx).get(&self.job).cloned();
        let preview_job: Option<Job> = self.jobs.read(cx).get(&self.preview_job).cloned();
        let info = info_job
            .as_ref()
            .and_then(|j| j.result())
            .cloned()
            .or_else(|| self.info.clone());
        if info.is_some() {
            self.info = info.clone();
        }
        let busy = job.as_ref().map(Job::is_running).unwrap_or(false);
        let result = job.as_ref().and_then(|j| j.result()).cloned();

        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "No file chosen".into());

        let pages = info
            .as_ref()
            .and_then(|i| i.get("pages"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let bytes = info
            .as_ref()
            .and_then(|i| i.get("bytes"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let image_count = info
            .as_ref()
            .and_then(|i| i.get("images"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let image_bytes = info
            .as_ref()
            .and_then(|i| i.get("image_bytes"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let has_text = info
            .as_ref()
            .and_then(|i| i.get("has_text"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let encrypted = info
            .as_ref()
            .and_then(|i| i.get("encrypted"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut presets = div().flex().flex_wrap().gap_2().items_center();
        for preset in [Preset::Screen, Preset::Balanced, Preset::Print] {
            let selected = self.preset == preset;
            let button = Button::new(preset.id()).label(preset.label()).small();
            let button = if selected {
                button.primary()
            } else {
                button.secondary()
            };
            presets = presets.child(button.on_click(cx.listener(move |this, _, _, cx| {
                this.preset = preset;
                cx.notify();
            })));
        }

        let mut body = card(cx)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(field_label("Document", cx))
                    .child(
                        Button::new("pdf-pick")
                            .label("Choose PDF…")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_lg().font_weight(FontWeight::MEDIUM).child(name))
                    .child(if let Some(path) = &self.path {
                        tiny_path(path, cx)
                    } else {
                        note(
                            "Text PDFs barely shrink. Scans and image-heavy files shrink a lot.",
                            cx,
                        )
                    }),
            );

        if self.path.is_none() {
            body = body.child(
                dropzone(
                    "Drop a PDF here",
                    "or use Choose PDF… · scans shrink the most",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
            );
        }

        if self.path.is_some() {
            body = body.child(job_progress(info_job.as_ref(), cx));
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_8()
                    .child(stat("Pages", pages.to_string(), cx))
                    .child(stat("Size", human_bytes(bytes), cx))
                    .child(stat("Images", image_count.to_string(), cx))
                    .child(stat("Image data", human_bytes(image_bytes), cx))
                    .child(if has_text {
                        pill("Has a text layer", cx.theme().info, cx)
                    } else {
                        pill("Image only — good candidate", cx.theme().success, cx)
                    }),
            );

            if encrypted {
                body = body.child(error_box(
                    "This PDF is password protected. Unlock it in PDF pages first.",
                    cx,
                ));
            }

            if has_text {
                body = body.child(note(
                    "Text and vectors are already compressed. Expect single-digit gains unless the pages contain large photos.",
                    cx,
                ));
            }
        }

        body = body
            .child(field_label("Preset", cx))
            .child(presets)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .items_center()
                    .child(toggle("Grayscale", self.grayscale, cx, |this, cx| {
                        this.grayscale = !this.grayscale;
                        cx.notify();
                    }))
                    .child(toggle(
                        "Font subsetting",
                        self.subset_fonts,
                        cx,
                        |this, cx| {
                            this.subset_fonts = !this.subset_fonts;
                            cx.notify();
                        },
                    ))
                    .child(toggle(
                        "Ghostscript pass",
                        self.ghostscript,
                        cx,
                        |this, cx| {
                            this.ghostscript = !this.ghostscript;
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(note(
                        "Writes a .min.pdf next to the original. The original is never touched.",
                        cx,
                    ))
                    .child(
                        Button::new("pdf-run")
                            .label(if busy {
                                "Compressing…"
                            } else {
                                "Compress PDF"
                            })
                            .primary()
                            .disabled(busy || self.path.is_none() || encrypted)
                            .on_click(cx.listener(|this, _, _, cx| this.compress(cx))),
                    ),
            )
            .child(job_progress(job.as_ref(), cx));

        if let Some(preview) = preview_job.as_ref().and_then(|j| j.result()) {
            let dst = preview
                .get("dst")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let w = preview.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
            let h = preview.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
            if !dst.is_empty() && w > 0 && h > 0 {
                self.preview_path = Some(PathBuf::from(dst));
                self.preview_dims = Some((w, h));
            }
        }

        if let Some(result) = result {
            let before = result.get("before").and_then(Value::as_u64).unwrap_or(0);
            let after = result.get("after").and_then(Value::as_u64).unwrap_or(0);
            let saved = result.get("saved").and_then(Value::as_f64).unwrap_or(0.0);
            body = body.child(
                result_section(cx)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_8()
                            .child(stat("Before", human_bytes(before), cx))
                            .child(stat("After", human_bytes(after), cx))
                            .child(stat("Saved", format!("{:.0}%", saved * 100.0), cx))
                            .child(stat(
                                "Images rewritten",
                                result
                                    .get("images")
                                    .and_then(Value::as_array)
                                    .map(Vec::len)
                                    .unwrap_or(0)
                                    .to_string(),
                                cx,
                            )),
                    )
                    .child(
                        div().flex().flex_col().gap_1().children(
                            result
                                .get("images")
                                .and_then(Value::as_array)
                                .map(|images| {
                                    images
                                        .iter()
                                        .take(40)
                                        .map(|image| {
                                            let page = image
                                                .get("page")
                                                .and_then(Value::as_u64)
                                                .unwrap_or(1);
                                            let from = image
                                                .get("from")
                                                .and_then(Value::as_str)
                                                .unwrap_or("?");
                                            let to = image
                                                .get("to")
                                                .and_then(Value::as_str)
                                                .unwrap_or("?");
                                            let b = image
                                                .get("before")
                                                .and_then(Value::as_u64)
                                                .unwrap_or(0);
                                            let a = image
                                                .get("after")
                                                .and_then(Value::as_u64)
                                                .unwrap_or(0);
                                            file_row(
                                                &format!("page {page} · {from} → {to}"),
                                                format!("{} → {}", human_bytes(b), human_bytes(a)),
                                                cx,
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default(),
                        ),
                    )
                    .child(
                        div().flex().flex_col().gap_1().children(
                            result
                                .get("notes")
                                .and_then(Value::as_array)
                                .map(|notes| {
                                    notes
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .map(|n| note(n.to_string(), cx))
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default(),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .items_center()
                            .child(save_button(
                                "pdf-save",
                                result
                                    .get("dst")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                self.job.clone(),
                                self.jobs.clone(),
                                self.output_dir.clone(),
                                cx,
                            ))
                            .child(save_button(
                                "pdf-save",
                                result
                                    .get("dst")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                self.job.clone(),
                                self.jobs.clone(),
                                self.output_dir.clone(),
                                cx,
                            ))
                            .child(
                                Button::new("pdf-open")
                                    .label("Open result")
                                    .small()
                                    .secondary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(dst) = this
                                            .jobs
                                            .read(cx)
                                            .get(&this.job)
                                            .and_then(|j| j.result())
                                            .and_then(|r| r.get("dst"))
                                            .and_then(Value::as_str)
                                            .map(str::to_string)
                                        {
                                            cx.open_with_system(std::path::Path::new(&dst));
                                        }
                                    })),
                            )
                            .child(
                                Button::new("pdf-reveal")
                                    .label("Show in folder")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(dst) = this
                                            .jobs
                                            .read(cx)
                                            .get(&this.job)
                                            .and_then(|j| j.result())
                                            .and_then(|r| r.get("dst"))
                                            .and_then(Value::as_str)
                                            .map(str::to_string)
                                        {
                                            let path = std::path::PathBuf::from(dst);
                                            if let Some(parent) = path.parent() {
                                                cx.reveal_path(parent);
                                            }
                                        }
                                    })),
                            ),
                    ),
            );
            body = body.child(success_box("Done. The original is unchanged.", cx));
            if let Some(note) = saved_note(self.jobs.read(cx).saved_for(&self.job), cx) {
                body = body.child(note);
            }
        }

        let preview = if let (Some(path), Some((w, h))) = (&self.preview_path, self.preview_dims) {
            let width = 460.0f32;
            let _height = width * h as f32 / w as f32;
            let current = self.preview_page;
            let total = pages.max(1) as u32;
            let source = self.path.clone();
            Some(
                card(cx)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .justify_between()
                            .items_center()
                            .child(field_label("Page preview", cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new("preview-prev")
                                            .child(Icon::new(IconName::ChevronLeft).size(px(16.)))
                                            .tooltip("Previous page")
                                            .small()
                                            .ghost()
                                            .disabled(current <= 1)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(path) = this.path.clone() {
                                                    let page =
                                                        this.preview_page.saturating_sub(1).max(1);
                                                    this.render_preview(&path, page, cx);
                                                }
                                            })),
                                    )
                                    .child(note(format!("{current} / {total}"), cx))
                                    .child(
                                        Button::new("preview-next")
                                            .child(Icon::new(IconName::ChevronRight).size(px(16.)))
                                            .tooltip("Next page")
                                            .small()
                                            .ghost()
                                            .disabled(current >= total)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(path) = this.path.clone() {
                                                    let page = (this.preview_page + 1).min(total);
                                                    this.render_preview(&path, page, cx);
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(360.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(cx.theme().muted.opacity(0.2))
                            .child(
                                img(path.clone())
                                    .w_full()
                                    .max_h(px(350.))
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .child(note(
                        format!(
                            "{} · rendered locally from {}",
                            source
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            if self.preview_dims.is_some() {
                                "the PDF"
                            } else {
                                "cache"
                            }
                        ),
                        cx,
                    )),
            )
        } else {
            None
        };

        let mut root = page(window)
            .child(heading(
                "Compress PDF",
                "Downsample the images inside the PDF, clean the object graph, and keep the text as text.",
                cx,
            ))
            .child(body);
        if let Some(preview) = preview {
            root = root.child(preview);
        }
        root.child(muted("No upload, no queue, no account.", cx))
    }
}

fn tiny_path(path: &Path, cx: &App) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(path.to_string_lossy().to_string())
}

fn toggle(
    label: &'static str,
    checked: bool,
    cx: &mut Context<PdfView>,
    handler: impl Fn(&mut PdfView, &mut Context<PdfView>) + 'static,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .items_center()
        .child(muted(label, cx))
        .child(
            Switch::new(label)
                .checked(checked)
                .on_click(cx.listener(move |this, _, _, cx| handler(this, cx))),
        )
}
