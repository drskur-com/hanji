//! 텍스트 변환: 저장되는 마크다운 ↔ 에디터 본문(ADR-0011).
//!
//! 에디터에서는 줄 하나가 문단이다(Enter = 새 문단, Ulysses·iA Writer와 같은 경험). 파일에는 표준
//! 마크다운으로 저장해 다른 앱에서도 문단이 그대로 보이게 한다.
//!
//! - 에디터의 줄바꿈 하나 = 마크다운의 빈 줄 하나(문단 경계).
//! - 에디터의 빈 줄 k개 = 마크다운의 빈 줄 k+1개. 표준 렌더러는 추가 빈 줄을 버리므로 다른 앱과
//!   내보내기에서는 문단 경계 하나로 보이고, 이 앱으로 다시 열면 빈 줄이 돌아온다.
//! - 줄바꿈 하나로 이어져야 하는 줄(목록 항목끼리, 인용 줄끼리, 표 행끼리, 들여쓴 코드, 줄 끝
//!   강제 줄바꿈, setext 밑줄)은 빈 줄을 넣지 않는다. 코드·수식 펜스와 frontmatter 안은 그대로 둔다.
//! - 줄 묶음(`:::` 줄로 열고 닫는 구간) 안도 그대로 둔다. 책에서는 구간 전체가 한 문단이고 안의
//!   줄바꿈은 강제 줄바꿈이다([`line_groups`]).
//! - 다른 앱에서 만든 파일의 문단 안 줄바꿈(`A\nB`, 한 문단)은 읽을 때 공백으로 이어 한 줄로 만든다.
//!
//! 에디터 본문 → 마크다운 → 에디터 본문은 원래대로 돌아온다(테스트로 지킨다).

use std::ops::Range;

/// 책별 입력 자동 변환 설정. 기본값은 모두 꺼짐이다.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Options {
    pub smart_quotes: bool,
    pub ellipsis: bool,
    pub dash: bool,
    pub apply_on_paste: bool,
    pub replacements: Vec<Replacement>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Replacement {
    pub before: String,
    pub after: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Options {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.smart_quotes
            || self.ellipsis
            || self.dash
            || self
                .replacements
                .iter()
                .any(|rule| rule.enabled && !rule.before.is_empty())
    }
}

/// 코드·수식·마크다운 구문과 링크는 변환하지 않는다.
fn protected_ranges(text: &str) -> Vec<Range<usize>> {
    use crate::markdown::SpanKind;
    crate::markdown::spans(text)
        .into_iter()
        .filter(|span| {
            matches!(
                span.kind,
                SpanKind::CodeBlock | SpanKind::InlineCode | SpanKind::Marker | SpanKind::Link
            )
        })
        .map(|span| span.range)
        .collect()
}

fn quote(mark: char, previous: Option<char>) -> char {
    let opening = previous
        .is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '—' | '“' | '‘'));
    match (mark, opening) {
        ('"', true) => '“',
        ('"', false) => '”',
        (_, true) => '‘',
        (_, false) => '’',
    }
}

/// 아직 닫지 않은 코드·수식·링크 주소를 입력할 때도 원문을 유지한다.
fn in_literal_context(document: &str, offset: usize) -> bool {
    let prefix = &document[..offset];
    let lines = split(prefix);
    if classify(&lines)
        .last()
        .is_some_and(|kind| matches!(kind, Kind::Verbatim | Kind::Indented))
    {
        return true;
    }
    let line = lines.last().copied().unwrap_or_default();
    if line
        .rsplit_once("](")
        .is_some_and(|(_, destination)| !destination.contains(')'))
    {
        return true;
    }
    let mut open: Option<(char, usize)> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
            continue;
        }
        if !matches!(c, '`' | '$') {
            continue;
        }
        let mut count = 1;
        while chars.peek() == Some(&c) {
            chars.next();
            count += 1;
        }
        match open {
            Some((mark, length)) if mark == c && length == count => open = None,
            None => open = Some((c, count)),
            _ => {}
        }
    }
    open.is_some()
}

/// 새로 입력한 텍스트와 바로 앞에서 완성되는 문장부호·단어만 변환한다.
/// UTF-8 바이트 범위와 치환 텍스트를 한 편집으로 돌려준다.
pub fn on_input(
    document: &str,
    range: Range<usize>,
    inserted: &str,
    options: &Options,
) -> (Range<usize>, String) {
    if inserted.is_empty() || !options.is_enabled() || in_literal_context(document, range.start) {
        return (range, inserted.to_string());
    }
    let mut candidate = document.to_string();
    candidate.replace_range(range.clone(), inserted);
    let protected = protected_ranges(&candidate);
    let insertion_end = range.start + inserted.len();
    if protected
        .iter()
        .any(|span| span.start < insertion_end && span.end > range.start)
    {
        return (range, inserted.to_string());
    }
    // 선택을 교체할 때에는 선택 밖의 기존 단어까지 치환하지 않는다.
    let mut start = range.start;
    let mut replacement = inserted.to_string();
    if options.smart_quotes {
        let mut previous = document[..start].chars().next_back();
        replacement = inserted
            .chars()
            .map(|c| {
                let c = if matches!(c, '\'' | '"') {
                    quote(c, previous)
                } else {
                    c
                };
                previous = Some(c);
                c
            })
            .collect();
    }
    if range.is_empty() {
        let prefix = &document[..start];
        for (enabled, before, after) in [(options.ellipsis, "...", "…"), (options.dash, "--", "—")]
        {
            if enabled
                && prefix.ends_with(&before[..before.len() - 1])
                && inserted == &before[before.len() - 1..]
            {
                start -= before.len() - 1;
                replacement = after.to_string();
                break;
            }
        }
        if inserted.chars().next().is_some_and(char::is_whitespace) {
            for rule in &options.replacements {
                if !rule.enabled || rule.before.is_empty() || !prefix.ends_with(&rule.before) {
                    continue;
                }
                let rule_start = start - rule.before.len();
                if document[..rule_start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    || protected
                        .iter()
                        .any(|span| span.start < start && span.end > rule_start)
                {
                    continue;
                }
                start = rule_start;
                replacement = format!("{}{replacement}", rule.after);
                break;
            }
        }
    }
    (start..range.end, replacement)
}

/// 붙여넣기 변환: 문단 표현을 먼저 맞추고 선택한 자동 변환을 적용한다.
pub fn paste(text: &str, same_format: bool, options: &Options) -> String {
    let text = from_clipboard(text, same_format);
    if !options.apply_on_paste || !options.is_enabled() {
        return text;
    }
    let protected = protected_ranges(&text);
    let mut out = String::with_capacity(text.len());
    let mut offset = 0;
    while offset < text.len() {
        if let Some(span) = protected
            .iter()
            .filter(|span| span.contains(&offset))
            .max_by_key(|span| span.end)
        {
            out.push_str(&text[offset..span.end]);
            offset = span.end;
            continue;
        }
        let limit = protected
            .iter()
            .filter(|span| span.start > offset)
            .map(|span| span.start)
            .min()
            .unwrap_or(text.len());
        let rest = &text[offset..limit];
        if let Some(rule) = options
            .replacements
            .iter()
            .find(|rule| rule.enabled && !rule.before.is_empty() && rest.starts_with(&rule.before))
        {
            out.push_str(&rule.after);
            offset += rule.before.len();
        } else if options.ellipsis && rest.starts_with("...") {
            out.push('…');
            offset += 3;
        } else if options.dash && rest.starts_with("--") {
            out.push('—');
            offset += 2;
        } else {
            let c = rest.chars().next().unwrap();
            out.push(if options.smart_quotes && matches!(c, '\'' | '"') {
                quote(c, out.chars().next_back())
            } else {
                c
            });
            offset += c.len_utf8();
        }
    }
    out
}

/// 클립보드에서 Hanji 에디터 본문으로 변환한다.
///
/// Hanji에서 복사한 본문은 이미 에디터 표현이므로 문단 구조를 그대로 둔다.
/// 외부 텍스트는 줄바꿈을 정규화한 뒤 저장용 마크다운과 같은 규칙으로 읽는다.
pub fn from_clipboard(text: &str, same_format: bool) -> String {
    if same_format {
        return text.to_string();
    }
    to_editor(&text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// 줄 하나의 종류. 줄 사이에 무엇을 넣을지 정할 때 쓴다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Blank,
    /// 펜스·frontmatter 안의 줄(여는·닫는 줄 포함). 원문 그대로 둔다.
    Verbatim,
    /// 줄 묶음 안의 줄(여는·닫는 `:::` 포함). 원문 그대로 두지만 문장 자동 변환은 한다.
    Grouped,
    /// 목록 항목(`-`, `*`, `+`, `1.`, `1)`).
    ListItem,
    Quote,
    TableRow,
    /// 네 칸 이상 들여쓴 줄(들여쓴 코드나 목록 안 문단).
    Indented,
    /// setext 제목 밑줄이나 구분선(`===`, `---`).
    Rule,
    /// ATX 제목.
    Heading,
    /// 문단 줄.
    Text,
}

/// 펜스·frontmatter를 따라가며 줄마다 종류를 정한다.
fn classify(lines: &[&str]) -> Vec<Kind> {
    let mut kinds = Vec::with_capacity(lines.len());
    // 열린 펜스의 닫는 표시. frontmatter는 `---`로 닫는다.
    let mut fence: Option<(char, usize)> = None;
    // 열린 줄 묶음의 콜론 수.
    let mut group: Option<usize> = None;
    for (ix, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(len) = group {
            if group_fence(line).is_some_and(|(colons, bare)| bare && colons >= len) {
                group = None;
            }
            kinds.push(Kind::Grouped);
            continue;
        }
        if let Some((mark, len)) = fence {
            let closes = indent < 4
                && trimmed.starts_with(&mark.to_string().repeat(len))
                && trimmed.trim_end().chars().all(|c| c == mark);
            if closes {
                fence = None;
            }
            kinds.push(Kind::Verbatim);
            continue;
        }
        if let Some((colons, _)) = group_fence(line) {
            group = Some(colons);
            kinds.push(Kind::Grouped);
            continue;
        }
        if ix == 0 && line.trim_end() == "---" {
            fence = Some(('-', 3));
            kinds.push(Kind::Verbatim);
            continue;
        }
        if indent < 4
            && let Some(mark) = ['`', '~', '$']
                .into_iter()
                .find(|&c| trimmed.starts_with(c))
        {
            let len = trimmed.chars().take_while(|&c| c == mark).count();
            // 수식 펜스는 `$$`만 있는 줄로 연다(`$$x$$` 같은 한 줄 수식과 구분).
            let opens = if mark == '$' {
                len >= 2 && trimmed.trim_end().len() == len
            } else {
                len >= 3
            };
            if opens {
                fence = Some((mark, len));
                kinds.push(Kind::Verbatim);
                continue;
            }
        }
        kinds.push(kind_of(line));
    }
    kinds
}

/// 줄 묶음 펜스(줄 첫머리의 콜론 세 개 이상)인지. (콜론 수, 콜론뿐인지)를 돌려준다.
/// 여는 줄에는 뒤에 이름 등을 적어도 되고(`::: 시`), 닫는 줄은 콜론뿐이다.
fn group_fence(line: &str) -> Option<(usize, bool)> {
    let colons = line.bytes().take_while(|&b| b == b':').count();
    (colons >= 3).then(|| (colons, line[colons..].trim().is_empty()))
}

/// 줄 묶음 하나. 줄 번호는 0부터 센다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineGroup {
    /// 여는·닫는 `:::` 줄을 포함한 줄 범위. 닫지 않은 묶음은 문서 끝까지다.
    pub lines: Range<usize>,
    /// 닫는 줄이 있는지(있으면 `lines`의 마지막 줄이다).
    pub closed: bool,
}

impl LineGroup {
    /// 펜스 줄을 뺀 안쪽 줄 범위.
    pub fn body(&self) -> Range<usize> {
        self.lines.start + 1..self.lines.end - usize::from(self.closed)
    }

    /// 여는 줄이나 닫는 줄인지.
    pub fn is_fence(&self, row: usize) -> bool {
        self.lines.contains(&row) && !self.body().contains(&row)
    }
}

/// 줄 묶음들. 묶음 안에서는 줄바꿈이 문단을 나누지 않는다(ADR-0016).
/// 코드 펜스·frontmatter 안의 `:::`는 묶음이 아니다.
pub fn line_groups(text: &str) -> Vec<LineGroup> {
    let lines = split(text);
    let kinds = classify(&lines);
    let mut groups = Vec::new();
    // 열린 묶음의 (첫 줄, 콜론 수). 묶음은 닫는 줄이나 문서 끝에서만 끝난다(`classify`).
    let mut open: Option<(usize, usize)> = None;
    for (ix, (line, kind)) in lines.iter().zip(&kinds).enumerate() {
        if *kind != Kind::Grouped {
            continue;
        }
        let fence = group_fence(line);
        match open {
            None => open = fence.map(|(colons, _)| (ix, colons)),
            Some((start, len)) => {
                if fence.is_some_and(|(colons, bare)| bare && colons >= len) {
                    groups.push(LineGroup {
                        lines: start..ix + 1,
                        closed: true,
                    });
                    open = None;
                }
            }
        }
    }
    if let Some((start, _)) = open {
        groups.push(LineGroup {
            lines: start..lines.len(),
            closed: false,
        });
    }
    groups
}

/// 표 행(`|`로 시작하는 줄)이 이어진 줄 범위. 펜스·줄 묶음 안은 빼고, 표인지는 따지지 않는다.
pub fn table_runs(text: &str) -> Vec<Range<usize>> {
    let lines = split(text);
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (ix, kind) in classify(&lines).into_iter().enumerate() {
        if kind != Kind::TableRow {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.end == ix => run.end = ix + 1,
            _ => runs.push(ix..ix + 1),
        }
    }
    runs
}

/// 선택한 줄을 줄 묶음으로 감싸거나, 선택이 걸친 묶음을 푼다(Edit 메뉴의 토글).
///
/// 선택은 줄 단위로 넓힌다(빈 선택이면 커서 줄). 선택이 묶음(펜스 줄 포함)에 닿아 있으면 그 묶음들의
/// `:::` 줄을 지우고, 아니면 앞뒤에 `:::` 줄을 넣는다. (바꿀 범위, 새 글, 바꾼 뒤 선택할 범위)를 돌려준다.
pub fn toggle_line_group(
    text: &str,
    selection: Range<usize>,
) -> (Range<usize>, String, Range<usize>) {
    let lines = split(text);
    let mut starts = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for line in &lines {
        starts.push(offset);
        offset += line.len() + 1;
    }
    let line_at = |offset: usize| starts.partition_point(|&start| start <= offset) - 1;
    let first = line_at(selection.start);
    let mut last = line_at(selection.end);
    // 다음 줄 첫머리에서 끝난 선택은 그 줄을 넣지 않는다.
    if last > first && starts[last] == selection.end {
        last -= 1;
    }
    let line_end = |ix: usize| starts[ix] + lines[ix].len();
    let touched: Vec<LineGroup> = line_groups(text)
        .into_iter()
        .filter(|group| group.lines.start <= last && first < group.lines.end)
        .collect();
    if touched.is_empty() {
        let range = starts[first]..line_end(last);
        let body = &text[range.clone()];
        let replacement = format!(":::\n{body}\n:::");
        let selected = range.start + 4..range.start + 4 + body.len();
        return (range, replacement, selected);
    }
    // 묶음의 펜스 줄(여는 줄, 닫혔으면 닫는 줄)을 뺀 나머지 줄을 잇는다.
    let from = touched[0].lines.start.min(first);
    let to = touched[touched.len() - 1].lines.end.max(last + 1);
    let is_fence = |ix: usize| touched.iter().any(|group| group.is_fence(ix));
    let kept: Vec<&str> = (from..to)
        .filter(|&ix| !is_fence(ix))
        .map(|ix| lines[ix])
        .collect();
    let range = starts[from]..line_end(to - 1);
    let mut replacement = kept.join("\n");
    let mut range = range;
    if kept.is_empty() {
        // 빈 묶음은 줄째 지운다.
        if range.end < text.len() {
            range.end += 1;
        } else if range.start > 0 {
            range.start -= 1;
        }
        replacement.clear();
    }
    let selected = range.start..range.start + replacement.len();
    (range, replacement, selected)
}

/// `at`에 각주 참조 `[^N]`을 넣고 시트 끝에 정의 `[^N]: 내용`을 더한다(Insert 메뉴의 각주).
///
/// N은 시트에 있는 숫자 각주 이름 중 가장 큰 것 + 1이다. 정의는 끝의 정의 줄들 뒤에 잇고, 처음이면
/// 본문과 빈 줄 하나로 띄운다. 한 번에 되돌릴 수 있게 `at`부터 끝까지 한 편집으로 바꾼다.
/// (바꿀 범위, 새 글, 바꾼 뒤 커서)를 돌려준다. 커서는 참조 바로 뒤다.
pub fn insert_footnote(text: &str, at: usize, content: &str) -> (Range<usize>, String, usize) {
    let number = footnote_numbers(text).max().unwrap_or(0) + 1;
    let reference = format!("[^{number}]");
    let content = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut edited = text.to_string();
    edited.insert_str(at, &reference);
    // 끝의 빈 줄은 버린다. 끝 줄이 정의면 바로 다음 줄에, 아니면 빈 줄을 하나 두고 쓴다.
    let body = edited.trim_end_matches('\n');
    let lead = if is_footnote_definition(body.rsplit('\n').next().unwrap_or("")) {
        "\n"
    } else {
        "\n\n"
    };
    let replacement = format!("{}{lead}{reference}: {content}", &body[at..]);
    (at..text.len(), replacement, at + reference.len())
}

/// 글에 나오는 숫자 각주 이름(`[^3]`의 3).
fn footnote_numbers(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.match_indices("[^").filter_map(|(ix, _)| {
        let rest = &text[ix + 2..];
        let end = rest.find(']')?;
        rest[..end].parse().ok()
    })
}

/// `[^이름]:`로 시작하는 각주 정의 줄인지.
fn is_footnote_definition(line: &str) -> bool {
    line.strip_prefix("[^")
        .and_then(|rest| {
            rest.find("]:")
                .map(|end| end > 0 && !rest[..end].contains(']'))
        })
        .unwrap_or(false)
}

/// 펜스 밖 줄 하나의 종류.
fn kind_of(line: &str) -> Kind {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Kind::Blank;
    }
    let indent = line.len() - line.trim_start().len();
    if indent >= 4 {
        return Kind::Indented;
    }
    if crate::markdown::heading_level(line).is_some() {
        return Kind::Heading;
    }
    if trimmed.len() >= 3
        && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'))
    {
        return Kind::Rule;
    }
    if trimmed.starts_with('>') {
        return Kind::Quote;
    }
    if trimmed.starts_with('|') {
        return Kind::TableRow;
    }
    let marker = trimmed.split_once([' ', '\t']).map(|(marker, _)| marker);
    let list = marker.is_some_and(|marker| {
        matches!(marker, "-" | "*" | "+")
            || marker.strip_suffix(['.', ')']).is_some_and(|number| {
                !number.is_empty()
                    && number.len() <= 9
                    && number.bytes().all(|b| b.is_ascii_digit())
            })
    });
    if list { Kind::ListItem } else { Kind::Text }
}

/// 줄 끝 강제 줄바꿈(공백 두 칸 이상 또는 `\`)이 있는지.
fn hard_break(line: &str) -> bool {
    line.ends_with("  ") || line.ends_with('\\')
}

/// 두 줄이 빈 줄 없이 줄바꿈 하나로 이어져야 하는지(빈 줄을 넣으면 뜻이 바뀌는지).
fn tight(prev: (&str, Kind), next: (&str, Kind)) -> bool {
    use Kind::*;
    let (prev_line, prev) = prev;
    let (next_line, next) = next;
    if matches!(prev, Verbatim | Grouped) || matches!(next, Verbatim | Grouped) {
        return false;
    }
    let indented_item = next_line.starts_with([' ', '\t']) && prev == ListItem;
    hard_break(prev_line)
        || matches!(
            (prev, next),
            (ListItem | Indented, ListItem | Indented)
                | (Quote, Quote)
                | (TableRow, TableRow)
                | (Text, Rule)
        )
        || indented_item
}

/// 빈 줄이 아닌 줄과 그 앞의 빈 줄 수. 펜스 안의 빈 줄은 빈 줄로 치지 않는다(원문 그대로).
struct Line<'a> {
    text: &'a str,
    kind: Kind,
    /// 이 줄 앞(바로 앞 줄 이후)의 빈 줄 수.
    blanks_before: usize,
}

fn lines_with_blanks<'a>(lines: &[&'a str], kinds: &[Kind]) -> (Vec<Line<'a>>, usize) {
    let mut out = Vec::new();
    let mut blanks = 0;
    for (line, &kind) in lines.iter().zip(kinds) {
        if kind == Kind::Blank {
            blanks += 1;
        } else {
            out.push(Line {
                text: line,
                kind,
                blanks_before: blanks,
            });
            blanks = 0;
        }
    }
    (out, blanks)
}

/// 텍스트를 줄로 나눈다. 끝의 줄바꿈 뒤에는 빈 줄이 하나 있는 것으로 친다.
///
/// 문서 처음과 끝의 빈 줄은 변환하지 않고 그대로 옮긴다.
fn split(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// 줄을 잇는다. 문서 처음의 빈 줄은 그대로 옮기고, 줄 사이에는 `gap`이 정한 줄바꿈을 넣는다.
fn join(out: &mut String, lines: &[Line<'_>], gap: impl Fn(&Line<'_>, &Line<'_>) -> String) {
    for (ix, line) in lines.iter().enumerate() {
        if ix == 0 {
            out.push_str(&"\n".repeat(line.blanks_before));
        } else {
            out.push_str(&gap(&lines[ix - 1], line));
        }
        out.push_str(line.text);
    }
}

/// 두 줄이 같은 펜스나 줄 묶음 안에 있는지(사이를 원문 그대로 둔다).
/// 펜스·묶음 안의 빈 줄은 종류가 `Verbatim`·`Grouped`인 빈 줄 Line으로 남으므로
/// `blanks_before`가 0이다. 바깥 빈 줄(`blanks_before` > 0)을 두고 마주 보는 같은 종류는
/// 서로 다른 두 블록의 경계니 원문 그대로 두지 않는다.
fn same_block(prev: &Line<'_>, next: &Line<'_>) -> bool {
    next.blanks_before == 0
        && matches!(
            (prev.kind, next.kind),
            (Kind::Verbatim, Kind::Verbatim) | (Kind::Grouped, Kind::Grouped)
        )
}

/// 에디터 본문 → 저장할 마크다운.
pub fn to_markdown(editor: &str) -> String {
    let lines = split(editor);
    let kinds = classify(&lines);
    let (lines, trailing_blanks) = lines_with_blanks(&lines, &kinds);
    if lines.is_empty() {
        return editor.to_string();
    }
    let mut out = String::with_capacity(editor.len() + lines.len());
    join(&mut out, &lines, |prev, next| {
        if same_block(prev, next) {
            // 펜스·줄 묶음 안의 빈 줄은 원문 그대로.
            return "\n".repeat(next.blanks_before + 1);
        }
        let blanks =
            if next.blanks_before == 0 && tight((prev.text, prev.kind), (next.text, next.kind)) {
                0
            } else {
                next.blanks_before + 1
            };
        "\n".repeat(blanks + 1)
    });
    out.push_str(&"\n".repeat(trailing_blanks));
    out
}

/// 저장된 마크다운 → 에디터 본문.
pub fn to_editor(markdown: &str) -> String {
    let lines = split(markdown);
    let kinds = classify(&lines);
    let (lines, trailing_blanks) = lines_with_blanks(&lines, &kinds);
    if lines.is_empty() {
        return markdown.to_string();
    }
    let mut out = String::with_capacity(markdown.len());
    for (ix, line) in lines.iter().enumerate() {
        if ix == 0 {
            out.push_str(&"\n".repeat(line.blanks_before));
            out.push_str(line.text);
            continue;
        }
        let prev = &lines[ix - 1];
        if same_block(prev, line) {
            out.push_str(&"\n".repeat(line.blanks_before + 1));
        } else if line.blanks_before > 0 {
            // 빈 줄 하나는 문단 경계, 나머지는 에디터의 빈 줄.
            out.push_str(&"\n".repeat(line.blanks_before));
        } else if !tight((prev.text, prev.kind), (line.text, line.kind))
            && line.kind == Kind::Text
            && matches!(prev.kind, Kind::Text | Kind::ListItem | Kind::Quote)
        {
            // 문단 안 줄바꿈(다른 앱에서 만든 파일): 한 줄로 잇는다.
            out.push(' ');
            out.push_str(line.text.trim_start());
            continue;
        } else {
            out.push('\n');
        }
        out.push_str(line.text);
    }
    out.push_str(&"\n".repeat(trailing_blanks));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 에디터 본문이 저장 후 다시 열었을 때 그대로인지.
    #[track_caller]
    fn round_trips(editor: &str) {
        let markdown = to_markdown(editor);
        assert_eq!(to_editor(&markdown), editor, "markdown: {markdown:?}");
    }

    #[test]
    fn lines_are_paragraphs_and_blank_lines_survive() {
        assert_eq!(to_markdown("A\nB"), "A\n\nB");
        assert_eq!(to_markdown("A\n\nB"), "A\n\n\nB");
        assert_eq!(to_markdown("A\n\n\nB\n"), "A\n\n\n\nB\n");
        assert_eq!(to_editor("A\n\nB"), "A\nB");
        assert_eq!(to_editor("A\n\n\n\nB"), "A\n\n\nB");
        for editor in [
            "",
            "A",
            "A\n",
            "\n\nA\nB\n\n",
            "# 제목\n문단\n\n## 장\n문단",
        ] {
            round_trips(editor);
        }
    }

    #[test]
    fn soft_breaks_from_other_apps_join_into_one_paragraph() {
        assert_eq!(
            to_editor("긴 문단의\n둘째 줄\n\n다음 문단"),
            "긴 문단의 둘째 줄\n다음 문단"
        );
        // 강제 줄바꿈은 이어 붙이지 않고 그대로 둔다.
        assert_eq!(to_editor("시의 첫 행  \n둘째 행"), "시의 첫 행  \n둘째 행");
        round_trips("시의 첫 행  \n둘째 행\n다음 연");
    }

    #[test]
    fn lists_quotes_and_tables_stay_tight() {
        let editor = "- 하나\n- 둘\n  - 셋\n1. 첫째\n2. 둘째\n> 인용\n> 이어짐\n| a | b |\n|---|---|\n| 1 | 2 |";
        let markdown = to_markdown(editor);
        assert!(markdown.contains("- 하나\n- 둘\n  - 셋"), "{markdown}");
        assert!(markdown.contains("> 인용\n> 이어짐"), "{markdown}");
        assert!(
            markdown.contains("| a | b |\n|---|---|\n| 1 | 2 |"),
            "{markdown}"
        );
        round_trips(editor);
        round_trips("문단\n- 목록\n다음 문단");
        round_trips("제목\n===\n문단");
    }

    #[test]
    fn fences_and_frontmatter_are_kept_verbatim() {
        let editor = "---\ntitle: a\n---\n문단\n```rust\nfn a() {}\n\n\nfn b() {}\n```\n$$\nx\n\ny\n$$\n문단";
        let markdown = to_markdown(editor);
        assert!(
            markdown.starts_with("---\ntitle: a\n---\n\n문단"),
            "{markdown}"
        );
        assert!(
            markdown.contains("```rust\nfn a() {}\n\n\nfn b() {}\n```"),
            "{markdown}"
        );
        assert!(markdown.contains("$$\nx\n\ny\n$$"), "{markdown}");
        round_trips(editor);
        // 닫히지 않은 펜스는 끝까지 그대로.
        round_trips("문단\n```\n코드\n\n코드");
        // 한 줄 수식은 펜스가 아니다.
        round_trips("$$x$$\n문단");
    }

    #[test]
    fn table_runs_skip_fences_and_groups() {
        let text = "| a |\n|---|\n본문\n```\n| 코드 |\n```\n:::\n| 묶음 |\n:::\n| b |";
        assert_eq!(table_runs(text), [0..2, 9..10]);
    }

    #[test]
    fn blank_lines_between_distinct_fenced_blocks_are_boundaries() {
        // 서로 다른 두 줄 묶음·펜스 사이의 빈 줄은 블록 경계다(k+1 규칙). 안의 빈 줄만 원문 그대로다.
        // 경계를 같은 묶음으로 보면 빈 줄 하나만 저장되어 프리뷰에서 간격이 사라진다.
        round_trips(":::\nA\n:::\n\n:::\nB\n:::");
        assert_eq!(
            to_markdown(":::\nA\n:::\n\n:::\nB\n:::"),
            ":::\nA\n:::\n\n\n:::\nB\n:::"
        );
        round_trips("```\nA\n```\n\n```\nB\n```");
        assert_eq!(
            to_markdown("```\nA\n```\n\n```\nB\n```"),
            "```\nA\n```\n\n\n```\nB\n```"
        );
        // 안의 빈 줄은 그대로다.
        assert_eq!(to_markdown(":::\nA\n\nB\n:::"), ":::\nA\n\nB\n:::");
    }

    #[test]
    fn line_groups_are_stored_as_written() {
        let editor = "앞 문단\n:::\n첫 행\n둘째 행\n\n셋째 행\n:::\n뒤 문단";
        assert_eq!(
            to_markdown(editor),
            "앞 문단\n\n:::\n첫 행\n둘째 행\n\n셋째 행\n:::\n\n뒤 문단"
        );
        round_trips(editor);
        // 묶음 안의 줄은 다른 앱 파일처럼 한 줄로 잇지 않는다.
        assert_eq!(to_editor(":::\nA\nB\n:::"), ":::\nA\nB\n:::");
        assert_eq!(
            line_groups(editor),
            vec![LineGroup {
                lines: 1..7,
                closed: true
            }]
        );
        // 닫지 않은 묶음은 끝까지, 코드 펜스 안의 `:::`는 묶음이 아니다.
        assert_eq!(
            line_groups("A\n::: 시\nB"),
            vec![LineGroup {
                lines: 1..3,
                closed: false
            }]
        );
        assert!(line_groups("```\n:::\n```").is_empty());
        // 묶음 안에서도 문장 자동 변환은 한다.
        assert!(!in_literal_context(":::\n가", 7));
    }

    /// 토글을 적용한 글과 선택한 글.
    fn toggled(text: &str, selection: Range<usize>) -> (String, String) {
        let (range, replacement, selected) = toggle_line_group(text, selection);
        let mut out = text.to_string();
        out.replace_range(range, &replacement);
        let selected = out[selected].to_string();
        (out, selected)
    }

    #[test]
    fn toggle_line_group_wraps_and_unwraps_selected_lines() {
        let text = "앞\n첫 행\n둘째 행\n뒤";
        // 줄 가운데부터 다음 줄 가운데까지 고르면 두 줄 전체를 감싼다.
        let start = text.find("행").unwrap();
        let end = text.find("째").unwrap();
        let (wrapped, selected) = toggled(text, start..end);
        assert_eq!(wrapped, "앞\n:::\n첫 행\n둘째 행\n:::\n뒤");
        assert_eq!(selected, "첫 행\n둘째 행");
        // 묶음 안을 고르고 다시 누르면 푼다.
        let inside = wrapped.find("둘째").unwrap();
        assert_eq!(
            toggled(&wrapped, inside..inside),
            (text.to_string(), "첫 행\n둘째 행".to_string())
        );
        // 빈 선택은 커서 줄, 다음 줄 첫머리에서 끝난 선택은 그 줄을 빼고 감싼다.
        assert_eq!(toggled("A\nB", 0..0).0, ":::\nA\n:::\nB");
        assert_eq!(toggled("A\nB", 0..2).0, ":::\nA\n:::\nB");
        // 펜스 줄만 골라도 풀고, 닫지 않은 묶음은 여는 줄만 지운다. 빈 묶음은 줄째 없앤다.
        assert_eq!(toggled(":::\nA\n:::", 0..0).0, "A");
        assert_eq!(toggled("X\n::: 시\nA", 9..9).0, "X\nA");
        assert_eq!(toggled("X\n:::\n:::\nY", 2..2).0, "X\nY");
        // 여러 묶음에 걸친 선택은 모두 푼다.
        let both = ":::\nA\n:::\nB\n:::\nC\n:::";
        assert_eq!(toggled(both, 0..both.len()).0, "A\nB\nC");
    }

    /// 각주를 넣은 글과 커서 앞 글.
    fn with_footnote(text: &str, at: usize, content: &str) -> (String, String) {
        let (range, replacement, cursor) = insert_footnote(text, at, content);
        let mut out = text.to_string();
        out.replace_range(range, &replacement);
        let before = out[..cursor].to_string();
        (out, before)
    }

    #[test]
    fn insert_footnote_adds_reference_and_definition() {
        // 처음 넣는 각주는 본문과 빈 줄 하나로 띄운다. 커서는 참조 바로 뒤.
        let (out, before) = with_footnote("첫 문장. 둘째", "첫 문장.".len(), " 출처\n설명 ");
        assert_eq!(out, "첫 문장.[^1] 둘째\n\n[^1]: 출처 설명");
        assert_eq!(before, "첫 문장.[^1]");
        // 다음 번호는 가장 큰 숫자 + 1, 정의는 끝 정의 줄 바로 다음. 끝 빈 줄은 버린다.
        let text = "A[^1] B[^note]\n\n[^1]: 하나\n[^note]: 이름\n\n";
        let (out, _) = with_footnote(text, "A[^1] B".len(), "둘");
        assert_eq!(
            out,
            "A[^1] B[^2][^note]\n\n[^1]: 하나\n[^note]: 이름\n[^2]: 둘"
        );
        // 빈 시트, 끝 줄(정의 아님)에 넣기.
        assert_eq!(with_footnote("", 0, "x").0, "[^1]\n\n[^1]: x");
        assert_eq!(with_footnote("끝", 3, "x").0, "끝[^1]\n\n[^1]: x");
        // 정의 줄 맨 앞에 넣으면 그 줄은 더는 정의가 아니다.
        let text = "A\n[^1]: 하나";
        assert_eq!(
            with_footnote(text, 2, "x").0,
            "A\n[^2][^1]: 하나\n\n[^2]: x"
        );
    }

    /// 여러 종류의 줄을 네 줄까지 모든 순서로 섞어도 원래대로 돌아온다.
    #[test]
    fn every_short_mix_of_lines_round_trips() {
        const SAMPLES: [&str; 15] = [
            "문단",
            "",
            "- 항목",
            "  - 하위",
            "> 인용",
            "| 표 |",
            "    코드",
            "===",
            "# 제목",
            "강제 줄바꿈  ",
            "```",
            "x",
            "---",
            "1. 번호",
            ":::",
        ];
        let mut stack = vec![Vec::<&str>::new()];
        while let Some(lines) = stack.pop() {
            let editor = lines.join("\n");
            round_trips(&editor);
            round_trips(&format!("{editor}\n"));
            if lines.len() < 4 {
                for sample in SAMPLES {
                    let mut next = lines.clone();
                    next.push(sample);
                    stack.push(next);
                }
            }
        }
    }

    #[test]
    fn standard_renderers_see_separate_paragraphs() {
        let html = markdown::to_html(&to_markdown("A\n\nB\nC"));
        assert_eq!(html, "<p>A</p>\n<p>B</p>\n<p>C</p>");
    }
}
