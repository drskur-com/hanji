//! 에디터 영역: 떠 있는 툴바 + 본문, 열린 시트의 편집·자동 저장.

use gpui_base::input::{LineContext, LineStyle};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{
    Editor, EditorState, InputEvent, TextDecoration, TextDecorationCollection,
};
use gpui_kit::gpui::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::gpui::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use super::HanjiApp;
use super::editor_extension::ExtensionBlocks;
use super::editor_image::ImageBlocks;
use super::editor_layout::{BookLayout, PAGE_BREAK_HEIGHT};
use super::editor_table::TableBlocks;
use super::style::*;
use crate::markdown::{self, Span, SpanKind};
use crate::text_transform::LineGroup;
use crate::{TogglePreview, icons, library};

/// 마지막 입력 후 자동 저장까지 기다리는 시간.
const AUTOSAVE_DELAY: Duration = Duration::from_secs(1);

/// 마지막 입력 후 마크업 표시를 다시 계산하기까지 기다리는 시간.
/// 그 사이 기존 표시는 편집을 따라 움직인다.
const MARKUP_DELAY: Duration = Duration::from_millis(150);

/// 마크업 기호(`#`, `**`, `[]()` 등)를 흐리게 하는 정도.
const MARKER_FADE: f32 = 0.6;

/// 인용 본문을 흐리게 하는 정도.
const QUOTE_FADE: f32 = 0.3;

/// 완료한 할 일 항목 글 흐림(취소선과 함께).
const TASK_DONE_FADE: f32 = 0.5;

/// 본문 최대 폭(가독 폭). 창이 넓으면 가운데에 둔다.
pub(super) const EDITOR_MAX_WIDTH: f32 = 680.0;

/// 커서를 화면 위·아래 끝에서 떨어뜨려 두는 줄 수. 마지막 줄 아래에도 그만큼 빈 공간을 둬서
/// 문서 끝에서 입력할 때도 이 여백이 유지된다.
const CURSOR_MARGIN_LINES: usize = 3;

/// 본문 좌우 최소 여백.
const EDITOR_MIN_SIDE_PADDING: f32 = 24.0;

fn configure_text_transform(state: &mut EditorState, styles: Option<&library::Styles>) {
    let options = styles
        .map(|styles| styles.input_transform.clone())
        .unwrap_or_default();
    let paste_options = options.clone();
    if options.is_enabled() {
        state.set_typing_transform(move |document, range, inserted| {
            crate::text_transform::on_input(document, range, inserted, &options)
        });
    } else {
        state.clear_typing_transform();
    }
    state.set_paste_transform(move |text, same_format| {
        crate::text_transform::paste(text, same_format, &paste_options)
    });
}

/// 에디터에 열린 시트.
pub(super) struct OpenSheet {
    path: PathBuf,
    state: Entity<EditorState>,
    /// 저장하지 않은 변경이 있는지.
    dirty: bool,
    /// 마지막으로 읽거나 저장한 시점의 파일 수정 시각. 외부 수정 감지에 쓴다.
    disk_modified: u128,
    words: usize,
    /// 자동 저장 대기 태스크. 입력이 오면 새로 만들어 이전 대기를 취소한다.
    autosave: Option<Task<()>>,
    markup: Markup,
    /// 마크업 재계산 대기 태스크(자동 저장과 같은 방식).
    markup_refresh: Option<Task<()>>,
    /// 지금 적용한 좌우 여백. 영역 폭이 바뀔 때만 다시 넣는다.
    side_padding: Rc<Cell<Pixels>>,
    /// 시트가 든 프로젝트 폴더(프로젝트 밖이면 `None`).
    project: Option<PathBuf>,
    /// 적용한 책 스타일. 서체·줄 간격·제목 크기를 에디터에 반영한다.
    /// 프로젝트 밖(메모)이면 `None`: 책 스타일을 쓰지 않는다.
    styles: Option<library::Styles>,
    /// 줄 묶음(ADR-0016). 줄 스타일이 읽으므로 본문이 바뀔 때마다 맞춘다([`sync_line_groups`]).
    line_groups: Rc<RefCell<Vec<LineGroup>>>,
    /// `line_groups`를 맞춘 때의 줄 수.
    line_count: usize,
    /// 표(ADR-0017). 줄 스타일·줄 블록이 읽으므로 본문이 바뀔 때마다 맞춘다([`sync_tables`]).
    tables: Rc<TableBlocks>,
    /// 확장 블록(ADR-0026). 표와 같이 본문이 바뀔 때마다 맞춘다.
    extensions: Rc<ExtensionBlocks>,
    _subscription: Subscription,
}

/// 줄 스타일: 책 스타일대로 문단·제목을 배치한다(ADR-0010). 제목 줄 확대는 gpui-base 패치(ADR-0006),
/// 이미지 줄은 커서가 들어올 때만 글자를 보인다(ADR-0008). 책 스타일이 없으면 기본 배치다.
/// 줄 묶음 안의 줄은 한 문단의 행으로 배치한다(ADR-0016). 표의 줄은 커서가 표에 닿을 때만 글자를
/// 보이고, 마지막 행 아래에 격자를 그린다(ADR-0017). 확장 블록도 같다(ADR-0026).
fn line_style(
    styles: Option<&library::Styles>,
    groups: Rc<RefCell<Vec<LineGroup>>>,
    tables: Rc<TableBlocks>,
    extensions: Rc<ExtensionBlocks>,
    font_size: f32,
    cx: &App,
) -> impl Fn(LineContext<'_>) -> LineStyle + 'static {
    let layout = book_layout(styles, font_size);
    // 체크해도 할 일 글이 밀리지 않도록 `[ ]`·`[x]` 폭 차이를 첫 행 여백으로 메운다(ADR-0023).
    let (unchecked, checked) =
        super::editor_task::check_indents(&editor_font(styles, cx), font_size, cx);
    move |line| {
        let row = line.row;
        let mut style = layout.line_style(line, &groups.borrow());
        if tables.contains(row) || extensions.contains(row) {
            style.collapsed = true;
            style.reveal_run = true;
            // 붙어 있는 다른 표·블록까지 함께 펼치지 않는다.
            style.run_start = tables.starts(row) || extensions.starts(row);
        }
        if let Some(marker) = markdown::task_marker(line.text) {
            style.first_indent += if marker.checked { checked } else { unchecked };
        }
        // `scale`이 큰 인라인 확장이 들어가도록 행을 키운다(ADR-0036).
        let inline = super::editor_extension::inline_scale(line.text);
        if inline > style.height_scale {
            style.height_scale = inline;
        }
        style
    }
}

/// 본문에 맞춰 표를 다시 찾고, 바뀐 표가 닿았던 줄과 닿는 줄을 다시 배치한다.
fn sync_tables(editor: &OpenSheet, text: &str, cx: &mut App) {
    let mut rows = editor.tables.sync(text);
    rows.extend(editor.extensions.sync(text));
    if rows.is_empty() {
        return;
    }
    editor.state.update(cx, |state, cx| {
        for rows in rows {
            state.restyle_lines(rows, cx);
        }
    });
}

/// 에디터 본문 서체: 책 스타일의 본문 서체, 없으면 환경설정의 에디터 서체.
fn editor_font(styles: Option<&library::Styles>, cx: &App) -> SharedString {
    styles
        .and_then(|styles| styles.resolve(library::StyleTarget::Body).font)
        .map_or_else(
            || crate::settings::Prefs::editor_font_family(cx),
            SharedString::from,
        )
}

/// 본문에 맞춰 줄 묶음을 다시 구하고, 바뀌었으면 묶음이 닿았던 줄과 닿는 줄을 다시 배치한다.
///
/// 편집한 줄은 엔진이 편집 시점(묶음을 맞추기 전)에 배치하므로 옛 묶음 기준일 수 있다. 펜스를
/// 고치면 편집하지 않은 줄의 소속도 바뀐다. 두 경우 모두 옛 묶음과 새 묶음이 걸친 줄 안이고, 줄이
/// 밀린 만큼(줄 수 차이) 더 넓혀 본다.
fn sync_line_groups(editor: &mut OpenSheet, text: &str, cx: &mut App) {
    let groups = crate::text_transform::line_groups(text);
    let line_count = text.split('\n').count();
    let shift = line_count.abs_diff(editor.line_count);
    editor.line_count = line_count;
    let old = editor.line_groups.replace(groups);
    let new = editor.line_groups.borrow();
    if *new == old {
        return;
    }
    let spans = old
        .iter()
        .chain(new.iter())
        .map(|group| group.lines.clone());
    let start = spans.clone().map(|rows| rows.start).min().unwrap_or(0);
    let end = spans.map(|rows| rows.end).max().unwrap_or(0) + shift;
    drop(new);
    editor
        .state
        .update(cx, |state, cx| state.restyle_lines(start..end, cx));
}

fn book_layout(styles: Option<&library::Styles>, font_size: f32) -> BookLayout {
    styles.map_or_else(
        || BookLayout::plain(font_size),
        |styles| BookLayout::new(styles, font_size),
    )
}

/// 제목 레벨(H1~H6)별 굵기. 책 스타일이 없거나 부·장·절이 아니면 굵게.
fn heading_weights(styles: Option<&library::Styles>) -> [FontWeight; 6] {
    std::array::from_fn(|index| {
        let target = library::StyleTarget::for_heading(index as u8 + 1);
        match (styles, target) {
            (Some(styles), Some(target)) => {
                FontWeight(f32::from(styles.resolve(target).font_weight))
            }
            _ => FontWeight::BOLD,
        }
    })
}

/// "여기서 페이지가 나뉩니다" 표시: 가운데 글자 양옆으로 가는 선.
fn page_break_mark(size: Size<Pixels>, cx: &App) -> AnyElement {
    let pal = Palette::from_theme(cx.theme());
    let rule = || div().h(px(1.0)).flex_1().bg(pal.panel_border);
    div()
        .w(size.width)
        .h(size.height)
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(pal.muted)
        .child(rule())
        .child(t!("editor.page_break"))
        .child(rule())
        .into_any_element()
}

/// 마크다운 마크업 표시. 겹치는 스팬이 같은 스타일 속성을 다투지 않도록 종류별로 나눈다.
/// 먼저 만든 묶음이 우선한다.
struct Markup {
    /// ATX 제목 굵기와 `#` 흐림. 줄 크기와 함께 바뀌도록 키 입력마다 즉시 다시 계산한다.
    headings: TextDecorationCollection,
    /// 펼친 인라인 확장(`{{…}}`)의 원문 흐림. 객체가 펼쳐지는 대로 보이도록 제목과 함께 즉시 계산한다.
    calls: TextDecorationCollection,
    /// 제목 레벨(H1~H6)별 굵기. 책 스타일이 있으면 H1~H3은 부·장·절 굵기를 쓴다.
    heading_weights: [FontWeight; 6],
    /// 기호 흐림.
    markers: TextDecorationCollection,
    /// 굵게·기울임·취소선·인라인 코드·링크.
    inline: TextDecorationCollection,
    /// setext 제목·인용·코드 블록.
    blocks: TextDecorationCollection,
}

impl Markup {
    fn new(state: &Entity<EditorState>, styles: Option<&library::Styles>, cx: &mut App) -> Self {
        state.update(cx, |state, cx| Self {
            headings: state.create_decorations_collection(Vec::new(), cx),
            calls: state.create_decorations_collection(Vec::new(), cx),
            heading_weights: heading_weights(styles),
            markers: state.create_decorations_collection(Vec::new(), cx),
            inline: state.create_decorations_collection(Vec::new(), cx),
            blocks: state.create_decorations_collection(Vec::new(), cx),
        })
    }

    /// ATX 제목과 인라인 확장 표시를 원문에서 바로 다시 계산한다. 줄 텍스트만 보므로 긴 문서도 가볍다.
    fn apply_lines(&self, text: &str, cx: &mut App) {
        let decorations = markdown::heading_spans(text)
            .into_iter()
            .map(|span| {
                let style = match span.kind {
                    SpanKind::Marker => HighlightStyle {
                        fade_out: Some(MARKER_FADE),
                        ..Default::default()
                    },
                    SpanKind::Heading(level) => self.heading_weights[usize::from(level) - 1].into(),
                    _ => FontWeight::BOLD.into(),
                };
                TextDecoration::new(span.range, style)
            })
            .collect();
        self.headings.set(decorations, cx);
        // 객체로 보이는 동안은 가려지고, 커서가 닿아 펼치면 원문이 흐리게 보인다(ADR-0036).
        let mut calls = Vec::new();
        let mut start = 0;
        for line in text.split('\n') {
            calls.extend(
                crate::extension::inline_calls(line)
                    .into_iter()
                    .map(|call| {
                        TextDecoration::new(
                            start + call.range.start..start + call.range.end,
                            HighlightStyle {
                                fade_out: Some(MARKER_FADE),
                                ..Default::default()
                            },
                        )
                    }),
            );
            start += line.len() + 1;
        }
        self.calls.set(calls, cx);
    }

    /// 파서 스팬으로 표시를 바꾼다. 색은 지금 테마에서 가져온다.
    fn apply(&self, spans: Vec<Span>, cx: &mut App) {
        let theme = cx.theme();
        let code_background = theme.muted;
        let link = theme.link;
        let (mut markers, mut inline, mut blocks) = (Vec::new(), Vec::new(), Vec::new());
        for span in spans {
            let (target, style) = match span.kind {
                SpanKind::Marker => (
                    &mut markers,
                    HighlightStyle {
                        fade_out: Some(MARKER_FADE),
                        ..Default::default()
                    },
                ),
                SpanKind::Strong => (&mut inline, FontWeight::BOLD.into()),
                SpanKind::Emphasis => (&mut inline, FontStyle::Italic.into()),
                SpanKind::Strikethrough => (
                    &mut inline,
                    HighlightStyle {
                        strikethrough: Some(StrikethroughStyle {
                            thickness: px(1.0),
                            color: None,
                        }),
                        ..Default::default()
                    },
                ),
                // 인용 흐림(`blocks`)보다 먼저 만든 묶음이라 인용 안의 할 일도 이 흐림을 쓴다.
                SpanKind::TaskDone => (
                    &mut inline,
                    HighlightStyle {
                        fade_out: Some(TASK_DONE_FADE),
                        strikethrough: Some(StrikethroughStyle {
                            thickness: px(1.0),
                            color: None,
                        }),
                        ..Default::default()
                    },
                ),
                SpanKind::InlineCode => (
                    &mut inline,
                    HighlightStyle {
                        background_color: Some(code_background),
                        ..Default::default()
                    },
                ),
                SpanKind::Link => (
                    &mut inline,
                    HighlightStyle {
                        color: Some(link),
                        underline: Some(UnderlineStyle {
                            thickness: px(1.0),
                            color: None,
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ),
                // 각주 번호는 링크 색으로 띄우고 `[^` `]`는 기호로 흐린다(위 첨자 대신).
                SpanKind::FootnoteRef => (
                    &mut inline,
                    HighlightStyle {
                        color: Some(link),
                        ..Default::default()
                    },
                ),
                // setext 제목. ATX 제목은 `apply_lines`가 맡는다.
                SpanKind::Heading(_) => (&mut blocks, FontWeight::BOLD.into()),
                SpanKind::Quote => (
                    &mut blocks,
                    HighlightStyle {
                        fade_out: Some(QUOTE_FADE),
                        ..Default::default()
                    },
                ),
                SpanKind::CodeBlock => (
                    &mut blocks,
                    HighlightStyle {
                        background_color: Some(code_background),
                        ..Default::default()
                    },
                ),
            };
            target.push(TextDecoration::new(span.range, style));
        }
        self.markers.set(markers, cx);
        self.inline.set(inline, cx);
        self.blocks.set(blocks, cx);
    }
}

impl HanjiApp {
    /// 설정 변경을 열린 에디터의 줄 배치와 표 격자에 즉시 반영한다.
    pub(super) fn refresh_editor_preferences(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let font_size = crate::settings::Prefs::editor_font_size(cx);
        editor.tables.set_font_size(px(font_size));
        editor
            .tables
            .set_family(&editor_font(editor.styles.as_ref(), cx));
        let style = line_style(
            editor.styles.as_ref(),
            editor.line_groups.clone(),
            editor.tables.clone(),
            editor.extensions.clone(),
            font_size,
            cx,
        );
        editor.state.update(cx, |state, cx| {
            state.set_line_style(Some(style), cx);
            state.refresh_line_blocks(cx);
        });
    }

    /// 에디터 영역: 떠 있는 툴바 + 본문(빈 상태 또는 선택한 시트).
    pub(super) fn render_editor_column(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let toolbar = window_drag_area(
            div()
                .h(px(TOOLBAR_HEIGHT))
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .gap_2(),
        )
        // 시트 목록 칸이 숨은 책 스타일 화면에서는 패널 다시 열기 버튼을 여기 둔다.
        .when(self.book_style.is_some() && !self.sidebar_visible, |this| {
            #[cfg(target_os = "macos")]
            let this = this.pl(px(TRAFFIC_LIGHT_SPACE));
            this.child(tool_group(pal).child(
                icon_button("show-sidebar", icons::panel(), true, pal).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.set_sidebar_visible(true, cx);
                    },
                )),
            ))
        })
        .child(
            tool_group(pal)
                .child(icon_button("nav-back", icons::back(), false, pal))
                .child(icon_button("nav-forward", icons::forward(), false, pal)),
        )
        .child(tool_group(pal).child(
            icon_button("search", icons::search(), self.library.is_some(), pal).on_click(
                cx.listener(|this, _, window, cx| {
                    this.on_search_sheets(&crate::SearchSheets, window, cx)
                }),
            ),
        ))
        .child(div().flex_1())
        // 파일 뷰어에서는 편집 도구 대신 파일 열기·보기·정보 버튼을 둔다.
        .map(|this| match &self.open_file {
            Some(file) => this.child(self.file_actions(file, pal)),
            // 책 스타일 화면에는 편집 도구가 없다.
            None if self.book_style.is_some() => this,
            None => this.child(
                tool_group(pal)
                    .child(
                        icon_button(
                            "editor-compile",
                            icons::build(),
                            self.editor_selection().is_some(),
                            pal,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_selection_compile_dialog(window, cx);
                        })),
                    )
                    .child(
                        icon_button(
                            "toggle-preview",
                            icons::book_preview(),
                            self.editor_selection().is_some(),
                            pal,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_toggle_preview(&TogglePreview, window, cx)
                        })),
                    ),
            ),
        })
        .when(!self.assistant_open(), |this| {
            this.child(self.assistant_toggle(cx, pal))
        });

        let body: AnyElement = match &self.editor {
            None if let Some(view) = &self.book_style => view.clone().into_any_element(),
            // 파일 그룹의 파일을 골랐으면 에디터 대신 뷰어를 보여 준다.
            None if let Some(file) = &self.open_file => self.render_file_viewer(file, pal),
            Some(editor) => {
                let font_size = crate::settings::Prefs::editor_font_size(cx);
                let line_height = book_layout(editor.styles.as_ref(), font_size).line_height();
                let font = editor_font(editor.styles.as_ref(), cx);
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    // 에디터보다 먼저 받아 YouTube 주소 붙여넣기를 카드로 바꾼다.
                    .capture_action(cx.listener(Self::on_paste_youtube))
                    // 입력창은 영역을 꽉 채워 스크롤바가 오른쪽 끝에 붙게 하고,
                    // 본문은 좌우 여백으로 가독 폭(EDITOR_MAX_WIDTH) 가운데에 둔다.
                    .child(
                        // 코드 에디터 컴포넌트지만 산문용으로 글꼴·줄 높이를 덮어쓴다.
                        Editor::new(&editor.state)
                            .appearance(false)
                            .bordered(false)
                            .size_full()
                            .font_family(font.clone())
                            .text_size(px(font_size))
                            .line_height(relative(line_height as f32)),
                    )
                    // 영역 폭을 재서 여백을 맞춘다(사이드바 애니메이션 중에도 따라감).
                    .child({
                        let state = editor.state.clone();
                        let applied = editor.side_padding.clone();
                        let tables = editor.tables.clone();
                        canvas(
                            move |bounds, _, cx| {
                                // 서체가 바뀌면 표 격자를 다시 잰다.
                                if tables.set_family(&font) {
                                    state.update(cx, |state, cx| state.refresh_line_blocks(cx));
                                }
                                let side = ((bounds.size.width - px(EDITOR_MAX_WIDTH)) / 2.0)
                                    .max(px(EDITOR_MIN_SIDE_PADDING));
                                if applied.get() != side {
                                    applied.set(side);
                                    state.update(cx, |state, cx| {
                                        state.set_editor_paddings(Edges {
                                            left: side,
                                            right: side,
                                            ..Default::default()
                                        });
                                        cx.notify();
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    // 단어 수는 툴바 아래 오른쪽 위에 둔다(Ulysses).
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_6()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("editor.word_count", count = editor.words)),
                    )
                    .into_any_element()
            }
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_2xl()
                .text_color(pal.muted.opacity(0.7))
                // 파일 그룹을 보고 있으면 파일을 고르라고 안내한다.
                .child(if self.viewing_file_group() {
                    t!("editor.no_file_selected")
                } else {
                    t!("editor.no_sheet_selected")
                })
                .into_any_element(),
        };

        div()
            .flex_1()
            // 좁은 창에서는 오른쪽 패널이 밀려나지 않게 에디터 칸이 줄어든다(툴바는 잘린다).
            .min_w_0()
            .overflow_hidden()
            .h_full()
            .flex()
            .flex_col()
            .bg(pal.background)
            .child(toolbar)
            .child(body)
            .children(self.render_dictation_bar(cx, pal))
    }
}

impl OpenSheet {
    /// 열린 시트 파일 경로.
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 편집 상태(본문·커서).
    pub(super) fn state(&self) -> &Entity<EditorState> {
        &self.state
    }

    /// 본문을 `text`(파일에서 다시 읽은 글)로 바꾸고 단어 수·줄 묶음·표·마크업을 다시 맞춘다. 프로그램으로 바꾼
    /// 글은 변경 소식이 없어, 맞추지 않으면 옛 줄 자리의 마크업(`#` 흐림 등)이 새 글에 남는다.
    pub(super) fn replace_text(&mut self, text: String, window: &mut Window, cx: &mut App) {
        self.words = library::count_words(text.chars());
        self.state
            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        sync_line_groups(self, &text, cx);
        sync_tables(self, &text, cx);
        self.markup.apply_lines(&text, cx);
        self.markup.apply(markdown::spans(&text), cx);
    }

    /// 본문이 지금 파일과 같다고 표시한다(받아쓰기가 파일에 쓴 것을 본문에도 붙였을 때).
    pub(super) fn mark_saved(&mut self, path: &std::path::Path) {
        self.autosave = None;
        self.dirty = false;
        self.disk_modified = library::modified_nanos(path);
    }
}

impl HanjiApp {
    /// 시트를 선택해 에디터에 연다(`None`이면 선택 해제). 열려 있던 시트는 먼저 저장한다.
    ///
    /// 저장에 실패하면 편집 내용을 잃지 않도록 시트를 바꾸지 않는다.
    pub(super) fn select_sheet(
        &mut self,
        path: Option<PathBuf>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let already_open = self.editor.as_ref().map(|editor| &editor.path) == path.as_ref();
        if !already_open {
            if let Err(err) = self.save_open_sheet(cx) {
                self.notify(t!("editor.save_failed", err = err), window, cx);
                return;
            }
            self.editor = None;
            if let Some(path) = &path {
                self.open_sheet(path.clone(), window, cx);
                // 녹음 중인 받아쓰기 기록은 읽기 전용이다.
                self.sync_recording_readonly(cx);
            }
        }
        self.selected_sheet = path;
        self.selected_sheets.clear();
        if self.selected_sheet.is_some() {
            self.selected_groups.clear();
            self.group_anchor = None;
        }
        self.open_file = None;
        self.book_style = None;
        if focus && let Some(editor) = &self.editor {
            editor.state.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    /// 시트 파일을 읽어 새 편집 상태를 만든다.
    fn open_sheet(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // 파일은 표준 마크다운, 에디터는 줄 하나가 문단이다(ADR-0011).
        let text = match library::read_sheet(&path) {
            Ok(text) => crate::text_transform::to_editor(&text),
            Err(err) => {
                self.notify(t!("editor.open_failed", err = err), window, cx);
                return;
            }
        };
        let disk_modified = library::modified_nanos(&path);
        let words = library::count_words(text.chars());
        let project = self.library.as_ref().and_then(|lib| lib.project_of(&path));
        let styles = project.as_deref().map(library::read_styles);
        let line_groups = Rc::new(RefCell::new(crate::text_transform::line_groups(&text)));
        let line_count = text.split('\n').count();
        let style_groups = line_groups.clone();
        let tables = Rc::new(TableBlocks::new(
            &text,
            editor_font(styles.as_ref(), cx),
            px(crate::settings::Prefs::editor_font_size(cx)),
            cx.text_system().clone(),
        ));
        let (style_tables, measure_tables, render_tables) =
            (tables.clone(), tables.clone(), tables.clone());
        let root = self
            .library
            .as_ref()
            .map_or_else(|| path.clone(), |lib| lib.root.clone());
        let sheet = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let extensions = Rc::new(ExtensionBlocks::new(
            &text,
            crate::extension::Host::new(root),
            sheet,
        ));
        let (inline_find, inline_render) = (extensions.clone(), extensions.clone());
        let (style_extensions, measure_extensions, render_extensions) =
            (extensions.clone(), extensions.clone(), extensions.clone());
        // 마지막 줄 아래 빈 공간(scroll_beyond_last_line)은 코드 에디터 모드에만 있어서
        // EditorState를 쓰고 코드용 기능(줄 번호·들여쓰기 안내선·괄호 자동 닫기·접기)은 끈다.
        // 빈 공간이 없으면 문서 끝에서 커서 여백을 맞추려 끝 너머로 스크롤했다가 되돌아오며 출렁인다.
        let image_app = cx.entity().downgrade();
        let inline_app = image_app.clone();
        let link_app = cx.entity().downgrade();
        let image_sheet = path.clone();
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .clipboard_text_transform(
                    "com.drskur.hanji.editor-text.v1",
                    crate::text_transform::from_clipboard,
                )
                .line_number(false)
                .indent_guides(false)
                .auto_close(false)
                .smart_indent(false)
                .folding(false)
                .cursor_surrounding_lines(Some(CURSOR_MARGIN_LINES))
                .scroll_beyond_last_line(Some(CURSOR_MARGIN_LINES))
                .language(super::code_highlight::LANGUAGE);
            state.set_highlighter_factory(super::code_highlight::factory(), cx);
            configure_text_transform(&mut state, styles.as_ref());
            state.set_line_style(
                Some(line_style(
                    styles.as_ref(),
                    style_groups,
                    style_tables,
                    style_extensions,
                    crate::settings::Prefs::editor_font_size(cx),
                    cx,
                )),
                cx,
            );
            // 확장 블록의 영역을 누르면 블록 본문(인자)을 고친다(ADR-0032).
            let block_editor = cx.entity().downgrade();
            // 이미지 줄 아래에 이미지를 그린다(ADR-0008).
            let images = Rc::new(ImageBlocks::new(&path));
            let measure = images.clone();
            // 줄 블록 = 이미지·표 격자·확장 캔버스(측정한 몫) + 줄 스타일이 잡은 나머지(페이지 나눔
            // 표시, ADR-0010). 첫 줄 위 블록은 페이지 나눔 표시뿐이다.
            state.set_line_blocks(
                move |row, line, wrap_width| {
                    measure.measure(line, wrap_width)
                        + measure_tables.measure(row, wrap_width)
                        + measure_extensions.measure(row, wrap_width)
                },
                move |block, window, cx| {
                    if block.above {
                        return page_break_mark(block.size, cx);
                    }
                    let width = Some(block.size.width);
                    let drawn = render_extensions.measure(block.line, width);
                    if drawn > px(0.) {
                        let mark = block.size.height - drawn;
                        return div()
                            .flex()
                            .flex_col()
                            .children(render_extensions.render(
                                block.line,
                                size(block.size.width, drawn),
                                &block_editor,
                                &image_app,
                                window,
                                cx,
                            ))
                            .when(mark >= px(PAGE_BREAK_HEIGHT), |this| {
                                this.child(page_break_mark(size(block.size.width, mark), cx))
                            })
                            .into_any_element();
                    }
                    let grid = render_tables.measure(block.line, width);
                    if grid > px(0.) {
                        let mark = block.size.height - grid;
                        return div()
                            .flex()
                            .flex_col()
                            .children(render_tables.render(
                                block.line,
                                size(block.size.width, grid),
                                cx,
                            ))
                            .when(mark >= px(PAGE_BREAK_HEIGHT), |this| {
                                this.child(page_break_mark(size(block.size.width, mark), cx))
                            })
                            .into_any_element();
                    }
                    let image = images.measure(&block.text, width);
                    let mark = block.size.height - image;
                    let render_image = |size| {
                        let app = image_app.clone();
                        let sheet = image_sheet.clone();
                        let line = block.line;
                        let original = block.text.clone();
                        div()
                            .id(("editor-image", line))
                            .cursor_pointer()
                            .on_click(move |event: &ClickEvent, window, cx| {
                                if event.click_count() == 2 {
                                    cx.stop_propagation();
                                    app.update(cx, |app, cx| {
                                        app.edit_image(&sheet, line, &original, window, cx);
                                    })
                                    .ok();
                                }
                            })
                            .child(images.render(&block.text, size, cx))
                            .into_any_element()
                    };
                    if mark < px(PAGE_BREAK_HEIGHT) {
                        return render_image(block.size);
                    }
                    div()
                        .flex()
                        .flex_col()
                        .when(image > px(0.), |this| {
                            this.child(render_image(size(block.size.width, image)))
                        })
                        .child(page_break_mark(size(block.size.width, mark), cx))
                        .into_any_element()
                },
                cx,
            );
            // 문단 안의 `{{이름 인자}}`는 인라인 확장으로 보인다(ADR-0036).
            let inline_editor = cx.entity().downgrade();
            state.set_inline_objects(
                move |_, line| inline_find.inline_tokens(line),
                move |token, window, cx| {
                    inline_render.render_inline(token, &inline_editor, &inline_app, window, cx)
                },
                None,
                cx,
            );
            // 할 일 항목의 기호 자리에 체크 상자를 그린다(ADR-0023).
            super::editor_task::install(&mut state, cx);
            // ⌘(macOS)·Ctrl(Windows)+클릭으로 링크를 연다.
            super::editor_link::install(&mut state, link_app);
            state.set_value(text.clone(), window, cx);
            state
        });
        let markup = Markup::new(&state, styles.as_ref(), cx);
        markup.apply_lines(&text, cx);
        markup.apply(markdown::spans(&text), cx);
        let subscription = cx.subscribe_in(
            &state,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.on_editor_changed(window, cx),
                // 포커스가 빠지면 기다리지 않고 저장한다.
                InputEvent::Blur => {
                    if let Err(err) = this.save_open_sheet(cx) {
                        this.notify(t!("editor.save_failed", err = err), window, cx);
                    }
                }
                _ => {}
            },
        );
        self.editor = Some(OpenSheet {
            path,
            state,
            dirty: false,
            disk_modified,
            words,
            autosave: None,
            markup,
            markup_refresh: None,
            side_padding: Rc::new(Cell::new(px(-1.0))),
            project,
            styles,
            line_groups,
            line_count,
            tables,
            extensions,
            _subscription: subscription,
        });
    }

    /// 테마가 바뀌면 열린 시트의 마크업 색을 다시 입힌다.
    pub(super) fn refresh_markup(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            let text = editor.state.read(cx).value();
            editor.markup.apply(markdown::spans(&text), cx);
        }
    }

    /// 본문이 바뀌면 단어 수를 갱신하고 자동 저장·마크업 재계산을 (다시) 예약한다.
    pub(super) fn on_editor_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.dirty = true;
        editor.words = library::count_words(editor.state.read(cx).text().chars());
        // 제목 크기는 엔진이 즉시 바꾸므로 굵기·`#` 흐림도 같은 프레임에 맞춘다.
        let text = editor.state.read(cx).value();
        sync_line_groups(editor, &text, cx);
        sync_tables(editor, &text, cx);
        editor.markup.apply_lines(&text, cx);
        editor.markup_refresh = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(MARKUP_DELAY).await;
            let Ok(text) = this.update(cx, |this, cx| {
                this.editor
                    .as_ref()
                    .map(|editor| editor.state.read(cx).value())
            }) else {
                return;
            };
            let Some(text) = text else {
                return;
            };
            // 긴 문서(2만 단어 ≈ 35ms)도 UI 스레드를 막지 않도록 파싱은 백그라운드에서.
            let spans = cx
                .background_executor()
                .spawn(async move { markdown::spans(&text) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(editor) = &this.editor {
                    editor.markup.apply(spans, cx);
                }
            })
            .ok();
        }));
        editor.autosave = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                if let Err(err) = this.save_open_sheet(cx) {
                    this.notify(t!("editor.save_failed", err = err), window, cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// 열린 시트에 저장하지 않은 변경이 있으면 원자적으로 저장한다.
    pub(super) fn save_open_sheet(&mut self, cx: &mut Context<Self>) -> std::io::Result<()> {
        let recording = self.recording_path().map(std::path::Path::to_path_buf);
        let Some(editor) = &mut self.editor else {
            return Ok(());
        };
        if !editor.dirty {
            return Ok(());
        }
        // 녹음 중인 받아쓰기 기록은 작업 스레드만 쓴다. 본문은 읽기 전용이고 붙인 문단은 이미 파일에 있다.
        // (덮어쓰면 원자적 저장이 파일을 바꿔치워 작업 스레드가 쓰는 문단이 사라진다.)
        if recording.as_deref() == Some(editor.path.as_path()) {
            editor.autosave = None;
            editor.dirty = false;
            return Ok(());
        }
        editor.autosave = None;
        let text = editor.state.read(cx).value();
        editor.disk_modified =
            library::write_sheet(&editor.path, &crate::text_transform::to_markdown(&text))?;
        editor.dirty = false;
        Ok(())
    }

    /// 열린 시트를 저장하고 닫는다(라이브러리 닫기·종료). 실패는 기록만 한다.
    pub(super) fn close_sheet(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.save_open_sheet(cx) {
            eprintln!("시트 저장 실패: {err}");
        }
        self.editor = None;
        self.selected_sheet = None;
        self.open_file = None;
        self.book_style = None;
    }

    /// 다시 스캔한 뒤 열린 시트를 맞춘다.
    ///
    /// - 책 스타일이 바뀌었으면 다시 입힌다.
    /// - 파일이 사라졌으면 닫는다.
    /// - 밖에서 바뀌었고 저장하지 않은 변경이 없으면 다시 읽는다.
    ///   (저장하지 않은 변경이 있으면 지금은 그대로 두고 다음 저장이 덮어쓴다.)
    pub(super) fn sync_open_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let recording = self.recording_path().map(std::path::Path::to_path_buf);
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some(project) = &editor.project {
            let styles = Some(library::read_styles(project));
            if styles != editor.styles {
                let line_style = line_style(
                    styles.as_ref(),
                    editor.line_groups.clone(),
                    editor.tables.clone(),
                    editor.extensions.clone(),
                    crate::settings::Prefs::editor_font_size(cx),
                    cx,
                );
                editor.markup.heading_weights = heading_weights(styles.as_ref());
                editor.styles = styles;
                editor.state.update(cx, |state, cx| {
                    state.set_line_style(Some(line_style), cx);
                    configure_text_transform(state, editor.styles.as_ref());
                });
                let text = editor.state.read(cx).value();
                editor.markup.apply_lines(&text, cx);
            }
        }
        // 확장 파일이 바뀌었으면(모델이 고쳤으면) 캔버스를 다시 재고 그린다.
        if editor.extensions.files_changed() {
            editor.extensions.forget_values();
            editor.state.update(cx, |state, cx| {
                state.refresh_line_blocks(cx);
                state.refresh_inline_objects(cx);
            });
        }
        if !editor.path.is_file() {
            self.editor = None;
            self.selected_sheet = None;
            return;
        }
        let modified = library::modified_nanos(&editor.path);
        // 녹음 중인 기록은 문단이 올 때마다 본문 끝에 붙이므로 다시 읽지 않는다(끝나면 맞춘다).
        if editor.dirty
            || modified == editor.disk_modified
            || recording.as_deref() == Some(editor.path.as_path())
        {
            return;
        }
        let Ok(text) =
            library::read_sheet(&editor.path).map(|text| crate::text_transform::to_editor(&text))
        else {
            return;
        };
        editor.disk_modified = modified;
        editor.replace_text(text, window, cx);
    }

    /// 열린 시트 파일이 다른 그룹으로 옮겨졌을 때 경로와 책 스타일을 새 위치에 맞춘다.
    ///
    /// 옮기기 전에 저장했으므로 본문·커서는 그대로 둔다. 프로젝트가 바뀌면 그 프로젝트의
    /// 스타일을 다시 입히고, 프로젝트 밖으로 나가면 기본 배치로 돌린다.
    pub(super) fn relocate_open_sheet(
        &mut self,
        old: &std::path::Path,
        new: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        if self.editor.as_ref().is_none_or(|editor| editor.path != old) {
            return;
        }
        let project = self.library.as_ref().and_then(|lib| lib.project_of(new));
        let styles = project.as_deref().map(library::read_styles);
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.path = new.to_path_buf();
        editor.project = project;
        editor.disk_modified = library::modified_nanos(new);
        if styles == editor.styles {
            return;
        }

        let style = line_style(
            styles.as_ref(),
            editor.line_groups.clone(),
            editor.tables.clone(),
            editor.extensions.clone(),
            crate::settings::Prefs::editor_font_size(cx),
            cx,
        );
        editor.markup.heading_weights = heading_weights(styles.as_ref());
        let transform_styles = styles.clone();
        editor.styles = styles;
        editor.state.update(cx, |state, cx| {
            state.set_line_style(Some(style), cx);
            configure_text_transform(state, transform_styles.as_ref());
        });
        let text = editor.state.read(cx).value();
        editor.markup.apply_lines(&text, cx);
    }
}
