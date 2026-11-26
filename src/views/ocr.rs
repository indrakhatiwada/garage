use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, field_label, file_row, heading, human_bytes, job_progress, muted, note, page,
    panel_width, pill, result_section, save_button, saved_note, stat, success_box,
};
use crate::bs::{self, DateStyle};
use crate::engine::Engine;
use crate::jobs::{Job, Jobs, Tool};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Dates,
    SearchablePdf,
}

impl Mode {
    fn id(self) -> &'static str {
        match self {
            Mode::Text => "ocr-text",
            Mode::Dates => "ocr-dates",
            Mode::SearchablePdf => "ocr-pdf",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Text => "Extract text",
            Mode::Dates => "Find dates",
            Mode::SearchablePdf => "Searchable PDF",
        }
    }
}

pub struct OcrView {
    engine: Engine,
    jobs: Entity<Jobs>,
    path: Option<PathBuf>,
    mode: Mode,
    job: Option<String>,
    text: Option<String>,
    copied: bool,
    preview_job: Option<String>,
    preview_page: u64,
    preview_revision: u64,
    cache_dir: PathBuf,
    output_dir: Option<PathBuf>,
    _observe: Subscription,
}

impl OcrView {
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
            mode: Mode::Text,
            job: None,
            text: None,
            copied: false,
            preview_job: None,
            preview_page: 1,
            preview_revision: 0,
            cache_dir: std::env::temp_dir().join(format!(
                "garage-ocr-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )),
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
            prompt: Some("Choose a scan or PDF".into()),
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
        if let Some(path) = paths.into_iter().next() {
            self.load(path, cx);
        }
    }

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(id) = self.job.take() {
            self.engine.cancel(&id);
        }
        self.path = Some(path);
        self.text = None;
        self.copied = false;
        self.preview_page = 1;
        self.load_preview(cx);
    }

    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(id) = self.preview_job.take() {
            self.engine.cancel(&id);
        }
        self.preview_revision += 1;
        let dst = self
            .cache_dir
            .join(format!("{}.png", self.preview_revision));
        let is_pdf = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
        let id = self.engine.call(
            if is_pdf {
                "pdf.render"
            } else {
                "image.preview"
            },
            json!({
                "src": path.to_string_lossy(),
                "dst": dst.to_string_lossy(),
                "page": self.preview_page,
                "dpi": 120,
            }),
        );
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Ocr, "Preparing document preview…");
            cx.notify();
        });
        self.preview_job = Some(id);
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let src = path.to_string_lossy().to_string();
        let (op, args) = match self.mode {
            Mode::Text => ("ocr.text", json!({ "src": src })),
            Mode::Dates => ("ocr.scan_dates", json!({ "src": src, "dpi": 300 })),
            Mode::SearchablePdf => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "document".into());
                let dir = self.output_dir.clone().unwrap_or_else(|| {
                    path.parent()
                        .unwrap_or(std::path::Path::new("."))
                        .to_path_buf()
                });
                let dst = dir.join(format!("{stem}.ocr.pdf"));
                (
                    "ocr.searchable_pdf",
                    json!({ "src": src, "dst": dst.to_string_lossy().to_string() }),
                )
            }
        };
        let id = self.engine.call(op, args);
        self.jobs.update(cx, |jobs, _| {
            jobs.start(id.clone(), Tool::Ocr, "starting tesseract");
        });
        self.job = Some(id);
        self.text = None;
        self.copied = false;
        cx.notify();
    }
}

impl Render for OcrView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let job: Option<Job> = self.jobs.read(cx).get(&self.job).cloned();
        let busy = job.as_ref().map(Job::is_running).unwrap_or(false);
        let result = job.as_ref().and_then(|j| j.result()).cloned();
        let preview_job = self.jobs.read(cx).get(&self.preview_job).cloned();
        let preview = preview_job.as_ref().and_then(Job::result);
        let preview_path = preview
            .and_then(|value| value.get("dst"))
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let pages = preview
            .and_then(|value| value.get("pages"))
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let loading_preview = preview_job.as_ref().is_some_and(Job::is_running);

        if let Some(result) = &result
            && let Some(text) = result.get("text").and_then(Value::as_str)
            && self.text.as_deref() != Some(text)
        {
            self.text = Some(text.to_string());
        }

        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "No file chosen".into());

        let mut modes = div().flex().flex_wrap().gap_2().items_center();
        for mode in [Mode::Text, Mode::Dates, Mode::SearchablePdf] {
            let selected = self.mode == mode;
            let button = Button::new(mode.id())
                .label(mode.label())
                .small()
                .disabled(busy);
            let button = if selected {
                button.primary()
            } else {
                button.secondary()
            };
            modes = modes.child(button.on_click(cx.listener(move |this, _, _, cx| {
                if this.mode == mode {
                    return;
                }
                this.mode = mode;
                this.job = None;
                this.text = None;
                this.copied = false;
                cx.notify();
            })));
        }

        let mut body = card(cx)
            .child(
                div()
                    .flex().flex_wrap().gap_3()
                    .justify_between()
                    .items_center()
                    .min_w_0()
                    .child(
                        div().flex_1().min_w_0().truncate().text_sm()
                            .font_weight(FontWeight::MEDIUM).child(name),
                    )
                    .child(
                        Button::new("ocr-pick")
                            .label("Choose file…")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(match self.mode {
                        Mode::Text => note(
                            "Reads the visible text. PDFs with a real text layer return that instead of OCR.",
                            cx,
                        ),
                        Mode::Dates => note(
                            "Looks for numeric date triples, reads Devanagari digits, and offers both calendar readings when the year is ambiguous.",
                            cx,
                        ),
                        Mode::SearchablePdf => note(
                            "Keeps your scan exactly as it is and adds an invisible, selectable text layer.",
                            cx,
                        ),
                    }),
            );

        if self.path.is_none() {
            body = body.child(
                dropzone(
                    "Drop a scan or PDF here",
                    "or choose a file · OCR runs entirely on this machine",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
            );
        }

        body = body
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(modes)
                    .child(
                        Button::new("ocr-run")
                            .label(if busy { "Reading…" } else { "Run OCR" })
                            .primary()
                            .disabled(busy || self.path.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .when(busy || job.as_ref().and_then(Job::error).is_some(), |el| {
                el.child(job_progress(job.as_ref(), cx))
            });

        let side_by_side = panel_width(window) >= 640.;
        let pane_height = (f32::from(window.viewport_size().height) - 440.).clamp(280., 680.);
        let pane = || div().flex().flex_col().flex_1().min_w_0().gap_3();
        let mut output = pane();
        let mut output_header = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .justify_between()
            .min_h(px(28.))
            .child(field_label(
                match self.mode {
                    Mode::Text => "Extracted text",
                    Mode::Dates => "Dates found",
                    Mode::SearchablePdf => "Searchable PDF",
                },
                cx,
            ));
        if self.mode == Mode::Text {
            output_header = output_header.child(
                Button::new("ocr-copy")
                    .label(if self.copied { "Copied" } else { "Copy all" })
                    .icon(if self.copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .small()
                    .secondary()
                    .disabled(busy || self.text.as_ref().is_none_or(|text| text.is_empty()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(text) = this.text.clone() {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            this.copied = true;
                            cx.notify();
                        }
                    })),
            );
        }
        output = output.child(output_header);

        if self.mode == Mode::Text {
            let mut text_area = div()
                .id("ocr-text-output")
                .h(px(pane_height))
                .w_full()
                .min_w_0()
                .overflow_y_scroll()
                .overflow_x_scroll()
                .p_4()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .text_sm()
                .line_height(relative(1.6));
            text_area = if let Some(text) = &self.text {
                if text.is_empty() {
                    text_area.child(muted("No text recognized. Try a clearer scan.", cx))
                } else {
                    text_area.child(text.clone())
                }
            } else {
                text_area
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(muted(
                        if busy {
                            "Reading your document…"
                        } else if job.as_ref().and_then(Job::error).is_some() {
                            "Text extraction failed. Try running OCR again."
                        } else {
                            "Run OCR to see the extracted text here."
                        },
                        cx,
                    ))
            };
            output = output.child(text_area);
            if let Some(text) = &self.text {
                output = output.child(note(
                    format!("{} characters · All pages", text.chars().count()),
                    cx,
                ));
            }
        } else if result.is_none() {
            output = output.child(
                div()
                    .h(px(pane_height))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .rounded_md()
                    .bg(cx.theme().muted.opacity(0.2))
                    .child(muted(
                        if busy {
                            "Reading your document…"
                        } else {
                            match self.mode {
                                Mode::Dates => "Run OCR to find dates in this document.",
                                _ => "Run OCR to create a searchable PDF.",
                            }
                        },
                        cx,
                    )),
            );
        }

        if let Some(result) = &result {
            if let Some(dates) = result.get("dates").and_then(Value::as_array) {
                output = output.child(
                    result_section(cx)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(field_label("Dates found", cx))
                                .child(pill(
                                    format!("{}", dates.len()),
                                    if dates.is_empty() {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().success
                                    },
                                    cx,
                                )),
                        )
                        .child(if dates.is_empty() {
                            muted("No numeric dates were recognized.", cx).into_any_element()
                        } else {
                            div()
                                .id("ocr-dates-output")
                                .max_h(px(pane_height))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(dates.iter().take(200).map(|date| date_row(date, cx)))
                                .into_any_element()
                        }),
                );
            }

            if let Some(dst) = result.get("dst").and_then(Value::as_str) {
                let dst = dst.to_string();
                let bytes = result.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                let words = result.get("words").and_then(Value::as_u64).unwrap_or(0);
                output = output.child(
                    result_section(cx)
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_8()
                                .child(stat("Words placed", words.to_string(), cx))
                                .child(stat("Size", human_bytes(bytes), cx)),
                        )
                        .child(file_row(&dst, "written", cx))
                        .child(save_button(
                            "ocr-save",
                            Some(dst.clone()),
                            self.job.clone(),
                            self.jobs.clone(),
                            self.output_dir.clone(),
                            cx,
                        ))
                        .child(
                            Button::new("ocr-open")
                                .label("Open result")
                                .small()
                                .secondary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let _ = &this;
                                    cx.open_with_system(std::path::Path::new(&dst));
                                })),
                        ),
                );
            }

            if self.mode != Mode::Text {
                output = output.child(success_box("Done.", cx));
            }
            if let Some(note) = saved_note(self.jobs.read(cx).saved_for(&self.job), cx) {
                output = output.child(note);
            }
        }

        let mut root = page(window)
            .child(heading(
                "OCR",
                "Tesseract runs locally. Nepali is used when the language pack is installed, English otherwise.",
                cx,
            ))
            .child(body);

        if self.path.is_some() {
            let preview_header = div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .min_h(px(28.))
                .child(field_label("Document preview", cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("ocr-preview-prev")
                                .child(Icon::new(IconName::ChevronLeft).size(px(16.)))
                                .tooltip("Previous page")
                                .small()
                                .ghost()
                                .disabled(loading_preview || self.preview_page <= 1)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.preview_page = this.preview_page.saturating_sub(1).max(1);
                                    this.load_preview(cx);
                                })),
                        )
                        .child(note(
                            format!(
                                "Page {} / {}",
                                self.preview_page,
                                pages.max(self.preview_page)
                            ),
                            cx,
                        ))
                        .child(
                            Button::new("ocr-preview-next")
                                .child(Icon::new(IconName::ChevronRight).size(px(16.)))
                                .tooltip("Next page")
                                .small()
                                .ghost()
                                .disabled(loading_preview || self.preview_page >= pages)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.preview_page = (this.preview_page + 1).min(pages);
                                    this.load_preview(cx);
                                })),
                        ),
                );
            let mut frame = div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(pane_height))
                .flex_shrink_0()
                .min_w_0()
                .rounded_md()
                .overflow_hidden()
                .bg(cx.theme().muted.opacity(0.3));
            if let Some(path) = preview_path {
                frame = frame.child(img(path).w_full().h_full().object_fit(ObjectFit::Contain));
            } else {
                let failed = preview_job.as_ref().and_then(Job::error).is_some();
                let mut state = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .w_full()
                    .child(muted(
                        if failed {
                            "Preview unavailable"
                        } else {
                            "Preparing document preview…"
                        },
                        cx,
                    ))
                    .child(job_progress(preview_job.as_ref(), cx));
                if failed {
                    state = state.child(
                        Button::new("ocr-preview-retry")
                            .label("Retry preview")
                            .small()
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| this.load_preview(cx))),
                    );
                }
                frame = frame.child(state);
            }
            root = root.child(
                card(cx).child(
                    div()
                        .flex()
                        .gap_5()
                        .min_w_0()
                        .when(!side_by_side, |el| el.flex_col())
                        .child(pane().child(preview_header).child(frame))
                        .child(output),
                ),
            );
        }
        root
    }
}

fn date_row(date: &Value, cx: &App) -> Div {
    let page = date.get("page").and_then(Value::as_u64).unwrap_or(1);
    let raw = date
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let system = date.get("system").and_then(Value::as_str).unwrap_or("AD");
    let year = date.get("year").and_then(Value::as_i64).unwrap_or(0) as i32;
    let month = date.get("month").and_then(Value::as_i64).unwrap_or(1) as u32;
    let day = date.get("day").and_then(Value::as_i64).unwrap_or(1) as u32;
    let conf = date.get("conf").and_then(Value::as_f64).unwrap_or(0.0);

    // Resolve which calendar the triple is, and show the other side.
    let readings = match system {
        "AD" => {
            let ad = chrono::NaiveDate::from_ymd_opt(year, month, day);
            let bs = ad.and_then(|d| bs::from_ad(d).ok());
            (ad.map(|d| d.format("%Y-%m-%d").to_string()), bs)
        }
        _ => {
            let bs_date = bs::to_ad(year, month, day).ok();
            (
                bs_date.map(|d| d.format("%Y-%m-%d").to_string()),
                Some(bs::BsDate { year, month, day }),
            )
        }
    };

    let primary = match system {
        "AD" => format!("AD {year}-{month:02}-{day:02}"),
        "ambiguous" => format!("{year}-{month:02}-{day:02} (ambiguous)"),
        _ => format!("BS {year}-{month:02}-{day:02}"),
    };

    let ad_text = readings.0.clone().unwrap_or_else(|| "invalid".to_string());
    let bs_text = readings
        .1
        .map(|b| b.format(DateStyle::Long))
        .unwrap_or_else(|| "invalid".to_string());

    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(cx.theme().muted.opacity(0.3))
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
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(primary),
                        )
                        .child(pill(
                            format!("page {page}"),
                            cx.theme().muted_foreground,
                            cx,
                        )),
                )
                .child(note(format!("conf {conf:.0} · raw {raw}"), cx)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_4()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Gregorian {ad_text}"))
                .child(format!("Bikram Sambat {bs_text}")),
        )
}
