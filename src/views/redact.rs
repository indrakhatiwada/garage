use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, field_label, file_row, heading, human_bytes, job_progress, muted, note, page,
    result_section, save_button, saved_note, success_box,
};
use crate::engine::Engine;
use crate::jobs::{Job, Jobs, Tool};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pixelate,
    Blur,
    Solid,
}

impl Mode {
    fn key(self) -> &'static str {
        match self {
            Self::Pixelate => "pixelate",
            Self::Blur => "blur",
            Self::Solid => "solid",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Pixelate => "Pixelate",
            Self::Blur => "Blur",
            Self::Solid => "Solid black",
        }
    }
}

#[derive(Clone, Debug)]
struct RedactBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    page: u64,
}

impl RedactBox {
    fn between(start: Point<f64>, end: Point<f64>, page: u64) -> Option<Self> {
        let x1 = start.x.clamp(0., 1.);
        let y1 = start.y.clamp(0., 1.);
        let x2 = end.x.clamp(0., 1.);
        let y2 = end.y.clamp(0., 1.);
        let w = (x2 - x1).abs();
        let h = (y2 - y1).abs();
        (w > 0. && h > 0.).then_some(Self {
            x: x1.min(x2),
            y: y1.min(y2),
            w,
            h,
            page,
        })
    }
}

#[derive(Clone, Copy)]
struct Selection {
    start: Point<f64>,
    end: Point<f64>,
}

fn normalized(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Point<f64> {
    point(
        f64::from((position.x - bounds.left()) / bounds.size.width.max(px(1.))).clamp(0., 1.),
        f64::from((position.y - bounds.top()) / bounds.size.height.max(px(1.))).clamp(0., 1.),
    )
}

pub struct RedactView {
    engine: Engine,
    jobs: Entity<Jobs>,
    path: Option<PathBuf>,
    is_pdf: bool,
    dims: Option<(u32, u32)>,
    preview: Option<PathBuf>,
    redacted_preview: Option<PathBuf>,
    redacted_job: Option<String>,
    preview_revision: u64,
    show_redacted: bool,
    cache_dir: PathBuf,
    document: u64,
    current_page: u64,
    pages: u64,
    fit_page: bool,
    boxes: Vec<RedactBox>,
    selection: Option<Selection>,
    mode: Mode,
    strength: u32,
    job: Option<String>,
    preview_job: Option<String>,
    focus: FocusHandle,
    output_dir: Option<PathBuf>,
    choices: super::ChoiceStore,
    choice_events: Vec<Subscription>,
    _observe: Subscription,
}

impl RedactView {
    pub fn new(
        engine: Engine,
        jobs: Entity<Jobs>,
        _window: &mut Window,
        cx: &mut Context<Self>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            choices: super::ChoiceStore::new(),
            choice_events: vec![],
            engine,
            jobs: jobs.clone(),
            path: None,
            is_pdf: false,
            dims: None,
            preview: None,
            redacted_preview: None,
            redacted_job: None,
            preview_revision: 0,
            show_redacted: false,
            cache_dir: std::env::temp_dir()
                .join(format!("garage-redact-{}-{unique}", std::process::id())),
            document: 0,
            current_page: 1,
            pages: 1,
            fit_page: true,
            boxes: Vec::new(),
            selection: None,
            mode: Mode::Pixelate,
            strength: 12,
            job: None,
            preview_job: None,
            focus: cx.focus_handle(),
            output_dir,
            _observe: cx.observe(&jobs, |_, _, cx| cx.notify()),
        }
    }

    pub(crate) fn set_output_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        self.output_dir = dir;
        cx.notify();
    }

    pub(crate) fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if let Some(path) = paths.into_iter().next() {
            self.load(path, cx);
        }
    }

    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose an image or PDF to redact".into()),
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

    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.document += 1;
        self.boxes.clear();
        self.selection = None;
        self.job = None;
        self.current_page = 1;
        self.pages = 1;
        self.fit_page = true;
        self.show_redacted = false;
        self.is_pdf = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if self.is_pdf {
            self.mode = Mode::Solid;
        }
        self.path = Some(path);
        self.load_preview(cx);
    }

    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(id) = &self.preview_job {
            self.engine.cancel(id);
        }
        self.preview = None;
        self.dims = None;
        self.selection = None;
        let dst = self
            .cache_dir
            .join(format!("{}-{}.png", self.document, self.current_page));
        let id = self.engine.call(
            if self.is_pdf {
                "pdf.render"
            } else {
                "image.preview"
            },
            json!({
                "src": path.to_string_lossy(), "dst": dst.to_string_lossy(),
                "page": self.current_page, "dpi": 120,
            }),
        );
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Redact, "Preparing preview…");
            cx.notify();
        });
        self.preview_job = Some(id);
        self.refresh_redacted_preview(cx);
        cx.notify();
    }

    fn box_values(&self, page: Option<u64>) -> Vec<Value> {
        self.boxes
            .iter()
            .filter(|area| page.is_none_or(|page| area.page == page))
            .map(|area| {
                json!({
                    "x": area.x, "y": area.y, "w": area.w, "h": area.h,
                    "page": area.page, "mode": self.mode.key(), "strength": self.strength,
                    "color": [0, 0, 0],
                })
            })
            .collect()
    }

    fn refresh_redacted_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.redacted_job.take() {
            self.engine.cancel(&id);
        }
        self.redacted_preview = None;
        let boxes = self.box_values(Some(self.current_page));
        if boxes.is_empty() {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        // A unique path prevents the image cache from reusing an earlier edit.
        self.preview_revision += 1;
        let dst = self.cache_dir.join(format!(
            "redacted-{}-{}-{}.png",
            self.document, self.current_page, self.preview_revision
        ));
        let id = self.engine.call(
            if self.is_pdf {
                "pdf.render"
            } else {
                "image.preview"
            },
            json!({
                "src": path.to_string_lossy(), "dst": dst.to_string_lossy(),
                "page": self.current_page, "dpi": 120, "boxes": boxes,
            }),
        );
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Redact, "Updating redacted preview…");
            cx.notify();
        });
        self.redacted_job = Some(id);
    }

    fn export_busy(&self, cx: &App) -> bool {
        self.jobs
            .read(cx)
            .get(&self.job)
            .is_some_and(Job::is_running)
    }

    fn change_page(&mut self, page: u64, cx: &mut Context<Self>) {
        if page < 1 || page > self.pages || page == self.current_page || self.export_busy(cx) {
            return;
        }
        self.current_page = page;
        self.load_preview(cx);
    }

    fn finish_selection(
        &mut self,
        position: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.selection.take() else {
            return;
        };
        if let Some(area) = RedactBox::between(
            selection.start,
            normalized(position, bounds),
            self.current_page,
        ) && area.w * f64::from(bounds.size.width) >= 3.
            && area.h * f64::from(bounds.size.height) >= 3.
        {
            self.boxes.push(area);
            self.job = None;
            self.show_redacted = true;
            self.refresh_redacted_preview(cx);
        }
        cx.notify();
    }

    /// Listen at window level so a drag can finish beyond the preview's edge.
    fn drawing_surface(&self, cx: &Context<Self>) -> impl IntoElement {
        let down_view = cx.entity().downgrade();
        let move_view = down_view.clone();
        let up_view = down_view.clone();
        let enabled = !self.export_busy(cx);
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                if !enabled {
                    return;
                }
                window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble()
                        || event.button != MouseButton::Left
                        || !hitbox.is_hovered(window)
                    {
                        return;
                    }
                    let _ = down_view.update(cx, |view, cx| {
                        if view.export_busy(cx) {
                            return;
                        }
                        let start = normalized(event.position, bounds);
                        view.selection = Some(Selection { start, end: start });
                        window.focus(&view.focus, cx);
                        window.capture_pointer(hitbox.id);
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    });
                });
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let _ = move_view.update(cx, |view, cx| {
                        let Some(selection) = &mut view.selection else {
                            return;
                        };
                        if event.pressed_button == Some(MouseButton::Left) {
                            selection.end = normalized(event.position, bounds);
                        } else {
                            view.selection = None;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    });
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if !phase.bubble() || event.button != MouseButton::Left {
                        return;
                    }
                    let _ = up_view.update(cx, |view, cx| {
                        if view.selection.is_some() {
                            view.finish_selection(event.position, bounds, cx);
                            cx.stop_propagation();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.boxes.is_empty() {
            return;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "document".into());
        let dir = self.output_dir.clone().unwrap_or_else(|| {
            path.parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        });
        self.show_redacted = true;
        let box_values = self.box_values(None);
        let (op, args) = if self.is_pdf {
            (
                "pdf.redact",
                json!({
                    "src": path.to_string_lossy().to_string(),
                    "dst": dir.join(format!("{stem}.redacted.pdf")).to_string_lossy().to_string(),
                    "boxes": box_values,
                    "flatten": true,
                }),
            )
        } else {
            (
                "image.redact",
                json!({
                    "src": path.to_string_lossy().to_string(),
                    "dst": dir.join(format!("{stem}.redacted.jpg")).to_string_lossy().to_string(),
                    "boxes": box_values,
                }),
            )
        };
        let id = self.engine.call(op, args);
        self.jobs.update(cx, |jobs, _| {
            jobs.start_export(id.clone(), Tool::Redact, "redacting");
        });
        self.job = Some(id);
        cx.notify();
    }
}

impl Drop for RedactView {
    fn drop(&mut self) {
        if let Some(id) = &self.preview_job {
            self.engine.cancel(id);
        }
        if let Some(id) = &self.redacted_job {
            self.engine.cancel(id);
        }
        let _ = std::fs::remove_dir_all(&self.cache_dir);
    }
}

fn area_overlay(area: &RedactBox, width: f32, height: f32, label: String, cx: &App) -> Div {
    div()
        .absolute()
        .left(px(area.x as f32 * width))
        .top(px(area.y as f32 * height))
        .w(px(area.w as f32 * width))
        .h(px(area.h as f32 * height))
        .bg(cx.theme().danger.opacity(0.28))
        .border_1()
        .border_color(cx.theme().danger)
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .px_1()
                .text_xs()
                .bg(cx.theme().danger)
                .text_color(cx.theme().danger_foreground)
                .child(label),
        )
}

impl Render for RedactView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview_job = self.jobs.read(cx).get(&self.preview_job).cloned();
        let job = self.jobs.read(cx).get(&self.job).cloned();
        let redacted_job = self.jobs.read(cx).get(&self.redacted_job).cloned();
        if let Some(result) = redacted_job.as_ref().and_then(Job::result) {
            self.redacted_preview = result.get("dst").and_then(Value::as_str).map(PathBuf::from);
        }
        if let Some(info) = preview_job.as_ref().and_then(Job::result) {
            let w = info.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
            let h = info.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
            if w > 0 && h > 0 {
                self.dims = Some((w, h));
                self.preview = info.get("dst").and_then(Value::as_str).map(PathBuf::from);
                self.pages = info
                    .get("pages")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1);
            }
        }
        let busy = self.export_busy(cx);
        let loading = preview_job.as_ref().is_some_and(Job::is_running);
        let result = job.as_ref().and_then(Job::result).cloned();
        let page_areas = self
            .boxes
            .iter()
            .filter(|area| area.page == self.current_page)
            .count();
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Choose an image or PDF to begin.".into());
        let mut body = card(cx)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(field_label("Source", cx))
                    .child(
                        Button::new("redact-pick")
                            .label("Choose file…")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
                    ),
            )
            .child(div().text_sm().truncate().child(name));
        if self.path.is_none() {
            body = body.child(
                dropzone(
                    "Drop an image or PDF here",
                    "Click to choose a file, then draw over the areas to redact.",
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
            );
        } else {
            let mut toolbar = div()
                .flex()
                .flex_wrap()
                .gap_3()
                .justify_between()
                .items_center();
            if self.is_pdf {
                toolbar = toolbar.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("redact-prev")
                                .ghost()
                                .small()
                                .tooltip("Previous page")
                                .child(Icon::new(IconName::ChevronLeft).size(px(16.)))
                                .disabled(busy || self.current_page <= 1)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.change_page(this.current_page.saturating_sub(1), cx)
                                })),
                        )
                        .child(note(
                            format!("Page {} of {}", self.current_page, self.pages),
                            cx,
                        ))
                        .child(
                            Button::new("redact-next")
                                .ghost()
                                .small()
                                .tooltip("Next page")
                                .child(Icon::new(IconName::ChevronRight).size(px(16.)))
                                .disabled(busy || self.current_page >= self.pages)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.change_page(this.current_page + 1, cx)
                                })),
                        ),
                );
            } else {
                toolbar = toolbar.child(field_label("Draw areas to redact", cx));
            }
            toolbar = toolbar.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("redact-fit")
                            .label(if self.fit_page {
                                "Enlarge preview"
                            } else {
                                "Fit preview"
                            })
                            .small()
                            .ghost()
                            .disabled(loading || self.preview.is_none() || busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.fit_page = !this.fit_page;
                                this.selection = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("redact-undo")
                            .label("Undo area")
                            .small()
                            .ghost()
                            .disabled(busy || page_areas == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(index) = this
                                    .boxes
                                    .iter()
                                    .rposition(|area| area.page == this.current_page)
                                {
                                    this.boxes.remove(index);
                                    this.job = None;
                                    this.refresh_redacted_preview(cx);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new("redact-clear")
                            .label(if self.is_pdf {
                                "Clear page"
                            } else {
                                "Clear areas"
                            })
                            .small()
                            .ghost()
                            .disabled(busy || page_areas == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.boxes.retain(|area| area.page != this.current_page);
                                this.job = None;
                                this.refresh_redacted_preview(cx);
                                cx.notify();
                            })),
                    ),
            );
            let mut comparison = div().flex().flex_wrap().gap_2().items_center();
            for (id, label, redacted) in [
                ("redact-original", "Original", false),
                ("redact-result-preview", "Redacted preview", true),
            ] {
                let button = Button::new(id)
                    .label(label)
                    .small()
                    .disabled(loading || self.preview.is_none());
                let button = if self.show_redacted == redacted {
                    button.primary()
                } else {
                    button.ghost()
                };
                comparison =
                    comparison.child(button.on_click(cx.listener(move |this, _, _, cx| {
                        this.show_redacted = redacted;
                        cx.notify();
                    })));
            }
            body = body.child(toolbar).child(comparison).child(note(
                if self.show_redacted { "Preview of the redacted result. Drag to add another area, or choose Original to review your selections." }
                else { "Drag across the preview to draw a box. Press Esc to cancel a box in progress." }, cx,
            ));
            let display = if self.show_redacted {
                self.redacted_preview.as_ref().or(self.preview.as_ref())
            } else {
                self.preview.as_ref()
            };
            if let (Some(path), Some((w, h))) = (display, self.dims) {
                let available = super::panel_width(window).clamp(1., 1000.);
                let fit_height =
                    (f32::from(window.viewport_size().height) - 280.).clamp(320., 760.);
                let width = if self.fit_page {
                    available.min(fit_height * w as f32 / h as f32)
                } else {
                    available
                };
                let height = width * h as f32 / w as f32;
                let mut overlay = div()
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .flex_shrink_0()
                    .bg(cx.theme().muted)
                    .child(img(path.clone()).w(px(width)).h(px(height)));
                for (index, area) in self
                    .boxes
                    .iter()
                    .enumerate()
                    .filter(|(_, area)| area.page == self.current_page)
                {
                    if !self.show_redacted {
                        overlay = overlay.child(area_overlay(
                            area,
                            width,
                            height,
                            (index + 1).to_string(),
                            cx,
                        ));
                    } else if self.redacted_preview.is_none() {
                        // Cover selected content while the real effect is rendering.
                        overlay = overlay.child(
                            div()
                                .absolute()
                                .left(px(area.x as f32 * width))
                                .top(px(area.y as f32 * height))
                                .w(px(area.w as f32 * width))
                                .h(px(area.h as f32 * height))
                                .bg(rgb(0x000000)),
                        );
                    }
                }
                if self.show_redacted && self.redacted_preview.is_none() && page_areas > 0 {
                    overlay = overlay.child(
                        div()
                            .absolute()
                            .top_2()
                            .right_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(cx.theme().popover)
                            .text_color(cx.theme().foreground)
                            .text_xs()
                            .child(if redacted_job.as_ref().and_then(Job::error).is_some() {
                                "Preview unavailable"
                            } else {
                                "Updating preview…"
                            }),
                    );
                }
                if let Some(selection) = self.selection
                    && let Some(area) =
                        RedactBox::between(selection.start, selection.end, self.current_page)
                {
                    overlay = overlay.child(area_overlay(&area, width, height, String::new(), cx));
                }
                body = body.child(
                    div()
                        .flex()
                        .justify_center()
                        .w_full()
                        .min_w_0()
                        .child(overlay.child(self.drawing_surface(cx))),
                );
            } else if loading {
                body = body.child(
                    div()
                        .h(px(240.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .bg(cx.theme().muted.opacity(0.3))
                        .child(muted("Preparing your preview…", cx)),
                );
            }
            body = body.child(job_progress(preview_job.as_ref(), cx));
            if preview_job.as_ref().and_then(Job::error).is_some() {
                body = body.child(
                    Button::new("redact-retry")
                        .label("Retry preview")
                        .small()
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.load_preview(cx))),
                );
            }
            if self.show_redacted && redacted_job.as_ref().and_then(Job::error).is_some() {
                body = body.child(job_progress(redacted_job.as_ref(), cx)).child(
                    Button::new("redact-result-retry")
                        .label("Retry redacted preview")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_redacted_preview(cx);
                            cx.notify();
                        })),
                );
            }
            let mut areas = div().flex().flex_col().gap_2().child(field_label(
                &format!("{} areas selected", self.boxes.len()),
                cx,
            ));
            for (index, area) in self.boxes.iter().enumerate() {
                let label = if self.is_pdf {
                    format!("Area {} · Page {}", index + 1, area.page)
                } else {
                    format!("Area {}", index + 1)
                };
                areas = areas.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(note(label, cx))
                        .child(
                            Button::new(SharedString::from(format!("redact-remove-{index}")))
                                .label("Remove")
                                .small()
                                .ghost()
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if index < this.boxes.len() {
                                        this.boxes.remove(index);
                                        this.job = None;
                                        this.refresh_redacted_preview(cx);
                                        cx.notify();
                                    }
                                })),
                        ),
                );
            }
            body = body.child(areas);
            if self.is_pdf {
                body = body.child(note("PDF areas are filled with solid black. Export flattens the PDF and removes its hidden text layer.", cx));
            } else {
                let modes = super::choice(
                    &mut self.choices,
                    &mut self.choice_events,
                    "redaction-style",
                    vec![
                        ("pixelate", "Pixelate"),
                        ("blur", "Blur"),
                        ("solid", "Solid black"),
                    ],
                    self.mode.key(),
                    busy,
                    window,
                    cx,
                    |t, value, _, cx| {
                        t.mode = match value {
                            "blur" => Mode::Blur,
                            "solid" => Mode::Solid,
                            _ => Mode::Pixelate,
                        };
                        t.show_redacted = true;
                        t.job = None;
                        t.refresh_redacted_preview(cx);
                        cx.notify();
                    },
                );
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .max_w(px(320.))
                        .child(field_label("Redaction style", cx))
                        .child(modes),
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
                        .child(note(
                            "Writes a new file. The original is never modified.",
                            cx,
                        ))
                        .child(
                            Button::new("redact-run")
                                .label(if busy {
                                    "Redacting…"
                                } else {
                                    "Redact selected areas"
                                })
                                .primary()
                                .disabled(busy || self.boxes.is_empty() || self.selection.is_some())
                                .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                        ),
                )
                .child(job_progress(job.as_ref(), cx));
        }

        if let Some(result) = result {
            let dst = result
                .get("dst")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let bytes = result.get("bytes").and_then(Value::as_u64).unwrap_or(0);
            let opened = dst.clone();
            body = body.child(
                result_section(cx)
                    .child(file_row(&dst, human_bytes(bytes), cx))
                    .child(save_button(
                        "redact-save",
                        Some(dst.clone()),
                        self.job.clone(),
                        self.jobs.clone(),
                        self.output_dir.clone(),
                        cx,
                    ))
                    .child(
                        Button::new("redact-open")
                            .label("Open result")
                            .small()
                            .secondary()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let _ = &this;
                                cx.open_with_system(std::path::Path::new(&opened));
                            })),
                    ),
            );
            body = body.child(success_box("Done.", cx));
            if let Some(note) = saved_note(self.jobs.read(cx).saved_for(&self.job), cx) {
                body = body.child(note);
            }
        }

        div().id("redact-workspace").track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.selection.take().is_some() {
                    window.release_pointer(); cx.stop_propagation(); cx.notify();
                }
            }))
            .child(page(window).child(heading("Redact", "Draw over names, faces, or other details to remove them from an image or PDF.", cx)).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::{RedactBox, normalized};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn dragging_in_either_direction_marks_the_same_area() {
        let a = RedactBox::between(point(0.2, 0.3), point(0.8, 0.9), 3).unwrap();
        let b = RedactBox::between(point(0.8, 0.9), point(0.2, 0.3), 3).unwrap();
        assert_eq!((a.x, a.y, a.w, a.h, a.page), (b.x, b.y, b.w, b.h, b.page));
        assert_eq!(a.page, 3);
    }

    #[test]
    fn dragging_beyond_the_preview_stays_within_the_document() {
        let area = RedactBox::between(point(0.2, 0.3), point(1.4, -0.5), 1).unwrap();
        assert_eq!((area.x, area.y, area.w, area.h), (0.2, 0., 0.8, 0.3));
        assert!(RedactBox::between(point(0.2, 0.3), point(0.2, 0.9), 1).is_none());
    }

    #[test]
    fn normalized_coordinates_survive_resize_and_scroll() {
        let small = normalized(
            point(px(150.), px(160.)),
            Bounds::new(point(px(50.), px(-40.)), size(px(200.), px(400.))),
        );
        let large = normalized(
            point(px(300.), px(410.)),
            Bounds::new(point(px(100.), px(10.)), size(px(400.), px(800.))),
        );
        assert_eq!(small, point(0.5, 0.5));
        assert_eq!(large, small);
    }
}
