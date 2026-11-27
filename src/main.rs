//! Garage — a local-first document toolkit.
//!
//! The Rust side owns the window, the queue, and the calendar. Every byte of
//! real work happens in a Python child process over stdin/stdout. No socket is
//! opened, no request leaves the machine.

// Several helpers are part of the engine/calendar API surface or are used by
// unit tests rather than the binary itself; keep the build output quiet.
#![allow(dead_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod brand;
mod bs;
mod engine;
mod jobs;
mod success;
mod views;

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{ActiveTheme, Icon, Selectable, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use engine::Engine;
use jobs::{Jobs, Tool};
use views::cleanup::CleanupView;
use views::dates::DatesView;
use views::images::ImagesView;
use views::ocr::OcrView;
use views::pdf::PdfView;
use views::pdf_workspace::PdfWorkspace;
use views::redact::RedactView;

const TOOLS: [Tool; 8] = [
    Tool::Images,
    Tool::Pdf,
    Tool::Pages,
    Tool::PdfTools,
    Tool::Dates,
    Tool::Ocr,
    Tool::Redact,
    Tool::Cleanup,
];

struct Views {
    images: Entity<ImagesView>,
    pdf: Entity<PdfView>,
    pages: Entity<PdfWorkspace>,
    pdf_tools: Entity<PdfWorkspace>,
    dates: Entity<DatesView>,
    ocr: Entity<OcrView>,
    redact: Entity<RedactView>,
    cleanup: Entity<CleanupView>,
}

pub struct GarageApp {
    tool: Tool,
    engine: Option<Engine>,
    error: Option<String>,
    jobs: Entity<Jobs>,
    views: Option<Views>,
    output_dir: Option<PathBuf>,
    pdf_expanded: bool,
    _events: Option<Task<()>>,
    _success: Entity<success::SuccessFeedback>,
}

impl GarageApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let jobs = cx.new(|_| Jobs::default());
        let success = cx.new(|cx| success::SuccessFeedback::new(&jobs, window, cx));
        let (engine, error, events) = match Engine::spawn() {
            Ok(engine) => {
                let rx = engine.events();
                let events = cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = rx.recv().await {
                        let alive = this.update(cx, |app, cx| {
                            app.jobs.update(cx, |jobs, cx| {
                                if let Some(success) = jobs.apply(event) {
                                    cx.emit(success);
                                }
                                cx.notify();
                            });
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                });
                (Some(engine), None, Some(events))
            }
            Err(message) => (None, Some(message), None),
        };

        let output_dir: Option<PathBuf> = None;
        let views = engine.as_ref().map(|engine| Views {
            images: cx.new(|cx| {
                ImagesView::new(engine.clone(), jobs.clone(), window, cx, output_dir.clone())
            }),
            pdf: cx.new(|cx| PdfView::new(engine.clone(), jobs.clone(), cx, output_dir.clone())),
            pages: cx.new(|cx| {
                PdfWorkspace::new(
                    engine.clone(),
                    jobs.clone(),
                    Tool::Pages,
                    window,
                    cx,
                    output_dir.clone(),
                )
            }),
            pdf_tools: cx.new(|cx| {
                PdfWorkspace::new(
                    engine.clone(),
                    jobs.clone(),
                    Tool::PdfTools,
                    window,
                    cx,
                    output_dir.clone(),
                )
            }),
            dates: cx.new(|cx| DatesView::new(window, cx)),
            ocr: cx.new(|cx| OcrView::new(engine.clone(), jobs.clone(), cx, output_dir.clone())),
            redact: cx.new(|cx| {
                RedactView::new(engine.clone(), jobs.clone(), window, cx, output_dir.clone())
            }),
            cleanup: cx
                .new(|cx| CleanupView::new(engine.clone(), jobs.clone(), cx, output_dir.clone())),
        });

        Self {
            tool: Tool::Images,
            pdf_expanded: true,
            engine,
            error,
            jobs,
            views,
            output_dir,
            _events: events,
            _success: success,
        }
    }

    fn choose_output_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose output folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(path) = paths.pop()
            {
                let _ = this.update(cx, |this, cx| {
                    this.output_dir = Some(path.clone());
                    if let Some(views) = &this.views {
                        views
                            .images
                            .update(cx, |view, cx| view.set_output_dir(Some(path.clone()), cx));
                        views
                            .pdf
                            .update(cx, |view, cx| view.set_output_dir(Some(path.clone()), cx));
                        views
                            .pages
                            .update(cx, |view, cx| view.set_output_dir(Some(path.clone()), cx));
                        views
                            .pdf_tools
                            .update(cx, |view, cx| view.set_output_dir(Some(path.clone()), cx));
                        views
                            .ocr
                            .update(cx, |view, cx| view.set_output_dir(Some(path.clone()), cx));
                        views
                            .redact
                            .update(cx, |view, cx| view.set_output_dir(Some(path.clone()), cx));
                        views
                            .cleanup
                            .update(cx, |view, cx| view.set_output_dir(Some(path), cx));
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn accept(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(views) = &self.views else {
            return;
        };
        match self.tool {
            Tool::Images => {
                let view = views.images.clone();
                view.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::Pdf => {
                let view = views.pdf.clone();
                view.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::Pages => {
                let view = views.pages.clone();
                view.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::Ocr => {
                let view = views.ocr.clone();
                view.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::Redact => {
                let view = views.redact.clone();
                view.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::Cleanup => {
                let view = views.cleanup.clone();
                view.update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::PdfTools => {
                views
                    .pdf_tools
                    .update(cx, |view, cx| view.accept_paths(paths, cx));
            }
            Tool::Dates => {}
        }
    }

    fn sidebar(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let mut nav = div().flex().flex_col().gap_1().px_3().py_4();
        for (tool, label, icon) in [
            (Tool::Images, "Images", "garage/images.svg"),
            (Tool::Pdf, "PDF tools", "garage/pdf-pages.svg"),
            (Tool::Ocr, "OCR", "garage/ocr.svg"),
            (Tool::Cleanup, "Cleanup", "garage/cleanup.svg"),
            (Tool::Dates, "Dates", "garage/dates.svg"),
        ] {
            let pdf = tool == Tool::Pdf;
            let selected = if pdf {
                matches!(
                    self.tool,
                    Tool::Pdf | Tool::Pages | Tool::PdfTools | Tool::Redact
                )
            } else {
                self.tool == tool
            };
            nav = nav.child(
                Button::new(SharedString::from(format!("nav-{tool:?}")))
                    .ghost()
                    .w_full()
                    .h(px(42.))
                    .px_3()
                    .rounded(px(6.))
                    .tooltip(label)
                    .selected(selected)
                    .text_color(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .w_full()
                            .child(Icon::default().path(icon).size(px(18.)).flex_shrink_0())
                            .when(!compact, |d| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(label),
                                )
                            })
                            .when(!compact && pdf, |d| {
                                d.child(
                                    Icon::new(if self.pdf_expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .size(px(14.)),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if pdf {
                            this.pdf_expanded = !this.pdf_expanded;
                        } else {
                            this.tool = tool;
                        }
                        cx.notify();
                    })),
            );
            if pdf && self.pdf_expanded {
                let pages_action = self
                    .views
                    .as_ref()
                    .map(|v| v.pages.read(cx).current_action())
                    .unwrap_or("");
                let tools_action = self
                    .views
                    .as_ref()
                    .map(|v| v.pdf_tools.read(cx).current_action())
                    .unwrap_or("");
                for (id, label, target, action) in [
                    ("compress", "Compress PDF", Tool::Pdf, ""),
                    ("organize", "Organize pages", Tool::Pages, "organize"),
                    ("merge", "Merge / split / extract", Tool::Pages, "legacy"),
                    ("images-pdf", "Images to PDF", Tool::Pages, "images"),
                    ("pdf-images", "PDF to images", Tool::Pages, "export"),
                    ("redact", "Redact documents", Tool::Redact, ""),
                    ("watermark", "Watermarks", Tool::PdfTools, "watermark"),
                    ("numbers", "Page numbers", Tool::PdfTools, "numbers"),
                    ("passwords", "Passwords", Tool::PdfTools, "protect"),
                ] {
                    let active = self.tool == target
                        && match target {
                            Tool::Pages => pages_action == action,
                            Tool::PdfTools => {
                                tools_action == action
                                    || (action == "protect" && tools_action == "unlock")
                            }
                            _ => true,
                        };
                    nav = nav.child(
                        Button::new(SharedString::from(format!("pdf-nav-{id}")))
                            .ghost()
                            .w_full()
                            .h(px(34.))
                            .tooltip(label)
                            .selected(active)
                            .text_color(if active {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(
                                div()
                                    .w_full()
                                    .pl(if compact { px(0.) } else { px(33.) })
                                    .text_xs()
                                    .child(if compact {
                                        label.split_whitespace().next().unwrap_or(label)
                                    } else {
                                        label
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.tool = target;
                                if let Some(views) = &this.views {
                                    match target {
                                        Tool::Pages => views
                                            .pages
                                            .update(cx, |v, cx| v.select_action(action, w, cx)),
                                        Tool::PdfTools => views
                                            .pdf_tools
                                            .update(cx, |v, cx| v.select_action(action, w, cx)),
                                        _ => (),
                                    }
                                }
                                cx.notify();
                            })),
                    );
                }
            }
        }
        let output_label = self
            .output_dir
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "Choose output folder".to_string());
        nav = nav.child(
            div()
                .mt_auto()
                .pt_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    Button::new("output-folder-pick")
                        .ghost()
                        .w_full()
                        .tooltip(output_label.clone())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_output_dir(window, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .w_full()
                                .when(compact, |el| el.flex_col())
                                .child(
                                    Icon::new(IconName::FolderOpen)
                                        .size(px(18.))
                                        .flex_shrink_0(),
                                )
                                .when(!compact, |el| {
                                    el.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(output_label),
                                    )
                                }),
                        ),
                ),
        );

        div()
            .flex()
            .flex_col()
            .w(px(if compact { 88. } else { 232. }))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_6()
                    .pt_6()
                    .pb_3()
                    .child(img("garage/logo.png").size(px(40.)).flex_shrink_0())
                    .when(!compact, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Garage"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Your local toolkit"),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("tool-navigation")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(nav),
            )
            .child(
                div()
                    .flex()
                    .when(compact, |el| el.flex_col().items_center())
                    .gap_2()
                    .m_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::default()
                            .path("garage/privacy.svg")
                            .size(px(18.))
                            .flex_shrink_0(),
                    )
                    .when(!compact, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Private by design"),
                                )
                                .child(div().text_xs().child("Files stay on this device.")),
                        )
                    }),
            )
    }

    fn status_bar(&self, compact: bool, cx: &App) -> Div {
        let mut left = div().flex().items_center().gap_3().text_xs().min_w_0();
        match &self.engine {
            Some(engine) => {
                let features = engine.features();
                left = left.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(6.)).rounded_full().bg(cx.theme().success))
                        .child("Engine ready"),
                );
                if !compact {
                    left = left
                        .child(dot(cx))
                        .child(feature_label("PDF", features.pdf, cx))
                        .child(feature_label("OCR", features.ocr, cx))
                        .child(feature_label("Ghostscript", features.ghostscript, cx))
                        .child(feature_label("HEIC", features.heif, cx))
                        .child(feature_label("AVIF", features.avif, cx))
                        .child(feature_label("C2PA", features.c2pa, cx));
                }
            }
            None => {
                left = left.child(
                    div()
                        .text_color(cx.theme().danger)
                        .child("Engine unavailable"),
                );
            }
        }
        div()
            .flex()
            .justify_between()
            .items_center()
            .gap_3()
            .px_4()
            .h(px(34.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().status_bar)
            .text_color(cx.theme().muted_foreground)
            .child(left)
            .child(div().text_xs().child("All processing stays local"))
    }

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(views) = &self.views else {
            return div()
                .flex()
                .flex_col()
                .gap_3()
                .p_8()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("The engine did not start"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(self.error.clone().unwrap_or_default()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Keep the garage-engine folder beside the app, or set GARAGE_ENGINE to the engine executable. Developers can build the sidecar with scripts/install.sh or scripts/build-windows.ps1.",
                        ),
                )
                .into_any_element();
        };
        match self.tool {
            Tool::Images => views.images.clone().into_any_element(),
            Tool::Pdf => views.pdf.clone().into_any_element(),
            Tool::Pages => views.pages.clone().into_any_element(),
            Tool::PdfTools => views.pdf_tools.clone().into_any_element(),
            Tool::Dates => views.dates.clone().into_any_element(),
            Tool::Ocr => views.ocr.clone().into_any_element(),
            Tool::Redact => views.redact.clone().into_any_element(),
            Tool::Cleanup => views.cleanup.clone().into_any_element(),
        }
    }
}

#[derive(Clone, Copy)]
enum WindowAction {
    Minimize,
    Zoom,
    Quit,
}

fn window_control(
    id: &'static str,
    icon: IconName,
    action: WindowAction,
    cx: &mut Context<GarageApp>,
) -> impl IntoElement {
    let label = match action {
        WindowAction::Minimize => "Minimize",
        WindowAction::Zoom => "Maximize or restore",
        WindowAction::Quit => "Close Garage",
    };
    Button::new(id)
        .ghost()
        .w(px(44.))
        .h(px(40.))
        .tooltip(label)
        .child(Icon::new(icon).size(px(15.)))
        .on_click(cx.listener(move |_, _, window, cx| match action {
            WindowAction::Minimize => window.minimize_window(),
            WindowAction::Zoom => window.zoom_window(),
            WindowAction::Quit => cx.quit(),
        }))
}

fn dot(cx: &App) -> Div {
    div()
        .w(px(3.))
        .h(px(3.))
        .rounded_full()
        .bg(cx.theme().muted_foreground)
}

fn feature_label(name: &str, on: bool, cx: &App) -> Div {
    div()
        .text_color(if on {
            cx.theme().muted_foreground
        } else {
            cx.theme().danger.opacity(0.8)
        })
        .child(if on {
            name.to_string()
        } else {
            format!("{name} off")
        })
}

impl Render for GarageApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.jobs.read(cx).running_count();
        let compact = window.viewport_size().width < px(960.);
        let maximized = window.is_maximized();
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_drop::<ExternalPaths>(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.accept(paths.paths().to_vec(), cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(52.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().title_bar)
                    .child(
                        div()
                            .id("titlebar-drag")
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_3()
                            .h_full()
                            .px_4()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(MouseButton::Left, |event, window, _| {
                                if event.click_count >= 2 {
                                    window.zoom_window();
                                } else {
                                    window.start_window_move();
                                }
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Garage"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("/"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.tool.label()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .pr_3()
                            .when(!compact || busy > 0, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if busy > 0 {
                                            format!("{busy} running")
                                        } else {
                                            "Ready".to_string()
                                        }),
                                )
                            })
                            .child(
                                Button::new("open-folder")
                                    .small()
                                    .ghost()
                                    .tooltip("Open output folder")
                                    .child(Icon::new(IconName::FolderOpen).size(px(16.)))
                                    .when(!compact, |el| el.label("Output folder"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let path = this.output_dir.clone().or_else(|| {
                                            std::env::var_os("HOME")
                                                .map(PathBuf::from)
                                                .map(|home| home.join("Documents"))
                                        });
                                        if let Some(path) = path {
                                            cx.open_with_system(&path);
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h_full()
                            .border_l_1()
                            .border_color(cx.theme().border)
                            .child(window_control(
                                "win-minimize",
                                IconName::Minus,
                                WindowAction::Minimize,
                                cx,
                            ))
                            .child(window_control(
                                "win-maximize",
                                if maximized {
                                    IconName::Minimize
                                } else {
                                    IconName::Maximize
                                },
                                WindowAction::Zoom,
                                cx,
                            ))
                            .child(window_control(
                                "win-close",
                                IconName::Close,
                                WindowAction::Quit,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.sidebar(compact, cx))
                    .child(
                        div()
                            .id(SharedString::from(format!("content-{:?}", self.tool)))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_y_scroll()
                            .child(self.content(cx)),
                    ),
            )
            .child(self.status_bar(compact, cx))
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("Garage {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if std::env::args().any(|arg| arg == "--check-engine") {
        // An optional report path makes this usable from CI even on Windows,
        // where GUI-subsystem apps have no visible stdout.
        let args: Vec<String> = std::env::args().collect();
        let report = args
            .iter()
            .position(|arg| arg == "--check-engine")
            .and_then(|index| args.get(index + 1))
            .map(PathBuf::from);
        let fail = |message: &str| -> ! {
            if let Some(path) = &report {
                let _ = std::fs::write(path, format!("ERROR: {message}"));
            }
            eprintln!("{message}");
            std::process::exit(1);
        };
        let engine = Engine::spawn().unwrap_or_else(|error| fail(&error));
        // Always reap the engine before leaving: a surviving child can keep
        // bundled libraries (notably c2pa_c.dll) locked on Windows while CI
        // removes the extracted package directory.
        let outcome = loop {
            match engine.events().recv_blocking() {
                Ok(engine::Event::Done { result, .. }) if result.get("features").is_some() => {
                    break Ok(result.to_string());
                }
                Ok(engine::Event::Exited(error)) => break Err(error),
                Ok(_) => {}
                Err(_) => break Err("engine exited before replying to the check".to_string()),
            }
        };
        engine.kill();
        match outcome {
            Ok(line) => {
                println!("{line}");
                if let Some(path) = &report
                    && let Err(error) = std::fs::write(path, &line)
                {
                    fail(&error.to_string());
                }
                return;
            }
            Err(error) => fail(&error),
        }
    }
    gpui_kit::application()
        .with_assets(brand::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            brand::configure_theme(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(120.), px(90.)),
                    size(px(1180.), px(820.)),
                ))),
                // The custom title bar above draws its own controls, so suppress
                // the native decoration set to avoid two rows of buttons.
                window_decorations: Some(WindowDecorations::Client),
                titlebar: None,
                app_id: Some("garage".into()),
                window_min_size: Some(size(px(560.), px(480.))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| GarageApp::new(window, cx))
            })
            .expect("failed to open the Garage window");
        });
}
