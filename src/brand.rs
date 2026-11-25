//! Garage's embedded artwork and forest-green theme. No asset needs a network.
use std::{borrow::Cow, rc::Rc};

use gpui_kit::component::theme::{Theme, ThemeConfig};
use gpui_kit::{App, AssetSource, Result, SharedString};

pub struct Assets;

macro_rules! bundled_assets {
    ($($path:literal => $file:literal),+ $(,)?) => {
        impl AssetSource for Assets {
            fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
                match path {
                    $($path => Ok(Some(Cow::Borrowed(include_bytes!($file)))),)+
                    _ => gpui_kit::assets::Assets.load(path),
                }
            }

            fn list(&self, path: &str) -> Result<Vec<SharedString>> {
                let mut paths = gpui_kit::assets::Assets.list(path)?;
                paths.extend([$($path),+].into_iter().filter(|name| name.starts_with(path)).map(Into::into));
                Ok(paths)
            }
        }
    };
}

bundled_assets! {
    "garage/logo.png" => "../assets/brand/garage-128.png",
    "garage/paper-light.webp" => "../assets/backgrounds/paper-light.webp",
    "garage/paper-dark.webp" => "../assets/backgrounds/paper-dark.webp",
    "garage/images.svg" => "../assets/icons/images.svg",
    "garage/compress-pdf.svg" => "../assets/icons/compress-pdf.svg",
    "garage/pdf-pages.svg" => "../assets/icons/pdf-pages.svg",
    "garage/dates.svg" => "../assets/icons/dates.svg",
    "garage/ocr.svg" => "../assets/icons/ocr.svg",
    "garage/redact.svg" => "../assets/icons/redact.svg",
    "garage/cleanup.svg" => "../assets/icons/cleanup.svg",
    "garage/upload.svg" => "../assets/icons/upload.svg",
    "garage/privacy.svg" => "../assets/icons/privacy.svg",
}

/// Register both appearances so system theme changes retain the same identity.
pub fn configure_theme(cx: &mut App) {
    let light = palette(Theme::global(cx).light_theme.as_ref().clone(), false);
    let dark = palette(Theme::global(cx).dark_theme.as_ref().clone(), true);
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    });
    Theme::change(Theme::global(cx).mode, None, cx);
}

fn palette(mut config: ThemeConfig, dark: bool) -> ThemeConfig {
    config.name = if dark { "Garage Dark" } else { "Garage Light" }.into();
    config.radius = Some(8);
    config.radius_lg = Some(12);
    let (
        background,
        panel,
        sidebar,
        border,
        foreground,
        muted,
        muted_foreground,
        primary,
        hover,
        active,
        primary_foreground,
        selection,
    ) = if dark {
        (
            "#141c18", "#1a2520", "#17211c", "#33453a", "#e7ede7", "#29362e", "#a4b7a8", "#a0d1ac",
            "#b2ddbd", "#8bc49a", "#143021", "#2d4535",
        )
    } else {
        (
            "#f6f7f3", "#fffdf9", "#eef1eb", "#d8dfd6", "#22372d", "#e7ece5", "#5b6d61", "#286447",
            "#20543b", "#18432f", "#ffffff", "#deecdf",
        )
    };
    let c = &mut config.colors;
    c.background = Some(background.into());
    c.foreground = Some(foreground.into());
    c.popover = Some(panel.into());
    c.popover_foreground = Some(foreground.into());
    c.sidebar = Some(sidebar.into());
    c.sidebar_foreground = Some(foreground.into());
    c.title_bar = Some(panel.into());
    c.status_bar = Some(sidebar.into());
    c.border = Some(border.into());
    c.input = Some(border.into());
    c.muted = Some(muted.into());
    c.muted_foreground = Some(muted_foreground.into());
    c.primary = Some(primary.into());
    c.primary_hover = Some(hover.into());
    c.primary_active = Some(active.into());
    c.primary_foreground = Some(primary_foreground.into());
    c.button_primary = Some(primary.into());
    c.button_primary_hover = Some(hover.into());
    c.button_primary_active = Some(active.into());
    c.button_primary_foreground = Some(primary_foreground.into());
    c.secondary = Some(muted.into());
    c.secondary_foreground = Some(foreground.into());
    c.secondary_hover = Some(selection.into());
    c.secondary_active = Some(selection.into());
    c.button_secondary = Some(muted.into());
    c.button_secondary_foreground = Some(foreground.into());
    c.button_secondary_hover = Some(selection.into());
    c.button_secondary_active = Some(selection.into());
    c.button = Some(panel.into());
    c.button_foreground = Some(foreground.into());
    c.button_hover = Some(muted.into());
    c.button_active = Some(selection.into());
    c.accent = Some(selection.into());
    c.accent_foreground = Some(foreground.into());
    c.selection = Some(selection.into());
    c.ring = Some(primary.into());
    c.caret = Some(primary.into());
    config
}
