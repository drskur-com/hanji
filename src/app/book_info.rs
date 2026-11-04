//! 책 정보 대화상자: 제목·지은이·언어·소개·표지. 내보내기(ePub·DOCX 등)가 이 값을 쓴다.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, FocusableExt as _, Icon, WindowExt as _};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};

use super::HanjiApp;
use super::new_project::form_row;
use super::style::*;
use crate::{icons, library};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 640.0;

/// 표지 미리보기 크기(책 표지 비율 2:3).
const COVER_WIDTH: f32 = 140.0;
const COVER_HEIGHT: f32 = 210.0;

/// 언어 목록: (BCP 47 태그, 표시 이름). 첫 항목이 기본.
const LANGUAGES: [(&str, &str); 7] = [
    ("ko", "한국어"),
    ("en", "English"),
    ("ja", "日本語"),
    ("zh", "中文"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("es", "Español"),
];

/// 언어 태그의 표시 이름. 목록에 없으면 태그 그대로.
fn language_title(tag: &str) -> SharedString {
    LANGUAGES
        .iter()
        .find(|(code, _)| *code == tag)
        .map_or_else(|| tag.to_string().into(), |(_, title)| (*title).into())
}

/// 대화상자 안의 입력 상태.
struct BookInfoForm {
    project: PathBuf,
    /// 읽어 온 책 정보. 식별자처럼 화면에 없는 값을 지키려고 들고 있는다.
    book: library::BookMeta,
    title: Entity<InputState>,
    authors: Entity<InputState>,
    description: Entity<TextareaState>,
    language: SharedString,
    /// 표지 이미지의 절대 경로. 저장할 때 첨부파일 그룹의 `cover.<확장자>`로 넣는다.
    cover: Option<PathBuf>,
    /// 표지 파일의 수정 시각(썸네일 캐시 키).
    cover_modified: u128,
    /// 표지를 고르다 생긴 문제(지원하지 않는 형식 등).
    cover_error: Option<SharedString>,
    app: WeakEntity<HanjiApp>,
}

impl BookInfoForm {
    fn new(
        app: WeakEntity<HanjiApp>,
        project: &library::Group,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let book = library::read_book(&project.path);
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(project.name.clone())
                .default_value(book.title.clone())
        });
        let authors = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("book.info.authors_placeholder"))
                .default_value(book.authors.join(", "))
        });
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .default_value(book.description.clone())
        });
        let language = if book.language.is_empty() {
            LANGUAGES[0].0.into()
        } else {
            book.language.clone().into()
        };
        let cover = book.cover_path(&project.path);
        Self {
            cover_modified: cover.as_deref().map_or(0, library::modified_nanos),
            cover,
            project: project.path.clone(),
            book,
            title,
            authors,
            description,
            language,
            cover_error: None,
            app,
        }
    }

    /// 운영체제 파일 선택기로 표지 이미지를 고른다.
    fn pick_cover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("book.info.pick_cover").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| this.use_cover(&[path], cx)).ok();
        })
        .detach();
    }

    /// 고른(끌어다 놓은) 파일 중 첫 이미지를 표지로 쓴다. 이미지가 없으면 안내를 띄운다.
    fn use_cover(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let image = paths.iter().find(|path| {
            path.extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .is_some_and(|ext| library::COVER_EXTENSIONS.contains(&ext.as_str()))
        });
        match image {
            Some(path) => {
                self.cover = Some(path.clone());
                self.cover_modified = library::modified_nanos(path);
                self.cover_error = None;
            }
            None => {
                self.cover_error = Some(t!("book.info.cover_formats").into());
            }
        }
        cx.notify();
    }

    /// 책 정보를 저장하고 대화상자를 닫는다. 실패하면 알림을 띄우고 열어 둔다.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut book = self.book.clone();
        book.title = self.title.read(cx).value().trim().to_string();
        book.authors = self
            .authors
            .read(cx)
            .value()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        book.description = self.description.read(cx).value().trim().to_string();
        book.language = self.language.to_string();
        let project = self.project.clone();
        let cover = self.cover.clone();
        let done = self.app.update(cx, |app, cx| {
            app.save_book(&project, book, cover.as_deref(), window, cx)
        });
        if done.unwrap_or(false) {
            window.close_dialog(cx);
        }
    }
}

impl Render for BookInfoForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let form = cx.entity().downgrade();
        let current = self.language.clone();
        let language = Button::new("book-language")
            .label(language_title(&self.language))
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                LANGUAGES
                    .into_iter()
                    .fold(menu.min_w(px(160.0)), |menu, (code, title)| {
                        let form = form.clone();
                        menu.item(
                            PopupMenuItem::new(title)
                                .checked(code == current.as_ref())
                                .on_click(move |_, _, cx| {
                                    form.update(cx, |form, cx| {
                                        form.language = code.into();
                                        cx.notify();
                                    })
                                    .ok();
                                }),
                        )
                    })
            });

        // 표지 사각형: 이미지 파일을 끌어다 놓거나, 눌러서 나오는 메뉴로 고르거나 지운다.
        let form = cx.entity().downgrade();
        let has_cover = self.cover.is_some();
        let preview = Button::new("book-cover")
            .ghost()
            .w(px(COVER_WIDTH))
            .h(px(COVER_HEIGHT))
            .p_0()
            .overflow_hidden()
            .border_1()
            .border_color(pal.panel_border)
            .bg(pal.panel)
            .tooltip(t!("book.info.cover_tooltip"))
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(pal.foreground).bg(pal.hover)
            })
            .on_drop(
                cx.listener(|this, paths: &ExternalPaths, _, cx| this.use_cover(paths.paths(), cx)),
            )
            .map(|this| match &self.cover {
                Some(path) => this.child(
                    img(crate::thumbnail::source(
                        path.clone(),
                        self.cover_modified,
                        px(COVER_HEIGHT),
                    ))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Icon::new(icons::image())
                            .text_color(pal.muted)
                            .into_any_element()
                    }),
                ),
                None => this.child(Icon::new(icons::image()).text_color(pal.muted)),
            })
            .dropdown_menu(move |menu, _, _| {
                let (pick_form, clear_form) = (form.clone(), form.clone());
                menu.min_w(px(160.0))
                    .item(PopupMenuItem::new(t!("book.info.choose_image")).on_click(
                        move |_, window, cx| {
                            pick_form
                                .update(cx, |form, cx| form.pick_cover(window, cx))
                                .ok();
                        },
                    ))
                    .when(has_cover, |menu| {
                        menu.item(PopupMenuItem::new(t!("book.info.clear_image")).on_click(
                            move |_, _, cx| {
                                clear_form
                                    .update(cx, |form, cx| {
                                        form.cover = None;
                                        form.cover_error = None;
                                        cx.notify();
                                    })
                                    .ok();
                            },
                        ))
                    })
            });
        // 왼쪽 위에 표지, 오른쪽에 나머지 항목.
        let cover = div()
            .w(px(COVER_WIDTH))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(preview)
            .when_some(self.cover_error.clone(), |this, error| {
                this.child(div().text_xs().text_color(pal.danger).child(error))
            });
        let fields = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(form_row(
                pal,
                t!("book.info.title"),
                Input::new(&self.title).focus_ring(false),
            ))
            .child(form_row(
                pal,
                t!("book.info.authors"),
                Input::new(&self.authors).focus_ring(false),
            ))
            .child(form_row(pal, t!("book.info.language"), language))
            .child(form_row(
                pal,
                t!("book.info.description"),
                Textarea::new(&self.description),
            ));

        div()
            .flex()
            .items_start()
            .gap_5()
            .child(cover)
            .child(fields)
    }
}

impl HanjiApp {
    /// 프로젝트의 책 정보 대화상자를 연다.
    pub(super) fn open_book_info(
        &mut self,
        dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.find_group(dir) else {
            return;
        };
        let app = cx.entity().downgrade();
        let form = cx.new(|cx| BookInfoForm::new(app, &project, window, cx));
        // 처음 그릴 때만 제목 칸에 포커스를 준다.
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let title = form.read(cx).title.clone();
                title.update(cx, |state, cx| state.focus(window, cx));
            }
            let ok_form = form.clone();
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("book.info.dialog_title"))
                .close_button(false)
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("book-info-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("book-info-ok")
                                .primary()
                                .label(t!("book.info.done"))
                                .on_click(move |_, window, cx| {
                                    ok_form.update(cx, |form, cx| form.submit(window, cx));
                                }),
                        ),
                )
        });
    }

    /// 책 정보를 저장한다. 고른 표지는 첨부파일 그룹에 `cover.<확장자>`로 넣는다. 저장했으면 `true`.
    fn save_book(
        &mut self,
        dir: &Path,
        mut book: library::BookMeta,
        cover: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(root) = self.library.as_ref().map(|lib| lib.root.clone()) else {
            return false;
        };
        let saved = cover
            .map(|cover| library::set_cover(dir, cover))
            .transpose()
            .and_then(|cover| {
                book.cover = cover;
                library::write_book(dir, &mut book)?;
                library::scan(&root)
            });
        match saved {
            Ok(scanned) => {
                // 첨부파일 그룹이 새로 생겼을 수 있다.
                self.library = Some(scanned);
                self.load_previews(cx);
                cx.notify();
                true
            }
            Err(err) => {
                self.notify(t!("book.info.save_failed", err = err), window, cx);
                false
            }
        }
    }
}
