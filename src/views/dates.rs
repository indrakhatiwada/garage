use chrono::{Datelike, Local, NaiveDate};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::component::{Icon, switch::Switch};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::{card, field_label, heading, muted, note, page, pill};
use crate::bs::{self, BsDate, DateStyle};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Bs,
    Ad,
}

pub struct DatesView {
    selected: NaiveDate,
    mode: Mode,
    view_year: i32,
    view_month: u32,
    nepali_digits: bool,
    copied: Option<String>,
    bs_input: Entity<InputState>,
    ad_input: Entity<InputState>,
    bs_result: Option<String>,
    ad_result: Option<String>,
}

impl DatesView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let bs = bs::from_ad(today).unwrap_or(BsDate {
            year: 2082,
            month: 1,
            day: 1,
        });
        let bs_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("2082-08-06 or ०६/०८/२०८२"));
        bs_input.update(cx, |state, cx| {
            state.set_value(bs.format(DateStyle::Iso), window, cx)
        });
        let ad_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("2025-11-22 or 22/11/2025"));
        ad_input.update(cx, |state, cx| {
            state.set_value(today.format("%Y-%m-%d").to_string(), window, cx)
        });
        Self {
            selected: today,
            mode: Mode::Bs,
            view_year: bs.year,
            view_month: bs.month,
            nepali_digits: false,
            copied: None,
            bs_input,
            ad_input,
            bs_result: None,
            ad_result: None,
        }
    }

    fn convert_bs(&mut self, cx: &mut Context<Self>) {
        let text = self.bs_input.read(cx).value().to_string();
        let converted = bs::parse(&text).and_then(|date| date.to_ad().ok());
        self.bs_result = Some(match converted {
            Some(ad) => {
                self.set_date(ad);
                format!("{} · {}", ad.format("%Y-%m-%d"), ad.format("%A"))
            }
            None => "Could not read that date. Try 2082-08-06 or ०६/०८/२०८२.".into(),
        });
        cx.notify();
    }

    fn convert_ad(&mut self, cx: &mut Context<Self>) {
        let text = self.ad_input.read(cx).value().to_string();
        let parsed = NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .or_else(|_| NaiveDate::parse_from_str(&text, "%d/%m/%Y"))
            .or_else(|_| NaiveDate::parse_from_str(&text, "%m/%d/%Y"))
            .ok();
        self.ad_result = Some(
            match parsed.and_then(|date| bs::from_ad(date).ok().map(|bs| (date, bs))) {
                Some((date, bs_date)) => {
                    self.set_date(date);
                    format!(
                        "{} · {} {}",
                        bs_date.format(DateStyle::Iso),
                        bs_date.month_np(),
                        bs_date.day
                    )
                }
                None => "Could not read that date. Try 2025-11-22 or 22/11/2025.".into(),
            },
        );
        cx.notify();
    }

    fn bs(&self) -> Option<BsDate> {
        bs::from_ad(self.selected).ok()
    }

    fn first_weekday(&self) -> u32 {
        match self.mode {
            Mode::Bs => bs::to_ad(self.view_year, self.view_month, 1)
                .map(|d| d.weekday().num_days_from_sunday())
                .unwrap_or(0),
            Mode::Ad => NaiveDate::from_ymd_opt(self.view_year, self.view_month, 1)
                .map(|d| d.weekday().num_days_from_sunday())
                .unwrap_or(0),
        }
    }

    fn days_in_view(&self) -> u32 {
        match self.mode {
            Mode::Bs => bs::month_days(self.view_year, self.view_month).unwrap_or(30),
            Mode::Ad => {
                let (ny, nm) = if self.view_month == 12 {
                    (self.view_year + 1, 1)
                } else {
                    (self.view_year, self.view_month + 1)
                };
                NaiveDate::from_ymd_opt(ny, nm, 1)
                    .and_then(|next| {
                        NaiveDate::from_ymd_opt(self.view_year, self.view_month, 1)
                            .map(|first| (next - first).num_days() as u32)
                    })
                    .unwrap_or(30)
            }
        }
    }

    fn sync_view_to_selection(&mut self) {
        match self.mode {
            Mode::Bs => {
                if let Some(bs) = self.bs() {
                    self.view_year = bs.year;
                    self.view_month = bs.month;
                }
            }
            Mode::Ad => {
                self.view_year = self.selected.year();
                self.view_month = self.selected.month();
            }
        }
    }

    fn shift_month(&mut self, delta: i32) {
        let mut month = self.view_month as i32 + delta;
        let mut year = self.view_year;
        while month < 1 {
            month += 12;
            year -= 1;
        }
        while month > 12 {
            month -= 12;
            year += 1;
        }
        let (min, max) = match self.mode {
            Mode::Bs => (bs::BS_MIN_YEAR, bs::BS_MAX_YEAR),
            Mode::Ad => (1918, 2044),
        };
        if (min..=max).contains(&year) {
            self.view_year = year;
            self.view_month = month as u32;
        }
    }

    fn select_day(&mut self, day: u32) {
        let date = match self.mode {
            Mode::Bs => bs::to_ad(self.view_year, self.view_month, day).ok(),
            Mode::Ad => NaiveDate::from_ymd_opt(self.view_year, self.view_month, day),
        };
        if let Some(date) = date {
            self.selected = date;
        }
    }

    fn set_date(&mut self, date: NaiveDate) {
        self.selected = date;
        self.sync_view_to_selection();
    }

    fn month_title(&self) -> String {
        match self.mode {
            Mode::Bs => {
                let name = bs::MONTH_NAMES_NP
                    .get(self.view_month.saturating_sub(1) as usize)
                    .copied()
                    .unwrap_or("");
                let en = bs::MONTH_NAMES_EN
                    .get(self.view_month.saturating_sub(1) as usize)
                    .copied()
                    .unwrap_or("");
                if self.nepali_digits {
                    format!(
                        "{} {} · {}",
                        name,
                        bs::to_devanagari_digits(&self.view_year.to_string()),
                        en
                    )
                } else {
                    format!("{} {} · {}", name, self.view_year, en)
                }
            }
            Mode::Ad => NaiveDate::from_ymd_opt(self.view_year, self.view_month, 1)
                .map(|d| d.format("%B %Y").to_string())
                .unwrap_or_default(),
        }
    }

    fn num(&self, value: i32) -> String {
        if self.nepali_digits {
            bs::to_devanagari_digits(&value.to_string())
        } else {
            value.to_string()
        }
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.copied = Some(text);
        cx.notify();
    }
}

impl Render for DatesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bs_today = self.bs();
        let today = Local::now().date_naive();
        let first = self.first_weekday();
        let days = self.days_in_view();
        let selected_day = match self.mode {
            Mode::Bs => bs_today
                .filter(|b| b.year == self.view_year && b.month == self.view_month)
                .map(|b| b.day),
            Mode::Ad => Some(self.selected.day()).filter(|_| {
                self.selected.year() == self.view_year && self.selected.month() == self.view_month
            }),
        };
        let today_day = match self.mode {
            Mode::Bs => bs::from_ad(today)
                .ok()
                .filter(|b| b.year == self.view_year && b.month == self.view_month)
                .map(|b| b.day),
            Mode::Ad => Some(today.day())
                .filter(|_| today.year() == self.view_year && today.month() == self.view_month),
        };

        let weekdays_np = ["आइत", "सोम", "मंगल", "बुध", "बिही", "शुक्र", "शनि"];
        let weekdays_en = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

        let mut grid = div().flex().flex_col().gap_1().w_full();
        let mut header = div().flex().gap_1().w_full();
        for i in 0..7 {
            let label = if self.nepali_digits {
                weekdays_np[i]
            } else {
                weekdays_en[i]
            };
            header = header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            );
        }
        grid = grid.child(header);

        let mut cells: Vec<Option<u32>> = (0..first).map(|_| None).collect();
        cells.extend((1..=days).map(Some));
        cells.resize(cells.len().div_ceil(7) * 7, None);

        for week in cells.chunks(7) {
            let mut row = div().flex().gap_1().w_full();
            for cell in week {
                let mut day_cell = div()
                    .flex_1()
                    .min_w_0()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .text_sm();
                if let Some(day) = cell {
                    let day = *day;
                    let is_selected = selected_day == Some(day);
                    let is_today = today_day == Some(day);
                    let button = Button::new(SharedString::from(format!("calendar-day-{day}")))
                        .label(self.num(day as i32))
                        .w_full()
                        .h_full()
                        .when(is_selected, |button| button.primary())
                        .when(!is_selected, |button| button.ghost())
                        .when(is_today && !is_selected, |button| {
                            button
                                .border_1()
                                .border_color(cx.theme().primary.opacity(0.6))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_day(day);
                            cx.notify();
                        }));
                    day_cell = day_cell.child(button);
                }
                row = row.child(day_cell);
            }
            grid = grid.child(row);
        }

        let bs_text = bs_today
            .map(|b| b.format(DateStyle::Iso))
            .unwrap_or_else(|| "out of range".into());
        let bs_long = bs_today
            .map(|b| b.format(DateStyle::Long))
            .unwrap_or_default();
        let bs_dev = bs_today
            .map(|b| b.format(DateStyle::Devanagari))
            .unwrap_or_default();
        let bs_weekday = bs_today.map(|b| b.weekday_np()).unwrap_or("");
        let ad_iso = self.selected.format("%Y-%m-%d").to_string();
        let ad_long = self.selected.format("%A, %d %B %Y").to_string();
        let delta_days = (self.selected - today).num_days();

        let delta_label = match delta_days {
            0 => "Today".to_string(),
            d if d > 0 => format!("{d} days from today"),
            d => format!("{} days ago", -d),
        };

        let conversion = card(cx)
            .child(
                div()
                    .flex()
                    .when(super::panel_width(window) < 600., |el| el.flex_col())
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .child(field_label("Bikram Sambat", cx))
                            .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child(
                                if self.nepali_digits {
                                    bs_dev.clone()
                                } else {
                                    bs_text.clone()
                                },
                            ))
                            .child(muted(bs_long, cx))
                            .child(note(format!("{} · {bs_weekday}", bs_dev), cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .child(field_label("Gregorian", cx))
                            .child(
                                div()
                                    .text_3xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(ad_iso.clone()),
                            )
                            .child(muted(ad_long.clone(), cx))
                            .child(note(delta_label.clone(), cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("copy-bs")
                            .label("Copy BS")
                            .small()
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this
                                    .bs()
                                    .map(|b| b.format(DateStyle::Iso))
                                    .unwrap_or_default();
                                this.copy(text, cx);
                            })),
                    )
                    .child(
                        Button::new("copy-ad")
                            .label("Copy AD")
                            .small()
                            .secondary()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.selected.format("%Y-%m-%d").to_string();
                                this.copy(text, cx);
                            })),
                    )
                    .child(
                        Button::new("today")
                            .label("Today")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let today = Local::now().date_naive();
                                this.set_date(today);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(note("Nepali digits", cx))
                            .child(
                                Switch::new("nepali-digits")
                                    .checked(self.nepali_digits)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.nepali_digits = !this.nepali_digits;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );

        let converter = card(cx)
            .child(field_label("Bikram Sambat → Gregorian", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&self.bs_input).w(px(super::panel_width(window).min(280.))))
                    .child(
                        Button::new("bs-convert")
                            .label("Convert →")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.convert_bs(cx))),
                    )
                    .child(
                        Button::new("copy-bs-result")
                            .label("Copy")
                            .small()
                            .ghost()
                            .disabled(self.bs_result.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(result) = this.bs_result.clone() {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        result.clone(),
                                    ));
                                    this.copied = Some(result);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(match &self.bs_result {
                Some(result) => note(result.clone(), cx),
                None => note("Enter a BS date, or pick one from the calendar below.", cx),
            })
            .child(field_label("Gregorian → Bikram Sambat", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&self.ad_input).w(px(super::panel_width(window).min(280.))))
                    .child(
                        Button::new("ad-convert")
                            .label("Convert →")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.convert_ad(cx))),
                    )
                    .child(
                        Button::new("copy-ad-result")
                            .label("Copy")
                            .small()
                            .ghost()
                            .disabled(self.ad_result.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(result) = this.ad_result.clone() {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        result.clone(),
                                    ));
                                    this.copied = Some(result);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(match &self.ad_result {
                Some(result) => note(result.clone(), cx),
                None => note("Accepts YYYY-MM-DD, DD/MM/YYYY, or MM/DD/YYYY.", cx),
            });

        let calendar = card(cx)
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
                            .child({
                                let b = Button::new("mode-bs").label("BS").small();
                                let b = if self.mode == Mode::Bs {
                                    b.primary()
                                } else {
                                    b.ghost()
                                };
                                b.on_click(cx.listener(|this, _, _, cx| {
                                    this.mode = Mode::Bs;
                                    this.sync_view_to_selection();
                                    cx.notify();
                                }))
                            })
                            .child({
                                let b = Button::new("mode-ad").label("AD").small();
                                let b = if self.mode == Mode::Ad {
                                    b.primary()
                                } else {
                                    b.ghost()
                                };
                                b.on_click(cx.listener(|this, _, _, cx| {
                                    this.mode = Mode::Ad;
                                    this.sync_view_to_selection();
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("prev-month")
                                    .child(Icon::new(IconName::ChevronLeft).size(px(16.)))
                                    .tooltip("Previous month")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.shift_month(-1);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::MEDIUM)
                                    .min_w_0()
                                    .text_center()
                                    .child(self.month_title()),
                            )
                            .child(
                                Button::new("next-month")
                                    .child(Icon::new(IconName::ChevronRight).size(px(16.)))
                                    .tooltip("Next month")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.shift_month(1);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(grid);

        let footer = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(pill(
                format!("BS {} – {}", bs::BS_MIN_YEAR, bs::BS_MAX_YEAR),
                cx.theme().info,
                cx,
            ))
            .child(pill("Nepali · English", cx.theme().muted_foreground, cx))
            .child(if let Some(copied) = self.copied.clone() {
                muted(format!("copied {copied}"), cx)
            } else {
                note("Click a day, or use the arrows to move months.", cx)
            });

        page(window)
            .child(heading(
                "Dates",
                "Bikram Sambat and Gregorian, both directions. Month lengths come from the canonical table.",
                cx,
            ))
            .child(conversion)
            .child(converter)
            .child(calendar)
            .child(footer)
    }
}
