//! 열·본문 행 수를 받아 커서 위치에 Markdown 표를 삽입한다.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{FocusableExt as _, WindowExt as _};
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::rc::Rc;

use super::HanjiApp;
use crate::InsertTable;

impl HanjiApp {
    pub(super) fn on_insert_table(
        &mut self,
        _: &InsertTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        let target = editor.state().clone();
        let app = cx.entity().downgrade();
        let columns = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("insert.table.columns_placeholder"))
        });
        let rows = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("insert.table.rows_placeholder")));
        let submit = {
            let columns = columns.clone();
            let rows = rows.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let count = |value: &str, default: usize| {
                    if value.trim().is_empty() {
                        Some(default)
                    } else {
                        value
                            .trim()
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=100).contains(n))
                    }
                };
                let (Some(columns), Some(rows)) = (
                    count(&columns.read(cx).value(), 3),
                    count(&rows.read(cx).value(), 2),
                ) else {
                    app.update(cx, |app, cx| {
                        app.notify(t!("insert.table.invalid"), window, cx);
                    })
                    .ok();
                    return false;
                };
                app.update(cx, |app, cx| {
                    app.insert_table(&target, columns, rows, window, cx);
                })
                .ok();
                true
            })
        };
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                columns.update(cx, |state, cx| state.focus(window, cx));
            }
            let (enter, ok) = (submit.clone(), submit.clone());
            dialog
                .w(px(440.))
                .title(t!("insert.table.title"))
                .close_button(false)
                .on_ok(move |_, window, cx| enter(window, cx))
                .child(Input::new(&columns).focus_ring(false))
                .child(Input::new(&rows).focus_ring(false))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-table-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("insert-table-ok")
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

    fn insert_table(
        &mut self,
        target: &Entity<gpui_kit::component::input::EditorState>,
        columns: usize,
        rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.state() != target {
            self.notify(t!("insert.table.sheet_closed"), window, cx);
            return;
        }
        editor.state().update(cx, |state, cx| {
            let text = state.value();
            let at = state.selected_range().end.min(text.len());
            let before = &text[..at];
            let after = &text[at..];
            let mut table = String::new();
            if !before.is_empty() {
                if !before.ends_with('\n') {
                    table.push('\n');
                }
                if !before.ends_with("\n\n") {
                    table.push('\n');
                }
            }
            let cursor = at + table.len() + 2;
            table.push_str("| ");
            for column in 1..=columns {
                table.push_str(&format!("열 {column} | "));
            }
            table.push_str("\n|");
            for _ in 0..columns {
                table.push_str(" --- |");
            }
            table.push('\n');
            for _ in 0..rows {
                table.push('|');
                for _ in 0..columns {
                    table.push_str("   |");
                }
                table.push('\n');
            }
            if !after.starts_with('\n') {
                table.push('\n');
            }
            state.set_selected_range(at..at, cx);
            state.replace(table, window, cx);
            state.set_selected_range(cursor..cursor, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }
}
