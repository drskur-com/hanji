//! 라이브러리 열기·감시·다시 스캔·미리보기·시트 만들기/옮기기 (상태 변경 로직).

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::*;
use gpui_kit::gpui::*;
use std::path::{Path, PathBuf};

use super::library_panel::DraggedGroups;
use super::{GroupFamily, HanjiApp, Selection};
use crate::{APP_NAME, NewSheet, config, library, watch};

/// 정렬 메뉴에서 고른 것.
#[derive(Clone, Copy)]
pub(super) enum SortChange {
    Mode(library::SortMode),
    Descending(bool),
}

impl HanjiApp {
    // --- 라이브러리 폴더 설정 ---------------------------------------------

    /// 기본 제안 경로에 새 라이브러리를 만든다.
    pub(super) fn create_default_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = config::default_library_root() else {
            self.set_notice(t!("library.notice.no_default_location"), cx);
            return;
        };
        self.initialize_at(root, window, cx);
    }

    /// 라이브러리 골격을 만든 뒤 연다.
    pub(super) fn initialize_at(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match library::initialize(&root) {
            Ok(()) => self.adopt(root, window, cx),
            Err(err) => self.set_notice(t!("library.notice.create_failed", err = err), cx),
        }
    }

    /// 기존 폴더를 검사해 알맞게 연다.
    pub(super) fn open_folder(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match library::inspect(&path) {
            Ok(library::FolderKind::Library) => self.adopt(path, window, cx),
            Ok(library::FolderKind::Empty) | Ok(library::FolderKind::MarkdownOnly) => {
                self.initialize_at(path, window, cx);
            }
            // 비어 있지 않은 폴더: 하위에 Hanji/ 를 만든다 (ADR-0003 D3).
            Ok(library::FolderKind::NonEmpty) => {
                self.initialize_at(path.join(APP_NAME), window, cx);
            }
            Err(err) => self.set_notice(t!("library.notice.folder_unusable", err = err), cx),
        }
    }

    /// 라이브러리를 현재 라이브러리로 채택하고 설정에 저장한다.
    pub(super) fn adopt(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match library::open(&root) {
            Ok(opened) => {
                crate::sync_builtin_skills(&opened.root);
                let mut config = config::Config::load();
                config.set_library(opened.root.clone());
                if let Err(err) = config.save() {
                    eprintln!("설정 저장 실패: {err}");
                }
                window.push_notification(
                    Notification::info(t!("library.notice.opened", path = opened.root.display())),
                    cx,
                );
                self.close_sheet(cx);
                self.library = Some(opened);
                self.notice = None;
                self.open_project = None;
                self.expanded_groups.clear();
                self.selected = None;
                self.selected_groups.clear();
                self.group_anchor = None;
                self.previews.clear();
                self.start_watching(window, cx);
                self.load_previews(cx);
                cx.notify();
            }
            Err(err) => self.set_notice(t!("library.notice.read_failed", err = err), cx),
        }
    }

    /// OS 네이티브 폴더 선택기로 라이브러리 폴더를 고른다.
    pub(super) fn pick_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("library.pick_folder").into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let Some(path) = picked else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_folder(path, window, cx))
                .ok();
        })
        .detach();
    }

    /// 열린 라이브러리의 `Themes/`를 다시 읽는다(ADR-0047). 바뀌면 테마 관찰이 메뉴와 화면을 맞춘다.
    pub(super) fn refresh_library_themes(&self, cx: &mut Context<Self>) {
        let root = self.library.as_ref().map(|lib| lib.root.clone());
        crate::theme::load_library(root.as_deref(), cx);
    }

    // --- 폴더 감시 ---------------------------------------------------------

    /// 현재 라이브러리 루트 감시를 (다시) 시작한다. 변경이 오면 잠깐 모았다가 다시 스캔한다.
    /// 할 일 색인(ADR-0024)도 새로 만들고, 이후에는 바뀐 경로만 다시 읽는다.
    pub(super) fn start_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.watcher = None;
        self.refresh_extension_commands(cx);
        self.refresh_library_themes(cx);
        self.load_todos(cx);
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let (watcher, mut events) = match watch::watch(&root) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("폴더 감시 실패: {err}");
                return;
            }
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(mut changed) = events.next().await {
                // 저장·이동은 이벤트가 여러 개 연달아 온다. 잠깐 기다렸다가 한꺼번에 처리한다.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                while let Ok(more) = events.try_recv() {
                    changed.extend(more);
                }
                changed.sort();
                changed.dedup();

                // 디스크 I/O는 백그라운드에서 하고, 결과 반영만 UI 스레드에서 한다.
                let scan_root = root.clone();
                let scanned = cx
                    .background_executor()
                    .spawn(async move { library::scan(&scan_root) })
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        this.refresh_todos(&changed, cx);
                        this.apply_scan(&root, scanned, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.watcher = Some((watcher, task));
    }

    /// 다시 읽은 라이브러리를 반영한다. 사라진 시트/그룹의 선택은 푼다.
    ///
    /// 스캔하는 사이에 다른 라이브러리로 바뀌었으면 결과를 버린다.
    pub(super) fn apply_scan(
        &mut self,
        root: &std::path::Path,
        scanned: std::io::Result<library::Library>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.library.as_ref().map(|lib| lib.root.as_path()) != Some(root) {
            return;
        }
        let scanned = match scanned {
            Ok(scanned) => scanned,
            Err(err) => {
                eprintln!("라이브러리 다시 읽기 실패: {err}");
                return;
            }
        };
        self.library = Some(scanned);
        self.refresh_library_themes(cx);
        // 받아쓰기 그룹은 폴더가 아직 없어도(첫 녹음 전) 고를 수 있다.
        let stt = self.library.as_ref().map(|lib| lib.stt.path.clone());
        if let Some(Selection::Group(path)) = &self.selected
            && !path.is_dir()
            && Some(path) != stt.as_ref()
        {
            self.selected = None;
        }
        self.selected_groups.retain(|path| path.is_dir());
        if self
            .group_anchor
            .as_ref()
            .is_some_and(|path| !path.is_dir())
        {
            self.group_anchor = None;
        }
        if self.open_project.as_ref().is_some_and(|dir| !dir.is_dir()) {
            self.open_project = None;
        }
        self.expanded_groups.retain(|dir| dir.is_dir());
        if let Some(path) = &self.selected_sheet
            && self.find_sheet(path).is_none()
        {
            self.selected_sheet = None;
        }
        if self
            .open_file
            .as_ref()
            .is_some_and(|file| !file.path.is_file())
        {
            self.open_file = None;
        }
        if self
            .book_style
            .as_ref()
            .is_some_and(|view| !view.read(cx).project().is_dir())
        {
            self.book_style = None;
        }
        self.sync_open_sheet(window, cx);
        self.refresh_extension_commands(cx);
        self.load_previews(cx);
        cx.notify();
    }

    /// 그룹 경로가 속한 이동 범위. 같은 범위 안의 그룹끼리만 옮길 수 있다.
    pub(super) fn group_family(&self, path: &Path) -> Option<GroupFamily> {
        let lib = self.library.as_ref()?;
        let notes = lib.root.join(library::SectionKind::Notes.dir());
        if path.starts_with(&notes) {
            return Some(GroupFamily::Notes);
        }
        let projects = lib.root.join(library::SectionKind::Projects.dir());
        let relative = path.strip_prefix(&projects).ok()?;
        let mut parts = relative.components();
        let project_name = parts.next()?.as_os_str().to_owned();
        let project = projects.join(project_name);
        match parts.next() {
            None => Some(GroupFamily::Project(project)),
            Some(part) if part.as_os_str() == library::EXTRAS_DIR => {
                Some(GroupFamily::Extras(project))
            }
            Some(_) => Some(GroupFamily::Manuscript(project)),
        }
    }

    /// 실제 그룹이고, 다른 그룹의 하위로 옮길 수 있는지.
    pub(super) fn group_is_movable(&self, path: &Path) -> bool {
        let is_group = self
            .library
            .as_ref()
            .is_some_and(|lib| lib.find_group(path).is_some());
        is_group
            && !library::is_reserved_group(path)
            && !matches!(
                self.group_family(path),
                None | Some(GroupFamily::Project(_))
            )
    }

    /// 지금 화면에 보이는 그룹 줄(경로, 위에서 아래 순서).
    pub(super) fn visible_group_paths(&self) -> Vec<PathBuf> {
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        if let Some(project) = self
            .open_project
            .as_ref()
            .and_then(|dir| lib.find_project(dir))
        {
            let mut paths = Vec::new();
            for group in project.groups.iter().chain(&project.extras) {
                push_visible_groups(group, &self.expanded_groups, &mut paths);
            }
            return paths;
        }
        // 라이브러리 화면은 섹션 아래 최상위 그룹만 그린다.
        let mut paths = Vec::new();
        for section in &lib.sections {
            for entry in &section.entries {
                if let library::Entry::Group(group) = entry {
                    paths.push(group.path.clone());
                }
            }
        }
        paths
    }

    /// Shift+클릭으로 기준 그룹부터 누른 그룹까지 넓혀 고른다. 넓혔으면 `true`.
    pub(super) fn select_group_range(&mut self, clicked: &Path, cx: &mut Context<Self>) -> bool {
        let Some(anchor) = self.group_anchor.clone() else {
            return false;
        };
        if !self.group_is_movable(&anchor)
            || !self.group_is_movable(clicked)
            || self.group_family(&anchor) != self.group_family(clicked)
        {
            return false;
        }
        let visible = self.visible_group_paths();
        let (Some(anchor_index), Some(clicked_index)) = (
            visible.iter().position(|path| path == &anchor),
            visible.iter().position(|path| path == clicked),
        ) else {
            return false;
        };
        let range = anchor_index.min(clicked_index)..=anchor_index.max(clicked_index);
        self.selected_groups = visible[range].to_vec();
        self.selected_sheets.clear();
        cx.notify();
        true
    }

    /// 끌 그룹들. 선택한 그룹 중 같은 범위의 그룹을 함께 옮기고, 부모와 자식이 모두 있으면 부모만 쓴다.
    pub(super) fn drag_group_paths(&self, clicked: &Path) -> Vec<PathBuf> {
        let family = self.group_family(clicked);
        let mut paths: Vec<PathBuf> = if self.selected_groups.iter().any(|path| path == clicked) {
            self.selected_groups
                .iter()
                .filter(|path| self.group_is_movable(path) && self.group_family(path) == family)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        if paths.is_empty() {
            paths.push(clicked.to_path_buf());
        }
        paths.sort_by_key(|path| path.components().count());
        let mut roots: Vec<PathBuf> = Vec::new();
        for path in paths {
            if !roots.iter().any(|root| path.starts_with(root)) {
                roots.push(path);
            }
        }
        roots
    }

    /// 그룹을 끌어 다른 그룹의 하위로 옮긴다. 파일을 옮기고 양쪽 부모의 `groups` 순서를 저장한다.
    pub(super) fn move_groups_to(
        &mut self,
        target_path: &Path,
        dragged: &DraggedGroups,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.group_family(target_path).as_ref() != Some(&dragged.family)
            || !self.group_is_movable(target_path)
        {
            return;
        }
        let mut sources: Vec<PathBuf> = dragged
            .paths
            .iter()
            .filter(|path| {
                let path = path.as_path();
                path != target_path
                    && !target_path.starts_with(path)
                    && !path.starts_with(target_path)
            })
            .filter(|path| {
                self.group_is_movable(path)
                    && self.group_family(path) == Some(dragged.family.clone())
            })
            .cloned()
            .collect();
        if sources.is_empty() {
            return;
        }
        // 부모를 옮기면 자식은 폴더째 따라오므로, 겹치는 자식은 뺀다.
        sources.sort_by_key(|path| path.components().count());
        let mut roots: Vec<PathBuf> = Vec::new();
        for path in sources {
            if !roots.iter().any(|root| path.starts_with(root)) {
                roots.push(path);
            }
        }

        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        let target_name = self
            .library
            .as_ref()
            .and_then(|lib| lib.find_group(target_path))
            .map(|group| group.name.clone())
            .unwrap_or_else(|| target_path.display().to_string());

        // 파일 경로가 바뀌므로 열린 시트를 먼저 저장한다. 저장에 실패하면 옮기지 않는다.
        if self.editor.is_some()
            && let Err(err) = self.save_open_sheet(cx)
        {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
            return;
        }
        let open_path = self
            .editor
            .as_ref()
            .map(|editor| editor.path().to_path_buf());

        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for source in &roots {
            match library::move_group(source, target_path) {
                Ok(new_path) => moved.push((source.clone(), new_path)),
                Err(err) => {
                    self.notify(t!("library.group.move_failed", err = err), window, cx);
                    break;
                }
            }
        }
        if moved.is_empty() {
            return;
        }

        match library::scan(&root) {
            Ok(scanned) => self.library = Some(scanned),
            Err(err) => {
                self.notify(t!("library.reread_failed", err = err), window, cx);
                return;
            }
        }

        let rebase = |path: &Path| rebase_moved(path, &moved);
        if let Some(Selection::Group(path)) = &self.selected {
            self.selected = Some(Selection::Group(rebase(path)));
        }
        self.group_anchor = self.group_anchor.as_deref().map(rebase);
        self.selected_groups = self
            .selected_groups
            .iter()
            .map(|path| rebase(path))
            .collect();
        self.selected_sheet = self.selected_sheet.as_deref().map(rebase);
        self.selected_sheets = self
            .selected_sheets
            .iter()
            .map(|path| rebase(path))
            .collect();
        self.expanded_groups = self
            .expanded_groups
            .iter()
            .map(|path| rebase(path))
            .collect();
        self.open_project = self.open_project.as_deref().map(rebase);
        if let Some(file) = &mut self.open_file {
            file.path = rebase(&file.path);
        }
        if let Some(old) = open_path {
            let new = rebase(&old);
            if new != old {
                self.relocate_open_sheet(&old, &new, cx);
            }
        }

        // 옮긴 그룹이 대상 아래에 보이도록 펼쳐 둔다.
        self.expanded_groups.insert(target_path.to_path_buf());
        self.load_previews(cx);
        self.notify(
            t!(
                "library.group.moved",
                count = moved.len(),
                target = target_name
            ),
            window,
            cx,
        );
    }

    /// 그룹 안에서 선택한 시트를 옮기고 순서를 저장한다(ADR-0014, ADR-0019).
    pub(super) fn move_selected_sheets(
        &mut self,
        group_path: &std::path::Path,
        indices: &[usize],
        anchor: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .library
            .as_mut()
            .and_then(|lib| lib.find_group_mut(group_path))
        else {
            return;
        };
        let result = if indices == [anchor] {
            library::move_sheet(group, anchor, to)
        } else {
            library::move_selected_sheets(group, indices, anchor, to)
        };
        if let Err(err) = result {
            self.notify(t!("library.order_save_failed", err = err), window, cx);
            return;
        }
        if !self.selected_sheets.is_empty() {
            self.selected_sheets = group
                .sheets
                .iter()
                .filter(|sheet| self.selected_sheets.contains(&sheet.path))
                .map(|sheet| sheet.path.clone())
                .collect();
        }
        cx.notify();
    }

    /// 선택한 시트를 다른 그룹의 끝으로 옮긴다(ADR-0019).
    ///
    /// `paths`는 옮길 시트 파일 경로다. 표시 순서(정렬 후)와 그룹의 실제 순서가 다를 수 있어
    /// 원본 그룹에서의 위치로 바꿔 [`library::move_sheets_between`]에 넘긴다.
    /// 파일이 실제로 그룹 폴더 사이를 이동한다. 열린 시트가 옮겨지면 저장한 뒤 새 경로로 옮겨
    /// 편집을 이어 간다. 원본에 남은 시트와 옮긴 묶음 안의 접합은 도메인 함수가 맞춘다.
    pub(super) fn move_sheets_to_group(
        &mut self,
        source_path: &std::path::Path,
        target_path: &std::path::Path,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if source_path == target_path || self.refuse_recording_sheets(paths, window, cx) {
            return;
        }
        let Some(source) = self
            .library
            .as_ref()
            .and_then(|lib| lib.find_group(source_path))
            .cloned()
        else {
            return;
        };
        let Some(target) = self
            .library
            .as_ref()
            .and_then(|lib| lib.find_group(target_path))
        else {
            return;
        };
        if target.kind == library::GroupKind::Files {
            self.notify(t!("library.sheet.no_sheets_in_file_group"), window, cx);
            return;
        }
        let target_name = target.name.clone();
        let mut target = target.clone();

        // 파일 경로가 바뀌므로 열린 시트를 먼저 저장한다. 저장에 실패하면 옮기지 않는다.
        if self.editor.is_some()
            && let Err(err) = self.save_open_sheet(cx)
        {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
            return;
        }
        let open_path = self
            .editor
            .as_ref()
            .map(|editor| editor.path().to_path_buf());

        let mut source = source;
        let indices: Vec<usize> = paths
            .iter()
            .filter_map(|path| source.sheets.iter().position(|sheet| &sheet.path == path))
            .collect();
        let moved = match library::move_sheets_between(&mut source, &mut target, &indices) {
            Ok(moved) => moved,
            Err(err) => {
                self.notify(t!("library.sheet.move_failed", err = err), window, cx);
                return;
            }
        };
        if moved.is_empty() {
            return;
        }

        if let Some(lib) = self.library.as_mut() {
            if let Some(group) = lib.find_group_mut(source_path) {
                group.sheets = source.sheets;
            }
            if let Some(group) = lib.find_group_mut(target_path) {
                group.sheets = target.sheets;
            }
        }

        let new_path = |path: &PathBuf| {
            moved
                .iter()
                .find(|(old, _)| old == path)
                .map(|(_, new)| new.clone())
                .unwrap_or_else(|| path.clone())
        };
        self.selected_sheet = self.selected_sheet.take().map(|path| new_path(&path));
        self.selected_sheets = self.selected_sheets.iter().map(new_path).collect();
        if let Some(old) = open_path
            && let Some((_, new)) = moved.iter().find(|(from, _)| *from == old)
        {
            self.relocate_open_sheet(&old, new, cx);
        }

        // 옮긴 그룹을 펼쳐 두면 다음에 그 그룹을 열었을 때 바로 보인다.
        self.expanded_groups.insert(target_path.to_path_buf());
        self.load_previews(cx);
        self.notify(
            t!(
                "library.sheet.moved",
                count = moved.len(),
                target = target_name
            ),
            window,
            cx,
        );
    }

    /// 그룹의 `range` 시트를 접합하거나(`glue`) 앞뒤 시트에서 떼어 내고 그룹 메타에 저장한다.
    pub(super) fn set_glued(
        &mut self,
        group_path: &std::path::Path,
        range: std::ops::Range<usize>,
        glue: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .library
            .as_mut()
            .and_then(|lib| lib.find_group_mut(group_path))
        else {
            return;
        };
        let result = if glue {
            library::glue_sheets(group, range)
        } else {
            library::unglue_sheets(group, range)
        };
        if let Err(err) = result {
            self.notify(t!("library.sheet.glue_save_failed", err = err), window, cx);
            return;
        }
        cx.notify();
    }

    /// 확인을 받은 뒤 시트 `paths`를 운영체제 휴지통으로 옮긴다. `label`은 하나일 때 보일 제목이다.
    pub(super) fn confirm_delete_sheets(
        &mut self,
        paths: Vec<PathBuf>,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_recording_sheets(&paths, window, cx) {
            return;
        }
        let description = match paths.len() {
            0 => return,
            1 => t!("library.sheet.delete_one", label = label),
            count => t!("library.sheet.delete_many", count = count),
        };
        let app = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let app = app.clone();
            let paths = paths.clone();
            alert
                .confirm()
                .title(t!("library.sheet.delete_title"))
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("common.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    app.update(cx, |app, cx| app.delete_sheets(&paths, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// 시트를 휴지통으로 옮긴다. 열린 시트가 그 안에 있으면 저장하고 닫는다. 그룹의 접합은
    /// [`library::trash_sheets`]가 맞춘다.
    fn delete_sheets(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        if self
            .selected_sheet
            .as_ref()
            .is_some_and(|path| paths.contains(path))
        {
            self.close_sheet(cx);
        }
        // 그룹별로 나눠 옮긴다. 그룹에 속하지 않은 시트(섹션 바로 아래)는 파일만 옮긴다.
        let mut by_dir: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        for path in paths {
            let dir = path.parent().unwrap_or(&root).to_path_buf();
            match by_dir.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, list)) => list.push(path.clone()),
                None => by_dir.push((dir, vec![path.clone()])),
            }
        }
        let mut error = None;
        for (dir, list) in by_dir {
            match self
                .library
                .as_mut()
                .and_then(|lib| lib.find_group_mut(&dir))
            {
                Some(group) => {
                    if let (_, Some(err)) = library::trash_sheets(group, &list) {
                        error.get_or_insert(err);
                    }
                }
                None => {
                    for path in &list {
                        if let Err(err) = library::move_to_trash(path) {
                            error.get_or_insert(err);
                        }
                    }
                }
            }
        }
        match library::scan(&root) {
            Ok(scanned) => self.library = Some(scanned),
            Err(err) => {
                error.get_or_insert(err);
            }
        }
        self.selected_sheets.retain(|path| path.is_file());
        self.load_previews(cx);
        if let Some(err) = error {
            self.notify(t!("library.sheet.delete_failed", err = err), window, cx);
        }
        cx.notify();
    }

    /// 파일 그룹에서 파일 순서를 바꾸고 그룹 메타에 저장한다.
    pub(super) fn move_file(
        &mut self,
        group_path: &std::path::Path,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .library
            .as_mut()
            .and_then(|lib| lib.find_group_mut(group_path))
        else {
            return;
        };
        if let Err(err) = library::move_file(group, from, to) {
            self.notify(t!("library.order_save_failed", err = err), window, cx);
            return;
        }
        cx.notify();
    }

    /// 그룹의 정렬 방법이나 방향을 바꾸고 그룹 메타에 저장한다.
    pub(super) fn set_sort(
        &mut self,
        group_path: &std::path::Path,
        change: SortChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .library
            .as_mut()
            .and_then(|lib| lib.find_group_mut(group_path))
        else {
            return;
        };
        let saved = match change {
            SortChange::Mode(mode) => library::set_sort(group, mode),
            SortChange::Descending(descending) => library::set_sort_descending(group, descending),
        };
        if let Err(err) = saved {
            self.notify(t!("library.sort_save_failed", err = err), window, cx);
            return;
        }
        cx.notify();
    }

    /// 지금 보이는 목록에서 아직 안 읽었거나 바뀐 시트의 미리보기를 백그라운드에서 읽는다.
    ///
    /// 제목순 그룹은 순서가 미리보기 제목에 달려 있으므로 바로 읽는다. 백그라운드로 읽으면
    /// 처음엔 읽은 순서로 그렸다가 제목이 들어오면서 다시 정렬되어 목록이 한 번 뒤바뀐다.
    pub(super) fn load_previews(&mut self, cx: &mut Context<Self>) {
        let selection = self.effective_selection();
        let by_title = matches!(&selection, Some(Selection::Group(path))
            if self.sort_for(path).mode == library::SortMode::Title);
        let pending: Vec<(PathBuf, u128)> = selection
            .map(|selection| self.sheets_for(&selection))
            .unwrap_or_default()
            .into_iter()
            .filter(|sheet| {
                self.previews
                    .get(&sheet.path)
                    .is_none_or(|(modified, _)| *modified != sheet.modified)
            })
            .map(|sheet| (sheet.path, sheet.modified))
            .collect();
        if pending.is_empty() {
            return;
        }
        if by_title {
            self.preview_task = None;
            self.previews.extend(
                pending.into_iter().map(|(path, modified)| {
                    (path.clone(), (modified, library::read_preview(&path)))
                }),
            );
            cx.notify();
            return;
        }
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    pending
                        .into_iter()
                        .map(|(path, modified)| {
                            let preview = library::read_preview(&path);
                            (path, (modified, preview))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.previews.extend(loaded);
                cx.notify();
            })
            .ok();
        }));
    }

    /// 현재 선택한 그룹에 빈 시트를 만들고 선택한다.
    pub(super) fn on_new_sheet(
        &mut self,
        _: &NewSheet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(lib) = &self.library else {
            self.notify(t!("common.open_library_first"), window, cx);
            return;
        };
        let root = lib.root.clone();
        let dir = match self.effective_selection() {
            Some(Selection::Group(path)) => path,
            // "모두"(메모)·할 일에서는 Inbox에 만든다.
            Some(Selection::Section(library::SectionKind::Notes) | Selection::Todos) | None => root
                .join(library::SectionKind::Notes.dir())
                .join(library::DEFAULT_INBOX),
            Some(Selection::Section(kind)) => root.join(kind.dir()),
        };
        if lib
            .find_group(&dir)
            .is_some_and(|group| group.kind == library::GroupKind::Files)
        {
            self.notify(t!("library.sheet.cant_create_in_file_group"), window, cx);
            return;
        }

        // 수동 정렬 그룹(프로젝트 원고 등)은 새 시트를 목록 끝에 붙인다.
        let manual = self.sort_for(&dir).mode == library::SortMode::Manual;
        let created =
            library::create_sheet(&dir).and_then(|path| Ok((path, library::scan(&root)?)));
        match created {
            Ok((path, mut scanned)) => {
                // 순서 저장 실패는 시트 만들기 실패로 보지 않는다. 파일은 이미 있고, 목록은
                // 다음 스캔에서 최근 수정순으로 보인다.
                if manual
                    && let Some(group) = scanned.find_group_mut(&dir)
                    && let Err(err) = library::append_sheet(group, &path)
                {
                    self.notify(t!("library.sheet.order_save_failed", err = err), window, cx);
                }
                self.library = Some(scanned);
                // 새 시트는 바로 쓸 수 있게 에디터에 열고 포커스를 준다.
                self.select_sheet(Some(path), true, window, cx);
                self.load_previews(cx);
            }
            Err(err) => self.notify(t!("library.sheet.create_failed", err = err), window, cx),
        }
    }
}

/// 그룹과, 펼쳐져 있으면 그 하위 그룹을 화면 순서대로 모은다.
fn push_visible_groups(
    group: &library::Group,
    expanded: &std::collections::HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    out.push(group.path.clone());
    if expanded.contains(&group.path) {
        for child in &group.groups {
            push_visible_groups(child, expanded, out);
        }
    }
}

/// 옮긴 그룹 안의 경로를 새 위치로 바꾼다.
fn rebase_moved(path: &Path, moved: &[(PathBuf, PathBuf)]) -> PathBuf {
    for (from, to) in moved {
        if path == from {
            return to.clone();
        }
        if let Ok(relative) = path.strip_prefix(from) {
            return to.join(relative);
        }
    }
    path.to_path_buf()
}
