//! 사이드바 1단의 프로젝트 화면: 원고 그룹 트리 + 추가자료. 그룹 만들기·삭제도 여기 둔다.
//!
//! 원고 그룹(`<Project>/<Group>/…`)은 빌드(PDF/ePub) 대상이고, 추가자료(`<Project>/Extras/…`)는
//! 빌드에서 빠진다.

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::{
    FluentBuilder, InteractiveElement as _, StatefulInteractiveElement as _,
};
use gpui_kit::gpui::*;
use std::path::{Path, PathBuf};

use super::library_panel::{Disclosure, menu_row};
use super::new_project::FormTarget;
use super::sheet_list::DraggedSheet;
use super::style::*;
use super::{HanjiApp, Selection};
use crate::{icons, library};

/// 프로젝트를 열 때의 기본 선택: 첫 원고 그룹 → 첫 추가자료 → 프로젝트 자신(바로 아래 시트).
pub(super) fn default_selection(project: &library::Group) -> Selection {
    let first = project.groups.first().or(project.extras.first());
    Selection::Group(first.unwrap_or(project).path.clone())
}

impl HanjiApp {
    /// 사이드바를 프로젝트 화면으로 바꾼다.
    pub(super) fn enter_project(
        &mut self,
        dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.library.as_ref().and_then(|lib| lib.find_project(dir)) else {
            return;
        };
        self.selected = Some(default_selection(project));
        self.selected_groups.clear();
        self.group_anchor = match &self.selected {
            Some(Selection::Group(path)) => Some(path.clone()),
            _ => None,
        };
        self.open_project = Some(dir.to_path_buf());
        self.select_sheet(None, false, window, cx);
        self.load_previews(cx);
        cx.notify();
    }

    /// 프로젝트 화면을 닫고 라이브러리 화면으로 돌아간다. 돌아가면 그 프로젝트를 선택해 둔다.
    pub(super) fn leave_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.open_project.take() else {
            return;
        };
        self.selected = Some(Selection::Group(dir));
        self.selected_groups.clear();
        self.group_anchor = match &self.selected {
            Some(Selection::Group(path)) => Some(path.clone()),
            _ => None,
        };
        self.select_sheet(None, false, window, cx);
        self.load_previews(cx);
        cx.notify();
    }

    /// 하위 그룹을 펼치거나 접는다.
    pub(super) fn toggle_group(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.expanded_groups.remove(dir) {
            self.expanded_groups.insert(dir.to_path_buf());
        }
        cx.notify();
    }

    /// 프로젝트 화면 위쪽: 이름, 도구 줄, 구분선. 이름을 누르면 프로젝트 바로 아래 시트를 본다.
    pub(super) fn project_header(
        &self,
        project: &library::Group,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let dir = project.path.clone();
        let preview_dir = project.path.clone();
        let compile_dir = project.path.clone();
        let drop_dir = project.path.clone();
        let drag_dir = project.path.clone();
        div()
            .flex_shrink_0()
            .px_2()
            .child(
                div()
                    .id("project-title")
                    .px_3()
                    .pt_2()
                    .text_xl()
                    .font_bold()
                    .truncate()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = Some(Selection::Group(dir.clone()));
                        this.selected_groups.clear();
                        this.group_anchor = Some(dir.clone());
                        this.select_sheet(None, false, window, cx);
                        this.load_previews(cx);
                    }))
                    // 프로젝트 바로 아래(그룹 루트)로 시트를 옮길 수 있다(ADR-0019).
                    .drag_over::<DraggedSheet>(move |style, dragged, _, _| {
                        if !dragged.files && dragged.group != drag_dir {
                            style.bg(pal.active)
                        } else {
                            style
                        }
                    })
                    .on_drop(
                        cx.listener(move |this, dragged: &DraggedSheet, window, cx| {
                            if dragged.files || dragged.group == drop_dir {
                                return;
                            }
                            this.move_sheets_to_group(
                                &dragged.group,
                                &drop_dir,
                                &dragged.paths,
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(project.name.clone()),
            )
            .child(
                div()
                    .px_1()
                    .pt_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        icon_button("project-build", icons::build(), true, pal).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.open_compile_dialog(&compile_dir, window, cx);
                            }),
                        ),
                    )
                    .child(
                        icon_button("project-preview", icons::book_preview(), true, pal).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.open_book_preview(&preview_dir, window, cx)
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(self.project_more_button(project.path.clone(), cx, pal)),
            )
            .child(
                div()
                    .mx_2()
                    .mt_1()
                    .mb_2()
                    .border_b_1()
                    .border_color(pal.panel_border),
            )
    }

    /// 프로젝트 미리보기 창을 연다. 열린 시트를 먼저 저장해 지금 내용이 보이게 한다.
    fn open_book_preview(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.save_open_sheet(cx) {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
        }
        super::book_preview::open(dir, cx);
    }

    /// 도구 줄의 "더 보기" 버튼. 누르면 프로젝트 편집·책 정보·책 스타일을 고른다.
    fn project_more_button(
        &self,
        dir: PathBuf,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let app = cx.entity().downgrade();
        Button::new("project-more")
            .icon(Icon::new(icons::more()).text_color(pal.foreground.opacity(0.8)))
            .ghost()
            .size(px(30.0))
            .rounded(px(15.0))
            .dropdown_menu(move |menu, _, _| {
                let (edit_app, edit_dir) = (app.clone(), dir.clone());
                let (book_app, book_dir) = (app.clone(), dir.clone());
                let (style_app, style_dir) = (app.clone(), dir.clone());
                let (export_app, export_dir) = (app.clone(), dir.clone());
                menu.min_w(px(200.0))
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            menu_row(icons::edit(), t!("library.project.edit_menu"), None)
                        })
                        .on_click(move |_, window, cx| {
                            edit_app
                                .update(cx, |app, cx| app.open_edit_project(&edit_dir, window, cx))
                                .ok();
                        }),
                    )
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            menu_row(
                                icons::book_info(),
                                t!("library.project.book_info_menu"),
                                None,
                            )
                        })
                        .on_click(move |_, window, cx| {
                            book_app
                                .update(cx, |app, cx| app.open_book_info(&book_dir, window, cx))
                                .ok();
                        }),
                    )
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            menu_row(
                                icons::book_style(),
                                t!("library.project.book_style_menu"),
                                None,
                            )
                        })
                        .on_click(move |_, window, cx| {
                            style_app
                                .update(cx, |app, cx| app.open_book_style(&style_dir, window, cx))
                                .ok();
                        }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            menu_row(icons::export(), t!("library.project.export_menu"), None)
                        })
                        .on_click(move |_, window, cx| {
                            export_app
                                .update(cx, |app, cx| app.export_project(&export_dir, window, cx))
                                .ok();
                        }),
                    )
            })
    }

    /// 프로젝트 폴더 전체를 ZIP으로 묶어 `.hanji` 파일 하나로 내보낸다.
    fn export_project(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        // 지금 에디터에 보이는 내용이 파일에 있어야 한다.
        if let Err(err) = self.save_open_sheet(cx) {
            self.notify(t!("library.sheet.save_failed", err = err), window, cx);
            return;
        }

        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| t!("library.project.default_name").to_string());
        let default_name = format!("{name}.hanji");
        let default_dir = dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dir.to_path_buf());
        let project_dir = dir.to_path_buf();

        let receiver = cx.prompt_for_new_path(&default_dir, Some(&default_name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(output_path))) = receiver.await else {
                return;
            };
            // 저장 대화상자에서 확장자를 지우거나 바꿔도 `.hanji`로 맞춘다.
            let output_path = crate::export::project::with_hanji_extension(&output_path);
            let display_name = output_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| default_name.clone());
            let target_path = output_path.clone();
            let result =
                cx.background_executor()
                    .spawn(async move {
                        crate::export::project::export_to_file(&project_dir, &target_path)
                    })
                    .await;

            this.update(cx, |app, cx| match result {
                Ok(()) => {
                    app.set_notice(t!("library.project.export_done", name = display_name), cx)
                }
                Err(err) => app.set_notice(t!("library.project.export_failed", err = err), cx),
            })
            .ok();
        })
        .detach();
    }

    /// 프로젝트 화면의 줄들: 원고 그룹 트리(없으면 "새로운 그룹…"), 추가자료.
    pub(super) fn project_rows(
        &self,
        project: &library::Group,
        current: Option<&Selection>,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
        pal: Palette,
    ) {
        if project.groups.is_empty() {
            let dir = project.path.clone();
            rows.push(
                div()
                    .id("new-manuscript-group")
                    .mx_3()
                    .mt_1()
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(pal.tint)
                    .text_xs()
                    .text_color(pal.tint_foreground)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_new_group(dir.clone(), true, window, cx);
                    }))
                    .child(t!("library.project.new_group"))
                    .into_any_element(),
            );
        }
        for group in &project.groups {
            self.push_group_tree(group, 0, true, current, rows, cx, pal);
        }

        // 추가자료 머리글. 마우스를 올리면 "추가"가 보인다.
        let extras_dir = project.path.join(library::EXTRAS_DIR);
        rows.push(
            div()
                .group("extras-header")
                .mt_4()
                .mb_1()
                .px_4()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .font_semibold()
                .text_color(pal.muted)
                .child(t!("library.project.extras"))
                .child(
                    div()
                        .id("new-extras-group")
                        .cursor_pointer()
                        .font_normal()
                        .invisible()
                        .group_hover("extras-header", |style| style.visible())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_new_group(extras_dir.clone(), false, window, cx);
                        }))
                        .child(t!("library.project.add")),
                )
                .into_any_element(),
        );
        for group in &project.extras {
            self.push_group_tree(group, 0, false, current, rows, cx, pal);
        }
    }

    /// 그룹 한 줄과, 펼쳐져 있으면 그 하위 그룹들을 넣는다. 우클릭으로 하위 그룹 추가·삭제.
    #[allow(clippy::too_many_arguments)]
    fn push_group_tree(
        &self,
        group: &library::Group,
        depth: usize,
        manuscript: bool,
        current: Option<&Selection>,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
        pal: Palette,
    ) {
        let expanded = self.expanded_groups.contains(&group.path);
        let disclosure = if group.groups.is_empty() {
            Disclosure::Leaf
        } else {
            Disclosure::Branch { expanded }
        };
        let default_icon = if manuscript {
            icons::MANUSCRIPT_GROUP_ICON
        } else {
            icons::EXTRAS_GROUP_ICON
        };
        let selection = Selection::Group(group.path.clone());
        let row = self.library_row(
            cx,
            pal,
            format!("group-{}", group.path.display()).into(),
            icons::group_icon(Some(group.icon.as_deref().unwrap_or(default_icon))),
            group.name.clone().into(),
            group
                .display_color()
                .and_then(|color| project_color(color, cx.theme())),
            self.group_is_selected(&group.path, current),
            selection,
            depth,
            disclosure,
        );

        let app = cx.entity().downgrade();
        let dir = group.path.clone();
        let drop_dir = group.path.clone();
        let drag_dir = group.path.clone();
        let accepts = group.kind == library::GroupKind::Sheets;
        let row = row
            .drag_over::<DraggedSheet>(move |style, dragged, _, _| {
                if accepts && !dragged.files && dragged.group != drag_dir {
                    style.bg(pal.active)
                } else {
                    style
                }
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedSheet, window, cx| {
                    if dragged.files || dragged.group == drop_dir {
                        return;
                    }
                    this.move_sheets_to_group(
                        &dragged.group,
                        &drop_dir,
                        &dragged.paths,
                        window,
                        cx,
                    );
                }),
            );
        // 같은 범위의 다른 그룹 위로 끌어다 놓으면 그 그룹의 하위로 옮긴다.
        let row = self.group_drag_handlers(row, &group.path, pal, cx);
        rows.push(
            row.context_menu(move |menu, _, cx| {
                let (edit_app, edit_dir) = (app.clone(), dir.clone());
                let (child_app, child_dir) = (app.clone(), dir.clone());
                let (delete_app, delete_dir) = (app.clone(), dir.clone());
                let danger = cx.theme().danger;
                menu.min_w(px(200.0))
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            menu_row(icons::edit(), t!("library.menu.edit"), None)
                        })
                        .on_click(move |_, window, cx| {
                            edit_app
                                .update(cx, |app, cx| {
                                    app.open_edit_group(&edit_dir, manuscript, window, cx)
                                })
                                .ok();
                        }),
                    )
                    .when(manuscript, |menu| {
                        menu.item(
                            PopupMenuItem::element(move |_, _| {
                                menu_row(icons::new_group(), t!("library.menu.new_subgroup"), None)
                            })
                            .on_click(move |_, window, cx| {
                                child_app
                                    .update(cx, |app, cx| {
                                        app.open_new_group(child_dir.clone(), true, window, cx)
                                    })
                                    .ok();
                            }),
                        )
                    })
                    .separator()
                    .item(
                        PopupMenuItem::element(move |_, _| {
                            menu_row(icons::trash(), t!("common.move_to_trash"), Some(danger))
                        })
                        .on_click(move |_, window, cx| {
                            delete_app
                                .update(cx, |app, cx| {
                                    app.confirm_delete_group(&delete_dir, window, cx)
                                })
                                .ok();
                        }),
                    )
            })
            .into_any_element(),
        );

        if expanded {
            for child in &group.groups {
                self.push_group_tree(child, depth + 1, manuscript, current, rows, cx, pal);
            }
        }
    }

    /// 새 그룹 대화상자(프로젝트 대화상자와 같은 모양)를 연다.
    /// `parent` 아래에 만든다(`manuscript`면 원고, 아니면 추가자료).
    pub(super) fn open_new_group(
        &mut self,
        parent: PathBuf,
        manuscript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_form_dialog(FormTarget::NewGroup { parent, manuscript }, window, cx);
    }

    /// `parent` 아래에 그룹을 만들고 선택한다. 만들었으면 `true`.
    pub(super) fn create_group(
        &mut self,
        parent: &Path,
        name: &str,
        icon: &str,
        color: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return false;
        };
        let created = library::create_group(parent, name, Some(icon), Some(color))
            .and_then(|dir| Ok((dir, library::scan(&root)?)));
        match created {
            Ok((dir, scanned)) => {
                self.library = Some(scanned);
                // 하위 그룹이면 부모를 펼쳐 새 그룹이 보이게 한다.
                if self.find_group(parent).is_some() {
                    self.expanded_groups.insert(parent.to_path_buf());
                }
                self.selected = Some(Selection::Group(dir));
                self.selected_groups.clear();
                self.group_anchor = match &self.selected {
                    Some(Selection::Group(path)) => Some(path.clone()),
                    _ => None,
                };
                self.select_sheet(None, false, window, cx);
                self.load_previews(cx);
                cx.notify();
                true
            }
            Err(err) => {
                self.notify(t!("library.group.create_failed", err = err), window, cx);
                false
            }
        }
    }

    /// 확인을 받은 뒤 그룹(하위 그룹 포함)을 운영체제 휴지통으로 옮긴다.
    pub(super) fn confirm_delete_group(
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
            "library.group.delete_desc",
            name = group.name,
            sheets = sheets
        );
        window.open_alert_dialog(cx, move |alert, _, _| {
            let app = app.clone();
            let dir = dir.clone();
            alert
                .confirm()
                .title(t!("library.group.delete_title"))
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("common.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    app.update(cx, |app, cx| app.delete_group(&dir, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// 그룹을 휴지통으로 옮긴다. 열린 시트가 그 안에 있으면 저장하고 닫는다.
    fn delete_group(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
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
                self.selected_groups.retain(|path| path.is_dir());
                if self
                    .group_anchor
                    .as_ref()
                    .is_some_and(|path| !path.is_dir())
                {
                    self.group_anchor = None;
                }
                self.load_previews(cx);
                cx.notify();
            }
            Err(err) => self.notify(t!("library.group.delete_failed", err = err), window, cx),
        }
    }
}
