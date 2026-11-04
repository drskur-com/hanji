//! 특수 문자 및 이모지 삽입 대화상자: 조판 공백, 대시, 낫표/겹낫표, 기호, 이모지 등을 선택해 삽입한다.

use gpui_base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{
    EditorState, Input, InputEvent, InputState, MoveDown, MoveLeft, MoveRight, MoveUp,
};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, FocusableExt as _, Icon, Sizable as _, WindowExt as _,
};
use gpui_kit::gpui::prelude::{FluentBuilder as _, InteractiveElement as _};
use gpui_kit::gpui::*;
use std::cell::Cell;
use std::rc::Rc;

use super::HanjiApp;
use super::style::Palette;
use crate::{InsertSpecialChar, icons};

/// 대화상자 기본 너비.
const DIALOG_WIDTH: f32 = 600.0;
/// 격자 칸 한 변. 모든 칸이 같은 크기라 줄이 가지런하고, ↑·↓가 한 줄씩 옮겨 간다.
const CELL: f32 = 36.0;
/// 한 줄의 칸 수. 대화상자 본문 폭(`DIALOG_WIDTH` - 양쪽 안쪽 여백 16px = 568px)에서 오른쪽에
/// 세로 스크롤바 자리를 남긴다.
const COLUMNS: usize = 15;
/// 격자 영역 높이(7줄). 범주를 바꿔도 대화상자 높이가 흔들리지 않도록 고정한다.
const GRID_HEIGHT: f32 = CELL * 7.0;
/// 이름표 칸(`NBSP` 등)의 글자 크기. 네 글자가 한 칸에 들어간다.
const ABBR_TEXT_SIZE: f32 = 9.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CharCategory {
    All,
    Typesetting,
    Quotes,
    Symbols,
    Arrows,
    Math,
    Numbers,
    Emoji,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SpecialCharItem {
    pub display: &'static str,
    pub insert: &'static str,
    /// 이름·설명의 번역 키(`locales/special_char.yml`).
    pub name: &'static str,
    pub desc: &'static str,
    pub category: CharCategory,
    pub keywords: &'static str,
    pub is_pair: bool,
    pub pair_open_len: usize,
}

const fn item(
    display: &'static str,
    insert: &'static str,
    name: &'static str,
    desc: &'static str,
    category: CharCategory,
    keywords: &'static str,
) -> SpecialCharItem {
    SpecialCharItem {
        display,
        insert,
        name,
        desc,
        category,
        keywords,
        is_pair: false,
        pair_open_len: 0,
    }
}

const fn pair(
    display: &'static str,
    insert: &'static str,
    name: &'static str,
    desc: &'static str,
    category: CharCategory,
    keywords: &'static str,
    pair_open_len: usize,
) -> SpecialCharItem {
    SpecialCharItem {
        display,
        insert,
        name,
        desc,
        category,
        keywords,
        is_pair: true,
        pair_open_len,
    }
}

/// 칸에 그릴 내용의 종류. 글자 수·바이트 수가 아니라 보이는 글자 수로 정한다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TileKind {
    /// `NBSP`, `ZWSP`처럼 글자 대신 이름을 쓰는 칸.
    Abbr,
    /// `「…」`처럼 괄호 싸개를 보여 주는 칸.
    Pair,
    /// 글자 하나(이모지·문장부호).
    Glyph,
}

/// 이모지의 변이 선택자·결합자를 뺀, 실제로 보이는 글자 수.
fn glyph_count(display: &str) -> usize {
    display
        .chars()
        .filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}' | '\u{200D}'))
        .count()
}

fn tile_kind(item: &SpecialCharItem) -> TileKind {
    let ascii_name = item.display.len() >= 2
        && item
            .display
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ');
    if ascii_name {
        TileKind::Abbr
    } else if glyph_count(item.display) <= 1 {
        TileKind::Glyph
    } else {
        TileKind::Pair
    }
}

/// 설명줄에 보여 줄 코드 포인트(`U+2009`). 싸개는 여는 글자 기준.
pub(crate) fn code_point(item: &SpecialCharItem) -> String {
    item.insert
        .chars()
        .next()
        .map(|c| format!("U+{:04X}", c as u32))
        .unwrap_or_default()
}

pub(crate) static SPECIAL_CHARS: &[SpecialCharItem] = &[
    // --- 조판 및 공백 (Typesetting & Spaces) ---
    item(
        "NBSP",
        "\u{00A0}",
        "special_char.u00a0.name",
        "special_char.u00a0.desc",
        CharCategory::Typesetting,
        "공백 nbsp space 줄바꿈 non-breaking",
    ),
    item(
        "Thin",
        "\u{2009}",
        "special_char.u2009.name",
        "special_char.u2009.desc",
        CharCategory::Typesetting,
        "공백 thin space 얇은",
    ),
    item(
        "En Sp",
        "\u{2002}",
        "special_char.u2002.name",
        "special_char.u2002.desc",
        CharCategory::Typesetting,
        "공백 en space 반각",
    ),
    item(
        "Em Sp",
        "\u{2003}",
        "special_char.u2003.name",
        "special_char.u2003.desc",
        CharCategory::Typesetting,
        "공백 em space 전각",
    ),
    item(
        "ZWSP",
        "\u{200B}",
        "special_char.u200b.name",
        "special_char.u200b.desc",
        CharCategory::Typesetting,
        "공백 zwsp zero width space 줄바꿈",
    ),
    item(
        "WJ",
        "\u{2060}",
        "special_char.u2060.name",
        "special_char.u2060.desc",
        CharCategory::Typesetting,
        "공백 결합방지 wj word joiner",
    ),
    item(
        "SHY",
        "\u{00AD}",
        "special_char.u00ad.name",
        "special_char.u00ad.desc",
        CharCategory::Typesetting,
        "하이픈 shy soft hyphen 붙임표",
    ),
    item(
        "‑",
        "\u{2011}",
        "special_char.u2011.name",
        "special_char.u2011.desc",
        CharCategory::Typesetting,
        "하이픈 non-breaking hyphen 붙임표",
    ),
    item(
        "—",
        "—",
        "special_char.u2014.name",
        "special_char.u2014.desc",
        CharCategory::Typesetting,
        "대시 em dash 줄표",
    ),
    item(
        "–",
        "–",
        "special_char.u2013.name",
        "special_char.u2013.desc",
        CharCategory::Typesetting,
        "대시 en dash 줄표 기간 범위",
    ),
    item(
        "…",
        "…",
        "special_char.u2026.name",
        "special_char.u2026.desc",
        CharCategory::Typesetting,
        "말줄임표 ellipsis 점 리더",
    ),
    item(
        "‥",
        "‥",
        "special_char.u2025.name",
        "special_char.u2025.desc",
        CharCategory::Typesetting,
        "말줄임표 이점 점",
    ),
    item(
        "·",
        "·",
        "special_char.u00b7.name",
        "special_char.u00b7.desc",
        CharCategory::Typesetting,
        "가운뎃점 middle dot 점",
    ),
    item(
        "•",
        "•",
        "special_char.u2022.name",
        "special_char.u2022.desc",
        CharCategory::Typesetting,
        "불릿 bullet 목록 점",
    ),
    item(
        "※",
        "※",
        "special_char.u203b.name",
        "special_char.u203b.desc",
        CharCategory::Typesetting,
        "참고표 당구장 주석",
    ),
    item(
        "¶",
        "¶",
        "special_char.u00b6.name",
        "special_char.u00b6.desc",
        CharCategory::Typesetting,
        "단락 문단 pilcrow paragraph",
    ),
    item(
        "§",
        "§",
        "special_char.u00a7.name",
        "special_char.u00a7.desc",
        CharCategory::Typesetting,
        "절 섹션 section 조항",
    ),
    item(
        "†",
        "†",
        "special_char.u2020.name",
        "special_char.u2020.desc",
        CharCategory::Typesetting,
        "칼표 dagger 각주",
    ),
    item(
        "‡",
        "‡",
        "special_char.u2021.name",
        "special_char.u2021.desc",
        CharCategory::Typesetting,
        "칼표 double dagger 각주",
    ),
    item(
        "Fig",
        "\u{2007}",
        "special_char.u2007.name",
        "special_char.u2007.desc",
        CharCategory::Typesetting,
        "숫자 공백 figure space",
    ),
    item(
        "Hair",
        "\u{200A}",
        "special_char.u200a.name",
        "special_char.u200a.desc",
        CharCategory::Typesetting,
        "머리카락 아주 좁은 공백 hair space",
    ),
    item(
        "NNBSP",
        "\u{202F}",
        "special_char.u202f.name",
        "special_char.u202f.desc",
        CharCategory::Typesetting,
        "좁은 줄바꿈 없는 공백 narrow nbsp nnbsp no-break space",
    ),
    item(
        "IDSP",
        "\u{3000}",
        "special_char.u3000.name",
        "special_char.u3000.desc",
        CharCategory::Typesetting,
        "전각 공백 ideographic space",
    ),
    item(
        "‐",
        "‐",
        "special_char.u2010.name",
        "special_char.u2010.desc",
        CharCategory::Typesetting,
        "하이픈 붙임표 hyphen",
    ),
    item(
        "‒",
        "‒",
        "special_char.u2012.name",
        "special_char.u2012.desc",
        CharCategory::Typesetting,
        "숫자 대시 figure dash",
    ),
    item(
        "―",
        "―",
        "special_char.u2015.name",
        "special_char.u2015.desc",
        CharCategory::Typesetting,
        "가로 막대 줄표 horizontal bar quotation dash",
    ),
    item(
        "⁓",
        "⁓",
        "special_char.u2053.name",
        "special_char.u2053.desc",
        CharCategory::Typesetting,
        "물결 swung dash 대시",
    ),
    item(
        "〜",
        "〜",
        "special_char.u301c.name",
        "special_char.u301c.desc",
        CharCategory::Typesetting,
        "물결표 범위 wave dash tilde",
    ),
    item(
        "～",
        "～",
        "special_char.uff5e.name",
        "special_char.uff5e.desc",
        CharCategory::Typesetting,
        "전각 물결 tilde fullwidth 물결표",
    ),
    item(
        "⋯",
        "⋯",
        "special_char.u22ef.name",
        "special_char.u22ef.desc",
        CharCategory::Typesetting,
        "줄임표 말줄임 가운데 midline ellipsis",
    ),
    item(
        "‣",
        "‣",
        "special_char.u2023.name",
        "special_char.u2023.desc",
        CharCategory::Typesetting,
        "글머리 세모 bullet triangle 글머리표 triangular",
    ),
    item(
        "◦",
        "◦",
        "special_char.u25e6.name",
        "special_char.u25e6.desc",
        CharCategory::Typesetting,
        "글머리 흰 동그라미 white bullet 글머리표",
    ),
    item(
        "⁃",
        "⁃",
        "special_char.u2043.name",
        "special_char.u2043.desc",
        CharCategory::Typesetting,
        "글머리 하이픈 hyphen bullet 글머리표",
    ),
    item(
        "⁂",
        "⁂",
        "special_char.u2042.name",
        "special_char.u2042.desc",
        CharCategory::Typesetting,
        "별 장면 전환 asterism 세 개",
    ),
    item(
        "‖",
        "‖",
        "special_char.u2016.name",
        "special_char.u2016.desc",
        CharCategory::Typesetting,
        "세로선 이중 double vertical bar line",
    ),
    item(
        "〃",
        "〃",
        "special_char.u3003.name",
        "special_char.u3003.desc",
        CharCategory::Typesetting,
        "같음표 위와 같음 ditto mark",
    ),
    // --- 따옴표 및 괄호 (Quotes & Brackets) ---
    pair(
        "「…」",
        "「」",
        "special_char.u300c_300d.name",
        "special_char.u300c_300d.desc",
        CharCategory::Quotes,
        "낫표 괄호 인용",
        3,
    ),
    pair(
        "『…』",
        "『』",
        "special_char.u300e_300f.name",
        "special_char.u300e_300f.desc",
        CharCategory::Quotes,
        "겹낫표 책제목 괄호 인용",
        3,
    ),
    pair(
        "〈…〉",
        "〈〉",
        "special_char.u3008_3009.name",
        "special_char.u3008_3009.desc",
        CharCategory::Quotes,
        "홑화살괄호 꺾쇠 괄호",
        3,
    ),
    pair(
        "《…》",
        "《》",
        "special_char.u300a_300b.name",
        "special_char.u300a_300b.desc",
        CharCategory::Quotes,
        "겹화살괄호 책제목 꺾쇠",
        3,
    ),
    pair(
        "“…”",
        "“”",
        "special_char.u201c_201d.name",
        "special_char.u201c_201d.desc",
        CharCategory::Quotes,
        "큰따옴표 quote 인용",
        3,
    ),
    pair(
        "‘…’",
        "‘’",
        "special_char.u2018_2019.name",
        "special_char.u2018_2019.desc",
        CharCategory::Quotes,
        "작은따옴표 quote 인용",
        3,
    ),
    pair(
        "«…»",
        "«»",
        "special_char.u00ab_00bb.name",
        "special_char.u00ab_00bb.desc",
        CharCategory::Quotes,
        "기예메 guillemet 인용",
        2,
    ),
    pair(
        "【…】",
        "【】",
        "special_char.u3010_3011.name",
        "special_char.u3010_3011.desc",
        CharCategory::Quotes,
        "볼드 꺾쇠 괄호",
        3,
    ),
    pair(
        "〔…〕",
        "〔〕",
        "special_char.u3014_3015.name",
        "special_char.u3014_3015.desc",
        CharCategory::Quotes,
        "거북등 괄호 귀호",
        3,
    ),
    item(
        "「",
        "「",
        "special_char.u300c.name",
        "special_char.u300c.desc",
        CharCategory::Quotes,
        "낫표 괄호 인용",
    ),
    item(
        "」",
        "」",
        "special_char.u300d.name",
        "special_char.u300d.desc",
        CharCategory::Quotes,
        "낫표 괄호 인용",
    ),
    item(
        "『",
        "『",
        "special_char.u300e.name",
        "special_char.u300e.desc",
        CharCategory::Quotes,
        "겹낫표 괄호 책제목",
    ),
    item(
        "』",
        "』",
        "special_char.u300f.name",
        "special_char.u300f.desc",
        CharCategory::Quotes,
        "겹낫표 괄호 책제목",
    ),
    item(
        "〈",
        "〈",
        "special_char.u3008.name",
        "special_char.u3008.desc",
        CharCategory::Quotes,
        "홑화살괄호 꺾쇠",
    ),
    item(
        "〉",
        "〉",
        "special_char.u3009.name",
        "special_char.u3009.desc",
        CharCategory::Quotes,
        "홑화살괄호 꺾쇠",
    ),
    item(
        "《",
        "《",
        "special_char.u300a.name",
        "special_char.u300a.desc",
        CharCategory::Quotes,
        "겹화살괄호 꺾쇠 책제목",
    ),
    item(
        "》",
        "》",
        "special_char.u300b.name",
        "special_char.u300b.desc",
        CharCategory::Quotes,
        "겹화살괄호 꺾쇠 책제목",
    ),
    item(
        "“",
        "“",
        "special_char.u201c.name",
        "special_char.u201c.desc",
        CharCategory::Quotes,
        "큰따옴표 quote 인용",
    ),
    item(
        "”",
        "”",
        "special_char.u201d.name",
        "special_char.u201d.desc",
        CharCategory::Quotes,
        "큰따옴표 quote 인용",
    ),
    item(
        "‘",
        "‘",
        "special_char.u2018.name",
        "special_char.u2018.desc",
        CharCategory::Quotes,
        "작은따옴표 quote 인용",
    ),
    item(
        "’",
        "’",
        "special_char.u2019.name",
        "special_char.u2019.desc",
        CharCategory::Quotes,
        "작은따옴표 quote 인용",
    ),
    pair(
        "（…）",
        "（）",
        "special_char.uff08_ff09.name",
        "special_char.uff08_ff09.desc",
        CharCategory::Quotes,
        "괄호 소괄호 전각 parentheses fullwidth",
        3,
    ),
    pair(
        "［…］",
        "［］",
        "special_char.uff3b_ff3d.name",
        "special_char.uff3b_ff3d.desc",
        CharCategory::Quotes,
        "괄호 대괄호 전각 square brackets fullwidth",
        3,
    ),
    pair(
        "｛…｝",
        "｛｝",
        "special_char.uff5b_ff5d.name",
        "special_char.uff5b_ff5d.desc",
        CharCategory::Quotes,
        "괄호 중괄호 전각 curly braces fullwidth brackets",
        3,
    ),
    pair(
        "〖…〗",
        "〖〗",
        "special_char.u3016_3017.name",
        "special_char.u3016_3017.desc",
        CharCategory::Quotes,
        "괄호 렌즈 lenticular brackets 흰 white",
        3,
    ),
    pair(
        "〘…〙",
        "〘〙",
        "special_char.u3018_3019.name",
        "special_char.u3018_3019.desc",
        CharCategory::Quotes,
        "괄호 거북 tortoise shell brackets 흰 거북등 white",
        3,
    ),
    pair(
        "｢…｣",
        "｢｣",
        "special_char.uff62_ff63.name",
        "special_char.uff62_ff63.desc",
        CharCategory::Quotes,
        "낫표 반각 halfwidth corner brackets",
        3,
    ),
    pair(
        "‹…›",
        "‹›",
        "special_char.u2039_203a.name",
        "special_char.u2039_203a.desc",
        CharCategory::Quotes,
        "따옴표 꺾쇠 guillemet single angle 홑화살괄호 ‹ › guillemets",
        3,
    ),
    pair(
        "„…“",
        "„“",
        "special_char.u201e_201c.name",
        "special_char.u201e_201c.desc",
        CharCategory::Quotes,
        "따옴표 독일 low double quotes german 아래 큰따옴표 „ “",
        3,
    ),
    pair(
        "‚…‘",
        "‚‘",
        "special_char.u201a_2018.name",
        "special_char.u201a_2018.desc",
        CharCategory::Quotes,
        "따옴표 독일 low single quotes german 아래 작은따옴표 ‚ ‘",
        3,
    ),
    pair(
        "⟨…⟩",
        "⟨⟩",
        "special_char.u27e8_27e9.name",
        "special_char.u27e8_27e9.desc",
        CharCategory::Quotes,
        "꺾쇠 괄호 수학 angle brackets mathematical",
        3,
    ),
    item(
        "【",
        "【",
        "special_char.u3010.name",
        "special_char.u3010.desc",
        CharCategory::Quotes,
        "괄호 렌즈 lenticular 왼쪽 left black bracket",
    ),
    item(
        "】",
        "】",
        "special_char.u3011.name",
        "special_char.u3011.desc",
        CharCategory::Quotes,
        "괄호 렌즈 lenticular 오른쪽 right black bracket",
    ),
    item(
        "〔",
        "〔",
        "special_char.u3014.name",
        "special_char.u3014.desc",
        CharCategory::Quotes,
        "괄호 거북 tortoise 왼쪽 거북등 left shell bracket",
    ),
    item(
        "〕",
        "〕",
        "special_char.u3015.name",
        "special_char.u3015.desc",
        CharCategory::Quotes,
        "괄호 거북 tortoise 오른쪽 거북등 right shell bracket",
    ),
    item(
        "«",
        "«",
        "special_char.u00ab.name",
        "special_char.u00ab.desc",
        CharCategory::Quotes,
        "따옴표 꺾쇠 guillemet 왼쪽 겹화살괄호 left",
    ),
    item(
        "»",
        "»",
        "special_char.u00bb.name",
        "special_char.u00bb.desc",
        CharCategory::Quotes,
        "따옴표 꺾쇠 guillemet 오른쪽 겹화살괄호 right",
    ),
    // --- 기호 및 단위 (Symbols & Units) ---
    item(
        "©",
        "©",
        "special_char.u00a9.name",
        "special_char.u00a9.desc",
        CharCategory::Symbols,
        "저작권 copyright copy c",
    ),
    item(
        "®",
        "®",
        "special_char.u00ae.name",
        "special_char.u00ae.desc",
        CharCategory::Symbols,
        "등록상표 registered r tm",
    ),
    item(
        "™",
        "™",
        "special_char.u2122.name",
        "special_char.u2122.desc",
        CharCategory::Symbols,
        "상표 trademark tm",
    ),
    item(
        "°",
        "°",
        "special_char.u00b0.name",
        "special_char.u00b0.desc",
        CharCategory::Symbols,
        "도 degree 각도 온도",
    ),
    item(
        "′",
        "′",
        "special_char.u2032.name",
        "special_char.u2032.desc",
        CharCategory::Symbols,
        "프라임 분 prime foot",
    ),
    item(
        "″",
        "″",
        "special_char.u2033.name",
        "special_char.u2033.desc",
        CharCategory::Symbols,
        "더블프라임 초 inch double prime",
    ),
    item(
        "℃",
        "℃",
        "special_char.u2103.name",
        "special_char.u2103.desc",
        CharCategory::Symbols,
        "온도 섭씨 celcius",
    ),
    item(
        "℉",
        "℉",
        "special_char.u2109.name",
        "special_char.u2109.desc",
        CharCategory::Symbols,
        "온도 화씨 fahrenheit",
    ),
    item(
        "‰",
        "‰",
        "special_char.u2030.name",
        "special_char.u2030.desc",
        CharCategory::Symbols,
        "퍼밀 천분율 permil",
    ),
    item(
        "₩",
        "₩",
        "special_char.u20a9.name",
        "special_char.u20a9.desc",
        CharCategory::Symbols,
        "원 통화 money won krw",
    ),
    item(
        "$",
        "$",
        "special_char.u0024.name",
        "special_char.u0024.desc",
        CharCategory::Symbols,
        "달러 통화 money dollar usd",
    ),
    item(
        "€",
        "€",
        "special_char.u20ac.name",
        "special_char.u20ac.desc",
        CharCategory::Symbols,
        "유로 통화 money euro eur",
    ),
    item(
        "£",
        "£",
        "special_char.u00a3.name",
        "special_char.u00a3.desc",
        CharCategory::Symbols,
        "파운드 통화 money pound gbp",
    ),
    item(
        "¥",
        "¥",
        "special_char.u00a5.name",
        "special_char.u00a5.desc",
        CharCategory::Symbols,
        "엔 위안 통화 yen yuan",
    ),
    item(
        "✓",
        "✓",
        "special_char.u2713.name",
        "special_char.u2713.desc",
        CharCategory::Symbols,
        "체크 check mark",
    ),
    item(
        "★",
        "★",
        "special_char.u2605.name",
        "special_char.u2605.desc",
        CharCategory::Symbols,
        "별 star",
    ),
    item(
        "☆",
        "☆",
        "special_char.u2606.name",
        "special_char.u2606.desc",
        CharCategory::Symbols,
        "별 star",
    ),
    item(
        "◆",
        "◆",
        "special_char.u25c6.name",
        "special_char.u25c6.desc",
        CharCategory::Symbols,
        "다이아몬드 diamond",
    ),
    item(
        "◇",
        "◇",
        "special_char.u25c7.name",
        "special_char.u25c7.desc",
        CharCategory::Symbols,
        "다이아몬드 diamond",
    ),
    item(
        "■",
        "■",
        "special_char.u25a0.name",
        "special_char.u25a0.desc",
        CharCategory::Symbols,
        "네모 square",
    ),
    item(
        "□",
        "□",
        "special_char.u25a1.name",
        "special_char.u25a1.desc",
        CharCategory::Symbols,
        "네모 square",
    ),
    item(
        "¢",
        "¢",
        "special_char.u00a2.name",
        "special_char.u00a2.desc",
        CharCategory::Symbols,
        "화폐 통화 센트 cent currency",
    ),
    item(
        "₹",
        "₹",
        "special_char.u20b9.name",
        "special_char.u20b9.desc",
        CharCategory::Symbols,
        "화폐 통화 루피 rupee currency 인도 indian",
    ),
    item(
        "₽",
        "₽",
        "special_char.u20bd.name",
        "special_char.u20bd.desc",
        CharCategory::Symbols,
        "화폐 통화 루블 ruble currency 러시아",
    ),
    item(
        "₿",
        "₿",
        "special_char.u20bf.name",
        "special_char.u20bf.desc",
        CharCategory::Symbols,
        "화폐 비트코인 bitcoin btc currency",
    ),
    item(
        "µ",
        "µ",
        "special_char.u00b5.name",
        "special_char.u00b5.desc",
        CharCategory::Symbols,
        "마이크로 단위 micro mu",
    ),
    item(
        "ℓ",
        "ℓ",
        "special_char.u2113.name",
        "special_char.u2113.desc",
        CharCategory::Symbols,
        "리터 단위 liter litre ℓ script small l",
    ),
    item(
        "㎜",
        "㎜",
        "special_char.u339c.name",
        "special_char.u339c.desc",
        CharCategory::Symbols,
        "밀리미터 단위 mm millimeter ㎜ square",
    ),
    item(
        "㎝",
        "㎝",
        "special_char.u339d.name",
        "special_char.u339d.desc",
        CharCategory::Symbols,
        "센티미터 단위 cm centimeter ㎝ square",
    ),
    item(
        "㎞",
        "㎞",
        "special_char.u339e.name",
        "special_char.u339e.desc",
        CharCategory::Symbols,
        "킬로미터 단위 km kilometer ㎞ square",
    ),
    item(
        "㎎",
        "㎎",
        "special_char.u338e.name",
        "special_char.u338e.desc",
        CharCategory::Symbols,
        "밀리그램 단위 mg milligram ㎎ square",
    ),
    item(
        "㎏",
        "㎏",
        "special_char.u338f.name",
        "special_char.u338f.desc",
        CharCategory::Symbols,
        "킬로그램 단위 kg kilogram ㎏ square",
    ),
    item(
        "㎖",
        "㎖",
        "special_char.u3396.name",
        "special_char.u3396.desc",
        CharCategory::Symbols,
        "밀리리터 단위 ml milliliter ㎖ square",
    ),
    item(
        "㎡",
        "㎡",
        "special_char.u33a1.name",
        "special_char.u33a1.desc",
        CharCategory::Symbols,
        "제곱미터 단위 넓이 m2 square meter ㎡ m²",
    ),
    item(
        "㎥",
        "㎥",
        "special_char.u33a5.name",
        "special_char.u33a5.desc",
        CharCategory::Symbols,
        "세제곱미터 단위 부피 m3 cubic meter ㎥ square m³",
    ),
    item(
        "㏄",
        "㏄",
        "special_char.u33c4.name",
        "special_char.u33c4.desc",
        CharCategory::Symbols,
        "시시 단위 cc ㏄ square",
    ),
    item(
        "Å",
        "Å",
        "special_char.u212b.name",
        "special_char.u212b.desc",
        CharCategory::Symbols,
        "옹스트롬 단위 angstrom",
    ),
    item(
        "№",
        "№",
        "special_char.u2116.name",
        "special_char.u2116.desc",
        CharCategory::Symbols,
        "번호 넘버 numero number 기호 № sign",
    ),
    item(
        "℡",
        "℡",
        "special_char.u2121.name",
        "special_char.u2121.desc",
        CharCategory::Symbols,
        "전화 tel telephone 기호 ℡ sign",
    ),
    item(
        "㈜",
        "㈜",
        "special_char.u321c.name",
        "special_char.u321c.desc",
        CharCategory::Symbols,
        "주식회사 회사 corporation ㈜ parenthesized",
    ),
    item(
        "㏂",
        "㏂",
        "special_char.u33c2.name",
        "special_char.u33c2.desc",
        CharCategory::Symbols,
        "오전 am square a.m.",
    ),
    item(
        "㏘",
        "㏘",
        "special_char.u33d8.name",
        "special_char.u33d8.desc",
        CharCategory::Symbols,
        "오후 pm square p.m.",
    ),
    item(
        "℗",
        "℗",
        "special_char.u2117.name",
        "special_char.u2117.desc",
        CharCategory::Symbols,
        "음반 저작권 phonogram copyright ℗ sound recording",
    ),
    item(
        "℠",
        "℠",
        "special_char.u2120.name",
        "special_char.u2120.desc",
        CharCategory::Symbols,
        "서비스표 service mark sm ℠",
    ),
    item(
        "¡",
        "¡",
        "special_char.u00a1.name",
        "special_char.u00a1.desc",
        CharCategory::Symbols,
        "느낌표 거꾸로 스페인 inverted exclamation mark",
    ),
    item(
        "¿",
        "¿",
        "special_char.u00bf.name",
        "special_char.u00bf.desc",
        CharCategory::Symbols,
        "물음표 거꾸로 스페인 inverted question mark",
    ),
    item(
        "‽",
        "‽",
        "special_char.u203d.name",
        "special_char.u203d.desc",
        CharCategory::Symbols,
        "물음표 느낌표 interrobang 물음느낌표",
    ),
    item(
        "♀",
        "♀",
        "special_char.u2640.name",
        "special_char.u2640.desc",
        CharCategory::Symbols,
        "여성 female venus 기호 sign",
    ),
    item(
        "♂",
        "♂",
        "special_char.u2642.name",
        "special_char.u2642.desc",
        CharCategory::Symbols,
        "남성 male mars 기호 sign",
    ),
    item(
        "♪",
        "♪",
        "special_char.u266a.name",
        "special_char.u266a.desc",
        CharCategory::Symbols,
        "음표 음악 note music eighth",
    ),
    item(
        "♫",
        "♫",
        "special_char.u266b.name",
        "special_char.u266b.desc",
        CharCategory::Symbols,
        "음표 음악 notes music 겹음표 beamed eighth",
    ),
    item(
        "☎",
        "☎",
        "special_char.u260e.name",
        "special_char.u260e.desc",
        CharCategory::Symbols,
        "전화 phone telephone 전화기",
    ),
    item(
        "✉",
        "✉",
        "special_char.u2709.name",
        "special_char.u2709.desc",
        CharCategory::Symbols,
        "편지 봉투 메일 envelope mail",
    ),
    item(
        "✂",
        "✂",
        "special_char.u2702.name",
        "special_char.u2702.desc",
        CharCategory::Symbols,
        "가위 자르기 scissors cut",
    ),
    item(
        "☞",
        "☞",
        "special_char.u261e.name",
        "special_char.u261e.desc",
        CharCategory::Symbols,
        "손가락 가리키기 오른쪽 pointing hand 가리키는 손 right",
    ),
    item(
        "☜",
        "☜",
        "special_char.u261c.name",
        "special_char.u261c.desc",
        CharCategory::Symbols,
        "손가락 가리키기 왼쪽 pointing hand 가리키는 손 left",
    ),
    item(
        "⌘",
        "⌘",
        "special_char.u2318.name",
        "special_char.u2318.desc",
        CharCategory::Symbols,
        "커맨드 키 단축키 command cmd key ⌘",
    ),
    item(
        "⌥",
        "⌥",
        "special_char.u2325.name",
        "special_char.u2325.desc",
        CharCategory::Symbols,
        "옵션 alt 키 단축키 option key ⌥",
    ),
    item(
        "⇧",
        "⇧",
        "special_char.u21e7.name",
        "special_char.u21e7.desc",
        CharCategory::Symbols,
        "시프트 키 단축키 shift key ⇧",
    ),
    item(
        "⌃",
        "⌃",
        "special_char.u2303.name",
        "special_char.u2303.desc",
        CharCategory::Symbols,
        "컨트롤 ctrl 키 단축키 control key ⌃",
    ),
    item(
        "⏎",
        "⏎",
        "special_char.u23ce.name",
        "special_char.u23ce.desc",
        CharCategory::Symbols,
        "리턴 엔터 키 return enter key ⏎",
    ),
    item(
        "⌫",
        "⌫",
        "special_char.u232b.name",
        "special_char.u232b.desc",
        CharCategory::Symbols,
        "지우기 백스페이스 키 delete backspace key ⌫",
    ),
    item(
        "⎋",
        "⎋",
        "special_char.u238b.name",
        "special_char.u238b.desc",
        CharCategory::Symbols,
        "이스케이프 esc 키 escape key ⎋",
    ),
    item(
        "✔",
        "✔",
        "special_char.u2714.name",
        "special_char.u2714.desc",
        CharCategory::Symbols,
        "체크 확인 check mark 굵은 표시 heavy",
    ),
    item(
        "✗",
        "✗",
        "special_char.u2717.name",
        "special_char.u2717.desc",
        CharCategory::Symbols,
        "가위표 엑스 틀림 ballot x cross ✗",
    ),
    item(
        "✘",
        "✘",
        "special_char.u2718.name",
        "special_char.u2718.desc",
        CharCategory::Symbols,
        "가위표 엑스 틀림 heavy x cross 굵은 ballot",
    ),
    item(
        "☐",
        "☐",
        "special_char.u2610.name",
        "special_char.u2610.desc",
        CharCategory::Symbols,
        "체크 상자 할일 ballot box checkbox 빈",
    ),
    item(
        "☑",
        "☑",
        "special_char.u2611.name",
        "special_char.u2611.desc",
        CharCategory::Symbols,
        "체크 상자 완료 checked box 체크한 ballot with check",
    ),
    item(
        "☒",
        "☒",
        "special_char.u2612.name",
        "special_char.u2612.desc",
        CharCategory::Symbols,
        "가위표 상자 ballot box x with",
    ),
    item(
        "○",
        "○",
        "special_char.u25cb.name",
        "special_char.u25cb.desc",
        CharCategory::Symbols,
        "동그라미 원 흰 circle white",
    ),
    item(
        "●",
        "●",
        "special_char.u25cf.name",
        "special_char.u25cf.desc",
        CharCategory::Symbols,
        "동그라미 원 검은 circle black",
    ),
    item(
        "◎",
        "◎",
        "special_char.u25ce.name",
        "special_char.u25ce.desc",
        CharCategory::Symbols,
        "겹동그라미 이중 원 bullseye",
    ),
    item(
        "◉",
        "◉",
        "special_char.u25c9.name",
        "special_char.u25c9.desc",
        CharCategory::Symbols,
        "동그라미 원 fisheye 가운데 찬",
    ),
    item(
        "△",
        "△",
        "special_char.u25b3.name",
        "special_char.u25b3.desc",
        CharCategory::Symbols,
        "세모 삼각형 흰 triangle white up",
    ),
    item(
        "▲",
        "▲",
        "special_char.u25b2.name",
        "special_char.u25b2.desc",
        CharCategory::Symbols,
        "세모 삼각형 검은 triangle black up",
    ),
    item(
        "▽",
        "▽",
        "special_char.u25bd.name",
        "special_char.u25bd.desc",
        CharCategory::Symbols,
        "역세모 삼각형 흰 triangle down white",
    ),
    item(
        "▼",
        "▼",
        "special_char.u25bc.name",
        "special_char.u25bc.desc",
        CharCategory::Symbols,
        "역세모 삼각형 검은 triangle down black",
    ),
    item(
        "◁",
        "◁",
        "special_char.u25c1.name",
        "special_char.u25c1.desc",
        CharCategory::Symbols,
        "세모 왼쪽 triangle left 흰 white",
    ),
    item(
        "◀",
        "◀",
        "special_char.u25c0.name",
        "special_char.u25c0.desc",
        CharCategory::Symbols,
        "세모 왼쪽 triangle left 검은 black",
    ),
    item(
        "▷",
        "▷",
        "special_char.u25b7.name",
        "special_char.u25b7.desc",
        CharCategory::Symbols,
        "세모 오른쪽 재생 triangle right play 흰 white",
    ),
    item(
        "▶",
        "▶",
        "special_char.u25b6.name",
        "special_char.u25b6.desc",
        CharCategory::Symbols,
        "세모 오른쪽 재생 triangle right play 검은 black",
    ),
    item(
        "▣",
        "▣",
        "special_char.u25a3.name",
        "special_char.u25a3.desc",
        CharCategory::Symbols,
        "네모 사각형 겹 square 겹네모 white with black",
    ),
    item(
        "◈",
        "◈",
        "special_char.u25c8.name",
        "special_char.u25c8.desc",
        CharCategory::Symbols,
        "마름모 겹 diamond 겹마름모 in",
    ),
    item(
        "✦",
        "✦",
        "special_char.u2726.name",
        "special_char.u2726.desc",
        CharCategory::Symbols,
        "별 반짝 sparkle star four 검은 네 꼭지 black pointed",
    ),
    item(
        "✧",
        "✧",
        "special_char.u2727.name",
        "special_char.u2727.desc",
        CharCategory::Symbols,
        "별 반짝 sparkle star four 흰 네 꼭지 white pointed",
    ),
    item(
        "♠",
        "♠",
        "special_char.u2660.name",
        "special_char.u2660.desc",
        CharCategory::Symbols,
        "스페이드 카드 spade card 검은 black",
    ),
    item(
        "♤",
        "♤",
        "special_char.u2664.name",
        "special_char.u2664.desc",
        CharCategory::Symbols,
        "스페이드 카드 spade card 흰 white",
    ),
    item(
        "♥",
        "♥",
        "special_char.u2665.name",
        "special_char.u2665.desc",
        CharCategory::Symbols,
        "하트 카드 사랑 heart card 검은 black suit",
    ),
    item(
        "♡",
        "♡",
        "special_char.u2661.name",
        "special_char.u2661.desc",
        CharCategory::Symbols,
        "하트 카드 사랑 heart card 흰 white suit",
    ),
    item(
        "♦",
        "♦",
        "special_char.u2666.name",
        "special_char.u2666.desc",
        CharCategory::Symbols,
        "다이아몬드 카드 diamond card 검은 black suit",
    ),
    item(
        "♢",
        "♢",
        "special_char.u2662.name",
        "special_char.u2662.desc",
        CharCategory::Symbols,
        "다이아몬드 카드 diamond card 흰 white suit",
    ),
    item(
        "♣",
        "♣",
        "special_char.u2663.name",
        "special_char.u2663.desc",
        CharCategory::Symbols,
        "클로버 카드 club card 검은 black",
    ),
    item(
        "♧",
        "♧",
        "special_char.u2667.name",
        "special_char.u2667.desc",
        CharCategory::Symbols,
        "클로버 카드 club card 흰 white",
    ),
    item(
        "☀",
        "☀",
        "special_char.u2600.name",
        "special_char.u2600.desc",
        CharCategory::Symbols,
        "해 태양 맑음 sun",
    ),
    item(
        "☁",
        "☁",
        "special_char.u2601.name",
        "special_char.u2601.desc",
        CharCategory::Symbols,
        "구름 흐림 cloud",
    ),
    item(
        "☂",
        "☂",
        "special_char.u2602.name",
        "special_char.u2602.desc",
        CharCategory::Symbols,
        "우산 비 umbrella rain",
    ),
    // --- 화살표 (Arrows) ---
    item(
        "←",
        "←",
        "special_char.u2190.name",
        "special_char.u2190.desc",
        CharCategory::Arrows,
        "화살표 arrow left",
    ),
    item(
        "→",
        "→",
        "special_char.u2192.name",
        "special_char.u2192.desc",
        CharCategory::Arrows,
        "화살표 arrow right",
    ),
    item(
        "↑",
        "↑",
        "special_char.u2191.name",
        "special_char.u2191.desc",
        CharCategory::Arrows,
        "화살표 arrow up",
    ),
    item(
        "↓",
        "↓",
        "special_char.u2193.name",
        "special_char.u2193.desc",
        CharCategory::Arrows,
        "화살표 arrow down",
    ),
    item(
        "↔",
        "↔",
        "special_char.u2194.name",
        "special_char.u2194.desc",
        CharCategory::Arrows,
        "화살표 arrow both",
    ),
    item(
        "⇒",
        "⇒",
        "special_char.u21d2.name",
        "special_char.u21d2.desc",
        CharCategory::Arrows,
        "화살표 implies arrow",
    ),
    item(
        "↕",
        "↕",
        "special_char.u2195.name",
        "special_char.u2195.desc",
        CharCategory::Arrows,
        "위아래 화살표 arrow up down",
    ),
    item(
        "↖",
        "↖",
        "special_char.u2196.name",
        "special_char.u2196.desc",
        CharCategory::Arrows,
        "왼쪽 위 대각선 화살표 arrow north west",
    ),
    item(
        "↗",
        "↗",
        "special_char.u2197.name",
        "special_char.u2197.desc",
        CharCategory::Arrows,
        "오른쪽 위 대각선 상승 화살표 arrow north east",
    ),
    item(
        "↘",
        "↘",
        "special_char.u2198.name",
        "special_char.u2198.desc",
        CharCategory::Arrows,
        "오른쪽 아래 대각선 하락 화살표 arrow south east",
    ),
    item(
        "↙",
        "↙",
        "special_char.u2199.name",
        "special_char.u2199.desc",
        CharCategory::Arrows,
        "왼쪽 아래 대각선 화살표 arrow south west",
    ),
    item(
        "⇐",
        "⇐",
        "special_char.u21d0.name",
        "special_char.u21d0.desc",
        CharCategory::Arrows,
        "왼쪽 겹 이중 화살표 double arrow 겹화살표 leftwards",
    ),
    item(
        "⇑",
        "⇑",
        "special_char.u21d1.name",
        "special_char.u21d1.desc",
        CharCategory::Arrows,
        "위 겹 이중 화살표 double arrow 위쪽 겹화살표 upwards",
    ),
    item(
        "⇓",
        "⇓",
        "special_char.u21d3.name",
        "special_char.u21d3.desc",
        CharCategory::Arrows,
        "아래 겹 이중 화살표 double arrow 아래쪽 겹화살표 downwards",
    ),
    item(
        "⇔",
        "⇔",
        "special_char.u21d4.name",
        "special_char.u21d4.desc",
        CharCategory::Arrows,
        "양쪽 겹 이중 화살표 동치 iff double arrow 겹화살표 left right",
    ),
    item(
        "⇄",
        "⇄",
        "special_char.u21c4.name",
        "special_char.u21c4.desc",
        CharCategory::Arrows,
        "교환 화살표 exchange arrows 오른쪽 왼쪽 right over left",
    ),
    item(
        "⇆",
        "⇆",
        "special_char.u21c6.name",
        "special_char.u21c6.desc",
        CharCategory::Arrows,
        "교환 화살표 exchange arrows 왼쪽 오른쪽 left over right",
    ),
    item(
        "↩",
        "↩",
        "special_char.u21a9.name",
        "special_char.u21a9.desc",
        CharCategory::Arrows,
        "되돌아가기 갈고리 화살표 return hook arrow 왼쪽 leftwards with",
    ),
    item(
        "↪",
        "↪",
        "special_char.u21aa.name",
        "special_char.u21aa.desc",
        CharCategory::Arrows,
        "갈고리 화살표 hook arrow 오른쪽 rightwards with",
    ),
    item(
        "↵",
        "↵",
        "special_char.u21b5.name",
        "special_char.u21b5.desc",
        CharCategory::Arrows,
        "줄바꿈 엔터 화살표 carriage return arrow downwards with corner leftwards",
    ),
    item(
        "↺",
        "↺",
        "special_char.u21ba.name",
        "special_char.u21ba.desc",
        CharCategory::Arrows,
        "되돌리기 반시계 화살표 undo anticlockwise arrow 방향 open circle",
    ),
    item(
        "↻",
        "↻",
        "special_char.u21bb.name",
        "special_char.u21bb.desc",
        CharCategory::Arrows,
        "새로고침 시계 화살표 redo refresh clockwise arrow 방향 open circle",
    ),
    item(
        "⟵",
        "⟵",
        "special_char.u27f5.name",
        "special_char.u27f5.desc",
        CharCategory::Arrows,
        "긴 왼쪽 화살표 long arrow leftwards",
    ),
    item(
        "⟶",
        "⟶",
        "special_char.u27f6.name",
        "special_char.u27f6.desc",
        CharCategory::Arrows,
        "긴 오른쪽 화살표 long arrow rightwards",
    ),
    item(
        "➔",
        "➔",
        "special_char.u2794.name",
        "special_char.u2794.desc",
        CharCategory::Arrows,
        "굵은 오른쪽 화살표 heavy arrow wide-headed right",
    ),
    item(
        "➜",
        "➜",
        "special_char.u279c.name",
        "special_char.u279c.desc",
        CharCategory::Arrows,
        "굵은 오른쪽 화살표 heavy arrow 둥근 머리 round-tipped right",
    ),
    item(
        "⤴",
        "⤴",
        "special_char.u2934.name",
        "special_char.u2934.desc",
        CharCategory::Arrows,
        "굽은 위 화살표 curving up arrow 위로 pointing rightwards then upwards",
    ),
    item(
        "⤵",
        "⤵",
        "special_char.u2935.name",
        "special_char.u2935.desc",
        CharCategory::Arrows,
        "굽은 아래 화살표 curving down arrow 아래로 pointing rightwards then downwards",
    ),
    // --- 수학 (Math) ---
    item(
        "±",
        "±",
        "special_char.u00b1.name",
        "special_char.u00b1.desc",
        CharCategory::Math,
        "플러스마이너스 plusminus 오차",
    ),
    item(
        "×",
        "×",
        "special_char.u00d7.name",
        "special_char.u00d7.desc",
        CharCategory::Math,
        "곱셈 치수 크기 multiply",
    ),
    item(
        "÷",
        "÷",
        "special_char.u00f7.name",
        "special_char.u00f7.desc",
        CharCategory::Math,
        "나눗셈 divide",
    ),
    item(
        "≠",
        "≠",
        "special_char.u2260.name",
        "special_char.u2260.desc",
        CharCategory::Math,
        "같지않음 다름 not equal",
    ),
    item(
        "≤",
        "≤",
        "special_char.u2264.name",
        "special_char.u2264.desc",
        CharCategory::Math,
        "작거나같음 이하 less equal",
    ),
    item(
        "≥",
        "≥",
        "special_char.u2265.name",
        "special_char.u2265.desc",
        CharCategory::Math,
        "크거나같음 이상 greater equal",
    ),
    item(
        "≈",
        "≈",
        "special_char.u2248.name",
        "special_char.u2248.desc",
        CharCategory::Math,
        "근사 약 approx",
    ),
    item(
        "∞",
        "∞",
        "special_char.u221e.name",
        "special_char.u221e.desc",
        CharCategory::Math,
        "무한대 infinity",
    ),
    item(
        "√",
        "√",
        "special_char.u221a.name",
        "special_char.u221a.desc",
        CharCategory::Math,
        "루트 root",
    ),
    item(
        "−",
        "−",
        "special_char.u2212.name",
        "special_char.u2212.desc",
        CharCategory::Math,
        "빼기 마이너스 minus sign",
    ),
    item(
        "∓",
        "∓",
        "special_char.u2213.name",
        "special_char.u2213.desc",
        CharCategory::Math,
        "빼기 더하기 minus plus minus-plus sign",
    ),
    item(
        "≡",
        "≡",
        "special_char.u2261.name",
        "special_char.u2261.desc",
        CharCategory::Math,
        "합동 항등 같음 identical congruent to",
    ),
    item(
        "≒",
        "≒",
        "special_char.u2252.name",
        "special_char.u2252.desc",
        CharCategory::Math,
        "거의 같음 근사 approximately ≒ equal",
    ),
    item(
        "≪",
        "≪",
        "special_char.u226a.name",
        "special_char.u226a.desc",
        CharCategory::Math,
        "훨씬 작음 much less less-than",
    ),
    item(
        "≫",
        "≫",
        "special_char.u226b.name",
        "special_char.u226b.desc",
        CharCategory::Math,
        "훨씬 큼 much greater greater-than",
    ),
    item(
        "∝",
        "∝",
        "special_char.u221d.name",
        "special_char.u221d.desc",
        CharCategory::Math,
        "비례 proportional to",
    ),
    item(
        "∑",
        "∑",
        "special_char.u2211.name",
        "special_char.u2211.desc",
        CharCategory::Math,
        "합 시그마 sum sigma n-ary summation",
    ),
    item(
        "∏",
        "∏",
        "special_char.u220f.name",
        "special_char.u220f.desc",
        CharCategory::Math,
        "곱 product pi 파이 n-ary",
    ),
    item(
        "∫",
        "∫",
        "special_char.u222b.name",
        "special_char.u222b.desc",
        CharCategory::Math,
        "적분 integral",
    ),
    item(
        "∮",
        "∮",
        "special_char.u222e.name",
        "special_char.u222e.desc",
        CharCategory::Math,
        "선적분 contour integral",
    ),
    item(
        "∂",
        "∂",
        "special_char.u2202.name",
        "special_char.u2202.desc",
        CharCategory::Math,
        "편미분 미분 partial differential",
    ),
    item(
        "∆",
        "∆",
        "special_char.u2206.name",
        "special_char.u2206.desc",
        CharCategory::Math,
        "증분 델타 차이 delta increment",
    ),
    item(
        "∇",
        "∇",
        "special_char.u2207.name",
        "special_char.u2207.desc",
        CharCategory::Math,
        "나블라 기울기 nabla gradient",
    ),
    item(
        "∈",
        "∈",
        "special_char.u2208.name",
        "special_char.u2208.desc",
        CharCategory::Math,
        "원소 속함 집합 element in of",
    ),
    item(
        "∉",
        "∉",
        "special_char.u2209.name",
        "special_char.u2209.desc",
        CharCategory::Math,
        "원소 아님 집합 not in an element of",
    ),
    item(
        "∋",
        "∋",
        "special_char.u220b.name",
        "special_char.u220b.desc",
        CharCategory::Math,
        "원소 포함 집합 contains 원소로 가짐 as member",
    ),
    item(
        "⊂",
        "⊂",
        "special_char.u2282.name",
        "special_char.u2282.desc",
        CharCategory::Math,
        "부분집합 집합 subset of",
    ),
    item(
        "⊃",
        "⊃",
        "special_char.u2283.name",
        "special_char.u2283.desc",
        CharCategory::Math,
        "상위집합 집합 superset of",
    ),
    item(
        "⊆",
        "⊆",
        "special_char.u2286.name",
        "special_char.u2286.desc",
        CharCategory::Math,
        "부분집합 집합 subset equal 또는 같음 of or to",
    ),
    item(
        "⊇",
        "⊇",
        "special_char.u2287.name",
        "special_char.u2287.desc",
        CharCategory::Math,
        "상위집합 집합 superset equal 또는 같음 of or to",
    ),
    item(
        "∪",
        "∪",
        "special_char.u222a.name",
        "special_char.u222a.desc",
        CharCategory::Math,
        "합집합 집합 union",
    ),
    item(
        "∩",
        "∩",
        "special_char.u2229.name",
        "special_char.u2229.desc",
        CharCategory::Math,
        "교집합 집합 intersection",
    ),
    item(
        "∅",
        "∅",
        "special_char.u2205.name",
        "special_char.u2205.desc",
        CharCategory::Math,
        "공집합 집합 empty set",
    ),
    item(
        "∀",
        "∀",
        "special_char.u2200.name",
        "special_char.u2200.desc",
        CharCategory::Math,
        "모든 전칭 for all",
    ),
    item(
        "∃",
        "∃",
        "special_char.u2203.name",
        "special_char.u2203.desc",
        CharCategory::Math,
        "존재 exists 기호 there",
    ),
    item(
        "¬",
        "¬",
        "special_char.u00ac.name",
        "special_char.u00ac.desc",
        CharCategory::Math,
        "부정 아님 논리 not sign",
    ),
    item(
        "∧",
        "∧",
        "special_char.u2227.name",
        "special_char.u2227.desc",
        CharCategory::Math,
        "그리고 논리곱 and logical",
    ),
    item(
        "∨",
        "∨",
        "special_char.u2228.name",
        "special_char.u2228.desc",
        CharCategory::Math,
        "또는 논리합 or logical",
    ),
    item(
        "⊕",
        "⊕",
        "special_char.u2295.name",
        "special_char.u2295.desc",
        CharCategory::Math,
        "원 더하기 배타 xor circled plus",
    ),
    item(
        "⊥",
        "⊥",
        "special_char.u22a5.name",
        "special_char.u22a5.desc",
        CharCategory::Math,
        "수직 perpendicular up tack",
    ),
    item(
        "∠",
        "∠",
        "special_char.u2220.name",
        "special_char.u2220.desc",
        CharCategory::Math,
        "각 angle",
    ),
    item(
        "∴",
        "∴",
        "special_char.u2234.name",
        "special_char.u2234.desc",
        CharCategory::Math,
        "그러므로 따라서 therefore",
    ),
    item(
        "∵",
        "∵",
        "special_char.u2235.name",
        "special_char.u2235.desc",
        CharCategory::Math,
        "왜냐하면 because",
    ),
    item(
        "∛",
        "∛",
        "special_char.u221b.name",
        "special_char.u221b.desc",
        CharCategory::Math,
        "세제곱근 cube root",
    ),
    item(
        "‱",
        "‱",
        "special_char.u2031.name",
        "special_char.u2031.desc",
        CharCategory::Math,
        "만분율 basis point ‱ per ten thousand",
    ),
    item(
        "π",
        "π",
        "special_char.u03c0.name",
        "special_char.u03c0.desc",
        CharCategory::Math,
        "파이 원주율 그리스 pi π greek small",
    ),
    item(
        "α",
        "α",
        "special_char.u03b1.name",
        "special_char.u03b1.desc",
        CharCategory::Math,
        "알파 그리스 alpha α greek small",
    ),
    item(
        "β",
        "β",
        "special_char.u03b2.name",
        "special_char.u03b2.desc",
        CharCategory::Math,
        "베타 그리스 beta β greek small",
    ),
    item(
        "γ",
        "γ",
        "special_char.u03b3.name",
        "special_char.u03b3.desc",
        CharCategory::Math,
        "감마 그리스 gamma γ greek small",
    ),
    item(
        "δ",
        "δ",
        "special_char.u03b4.name",
        "special_char.u03b4.desc",
        CharCategory::Math,
        "델타 그리스 delta δ greek small",
    ),
    item(
        "θ",
        "θ",
        "special_char.u03b8.name",
        "special_char.u03b8.desc",
        CharCategory::Math,
        "세타 그리스 theta θ greek small",
    ),
    item(
        "λ",
        "λ",
        "special_char.u03bb.name",
        "special_char.u03bb.desc",
        CharCategory::Math,
        "람다 그리스 lambda λ greek small",
    ),
    item(
        "μ",
        "μ",
        "special_char.u03bc.name",
        "special_char.u03bc.desc",
        CharCategory::Math,
        "뮤 그리스 mu μ greek small",
    ),
    item(
        "σ",
        "σ",
        "special_char.u03c3.name",
        "special_char.u03c3.desc",
        CharCategory::Math,
        "시그마 그리스 sigma σ greek small",
    ),
    item(
        "φ",
        "φ",
        "special_char.u03c6.name",
        "special_char.u03c6.desc",
        CharCategory::Math,
        "파이 그리스 phi φ greek small",
    ),
    item(
        "ω",
        "ω",
        "special_char.u03c9.name",
        "special_char.u03c9.desc",
        CharCategory::Math,
        "오메가 그리스 omega ω greek small",
    ),
    item(
        "Ω",
        "Ω",
        "special_char.u03a9.name",
        "special_char.u03a9.desc",
        CharCategory::Math,
        "오메가 옴 그리스 omega ohm 대문자 ω greek capital",
    ),
    // --- 번호·원문자 (Numbers & Enclosed) ---
    item(
        "①",
        "①",
        "special_char.u2460.name",
        "special_char.u2460.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 1",
    ),
    item(
        "②",
        "②",
        "special_char.u2461.name",
        "special_char.u2461.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 2",
    ),
    item(
        "③",
        "③",
        "special_char.u2462.name",
        "special_char.u2462.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 3",
    ),
    item(
        "④",
        "④",
        "special_char.u2463.name",
        "special_char.u2463.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 4",
    ),
    item(
        "⑤",
        "⑤",
        "special_char.u2464.name",
        "special_char.u2464.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 5",
    ),
    item(
        "⑥",
        "⑥",
        "special_char.u2465.name",
        "special_char.u2465.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 6",
    ),
    item(
        "⑦",
        "⑦",
        "special_char.u2466.name",
        "special_char.u2466.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 7",
    ),
    item(
        "⑧",
        "⑧",
        "special_char.u2467.name",
        "special_char.u2467.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 8",
    ),
    item(
        "⑨",
        "⑨",
        "special_char.u2468.name",
        "special_char.u2468.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 9",
    ),
    item(
        "⑩",
        "⑩",
        "special_char.u2469.name",
        "special_char.u2469.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 10",
    ),
    item(
        "⑪",
        "⑪",
        "special_char.u246a.name",
        "special_char.u246a.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 11",
    ),
    item(
        "⑫",
        "⑫",
        "special_char.u246b.name",
        "special_char.u246b.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 12",
    ),
    item(
        "⑬",
        "⑬",
        "special_char.u246c.name",
        "special_char.u246c.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 13",
    ),
    item(
        "⑭",
        "⑭",
        "special_char.u246d.name",
        "special_char.u246d.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 14",
    ),
    item(
        "⑮",
        "⑮",
        "special_char.u246e.name",
        "special_char.u246e.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 15",
    ),
    item(
        "⑯",
        "⑯",
        "special_char.u246f.name",
        "special_char.u246f.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 16",
    ),
    item(
        "⑰",
        "⑰",
        "special_char.u2470.name",
        "special_char.u2470.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 17",
    ),
    item(
        "⑱",
        "⑱",
        "special_char.u2471.name",
        "special_char.u2471.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 18",
    ),
    item(
        "⑲",
        "⑲",
        "special_char.u2472.name",
        "special_char.u2472.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 19",
    ),
    item(
        "⑳",
        "⑳",
        "special_char.u2473.name",
        "special_char.u2473.desc",
        CharCategory::Numbers,
        "원 숫자 동그라미 circled number 20",
    ),
    item(
        "❶",
        "❶",
        "special_char.u2776.name",
        "special_char.u2776.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 1 negative",
    ),
    item(
        "❷",
        "❷",
        "special_char.u2777.name",
        "special_char.u2777.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 2 negative",
    ),
    item(
        "❸",
        "❸",
        "special_char.u2778.name",
        "special_char.u2778.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 3 negative",
    ),
    item(
        "❹",
        "❹",
        "special_char.u2779.name",
        "special_char.u2779.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 4 negative",
    ),
    item(
        "❺",
        "❺",
        "special_char.u277a.name",
        "special_char.u277a.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 5 negative",
    ),
    item(
        "❻",
        "❻",
        "special_char.u277b.name",
        "special_char.u277b.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 6 negative",
    ),
    item(
        "❼",
        "❼",
        "special_char.u277c.name",
        "special_char.u277c.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 7 negative",
    ),
    item(
        "❽",
        "❽",
        "special_char.u277d.name",
        "special_char.u277d.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 8 negative",
    ),
    item(
        "❾",
        "❾",
        "special_char.u277e.name",
        "special_char.u277e.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 9 negative",
    ),
    item(
        "❿",
        "❿",
        "special_char.u277f.name",
        "special_char.u277f.desc",
        CharCategory::Numbers,
        "검은 원 숫자 동그라미 circled number 10 negative",
    ),
    item(
        "⑴",
        "⑴",
        "special_char.u2474.name",
        "special_char.u2474.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 1",
    ),
    item(
        "⑵",
        "⑵",
        "special_char.u2475.name",
        "special_char.u2475.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 2",
    ),
    item(
        "⑶",
        "⑶",
        "special_char.u2476.name",
        "special_char.u2476.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 3",
    ),
    item(
        "⑷",
        "⑷",
        "special_char.u2477.name",
        "special_char.u2477.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 4",
    ),
    item(
        "⑸",
        "⑸",
        "special_char.u2478.name",
        "special_char.u2478.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 5",
    ),
    item(
        "⑹",
        "⑹",
        "special_char.u2479.name",
        "special_char.u2479.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 6",
    ),
    item(
        "⑺",
        "⑺",
        "special_char.u247a.name",
        "special_char.u247a.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 7",
    ),
    item(
        "⑻",
        "⑻",
        "special_char.u247b.name",
        "special_char.u247b.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 8",
    ),
    item(
        "⑼",
        "⑼",
        "special_char.u247c.name",
        "special_char.u247c.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 9",
    ),
    item(
        "⑽",
        "⑽",
        "special_char.u247d.name",
        "special_char.u247d.desc",
        CharCategory::Numbers,
        "괄호 숫자 parenthesized number 10",
    ),
    item(
        "Ⅰ",
        "Ⅰ",
        "special_char.u2160.name",
        "special_char.u2160.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 1",
    ),
    item(
        "Ⅱ",
        "Ⅱ",
        "special_char.u2161.name",
        "special_char.u2161.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 2",
    ),
    item(
        "Ⅲ",
        "Ⅲ",
        "special_char.u2162.name",
        "special_char.u2162.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 3",
    ),
    item(
        "Ⅳ",
        "Ⅳ",
        "special_char.u2163.name",
        "special_char.u2163.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 4",
    ),
    item(
        "Ⅴ",
        "Ⅴ",
        "special_char.u2164.name",
        "special_char.u2164.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 5",
    ),
    item(
        "Ⅵ",
        "Ⅵ",
        "special_char.u2165.name",
        "special_char.u2165.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 6",
    ),
    item(
        "Ⅶ",
        "Ⅶ",
        "special_char.u2166.name",
        "special_char.u2166.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 7",
    ),
    item(
        "Ⅷ",
        "Ⅷ",
        "special_char.u2167.name",
        "special_char.u2167.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 8",
    ),
    item(
        "Ⅸ",
        "Ⅸ",
        "special_char.u2168.name",
        "special_char.u2168.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 9",
    ),
    item(
        "Ⅹ",
        "Ⅹ",
        "special_char.u2169.name",
        "special_char.u2169.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 10",
    ),
    item(
        "Ⅺ",
        "Ⅺ",
        "special_char.u216a.name",
        "special_char.u216a.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 11",
    ),
    item(
        "Ⅻ",
        "Ⅻ",
        "special_char.u216b.name",
        "special_char.u216b.desc",
        CharCategory::Numbers,
        "로마 숫자 roman numeral 12",
    ),
    item(
        "ⅰ",
        "ⅰ",
        "special_char.u2170.name",
        "special_char.u2170.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 1",
    ),
    item(
        "ⅱ",
        "ⅱ",
        "special_char.u2171.name",
        "special_char.u2171.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 2",
    ),
    item(
        "ⅲ",
        "ⅲ",
        "special_char.u2172.name",
        "special_char.u2172.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 3",
    ),
    item(
        "ⅳ",
        "ⅳ",
        "special_char.u2173.name",
        "special_char.u2173.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 4",
    ),
    item(
        "ⅴ",
        "ⅴ",
        "special_char.u2174.name",
        "special_char.u2174.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 5",
    ),
    item(
        "ⅵ",
        "ⅵ",
        "special_char.u2175.name",
        "special_char.u2175.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 6",
    ),
    item(
        "ⅶ",
        "ⅶ",
        "special_char.u2176.name",
        "special_char.u2176.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 7",
    ),
    item(
        "ⅷ",
        "ⅷ",
        "special_char.u2177.name",
        "special_char.u2177.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 8",
    ),
    item(
        "ⅸ",
        "ⅸ",
        "special_char.u2178.name",
        "special_char.u2178.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 9",
    ),
    item(
        "ⅹ",
        "ⅹ",
        "special_char.u2179.name",
        "special_char.u2179.desc",
        CharCategory::Numbers,
        "작은 로마 숫자 roman numeral small 10",
    ),
    item(
        "㉠",
        "㉠",
        "special_char.u3260.name",
        "special_char.u3260.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㄱ",
    ),
    item(
        "㉡",
        "㉡",
        "special_char.u3261.name",
        "special_char.u3261.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㄴ",
    ),
    item(
        "㉢",
        "㉢",
        "special_char.u3262.name",
        "special_char.u3262.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㄷ",
    ),
    item(
        "㉣",
        "㉣",
        "special_char.u3263.name",
        "special_char.u3263.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㄹ",
    ),
    item(
        "㉤",
        "㉤",
        "special_char.u3264.name",
        "special_char.u3264.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅁ",
    ),
    item(
        "㉥",
        "㉥",
        "special_char.u3265.name",
        "special_char.u3265.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅂ",
    ),
    item(
        "㉦",
        "㉦",
        "special_char.u3266.name",
        "special_char.u3266.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅅ",
    ),
    item(
        "㉧",
        "㉧",
        "special_char.u3267.name",
        "special_char.u3267.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅇ",
    ),
    item(
        "㉨",
        "㉨",
        "special_char.u3268.name",
        "special_char.u3268.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅈ",
    ),
    item(
        "㉩",
        "㉩",
        "special_char.u3269.name",
        "special_char.u3269.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅊ",
    ),
    item(
        "㉪",
        "㉪",
        "special_char.u326a.name",
        "special_char.u326a.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅋ",
    ),
    item(
        "㉫",
        "㉫",
        "special_char.u326b.name",
        "special_char.u326b.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅌ",
    ),
    item(
        "㉬",
        "㉬",
        "special_char.u326c.name",
        "special_char.u326c.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅍ",
    ),
    item(
        "㉭",
        "㉭",
        "special_char.u326d.name",
        "special_char.u326d.desc",
        CharCategory::Numbers,
        "원 자모 자음 동그라미 circled hangul ㅎ",
    ),
    item(
        "㉮",
        "㉮",
        "special_char.u326e.name",
        "special_char.u326e.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 가",
    ),
    item(
        "㉯",
        "㉯",
        "special_char.u326f.name",
        "special_char.u326f.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 나",
    ),
    item(
        "㉰",
        "㉰",
        "special_char.u3270.name",
        "special_char.u3270.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 다",
    ),
    item(
        "㉱",
        "㉱",
        "special_char.u3271.name",
        "special_char.u3271.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 라",
    ),
    item(
        "㉲",
        "㉲",
        "special_char.u3272.name",
        "special_char.u3272.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 마",
    ),
    item(
        "㉳",
        "㉳",
        "special_char.u3273.name",
        "special_char.u3273.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 바",
    ),
    item(
        "㉴",
        "㉴",
        "special_char.u3274.name",
        "special_char.u3274.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 사",
    ),
    item(
        "㉵",
        "㉵",
        "special_char.u3275.name",
        "special_char.u3275.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 아",
    ),
    item(
        "㉶",
        "㉶",
        "special_char.u3276.name",
        "special_char.u3276.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 자",
    ),
    item(
        "㉷",
        "㉷",
        "special_char.u3277.name",
        "special_char.u3277.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 차",
    ),
    item(
        "㉸",
        "㉸",
        "special_char.u3278.name",
        "special_char.u3278.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 카",
    ),
    item(
        "㉹",
        "㉹",
        "special_char.u3279.name",
        "special_char.u3279.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 타",
    ),
    item(
        "㉺",
        "㉺",
        "special_char.u327a.name",
        "special_char.u327a.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 파",
    ),
    item(
        "㉻",
        "㉻",
        "special_char.u327b.name",
        "special_char.u327b.desc",
        CharCategory::Numbers,
        "원 글자 가나다 동그라미 circled hangul 하",
    ),
    item(
        "㈀",
        "㈀",
        "special_char.u3200.name",
        "special_char.u3200.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㄱ",
    ),
    item(
        "㈁",
        "㈁",
        "special_char.u3201.name",
        "special_char.u3201.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㄴ",
    ),
    item(
        "㈂",
        "㈂",
        "special_char.u3202.name",
        "special_char.u3202.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㄷ",
    ),
    item(
        "㈃",
        "㈃",
        "special_char.u3203.name",
        "special_char.u3203.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㄹ",
    ),
    item(
        "㈄",
        "㈄",
        "special_char.u3204.name",
        "special_char.u3204.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅁ",
    ),
    item(
        "㈅",
        "㈅",
        "special_char.u3205.name",
        "special_char.u3205.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅂ",
    ),
    item(
        "㈆",
        "㈆",
        "special_char.u3206.name",
        "special_char.u3206.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅅ",
    ),
    item(
        "㈇",
        "㈇",
        "special_char.u3207.name",
        "special_char.u3207.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅇ",
    ),
    item(
        "㈈",
        "㈈",
        "special_char.u3208.name",
        "special_char.u3208.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅈ",
    ),
    item(
        "㈉",
        "㈉",
        "special_char.u3209.name",
        "special_char.u3209.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅊ",
    ),
    item(
        "㈊",
        "㈊",
        "special_char.u320a.name",
        "special_char.u320a.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅋ",
    ),
    item(
        "㈋",
        "㈋",
        "special_char.u320b.name",
        "special_char.u320b.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅌ",
    ),
    item(
        "㈌",
        "㈌",
        "special_char.u320c.name",
        "special_char.u320c.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅍ",
    ),
    item(
        "㈍",
        "㈍",
        "special_char.u320d.name",
        "special_char.u320d.desc",
        CharCategory::Numbers,
        "괄호 자모 자음 parenthesized hangul ㅎ",
    ),
    item(
        "㈎",
        "㈎",
        "special_char.u320e.name",
        "special_char.u320e.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 가",
    ),
    item(
        "㈏",
        "㈏",
        "special_char.u320f.name",
        "special_char.u320f.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 나",
    ),
    item(
        "㈐",
        "㈐",
        "special_char.u3210.name",
        "special_char.u3210.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 다",
    ),
    item(
        "㈑",
        "㈑",
        "special_char.u3211.name",
        "special_char.u3211.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 라",
    ),
    item(
        "㈒",
        "㈒",
        "special_char.u3212.name",
        "special_char.u3212.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 마",
    ),
    item(
        "㈓",
        "㈓",
        "special_char.u3213.name",
        "special_char.u3213.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 바",
    ),
    item(
        "㈔",
        "㈔",
        "special_char.u3214.name",
        "special_char.u3214.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 사",
    ),
    item(
        "㈕",
        "㈕",
        "special_char.u3215.name",
        "special_char.u3215.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 아",
    ),
    item(
        "㈖",
        "㈖",
        "special_char.u3216.name",
        "special_char.u3216.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 자",
    ),
    item(
        "㈗",
        "㈗",
        "special_char.u3217.name",
        "special_char.u3217.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 차",
    ),
    item(
        "㈘",
        "㈘",
        "special_char.u3218.name",
        "special_char.u3218.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 카",
    ),
    item(
        "㈙",
        "㈙",
        "special_char.u3219.name",
        "special_char.u3219.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 타",
    ),
    item(
        "㈚",
        "㈚",
        "special_char.u321a.name",
        "special_char.u321a.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 파",
    ),
    item(
        "㈛",
        "㈛",
        "special_char.u321b.name",
        "special_char.u321b.desc",
        CharCategory::Numbers,
        "괄호 글자 가나다 parenthesized hangul 하",
    ),
    item(
        "ⓐ",
        "ⓐ",
        "special_char.u24d0.name",
        "special_char.u24d0.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter a",
    ),
    item(
        "ⓑ",
        "ⓑ",
        "special_char.u24d1.name",
        "special_char.u24d1.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter b",
    ),
    item(
        "ⓒ",
        "ⓒ",
        "special_char.u24d2.name",
        "special_char.u24d2.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter c",
    ),
    item(
        "ⓓ",
        "ⓓ",
        "special_char.u24d3.name",
        "special_char.u24d3.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter d",
    ),
    item(
        "ⓔ",
        "ⓔ",
        "special_char.u24d4.name",
        "special_char.u24d4.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter e",
    ),
    item(
        "ⓕ",
        "ⓕ",
        "special_char.u24d5.name",
        "special_char.u24d5.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter f",
    ),
    item(
        "ⓖ",
        "ⓖ",
        "special_char.u24d6.name",
        "special_char.u24d6.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter g",
    ),
    item(
        "ⓗ",
        "ⓗ",
        "special_char.u24d7.name",
        "special_char.u24d7.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter h",
    ),
    item(
        "ⓘ",
        "ⓘ",
        "special_char.u24d8.name",
        "special_char.u24d8.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter i",
    ),
    item(
        "ⓙ",
        "ⓙ",
        "special_char.u24d9.name",
        "special_char.u24d9.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter j",
    ),
    item(
        "ⓚ",
        "ⓚ",
        "special_char.u24da.name",
        "special_char.u24da.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter k",
    ),
    item(
        "ⓛ",
        "ⓛ",
        "special_char.u24db.name",
        "special_char.u24db.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter l",
    ),
    item(
        "ⓜ",
        "ⓜ",
        "special_char.u24dc.name",
        "special_char.u24dc.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter m",
    ),
    item(
        "ⓝ",
        "ⓝ",
        "special_char.u24dd.name",
        "special_char.u24dd.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter n",
    ),
    item(
        "ⓞ",
        "ⓞ",
        "special_char.u24de.name",
        "special_char.u24de.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter o",
    ),
    item(
        "ⓟ",
        "ⓟ",
        "special_char.u24df.name",
        "special_char.u24df.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter p",
    ),
    item(
        "ⓠ",
        "ⓠ",
        "special_char.u24e0.name",
        "special_char.u24e0.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter q",
    ),
    item(
        "ⓡ",
        "ⓡ",
        "special_char.u24e1.name",
        "special_char.u24e1.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter r",
    ),
    item(
        "ⓢ",
        "ⓢ",
        "special_char.u24e2.name",
        "special_char.u24e2.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter s",
    ),
    item(
        "ⓣ",
        "ⓣ",
        "special_char.u24e3.name",
        "special_char.u24e3.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter t",
    ),
    item(
        "ⓤ",
        "ⓤ",
        "special_char.u24e4.name",
        "special_char.u24e4.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter u",
    ),
    item(
        "ⓥ",
        "ⓥ",
        "special_char.u24e5.name",
        "special_char.u24e5.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter v",
    ),
    item(
        "ⓦ",
        "ⓦ",
        "special_char.u24e6.name",
        "special_char.u24e6.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter w",
    ),
    item(
        "ⓧ",
        "ⓧ",
        "special_char.u24e7.name",
        "special_char.u24e7.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter x",
    ),
    item(
        "ⓨ",
        "ⓨ",
        "special_char.u24e8.name",
        "special_char.u24e8.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter y",
    ),
    item(
        "ⓩ",
        "ⓩ",
        "special_char.u24e9.name",
        "special_char.u24e9.desc",
        CharCategory::Numbers,
        "원 영문 알파벳 동그라미 circled latin letter z",
    ),
    item(
        "⒜",
        "⒜",
        "special_char.u249c.name",
        "special_char.u249c.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter a",
    ),
    item(
        "⒝",
        "⒝",
        "special_char.u249d.name",
        "special_char.u249d.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter b",
    ),
    item(
        "⒞",
        "⒞",
        "special_char.u249e.name",
        "special_char.u249e.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter c",
    ),
    item(
        "⒟",
        "⒟",
        "special_char.u249f.name",
        "special_char.u249f.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter d",
    ),
    item(
        "⒠",
        "⒠",
        "special_char.u24a0.name",
        "special_char.u24a0.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter e",
    ),
    item(
        "⒡",
        "⒡",
        "special_char.u24a1.name",
        "special_char.u24a1.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter f",
    ),
    item(
        "⒢",
        "⒢",
        "special_char.u24a2.name",
        "special_char.u24a2.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter g",
    ),
    item(
        "⒣",
        "⒣",
        "special_char.u24a3.name",
        "special_char.u24a3.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter h",
    ),
    item(
        "⒤",
        "⒤",
        "special_char.u24a4.name",
        "special_char.u24a4.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter i",
    ),
    item(
        "⒥",
        "⒥",
        "special_char.u24a5.name",
        "special_char.u24a5.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter j",
    ),
    item(
        "⒦",
        "⒦",
        "special_char.u24a6.name",
        "special_char.u24a6.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter k",
    ),
    item(
        "⒧",
        "⒧",
        "special_char.u24a7.name",
        "special_char.u24a7.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter l",
    ),
    item(
        "⒨",
        "⒨",
        "special_char.u24a8.name",
        "special_char.u24a8.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter m",
    ),
    item(
        "⒩",
        "⒩",
        "special_char.u24a9.name",
        "special_char.u24a9.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter n",
    ),
    item(
        "⒪",
        "⒪",
        "special_char.u24aa.name",
        "special_char.u24aa.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter o",
    ),
    item(
        "⒫",
        "⒫",
        "special_char.u24ab.name",
        "special_char.u24ab.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter p",
    ),
    item(
        "⒬",
        "⒬",
        "special_char.u24ac.name",
        "special_char.u24ac.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter q",
    ),
    item(
        "⒭",
        "⒭",
        "special_char.u24ad.name",
        "special_char.u24ad.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter r",
    ),
    item(
        "⒮",
        "⒮",
        "special_char.u24ae.name",
        "special_char.u24ae.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter s",
    ),
    item(
        "⒯",
        "⒯",
        "special_char.u24af.name",
        "special_char.u24af.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter t",
    ),
    item(
        "⒰",
        "⒰",
        "special_char.u24b0.name",
        "special_char.u24b0.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter u",
    ),
    item(
        "⒱",
        "⒱",
        "special_char.u24b1.name",
        "special_char.u24b1.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter v",
    ),
    item(
        "⒲",
        "⒲",
        "special_char.u24b2.name",
        "special_char.u24b2.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter w",
    ),
    item(
        "⒳",
        "⒳",
        "special_char.u24b3.name",
        "special_char.u24b3.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter x",
    ),
    item(
        "⒴",
        "⒴",
        "special_char.u24b4.name",
        "special_char.u24b4.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter y",
    ),
    item(
        "⒵",
        "⒵",
        "special_char.u24b5.name",
        "special_char.u24b5.desc",
        CharCategory::Numbers,
        "괄호 영문 알파벳 parenthesized latin letter z",
    ),
    item(
        "½",
        "½",
        "special_char.u00bd.name",
        "special_char.u00bd.desc",
        CharCategory::Numbers,
        "분수 fraction 2분의 1 one half",
    ),
    item(
        "⅓",
        "⅓",
        "special_char.u2153.name",
        "special_char.u2153.desc",
        CharCategory::Numbers,
        "분수 fraction 3분의 1 one third",
    ),
    item(
        "⅔",
        "⅔",
        "special_char.u2154.name",
        "special_char.u2154.desc",
        CharCategory::Numbers,
        "분수 fraction 3분의 2 two thirds",
    ),
    item(
        "¼",
        "¼",
        "special_char.u00bc.name",
        "special_char.u00bc.desc",
        CharCategory::Numbers,
        "분수 fraction 4분의 1 one quarter",
    ),
    item(
        "¾",
        "¾",
        "special_char.u00be.name",
        "special_char.u00be.desc",
        CharCategory::Numbers,
        "분수 fraction 4분의 3 three quarters",
    ),
    item(
        "⅕",
        "⅕",
        "special_char.u2155.name",
        "special_char.u2155.desc",
        CharCategory::Numbers,
        "분수 fraction 5분의 1 one fifth",
    ),
    item(
        "⅛",
        "⅛",
        "special_char.u215b.name",
        "special_char.u215b.desc",
        CharCategory::Numbers,
        "분수 fraction 8분의 1 one eighth",
    ),
    item(
        "⁰",
        "⁰",
        "special_char.u2070.name",
        "special_char.u2070.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 0",
    ),
    item(
        "¹",
        "¹",
        "special_char.u00b9.name",
        "special_char.u00b9.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 1",
    ),
    item(
        "²",
        "²",
        "special_char.u00b2.name",
        "special_char.u00b2.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 2",
    ),
    item(
        "³",
        "³",
        "special_char.u00b3.name",
        "special_char.u00b3.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 3",
    ),
    item(
        "⁴",
        "⁴",
        "special_char.u2074.name",
        "special_char.u2074.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 4",
    ),
    item(
        "⁵",
        "⁵",
        "special_char.u2075.name",
        "special_char.u2075.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 5",
    ),
    item(
        "⁶",
        "⁶",
        "special_char.u2076.name",
        "special_char.u2076.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 6",
    ),
    item(
        "⁷",
        "⁷",
        "special_char.u2077.name",
        "special_char.u2077.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 7",
    ),
    item(
        "⁸",
        "⁸",
        "special_char.u2078.name",
        "special_char.u2078.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 8",
    ),
    item(
        "⁹",
        "⁹",
        "special_char.u2079.name",
        "special_char.u2079.desc",
        CharCategory::Numbers,
        "위첨자 제곱 superscript 9",
    ),
    item(
        "⁺",
        "⁺",
        "special_char.u207a.name",
        "special_char.u207a.desc",
        CharCategory::Numbers,
        "위첨자 더하기 superscript plus",
    ),
    item(
        "⁻",
        "⁻",
        "special_char.u207b.name",
        "special_char.u207b.desc",
        CharCategory::Numbers,
        "위첨자 빼기 superscript minus",
    ),
    item(
        "ⁿ",
        "ⁿ",
        "special_char.u207f.name",
        "special_char.u207f.desc",
        CharCategory::Numbers,
        "위첨자 n제곱 superscript n",
    ),
    item(
        "₀",
        "₀",
        "special_char.u2080.name",
        "special_char.u2080.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 0",
    ),
    item(
        "₁",
        "₁",
        "special_char.u2081.name",
        "special_char.u2081.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 1",
    ),
    item(
        "₂",
        "₂",
        "special_char.u2082.name",
        "special_char.u2082.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 2",
    ),
    item(
        "₃",
        "₃",
        "special_char.u2083.name",
        "special_char.u2083.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 3",
    ),
    item(
        "₄",
        "₄",
        "special_char.u2084.name",
        "special_char.u2084.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 4",
    ),
    item(
        "₅",
        "₅",
        "special_char.u2085.name",
        "special_char.u2085.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 5",
    ),
    item(
        "₆",
        "₆",
        "special_char.u2086.name",
        "special_char.u2086.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 6",
    ),
    item(
        "₇",
        "₇",
        "special_char.u2087.name",
        "special_char.u2087.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 7",
    ),
    item(
        "₈",
        "₈",
        "special_char.u2088.name",
        "special_char.u2088.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 8",
    ),
    item(
        "₉",
        "₉",
        "special_char.u2089.name",
        "special_char.u2089.desc",
        CharCategory::Numbers,
        "아래첨자 화학 subscript 9",
    ),
    // --- 이모지 (Emoji) ---
    item(
        "📝",
        "📝",
        "special_char.u1f4dd.name",
        "special_char.u1f4dd.desc",
        CharCategory::Emoji,
        "메모 memo note 글쓰기",
    ),
    item(
        "📖",
        "📖",
        "special_char.u1f4d6.name",
        "special_char.u1f4d6.desc",
        CharCategory::Emoji,
        "책 book read 독서",
    ),
    item(
        "📚",
        "📚",
        "special_char.u1f4da.name",
        "special_char.u1f4da.desc",
        CharCategory::Emoji,
        "책 서적 books library",
    ),
    item(
        "📄",
        "📄",
        "special_char.u1f4c4.name",
        "special_char.u1f4c4.desc",
        CharCategory::Emoji,
        "문서 doc page paper",
    ),
    item(
        "🖋️",
        "🖋️",
        "special_char.u1f58b.name",
        "special_char.u1f58b.desc",
        CharCategory::Emoji,
        "펜 만년필 pen fountain",
    ),
    item(
        "✏️",
        "✏️",
        "special_char.u270f.name",
        "special_char.u270f.desc",
        CharCategory::Emoji,
        "연필 pencil write",
    ),
    item(
        "📌",
        "📌",
        "special_char.u1f4cc.name",
        "special_char.u1f4cc.desc",
        CharCategory::Emoji,
        "압정 pin 고정",
    ),
    item(
        "📍",
        "📍",
        "special_char.u1f4cd.name",
        "special_char.u1f4cd.desc",
        CharCategory::Emoji,
        "핀 pin 위치",
    ),
    item(
        "📎",
        "📎",
        "special_char.u1f4ce.name",
        "special_char.u1f4ce.desc",
        CharCategory::Emoji,
        "클립 clip 첨부",
    ),
    item(
        "🔖",
        "🔖",
        "special_char.u1f516.name",
        "special_char.u1f516.desc",
        CharCategory::Emoji,
        "책갈피 bookmark 북마크",
    ),
    item(
        "💡",
        "💡",
        "special_char.u1f4a1.name",
        "special_char.u1f4a1.desc",
        CharCategory::Emoji,
        "전구 아이디어 idea lightbulb",
    ),
    item(
        "💭",
        "💭",
        "special_char.u1f4ad.name",
        "special_char.u1f4ad.desc",
        CharCategory::Emoji,
        "생각 thought bubble",
    ),
    item(
        "💬",
        "💬",
        "special_char.u1f4ac.name",
        "special_char.u1f4ac.desc",
        CharCategory::Emoji,
        "말풍선 comment speech chat",
    ),
    item(
        "✍️",
        "✍️",
        "special_char.u270d.name",
        "special_char.u270d.desc",
        CharCategory::Emoji,
        "글쓰기 write hand 서명",
    ),
    item(
        "🗓️",
        "🗓️",
        "special_char.u1f5d3.name",
        "special_char.u1f5d3.desc",
        CharCategory::Emoji,
        "달력 calendar 날짜",
    ),
    item(
        "⏱️",
        "⏱️",
        "special_char.u23f1.name",
        "special_char.u23f1.desc",
        CharCategory::Emoji,
        "시간 timer stopwatch 시계",
    ),
    item(
        "⏳",
        "⏳",
        "special_char.u23f3.name",
        "special_char.u23f3.desc",
        CharCategory::Emoji,
        "모래시계 hourglass 시간",
    ),
    item(
        "😀",
        "😀",
        "special_char.u1f600.name",
        "special_char.u1f600.desc",
        CharCategory::Emoji,
        "웃음 smile grin",
    ),
    item(
        "😊",
        "😊",
        "special_char.u1f60a.name",
        "special_char.u1f60a.desc",
        CharCategory::Emoji,
        "미소 smile happy",
    ),
    item(
        "🥰",
        "🥰",
        "special_char.u1f970.name",
        "special_char.u1f970.desc",
        CharCategory::Emoji,
        "하트 love heart",
    ),
    item(
        "😍",
        "😍",
        "special_char.u1f60d.name",
        "special_char.u1f60d.desc",
        CharCategory::Emoji,
        "하트 반함 love heart eyes",
    ),
    item(
        "🤔",
        "🤔",
        "special_char.u1f914.name",
        "special_char.u1f914.desc",
        CharCategory::Emoji,
        "생각 고민 thinking think",
    ),
    item(
        "🧐",
        "🧐",
        "special_char.u1f9d0.name",
        "special_char.u1f9d0.desc",
        CharCategory::Emoji,
        "관찰 monocle inspect",
    ),
    item(
        "🤫",
        "🤫",
        "special_char.u1f92b.name",
        "special_char.u1f92b.desc",
        CharCategory::Emoji,
        "쉿 quiet shh",
    ),
    item(
        "😅",
        "😅",
        "special_char.u1f605.name",
        "special_char.u1f605.desc",
        CharCategory::Emoji,
        "땀 sweat sweat smile",
    ),
    item(
        "😢",
        "😢",
        "special_char.u1f622.name",
        "special_char.u1f622.desc",
        CharCategory::Emoji,
        "눈물 슬픔 cry sad",
    ),
    item(
        "😭",
        "😭",
        "special_char.u1f62d.name",
        "special_char.u1f62d.desc",
        CharCategory::Emoji,
        "눈물 슬픔 sob crying",
    ),
    item(
        "🥳",
        "🥳",
        "special_char.u1f973.name",
        "special_char.u1f973.desc",
        CharCategory::Emoji,
        "파티 축하 party celebrate",
    ),
    item(
        "😎",
        "😎",
        "special_char.u1f60e.name",
        "special_char.u1f60e.desc",
        CharCategory::Emoji,
        "선글라스 cool sunglasses",
    ),
    item(
        "🤩",
        "🤩",
        "special_char.u1f929.name",
        "special_char.u1f929.desc",
        CharCategory::Emoji,
        "스타 star eyes 반짝",
    ),
    item(
        "👍",
        "👍",
        "special_char.u1f44d.name",
        "special_char.u1f44d.desc",
        CharCategory::Emoji,
        "좋아요 thumbs up good",
    ),
    item(
        "👎",
        "👎",
        "special_char.u1f44e.name",
        "special_char.u1f44e.desc",
        CharCategory::Emoji,
        "싫어요 thumbs down bad",
    ),
    item(
        "👏",
        "👏",
        "special_char.u1f44f.name",
        "special_char.u1f44f.desc",
        CharCategory::Emoji,
        "박수 clap applause",
    ),
    item(
        "🙌",
        "🙌",
        "special_char.u1f64c.name",
        "special_char.u1f64c.desc",
        CharCategory::Emoji,
        "만세 celebrate hooray",
    ),
    item(
        "🤝",
        "🤝",
        "special_char.u1f91d.name",
        "special_char.u1f91d.desc",
        CharCategory::Emoji,
        "악수 handshake deal",
    ),
    item(
        "✌️",
        "✌️",
        "special_char.u270c.name",
        "special_char.u270c.desc",
        CharCategory::Emoji,
        "브이 victory peace",
    ),
    item(
        "🤞",
        "🤞",
        "special_char.u1f91e.name",
        "special_char.u1f91e.desc",
        CharCategory::Emoji,
        "행운 luck fingers crossed",
    ),
    item(
        "✅",
        "✅",
        "special_char.u2705.name",
        "special_char.u2705.desc",
        CharCategory::Emoji,
        "체크 완료 check done",
    ),
    item(
        "❌",
        "❌",
        "special_char.u274c.name",
        "special_char.u274c.desc",
        CharCategory::Emoji,
        "엑스 취소 cross x no",
    ),
    item(
        "⚠️",
        "⚠️",
        "special_char.u26a0.name",
        "special_char.u26a0.desc",
        CharCategory::Emoji,
        "경고 warning caution",
    ),
    item(
        "ℹ️",
        "ℹ️",
        "special_char.u2139.name",
        "special_char.u2139.desc",
        CharCategory::Emoji,
        "정보 info information",
    ),
    item(
        "❓",
        "❓",
        "special_char.u2753.name",
        "special_char.u2753.desc",
        CharCategory::Emoji,
        "물음표 question mark",
    ),
    item(
        "❗",
        "❗",
        "special_char.u2757.name",
        "special_char.u2757.desc",
        CharCategory::Emoji,
        "느낌표 exclamation mark",
    ),
    item(
        "🔴",
        "🔴",
        "special_char.u1f534.name",
        "special_char.u1f534.desc",
        CharCategory::Emoji,
        "빨강 red circle",
    ),
    item(
        "🟢",
        "🟢",
        "special_char.u1f7e2.name",
        "special_char.u1f7e2.desc",
        CharCategory::Emoji,
        "초록 green circle",
    ),
    item(
        "🔵",
        "🔵",
        "special_char.u1f535.name",
        "special_char.u1f535.desc",
        CharCategory::Emoji,
        "파랑 blue circle",
    ),
    item(
        "⚪",
        "⚪",
        "special_char.u26aa.name",
        "special_char.u26aa.desc",
        CharCategory::Emoji,
        "흰색 white circle",
    ),
    item(
        "⚫",
        "⚫",
        "special_char.u26ab.name",
        "special_char.u26ab.desc",
        CharCategory::Emoji,
        "검정 black circle",
    ),
    item(
        "🚀",
        "🚀",
        "special_char.u1f680.name",
        "special_char.u1f680.desc",
        CharCategory::Emoji,
        "로켓 rocket launch",
    ),
    item(
        "✨",
        "✨",
        "special_char.u2728.name",
        "special_char.u2728.desc",
        CharCategory::Emoji,
        "반짝 sparkles star",
    ),
    item(
        "🎉",
        "🎉",
        "special_char.u1f389.name",
        "special_char.u1f389.desc",
        CharCategory::Emoji,
        "파티 폭죽 party tada",
    ),
    item(
        "☕",
        "☕",
        "special_char.u2615.name",
        "special_char.u2615.desc",
        CharCategory::Emoji,
        "커피 coffee break",
    ),
    item(
        "🍵",
        "🍵",
        "special_char.u1f375.name",
        "special_char.u1f375.desc",
        CharCategory::Emoji,
        "차 tea green tea",
    ),
    item(
        "🌿",
        "🌿",
        "special_char.u1f33f.name",
        "special_char.u1f33f.desc",
        CharCategory::Emoji,
        "새싹 허브 herb plant",
    ),
    item(
        "🔥",
        "🔥",
        "special_char.u1f525.name",
        "special_char.u1f525.desc",
        CharCategory::Emoji,
        "불 fire flame hot",
    ),
    item(
        "⭐️",
        "⭐️",
        "special_char.u2b50.name",
        "special_char.u2b50.desc",
        CharCategory::Emoji,
        "별 star rate",
    ),
    item(
        "💯",
        "💯",
        "special_char.u1f4af.name",
        "special_char.u1f4af.desc",
        CharCategory::Emoji,
        "백점 100 hundred perfect",
    ),
    item(
        "❤\u{FE0F}",
        "❤\u{FE0F}",
        "special_char.u2764.name",
        "special_char.u2764.desc",
        CharCategory::Emoji,
        "하트 사랑 heart love red 빨간",
    ),
    item(
        "💔",
        "💔",
        "special_char.u1f494.name",
        "special_char.u1f494.desc",
        CharCategory::Emoji,
        "하트 이별 broken heart 깨진",
    ),
    item(
        "🙏",
        "🙏",
        "special_char.u1f64f.name",
        "special_char.u1f64f.desc",
        CharCategory::Emoji,
        "부탁 감사 기도 please thanks pray 두 손 모음 folded hands",
    ),
    item(
        "💪",
        "💪",
        "special_char.u1f4aa.name",
        "special_char.u1f4aa.desc",
        CharCategory::Emoji,
        "힘 근육 화이팅 strong muscle flexed biceps",
    ),
    item(
        "👀",
        "👀",
        "special_char.u1f440.name",
        "special_char.u1f440.desc",
        CharCategory::Emoji,
        "눈 보기 eyes look",
    ),
    item(
        "🙂",
        "🙂",
        "special_char.u1f642.name",
        "special_char.u1f642.desc",
        CharCategory::Emoji,
        "웃음 미소 smile 살짝 웃는 얼굴 slightly smiling face",
    ),
    item(
        "😉",
        "😉",
        "special_char.u1f609.name",
        "special_char.u1f609.desc",
        CharCategory::Emoji,
        "윙크 wink winking face",
    ),
    item(
        "😂",
        "😂",
        "special_char.u1f602.name",
        "special_char.u1f602.desc",
        CharCategory::Emoji,
        "웃음 눈물 joy laugh lol 기쁨의 face with tears of",
    ),
    item(
        "🤣",
        "🤣",
        "special_char.u1f923.name",
        "special_char.u1f923.desc",
        CharCategory::Emoji,
        "웃음 rofl laugh 데굴데굴 rolling on the floor laughing",
    ),
    item(
        "😴",
        "😴",
        "special_char.u1f634.name",
        "special_char.u1f634.desc",
        CharCategory::Emoji,
        "잠 졸림 sleep 잠자는 얼굴 sleeping face",
    ),
    item(
        "😡",
        "😡",
        "special_char.u1f621.name",
        "special_char.u1f621.desc",
        CharCategory::Emoji,
        "화남 angry 화난 얼굴 pouting face",
    ),
    item(
        "🤯",
        "🤯",
        "special_char.u1f92f.name",
        "special_char.u1f92f.desc",
        CharCategory::Emoji,
        "충격 놀람 mind blown 머리 폭발 exploding head",
    ),
    item(
        "🙄",
        "🙄",
        "special_char.u1f644.name",
        "special_char.u1f644.desc",
        CharCategory::Emoji,
        "어이없음 eye roll 눈 굴리기 face with rolling eyes",
    ),
    item(
        "😱",
        "😱",
        "special_char.u1f631.name",
        "special_char.u1f631.desc",
        CharCategory::Emoji,
        "비명 공포 scream face screaming in fear",
    ),
    item(
        "💤",
        "💤",
        "special_char.u1f4a4.name",
        "special_char.u1f4a4.desc",
        CharCategory::Emoji,
        "잠 zzz sleep",
    ),
    item(
        "🌸",
        "🌸",
        "special_char.u1f338.name",
        "special_char.u1f338.desc",
        CharCategory::Emoji,
        "꽃 벚꽃 봄 flower blossom cherry",
    ),
    item(
        "🌙",
        "🌙",
        "special_char.u1f319.name",
        "special_char.u1f319.desc",
        CharCategory::Emoji,
        "달 밤 moon night 초승달 crescent",
    ),
    item(
        "☀\u{FE0F}",
        "☀\u{FE0F}",
        "special_char.u2600_fe0f.name",
        "special_char.u2600_fe0f.desc",
        CharCategory::Emoji,
        "해 맑음 sun",
    ),
    item(
        "🌈",
        "🌈",
        "special_char.u1f308.name",
        "special_char.u1f308.desc",
        CharCategory::Emoji,
        "무지개 rainbow",
    ),
    item(
        "🍀",
        "🍀",
        "special_char.u1f340.name",
        "special_char.u1f340.desc",
        CharCategory::Emoji,
        "행운 클로버 luck clover 네잎클로버 four leaf",
    ),
    item(
        "🎵",
        "🎵",
        "special_char.u1f3b5.name",
        "special_char.u1f3b5.desc",
        CharCategory::Emoji,
        "음악 노래 music note 음표 musical",
    ),
    item(
        "📷",
        "📷",
        "special_char.u1f4f7.name",
        "special_char.u1f4f7.desc",
        CharCategory::Emoji,
        "카메라 사진 camera photo",
    ),
    item(
        "💻",
        "💻",
        "special_char.u1f4bb.name",
        "special_char.u1f4bb.desc",
        CharCategory::Emoji,
        "노트북 컴퓨터 laptop computer",
    ),
    item(
        "📱",
        "📱",
        "special_char.u1f4f1.name",
        "special_char.u1f4f1.desc",
        CharCategory::Emoji,
        "휴대폰 전화 phone mobile",
    ),
    item(
        "📅",
        "📅",
        "special_char.u1f4c5.name",
        "special_char.u1f4c5.desc",
        CharCategory::Emoji,
        "달력 날짜 calendar date",
    ),
    item(
        "🔗",
        "🔗",
        "special_char.u1f517.name",
        "special_char.u1f517.desc",
        CharCategory::Emoji,
        "링크 연결 link",
    ),
    item(
        "🔍",
        "🔍",
        "special_char.u1f50d.name",
        "special_char.u1f50d.desc",
        CharCategory::Emoji,
        "검색 돋보기 search magnifying glass",
    ),
    item(
        "🔒",
        "🔒",
        "special_char.u1f512.name",
        "special_char.u1f512.desc",
        CharCategory::Emoji,
        "잠금 자물쇠 lock locked",
    ),
    item(
        "🗑\u{FE0F}",
        "🗑\u{FE0F}",
        "special_char.u1f5d1.name",
        "special_char.u1f5d1.desc",
        CharCategory::Emoji,
        "휴지통 삭제 trash delete wastebasket",
    ),
    item(
        "📢",
        "📢",
        "special_char.u1f4e2.name",
        "special_char.u1f4e2.desc",
        CharCategory::Emoji,
        "공지 확성기 announcement loudspeaker",
    ),
    item(
        "🎁",
        "🎁",
        "special_char.u1f381.name",
        "special_char.u1f381.desc",
        CharCategory::Emoji,
        "선물 gift present wrapped",
    ),
    item(
        "🎯",
        "🎯",
        "special_char.u1f3af.name",
        "special_char.u1f3af.desc",
        CharCategory::Emoji,
        "목표 과녁 target goal direct hit",
    ),
    item(
        "📈",
        "📈",
        "special_char.u1f4c8.name",
        "special_char.u1f4c8.desc",
        CharCategory::Emoji,
        "그래프 상승 chart up increasing",
    ),
    item(
        "📉",
        "📉",
        "special_char.u1f4c9.name",
        "special_char.u1f4c9.desc",
        CharCategory::Emoji,
        "그래프 하락 chart down decreasing",
    ),
    item(
        "🚧",
        "🚧",
        "special_char.u1f6a7.name",
        "special_char.u1f6a7.desc",
        CharCategory::Emoji,
        "공사 작업중 construction wip 중",
    ),
];

/// 특수 문자 입력 폼 뷰.
pub(super) struct SpecialCharForm {
    app: WeakEntity<HanjiApp>,
    target: Entity<EditorState>,
    pub(super) search: Entity<InputState>,
    category: CharCategory,
    /// 고른 칸(전체 목록 위치). 마우스를 올리거나 방향키로 옮긴다. 결과에 없으면 첫 칸이다.
    active: Option<usize>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SpecialCharForm {
    pub(super) fn new(
        app: WeakEntity<HanjiApp>,
        target: Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("special_char.search_placeholder"))
        });
        // 검색어가 바뀌면 고른 칸을 비운다(결과의 첫 칸이 된다).
        let search_sub = cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.reset_active();
                cx.notify();
            }
        });

        Self {
            app,
            target,
            search,
            category: CharCategory::All,
            active: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![search_sub],
        }
    }

    fn filtered_items(&self, cx: &App) -> Vec<(usize, &'static SpecialCharItem)> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        SPECIAL_CHARS
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let matches_cat = match self.category {
                    CharCategory::All => true,
                    cat => item.category == cat,
                };
                if !matches_cat {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                item.display.to_lowercase().contains(&query)
                    || item.insert.contains(&query)
                    // 이름·설명은 화면 언어와 상관없이 두 언어로 찾는다.
                    || ["en", "ko"].into_iter().any(|locale| {
                        t!(item.name, locale = locale).to_lowercase().contains(&query)
                            || t!(item.desc, locale = locale).to_lowercase().contains(&query)
                    })
                    || item.keywords.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// 고른 칸을 비우고 격자를 맨 위로 올린다.
    fn reset_active(&mut self) {
        self.active = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// 고른 칸을 결과 안에서 `delta`칸 옮긴다(끝에서 멈춘다).
    fn move_active(&mut self, delta: isize, cx: &mut Context<Self>) {
        let filtered = self.filtered_items(cx);
        let Some(current) = self
            .active_index(&filtered)
            .and_then(|ix| filtered.iter().position(|(global, _)| *global == ix))
        else {
            return;
        };
        let next = current.saturating_add_signed(delta).min(filtered.len() - 1);
        self.active = Some(filtered[next].0);
        self.scroll
            .scroll_to_item(next / COLUMNS, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// 고른 칸. 고른 칸이 결과에 없으면 첫 칸이다.
    fn active_index(&self, filtered: &[(usize, &'static SpecialCharItem)]) -> Option<usize> {
        self.active
            .filter(|ix| filtered.iter().any(|(global, _)| global == ix))
            .or_else(|| filtered.first().map(|(global, _)| *global))
    }

    fn active_item(&self, cx: &App) -> Option<&'static SpecialCharItem> {
        let filtered = self.filtered_items(cx);
        self.active_index(&filtered)
            .and_then(|ix| SPECIAL_CHARS.get(ix))
    }

    /// 지금 활성인 칸을 넣는다. 넣을 칸이 없으면 `false`(대화상자를 닫지 않는다).
    fn insert_active(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(item) = self.active_item(cx) else {
            return false;
        };
        self.insert_item(item, window, cx);
        true
    }

    fn insert_item(&mut self, item: &SpecialCharItem, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.target.clone();
        let item = *item;
        self.app
            .update(cx, |app, cx| {
                app.insert_special_char_item(&target, &item, window, cx);
            })
            .ok();
    }
}

impl Render for SpecialCharForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let form = cx.entity().downgrade();

        let categories = [
            (CharCategory::All, "special_char.category.all"),
            (
                CharCategory::Typesetting,
                "special_char.category.typesetting",
            ),
            (CharCategory::Quotes, "special_char.category.quotes"),
            (CharCategory::Symbols, "special_char.category.symbols"),
            (CharCategory::Arrows, "special_char.category.arrows"),
            (CharCategory::Math, "special_char.category.math"),
            (CharCategory::Numbers, "special_char.category.numbers"),
            (CharCategory::Emoji, "special_char.category.emoji"),
        ];

        let selected_tab = categories
            .iter()
            .position(|(cat, _)| *cat == self.category)
            .unwrap_or(0);

        let tabs = TabBar::new("special-char-category")
            .segmented()
            .selected_index(selected_tab)
            .on_click(move |index: &usize, _window, cx| {
                form.update(cx, |form, cx| {
                    if let Some((cat, _)) = categories.get(*index) {
                        form.category = *cat;
                        form.reset_active();
                        cx.notify();
                    }
                })
                .ok();
            })
            .children(
                categories
                    .iter()
                    // 범주가 많아 같은 폭으로 나누면 긴 이름이 잘린다. 이름 폭을 기준으로 남는 폭을 나눈다.
                    .map(|(_, label)| Tab::new().flex_auto().label(t!(*label))),
            );

        let filtered = self.filtered_items(cx);
        let active_index = self.active_index(&filtered);
        let active_item: Option<&'static SpecialCharItem> =
            active_index.and_then(|ix| SPECIAL_CHARS.get(ix));
        let mono = cx.theme().mono_font_family.clone();

        let tiles = if filtered.is_empty() {
            div()
                .h(px(GRID_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(pal.muted)
                .child(t!("special_char.empty"))
                .into_any_element()
        } else {
            let rows = filtered.len().div_ceil(COLUMNS);
            let grid = uniform_list(
                "special-char-grid",
                rows,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let filtered = this.filtered_items(cx);
                    let active_index = this.active_index(&filtered);
                    range
                        .map(|row| {
                            let start = row * COLUMNS;
                            let end = (start + COLUMNS).min(filtered.len());
                            let mut cells = Vec::with_capacity(end - start);
                            for &(global_idx, item) in &filtered[start..end] {
                                let active = active_index == Some(global_idx);
                                cells.push(char_cell(global_idx, item, active, pal, cx));
                            }
                            div().flex().children(cells)
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full();
            // 한 화면에 다 들어가지 않는 범주(모두)도 더 있다는 것이 보이게 스크롤바를 둔다.
            div()
                .relative()
                .h(px(GRID_HEIGHT))
                .child(grid)
                .child(Scrollbar::vertical(&self.scroll))
                .into_any_element()
        };

        // 상세 안내 바
        let info_bar = if let Some(item) = active_item {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_2()
                .rounded_md()
                .bg(pal.panel)
                .border_1()
                .border_color(pal.panel_border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w(px(0.))
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .size(px(30.0))
                                .rounded_md()
                                .bg(pal.hover)
                                .text_lg()
                                .child(item.display),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(pal.foreground)
                                        .child(t!(item.name)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(pal.muted)
                                        .truncate()
                                        .child(t!(item.desc)),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .font_family(mono)
                        .text_xs()
                        .text_color(pal.muted)
                        .child(code_point(item)),
                )
        } else {
            div()
                .p_2()
                .rounded_md()
                .bg(pal.panel)
                .text_xs()
                .text_color(pal.muted)
                .child(t!("special_char.hint"))
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            // 방향키는 검색창 커서 대신 고른 칸을 옮긴다.
            .capture_action(cx.listener(|this, _: &MoveLeft, _, cx| {
                this.move_active(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveRight, _, cx| {
                this.move_active(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.move_active(-(COLUMNS as isize), cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.move_active(COLUMNS as isize, cx);
                cx.stop_propagation();
            }))
            .child(tabs)
            .child(
                Input::new(&self.search).focus_ring(false).prefix(
                    Icon::new(icons::search_field())
                        .small()
                        .text_color(pal.muted),
                ),
            )
            .child(tiles)
            .child(info_bar)
    }
}

/// 격자의 한 칸. 고른 칸은 선택 배경(`active`), 마우스를 올린 칸은 hover 배경이다.
/// 누르면 넣고 대화상자를 닫는다.
fn char_cell(
    global_idx: usize,
    item: &'static SpecialCharItem,
    active: bool,
    pal: Palette,
    cx: &mut Context<SpecialCharForm>,
) -> Stateful<Div> {
    let kind = tile_kind(item);
    div()
        .id(("char-tile", global_idx))
        .size(px(CELL))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .rounded_md()
        .cursor_pointer()
        .when(active, |this| this.bg(pal.active))
        .when(!active, |this| this.hover(move |style| style.bg(pal.hover)))
        .text_color(pal.foreground)
        .when(kind == TileKind::Glyph, |this| this.text_lg())
        .when(kind == TileKind::Pair, |this| this.text_xs())
        .when(kind == TileKind::Abbr, |this| {
            this.text_size(px(ABBR_TEXT_SIZE))
                .font_semibold()
                .text_color(pal.muted)
        })
        .on_mouse_move(cx.listener(move |this, _, _, cx| {
            if this.active != Some(global_idx) {
                this.active = Some(global_idx);
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.insert_item(item, window, cx);
            window.close_dialog(cx);
        }))
        .child(item.display)
}

impl HanjiApp {
    /// 특수 문자 및 이모지 삽입 대화상자를 연다. 열린 시트가 없으면 안내만 한다.
    pub(super) fn on_insert_special_char(
        &mut self,
        _: &InsertSpecialChar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else {
            self.notify(t!("common.open_sheet_first"), window, cx);
            return;
        };
        let target = editor.state().clone();
        let app = cx.entity().downgrade();
        let form = cx.new(|cx| SpecialCharForm::new(app, target, window, cx));
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let search = form.read(cx).search.clone();
                search.update(cx, |state, cx| state.focus(window, cx));
            }
            // Enter(대화상자 확인)와 `삽입` 버튼은 같은 일을 한다: 지금 활성인 칸을 넣는다.
            let submit = {
                let form = form.clone();
                Rc::new(move |window: &mut Window, cx: &mut App| {
                    form.update(cx, |form, cx| form.insert_active(window, cx))
                })
            };
            let (enter, ok) = (submit.clone(), submit.clone());
            dialog
                .w(px(DIALOG_WIDTH))
                .title(t!("special_char.title"))
                .close_button(false)
                .on_ok(move |_, window, cx| enter(window, cx))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("insert-special-char-cancel")
                                .label(t!("common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("insert-special-char-ok")
                                .primary()
                                .label(t!("common.insert"))
                                .on_click(move |_, window, cx| {
                                    if ok(window, cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    /// 선택된 특수 문자 항목을 에디터에 삽입한다.
    pub(super) fn insert_special_char_item(
        &mut self,
        target: &Entity<EditorState>,
        item: &SpecialCharItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = &self.editor else { return };
        if editor.state() != target {
            self.notify(t!("special_char.sheet_closed"), window, cx);
            return;
        }

        editor.state().update(cx, |state, cx| {
            if item.is_pair {
                let text = state.value();
                let selected = state.selected_range();
                if selected.is_empty() {
                    let at = selected.start.min(text.len());
                    state.set_selected_range(at..at, cx);
                    state.replace(item.insert, window, cx);
                    let inside = at + item.pair_open_len;
                    state.set_selected_range(inside..inside, cx);
                } else {
                    let inside_text = &text[selected.clone()];
                    let open = &item.insert[..item.pair_open_len];
                    let close = &item.insert[item.pair_open_len..];
                    let wrapped = format!("{open}{inside_text}{close}");
                    let start = selected.start;
                    let end = start + wrapped.len();
                    state.replace(wrapped, window, cx);
                    state.set_selected_range(end..end, cx);
                }
            } else {
                state.replace(item.insert, window, cx);
            }
            state.focus(window, cx);
        });
        self.on_editor_changed(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{CharCategory, SPECIAL_CHARS, TileKind, code_point, tile_kind};

    #[test]
    fn test_special_chars_contain_typesetting_spaces_and_quotes() {
        let has_nbsp = SPECIAL_CHARS.iter().any(|item| item.insert == "\u{00A0}");
        let has_thin = SPECIAL_CHARS.iter().any(|item| item.insert == "\u{2009}");
        let has_em_dash = SPECIAL_CHARS.iter().any(|item| item.insert == "—");
        let has_natpyo = SPECIAL_CHARS
            .iter()
            .any(|item| item.insert == "「」" && item.is_pair);
        let has_gyeop_natpyo = SPECIAL_CHARS
            .iter()
            .any(|item| item.insert == "『』" && item.is_pair);

        assert!(has_nbsp, "NBSP space should exist");
        assert!(has_thin, "Thin space should exist");
        assert!(has_em_dash, "Em dash should exist");
        assert!(has_natpyo, "낫표 pair should exist");
        assert!(has_gyeop_natpyo, "겹낫표 pair should exist");
    }

    #[test]
    fn test_special_chars_categories() {
        let typesetting_count = SPECIAL_CHARS
            .iter()
            .filter(|i| i.category == CharCategory::Typesetting)
            .count();
        let quotes_count = SPECIAL_CHARS
            .iter()
            .filter(|i| i.category == CharCategory::Quotes)
            .count();
        let emoji_count = SPECIAL_CHARS
            .iter()
            .filter(|i| i.category == CharCategory::Emoji)
            .count();

        assert!(typesetting_count >= 15);
        assert!(quotes_count >= 15);
        assert!(emoji_count >= 20);
    }

    #[test]
    fn test_pair_open_len_validity() {
        for item in SPECIAL_CHARS.iter().filter(|i| i.is_pair) {
            assert!(
                item.pair_open_len > 0 && item.pair_open_len < item.insert.len(),
                "Pair item {} has invalid pair_open_len",
                item.display
            );
            assert!(
                item.insert.is_char_boundary(item.pair_open_len),
                "Pair item {} pair_open_len is not a char boundary",
                item.display
            );
        }
    }

    #[test]
    fn tile_kind_follows_visible_glyphs() {
        // 칸 크기는 바이트 수가 아니라 보이는 글자 수로 정한다(이모지는 4바이트지만 글자 하나).
        let kind_of = |display: &str| {
            SPECIAL_CHARS
                .iter()
                .find(|item| item.display == display)
                .map(tile_kind)
        };
        assert_eq!(kind_of("NBSP"), Some(TileKind::Abbr));
        assert_eq!(kind_of("ZWSP"), Some(TileKind::Abbr));
        assert_eq!(kind_of("📝"), Some(TileKind::Glyph));
        // 변이 선택자(U+FE0F)만 붙은 이모지도 글자 하나로 본다.
        assert_eq!(kind_of("🖋️"), Some(TileKind::Glyph));
        assert_eq!(kind_of("$"), Some(TileKind::Glyph));
        assert_eq!(kind_of("「…」"), Some(TileKind::Pair));
    }

    #[test]
    fn every_char_is_unique_and_translated() {
        let mut keys = std::collections::HashSet::new();
        let mut inserts = std::collections::HashSet::new();
        for item in SPECIAL_CHARS {
            assert!(keys.insert(item.name), "duplicate key {}", item.name);
            assert!(
                inserts.insert(item.insert),
                "duplicate char {}",
                item.display
            );
            for locale in ["en", "ko"] {
                for key in [item.name, item.desc] {
                    assert_ne!(t!(key, locale = locale), key, "missing {locale} {key}");
                }
            }
        }
    }

    #[test]
    fn code_point_is_the_first_inserted_char() {
        let nbso = SPECIAL_CHARS
            .iter()
            .find(|item| item.insert == "\u{00A0}")
            .expect("NBSP 항목 없음");
        assert_eq!(code_point(nbso), "U+00A0");
        // 괄호 싸개는 여는 글자를 보여 준다.
        let pair = SPECIAL_CHARS
            .iter()
            .find(|item| item.is_pair)
            .expect("싸개 항목 없음");
        assert!(code_point(pair).starts_with("U+"));
    }
}
