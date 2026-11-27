mod editor;

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme, Disableable, IndexPath, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    searchable_list::SearchableVec,
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{
    card, dropzone, error_box, field_label, file_row, heading, human_bytes, job_progress, muted,
    note, page, panel_width, pill, result_section, stat, success_box,
};
use crate::engine::Engine;
use crate::jobs::{Job, Jobs, Tool};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Preset {
    Email,
    Web,
    Max,
    Target,
}

impl Preset {
    fn key(self) -> &'static str {
        match self {
            Preset::Email => "email",
            Preset::Web => "web",
            Preset::Max => "max",
            Preset::Target => "custom",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Preset::Email => "preset-email",
            Preset::Web => "preset-web",
            Preset::Max => "preset-max",
            Preset::Target => "preset-target",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Preset::Email => "Email · 1600px",
            Preset::Web => "Web · 2400px",
            Preset::Max => "Max quality",
            Preset::Target => "Target size",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Compress,
    Edit,
}

impl Mode {
    fn id(self) -> &'static str {
        match self {
            Mode::Compress => "images-mode-compress",
            Mode::Edit => "images-mode-edit",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Compress => "Compress",
            Mode::Edit => "Edit & crop",
        }
    }
}

/// A normalised selection on the preview. `None` means the whole image.
#[derive(Clone, Copy, PartialEq)]
struct Crop {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Crop {
    fn from_points(ax: f64, ay: f64, bx: f64, by: f64) -> Self {
        let x = ax.min(bx);
        let y = ay.min(by);
        Self {
            x,
            y,
            w: bx.max(ax) - x,
            h: by.max(ay) - y,
        }
    }

    fn centered(aspect: f64) -> Self {
        let (mut w, mut h) = if aspect >= 1.0 {
            (1.0, 1.0 / aspect)
        } else {
            (aspect, 1.0)
        };
        w *= 0.98;
        h *= 0.98;
        Self {
            x: (1.0 - w) / 2.0,
            y: (1.0 - h) / 2.0,
            w,
            h,
        }
    }

    fn label(self) -> String {
        format!(
            "x {:.0}% · y {:.0}% · {:.0}% × {:.0}%",
            self.x * 100.0,
            self.y * 100.0,
            self.w * 100.0,
            self.h * 100.0
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Jpeg,
    Webp,
    Png,
}

impl Format {
    fn key(self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Webp => "webp",
            Format::Png => "png",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Format::Jpeg => "fmt-jpeg",
            Format::Webp => "fmt-webp",
            Format::Png => "fmt-png",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
            Format::Png => "PNG",
        }
    }
}

fn preset_from_label(label: &str) -> Preset {
    match label {
        "Web · 2400px" => Preset::Web,
        "Max quality" => Preset::Max,
        "Target size" => Preset::Target,
        _ => Preset::Email,
    }
}

fn format_from_label(label: &str) -> Format {
    match label {
        "WebP" => Format::Webp,
        "PNG" => Format::Png,
        _ => Format::Jpeg,
    }
}

fn target_from_label(label: &str) -> u64 {
    label
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(200)
}

pub struct ImagesView {
    engine: Engine,
    jobs: Entity<Jobs>,
    files: Vec<PathBuf>,
    mode: Mode,
    preset: Preset,
    format: Format,
    strip_exif: bool,
    target_kb: u64,
    output_dir: Option<PathBuf>,
    preset_select: Entity<SelectState<SearchableVec<String>>>,
    format_select: Entity<SelectState<SearchableVec<String>>>,
    target_select: Entity<SelectState<SearchableVec<String>>>,
    selected: usize,
    // Edit state
    crop: Option<Crop>,
    crop_ratio: &'static str,
    crop_start: Option<(f64, f64)>,
    rotate: u32,
    flip_h: bool,
    flip_v: bool,
    grayscale: bool,
    resize_w: Entity<InputState>,
    resize_h: Entity<InputState>,
    cache_dir: PathBuf,
    edited_preview: Option<PathBuf>,
    edit_job: Option<String>,
    original_job: Option<String>,
    editor_source: Option<PathBuf>,
    preview_dims: Option<(u32, u32)>,
    crop_draft: Option<Crop>,
    crop_open: bool,
    resize_open: bool,
    show_original: bool,
    details_open: bool,
    clear_resize_inputs: bool,
    edit_dirty: bool,
    edit_error: Option<String>,
    saved_path: Option<PathBuf>,
    save_error: Option<String>,
    saving: bool,
    _edit_timer: Option<Task<()>>,
    _resize_events: Vec<Subscription>,
    _select_events: Vec<Subscription>,
    edit_revision: u64,
    job: Option<String>,
    info_job: Option<String>,
    info: Option<Value>,
    choices: super::ChoiceStore,
    choice_events: Vec<Subscription>,
    _observe: Subscription,
}

impl ImagesView {
    pub fn new(
        engine: Engine,
        jobs: Entity<Jobs>,
        window: &mut Window,
        cx: &mut Context<Self>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        let observe = cx.observe(&jobs, |_, _, cx| cx.notify());
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let resize_w = cx.new(|cx| InputState::new(window, cx).placeholder("Auto"));
        let resize_h = cx.new(|cx| InputState::new(window, cx).placeholder("Auto"));
        let resize_events = [&resize_w, &resize_h]
            .into_iter()
            .map(|state| {
                cx.subscribe(state, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.queue_editor(cx);
                    }
                })
            })
            .collect();
        let preset_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    [Preset::Email, Preset::Web, Preset::Max, Preset::Target]
                        .iter()
                        .map(|preset| preset.label().to_string())
                        .collect::<Vec<String>>(),
                ),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let format_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    [Format::Jpeg, Format::Webp, Format::Png]
                        .iter()
                        .map(|format| format.label().to_string())
                        .collect::<Vec<String>>(),
                ),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let target_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![
                    "≤ 100 KB".to_string(),
                    "≤ 200 KB".to_string(),
                    "≤ 500 KB".to_string(),
                ]),
                Some(IndexPath::new(1)),
                window,
                cx,
            )
        });
        let select_events = [
            cx.subscribe(&preset_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(value) = event
                    && let Some(value) = value.as_deref()
                {
                    this.preset = preset_from_label(value);
                    cx.notify();
                }
            }),
            cx.subscribe(&format_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(value) = event
                    && let Some(value) = value.as_deref()
                {
                    this.format = format_from_label(value);
                    cx.notify();
                }
            }),
            cx.subscribe(&target_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(value) = event
                    && let Some(value) = value.as_deref()
                {
                    this.target_kb = target_from_label(value);
                    cx.notify();
                }
            }),
        ];
        Self {
            choices: super::ChoiceStore::new(),
            choice_events: vec![],
            engine,
            jobs,
            files: Vec::new(),
            mode: Mode::Edit,
            preset: Preset::Email,
            format: Format::Jpeg,
            strip_exif: true,
            target_kb: 200,
            output_dir,
            preset_select,
            format_select,
            target_select,
            selected: 0,
            crop: None,
            crop_ratio: "free",
            crop_start: None,
            rotate: 0,
            flip_h: false,
            flip_v: false,
            grayscale: false,
            resize_w,
            resize_h,
            cache_dir: std::env::temp_dir()
                .join(format!("garage-images-{unique}-{}", std::process::id())),
            edited_preview: None,
            edit_job: None,
            original_job: None,
            editor_source: None,
            preview_dims: None,
            crop_draft: None,
            crop_open: false,
            resize_open: false,
            show_original: false,
            details_open: false,
            clear_resize_inputs: false,
            edit_dirty: false,
            edit_error: None,
            saved_path: None,
            save_error: None,
            saving: false,
            _edit_timer: None,
            _resize_events: resize_events,
            _select_events: Vec::from(select_events),
            edit_revision: 0,
            job: None,
            info_job: None,
            info: None,
            _observe: observe,
        }
    }

    pub(crate) fn set_output_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        self.output_dir = dir;
        cx.notify();
    }

    pub(crate) fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.reset_edits();
        self.edited_preview = None;
        for path in paths {
            let is_image = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    matches!(
                        e.to_ascii_lowercase().as_str(),
                        "jpg"
                            | "jpeg"
                            | "png"
                            | "webp"
                            | "avif"
                            | "heic"
                            | "heif"
                            | "tif"
                            | "tiff"
                            | "bmp"
                    )
                })
                .unwrap_or(false);
            if is_image && !self.files.contains(&path) {
                self.files.push(path);
            }
        }
        if self.selected >= self.files.len() {
            self.selected = 0;
        }
        self.load_info(cx);
        self.editor_source = None;
        if self.mode == Mode::Edit {
            self.ensure_editor(cx);
        }
        cx.notify();
    }

    fn load_info(&mut self, cx: &mut Context<Self>) {
        self.info = None;
        let Some(path) = self.files.get(self.selected).cloned() else {
            return;
        };
        let id = self.engine.call(
            "image.info",
            json!({ "path": path.to_string_lossy().to_string() }),
        );
        self.jobs.update(cx, |jobs, _| {
            jobs.start(id.clone(), Tool::Images, "reading metadata");
        });
        self.info_job = Some(id);
    }

    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Choose images".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
        })
        .detach();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if self.files.is_empty() {
            return;
        }
        let mut args = json!({
            "paths": self.files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
            "preset": self.preset.key(),
            "format": self.format.key(),
            "strip_exif": self.strip_exif,
            "out_dir": self.output_dir.as_ref().map(|p|p.to_string_lossy().to_string()),
        });
        if self.preset == Preset::Target {
            args["target_bytes"] = json!(self.target_kb * 1024);
            args["max_dim"] = json!(3000);
        }
        let id = self.engine.call("image.compress", args);
        self.jobs.update(cx, |jobs, _| {
            jobs.start_export(id.clone(), Tool::Images, "queued");
        });
        self.job = Some(id);
        cx.notify();
    }

    fn reset_edits(&mut self) {
        self.crop = None;
        self.crop_ratio = "free";
        self.crop_start = None;
        self.rotate = 0;
        self.flip_h = false;
        self.flip_v = false;
        self.grayscale = false;
        self.crop_draft = None;
        self.crop_open = false;
        self.resize_open = false;
        self.show_original = false;
        self.clear_resize_inputs = true;
        self.saved_path = None;
        self.save_error = None;
        self.edit_error = None;
    }

    fn dimension(state: &Entity<InputState>, cx: &App) -> u64 {
        state
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
            .clamp(0.0, 20000.0) as u64
    }

    fn total_before(&self) -> u64 {
        self.files
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
            .sum()
    }
}

/// A transparent canvas over the preview. Mouse events are window-relative in
/// GPUI, so the canvas bounds (captured during prepaint) convert them into the
/// normalised coordinates the engine expects. Pointer capture lets a drag end
/// outside the preview.
fn crop_canvas(view: &Entity<ImagesView>) -> Canvas<Hitbox> {
    let down_view = view.downgrade();
    let move_view = down_view.clone();
    let up_view = down_view.clone();
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if !phase.bubble()
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let _ = down_view.update(cx, |this, cx| {
                    let start = normalized(event.position, bounds);
                    this.crop_start = Some((start.x, start.y));
                    this.crop_draft = None;
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
                let _ = move_view.update(cx, |this, cx| {
                    let Some((sx, sy)) = this.crop_start else {
                        return;
                    };
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.crop_start = None;
                        cx.notify();
                        return;
                    }
                    let point = normalized(event.position, bounds);
                    let crop = Crop::from_points(sx, sy, point.x, point.y);
                    if crop.w > 0.005 && crop.h > 0.005 {
                        this.crop_draft = Some(crop);
                    }
                    cx.stop_propagation();
                    cx.notify();
                });
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() || event.button != MouseButton::Left {
                    return;
                }
                let _ = up_view.update(cx, |this, cx| {
                    let Some((sx, sy)) = this.crop_start.take() else {
                        return;
                    };
                    let point = normalized(event.position, bounds);
                    let crop = Crop::from_points(sx, sy, point.x, point.y);
                    this.crop_draft = (crop.w * f64::from(bounds.size.width) >= 3.0
                        && crop.h * f64::from(bounds.size.height) >= 3.0)
                        .then_some(crop);
                    cx.stop_propagation();
                    cx.notify();
                });
            });
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn normalized(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Point<f64> {
    point(
        f64::from((position.x - bounds.left()) / bounds.size.width.max(px(1.))).clamp(0., 1.),
        f64::from((position.y - bounds.top()) / bounds.size.height.max(px(1.))).clamp(0., 1.),
    )
}

impl Render for ImagesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.mode == Mode::Edit {
            return self.render_editor(window, cx).into_any_element();
        }
        let job: Option<Job> = self.jobs.read(cx).get(&self.job).cloned();
        let info_job: Option<Job> = self.jobs.read(cx).get(&self.info_job).cloned();
        if let Some(result) = info_job.as_ref().and_then(|j| j.result())
            && self.info.as_ref() != Some(result)
        {
            self.info = Some(result.clone());
        }
        let busy = job.as_ref().map(Job::is_running).unwrap_or(false);
        let result: Option<Value> = if self.mode == Mode::Compress {
            job.as_ref().and_then(|j| j.result()).cloned()
        } else {
            None
        };
        for (state, label) in [
            (&self.preset_select, self.preset.label()),
            (&self.format_select, self.format.label()),
        ] {
            if state.read(cx).selected_value().map(String::as_str) != Some(label) {
                state.update(cx, |s, cx| {
                    s.set_selected_value(&label.to_string(), window, cx)
                });
            }
        }
        let total_before = self.total_before();
        let count = self.files.len();

        let mut modes = super::segmented(cx).flex_wrap();
        for mode in [Mode::Compress, Mode::Edit] {
            let selected = self.mode == mode;
            let button = super::segment(mode.id(), mode.label(), selected);
            modes = modes.child(button.on_click(cx.listener(move |this, _, _, cx| {
                this.mode = mode;
                this.job = None;
                this.edited_preview = None;
                cx.notify();
            })));
        }
        let mode_card = card(cx)
            .child(field_label("Mode", cx))
            .child(modes)
            .child(note(
                "Edit works on the selected image. Compress processes every file in the list.",
                cx,
            ));

        let compress_controls = div()
            .flex()
            .flex_wrap()
            .gap_4()
            .items_end()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(field_label("Preset", cx))
                    .child(
                        Select::new(&self.preset_select)
                            .w(px(210.))
                            .h(px(36.))
                            .disabled(busy),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(field_label("Format", cx))
                    .child(
                        Select::new(&self.format_select)
                            .w(px(160.))
                            .h(px(36.))
                            .disabled(busy),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(field_label("Target size", cx))
                    .child(
                        Select::new(&self.target_select)
                            .w(px(160.))
                            .disabled(self.preset != Preset::Target || busy),
                    ),
            );
        let output_note = self
            .output_dir
            .as_ref()
            .map(|path| format!("Saving to {}", path.display()))
            .unwrap_or_else(|| {
                "Saves new copies beside the originals. Choose an output folder to change this."
                    .to_string()
            });

        let mut file_list = div()
            .id("image-files")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(220.))
            .overflow_y_scroll();
        for path in self.files.iter().take(300) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let detail = if size > 0 {
                human_bytes(size)
            } else {
                "missing".to_string()
            };
            file_list = file_list.child(file_row(&name, detail, cx));
        }

        let mut thumbs = div()
            .id("image-thumbnails")
            .flex()
            .gap_2()
            .min_w_0()
            .overflow_x_scroll()
            .pb_2();
        for (index, path) in self.files.iter().take(40).enumerate() {
            let selected = index == self.selected;
            let mut cell = div()
                .id(SharedString::from(format!("thumb-{index}")))
                .w(px(96.))
                .flex_shrink_0()
                .h(px(76.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_2()
                .border_color(if selected {
                    cx.theme().primary
                } else {
                    cx.theme().border
                })
                .bg(cx.theme().muted.opacity(0.25))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = index;
                    this.reset_edits();
                    this.edited_preview = None;
                    this.load_info(cx);
                    cx.notify();
                }));
            cell = cell.child(
                img(path.clone())
                    .w(px(92.))
                    .h(px(72.))
                    .object_fit(ObjectFit::Contain),
            );
            thumbs = thumbs.child(cell);
        }

        let dims = self.info.as_ref().and_then(|info| {
            let w = info.get("width")?.as_u64()? as f64;
            let h = info.get("height")?.as_u64()? as f64;
            (w > 0.0 && h > 0.0).then_some((w, h))
        });
        let editable = self.mode == Mode::Edit
            && count > 0
            && self.selected < self.files.len()
            && dims.is_some();
        let preview = if count > 0 && self.selected < self.files.len() {
            let path = &self.files[self.selected];
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let stage = if let (true, Some((w, h))) = (editable, dims) {
                let width = panel_width(window)
                    .min(620.)
                    .min(440. * w as f32 / h as f32)
                    .max(1.);
                let height = width * h as f32 / w as f32;
                let mut canvas_box = div()
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        img(path.clone())
                            .w(px(width))
                            .h(px(height))
                            .object_fit(ObjectFit::Fill),
                    );
                if let Some(crop) = self.crop {
                    let x = crop.x as f32 * width;
                    let y = crop.y as f32 * height;
                    let x2 = (crop.x + crop.w) as f32 * width;
                    let y2 = (crop.y + crop.h) as f32 * height;
                    let dim = hsla(0.0, 0.0, 0.05, 0.62);
                    canvas_box = canvas_box
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(width))
                                .h(px(y))
                                .bg(dim),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(y2))
                                .w(px(width))
                                .h(px((height - y2).max(0.)))
                                .bg(dim),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(y))
                                .w(px(x))
                                .h(px((y2 - y).max(0.)))
                                .bg(dim),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(x2))
                                .top(px(y))
                                .w(px((width - x2).max(0.)))
                                .h(px((y2 - y).max(0.)))
                                .bg(dim),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(x))
                                .top(px(y))
                                .w(px((x2 - x).max(1.)))
                                .h(px((y2 - y).max(1.)))
                                .border_2()
                                .border_color(cx.theme().primary),
                        );
                }
                canvas_box.child(crop_canvas(&cx.entity()))
            } else {
                div()
                    .w_full()
                    .h(px(280.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(cx.theme().muted.opacity(0.2))
                    .child(
                        img(path.clone())
                            .w_full()
                            .max_h(px(268.))
                            .object_fit(ObjectFit::Contain),
                    )
            };
            Some(
                card(cx)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .justify_between()
                            .items_center()
                            .child(field_label("Preview", cx))
                            .child(note(name, cx)),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(stage),
                    )
                    .when(editable, |el| {
                        el.child(note(
                            "Drag on the image to choose the crop. Click without dragging for the full image.",
                            cx,
                        ))
                    }),
            )
        } else {
            None
        };

        let mut body = card(cx).child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .justify_between()
                .items_center()
                .child(field_label("Files", cx))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("images-add")
                                .label("Add images…")
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick(window, cx);
                                })),
                        )
                        .child(
                            Button::new("images-clear")
                                .label("Clear")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.files.clear();
                                    this.selected = 0;
                                    this.info = None;
                                    this.reset_edits();
                                    this.edited_preview = None;
                                    cx.notify();
                                })),
                        ),
                ),
        );

        body = body.child(if count == 0 {
            dropzone(
                "Drop images here",
                "or use Add images · JPEG, PNG, WebP, AVIF, HEIC",
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.pick(window, cx)))
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(thumbs)
                .child(file_list)
                .into_any_element()
        });

        if count > 0 {
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .child(muted(
                        format!("{count} file(s) · {}", human_bytes(total_before)),
                        cx,
                    ))
                    .child(pill(
                        if self.strip_exif {
                            "Metadata removed on export"
                        } else {
                            "Metadata kept on export"
                        },
                        if self.strip_exif {
                            cx.theme().success
                        } else {
                            cx.theme().muted_foreground
                        },
                        cx,
                    )),
            );
        }

        if self.mode == Mode::Compress {
            body = body
                .child(field_label("Compression settings", cx))
                .child(compress_controls)
                .child(note(output_note, cx));

            body = body
                .child(super::setting_row(
                    "Remove private metadata",
                    "Strip location, camera details, and EXIF data from exported copies.",
                    Switch::new("strip-exif")
                        .checked(self.strip_exif)
                        .disabled(busy)
                        .on_click(cx.listener(|t, _, _, cx| {
                            t.strip_exif = !t.strip_exif;
                            cx.notify();
                        })),
                    cx,
                ))
                .child(
                    div().flex().justify_end().pt_2().child(
                        Button::new("image-run")
                            .label(if busy { "Compressing…" } else { "Compress" })
                            .primary()
                            .h(px(38.))
                            .disabled(busy || count == 0)
                            .tooltip(if count == 0 {
                                "Add at least one image to compress"
                            } else if busy {
                                "Compression is running"
                            } else {
                                "Save compressed copies"
                            })
                            .on_click(cx.listener(|t, _, _, cx| t.run(cx))),
                    ),
                )
                .child(job_progress(job.as_ref(), cx));

            if let Some(result) = result {
                body = body.child(
                    result_section(cx)
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_8()
                                .child(stat(
                                    "Saved",
                                    result
                                        .get("saved")
                                        .and_then(Value::as_f64)
                                        .map(|s| format!("{:.0}%", s * 100.0))
                                        .unwrap_or_else(|| "—".into()),
                                    cx,
                                ))
                                .child(stat(
                                    "Before",
                                    human_bytes(
                                        result.get("before").and_then(Value::as_u64).unwrap_or(0),
                                    ),
                                    cx,
                                ))
                                .child(stat(
                                    "After",
                                    human_bytes(
                                        result.get("after").and_then(Value::as_u64).unwrap_or(0),
                                    ),
                                    cx,
                                )),
                        )
                        .child(
                            div().flex().flex_col().gap_1().children(
                                result
                                    .get("files")
                                    .and_then(Value::as_array)
                                    .map(|files| {
                                        files
                                            .iter()
                                            .take(300)
                                            .map(|file| {
                                                let dst = file
                                                    .get("dst")
                                                    .and_then(Value::as_str)
                                                    .unwrap_or_default()
                                                    .to_string();
                                                let name = std::path::Path::new(&dst)
                                                    .file_name()
                                                    .map(|n| n.to_string_lossy().to_string())
                                                    .unwrap_or_else(|| dst.clone());
                                                let before = file
                                                    .get("before")
                                                    .and_then(Value::as_u64)
                                                    .unwrap_or(0);
                                                let after = file
                                                    .get("after")
                                                    .and_then(Value::as_u64)
                                                    .unwrap_or(0);
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_2()
                                                    .child(file_row(
                                                        &name,
                                                        format!(
                                                            "{} → {}",
                                                            human_bytes(before),
                                                            human_bytes(after)
                                                        ),
                                                        cx,
                                                    ))
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default(),
                            ),
                        ),
                );
                body = body.child(success_box("Done. Originals were not modified.", cx));
                let folders = super::output_folders(&result);
                let multiple = folders.len() > 1;
                for (index, dir) in folders.into_iter().enumerate() {
                    let label = if multiple {
                        format!(
                            "Show in {}",
                            dir.file_name().unwrap_or_default().to_string_lossy()
                        )
                    } else {
                        "Show in folder".into()
                    };
                    body = body.child(
                        Button::new(SharedString::from(format!("images-reveal-output-{index}")))
                            .small()
                            .ghost()
                            .label(label)
                            .tooltip(dir.to_string_lossy().to_string())
                            .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&dir))),
                    );
                }
            }
        }

        let metadata = self.metadata_panel(cx);

        let mut root = page(window)
            .child(heading(
                "Images",
                "Compress a batch or make simple edits locally. GPS and camera metadata are stripped before anything is written.",
                cx,
            ))
            .child(mode_card)
            .child(body);
        if let Some(preview) = preview {
            root = root.child(preview);
        }
        if let Some(metadata) = metadata {
            root = root.child(metadata);
        }
        root.child(note(
            "Nothing is uploaded. The engine runs on this machine.",
            cx,
        ))
        .into_any_element()
    }
}

impl ImagesView {
    fn metadata_panel(&self, cx: &App) -> Option<Div> {
        if let Some(info) = &self.info {
            let number = |key: &str| info.get(key).and_then(Value::as_u64).unwrap_or(0);
            let text = |key: &str| {
                let value = info.get(key).and_then(Value::as_str).unwrap_or("");
                if value.is_empty() {
                    "—".to_string()
                } else {
                    value.to_string()
                }
            };
            let gps = info.get("gps").and_then(Value::as_object);
            let gps_count = gps.map(|map| map.len()).unwrap_or(0);
            let gps_rows = if gps_count > 0 {
                let mut rows = div().flex().flex_col().gap_1();
                if let Some(map) = gps {
                    for (key, value) in map.iter().take(10) {
                        rows = rows.child(note(
                            format!("{key}: {}", value.as_str().unwrap_or("?")),
                            cx,
                        ));
                    }
                }
                rows
            } else {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No GPS position recorded")
            };
            let clip = |value: &str, limit: usize| -> String {
                if value.chars().count() > limit {
                    format!("{}…", value.chars().take(limit).collect::<String>())
                } else {
                    value.to_string()
                }
            };
            let signals = info.get("signals");
            let c2pa = signals.and_then(|s| s.get("c2pa"));
            let state = c2pa
                .and_then(|c| c.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("unavailable");
            let (signature_label, signature_color) = match state {
                "valid" => ("C2PA signature verified", cx.theme().success),
                "invalid" => ("C2PA signature invalid", cx.theme().danger),
                "none" => ("No C2PA signature", cx.theme().muted_foreground),
                _ => ("C2PA verification unavailable", cx.theme().warning),
            };
            let mut signals_section = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(field_label("AI signals", cx))
                .child(div().flex().flex_wrap().gap_2().items_center().child(pill(
                    signature_label,
                    signature_color,
                    cx,
                )));

            if let Some(generator) = c2pa
                .and_then(|c| c.get("claim_generator"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                signals_section = signals_section.child(pill(
                    format!("Claim generator: {}", clip(generator, 48)),
                    cx.theme().accent,
                    cx,
                ));
            }

            if let Some(tags) = signals
                .and_then(|s| s.get("generator_tags"))
                .and_then(Value::as_array)
            {
                let has_tags = !tags.is_empty();
                signals_section = signals_section.child(pill(
                    if has_tags {
                        "Generator tag found"
                    } else {
                        "No generator tag"
                    },
                    if has_tags {
                        cx.theme().warning
                    } else {
                        cx.theme().muted_foreground
                    },
                    cx,
                ));
                for tag in tags.iter().take(8) {
                    let key = tag
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or("tag")
                        .to_string();
                    let value = tag
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    signals_section =
                        signals_section.child(note(format!("{key}: {}", clip(&value, 260)), cx));
                }
            }

            if let Some(prompt) = signals
                .and_then(|s| s.get("prompt"))
                .and_then(Value::as_str)
            {
                signals_section =
                    signals_section.child(note(format!("Prompt: {}", clip(prompt, 300)), cx));
            }

            if let Some(problem) = c2pa
                .and_then(|c| c.get("problems"))
                .and_then(Value::as_array)
                .and_then(|problems| problems.first())
            {
                let code = problem
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("verification error");
                let detail = problem
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                signals_section =
                    signals_section.child(error_box(&format!("{code}: {}", clip(detail, 220)), cx));
            }

            if let Some(message) = c2pa.and_then(|c| c.get("error")).and_then(Value::as_str) {
                signals_section = signals_section.child(note(clip(message, 220), cx));
            }

            signals_section = signals_section.child(note(
                "Verification is local and checks the signature's integrity. Trust anchors are not fetched offline, and absent metadata proves nothing.",
                cx,
            ));

            Some(
                card(cx)
                    .child(
                        div()
                            .flex().flex_wrap().gap_3()
                            .justify_between()
                            .items_center()
                            .child(field_label("EXIF / metadata", cx))
                            .child(
                                div()
                                    .flex().flex_wrap()
                                    .gap_2()
                                    .items_center()
                                    .child(pill(
                                        format!("{} tags", number("exif_tags")),
                                        cx.theme().info,
                                        cx,
                                    ))
                                    .child(if gps_count > 0 {
                                        pill("GPS found", cx.theme().warning, cx)
                                    } else {
                                        pill("No GPS", cx.theme().muted_foreground, cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_8()
                            .child(stat("Dimensions", format!("{} × {}", number("width"), number("height")), cx))
                            .child(stat("Format", text("format"), cx))
                            .child(stat("Colour", text("mode"), cx))
                            .child(stat("Camera", text("camera"), cx))
                            .child(stat("Taken", text("taken"), cx))
                            .child(stat("File size", human_bytes(number("bytes")), cx)),
                    )
                    .child(field_label("Location", cx))
                    .child(gps_rows)
                    .child(signals_section)
                    .child(note(
                        "This is the source file. Compressed exports are written without this metadata.",
                        cx,
                    )),
            )
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Crop, normalized};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn dragging_in_either_direction_marks_the_same_crop() {
        let a = Crop::from_points(0.2, 0.3, 0.8, 0.9);
        let b = Crop::from_points(0.8, 0.9, 0.2, 0.3);
        assert_eq!((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h));
    }

    #[test]
    fn preset_crops_are_centered() {
        let square = Crop::centered(1.0);
        assert!((square.x - square.y).abs() < 1e-12);
        assert!((square.w - square.h).abs() < 1e-12);
        let wide = Crop::centered(16.0 / 9.0);
        assert!(wide.w > wide.h);
        assert!(wide.x < wide.y);
    }

    #[test]
    fn preview_coordinates_normalize_against_scrolled_bounds() {
        let small = normalized(
            point(px(150.), px(160.)),
            Bounds::new(point(px(50.), px(-40.)), size(px(200.), px(400.))),
        );
        let large = normalized(
            point(px(300.), px(410.)),
            Bounds::new(point(px(100.), px(10.)), size(px(400.), px(800.))),
        );
        assert_eq!(small, large);
        assert_eq!(small, point(0.5, 0.5));
    }
}
