//! 책 스타일(본문·부/장/절 제목): 프로젝트 `.hanji.toml`의 `[styles.<대상>]` 표(ADR-0009).
//!
//! 에디터 렌더링과 내보내기가 같은 값을 쓴다. 파일에는 사용자가 바꾼 값만 쓰고,
//! 빠진 값은 대상별 기본값([`StyleTarget::defaults`])으로 채운다.
//! 다른 프로젝트의 스타일은 값을 그대로 복사해 가져온다([`import_styles`]).

use std::borrow::Cow;
use std::io;
use std::path::Path;

use super::GROUP_META_FILE;
use super::meta::write_meta_values;

/// 스타일 대상. 파일 키(`key`)는 바꾸지 않는다.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StyleTarget {
    /// 본문 문단.
    Body,
    /// 부 제목(`#`).
    Part,
    /// 장 제목(`##`).
    Chapter,
    /// 절 제목(`###`).
    Section,
}

impl StyleTarget {
    /// 화면에 늘어놓는 순서.
    pub const ALL: [StyleTarget; 4] = [
        StyleTarget::Body,
        StyleTarget::Part,
        StyleTarget::Chapter,
        StyleTarget::Section,
    ];

    /// 파일 키(`[styles.<key>]`).
    pub fn key(self) -> &'static str {
        match self {
            StyleTarget::Body => "body",
            StyleTarget::Part => "title:part",
            StyleTarget::Chapter => "title:chapter",
            StyleTarget::Section => "title:section",
        }
    }

    /// 표시 이름.
    pub fn title(self) -> Cow<'static, str> {
        match self {
            StyleTarget::Body => t!("book.style.target.body"),
            StyleTarget::Part => t!("book.style.target.part"),
            StyleTarget::Chapter => t!("book.style.target.chapter"),
            StyleTarget::Section => t!("book.style.target.section"),
        }
    }

    /// 제목 대상인지.
    pub fn is_title(self) -> bool {
        self != StyleTarget::Body
    }

    /// ATX 제목 레벨이 쓰는 대상. `#`~`###`만 제목 대상이고 나머지는 없다.
    pub fn for_heading(level: u8) -> Option<StyleTarget> {
        match level {
            1 => Some(StyleTarget::Part),
            2 => Some(StyleTarget::Chapter),
            3 => Some(StyleTarget::Section),
            _ => None,
        }
    }

    /// 번호 깊이. 부 0, 장 1, 절 2. 본문은 번호가 없다.
    pub fn depth(self) -> usize {
        match self {
            StyleTarget::Body | StyleTarget::Part => 0,
            StyleTarget::Chapter => 1,
            StyleTarget::Section => 2,
        }
    }

    /// 대상의 기본 스타일. 파일에 없는 값은 이것으로 채운다.
    /// 부·장·절 제목은 기본적으로 앞 내용에 이어서 놓고 페이지를 나누지 않는다.
    pub fn defaults(self) -> ResolvedStyle {
        let body = ResolvedStyle {
            font: None,
            embed_font: false,
            font_size: 1.0,
            font_weight: 400,
            line_height: 1.6,
            align: Align::Justify,
            first_line_indent: 1.0,
            space_before: 0.0,
            space_after: 0.0,
            preserve_line_breaks: false,
            placement: Placement::Inline,
            page_break: false,
            prefix: String::new(),
            numbering: Numbering::Arabic,
            restart_on_parent: false,
            in_toc: true,
            vertical_align: VerticalAlign::Center,
        };
        let title = |font_size, align, space_before, space_after| ResolvedStyle {
            font_size,
            font_weight: 700,
            line_height: 1.3,
            align,
            first_line_indent: 0.0,
            space_before,
            space_after,
            ..body.clone()
        };
        match self {
            StyleTarget::Body => body,
            StyleTarget::Part => title(1.6, Align::Left, 4.0, 2.0),
            StyleTarget::Chapter => title(1.4, Align::Left, 3.0, 1.5),
            StyleTarget::Section => title(1.2, Align::Left, 1.5, 0.75),
        }
    }
}

/// 문단 정렬.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub const ALL: [Align; 4] = [Align::Left, Align::Center, Align::Right, Align::Justify];

    /// 표시 이름.
    pub fn title(self) -> Cow<'static, str> {
        match self {
            Align::Left => t!("book.style.align.left"),
            Align::Center => t!("book.style.align.center"),
            Align::Right => t!("book.style.align.right"),
            Align::Justify => t!("book.style.align.justify"),
        }
    }
}

/// 제목 배치.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// 본문 첫머리(제목 뒤에 본문이 이어진다).
    Inline,
    /// 제목만 있는 별도 페이지. 본문은 다음 페이지에서 시작한다.
    Page,
    /// 제목을 보이지 않는다(목차에는 남는다).
    Hidden,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Inline, Placement::Page, Placement::Hidden];

    /// 표시 이름.
    pub fn title(self) -> Cow<'static, str> {
        match self {
            Placement::Inline => t!("book.style.placement.inline"),
            Placement::Page => t!("book.style.placement.page"),
            Placement::Hidden => t!("book.style.placement.hidden"),
        }
    }
}

/// 제목 페이지 안 세로 위치.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerticalAlign {
    Top,
    Center,
    Bottom,
}

impl VerticalAlign {
    pub const ALL: [VerticalAlign; 3] = [
        VerticalAlign::Top,
        VerticalAlign::Center,
        VerticalAlign::Bottom,
    ];

    /// 표시 이름.
    pub fn title(self) -> Cow<'static, str> {
        match self {
            VerticalAlign::Top => t!("book.style.valign.top"),
            VerticalAlign::Center => t!("book.style.valign.center"),
            VerticalAlign::Bottom => t!("book.style.valign.bottom"),
        }
    }
}

/// 고를 수 있는 글자 굵기.
pub const FONT_WEIGHTS: [u16; 7] = [300, 400, 500, 600, 700, 800, 900];

/// 제목 접두사에서 자동 번호로 바뀌는 자리.
pub const PREFIX_NUMBER: &str = "{n}";

/// 제목 번호를 적는 방식. `{n}` 자리에 들어간다.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Numbering {
    /// 아라비아 숫자(1, 2, 3).
    Arabic,
    /// 로마 숫자(I, II, III).
    Roman,
}

impl Numbering {
    pub const ALL: [Numbering; 2] = [Numbering::Arabic, Numbering::Roman];

    /// 표시 이름.
    pub fn title(self) -> &'static str {
        match self {
            Numbering::Arabic => "1, 2, 3",
            Numbering::Roman => "I, II, III",
        }
    }

    /// 번호를 글자로 바꾼다.
    pub fn format(self, number: usize) -> String {
        match self {
            Numbering::Arabic => number.to_string(),
            Numbering::Roman => roman(number),
        }
    }
}

/// 번호를 로마 숫자로 바꾼다(1~3999). 범위를 벗어나면 아라비아 숫자를 그대로 쓴다.
fn roman(number: usize) -> String {
    const TABLE: [(usize, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    if number == 0 || number >= 4000 {
        return number.to_string();
    }
    let mut rest = number;
    let mut out = String::new();
    for (value, glyph) in TABLE {
        while rest >= value {
            out.push_str(glyph);
            rest -= value;
        }
    }
    out
}

/// 한 대상의 스타일. 비어 있는 값은 기본값을 따른다. 길이 단위는 em(본문 글자 크기 대비)이다.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct StyleSpec {
    /// 서체 이름(에디터 서체). 없으면 앱 설정의 에디터 서체. 제목은 없으면 본문 서체를 따른다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// 내보낼 때 서체 파일을 넣는지. 넣으면 책도 에디터와 같은 서체, 아니면 리더 기본 서체로 보인다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_font: Option<bool>,
    /// 글자 크기(본문 대비 배율).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// 글자 굵기(100~900).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<u16>,
    /// 줄 간격(글자 크기 대비).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    /// 첫 줄 들여쓰기(em).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_line_indent: Option<f64>,
    /// 앞 여백(em).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_before: Option<f64>,
    /// 뒤 여백(em). 본문에서는 문단 사이 간격이다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_after: Option<f64>,
    /// 본문 원문의 일반 개행과 빈 줄을 유지한다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserve_line_breaks: Option<bool>,
    /// 제목 배치(제목만).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// 새 페이지에서 시작하는지(제목만). 별도 제목 페이지는 늘 새 페이지다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_break: Option<bool>,
    /// 제목 앞에 붙는 글(제목만). `{n}`은 자동 번호로 바뀐다(예: `제{n}장 `).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// 제목 번호를 적는 방식(제목만). `{n}` 자리에 들어간다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numbering: Option<Numbering>,
    /// 상위 제목이 바뀌면 번호를 1부터 다시 세는지(장·절만). 없으면 이어서 센다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_on_parent: Option<bool>,
    /// 별도 제목 페이지 안 세로 위치(제목만).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_align: Option<VerticalAlign>,
    /// 목차에 넣는지(제목만). 넣지 않는 제목도 번호와 접두사는 붙는다(목차에서만 뺀다).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_toc: Option<bool>,
}

impl StyleSpec {
    /// 바꾼 값이 하나도 없는지.
    pub fn is_empty(&self) -> bool {
        *self == StyleSpec::default()
    }

    /// 빠진 값을 대상 기본값으로 채운다.
    pub fn resolve(&self, target: StyleTarget) -> ResolvedStyle {
        let defaults = target.defaults();
        ResolvedStyle {
            font: self.font.clone().or(defaults.font),
            embed_font: self.embed_font.unwrap_or(defaults.embed_font),
            font_size: self.font_size.unwrap_or(defaults.font_size),
            font_weight: self.font_weight.unwrap_or(defaults.font_weight),
            line_height: self.line_height.unwrap_or(defaults.line_height),
            align: self.align.unwrap_or(defaults.align),
            first_line_indent: self.first_line_indent.unwrap_or(defaults.first_line_indent),
            space_before: self.space_before.unwrap_or(defaults.space_before),
            space_after: self.space_after.unwrap_or(defaults.space_after),
            preserve_line_breaks: self
                .preserve_line_breaks
                .unwrap_or(defaults.preserve_line_breaks),
            placement: self.placement.unwrap_or(defaults.placement),
            page_break: self.page_break.unwrap_or(defaults.page_break),
            prefix: self.prefix.clone().unwrap_or(defaults.prefix),
            numbering: self.numbering.unwrap_or(defaults.numbering),
            restart_on_parent: self.restart_on_parent.unwrap_or(defaults.restart_on_parent),
            in_toc: self.in_toc.unwrap_or(defaults.in_toc),
            vertical_align: self.vertical_align.unwrap_or(defaults.vertical_align),
        }
    }

    /// 파일에 쓰기 좋게 숫자를 소수 둘째 자리로 맞춘다(슬라이더가 만드는 1.6500001 같은 값).
    fn rounded(&self) -> StyleSpec {
        let round = |value: Option<f64>| value.map(|value| (value * 100.0).round() / 100.0);
        StyleSpec {
            font_size: round(self.font_size),
            line_height: round(self.line_height),
            first_line_indent: round(self.first_line_indent),
            space_before: round(self.space_before),
            space_after: round(self.space_after),
            ..self.clone()
        }
    }
}

/// 기본값까지 채운 스타일.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedStyle {
    pub font: Option<String>,
    pub embed_font: bool,
    pub font_size: f64,
    pub font_weight: u16,
    pub line_height: f64,
    pub align: Align,
    pub first_line_indent: f64,
    pub space_before: f64,
    pub space_after: f64,
    pub preserve_line_breaks: bool,
    pub placement: Placement,
    pub page_break: bool,
    pub prefix: String,
    pub numbering: Numbering,
    pub restart_on_parent: bool,
    pub vertical_align: VerticalAlign,
    pub in_toc: bool,
}

/// 제목 번호 상태. 깊이는 부(0)·장(1)·절(2) 순서다.
///
/// 번호를 붙인 제목이 나오면 그보다 깊은 제목 중 `restart_on_parent`를 켠 것의 번호를
/// 0으로 되돌린다. 그래서 다음 하위 제목이 1부터 시작한다. 번호가 없는 제목(`{unnumbered}`,
/// 숨김+목차 제외)은 번호를 세지도, 하위 번호를 되돌리지도 않는다.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeadingNumbers {
    counts: [usize; 3],
}

impl HeadingNumbers {
    /// 제목 하나를 세고 그 번호를 돌려준다. 번호를 쓰지 않는 제목이면 0.
    pub fn count(&mut self, styles: &Styles, target: StyleTarget, numbered: bool) -> usize {
        if !numbered {
            return 0;
        }
        let depth = target.depth();
        self.counts[depth] += 1;
        for child in [StyleTarget::Chapter, StyleTarget::Section] {
            let child_depth = child.depth();
            if child_depth > depth && styles.resolve(child).restart_on_parent {
                self.counts[child_depth] = 0;
            }
        }
        self.counts[depth]
    }
}

impl ResolvedStyle {
    /// 접두사의 `{n}` 자리에 넣을 번호 글.
    pub fn number_text(&self, number: usize) -> String {
        self.numbering.format(number)
    }

    /// `{n}`을 번호로 채운 접두사. 목차 포함 여부와 무관하게 붙인다.
    pub fn prefix_text(&self, number: usize) -> String {
        self.prefix
            .replace(PREFIX_NUMBER, &self.number_text(number))
    }

    /// 접두사를 붙인 제목.
    pub fn title_text(&self, number: usize, title: &str) -> String {
        format!("{}{title}", self.prefix_text(number))
    }

    /// 목차용 접두사. 본문과 달리 접두사의 개행은 스페이스로 바꾼다.
    pub fn toc_prefix_text(&self, number: usize) -> String {
        self.prefix_text(number).replace(['\n', '\r'], " ")
    }

    /// 목차용 제목. 접두사의 개행은 스페이스로 바꾼다.
    pub fn toc_title_text(&self, number: usize, title: &str) -> String {
        format!("{}{title}", self.toc_prefix_text(number))
    }
}

/// 프로젝트의 책 스타일(`[styles]` 표). 바꾸지 않은 대상은 파일에 쓰지 않는다.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Styles {
    #[serde(skip_serializing_if = "crate::text_transform::Options::is_default")]
    pub input_transform: crate::text_transform::Options,
    #[serde(skip_serializing_if = "StyleSpec::is_empty")]
    pub body: StyleSpec,
    #[serde(rename = "title:part", skip_serializing_if = "StyleSpec::is_empty")]
    pub part: StyleSpec,
    #[serde(rename = "title:chapter", skip_serializing_if = "StyleSpec::is_empty")]
    pub chapter: StyleSpec,
    #[serde(rename = "title:section", skip_serializing_if = "StyleSpec::is_empty")]
    pub section: StyleSpec,
}

impl Styles {
    pub fn get(&self, target: StyleTarget) -> &StyleSpec {
        match target {
            StyleTarget::Body => &self.body,
            StyleTarget::Part => &self.part,
            StyleTarget::Chapter => &self.chapter,
            StyleTarget::Section => &self.section,
        }
    }

    pub fn get_mut(&mut self, target: StyleTarget) -> &mut StyleSpec {
        match target {
            StyleTarget::Body => &mut self.body,
            StyleTarget::Part => &mut self.part,
            StyleTarget::Chapter => &mut self.chapter,
            StyleTarget::Section => &mut self.section,
        }
    }

    /// 대상의 스타일을 기본값까지 채워 돌려준다. 제목의 서체·서체 포함은 정하지 않으면 본문을 따른다.
    pub fn resolve(&self, target: StyleTarget) -> ResolvedStyle {
        let spec = self.get(target);
        if !target.is_title() {
            return spec.resolve(target);
        }
        let inherited = StyleSpec {
            font: spec.font.clone().or_else(|| self.body.font.clone()),
            embed_font: spec.embed_font.or(self.body.embed_font),
            ..spec.clone()
        };
        inherited.resolve(target)
    }

    /// 바꾼 값이 하나도 없는지.
    pub fn is_empty(&self) -> bool {
        *self == Styles::default()
    }
}

/// 프로젝트의 책 스타일을 읽는다. 없거나 깨지면 기본값.
pub fn read_styles(project_dir: &Path) -> Styles {
    std::fs::read_to_string(project_dir.join(GROUP_META_FILE))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|mut table| table.remove("styles"))
        .and_then(|styles| styles.try_into().ok())
        .unwrap_or_default()
}

/// 책 스타일을 `.hanji.toml`의 `[styles]`에 쓴다. 다른 키는 그대로 두고, 비면 표를 지운다.
pub fn write_styles(project_dir: &Path, styles: &Styles) -> io::Result<()> {
    let styles = Styles {
        body: styles.body.rounded(),
        part: styles.part.rounded(),
        chapter: styles.chapter.rounded(),
        section: styles.section.rounded(),
        input_transform: styles.input_transform.clone(),
    };
    let value = if styles.is_empty() {
        None
    } else {
        Some(
            toml::Value::try_from(&styles)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?,
        )
    };
    write_meta_values(project_dir, [("styles", value)])
}

/// 다른 프로젝트의 책 스타일을 이 프로젝트에 그대로 복사하고, 복사한 스타일을 돌려준다.
pub fn import_styles(source_dir: &Path, project_dir: &Path) -> io::Result<Styles> {
    let styles = read_styles(source_dir);
    write_styles(project_dir, &styles)?;
    Ok(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;
    use crate::library::*;

    #[test]
    fn styles_round_trip_and_keep_other_meta() {
        let root = temp_dir("styles");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", Some("book"), None).unwrap();
        assert!(read_styles(&dir).is_empty());

        let mut styles = Styles::default();
        styles.body.font = Some("Noto Serif KR".into());
        styles.body.line_height = Some(1.6500001);
        styles.chapter.align = Some(Align::Left);
        styles.chapter.prefix = Some("제{n}장\n".into());
        styles.chapter.restart_on_parent = Some(true);
        write_styles(&dir, &styles).unwrap();

        let text = std::fs::read_to_string(dir.join(GROUP_META_FILE)).unwrap();
        assert!(text.contains("[styles.body]"), "{text}");
        assert!(text.contains("[styles.\"title:chapter\"]"), "{text}");
        assert!(!text.contains("title:part"), "{text}");
        assert!(text.contains("line_height = 1.65"), "{text}");
        assert!(text.contains("restart_on_parent = true"), "{text}");

        let read = read_styles(&dir);
        assert_eq!(read.body.font.as_deref(), Some("Noto Serif KR"));
        assert_eq!(read.body.line_height, Some(1.65));
        assert_eq!(read.resolve(StyleTarget::Chapter).align, Align::Left);
        assert_eq!(read.chapter.prefix.as_deref(), Some("제{n}장\n"));
        assert!(read.resolve(StyleTarget::Chapter).restart_on_parent);
        // 쓰지 않은 값은 기본값.
        assert_eq!(read.resolve(StyleTarget::Chapter).font_size, 1.4);
        let project = scan(&root).unwrap().find_project(&dir).cloned().unwrap();
        assert_eq!(project.icon.as_deref(), Some("book"));

        // 모두 기본값으로 돌리면 표가 사라진다.
        write_styles(&dir, &Styles::default()).unwrap();
        let text = std::fs::read_to_string(dir.join(GROUP_META_FILE)).unwrap();
        assert!(!text.contains("styles"), "{text}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn import_copies_styles_from_other_project() {
        let root = temp_dir("styles-import");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let source = create_project(&projects, "원본", None, None).unwrap();
        let target = create_project(&projects, "새 책", None, None).unwrap();

        let mut styles = Styles::default();
        styles.part.font_size = Some(2.0);
        write_styles(&source, &styles).unwrap();
        let mut own = Styles::default();
        own.body.align = Some(Align::Left);
        write_styles(&target, &own).unwrap();

        assert_eq!(import_styles(&source, &target).unwrap(), styles);
        // 덮어써서 원래 값은 남지 않는다.
        assert_eq!(read_styles(&target), styles);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn titles_inherit_body_font_and_embedding() {
        let mut styles = Styles::default();
        styles.body.font = Some("Noto Serif KR".into());
        styles.body.embed_font = Some(true);
        let chapter = styles.resolve(StyleTarget::Chapter);
        assert_eq!(chapter.font.as_deref(), Some("Noto Serif KR"));
        assert!(chapter.embed_font);

        styles.chapter.font = Some("Pretendard".into());
        styles.chapter.embed_font = Some(false);
        let chapter = styles.resolve(StyleTarget::Chapter);
        assert_eq!(chapter.font.as_deref(), Some("Pretendard"));
        assert!(!chapter.embed_font);
    }

    #[test]
    fn title_layout_defaults_and_prefix() {
        let styles = Styles::default();
        let part = styles.resolve(StyleTarget::Part);
        assert_eq!(part.placement, Placement::Inline);
        assert!(!part.page_break);
        assert_eq!(part.font_weight, 700);
        assert_eq!(
            styles.resolve(StyleTarget::Chapter).placement,
            Placement::Inline
        );
        assert!(!styles.resolve(StyleTarget::Chapter).page_break);
        assert!(!styles.resolve(StyleTarget::Section).page_break);
        assert_eq!(styles.resolve(StyleTarget::Body).font_weight, 400);

        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장 ".into());
        styles.chapter.placement = Some(Placement::Hidden);
        let text = toml::to_string(&styles).unwrap();
        assert!(text.contains("prefix = \"제{n}장 \""), "{text}");
        assert!(text.contains("placement = \"hidden\""), "{text}");
        let chapter = styles.resolve(StyleTarget::Chapter);
        assert_eq!(chapter.title_text(3, "시라 강 건너"), "제3장 시라 강 건너");
        assert_eq!(part.title_text(1, "강을 건너며"), "강을 건너며");
        assert!(chapter.in_toc);

        styles.chapter.in_toc = Some(false);
        assert!(toml::to_string(&styles).unwrap().contains("in_toc = false"));
        let chapter = styles.resolve(StyleTarget::Chapter);
        // 목차에 넣지 않아도 접두사(번호)는 붙는다.
        assert_eq!(chapter.title_text(3, "시라 강 건너"), "제3장 시라 강 건너");

        // 접두사의 줄바꿈(Enter)도 그대로 저장·복원된다.
        styles.chapter.in_toc = Some(true);
        styles.chapter.prefix = Some("제{n}장\n".into());
        let text = toml::to_string(&styles).unwrap();
        let back: Styles = toml::from_str(&text).unwrap();
        assert_eq!(back.chapter.prefix.as_deref(), Some("제{n}장\n"));
        assert_eq!(
            back.resolve(StyleTarget::Chapter)
                .title_text(3, "시라 강 건너"),
            "제3장\n시라 강 건너"
        );
        // 목차에서는 접두사의 개행을 스페이스로 바꾼다.
        assert_eq!(
            back.resolve(StyleTarget::Chapter)
                .toc_title_text(3, "시라 강 건너"),
            "제3장 시라 강 건너"
        );
    }

    #[test]
    fn roman_numbering_for_titles() {
        let mut styles = Styles::default();
        styles.chapter.prefix = Some("제{n}장 ".into());
        styles.chapter.numbering = Some(Numbering::Roman);

        // 파일에 그대로 저장·복원된다.
        let text = toml::to_string(&styles).unwrap();
        assert!(text.contains("numbering = \"roman\""), "{text}");
        let back: Styles = toml::from_str(&text).unwrap();
        assert_eq!(back.chapter.numbering, Some(Numbering::Roman));

        let chapter = back.resolve(StyleTarget::Chapter);
        assert_eq!(chapter.title_text(4, "시라 강 건너"), "제IV장 시라 강 건너");
        // 정하지 않으면 아라비아 숫자.
        let mut arabic = Styles::default();
        arabic.chapter.prefix = Some("제{n}장 ".into());
        assert_eq!(
            arabic
                .resolve(StyleTarget::Chapter)
                .title_text(4, "시라 강 건너"),
            "제4장 시라 강 건너"
        );
        // 목차에 넣지 않아도 번호·접두사는 붙는다.
        styles.chapter.in_toc = Some(false);
        assert_eq!(
            styles
                .resolve(StyleTarget::Chapter)
                .title_text(4, "시라 강 건너"),
            "제IV장 시라 강 건너"
        );
    }

    #[test]
    fn heading_numbers_restart_at_parent_when_configured() {
        // 기본은 이어서 센다.
        let mut styles = Styles::default();
        let mut numbers = HeadingNumbers::default();
        assert_eq!(numbers.count(&styles, StyleTarget::Part, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 2);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 2);
        assert_eq!(numbers.count(&styles, StyleTarget::Part, true), 2);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 3);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 3);

        // 장·절 모두 상위 제목이 바뀔 때 1부터.
        styles.chapter.restart_on_parent = Some(true);
        styles.section.restart_on_parent = Some(true);
        let mut numbers = HeadingNumbers::default();
        assert_eq!(numbers.count(&styles, StyleTarget::Part, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 2);
        // 새 장이 나오면 절이 1부터.
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 2);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 1);
        // 새 부가 나오면 장·절이 1부터.
        assert_eq!(numbers.count(&styles, StyleTarget::Part, true), 2);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Section, true), 1);

        // 번호 없는 제목(`{unnumbered}`)은 세지도, 하위 번호를 되돌리지도 않는다.
        let mut numbers = HeadingNumbers::default();
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 1);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 2);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, false), 0);
        assert_eq!(numbers.count(&styles, StyleTarget::Chapter, true), 3);
    }

    #[test]
    fn roman_numeral_conversion() {
        assert_eq!(roman(1), "I");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(9), "IX");
        assert_eq!(roman(14), "XIV");
        assert_eq!(roman(40), "XL");
        assert_eq!(roman(1987), "MCMLXXXVII");
        // 0이나 4000 이상은 아라비아 숫자를 그대로.
        assert_eq!(roman(0), "0");
        assert_eq!(roman(4000), "4000");
    }

    #[test]
    fn heading_levels_map_to_title_targets() {
        assert_eq!(StyleTarget::for_heading(1), Some(StyleTarget::Part));
        assert_eq!(StyleTarget::for_heading(2), Some(StyleTarget::Chapter));
        assert_eq!(StyleTarget::for_heading(3), Some(StyleTarget::Section));
        assert_eq!(StyleTarget::for_heading(4), None);
    }
}
