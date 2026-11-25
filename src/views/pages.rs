use std::path::PathBuf;

use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, field_label, file_row, heading, human_bytes, job_progress, muted, note, page,
    result_section, save_button, saved_note, stat, success_box,
};
use crate::engine::Engine;
use crate::jobs::{Job, Jobs, Tool};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Merge,
    Split,
    Extract,
}

pub struct PagesView {
    engine: Engine,
    jobs: Entity<Jobs>,
    files: Vec<PathBuf>,
    mode: Mode,
    every: u32,
    range: Entity<InputState>,
    range_text: String,
    job: Option<String>,
    _observe: Subscription,
    _input: Subscription,
}

impl PagesView {
    pub fn new(
        engine: Engine,
        jobs: Entity<Jobs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let range = cx.new(|cx| InputState::new(window, cx).placeholder("1-5, 8, 11-13"));
        let observe = cx.observe(&jobs, |_, _, cx| cx.notify());
        let input = cx.subscribe_in(&range, window, |this, state, event, _window, cx| {
            if matches!(event, InputEvent::Change) {
                this.range_text = state.read(cx).value().to_string();
                cx.notify();
            }
        });
        Self {
            engine,
            jobs,
            files: Vec::new(),
            mode: Mode::Merge,
            every: 1,
            range,
            range_text: String::new(),
            job: None,
            _observe: observe,
            _input: input,
        }
    }

    pub(crate) fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let is_pdf = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            if is_pdf && !self.files.contains(&path) {
                self.files.push(path);
            }
        }
        if self.files.len() > 1 && self.mode == Mode::Extract {
            self.mode = Mode::Merge;
        }
        cx.notify();
    }

    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Choose PDFs".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
        })
        .detach();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let Some(first) = self.files.first().cloned() else {
            return;
        };
        let op = match self.mode {
            Mode::Merge => "pdf.merge",
            Mode::Split => "pdf.split",
            Mode::Extract => "pdf.extract",
        };
        let dir = first
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let args = match self.mode {
            Mode::Merge => json!({
                "srcs": self.files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                "dst": dir.join("merged.pdf").to_string_lossy().to_string(),
            }),
            Mode::Split => json!({
                "src": first.to_string_lossy().to_string(),
                "out_dir": dir.to_string_lossy().to_string(),
                "every": self.every,
            }),
            Mode::Extract => {
                let pages = parse_ranges(&self.range_text);
                if pages.is_empty() {
                    return;
                }
                json!({
                    "src": first.to_string_lossy().to_string(),
                    "dst": dir.join("extracted.pdf").to_string_lossy().to_string(),
                    "pages": pages,
                })
            }
        };
        let id = self.engine.call(op, args);
        self.jobs.update(cx, |jobs, _| {
            jobs.start(id.clone(), Tool::Pages, "queued");
        });
        self.job = Some(id);
        cx.notify();
    }

    fn can_run(&self) -> bool {
        match self.mode {
            Mode::Merge => self.files.len() >= 2,
            Mode::Split => self.files.len() == 1,
            Mode::Extract => self.files.len() == 1 && !parse_ranges(&self.range_text).is_empty(),
        }
    }
}

fn parse_ranges(text: &str) -> Vec<u64> {
    let mut pages = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u64>(), b.trim().parse::<u64>())
                && a >= 1
                && b >= a
                && b - a < 2000
            {
                pages.extend(a..=b);
            }
        } else if let Ok(n) = part.parse::<u64>()
            && n >= 1
        {
            pages.push(n);
        }
        if pages.len() > 5000 {
            break;
        }
    }
    pages.sort_unstable();
    pages.dedup();
    pages
}

impl Render for PagesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let job: Option<Job> = self.jobs.read(cx).get(&self.job).cloned();
        let busy = job.as_ref().map(Job::is_running).unwrap_or(false);
        let result = job.as_ref().and_then(|j| j.result()).cloned();
        let can_run = self.can_run();

        let mut mode_row = div().flex().flex_wrap().gap_2().items_center();
        for mode in [Mode::Merge, Mode::Split, Mode::Extract] {
            let selected = self.mode == mode;
            let (id, label) = match mode {
                Mode::Merge => ("pages-merge", "Merge"),
                Mode::Split => ("pages-split", "Split"),
                Mode::Extract => ("pages-extract", "Extract pages"),
            };
            let button = Button::new(id).label(label).small();
            let button = if selected {
                button.primary()
            } else {
                button.secondary()
            };
            mode_row = mode_row.child(button.on_click(cx.listener(move |this, _, _, cx| {
                this.mode = mode;
                if mode == Mode::Merge && this.files.len() < 2 {
                    // still allowed, the run button explains what is missing
                }
                cx.notify();
            })));
        }

        let mut list = div()
            .id("pdf-files")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(200.))
            .overflow_y_scroll();
        for path in self.files.iter().take(200) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            list = list.child(file_row(&name, human_bytes(size), cx));
        }

        let mut body = card(cx).child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .justify_between()
                .items_center()
                .child(field_label("Documents", cx))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("pages-add")
                                .label("Add PDFs…")
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
                        )
                        .child(
                            Button::new("pages-clear")
                                .label("Clear")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.files.clear();
                                    cx.notify();
                                })),
                        ),
                ),
        );

        body = body.child(if self.files.is_empty() {
            dropzone(
                "Drop PDFs here",
                "or use Add PDFs… · merge, split, or extract pages",
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx)))
            .into_any_element()
        } else {
            list.into_any_element()
        });

        body = body.child(field_label("Action", cx)).child(mode_row);

        match self.mode {
            Mode::Merge => {
                body = body.child(note(
                    "Concatenates in the order shown. Writes merged.pdf next to the first file.",
                    cx,
                ));
            }
            Mode::Split => {
                let mut every_row = div().flex().flex_wrap().gap_2().items_center();
                for every in [1u32, 2, 5, 10] {
                    let selected = self.every == every;
                    let button = Button::new(SharedString::from(format!("every-{every}")))
                        .label(format!("{every} page(s)"))
                        .small();
                    let button = if selected {
                        button.primary()
                    } else {
                        button.ghost()
                    };
                    every_row =
                        every_row.child(button.on_click(cx.listener(move |this, _, _, cx| {
                            this.every = every;
                            cx.notify();
                        })));
                }
                body = body
                    .child(field_label("Split every", cx))
                    .child(every_row)
                    .child(note("Writes one file per chunk, named by page range.", cx));
            }
            Mode::Extract => {
                body = body
                    .child(field_label("Pages", cx))
                    .child(Input::new(&self.range))
                    .child(note("Examples: 1-5, 8, 11-13. Writes extracted.pdf.", cx));
            }
        }

        body = body
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(muted(format!("{} file(s) selected", self.files.len()), cx))
                    .child(
                        Button::new("pages-run")
                            .label(if busy { "Working…" } else { "Run" })
                            .primary()
                            .disabled(busy || !can_run)
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    ),
            )
            .child(job_progress(job.as_ref(), cx));

        if let Some(result) = result {
            if let Some(files) = result.get("files").and_then(Value::as_array) {
                body = body.child(
                    result_section(cx).child(field_label("Output", cx)).child(
                        div().flex().flex_col().gap_1().children(
                            files
                                .iter()
                                .take(200)
                                .map(|file| {
                                    let dst =
                                        file.get("dst").and_then(Value::as_str).unwrap_or_default();
                                    let name = std::path::Path::new(dst)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let pages =
                                        file.get("pages").and_then(Value::as_u64).unwrap_or(0);
                                    file_row(&name, format!("{pages} page(s)"), cx)
                                })
                                .collect::<Vec<_>>(),
                        ),
                    ),
                );
            } else {
                let dst = result
                    .get("dst")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let pages = result.get("pages").and_then(Value::as_u64).unwrap_or(0);
                let bytes = result.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                body = body.child(
                    result_section(cx)
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_8()
                                .child(stat("Pages", pages.to_string(), cx))
                                .child(stat("Size", human_bytes(bytes), cx)),
                        )
                        .child(file_row(&dst, "written", cx))
                        .child(save_button(
                            "pages-save",
                            Some(dst.clone()),
                            self.job.clone(),
                            self.jobs.clone(),
                            cx,
                        ))
                        .child(
                            Button::new("pages-open")
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
                "PDF pages",
                "Merge, split, and extract without uploading anything. Page-level editing stays local.",
                cx,
            ))
            .child(body)
    }
}
