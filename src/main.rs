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
use views::pages::PagesView;
use views::pdf::PdfView;
use views::redact::RedactView;

const TOOLS: [Tool; 7] = [
    Tool::Images,
    Tool::Pdf,
    Tool::Pages,
    Tool::Dates,
    Tool::Ocr,
    Tool::Redact,
    Tool::Cleanup,
];

struct Views {
    images: Entity<ImagesView>,
    pdf: Entity<PdfView>,
    pages: Entity<PagesView>,
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
    _events: Option<Task<()>>,
}

impl GarageApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let jobs = cx.new(|_| Jobs::default());
        let (engine, error, events) = match Engine::spawn() {
            Ok(engine) => {
                let rx = engine.events();
                let events = cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = rx.recv().await {
                        let alive = this.update(cx, |app, cx| {
                            app.jobs.update(cx, |jobs, cx| {
                                jobs.apply(event);
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

        let views = engine.as_ref().map(|engine| Views {
            images: cx.new(|cx| ImagesView::new(engine.clone(), jobs.clone(), window, cx)),
            pdf: cx.new(|cx| PdfView::new(engine.clone(), jobs.clone(), cx)),
            pages: cx.new(|cx| PagesView::new(engine.clone(), jobs.clone(), window, cx)),
            dates: cx.new(|cx| DatesView::new(window, cx)),
            ocr: cx.new(|cx| OcrView::new(engine.clone(), jobs.clone(), cx)),
            redact: cx.new(|cx| RedactView::new(engine.clone(), jobs.clone(), window, cx)),
            cleanup: cx.new(|cx| CleanupView::new(engine.clone(), jobs.clone(), cx)),
        });

        Self {
            tool: Tool::Images,
            engine,
            error,
            jobs,
            views,
            _events: events,
        }
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
            Tool::Dates => {}
        }
    }

    fn sidebar(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let mut nav = div().flex().flex_col().gap_2().px_3().py_4();
        for tool in TOOLS {
            let selected = self.tool == tool;
            let icon = match tool {
                Tool::Images => "garage/images.svg",
                Tool::Pdf => "garage/compress-pdf.svg",
                Tool::Pages => "garage/pdf-pages.svg",
                Tool::Dates => "garage/dates.svg",
                Tool::Ocr => "garage/ocr.svg",
                Tool::Redact => "garage/redact.svg",
                Tool::Cleanup => "garage/cleanup.svg",
            };
            let short_label = match tool {
                Tool::Pdf => "PDF",
                Tool::Pages => "Pages",
                _ => tool.label(),
            };
            let label = if compact { short_label } else { tool.label() };
            let mut button = Button::new(SharedString::from(format!("nav-{tool:?}")))
                .ghost()
                .w_full()
                .h(px(if compact { 58. } else { 44. }))
                .px(if compact { px(4.) } else { px(12.) })
                .rounded_lg()
                .selected(selected)
                .tooltip(tool.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tool = tool;
                    cx.notify();
                }));
            button = button.child(
                div()
                    .flex()
                    .when(compact, |el| el.flex_col())
                    .items_center()
                    .gap(if compact { px(4.) } else { px(12.) })
                    .w_full()
                    .text_color(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(Icon::default().path(icon).size(px(18.)).flex_shrink_0())
                    .child(
                        div()
                            .when(compact, |el| el.text_xs())
                            .when(!compact, |el| el.text_sm())
                            .font_weight(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .child(label),
                    ),
            );
            nav = nav.child(button);
        }

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
                            "Run scripts/install.sh to build the sidecar, or set GARAGE_ENGINE to a Python interpreter that has the engine on its path.",
                        ),
                )
                .into_any_element();
        };
        match self.tool {
            Tool::Images => views.images.clone().into_any_element(),
            Tool::Pdf => views.pdf.clone().into_any_element(),
            Tool::Pages => views.pages.clone().into_any_element(),
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
                                        if let Some(views) = &this.views {
                                            let path =
                                                views.pdf.read(cx).output_dir().or_else(|| {
                                                    std::env::var_os("HOME")
                                                        .map(PathBuf::from)
                                                        .map(|home| home.join("Documents"))
                                                });
                                            if let Some(path) = path {
                                                cx.reveal_path(&path);
                                            }
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
        let engine = Engine::spawn().unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1);
        });
        while let Ok(event) = engine.events().recv_blocking() {
            match event {
                engine::Event::Done { result, .. } if result.get("features").is_some() => {
                    println!("{result}");
                    engine.kill();
                    return;
                }
                engine::Event::Exited(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
                _ => {}
            }
        }
        std::process::exit(1);
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
