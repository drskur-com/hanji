//! ePub 빌더: 프로젝트 원고를 표준 전자책(.epub)으로 패키징한다.
//!
//! - 제목 스타일(ADR-0009):
//!   - 별도 제목 페이지(`Placement::Page`, 도비라): 독립된 단독 페이지로 분리 및 세로 위치(`vertical_align`) 적용.
//!   - 제목 숨김(`Placement::Hidden`): 화면에서는 숨기되 목차에만 보존.
//!   - 본문 첫머리(`Placement::Inline`): 본문과 연결 및 새 페이지 시작(`page_break`) 처리.
//!   - 접두사 및 번호 매기기(`prefix`, `{n}`): 부/장/절별 자동 번호 매김.
//!   - 목차 포함(`in_toc`): 목차에서 뺄지 제어. 번호와 접두사는 그대로 붙는다.
//! - 독서 앱 호환: 별도의 불필요한 nav 페이지 없이 전자책 내장 목차(`toc.ncx`)가 단독으로 차례를 담당.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use super::{ExportError, ExportProject, load_project};
use crate::library::{Align, HeadingNumbers, Placement, ResolvedStyle, StyleTarget, VerticalAlign};
use crate::markdown::{Block, ColumnAlign, Inline, InlineObject, Marks, Rich};

/// XML 특수문자를 이스케이프한다.
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// 목차 항목에 쓸 글자. 개행은 스페이스로 바꾼다.
fn toc_label(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// 이미지 확장자에 맞는 MIME 타입을 돌려준다.
fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// 목차 항목.
#[derive(Debug, Clone)]
struct TocItem {
    id: String,
    title: String,
    href: String,
}

/// XHTML 섹션 하나 (독립된 페이지 또는 장 파일).
struct Section {
    id: String,
    filename: String,
    title: String,
    html_body: String,
}

/// EPUB 아카이브에 포함할 이미지 에셋.
struct ImageAsset {
    id: String,
    zip_path: String,
    source: AssetSource,
    media_type: &'static str,
    is_cover: bool,
}

/// 에셋 내용: 파일, 또는 만든 바이트(확장 그림 SVG).
enum AssetSource {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// ePub 생성 상태.
struct EpubBuilder<'a> {
    project: &'a ExportProject,
    sections: Vec<Section>,
    current_section: Option<Section>,
    section_counter: usize,
    /// 목차 포함 여부와 무관하게 제목마다 고유 앵커를 만든다.
    heading_counter: usize,
    image_counter: usize,
    images: Vec<ImageAsset>,
    image_dest_map: HashMap<String, String>,
    toc_items: Vec<TocItem>,
    /// 부(0), 장(1), 절(2) 카운터.
    numbers: HeadingNumbers,
    /// 지금 시트의 각주 참조 상태.
    notes: Notes,
}

/// 시트 하나의 각주 링크 상태. 각주 번호는 시트마다 1부터라 id에 시트 번호를 넣는다.
#[derive(Default)]
struct Notes {
    /// 몇 번째 시트인지(1부터).
    sheet: usize,
    /// 본문에서 이미 나온 각주 번호. 처음 나온 참조에만 돌아올 자리(id)를 단다.
    referenced: HashSet<usize>,
    /// 문단 안 확장 그림(ADR-0036)의 `<img>`. 글자 조각을 XHTML로 바꿀 때 그 자리에 넣는다(모든 시트에 같다).
    drawings: HashMap<InlineObject, String>,
}

impl Notes {
    /// 다음 시트로 넘어간다.
    fn next_sheet(&mut self) {
        self.sheet += 1;
        self.referenced.clear();
    }

    fn note_id(&self, number: usize) -> String {
        format!("fn-{}-{number}", self.sheet)
    }

    fn ref_id(&self, number: usize) -> String {
        format!("fnref-{}-{number}", self.sheet)
    }

    /// 본문의 각주 참조: EPUB 3 `noteref` 링크. 지원하는 리더는 팝업으로, 나머지는 링크로 보여 준다.
    fn reference(&mut self, number: usize) -> String {
        let id = if self.referenced.insert(number) {
            format!(" id=\"{}\"", self.ref_id(number))
        } else {
            String::new()
        };
        format!(
            "<a epub:type=\"noteref\" role=\"doc-noteref\" class=\"noteref\" href=\"#{}\"{id}><sup>{number}</sup></a>",
            self.note_id(number)
        )
    }
}

impl<'a> EpubBuilder<'a> {
    fn new(project: &'a ExportProject) -> Self {
        Self {
            project,
            sections: Vec::new(),
            current_section: None,
            section_counter: 0,
            heading_counter: 0,
            image_counter: 0,
            images: Vec::new(),
            image_dest_map: HashMap::new(),
            toc_items: Vec::new(),
            numbers: HeadingNumbers::default(),
            notes: Notes::default(),
        }
    }

    /// 현재 작업 중인 섹션이 있고 비어있지 않다면 마감한다.
    fn finish_section(&mut self) {
        if let Some(sec) = self.current_section.take()
            && !sec.html_body.trim().is_empty()
        {
            self.sections.push(sec);
        }
    }

    /// 현재 섹션을 가져오거나, 없으면 새로 만든다.
    fn ensure_section(&mut self, default_title: &str) -> &mut Section {
        if self.current_section.is_none() {
            self.section_counter += 1;
            let filename = format!("text/section_{:04}.xhtml", self.section_counter);
            let id = format!("sec_{:04}", self.section_counter);
            self.current_section = Some(Section {
                id,
                filename,
                title: default_title.to_string(),
                html_body: String::new(),
            });
        }
        self.current_section.as_mut().unwrap()
    }

    /// 이미지 파일을 ePub에 넣고, 섹션 파일(`text/…`)에서 본 상대 경로를 돌려준다.
    /// 같은 파일을 두 번 넣지 않는다.
    fn embed_image(&mut self, path: PathBuf) -> String {
        let key = path.to_string_lossy().to_string();
        if let Some(existing) = self.image_dest_map.get(&key) {
            return format!("../{existing}");
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_ascii_lowercase();
        let mime = mime_type_for_extension(&ext);
        self.image_counter += 1;
        let filename = format!("images/img_{:04}.{ext}", self.image_counter);

        self.images.push(ImageAsset {
            id: format!("img_{}", self.image_counter),
            zip_path: filename.clone(),
            source: AssetSource::File(path),
            media_type: mime,
            is_cover: false,
        });

        self.image_dest_map.insert(key, filename.clone());
        format!("../{filename}")
    }

    /// 동영상·오디오 파일을 `media/`에 넣고 섹션 파일에서 본 상대 경로를 돌려준다(ADR-0050). 같은 파일은 한 번만.
    fn embed_media(&mut self, path: PathBuf) -> String {
        let key = format!("media\u{0}{}", path.to_string_lossy());
        if let Some(existing) = self.image_dest_map.get(&key) {
            return format!("../{existing}");
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase();
        self.image_counter += 1;
        let filename = format!("media/media_{:04}.{ext}", self.image_counter);
        self.images.push(ImageAsset {
            id: format!("media_{}", self.image_counter),
            zip_path: filename.clone(),
            media_type: crate::media::mime_type(&path),
            source: AssetSource::File(path),
            is_cover: false,
        });
        self.image_dest_map.insert(key, filename.clone());
        format!("../{filename}")
    }

    /// 동영상 포스터(첫 장면 PNG)를 넣고 경로를 돌려준다. 만들 수 없으면(macOS 밖 등) `None`.
    fn embed_poster(&mut self, path: &Path) -> Option<String> {
        let key = format!("poster\u{0}{}", path.to_string_lossy());
        if let Some(existing) = self.image_dest_map.get(&key) {
            return Some(format!("../{existing}"));
        }
        let png = crate::media::poster_png(path)?;
        self.image_counter += 1;
        let filename = format!("images/poster_{:04}.png", self.image_counter);
        self.images.push(ImageAsset {
            id: format!("img_{}", self.image_counter),
            zip_path: filename.clone(),
            source: AssetSource::Bytes(png),
            media_type: "image/png",
            is_cover: false,
        });
        self.image_dest_map.insert(key, filename.clone());
        Some(format!("../{filename}"))
    }

    /// YouTube 줄(ADR-0051): 썸네일을 내려받아 넣고 YouTube로 가는 링크를 건다. ePub 리더 대부분이 iframe을
    /// 막아서 플레이어는 넣지 않는다. 썸네일을 받지 못하면 링크 문단이다.
    fn youtube(&mut self, img: &crate::markdown::ImageLine, id: &str) {
        let watch = crate::media::youtube_watch_url(id);
        let caption = img.alt.trim();
        let key = format!("youtube\u{0}{id}");
        let thumbnail = match self.image_dest_map.get(&key) {
            Some(existing) => Some(format!("../{existing}")),
            None => crate::media::youtube_poster(id).map(|jpeg| {
                self.image_counter += 1;
                let filename = format!("images/youtube_{:04}.jpg", self.image_counter);
                self.images.push(ImageAsset {
                    id: format!("img_{}", self.image_counter),
                    zip_path: filename.clone(),
                    source: AssetSource::Bytes(jpeg),
                    media_type: "image/jpeg",
                    is_cover: false,
                });
                self.image_dest_map.insert(key, filename.clone());
                format!("../{filename}")
            }),
        };
        let label = if caption.is_empty() {
            "YouTube"
        } else {
            caption
        };
        let sec = self.ensure_section(&self.project.name);
        let Some(thumbnail) = thumbnail else {
            sec.html_body.push_str(&format!(
                "<p class=\"media-link\"><a href=\"{}\">▶ {}</a></p>\n",
                escape_xml(&watch),
                escape_xml(label)
            ));
            return;
        };
        let align = match img.options.align {
            crate::markdown::ImageAlign::Left => "align-left",
            crate::markdown::ImageAlign::Center => "align-center",
            crate::markdown::ImageAlign::Right => "align-right",
        };
        let style = img
            .options
            .width
            .map(|w| format!(" style=\"width: {w}px;\""))
            .unwrap_or_default();
        sec.html_body.push_str(&format!(
            "<figure class=\"video youtube {align}\">\n  <a href=\"{watch}\"><img src=\"{}\" alt=\"{}\"{style}/></a>\n  <figcaption><a href=\"{watch}\">▶ {}</a></figcaption>\n</figure>\n",
            escape_xml(&thumbnail),
            escape_xml(caption),
            escape_xml(label),
            watch = escape_xml(&watch),
        ));
    }

    /// 동영상·오디오 줄: `<video>`·`<audio>`로 넣는다. 재생하지 못하는 리더는 안쪽의 포스터·캡션을 보인다.
    /// 웹 주소는 링크로, 찾지 못한 파일은 안내 문단으로 둔다.
    fn media(
        &mut self,
        img: &crate::markdown::ImageLine,
        kind: crate::media::MediaKind,
        sheet_dir: &Path,
    ) {
        use crate::media::MediaKind;
        let caption = img.alt.trim();
        if crate::library::is_web_address(&img.destination) {
            let label = if caption.is_empty() {
                img.destination.as_str()
            } else {
                caption
            };
            let sec = self.ensure_section(&self.project.name);
            sec.html_body.push_str(&format!(
                "<p class=\"media-link\"><a href=\"{}\">{}</a></p>\n",
                escape_xml(&img.destination),
                escape_xml(label)
            ));
            return;
        }
        let Some(path) = crate::library::resolve_image_path(
            sheet_dir,
            Some(&self.project.dir),
            &img.destination,
        )
        .filter(|path| path.is_file()) else {
            let sec = self.ensure_section(&self.project.name);
            sec.html_body.push_str(&format!(
                "<p class=\"missing-image\">[미디어를 찾을 수 없음: {}]</p>\n",
                escape_xml(&img.destination)
            ));
            return;
        };
        let poster = match kind {
            MediaKind::Video => self.embed_poster(&path),
            MediaKind::Audio => None,
        };
        let src = self.embed_media(path);
        let align = match img.options.align {
            crate::markdown::ImageAlign::Left => "align-left",
            crate::markdown::ImageAlign::Center => "align-center",
            crate::markdown::ImageAlign::Right => "align-right",
        };
        let mut style = String::new();
        if let Some(w) = img.options.width {
            style.push_str(&format!("width: {w}px; "));
        }
        if kind == MediaKind::Video
            && let Some(h) = img.options.height
        {
            style.push_str(&format!("height: {h}px; "));
        }
        let style = if style.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", style.trim())
        };
        // 재생하지 못하는 리더가 보일 내용.
        let fallback = match &poster {
            Some(poster) => format!(
                "<img src=\"{}\" alt=\"{}\"/>",
                escape_xml(poster),
                escape_xml(caption)
            ),
            None if caption.is_empty() => String::new(),
            None => escape_xml(caption),
        };
        let element = match kind {
            MediaKind::Video => format!(
                "  <video src=\"{}\" controls=\"controls\"{}{style}>{fallback}</video>\n",
                escape_xml(&src),
                poster
                    .as_ref()
                    .map(|poster| format!(" poster=\"{}\"", escape_xml(poster)))
                    .unwrap_or_default(),
            ),
            MediaKind::Audio => format!(
                "  <audio src=\"{}\" controls=\"controls\"{style}>{fallback}</audio>\n",
                escape_xml(&src)
            ),
        };
        let class = match kind {
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
        };
        let sec = self.ensure_section(&self.project.name);
        sec.html_body
            .push_str(&format!("<figure class=\"{class} {align}\">\n{element}"));
        if !caption.is_empty() {
            sec.html_body.push_str(&format!(
                "  <figcaption>{}</figcaption>\n",
                escape_xml(caption)
            ));
        }
        sec.html_body.push_str("</figure>\n");
    }

    /// 확장 블록 그림(`t = 0`, 밝은 테마)을 SVG로 넣고 경로를 돌려준다. 같은 이름·인자는 한 번만 넣는다.
    /// 내보내기에서 빼는 블록이면 `Ok(None)`, 그리지 못하면 `Err`(코드 블록으로 둔다).
    fn embed_drawing(
        &mut self,
        name: &str,
        args: &[(String, String)],
    ) -> Result<Option<String>, ()> {
        let key = format!("ext\u{0}{name}\u{0}{args:?}");
        if let Some(existing) = self.image_dest_map.get(&key) {
            return Ok(Some(format!("../{existing}")));
        }
        let Some(still) =
            crate::extension::still(&self.project.root, name, args).map_err(|_| ())?
        else {
            return Ok(None);
        };
        self.image_counter += 1;
        let filename = format!("images/ext_{:04}.svg", self.image_counter);
        self.images.push(ImageAsset {
            id: format!("img_{}", self.image_counter),
            zip_path: filename.clone(),
            source: AssetSource::Bytes(crate::extension::svg::to_svg(&still).into_bytes()),
            media_type: "image/svg+xml",
            is_cover: false,
        });
        self.image_dest_map.insert(key, filename.clone());
        Ok(Some(format!("../{filename}")))
    }

    /// 문단 안 확장 그림을 SVG로 넣고 그 `<img>`를 [`Notes::drawings`]에 둔다. 높이는 본문 행 높이 × 인라인
    /// `scale=`이다(에디터와 같다). 그리지 못한 그림은 넣지 않는다(그 자리는 비운다).
    fn embed_inline_drawings(&mut self, blocks: &[Block]) {
        let mut objects: Vec<InlineObject> = Vec::new();
        crate::markdown::for_each_rich(blocks, &mut |rich| {
            objects.extend(rich.iter().filter_map(|inline| inline.object.clone()));
        });
        let line_height = self.project.styles.resolve(StyleTarget::Body).line_height;
        for object in objects {
            if self.notes.drawings.contains_key(&object) {
                continue;
            }
            let Ok(Some(src)) = self.embed_drawing(&object.name, &object.args) else {
                continue;
            };
            let height = line_height * f64::from(crate::extension::scale_of(&object.args));
            let img = format!(
                "<img class=\"inline-extension\" src=\"{}\" alt=\"{}\" style=\"height: {height:.3}em\"/>",
                escape_xml(&src),
                escape_xml(&object.name)
            );
            self.notes.drawings.insert(object, img);
        }
    }

    /// 제목 블록을 스타일 설정에 맞춰 렌더링한다.
    fn render_heading(&mut self, level: u8, text: &Rich, unnumbered: bool) {
        self.heading_counter += 1;
        let anchor_id = format!("h-{}", self.heading_counter);
        let raw_title = crate::markdown::rich_text(text).trim().to_string();
        let target = StyleTarget::for_heading(level);

        if let Some(target) = target {
            let resolved = self.project.styles.resolve(target);

            // 번호와 접두사는 목차 포함 여부와 무관하게 붙인다. 다만 아예 그리지도 않고
            // 목차에도 넣지 않는 제목(숨김·목차 제외)은 번호를 쓰지 않는다.
            // `{unnumbered}` 제목(시트 첫 줄만)은 번호를 소비하지 않고 접두사도 붙지 않는다(ADR-0018).
            let numbered =
                !unnumbered && (resolved.placement != Placement::Hidden || resolved.in_toc);
            let number = self.numbers.count(&self.project.styles, target, numbered);
            // 본문 제목과 목차 제목을 따로 만든다. 목차에서는 접두사의 개행을 스페이스로 바꾼다.
            let (display_title, toc_title) = if numbered {
                (
                    resolved.title_text(number, &raw_title),
                    resolved.toc_title_text(number, &raw_title),
                )
            } else {
                (raw_title.clone(), raw_title.clone())
            };

            let target_class = match target {
                StyleTarget::Part => "title-part",
                StyleTarget::Chapter => "title-chapter",
                StyleTarget::Section => "title-section",
                _ => "title-heading",
            };

            match resolved.placement {
                Placement::Hidden => {
                    // 제목 숨김: 본문에는 나타나지 않음. 목차 포함 설정 시 목차에만 등록
                    if resolved.in_toc {
                        let filename = self.ensure_section(&display_title).filename.clone();
                        let href = format!("{}#{}", filename, anchor_id);
                        self.toc_items.push(TocItem {
                            id: anchor_id.clone(),
                            title: toc_title,
                            href,
                        });
                        self.ensure_section("")
                            .html_body
                            .push_str(&format!("<span id=\"{anchor_id}\"></span>\n"));
                    }
                }
                Placement::Page => {
                    // 별도 제목 페이지(도비라): 이전 섹션을 마감하고 단독 제목 섹션 생성
                    self.finish_section();

                    self.section_counter += 1;
                    let filename = format!("text/section_{:04}.xhtml", self.section_counter);
                    let id = format!("sec_{:04}", self.section_counter);

                    let v_align_class = match resolved.vertical_align {
                        VerticalAlign::Top => "vertical-top",
                        VerticalAlign::Center => "vertical-center",
                        VerticalAlign::Bottom => "vertical-bottom",
                    };

                    let heading_html = if numbered {
                        render_rich_with_prefix(text, &resolved, number, &mut self.notes)
                    } else {
                        render_rich(text, &mut self.notes)
                    };
                    let page_sec = Section {
                        id,
                        filename: filename.clone(),
                        title: display_title.clone(),
                        html_body: format!(
                            "<div class=\"title-page {v_align_class}\">\n  <h{level} class=\"{target_class}\" id=\"{anchor_id}\">{heading_html}</h{level}>\n</div>\n"
                        ),
                    };

                    if resolved.in_toc {
                        let href = format!("{}#{}", filename, anchor_id);
                        self.toc_items.push(TocItem {
                            id: anchor_id,
                            title: toc_title,
                            href,
                        });
                    }

                    // 단독 제목 페이지이므로 즉시 닫아 다음 본문이 새 파일에서 시작하도록 함
                    self.sections.push(page_sec);
                }
                Placement::Inline => {
                    // 본문 첫머리: 새 페이지 시작(page_break)이 설정되어 있으면 이전 섹션 마감
                    if resolved.page_break {
                        self.finish_section();
                    }

                    let heading_html = if numbered {
                        render_rich_with_prefix(text, &resolved, number, &mut self.notes)
                    } else {
                        render_rich(text, &mut self.notes)
                    };
                    let filename = self.ensure_section(&display_title).filename.clone();
                    if resolved.in_toc {
                        let href = format!("{}#{}", filename, anchor_id);
                        self.toc_items.push(TocItem {
                            id: anchor_id.clone(),
                            title: toc_title,
                            href,
                        });
                    }
                    self.ensure_section("").html_body.push_str(&format!(
                        "<h{level} class=\"{target_class}\" id=\"{anchor_id}\">{heading_html}</h{level}>\n"
                    ));
                }
            }
        } else {
            // H4~H6 등 제목 스타일 대상이 아닌 일반 헤딩
            let filename = self.ensure_section(&raw_title).filename.clone();
            let href = format!("{}#{}", filename, anchor_id);
            self.toc_items.push(TocItem {
                id: anchor_id.clone(),
                title: raw_title,
                href,
            });
            let html = render_rich(text, &mut self.notes);
            self.ensure_section("")
                .html_body
                .push_str(&format!("<h{level} id=\"{anchor_id}\">{html}</h{level}>\n"));
        }
    }

    /// 일반 블록 렌더링.
    fn render_block(&mut self, block: &Block, sheet_dir: &Path) {
        match block {
            Block::Heading {
                level,
                text,
                unnumbered,
            } => {
                self.render_heading(*level, text, *unnumbered);
            }
            Block::Paragraph(rich) => {
                let html = render_rich(rich, &mut self.notes);
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str("<p>");
                sec.html_body.push_str(&html);
                sec.html_body.push_str("</p>\n");
            }
            Block::Lines(rich) => {
                let html = render_rich(rich, &mut self.notes);
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str("<p class=\"lines\">");
                sec.html_body.push_str(&html);
                sec.html_body.push_str("</p>\n");
            }
            Block::BlankLines(count) => {
                let sec = self.ensure_section(&self.project.name);
                for _ in 0..*count {
                    sec.html_body
                        .push_str("<div class=\"blank-line\" aria-hidden=\"true\">&#160;</div>\n");
                }
            }
            Block::Quote(blocks) => {
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str("<blockquote>\n");
                for b in blocks {
                    self.render_block(b, sheet_dir);
                }
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str("</blockquote>\n");
            }
            Block::List {
                ordered,
                start,
                items,
            } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let sec = self.ensure_section(&self.project.name);
                if *ordered && *start != 1 {
                    sec.html_body
                        .push_str(&format!("<{tag} start=\"{start}\">\n"));
                } else {
                    sec.html_body.push_str(&format!("<{tag}>\n"));
                }
                for item in items {
                    let Some(checked) = item.checked else {
                        let sec = self.ensure_section(&self.project.name);
                        sec.html_body.push_str("<li>\n");
                        for b in &item.blocks {
                            self.render_block(b, sheet_dir);
                        }
                        let sec = self.ensure_section(&self.project.name);
                        sec.html_body.push_str("</li>\n");
                        continue;
                    };
                    // 할 일 항목: 글머리표 대신 체크 상자를 첫 문단 앞에 둔다. 리더마다 폼 요소를
                    // 다르게 다루므로 `<input>` 대신 글자(☐·☑)를 쓴다.
                    let (mark, label) = if checked {
                        ("☑", "완료")
                    } else {
                        ("☐", "할 일")
                    };
                    let task_box = format!(
                        "<span class=\"task-box\" role=\"img\" aria-label=\"{label}\">{mark}</span> "
                    );
                    let sec = self.ensure_section(&self.project.name);
                    sec.html_body.push_str("<li class=\"task\">\n");
                    let mut blocks = item.blocks.iter();
                    match blocks.next() {
                        Some(Block::Paragraph(rich) | Block::Lines(rich)) => {
                            let class = if matches!(item.blocks[0], Block::Lines(_)) {
                                " class=\"lines\""
                            } else {
                                ""
                            };
                            let html = render_rich(rich, &mut self.notes);
                            let sec = self.ensure_section(&self.project.name);
                            sec.html_body
                                .push_str(&format!("<p{class}>{task_box}{html}</p>\n"));
                        }
                        first => {
                            sec.html_body.push_str(&format!("<p>{task_box}</p>\n"));
                            if let Some(first) = first {
                                self.render_block(first, sheet_dir);
                            }
                        }
                    }
                    for b in blocks {
                        self.render_block(b, sheet_dir);
                    }
                    let sec = self.ensure_section(&self.project.name);
                    sec.html_body.push_str("</li>\n");
                }
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str(&format!("</{tag}>\n"));
            }
            Block::Extension { name, args, code } => match self.embed_drawing(name, args) {
                Ok(Some(src)) => {
                    let align = match crate::extension::align_of(args) {
                        crate::extension::Align::Left => "left",
                        crate::extension::Align::Center => "center",
                        crate::extension::Align::Right => "right",
                    };
                    let sec = self.ensure_section(&self.project.name);
                    sec.html_body.push_str(&format!(
                        "<figure class=\"image align-{align} extension\">\n  <img src=\"{}\" alt=\"{}\"/>\n</figure>\n",
                        escape_xml(&src),
                        escape_xml(name)
                    ));
                }
                Ok(None) => {}
                Err(()) => self.render_block(&Block::Code(code.clone()), sheet_dir),
            },
            Block::Code(code) => {
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str("<pre><code>");
                sec.html_body.push_str(&escape_xml(code));
                sec.html_body.push_str("</code></pre>\n");
            }
            Block::Image(img) if crate::media::youtube_id(&img.destination).is_some() => {
                let id = crate::media::youtube_id(&img.destination).unwrap();
                self.youtube(img, &id);
            }
            Block::Image(img) if crate::media::kind_of(&img.destination).is_some() => {
                let kind = crate::media::kind_of(&img.destination).unwrap();
                self.media(img, kind, sheet_dir);
            }
            Block::Image(img) => {
                let resolved_path = crate::library::resolve_image_path(
                    sheet_dir,
                    Some(&self.project.dir),
                    &img.destination,
                );
                // ePub 안에서 존재하지 않는 경로를 가리키면 검증기(RSC-007)와 리더가 오류로 본다.
                // 파일을 찾지 못한 이미지는 미리보기·DOCX처럼 안내 문단으로 바꾼다.
                let src = resolved_path
                    .map(|path| self.embed_image(path))
                    .or_else(|| {
                        crate::library::is_web_address(&img.destination)
                            .then(|| img.destination.clone())
                    });
                let Some(src) = src else {
                    let sec = self.ensure_section(&self.project.name);
                    sec.html_body.push_str(&format!(
                        "<p class=\"missing-image\">[이미지를 찾을 수 없음: {}]</p>\n",
                        escape_xml(&img.destination)
                    ));
                    return;
                };

                let mut img_class = "image".to_string();
                match img.options.align {
                    crate::markdown::ImageAlign::Left => img_class.push_str(" align-left"),
                    crate::markdown::ImageAlign::Center => img_class.push_str(" align-center"),
                    crate::markdown::ImageAlign::Right => img_class.push_str(" align-right"),
                }
                if img.options.full_page {
                    img_class.push_str(" full-page");
                }

                let mut style_attr = String::new();
                if let Some(w) = img.options.width {
                    style_attr.push_str(&format!("width: {w}px; "));
                }
                if let Some(h) = img.options.height {
                    style_attr.push_str(&format!("height: {h}px; "));
                }

                let sec = self.ensure_section(&self.project.name);
                sec.html_body
                    .push_str(&format!("<figure class=\"{img_class}\">\n"));
                if style_attr.is_empty() {
                    sec.html_body.push_str(&format!(
                        "  <img src=\"{}\" alt=\"{}\"/>\n",
                        escape_xml(&src),
                        escape_xml(&img.alt)
                    ));
                } else {
                    sec.html_body.push_str(&format!(
                        "  <img src=\"{}\" alt=\"{}\" style=\"{}\"/>\n",
                        escape_xml(&src),
                        escape_xml(&img.alt),
                        style_attr.trim()
                    ));
                }
                if !img.alt.is_empty() {
                    sec.html_body.push_str(&format!(
                        "  <figcaption>{}</figcaption>\n",
                        escape_xml(&img.alt)
                    ));
                }
                sec.html_body.push_str("</figure>\n");
            }
            Block::Rule => {
                let sec = self.ensure_section(&self.project.name);
                sec.html_body.push_str("<hr/>\n");
            }
            Block::Table(table) => {
                let mut html = String::from("<table>\n");
                for (r_ix, row) in table.rows.iter().enumerate() {
                    if r_ix <= 1 {
                        html.push_str(if r_ix == 0 {
                            "<thead>\n"
                        } else {
                            "</thead>\n<tbody>\n"
                        });
                    }
                    html.push_str("<tr>\n");
                    let cell_tag = if r_ix == 0 { "th" } else { "td" };
                    for (c_ix, cell) in row.iter().enumerate() {
                        // 왼쪽은 스타일시트 기본값이라 따로 적지 않는다.
                        let align = match table.align(c_ix) {
                            ColumnAlign::Left => "",
                            ColumnAlign::Center => " style=\"text-align: center\"",
                            ColumnAlign::Right => " style=\"text-align: right\"",
                        };
                        html.push_str(&format!(
                            "  <{cell_tag}{align}>{}</{cell_tag}>\n",
                            render_rich(cell, &mut self.notes)
                        ));
                    }
                    html.push_str("</tr>\n");
                }
                html.push_str(if table.rows.len() > 1 {
                    "</tbody>\n"
                } else {
                    "</thead>\n"
                });
                html.push_str("</table>\n");
                self.ensure_section(&self.project.name)
                    .html_body
                    .push_str(&html);
            }
            // 각주마다 `aside`(EPUB 3 footnote). 팝업을 지원하는 리더는 본문 흐름에서 감추고 참조를
            // 누르면 띄운다. 그 밖의 리더는 시트 끝에 보여 준다. 번호는 본문으로 돌아가는 링크다.
            Block::Footnotes(notes) => {
                for (num, body) in notes {
                    let (id, back) = (self.notes.note_id(*num), self.notes.ref_id(*num));
                    let sec = self.ensure_section(&self.project.name);
                    sec.html_body.push_str(&format!(
                        "<aside id=\"{id}\" class=\"footnote\" epub:type=\"footnote\">\n"
                    ));
                    let body_start = sec.html_body.len();
                    for b in body {
                        self.render_block(b, sheet_dir);
                    }
                    let number = format!(
                        "<a class=\"footnote-back\" role=\"doc-backlink\" href=\"#{back}\">{num}</a> "
                    );
                    let sec = self.ensure_section(&self.project.name);
                    // 번호는 첫 문단 머리에 넣는다. 첫 블록이 문단이 아니면 번호만 한 줄로 둔다.
                    if sec.html_body[body_start..].starts_with("<p>") {
                        sec.html_body.insert_str(body_start + "<p>".len(), &number);
                    } else {
                        sec.html_body
                            .insert_str(body_start, &format!("<p>{}</p>\n", number.trim_end()));
                    }
                    sec.html_body.push_str("</aside>\n");
                }
            }
        }
    }
}

/// 프로젝트 원고를 ePub 파일로 저장한다. 완성된 임시 파일로 목적 파일을 교체한다.
pub fn export_to_file(project_dir: &Path, output_path: &Path) -> Result<(), ExportError> {
    let project = load_project(project_dir)?;
    save_to_file(&project, output_path)
}

/// 선택한 그룹 또는 복수 시트의 접합 묶음만 ePub 파일로 저장한다.
pub fn export_selection_to_file(
    root: &Path,
    selected: &[PathBuf],
    output_path: &Path,
) -> Result<(), ExportError> {
    let project = super::load_selection(root, selected)?;
    save_to_file(&project, output_path)
}

fn save_to_file(project: &ExportProject, output_path: &Path) -> Result<(), ExportError> {
    let parent = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".hanji-export-{}.tmp", ulid::Ulid::generate()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        build(project, &mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        std::fs::rename(&temporary, output_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// 읽어 둔 프로젝트 원고를 ePub 아카이브로 만들어 스트림에 쓴다.
pub fn build<W: Write + Seek>(project: &ExportProject, writer: &mut W) -> Result<(), ExportError> {
    let mut zip = zip::ZipWriter::new(writer);

    // 1. mimetype: 반드시 첫 번째 파일이며 압축 없이(Stored) 저장되어야 함 (EPUB 규격 필수).
    let stored_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored_options)?;
    zip.write_all(b"application/epub+zip")?;

    let deflated_options =
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // 2. META-INF/container.xml
    zip.start_file("META-INF/container.xml", deflated_options)?;
    zip.write_all(container_xml().as_bytes())?;

    // 3. 생성 상태 초기화 및 표지 확인
    let mut builder = EpubBuilder::new(project);

    let cover_path = project
        .book
        .cover_path(&project.dir)
        .filter(|p| p.is_file());
    let has_cover = if let Some(path) = cover_path {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
            .to_ascii_lowercase();
        let mime = mime_type_for_extension(&ext);
        let zip_path = format!("images/cover.{ext}");
        builder.images.push(ImageAsset {
            id: "cover-image".to_string(),
            zip_path: zip_path.clone(),
            source: AssetSource::File(path),
            media_type: mime,
            is_cover: true,
        });
        Some(zip_path)
    } else {
        None
    };

    // 4. 시트 원고를 순회하며 섹션 구성
    for sheet in &project.sheets {
        let sheet_dir = sheet.path.parent().unwrap_or(&project.dir);
        let mut blocks = crate::markdown::manuscript_blocks(
            &sheet.content,
            project
                .styles
                .resolve(StyleTarget::Body)
                .preserve_line_breaks,
        );
        let relative = sheet
            .path
            .strip_prefix(&project.root)
            .unwrap_or(&sheet.path);
        crate::extension::resolve_inline(&mut blocks, &project.root, &relative.to_string_lossy());

        // 앞 시트와 접합되지 않은 새 시트라면 이전 섹션을 마감하여 새 파일에서 시작
        if !sheet.glued {
            builder.finish_section();
        }
        builder.notes.next_sheet();
        builder.embed_inline_drawings(&blocks);

        for block in &blocks {
            builder.render_block(block, sheet_dir);
        }
    }

    builder.finish_section();
    link_notes_across_sections(&mut builder.sections);

    // 최소 하나 이상의 섹션이 있도록 보장
    if builder.sections.is_empty() {
        builder.sections.push(Section {
            id: "sec_0001".to_string(),
            filename: "text/section_0001.xhtml".to_string(),
            title: project.book.title.clone(),
            html_body: "<p>내용이 비어 있습니다.</p>".to_string(),
        });
    }

    // 섹션 파일명과 ID를 1번부터 차례대로 정리
    for (ix, sec) in builder.sections.iter_mut().enumerate() {
        let num = ix + 1;
        let old_filename = sec.filename.clone();
        let new_filename = format!("text/section_{num:04}.xhtml");
        sec.id = format!("sec_{num:04}");
        sec.filename = new_filename.clone();
        for item in &mut builder.toc_items {
            if item.href.starts_with(&old_filename) {
                item.href = item.href.replace(&old_filename, &new_filename);
            }
        }
    }

    // 5. CSS 스타일 생성 및 저장
    let css_content = generate_css(project);
    zip.start_file("OEBPS/style.css", deflated_options)?;
    zip.write_all(css_content.as_bytes())?;

    // 6. 표지 XHTML 생성 (표지가 있을 경우)
    if let Some(ref cover_img_path) = has_cover {
        let cover_html = generate_cover_xhtml(cover_img_path);
        zip.start_file("OEBPS/cover.xhtml", deflated_options)?;
        zip.write_all(cover_html.as_bytes())?;
    }

    // 7. 각 섹션 XHTML 저장
    for sec in &builder.sections {
        let doc = wrap_xhtml(&sec.title, &sec.html_body);
        zip.start_file(format!("OEBPS/{}", sec.filename), deflated_options)?;
        zip.write_all(doc.as_bytes())?;
    }

    // 8. 이미지 에셋 저장
    for img in &builder.images {
        zip.start_file(format!("OEBPS/{}", img.zip_path), deflated_options)?;
        match &img.source {
            AssetSource::File(path) => {
                std::io::copy(&mut File::open(path)?, &mut zip)?;
            }
            AssetSource::Bytes(bytes) => zip.write_all(bytes)?,
        }
    }

    // 9. 목차 생성 (toc.ncx)
    let toc_ncx = generate_toc_ncx(project, &builder.sections, &builder.toc_items);
    zip.start_file("OEBPS/toc.ncx", deflated_options)?;
    zip.write_all(toc_ncx.as_bytes())?;

    // 10. OEBPS/content.opf (패키지 문서)
    let content_opf = generate_content_opf(
        project,
        has_cover.is_some(),
        &builder.sections,
        &builder.images,
    );
    zip.start_file("OEBPS/content.opf", deflated_options)?;
    zip.write_all(content_opf.as_bytes())?;

    zip.finish()?;
    Ok(())
}

fn container_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#
}

fn wrap_xhtml(title: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="ko">
<head>
  <meta charset="utf-8"/>
  <title>{}</title>
  <link rel="stylesheet" type="text/css" href="../style.css"/>
</head>
<body>
{}
</body>
</html>"#,
        escape_xml(title),
        body
    )
}

fn generate_cover_xhtml(cover_img_path: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="ko">
<head>
  <meta charset="utf-8"/>
  <title>표지</title>
  <link rel="stylesheet" type="text/css" href="style.css"/>
  <style type="text/css">
    body {{
      margin: 0;
      padding: 0;
      text-align: center;
      background-color: #ffffff;
    }}
    div.cover-wrapper {{
      height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
    }}
    img.cover-image {{
      max-width: 100%;
      max-height: 100%;
      object-fit: contain;
    }}
  </style>
</head>
<body>
  <div class="cover-wrapper">
    <img class="cover-image" src="{}" alt="표지"/>
  </div>
</body>
</html>"#,
        escape_xml(cover_img_path)
    )
}

/// 접두사(`prefix`)가 설정된 제목의 HTML 텍스트를 생성한다.
fn render_rich_with_prefix(
    text: &Rich,
    style: &ResolvedStyle,
    number: usize,
    notes: &mut Notes,
) -> String {
    let mut out = String::new();
    let prefix = style.prefix_text(number);
    if !prefix.is_empty() {
        // 접두사에 넣은 줄바꿈은 XHTML에서 줄바꿈이 되도록 <br/>로 바꾼다.
        out.push_str(&escape_xml(&prefix).replace('\n', "<br/>\n"));
    }
    out.push_str(&render_rich(text, notes));
    out
}

/// 인라인 서식 텍스트를 XHTML로 변환한다. 각주 참조는 `noteref` 링크가 된다.
fn render_rich(rich: &[Inline], notes: &mut Notes) -> String {
    let mut out = String::new();
    for inline in rich {
        if let Some(number) = inline.footnote {
            out.push_str(&notes.reference(number));
            continue;
        }
        if let Some(object) = &inline.object {
            if let Some(img) = notes.drawings.get(object) {
                out.push_str(img);
            }
            continue;
        }
        let escaped = escape_xml(&inline.text).replace('\n', "<br/>\n");
        let marks = inline.marks;
        if marks == Marks::default() {
            out.push_str(&escaped);
            continue;
        }

        let mut seg = escaped;
        if marks.code {
            seg = format!("<code>{seg}</code>");
        }
        if marks.strong {
            seg = format!("<strong>{seg}</strong>");
        }
        if marks.emphasis {
            seg = format!("<em>{seg}</em>");
        }
        if marks.strike {
            seg = format!("<del>{seg}</del>");
        }
        out.push_str(&seg);
    }
    out
}

/// 각주 링크(`href="#fn…"`)의 대상이 다른 XHTML 파일에 있으면 파일 이름을 붙인다.
///
/// 제목 페이지·페이지 나눔이 시트 중간에서 파일을 나누면 참조와 각주 목록이 다른 파일에 놓인다.
fn link_notes_across_sections(sections: &mut [Section]) {
    let mut owner: HashMap<String, String> = HashMap::new();
    for sec in sections.iter() {
        let file = sec
            .filename
            .rsplit('/')
            .next()
            .unwrap_or(&sec.filename)
            .to_string();
        for (ix, _) in sec.html_body.match_indices(" id=\"fn") {
            let rest = &sec.html_body[ix + 5..];
            if let Some(end) = rest.find('"') {
                owner.insert(rest[..end].to_string(), file.clone());
            }
        }
    }
    for sec in sections.iter_mut() {
        let file = sec
            .filename
            .rsplit('/')
            .next()
            .unwrap_or(&sec.filename)
            .to_string();
        let mut out = String::with_capacity(sec.html_body.len());
        let mut rest = sec.html_body.as_str();
        while let Some(ix) = rest.find("href=\"#fn") {
            let (head, tail) = rest.split_at(ix + "href=\"".len());
            out.push_str(head);
            let id = &tail[1..tail[1..].find('"').map_or(tail.len(), |end| end + 1)];
            if let Some(target) = owner.get(id).filter(|target| **target != file) {
                out.push_str(target);
            }
            rest = tail;
        }
        out.push_str(rest);
        sec.html_body = out;
    }
}

/// 프로젝트 책 스타일(`[styles.*]`)을 반영한 CSS를 생성한다.
fn generate_css(project: &ExportProject) -> String {
    let body_style = project.styles.resolve(StyleTarget::Body);
    let part_style = project.styles.resolve(StyleTarget::Part);
    let chapter_style = project.styles.resolve(StyleTarget::Chapter);
    let section_style = project.styles.resolve(StyleTarget::Section);

    let align_str = |align: Align| match align {
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
        Align::Justify => "justify",
    };

    let font_decl = |font: &Option<String>| {
        font.as_deref()
            .map(|f| format!("font-family: \"{f}\", sans-serif;\n  "))
            .unwrap_or_default()
    };

    format!(
        r#"@charset "utf-8";

html, body {{
  margin: 0;
  padding: 0;
}}

body {{
  {body_font}font-size: {body_font_size}em;
  line-height: {body_line_height};
  text-align: {body_align};
  word-break: break-word;
}}

p {{
  margin-top: {body_space_before}em;
  margin-bottom: {body_space_after}em;
  text-indent: {body_indent}em;
}}

/* 줄 묶음(ADR-0016): 첫 줄 들여쓰기 없음 */
p.lines {{
  text-indent: 0;
}}

/* 문단 안 확장 그림(ADR-0036): 높이는 행 높이 × scale(요소의 style), 글자 아래에 맞춘다 */
img.inline-extension {{
  width: auto;
  vertical-align: text-bottom;
}}

.blank-line {{
  margin: 0;
  padding: 0;
  text-indent: 0;
  height: {body_line_height}em;
  break-inside: avoid;
}}

/* 별도 제목 페이지 (도비라) 컨테이너 */
.title-page {{
  box-sizing: border-box;
  min-height: 80vh;
  display: flex;
  flex-direction: column;
  justify-content: center;
  page-break-before: always;
  page-break-after: always;
  margin: 0;
  padding: 0;
}}

.title-page.vertical-top {{
  justify-content: flex-start;
  padding-top: 5em;
}}

.title-page.vertical-center {{
  justify-content: center;
}}

.title-page.vertical-bottom {{
  justify-content: flex-end;
  padding-bottom: 5em;
}}

/* 부 제목 (H1) */
h1.title-part, h1 {{
  {part_font}font-size: {part_font_size}em;
  font-weight: {part_font_weight};
  line-height: {part_line_height};
  text-align: {part_align};
  margin-top: {part_space_before}em;
  margin-bottom: {part_space_after}em;
  text-indent: 0;
}}

/* 장 제목 (H2) */
h2.title-chapter, h2 {{
  {chapter_font}font-size: {chapter_font_size}em;
  font-weight: {chapter_font_weight};
  line-height: {chapter_line_height};
  text-align: {chapter_align};
  margin-top: {chapter_space_before}em;
  margin-bottom: {chapter_space_after}em;
  text-indent: 0;
}}

/* 절 제목 (H3) */
h3.title-section, h3 {{
  {section_font}font-size: {section_font_size}em;
  font-weight: {section_font_weight};
  line-height: {section_line_height};
  text-align: {section_align};
  margin-top: {section_space_before}em;
  margin-bottom: {section_space_after}em;
  text-indent: 0;
}}

h4, h5, h6 {{
  margin-top: 1.2em;
  margin-bottom: 0.4em;
  text-indent: 0;
}}

blockquote {{
  margin: 1.2em 1.5em;
  padding-left: 1em;
  border-left: 3px solid #ccc;
  color: #555;
}}

pre, code {{
  font-family: monospace, monospace;
  font-size: 0.9em;
  background-color: #f7f7f7;
}}

pre {{
  padding: 0.8em;
  overflow-x: auto;
  border-radius: 4px;
}}

code {{
  padding: 0.1em 0.3em;
  border-radius: 3px;
}}

figure.image {{
  margin: 1.5em 0;
  text-align: center;
}}

figure.image img {{
  max-width: 100%;
  height: auto;
}}

figure.image figcaption {{
  font-size: 0.85em;
  color: #666;
  margin-top: 0.4em;
}}

li.task {{
  list-style: none;
}}

figure.video, figure.audio {{
  margin: 1.5em 0;
}}

figure.video video {{
  max-width: 100%;
  height: auto;
}}

figure.youtube img {{
  max-width: 100%;
  height: auto;
}}

figure.video video img {{
  max-width: 100%;
  height: auto;
}}

figure.audio audio {{
  width: 100%;
  max-width: 30em;
}}

figure.video figcaption, figure.audio figcaption {{
  font-size: 0.85em;
  color: #666;
  margin-top: 0.4em;
}}

p.media-link {{
  margin: 1em 0;
}}

p.missing-image {{
  font-size: 0.85em;
  color: #666;
  margin: 1em 0;
}}

figure.align-left {{
  text-align: left;
}}

figure.align-center {{
  text-align: center;
}}

figure.align-right {{
  text-align: right;
}}

figure.full-page {{
  height: 95vh;
  display: flex;
  align-items: center;
  justify-content: center;
}}

hr {{
  border: 0;
  border-top: 1px solid #ddd;
  margin: 2em 0;
}}

table {{
  border-collapse: collapse;
  width: 100%;
  margin: 1.5em 0;
}}

th, td {{
  border: 1px solid #ddd;
  padding: 0.5em 0.8em;
  text-align: left;
}}

th {{
  background-color: #f5f5f5;
  font-weight: bold;
}}

aside.footnote {{
  margin-top: 1em;
  font-size: 0.85em;
}}

aside.footnote p {{
  text-indent: 0;
}}

a.noteref, a.footnote-back {{
  text-decoration: none;
}}
"#,
        body_font = font_decl(&body_style.font),
        body_font_size = body_style.font_size,
        body_line_height = body_style.line_height,
        body_align = align_str(body_style.align),
        body_indent = body_style.first_line_indent,
        body_space_before = body_style.space_before,
        body_space_after = body_style.space_after,
        part_font = font_decl(&part_style.font),
        part_font_size = part_style.font_size * 1.6,
        part_font_weight = part_style.font_weight,
        part_line_height = part_style.line_height,
        part_align = align_str(part_style.align),
        part_space_before = part_style.space_before,
        part_space_after = part_style.space_after,
        chapter_font = font_decl(&chapter_style.font),
        chapter_font_size = chapter_style.font_size * 1.3,
        chapter_font_weight = chapter_style.font_weight,
        chapter_line_height = chapter_style.line_height,
        chapter_align = align_str(chapter_style.align),
        chapter_space_before = chapter_style.space_before,
        chapter_space_after = chapter_style.space_after,
        section_font = font_decl(&section_style.font),
        section_font_size = section_style.font_size * 1.1,
        section_font_weight = section_style.font_weight,
        section_line_height = section_style.line_height,
        section_align = align_str(section_style.align),
        section_space_before = section_style.space_before,
        section_space_after = section_style.space_after,
    )
}

/// EPUB NCX (`toc.ncx`) 생성.
fn generate_toc_ncx(
    project: &ExportProject,
    sections: &[Section],
    toc_items: &[TocItem],
) -> String {
    let title = if project.book.title.is_empty() {
        &project.name
    } else {
        &project.book.title
    };
    let identifier = if project.book.identifier.is_empty() {
        "urn:uuid:hanji-book"
    } else {
        &project.book.identifier
    };

    let mut nav_points = String::new();
    let mut play_order = 1;

    if toc_items.is_empty() {
        for sec in sections {
            nav_points.push_str(&format!(
                r#"    <navPoint id="navPoint-{play_order}" playOrder="{play_order}">
      <navLabel><text>{}</text></navLabel>
      <content src="{}"/>
    </navPoint>
"#,
                escape_xml(&toc_label(&sec.title)),
                escape_xml(&sec.filename)
            ));
            play_order += 1;
        }
    } else {
        for item in toc_items {
            nav_points.push_str(&format!(
                r#"    <navPoint id="navPoint-{}-{play_order}" playOrder="{play_order}">
      <navLabel><text>{}</text></navLabel>
      <content src="{}"/>
    </navPoint>
"#,
                escape_xml(&item.id),
                escape_xml(&toc_label(&item.title)),
                escape_xml(&item.href)
            ));
            play_order += 1;
        }
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="{}"/>
    <meta name="dtb:depth" content="2"/>
    <meta name="dtb:totalPageCount" content="0"/>
    <meta name="dtb:maxPageNumber" content="0"/>
  </head>
  <docTitle>
    <text>{}</text>
  </docTitle>
  <navMap>
{nav_points}  </navMap>
</ncx>"#,
        escape_xml(identifier),
        escape_xml(title)
    )
}

/// `OEBPS/content.opf` (패키지 문서) 생성.
fn generate_content_opf(
    project: &ExportProject,
    has_cover: bool,
    sections: &[Section],
    images: &[ImageAsset],
) -> String {
    let title = if project.book.title.is_empty() {
        &project.name
    } else {
        &project.book.title
    };
    let lang = if project.book.language.is_empty() {
        "ko"
    } else {
        &project.book.language
    };
    let identifier = if project.book.identifier.is_empty() {
        "urn:uuid:hanji-book"
    } else {
        &project.book.identifier
    };

    let mut creators = String::new();
    for (i, author) in project.book.authors.iter().enumerate() {
        creators.push_str(&format!(
            "    <dc:creator id=\"creator_{i}\">{}</dc:creator>\n",
            escape_xml(author)
        ));
    }
    if creators.is_empty() {
        creators.push_str("    <dc:creator id=\"creator_0\">작가 미상</dc:creator>\n");
    }

    let description_tag = if !project.book.description.is_empty() {
        format!(
            "    <dc:description>{}</dc:description>\n",
            escape_xml(&project.book.description)
        )
    } else {
        String::new()
    };

    let cover_meta = if has_cover {
        "    <meta name=\"cover\" content=\"cover-image\"/>\n"
    } else {
        ""
    };

    // manifest 생성
    let mut manifest = String::new();
    manifest.push_str("    <item id=\"style\" href=\"style.css\" media-type=\"text/css\"/>\n");
    manifest.push_str(
        "    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n",
    );

    if has_cover {
        manifest.push_str(
            "    <item id=\"cover\" href=\"cover.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
        );
    }

    for sec in sections {
        manifest.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            escape_xml(&sec.id),
            escape_xml(&sec.filename)
        ));
    }

    for img in images {
        let prop = if img.is_cover {
            " properties=\"cover-image\""
        } else {
            ""
        };
        manifest.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"{}\"{prop}/>\n",
            escape_xml(&img.id),
            escape_xml(&img.zip_path),
            img.media_type
        ));
    }

    // spine 생성
    let mut spine = String::new();
    if has_cover {
        spine.push_str("    <itemref idref=\"cover\"/>\n");
    }
    for sec in sections {
        spine.push_str(&format!(
            "    <itemref idref=\"{}\"/>\n",
            escape_xml(&sec.id)
        ));
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="BookId" xml:lang="{lang}">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="BookId">{}</dc:identifier>
    <dc:title>{}</dc:title>
    <dc:language>{}</dc:language>
{creators}{description_tag}    <meta property="dcterms:modified">2026-10-21T00:00:00Z</meta>
{cover_meta}  </metadata>
  <manifest>
{manifest}  </manifest>
  <spine toc="ncx">
{spine}  </spine>
</package>"#,
        escape_xml(identifier),
        escape_xml(title),
        escape_xml(lang)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ExportSheet;
    use crate::library::{BookMeta, Styles};
    use std::io::Cursor;

    #[test]
    fn epub_builds_valid_archive() {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장 ".to_string());
        styles.part.placement = Some(Placement::Page);
        styles.part.page_break = Some(true);
        styles.chapter.page_break = Some(true);

        let project = ExportProject {
            root: Default::default(),
            name: "테스트 프로젝트".to_string(),
            dir: PathBuf::from("fake/project"),
            book: BookMeta {
                title: "나의 소설".to_string(),
                authors: vec!["김작가".to_string()],
                language: "ko".to_string(),
                description: "재미있는 이야기".to_string(),
                cover: None,
                identifier: "urn:uuid:12345678-1234-1234-1234-123456789abc".to_string(),
            },
            styles,
            sheets: vec![
                ExportSheet {
                    path: PathBuf::from("fake/project/01.md"),
                    glued: false,
                    content: "# 1부 시작\n\n부 제목을 별도 제목 페이지(도비라)로 둔다.\n"
                        .to_string(),
                },
                ExportSheet {
                    path: PathBuf::from("fake/project/02.md"),
                    glued: true,
                    content: "## 첫 이야기\n\n이 문장은 앞 시트에 이어진 본문이다.\n".to_string(),
                },
                ExportSheet {
                    path: PathBuf::from("fake/project/03.md"),
                    glued: false,
                    content: "## 두 번째 이야기\n\n모험을 떠났습니다.\n".to_string(),
                },
            ],
        };

        let mut buffer = Cursor::new(Vec::new());
        build(&project, &mut buffer).unwrap();

        let bytes = buffer.into_inner();
        assert!(!bytes.is_empty());

        let mut reader = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();

        // 1. mimetype 확인
        {
            let mut mimetype = reader.by_name("mimetype").unwrap();
            let mut mime_str = String::new();
            std::io::Read::read_to_string(&mut mimetype, &mut mime_str).unwrap();
            assert_eq!(mime_str, "application/epub+zip");
            assert_eq!(mimetype.compression(), CompressionMethod::Stored);
        }

        // 2. 주요 파일 존재 확인 (별도 nav 페이지 없이 toc.ncx를 목차로 사용)
        assert!(reader.by_name("META-INF/container.xml").is_ok());
        assert!(reader.by_name("OEBPS/content.opf").is_ok());
        assert!(reader.by_name("OEBPS/style.css").is_ok());
        assert!(reader.by_name("OEBPS/toc.ncx").is_ok());
        assert!(reader.by_name("OEBPS/nav.xhtml").is_err());

        // 3. 섹션 파일 구조 검증:
        // - 01.md의 "# 1부 시작"은 Placement::Page 이므로 독립된 title-page 섹션 생성
        // - 그 뒤의 "부 제목은..." 본문은 다음 섹션
        // - "## 첫 이야기"는 page_break에 따라 섹션 분할
        assert!(reader.by_name("OEBPS/text/section_0001.xhtml").is_ok());
        assert!(reader.by_name("OEBPS/text/section_0002.xhtml").is_ok());

        // 4. 별도 제목 페이지(title-page) 및 접두사 확인
        let mut title_page = reader.by_name("OEBPS/text/section_0001.xhtml").unwrap();
        let mut title_content = String::new();
        std::io::Read::read_to_string(&mut title_page, &mut title_content).unwrap();
        assert!(title_content.contains("class=\"title-page vertical-center\""));
        assert!(title_content.contains("class=\"title-part\""));
        assert!(title_content.contains("1부 시작"));

        // 5. 목차(toc.ncx) 접두사("제1장 첫 이야기") 반영 확인
        drop(title_page);
        let mut ncx_file = reader.by_name("OEBPS/toc.ncx").unwrap();
        let mut ncx_content = String::new();
        std::io::Read::read_to_string(&mut ncx_file, &mut ncx_content).unwrap();
        assert!(ncx_content.contains("제1장 첫 이야기"));
    }

    /// 원고 시트들로 ePub을 만들어 본문 XHTML 파일(이름, 내용)을 돌려준다.
    fn sections_of(sheets: &[(&str, bool)], styles: Styles) -> Vec<(String, String)> {
        let project = ExportProject {
            root: Default::default(),
            name: "각주".to_string(),
            dir: PathBuf::from("fake/project"),
            book: BookMeta {
                title: "각주".to_string(),
                authors: Vec::new(),
                language: "ko".to_string(),
                description: String::new(),
                cover: None,
                identifier: "urn:uuid:00000000-0000-0000-0000-000000000000".to_string(),
            },
            styles,
            sheets: sheets
                .iter()
                .enumerate()
                .map(|(ix, (content, glued))| ExportSheet {
                    path: PathBuf::from(format!("fake/project/{ix}.md")),
                    glued: *glued,
                    content: content.to_string(),
                })
                .collect(),
        };
        let mut buffer = Cursor::new(Vec::new());
        build(&project, &mut buffer).unwrap();
        let mut reader = zip::ZipArchive::new(Cursor::new(buffer.into_inner())).unwrap();
        let names: Vec<String> = reader
            .file_names()
            .filter(|name| name.starts_with("OEBPS/text/"))
            .map(str::to_string)
            .collect();
        let mut out: Vec<(String, String)> = names
            .into_iter()
            .map(|name| {
                let mut content = String::new();
                std::io::Read::read_to_string(&mut reader.by_name(&name).unwrap(), &mut content)
                    .unwrap();
                (name.rsplit('/').next().unwrap().to_string(), content)
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn footnotes_are_epub3_noterefs_with_unique_ids() {
        // 이어 붙은 두 시트가 한 파일에 들어가도 각주 id가 겹치지 않는다.
        let sections = sections_of(
            &[
                ("가[^a] 또[^a]\n\n[^a]: 에이\n", false),
                ("나[^b]\n\n[^b]: 비\n", true),
            ],
            Styles::default(),
        );
        assert_eq!(sections.len(), 1);
        let html = &sections[0].1;
        assert!(html.contains(
            r##"<a epub:type="noteref" role="doc-noteref" class="noteref" href="#fn-1-1" id="fnref-1-1"><sup>1</sup></a>"##
        ));
        // 같은 각주를 다시 참조하면 돌아올 자리(id)는 처음 것에만 있다.
        assert_eq!(html.matches(r#"id="fnref-1-1""#).count(), 1);
        assert_eq!(html.matches(r##"href="#fn-1-1""##).count(), 2);
        assert!(html.contains(
            r##"<aside id="fn-1-1" class="footnote" epub:type="footnote">
<p><a class="footnote-back" role="doc-backlink" href="#fnref-1-1">1</a> 에이</p>"##
        ));
        assert!(html.contains(r#"<aside id="fn-2-1""#));
        assert!(html.contains(r##"href="#fn-2-1""##));
    }

    #[test]
    fn footnote_links_cross_files_split_by_page_break() {
        let mut styles = Styles::default();
        styles.chapter.page_break = Some(true);
        // 참조는 장 제목 앞 파일에, 각주 목록은 시트 끝(장 제목 뒤 파일)에 놓인다.
        let sections = sections_of(
            &[("본문[^a]\n\n## 장\n\n다음\n\n[^a]: 내용\n", false)],
            styles,
        );
        assert_eq!(sections.len(), 2);
        let (first, second) = (&sections[0], &sections[1]);
        assert!(first.1.contains(&format!(r#"href="{}#fn-1-1""#, second.0)));
        assert!(
            second
                .1
                .contains(&format!(r#"href="{}#fnref-1-1""#, first.0))
        );
    }

    #[test]
    fn prefix_newline_becomes_line_break() {
        let mut style = Styles::default().resolve(StyleTarget::Chapter);
        style.prefix = "제{n}장\n".to_string();
        let html = render_rich_with_prefix(&Rich::new(), &style, 2, &mut Notes::default());
        assert_eq!(html, "제2장<br/>\n");
    }

    #[test]
    fn toc_flattens_prefix_newline() {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장\n".to_string());

        let project = ExportProject {
            root: Default::default(),
            name: "테스트".to_string(),
            dir: PathBuf::from("fake/project"),
            book: BookMeta::default(),
            styles,
            sheets: vec![ExportSheet {
                path: PathBuf::from("fake/project/01.md"),
                glued: false,
                content: "## 첫 이야기\n\n본문.\n".to_string(),
            }],
        };

        let mut buffer = Cursor::new(Vec::new());
        build(&project, &mut buffer).unwrap();

        let mut reader = zip::ZipArchive::new(Cursor::new(buffer.into_inner())).unwrap();
        let mut ncx = reader.by_name("OEBPS/toc.ncx").unwrap();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut ncx, &mut content).unwrap();
        assert!(
            content.contains("<text>제1장 첫 이야기</text>"),
            "{content}"
        );
    }

    #[test]
    fn toc_flattens_title_break() {
        let project = ExportProject {
            root: Default::default(),
            name: "테스트".to_string(),
            dir: PathBuf::from("fake/project"),
            book: BookMeta::default(),
            styles: Styles::default(),
            sheets: vec![ExportSheet {
                path: PathBuf::from("fake/project/01.md"),
                glued: false,
                content: "## 제목<br>부제\n\n본문.\n".to_string(),
            }],
        };

        let mut buffer = Cursor::new(Vec::new());
        build(&project, &mut buffer).unwrap();

        let mut reader = zip::ZipArchive::new(Cursor::new(buffer.into_inner())).unwrap();
        let mut ncx = reader.by_name("OEBPS/toc.ncx").unwrap();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut ncx, &mut content).unwrap();
        assert!(content.contains("<text>제목 부제</text>"), "{content}");
        assert!(!content.contains("제목\n부제"), "{content}");
    }

    #[test]
    fn prefix_uses_roman_numbering() {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장 ".to_string());
        styles.chapter.numbering = Some(crate::library::Numbering::Roman);
        let style = styles.resolve(StyleTarget::Chapter);
        let html = render_rich_with_prefix(&Rich::new(), &style, 4, &mut Notes::default());
        assert_eq!(html, "제IV장 ");
    }

    #[test]
    fn restart_on_parent_resets_child_numbers_in_export() {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장 ".to_string());
        styles.section.prefix = Some("{n}절 ".to_string());
        styles.chapter.restart_on_parent = Some(true);
        styles.section.restart_on_parent = Some(true);
        let sections = sections_of(
            &[(
                "# 1부\n\n## 첫째\n\n### 하나\n\n## 둘째\n\n### 둘\n\n# 2부\n\n## 다시\n\n### 다시 하나\n",
                false,
            )],
            styles,
        );
        let html = sections
            .iter()
            .map(|(_, content)| content.as_str())
            .collect::<String>();
        // 새 부 뒤 장은 1부터. 첫째와 다시가 모두 "제1장".
        assert_eq!(html.matches("제1장 ").count(), 2, "{html}");
        assert!(html.contains("제2장 둘째"), "{html}");
        // 새 장 뒤 절도 1부터.
        assert!(html.contains("1절 하나"), "{html}");
        assert!(html.contains("1절 둘"), "{html}");
        assert!(html.contains("1절 다시 하나"), "{html}");
        assert!(!html.contains("2절"), "{html}");
    }

    #[test]
    fn unnumbered_heading_skips_prefix_and_number() {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장 ".to_string());
        let sections = sections_of(
            &[
                ("## 서장 {unnumbered}\n\n본문.\n", false),
                ("## 첫 이야기\n\n본문.\n", false),
            ],
            styles,
        );
        // 서장: 접두사 없이 제목만. 다음 장이 1번을 쓴다.
        assert!(sections[0].1.contains(">서장</h2>"), "{}", sections[0].1);
        assert!(!sections[0].1.contains("제"), "{}", sections[0].1);
        assert!(
            sections[1].1.contains("제1장 첫 이야기"),
            "{}",
            sections[1].1
        );
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use crate::export::ExportSheet;
    use crate::library::test_util::temp_dir;
    use crate::library::{ATTACHMENTS_GROUP, BookMeta, EXTRAS_DIR, GROUP_META_FILE, Styles};
    use std::io::Cursor;

    /// `Projects/책/Extras/Attachments/a.png`가 있는 프로젝트와, `본문/1장`에 둔 시트로
    /// ePub을 만든다. `chapters`는 `본문/1장/<이름>.md`의 원문이다.
    fn build_with(chapters: &str) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let root = temp_dir("epub-images");
        let project_dir = root.join("Projects").join("책");
        let attachments = project_dir.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);
        std::fs::create_dir_all(&attachments).unwrap();
        let sheet_dir = project_dir.join("본문").join("1장");
        std::fs::create_dir_all(&sheet_dir).unwrap();
        for group in [&project_dir, &project_dir.join("본문"), &sheet_dir] {
            std::fs::write(group.join(GROUP_META_FILE), "").unwrap();
        }
        std::fs::write(attachments.join("a.png"), b"\x89PNG\r\n\x1a\nimage").unwrap();
        std::fs::write(attachments.join("a.mp3"), b"ID3 sound").unwrap();
        std::fs::write(attachments.join("a.mp4"), b"not really a movie").unwrap();
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        std::fs::write(
            root.join("Extensions/dot.lua"),
            "return { size = { 40, 20 }, draw = function(g, t, args) g.circle('fill', 10, 10, args.r or 5) end }",
        )
        .unwrap();

        let project = ExportProject {
            root: root.clone(),
            name: "책".to_string(),
            dir: project_dir.clone(),
            book: BookMeta {
                title: "책".to_string(),
                ..Default::default()
            },
            styles: Styles::default(),
            sheets: vec![ExportSheet {
                path: sheet_dir.join("01.md"),
                glued: false,
                content: chapters.to_string(),
            }],
        };
        let mut buffer = Cursor::new(Vec::new());
        build(&project, &mut buffer).unwrap();
        zip::ZipArchive::new(Cursor::new(buffer.into_inner())).unwrap()
    }

    fn section(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>) -> String {
        let mut file = archive.by_name("OEBPS/text/section_0001.xhtml").unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut file, &mut text).unwrap();
        text
    }

    #[test]
    fn extension_blocks_become_svg_images_or_stay_code() {
        let mut archive = build_with(
            "```ext dot\nr: 3\nscale: 2\n```\n\n```ext dot\nr: 3\nscale: 2\n```\n\n```ext missing\nr: 1\n```\n",
        );
        let html = section(&mut archive);
        // 같은 블록은 한 번만 넣는다.
        assert_eq!(html.matches("../images/ext_0001.svg").count(), 2, "{html}");
        // 없는 확장은 코드 블록으로 둔다.
        assert!(html.contains("<pre><code>r: 1"), "{html}");
        let mut svg = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("OEBPS/images/ext_0001.svg").unwrap(),
            &mut svg,
        )
        .unwrap();
        assert!(
            svg.contains(r#"width="80" height="40" viewBox="0 0 40 20""#),
            "{svg}"
        );
        let mut opf = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("OEBPS/content.opf").unwrap(), &mut opf)
            .unwrap();
        assert!(
            opf.contains(r#"href="images/ext_0001.svg" media-type="image/svg+xml""#),
            "{opf}"
        );
    }

    #[test]
    fn inline_drawings_become_images_in_the_paragraph() {
        let mut archive = build_with("앞 {{dot r=3}} 뒤 {{dot r=3 scale=2}}\n");
        let html = section(&mut archive);
        // 같은 그림(scale만 다름)은 파일 하나를 쓰고, 높이는 행 높이 × scale이다.
        assert!(
            html.contains(r#"<p>앞 <img class="inline-extension" src="../images/ext_0001.svg" alt="dot" style="height: 1.600em"/> 뒤 "#),
            "{html}"
        );
        assert!(html.contains(r#"style="height: 3.200em"/>"#), "{html}");
        assert!(archive.by_name("OEBPS/images/ext_0001.svg").is_ok());
    }

    #[test]
    fn relative_image_path_is_embedded_once() {
        // 같은 그림을 여러 번(다르게 적은 경로로) 써도 파일은 한 번만 넣고 같은 경로를 가리킨다.
        let mut archive = build_with(
            "![사진](../../Extras/Attachments/a.png){width=320px}\n\n![또](../../Extras/Attachments/a.png)\n\n![옛](../Extras/Attachments/a.png)\n",
        );
        let html = section(&mut archive);
        assert_eq!(html.matches("../images/img_0001.png").count(), 3, "{html}");
        assert!(archive.by_name("OEBPS/images/img_0001.png").is_ok());
        let names: Vec<String> = (0..archive.len())
            .map(|ix| archive.by_index(ix).unwrap().name().to_string())
            .filter(|name| name.starts_with("OEBPS/images/"))
            .collect();
        assert_eq!(names, ["OEBPS/images/img_0001.png"]);
    }

    #[test]
    fn stale_parent_image_path_is_still_embedded() {
        // 그룹을 옮겨 `../` 깊이가 어긋난 경로도 찾아서 넣는다(ADR-0019·0020).
        let mut archive = build_with("![사진](../Extras/Attachments/a.png)\n");
        let html = section(&mut archive);
        assert!(html.contains("../images/img_0001.png"), "{html}");
        assert!(archive.by_name("OEBPS/images/img_0001.png").is_ok());
    }

    #[test]
    fn missing_image_becomes_notice_instead_of_broken_link() {
        // 찾지 못한 이미지의 상대 경로를 그대로 쓰면 ePub 안에서 깨진 참조(RSC-007)가 된다.
        let mut archive = build_with("![사진](../Extras/Attachments/없는.png)\n");
        let html = section(&mut archive);
        assert!(!html.contains("src=\""), "{html}");
        assert!(
            html.contains("[이미지를 찾을 수 없음: ../Extras/Attachments/없는.png]"),
            "{html}"
        );
    }

    #[test]
    fn audio_and_video_become_playable_elements_with_their_files() {
        let mut archive = build_with(
            "![주제가](../Extras/Attachments/a.mp3)\n\n![](../Extras/Attachments/a.mp4){width=320px}\n\n![없음](없는.mov)\n",
        );
        let html = section(&mut archive);
        assert!(
            html.contains(
                r#"<audio src="../media/media_0001.mp3" controls="controls">주제가</audio>"#
            ),
            "{html}"
        );
        assert!(html.contains("<figcaption>주제가</figcaption>"), "{html}");
        // 가짜 파일이라 포스터는 없다.
        assert!(
            html.contains(r#"<video src="../media/media_0002.mp4" controls="controls" style="width: 320px;"></video>"#),
            "{html}"
        );
        assert!(html.contains("[미디어를 찾을 수 없음: 없는.mov]"), "{html}");
        assert!(archive.by_name("OEBPS/media/media_0001.mp3").is_ok());
        assert!(archive.by_name("OEBPS/media/media_0002.mp4").is_ok());
        let mut opf = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("OEBPS/content.opf").unwrap(), &mut opf)
            .unwrap();
        assert!(
            opf.contains(r#"href="media/media_0001.mp3" media-type="audio/mpeg""#),
            "{opf}"
        );
        assert!(
            opf.contains(r#"href="media/media_0002.mp4" media-type="video/mp4""#),
            "{opf}"
        );
    }

    #[test]
    fn youtube_becomes_a_link_without_the_player() {
        // 시험에서는 썸네일을 내려받지 않으므로 링크 문단이다.
        let mut archive = build_with("![예고편](https://youtu.be/dQw4w9WgXcQ)\n");
        let html = section(&mut archive);
        assert!(
            html.contains(r#"<a href="https://www.youtube.com/watch?v=dQw4w9WgXcQ">▶ 예고편</a>"#),
            "{html}"
        );
        assert!(!html.contains("iframe"), "{html}");
    }

    #[test]
    fn task_items_show_check_boxes() {
        let mut archive = build_with("- [ ] 장보기\n- [x] 일정\n- 그냥 항목\n");
        let html = section(&mut archive);
        assert!(
            html.contains("<li class=\"task\">\n<p><span class=\"task-box\" role=\"img\" aria-label=\"할 일\">☐</span> 장보기</p>"),
            "{html}"
        );
        assert!(
            html.contains("aria-label=\"완료\">☑</span> 일정</p>"),
            "{html}"
        );
        assert!(html.contains("<li>\n<p>그냥 항목</p>"), "{html}");
        assert!(!html.contains("[x]"), "{html}");
    }

    #[test]
    fn web_image_keeps_its_address() {
        // 프로젝트 밖(메모)에서 쓴 주소 이미지는 파일이 없어도 주소를 그대로 쓴다.
        let mut archive = build_with("![사진](https://x.io/a.png)\n");
        let html = section(&mut archive);
        assert!(html.contains("src=\"https://x.io/a.png\""), "{html}");
    }
}
