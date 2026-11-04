//! 프로젝트 대화상자: 이름·색상·아이콘을 고르고 `Projects/` 아래에 그룹 폴더를 만들거나 고친다.
//! 프로젝트 안의 새 그룹도 같은 대화상자로 만든다. 프로젝트 편집·삭제(휴지통)도 여기 둔다.

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, FocusableExt as _, Icon, WindowExt as _};
use gpui_kit::gpui::prelude::{FluentBuilder as _, InteractiveElement as _};
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};

use super::style::*;
use super::{HanjiApp, Selection};
use crate::{icons, library};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 460.0;

/// 라벨 열 폭. 라벨은 오른쪽 정렬해 컨트롤과 붙인다.
const LABEL_WIDTH: f32 = 48.0;

/// 색 견본 지름.
const SWATCH_SIZE: f32 = 18.0;

/// 아이콘 버튼 지름.
const ICON_BUTTON_SIZE: f32 = 40.0;

/// 처음 고른 색(`PROJECT_COLORS`의 파랑).
const DEFAULT_COLOR: usize = 5;

/// 아이콘 고르기 팝오버: 한 줄에 놓는 아이콘 수, 칸 크기, 목록 높이.
const PICKER_COLUMNS: usize = 9;
const PICKER_CELL: f32 = 36.0;
const PICKER_LIST_HEIGHT: f32 = 288.0;

/// 대화상자가 하는 일.
#[derive(Clone)]
pub(super) enum FormTarget {
    NewProject,
    /// 이 폴더의 프로젝트를 고친다.
    EditProject(PathBuf),
    /// `parent` 아래에 그룹을 만든다(`manuscript`면 원고, 아니면 추가자료).
    NewGroup {
        parent: PathBuf,
        manuscript: bool,
    },
    /// 이 폴더의 그룹을 고친다(`manuscript`면 원고, 아니면 추가자료).
    EditGroup {
        dir: PathBuf,
        manuscript: bool,
    },
}

/// 대화상자 안의 입력 상태.
struct NewProjectForm {
    name: Entity<InputState>,
    color: usize,
    /// 고른 아이콘의 메타 이름(별칭 또는 Lucide 파일 이름).
    icon: SharedString,
    picker: Entity<IconPicker>,
    picker_open: bool,
    target: FormTarget,
    app: WeakEntity<HanjiApp>,
    _subscriptions: Vec<Subscription>,
}

impl NewProjectForm {
    /// `editing`이 있으면 그 프로젝트의 이름·색·아이콘으로 채운다.
    fn new(
        app: WeakEntity<HanjiApp>,
        target: FormTarget,
        editing: Option<&library::Group>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_name = editing.map_or(t!("library.untitled").to_string(), |group| {
            group.name.clone()
        });
        let name = cx.new(|cx| InputState::new(window, cx).default_value(initial_name));
        // 새 그룹의 기본값: 원고는 본문 색 + 문서 묶음, 추가자료는 파랑 + 폴더.
        let (default_color, default_icon) = match &target {
            FormTarget::NewGroup {
                manuscript: true, ..
            }
            | FormTarget::EditGroup {
                manuscript: true, ..
            } => (0, icons::MANUSCRIPT_GROUP_ICON),
            FormTarget::NewGroup {
                manuscript: false, ..
            }
            | FormTarget::EditGroup {
                manuscript: false, ..
            } => (DEFAULT_COLOR, icons::EXTRAS_GROUP_ICON),
            _ => (DEFAULT_COLOR, icons::PROJECT_ICONS[0]),
        };
        let color = editing
            .and_then(|group| group.color.as_deref())
            .and_then(|color| PROJECT_COLORS.iter().position(|&name| name == color))
            .unwrap_or(default_color);
        let icon = match editing {
            Some(group) => group.icon.clone().unwrap_or_else(|| match target {
                FormTarget::EditGroup { .. } => default_icon.to_string(),
                _ => "folder".into(),
            }),
            None => default_icon.to_string(),
        };
        // 이름 칸에서 Enter를 누르면 바로 만든다.
        let enter = cx.subscribe_in(&name, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        });
        let picker = cx.new(|cx| IconPicker::new(window, cx));
        let picked = cx.subscribe(&picker, |this, _, IconPicked(name), cx| {
            this.icon = name.clone();
            this.picker_open = false;
            cx.notify();
        });
        Self {
            name,
            color,
            icon: icon.into(),
            picker,
            picker_open: false,
            target,
            app,
            _subscriptions: vec![enter, picked],
        }
    }

    /// 프로젝트·그룹을 만들거나 고치고 대화상자를 닫는다. 실패하면 알림을 띄우고 열어 둔다.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let icon = self.icon.clone();
        let color = PROJECT_COLORS[self.color];
        let target = self.target.clone();
        let done = self.app.update(cx, |app, cx| match &target {
            FormTarget::NewProject => app.create_project(&name, &icon, color, window, cx),
            FormTarget::EditProject(dir) => {
                app.edit_folder(dir, false, &name, &icon, color, window, cx)
            }
            FormTarget::EditGroup { dir, .. } => {
                app.edit_folder(dir, true, &name, &icon, color, window, cx)
            }
            FormTarget::NewGroup { parent, .. } => {
                app.create_group(parent, &name, &icon, color, window, cx)
            }
        });
        if done.unwrap_or(false) {
            window.close_dialog(cx);
        }
    }
}

impl Render for NewProjectForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let tint = project_color(PROJECT_COLORS[self.color], cx.theme()).unwrap_or(pal.foreground);
        // 아이콘 고르기도 고른 색으로 그린다.
        self.picker.update(cx, |picker, _| picker.tint = tint);

        let swatches = PROJECT_COLORS.iter().enumerate().map(|(ix, name)| {
            let color = project_color(name, cx.theme()).unwrap_or(pal.foreground);
            let selected = ix == self.color;
            div()
                .id(("project-color", ix))
                .size(px(SWATCH_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(color)
                .cursor_pointer()
                // 고른 색은 가운데에 배경색 점을 찍어 표시한다(Ulysses).
                .when(selected, |this| {
                    this.child(div().size(px(6.0)).rounded_full().bg(pal.background))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.color = ix;
                    cx.notify();
                }))
        });

        // 기본 아이콘들 + 더 보기(…). 목록에서 고른 아이콘은 더 보기 자리를 대신한다(Ulysses).
        let mut icon_buttons: Vec<AnyElement> = icons::PROJECT_ICONS
            .iter()
            .enumerate()
            .map(|(ix, &name)| {
                let selected = name == self.icon.as_ref();
                icon_choice(("project-icon", ix), pal, selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.icon = name.into();
                        cx.notify();
                    }))
                    .child(Icon::new(icons::by_name(name)).text_color(tint))
                    .into_any_element()
            })
            .collect();
        let custom = (!icons::PROJECT_ICONS.contains(&self.icon.as_ref()))
            .then(|| icons::by_name(&self.icon));
        let form = cx.entity().downgrade();
        let picker = self.picker.clone();
        let popover = Popover::new("project-icon-picker")
            .open(self.picker_open)
            .on_open_change(move |open, _, cx| {
                form.update(cx, |form, cx| {
                    form.picker_open = *open;
                    cx.notify();
                })
                .ok();
            })
            .trigger(
                Button::new("project-icon-more")
                    .icon(match custom {
                        Some(icon) => Icon::new(icon).text_color(tint),
                        None => Icon::new(icons::more()),
                    })
                    .ghost()
                    .rounded_full()
                    .size(px(ICON_BUTTON_SIZE))
                    // 다른 아이콘 버튼과 같은 둥근 테두리. 고른 아이콘이면 진하게.
                    .border_1()
                    .border_color(if custom.is_some() {
                        pal.foreground
                    } else {
                        pal.panel_border
                    })
                    .when(custom.is_none(), |this| this.bg(pal.panel)),
            )
            .content(move |_, _, _| picker.clone());
        icon_buttons.push(
            div()
                .relative()
                .child(popover)
                // 고른 아이콘 오른쪽 아래의 작은 … 배지: 다시 누르면 목록이 열린다.
                .when(custom.is_some(), |this| {
                    this.child(
                        div()
                            .id("project-icon-more-badge")
                            .absolute()
                            .right(px(-2.0))
                            .bottom(px(-2.0))
                            .size(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(pal.foreground)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picker_open = true;
                                cx.notify();
                            }))
                            .child(
                                Icon::new(icons::more())
                                    .size(px(10.0))
                                    .text_color(pal.background),
                            ),
                    )
                })
                .into_any_element(),
        );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(form_row(
                pal,
                t!("library.form.name"),
                Input::new(&self.name).focus_ring(false),
            ))
            .child(form_row(
                pal,
                t!("library.form.color"),
                div().flex().flex_wrap().gap_2().children(swatches),
            ))
            .child(form_row(
                pal,
                t!("library.form.icon"),
                div().flex().gap_2().children(icon_buttons),
            ))
    }
}

/// 둥근 아이콘 선택 버튼. 고른 것은 진한 테두리로 표시한다.
fn icon_choice(id: impl Into<ElementId>, pal: Palette, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(ICON_BUTTON_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if selected {
            pal.foreground
        } else {
            pal.panel_border
        })
        .when(!selected, |this| this.bg(pal.panel))
        .cursor_pointer()
}

/// 아이콘을 골랐다는 알림(메타 이름).
struct IconPicked(SharedString);

impl EventEmitter<IconPicked> for IconPicker {}

/// 검색할 수 있는 아이콘 격자(Lucide 전체). 긴 목록이라 보이는 줄만 그린다.
struct IconPicker {
    search: Entity<InputState>,
    /// 검색에 맞는 아이콘의 `icons::catalog()` 위치.
    matches: Vec<usize>,
    /// 아이콘 색(고른 프로젝트 색).
    tint: Hsla,
    _subscription: Subscription,
}

impl IconPicker {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("library.form.search")));
        let subscription = cx.subscribe(&search, |this, search, event, cx| {
            if let InputEvent::Change = event {
                let query = search.read(cx).value().trim().to_lowercase();
                this.matches = Self::filter(&query);
                cx.notify();
            }
        });
        Self {
            search,
            matches: Self::filter(""),
            tint: gpui_kit::gpui::black(),
            _subscription: subscription,
        }
    }

    /// 이름(예: `book-open`)에 검색어가 들어 있는 아이콘. 공백은 `-`로 본다.
    fn filter(query: &str) -> Vec<usize> {
        let query = query.replace(' ', "-");
        icons::catalog()
            .iter()
            .enumerate()
            .filter(|(_, (stem, _))| stem.contains(&query))
            .map(|(ix, _)| ix)
            .collect()
    }
}

impl Render for IconPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let rows = self.matches.len().div_ceil(PICKER_COLUMNS);
        let tint = self.tint;
        let grid = uniform_list(
            "project-icon-grid",
            rows,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let start = row * PICKER_COLUMNS;
                        let end = (start + PICKER_COLUMNS).min(this.matches.len());
                        div()
                            .flex()
                            .children(this.matches[start..end].iter().map(|&ix| {
                                let (stem, icon) = &icons::catalog()[ix];
                                let name = SharedString::from(stem.clone());
                                div()
                                    .id(("icon", ix))
                                    .size(px(PICKER_CELL))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(pal.hover))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(IconPicked(name.clone()));
                                    }))
                                    .child(Icon::new(*icon).text_color(tint))
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .h(px(PICKER_LIST_HEIGHT));

        div()
            .w(px(PICKER_CELL * PICKER_COLUMNS as f32))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Input::new(&self.search)
                    .focus_ring(false)
                    .prefix(Icon::new(icons::search_field()).text_color(pal.muted)),
            )
            .child(grid)
    }
}

/// 폼 한 줄: 오른쪽 정렬 라벨 + 컨트롤.
pub(super) fn form_row(
    pal: Palette,
    label: impl Into<SharedString>,
    control: impl IntoElement,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_shrink_0()
                .text_right()
                .text_sm()
                .text_color(pal.muted)
                .child(format!("{label}:")),
        )
        .child(div().flex_1().min_w_0().child(control))
}

impl HanjiApp {
    /// "새로운 프로젝트" 대화상자를 연다.
    pub(super) fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form_dialog(FormTarget::NewProject, window, cx);
    }

    /// 프로젝트 편집 대화상자를 연다(새 프로젝트와 같은 모양, 값이 채워진 채로).
    pub(super) fn open_edit_project(
        &mut self,
        dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_form_dialog(FormTarget::EditProject(dir.to_path_buf()), window, cx);
    }

    /// 메모 섹션(`Notes/`) 바로 아래에 새 메모 그룹을 만드는 대화상자를 연다.
    pub(super) fn open_new_note_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = &self.library else {
            return;
        };
        let parent = lib.root.join(library::SectionKind::Notes.dir());
        self.open_new_group(parent, true, window, cx);
    }

    /// 메모 섹션 폴더인지.
    fn is_notes_section(&self, dir: &Path) -> bool {
        self.library
            .as_ref()
            .is_some_and(|lib| lib.root.join(library::SectionKind::Notes.dir()) == dir)
    }

    /// 그룹 편집 대화상자를 연다(새 그룹과 같은 모양, 값이 채워진 채로).
    pub(super) fn open_edit_group(
        &mut self,
        dir: &Path,
        manuscript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = dir.to_path_buf();
        self.open_form_dialog(FormTarget::EditGroup { dir, manuscript }, window, cx);
    }

    /// 프로젝트 대화상자를 연다. 새 그룹도 같은 모양을 쓰고 제목만 다르다.
    pub(super) fn open_form_dialog(
        &mut self,
        target: FormTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.library.is_none() {
            return;
        }
        let group = match &target {
            FormTarget::EditProject(dir) | FormTarget::EditGroup { dir, .. } => {
                match self.find_group(dir) {
                    Some(group) => Some(group),
                    None => return,
                }
            }
            _ => None,
        };
        let title: SharedString = match &target {
            FormTarget::NewProject => t!("library.form.new_project"),
            FormTarget::EditProject(_) => t!("library.form.edit_project"),
            FormTarget::NewGroup {
                manuscript: false, ..
            } => t!("library.form.new_extra"),
            FormTarget::NewGroup { parent, .. } if self.open_project.as_ref() == Some(parent) => {
                t!("library.form.new_group")
            }
            FormTarget::NewGroup { parent, .. } if self.is_notes_section(parent) => {
                t!("library.form.new_note_group")
            }
            FormTarget::NewGroup { .. } => t!("library.form.new_subgroup"),
            FormTarget::EditGroup { .. } => t!("library.form.edit_group"),
        }
        .into();
        let app = cx.entity().downgrade();
        let form = cx.new(|cx| NewProjectForm::new(app, target, group.as_ref(), window, cx));
        // 처음 그릴 때만 이름 칸에 포커스를 주고 "무제"를 전부 선택해 바로 고쳐 쓰게 한다.
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let name = form.read(cx).name.clone();
                name.update(cx, |state, cx| {
                    state.focus(window, cx);
                    state.select_all(window, cx);
                });
            }
            let ok_form = form.clone();
            dialog
                .w(px(DIALOG_WIDTH))
                .title(title.clone())
                .close_button(false)
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("new-project-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-project-ok")
                                .primary()
                                .label(t!("library.form.done"))
                                .on_click(move |_, window, cx| {
                                    ok_form.update(cx, |form, cx| form.submit(window, cx));
                                }),
                        ),
                )
        });
    }

    /// 라이브러리에서 경로로 그룹을 찾는다.
    pub(super) fn find_group(&self, dir: &Path) -> Option<library::Group> {
        self.library.as_ref()?.find_group(dir).cloned()
    }

    /// 프로젝트(`group`이면 그룹)의 이름·색·아이콘을 고친다. 고쳤으면 `true`.
    ///
    /// 열린 시트가 이 폴더 안에 있으면 먼저 저장하고, 폴더 이름이 바뀌면 새 경로로 다시 연다.
    #[allow(clippy::too_many_arguments)]
    fn edit_folder(
        &mut self,
        dir: &Path,
        group: bool,
        name: &str,
        icon: &str,
        color: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return false;
        };
        let open_inside = self
            .selected_sheet
            .as_ref()
            .and_then(|path| path.strip_prefix(dir).ok())
            .map(Path::to_path_buf);
        if open_inside.is_some()
            && let Err(err) = self.save_open_sheet(cx)
        {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
            return false;
        }
        let updated = if group {
            library::update_group(dir, name, Some(icon), Some(color))
        } else {
            library::update_project(dir, name, Some(icon), Some(color))
        };
        let updated = updated.and_then(|new_dir| Ok((new_dir, library::scan(&root)?)));
        match updated {
            Ok((new_dir, scanned)) => {
                let renamed = new_dir != dir;
                if renamed && open_inside.is_some() {
                    // 저장은 위에서 했으므로 닫아도 잃는 것이 없다.
                    self.close_sheet(cx);
                }
                self.library = Some(scanned);
                // 프로젝트 안을 가리키던 화면 상태를 새 경로로 옮긴다.
                let rebase = |path: &Path| match path.strip_prefix(dir) {
                    Ok(relative) => new_dir.join(relative),
                    Err(_) => path.to_path_buf(),
                };
                if let Some(Selection::Group(path)) = &self.selected {
                    self.selected = Some(Selection::Group(rebase(path)));
                }
                self.group_anchor = self.group_anchor.as_deref().map(rebase);
                self.selected_groups = self
                    .selected_groups
                    .iter()
                    .map(|path| rebase(path))
                    .collect();
                self.open_project = self.open_project.as_deref().map(rebase);
                self.expanded_groups = self.expanded_groups.iter().map(|p| rebase(p)).collect();
                if renamed && let Some(relative) = open_inside {
                    self.select_sheet(Some(new_dir.join(relative)), false, window, cx);
                }
                self.load_previews(cx);
                cx.notify();
                true
            }
            Err(err) => {
                let message = if group {
                    t!("library.group.update_failed", err = err)
                } else {
                    t!("library.project.update_failed", err = err)
                };
                self.notify(message, window, cx);
                false
            }
        }
    }

    /// 확인을 받은 뒤 프로젝트를 운영체제 휴지통으로 옮긴다.
    pub(super) fn confirm_delete_project(
        &mut self,
        dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.find_group(dir) else {
            return;
        };
        let app = cx.entity().downgrade();
        let dir = dir.to_path_buf();
        let sheets: usize = group.descendants().iter().map(|g| g.sheets.len()).sum();
        let description = t!(
            "library.project.delete_desc",
            name = group.name,
            sheets = sheets
        );
        window.open_alert_dialog(cx, move |alert, _, _| {
            let app = app.clone();
            let dir = dir.clone();
            alert
                .confirm()
                .title(t!("library.project.delete_title"))
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("common.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    app.update(cx, |app, cx| app.delete_project(&dir, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// 프로젝트를 휴지통으로 옮긴다. 열린 시트가 그 안에 있으면 저장하고 닫는다.
    fn delete_project(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        if self
            .selected_sheet
            .as_ref()
            .is_some_and(|path| path.starts_with(dir))
        {
            self.close_sheet(cx);
        }
        match library::move_to_trash(dir).and_then(|()| library::scan(&root)) {
            Ok(scanned) => {
                self.library = Some(scanned);
                if matches!(&self.selected, Some(Selection::Group(path)) if path.starts_with(dir)) {
                    self.selected = None;
                }
                self.selected_groups.retain(|path| !path.starts_with(dir));
                if self
                    .group_anchor
                    .as_ref()
                    .is_some_and(|path| path.starts_with(dir))
                {
                    self.group_anchor = None;
                }
                if self.open_project.as_deref() == Some(dir) {
                    self.open_project = None;
                }
                self.load_previews(cx);
                cx.notify();
            }
            Err(err) => self.notify(t!("library.project.delete_failed", err = err), window, cx),
        }
    }

    /// `Projects/` 아래에 프로젝트를 만들고 프로젝트 화면으로 연다. 만들었으면 `true`.
    fn create_project(
        &mut self,
        name: &str,
        icon: &str,
        color: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(lib) = &self.library else {
            return false;
        };
        let root = lib.root.clone();
        let projects = root.join(library::SectionKind::Projects.dir());
        let created = library::create_project(&projects, name, Some(icon), Some(color))
            .and_then(|dir| Ok((dir, library::scan(&root)?)));
        match created {
            Ok((dir, scanned)) => {
                self.library = Some(scanned);
                self.enter_project(&dir, window, cx);
                true
            }
            Err(err) => {
                self.notify(t!("library.project.create_failed", err = err), window, cx);
                false
            }
        }
    }
}
