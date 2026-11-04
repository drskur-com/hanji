//! 각주 삽입 대화상자: 내용을 받아 커서 자리에 `[^N]`을, 시트 끝에 `[^N]: 내용`을 넣는다.
//!
//! 번호와 정의 자리는 `text_transform::insert_footnote`가 정한다. 책·내보내기는 나온 순서로
//! 번호를 다시 매기므로(`markdown::book`) 시트의 이름은 짝을 잇는 데만 쓰인다.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{FocusableExt as _, WindowExt as _};
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::rc::Rc;

use super::HanjiApp;
use crate::InsertFootnote;

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 440.0;

impl HanjiApp {
    /// 각주 삽입 대화상자를 연다. 열린 시트가 없으면 안내만 한다.
    pub(super) fn on_insert_footnote(
        &mut self,
        _: &InsertFootnote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        let target = editor.state().clone();
        let app = cx.entity().downgrade();
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("insert.footnote.placeholder")));
        // 넣었으면 `true`(대화상자를 닫는다). 내용이 비었으면 그대로 둔다.
        let submit = {
            let input = input.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let content = input.read(cx).value().trim().to_string();
                if content.is_empty() {
                    return false;
                }
                app.update(cx, |app, cx| {
                    app.insert_footnote(&target, &content, window, cx)
                })
                .ok();
                true
            })
        };
        // 처음 그릴 때만 입력 칸에 포커스를 줘서 바로 쓰고 Enter로 넣을 수 있게 한다.
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            let (enter, ok) = (submit.clone(), submit.clone());
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("insert.footnote.title"))
                .close_button(false)
                // Enter는 대화상자의 확인(Confirm)이다. 여기서 넣고, 비었으면 닫지 않는다.
                .on_ok(move |_, window, cx| enter(window, cx))
                .child(Input::new(&input).focus_ring(false))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-footnote-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("insert-footnote-ok")
                                .primary()
                                .label(t!("common.insert"))
                                .on_click(move |_, window, cx| {
                                    if ok(window, cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    /// 대화상자를 연 시트(`target`)가 아직 열려 있으면 커서(선택 끝) 자리에 각주를 넣는다.
    fn insert_footnote(
        &mut self,
        target: &Entity<gpui_kit::component::input::EditorState>,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.state() != target {
            self.notify(t!("insert.footnote.sheet_closed"), window, cx);
            return;
        }
        editor.state().update(cx, |state, cx| {
            let text = state.value();
            let at = state.selected_range().end.min(text.len());
            let (range, replacement, cursor) =
                crate::text_transform::insert_footnote(&text, at, content);
            state.set_selected_range(range, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range(cursor..cursor, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }
}
