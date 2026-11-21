// The "Continue with ChatGPT" button in the format OpenAI approves for Sign in
// with ChatGPT (its UI/UX guidelines): black with the white logo, or white
// with the black logo, not the app's own button colors. White on a dark
// theme, where black would sink into the page.

use std::sync::{Arc, LazyLock};

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    App, ClickEvent, ElementId, FontWeight, Image, ImageFormat, IntoElement, SharedString, Window,
    div, img, prelude::*, px, rgb,
};

/// OpenAI's logos for the button, as its guidelines serve them.
static WHITE_LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| {
    let bytes = include_bytes!("../../assets/chatgpt/logo-white.svg");
    Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.to_vec()))
});
static BLACK_LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| {
    let bytes = include_bytes!("../../assets/chatgpt/logo-black.svg");
    Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.to_vec()))
});

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct ContinueWithChatGpt {
    id: ElementId,
    label: SharedString,
    disabled: bool,
    on_click: Option<OnClick>,
}

impl RenderOnce for ContinueWithChatGpt {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let (background, text, logo) = match dark {
            true => (rgb(0xffffff), rgb(0x000000), BLACK_LOGO.clone()),
            false => (rgb(0x000000), rgb(0xffffff), WHITE_LOGO.clone()),
        };
        let disabled = self.disabled;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(45.))
            .px(px(20.))
            .rounded(px(12.))
            .bg(background)
            .text_color(text)
            .text_size(px(15.))
            .font_weight(FontWeight::MEDIUM)
            .when(disabled, |this| this.opacity(0.6))
            .when(!disabled, |this| this.cursor_pointer())
            .child(img(logo).size(px(20.)).flex_none())
            .child(self.label)
            .when_some(self.on_click.filter(|_| !disabled), |this, on_click| {
                this.on_click(on_click)
            })
            .test_support()
    }
}

impl ContinueWithChatGpt {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: "Continue with ChatGPT".into(),
            disabled: false,
            on_click: None,
        }
    }

    /// In place of "Continue with ChatGPT", while it's waiting.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}
