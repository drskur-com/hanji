//! 서식 메뉴의 편집 도구(굵게·기울임·링크·문단 형식): Markdown 원문을 바꾸어 에디터의 기존 렌더링에 반영한다.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{EditorState, Input, InputState};
use gpui_kit::component::{FocusableExt as _, WindowExt as _};
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use super::HanjiApp;
use crate::markdown;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockFormat {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Quote,
}

/// 들여쓰기는 남기고 한 단계의 문단 서식 기호를 읽는다.
fn block_format(line: &str) -> (BlockFormat, usize) {
    let body = line.trim_start_matches([' ', '\t']);
    let indent = line.len() - body.len();
    if let Some(level) = markdown::heading_level(body) {
        return (
            BlockFormat::Heading(level),
            indent + markdown::heading_marker_len(body),
        );
    }
    if body.starts_with("> ") || body == ">" {
        return (BlockFormat::Quote, indent + body.len().min(2));
    }
    if ["- ", "* ", "+ "]
        .iter()
        .any(|prefix| body.starts_with(prefix))
    {
        return (BlockFormat::Bullet, indent + 2);
    }
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0
        && digits <= 9
        && (body[digits..].starts_with(". ") || body[digits..].starts_with(") "))
    {
        return (BlockFormat::Numbered, indent + digits + 2);
    }
    (BlockFormat::Paragraph, indent)
}

fn line_range(text: &str, at: usize) -> Range<usize> {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    start..end
}

/// 파서가 확인한 인라인 서식만 해제한다. 표 머리 행의 굵기는 Markdown 기호가 아니다.
fn inline_range(
    text: &str,
    selection: &Range<usize>,
    kind: markdown::SpanKind,
) -> Option<Range<usize>> {
    markdown::spans(text).into_iter().rev().find_map(|span| {
        let source = &text[span.range.clone()];
        let marked = match kind {
            markdown::SpanKind::Strong => ["**", "__"].iter().any(|marker| {
                source.len() >= 4 && source.starts_with(marker) && source.ends_with(marker)
            }),
            markdown::SpanKind::Emphasis => ["*", "_"].iter().any(|marker| {
                source.len() >= 2 && source.starts_with(marker) && source.ends_with(marker)
            }),
            _ => false,
        };
        (span.kind == kind
            && span.range.start <= selection.start
            && selection.end <= span.range.end
            && marked)
            .then_some(span.range)
    })
}

impl HanjiApp {
    pub(super) fn toggle_inline(
        &mut self,
        kind: markdown::SpanKind,
        marker: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else { return };
        let changed = editor.state().update(cx, |state, cx| {
            let text = state.value();
            let selected = state.selected_range();
            let (range, replacement, selection) =
                if let Some(range) = inline_range(&text, &selected, kind) {
                    let source = &text[range.clone()];
                    let width = if kind == markdown::SpanKind::Strong {
                        2
                    } else {
                        1
                    };
                    let body = source[width..source.len() - width].to_string();
                    let end = range.start + body.len();
                    let selection = if selected.is_empty() {
                        let cursor = selected.start.saturating_sub(width).clamp(range.start, end);
                        cursor..cursor
                    } else {
                        range.start..end
                    };
                    (range, body, selection)
                } else if selected.is_empty() {
                    // 선택 없이 누르면(이미 서식 안이 아니면) 아무것도 하지 않는다. 자리 글자를 넣지 않는다.
                    return false;
                } else {
                    let body = &text[selected.clone()];
                    let start = selected.start + marker.len();
                    (
                        selected,
                        format!("{marker}{body}{marker}"),
                        start..start + body.len(),
                    )
                };
            state.set_selected_range(range, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range(selection, cx);
            state.focus(window, cx);
            true
        });
        if changed {
            self.on_editor_changed(window, cx);
        }
    }

    /// 서식 메뉴의 문단 형식 항목: 지금 커서(선택)가 있는 줄들의 문단 형식을 바꾼다.
    fn apply_block_format(
        &mut self,
        format: BlockFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        let target = editor.state().clone();
        let selection = target.read(cx).selected_range();
        self.set_block_format(&target, selection, format, window, cx);
    }

    pub(super) fn on_insert_link(
        &mut self,
        _: &crate::InsertLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_none() {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        }
        self.open_link_dialog(window, cx);
    }

    pub(super) fn on_format_body(
        &mut self,
        _: &crate::FormatBody,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Paragraph, window, cx);
    }

    pub(super) fn on_format_heading1(
        &mut self,
        _: &crate::FormatHeading1,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Heading(1), window, cx);
    }

    pub(super) fn on_format_heading2(
        &mut self,
        _: &crate::FormatHeading2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Heading(2), window, cx);
    }

    pub(super) fn on_format_heading3(
        &mut self,
        _: &crate::FormatHeading3,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Heading(3), window, cx);
    }

    pub(super) fn on_format_heading4(
        &mut self,
        _: &crate::FormatHeading4,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Heading(4), window, cx);
    }

    pub(super) fn on_format_heading5(
        &mut self,
        _: &crate::FormatHeading5,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Heading(5), window, cx);
    }

    pub(super) fn on_format_heading6(
        &mut self,
        _: &crate::FormatHeading6,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Heading(6), window, cx);
    }

    pub(super) fn on_format_bullet_list(
        &mut self,
        _: &crate::FormatBulletList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Bullet, window, cx);
    }

    pub(super) fn on_format_numbered_list(
        &mut self,
        _: &crate::FormatNumberedList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Numbered, window, cx);
    }

    pub(super) fn on_format_quote(
        &mut self,
        _: &crate::FormatQuote,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_block_format(BlockFormat::Quote, window, cx);
    }

    fn set_block_format(
        &mut self,
        target: &Entity<EditorState>,
        selection: Range<usize>,
        format: BlockFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else { return };
        if editor.state() != target {
            return;
        }
        editor.state().update(cx, |state, cx| {
            let text = state.value();
            let last = if !selection.is_empty() && text[..selection.end].ends_with('\n') {
                selection.end - 1
            } else {
                selection.end
            };
            let range = line_range(&text, selection.start).start..line_range(&text, last).end;
            let mut cursor = range.start;
            let replacement = text[range.clone()]
                .split('\n')
                .enumerate()
                .map(|(index, line)| {
                    let (_, marker_end) = block_format(line);
                    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                    let prefix = match format {
                        BlockFormat::Paragraph => String::new(),
                        BlockFormat::Heading(level) => {
                            format!("{} ", "#".repeat(usize::from(level)))
                        }
                        BlockFormat::Bullet => "- ".to_string(),
                        BlockFormat::Numbered => format!("{}. ", index + 1),
                        BlockFormat::Quote => "> ".to_string(),
                    };
                    if index == 0 {
                        cursor += indent
                            + prefix.len()
                            + selection
                                .start
                                .saturating_sub(range.start + marker_end)
                                .min(line.len() - marker_end);
                    }
                    format!("{}{prefix}{}", &line[..indent], &line[marker_end..])
                })
                .collect::<Vec<_>>()
                .join("\n");
            let next_selection = if selection.is_empty() {
                cursor..cursor
            } else {
                range.start..range.start + replacement.len()
            };
            state.set_selected_range(range, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range(next_selection, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }

    fn open_link_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else { return };
        let target = editor.state().clone();
        let selection = target.read(cx).selected_range();
        let selected_text = target.read(cx).value()[selection.clone()].to_string();
        let label = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("editor.link.text_placeholder")));
        label.update(cx, |state, cx| state.set_value(selected_text, window, cx));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://"));
        let app = cx.entity().downgrade();
        let submit = {
            let label = label.clone();
            let url = url.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let url = url.read(cx).value().trim().to_string();
                if url.is_empty() {
                    return false;
                }
                let label = label.read(cx).value().to_string();
                let label = if label.trim().is_empty() {
                    url.clone()
                } else {
                    label
                };
                app.update(cx, |app, cx| {
                    let Some(editor) = &app.editor else { return };
                    if editor.state() != &target {
                        app.notify(t!("editor.link.sheet_closed"), window, cx);
                        return;
                    }
                    let label = label
                        .replace('\\', "\\\\")
                        .replace('[', "\\[")
                        .replace(']', "\\]")
                        .replace(['\n', '\r'], " ");
                    let url = url
                        .replace('\\', "%5C")
                        .replace('<', "%3C")
                        .replace('>', "%3E")
                        .replace(['\n', '\r'], "");
                    let replacement = format!("[{label}](<{url}>)");
                    editor.state().update(cx, |state, cx| {
                        let cursor = selection.start + replacement.len();
                        state.set_selected_range(selection.clone(), cx);
                        state.replace(replacement, window, cx);
                        state.set_selected_range(cursor..cursor, cx);
                        state.focus(window, cx);
                    });
                    app.on_editor_changed(window, cx);
                })
                .ok();
                true
            })
        };
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                url.update(cx, |state, cx| state.focus(window, cx));
            }
            let (enter, ok) = (submit.clone(), submit.clone());
            dialog
                .w(px(440.))
                .title(t!("editor.link.dialog_title"))
                .close_button(false)
                .on_ok(move |_, window, cx| enter(window, cx))
                .child(Input::new(&label).focus_ring(false))
                .child(Input::new(&url).focus_ring(false))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-link-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("insert-link-ok")
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
}
