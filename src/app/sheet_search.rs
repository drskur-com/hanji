//! 시트 검색 대화상자: 검색어를 넣으면 라이브러리의 모든 시트 본문을 찾아(`library::search_sheets`,
//! ripgrep 엔진) 든 시트를 최근 수정순으로 보여 준다. 고르면 그 시트를 그 그룹에서 연다.

use gpui_base::StyledExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{
    ActiveTheme as _, FocusableExt as _, Icon, Sizable as _, WindowExt as _,
};
use gpui_kit::gpui::prelude::{FluentBuilder as _, InteractiveElement as _};
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::HanjiApp;
use super::sheet_list::sheet_label;
use super::style::Palette;
use crate::{SearchSheets, icons, library};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 560.0;

/// 결과 목록 높이.
const LIST_HEIGHT: f32 = 400.0;

/// 결과 한 줄 높이: 제목(20) + 위치(16) + 발췌(16) + 위아래 여백(16).
const ROW_HEIGHT: f32 = 68.0;

/// 마지막 입력 후 검색을 시작하기까지 기다리는 시간.
const SEARCH_DELAY: Duration = Duration::from_millis(120);

/// 검색 결과 한 줄.
struct Row {
    hit: library::SearchHit,
    title: String,
}

/// 시트 검색 폼 뷰.
pub(super) struct SheetSearch {
    app: WeakEntity<HanjiApp>,
    input: Entity<InputState>,
    /// 찾을 시트(최근 수정순).
    sheets: Arc<Vec<PathBuf>>,
    /// 시트가 든 곳(“프로젝트 › 그룹”).
    locations: HashMap<PathBuf, SharedString>,
    rows: Vec<Row>,
    /// `rows`를 찾은 검색어.
    searched: String,
    /// 고른 줄(↑·↓, Enter로 연다).
    active: usize,
    scroll: UniformListScrollHandle,
    /// 검색 대기·실행 태스크. 새로 시작하면 이전 것은 버려진다.
    task: Option<Task<()>>,
    _subscription: Subscription,
}

impl SheetSearch {
    fn new(
        app: WeakEntity<HanjiApp>,
        library: &library::Library,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("search.placeholder")));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_search(cx);
            }
        });
        let (sheets, locations) = library_sheets(library);
        Self {
            app,
            input,
            sheets: Arc::new(sheets),
            locations,
            rows: Vec::new(),
            searched: String::new(),
            active: 0,
            scroll: UniformListScrollHandle::new(),
            task: None,
            _subscription: subscription,
        }
    }

    /// 입력이 잠깐 멈추면 백그라운드에서 찾는다. 빈 검색어는 결과를 비운다.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_string();
        if query == self.searched {
            return;
        }
        if query.is_empty() {
            self.task = None;
            self.set_rows(query, Vec::new(), cx);
            return;
        }
        let sheets = self.sheets.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let background_query = query.clone();
            let rows = cx
                .background_executor()
                .spawn(async move {
                    library::search_sheets(&sheets, &background_query)
                        .into_iter()
                        .map(|hit| {
                            let preview = library::read_preview(&hit.path);
                            Row {
                                title: sheet_label(Some(&preview)),
                                hit,
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| this.set_rows(query, rows, cx))
                .ok();
        }));
    }

    fn set_rows(&mut self, query: String, rows: Vec<Row>, cx: &mut Context<Self>) {
        self.searched = query;
        self.rows = rows;
        self.active = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// 고른 줄을 ↑·↓로 옮긴다(끝에서 멈춘다).
    fn move_active(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.active = if down {
            (self.active + 1).min(self.rows.len() - 1)
        } else {
            self.active.saturating_sub(1)
        };
        self.scroll
            .scroll_to_item(self.active, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// 결과 시트를 연다. 열었으면 `true`(대화상자를 닫는다).
    fn open(&self, index: usize, window: &mut Window, cx: &mut App) -> bool {
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        let path = row.hit.path.clone();
        self.app
            .update(cx, |app, cx| app.open_linked_sheet(path, window, cx))
            .is_ok()
    }

    fn render_row(&self, index: usize, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[index];
        let active = index == self.active;
        let highlight = HighlightStyle {
            color: Some(pal.foreground),
            background_color: Some(pal.accent.opacity(0.25)),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let snippet = StyledText::new(row.hit.snippet.clone()).with_highlights(
            row.hit
                .ranges
                .iter()
                .map(|range| (range.clone(), highlight)),
        );
        let location = self
            .locations
            .get(&row.hit.path)
            .cloned()
            .unwrap_or_default();
        let location = if row.hit.lines > 1 {
            SharedString::from(t!(
                "search.lines",
                location = location,
                count = row.hit.lines
            ))
        } else {
            location
        };
        div()
            .id(("sheet-search-row", index))
            .h(px(ROW_HEIGHT))
            .py_1()
            .child(
                div()
                    .h_full()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |this| this.bg(pal.active))
                    .when(!active, |this| this.hover(move |style| style.bg(pal.hover)))
                    .flex()
                    .flex_col()
                    .text_sm()
                    .text_color(pal.foreground)
                    .child(
                        div()
                            .h(px(20.0))
                            .line_height(px(20.0))
                            .truncate()
                            .font_semibold()
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .h(px(16.0))
                            .line_height(px(16.0))
                            .text_xs()
                            .text_color(pal.muted)
                            .truncate()
                            .child(location),
                    )
                    .child(
                        div()
                            .h(px(16.0))
                            .line_height(px(16.0))
                            .text_xs()
                            .text_color(pal.muted)
                            .truncate()
                            .child(snippet),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.open(index, window, cx) {
                    window.close_dialog(cx);
                }
            }))
            .into_any_element()
    }
}

impl Render for SheetSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let body: AnyElement = if self.rows.is_empty() {
            let hint = if self.searched.is_empty() {
                t!("search.hint")
            } else {
                t!("search.none")
            };
            div()
                .h(px(LIST_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(pal.muted)
                .child(hint)
                .into_any_element()
        } else {
            uniform_list(
                "sheet-search-results",
                self.rows.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|index| this.render_row(index, pal, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .h(px(LIST_HEIGHT))
            .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            // ↑·↓는 입력창 커서 대신 결과 줄을 옮긴다.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.move_active(false, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.move_active(true, cx);
                cx.stop_propagation();
            }))
            .child(
                Input::new(&self.input).focus_ring(false).prefix(
                    Icon::new(icons::search_field())
                        .small()
                        .text_color(pal.muted),
                ),
            )
            .child(body)
    }
}

/// 라이브러리의 모든 시트(메모·프로젝트·받아쓰기, 최근 수정순)와 각 시트가 든 곳.
fn library_sheets(library: &library::Library) -> (Vec<PathBuf>, HashMap<PathBuf, SharedString>) {
    fn collect(
        group: &library::Group,
        parent: Option<&str>,
        out: &mut Vec<(library::Sheet, SharedString)>,
    ) {
        let label = match parent {
            Some(parent) => format!("{parent} › {}", group.name),
            None => group.name.clone(),
        };
        out.extend(
            group
                .sheets
                .iter()
                .map(|sheet| (sheet.clone(), SharedString::from(label.clone()))),
        );
        for child in group.groups.iter().chain(&group.extras) {
            collect(child, Some(&label), out);
        }
    }

    let mut found = Vec::new();
    for section in &library.sections {
        for entry in &section.entries {
            match entry {
                library::Entry::Sheet(sheet) => {
                    found.push((sheet.clone(), SharedString::from(section.kind.title())))
                }
                library::Entry::Group(group) => collect(group, None, &mut found),
            }
        }
    }
    if crate::dictation::ENABLED {
        collect(&library.stt, None, &mut found);
    }
    found.sort_by_key(|(sheet, _)| std::cmp::Reverse(sheet.modified));
    let locations = found
        .iter()
        .map(|(sheet, location)| (sheet.path.clone(), location.clone()))
        .collect();
    (
        found.into_iter().map(|(sheet, _)| sheet.path).collect(),
        locations,
    )
}

impl HanjiApp {
    /// 시트 검색 대화상자를 연다. 라이브러리가 없으면 열지 않는다.
    pub(super) fn on_search_sheets(
        &mut self,
        _: &SearchSheets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(library) = &self.library else {
            return;
        };
        let app = cx.entity().downgrade();
        let form = cx.new(|cx| SheetSearch::new(app, library, window, cx));
        // 처음 그릴 때만 입력 칸에 포커스를 줘서 바로 쓸 수 있게 한다.
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let input = form.read(cx).input.clone();
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            let enter = form.clone();
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("search.title"))
                .close_button(true)
                // Enter(대화상자 확인)는 고른 줄을 연다. 결과가 없으면 닫지 않는다.
                .on_ok(move |_, window, cx| {
                    enter.update(cx, |form, cx| form.open(form.active, window, cx))
                })
                .child(form.clone())
        });
    }
}
