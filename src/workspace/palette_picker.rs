use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Selectable, Sizable, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, IntoElement, SharedString, Window, div, prelude::*, px,
    relative, rgb,
};

use crate::appearance::{Palette, Palettes};
use crate::database::Database;
use crate::model::settings;

/// The palette used while Kron is light, or dark: a swatch and its name,
/// opening the others with previews.
pub fn palette_picker(dark: bool, database: Entity<Database>, cx: &App) -> AnyElement {
    let palette = Palettes::current(cx).for_mode(dark);
    Popover::new(if dark {
        "dark-palette"
    } else {
        "light-palette"
    })
    .trigger(
        Button::new(if dark {
            "choose-dark-palette"
        } else {
            "choose-light-palette"
        })
        .outline()
        .small()
        .dropdown_caret(true)
        .child(
            h_flex()
                .gap_2()
                .child(swatch(palette, cx))
                .child(palette.label()),
        ),
    )
    .content(move |_, window, cx| options(dark, database.clone(), window, cx))
    .into_any_element()
}

fn options(
    dark: bool,
    database: Entity<Database>,
    window: &Window,
    cx: &mut Context<PopoverState>,
) -> AnyElement {
    let selected = Palettes::current(cx).for_mode(dark);
    let popover = cx.entity();
    v_flex()
        .id(if dark {
            "dark-palette-options"
        } else {
            "light-palette-options"
        })
        .w(px(444.))
        .max_w(window.viewport_size().width - px(48.))
        .max_h(window.viewport_size().height - px(80.))
        .overflow_y_scrollbar()
        .p_3()
        .gap_3()
        .child(div().text_sm().child(if dark {
            "Dark palettes"
        } else {
            "Light palettes"
        }))
        .child(
            h_flex().flex_wrap().gap_3().children(
                Palette::ALL
                    .into_iter()
                    .filter(move |palette| palette.is_dark() == dark)
                    .map(move |palette| {
                        let database = database.clone();
                        let popover = popover.clone();
                        let active = palette == selected;
                        Button::new(SharedString::from(format!("palette-{}", palette.stored())))
                            .ghost()
                            .w(px(132.))
                            .h(px(112.))
                            .p_2()
                            .border_1()
                            .border_color(if active {
                                cx.theme().primary
                            } else {
                                cx.theme().border
                            })
                            .selected(active)
                            .accessibility_label(palette.label())
                            .child(
                                v_flex().w_full().gap_2().child(preview(palette)).child(
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .text_xs()
                                        .child(div().flex_1().child(palette.label()))
                                        .when(active, |this| {
                                            this.child(Icon::new(IconName::Check).xsmall())
                                        }),
                                ),
                            )
                            .on_click(move |_, window, cx| {
                                database
                                    .read(cx)
                                    .queue(move |write| settings::set_palette(write, palette));
                                popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                            })
                    }),
            ),
        )
        .into_any_element()
}

/// The palette's background beside its accent, outlined so a dark palette's
/// shows on a dark window and a light one's on a light window.
fn swatch(palette: Palette, cx: &App) -> Div {
    let colors = palette.colors();
    h_flex()
        .w(px(28.))
        .h(px(16.))
        .overflow_hidden()
        .rounded_sm()
        .border_1()
        .border_color(cx.theme().muted_foreground.opacity(0.5))
        .child(div().flex_1().h_full().bg(rgb(colors.background)))
        .child(div().flex_1().h_full().bg(rgb(colors.accent)))
}

/// A small window in the palette's own colors: sidebar with a selected item,
/// a primary button and a card, so palettes with similar backgrounds still
/// look different.
fn preview(palette: Palette) -> Div {
    let colors = palette.colors();
    let line = |width: f32, color: u32, opacity: f32| {
        div()
            .h(px(3.))
            .w(relative(width))
            .rounded_full()
            .bg(rgb(color).opacity(opacity))
    };
    h_flex()
        .w_full()
        .h(px(64.))
        .overflow_hidden()
        .rounded_sm()
        .border_1()
        .border_color(rgb(colors.border))
        .bg(rgb(colors.background))
        .child(
            v_flex()
                .h_full()
                .w(relative(0.3))
                .p_1()
                .gap_1()
                .bg(rgb(colors.sidebar))
                .border_r_1()
                .border_color(rgb(colors.border))
                .child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded_sm()
                        .bg(rgb(colors.selection)),
                )
                .child(line(0.7, colors.ink, 0.35))
                .child(line(0.5, colors.ink, 0.35)),
        )
        .child(
            v_flex()
                .flex_1()
                .h_full()
                .p_1()
                .gap_1()
                .child(
                    h_flex()
                        .justify_between()
                        .child(line(0.4, colors.ink, 0.7))
                        .child(
                            div()
                                .w(px(14.))
                                .h(px(6.))
                                .rounded_sm()
                                .bg(rgb(colors.accent)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .p_1()
                        .gap_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .child(line(0.5, colors.accent, 1.))
                        .child(line(0.9, colors.ink, 0.3))
                        .child(line(0.7, colors.ink, 0.3)),
                ),
        )
}
