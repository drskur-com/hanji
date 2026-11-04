//! 앱 정보 대화상자: 아이콘·이름·버전·한 줄 소개·만든 사람·쓴 아이콘 팩.

use std::sync::{Arc, LazyLock};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::gpui::*;

use super::HanjiApp;
use super::style::Palette;
use crate::{APP_NAME, icons};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 360.0;

/// 앱 아이콘 한 변.
const ICON_SIZE: f32 = 96.0;

/// 앱 아이콘(Dock 아이콘과 같은 그림).
static APP_ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        include_bytes!("../../resources/icon/hanji-512.png").to_vec(),
    ))
});

impl HanjiApp {
    /// 앱 정보 대화상자를 연다.
    pub(super) fn open_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, cx| {
            let pal = Palette::from_theme(cx.theme());
            dialog
                .w(px(DIALOG_WIDTH))
                .close_button(false)
                .child(about_body(pal))
                .footer(
                    DialogFooter::new().child(
                        Button::new("about-close")
                            .primary()
                            .label(t!("common.close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
    }
}

fn about_body(pal: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .pt_2()
        .child(
            img(APP_ICON.clone())
                .size(px(ICON_SIZE))
                .object_fit(ObjectFit::Contain),
        )
        .child(
            div()
                .mt_3()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pal.foreground)
                .child(APP_NAME),
        )
        .child(
            div()
                .mt_1()
                .text_sm()
                .text_color(pal.muted)
                .child(t!("about.version", version = env!("CARGO_PKG_VERSION")).to_string()),
        )
        .child(
            div()
                .mt_3()
                .text_sm()
                .text_center()
                .text_color(pal.foreground)
                .child(t!("about.tagline").to_string()),
        )
        .child(
            div()
                .mt_4()
                .pt_3()
                .w_full()
                .border_t_1()
                .border_color(pal.border)
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(pal.muted)
                .child(t!("about.icons", pack = icons::PACK).to_string())
                .child(t!("about.copyright", year = "2026", author = "ChangHun Yoon").to_string()),
        )
}
