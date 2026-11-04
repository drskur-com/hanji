//! 사이드바 2단의 할 일 보기(ADR-0024): 메모(`Notes/`)의 할 일을 모아 날짜별로 묶어 보인다.
//!
//! 원본은 메모 원문이다. 목록은 색인([`library::TodoIndex`])을 그리고, 체크하면 원문의 그 줄을 고친다.
//! 그 메모가 에디터에 열려 있으면 에디터를 거쳐 고친다(화면에 바로 보이고 되돌릴 수 있다).

use std::path::{Path, PathBuf};

use gpui_kit::component::*;
use gpui_kit::gpui::prelude::{FluentBuilder as _, StatefulInteractiveElement as _};
use gpui_kit::gpui::*;

use super::style::*;
use super::{HanjiApp, editor_task, sheet_list};
use crate::markdown::Todo;
use crate::{cache, icons, library, markdown};

/// 목록의 체크 상자 한 변(px).
const BOX_SIDE: f32 = 14.0;

/// 할 일 색인과 보기 상태. 라이브러리를 열 때마다 새로 만든다.
#[derive(Default)]
pub(super) struct Todos {
    /// 색인. 처음 읽는 동안은 `None`이다.
    index: Option<library::TodoIndex>,
    /// 처음 읽는 백그라운드 태스크. 버리면 멈춘다.
    loading: Option<Task<()>>,
    /// 처음 읽는 동안 폴더 감시가 알려 준 경로. 다 읽은 뒤 다시 읽는다.
    pending: Vec<PathBuf>,
    /// 완료 묶음을 펼쳤는지.
    done_open: bool,
}

/// 할 일 묶음. 이 순서로 보인다.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Bucket {
    /// 마감일이 지났다.
    Overdue,
    /// 오늘 마감이거나, 예정일이 오늘이거나 지났다.
    Today,
    /// 예정일이나 마감일이 앞으로 온다.
    Upcoming,
    /// 날짜가 없다.
    Anytime,
    Done,
}

impl Bucket {
    fn of(todo: &Todo, today: &str) -> Self {
        if todo.checked {
            return Self::Done;
        }
        let due = todo.due.as_deref();
        let scheduled = todo.scheduled.as_deref();
        if due.is_some_and(|due| due < today) {
            Self::Overdue
        } else if due == Some(today) || scheduled.is_some_and(|date| date <= today) {
            Self::Today
        } else if due.is_some() || scheduled.is_some() {
            Self::Upcoming
        } else {
            Self::Anytime
        }
    }

    fn title(self) -> SharedString {
        match self {
            Self::Overdue => t!("library.todos.overdue"),
            Self::Today => t!("library.todos.today"),
            Self::Upcoming => t!("library.todos.upcoming"),
            Self::Anytime => t!("library.todos.anytime"),
            Self::Done => t!("library.todos.done"),
        }
        .into()
    }
}

/// 할 일을 묶음별로 나누고 묶음 안에서 늘어놓는다. 빈 묶음은 뺀다.
///
/// - 기한 지남: 마감일순. 예정: 가장 이른 날짜순. 오늘·날짜 없음: 메모 경로와 메모 안 순서.
/// - 완료: 완료일 최근순(완료일 없는 것은 뒤).
fn arrange(todos: Vec<(PathBuf, Todo)>, today: &str) -> Vec<(Bucket, Vec<(PathBuf, Todo)>)> {
    let mut buckets: Vec<(Bucket, Vec<(PathBuf, Todo)>)> = Vec::new();
    for (path, todo) in todos {
        let bucket = Bucket::of(&todo, today);
        match buckets.iter_mut().find(|(other, _)| *other == bucket) {
            Some((_, items)) => items.push((path, todo)),
            None => buckets.push((bucket, vec![(path, todo)])),
        }
    }
    buckets.sort_by_key(|(bucket, _)| *bucket);
    for (bucket, items) in &mut buckets {
        // 정렬은 안정적이라 같은 날짜끼리는 메모 경로·메모 안 순서가 남는다.
        match bucket {
            Bucket::Overdue => items.sort_by(|(_, a), (_, b)| a.due.cmp(&b.due)),
            Bucket::Upcoming => {
                let first = |todo: &Todo| {
                    [todo.scheduled.as_deref(), todo.due.as_deref()]
                        .into_iter()
                        .flatten()
                        .min()
                        .map(str::to_owned)
                };
                items.sort_by_cached_key(|(_, todo)| first(todo));
            }
            Bucket::Done => items.sort_by(|(_, a), (_, b)| match (&a.done, &b.done) {
                (Some(a), Some(b)) => b.cmp(a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }),
            Bucket::Today | Bucket::Anytime => {}
        }
    }
    buckets
}

/// 목록에 보일 날짜. 올해면 `MM-DD`, 아니면 그대로.
fn short_date<'a>(date: &'a str, today: &str) -> &'a str {
    match (date.get(..5), today.get(..5)) {
        (Some(year), Some(this_year)) if year == this_year => &date[5..],
        _ => date,
    }
}

impl HanjiApp {
    /// 열린 라이브러리의 할 일 색인을 (다시) 만든다. 캐시를 읽고 바뀐 메모만 읽는 일은 백그라운드에서 한다.
    pub(super) fn load_todos(&mut self, cx: &mut Context<Self>) {
        self.todos = Todos::default();
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        self.todos.loading = Some(cx.spawn(async move |this, cx| {
            let index = cx
                .background_executor()
                .spawn(async move { library::TodoIndex::load(&root, cache::Cache::open(&root)) })
                .await;
            this.update(cx, |this, cx| {
                let mut index = index;
                let pending = std::mem::take(&mut this.todos.pending);
                if !pending.is_empty() {
                    index.refresh(&pending);
                }
                this.todos.index = Some(index);
                this.todos.loading = None;
                this.load_previews(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// 바뀐 경로(폴더 감시)의 할 일을 다시 읽는다. 메모 밖 경로는 색인이 거른다.
    pub(super) fn refresh_todos(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        match &mut self.todos.index {
            Some(index) => {
                if index.refresh(paths) {
                    cx.notify();
                }
            }
            None => self.todos.pending.extend_from_slice(paths),
        }
    }

    /// 할 일이 있는 메모(목록에 제목을 보이려고 미리보기를 읽는다).
    pub(super) fn todo_sheets(&self) -> Vec<library::Sheet> {
        let Some(index) = &self.todos.index else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = index.todos().map(|(path, _)| path).collect();
        paths.dedup();
        paths
            .iter()
            .filter_map(|path| self.find_sheet(path))
            .collect()
    }

    /// 할 일이 있는 메모를 에디터에 열고 커서를 그 할 일 글 앞에 둔다.
    ///
    /// 파일과 에디터는 줄 번호가 다르다(에디터는 줄 하나가 문단, ADR-0011). 파일에서 원문이 같은 줄
    /// 가운데 몇 번째인지 보고 에디터에서 같은 순번의 줄을 찾는다.
    fn open_todo(&mut self, path: &Path, todo: &Todo, window: &mut Window, cx: &mut Context<Self>) {
        self.select_sheet(Some(path.to_path_buf()), true, window, cx);
        // 이미 열려 있던 메모는 저장해 두어야 파일과 에디터의 줄 순번이 맞는다.
        if let Err(err) = self.save_open_sheet(cx) {
            self.notify(t!("editor.save_failed", err = err), window, cx);
            return;
        }
        let Some(nth) = library::read_sheet(path)
            .ok()
            .and_then(|text| markdown::locate_todo(&text, todo.line, &todo.source))
            .map(|(_, nth)| nth)
        else {
            return;
        };
        let Some(editor) = self.editor.as_ref().filter(|editor| editor.path() == path) else {
            return;
        };
        editor.state().update(cx, |state, cx| {
            let text = state.value();
            let Some(row) = markdown::nth_line(&text, &todo.source, nth) else {
                return;
            };
            let Some(start) = line_start(&text, row) else {
                return;
            };
            let content = markdown::task_marker(&todo.source).map_or(0, |marker| marker.cover.end);
            let at = start + content.min(todo.source.len());
            state.set_selected_range(at..at, cx);
            state.focus(window, cx);
        });
    }

    /// 할 일의 체크를 뒤집어 원문에 쓴다. 체크하면 완료일(`✅ 오늘`)을 붙이고, 풀면 뗀다.
    ///
    /// 그 사이 원문이 바뀌어 같은 줄을 찾지 못하면 쓰지 않고, 그 메모를 다시 읽어 알린다.
    fn toggle_todo(
        &mut self,
        path: &Path,
        todo: &Todo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.path() == path)
        {
            self.toggle_todo_in_editor(path, todo, window, cx)
        } else {
            toggle_todo_in_file(path, todo)
        };
        match result {
            Ok(true) => {}
            Ok(false) => self.notify(t!("library.todos.changed"), window, cx),
            Err(err) => self.notify(t!("library.todos.toggle_failed", err = err), window, cx),
        }
        self.refresh_todos(&[path.to_path_buf()], cx);
        cx.notify();
    }

    /// 열린 메모의 할 일을 에디터를 거쳐 고치고 바로 저장한다. 같은 줄을 찾지 못하면 `Ok(false)`.
    fn toggle_todo_in_editor(
        &mut self,
        path: &Path,
        todo: &Todo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> std::io::Result<bool> {
        // 저장해 두면 파일과 에디터의 내용이 같아 순번으로 같은 줄을 찾을 수 있다.
        self.save_open_sheet(cx)?;
        let text = library::read_sheet(path)?;
        let Some((_, nth)) = markdown::locate_todo(&text, todo.line, &todo.source) else {
            return Ok(false);
        };
        let Some(editor) = &self.editor else {
            return Ok(false);
        };
        let today = library::today();
        let changed = editor.state().update(cx, |state, cx| {
            let text = state.value();
            let Some(row) = markdown::nth_line(&text, &todo.source, nth) else {
                return false;
            };
            let (Some(start), Some(marker), Some(toggled)) = (
                line_start(&text, row),
                markdown::task_marker(&todo.source),
                markdown::toggle_todo(&todo.source, &today),
            ) else {
                return false;
            };
            // 체크 상자 글자부터 줄 끝까지 한 번에 바꾼다(되돌리기 한 번).
            state.replace_range_keeping_selections(
                start + marker.check..start + todo.source.len(),
                &toggled[marker.check..],
                window,
                cx,
            );
            true
        });
        if !changed {
            return Ok(false);
        }
        self.on_editor_changed(window, cx);
        self.save_open_sheet(cx)?;
        Ok(true)
    }

    /// 사이드바 2단의 할 일 목록.
    pub(super) fn render_todo_list(&self, cx: &mut Context<Self>, pal: Palette) -> AnyElement {
        let Some(index) = &self.todos.index else {
            return empty_state(t!("library.todos.loading").into(), None, pal);
        };
        let today = library::today();
        let todos: Vec<(PathBuf, Todo)> = index
            .todos()
            .map(|(path, todo)| (path, todo.clone()))
            .collect();
        if todos.is_empty() {
            return empty_state(
                t!("library.todos.none").into(),
                Some(t!("library.todos.hint").into()),
                pal,
            );
        }
        let mut items: Vec<AnyElement> = Vec::new();
        for (bucket, todos) in arrange(todos, &today) {
            let done = bucket == Bucket::Done;
            let header = div()
                .id(SharedString::from(format!("todo-bucket-{bucket:?}")))
                .px_4()
                .pt_4()
                .pb_1()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .font_semibold()
                .text_color(if bucket == Bucket::Overdue {
                    pal.danger
                } else {
                    pal.muted
                })
                .child(bucket.title())
                .child(
                    div()
                        .font_normal()
                        .text_color(pal.muted)
                        .child(todos.len().to_string()),
                );
            // 완료 묶음은 접어 두고 눌러서 펼친다.
            let header = if done {
                header
                    .cursor_pointer()
                    .child(
                        Icon::new(icons::disclosure(self.todos.done_open))
                            .xsmall()
                            .text_color(pal.muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.todos.done_open = !this.todos.done_open;
                        cx.notify();
                    }))
            } else {
                header
            };
            items.push(header.into_any_element());
            if done && !self.todos.done_open {
                continue;
            }
            for (path, todo) in todos {
                items.push(self.todo_row(path, todo, bucket, &today, cx, pal));
            }
        }
        div()
            .id("todo-list")
            .flex_1()
            .overflow_y_scroll()
            .pb_4()
            .children(items)
            .into_any_element()
    }

    /// 할 일 한 줄: 체크 상자, 제목, 그리고 메모·날짜·태그·체크리스트 진행.
    fn todo_row(
        &self,
        path: PathBuf,
        todo: Todo,
        bucket: Bucket,
        today: &str,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> AnyElement {
        let sheet = sheet_list::sheet_label(self.previews.get(&path).map(|(_, preview)| preview));
        let group = path
            .parent()
            .and_then(|dir| self.library.as_ref()?.find_group(dir))
            .map(|group| group.name.clone());
        let mut meta: Vec<AnyElement> = Vec::new();
        let source = [group, Some(sheet).filter(|sheet| !sheet.is_empty())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        if !source.is_empty() {
            meta.push(div().min_w_0().truncate().child(source).into_any_element());
        }
        if let Some(date) = &todo.scheduled {
            meta.push(
                div()
                    .flex_shrink_0()
                    .child(format!("⏳ {}", short_date(date, today)))
                    .into_any_element(),
            );
        }
        if let Some(date) = &todo.due {
            meta.push(
                div()
                    .flex_shrink_0()
                    .when(bucket == Bucket::Overdue, |this| {
                        this.text_color(pal.danger)
                    })
                    .child(format!("📅 {}", short_date(date, today)))
                    .into_any_element(),
            );
        }
        if bucket == Bucket::Done
            && let Some(date) = &todo.done
        {
            meta.push(
                div()
                    .flex_shrink_0()
                    .child(format!("✅ {}", short_date(date, today)))
                    .into_any_element(),
            );
        }
        if !todo.checklist.is_empty() {
            let checked = todo.checklist.iter().filter(|item| item.checked).count();
            meta.push(
                div()
                    .flex_shrink_0()
                    .child(format!("{checked}/{}", todo.checklist.len()))
                    .into_any_element(),
            );
        }
        for tag in &todo.tags {
            meta.push(
                div()
                    .flex_shrink_0()
                    .text_color(pal.tint_foreground)
                    .child(format!("#{tag}"))
                    .into_any_element(),
            );
        }

        let id = format!("{}:{}", path.display(), todo.line);
        let checked = todo.checked;
        let check = {
            let (path, todo) = (path.clone(), todo.clone());
            div()
                .id(SharedString::from(format!("todo-check-{id}")))
                .pt(px(3.0))
                .cursor_pointer()
                // 체크만 하고 줄 누르기(메모 열기)로 번지지 않게 막는다.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_todo(&path, &todo, window, cx);
                }))
                .child(editor_task::check_box(px(BOX_SIDE), checked, pal))
        };
        let title = if todo.title.is_empty() {
            t!("library.todos.untitled").into()
        } else {
            SharedString::from(todo.title.clone())
        };
        div()
            .id(SharedString::from(format!("todo-{id}")))
            .mx_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .flex()
            .gap_2()
            .cursor_pointer()
            .hover(|this| this.bg(pal.hover))
            .on_click(cx.listener({
                let (path, todo) = (path.clone(), todo.clone());
                move |this, _, window, cx| this.open_todo(&path, &todo, window, cx)
            }))
            .child(check)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .line_clamp(2)
                            .text_ellipsis()
                            .when(todo.checked, |this| {
                                this.line_through().text_color(pal.muted)
                            })
                            .when(!todo.checked, |this| this.text_color(pal.foreground))
                            .child(title),
                    )
                    .when(!meta.is_empty(), |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_x_2()
                                .text_xs()
                                .text_color(pal.muted)
                                .children(meta),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// 비었거나 읽는 중인 목록의 안내.
fn empty_state(message: SharedString, hint: Option<SharedString>, pal: Palette) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .px_6()
        .text_sm()
        .text_color(pal.muted)
        .child(message)
        .when_some(hint, |this, hint| {
            this.child(div().text_xs().text_center().child(hint))
        })
        .into_any_element()
}

/// 에디터에 열리지 않은 메모의 할 일을 파일에서 고친다. 같은 줄을 찾지 못하면 `Ok(false)`.
fn toggle_todo_in_file(path: &Path, todo: &Todo) -> std::io::Result<bool> {
    let text = library::read_sheet(path)?;
    let Some(toggled) = markdown::toggle_todo_in(&text, todo.line, &todo.source, &library::today())
    else {
        return Ok(false);
    };
    library::write_sheet(path, &toggled)?;
    Ok(true)
}

/// `row`번 줄이 시작하는 바이트 위치.
fn line_start(text: &str, row: usize) -> Option<usize> {
    if row == 0 {
        return Some(0);
    }
    text.match_indices('\n').nth(row - 1).map(|(at, _)| at + 1)
}

#[cfg(test)]
mod tests {
    // `super::*`는 gpui의 `test` 속성까지 가져와 `#[test]`가 그쪽으로 풀린다.
    use super::{Bucket, PathBuf, Todo, arrange, line_start, short_date};

    fn todo(line: usize, due: Option<&str>, scheduled: Option<&str>, checked: bool) -> Todo {
        Todo {
            line,
            due: due.map(str::to_owned),
            scheduled: scheduled.map(str::to_owned),
            checked,
            ..Todo::default()
        }
    }

    #[test]
    fn buckets_by_date() {
        let today = "2026-10-25";
        let cases = [
            (todo(0, Some("2026-10-24"), None, false), Bucket::Overdue),
            (todo(0, Some("2026-10-25"), None, false), Bucket::Today),
            (todo(0, None, Some("2026-10-20"), false), Bucket::Today),
            (
                todo(0, Some("2026-11-01"), Some("2026-10-25"), false),
                Bucket::Today,
            ),
            (todo(0, None, Some("2026-10-26"), false), Bucket::Upcoming),
            (todo(0, Some("2026-10-30"), None, false), Bucket::Upcoming),
            (todo(0, None, None, false), Bucket::Anytime),
            (todo(0, Some("2026-10-01"), None, true), Bucket::Done),
        ];
        for (todo, bucket) in cases {
            assert_eq!(Bucket::of(&todo, today), bucket, "{todo:?}");
        }
    }

    #[test]
    fn arranges_buckets_in_order_and_sorts_inside() {
        let today = "2026-10-25";
        let path = PathBuf::from("a.md");
        let mut done_old = todo(5, None, None, true);
        done_old.done = Some("2026-10-01".into());
        let mut done_new = todo(6, None, None, true);
        done_new.done = Some("2026-10-20".into());
        let todos = vec![
            (path.clone(), todo(0, None, None, false)),
            (path.clone(), todo(1, Some("2026-11-02"), None, false)),
            (path.clone(), todo(2, None, Some("2026-10-28"), false)),
            (path.clone(), done_old),
            (path.clone(), todo(3, Some("2026-10-24"), None, false)),
            (path.clone(), done_new),
        ];
        let arranged: Vec<(Bucket, Vec<usize>)> = arrange(todos, today)
            .into_iter()
            .map(|(bucket, items)| (bucket, items.iter().map(|(_, todo)| todo.line).collect()))
            .collect();
        assert_eq!(
            arranged,
            [
                (Bucket::Overdue, vec![3]),
                (Bucket::Upcoming, vec![2, 1]),
                (Bucket::Anytime, vec![0]),
                (Bucket::Done, vec![6, 5]),
            ]
        );
    }

    #[test]
    fn finds_line_starts_and_short_dates() {
        assert_eq!(line_start("a\nbc\nd", 0), Some(0));
        assert_eq!(line_start("a\nbc\nd", 2), Some(5));
        assert_eq!(line_start("a", 1), None);
        assert_eq!(short_date("2026-10-30", "2026-10-25"), "10-30");
        assert_eq!(short_date("2027-01-02", "2026-10-25"), "2027-01-02");
    }
}
