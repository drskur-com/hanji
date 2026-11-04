//! 에디터의 이미지 줄 아래에 이미지를 그린다(ADR-0008).
//!
//! 이미지만 있는 줄(`![캡션](경로){속성}`, ADR-0007)은 그대로 고칠 수 있는 글자로 두고,
//! 그 아래에 gpui-base 줄 블록(`EditorState::set_line_blocks`)으로 이미지와 캡션을 그린다.
//! 이미지 줄 글자는 커서가 그 줄에 들어올 때만 보인다(`LineStyle::collapsed`).
//! 크기·정렬은 속성을 따르고, 없으면 원본 크기를 본문 폭 안에 맞춘다.
//!
//! 경로가 동영상·오디오면(ADR-0050) 같은 자리에 재생기를 그린다. 동영상은 포스터(첫 장면 썸네일) 위에
//! 재생 단추를 두고, 재생하면 프레임을 그린다. 오디오는 한 줄 카드다. 단추·진행 막대는 눌러도 커서가
//! 줄로 가지 않는다.
//!
//! YouTube 주소(ADR-0051)는 썸네일 카드로 그린다. 재생 단추를 누르면 새 창에서 재생한다(ADR-0052, 에디터 안에서는
//! 재생하지 않는다).

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use super::editor::EDITOR_MAX_WIDTH;
use super::gif_player::GifPlayer;
use super::media_player::MediaPlayer;
use super::style::Palette;
use crate::markdown::{self, ImageAlign, ImageOptions};
use crate::media::{self, MediaKind};
use crate::{icons, library};

/// 이미지 위 여백.
const PADDING_TOP: f32 = 4.0;

/// 이미지 아래 여백(다음 줄과 띄운다).
const PADDING_BOTTOM: f32 = 16.0;

/// 크기를 적지 않은 이미지의 최대 높이. 세로로 긴 사진이 화면을 다 차지하지 않게 한다.
const MAX_AUTO_HEIGHT: f32 = 480.0;

/// 비율을 모르는 이미지(웹 주소)를 그릴 칸 높이.
const UNKNOWN_HEIGHT: f32 = 240.0;

/// 이미지를 칸보다 크게 그려 잘라 내는 폭(가장자리 번짐 숨김, `render` 참고).
const EDGE_BLEED: f32 = 1.0;

/// 이미지 아래 캡션 줄 높이.
const CAPTION_HEIGHT: f32 = 24.0;

/// 이미지를 찾지 못했을 때 안내 줄 높이.
const MISSING_HEIGHT: f32 = 28.0;

/// 이보다 큰 GIF는 움직이지 않고 첫 프레임 썸네일만 보인다(원본을 메모리에 들고 있으므로).
const MAX_GIF_BYTES: u64 = 64 * 1024 * 1024;

/// 이미지 모서리 반경.
const CORNER_RADIUS: f32 = 4.0;

/// 크기를 모르는 동영상의 원본 크기(16:9).
const DEFAULT_VIDEO_SIZE: (f32, f32) = (640.0, 360.0);

/// 오디오 카드 높이와 기본 폭.
const AUDIO_HEIGHT: f32 = 48.0;
const AUDIO_WIDTH: f32 = 440.0;

/// 동영상 위 조작 줄 높이.
const CONTROLS_HEIGHT: f32 = 32.0;

/// 포스터 가운데 재생 단추 지름.
const BIG_PLAY_SIZE: f32 = 48.0;

/// 열린 시트의 재생기(경로마다 하나). 재생 단추가 다른 재생기를 멈출 때 함께 본다.
type Players = Rc<RefCell<HashMap<PathBuf, Rc<RefCell<MediaPlayer>>>>>;

/// 동영상·오디오 파일 정보. 헤더만 읽는다.
#[derive(Clone, Copy)]
struct MediaFile {
    /// 동영상의 화면 크기(px). 오디오는 `None`.
    size: Option<(f32, f32)>,
    modified: u128,
}

/// 로컬 이미지 파일 정보. 헤더만 읽는다.
#[derive(Clone, Copy)]
struct ImageFile {
    width: f32,
    height: f32,
    modified: u128,
}

/// 줄 하나에 그릴 이미지.
enum Source {
    /// 파일, 수정 시각, 썸네일 긴 변.
    File(PathBuf, u128, Pixels),
    /// 움직이는 GIF(ADR-0049)와 표시 크기의 긴 변.
    Animated(Rc<RefCell<GifPlayer>>, Pixels),
    Url(SharedUri),
    /// 동영상: 파일, 수정 시각, 포스터 긴 변, 재생기.
    Video(PathBuf, u128, Pixels, Rc<RefCell<MediaPlayer>>),
    Audio(Rc<RefCell<MediaPlayer>>),
    /// YouTube 동영상 ID.
    YouTube(String),
    /// 파일이 없거나 이미지로 읽을 수 없다. 적힌 경로를 보여 준다.
    Missing(String),
}

struct Placement {
    source: Source,
    /// 이미지 크기(여백 제외).
    size: Size<Pixels>,
    align: ImageAlign,
    /// 이미지 아래에 보일 캡션(alt). 비었거나 이미지를 못 찾았으면 `None`.
    caption: Option<SharedString>,
}

/// 열린 시트의 이미지 줄 판정·배치. 에디터 줄 블록의 측정·그리기에서 함께 쓴다.
pub(super) struct ImageBlocks {
    /// 상대 경로의 기준(시트가 든 폴더).
    dir: PathBuf,
    /// 읽은 이미지 헤더. 키 입력마다 같은 줄을 다시 재므로 파일을 매번 열지 않게 한다.
    /// 없는 파일은 남기지 않는다(나중에 생기면 바로 보이도록).
    files: RefCell<HashMap<PathBuf, ImageFile>>,
    /// 프레임이 여럿인 GIF의 재생기. 헤더를 읽을 때 함께 만들고, 스레드는 처음 그릴 때 띄운다.
    gifs: RefCell<HashMap<PathBuf, Rc<RefCell<GifPlayer>>>>,
    /// 읽은 동영상·오디오 헤더. 없는 파일은 남기지 않는다.
    media: RefCell<HashMap<PathBuf, MediaFile>>,
    players: Players,
}

impl ImageBlocks {
    pub(super) fn new(sheet: &Path) -> Self {
        Self {
            dir: sheet.parent().map(Path::to_path_buf).unwrap_or_default(),
            files: RefCell::default(),
            gifs: RefCell::default(),
            media: RefCell::default(),
            players: Players::default(),
        }
    }

    /// 줄 아래에 둘 블록 높이. 이미지 줄이 아니면 0.
    pub(super) fn measure(&self, line: &str, wrap_width: Option<Pixels>) -> Pixels {
        let width = wrap_width.unwrap_or(px(EDITOR_MAX_WIDTH));
        match self.place(line, width) {
            Some(placement) => {
                let caption = if placement.caption.is_some() {
                    CAPTION_HEIGHT
                } else {
                    0.0
                };
                placement.size.height + px(PADDING_TOP + caption + PADDING_BOTTOM)
            }
            None => px(0.),
        }
    }

    /// 줄 아래 블록에 이미지를 그린다.
    pub(super) fn render(&self, line: &str, size: Size<Pixels>, cx: &App) -> AnyElement {
        let Some(placement) = self.place(line, size.width) else {
            return Empty.into_any_element();
        };
        let now = Instant::now();
        for player in self.gifs.borrow().values() {
            player.borrow_mut().sleep_if_idle(now);
        }
        let pal = Palette::from_theme(cx.theme());
        let image = match placement.source {
            Source::Missing(destination) => div()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(pal.muted)
                .child(Icon::new(icons::image()).small())
                .child(t!("editor.image.not_found", path = destination))
                .into_any_element(),
            Source::Video(path, modified, long_side, player) => render_video(
                path,
                modified,
                long_side,
                player,
                self.players.clone(),
                placement.size,
                pal,
            ),
            Source::YouTube(id) => {
                render_youtube(&id, placement.caption.clone(), placement.size, pal)
            }
            Source::Audio(player) => render_audio(
                player,
                self.players.clone(),
                placement.caption.clone(),
                placement.size,
                pal,
            ),
            Source::Animated(player, long_side) => {
                let bleed = px(EDGE_BLEED);
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(placement.size.width)
                    .h(placement.size.height)
                    .overflow_hidden()
                    .rounded(px(CORNER_RADIUS))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                player.borrow_mut().paint(
                                    bounds,
                                    long_side,
                                    Corners::all(px(CORNER_RADIUS)),
                                    window,
                                    cx,
                                );
                            },
                        )
                        .absolute()
                        .left(-bleed)
                        .top(-bleed)
                        .w(placement.size.width + bleed * 2.)
                        .h(placement.size.height + bleed * 2.),
                    )
                    .into_any_element()
            }
            source => {
                // 파일은 칸 비율이 이미지 비율과 같아 가장자리까지 채우고, 웹 이미지는 비율을 몰라
                // 칸 안에 맞춘다.
                let (source, fit) = match source {
                    Source::File(path, modified, long_side) => (
                        crate::thumbnail::source(path, modified, long_side),
                        ObjectFit::Cover,
                    ),
                    Source::Url(uri) => (uri.into(), ObjectFit::Contain),
                    Source::Animated(..)
                    | Source::Video(..)
                    | Source::Audio(_)
                    | Source::YouTube(_)
                    | Source::Missing(_) => unreachable!(),
                };
                // gpui는 아틀라스에서 이미지를 선형 보간으로 읽는데 타일 경계에서 멈추지 않는다.
                // 그래서 줄이거나 소수 픽셀 위치(줄 높이 25.6px)에 그리면 오른쪽·아래 가장자리에
                // 옆 타일 색이 섞여 가는 테두리처럼 보인다. 칸보다 조금 크게 그리고 칸에서 잘라
                // 그 가장자리를 숨긴다.
                let bleed = px(EDGE_BLEED);
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(placement.size.width)
                    .h(placement.size.height)
                    .overflow_hidden()
                    .rounded(px(CORNER_RADIUS))
                    .child(
                        img(source)
                            .absolute()
                            .left(-bleed)
                            .top(-bleed)
                            .w(placement.size.width + bleed * 2.)
                            .h(placement.size.height + bleed * 2.)
                            .object_fit(fit)
                            .rounded(px(CORNER_RADIUS))
                            .with_fallback(move || {
                                Icon::new(icons::image())
                                    .text_color(pal.muted)
                                    .into_any_element()
                            }),
                    )
                    .into_any_element()
            }
        };
        div()
            .w(size.width)
            .h(size.height)
            .pt(px(PADDING_TOP))
            .flex()
            .flex_col()
            .map(|this| match placement.align {
                ImageAlign::Left => this.items_start(),
                ImageAlign::Center => this.items_center(),
                ImageAlign::Right => this.items_end(),
            })
            .child(image)
            .when_some(placement.caption, |this, caption| {
                this.child(
                    div()
                        .h(px(CAPTION_HEIGHT))
                        .max_w(placement.size.width.max(px(240.)))
                        .flex()
                        .items_end()
                        .text_sm()
                        .text_color(pal.muted)
                        .truncate()
                        .child(caption),
                )
            })
            .into_any_element()
    }

    /// 이미지 줄이면 그릴 이미지와 크기를 정한다. `width`는 본문 폭.
    fn place(&self, line: &str, width: Pixels) -> Option<Placement> {
        let image = markdown::image_line(line)?;
        let destination = image.destination;
        let caption = (!image.alt.trim().is_empty()).then(|| image.alt.trim().to_string().into());
        if let Some(id) = media::youtube_id(&destination) {
            let (w, h) = DEFAULT_VIDEO_SIZE;
            return Some(Placement {
                size: image_size(Some(size(px(w), px(h))), &image.options, width),
                source: Source::YouTube(id),
                align: image.options.align,
                caption,
            });
        }
        if let Some(kind) = media::kind_of(&destination) {
            return Some(self.place_media(kind, destination, &image.options, caption, width));
        }
        if library::is_web_address(&destination) {
            return Some(Placement {
                size: image_size(None, &image.options, width),
                source: Source::Url(destination.into()),
                align: image.options.align,
                caption,
            });
        }
        let resolved = library::resolve_image_path(&self.dir, None, &destination)
            .and_then(|path| self.file(&path).map(|file| (path, file)));
        let Some((path, file)) = resolved else {
            return Some(Placement {
                source: Source::Missing(destination),
                size: size(width, px(MISSING_HEIGHT)),
                align: ImageAlign::Left,
                caption: None,
            });
        };
        let natural = size(px(file.width), px(file.height));
        // 화면에 그릴 크기로 줄인 썸네일을 쓴다(큰 사진을 통째로 디코딩하지 않음).
        // 크기는 본문 최대 폭 기준으로 고정해 창 폭이 바뀔 때마다 새로 만들지 않는다.
        let largest = image_size(Some(natural), &image.options, px(EDITOR_MAX_WIDTH));
        let long_side = largest.width.max(largest.height);
        let source = match self.gifs.borrow().get(&path) {
            Some(player) => Source::Animated(player.clone(), long_side),
            None => Source::File(path, file.modified, long_side),
        };
        Some(Placement {
            size: image_size(Some(natural), &image.options, width),
            source,
            align: image.options.align,
            caption,
        })
    }

    /// 동영상·오디오 줄. 웹 주소는 재생하지 않는다(첨부파일로 넣어야 내보내기에도 들어간다).
    fn place_media(
        &self,
        kind: MediaKind,
        destination: String,
        options: &ImageOptions,
        caption: Option<SharedString>,
        width: Pixels,
    ) -> Placement {
        let resolved = (!library::is_web_address(&destination))
            .then(|| library::resolve_image_path(&self.dir, None, &destination))
            .flatten()
            .and_then(|path| self.media_file(kind, &path).map(|file| (path, file)));
        let Some((path, file)) = resolved else {
            return Placement {
                source: Source::Missing(destination),
                size: size(width, px(MISSING_HEIGHT)),
                align: ImageAlign::Left,
                caption: None,
            };
        };
        let player = self.player(&path, kind);
        match kind {
            MediaKind::Video => {
                let (w, h) = file.size.unwrap_or(DEFAULT_VIDEO_SIZE);
                let natural = size(px(w), px(h));
                let largest = image_size(Some(natural), options, px(EDITOR_MAX_WIDTH));
                Placement {
                    size: image_size(Some(natural), options, width),
                    source: Source::Video(
                        path,
                        file.modified,
                        largest.width.max(largest.height),
                        player,
                    ),
                    align: options.align,
                    caption,
                }
            }
            // 오디오 카드는 캡션을 제목으로 안에 보인다.
            MediaKind::Audio => Placement {
                size: size(
                    options
                        .width
                        .map_or(px(AUDIO_WIDTH), |width| px(width as f32))
                        .min(width)
                        .max(px(160.)),
                    px(AUDIO_HEIGHT),
                ),
                source: Source::Audio(player),
                align: options.align,
                caption: None,
            },
        }
    }

    fn media_file(&self, kind: MediaKind, path: &Path) -> Option<MediaFile> {
        if let Some(file) = self.media.borrow().get(path) {
            return Some(*file);
        }
        if !path.is_file() {
            return None;
        }
        let info = match kind {
            MediaKind::Video => media::video_info(path),
            MediaKind::Audio => None,
        };
        let file = MediaFile {
            size: info.map(|info| (info.width as f32, info.height as f32)),
            modified: library::modified_nanos(path),
        };
        self.media.borrow_mut().insert(path.to_path_buf(), file);
        Some(file)
    }

    /// 경로의 재생기. 처음 보면 만든다(파일은 재생할 때 연다). 길이는 헤더에서 읽어 둔다.
    fn player(&self, path: &Path, kind: MediaKind) -> Rc<RefCell<MediaPlayer>> {
        self.players
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let duration = match kind {
                    MediaKind::Video => media::video_info(path).and_then(|info| info.duration),
                    MediaKind::Audio => media::audio_duration(path),
                };
                Rc::new(RefCell::new(MediaPlayer::new(path.to_path_buf(), duration)))
            })
            .clone()
    }

    /// 프레임이 여럿인 GIF면 재생기를 만들어 둔다. 아니면(한 장, 너무 큼, 못 읽음) 썸네일로 보인다.
    fn add_gif(&self, path: &Path) {
        let small = std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_GIF_BYTES);
        let Some(data) = small.then(|| std::fs::read(path).ok()).flatten() else {
            return;
        };
        let Some(info) = crate::gif::scan(&data).filter(|info| info.frames() > 1) else {
            return;
        };
        let player = GifPlayer::new(Arc::new(data), info.repeat);
        self.gifs
            .borrow_mut()
            .insert(path.to_path_buf(), Rc::new(RefCell::new(player)));
    }

    fn file(&self, path: &Path) -> Option<ImageFile> {
        if let Some(file) = self.files.borrow().get(path) {
            return Some(*file);
        }
        let size = imagesize::size(path).ok()?;
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let file = ImageFile {
            width: size.width as f32,
            height: size.height as f32,
            modified: library::modified_nanos(path),
        };
        if is_gif(path) {
            self.add_gif(path);
        }
        self.files.borrow_mut().insert(path.to_path_buf(), file);
        Some(file)
    }
}

/// 동영상: 포스터 위에 (재생을 시작했으면) 프레임을 그리고, 아래에 조작 줄을 겹친다.
fn render_video(
    path: PathBuf,
    modified: u128,
    long_side: Pixels,
    player: Rc<RefCell<MediaPlayer>>,
    players: Players,
    shown: Size<Pixels>,
    pal: Palette,
) -> AnyElement {
    let started = player.borrow().started();
    let playing = player.borrow().playing();
    // 동영상 재생은 macOS에서만 된다(AVFoundation). 다른 OS는 포스터만 보인다.
    let playable = cfg!(target_os = "macos") && !player.borrow().failed();
    let painter = player.clone();
    div()
        .relative()
        .flex_shrink_0()
        .w(shown.width)
        .h(shown.height)
        .overflow_hidden()
        .rounded(px(CORNER_RADIUS))
        .bg(gpui_kit::gpui::black())
        .child(
            img(crate::thumbnail::source(path, modified, long_side))
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .with_fallback(move || {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icons::video()).text_color(pal.muted))
                        .into_any_element()
                }),
        )
        .when(started, |this| {
            this.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        painter.borrow_mut().paint_video(bounds, window);
                    },
                )
                .absolute()
                .size_full(),
            )
        })
        .when(playable && !playing && !started, |this| {
            this.child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(BIG_PLAY_SIZE))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui_kit::gpui::black().opacity(0.55))
                            .text_color(gpui_kit::gpui::white())
                            .cursor_pointer()
                            .child(Icon::new(icons::play()))
                            .on_mouse_down(
                                MouseButton::Left,
                                toggle_handler(player.clone(), players.clone(), MediaKind::Video),
                            ),
                    ),
            )
        })
        .when(playable, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(CONTROLS_HEIGHT))
                    .px_2()
                    .bg(gpui_kit::gpui::black().opacity(0.45))
                    .text_color(gpui_kit::gpui::white())
                    .child(controls(
                        player,
                        players,
                        MediaKind::Video,
                        None,
                        gpui_kit::gpui::white(),
                        gpui_kit::gpui::white().opacity(0.35),
                    )),
            )
        })
        .into_any_element()
}

/// YouTube 카드: 썸네일(16:9 칸에 채움), 가운데 재생 단추(누르면 새 창에서 재생한다), 왼쪽 아래 `YouTube` 표시.
fn render_youtube(
    id: &str,
    caption: Option<SharedString>,
    shown: Size<Pixels>,
    pal: Palette,
) -> AnyElement {
    let id = id.to_string();
    let thumbnail = media::youtube_thumbnail_url(&id);
    div()
        .relative()
        .flex_shrink_0()
        .w(shown.width)
        .h(shown.height)
        .overflow_hidden()
        .rounded(px(CORNER_RADIUS))
        .bg(gpui_kit::gpui::black())
        .child(
            img(SharedUri::from(thumbnail))
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(move || {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(icons::video()).text_color(pal.muted))
                        .into_any_element()
                }),
        )
        .child(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(BIG_PLAY_SIZE))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::gpui::black().opacity(0.55))
                        .text_color(gpui_kit::gpui::white())
                        .cursor_pointer()
                        .child(Icon::new(icons::play()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            crate::youtube_player::open(&id, caption.as_deref().unwrap_or(""), cx);
                        }),
                ),
        )
        .child(
            div()
                .absolute()
                .left_2()
                .bottom_2()
                .px_1p5()
                .rounded_sm()
                .bg(gpui_kit::gpui::black().opacity(0.55))
                .text_xs()
                .text_color(gpui_kit::gpui::white())
                .child("YouTube"),
        )
        .into_any_element()
}

/// 오디오 카드: [재생] 제목 ─── 진행 막대 ─── 시간.
fn render_audio(
    player: Rc<RefCell<MediaPlayer>>,
    players: Players,
    caption: Option<SharedString>,
    shown: Size<Pixels>,
    pal: Palette,
) -> AnyElement {
    let title: SharedString = caption.unwrap_or_else(|| {
        player
            .borrow()
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into()
    });
    div()
        .flex_shrink_0()
        .w(shown.width)
        .h(shown.height)
        .px_3()
        .rounded(px(CORNER_RADIUS * 2.))
        .border_1()
        .border_color(pal.panel_border)
        .bg(pal.panel)
        .text_color(pal.foreground)
        .child(controls(
            player,
            players,
            MediaKind::Audio,
            Some(title),
            pal.foreground,
            pal.panel_border,
        ))
        .into_any_element()
}

/// 재생 단추, (제목), 진행 막대, 시간을 한 줄로 둔다.
fn controls(
    player: Rc<RefCell<MediaPlayer>>,
    players: Players,
    kind: MediaKind,
    title: Option<SharedString>,
    fill: Hsla,
    track: Hsla,
) -> impl IntoElement {
    let (playing, position, duration, failed) = {
        let player = player.borrow();
        (
            player.playing(),
            player.position(),
            player.duration(),
            player.failed(),
        )
    };
    let fraction = duration
        .filter(|duration| !duration.is_zero())
        .map_or(0., |duration| {
            (position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
        });
    let time = match duration {
        Some(duration) if position.is_zero() && !playing => media::format_time(duration),
        Some(duration) => format!(
            "{} / {}",
            media::format_time(position),
            media::format_time(duration)
        ),
        None => media::format_time(position),
    };
    // 진행 막대를 누른 자리를 길이의 비율로 바꾸려고 막대 칸을 그릴 때 적어 둔다.
    let bar = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let bar_painted = bar.clone();
    let seeker = player.clone();
    let refresher = player.clone();
    div()
        .size_full()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .size(px(24.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .when(!failed, |this| this.cursor_pointer())
                .when(failed, |this| this.opacity(0.4))
                .child(
                    Icon::new(if playing {
                        icons::pause()
                    } else {
                        icons::play()
                    })
                    .small(),
                )
                .on_mouse_down(MouseButton::Left, toggle_handler(player, players, kind)),
        )
        .when_some(title, |this, title| {
            this.child(
                div()
                    .flex_shrink(1.)
                    .min_w(px(40.))
                    .max_w(relative(0.4))
                    .text_sm()
                    .truncate()
                    .child(title),
            )
        })
        .child(
            div()
                .flex_1()
                .h(px(16.))
                .flex()
                .items_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |event, window, _| {
                    window.prevent_default();
                    let bounds = bar.get();
                    if bounds.size.width > px(0.) {
                        let fraction = (event.position.x - bounds.origin.x) / bounds.size.width;
                        seeker.borrow_mut().seek(kind, fraction);
                        window.refresh();
                    }
                })
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(4.))
                        .rounded_full()
                        .bg(track)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(relative(fraction))
                                .rounded_full()
                                .bg(fill),
                        )
                        .child(
                            canvas(
                                move |bounds, _, _| bar_painted.set(bounds),
                                move |_, _, window, cx| {
                                    refresher.borrow_mut().schedule_refresh(window, cx);
                                },
                            )
                            .absolute()
                            .size_full(),
                        ),
                ),
        )
        .child(div().flex_shrink_0().text_xs().opacity(0.8).child(time))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            // 조작 줄 어디를 눌러도 커서가 이미지 줄로 가지 않는다.
            window.prevent_default();
            cx.stop_propagation();
        })
}

/// 재생 단추: 다른 재생기를 멈추고 이것을 재생하거나 멈춘다.
fn toggle_handler(
    player: Rc<RefCell<MediaPlayer>>,
    players: Players,
    kind: MediaKind,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        window.prevent_default();
        cx.stop_propagation();
        if !player.borrow().playing() {
            for other in players.borrow().values() {
                if !Rc::ptr_eq(other, &player) {
                    other.borrow_mut().pause();
                }
            }
        }
        player.borrow_mut().toggle(kind);
        window.refresh();
    }
}

/// 확장자가 `.gif`인지.
fn is_gif(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gif"))
}

/// 그릴 크기. `natural`은 원본 크기(모르면 `None`), `max_width`는 본문 폭.
///
/// - 크기를 둘 다 적었으면 그대로, 하나만 적었으면 원본 비율로 나머지를 정한다.
/// - 한 페이지 가득은 본문 폭에 맞춘다.
/// - 아무것도 적지 않았으면 원본 크기. 너무 크면 `MAX_AUTO_HEIGHT` 안으로 줄인다.
/// - 어느 경우든 본문 폭을 넘으면 비율을 지켜 줄인다.
fn image_size(
    natural: Option<Size<Pixels>>,
    options: &ImageOptions,
    max_width: Pixels,
) -> Size<Pixels> {
    let max_width = max_width.max(px(1.));
    let Some(natural) = natural else {
        // 비율을 모르면 정해진 칸에 맞춰 그린다(`object_fit(Contain)`).
        let width = options.width.map_or(max_width, |width| px(width as f32));
        let height = options
            .height
            .map_or(px(UNKNOWN_HEIGHT), |height| px(height as f32));
        return size(width.min(max_width), height);
    };
    let aspect = natural.height / natural.width;
    let (mut width, mut height, auto) = if options.full_page {
        (max_width, max_width * aspect, true)
    } else {
        match (options.width, options.height) {
            (Some(width), Some(height)) => (px(width as f32), px(height as f32), false),
            (Some(width), None) => (px(width as f32), px(width as f32) * aspect, false),
            (None, Some(height)) => (px(height as f32) / aspect, px(height as f32), false),
            (None, None) => (natural.width, natural.height, true),
        }
    };
    if width > max_width {
        height *= max_width / width;
        width = max_width;
    }
    if auto && height > px(MAX_AUTO_HEIGHT) {
        width *= px(MAX_AUTO_HEIGHT) / height;
        height = px(MAX_AUTO_HEIGHT);
    }
    size(width.round().max(px(1.)), height.round().max(px(1.)))
}

#[cfg(test)]
mod tests {
    // `gpui::*`의 `test` 매크로와 겹치지 않도록 하나씩 가져온다.
    use super::{
        CAPTION_HEIGHT, ImageBlocks, ImageOptions, MISSING_HEIGHT, PADDING_BOTTOM, PADDING_TOP,
        UNKNOWN_HEIGHT, image_size,
    };
    use gpui_kit::gpui::{px, size};

    fn options(width: Option<u32>, height: Option<u32>, full_page: bool) -> ImageOptions {
        ImageOptions {
            width,
            height,
            full_page,
            ..Default::default()
        }
    }

    #[test]
    fn image_size_follows_options_and_fits_the_text_width() {
        let photo = Some(size(px(1200.), px(800.)));
        let max = px(600.);
        // 자동: 본문 폭에 맞춘다.
        assert_eq!(
            image_size(photo, &options(None, None, false), max),
            size(px(600.), px(400.))
        );
        // 작은 이미지는 원본 크기.
        let icon = Some(size(px(64.), px(32.)));
        assert_eq!(
            image_size(icon, &options(None, None, false), max),
            size(px(64.), px(32.))
        );
        // 하나만 적으면 원본 비율.
        assert_eq!(
            image_size(photo, &options(Some(300), None, false), max),
            size(px(300.), px(200.))
        );
        assert_eq!(
            image_size(photo, &options(None, Some(100), false), max),
            size(px(150.), px(100.))
        );
        // 둘 다 적으면 그대로, 폭을 넘으면 비율대로 줄인다.
        assert_eq!(
            image_size(photo, &options(Some(300), Some(300), false), max),
            size(px(300.), px(300.))
        );
        assert_eq!(
            image_size(photo, &options(Some(1200), Some(300), false), max),
            size(px(600.), px(150.))
        );
        // 한 페이지 가득은 본문 폭.
        assert_eq!(
            image_size(icon, &options(None, None, true), max),
            size(px(600.), px(300.))
        );
        // 세로로 긴 자동 이미지는 높이를 제한한다.
        let tall = Some(size(px(400.), px(1600.)));
        assert_eq!(
            image_size(tall, &options(None, None, false), max),
            size(px(120.), px(480.))
        );
        // 비율을 모르면 정해진 칸.
        assert_eq!(
            image_size(None, &options(None, None, false), max),
            size(max, px(UNKNOWN_HEIGHT))
        );
    }

    #[test]
    fn measure_only_image_lines() {
        let dir = std::env::temp_dir().join(format!("hanji-editor-image-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Extras")).unwrap();
        image::RgbImage::new(200, 100)
            .save(dir.join("Extras/a.png"))
            .unwrap();
        let blocks = ImageBlocks::new(&dir.join("sheet.md"));
        let padding = px(PADDING_TOP + PADDING_BOTTOM);
        assert_eq!(blocks.measure("본문", Some(px(600.))), px(0.));
        assert_eq!(
            blocks.measure("![](Extras/a.png)", Some(px(600.))),
            px(100.) + padding
        );
        assert_eq!(
            blocks.measure("![](Extras/a.png){width=100px}", Some(px(600.))),
            px(50.) + padding
        );
        // 캡션이 있으면 그 줄만큼 더 높다.
        assert_eq!(
            blocks.measure("![지도](Extras/a.png){width=100px}", Some(px(600.))),
            px(50. + CAPTION_HEIGHT) + padding
        );
        assert_eq!(
            blocks.measure("![](없음.png)", Some(px(600.))),
            px(MISSING_HEIGHT) + padding
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
