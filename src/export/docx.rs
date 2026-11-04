//! DOCX 빌더: 프로젝트 원고를 Word 문서(.docx, OOXML)로 만든다(ADR-0015).
//!
//! DOCX는 흐르는 문서라 쪽 나눔·줄바꿈은 Word가 한다. 미리보기 배치(PDF)를 옮기지 않고, ePub처럼
//! 원고 블록을 Word 구조로 바꾼다. 받는 사람이 고쳐 쓸 수 있게 책 스타일은 Word 스타일로 둔다.
//!
//! - 본문 → "Normal", 부·장·절(`#`~`###`) → "Heading1~3"(개요 수준 0~2, 목차·탐색 창에 쓰임).
//!   목차에서 뺀 제목은 개요 수준을 본문으로 바꾼다.
//! - 새 페이지에서 시작 → `pageBreakBefore`. 별도 제목 페이지 → 구역을 나누고 구역 세로 맞춤.
//!   숨긴 제목은 쓰지 않는다(쪽 나눔만 남는다).
//! - 접두사·번호(`{n}`)는 계산한 글자로 넣는다(Word 자동 번호를 쓰지 않는다).
//! - 각주는 Word 각주(같은 쪽 아래)로 넣는다. 본문의 위 첨자 번호 자리가 각주 참조가 된다.
//! - 목차 생성 → Word 목차 필드. 문서를 열 때 Word가 쪽 번호를 채운다(`updateFields`).
//! - 접합하지 않은 시트는 새 페이지에서 시작한다(ePub·PDF와 같은 규칙).
//! - 서체는 이름만 적는다(넣지 않는다).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;

use super::{ExportError, ExportProject};
use crate::library::{Align, HeadingNumbers, Placement, ResolvedStyle, StyleTarget, VerticalAlign};
use crate::markdown::{
    Block, ColumnAlign, ImageAlign, ImageLine, Inline, InlineObject, Rich, Table,
};

/// 판형과 서체(컴파일 대화상자에서 정한다). 길이는 pt.
#[derive(Clone, Debug)]
pub struct DocxOptions {
    pub page_width: f32,
    pub page_height: f32,
    pub margin_top: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    /// 본문 글자 크기(em 기준).
    pub font_size: f32,
    /// 스타일에 서체가 없을 때 쓰는 서체(설정의 에디터 서체). 없으면 Word 기본 서체.
    pub default_font: Option<String>,
    pub mono_font: String,
    /// 차례(Word 목차 필드)를 넣는지.
    pub generate_toc: bool,
}

impl Default for DocxOptions {
    /// A4, 본문 11pt.
    fn default() -> Self {
        Self {
            page_width: 595.3,
            page_height: 841.9,
            margin_top: 72.,
            margin_right: 72.,
            margin_bottom: 72.,
            margin_left: 72.,
            font_size: 11.,
            default_font: None,
            mono_font: "Menlo".into(),
            generate_toc: false,
        }
    }
}

/// 프로젝트 원고를 DOCX 파일로 저장한다. 완성된 임시 파일로 목적 파일을 바꿔치기한다.
pub fn save_to_file(
    project: &ExportProject,
    options: &DocxOptions,
    output_path: &Path,
) -> Result<(), ExportError> {
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
        build(project, options, &mut writer)?;
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

/// 읽어 둔 원고를 DOCX 아카이브로 만들어 스트림에 쓴다.
pub fn build<W: Write + Seek>(
    project: &ExportProject,
    options: &DocxOptions,
    writer: &mut W,
) -> Result<(), ExportError> {
    let mut doc = DocBuilder::new(project, options);
    doc.front_matter();
    let preserve = project
        .styles
        .resolve(StyleTarget::Body)
        .preserve_line_breaks;
    for sheet in &project.sheets {
        let dir = sheet.path.parent().unwrap_or(&project.dir).to_path_buf();
        let mut blocks = crate::markdown::manuscript_blocks(&sheet.content, preserve);
        let relative = sheet
            .path
            .strip_prefix(&project.root)
            .unwrap_or(&sheet.path);
        crate::extension::resolve_inline(&mut blocks, &project.root, &relative.to_string_lossy());
        if !sheet.glued {
            doc.pending_break = true;
        }
        // 시트의 각주는 Word 각주로 옮긴다. 본문의 위 첨자 번호가 참조 자리다.
        doc.sheet_notes = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Footnotes(notes) => Some(notes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        doc.dir = dir;
        for block in &blocks {
            doc.block(block, Context::default());
        }
    }
    let body = doc.finish_body();

    let mut zip = zip::ZipWriter::new(writer);
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, content: &[u8]| -> Result<(), ExportError> {
        zip.start_file(name, deflated)?;
        zip.write_all(content)?;
        Ok(())
    };
    put("[Content_Types].xml", content_types(&doc.images).as_bytes())?;
    put("_rels/.rels", ROOT_RELS.as_bytes())?;
    put("docProps/core.xml", core_xml(project).as_bytes())?;
    put("word/document.xml", body.as_bytes())?;
    put("word/styles.xml", doc.styles_xml().as_bytes())?;
    put(
        "word/settings.xml",
        settings_xml(options.generate_toc).as_bytes(),
    )?;
    put("word/footnotes.xml", doc.footnotes_xml().as_bytes())?;
    put("word/footer1.xml", FOOTER_XML.as_bytes())?;
    put(
        "word/_rels/document.xml.rels",
        document_rels(&doc.images).as_bytes(),
    )?;
    for image in &doc.images {
        put(&format!("word/{}", image.name), &image.data)?;
        if let Some((_, name, svg)) = &image.svg {
            put(&format!("word/{name}"), svg)?;
        }
    }
    zip.finish()?;
    Ok(())
}

// --- 단위 -----------------------------------------------------------------

/// pt → twip(1/20pt).
fn twips(pt: f32) -> i64 {
    (pt * 20.).round() as i64
}

/// pt → EMU(그림 크기).
fn emu(pt: f32) -> i64 {
    (pt * 12700.).round() as i64
}

/// 이미지 속성의 px → pt(96dpi).
const PT_PER_PX: f32 = 0.75;

/// 확장 그림 PNG의 해상도(캔버스 px당 픽셀). 인쇄에 쓸 만하게 3배(약 288dpi)로 그린다.
const DRAWING_RESOLUTION: f32 = 3.0;

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // XML 1.0에 쓸 수 없는 제어 문자는 버린다.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            _ => out.push(c),
        }
    }
    out
}

// --- 문서 ---------------------------------------------------------------

/// 넣을 이미지.
struct ImagePart {
    rel_id: String,
    /// `media/imageN.ext`.
    name: String,
    extension: &'static str,
    data: Vec<u8>,
    width: u32,
    height: u32,
    /// 같은 그림의 SVG(관계 id, `media/imageN.svg`, 내용). Word 2016 이상은 이것을 벡터로 보이고, 다른 앱은
    /// PNG를 쓴다(확장 그림).
    svg: Option<(String, String, Vec<u8>)>,
    /// YouTube 동영상 ID(ADR-0051). 있으면 그림을 Word의 온라인 비디오로 둔다(누르면 Word 안에서 재생).
    video: Option<String>,
}

/// 확장 블록 (이름, 인자).
type DrawingKey = (String, Vec<(String, String)>);

/// 블록을 놓는 자리.
#[derive(Clone, Copy, Default)]
struct Context {
    /// 왼쪽 들여쓰기(em).
    indent: f32,
    /// 인용 안.
    quote: bool,
    /// 목록·각주 안이라 첫 줄 들여쓰기를 쓰지 않는다.
    no_first_indent: bool,
}

struct DocBuilder<'a> {
    project: &'a ExportProject,
    options: &'a DocxOptions,
    body: String,
    /// 다음 단락을 새 페이지에서 시작한다(시트 경계·앞 부속 뒤·별도 제목 페이지 뒤).
    pending_break: bool,
    /// 부(0)·장(1)·절(2) 번호.
    numbers: HeadingNumbers,
    dir: PathBuf,
    sheet_notes: Vec<(usize, Vec<Block>)>,
    /// Word 각주(id, 내용 XML). id는 1부터.
    footnotes: Vec<String>,
    images: Vec<ImagePart>,
    image_ids: HashMap<PathBuf, usize>,
    /// 확장 블록 (이름, 인자) → 넣은 그림(그리지 못했으면 `None`).
    /// 확장 그림 → 넣은 그림 번호. `Ok(None)`은 내보내기에서 빼는 블록, `Err`는 그리지 못한 블록이다.
    drawing_ids: HashMap<DrawingKey, Result<Option<usize>, ()>>,
    /// 첫 본문 구역인지(쪽 번호를 1부터 시작).
    first_body_section: bool,
}

impl<'a> DocBuilder<'a> {
    fn new(project: &'a ExportProject, options: &'a DocxOptions) -> Self {
        Self {
            project,
            options,
            body: String::new(),
            pending_break: false,
            numbers: HeadingNumbers::default(),
            dir: project.dir.clone(),
            sheet_notes: Vec::new(),
            footnotes: Vec::new(),
            images: Vec::new(),
            image_ids: HashMap::new(),
            drawing_ids: HashMap::new(),
            first_body_section: true,
        }
    }

    fn content_width(&self) -> f32 {
        self.options.page_width - self.options.margin_left - self.options.margin_right
    }

    fn content_height(&self) -> f32 {
        self.options.page_height - self.options.margin_top - self.options.margin_bottom
    }

    fn em(&self, value: f32) -> f32 {
        self.options.font_size * value
    }

    // --- 구역 --------------------------------------------------------------

    /// 구역 속성. `body`면 쪽 번호 꼬리말을 단다.
    fn sect_pr(&mut self, body: bool, v_align: VerticalAlign) -> String {
        let o = self.options;
        let footer = if body {
            r#"<w:footerReference w:type="default" r:id="rIdFooter1"/>"#
        } else {
            ""
        };
        let numbering = if body && self.first_body_section {
            self.first_body_section = false;
            r#"<w:pgNumType w:start="1"/>"#
        } else {
            ""
        };
        let v_align = match v_align {
            VerticalAlign::Top => "top",
            VerticalAlign::Center => "center",
            VerticalAlign::Bottom => "bottom",
        };
        format!(
            r#"<w:sectPr>{footer}<w:type w:val="nextPage"/><w:pgSz w:w="{}" w:h="{}"/><w:pgMar w:top="{}" w:right="{}" w:bottom="{}" w:left="{}" w:header="{}" w:footer="{}" w:gutter="0"/>{numbering}<w:vAlign w:val="{v_align}"/></w:sectPr>"#,
            twips(o.page_width),
            twips(o.page_height),
            twips(o.margin_top),
            twips(o.margin_right),
            twips(o.margin_bottom),
            twips(o.margin_left),
            twips(o.margin_top / 2.),
            twips(o.margin_bottom / 2.),
        )
    }

    /// 지금까지를 한 구역으로 닫는다(빈 단락에 구역 속성을 단다). 다음 내용은 새 페이지다.
    fn close_section(&mut self, body: bool, v_align: VerticalAlign) {
        let sect = self.sect_pr(body, v_align);
        self.body.push_str(&format!(
            r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/>{sect}</w:pPr></w:p>"#
        ));
        self.pending_break = false;
    }

    // --- 단락 ---------------------------------------------------------------

    /// 단락 하나. `ppr`은 스타일 밖의 단락 속성이다. 미뤄 둔 쪽 나눔을 여기서 건다.
    fn paragraph(&mut self, style: &str, ppr: &str, runs: &str) {
        let page_break = if std::mem::take(&mut self.pending_break) {
            "<w:pageBreakBefore/>"
        } else {
            ""
        };
        self.body.push_str(&format!(
            r#"<w:p><w:pPr><w:pStyle w:val="{style}"/>{page_break}{ppr}</w:pPr>{runs}</w:p>"#
        ));
    }

    /// 글자 조각들 → 런 XML. 각주 참조 조각은 이 시트 각주가 있으면 Word 각주 참조로 바꾼다.
    fn runs(&mut self, rich: &[Inline]) -> String {
        self.runs_with(rich, false)
    }

    /// `runs`와 같고, `bold`면 모든 글자를 굵게 한다(표 머리 행).
    fn runs_with(&mut self, rich: &[Inline], bold: bool) -> String {
        let mut out = String::new();
        for inline in rich {
            if let Some(object) = &inline.object {
                if let Some(drawing) = self.inline_drawing(object) {
                    out.push_str(&format!("<w:r>{drawing}</w:r>"));
                }
                continue;
            }
            let mut rpr = String::new();
            // 글자 속성 순서는 OOXML 스키마를 따른다(rFonts → b → i → strike).
            if inline.marks.code {
                let mono = escape(&self.options.mono_font);
                rpr.push_str(&format!(
                    r#"<w:rFonts w:ascii="{mono}" w:hAnsi="{mono}" w:eastAsia="{mono}" w:cs="{mono}"/>"#
                ));
            }
            if inline.marks.strong || bold {
                rpr.push_str("<w:b/><w:bCs/>");
            }
            if inline.marks.emphasis {
                rpr.push_str("<w:i/><w:iCs/>");
            }
            if inline.marks.strike {
                rpr.push_str("<w:strike/>");
            }
            if let Some(id) = inline.footnote.and_then(|number| self.footnote(number)) {
                out.push_str(&format!(
                    r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="{id}"/></w:r>"#
                ));
                continue;
            }
            out.push_str(&text_run(&rpr, &inline.text));
        }
        out
    }

    /// 시트 각주 `number`를 Word 각주로 만들고 id를 돌려준다. 없으면 `None`.
    fn footnote(&mut self, number: usize) -> Option<usize> {
        let blocks = self
            .sheet_notes
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, blocks)| blocks.clone())?;
        // 각주 내용은 본문 대신 각주 버퍼에 쓴다.
        let saved_body = std::mem::take(&mut self.body);
        let saved_break = std::mem::take(&mut self.pending_break);
        let saved_notes = std::mem::take(&mut self.sheet_notes);
        let id = self.footnotes.len() + 1;
        let mut first = true;
        for block in &blocks {
            let runs = match block {
                Block::Paragraph(rich) | Block::Lines(rich) => self.runs(rich),
                other => {
                    self.block(
                        other,
                        Context {
                            no_first_indent: true,
                            ..Context::default()
                        },
                    );
                    continue;
                }
            };
            let reference = if first {
                r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> </w:t></w:r>"#
            } else {
                ""
            };
            first = false;
            self.body.push_str(&format!(
                r#"<w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>{reference}{runs}</w:p>"#
            ));
        }
        if first {
            self.body.push_str(r#"<w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r></w:p>"#);
        }
        let content = std::mem::replace(&mut self.body, saved_body);
        self.pending_break = saved_break;
        self.sheet_notes = saved_notes;
        self.footnotes.push(content);
        Some(id)
    }

    fn block(&mut self, block: &Block, ctx: Context) {
        match block {
            Block::Heading {
                level,
                text,
                unnumbered,
            } => self.heading(*level, text, *unnumbered),
            Block::Paragraph(rich) | Block::Lines(rich) => {
                let runs = self.runs(rich);
                let style = if ctx.quote { "Quote" } else { "Normal" };
                let mut ppr = String::new();
                // 줄 묶음은 첫 줄 들여쓰기가 없다(ADR-0016).
                if ctx.indent > 0. || ctx.no_first_indent || matches!(block, Block::Lines(_)) {
                    ppr.push_str(&format!(
                        r#"<w:ind w:left="{}" w:firstLine="0"/>"#,
                        twips(self.em(ctx.indent))
                    ));
                }
                self.paragraph(style, &ppr, &runs);
            }
            Block::BlankLines(count) => {
                for _ in 0..*count {
                    self.paragraph("Normal", r#"<w:ind w:firstLine="0"/>"#, "");
                }
            }
            Block::Quote(blocks) => {
                let inner = Context {
                    quote: true,
                    no_first_indent: true,
                    ..ctx
                };
                for block in blocks {
                    self.block(block, inner);
                }
            }
            Block::List {
                ordered,
                start,
                items,
            } => {
                for (ix, item) in items.iter().enumerate() {
                    let marker = item.marker(*ordered, *start as usize + ix);
                    self.list_item(&marker, &item.blocks, ctx);
                }
            }
            Block::Extension { name, args, code } => {
                if self.extension(name, args, ctx).is_err() {
                    self.block(&Block::Code(code.clone()), ctx);
                }
            }
            Block::Code(code) => {
                for line in code.split('\n') {
                    let runs = text_run("", &line.replace('\t', "    "));
                    let ppr = if ctx.indent > 0. {
                        format!(r#"<w:ind w:left="{}"/>"#, twips(self.em(ctx.indent + 1.)))
                    } else {
                        String::new()
                    };
                    self.paragraph("Code", &ppr, &runs);
                }
            }
            Block::Image(image) => self.image(image, ctx),
            Block::Rule => {
                self.paragraph(
                    "Normal",
                    r#"<w:spacing w:before="240" w:after="240"/><w:ind w:firstLine="0"/><w:jc w:val="center"/>"#,
                    &text_run("", "*\u{2003}*\u{2003}*"),
                );
            }
            Block::Table(table) => self.table(table),
            // 각주는 참조 자리에서 Word 각주로 넣었다.
            Block::Footnotes(_) => {}
        }
    }

    fn list_item(&mut self, marker: &str, blocks: &[Block], ctx: Context) {
        let hang = 1.6;
        let left = ctx.indent + hang;
        let inner = Context {
            indent: left,
            no_first_indent: true,
            ..ctx
        };
        let mut blocks = blocks.iter();
        let first = blocks.next();
        let first_runs = match first {
            Some(Block::Paragraph(rich) | Block::Lines(rich)) => Some(self.runs(rich)),
            _ => None,
        };
        let ppr = format!(
            r#"<w:tabs><w:tab w:val="left" w:pos="{left_tw}"/></w:tabs><w:spacing w:after="0"/><w:ind w:left="{left_tw}" w:hanging="{hang_tw}"/>"#,
            left_tw = twips(self.em(left)),
            hang_tw = twips(self.em(hang)),
        );
        let style = if ctx.quote { "Quote" } else { "ListParagraph" };
        let marker_runs = format!("{}<w:r><w:tab/></w:r>", text_run("", marker));
        match first_runs {
            Some(runs) => self.paragraph(style, &ppr, &format!("{marker_runs}{runs}")),
            None => {
                self.paragraph(style, &ppr, &marker_runs);
                if let Some(first) = first {
                    self.block(first, inner);
                }
            }
        }
        for block in blocks {
            self.block(block, inner);
        }
    }

    fn heading(&mut self, level: u8, text: &Rich, unnumbered: bool) {
        let Some(target) = StyleTarget::for_heading(level) else {
            let runs = self.runs(text);
            let style = format!("Heading{}", level.min(6));
            self.paragraph(&style, "", &runs);
            return;
        };
        let resolved = self.project.styles.resolve(target);
        // 번호는 ePub과 같게: 그리지도 않고 목차에도 없는 제목만 번호를 쓰지 않는다.
        // `{unnumbered}` 제목(시트 첫 줄만)은 번호를 소비하지 않고 접두사도 붙지 않는다(ADR-0018).
        let numbered = !unnumbered && (resolved.placement != Placement::Hidden || resolved.in_toc);
        let number = self.numbers.count(&self.project.styles, target, numbered);
        let prefix = if numbered {
            resolved.prefix_text(number)
        } else {
            String::new()
        };
        let style = format!("Heading{}", target.depth() + 1);
        // 목차에서 뺀 제목은 개요 수준을 본문으로 내려 목차·탐색 창에서 뺀다.
        let ppr = if resolved.in_toc {
            ""
        } else {
            r#"<w:outlineLvl w:val="9"/>"#
        };
        let mut runs = String::new();
        for (ix, line) in prefix.split('\n').enumerate() {
            if ix > 0 {
                runs.push_str("<w:r><w:br/></w:r>");
            }
            runs.push_str(&text_run("", line));
        }
        runs.push_str(&self.runs(text));

        match resolved.placement {
            Placement::Hidden => {
                if resolved.page_break {
                    self.pending_break = true;
                }
            }
            Placement::Inline => {
                // 새 페이지 시작은 스타일의 `pageBreakBefore`가 한다.
                if resolved.page_break {
                    self.pending_break = false;
                }
                self.paragraph(&style, ppr, &runs);
            }
            Placement::Page => {
                // 앞 내용을 본문 구역으로 닫고, 제목만 든 구역을 세로 맞춤으로 둔다.
                if !self.body.is_empty() {
                    self.close_section(true, VerticalAlign::Top);
                }
                self.pending_break = false;
                let ppr = format!(r#"<w:pageBreakBefore w:val="0"/>{ppr}"#);
                self.paragraph(&style, &ppr, &runs);
                self.close_section(false, resolved.vertical_align);
            }
        }
    }

    fn image(&mut self, image: &ImageLine, ctx: Context) {
        let caption = image.alt.trim().to_string();
        if let Some(id) = crate::media::youtube_id(&image.destination) {
            match self.load_youtube(&id) {
                Some(index) => self.place_image(index, image, &caption, ctx),
                None => {
                    let label = if caption.is_empty() {
                        "YouTube"
                    } else {
                        &caption
                    };
                    let text = format!("▶ {label} ({})", crate::media::youtube_watch_url(&id));
                    self.paragraph("Caption", "", &text_run("", &text));
                }
            }
            return;
        }
        let kind = crate::media::kind_of(&image.destination);
        let resolved = crate::library::resolve_image_path(
            &self.dir,
            Some(&self.project.dir),
            &image.destination,
        )
        .filter(|path| kind.is_none() || path.is_file());
        // 동영상·오디오(ADR-0050): Word에는 로컬 파일을 재생하는 그림이 없어 동영상은 포스터, 오디오는 제목 한 줄이다.
        if let Some(kind) = kind {
            let Some(path) = resolved else {
                self.paragraph(
                    "Caption",
                    "",
                    &text_run(
                        "",
                        &format!("[미디어를 찾을 수 없음: {}]", image.destination),
                    ),
                );
                return;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let poster = match kind {
                crate::media::MediaKind::Video => self.load_poster(&path),
                crate::media::MediaKind::Audio => None,
            };
            let Some(index) = poster else {
                let label = if caption.is_empty() { &name } else { &caption };
                let mark = match kind {
                    crate::media::MediaKind::Video => "▶",
                    crate::media::MediaKind::Audio => "♪",
                };
                self.paragraph("Caption", "", &text_run("", &format!("{mark} {label}")));
                return;
            };
            self.place_image(index, image, &caption, ctx);
            return;
        }
        let Some(path) = resolved else {
            self.paragraph(
                "Caption",
                "",
                &text_run("", &format!("[이미지: {}]", image.destination)),
            );
            return;
        };
        let Some(index) = self.load_image(&path) else {
            self.paragraph(
                "Caption",
                "",
                &text_run(
                    "",
                    &format!("[이미지를 넣을 수 없음: {}]", image.destination),
                ),
            );
            return;
        };
        self.place_image(index, image, &caption, ctx);
    }

    /// 넣은 그림(`index`)을 이미지 줄의 크기·정렬대로 한 문단에 두고 캡션을 단다.
    fn place_image(&mut self, index: usize, image: &ImageLine, caption: &str, ctx: Context) {
        let caption = caption.to_string();
        let (natural_w, natural_h) = {
            let part = &self.images[index];
            (
                part.width as f32 * PT_PER_PX,
                part.height as f32 * PT_PER_PX,
            )
        };
        let max_w = self.content_width() - self.em(ctx.indent);
        let caption_h = if caption.is_empty() { 0. } else { self.em(2.) };
        let max_h = self.content_height() - caption_h - self.em(1.);
        let options = &image.options;
        let aspect = natural_h / natural_w.max(1.);
        let (mut w, mut h) = if options.full_page {
            (max_w, max_w * aspect)
        } else {
            match (options.width, options.height) {
                (Some(w), Some(h)) => (w as f32 * PT_PER_PX, h as f32 * PT_PER_PX),
                (Some(w), None) => (w as f32 * PT_PER_PX, w as f32 * PT_PER_PX * aspect),
                (None, Some(h)) => (h as f32 * PT_PER_PX / aspect, h as f32 * PT_PER_PX),
                (None, None) => (natural_w, natural_h),
            }
        };
        let scale = (max_w / w).min(max_h / h).min(1.);
        w *= scale;
        h *= scale;
        let jc = match (options.full_page, options.align) {
            (true, _) | (_, ImageAlign::Center) => "center",
            (_, ImageAlign::Left) => "left",
            (_, ImageAlign::Right) => "right",
        };
        if options.full_page {
            self.pending_break = true;
        }
        let drawing = self.drawing(index, w, h, &caption);
        self.paragraph(
            "Normal",
            &format!(
                r#"<w:spacing w:before="120" w:after="60" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="{jc}"/>"#
            ),
            &format!("<w:r>{drawing}</w:r>"),
        );
        if !caption.is_empty() {
            self.paragraph("Caption", "", &text_run("", &caption));
        }
        if options.full_page {
            self.pending_break = true;
        }
    }

    /// 확장 블록 그림(`t = 0`, 밝은 테마)을 `align:` 자리(기본 가운데)에 넣는다. 크기는 캔버스 × `scale:`(px)이고
    /// 칸보다 크면 줄인다. 내보내기에서 빼는 블록은 넣지 않고, 그리지 못했으면 `Err`(코드 블록으로 둔다).
    fn extension(&mut self, name: &str, args: &[(String, String)], ctx: Context) -> Result<(), ()> {
        let key = (name.to_string(), args.to_vec());
        let index = match self.drawing_ids.get(&key) {
            Some(index) => *index,
            None => {
                let index = self.load_drawing(name, args);
                self.drawing_ids.insert(key, index);
                index
            }
        };
        let Some(index) = index? else {
            return Ok(());
        };
        let (w, h) = self.drawing_size(index);
        let jc = match crate::extension::align_of(args) {
            crate::extension::Align::Left => "left",
            crate::extension::Align::Center => "center",
            crate::extension::Align::Right => "right",
        };
        let max_w = self.content_width() - self.em(ctx.indent);
        let max_h = self.content_height() - self.em(1.);
        let scale = (max_w / w).min(max_h / h).min(1.);
        let drawing = self.drawing(index, w * scale, h * scale, name);
        self.paragraph(
            "Normal",
            &format!(
                r#"<w:spacing w:before="120" w:after="120" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="{jc}"/>"#
            ),
            &format!("<w:r>{drawing}</w:r>"),
        );
        Ok(())
    }

    /// 문단 안 확장 그림(ADR-0036)의 `w:drawing`. 높이는 본문 행 높이 × 인라인 `scale=`이고(에디터와 같다)
    /// 칸보다 넓으면 줄인다. 그리지 못하거나 빼는 그림이면 `None`(그 자리는 비운다).
    fn inline_drawing(&mut self, object: &InlineObject) -> Option<String> {
        let key = (object.name.clone(), object.args.clone());
        let index = match self.drawing_ids.get(&key) {
            Some(index) => *index,
            None => {
                let index = self.load_drawing(&object.name, &object.args);
                self.drawing_ids.insert(key, index);
                index
            }
        };
        let index = index.ok()??;
        let (w, h) = self.drawing_size(index);
        let line_height = self.project.styles.resolve(StyleTarget::Body).line_height as f32;
        let height = self.em(line_height * crate::extension::scale_of(&object.args));
        let width = height * w / h.max(f32::EPSILON);
        let fit = (self.content_width() / width).min(1.);
        Some(self.drawing(index, width * fit, height * fit, &object.name))
    }

    /// 확장 그림의 기본 크기(pt): 캔버스 × `scale:`.
    fn drawing_size(&self, index: usize) -> (f32, f32) {
        let part = &self.images[index];
        // PNG는 DRAWING_RESOLUTION배로 그렸다.
        let px = |v: u32| v as f32 / DRAWING_RESOLUTION * PT_PER_PX;
        (px(part.width), px(part.height))
    }

    /// 확장 그림을 PNG(인쇄용 해상도)와 SVG로 만들어 넣는다. 내보내기에서 빼는 블록이면 `Ok(None)`이다.
    fn load_drawing(&mut self, name: &str, args: &[(String, String)]) -> Result<Option<usize>, ()> {
        let Some(still) =
            crate::extension::still(&self.project.root, name, args).map_err(|_| ())?
        else {
            return Ok(None);
        };
        let (w, h) = (still.canvas.0 * still.scale, still.canvas.1 * still.scale);
        let frame = crate::extension::raster::Frame {
            label: String::new(),
            shapes: &still.shapes,
        };
        // 캔버스 좌표로 그리고 배율(scale × 해상도)을 한 번에 준다.
        let resolution = DRAWING_RESOLUTION * still.scale;
        let png = crate::extension::raster::render_png_at(
            std::slice::from_ref(&frame),
            still.canvas.0,
            still.canvas.1,
            crate::extension::Theme::default().background,
            f32::MAX,
            resolution,
        )
        .map_err(|_| ())?;
        let index = self.images.len();
        let id = index + 1;
        // Word는 SVG의 섞기·흐림·그림자를 그리지 않으므로 효과가 있는 그림은 PNG만 넣는다. 그림 파일이 든 것도
        // (SVG에 파일을 한 번 더 넣지 않도록) PNG만 넣는다.
        let plain = still.shapes.iter().all(|shape| match shape {
            crate::extension::Shape::Effects(effects) => {
                *effects == crate::extension::canvas::Effects::default()
            }
            crate::extension::Shape::Image { .. } => false,
            _ => true,
        });
        self.images.push(ImagePart {
            rel_id: format!("rIdImage{id}"),
            name: format!("media/image{id}.png"),
            extension: "png",
            data: png,
            width: (w * DRAWING_RESOLUTION).ceil().max(1.) as u32,
            height: (h * DRAWING_RESOLUTION).ceil().max(1.) as u32,
            video: None,
            svg: plain.then(|| {
                (
                    format!("rIdImage{id}Svg"),
                    format!("media/image{id}.svg"),
                    crate::extension::svg::to_svg(&still).into_bytes(),
                )
            }),
        });
        Ok(Some(index))
    }

    /// YouTube 썸네일을 내려받아 온라인 비디오 그림으로 넣는다. 받지 못하면 `None`.
    fn load_youtube(&mut self, id: &str) -> Option<usize> {
        let key = PathBuf::from(format!("youtube:{id}"));
        if let Some(index) = self.image_ids.get(&key) {
            return Some(*index);
        }
        let data = crate::media::youtube_poster(id)?;
        let size = imagesize::blob_size(&data).ok()?;
        let index = self.images.len();
        self.images.push(ImagePart {
            rel_id: format!("rIdImage{}", index + 1),
            name: format!("media/image{}.jpeg", index + 1),
            extension: "jpeg",
            data,
            width: size.width.max(1) as u32,
            height: size.height.max(1) as u32,
            video: Some(id.to_string()),
            svg: None,
        });
        self.image_ids.insert(key, index);
        Some(index)
    }

    /// 동영상 포스터(첫 장면 PNG)를 그림으로 넣는다. 만들 수 없으면 `None`.
    fn load_poster(&mut self, path: &Path) -> Option<usize> {
        let key = path.with_extension(format!(
            "{}.poster",
            path.extension().unwrap_or_default().to_string_lossy()
        ));
        if let Some(index) = self.image_ids.get(&key) {
            return Some(*index);
        }
        let data = crate::media::poster_png(path)?;
        let size = imagesize::blob_size(&data).ok()?;
        let index = self.images.len();
        self.images.push(ImagePart {
            rel_id: format!("rIdImage{}", index + 1),
            name: format!("media/image{}.png", index + 1),
            extension: "png",
            data,
            width: size.width.max(1) as u32,
            height: size.height.max(1) as u32,
            video: None,
            svg: None,
        });
        self.image_ids.insert(key, index);
        Some(index)
    }

    fn load_image(&mut self, path: &Path) -> Option<usize> {
        if let Some(index) = self.image_ids.get(path) {
            return Some(*index);
        }
        let data = std::fs::read(path).ok()?;
        // Word가 바로 읽는 PNG·JPEG·GIF는 그대로, 나머지(WebP 등)는 PNG로 바꾼다.
        let (data, extension) = match crate::library::image_extension(&data) {
            Some("png") => (data, "png"),
            Some("jpg") => (data, "jpeg"),
            Some("gif") => (data, "gif"),
            Some("svg") => return None,
            _ => {
                let decoded = image::load_from_memory(&data).ok()?;
                let mut png = Vec::new();
                decoded
                    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                    .ok()?;
                (png, "png")
            }
        };
        let size = imagesize::blob_size(&data).ok()?;
        let index = self.images.len();
        self.images.push(ImagePart {
            rel_id: format!("rIdImage{}", index + 1),
            name: format!("media/image{}.{extension}", index + 1),
            extension,
            data,
            width: size.width.max(1) as u32,
            height: size.height.max(1) as u32,
            video: None,
            svg: None,
        });
        self.image_ids.insert(path.to_path_buf(), index);
        Some(index)
    }

    /// 인라인 그림 XML.
    fn drawing(&self, index: usize, width: f32, height: f32, description: &str) -> String {
        let part = &self.images[index];
        let (cx, cy) = (emu(width), emu(height));
        let id = index + 1;
        let description = escape(description);
        // 온라인 비디오: 그림 속성에 YouTube 링크와 플레이어(iframe)를 단다. Word가 그림을 누르면 재생한다.
        let video = part.video.as_ref().map_or(String::new(), |id| {
            let player = escape(&format!(
                r#"<iframe width="560" height="315" src="{}?feature=oembed" frameborder="0" allow="autoplay; encrypted-media; picture-in-picture" allowfullscreen=""></iframe>"#,
                crate::media::youtube_embed_url(id)
            ));
            format!(
                r#"<a:hlinkClick xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" r:id="{rel}Video"/><a:extLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:ext uri="{{C809E66F-F1BF-436E-b5F7-EEA9579F0CBA}}"><wp15:webVideoPr xmlns:wp15="http://schemas.microsoft.com/office/word/2012/wordprocessingDrawing" embeddedHtml="{player}" h="315" w="560"/></a:ext></a:extLst>"#,
                rel = part.rel_id
            )
        });
        let doc_pr = if video.is_empty() {
            format!(r#"<wp:docPr id="{id}" name="그림 {id}" descr="{description}"/>"#)
        } else {
            format!(
                r#"<wp:docPr id="{id}" name="비디오 {id}" descr="{description}">{video}</wp:docPr>"#
            )
        };
        format!(
            r#"<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/>{doc_pr}<wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="image{id}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rel}">{svg}</a:blip><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>"#,
            rel = part.rel_id,
            svg = part.svg.as_ref().map_or(String::new(), |(svg_rel, _, _)| format!(
                r#"<a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="{svg_rel}"/></a:ext></a:extLst>"#
            )),
        )
    }

    fn table(&mut self, table: &Table) {
        let columns = table.columns();
        if columns == 0 {
            return;
        }
        let width = twips(self.content_width());
        let column = width / columns as i64;
        let mut xml = format!(
            r#"<w:tbl><w:tblPr><w:tblW w:w="{width}" w:type="dxa"/><w:tblBorders><w:top w:val="single" w:sz="4" w:color="BFBFBF"/><w:bottom w:val="single" w:sz="4" w:color="BFBFBF"/><w:insideH w:val="single" w:sz="4" w:color="BFBFBF"/></w:tblBorders><w:tblCellMar><w:top w:w="60" w:type="dxa"/><w:bottom w:w="60" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid>"#
        );
        for _ in 0..columns {
            xml.push_str(&format!(r#"<w:gridCol w:w="{column}"/>"#));
        }
        xml.push_str("</w:tblGrid>");
        for (row_ix, row) in table.rows.iter().enumerate() {
            xml.push_str("<w:tr>");
            if row_ix == 0 {
                xml.push_str("<w:trPr><w:tblHeader/></w:trPr>");
            }
            for col in 0..columns {
                // 머리 행은 굵게.
                let runs = row
                    .get(col)
                    .map(|cell| self.runs_with(cell, row_ix == 0))
                    .unwrap_or_default();
                let jc = match table.align(col) {
                    ColumnAlign::Left => "",
                    ColumnAlign::Center => r#"<w:jc w:val="center"/>"#,
                    ColumnAlign::Right => r#"<w:jc w:val="right"/>"#,
                };
                xml.push_str(&format!(
                    r#"<w:tc><w:tcPr><w:tcW w:w="{column}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:pStyle w:val="TableText"/>{jc}</w:pPr>{runs}</w:p></w:tc>"#
                ));
            }
            xml.push_str("</w:tr>");
        }
        xml.push_str("</w:tbl>");
        // 표 앞에 미뤄 둔 쪽 나눔은 빈 단락으로 건다(표에는 쪽 나눔 속성이 없다).
        if self.pending_break {
            self.paragraph("Normal", r#"<w:ind w:firstLine="0"/>"#, "");
        }
        self.body.push_str(&xml);
        // 표 바로 뒤에 표가 오면 Word가 하나로 붙이므로 빈 단락으로 띄운다.
        self.paragraph(
            "Normal",
            r#"<w:spacing w:before="0" w:after="0"/><w:ind w:firstLine="0"/>"#,
            "",
        );
    }

    // --- 앞 부속 -------------------------------------------------------------

    /// 표지 → 속표지 → 차례. 쪽 번호가 없는 구역으로 닫는다.
    fn front_matter(&mut self) {
        let book = &self.project.book;
        let title = if book.title.trim().is_empty() {
            self.project.name.clone()
        } else {
            book.title.clone()
        };
        if let Some(cover) = book.cover_path(&self.project.dir).filter(|p| p.is_file())
            && let Some(index) = self.load_image(&cover)
        {
            let part = &self.images[index];
            let aspect = part.height as f32 / part.width as f32;
            let (max_w, max_h) = (self.content_width(), self.content_height() - 4.);
            let (w, h) = if max_w * aspect <= max_h {
                (max_w, max_w * aspect)
            } else {
                (max_h / aspect, max_h)
            };
            let drawing = self.drawing(index, w, h, "표지");
            self.paragraph(
                "Normal",
                r#"<w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="center"/>"#,
                &format!("<w:r>{drawing}</w:r>"),
            );
            self.close_section(false, VerticalAlign::Center);
        }

        // 속표지: 제목·지은이를 세로 가운데에.
        self.paragraph("Title", "", &text_run("", &title));
        if !book.authors.is_empty() {
            self.paragraph("Subtitle", "", &text_run("", &book.authors.join(", ")));
        }
        self.close_section(false, VerticalAlign::Center);

        if self.options.generate_toc {
            self.paragraph("TOCHeading", "", &text_run("", "차례"));
            // 필드 결과는 비워 두고 Word가 열 때 채운다(settings의 updateFields).
            self.body.push_str(
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin" w:dirty="true"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>목차를 업데이트하려면 필드를 갱신하세요.</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
            );
            self.close_section(false, VerticalAlign::Top);
        }
    }

    /// 본문을 마지막 구역으로 닫고 `document.xml`을 만든다.
    fn finish_body(&mut self) -> String {
        let sect = self.sect_pr(true, VerticalAlign::Top);
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{}{sect}</w:body></w:document>"#,
            self.body
        )
    }

    // --- 스타일 --------------------------------------------------------------

    /// 대상 스타일 → 단락·글자 속성.
    fn style_props(&self, style: &ResolvedStyle, heading: Option<usize>) -> (String, String) {
        let size = self.options.font_size * style.font_size as f32;
        let line = (style.line_height * 240.).round() as i64;
        let jc = match style.align {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
        };
        let mut ppr = format!(
            r#"<w:spacing w:before="{}" w:after="{}" w:line="{line}" w:lineRule="auto"/><w:ind w:firstLine="{}"/><w:jc w:val="{jc}"/>"#,
            twips(self.em(style.space_before as f32)),
            twips(self.em(style.space_after as f32)),
            twips(self.em(style.first_line_indent as f32)),
        );
        if let Some(level) = heading {
            // 순서: keepNext → keepLines → pageBreakBefore → spacing → ind → jc → outlineLvl.
            let page_break = if style.page_break || style.placement == Placement::Page {
                "<w:pageBreakBefore/>"
            } else {
                ""
            };
            ppr = format!(
                "<w:keepNext/><w:keepLines/>{page_break}{ppr}<w:outlineLvl w:val=\"{level}\"/>"
            );
        }
        let font = style
            .font
            .clone()
            .or_else(|| self.options.default_font.clone());
        let mut rpr = String::new();
        if let Some(font) = font {
            let font = escape(&font);
            rpr.push_str(&format!(
                r#"<w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:eastAsia="{font}" w:cs="{font}"/>"#
            ));
        }
        if style.font_weight >= 600 {
            rpr.push_str("<w:b/><w:bCs/>");
        }
        let half_points = (size * 2.).round() as i64;
        rpr.push_str(&format!(
            r#"<w:sz w:val="{half_points}"/><w:szCs w:val="{half_points}"/>"#
        ));
        (ppr, rpr)
    }

    fn styles_xml(&self) -> String {
        let styles = &self.project.styles;
        let body = styles.resolve(StyleTarget::Body);
        let (body_ppr, body_rpr) = self.style_props(&body, None);
        let mut out = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
        );
        out.push_str(&format!(
            r#"<w:docDefaults><w:rPrDefault><w:rPr>{body_rpr}<w:lang w:val="{lang}" w:eastAsia="{lang}"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:widowControl/><w:wordWrap w:val="0"/></w:pPr></w:pPrDefault></w:docDefaults>"#,
            lang = escape(language(self.project)),
        ));
        out.push_str(&paragraph_style(
            "Normal", "Normal", None, &body_ppr, &body_rpr, true,
        ));
        for (ix, target) in [
            StyleTarget::Part,
            StyleTarget::Chapter,
            StyleTarget::Section,
        ]
        .into_iter()
        .enumerate()
        {
            let resolved = styles.resolve(target);
            let (ppr, rpr) = self.style_props(&resolved, Some(ix));
            out.push_str(&paragraph_style(
                &format!("Heading{}", ix + 1),
                &format!("heading {}", ix + 1),
                Some("Normal"),
                &ppr,
                &rpr,
                false,
            ));
        }
        let flat = |before: f32, after: f32| {
            format!(
                r#"<w:spacing w:before="{}" w:after="{}"/><w:ind w:firstLine="0"/><w:jc w:val="left"/>"#,
                twips(before),
                twips(after)
            )
        };
        let size = |scale: f32| {
            let half = (self.options.font_size * scale * 2.).round() as i64;
            format!(r#"<w:sz w:val="{half}"/><w:szCs w:val="{half}"/>"#)
        };
        for level in 4..=6 {
            out.push_str(&paragraph_style(
                &format!("Heading{level}"),
                &format!("heading {level}"),
                Some("Normal"),
                &format!(
                    "<w:keepNext/>{}<w:outlineLvl w:val=\"{}\"/>",
                    flat(self.em(1.), self.em(0.25)),
                    level - 1
                ),
                &format!("<w:b/><w:bCs/>{}", size(1.)),
                false,
            ));
        }
        let mono = escape(&self.options.mono_font);
        // 가운데 맞춤 단락(spacing → ind → jc 순서).
        let center = |before: f32, after: f32| {
            format!(
                r#"<w:spacing w:before="{}" w:after="{}"/><w:ind w:firstLine="0"/><w:jc w:val="center"/>"#,
                twips(before),
                twips(after)
            )
        };
        out.push_str(&paragraph_style(
            "Title",
            "Title",
            Some("Normal"),
            &center(0., self.em(1.5)),
            &format!("<w:b/><w:bCs/>{}", size(2.)),
            false,
        ));
        out.push_str(&paragraph_style(
            "Subtitle",
            "Subtitle",
            Some("Normal"),
            &center(0., 0.),
            &size(1.1),
            false,
        ));
        out.push_str(&paragraph_style(
            "TOCHeading",
            "TOC Heading",
            Some("Normal"),
            &center(0., self.em(2.)),
            &format!("<w:b/><w:bCs/>{}", size(1.4)),
            false,
        ));
        for level in 1..=3 {
            out.push_str(&paragraph_style(
                &format!("TOC{level}"),
                &format!("toc {level}"),
                Some("Normal"),
                &format!(
                    r#"<w:spacing w:before="{}" w:after="0"/><w:ind w:left="{}" w:firstLine="0"/><w:jc w:val="left"/>"#,
                    if level == 1 { twips(self.em(0.6)) } else { 0 },
                    twips(self.em(1.5 * (level - 1) as f32)),
                ),
                if level == 1 { "<w:b/><w:bCs/>" } else { "" },
                false,
            ));
        }
        out.push_str(&paragraph_style(
            "Quote",
            "Quote",
            Some("Normal"),
            &format!(
                r#"<w:spacing w:before="{0}" w:after="{0}"/><w:ind w:left="{1}" w:right="{1}" w:firstLine="0"/>"#,
                twips(self.em(0.5)),
                twips(self.em(1.5)),
            ),
            "",
            false,
        ));
        out.push_str(&paragraph_style(
            "ListParagraph",
            "List Paragraph",
            Some("Normal"),
            r#"<w:ind w:firstLine="0"/>"#,
            "",
            false,
        ));
        out.push_str(&paragraph_style(
            "Code",
            "Code",
            Some("Normal"),
            &format!(
                r#"<w:spacing w:before="0" w:after="0" w:line="260" w:lineRule="auto"/><w:ind w:left="{}" w:firstLine="0"/><w:jc w:val="left"/>"#,
                twips(self.em(1.))
            ),
            &format!(
                r#"<w:rFonts w:ascii="{mono}" w:hAnsi="{mono}" w:eastAsia="{mono}" w:cs="{mono}"/>{}"#,
                size(0.85)
            ),
            false,
        ));
        out.push_str(&paragraph_style(
            "Caption",
            "caption",
            Some("Normal"),
            &center(0., self.em(1.)),
            &format!(r#"<w:i/><w:iCs/><w:color w:val="737373"/>{}"#, size(0.85)),
            false,
        ));
        out.push_str(&paragraph_style(
            "TableText",
            "Table Text",
            Some("Normal"),
            &flat(0., 0.),
            &size(0.9),
            false,
        ));
        out.push_str(&paragraph_style(
            "FootnoteText",
            "footnote text",
            Some("Normal"),
            r#"<w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="left"/>"#,
            &size(0.85),
            false,
        ));
        out.push_str(&paragraph_style(
            "Footer",
            "footer",
            Some("Normal"),
            &center(0., 0.),
            &format!(r#"<w:color w:val="737373"/>{}"#, size(0.8)),
            false,
        ));
        out.push_str(r#"<w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#);
        out.push_str("</w:styles>");
        out
    }

    fn footnotes_xml(&self) -> String {
        let mut out = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#,
        );
        for (ix, content) in self.footnotes.iter().enumerate() {
            out.push_str(&format!(
                r#"<w:footnote w:id="{}">{content}</w:footnote>"#,
                ix + 1
            ));
        }
        out.push_str("</w:footnotes>");
        out
    }
}

/// 글자 런(빈 글자면 빈 문자열). 앞뒤 공백을 지키고 개행은 줄바꿈으로 바꾼다.
fn text_run(rpr: &str, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let rpr = if rpr.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{rpr}</w:rPr>")
    };
    let mut out = String::new();
    for (ix, line) in text.split('\n').enumerate() {
        if ix > 0 {
            out.push_str(&format!("<w:r>{rpr}<w:br/></w:r>"));
        }
        if !line.is_empty() {
            out.push_str(&format!(
                r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#,
                escape(line)
            ));
        }
    }
    out
}

fn paragraph_style(
    id: &str,
    name: &str,
    based_on: Option<&str>,
    ppr: &str,
    rpr: &str,
    default: bool,
) -> String {
    let default = if default { r#" w:default="1""# } else { "" };
    let based_on = based_on
        .map(|base| format!(r#"<w:basedOn w:val="{base}"/><w:next w:val="Normal"/>"#))
        .unwrap_or_default();
    format!(
        r#"<w:style w:type="paragraph"{default} w:styleId="{id}"><w:name w:val="{name}"/>{based_on}<w:qFormat/><w:pPr>{ppr}</w:pPr><w:rPr>{rpr}</w:rPr></w:style>"#
    )
}

fn language(project: &ExportProject) -> &str {
    let language = project.book.language.trim();
    if language.is_empty() {
        "ko-KR"
    } else {
        language
    }
}

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;

const FOOTER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:p><w:pPr><w:pStyle w:val="Footer"/></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#;

fn content_types(images: &[ImagePart]) -> String {
    let mut defaults = String::new();
    let mut seen = Vec::new();
    for image in images {
        if !seen.contains(&image.extension) {
            seen.push(image.extension);
            defaults.push_str(&format!(
                r#"<Default Extension="{0}" ContentType="image/{0}"/>"#,
                image.extension
            ));
        }
        if image.svg.is_some() && !seen.contains(&"svg") {
            seen.push("svg");
            defaults.push_str(r#"<Default Extension="svg" ContentType="image/svg+xml"/>"#);
        }
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>{defaults}<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#
    )
}

fn document_rels(images: &[ImagePart]) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdSettings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/><Relationship Id="rIdFootnotes" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/><Relationship Id="rIdFooter1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>"#,
    );
    for image in images {
        out.push_str(&format!(
            r#"<Relationship Id="{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{}"/>"#,
            image.rel_id, image.name
        ));
        if let Some(id) = &image.video {
            out.push_str(&format!(
                r#"<Relationship Id="{}Video" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>"#,
                image.rel_id,
                escape(&crate::media::youtube_watch_url(id))
            ));
        }
        if let Some((rel, name, _)) = &image.svg {
            out.push_str(&format!(
                r#"<Relationship Id="{rel}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{name}"/>"#
            ));
        }
    }
    out.push_str("</Relationships>");
    out
}

fn settings_xml(update_fields: bool) -> String {
    let update = if update_fields {
        r#"<w:updateFields w:val="true"/>"#
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="compressPunctuation"/>{update}<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#
    )
}

fn core_xml(project: &ExportProject) -> String {
    let title = if project.book.title.trim().is_empty() {
        &project.name
    } else {
        &project.book.title
    };
    let description = if project.book.description.trim().is_empty() {
        String::new()
    } else {
        format!(
            "<dc:description>{}</dc:description>",
            escape(&project.book.description)
        )
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>{}</dc:creator><dc:language>{}</dc:language>{description}</cp:coreProperties>"#,
        escape(title),
        escape(&project.book.authors.join(", ")),
        escape(language(project)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ExportSheet;
    use crate::library::{BookMeta, Styles};
    use std::io::Read;

    fn project(content: &str) -> ExportProject {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장\n".into());
        styles.chapter.page_break = Some(true);
        styles.part.placement = Some(Placement::Page);
        ExportProject {
            root: Default::default(),
            name: "시험".into(),
            dir: std::env::temp_dir(),
            book: BookMeta {
                title: "시험 책".into(),
                authors: vec!["홍길동".into()],
                ..Default::default()
            },
            styles,
            sheets: vec![ExportSheet {
                path: std::env::temp_dir().join("a.md"),
                glued: false,
                content: content.into(),
            }],
        }
    }

    fn read(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        zip.by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn extension_blocks_become_png_with_svg_or_stay_code() {
        let root = std::env::temp_dir().join(format!("docx-ext-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        std::fs::write(
            root.join("Extensions/dot.lua"),
            "return { size = { 40, 20 }, draw = function(g) g.circle('fill', 10, 10, 5) end }",
        )
        .unwrap();
        let mut project = project(
            "```ext dot\nscale: 2\n```\n\n```ext dot\nscale: 2\n```\n\n```ext missing\nr: 1\n```\n",
        );
        project.root = root.clone();
        let mut bytes = std::io::Cursor::new(Vec::new());
        build(&project, &DocxOptions::default(), &mut bytes).unwrap();
        let bytes = bytes.into_inner();
        let document = read(&bytes, "word/document.xml");
        // 같은 블록은 그림 하나를 같이 쓰고, Word 2016 이상용 SVG를 함께 단다.
        assert_eq!(
            document.matches(r#"r:embed="rIdImage1""#).count(),
            2,
            "{document}"
        );
        assert!(document.contains(r#"<asvg:svgBlip"#), "{document}");
        // 40 × 2 px = 60pt = 762000 EMU.
        assert!(
            document.contains(r#"<wp:extent cx="762000" cy="381000"/>"#),
            "{document}"
        );
        assert!(document.contains("r: 1"), "{document}");
        assert!(
            read(&bytes, "[Content_Types].xml")
                .contains(r#"Extension="svg" ContentType="image/svg+xml""#)
        );
        assert!(
            read(&bytes, "word/_rels/document.xml.rels").contains(r#"Target="media/image1.svg""#)
        );
        assert!(read(&bytes, "word/media/image1.svg").contains("<svg"));
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut png = Vec::new();
        zip.by_name("word/media/image1.png")
            .unwrap()
            .read_to_end(&mut png)
            .unwrap();
        let image = image::load_from_memory(&png).unwrap();
        // 캔버스 × scale 2 × 해상도 3.
        assert_eq!((image.width(), image.height()), (240, 120));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn youtube_is_an_online_video_or_a_link_line() {
        // 시험에서는 썸네일을 내려받지 않으므로 링크 줄이다.
        let project = project("![예고편](https://youtu.be/dQw4w9WgXcQ)\n");
        let mut bytes = std::io::Cursor::new(Vec::new());
        build(&project, &DocxOptions::default(), &mut bytes).unwrap();
        let document = read(&bytes.into_inner(), "word/document.xml");
        assert!(
            document.contains("▶ 예고편 (https://www.youtube.com/watch?v=dQw4w9WgXcQ)"),
            "{document}"
        );
        // 썸네일을 받으면 하이퍼링크 관계를 단다.
        let rels = document_rels(&[ImagePart {
            rel_id: "rIdImage1".into(),
            name: "media/image1.jpeg".into(),
            extension: "jpeg",
            data: Vec::new(),
            width: 1280,
            height: 720,
            svg: None,
            video: Some("dQw4w9WgXcQ".into()),
        }]);
        assert!(rels.contains(r#"Id="rIdImage1Video""#), "{rels}");
        assert!(rels.contains(r#"TargetMode="External""#), "{rels}");
    }

    #[test]
    fn inline_drawings_are_drawings_in_the_paragraph() {
        let root = std::env::temp_dir().join(format!("docx-inline-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        std::fs::write(
            root.join("Extensions/dot.lua"),
            "return { size = { 40, 20 }, draw = function(g) g.circle('fill', 10, 10, 5) end }",
        )
        .unwrap();
        let mut project = project("앞 {{dot}} 뒤\n");
        project.root = root.clone();
        let mut bytes = std::io::Cursor::new(Vec::new());
        build(&project, &DocxOptions::default(), &mut bytes).unwrap();
        let document = read(&bytes.into_inner(), "word/document.xml");
        let paragraph = document
            .split("<w:p>")
            .find(|p| p.contains("앞"))
            .unwrap_or_default();
        assert!(paragraph.contains("<wp:inline"), "{document}");
        assert!(paragraph.contains("뒤"), "{document}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn builds_document_with_styles_headings_and_footnotes() {
        let project = project(
            "# 1부\n\n## 첫 장\n\n본문 **굵게**[^a] & <꺾쇠>.\n\n- 하나\n\n[^a]: 각주 내용\n",
        );
        let options = DocxOptions {
            generate_toc: true,
            ..Default::default()
        };
        let mut bytes = std::io::Cursor::new(Vec::new());
        build(&project, &options, &mut bytes).unwrap();
        let bytes = bytes.into_inner();

        let document = read(&bytes, "word/document.xml");
        assert!(
            document.contains(r#"<w:pStyle w:val="Heading1"/>"#),
            "{document}"
        );
        assert!(document.contains(r#"<w:vAlign w:val="center"/>"#));
        // 접두사 개행은 줄바꿈, 번호는 계산한 글자.
        assert!(
            document.contains("제1장</w:t></w:r><w:r><w:br/></w:r>"),
            "{document}"
        );
        assert!(document.contains(r#"<w:footnoteReference w:id="1"/>"#));
        assert!(document.contains("&amp; &lt;꺾쇠&gt;"));
        assert!(document.contains(r#"TOC \o "1-3""#));
        assert!(!document.contains("¹"));
        let footnotes = read(&bytes, "word/footnotes.xml");
        assert!(footnotes.contains("각주 내용"));
        let styles = read(&bytes, "word/styles.xml");
        assert!(styles.contains(r#"w:styleId="Heading2""#));
        assert!(styles.contains("<w:pageBreakBefore/>"));
        assert!(read(&bytes, "docProps/core.xml").contains("<dc:creator>홍길동</dc:creator>"));
        assert!(read(&bytes, "word/settings.xml").contains("updateFields"));
    }
}
