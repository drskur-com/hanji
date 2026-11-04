//! 이미지 삽입 대화상자: 파일·첨부파일·URL에서 이미지를 고르고 크기·정렬·캡션을 정해
//! 열린 시트의 커서 자리에 `![캡션](경로){속성}`을 넣는다(속성 문법은 ADR-0007).
//!
//! 프로젝트 시트에 고른 파일은 첨부파일 그룹에 복사하고 프로젝트 폴더 기준 경로
//! (`Extras/Attachments/…`)로 잇는다. 시트를 옮겨도 경로가 바뀌지 않는다(ADR-0022).
//! 메모에 고른 파일은 메모가 함께 쓰는 첨부파일 그룹(`Notes/Attachments`)에 복사하고 `Notes/`
//! 기준 경로(`Attachments/…`)로 잇는다(ADR-0041).
//! 이미 첨부파일 그룹에 있는 이미지는 "첨부파일" 탭에서 골라 복사 없이 다시 쓴다.
//! 웹 주소 이미지도 내려받아 첨부파일로 넣는다(미리보기·PDF·ePub이 오프라인에서 같도록).
//! 라이브러리 밖의 시트에는 첨부파일 그룹이 없어서 원본 파일의 절대 경로와 주소를 그대로 쓴다.
//!
//! 동영상·오디오(ADR-0050)도 같은 대화상자로 넣는다. 문법은 이미지와 같고 확장자로 가른다. 웹 주소 탭은
//! 없고(파일로 넣어야 내보내기에 들어간다), 오디오는 크기·한 페이지 가득을 쓰지 않는다. 동영상의 URL 탭은
//! YouTube 주소만 받아 그대로 적는다(썸네일 카드, ADR-0051).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState, Paste};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, FocusableExt as _, Icon, Sizable as _, WindowExt as _,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::HanjiApp;
use super::new_project::form_row;
use super::style::*;
use crate::markdown::{ImageAlign, ImageOptions};
use crate::media::{self, MediaKind};
use crate::{InsertAudio, InsertImage, InsertVideo, icons, library, markdown};

/// 대화상자 폭.
const DIALOG_WIDTH: f32 = 520.0;

/// 파일을 놓는 자리와 첨부파일 목록의 높이. 탭을 바꿔도 대화상자 크기가 덜 출렁이게 같게 둔다.
const SOURCE_HEIGHT: f32 = 180.0;

/// 첨부파일 목록의 썸네일 한 칸 크기.
const THUMBNAIL_SIZE: f32 = 96.0;

/// 정렬 버튼 순서.
const ALIGNS: [(ImageAlign, &str); 3] = [
    (ImageAlign::Left, "insert.image.align_left"),
    (ImageAlign::Center, "insert.image.align_center"),
    (ImageAlign::Right, "insert.image.align_right"),
];

/// 이미지를 어디서 가져오는지.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    File,
    Attachment,
    Url,
}

/// 넣을 이미지.
enum ImageSource {
    File(PathBuf),
    Url(String),
    Existing(String),
    /// 웹에서 내려받은 이미지(바이트, 확장자). 첨부파일 그룹에 쓴다.
    Downloaded(Vec<u8>, &'static str),
}

/// 내려받을 이미지 크기 한도.
const MAX_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;

/// 웹 주소의 이미지를 내려받는다. 이미지가 아니거나 너무 크면 오류.
async fn download_image(
    client: std::sync::Arc<dyn http_client::HttpClient>,
    url: String,
) -> Result<(Vec<u8>, &'static str), String> {
    use futures::AsyncReadExt as _;
    let mut response = client
        .get(&url, http_client::AsyncBody::default(), true)
        .await
        .map_err(|err| t!("insert.image.download_failed", err = err).to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(t!("insert.image.download_http", status = status.as_u16()).to_string());
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .take(MAX_DOWNLOAD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|err| t!("insert.image.download_failed", err = err).to_string())?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(t!("insert.image.too_large").to_string());
    }
    let extension =
        library::image_extension(&bytes).ok_or_else(|| t!("insert.image.not_image").to_string())?;
    Ok((bytes, extension))
}

struct EditImage {
    state: Entity<gpui_kit::component::input::EditorState>,
    range: Range<usize>,
    original: String,
    destination: String,
    source: Source,
    path: Option<PathBuf>,
}

/// 대화상자 안의 입력 상태.
struct InsertImageForm {
    /// 넣을 것의 종류. `None`이면 이미지.
    kind: Option<MediaKind>,
    edit: Option<EditImage>,
    sheet: PathBuf,
    source: Source,
    /// 보여 줄 탭. 프로젝트 시트에만 "첨부파일" 탭이 있다.
    tabs: Vec<Source>,
    file: Option<PathBuf>,
    /// 고른 파일의 수정 시각(썸네일 캐시 키).
    file_modified: u128,
    /// 파일을 고르다 생긴 문제(지원하지 않는 형식 등).
    file_error: Option<SharedString>,
    /// 프로젝트 첨부파일 그룹의 이미지(프로젝트 밖이면 비어 있다)와 그중 고른 것.
    attachments: Vec<library::FileItem>,
    attachment: Option<PathBuf>,
    url: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    /// 가로세로 비율을 지킨다. 원본 크기를 알 때만 따른다.
    keep_ratio: bool,
    align: ImageAlign,
    full_page: bool,
    caption: Entity<InputState>,
    /// 웹 이미지를 내려받는 중. 끝나면 넣고 대화상자를 닫는다.
    download: Option<Task<()>>,
    download_error: Option<SharedString>,
    app: WeakEntity<HanjiApp>,
    _subscriptions: Vec<Subscription>,
}

/// 숫자만 받는 크기 입력 칸. 비우면 자동.
fn size_input(window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("insert.image.auto"))
            .validate(|text, _| text.chars().all(|c| c.is_ascii_digit()))
    })
}

/// 크기 입력 칸의 값. 비었거나 0이면 자동(`None`).
fn size_value(input: &Entity<InputState>, cx: &App) -> Option<u32> {
    input
        .read(cx)
        .value()
        .parse()
        .ok()
        .filter(|&value| value > 0)
}

/// 종류마다 다른 문구의 번역 키(`insert.image.*`·`insert.video.*`·`insert.audio.*`).
fn kind_key(kind: Option<MediaKind>, name: &str) -> String {
    let prefix = match kind {
        None => "insert.image",
        Some(MediaKind::Video) => "insert.video",
        Some(MediaKind::Audio) => "insert.audio",
    };
    format!("{prefix}.{name}")
}

/// 종류마다 다른 문구.
fn kind_text(kind: Option<MediaKind>, name: &str) -> SharedString {
    let key = kind_key(kind, name);
    let text = t!(&key).to_string();
    text.into()
}

/// 종류의 아이콘.
fn kind_icon(kind: Option<MediaKind>) -> IconName {
    match kind {
        None => icons::image(),
        Some(MediaKind::Video) => icons::video(),
        Some(MediaKind::Audio) => icons::audio(),
    }
}

impl InsertImageForm {
    fn new(
        kind: Option<MediaKind>,
        app: WeakEntity<HanjiApp>,
        sheet: PathBuf,
        attachments: Option<Vec<library::FileItem>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://"));
        let width = size_input(window, cx);
        let height = size_input(window, cx);
        let caption = cx.new(|cx| {
            InputState::new(window, cx).placeholder(kind_text(kind, "caption_placeholder"))
        });
        // 입력 칸의 Enter는 대화상자의 확인(Confirm)으로 올라가 `.on_ok`가 맡는다. 여기서 따로
        // 구독하면 확인과 겹쳐 이미지가 두 번 들어갈 수 있다.
        let mut subscriptions: Vec<Subscription> = Vec::new();
        // 비율을 지키면 한쪽을 고칠 때 다른 쪽을 맞춘다. `set_value`는 이벤트를 내지 않아 되돌아오지 않는다.
        for (changed, other, horizontal) in [(&width, &height, true), (&height, &width, false)] {
            let other = other.clone();
            subscriptions.push(cx.subscribe_in(
                changed,
                window,
                move |this, input, event, window, cx| {
                    if let InputEvent::Change = event {
                        this.sync_ratio(input, &other, horizontal, window, cx);
                    }
                },
            ));
        }
        let mut tabs = match attachments {
            Some(_) => vec![Source::File, Source::Attachment, Source::Url],
            None => vec![Source::File, Source::Url],
        };
        if kind == Some(MediaKind::Audio) {
            tabs.retain(|tab| *tab != Source::Url);
        }
        Self {
            kind,
            edit: None,
            sheet,
            source: Source::File,
            tabs,
            file: None,
            file_modified: 0,
            file_error: None,
            attachments: attachments.unwrap_or_default(),
            attachment: None,
            url,
            width,
            height,
            keep_ratio: true,
            align: ImageAlign::default(),
            full_page: false,
            caption,
            download: None,
            download_error: None,
            app,
            _subscriptions: subscriptions,
        }
    }

    /// 지금 고른 로컬 이미지 파일(파일·첨부파일 탭).
    fn local_path(&self) -> Option<&PathBuf> {
        match self.source {
            Source::File => self.file.as_ref(),
            Source::Attachment => self.attachment.as_ref(),
            Source::Url => None,
        }
    }

    /// 고른 이미지(동영상)의 원본 크기(px). URL·오디오이거나 읽지 못하면 `None`.
    fn natural_size(&self) -> Option<(u32, u32)> {
        match self.kind {
            Some(MediaKind::Video) => {
                let info = media::video_info(self.local_path()?)?;
                return Some((info.width, info.height));
            }
            Some(MediaKind::Audio) => return None,
            None => {}
        }
        let size = imagesize::size(self.local_path()?).ok()?;
        let (width, height) = (
            u32::try_from(size.width).ok()?,
            u32::try_from(size.height).ok()?,
        );
        (width > 0 && height > 0).then_some((width, height))
    }

    /// 크기 한쪽(`changed`)이 바뀌면 원본 비율대로 다른 쪽(`other`)을 채운다. 비우면 같이 비운다.
    fn sync_ratio(
        &mut self,
        changed: &Entity<InputState>,
        other: &Entity<InputState>,
        horizontal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.keep_ratio {
            return;
        }
        let Some((width, height)) = self.natural_size() else {
            return;
        };
        let value = match size_value(changed, cx) {
            Some(value) if horizontal => {
                (u64::from(value) * u64::from(height) + u64::from(width) / 2) / u64::from(width)
            }
            Some(value) => {
                (u64::from(value) * u64::from(width) + u64::from(height) / 2) / u64::from(height)
            }
            None => {
                other.update(cx, |state, cx| state.set_value("", window, cx));
                return;
            }
        };
        other.update(cx, |state, cx| {
            state.set_value(value.max(1).to_string(), window, cx)
        });
    }

    /// 다른 이미지를 고르면 비율을 지키는 중인 크기를 새 이미지에 맞춘다.
    fn refit_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.width.clone();
        let height = self.height.clone();
        if size_value(&width, cx).is_some() {
            self.sync_ratio(&width, &height, true, window, cx);
        } else if size_value(&height, cx).is_some() {
            self.sync_ratio(&height, &width, false, window, cx);
        }
    }

    /// 운영체제 파일 선택기로 이미지를 고른다.
    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(kind_text(self.kind, "choose_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.use_file(&paths, window, cx))
                .ok();
        })
        .detach();
    }

    /// 고른(끌어다 놓은) 파일 중 첫 이미지(동영상·오디오)를 쓴다. 없으면 안내를 띄운다.
    fn use_file(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.kind;
        let image = paths.iter().find(|path| {
            let path_str = path.to_string_lossy();
            match kind {
                Some(kind) => media::kind_of(&path_str) == Some(kind),
                None => path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase())
                    .is_some_and(|ext| library::COVER_EXTENSIONS.contains(&ext.as_str())),
            }
        });
        match image {
            Some(path) => {
                self.file = Some(path.clone());
                self.file_modified = library::modified_nanos(path);
                self.file_error = None;
                self.refit_size(window, cx);
            }
            None => {
                self.file_error = Some(kind_text(self.kind, "unsupported"));
            }
        }
        cx.notify();
    }

    /// 지금 탭에서 고른 이미지. 아직 없으면 `None`.
    fn image(&self, cx: &App) -> Option<ImageSource> {
        if let Some(edit) = &self.edit {
            let unchanged = self.source == edit.source
                && match self.source {
                    Source::Url => self.url.read(cx).value().trim() == edit.destination,
                    _ => self.local_path() == edit.path.as_ref(),
                };
            if unchanged {
                return Some(ImageSource::Existing(edit.destination.clone()));
            }
        }
        match self.source {
            Source::Url => {
                let url = self.url.read(cx).value().trim().to_string();
                (!url.is_empty()).then_some(ImageSource::Url(url))
            }
            _ => self.local_path().cloned().map(ImageSource::File),
        }
    }

    fn options(&self, cx: &App) -> ImageOptions {
        ImageOptions {
            width: size_value(&self.width, cx),
            height: size_value(&self.height, cx),
            align: self.align,
            full_page: self.full_page,
        }
    }

    /// 이미지를 넣고 대화상자를 닫는다. 실패하면 알림을 띄우고 열어 둔다.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.download.is_some() {
            return;
        }
        let Some(image) = self.image(cx) else {
            return;
        };
        // 첨부파일 그룹이 있는 시트의 웹 이미지는 먼저 내려받는다(이미 넣은 웹 이미지를 고쳐 저장할 때도).
        let attachable = self.tabs.contains(&Source::Attachment);
        let web = match &image {
            ImageSource::Url(url) | ImageSource::Existing(url) if library::is_web_address(url) => {
                Some(url.clone())
            }
            _ => None,
        };
        // YouTube 주소는 내려받지 않고 그대로 적는다. 동영상 대화상자의 주소는 YouTube만 받는다.
        let youtube = web.as_deref().and_then(media::youtube_id).is_some();
        if self.kind == Some(MediaKind::Video) && web.is_some() && !youtube {
            self.download_error = Some(t!("insert.video.youtube_only").into());
            cx.notify();
            return;
        }
        let web = web.filter(|_| !youtube);
        if attachable && let Some(url) = web {
            self.start_download(url, window, cx);
            return;
        }
        self.insert(image, window, cx);
    }

    fn insert(&mut self, image: ImageSource, window: &mut Window, cx: &mut Context<Self>) {
        let caption = self.caption.read(cx).value().to_string();
        let options = self.options(cx);
        let sheet = self.sheet.clone();
        let done = self.app.update(cx, |app, cx| {
            app.insert_image(
                &sheet,
                image,
                &caption,
                &options,
                self.edit.as_ref(),
                window,
                cx,
            )
        });
        if done.unwrap_or(false) {
            window.close_dialog(cx);
        }
    }

    /// 웹 이미지를 내려받아 첨부파일로 넣는다. 실패하면 대화상자에 알리고 열어 둔다.
    fn start_download(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.download_error = None;
        let client = cx.http_client();
        self.download = Some(cx.spawn_in(window, async move |this, cx| {
            let result = download_image(client, url).await;
            this.update_in(cx, |form, window, cx| {
                form.download = None;
                match result {
                    Ok((bytes, extension)) => {
                        form.insert(ImageSource::Downloaded(bytes, extension), window, cx)
                    }
                    Err(err) => form.download_error = Some(err.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl InsertImageForm {
    /// 파일 탭: 책 정보의 표지 칸처럼 끌어다 놓거나 눌러서 고른다. 고르면 미리 보여 준다.
    fn render_file(&self, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let zone = Button::new("insert-image-drop")
            .ghost()
            .w_full()
            .h(px(SOURCE_HEIGHT))
            .p_0()
            .overflow_hidden()
            .border_1()
            .border_color(pal.panel_border)
            .bg(pal.panel)
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(pal.foreground).bg(pal.hover)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.use_file(paths.paths(), window, cx)
            }))
            .on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx)))
            .map(|this| match &self.file {
                Some(path) if self.kind == Some(MediaKind::Audio) => this.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(pal.muted)
                        .child(Icon::new(icons::audio()).large())
                        .child(
                            path.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        ),
                ),
                Some(path) => this.child(
                    img(crate::thumbnail::source(
                        path.clone(),
                        self.file_modified,
                        px(DIALOG_WIDTH),
                    ))
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(move || {
                        Icon::new(icons::image())
                            .text_color(pal.muted)
                            .into_any_element()
                    }),
                ),
                None => this.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(pal.muted)
                        .child(Icon::new(kind_icon(self.kind)).large())
                        .child(kind_text(self.kind, "drop_hint")),
                ),
            });
        let status = match (&self.file_error, &self.file) {
            (Some(error), _) => Some(
                div()
                    .text_xs()
                    .text_color(pal.danger)
                    .child(error.clone())
                    .into_any_element(),
            ),
            (None, Some(path)) => Some(self.image_status(path, pal)),
            (None, None) => None,
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(zone)
            .children(status)
            .into_any_element()
    }

    /// 고른 이미지의 파일 이름과 원본 크기.
    fn image_status(&self, path: &Path, pal: Palette) -> AnyElement {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        div()
            .flex()
            .gap_2()
            .text_xs()
            .text_color(pal.muted)
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when_some(self.natural_size(), |this, (width, height)| {
                this.child(format!("{width} × {height}"))
            })
            .into_any_element()
    }

    /// 첨부파일 탭: 이미지 썸네일 격자. 누르면 고르고, 두 번 누르면 바로 넣는다.
    /// 고른 것은 아이콘·색 고르기처럼 진한 테두리로 표시한다.
    fn render_attachments(&self, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let frame = div()
            .h(px(SOURCE_HEIGHT))
            .rounded_md()
            .border_1()
            .border_color(pal.panel_border)
            .bg(pal.panel);
        if self.attachments.is_empty() {
            return frame
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(pal.muted)
                .child(Icon::new(icons::attachments()).large())
                .child(kind_text(self.kind, "no_attachments"))
                .into_any_element();
        }
        let fallback_kind = self.kind;
        let thumbnails = self.attachments.iter().enumerate().map(|(ix, file)| {
            let selected = self.attachment.as_ref() == Some(&file.path);
            let picked = file.path.clone();
            div()
                .id(("insert-image-attachment", ix))
                .size(px(THUMBNAIL_SIZE))
                .flex_shrink_0()
                .rounded_md()
                .overflow_hidden()
                .border_2()
                .border_color(if selected {
                    pal.foreground
                } else {
                    pal.panel_border
                })
                .bg(pal.background)
                .cursor_pointer()
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.attachment = Some(picked.clone());
                    this.refit_size(window, cx);
                    cx.notify();
                    if event.click_count() >= 2 {
                        this.submit(window, cx);
                    }
                }))
                .child(
                    img(crate::thumbnail::source(
                        file.path.clone(),
                        file.modified,
                        px(THUMBNAIL_SIZE),
                    ))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(kind_icon(fallback_kind)).text_color(pal.muted))
                            .into_any_element()
                    }),
                )
        });
        let status = self
            .attachment
            .as_ref()
            .map(|path| self.image_status(path, pal));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                frame.p_2().child(
                    div()
                        .id("insert-image-attachments")
                        .size_full()
                        .overflow_y_scroll()
                        .child(div().flex().flex_wrap().gap_2().children(thumbnails)),
                ),
            )
            .children(status)
            .into_any_element()
    }

    /// 크기: `[가로 px] × [세로 px] □ 비율 유지`. 라벨과 한 줄에 맞도록 한 줄로 둔다.
    fn render_size(&self, pal: Palette, disabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let field = |input: &Entity<InputState>| {
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .focus_ring(false)
                    .disabled(disabled)
                    .suffix(div().text_xs().text_color(pal.muted).child("px")),
            )
        };
        let can_keep = !disabled && self.natural_size().is_some();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(field(&self.width))
            .child(div().text_sm().text_color(pal.muted).child("×"))
            .child(field(&self.height))
            .child(
                Checkbox::new("insert-image-keep-ratio")
                    .ml_1()
                    .label(t!("insert.image.keep_ratio").to_string())
                    .checked(self.keep_ratio)
                    .disabled(!can_keep)
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.keep_ratio = *checked;
                        this.refit_size(window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// 정렬(왼쪽·가운데·오른쪽).
    fn render_align(&self, disabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let form = cx.entity().downgrade();
        TabBar::new("insert-image-align")
            .segmented()
            .selected_index(
                ALIGNS
                    .iter()
                    .position(|(align, _)| *align == self.align)
                    .unwrap_or(1),
            )
            .on_click(move |index: &usize, _, cx| {
                form.update(cx, |form, cx| {
                    if let Some((align, _)) = ALIGNS.get(*index) {
                        form.align = *align;
                        cx.notify();
                    }
                })
                .ok();
            })
            .children(
                ALIGNS
                    .iter()
                    .map(|(_, key)| Tab::new().flex_1().label(t!(*key)).disabled(disabled)),
            )
            .into_any_element()
    }

    /// 배치: 한 페이지 가득. 켜면 크기·정렬은 쓰지 않는다.
    fn render_full_page(&self, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Checkbox::new("insert-image-full-page")
                    .label(t!("insert.image.full_page").to_string())
                    .checked(self.full_page)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.full_page = *checked;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(pal.muted)
                    .child(t!("insert.image.full_page_hint")),
            )
            .into_any_element()
    }
}

impl Render for InsertImageForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let form = cx.entity().downgrade();
        let tabs = TabBar::new("insert-image-source")
            .segmented()
            .selected_index(
                self.tabs
                    .iter()
                    .position(|&tab| tab == self.source)
                    .unwrap_or(0),
            )
            .on_click(move |index: &usize, window, cx| {
                form.update(cx, |form, cx| {
                    let Some(&source) = form.tabs.get(*index) else {
                        return;
                    };
                    form.source = source;
                    match source {
                        Source::Url => form.url.update(cx, |state, cx| state.focus(window, cx)),
                        _ => form.refit_size(window, cx),
                    }
                    cx.notify();
                })
                .ok();
            })
            .children(self.tabs.iter().map(|tab| {
                let (icon, label) = match tab {
                    Source::File => (kind_icon(self.kind), t!("insert.image.tab_file")),
                    Source::Attachment => {
                        (icons::attachments(), t!("insert.image.tab_attachments"))
                    }
                    Source::Url if self.kind == Some(MediaKind::Video) => {
                        (icons::link(), "YouTube".into())
                    }
                    Source::Url => (icons::link(), "URL".into()),
                };
                Tab::new().flex_1().icon(icon).label(label)
            }));

        let source = match self.source {
            Source::File => self.render_file(pal, cx),
            Source::Attachment => self.render_attachments(pal, cx),
            Source::Url => {
                // 프로젝트 시트는 내려받아 첨부파일로 넣는다.
                let note: Option<(SharedString, Hsla)> = if self.download.is_some() {
                    Some((t!("insert.image.downloading").into(), pal.muted))
                } else if let Some(error) = &self.download_error {
                    Some((error.clone(), pal.danger))
                } else if self.kind == Some(MediaKind::Video) {
                    Some((t!("insert.video.youtube_note").into(), pal.muted))
                } else if self.tabs.contains(&Source::Attachment) {
                    Some((t!("insert.image.download_note").into(), pal.muted))
                } else {
                    None
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(form_row(
                        pal,
                        t!("insert.image.address"),
                        Input::new(&self.url).focus_ring(false),
                    ))
                    .when_some(note, |this, (note, color)| {
                        this.child(div().text_xs().text_color(color).child(note))
                    })
                    .into_any_element()
            }
        };

        // 한 페이지 가득이면 크기·정렬은 쓰지 않으므로 흐리게 막는다.
        let layout_disabled = self.full_page;
        let size = self.render_size(pal, layout_disabled, cx);
        let align = self.render_align(layout_disabled, cx);
        // 오디오는 카드라 크기를 고르지 않는다. 한 페이지 가득은 이미지만 쓴다.
        let sized = self.kind != Some(MediaKind::Audio);
        let full_page = self.kind.is_none();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(tabs)
            .child(source)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .when(layout_disabled, |this| this.opacity(0.5))
                    .when(sized, |this| {
                        this.child(form_row(pal, t!("insert.image.size"), size))
                    })
                    .child(form_row(pal, t!("insert.image.align"), align)),
            )
            .when(full_page, |this| {
                this.child(form_row(
                    pal,
                    t!("insert.image.placement"),
                    self.render_full_page(pal, cx),
                ))
            })
            .child(form_row(
                pal,
                t!("insert.image.caption"),
                Input::new(&self.caption).focus_ring(false),
            ))
    }
}

impl HanjiApp {
    /// 이미지 삽입 대화상자를 연다. 열린 시트가 없으면 안내만 한다.
    pub(super) fn on_insert_image(
        &mut self,
        _: &InsertImage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sheet) = self
            .editor
            .as_ref()
            .map(|editor| editor.path().to_path_buf())
        else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        self.open_image_dialog(sheet, None, None, window, cx);
    }

    /// 동영상 삽입 대화상자를 연다(ADR-0050).
    pub(super) fn on_insert_video(
        &mut self,
        _: &InsertVideo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_media_dialog(MediaKind::Video, window, cx);
    }

    /// 오디오 삽입 대화상자를 연다(ADR-0050).
    pub(super) fn on_insert_audio(
        &mut self,
        _: &InsertAudio,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_media_dialog(MediaKind::Audio, window, cx);
    }

    fn open_media_dialog(&mut self, kind: MediaKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self
            .editor
            .as_ref()
            .map(|editor| editor.path().to_path_buf())
        else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        self.open_image_dialog(sheet, Some(kind), None, window, cx);
    }

    pub(super) fn edit_image(
        &mut self,
        sheet: &Path,
        line: usize,
        original: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.path() != sheet {
            return;
        }
        let text = editor.state().read(cx).value();
        let Some((start, current)) = text
            .split('\n')
            .scan(0, |offset, line| {
                let start = *offset;
                *offset += line.len() + 1;
                Some((start, line))
            })
            .nth(line)
        else {
            return;
        };
        if current != original {
            return;
        }
        let Some(image) = markdown::image_line(current) else {
            return;
        };
        let source = if library::is_web_address(&image.destination) {
            Source::Url
        } else {
            Source::File
        };
        let path = (source == Source::File).then(|| {
            let dir = sheet.parent().unwrap_or(Path::new(""));
            library::resolve_image_path(dir, None, &image.destination).unwrap_or_else(|| {
                dir.join(
                    image
                        .destination
                        .strip_prefix("file://")
                        .unwrap_or(&image.destination),
                )
            })
        });
        let edit = EditImage {
            state: editor.state().clone(),
            range: start..start + current.len(),
            original: current.to_string(),
            destination: image.destination,
            source,
            path,
        };
        let kind = media::kind_of(&edit.destination)
            .or_else(|| media::youtube_id(&edit.destination).map(|_| MediaKind::Video));
        self.open_image_dialog(sheet.to_path_buf(), kind, Some(edit), window, cx);
    }

    fn open_image_dialog(
        &mut self,
        sheet: PathBuf,
        kind: Option<MediaKind>,
        edit: Option<EditImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let attachments = self.attachment_base_of(&sheet).map(|base| {
            self.find_group(&library::attachments_dir(&base))
                .map(|group| {
                    group
                        .files
                        .into_iter()
                        .filter(|file| {
                            file.kind
                                == match kind {
                                    None => library::FileKind::Image,
                                    Some(MediaKind::Video) => library::FileKind::Video,
                                    Some(MediaKind::Audio) => library::FileKind::Audio,
                                }
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        let app = cx.entity().downgrade();
        let editing = edit.is_some();
        let form = cx.new(|cx| {
            let mut form = InsertImageForm::new(kind, app, sheet, attachments, window, cx);
            if let Some(edit) = edit {
                let image = markdown::image_line(&edit.original).unwrap();
                form.source = edit.source;
                form.file = edit.path.clone();
                form.file_modified = form
                    .file
                    .as_deref()
                    .map(library::modified_nanos)
                    .unwrap_or(0);
                form.url.update(cx, |state, cx| {
                    state.set_value(image.destination, window, cx)
                });
                form.caption
                    .update(cx, |state, cx| state.set_value(image.alt, window, cx));
                form.width.update(cx, |state, cx| {
                    state.set_value(
                        image
                            .options
                            .width
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                        window,
                        cx,
                    )
                });
                form.height.update(cx, |state, cx| {
                    state.set_value(
                        image
                            .options
                            .height
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                        window,
                        cx,
                    )
                });
                form.align = image.options.align;
                form.full_page = image.options.full_page;
                form.edit = Some(edit);
            }
            form
        });
        // 처음 그릴 때만 캡션 칸에 포커스를 줘서 Enter로 바로 넣을 수 있게 한다.
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let caption = form.read(cx).caption.clone();
                caption.update(cx, |state, cx| state.focus(window, cx));
            }
            let ok_enter = form.clone();
            let ok_button = form.clone();
            dialog
                .w(px(DIALOG_WIDTH))
                .title(if editing {
                    kind_text(kind, "edit_title")
                } else {
                    kind_text(kind, "insert_title")
                })
                .close_button(false)
                // Enter는 대화상자의 확인(Confirm)이다. 넣었거나 내려받기를 시작했으면 `submit`이
                // 대화상자를 닫고, 아직 고른 이미지가 없으면 열어 둔다.
                .on_ok(move |_, window, cx| {
                    ok_enter.update(cx, |form, cx| form.submit(window, cx));
                    false
                })
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-image-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("insert-image-ok")
                                .primary()
                                .label(if editing {
                                    t!("common.save")
                                } else {
                                    t!("common.insert")
                                })
                                .on_click(move |_, window, cx| {
                                    ok_button.update(cx, |form, cx| form.submit(window, cx));
                                }),
                        ),
                )
        });
    }

    /// 열린 시트(`sheet`)의 커서 자리에 이미지 마크다운을 넣는다. 넣었으면 `true`.
    ///
    /// 이미지는 문단 하나로 둔다(캡션이 붙는 그림이 되도록). 커서 앞뒤에 글이 있으면 빈 줄로 띄운다.
    #[allow(clippy::too_many_arguments)]
    fn insert_image(
        &mut self,
        sheet: &Path,
        image: ImageSource,
        caption: &str,
        options: &ImageOptions,
        edit: Option<&EditImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editor.as_ref().map(|editor| editor.path()) != Some(sheet) {
            self.notify(t!("insert.image.sheet_closed"), window, cx);
            return false;
        }
        if let Some(edit) = edit {
            let editor = self.editor.as_ref().unwrap();
            let text = editor.state().read(cx).value();
            if editor.state() != &edit.state
                || text.get(edit.range.clone()) != Some(edit.original.as_str())
            {
                self.notify(t!("insert.image.changed"), window, cx);
                return false;
            }
        }
        let destination = match image {
            ImageSource::Existing(destination) => destination,
            ImageSource::Url(url) => url,
            ImageSource::Downloaded(bytes, extension) => match self.attachment_base_of(sheet) {
                Some(base) => match library::attach_bytes(&base, &bytes, extension) {
                    Ok(relative) => relative,
                    Err(err) => {
                        self.notify(t!("insert.image.attach_failed", err = err), window, cx);
                        return false;
                    }
                },
                None => return false,
            },
            ImageSource::File(path) => match self.attachment_base_of(sheet) {
                Some(base) => match library::add_attachment(&base, &path) {
                    // 첨부파일 그룹의 변화는 폴더 감시가 다시 읽어 반영한다.
                    Ok(relative) => relative,
                    Err(err) => {
                        self.notify(t!("insert.image.attach_failed", err = err), window, cx);
                        return false;
                    }
                },
                None => path.to_string_lossy().into_owned(),
            },
        };
        let Some(editor) = &self.editor else {
            return false;
        };
        let image = markdown::image(caption, &destination, options);
        editor.state().update(cx, |state, cx| {
            if let Some(edit) = edit {
                state.set_selected_range(edit.range.clone(), cx);
                state.replace(image, window, cx);
                state.focus(window, cx);
                return;
            }
            let text = state.value();
            let cursor = state.cursor().min(text.len());
            let text = paragraph_around(&text[..cursor], &text[cursor..], &image);
            state.insert(text, window, cx);
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
        true
    }

    /// 빈 줄에 YouTube 주소 하나를 붙여넣으면 YouTube 카드 줄(`![](주소)`)로 넣는다(ADR-0051).
    /// 그 밖의 붙여넣기는 에디터의 보통 붙여넣기로 넘긴다.
    pub(super) fn on_paste_youtube(
        &mut self,
        _: &Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(clipboard) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(editor) = &self.editor else {
            return;
        };
        let handled = editor.state().update(cx, |state, cx| {
            let text = state.value();
            let Some((range, line)) = youtube_paste(&text, state.selected_range(), &clipboard)
            else {
                return false;
            };
            state.set_selected_range(range, cx);
            state.replace(line, window, cx);
            true
        });
        if handled {
            cx.stop_propagation();
            self.on_editor_changed(window, cx);
        }
    }

    /// 시트의 첨부파일 기준 폴더(프로젝트 폴더나 `Notes/`). 라이브러리 밖이면 `None`.
    fn attachment_base_of(&self, sheet: &Path) -> Option<PathBuf> {
        self.library.as_ref()?.attachment_base_of(sheet)
    }
}

/// `before`와 `after` 사이에 `block`을 독립 문단으로 넣을 때 쓸 텍스트(앞뒤 빈 줄 포함).
fn paragraph_around(before: &str, after: &str, block: &str) -> String {
    let lead = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let trail = if after.is_empty() || after.starts_with("\n\n") {
        ""
    } else if after.starts_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{lead}{block}{trail}")
}

/// `clipboard`가 YouTube 주소 하나이고 `selection`이 든 줄에 다른 글이 없으면, 그 줄을 바꿀 범위와 카드 줄.
fn youtube_paste(
    text: &str,
    selection: Range<usize>,
    clipboard: &str,
) -> Option<(Range<usize>, String)> {
    let url = clipboard.trim();
    if url.contains(char::is_whitespace) {
        return None;
    }
    media::youtube_id(url)?;
    let start = text[..selection.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[selection.end..]
        .find('\n')
        .map_or(text.len(), |i| selection.end + i);
    let blank = |part: &str| part.trim().is_empty();
    (blank(&text[start..selection.start]) && blank(&text[selection.end..end])).then(|| {
        (
            start..end,
            markdown::image("", url, &ImageOptions::default()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{paragraph_around, youtube_paste};

    #[test]
    fn youtube_address_on_an_empty_line_becomes_a_card() {
        let url = "https://youtu.be/dQw4w9WgXcQ";
        let card = format!("![]({url})");
        assert_eq!(youtube_paste("", 0..0, url), Some((0..0, card.clone())));
        assert_eq!(
            youtube_paste("글\n\n다음", 4..4, &format!(" {url}\n")),
            Some((4..4, card.clone()))
        );
        // 줄 전체를 고르고 붙여넣으면 그 줄을 바꾼다.
        assert_eq!(
            youtube_paste("글\n옛 줄\n", 4..11, url),
            Some((4..11, card))
        );
        // 글 사이에 붙여넣거나 YouTube가 아니면 보통 붙여넣기.
        assert_eq!(youtube_paste("보세요 ", 10..10, url), None);
        assert_eq!(youtube_paste("", 0..0, "https://vimeo.com/123456"), None);
        assert_eq!(youtube_paste("", 0..0, &format!("{url} {url}")), None);
    }

    #[test]
    fn image_becomes_its_own_paragraph() {
        assert_eq!(paragraph_around("", "", "IMG"), "IMG");
        assert_eq!(paragraph_around("글", "", "IMG"), "\n\nIMG");
        assert_eq!(paragraph_around("글\n", "\n다음", "IMG"), "\nIMG\n");
        assert_eq!(paragraph_around("글\n\n", "다음", "IMG"), "IMG\n\n");
    }
}
