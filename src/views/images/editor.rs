use super::*;
use gpui_kit::component::Icon;
use std::time::Duration;

fn icon(path: &str) -> Icon {
    Icon::default().data(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{path}</svg>").as_bytes()).size(px(16.))
}
fn tool(id: &'static str, label: &'static str, path: &str) -> Button {
    Button::new(id)
        .label(label)
        .icon(icon(path))
        .small()
        .ghost()
        .tooltip(label)
}

impl ImagesView {
    pub(super) fn ensure_editor(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.files.get(self.selected).cloned() else {
            return;
        };
        if self.editor_source.as_ref() == Some(&path) {
            return;
        }
        self.reset_edits();
        self._edit_timer = None;
        self.editor_source = Some(path.clone());
        self.edited_preview = None;
        self.preview_dims = None;
        if let Some(id) = self.edit_job.take() {
            self.engine.cancel(&id);
        }
        if let Some(id) = self.original_job.take() {
            self.engine.cancel(&id);
        }
        self.format = match path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "jpg" | "jpeg" => Format::Jpeg,
            "webp" => Format::Webp,
            _ => Format::Png,
        };
        self.edit_revision += 1;
        let dst = self
            .cache_dir
            .join(format!("original-{}.png", self.edit_revision));
        let id = self.engine.call(
            "image.preview",
            json!({"src":path.to_string_lossy(),"dst":dst.to_string_lossy()}),
        );
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Images, "Loading image…");
            cx.notify();
        });
        self.original_job = Some(id);
        self.render_edit(cx);
    }

    pub(super) fn queue_editor(&mut self, cx: &mut Context<Self>) {
        if self.mode != Mode::Edit || self.files.is_empty() {
            return;
        }
        self.edit_dirty = true;
        self.show_original = false;
        self.saved_path = None;
        self.save_error = None;
        self._edit_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(160))
                .await;
            let _ = this.update(cx, |this, cx| this.render_edit(cx));
        }));
        cx.notify();
    }

    fn render_edit(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.files.get(self.selected).cloned() else {
            return;
        };
        if let Some(id) = self.edit_job.take() {
            self.engine.cancel(&id);
        }
        self.edit_dirty = false;
        self.edit_error = None;
        let dimensions = if self.clear_resize_inputs {
            Ok((0, 0))
        } else {
            let parse = |state: &Entity<InputState>| -> Result<u32, String> {
                let value = state.read(cx).value().trim().to_string();
                if value.is_empty() {
                    return Ok(0);
                }
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n > 0 && *n <= 20000)
                    .ok_or_else(|| {
                        "Enter a width or height from 1 to 20,000 pixels, or leave it blank.".into()
                    })
            };
            parse(&self.resize_w).and_then(|w| parse(&self.resize_h).map(|h| (w, h)))
        };
        let (w, h) = match dimensions {
            Ok(value) => value,
            Err(error) => {
                self.edit_error = Some(error);
                cx.notify();
                return;
            }
        };
        self.edit_revision += 1;
        let extension = match self.format {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Webp => "webp",
        };
        let dst = self
            .cache_dir
            .join(format!("edited-{}.{}", self.edit_revision, extension));
        let mut args = json!({"src":path.to_string_lossy(),"dst":dst.to_string_lossy(),"format":self.format.key(),
            "rotate":self.rotate,"flip_h":self.flip_h,"flip_v":self.flip_v,"grayscale":self.grayscale});
        if let Some(c) = self.crop {
            args["crop"] = json!({"x":c.x,"y":c.y,"w":c.w,"h":c.h});
        }
        if w > 0 || h > 0 {
            args["resize"] = json!({"width":w,"height":h});
        }
        let id = self.engine.call("image.edit", args);
        self.jobs.update(cx, |jobs, cx| {
            jobs.start(id.clone(), Tool::Images, "Updating preview…");
            cx.notify();
        });
        self.edit_job = Some(id);
        cx.notify();
    }

    fn save_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_dirty || self.edit_error.is_some() || self.saving {
            return;
        }
        let Some(job) = self.jobs.read(cx).get(&self.edit_job) else {
            return;
        };
        let Some(result) = job.result() else {
            return;
        };
        let Some(source) = result.get("dst").and_then(Value::as_str).map(PathBuf::from) else {
            return;
        };
        let Some(original) = self.files.get(self.selected).cloned() else {
            return;
        };
        let extension = match self.format {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Webp => "webp",
        };
        let name = format!(
            "{}-edited.{extension}",
            original.file_stem().unwrap_or_default().to_string_lossy()
        );
        let directory = original.parent().unwrap_or(std::path::Path::new("."));
        let receiver = cx.prompt_for_new_path(directory, Some(&name));
        let revision = self.edit_revision;
        self.saving = true;
        self.save_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let selected = receiver.await;
            if let Ok(Ok(Some(destination))) = selected {
                let output = destination.clone();
                let copied = cx
                    .background_executor()
                    .spawn(async move { copy_new(&source, &output) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    match copied {
                        Ok(()) => {
                            if this.edit_revision == revision && !this.edit_dirty {
                                this.saved_path = Some(destination);
                            }
                        }
                        Err(error) => {
                            this.save_error =
                                Some(if error.kind() == std::io::ErrorKind::AlreadyExists {
                                    "Choose a new filename. Existing files are kept unchanged."
                                        .into()
                                } else {
                                    format!("Could not save the image: {error}")
                                });
                        }
                    }
                    cx.notify();
                });
            } else {
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn rotate_editor(&mut self, degrees: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.rotate = (self.rotate + degrees) % 360;
        std::mem::swap(&mut self.flip_h, &mut self.flip_v);
        let w = self.resize_w.read(cx).value().to_string();
        let h = self.resize_h.read(cx).value().to_string();
        self.resize_w
            .update(cx, |state, cx| state.set_value(h, window, cx));
        self.resize_h
            .update(cx, |state, cx| state.set_value(w, window, cx));
        self.queue_editor(cx);
    }

    fn apply_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.crop_draft.take() {
            self.crop = Some(draft.to_source(self.crop, self.rotate, self.flip_h, self.flip_v));
        }
        self.crop_open = false;
        self.crop_start = None;
        self.queue_editor(cx);
    }

    pub(super) fn render_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.ensure_editor(cx);
        if self.clear_resize_inputs {
            self.clear_resize_inputs = false;
            self.resize_w
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.resize_h
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        let info_job = self.jobs.read(cx).get(&self.info_job).cloned();
        if let Some(info) = info_job.as_ref().and_then(Job::result) {
            self.info = Some(info.clone());
        }
        let job = self.jobs.read(cx).get(&self.edit_job).cloned();
        let original = self
            .jobs
            .read(cx)
            .get(&self.original_job)
            .and_then(Job::result)
            .cloned();
        let ready = job.as_ref().and_then(Job::result);
        if !self.edit_dirty
            && let Some(result) = ready
        {
            self.edited_preview = result.get("dst").and_then(Value::as_str).map(PathBuf::from);
            self.preview_dims = Some((
                result["width"].as_u64().unwrap_or(1) as u32,
                result["height"].as_u64().unwrap_or(1) as u32,
            ));
        }
        let busy = self.edit_dirty || job.as_ref().is_some_and(Job::is_running);
        let available = !self.files.is_empty();
        let can_save = available
            && !busy
            && ready.is_some()
            && self.edit_error.is_none()
            && !self.crop_open
            && !self.saving;
        let filename = self
            .files
            .get(self.selected)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Open an image to get started".into());
        let mut root = page(window).max_w(px(1520.)).gap_4().child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .text_2xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Images"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(filename),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("editor-compress")
                                .label("Compress")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.mode = Mode::Compress;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("editor-open")
                                .label("Open…")
                                .small()
                                .secondary()
                                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
                        )
                        .child(
                            Button::new("editor-save")
                                .label(if self.saving { "Saving…" } else { "Save…" })
                                .primary()
                                .disabled(!can_save)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_editor(window, cx)),
                                ),
                        ),
                ),
        );
        if !available {
            return root.child(
                dropzone(
                    "Drop an image to start editing",
                    "JPEG, PNG, WebP, AVIF, HEIC and TIFF · files stay on this device",
                    cx,
                )
                .min_h(px(320.))
                .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx))),
            );
        }
        let locked = self.crop_open || self.saving;
        let toolbar=div().flex().flex_wrap().gap_1().items_center().p_2().rounded_lg().border_1().border_color(cx.theme().border).bg(cx.theme().popover)
            .child(tool("editor-crop","Crop","<path d='M6 3v13a2 2 0 0 0 2 2h13M3 6h13a2 2 0 0 1 2 2v13'/>").disabled(busy||self.saving)
                .when(self.crop_open,|b|b.primary()).on_click(cx.listener(|this,_,_,cx|{this.crop_open= !this.crop_open;this.resize_open=false;this.show_original=false;this.crop_draft=None;this.crop_start=None;cx.notify();})))
            .child(tool("editor-rotate-left","Left","<path d='M3 10a9 9 0 1 1 3 9M3 4v6h6'/>").disabled(locked).on_click(cx.listener(|this,_,window,cx|this.rotate_editor(270,window,cx))))
            .child(tool("editor-rotate-right","Right","<path d='M21 10a9 9 0 1 0-3 9M21 4v6h-6'/>").disabled(locked).on_click(cx.listener(|this,_,window,cx|this.rotate_editor(90,window,cx))))
            .child(tool("editor-flip-h","Flip H","<path d='M12 3v18M3 7v10l5-5zm18 0v10l-5-5z'/>").disabled(locked).when(self.flip_h,|b|b.secondary()).on_click(cx.listener(|this,_,_,cx|{this.flip_h= !this.flip_h;this.queue_editor(cx);})))
            .child(tool("editor-flip-v","Flip V","<path d='M3 12h18M7 3h10l-5 5zm0 18h10l-5-5z'/>").disabled(locked).when(self.flip_v,|b|b.secondary()).on_click(cx.listener(|this,_,_,cx|{this.flip_v= !this.flip_v;this.queue_editor(cx);})))
            .child(tool("editor-resize","Resize","<rect x='3' y='3' width='18' height='18' rx='2'/><path d='m8 16 8-8m-5 0h5v5'/>").disabled(locked).when(self.resize_open,|b|b.secondary()).on_click(cx.listener(|this,_,_,cx|{this.resize_open= !this.resize_open;cx.notify();})))
            .child(tool("editor-bw","B&W","<circle cx='12' cy='12' r='9'/><path d='M12 3v18'/> <path d='M12 3a9 9 0 0 1 0 18z' fill='currentColor'/>").disabled(locked).when(self.grayscale,|b|b.secondary()).on_click(cx.listener(|this,_,_,cx|{this.grayscale= !this.grayscale;this.queue_editor(cx);})))
            .child(div().flex_1())
            .child(Button::new("editor-reset").label("Reset").small().ghost().on_click(cx.listener(|this,_,_,cx|{this.reset_edits();this.queue_editor(cx);})))
            .child(Button::new("editor-original").label(if self.show_original{"Show edited"}else{"Original"}).small().ghost().disabled(self.crop_open||original.is_none()).on_click(cx.listener(|this,_,_,cx|{this.show_original= !this.show_original;cx.notify();})));
        root = root.child(toolbar);
        if self.crop_open {
            let mut crops = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .child(note("Drag on the image, or choose a ratio:", cx));
            for (id, label, ratio) in [
                ("editor-crop-free", "Free", 0.),
                ("editor-crop-square", "1:1", 1.),
                ("editor-crop-4x3", "4:3", 4. / 3.),
                ("editor-crop-wide", "16:9", 16. / 9.),
            ] {
                crops = crops.child(Button::new(id).label(label).small().ghost().on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.crop_draft = if ratio == 0. {
                            None
                        } else {
                            let (w, h) = this.preview_dims.unwrap_or((1, 1));
                            Some(Crop::centered(ratio / (w as f64 / h as f64)))
                        };
                        cx.notify();
                    }),
                ));
            }
            root = root.child(
                crops
                    .child(div().flex_1())
                    .child(
                        Button::new("editor-crop-cancel")
                            .label("Cancel")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.crop_open = false;
                                this.crop_draft = None;
                                this.crop_start = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("editor-crop-apply")
                            .label("Apply crop")
                            .small()
                            .primary()
                            .disabled(self.crop_draft.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.apply_crop(cx))),
                    ),
            );
        }
        if self.resize_open {
            root = root.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(field_label("Width", cx))
                    .child(Input::new(&self.resize_w).w(px(110.)))
                    .child(field_label("Height", cx))
                    .child(Input::new(&self.resize_h).w(px(110.)))
                    .child(note(
                        "Pixels · leave one blank to keep the aspect ratio",
                        cx,
                    )),
            );
        }
        if let Some(error) = self.edit_error.as_ref().or(self.save_error.as_ref()) {
            root = root.child(error_box(error, cx));
        }
        let stage_height = (f32::from(window.viewport_size().height)
            - 380.
            - if self.crop_open || self.resize_open {
                60.
            } else {
                0.
            })
        .clamp(220., 700.);
        let original_path = original
            .as_ref()
            .and_then(|v| v["dst"].as_str())
            .map(PathBuf::from);
        let path = if self.show_original {
            original_path
        } else {
            self.edited_preview.clone()
        };
        let dims = if self.show_original {
            original.as_ref().map(|v| {
                (
                    v["width"].as_u64().unwrap_or(1) as u32,
                    v["height"].as_u64().unwrap_or(1) as u32,
                )
            })
        } else {
            self.preview_dims
        };
        let mut stage = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(stage_height))
            .min_w_0()
            .flex_shrink_0()
            .rounded_lg()
            .overflow_hidden()
            .bg(cx.theme().muted.opacity(0.5));
        if let (Some(path), Some((w, h))) = (path, dims) {
            let width = (f32::from(window.viewport_size().width)
                - if window.viewport_size().width < px(960.) {
                    128.
                } else {
                    296.
                })
            .min(1456.)
            .min(stage_height * w as f32 / h as f32)
            .max(1.);
            let height = width * h as f32 / w as f32;
            let mut image = div()
                .relative()
                .w(px(width))
                .h(px(height))
                .child(img(path).w(px(width)).h(px(height)));
            if self.crop_open {
                if let Some(c) = self.crop_draft {
                    image = image.child(crop_overlay(c, width, height, cx));
                }
                image = image.child(crop_canvas(&cx.entity()));
            }
            stage = stage.child(image);
        } else {
            stage = stage.child(muted("Preparing image…", cx));
        }
        if busy {
            stage = stage.child(
                div()
                    .absolute()
                    .top_3()
                    .right_3()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().popover)
                    .text_sm()
                    .child("Updating preview…"),
            );
        }
        root = root.child(stage);
        let dimensions = self
            .preview_dims
            .map(|(w, h)| format!("{w} × {h} px"))
            .unwrap_or_default();
        let mut formats = div().flex().items_center().gap_1();
        for format in [Format::Png, Format::Jpeg, Format::Webp] {
            formats = formats.child(
                Button::new(format.id())
                    .label(format.label())
                    .small()
                    .ghost()
                    .when(self.format == format, |b| b.secondary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.format = format;
                        this.queue_editor(cx);
                    })),
            )
        }
        root = root.child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .items_center()
                .justify_between()
                .child(note(
                    format!(
                        "{} · {}",
                        if self.show_original {
                            "Original"
                        } else {
                            "Edited preview"
                        },
                        dimensions
                    ),
                    cx,
                ))
                .child(formats)
                .child(
                    Button::new("editor-details")
                        .label("Metadata & AI signals")
                        .small()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.details_open = !this.details_open;
                            cx.notify();
                        })),
                ),
        );
        if job.as_ref().and_then(Job::error).is_some() {
            root = root.child(job_progress(job.as_ref(), cx));
        }
        if let Some(path) = &self.saved_path {
            root = root.child(note(format!("Saved to {}", path.display()), cx));
        }
        if self.files.len() > 1 {
            let mut strip = div()
                .id("editor-filmstrip")
                .flex()
                .gap_2()
                .overflow_x_scroll()
                .pb_2();
            for (index, path) in self.files.iter().enumerate() {
                strip = strip.child(
                    Button::new(SharedString::from(format!("editor-file-{index}")))
                        .small()
                        .ghost()
                        .when(index == self.selected, |b| b.secondary())
                        .label(
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = index;
                            this.ensure_editor(cx);
                            this.load_info(cx);
                            cx.notify();
                        })),
                );
            }
            root = root.child(strip);
        }
        if self.details_open {
            if let Some(metadata) = self.metadata_panel(cx) {
                root = root.child(metadata);
            } else {
                root = root
                    .child(note("Reading metadata…", cx))
                    .child(job_progress(info_job.as_ref(), cx));
            }
        }
        root
    }
}

fn crop_overlay(c: Crop, width: f32, height: f32, cx: &App) -> Div {
    let (x, y, w, h) = (
        c.x as f32 * width,
        c.y as f32 * height,
        c.w as f32 * width,
        c.h as f32 * height,
    );
    let mask = rgb(0x101712).opacity(0.6);
    div()
        .absolute()
        .inset_0()
        .size_full()
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(width))
                .h(px(y))
                .bg(mask),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(y + h))
                .w(px(width))
                .h(px((height - y - h).max(0.)))
                .bg(mask),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(y))
                .w(px(x))
                .h(px(h))
                .bg(mask),
        )
        .child(
            div()
                .absolute()
                .left(px(x + w))
                .top(px(y))
                .w(px((width - x - w).max(0.)))
                .h(px(h))
                .bg(mask),
        )
        .child(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w))
                .h(px(h))
                .border_2()
                .border_color(cx.theme().primary),
        )
}

impl Crop {
    fn to_source(self, base: Option<Crop>, rotate: u32, flip_h: bool, flip_v: bool) -> Self {
        let base = base.unwrap_or(Crop {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        });
        let mut points = Vec::new();
        for (mut x, mut y) in [
            (self.x, self.y),
            (self.x + self.w, self.y),
            (self.x, self.y + self.h),
            (self.x + self.w, self.y + self.h),
        ] {
            if flip_h {
                x = 1. - x;
            }
            if flip_v {
                y = 1. - y;
            }
            let (u, v) = match rotate % 360 {
                90 => (y, 1. - x),
                180 => (1. - x, 1. - y),
                270 => (1. - y, x),
                _ => (x, y),
            };
            points.push((base.x + u * base.w, base.y + v * base.h));
        }
        let x = points.iter().map(|p| p.0).fold(1., f64::min);
        let y = points.iter().map(|p| p.1).fold(1., f64::min);
        let right = points.iter().map(|p| p.0).fold(0., f64::max);
        let bottom = points.iter().map(|p| p.1).fold(0., f64::max);
        Crop {
            x: x.clamp(0., 1.),
            y: y.clamp(0., 1.),
            w: (right - x).clamp(0., 1.),
            h: (bottom - y).clamp(0., 1.),
        }
    }
}

fn copy_new(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = std::io::copy(&mut input, &mut output).and_then(|_| output.flush());
    if let Err(error) = result {
        drop(output);
        let _ = std::fs::remove_file(destination);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Crop, copy_new};
    #[test]
    fn crop_maps_back_through_rotations_flips_and_an_existing_crop() {
        let draft = Crop {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        };
        let rotated = draft.to_source(None, 90, false, false);
        assert!((rotated.x - 0.2).abs() < 1e-9 && (rotated.y - 0.6).abs() < 1e-9);
        assert!((rotated.w - 0.4).abs() < 1e-9 && (rotated.h - 0.3).abs() < 1e-9);
        let flipped = draft.to_source(
            Some(Crop {
                x: 0.1,
                y: 0.2,
                w: 0.8,
                h: 0.6,
            }),
            90,
            true,
            false,
        );
        assert!((flipped.x - 0.26).abs() < 1e-9 && (flipped.y - 0.26).abs() < 1e-9);
        assert!((flipped.w - 0.32).abs() < 1e-9 && (flipped.h - 0.18).abs() < 1e-9);
    }
    #[test]
    fn ratio_presets_keep_the_requested_physical_aspect_ratio() {
        let image_aspect = 3.0 / 2.0;
        let crop = Crop::centered((16.0 / 9.0) / image_aspect);
        assert!((crop.w / crop.h * image_aspect - 16.0 / 9.0).abs() < 1e-9);
    }
    #[test]
    fn saving_copies_the_preview_and_keeps_existing_files() {
        let root =
            std::env::temp_dir().join(format!("garage-editor-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let src = root.join("preview.png");
        let dst = root.join("saved.png");
        let _ = std::fs::remove_file(&dst);
        std::fs::write(&src, b"edited image bytes").unwrap();
        copy_new(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), std::fs::read(&src).unwrap());
        assert_eq!(
            copy_new(&src, &dst).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&dst).unwrap(), b"edited image bytes");
        let _ = std::fs::remove_dir_all(root);
    }
}
