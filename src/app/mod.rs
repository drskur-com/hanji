//! 앱 상태(`HanjiApp`)와 액션 핸들러. 화면별 렌더는 하위 모듈에 나눠 둔다.

mod about;
mod assistant_panel;
mod book_info;
mod book_preview;
mod book_style;
mod code_highlight;
mod compile_dialog;
mod dictation;
mod editor;
mod editor_extension;
mod editor_format;
mod editor_image;
mod editor_layout;
mod editor_link;
mod editor_table;
mod editor_task;
mod extension_actions;
mod extension_commands;
mod extension_painter;
mod file_view;
mod gif_player;
mod import_project;
mod insert_extension;
mod insert_footnote;
mod insert_image;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod insert_ocr;
mod insert_special_char;
mod insert_table;
mod library_ops;
mod library_panel;
mod manage_extensions;
mod media_player;
mod new_project;
mod onboarding;
mod play_window;
mod project_panel;
mod sheet_list;
mod sheet_search;
pub(crate) mod style;
mod todo_list;
mod update;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::FluentBuilder;
use gpui_kit::gpui::*;
use std::path::PathBuf;

#[cfg(not(target_os = "macos"))]
use gpui_kit::component::menu::AppMenuBar;

use crate::menus::{PanelsVisible, install_menus};
use crate::{
    CloseLibrary, CompileProject, OpenDictation, OpenLibrary, QuitApp, SaveSheet, SetTheme,
    ShowAbout, ToggleAssistant, ToggleBold, ToggleItalic, ToggleLineGroup, TogglePreview,
    ToggleSidebar, ToggleTheme, library, markdown, theme,
};
use style::*;

/// 그룹의 시트 정렬(방법 + 방향)과, 사용자가 고를 수 있는지.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GroupSort {
    mode: library::SortMode,
    descending: bool,
    choosable: bool,
}

/// 사이드바에서 선택된 항목(섹션, 그룹, 할 일).
#[derive(Clone, PartialEq, Eq, Debug)]
enum Selection {
    Section(library::SectionKind),
    Group(PathBuf),
    /// 메모의 할 일을 모은 보기(ADR-0024).
    Todos,
}

/// 그룹을 옮길 수 있는 범위. 같은 범위 안의 그룹끼리만 하위로 옮길 수 있다.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum GroupFamily {
    /// 메모 섹션의 그룹.
    Notes,
    /// 프로젝트의 원고 그룹(빌드 대상).
    Manuscript(PathBuf),
    /// 프로젝트의 추가자료 그룹(빌드에서 빠짐).
    Extras(PathBuf),
    /// `Projects/` 바로 아래의 프로젝트. 그룹을 담는 대상이 아니며 끌 수 없다.
    Project(PathBuf),
}

pub(crate) struct HanjiApp {
    /// 사이드바 1단(섹션+그룹) 표시 여부. 2단(시트 목록)은 항상 보인다.
    sidebar_visible: bool,
    /// 오른쪽 어시스턴트 패널 표시 여부.
    assistant_visible: bool,
    /// 어시스턴트 패널에 대화 대신 대화 기록을 보이는지.
    assistant_history: bool,
    /// 대화 기록에서 열린 시트의 대화만 보이는지.
    assistant_sheet_only: bool,
    /// 대화 기록 저장소와 그 라이브러리 루트(ADR-0031). 처음 쓸 때 연다. 열지 못했으면 `None`을 둔다.
    assistant_store: Option<(PathBuf, Option<std::sync::Arc<crate::agent::History>>)>,
    /// 어시스턴트 새 대화(실제 모델과 주고받는 대화).
    assistant_chat: assistant_panel::LiveChat,
    library: Option<library::Library>,
    /// 접힌 최상위 섹션(UI 상태; 추후 registry에 저장).
    collapsed_sections: std::collections::HashSet<library::SectionKind>,
    /// 확장 폴더의 파일과 수정 시각. 바뀔 때만 확장 명령을 다시 읽는다(ADR-0026).
    extension_files: Option<Vec<(String, Option<std::time::SystemTime>)>>,
    /// 사이드바 1단에 펼쳐 둔 프로젝트. 있으면 라이브러리 대신 프로젝트 화면을 그린다.
    open_project: Option<PathBuf>,
    /// 프로젝트 화면에서 하위 그룹을 펼친 그룹.
    expanded_groups: std::collections::HashSet<PathBuf>,
    /// 사이드바에서 선택한 항목(섹션/그룹). None이면 기본 선택을 쓴다.
    selected: Option<Selection>,
    /// 선택한 시트(에디터에 연 시트).
    selected_sheet: Option<PathBuf>,
    /// Shift+클릭으로 넓힌 시트 선택(연 시트 포함, 목록 순서). 비면 `selected_sheet`만 선택이다.
    selected_sheets: Vec<PathBuf>,
    /// Shift+클릭으로 넓힌 그룹 선택(목록 순서). 비면 `selected`의 그룹 하나만 선택이다.
    selected_groups: Vec<PathBuf>,
    /// Shift+클릭으로 그룹을 넓힐 때 기준이 되는 그룹(마지막으로 그냥 누른 그룹).
    group_anchor: Option<PathBuf>,
    /// 그룹을 끄는 동안 지금 놓일 자리(대상 줄, 앞/안/뒤). 줄에 자리 표시를 그린다.
    group_drop: Option<(PathBuf, library_panel::GroupDrop)>,
    /// 시트를 끄는 중: (끄는 시트, 지금 올라가 있는 시트)의 목록 위치. 놓일 자리 표시에 쓴다.
    /// 끌기가 끝났는지는 `has_active_drag`로 본다.
    sheet_drop: Option<(usize, usize)>,
    /// 온보딩 화면에 표시할 안내/오류 문구.
    notice: Option<SharedString>,
    /// 읽어 둔 시트 미리보기(경로 → 읽은 시점의 수정 시각, 미리보기).
    /// 보이는 목록의 시트만 읽는다.
    previews: std::collections::HashMap<PathBuf, (u128, library::Preview)>,
    /// 미리보기를 읽는 백그라운드 태스크. 새로 시작하면 이전 것은 버려진다.
    preview_task: Option<Task<()>>,
    /// 에디터에 열린 시트.
    editor: Option<editor::OpenSheet>,
    /// 에디터 자리 뷰어에 연 파일(파일 그룹). 시트와 동시에 열리지 않는다.
    open_file: Option<file_view::OpenFile>,
    /// 에디터 자리에 연 책 스타일 화면. 시트·파일과 동시에 열리지 않는다.
    book_style: Option<Entity<book_style::BookStyleView>>,
    /// 파일 그룹 목록에서 보여 줄 파일 종류. None이면 모두.
    file_filter: Option<library::FileKind>,
    /// 메모의 할 일 색인과 그 보기의 상태(ADR-0024).
    todos: todo_list::Todos,
    /// 받아쓰기(ADR-0054). 처음 녹음할 때 만든다.
    dictation: Option<dictation::Dictation>,
    /// 자동 업데이트 상태와 진행 중인 확인·내려받기.
    update: update::UpdateState,
    update_task: Option<Task<()>>,
    /// 받아쓰기의 녹음 중 요약(ADR-0055).
    dictation_ai: dictation::DictationAi,
    /// 앱 종료 직전에 열린 시트를 저장하는 구독.
    _quit_subscription: Subscription,
    _prefs_subscription: Subscription,
    _theme_subscription: Subscription,
    /// 라이브러리 폴더 감시(watcher + 변경 시 다시 스캔하는 태스크). 버리면 멈춘다.
    watcher: Option<(notify::RecommendedWatcher, Task<()>)>,
    /// 라이브러리를 읽을 때의 UI 언어. 바뀌면 다시 읽는다(기본 그룹 이름).
    ui_locale: String,
    /// 앱 루트의 포커스. 시트를 열지 않아 포커스된 요소가 없어도 메뉴 액션이 이 뷰로 오게 한다.
    focus: FocusHandle,
    #[cfg(not(target_os = "macos"))]
    menu_bar: Entity<AppMenuBar>,
}

impl HanjiApp {
    /// 앱 상태를 만들고, 라이브러리가 있으면 감시와 미리보기 읽기를 시작한다.
    pub(crate) fn new(
        library: Option<library::Library>,
        notice: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // 창을 닫거나 앱을 끝낼 때 저장하지 않은 본문을 내린다.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            this.update(cx, |this, cx| this.close_sheet(cx)).ok();
            true
        });
        // 설정(서체 등)이 바뀌면 다시 그린다.
        let prefs_subscription =
            cx.observe_global_in::<crate::settings::Prefs>(window, |this, window, cx| {
                this.refresh_editor_preferences(cx);
                this.refresh_assistant_models(window, cx);
                // 언어가 바뀌었을 수 있다(메뉴는 `i18n::apply`가 다시 설치한다).
                #[cfg(not(target_os = "macos"))]
                this.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
                // 기본 그룹(템플릿·첨부파일) 이름은 읽을 때 정하므로 언어가 바뀌면 다시 읽는다.
                let locale = rust_i18n::locale().to_string();
                if locale != this.ui_locale {
                    this.ui_locale = locale;
                    if let Some(root) = this.library.as_ref().map(|lib| lib.root.clone()) {
                        this.apply_scan(&root, library::scan(&root), window, cx);
                    }
                }
                cx.notify();
            });
        let theme_subscription = cx.observe_global::<Theme>(|this, cx| this.after_theme_change(cx));
        let quit_subscription = cx.on_app_quit(|this, cx| {
            this.close_sheet(cx);
            async {}
        });
        let mut app = HanjiApp {
            sidebar_visible: true,
            assistant_visible: false,
            assistant_history: false,
            assistant_sheet_only: false,
            assistant_store: None,
            assistant_chat: assistant_panel::LiveChat::new(window, cx),
            library,
            notice,
            collapsed_sections: std::collections::HashSet::new(),
            extension_files: None,
            open_project: None,
            expanded_groups: std::collections::HashSet::new(),
            selected: None,
            selected_sheet: None,
            selected_sheets: Vec::new(),
            selected_groups: Vec::new(),
            group_anchor: None,
            group_drop: None,
            sheet_drop: None,
            previews: std::collections::HashMap::new(),
            preview_task: None,
            editor: None,
            open_file: None,
            book_style: None,
            file_filter: None,
            todos: Default::default(),
            dictation: None,
            update: Default::default(),
            update_task: None,
            dictation_ai: Default::default(),
            _quit_subscription: quit_subscription,
            _prefs_subscription: prefs_subscription,
            _theme_subscription: theme_subscription,
            watcher: None,
            ui_locale: rust_i18n::locale().to_string(),
            focus: cx.focus_handle(),
            #[cfg(not(target_os = "macos"))]
            menu_bar: AppMenuBar::new(cx),
        };
        window.focus(&app.focus, cx);
        app.start_watching(window, cx);
        app.schedule_update_check(window, cx);
        app.refresh_assistant_models(window, cx);
        app.load_previews(cx);
        app
    }

    fn notify(
        &mut self,
        message: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.push_notification(Notification::info(message.into()), cx);
        cx.notify();
    }

    fn set_notice(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        cx.notify();
    }

    // --- 액션 핸들러 --------------------------------------------------------

    fn on_save_sheet(&mut self, _: &SaveSheet, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.save_open_sheet(cx) {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
        }
    }

    /// 선택한 줄을 줄 묶음으로 감싸거나 푼다(ADR-0016). 한 번에 되돌릴 수 있게 한 편집으로 바꾼다.
    fn on_toggle_line_group(
        &mut self,
        _: &ToggleLineGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        editor.state().update(cx, |state, cx| {
            let text = state.value();
            let (range, replacement, selected) =
                crate::text_transform::toggle_line_group(&text, state.selected_range());
            state.set_selected_range(range, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range(selected, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }

    fn on_compile_project(
        &mut self,
        _: &CompileProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project_dir) = self.active_project() {
            self.open_compile_dialog(&project_dir, window, cx);
        } else {
            self.notify(t!("library.compile_select_project"), window, cx);
        }
    }

    /// 현재 작업 중이거나 선택된 프로젝트 경로.
    pub(crate) fn active_project(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.open_project {
            return Some(dir.clone());
        }
        let library = self.library.as_ref()?;
        if let Some(ref sheet) = self.selected_sheet
            && let Some(p) = library.project_of(sheet)
        {
            return Some(p);
        }
        if let Some(Selection::Group(ref group)) = self.selected
            && let Some(p) = library.project_of(group)
        {
            return Some(p);
        }
        None
    }

    fn on_open_library(&mut self, _: &OpenLibrary, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_library(window, cx);
    }

    fn on_close_library(&mut self, _: &CloseLibrary, _: &mut Window, cx: &mut Context<Self>) {
        // 녹음은 멈춘다. 남은 문단은 작업 스레드가 기록 파일에 마저 쓴다.
        self.stop_dictation(cx);
        self.close_sheet(cx);
        self.watcher = None;
        self.preview_task = None;
        self.previews.clear();
        self.todos = Default::default();
        self.library = None;
        self.refresh_extension_commands(cx);
        self.refresh_library_themes(cx);
        self.notice = None;
        self.open_project = None;
        self.expanded_groups.clear();
        self.selected = None;
        self.selected_sheet = None;
        self.selected_sheets.clear();
        self.selected_groups.clear();
        self.group_anchor = None;
        self.open_file = None;
        self.book_style = None;
        cx.notify();
    }

    fn on_show_about(&mut self, _: &ShowAbout, window: &mut Window, cx: &mut Context<Self>) {
        self.open_about(window, cx);
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_visible(!self.sidebar_visible, cx);
    }

    /// 사이드바를 보이거나 가리고, `View` 메뉴 항목 이름을 맞춘다.
    pub(super) fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.sidebar_visible = visible;
        self.sync_panel_menus(cx);
    }

    fn on_toggle_assistant(&mut self, _: &ToggleAssistant, _: &mut Window, cx: &mut Context<Self>) {
        self.set_assistant_visible(!self.assistant_visible, cx);
    }

    /// 어시스턴트 패널을 보이거나 가리고, `View` 메뉴 항목 이름을 맞춘다.
    pub(super) fn set_assistant_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.assistant_visible = visible;
        self.sync_panel_menus(cx);
    }

    fn sync_panel_menus(&self, cx: &mut Context<Self>) {
        cx.set_global(PanelsVisible {
            sidebar: self.sidebar_visible,
            assistant: self.assistant_visible,
        });
        install_menus(cx);
        cx.notify();
    }

    /// 선택 영역(또는 커서 위치)의 굵게 표시를 켜고 끈다.
    fn on_toggle_bold(&mut self, _: &ToggleBold, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_none() {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        }
        self.toggle_inline(markdown::SpanKind::Strong, "**", window, cx);
    }

    /// 선택 영역(또는 커서 위치)의 기울임 표시를 켜고 끈다.
    fn on_toggle_italic(&mut self, _: &ToggleItalic, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_none() {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        }
        self.toggle_inline(markdown::SpanKind::Emphasis, "_", window, cx);
    }

    /// 최상위 섹션을 접거나 편다.
    fn toggle_section(&mut self, kind: library::SectionKind, cx: &mut Context<Self>) {
        // remove()가 true면 펼치기(이미 접힘), false면 접기.
        if !self.collapsed_sections.remove(&kind) {
            self.collapsed_sections.insert(kind);
        }
        cx.notify();
    }

    /// 사이드바의 받아쓰기 그룹을 고른다(ADR-0054).
    fn on_open_dictation(
        &mut self,
        _: &OpenDictation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if crate::dictation::ENABLED {
            self.show_dictation(window, cx);
        }
    }

    fn on_toggle_preview(
        &mut self,
        _: &TogglePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.editor_selection() else {
            self.notify(t!("library.preview_select"), window, cx);
            return;
        };
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        if let Err(err) = self.save_open_sheet(cx) {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
            return;
        }
        book_preview::open_selection(&root, &selected, cx);
    }

    /// 에디터의 미리보기·컴파일에서 사용할 선택 항목.
    fn editor_selection(&self) -> Option<Vec<PathBuf>> {
        if !self.selected_sheets.is_empty() {
            return Some(self.selected_sheets.clone());
        }
        self.selected_sheet
            .clone()
            .map(|path| vec![path])
            .or_else(|| match self.effective_selection() {
                Some(Selection::Group(path)) if !self.viewing_file_group() => Some(vec![path]),
                _ => None,
            })
    }

    fn on_set_theme(&mut self, action: &SetTheme, _: &mut Window, cx: &mut Context<Self>) {
        theme::choose(&action.name, cx);
    }

    fn on_toggle_theme(&mut self, _: &ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        let next = theme::counterpart(cx);
        theme::choose(&next, cx);
    }

    /// 테마가 바뀌면(메뉴·설정 창·라이브러리 테마 파일) 메뉴의 체크와 에디터 장식을 맞춘다.
    fn after_theme_change(&mut self, cx: &mut Context<Self>) {
        install_menus(cx);
        self.refresh_markup(cx);
        #[cfg(not(target_os = "macos"))]
        self.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
        cx.notify();
    }

    fn on_quit(&mut self, _: &QuitApp, _: &mut Window, cx: &mut Context<Self>) {
        self.close_sheet(cx);
        cx.quit();
    }

    // --- 렌더 헬퍼 ---------------------------------------------------------

    /// 선택 상태에 해당하는 시트 목록.
    fn sheets_for(&self, selection: &Selection) -> Vec<library::Sheet> {
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        match selection {
            // 섹션 선택("모두"): 섹션 안의 모든 시트를 최근 수정순으로.
            Selection::Section(kind) => {
                let mut sheets: Vec<library::Sheet> = lib
                    .section(*kind)
                    .map(|section| section.sheets().into_iter().cloned().collect())
                    .unwrap_or_default();
                sheets.sort_by_key(|sheet| std::cmp::Reverse(sheet.modified));
                sheets
            }
            Selection::Group(path) => {
                let mut sheets = lib
                    .find_group(path)
                    .map(|group| group.sheets.clone())
                    .unwrap_or_default();
                self.sort_sheets(&mut sheets, self.sort_for(path));
                sheets
            }
            // 할 일이 있는 메모. 목록에 메모 제목을 보이려고 미리보기를 읽는다.
            Selection::Todos => self.todo_sheets(),
        }
    }

    /// 그룹의 정렬 방법·방향과, 사용자가 고를 수 있는지.
    ///
    /// 프로젝트 원고는 순서가 곧 원고 구조라서 늘 수동이고 고를 수 없다.
    /// 지정이 없으면 메모는 수정일순, 프로젝트(추가자료)는 수동이다.
    fn sort_for(&self, path: &std::path::Path) -> GroupSort {
        let fixed = GroupSort {
            mode: library::SortMode::Manual,
            descending: false,
            choosable: false,
        };
        let Some(lib) = &self.library else {
            return fixed;
        };
        let (mode, choosable) = lib.sort_of(path);
        if !choosable {
            return fixed;
        }
        let group = lib.find_group(path);
        GroupSort {
            mode,
            descending: group
                .and_then(|group| group.descending)
                .unwrap_or(mode.default_descending()),
            choosable: true,
        }
    }

    /// 시트를 정렬 방법·방향대로 늘어놓는다. 수동이면 읽은 순서(메타 `sheets`) 그대로 둔다.
    ///
    /// 제목순은 읽어 둔 미리보기 제목을 쓴다. 아직 못 읽은 시트와 빈 시트는 방향과 상관없이 뒤로 간다.
    fn sort_sheets(&self, sheets: &mut [library::Sheet], sort: GroupSort) {
        let directed = |ordering: std::cmp::Ordering| {
            if sort.descending {
                ordering.reverse()
            } else {
                ordering
            }
        };
        match sort.mode {
            library::SortMode::Manual => {}
            library::SortMode::Modified => {
                sheets.sort_by(|a, b| directed(a.modified.cmp(&b.modified)))
            }
            library::SortMode::Created => {
                sheets.sort_by(|a, b| directed(a.created.cmp(&b.created)))
            }
            library::SortMode::Title => {
                let title = |sheet: &library::Sheet| {
                    self.previews
                        .get(&sheet.path)
                        .map(|(_, preview)| preview.title.to_lowercase())
                        .unwrap_or_default()
                };
                let mut keyed: Vec<(String, library::Sheet)> = sheets
                    .iter()
                    .map(|sheet| (title(sheet), sheet.clone()))
                    .collect();
                keyed.sort_by(|(a, _), (b, _)| {
                    a.is_empty()
                        .cmp(&b.is_empty())
                        .then_with(|| directed(a.cmp(b)))
                });
                for (slot, (_, sheet)) in sheets.iter_mut().zip(keyed) {
                    *slot = sheet;
                }
            }
        }
    }

    /// 파일 그룹의 파일을 정렬 방법·방향대로 늘어놓는다. 수동이면 읽은 순서(메타 `files`) 그대로,
    /// 제목순은 파일 이름순이다.
    fn sort_files(&self, files: &mut [library::FileItem], sort: GroupSort) {
        let directed = |ordering: std::cmp::Ordering| {
            if sort.descending {
                ordering.reverse()
            } else {
                ordering
            }
        };
        match sort.mode {
            library::SortMode::Manual => {}
            library::SortMode::Modified => {
                files.sort_by(|a, b| directed(a.modified.cmp(&b.modified)))
            }
            library::SortMode::Created => files.sort_by(|a, b| directed(a.created.cmp(&b.created))),
            library::SortMode::Title => {
                files.sort_by_cached_key(|file| file.name().to_lowercase());
                if sort.descending {
                    files.reverse();
                }
            }
        }
    }

    /// 기본 선택: 프로젝트 화면이면 그 프로젝트의 기본 선택, 아니면 첫 그룹, 없으면 첫 섹션.
    fn effective_selection(&self) -> Option<Selection> {
        if let Some(selection) = &self.selected {
            return Some(selection.clone());
        }
        let lib = self.library.as_ref()?;
        if let Some(project) = self
            .open_project
            .as_ref()
            .and_then(|dir| lib.find_project(dir))
        {
            return Some(project_panel::default_selection(project));
        }
        for section in &lib.sections {
            for entry in &section.entries {
                if let library::Entry::Group(group) = entry {
                    return Some(Selection::Group(group.path.clone()));
                }
            }
        }
        lib.sections
            .first()
            .map(|section| Selection::Section(section.kind))
    }

    /// 지금 보고 있는 목록이 파일 그룹(첨부파일 등)인지.
    fn viewing_file_group(&self) -> bool {
        match self.effective_selection() {
            Some(Selection::Group(path)) => self
                .library
                .as_ref()
                .and_then(|lib| lib.find_group(&path))
                .is_some_and(|group| group.kind == library::GroupKind::Files),
            _ => false,
        }
    }

    /// 선택 항목의 표시 이름. 그룹은 메타의 표시 이름(`title`)을 쓴다.
    fn selection_label(&self, selection: &Selection) -> String {
        match selection {
            Selection::Section(library::SectionKind::Notes) => t!("library.all").to_string(),
            Selection::Section(kind) => kind.title().to_string(),
            Selection::Todos => t!("library.todos.title").to_string(),
            Selection::Group(path) => self
                .library
                .as_ref()
                .and_then(|lib| lib.find_group(path))
                .map(|group| group.name.clone())
                .unwrap_or_default(),
        }
    }

    /// 경로로 시트를 찾는다.
    fn find_sheet(&self, path: &PathBuf) -> Option<library::Sheet> {
        self.library
            .iter()
            .flat_map(|lib| {
                lib.sections
                    .iter()
                    .flat_map(library::Section::sheets)
                    .chain(&lib.stt.sheets)
            })
            .find(|sheet| &sheet.path == path)
            .cloned()
    }

    fn render_workspace(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .size_full()
            .child(self.render_library_panel(cx, pal))
            // 책 스타일 화면은 폼과 미리보기를 나란히 두려고 시트 목록 칸까지 쓴다.
            .when(self.book_style.is_none(), |this| {
                this.child(self.render_sheet_column(cx, pal))
            })
            .child(self.render_editor_column(cx, pal))
            .child(self.render_assistant_panel(cx, pal))
    }
}

impl Render for HanjiApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 테마 토큰을 값(Copy)으로 읽어 둔다. 이후 `cx.listener`와 대여 충돌이 없다.
        let pal = Palette::from_theme(cx.theme());
        // 포커스가 없으면(시트를 닫았거나 연 적이 없으면) 메뉴 액션이 이 뷰까지 오지 않아 꺼진다.
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }

        let root = div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(pal.background)
            .text_color(pal.foreground)
            .on_action(cx.listener(Self::on_new_sheet))
            .on_action(cx.listener(Self::on_save_sheet))
            .on_action(cx.listener(Self::on_compile_project))
            .on_action(cx.listener(Self::on_insert_image))
            .on_action(cx.listener(Self::on_insert_video))
            .on_action(cx.listener(Self::on_insert_audio))
            .on_action(cx.listener(Self::on_insert_footnote))
            .on_action(cx.listener(Self::on_insert_table))
            .on_action(cx.listener(Self::on_insert_extension))
            .on_action(cx.listener(Self::on_insert_special_char))
            .on_action(cx.listener(Self::on_search_sheets))
            .on_action(cx.listener(Self::on_toggle_line_group))
            .on_action(cx.listener(Self::on_open_library))
            .on_action(cx.listener(Self::on_close_library))
            .on_action(cx.listener(Self::on_import_project))
            .on_action(cx.listener(Self::on_show_about))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_toggle_assistant))
            .on_action(cx.listener(Self::on_toggle_bold))
            .on_action(cx.listener(Self::on_toggle_italic))
            .on_action(cx.listener(Self::on_check_for_updates))
            .on_action(cx.listener(Self::on_insert_link))
            .on_action(cx.listener(Self::on_format_body))
            .on_action(cx.listener(Self::on_format_heading1))
            .on_action(cx.listener(Self::on_format_heading2))
            .on_action(cx.listener(Self::on_format_heading3))
            .on_action(cx.listener(Self::on_format_heading4))
            .on_action(cx.listener(Self::on_format_heading5))
            .on_action(cx.listener(Self::on_format_heading6))
            .on_action(cx.listener(Self::on_format_bullet_list))
            .on_action(cx.listener(Self::on_format_numbered_list))
            .on_action(cx.listener(Self::on_format_quote))
            .on_action(cx.listener(Self::on_toggle_preview))
            .on_action(cx.listener(Self::on_open_dictation))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_set_theme))
            .on_action(cx.listener(Self::on_run_extension_command))
            .on_action(cx.listener(Self::on_manage_extensions))
            .on_action(cx.listener(Self::on_quit));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let root = root.on_action(cx.listener(Self::on_insert_ocr_text));

        // 커스텀 타이틀바 + 메뉴바 (macOS는 투명 타이틀바 + 네이티브 메뉴를 쓰므로 생략)
        #[cfg(not(target_os = "macos"))]
        let root = {
            let theme = cx.theme();
            let (title_bar_background, title_bar_border) =
                (theme.title_bar, theme.title_bar_border);
            root.child(
                TitleBar::new()
                    .bg(title_bar_background)
                    .border_color(title_bar_border)
                    .child(self.menu_bar.clone()),
            )
        };

        // 바디: 라이브러리가 있으면 워크스페이스, 없으면 온보딩
        let body: AnyElement = if self.library.is_some() {
            self.render_workspace(cx, pal).into_any_element()
        } else {
            self.render_onboarding(cx, pal).into_any_element()
        };

        root.child(body)
    }
}
