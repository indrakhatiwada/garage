//! Shared offline PDF workspace, with per-document edits and a sequential batch queue.
use super::{card, dropzone, error_box, field_label, heading, job_progress, note, page};
use crate::{
    engine::Engine,
    jobs::{JobState, Jobs, Tool},
};
use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        slider::{Slider, SliderEvent, SliderState},
        switch::Switch,
    },
    *,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
};

struct Entry {
    path: PathBuf,
    settings: Option<Value>,
    job: Option<String>,
    order: Vec<u64>,
    rotations: HashMap<u64, i32>,
    undo: Vec<(Vec<u64>, HashMap<u64, i32>)>,
}
#[derive(Clone)]
struct PageDrag {
    index: usize,
}
impl Render for PageDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_3().child("Move page")
    }
}

pub struct PdfWorkspace {
    engine: Engine,
    jobs: Entity<Jobs>,
    tool: Tool,
    legacy: Option<Entity<super::pages::PagesView>>,
    action: &'static str,
    advanced: bool,
    choices: super::ChoiceStore,
    choice_events: Vec<Subscription>,
    opacity: Entity<SliderState>,
    entries: Vec<Entry>,
    selected: usize,
    fields: HashMap<&'static str, Entity<InputState>>,
    output: Option<PathBuf>,
    error: Option<String>,
    queue: VecDeque<(usize, Value)>,
    active: Option<(usize, String)>,
    info: Option<(usize, String)>,
    thumbs: Vec<(u64, String)>,
    preview: Option<String>,
    preview_timer: Option<Task<()>>,
    revision: u64,
    cache: PathBuf,
    offset: usize,
    _observe: Subscription,
    _inputs: Vec<Subscription>,
}
impl PdfWorkspace {
    pub fn new(
        engine: Engine,
        jobs: Entity<Jobs>,
        tool: Tool,
        window: &mut Window,
        cx: &mut Context<Self>,
        output: Option<PathBuf>,
    ) -> Self {
        let legacy = if tool == Tool::Pages {
            Some(cx.new(|cx| {
                super::pages::PagesView::new(
                    engine.clone(),
                    jobs.clone(),
                    window,
                    cx,
                    output.clone(),
                )
            }))
        } else {
            None
        };
        let mut fields = HashMap::new();
        for (key, default) in [
            ("preview_page", "1"),
            ("pages", ""),
            ("delete", ""),
            ("rotation", "0"),
            ("text", "DRAFT"),
            ("fontsize", "54"),
            ("opacity", "0.16"),
            ("color", "0.35,0.35,0.35"),
            ("position", "center"),
            ("number_start", "1"),
            ("number_style", "arabic"),
            ("show_total", "false"),
            ("diagonal", "true"),
            ("page_size", "fit"),
            ("margin", "0"),
            ("format", "png"),
            ("dpi", "150"),
            ("source_password", ""),
            ("new_password", ""),
            ("confirm_password", ""),
            ("owner_password", ""),
            ("permissions", "-1"),
        ] {
            fields.insert(
                key,
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(if key == "pages" {
                            "All pages"
                        } else if key == "delete" {
                            "e.g. 2-4, 8"
                        } else {
                            ""
                        })
                        .default_value(default)
                        .masked(key.contains("password"))
                }),
            );
        }
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(16.)
        });
        let mut inputs: Vec<Subscription> = fields
            .values()
            .map(|field| {
                cx.subscribe_in(field, window, |this, _, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        if let Some(id) = this.preview.take() {
                            this.engine.cancel(&id);
                        }
                        this.schedule_preview(cx);
                        cx.notify();
                    }
                })
            })
            .collect();
        inputs.push(
            cx.subscribe_in(&opacity, window, |this, _, event, window, cx| {
                if let SliderEvent::Change(value) = event {
                    this.fields["opacity"].update(cx, |field, cx| {
                        field.set_value(format!("{}", value.start() / 100.), window, cx)
                    });
                    if let Some(id) = this.preview.take() {
                        this.engine.cancel(&id);
                    }
                    this.schedule_preview(cx);
                    cx.notify();
                }
            }),
        );
        let observe = cx.observe(&jobs, |this, _, cx| {
            this.pump(cx);
            cx.notify();
        });
        let cache =
            std::env::temp_dir().join(format!("garage-pdf-{}-{:?}", std::process::id(), tool));
        let _ = std::fs::create_dir_all(&cache);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700));
        }
        Self {
            engine,
            jobs,
            tool,
            legacy,
            action: if tool == Tool::Pages {
                "legacy"
            } else {
                "watermark"
            },
            advanced: false,
            choices: super::ChoiceStore::new(),
            choice_events: vec![],
            opacity,
            entries: vec![],
            selected: 0,
            fields,
            output,
            error: None,
            queue: VecDeque::new(),
            active: None,
            info: None,
            thumbs: vec![],
            preview: None,
            preview_timer: None,
            revision: 0,
            cache,
            offset: 0,
            _observe: observe,
            _inputs: inputs,
        }
    }
    pub fn set_output_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        self.output = dir.clone();
        if let Some(v) = &self.legacy {
            v.update(cx, |v, cx| v.set_output_dir(dir, cx));
        }
        cx.notify();
    }
    pub fn accept_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.action == "legacy" {
            if let Some(v) = &self.legacy {
                v.update(cx, |v, cx| v.accept_paths(paths, cx));
            }
            return;
        }
        if self.active.is_some() {
            return;
        }
        for path in paths {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let valid = if self.action == "images" {
                [
                    "jpg", "jpeg", "png", "webp", "avif", "heic", "heif", "tif", "tiff", "bmp",
                ]
                .contains(&ext.as_str())
            } else {
                ext == "pdf"
            };
            if valid && !self.entries.iter().any(|e| e.path == path) {
                self.entries.push(Entry {
                    path,
                    settings: None,
                    job: None,
                    order: vec![],
                    rotations: HashMap::new(),
                    undo: vec![],
                });
            }
        }
        self.load(cx);
        cx.notify();
    }
    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Choose input files".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| this.accept_paths(paths, cx));
            }
        })
        .detach();
    }
    fn settings(&self, cx: &App) -> Result<Value, String> {
        let mut a = json!({"action":self.action});
        for (key, field) in &self.fields {
            a[*key] = json!(field.read(cx).value().to_string());
        }
        for key in ["fontsize", "opacity", "margin"] {
            a[key] = json!(
                a[key]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .map_err(|_| format!("Enter a number for {key}"))?
            );
        }
        for key in ["number_start", "dpi", "permissions"] {
            a[key] = json!(
                a[key]
                    .as_str()
                    .unwrap()
                    .parse::<i64>()
                    .map_err(|_| format!("Enter a whole number for {key}"))?
            );
        }
        for key in ["show_total", "diagonal"] {
            a[key] = json!(a[key].as_str() == Some("true"));
        }
        let color: Vec<f64> = a["color"]
            .as_str()
            .unwrap()
            .split(',')
            .map(|n| n.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| "Color must be three numbers, e.g. 0.35,0.35,0.35")?;
        a["color"] = json!(color);
        if self.action == "protect" && a["new_password"] != a["confirm_password"] {
            return Err("New passwords do not match".into());
        }
        a.as_object_mut().unwrap().remove("confirm_password");
        Ok(a)
    }
    fn effective(&self, i: usize, shared: &Value) -> Value {
        let mut a = self.entries[i]
            .settings
            .clone()
            .unwrap_or_else(|| shared.clone());
        a["action"] = json!(self.action);
        a["src"] = json!(self.entries[i].path);
        a["out_dir"] = json!(self.output.clone().unwrap_or_else(|| {
            self.entries[i]
                .path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        }));
        if self.action == "organize" {
            let e = &self.entries[i];
            if !e.undo.is_empty() {
                a["order"] = json!(e.order);
                a["rotate"] = json!(e.rotations);
            } else {
                let angle = a["rotation"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<i32>()
                    .unwrap_or(0);
                if angle != 0 {
                    a["shared_rotation"] = json!(angle);
                }
            }
        }
        a
    }
    fn start(&mut self, retry: bool, cx: &mut Context<Self>) {
        if self.active.is_some() {
            return;
        }
        let shared = match self.settings(cx) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.error = None;
        if self.action == "images" {
            if self.entries.is_empty() {
                return;
            }
            let mut a = self.effective(0, &shared);
            a["srcs"] = json!(self.entries.iter().map(|e| &e.path).collect::<Vec<_>>());
            self.queue.push_back((0, a));
        } else {
            for i in 0..self.entries.len() {
                let failed = self.entries[i]
                    .job
                    .as_ref()
                    .and_then(|id| self.jobs.read(cx).get(&Some(id.clone())))
                    .is_some_and(|j| matches!(j.state, JobState::Failed(_)));
                if !retry || failed {
                    self.queue.push_back((i, self.effective(i, &shared)));
                }
            }
        }
        self.pump(cx);
        cx.notify();
    }
    fn pump(&mut self, cx: &mut Context<Self>) {
        if let Some((_, id)) = &self.active {
            if self
                .jobs
                .read(cx)
                .get(&Some(id.clone()))
                .is_some_and(|j| j.is_running())
            {
                return;
            }
            self.active = None;
        }
        if let Some((i, args)) = self.queue.pop_front() {
            let id = self.call("pdf.workspace", args, cx);
            self.entries[i].job = Some(id.clone());
            self.active = Some((i, id));
        }
        if let Some((i, id)) = self.info.clone() {
            let result = self
                .jobs
                .read(cx)
                .get(&Some(id.clone()))
                .and_then(|j| j.result())
                .cloned();
            if let Some(result) = result {
                self.info = None;
                if let Some(e) = self.entries.get_mut(i) {
                    if e.order.is_empty() {
                        e.order = (1..=result["pages"].as_u64().unwrap_or(0)).collect();
                    }
                }
                if matches!(self.action, "watermark" | "numbers") {
                    self.refresh_preview(cx);
                } else if self.action == "organize" {
                    self.thumbnails(cx);
                }
            }
        }
    }
    fn call(&self, op: &str, args: Value, cx: &mut Context<Self>) -> String {
        let exports_file = op == "pdf.workspace" && args["preview"] != true;
        let id = self.engine.call(op, args);
        self.jobs.update(cx, |jobs, _| {
            if exports_file {
                jobs.start_export(id.clone(), self.tool, "Queued");
            } else {
                jobs.start(id.clone(), self.tool, "Queued");
            }
        });
        id
    }
    fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.action, "watermark" | "numbers") || self.entries.is_empty() {
            return;
        }
        if let Some(id) = self.preview.take() {
            self.engine.cancel(&id);
        }
        self.preview_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |t, cx| {
                t.refresh_preview(cx);
                cx.notify();
            });
        }));
    }
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_timer = None;
        if self.entries.is_empty() || !matches!(self.action, "watermark" | "numbers") {
            return;
        }
        if let Some(id) = self.preview.take() {
            self.engine.cancel(&id);
        }
        match self.settings(cx) {
            Ok(settings) => {
                let mut args = self.effective(self.selected, &settings);
                let count = self.entries[self.selected].order.len().max(1) as u64;
                let page = self.fields["preview_page"]
                    .read(cx)
                    .value()
                    .parse::<u64>()
                    .unwrap_or(1)
                    .clamp(1, count);
                args["preview"] = json!(true);
                args["preview_page"] = json!(page);
                args["out_dir"] = json!(self.cache);
                self.error = None;
                self.preview = Some(self.call("pdf.workspace", args, cx));
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.preview_timer = None;
        if let Some(id) = self.preview.take() {
            self.engine.cancel(&id);
        }
        if let Some((_, id)) = self.info.take() {
            self.engine.cancel(&id);
        }
        for (_, id) in self.thumbs.drain(..) {
            self.engine.cancel(&id);
        }
        self.offset = 0;
        if self.action == "images" || self.entries.is_empty() {
            return;
        }
        if let Ok(shared) = self.settings(cx) {
            let a = self.effective(self.selected, &shared);
            let id = self.call(
                "pdf.inspect",
                json!({"src":a["src"],"password":a["source_password"]}),
                cx,
            );
            self.info = Some((self.selected, id));
        }
    }
    fn thumbnails(&mut self, cx: &mut Context<Self>) {
        for (_, id) in self.thumbs.drain(..) {
            self.engine.cancel(&id);
        }
        if self.entries.is_empty() {
            return;
        }
        let Ok(shared) = self.settings(cx) else {
            return;
        };
        let a = self.effective(self.selected, &shared);
        let pages: Vec<u64> = self.entries[self.selected]
            .order
            .iter()
            .skip(self.offset)
            .take(12)
            .copied()
            .collect();
        for p in pages {
            self.revision += 1;
            let dst = self.cache.join(format!("{}-{p}.png", self.revision));
            let id = self.call(
                "pdf.render",
                json!({"src":a["src"],"password":a["source_password"],"page":p,"dpi":40,"dst":dst,"rotation":self.entries[self.selected].rotations.get(&p).copied().unwrap_or(0)}),
                cx,
            );
            self.thumbs.push((p, id));
        }
    }
    fn edit(&mut self, index: usize, command: &str, cx: &mut Context<Self>) {
        let e = &mut self.entries[self.selected];
        e.undo.push((e.order.clone(), e.rotations.clone()));
        match command {
            "left" if index > 0 => e.order.swap(index, index - 1),
            "right" if index + 1 < e.order.len() => e.order.swap(index, index + 1),
            "rotate" => {
                let p = e.order[index];
                *e.rotations.entry(p).or_default() += 90;
            }
            "delete" if e.order.len() > 1 => {
                e.order.remove(index);
            }
            _ => {
                e.undo.pop();
            }
        }
        self.thumbnails(cx);
        cx.notify();
    }
    pub fn current_action(&self) -> &'static str {
        self.action
    }
    pub fn select_action(
        &mut self,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active.is_none() && self.action != action {
            self.switch(action, window, cx);
        }
    }
    fn switch(&mut self, action: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.action == "images" || action == "images" {
            self.entries.clear();
            self.selected = 0;
        }
        self.action = action;
        self.error = None;
        for e in &mut self.entries {
            e.settings = None;
        }
        for (key, value) in match action {
            "numbers" => vec![
                ("text", "{page}"),
                ("fontsize", "12"),
                ("opacity", "1"),
                ("position", "bottom-center"),
                ("diagonal", "false"),
            ],
            "watermark" => vec![
                ("text", "DRAFT"),
                ("fontsize", "54"),
                ("opacity", "0.16"),
                ("position", "center"),
                ("diagonal", "true"),
            ],
            _ => vec![],
        } {
            self.fields[key].update(cx, |s, cx| s.set_value(value, window, cx));
        }
        self.load(cx);
        cx.notify();
    }
}
impl Drop for PdfWorkspace {
    fn drop(&mut self) {
        if let Some((_, id)) = &self.active {
            self.engine.cancel(id);
        }
        for (_, id) in &self.thumbs {
            self.engine.cancel(id);
        }
        let _ = std::fs::remove_dir_all(&self.cache);
    }
}
impl Render for PdfWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.active.is_some();
        let actions = if self.tool == Tool::Pages {
            vec![
                ("legacy", "Merge / Split / Extract"),
                ("organize", "Organize"),
                ("images", "Images → PDF"),
                ("export", "PDF → Images"),
            ]
        } else {
            vec![
                ("watermark", "Watermark"),
                ("numbers", "Page numbers"),
                ("protect", "Protect"),
                ("unlock", "Unlock"),
            ]
        };
        let mut tabs = div().flex().flex_wrap().gap_2();
        for (action, label) in actions.into_iter().filter(|(a, _)| {
            matches!(self.action, "protect" | "unlock") && matches!(*a, "protect" | "unlock")
        }) {
            let b = Button::new(action).label(label).small().disabled(busy);
            let b = if self.action == action {
                b.primary()
            } else {
                b.secondary()
            };
            tabs = tabs
                .child(b.on_click(cx.listener(move |this, _, w, cx| this.switch(action, w, cx))));
        }
        let mut root = page(window)
            .child(heading(
                match self.action {
                    "organize" => "Organize pages",
                    "images" => "Images to PDF",
                    "export" => "PDF to images",
                    "watermark" => "Watermarks",
                    "numbers" => "Page numbers",
                    "protect" | "unlock" => "PDF passwords",
                    _ => "PDF pages",
                },
                "Prepare documents locally. Every export creates a new file.",
                cx,
            ))
            .child(tabs);
        if self.action == "legacy" {
            return self.legacy.as_ref().unwrap().clone().into_any_element();
        }
        let mut body =
            card(cx).child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("add")
                            .label("Add files…")
                            .disabled(busy)
                            .on_click(cx.listener(|t, _, w, cx| t.pick(w, cx))),
                    )
                    .child(Button::new("clear").label("Clear").disabled(busy).on_click(
                        cx.listener(|t, _, _, cx| {
                            t.entries.clear();
                            t.selected = 0;
                            t.thumbs.clear();
                            t.preview = None;
                            t.info = None;
                            cx.notify();
                        }),
                    )),
            );
        if busy {
            body = body.child(note(
                format!("Processing one document · {} queued", self.queue.len()),
                cx,
            ));
        }
        if self.entries.is_empty() {
            body = body.child(
                dropzone(
                    if self.action == "images" {
                        "Drop images here"
                    } else {
                        "Drop PDFs here"
                    },
                    if self.action == "images" {
                        "JPEG, PNG, WebP, AVIF, HEIC and TIFF"
                    } else {
                        "PDF documents · add multiple files for batch processing"
                    },
                    cx,
                )
                .on_click(cx.listener(|t, _, w, cx| t.pick(w, cx))),
            );
        }
        for i in 0..self.entries.len() {
            let e = &self.entries[i];
            let name = e
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut row = div().flex().flex_wrap().gap_2().items_center().child(
                Button::new(SharedString::from(format!("file-{i}")))
                    .label(format!(
                        "{}{}",
                        name,
                        if e.settings.is_some() {
                            " · custom settings"
                        } else {
                            ""
                        }
                    ))
                    .disabled(busy)
                    .on_click(cx.listener(move |t, _, _, cx| {
                        t.selected = i;
                        t.load(cx);
                        cx.notify();
                    })),
            );
            if self.action == "images" {
                row = row.child(
                    img(e.path.clone())
                        .w(px(64.))
                        .h(px(64.))
                        .object_fit(ObjectFit::Contain),
                );
                for (label, delta) in [("Move up", -1isize), ("Move down", 1)] {
                    row = row.child(
                        Button::new(SharedString::from(format!("move-{i}-{delta}")))
                            .label(label)
                            .small()
                            .disabled(
                                busy || (delta < 0 && i == 0)
                                    || (delta > 0 && i + 1 == self.entries.len()),
                            )
                            .on_click(cx.listener(move |t, _, _, cx| {
                                t.entries.swap(i, (i as isize + delta) as usize);
                                cx.notify();
                            })),
                    );
                }
            }
            row = row.child(
                Button::new(SharedString::from(format!("remove-{i}")))
                    .label("Remove")
                    .small()
                    .disabled(busy)
                    .on_click(cx.listener(move |t, _, _, cx| {
                        t.entries.remove(i);
                        t.selected = 0;
                        t.load(cx);
                        cx.notify();
                    })),
            );
            let job = self.jobs.read(cx).get(&e.job).cloned();
            if let Some(j) = &job {
                row = row.child(job_progress(Some(j), cx));
                if let Some(r) = j.result() {
                    if let Some(dst) = r["dst"].as_str() {
                        let path = PathBuf::from(dst);
                        let folder = r["dir"]
                            .as_str()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| path.parent().unwrap().to_path_buf());
                        row = row
                            .child(
                                Button::new(SharedString::from(format!("open-{i}")))
                                    .label("Open file")
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_with_system(&path)),
                            )
                            .child(
                                Button::new(SharedString::from(format!("folder-{i}")))
                                    .label("Open folder")
                                    .small()
                                    .on_click(move |_, _, cx| cx.open_with_system(&folder)),
                            );
                    }
                }
            }
            body = body.child(row);
        }
        let mut documents = body
            .min_h(px(420.))
            .flex_1()
            .min_w_0()
            .bg(cx.theme().muted.opacity(0.35));
        let mut body = card(cx).w(px(320.)).flex_shrink_0().child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .pb_3()
                .border_b_1()
                .border_color(cx.theme().border)
                .child("Export settings"),
        );
        let keys: Vec<(&str, &str)> = match self.action {
            "images" => vec![("page_size", "Page size"), ("margin", "Margin (points)")],
            "export" => vec![
                ("pages", "Pages"),
                ("format", "Image format"),
                ("dpi", "Resolution (DPI)"),
                ("source_password", "Existing password"),
            ],
            "organize" => vec![
                ("delete", "Shared deletion ranges"),
                ("rotation", "Rotate all pages"),
                ("source_password", "Existing password"),
            ],
            "watermark" | "numbers" => {
                let mut k = vec![
                    ("pages", "Pages"),
                    ("text", "Text"),
                    ("fontsize", "Font size (points)"),
                    ("opacity", "Opacity"),
                    ("color", "Ink color"),
                    ("position", "Placement"),
                    ("source_password", "Existing password"),
                ];
                if self.action == "numbers" {
                    k.extend([
                        ("number_start", "Starting number"),
                        ("number_style", "Number style"),
                        ("show_total", "Include total pages"),
                    ]);
                } else {
                    k.push(("diagonal", "Diagonal watermark"));
                }
                k
            }
            "protect" => vec![
                ("source_password", "Existing password"),
                ("new_password", "New password (at least 4 characters)"),
                ("confirm_password", "Confirm new password"),
                ("owner_password", "Owner password (optional)"),
            ],
            _ => vec![("source_password", "Existing password")],
        };
        let mut form = div().flex().flex_col().gap_4();
        for (key, label) in keys {
            if !self.advanced
                && (["color", "fontsize", "owner_password"].contains(&key)
                    || (key == "source_password" && !matches!(self.action, "protect" | "unlock")))
            {
                continue;
            }
            let mut field = div()
                .flex()
                .flex_col()
                .gap_2()
                .w_full()
                .child(field_label(label, cx));
            let field_key = self.fields.keys().find(|k| **k == key).copied().unwrap();
            let current = self.fields[key].read(cx).value().to_string();
            let options = match key {
                "page_size" => vec![
                    ("fit", "Fit image"),
                    ("a4", "A4"),
                    ("letter", "Letter"),
                    ("legal", "Legal"),
                ],
                "format" => vec![("png", "PNG — lossless"), ("jpeg", "JPEG — smaller files")],
                "dpi" => vec![
                    ("72", "72 DPI — screen"),
                    ("150", "150 DPI — balanced"),
                    ("300", "300 DPI — print"),
                ],
                "rotation" => vec![
                    ("0", "No rotation"),
                    ("90", "90° clockwise"),
                    ("180", "180°"),
                    ("270", "90° counterclockwise"),
                ],
                "number_style" => vec![
                    ("arabic", "Arabic · 1, 2, 3"),
                    ("roman", "Roman · i, ii, iii"),
                ],
                "position" => vec![
                    ("top-left", "Top left"),
                    ("top-center", "Top center"),
                    ("top-right", "Top right"),
                    ("middle-left", "Middle left"),
                    ("center", "Center"),
                    ("middle-right", "Middle right"),
                    ("bottom-left", "Bottom left"),
                    ("bottom-center", "Bottom center"),
                    ("bottom-right", "Bottom right"),
                ],
                "color" => vec![
                    ("0.35,0.35,0.35", "Graphite"),
                    ("0,0,0", "Black"),
                    ("0.2,0.36,1", "Blue"),
                    ("0.8,0.15,0.15", "Red"),
                ],
                _ => vec![],
            };
            if key == "opacity" {
                let percent = current.parse::<f32>().unwrap_or(0.16) * 100.;
                if (self.opacity.read(cx).value().start() - percent).abs() > 0.1 {
                    self.opacity
                        .update(cx, |s, cx| s.set_value(percent, window, cx));
                }
                field = field.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .py_2()
                                .child(Slider::new(&self.opacity).disabled(busy)),
                        )
                        .child(
                            div()
                                .w(px(42.))
                                .text_right()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{percent:.0}%")),
                        ),
                );
            } else if ["show_total", "diagonal"].contains(&key) {
                let checked = current == "true";
                field = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .w_full()
                    .child(field_label(label, cx))
                    .child(
                        Switch::new(SharedString::from(format!("switch-{key}")))
                            .checked(checked)
                            .disabled(busy)
                            .on_click(cx.listener(move |t, _, w, cx| {
                                t.fields[field_key].update(cx, |s, cx| {
                                    s.set_value(if checked { "false" } else { "true" }, w, cx)
                                });
                                t.schedule_preview(cx);
                                cx.notify();
                            })),
                    );
            } else if options.is_empty() {
                field = field.child(Input::new(&self.fields[key]).h(px(36.)).disabled(busy));
            } else {
                field = field.child(super::choice(
                    &mut self.choices,
                    &mut self.choice_events,
                    field_key,
                    options,
                    &current,
                    busy,
                    window,
                    cx,
                    move |t, value, w, cx| {
                        t.fields[field_key]
                            .update(cx, |s, cx| s.set_value(value.to_string(), w, cx));
                        if let Some(id) = t.preview.take() {
                            t.engine.cancel(&id);
                        }
                        t.schedule_preview(cx);
                        cx.notify();
                    },
                ));
            }
            form = form.child(field);
        }
        body = body.child(form).child(
            Button::new("advanced-settings")
                .label(if self.advanced {
                    "Hide advanced settings"
                } else {
                    "Advanced settings"
                })
                .secondary()
                .w_full()
                .h(px(34.))
                .on_click(cx.listener(|t, _, _, cx| {
                    t.advanced = !t.advanced;
                    cx.notify();
                })),
        );

        if self.action == "protect" {
            let mask = self.fields["permissions"]
                .read(cx)
                .value()
                .parse::<i64>()
                .unwrap_or(-1);
            let mut permissions = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(field_label("Allowed actions", cx));
            for (label, bit) in [
                ("Printing", 4i64 | 2048),
                ("Copying", 16),
                ("Annotation", 32),
                ("Editing", 8 | 256 | 1024),
            ] {
                let enabled = mask & bit == bit;
                permissions = permissions.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(note(label, cx))
                        .child(
                            Switch::new(SharedString::from(format!("perm-{bit}")))
                                .checked(enabled)
                                .disabled(busy)
                                .on_click(cx.listener(move |t, _, w, cx| {
                                    let value = if enabled { mask & !bit } else { mask | bit };
                                    t.fields["permissions"]
                                        .update(cx, |s, cx| s.set_value(value.to_string(), w, cx));
                                    cx.notify();
                                })),
                        ),
                );
            }
            body = body.child(permissions).child(note(
                "AES-256 encryption. Permission support varies by PDF viewer.",
                cx,
            ));
        }
        if !self.entries.is_empty() {
            let name = self.entries[self.selected]
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            body = body
                .child(field_label(&format!("Selected: {name}"), cx))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("override")
                                .label("Apply to this file")
                                .disabled(busy)
                                .on_click(cx.listener(|t, _, _, cx| {
                                    match t.settings(cx) {
                                        Ok(a) => {
                                            t.entries[t.selected].settings = Some(a);
                                            t.error = None;
                                        }
                                        Err(e) => t.error = Some(e),
                                    }
                                    t.load(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("reset-override")
                                .label("Reset override")
                                .disabled(busy)
                                .on_click(cx.listener(|t, _, _, cx| {
                                    t.entries[t.selected].settings = None;
                                    t.load(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("reload")
                                .label("Reload document")
                                .disabled(busy)
                                .on_click(cx.listener(|t, _, _, cx| t.load(cx))),
                        ),
                );
            if let Some((_, id)) = &self.info {
                body = body.child(job_progress(self.jobs.read(cx).get(&Some(id.clone())), cx));
            }
            if self.action == "organize" {
                let mut controls = div().flex().gap_2();
                controls =
                    controls
                        .child(Button::new("undo").label("Undo").disabled(busy).on_click(
                            cx.listener(|t, _, _, cx| {
                                let e = &mut t.entries[t.selected];
                                if let Some((o, r)) = e.undo.pop() {
                                    e.order = o;
                                    e.rotations = r;
                                }
                                t.thumbnails(cx);
                                cx.notify();
                            }),
                        ))
                        .child(
                            Button::new("reset-pages")
                                .label("Reset pages")
                                .disabled(busy)
                                .on_click(cx.listener(|t, _, _, cx| {
                                    let e = &mut t.entries[t.selected];
                                    e.order.clear();
                                    e.rotations.clear();
                                    e.undo.clear();
                                    t.load(cx);
                                    cx.notify();
                                })),
                        );
                documents = documents.child(controls);
                let mut grid = div().flex().flex_wrap().gap_3();
                for (p, id) in self.thumbs.clone() {
                    let index = self.entries[self.selected]
                        .order
                        .iter()
                        .position(|v| *v == p)
                        .unwrap_or(0);
                    let mut tile = div()
                        .id(SharedString::from(format!("thumb-{p}")))
                        .w(px(160.))
                        .p_2()
                        .border_1()
                        .rounded_lg()
                        .child(format!(
                            "Page {p} · +{}°",
                            self.entries[self.selected].rotations.get(&p).unwrap_or(&0)
                        ));
                    if let Some(dst) = self
                        .jobs
                        .read(cx)
                        .get(&Some(id))
                        .and_then(|j| j.result())
                        .and_then(|r| r["dst"].as_str())
                    {
                        tile = tile.child(
                            img(PathBuf::from(dst))
                                .w(px(140.))
                                .h(px(180.))
                                .object_fit(ObjectFit::Contain),
                        );
                    }
                    let mut buttons = div().flex().flex_wrap().gap_1();
                    for (cmd, label) in [
                        ("left", "Earlier"),
                        ("right", "Later"),
                        ("rotate", "Rotate"),
                        ("delete", "Delete"),
                    ] {
                        buttons = buttons.child(
                            Button::new(SharedString::from(format!("{cmd}-{p}")))
                                .label(label)
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(move |t, _, _, cx| t.edit(index, cmd, cx))),
                        );
                    }
                    tile = tile
                        .child(buttons)
                        .on_drag(PageDrag { index }, |drag, _, _, cx| {
                            cx.new(|_| drag.clone())
                        })
                        .on_drop(cx.listener(move |t, drag: &PageDrag, _, cx| {
                            if t.active.is_none() {
                                let e = &mut t.entries[t.selected];
                                if drag.index < e.order.len() {
                                    e.undo.push((e.order.clone(), e.rotations.clone()));
                                    let p = e.order.remove(drag.index);
                                    e.order.insert(index, p);
                                    t.thumbnails(cx);
                                    cx.notify();
                                }
                            }
                        }));
                    grid = grid.child(tile);
                }
                documents = documents.child(grid).child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("prev-thumbs")
                                .label("Previous pages")
                                .disabled(busy || self.offset == 0)
                                .on_click(cx.listener(|t, _, _, cx| {
                                    t.offset = t.offset.saturating_sub(12);
                                    t.thumbnails(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("next-thumbs")
                                .label("Next pages")
                                .disabled(
                                    busy || self.offset + 12
                                        >= self.entries[self.selected].order.len(),
                                )
                                .on_click(cx.listener(|t, _, _, cx| {
                                    t.offset += 12;
                                    t.thumbnails(cx);
                                    cx.notify();
                                })),
                        ),
                );
            }
            if matches!(self.action, "watermark" | "numbers") {
                let count = self.entries[self.selected].order.len().max(1) as u64;
                let current = self.fields["preview_page"]
                    .read(cx)
                    .value()
                    .parse::<u64>()
                    .unwrap_or(1)
                    .clamp(1, count);
                documents = documents.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(field_label("Document preview", cx))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new("preview-previous")
                                        .label("Previous")
                                        .small()
                                        .disabled(current <= 1)
                                        .on_click(cx.listener(move |t, _, w, cx| {
                                            t.fields["preview_page"].update(cx, |s, cx| {
                                                s.set_value((current - 1).to_string(), w, cx)
                                            });
                                            t.schedule_preview(cx);
                                            cx.notify();
                                        })),
                                )
                                .child(note(format!("Page {current} of {count}"), cx))
                                .child(
                                    Button::new("preview-next")
                                        .label("Next")
                                        .small()
                                        .disabled(current >= count)
                                        .on_click(cx.listener(move |t, _, w, cx| {
                                            t.fields["preview_page"].update(cx, |s, cx| {
                                                s.set_value((current + 1).to_string(), w, cx)
                                            });
                                            t.schedule_preview(cx);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("preview-refresh")
                                        .label("Refresh")
                                        .small()
                                        .on_click(cx.listener(|t, _, _, cx| {
                                            t.refresh_preview(cx);
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
                if let Some(id) = &self.preview {
                    documents = documents
                        .child(job_progress(self.jobs.read(cx).get(&Some(id.clone())), cx));
                    if let Some(dst) = self
                        .jobs
                        .read(cx)
                        .get(&Some(id.clone()))
                        .and_then(|j| j.result())
                        .and_then(|r| r["dst"].as_str())
                    {
                        let height =
                            (f32::from(window.viewport_size().height) - 330.).clamp(300., 700.);
                        documents = documents.child(
                            img(PathBuf::from(dst))
                                .w_full()
                                .h(px(height))
                                .object_fit(ObjectFit::Contain),
                        );
                    }
                } else if self.preview_timer.is_some() {
                    documents = documents.child(note("Updating preview…", cx));
                } else if let Some((_, id)) = &self.info {
                    documents = documents
                        .child(job_progress(self.jobs.read(cx).get(&Some(id.clone())), cx));
                }
            }
        }
        if let Some(error) = &self.error {
            body = body.child(error_box(error, cx));
        }
        let failed = self.entries.iter().any(|e| {
            self.jobs
                .read(cx)
                .get(&e.job)
                .is_some_and(|j| matches!(j.state, JobState::Failed(_)))
        });
        let mut actions = div()
            .flex()
            .flex_col()
            .gap_2()
            .pt_4()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("run")
                    .label(if busy { "Exporting…" } else { "Export" })
                    .primary()
                    .w_full()
                    .h(px(38.))
                    .disabled(busy || self.entries.is_empty())
                    .on_click(cx.listener(|t, _, _, cx| t.start(false, cx))),
            );
        if busy {
            actions = actions.child(
                Button::new("cancel")
                    .label("Cancel batch")
                    .w_full()
                    .h(px(34.))
                    .on_click(cx.listener(|t, _, _, cx| {
                        t.queue.clear();
                        if let Some((_, id)) = &t.active {
                            t.engine.cancel(id);
                        }
                        cx.notify();
                    })),
            );
        } else if failed {
            actions = actions.child(
                Button::new("retry")
                    .label("Retry failed files")
                    .w_full()
                    .h(px(34.))
                    .on_click(cx.listener(|t, _, _, cx| t.start(true, cx))),
            );
        }
        body = body.child(actions);
        let workbench = if window.viewport_size().width < px(1100.) {
            div()
                .flex()
                .flex_col()
                .gap_5()
                .child(documents)
                .child(body.w_full())
        } else {
            div()
                .flex()
                .items_start()
                .gap_5()
                .child(documents)
                .child(body)
        };
        root = root.child(workbench);
        root.into_any_element()
    }
}
