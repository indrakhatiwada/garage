use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, field_label, file_row, heading, human_bytes, job_progress, muted, note, page,
    pill, result_section, save_button, saved_note, stat, success_box,
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
    _observe: Subscription,
}

impl OcrView {
    pub fn new(engine: Engine, jobs: Entity<Jobs>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&jobs, |_, _, cx| cx.notify());
        Self {
            engine,
            jobs,
            path: None,
            mode: Mode::Text,
            job: None,
            text: None,
            copied: false,
            _observe: observe,
        }
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
                let _ = this.update(cx, |view, cx| {
                    view.path = Some(path);
                    view.text = None;
                    view.copied = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if let Some(path) = paths.into_iter().next() {
            self.path = Some(path);
            self.text = None;
            self.copied = false;
            cx.notify();
        }
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
                let dst = path.with_file_name(format!(
                    "{}.ocr.pdf",
                    path.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "document".into())
                ));
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
            let button = Button::new(mode.id()).label(mode.label()).small();
            let button = if selected {
                button.primary()
            } else {
                button.secondary()
            };
            modes = modes.child(button.on_click(cx.listener(move |this, _, _, cx| {
                this.mode = mode;
                this.text = None;
                cx.notify();
            })));
        }

        let mut body = card(cx)
            .child(
                div()
                    .flex().flex_wrap().gap_3()
                    .justify_between()
                    .items_center()
                    .child(field_label("Source", cx))
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
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
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
            .child(field_label("Action", cx))
            .child(modes)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(note(
                        "Runs on this machine. The first page may take a few seconds.",
                        cx,
                    ))
                    .child(
                        Button::new("ocr-run")
                            .label(if busy { "Reading…" } else { "Run OCR" })
                            .primary()
                            .disabled(busy || self.path.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .child(job_progress(job.as_ref(), cx));

        if let Some(result) = &result {
            if let Some(dates) = result.get("dates").and_then(Value::as_array) {
                body = body.child(
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
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(dates.iter().take(200).map(|date| date_row(date, cx)))
                                .into_any_element()
                        }),
                );
            }

            if let Some(text) = &self.text {
                let chars = text.chars().count();
                body = body.child(
                    result_section(cx)
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_3()
                                .justify_between()
                                .items_center()
                                .child(field_label("Text", cx))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .items_center()
                                        .child(note(
                                            format!(
                                                "{} characters · {}",
                                                chars,
                                                result
                                                    .get("source")
                                                    .and_then(Value::as_str)
                                                    .unwrap_or("ocr")
                                            ),
                                            cx,
                                        ))
                                        .child(
                                            Button::new("ocr-copy")
                                                .label(if self.copied {
                                                    "Copied"
                                                } else {
                                                    "Copy all"
                                                })
                                                .small()
                                                .secondary()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some(text) = this.text.clone() {
                                                        cx.write_to_clipboard(
                                                            ClipboardItem::new_string(text),
                                                        );
                                                        this.copied = true;
                                                        cx.notify();
                                                    }
                                                })),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("ocr-text")
                                .max_h(px(360.))
                                .overflow_y_scroll()
                                .p_3()
                                .rounded_md()
                                .bg(cx.theme().muted.opacity(0.3))
                                .text_sm()
                                .child(if text.is_empty() {
                                    "No text recognized.".to_string()
                                } else {
                                    text.clone()
                                }),
                        ),
                );
            }

            if let Some(dst) = result.get("dst").and_then(Value::as_str) {
                let dst = dst.to_string();
                let bytes = result.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                let words = result.get("words").and_then(Value::as_u64).unwrap_or(0);
                body = body.child(
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

            body = body.child(success_box("Done.", cx));
            if let Some(note) = saved_note(self.jobs.read(cx).saved_for(&self.job), cx) {
                body = body.child(note);
            }
        }

        page(window)
            .child(heading(
                "OCR",
                "Tesseract runs locally. Nepali is used when the language pack is installed, English otherwise.",
                cx,
            ))
            .child(body)
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
