//! 라이브러리가 없을 때의 온보딩 화면.

use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::FluentBuilder;
use gpui_kit::gpui::*;

use super::HanjiApp;
use super::style::*;
use crate::{APP_NAME, config};

impl HanjiApp {
    /// 라이브러리가 없을 때의 온보딩 화면.
    pub(super) fn render_onboarding(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let suggested = config::default_library_root()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| t!("library.onboarding.location_unknown").to_string());

        div()
            .flex_1()
            .size_full()
            .flex()
            .flex_col()
            // macOS: 투명 타이틀바 자리에서 창을 끌 수 있게 한다.
            .child(window_drag_area(
                div().w_full().h(px(TOOLBAR_HEIGHT)).flex_shrink_0(),
            ))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(div().text_2xl().font_bold().child(APP_NAME))
                    .child(
                        div()
                            .text_sm()
                            .text_color(pal.muted)
                            .child(t!("library.onboarding.prompt")),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(pal.border)
                            .bg(pal.panel)
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("library.onboarding.suggested", path = suggested)),
                    )
                    .when_some(self.notice.clone(), |this, notice| {
                        this.child(div().text_sm().text_color(pal.danger).child(notice))
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("create-library")
                                    .primary()
                                    .label(t!("library.onboarding.create"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.create_default_library(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("open-library")
                                    .label(t!("library.onboarding.open"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_library(window, cx)
                                    })),
                            ),
                    ),
            )
    }
}
