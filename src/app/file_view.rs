//! 파일 그룹(첨부파일 등): 사이드바 2단의 파일 목록과, 에디터 자리의 간단한 뷰어.

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::gpui::prelude::{
    FluentBuilder, InteractiveElement as _, StatefulInteractiveElement as _,
};
use gpui_kit::gpui::*;
use std::path::{Path, PathBuf};

use super::HanjiApp;
use super::editor::EDITOR_MAX_WIDTH;
use super::library_panel::menu_row;
use super::sheet_list::DraggedSheet;
use super::style::*;
use crate::{icons, library};

/// 파일 목록 한 줄 높이.
const FILE_ROW_HEIGHT: f32 = 56.0;

/// 목록의 썸네일 크기.
const THUMBNAIL_SIZE: f32 = 36.0;

/// 파일 정보 팝오버 폭.
const FILE_INFO_WIDTH: f32 = 260.0;

/// 텍스트 뷰어가 읽는 최대 바이트. 넘으면 앞부분만 보여 준다.
const TEXT_PREVIEW_LIMIT: usize = 256 * 1024;

/// 뷰어에 연 파일. 텍스트 파일은 열 때 본문을 읽어 둔다.
pub(super) struct OpenFile {
    pub(super) path: PathBuf,
    /// 텍스트 본문(텍스트 파일만). 너무 길면 잘라 둔다.
    text: Option<SharedString>,
    truncated: bool,
    /// 이미지의 원래 크기(px, 이미지 파일만). 폭에 맞춰 줄일 때 높이를 비율로 정한다.
    image_size: Option<(f32, f32)>,
}

/// 파일 종류 설명(예: `PNG 이미지`).
fn kind_label(file: &library::FileItem) -> String {
    let extension = file
        .path
        .extension()
        .map(|ext| ext.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    match file.kind {
        library::FileKind::Image => t!("library.file.kind_image", ext = extension).to_string(),
        library::FileKind::Video => t!("library.file.kind_video", ext = extension).to_string(),
        library::FileKind::Audio => t!("library.file.kind_audio", ext = extension).to_string(),
        library::FileKind::Text => t!("library.file.kind_text").to_string(),
        library::FileKind::Pdf => t!("library.file.kind_pdf").to_string(),
        library::FileKind::Other if extension.is_empty() => {
            t!("library.file.kind_file").to_string()
        }
        library::FileKind::Other => t!("library.file.kind_ext_file", ext = extension).to_string(),
    }
}

/// 필터 메뉴의 파일 종류 이름.
fn kind_filter_title(kind: library::FileKind) -> SharedString {
    match kind {
        library::FileKind::Image => t!("library.file.filter_image"),
        library::FileKind::Video => t!("library.file.filter_video"),
        library::FileKind::Audio => t!("library.file.filter_audio"),
        library::FileKind::Text => t!("library.file.filter_text"),
        library::FileKind::Pdf | library::FileKind::Other => t!("library.file.filter_other"),
    }
    .into()
}

/// 표지 파일(`cover.<확장자>`)인지.
fn is_cover(file: &library::FileItem) -> bool {
    file.path
        .file_stem()
        .is_some_and(|stem| stem == library::COVER_STEM)
}

impl HanjiApp {
    /// 파일을 뷰어로 연다. 열려 있던 시트는 저장하고 닫는다.
    pub(super) fn select_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_sheet(None, false, window, cx);
        if self.selected_sheet.is_some() {
            // 시트를 저장하지 못해 그대로 열려 있다.
            return;
        }
        let (text, truncated) = match library::FileKind::of(&path) {
            library::FileKind::Text => match std::fs::read(&path) {
                Ok(mut bytes) => {
                    let truncated = bytes.len() > TEXT_PREVIEW_LIMIT;
                    bytes.truncate(TEXT_PREVIEW_LIMIT);
                    (
                        Some(String::from_utf8_lossy(&bytes).into_owned().into()),
                        truncated,
                    )
                }
                Err(err) => {
                    self.notify(t!("library.file.open_failed", err = err), window, cx);
                    (None, false)
                }
            },
            _ => (None, false),
        };
        // 헤더만 읽는다. 읽지 못하면(손상·모르는 형식) 비율을 모르는 채로 그린다.
        let image_size = (library::FileKind::of(&path) == library::FileKind::Image)
            .then(|| imagesize::size(&path).ok())
            .flatten()
            .filter(|size| size.width > 0 && size.height > 0)
            .map(|size| (size.width as f32, size.height as f32));
        self.book_style = None;
        self.open_file = Some(OpenFile {
            path,
            text,
            truncated,
            image_size,
        });
        cx.notify();
    }

    /// 확인을 받은 뒤 파일 그룹의 파일을 운영체제 휴지통으로 옮긴다.
    fn confirm_trash_file(
        &mut self,
        path: PathBuf,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (app, path) = (app.clone(), path.clone());
            alert
                .confirm()
                .title(t!("library.file.delete_title"))
                .description(t!("library.file.delete_one", label = label))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("common.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    app.update(cx, |app, cx| app.trash_file(&path, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// 파일을 휴지통으로 옮기고 라이브러리를 다시 읽는다. 열어 둔 파일이면 닫는다.
    /// 그룹 메타의 파일 순서(`files`)에 남은 이름은 읽을 때 건너뛴다.
    fn trash_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return;
        };
        if let Err(err) = library::move_to_trash(path) {
            self.notify(t!("library.file.delete_failed", err = err), window, cx);
            return;
        }
        if self
            .open_file
            .as_ref()
            .is_some_and(|file| file.path == path)
        {
            self.open_file = None;
        }
        match library::scan(&root) {
            Ok(scanned) => self.library = Some(scanned),
            Err(err) => self.notify(t!("library.file.delete_failed", err = err), window, cx),
        }
        cx.notify();
    }

    /// 경로로 파일 그룹의 파일을 찾는다.
    fn find_file(&self, path: &Path) -> Option<library::FileItem> {
        let group = self.library.as_ref()?.find_group(path.parent()?)?;
        group.files.iter().find(|file| file.path == path).cloned()
    }

    /// 사이드바 2단의 파일 목록. `manual_group`이 있으면(수동 정렬) 끌어서 순서를 바꾼다.
    pub(super) fn render_file_list(
        &self,
        files: Vec<library::FileItem>,
        manual_group: Option<PathBuf>,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> AnyElement {
        if files.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(pal.muted)
                .child(t!("library.file.none"))
                .into_any_element();
        }
        let selected = self.open_file.as_ref().map(|file| file.path.clone());
        let rows = files.into_iter().enumerate().map(|(index, file)| {
            let is_selected = selected.as_ref() == Some(&file.path);
            let thumbnail = div()
                .size(px(THUMBNAIL_SIZE))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .rounded_sm()
                .bg(pal.panel)
                .map(|this| match file.kind {
                    // 동영상도 QuickLook 썸네일(첫 장면)이 있다(macOS).
                    library::FileKind::Image | library::FileKind::Video => this.child(
                        img(crate::thumbnail::source(
                            file.path.clone(),
                            file.modified,
                            px(THUMBNAIL_SIZE),
                        ))
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .with_fallback(move || {
                            Icon::new(icons::file_kind(file.kind))
                                .text_color(pal.muted)
                                .into_any_element()
                        }),
                    ),
                    kind => this.child(Icon::new(icons::file_kind(kind)).text_color(pal.muted)),
                });
            let detail = format!(
                "{} · {}",
                kind_label(&file),
                library::format_size(file.size)
            );
            let text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .text_color(pal.foreground)
                                .child(file.name()),
                        )
                        .when(is_cover(&file), |this| {
                            this.child(
                                div()
                                    .flex_shrink_0()
                                    .px_1()
                                    .rounded_sm()
                                    .bg(pal.tint)
                                    .text_xs()
                                    .text_color(pal.tint_foreground)
                                    .child(t!("library.file.cover")),
                            )
                        }),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(pal.muted)
                        .child(detail),
                );
            let path = file.path.clone();
            let label = SharedString::from(file.name());
            let app = cx.entity().downgrade();
            let (trash_path, trash_label) = (file.path.clone(), label.clone());
            let row = div()
                .id(SharedString::from(format!("file-{}", file.path.display())))
                .h(px(FILE_ROW_HEIGHT))
                .px_2()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_file(path.clone(), window, cx)
                }))
                .child(
                    div()
                        .h_full()
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_md()
                        .when(is_selected, |this| this.bg(pal.active))
                        .child(thumbnail)
                        .child(text),
                );
            // 끌어다 놓을 자리는 줄의 위/아래 선으로 보여 준다(시트 목록과 같다).
            let row = match &manual_group {
                Some(group_path) => {
                    let group_path = group_path.clone();
                    let dragged = DraggedSheet {
                        group: group_path.clone(),
                        files: true,
                        anchor: index,
                        indices: vec![index],
                        paths: Vec::new(),
                        labels: vec![label],
                        pal,
                    };
                    row.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                        .drag_over::<DraggedSheet>(move |style, dragged, _, _| {
                            if dragged.anchor < index {
                                style.border_b_2().border_color(pal.accent)
                            } else if dragged.anchor > index {
                                style.border_t_2().border_color(pal.accent)
                            } else {
                                style
                            }
                        })
                        .on_drop(
                            cx.listener(move |this, dragged: &DraggedSheet, window, cx| {
                                if !dragged.files {
                                    return;
                                }
                                this.move_file(&group_path, dragged.anchor, index, window, cx);
                            }),
                        )
                }
                None => row,
            };
            row.context_menu(move |menu, _, cx| {
                let danger = cx.theme().danger;
                let (app, path, label) = (app.clone(), trash_path.clone(), trash_label.clone());
                menu.min_w(px(200.0)).item(
                    PopupMenuItem::element(move |_, _| {
                        menu_row(icons::trash(), t!("common.move_to_trash"), Some(danger))
                    })
                    .on_click(move |_, window, cx| {
                        app.update(cx, |app, cx| {
                            app.confirm_trash_file(path.clone(), label.clone(), window, cx)
                        })
                        .ok();
                    }),
                )
            })
        });
        div()
            .id("file-list")
            .flex_1()
            .overflow_y_scroll()
            .pt_1()
            .children(rows)
            .into_any_element()
    }

    /// 파일 목록 툴바의 필터 버튼. 누르면 보여 줄 파일 종류를 고른다. 거르는 중이면 강조색.
    pub(super) fn file_filter_button(
        &self,
        cx: &mut Context<Self>,
        pal: Palette,
    ) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let current = self.file_filter;
        let pick = move |kind: Option<library::FileKind>| {
            let app = app.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                app.update(cx, |app, cx| {
                    app.file_filter = kind;
                    cx.notify();
                })
                .ok();
            }
        };
        let color = if current.is_some() {
            pal.tint_foreground
        } else {
            pal.foreground.opacity(0.8)
        };
        Button::new("file-filter")
            .icon(Icon::new(icons::filter()).text_color(color))
            .ghost()
            .size(px(30.0))
            .rounded(px(15.0))
            .tooltip(
                current.map_or_else(|| t!("library.file.filter_all").into(), kind_filter_title),
            )
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let menu = menu.min_w(px(160.0)).item(
                    PopupMenuItem::new(t!("library.file.filter_all"))
                        .checked(current.is_none())
                        .on_click(pick(None)),
                );
                library::FileKind::FILTERS
                    .into_iter()
                    .fold(menu.separator(), |menu, kind| {
                        menu.item(
                            PopupMenuItem::new(kind_filter_title(kind))
                                .checked(current == Some(kind))
                                .on_click(pick(Some(kind))),
                        )
                    })
            })
    }

    /// 뷰어 툴바 버튼: 기본 앱으로 열기, Finder에서 보기, 파일 정보(누르면 팝오버).
    pub(super) fn file_actions(&self, open: &OpenFile, pal: Palette) -> impl IntoElement {
        let file = self.find_file(&open.path);
        let (open_path, reveal_path) = (open.path.clone(), open.path.clone());
        let mut details: Vec<(SharedString, String)> = Vec::new();
        if let Some(file) = &file {
            details.push((t!("library.file.info_name").into(), file.name()));
            details.push((t!("library.file.info_kind").into(), kind_label(file)));
            details.push((
                t!("library.file.info_size").into(),
                library::format_size(file.size),
            ));
            if let Some((width, height)) = open.image_size {
                details.push((
                    t!("library.file.info_image").into(),
                    format!("{width} × {height} px"),
                ));
            }
            let location = self
                .library
                .as_ref()
                .and_then(|lib| file.path.strip_prefix(&lib.root).ok())
                .unwrap_or(&file.path);
            details.push((
                t!("library.file.info_location").into(),
                location.display().to_string(),
            ));
        }
        let info = Popover::new("file-info")
            .anchor(Anchor::TopRight)
            .trigger(
                Button::new("file-info-button")
                    .icon(Icon::new(icons::info()).text_color(pal.foreground.opacity(0.8)))
                    .ghost()
                    .size(px(30.0))
                    .rounded(px(15.0))
                    .disabled(file.is_none()),
            )
            .content(move |_, _, _| {
                div()
                    .w(px(FILE_INFO_WIDTH))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .children(details.iter().map(|(label, value)| {
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(40.0))
                                    .flex_shrink_0()
                                    .text_color(pal.muted)
                                    .child(label.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(pal.foreground)
                                    .child(value.clone()),
                            )
                    }))
            });
        tool_group(pal)
            .child(
                icon_button("file-open-external", icons::open_external(), true, pal)
                    .tooltip(|window, cx| {
                        Tooltip::new(t!("library.file.open_default_app").to_string())
                            .build(window, cx)
                    })
                    .on_click(move |_, _, cx| cx.open_with_system(&open_path)),
            )
            .child(
                icon_button("file-reveal", icons::reveal(), true, pal)
                    .tooltip(|window, cx| Tooltip::new(reveal_label()).build(window, cx))
                    .on_click(move |_, _, cx| cx.reveal_path(&reveal_path)),
            )
            .child(info)
    }

    /// 에디터 자리의 파일 뷰어: 이미지·텍스트는 내용을, 그 밖의 파일은 종류 아이콘을 보여 준다.
    /// 열기·보기·정보 버튼은 툴바에 둔다([`Self::file_actions`]).
    pub(super) fn render_file_viewer(&self, open: &OpenFile, pal: Palette) -> AnyElement {
        let Some(file) = self.find_file(&open.path) else {
            // 바깥에서 지워졌다.
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_2xl()
                .text_color(pal.muted.opacity(0.7))
                .child(t!("library.file.not_found"))
                .into_any_element();
        };

        match (&file.kind, &open.text) {
            // 폭에 맞춰 줄이고(원래보다 키우지는 않는다) 세로로 넘치면 스크롤한다.
            // 크기를 모르면 4:3 칸 안에 맞춰 그린다.
            (library::FileKind::Image, _) => {
                let (width, height) = open.image_size.unwrap_or((4.0, 3.0));
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div().size_full().overflow_y_scrollbar().child(
                            div().w_full().p_6().flex().justify_center().child(
                                div()
                                    .w_full()
                                    .when(open.image_size.is_some(), |this| this.max_w(px(width)))
                                    .aspect_ratio(width / height)
                                    .child(
                                        img(file.path.clone())
                                            .size_full()
                                            .object_fit(ObjectFit::Contain)
                                            .with_fallback(move || {
                                                div()
                                                    .size_full()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_color(pal.muted)
                                                    .child(t!("library.file.image_unavailable"))
                                                    .into_any_element()
                                            }),
                                    ),
                            ),
                        ),
                    )
                    .into_any_element()
            }
            (library::FileKind::Text, Some(text)) => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .px_8()
                .py_6()
                .child(
                    div()
                        .max_w(px(EDITOR_MAX_WIDTH))
                        .mx_auto()
                        .text_sm()
                        .text_color(pal.foreground)
                        .whitespace_normal()
                        .child(text.clone()),
                )
                .when(open.truncated, |this| {
                    this.child(
                        div()
                            .pt_4()
                            .text_center()
                            .text_xs()
                            .text_color(pal.muted)
                            .child(t!("library.file.truncated")),
                    )
                })
                .overflow_y_scrollbar()
                .into_any_element(),
            (kind, _) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(icons::file_kind(*kind))
                        .size(px(64.0))
                        .text_color(pal.muted.opacity(0.7)),
                )
                .into_any_element(),
        }
    }
}

/// 파일 관리자에서 보기 버튼 이름.
fn reveal_label() -> SharedString {
    #[cfg(target_os = "macos")]
    let label = t!("common.reveal_in_finder");
    #[cfg(not(target_os = "macos"))]
    let label = t!("library.file.reveal_folder");
    label.into()
}
