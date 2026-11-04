use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Selectable, h_flex, v_flex};
use gpui_kit::{AnyElement, App, Entity, IntoElement, SharedString, div, prelude::*, px, rgb};

use crate::appearance::{Palette, Palettes};
use crate::database::Database;
use crate::model::settings;

/// Both color families stay visible while the user changes appearance mode.
pub fn palette_picker(database: Entity<Database>, cx: &App) -> AnyElement {
    let selected = Palettes::current(cx);
    v_flex()
        .w_full()
        .gap_5()
        .child(div().text_xs().text_color(cx.theme().muted_foreground)
            .child("Choose a palette for each appearance. Match system switches between your saved choices."))
        .children([false, true].map(|dark| {
            let database = database.clone();
            v_flex()
                .gap_2()
                .child(div().text_sm().child(if dark {
                    "Dark appearance"
                } else {
                    "Light appearance"
                }))
                .child(
                    h_flex().flex_wrap().gap_3().children(
                        Palette::ALL
                            .into_iter()
                            .filter(move |palette| palette.is_dark() == dark)
                            .map(move |palette| {
                                let database = database.clone();
                                let active = selected.for_mode(dark) == palette;
                                let [background, sidebar, accent, ink] = palette.preview();
                                Button::new(SharedString::from(format!(
                                    "palette-{}",
                                    palette.stored()
                                )))
                                .ghost()
                                .w(px(132.))
                                .h(px(116.))
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
                                    v_flex()
                                        .w_full()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .h(px(64.))
                                                .w_full()
                                                .overflow_hidden()
                                                .rounded_sm()
                                                .bg(rgb(background))
                                                .child(
                                                    v_flex()
                                                        .h_full()
                                                        .w(px(26.))
                                                        .p_1()
                                                        .gap_1()
                                                        .bg(rgb(sidebar))
                                                        .child(div().h_1().w_full().bg(rgb(accent)))
                                                        .child(
                                                            div()
                                                                .h_1()
                                                                .w_full()
                                                                .bg(rgb(ink).opacity(0.2)),
                                                        ),
                                                )
                                                .child(
                                                    h_flex().flex_1().justify_center().child(
                                                        v_flex()
                                                            .w(px(34.))
                                                            .h(px(46.))
                                                            .p_1()
                                                            .gap_1()
                                                            .bg(rgb(0xffffff))
                                                            .child(
                                                                div()
                                                                    .h_1()
                                                                    .w_full()
                                                                    .bg(rgb(accent)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .h(px(2.))
                                                                    .w_full()
                                                                    .bg(rgb(0xc9cecb)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .h(px(2.))
                                                                    .w_full()
                                                                    .bg(rgb(0xc9cecb)),
                                                            ),
                                                    ),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .w_full()
                                                .gap_1()
                                                .text_xs()
                                                .child(div().flex_1().child(palette.label()))
                                                .when(active, |this| {
                                                    this.child(
                                                        Icon::new(IconName::Check).size(px(12.)),
                                                    )
                                                }),
                                        ),
                                )
                                .on_click(move |_, _, cx| {
                                    database
                                        .read(cx)
                                        .queue(move |write| settings::set_palette(write, palette));
                                })
                            }),
                    ),
                )
        }))
        .into_any_element()
}
