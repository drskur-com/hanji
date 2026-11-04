//! 사이드바 2단: 시트 목록(카드, 끌어서 정렬, 순서 애니메이션).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::{
    FluentBuilder, InteractiveElement as _, StatefulInteractiveElement as _,
};
use gpui_kit::gpui::*;

use super::library_ops::SortChange;
use super::library_panel::menu_row;
use super::style::*;
use super::{GroupSort, HanjiApp, Selection};
use crate::{NewSheet, icons, library};

/// 시트 카드 높이(제목 한 줄 + 발췌 두 줄 + 여백). 모든 카드가 같아서
/// 위치를 `index * 높이`로 계산하고, 순서가 바뀌면 스프링으로 옮겨 간다.
///
/// gpui의 `text_sm`/`text_xs`는 줄 높이를 바꾸지 않으므로(기본 1.618배) 아래 줄 높이를 직접 지정한다.
/// 12(위 여백) + 20(제목) + 4(간격) + 16×2(발췌) + 12(아래 여백) + 1(구분선) = 81.
const SHEET_ROW_HEIGHT: f32 = 81.0;

/// 카드 제목 줄 높이(px).
const TITLE_LINE_HEIGHT: f32 = 20.0;

/// 카드 발췌 줄 높이(px).
const EXCERPT_LINE_HEIGHT: f32 = 16.0;

/// 접합 표시 선 굵기와, 이어지지 않는 쪽 끝을 카드 안으로 들이는 정도(px).
const GLUE_BAR_WIDTH: f32 = 3.0;
const GLUE_BAR_INSET: f32 = 14.0;

/// 순서가 바뀐 시트 카드가 새 자리로 옮겨 가는 스프링.
const SHEET_ROW_SPRING: SpringConfig = SpringConfig::new(500.0, 45.0, 1.0);

/// 목록/에디터에 표시할 시트 이름. 빈 시트는 "빈 시트", 아직 읽지 않았으면 빈 문자열.
pub(super) fn sheet_label(preview: Option<&library::Preview>) -> String {
    match preview {
        Some(preview) if preview.is_empty() => t!("library.sheet.empty").to_string(),
        Some(preview) => preview.title.clone(),
        None => String::new(),
    }
}

/// 시트 목록에서 끌고 있는 시트. 끄는 동안 커서를 따라다니는 미리보기로도 그린다.
/// 파일 그룹의 파일을 끌 때도 같은 모양을 쓴다.
#[derive(Clone)]
pub(super) struct DraggedSheet {
    /// 끌기 시작한 그룹. 다른 그룹 줄에 놓으면 그 그룹으로 옮긴다(ADR-0019).
    pub(super) group: std::path::PathBuf,
    /// 파일 그룹의 파일을 끄는 중인지. 시트 그룹으로는 놓을 수 없다.
    pub(super) files: bool,
    /// 끈 카드의 목록 위치. 놓일 자리(위/아래)를 정한다.
    pub(super) anchor: usize,
    /// 함께 옮기는 항목의 목록 위치(목록 순서). 단일 선택이면 `anchor` 하나다.
    pub(super) indices: Vec<usize>,
    /// 함께 옮기는 시트 파일 경로(다른 그룹으로 옮길 때 쓴다).
    pub(super) paths: Vec<std::path::PathBuf>,
    /// 미리보기에 보일 이름들. 여러 개면 첫 이름과 개수를 보여 준다.
    pub(super) labels: Vec<SharedString>,
    pub(super) pal: Palette,
}

impl Render for DraggedSheet {
    /// 끄는 시트의 이름을 보여 준다. 여러 개를 고르면 “첫 시트 외 N개”로 줄인다.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let pal = self.pal;
        let label = self.labels.first().cloned().unwrap_or_default();
        let extra = self.labels.len().saturating_sub(1);
        div()
            .px_3()
            .py_2()
            .max_w(px(SHEET_COLUMN_WIDTH - 32.0))
            .rounded_md()
            .border_1()
            .border_color(pal.border)
            .bg(pal.background)
            .shadow_md()
            .text_sm()
            .text_color(pal.foreground)
            .font_semibold()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().truncate().child(label))
                    .when(extra > 0, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .font_normal()
                                .text_color(pal.muted)
                                .child(t!("library.more_count", count = extra)),
                        )
                    }),
            )
    }
}

impl HanjiApp {
    /// 시트 목록 제목 오른쪽의 정렬 버튼. 누르면 정렬 방법과(수동이 아니면) 방향을 고르고,
    /// 고른 것은 그룹 메타에 저장한다.
    fn sort_button(
        &self,
        group_path: std::path::PathBuf,
        current: GroupSort,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let pick = move |path: &std::path::PathBuf, change: SortChange| {
            let (app, path) = (app.clone(), path.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                app.update(cx, |app, cx| app.set_sort(&path, change, window, cx))
                    .ok();
            }
        };
        Button::new("sheet-sort")
            .icon(Icon::new(icons::sort()))
            .ghost()
            .small()
            .tooltip(current.mode.title())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let menu =
                    library::SortMode::ALL
                        .into_iter()
                        .fold(menu.min_w(px(160.0)), |menu, mode| {
                            menu.item(
                                PopupMenuItem::new(mode.title())
                                    .checked(mode == current.mode)
                                    .on_click(pick(&group_path, SortChange::Mode(mode))),
                            )
                        });
                let Some((ascending, descending)) = current.mode.direction_titles() else {
                    return menu;
                };
                menu.separator()
                    .item(
                        PopupMenuItem::new(ascending)
                            .checked(!current.descending)
                            .on_click(pick(&group_path, SortChange::Descending(false))),
                    )
                    .item(
                        PopupMenuItem::new(descending)
                            .checked(current.descending)
                            .on_click(pick(&group_path, SortChange::Descending(true))),
                    )
            })
    }

    /// 사이드바 2단: 선택한 그룹(또는 "모두")의 시트 목록. 항상 보인다.
    pub(super) fn render_sheet_column(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let current = self.effective_selection();
        let title = current
            .as_ref()
            .map(|selection| self.selection_label(selection))
            .unwrap_or_else(|| t!("library.sheet.list_title").to_string());
        let sheets = current
            .as_ref()
            .map(|selection| self.sheets_for(selection))
            .unwrap_or_default();
        // 파일 그룹(첨부파일 등)이면 시트 대신 파일 목록을 보여 준다.
        let files = match &current {
            Some(Selection::Group(path)) => self
                .library
                .as_ref()
                .and_then(|lib| lib.find_group(path))
                .filter(|group| group.kind == library::GroupKind::Files)
                .map(|group| group.files.clone()),
            _ => None,
        };
        let sheet_group = files.is_none();
        // 그룹의 정렬 방법. 고를 수 없는 그룹(프로젝트 원고)은 버튼을 두지 않는다.
        let sort = match &current {
            Some(Selection::Group(path)) => Some((path.clone(), self.sort_for(path))),
            _ => None,
        };

        let toolbar = window_drag_area(
            div()
                .h(px(TOOLBAR_HEIGHT))
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .gap_2(),
        )
        // 패널이 접히면 (macOS) 신호등 자리를 비우고 다시 열기 버튼을 둔다.
        .when(!self.sidebar_visible, |this| {
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
        .child(div().flex_1())
        // 파일 그룹은 시트를 만들 수 없으니 작성 버튼 대신 파일 종류 필터를 둔다.
        .map(|this| {
            if sheet_group {
                this.child(tool_group(pal).child(icon_button(
                    "sheet-filter",
                    icons::filter(),
                    false,
                    pal,
                )))
                .child(tool_group(pal).child(
                    icon_button("new-sheet", icons::compose(), true, pal).on_click(
                        cx.listener(|this, _, window, cx| this.on_new_sheet(&NewSheet, window, cx)),
                    ),
                ))
            } else {
                this.child(tool_group(pal).child(self.file_filter_button(cx, pal)))
            }
        });

        let list: AnyElement = if current == Some(Selection::Todos) {
            self.render_todo_list(cx, pal)
        } else if let Some(mut files) = files {
            let (path, sort) = sort.clone().expect("파일 그룹은 그룹 선택에서만 나온다");
            self.sort_files(&mut files, sort);
            if let Some(kind) = self.file_filter {
                files.retain(|file| file.kind.filter_kind() == kind);
            }
            // 수동 정렬이고 거르지 않았을 때만 끌어서 순서를 바꾼다(위치가 그룹 순서와 같아야 한다).
            let manual = (sort.mode == library::SortMode::Manual && self.file_filter.is_none())
                .then_some(path);
            self.render_file_list(files, manual, cx, pal)
        } else if sheets.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(pal.muted)
                .child(t!("library.sheet.none"))
                .into_any_element()
        } else {
            // 순서 바꾸기는 수동 정렬일 때만 하고, 다른 그룹으로 옮기는 끌기는 정렬 방법과
            // 관계없이 할 수 있다(ADR-0019). "모두"(섹션)는 여러 그룹이 섞여 있어 끌지 않는다.
            let group_path = match &sort {
                Some((path, sort)) if sort.mode == library::SortMode::Manual => Some(path.clone()),
                _ => None,
            };
            let drag_group = sort.as_ref().map(|(path, _)| path.clone());
            // 다른 목록으로 바꿨을 때 같은 시트가 옛 자리에서 날아오지 않도록 목록별로 키를 나눈다.
            let list_key = match &current {
                Some(Selection::Group(path)) => path.display().to_string(),
                Some(Selection::Section(kind)) => kind.dir().to_string(),
                Some(Selection::Todos) | None => String::new(),
            };
            // 접합은 수동 순서에서만 뜻이 있다(앞 시트에 이어짐).
            let glued: Vec<bool> = sheets
                .iter()
                .enumerate()
                .map(|(index, sheet)| group_path.is_some() && index > 0 && sheet.glued)
                .collect();
            // 개별 시트의 놓일 자리. 접합 내부의 시트 사이에도 표시한다(ADR-0014).
            let drop_edge =
                self.sheet_drop
                    .filter(|_| cx.has_active_drag())
                    .and_then(|(from, over)| {
                        if from == over {
                            None
                        } else {
                            Some((over, over < from))
                        }
                    });
            let labels: Vec<SharedString> = sheets
                .iter()
                .map(|sheet| {
                    sheet_label(self.previews.get(&sheet.path).map(|(_, preview)| preview)).into()
                })
                .collect();
            let paths: std::rc::Rc<Vec<std::path::PathBuf>> =
                std::rc::Rc::new(sheets.iter().map(|sheet| sheet.path.clone()).collect());
            let mut items: Vec<AnyElement> = Vec::with_capacity(sheets.len());
            for (index, sheet) in sheets.into_iter().enumerate() {
                let path = sheet.path.clone();
                let selected = self.selected_sheet.as_ref() == Some(&path)
                    || self.selected_sheets.contains(&path);
                let (glued_prev, glued_next) =
                    (glued[index], glued.get(index + 1).copied().unwrap_or(false));
                let preview = self.previews.get(&path).map(|(_, preview)| preview);
                let is_empty = preview.is_some_and(|preview| preview.is_empty());
                let label = sheet_label(preview);
                // 제목 줄은 첫 줄이 H1~H3일 때만 둔다: H1 굵게, H2 보통, H3 회색.
                // 그 밖(헤딩 아님·H4 이하)은 첫 줄부터 발췌처럼 작은 회색 글로 잇는다.
                let level = preview.map_or(1, |preview| preview.level);
                let has_title = is_empty || (1..=3).contains(&level);
                let excerpt = match preview {
                    Some(preview) if !has_title => format!("{} {}", preview.title, preview.excerpt)
                        .trim()
                        .to_string(),
                    Some(preview) => preview.excerpt.clone(),
                    None => String::new(),
                };
                // 구분선은 카드 양옆을 비워 두고(본문 글자와 같은 들여쓰기) 긋는다.
                // 선택한 카드는 둥근 배경으로 감싸고 그 카드의 구분선은 숨긴다.
                let content = div()
                    .h_full()
                    .overflow_hidden()
                    .py_3()
                    .border_b_1()
                    // 접합한 시트끼리는 구분선 없이 붙인다.
                    .border_color(if selected || glued_next {
                        gpui_kit::gpui::transparent_black()
                    } else {
                        pal.panel_border
                    })
                    .text_sm()
                    .text_color(pal.foreground)
                    .flex()
                    .flex_col()
                    .gap_1()
                    // 빈 시트는 흐린 기본 굵기로, 카드 가운데에 가볍게 보인다(Ulysses).
                    .when(is_empty, |this| this.justify_center().items_center())
                    // 아직 안 읽은 시트도 한 줄 높이를 차지하게 한다.
                    .when(has_title, |this| {
                        this.child(
                            div()
                                .h(px(TITLE_LINE_HEIGHT))
                                .line_height(px(TITLE_LINE_HEIGHT))
                                .truncate()
                                .when(is_empty || level == 3, |this| this.text_color(pal.muted))
                                .when(!is_empty && level == 1, |this| this.font_semibold())
                                .child(label.clone()),
                        )
                    })
                    .when(!excerpt.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .line_height(px(EXCERPT_LINE_HEIGHT))
                                .text_color(pal.muted)
                                // 제목 줄이 없으면 그 자리까지 한 줄 더 쓴다.
                                .line_clamp(if has_title { 2 } else { 3 })
                                .text_ellipsis()
                                .child(excerpt),
                        )
                    });
                let card = div()
                    .h_full()
                    .px_2()
                    .child(
                        div()
                            .h_full()
                            .px_2()
                            .rounded_md()
                            .when(selected, |this| this.bg(pal.active))
                            .child(content),
                    )
                    // 접합한 시트는 왼쪽 선으로 잇는다. 이어지는 쪽은 카드 끝까지 긋는다.
                    .when(glued_prev || glued_next, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left(px(3.0))
                                .w(px(GLUE_BAR_WIDTH))
                                .top(px(if glued_prev { 0.0 } else { GLUE_BAR_INSET }))
                                .bottom(px(if glued_next { 0.0 } else { GLUE_BAR_INSET }))
                                // 시트 경계는 직선으로 맞닿고 묶음의 양 끝만 둥글게 한다.
                                .when(!glued_prev, |this| {
                                    this.rounded_tl(px(GLUE_BAR_WIDTH / 2.0))
                                        .rounded_tr(px(GLUE_BAR_WIDTH / 2.0))
                                })
                                .when(!glued_next, |this| {
                                    this.rounded_bl(px(GLUE_BAR_WIDTH / 2.0))
                                        .rounded_br(px(GLUE_BAR_WIDTH / 2.0))
                                })
                                .bg(pal.accent),
                        )
                    })
                    .context_menu({
                        let (app, path, paths) =
                            (cx.entity().downgrade(), path.clone(), paths.clone());
                        let group_path = group_path.clone();
                        move |menu, _, cx| {
                            sheet_menu(menu, &app, &path, &paths, group_path.as_deref(), cx)
                        }
                    });

                // 끌어다 놓을 자리는 카드를 감싼 줄의 위/아래 선으로 보여준다.
                let row = div()
                    .id(SharedString::from(format!("sheet-{}", path.display())))
                    .cursor_pointer()
                    .on_click(cx.listener({
                        let (path, paths) = (path.clone(), paths.clone());
                        move |this, event: &ClickEvent, window, cx| {
                            // Shift+클릭: 연 시트부터 누른 시트까지 넓혀 고른다(연 시트는 그대로).
                            if event.modifiers().shift
                                && let Some(range) = this
                                    .selected_sheet
                                    .as_ref()
                                    .and_then(|anchor| paths.iter().position(|p| p == anchor))
                                    .map(|anchor| anchor.min(index)..anchor.max(index) + 1)
                            {
                                this.selected_sheets = paths[range].to_vec();
                                cx.notify();
                                return;
                            }
                            this.select_sheet(Some(path.clone()), false, window, cx);
                        }
                    }))
                    .child(card);
                let row = match &drag_group {
                    Some(sheet_group) => {
                        let sheet_group = sheet_group.clone();
                        let same_list = group_path.as_ref() == Some(&sheet_group);
                        let hover_group = sheet_group.clone();
                        let drop_group = sheet_group.clone();
                        // 선택한 시트가 여럿이면 함께 옮긴다. 아니면 끈 시트 하나만 옮긴다.
                        let indices: Vec<usize> = if self.selected_sheets.is_empty() {
                            vec![index]
                        } else {
                            paths
                                .iter()
                                .enumerate()
                                .filter(|(_, path)| {
                                    **path == sheet.path || self.selected_sheets.contains(path)
                                })
                                .map(|(index, _)| index)
                                .collect()
                        };
                        // 접합 여부와 무관하게 끄는 시트(들)을 옮긴다.
                        let dragged = DraggedSheet {
                            group: sheet_group.clone(),
                            files: false,
                            anchor: index,
                            labels: indices.iter().map(|index| labels[*index].clone()).collect(),
                            paths: indices.iter().map(|index| paths[*index].clone()).collect(),
                            indices,
                            pal,
                        };
                        row.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                            .on_drag_move(cx.listener(
                                move |this, event: &DragMoveEvent<DraggedSheet>, _, cx| {
                                    if !event.bounds.contains(&event.event.position) {
                                        return;
                                    }
                                    let dragged = event.drag(cx);
                                    // 같은 목록의 다른 카드 위에서만 놓일 자리를 그린다.
                                    let drop = (same_list
                                        && !dragged.files
                                        && dragged.group == hover_group
                                        && !dragged.indices.contains(&index))
                                    .then_some((dragged.anchor, index));
                                    if this.sheet_drop != drop {
                                        this.sheet_drop = drop;
                                        cx.notify();
                                    }
                                },
                            ))
                            .map(|row| match drop_edge {
                                Some((edge, true)) if edge == index => {
                                    row.border_t_2().border_color(pal.accent)
                                }
                                Some((edge, false)) if edge == index => {
                                    row.border_b_2().border_color(pal.accent)
                                }
                                _ => row,
                            })
                            .on_drop(cx.listener(
                                move |this, dragged: &DraggedSheet, window, cx| {
                                    this.sheet_drop = None;
                                    if !same_list || dragged.files || dragged.group != drop_group {
                                        return;
                                    }
                                    this.move_selected_sheets(
                                        &drop_group,
                                        &dragged.indices,
                                        dragged.anchor,
                                        index,
                                        window,
                                        cx,
                                    );
                                },
                            ))
                    }
                    None => row,
                };
                // 카드는 절대 위치로 둔다. 스프링 상태는 시트 경로로 이어지므로
                // 순서가 바뀌면 이전 자리에서 새 자리로 미끄러져 간다.
                let top = index as f32 * SHEET_ROW_HEIGHT;
                let row = row.absolute().left_0().right_0().h(px(SHEET_ROW_HEIGHT));
                items.push(
                    row.with_spring(
                        SharedString::from(format!("sheet-pos-{list_key}-{}", path.display())),
                        SpringAnimation::new(SHEET_ROW_SPRING).to(top),
                        |this, y| this.top(px(y)),
                    )
                    .into_any_element(),
                );
            }
            let content_height = items.len() as f32 * SHEET_ROW_HEIGHT;
            div()
                .id("sheet-list")
                .flex_1()
                .overflow_y_scroll()
                // 목록 밖으로 끌고 나가면 놓일 자리 표시를 지운다.
                .on_drag_move(
                    cx.listener(|this, event: &DragMoveEvent<DraggedSheet>, _, cx| {
                        if !event.bounds.contains(&event.event.position)
                            && this.sheet_drop.take().is_some()
                        {
                            cx.notify();
                        }
                    }),
                )
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(content_height))
                        .children(items),
                )
                .into_any_element()
        };

        div()
            .w(px(SHEET_COLUMN_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(pal.background)
            .border_r_1()
            .border_color(pal.border)
            .child(toolbar)
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .border_b_1()
                    .border_color(pal.panel_border)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_lg()
                            .font_semibold()
                            .text_color(pal.foreground)
                            .child(title),
                    )
                    .when_some(
                        sort.filter(|(_, sort)| sort.choosable),
                        |this, (path, sort)| this.child(self.sort_button(path, sort, cx)),
                    ),
            )
            .child(list)
    }
}

/// 시트 카드 우클릭 메뉴: 시트 접합, 접합 해제, 휴지통으로 이동.
///
/// - 우클릭한 시트가 선택 안에 있으면 선택한 시트 모두에, 아니면 그 시트 하나에 한다.
/// - 접합·접합 해제는 수동 순서 그룹에서, 할 수 있을 때만 보인다. 접합은 둘 이상 골랐고 아직 다
///   이어지지 않았을 때, 해제는 범위 안이나 바로 뒤가 이어져 있을 때(시트 하나여도) 보인다.
/// - 휴지통으로 이동은 늘 보이고, 확인을 받는다.
fn sheet_menu(
    menu: PopupMenu,
    app: &WeakEntity<HanjiApp>,
    path: &std::path::Path,
    paths: &[std::path::PathBuf],
    group_path: Option<&std::path::Path>,
    cx: &App,
) -> PopupMenu {
    let Some(clicked) = paths.iter().position(|p| p == path) else {
        return menu;
    };
    let Some(entity) = app.upgrade() else {
        return menu;
    };
    let state = entity.read(cx);
    let selected: Vec<usize> = state
        .selected_sheets
        .iter()
        .filter_map(|selected| paths.iter().position(|p| p == selected))
        .collect();
    let in_selection = selected.contains(&clicked);
    let targets: Vec<std::path::PathBuf> = if in_selection {
        selected.iter().map(|index| paths[*index].clone()).collect()
    } else {
        vec![paths[clicked].clone()]
    };
    let label = sheet_label(state.previews.get(path).map(|(_, preview)| preview));
    let range = match (selected.iter().min(), selected.iter().max()) {
        (Some(&start), Some(&end)) if in_selection => start..end + 1,
        _ => clicked..clicked + 1,
    };
    let sheets = group_path
        .and_then(|group| state.library.as_ref()?.find_group(group))
        .map(|group| group.sheets.clone())
        .unwrap_or_default();
    let glued = |index: usize| index > 0 && sheets.get(index).is_some_and(|sheet| sheet.glued);
    let can_glue = range.len() > 1 && (range.start + 1..range.end).any(|index| !glued(index));
    let can_unglue = (range.start..range.end + 1).any(glued);
    let glue_item = |icon, label: SharedString, glue: bool| {
        let (app, group, range) = (
            app.clone(),
            group_path.map(std::path::Path::to_path_buf),
            range.clone(),
        );
        PopupMenuItem::element(move |_, _| menu_row(icon, label.clone(), None)).on_click(
            move |_, window, cx| {
                let Some(group) = &group else {
                    return;
                };
                app.update(cx, |app, cx| {
                    app.set_glued(group, range.clone(), glue, window, cx)
                })
                .ok();
            },
        )
    };
    let danger = cx.theme().danger;
    let delete_app = app.clone();
    menu.min_w(px(200.0))
        .when(group_path.is_some() && can_glue, |menu| {
            menu.item(glue_item(
                icons::glue(),
                t!("library.sheet.glue").into(),
                true,
            ))
        })
        .when(group_path.is_some() && can_unglue, |menu| {
            menu.item(glue_item(
                icons::unglue(),
                t!("library.sheet.unglue").into(),
                false,
            ))
        })
        .when(group_path.is_some() && (can_glue || can_unglue), |menu| {
            menu.separator()
        })
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_row(icons::trash(), t!("common.move_to_trash"), Some(danger))
            })
            .on_click(move |_, window, cx| {
                delete_app
                    .update(cx, |app, cx| {
                        app.confirm_delete_sheets(targets.clone(), label.clone(), window, cx)
                    })
                    .ok();
            }),
        )
}
