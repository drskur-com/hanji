//! PDF 내보내기: 미리보기가 배치한 페이지를 그대로 PDF에 찍는다(ADR-0014).
//!
//! 배치는 미리보기(`app/book_preview/layout.rs`)가 gpui 글자 측정으로 한다. 여기서는 그 결과
//! (페이지마다 글자 조각·이미지·선의 위치, 단위 pt)를 받아 krilla로 쓴다. gpui 타입은 쓰지 않는다.
//!
//! - 서체: 시스템 서체를 이름·굵기·기울임으로 찾아(fontdb) 넣는다. krilla가 쓰는 글자만 남긴다(subset).
//! - 서체에 없는 글자는 대체 서체(한글·한자·가나)로 그린다. 조각 안의 글자 위치는 서체 글자 폭으로 잇는다.
//! - 글자 조각의 시작 x는 배치 그대로라 양쪽 정렬·들여쓰기가 미리보기와 같다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule};
use krilla::surface::Surface;
use krilla::text::{Font, TextDirection};

use super::ExportError;

/// 내보낼 문서.
pub struct PdfDocument {
    pub title: String,
    pub authors: Vec<String>,
    pub pages: Vec<PdfPage>,
    /// 첨부파일로 넣을 파일(동영상·오디오, ADR-0050). PDF 뷰어의 첨부 목록에서 열어 재생한다.
    pub attachments: Vec<PathBuf>,
}

/// 한 페이지. 길이는 pt, 원점은 왼쪽 위.
pub struct PdfPage {
    pub width: f32,
    pub height: f32,
    pub items: Vec<PdfItem>,
}

pub enum PdfItem {
    /// 한 줄(또는 양쪽 정렬된 행의 한 조각) 글자. `top`은 줄 칸 위쪽이다.
    Text {
        x: f32,
        top: f32,
        line_height: f32,
        size: f32,
        text: String,
        runs: Vec<PdfRun>,
    },
    Image {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        path: PathBuf,
        /// 칸을 꽉 채운다(넘치는 부분은 자른다). 아니면 칸에 그대로 맞춘다.
        cover: bool,
    },
    Rule {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [u8; 3],
    },
    /// 동영상 자리: 포스터(첫 장면)와 가운데 재생 표시. 포스터를 만들 수 없으면 어두운 칸이다.
    Video {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        path: PathBuf,
    },
    /// YouTube 동영상(ADR-0051): 썸네일과 재생 표시, 누르면 YouTube가 열리는 링크.
    YouTube {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        id: String,
    },
    /// 확장 블록 그림(ADR-0026): `canvas` 크기의 모양 목록을 칸에 맞춰 벡터로 그린다.
    Drawing {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        canvas: (f32, f32),
        shapes: Arc<Vec<crate::extension::Shape>>,
    },
}

/// 같은 모양의 글자 조각(바이트 길이).
#[derive(Clone, Debug)]
pub struct PdfRun {
    pub len: usize,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub color: [u8; 3],
    pub strike: bool,
}

/// 서체에 없는 글자를 그릴 대체 서체(앞에서부터 찾는다).
pub(crate) const FALLBACK_FAMILIES: [&str; 6] = [
    "Apple SD Gothic Neo",
    "Hiragino Sans",
    "PingFang SC",
    "Noto Sans CJK KR",
    "Malgun Gothic",
    "Arial Unicode MS",
];

/// 이름을 찾지 못한 서체(시스템 UI 서체 등) 대신 쓸 서체.
pub(crate) const DEFAULT_FAMILIES: [&str; 3] =
    ["Apple SD Gothic Neo", "Helvetica Neue", "Malgun Gothic"];

/// PDF로 쓴다.
pub fn write(document: &PdfDocument, target: &Path) -> Result<(), ExportError> {
    let bytes = render(document)?;
    std::fs::write(target, bytes)?;
    Ok(())
}

/// PDF 바이트를 만든다.
pub fn render(document: &PdfDocument) -> Result<Vec<u8>, ExportError> {
    let mut fonts = Fonts::new();
    let mut images: HashMap<PathBuf, Option<Image>> = HashMap::new();
    let mut posters: HashMap<PathBuf, Option<Image>> = HashMap::new();
    let mut thumbnails: HashMap<String, Option<Image>> = HashMap::new();
    let mut pdf = Document::new();
    let mut metadata = Metadata::new().title(document.title.clone());
    if !document.authors.is_empty() {
        metadata = metadata.authors(document.authors.clone());
    }
    pdf.set_metadata(metadata);

    for page in &document.pages {
        let settings =
            PageSettings::from_wh(page.width.max(1.), page.height.max(1.)).ok_or_else(|| {
                ExportError::InvalidProject(t!("export.error.page_size").into_owned())
            })?;
        let mut pdf_page = pdf.start_page_with(settings);
        // 그리는 동안은 페이지를 빌릴 수 없어 링크는 모아 두었다가 다 그린 뒤 단다.
        let mut links: Vec<(Rect, String)> = Vec::new();
        let mut surface = pdf_page.surface();
        for item in &page.items {
            match item {
                PdfItem::Text {
                    x,
                    top,
                    line_height,
                    size,
                    text,
                    runs,
                } => draw_text(
                    &mut surface,
                    &mut fonts,
                    *x,
                    *top,
                    *line_height,
                    *size,
                    text,
                    runs,
                ),
                PdfItem::Rule {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => fill_rect(&mut surface, *x, *y, *width, height.max(0.5), *color),
                PdfItem::Image {
                    x,
                    y,
                    width,
                    height,
                    path,
                    cover,
                } => {
                    let image = images
                        .entry(path.clone())
                        .or_insert_with(|| load_image(path))
                        .clone();
                    if let Some(image) = image {
                        draw_image(&mut surface, image, *x, *y, *width, *height, *cover);
                    }
                }
                PdfItem::Video {
                    x,
                    y,
                    width,
                    height,
                    path,
                } => {
                    let poster = posters
                        .entry(path.clone())
                        .or_insert_with(|| {
                            crate::media::poster_png(path)
                                .and_then(|png| Image::from_png(Arc::new(png).into(), true).ok())
                        })
                        .clone();
                    match poster {
                        Some(poster) => {
                            fill_rect(&mut surface, *x, *y, *width, *height, [0, 0, 0]);
                            draw_image(&mut surface, poster, *x, *y, *width, *height, false);
                        }
                        None => fill_rect(&mut surface, *x, *y, *width, *height, [40, 40, 40]),
                    }
                    draw_play_badge(&mut surface, *x, *y, *width, *height);
                }
                PdfItem::YouTube {
                    x,
                    y,
                    width,
                    height,
                    id,
                } => {
                    let thumbnail = thumbnails
                        .entry(id.clone())
                        .or_insert_with(|| {
                            crate::media::youtube_poster(id)
                                .and_then(|jpeg| Image::from_jpeg(Arc::new(jpeg).into(), true).ok())
                        })
                        .clone();
                    fill_rect(&mut surface, *x, *y, *width, *height, [40, 40, 40]);
                    if let Some(thumbnail) = thumbnail {
                        draw_image(&mut surface, thumbnail, *x, *y, *width, *height, true);
                    }
                    draw_play_badge(&mut surface, *x, *y, *width, *height);
                    if let Some(rect) = Rect::from_xywh(*x, *y, *width, *height) {
                        links.push((rect, crate::media::youtube_watch_url(id)));
                    }
                }
                PdfItem::Drawing {
                    x,
                    y,
                    width,
                    canvas,
                    shapes,
                    ..
                } => draw_shapes(&mut surface, &mut fonts, *x, *y, *width, *canvas, shapes),
            }
        }
        surface.finish();
        for (rect, uri) in links {
            pdf_page.add_annotation(krilla::annotation::Annotation::new_link(
                krilla::annotation::LinkAnnotation::new(
                    rect,
                    krilla::annotation::Target::Action(krilla::action::LinkAction::new(uri).into()),
                ),
                Some("YouTube".into()),
            ));
        }
        pdf_page.finish();
    }
    for path in &document.attachments {
        embed_attachment(&mut pdf, path);
    }
    pdf.finish().map_err(|err| {
        ExportError::InvalidProject(t!("export.error.pdf", err = format!("{err:?}")).into_owned())
    })
}

/// 파일을 문서 첨부파일로 넣는다. 읽지 못하면 넘어간다.
fn embed_attachment(pdf: &mut Document, path: &Path) {
    let Ok(data) = std::fs::read(path) else {
        return;
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    pdf.embed_file(krilla::embed::EmbeddedFile {
        path: name.clone(),
        mime_type: krilla::embed::MimeType::new(crate::media::mime_type(path)),
        description: Some(name),
        association_kind: krilla::embed::AssociationKind::Supplement,
        data: Arc::new(data).into(),
        modification_date: None,
        // 동영상·오디오는 이미 압축되어 있다.
        compress: Some(false),
        location: None,
    });
}

/// 동영상 칸 가운데의 재생 표시: 반투명 검은 원과 흰 삼각형.
fn draw_play_badge(surface: &mut Surface, x: f32, y: f32, width: f32, height: f32) {
    let radius = (width.min(height) * 0.12).clamp(6., 24.);
    let (cx, cy) = (x + width / 2., y + height / 2.);
    // 원은 베지어 네 개로 그린다(k = 4(√2 − 1)/3).
    let k = radius * 0.552_284_8;
    let mut circle = PathBuilder::new();
    circle.move_to(cx + radius, cy);
    circle.cubic_to(cx + radius, cy + k, cx + k, cy + radius, cx, cy + radius);
    circle.cubic_to(cx - k, cy + radius, cx - radius, cy + k, cx - radius, cy);
    circle.cubic_to(cx - radius, cy - k, cx - k, cy - radius, cx, cy - radius);
    circle.cubic_to(cx + k, cy - radius, cx + radius, cy - k, cx + radius, cy);
    circle.close();
    if let Some(path) = circle.finish() {
        surface.set_fill(Some(Fill {
            paint: rgb::Color::new(0, 0, 0).into(),
            opacity: NormalizedF32::new(0.55).unwrap_or(NormalizedF32::ONE),
            rule: FillRule::default(),
        }));
        surface.draw_path(&path);
    }
    let side = radius * 0.9;
    let mut triangle = PathBuilder::new();
    triangle.move_to(cx - side * 0.4, cy - side * 0.55);
    triangle.line_to(cx + side * 0.6, cy);
    triangle.line_to(cx - side * 0.4, cy + side * 0.55);
    triangle.close();
    if let Some(path) = triangle.finish() {
        surface.set_fill(Some(fill([255, 255, 255])));
        surface.draw_path(&path);
    }
}

fn fill_rect(surface: &mut Surface, x: f32, y: f32, width: f32, height: f32, color: [u8; 3]) {
    let Some(rect) = Rect::from_xywh(x, y, width.max(0.01), height.max(0.01)) else {
        return;
    };
    let mut builder = PathBuilder::new();
    builder.push_rect(rect);
    let Some(path) = builder.finish() else {
        return;
    };
    surface.set_fill(Some(fill(color)));
    surface.draw_path(&path);
}

fn fill(color: [u8; 3]) -> Fill {
    Fill {
        paint: rgb::Color::new(color[0], color[1], color[2]).into(),
        opacity: NormalizedF32::ONE,
        rule: FillRule::default(),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    surface: &mut Surface,
    fonts: &mut Fonts,
    x: f32,
    top: f32,
    line_height: f32,
    size: f32,
    text: &str,
    runs: &[PdfRun],
) {
    // 기준선: 줄 칸 안에서 글자 높이(ascent + descent)를 세로 가운데에 둔다(gpui와 같다).
    let Some(first) = runs.first() else {
        return;
    };
    let Some(primary) = fonts.face(&first.family, first.weight, first.italic) else {
        return;
    };
    let (ascent, descent) = fonts.metrics(primary);
    let baseline = top + (line_height - (ascent + descent) * size) / 2. + ascent * size;

    let mut pen = x;
    let mut start = 0;
    for run in runs {
        let end = (start + run.len).min(text.len());
        let Some(slice) = text.get(start..end) else {
            break;
        };
        start = end;
        let Some(face) = fonts.face(&run.family, run.weight, run.italic) else {
            continue;
        };
        let run_start = pen;
        // 글자마다 그릴 서체를 정해 같은 서체끼리 묶어 그린다.
        let mut segment = String::new();
        let mut segment_face = face;
        for c in slice.chars() {
            let chosen = fonts.face_for_char(face, c);
            if chosen != segment_face && !segment.is_empty() {
                pen += fonts.draw(
                    surface,
                    segment_face,
                    &segment,
                    pen,
                    baseline,
                    size,
                    run.color,
                );
                segment.clear();
            }
            segment_face = chosen;
            segment.push(c);
        }
        if !segment.is_empty() {
            pen += fonts.draw(
                surface,
                segment_face,
                &segment,
                pen,
                baseline,
                size,
                run.color,
            );
        }
        if run.strike {
            let thickness = size * 0.06;
            fill_rect(
                surface,
                run_start,
                baseline - size * 0.3 - thickness / 2.,
                pen - run_start,
                thickness,
                run.color,
            );
        }
    }
}

fn draw_image(
    surface: &mut Surface,
    image: Image,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    cover: bool,
) {
    let (natural_w, natural_h) = image.size();
    let (natural_w, natural_h) = (natural_w as f32, natural_h as f32);
    if natural_w <= 0. || natural_h <= 0. || width <= 0. || height <= 0. {
        return;
    }
    // 칸 비율과 이미지 비율이 다르면 cover는 넘치는 쪽을 자르고, 아니면 칸 안에 맞춰 가운데 둔다.
    let scale = if cover {
        (width / natural_w).max(height / natural_h)
    } else {
        (width / natural_w).min(height / natural_h)
    };
    let (draw_w, draw_h) = (natural_w * scale, natural_h * scale);
    let (dx, dy) = (x + (width - draw_w) / 2., y + (height - draw_h) / 2.);
    let Some(size) = Size::from_wh(draw_w, draw_h) else {
        return;
    };
    let clip = Rect::from_xywh(x, y, width, height).and_then(|rect| {
        let mut builder = PathBuilder::new();
        builder.push_rect(rect);
        builder.finish()
    });
    if let Some(clip) = &clip {
        surface.push_clip_path(clip, &FillRule::NonZero);
    }
    surface.push_transform(&Transform::from_translate(dx, dy));
    surface.draw_image(image, size);
    surface.pop();
    if clip.is_some() {
        surface.pop();
    }
}

/// 확장 그림을 벡터로 그린다. 캔버스 좌표를 (`x`, `y`)에서 폭 `width`로 맞추고, 캔버스 밖은 자른다.
fn draw_shapes(
    surface: &mut Surface,
    fonts: &mut Fonts,
    x: f32,
    y: f32,
    width: f32,
    canvas: (f32, f32),
    shapes: &[crate::extension::Shape],
) {
    use crate::extension::{Align, Shape};
    if canvas.0 <= 0. || canvas.1 <= 0. {
        return;
    }
    // 흐림·그림자·더하기는 PDF로 옮기지 못해, 그런 그림은 통째로 크게 그린 그림(PNG)으로 넣는다.
    if shapes
        .iter()
        .any(|shape| matches!(shape, Shape::Effects(effects) if effects.needs_raster()))
    {
        let frame = crate::extension::raster::Frame {
            label: String::new(),
            shapes,
        };
        let png = crate::extension::raster::render_png_at(
            &[frame],
            canvas.0,
            canvas.1,
            [1., 1., 1., 0.],
            RASTER_MAX_SIDE,
            RASTER_SCALE,
        );
        if let Some(image) = png
            .ok()
            .and_then(|png| Image::from_png(Arc::new(png).into(), true).ok())
        {
            let height = width * canvas.1 / canvas.0;
            draw_image(surface, image, x, y, width, height, false);
        }
        return;
    }
    let scale = width / canvas.0;
    surface.push_transform(&Transform::from_row(scale, 0., 0., scale, x, y));
    let canvas_clip = Rect::from_xywh(0., 0., canvas.0, canvas.1).and_then(|rect| {
        let mut builder = PathBuilder::new();
        builder.push_rect(rect);
        builder.finish()
    });
    if let Some(clip) = &canvas_clip {
        surface.push_clip_path(clip, &FillRule::NonZero);
    }
    // 지금 걸린 자르기(사각형 하나 + 경로들). 바뀌면 모두 풀고 다시 건다.
    let mut clip_rect: Option<[f32; 4]> = None;
    let mut clip_paths: Vec<(krilla::geom::Path, FillRule)> = Vec::new();
    let mut pushed = 0;
    let mut blend = None;
    for shape in shapes {
        let blended = blend.is_some()
            && matches!(
                shape,
                Shape::Fill { .. }
                    | Shape::Stroke { .. }
                    | Shape::Text { .. }
                    | Shape::Image { .. }
                    | Shape::Scene3D(_)
            );
        if let (true, Some(mode)) = (blended, blend) {
            surface.push_blend_mode(mode);
        }
        match shape {
            Shape::Effects(effects) => blend = krilla_blend(effects.blend),
            Shape::Clip(rect) => {
                clip_rect = *rect;
                pushed = reclip(surface, pushed, clip_rect, &clip_paths);
            }
            Shape::ClipPath(next) => {
                match next {
                    Some((path, rule)) => match krilla_path(path) {
                        Some(path) => clip_paths.push((path, fill_rule(*rule))),
                        // 빈 경로로 자르면 아무것도 보이지 않는다.
                        None => clip_rect = Some([0., 0., 0., 0.]),
                    },
                    None => clip_paths.clear(),
                }
                pushed = reclip(surface, pushed, clip_rect, &clip_paths);
            }
            Shape::Fill { path, paint, rule } => {
                if let Some(path) = krilla_path(path) {
                    let (paint, opacity) = krilla_paint(paint);
                    surface.set_stroke(None);
                    surface.set_fill(Some(Fill {
                        paint,
                        opacity,
                        rule: fill_rule(*rule),
                    }));
                    surface.draw_path(&path);
                }
            }
            Shape::Stroke { path, paint, style } => {
                use crate::extension::{LineCap, LineJoin};
                if let Some(path) = krilla_path(path) {
                    let (paint, opacity) = krilla_paint(paint);
                    surface.set_fill(None);
                    surface.set_stroke(Some(krilla::paint::Stroke {
                        paint,
                        width: style.width,
                        miter_limit: style.miter_limit,
                        line_cap: match style.cap {
                            LineCap::Butt => krilla::paint::LineCap::Butt,
                            LineCap::Round => krilla::paint::LineCap::Round,
                            LineCap::Square => krilla::paint::LineCap::Square,
                        },
                        line_join: match style.join {
                            LineJoin::Miter => krilla::paint::LineJoin::Miter,
                            LineJoin::Round => krilla::paint::LineJoin::Round,
                            LineJoin::Bevel => krilla::paint::LineJoin::Bevel,
                        },
                        opacity,
                        dash: None,
                    }));
                    surface.draw_path(&path);
                    surface.set_stroke(None);
                }
            }
            Shape::Text {
                text,
                origin,
                size,
                color,
                align,
                width,
                angle,
            } if !text.trim().is_empty() => {
                let run = PdfRun {
                    len: text.len(),
                    family: String::new(),
                    weight: 400,
                    italic: false,
                    color: rgb8(*color),
                    strike: false,
                };
                let shift = match (align, width) {
                    (Align::Center, Some(width)) => (width - fonts.width(text, *size)) / 2.,
                    (Align::Right, Some(width)) => width - fonts.width(text, *size),
                    _ => 0.,
                };
                let rotated = *angle != 0.;
                if rotated {
                    let (sin, cos) = angle.sin_cos();
                    let [ox, oy] = *origin;
                    surface.push_transform(&Transform::from_row(
                        cos,
                        sin,
                        -sin,
                        cos,
                        ox - cos * ox + sin * oy,
                        oy - sin * ox - cos * oy,
                    ));
                }
                draw_text(
                    surface,
                    fonts,
                    origin[0] + shift,
                    origin[1],
                    size * crate::extension::canvas::LINE_HEIGHT,
                    *size,
                    text,
                    &[run],
                );
                if rotated {
                    surface.pop();
                }
            }
            Shape::Text { .. } => {}
            // 3차원 장면은 깊이 버퍼로 그린 그림으로 넣는다(인쇄용으로 크게).
            Shape::Scene3D(mesh) => {
                let png = crate::extension::raster3d::png(mesh, RASTER_SCALE * scale);
                let [bx, by, bw, bh] = mesh.bounds;
                if let Some(image) =
                    png.and_then(|png| Image::from_png(Arc::new(png).into(), true).ok())
                {
                    draw_image(surface, image, bx, by, bw, bh, false);
                }
            }
            Shape::Image {
                image,
                src: [sx, sy, sw, sh],
                transform: [a, b, c, d, e, f],
                opacity,
                ..
            } => {
                // 원래 파일을 그대로 넣는다(인쇄 해상도). `src` 밖은 자른다.
                let data: krilla::Data = image.encoded().clone().into();
                let pdf_image = match image.format {
                    "png" => Image::from_png(data, true),
                    "jpeg" => Image::from_jpeg(data, true),
                    "gif" => Image::from_gif(data, true),
                    _ => Image::from_webp(data, true),
                };
                let clip = Rect::from_xywh(*sx, *sy, *sw, *sh).and_then(|rect| {
                    let mut builder = PathBuilder::new();
                    builder.push_rect(rect);
                    builder.finish()
                });
                if let (Ok(pdf_image), Some(clip), Some(size)) = (
                    pdf_image,
                    clip,
                    Size::from_wh(image.width as f32, image.height as f32),
                ) {
                    surface.push_transform(&Transform::from_row(*a, *b, *c, *d, *e, *f));
                    surface.push_clip_path(&clip, &FillRule::NonZero);
                    surface.push_opacity(normalized(*opacity));
                    surface.draw_image(pdf_image, size);
                    surface.pop();
                    surface.pop();
                    surface.pop();
                }
            }
        }
        if blended {
            surface.pop();
        }
    }
    for _ in 0..pushed {
        surface.pop();
    }
    if canvas_clip.is_some() {
        surface.pop();
    }
    surface.pop();
}

/// 확장 그림을 그림으로 넣을 때의 긴 변 상한과 배율(인쇄용).
const RASTER_MAX_SIDE: f32 = 4096.;
const RASTER_SCALE: f32 = 4.;

/// 확장의 섞기를 PDF의 것으로. 보통이면 `None`.
fn krilla_blend(blend: crate::extension::Blend) -> Option<krilla::blend::BlendMode> {
    use crate::extension::Blend;
    use krilla::blend::BlendMode;
    Some(match blend {
        // 더하기는 그림으로 넣으므로(위) 여기에 오지 않는다.
        Blend::Normal | Blend::Add => return None,
        Blend::Multiply => BlendMode::Multiply,
        Blend::Screen => BlendMode::Screen,
        Blend::Overlay => BlendMode::Overlay,
        Blend::Darken => BlendMode::Darken,
        Blend::Lighten => BlendMode::Lighten,
        Blend::ColorDodge => BlendMode::ColorDodge,
        Blend::ColorBurn => BlendMode::ColorBurn,
        Blend::HardLight => BlendMode::HardLight,
        Blend::SoftLight => BlendMode::SoftLight,
        Blend::Difference => BlendMode::Difference,
        Blend::Exclusion => BlendMode::Exclusion,
        Blend::Hue => BlendMode::Hue,
        Blend::Saturation => BlendMode::Saturation,
        Blend::Color => BlendMode::Color,
        Blend::Luminosity => BlendMode::Luminosity,
    })
}

/// 걸린 자르기를 풀고 새로 건다. 건 수를 돌려준다.
fn reclip(
    surface: &mut Surface,
    pushed: usize,
    rect: Option<[f32; 4]>,
    paths: &[(krilla::geom::Path, FillRule)],
) -> usize {
    for _ in 0..pushed {
        surface.pop();
    }
    let mut count = 0;
    if let Some([x, y, w, h]) = rect {
        let mut builder = PathBuilder::new();
        match Rect::from_xywh(x, y, w.max(0.), h.max(0.)) {
            Some(rect) => builder.push_rect(rect),
            // 넓이가 없는 사각형: 아무것도 보이지 않게 아주 작은 곳으로 자른다.
            None => {
                if let Some(rect) = Rect::from_xywh(-10., -10., 0.001, 0.001) {
                    builder.push_rect(rect);
                }
            }
        }
        if let Some(path) = builder.finish() {
            surface.push_clip_path(&path, &FillRule::NonZero);
            count += 1;
        }
    }
    for (path, rule) in paths {
        surface.push_clip_path(path, rule);
        count += 1;
    }
    count
}

fn krilla_path(segments: &[crate::extension::Segment]) -> Option<krilla::geom::Path> {
    use crate::extension::Segment;
    let mut builder = PathBuilder::new();
    for segment in segments {
        match *segment {
            Segment::Move([x, y]) => builder.move_to(x, y),
            Segment::Line([x, y]) => builder.line_to(x, y),
            Segment::Cubic([ax, ay], [bx, by], [x, y]) => builder.cubic_to(ax, ay, bx, by, x, y),
            Segment::Close => builder.close(),
        }
    }
    builder.finish()
}

fn fill_rule(rule: crate::extension::FillRule) -> FillRule {
    match rule {
        crate::extension::FillRule::NonZero => FillRule::NonZero,
        crate::extension::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

fn rgb8([r, g, b, _]: crate::extension::Color) -> [u8; 3] {
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    [channel(r), channel(g), channel(b)]
}

fn normalized(value: f32) -> NormalizedF32 {
    NormalizedF32::new(value.clamp(0., 1.)).unwrap_or(NormalizedF32::ONE)
}

/// 확장의 칠을 PDF 칠과 불투명도로. 그라디언트의 투명도는 멈춤마다 둔다.
fn krilla_paint(paint: &crate::extension::Paint) -> (krilla::paint::Paint, NormalizedF32) {
    use crate::extension::Paint;
    use krilla::paint::{LinearGradient, RadialGradient, SpreadMethod, Stop};
    let stops = |stops: &[(f32, crate::extension::Color)]| -> Vec<Stop> {
        stops
            .iter()
            .map(|(offset, color)| {
                let [r, g, b] = rgb8(*color);
                Stop {
                    offset: normalized(*offset),
                    color: rgb::Color::new(r, g, b).into(),
                    opacity: normalized(color[3]),
                }
            })
            .collect()
    };
    match paint {
        Paint::Solid(color) => {
            let [r, g, b] = rgb8(*color);
            (rgb::Color::new(r, g, b).into(), normalized(color[3]))
        }
        Paint::Linear { from, to, stops: s } => (
            LinearGradient {
                x1: from[0],
                y1: from[1],
                x2: to[0],
                y2: to[1],
                transform: Transform::identity(),
                spread_method: SpreadMethod::Pad,
                stops: stops(s),
                anti_alias: true,
            }
            .into(),
            NormalizedF32::ONE,
        ),
        Paint::Radial {
            center,
            radius,
            stops: s,
        } => (
            RadialGradient {
                fx: center[0],
                fy: center[1],
                fr: 0.,
                cx: center[0],
                cy: center[1],
                cr: *radius,
                transform: Transform::identity(),
                spread_method: SpreadMethod::Pad,
                stops: stops(s),
                anti_alias: true,
            }
            .into(),
            NormalizedF32::ONE,
        ),
    }
}

/// 이미지 파일을 읽는다. PNG·JPEG·GIF·WebP는 그대로 넣고, 나머지는 PNG로 바꿔 넣는다.
fn load_image(path: &Path) -> Option<Image> {
    let data = std::fs::read(path).ok()?;
    let direct = if data.starts_with(b"\x89PNG") {
        Image::from_png(Arc::new(data.clone()).into(), true).ok()
    } else if data.starts_with(&[0xFF, 0xD8]) {
        Image::from_jpeg(Arc::new(data.clone()).into(), true).ok()
    } else if data.starts_with(b"GIF8") {
        Image::from_gif(Arc::new(data.clone()).into(), true).ok()
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Image::from_webp(Arc::new(data.clone()).into(), true).ok()
    } else {
        None
    };
    direct.or_else(|| {
        let decoded = image::load_from_memory(&data).ok()?;
        let mut png = Vec::new();
        decoded
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        Image::from_png(Arc::new(png).into(), true).ok()
    })
}

/// 읽은 서체 하나.
struct Face {
    font: Font,
    data: Arc<Vec<u8>>,
    index: u32,
    /// ascent, descent(양수), 글자 폭 단위(em 대비).
    ascent: f32,
    descent: f32,
    units_per_em: f32,
}

type FaceId = usize;

/// 시스템 서체 찾기와 읽은 서체 모음.
struct Fonts {
    db: fontdb::Database,
    faces: Vec<Face>,
    by_source: HashMap<fontdb::ID, FaceId>,
    by_query: HashMap<(String, u16, bool), Option<FaceId>>,
    fallbacks: Option<Vec<FaceId>>,
    /// (서체, 글자) → 글자가 있는지.
    coverage: HashMap<(FaceId, char), bool>,
}

impl Fonts {
    fn new() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Self {
            db,
            faces: Vec::new(),
            by_source: HashMap::new(),
            by_query: HashMap::new(),
            fallbacks: None,
            coverage: HashMap::new(),
        }
    }

    /// 이름·굵기·기울임에 가장 가까운 서체. 이름을 못 찾으면 기본 서체.
    fn face(&mut self, family: &str, weight: u16, italic: bool) -> Option<FaceId> {
        let key = (family.to_string(), weight, italic);
        if let Some(found) = self.by_query.get(&key) {
            return *found;
        }
        let found = std::iter::once(family)
            .chain(DEFAULT_FAMILIES)
            .find_map(|name| self.query(name, weight, italic));
        self.by_query.insert(key, found);
        found
    }

    fn query(&mut self, family: &str, weight: u16, italic: bool) -> Option<FaceId> {
        let families = [fontdb::Family::Name(family)];
        let query = fontdb::Query {
            families: &families,
            weight: fontdb::Weight(weight),
            style: if italic {
                fontdb::Style::Italic
            } else {
                fontdb::Style::Normal
            },
            stretch: fontdb::Stretch::Normal,
        };
        let id = self.db.query(&query)?;
        // 다른 서체로 대신 찾아 준 결과는 받지 않는다(이름이 같아야 한다).
        let matches = self.db.face(id).is_some_and(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(family))
        });
        if !matches {
            return None;
        }
        self.load(id)
    }

    fn load(&mut self, id: fontdb::ID) -> Option<FaceId> {
        if let Some(face) = self.by_source.get(&id) {
            return Some(*face);
        }
        let (data, index) = self
            .db
            .with_face_data(id, |data, index| (data.to_vec(), index))?;
        let data = Arc::new(data);
        let parsed = ttf_parser::Face::parse(&data, index).ok()?;
        let units_per_em = f32::from(parsed.units_per_em());
        let ascent = f32::from(parsed.ascender()) / units_per_em;
        let descent = -f32::from(parsed.descender()) / units_per_em;
        let font = Font::new(data.clone().into(), index)?;
        self.faces.push(Face {
            font,
            data,
            index,
            ascent,
            descent,
            units_per_em,
        });
        let face = self.faces.len() - 1;
        self.by_source.insert(id, face);
        Some(face)
    }

    fn metrics(&self, face: FaceId) -> (f32, f32) {
        let face = &self.faces[face];
        (face.ascent, face.descent)
    }

    fn has_glyph(&mut self, face: FaceId, c: char) -> bool {
        if let Some(found) = self.coverage.get(&(face, c)) {
            return *found;
        }
        let info = &self.faces[face];
        let found = ttf_parser::Face::parse(&info.data, info.index)
            .ok()
            .and_then(|parsed| parsed.glyph_index(c))
            .is_some();
        self.coverage.insert((face, c), found);
        found
    }

    /// 글자를 그릴 서체: 그 서체에 있으면 그대로, 없으면 글자가 있는 첫 대체 서체.
    fn face_for_char(&mut self, face: FaceId, c: char) -> FaceId {
        if c.is_whitespace() || c.is_control() || self.has_glyph(face, c) {
            return face;
        }
        if self.fallbacks.is_none() {
            let fallbacks = FALLBACK_FAMILIES
                .iter()
                .filter_map(|name| self.query(name, 400, false))
                .collect();
            self.fallbacks = Some(fallbacks);
        }
        let fallbacks = self.fallbacks.clone().unwrap_or_default();
        fallbacks
            .into_iter()
            .find(|fallback| self.has_glyph(*fallback, c))
            .unwrap_or(face)
    }

    /// 기본 서체(대체 서체 포함)로 쓴 한 줄의 폭.
    fn width(&mut self, text: &str, size: f32) -> f32 {
        let Some(face) = self.face("", 400, false) else {
            return 0.;
        };
        text.chars()
            .map(|c| {
                let chosen = self.face_for_char(face, c);
                let info = &self.faces[chosen];
                ttf_parser::Face::parse(&info.data, info.index)
                    .ok()
                    .and_then(|parsed| {
                        let glyph = parsed.glyph_index(c)?;
                        parsed.glyph_hor_advance(glyph)
                    })
                    .map_or(0., |units| f32::from(units) / info.units_per_em * size)
            })
            .sum()
    }

    /// 한 서체로 글자를 그리고 그린 폭을 돌려준다.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        surface: &mut Surface,
        face: FaceId,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        color: [u8; 3],
    ) -> f32 {
        let info = &self.faces[face];
        surface.set_fill(Some(fill(color)));
        surface.draw_text(
            Point::from_xy(x, baseline),
            info.font.clone(),
            size,
            text,
            false,
            TextDirection::Auto,
        );
        let Ok(parsed) = ttf_parser::Face::parse(&info.data, info.index) else {
            return 0.;
        };
        let units: f32 = text
            .chars()
            .map(|c| {
                parsed
                    .glyph_index(c)
                    .and_then(|glyph| parsed.glyph_hor_advance(glyph))
                    .map_or(0., f32::from)
            })
            .sum();
        units / info.units_per_em * size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_pages_with_text_rules_and_fallback_glyphs() {
        let run = |len| PdfRun {
            len,
            family: "존재하지 않는 서체".into(),
            weight: 400,
            italic: false,
            color: [20, 20, 20],
            strike: false,
        };
        let text = "가나 Abc 漢字";
        let document = PdfDocument {
            title: "시험".into(),
            authors: vec!["지은이".into()],
            attachments: Vec::new(),
            pages: vec![
                PdfPage {
                    width: 420.,
                    height: 595.,
                    items: vec![
                        PdfItem::Text {
                            x: 50.,
                            top: 50.,
                            line_height: 16.,
                            size: 10.,
                            text: text.into(),
                            runs: vec![run(text.len())],
                        },
                        PdfItem::Rule {
                            x: 50.,
                            y: 80.,
                            width: 100.,
                            height: 0.5,
                            color: [180, 180, 180],
                        },
                    ],
                },
                PdfPage {
                    width: 420.,
                    height: 595.,
                    items: Vec::new(),
                },
            ],
        };
        let bytes = render(&document).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn videos_become_posters_and_media_files_are_attached() {
        let dir = std::env::temp_dir().join(format!("hanji-pdf-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("clip.mp4");
        let audio = dir.join("song.mp3");
        std::fs::write(&video, b"not a movie").unwrap();
        std::fs::write(&audio, b"ID3 sound").unwrap();
        let document = PdfDocument {
            title: "시험".into(),
            authors: Vec::new(),
            attachments: vec![video.clone(), audio.clone()],
            pages: vec![PdfPage {
                width: 420.,
                height: 595.,
                items: vec![PdfItem::Video {
                    x: 50.,
                    y: 50.,
                    width: 320.,
                    height: 180.,
                    path: video,
                }],
            }],
        };
        let bytes = render(&document).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/EmbeddedFile"), "첨부파일이 없다");
        assert!(text.contains("clip.mp4") && text.contains("song.mp3"));
        assert!(text.contains("/video#2Fmp4") || text.contains("video/mp4"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn youtube_becomes_a_link_to_the_video() {
        let document = PdfDocument {
            title: "시험".into(),
            authors: Vec::new(),
            attachments: Vec::new(),
            pages: vec![PdfPage {
                width: 420.,
                height: 595.,
                items: vec![PdfItem::YouTube {
                    x: 50.,
                    y: 50.,
                    width: 320.,
                    height: 180.,
                    id: "dQw4w9WgXcQ".into(),
                }],
            }],
        };
        let text = String::from_utf8_lossy(&render(&document).unwrap()).into_owned();
        assert!(text.contains("/URI"), "링크가 없다");
        assert!(text.contains("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn draws_extension_shapes_as_vectors() {
        use crate::extension::{Paint, Segment, Shape};
        let square = vec![
            Segment::Move([0., 0.]),
            Segment::Line([50., 0.]),
            Segment::Line([50., 50.]),
            Segment::Close,
        ];
        let shapes = vec![
            Shape::Clip(Some([0., 0., 40., 40.])),
            Shape::Fill {
                path: square.clone(),
                paint: Paint::Radial {
                    center: [25., 25.],
                    radius: 25.,
                    stops: vec![(0., [1., 0., 0., 1.]), (1., [1., 0., 0., 0.])],
                },
                rule: crate::extension::FillRule::NonZero,
            },
            Shape::Clip(None),
            Shape::Text {
                text: "가a".into(),
                origin: [0., 60.],
                size: 12.,
                color: [0., 0., 0., 1.],
                align: crate::extension::Align::Center,
                width: Some(100.),
                angle: 0.,
            },
        ];
        let document = PdfDocument {
            title: "그림".into(),
            authors: Vec::new(),
            attachments: Vec::new(),
            pages: vec![PdfPage {
                width: 200.,
                height: 200.,
                items: vec![PdfItem::Drawing {
                    x: 10.,
                    y: 10.,
                    width: 100.,
                    height: 80.,
                    canvas: (100., 80.),
                    shapes: Arc::new(shapes),
                }],
            }],
        };
        assert!(render(&document).unwrap().starts_with(b"%PDF"));
        // 흐림이 있으면 통째로 그림으로 넣는다.
        let mut blurred = vec![crate::extension::Shape::Effects(
            crate::extension::canvas::Effects {
                blur: 2.,
                ..Default::default()
            },
        )];
        if let PdfItem::Drawing { shapes, .. } = &document.pages[0].items[0] {
            blurred.extend(shapes.iter().cloned());
        }
        let document = PdfDocument {
            attachments: Vec::new(),
            pages: vec![PdfPage {
                width: 200.,
                height: 200.,
                items: vec![PdfItem::Drawing {
                    x: 10.,
                    y: 10.,
                    width: 100.,
                    height: 80.,
                    canvas: (100., 80.),
                    shapes: Arc::new(blurred),
                }],
            }],
            ..document
        };
        let pdf = render(&document).unwrap();
        assert!(
            pdf.windows(13)
                .any(|w| w == b"/Subtype /Ima" || w == b"/Subtype/Imag")
        );
    }
}
