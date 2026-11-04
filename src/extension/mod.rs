//! 확장(ADR-0026): 라이브러리 `Extensions/<이름>.lua`(또는 여러 파일이면 `Extensions/<이름>/init.lua`와 모듈,
//! ADR-0039 [`package`])의 Lua(Luau) 스크립트.
//!
//! 확장 파일은 표 하나를 돌려주고, 앱은 그 안의 진입점만 쓴다.
//!
//! - `draw(g, t, args)`: 시트의 `` ```ext <이름> `` 펜스 자리에 그림을 그린다([`canvas`], LÖVE 모양 API).
//!   `t`는 초, `args`는 펜스 본문의 `key: value` 줄이다. `animated = true`면 보이는 동안 매 프레임 부른다.
//! - `commands = { { title = ..., run = function(ctx) ... end }, ... }`: `Extensions` 메뉴의 명령.
//!   돌려준 글로 선택 영역을 바꾸거나 커서 자리에 넣는다.
//!
//! 스크립트는 sandbox에서 돈다. 파일·프로세스·네트워크가 없고(Luau에는 `io`가 없고 `os`는 시간
//! 함수뿐이다), `require`는 확장 폴더의 모듈만 부른다. 메모리와 한 번 부를 때의 시간을 제한한다. `math.random`은 그릴 때마다
//! 같은 seed로 되돌려, 같은 `t`에서는 늘 같은 그림이 나온다(내보내기에서도 같아야 한다).
//!
//! 오류 글은 모델이 읽고 고치므로 영어로 쓴다.

mod boolean;
pub mod canvas;
pub mod image;
pub mod package;
pub(crate) mod path;
pub mod raster;
pub mod raster3d;
pub mod render;
mod scene;
pub mod sound;
pub mod svg;
mod worker;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use mlua::{Function, Lua, Table, Value, VmState};

pub use canvas::{
    Align, Blend, Color, FillRule, LineCap, LineJoin, Paint, Region, Segment, Shape, Theme,
    region_at,
};
pub use package::Package;
pub use worker::{Info, PlayEvent, PlayWorker, Worker};

/// 라이브러리 루트 안의 확장 폴더.
pub const EXTENSIONS_DIR: &str = "Extensions";

/// 확장 파일 확장자.
const EXTENSION_FILE: &str = "lua";

/// 펜스 info string의 첫 단어.
const FENCE_TAG: &str = "ext";

/// 확장 하나가 쓸 수 있는 메모리.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// 파일을 읽어 표를 돌려받는 데 쓸 수 있는 시간. 시트를 배치하려면 캔버스 크기가 필요해서 UI 스레드가
/// 이만큼 기다릴 수 있다. 확장의 최상위 코드는 표를 돌려주기만 하면 되므로 넉넉하다.
const LOAD_TIMEOUT: Duration = Duration::from_millis(100);

/// 반복 주기(`period`)의 최대 길이.
const MAX_PERIOD: f32 = 10.0;

/// 한 프레임을 그리는 데 쓸 수 있는 시간.
const DRAW_TIMEOUT: Duration = Duration::from_millis(100);

/// 명령 하나에 쓸 수 있는 시간.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// 이보다 큰 확장 파일은 읽지 않는다.
const MAX_SOURCE_BYTES: u64 = 512 * 1024;

/// `size`를 적지 않은 캔버스 크기.
const DEFAULT_SIZE: (f32, f32) = (400.0, 200.0);

/// 캔버스 한 변의 최대 크기.
const MAX_SIDE: f32 = 4096.0;

/// `math.random`의 seed. 그릴 때마다 되돌린다.
const RANDOM_SEED: i64 = 1;

/// 시트 안의 확장 블록 하나(`` ```ext <이름> `` 펜스).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// 펜스 여는 줄부터 닫는 줄까지(닫는 줄 포함).
    pub rows: Range<usize>,
    pub name: String,
    /// 본문의 `key: value` 줄. 순서대로.
    pub args: Vec<(String, String)>,
}

/// 앱이 읽는 블록 인자: 캔버스를 이 배율로 그린다(0.1~4, 기본 1). 확장에는 넘기지 않는다.
pub const SCALE_ARG: &str = "scale";

/// 앱이 읽는 블록 인자: 캔버스를 본문 폭 안 왼쪽·가운데·오른쪽에 둔다(`left`·`center`·`right`, 기본 가운데, ADR-0034).
pub const ALIGN_ARG: &str = "align";

/// 앱이 읽는 블록 인자: 책 미리보기·내보내기(PDF·ePub·DOCX)에 넣을지(`true`·`false`). 없으면 확장의 `export`를 따른다.
pub const EXPORT_ARG: &str = "export";

/// 앱이 읽는 블록 인자: 블록의 이름. 다른 블록의 `click`이 `set` 동작으로 이 블록의 인자를 바꿀 때 찾는다(ADR-0035).
pub const ID_ARG: &str = "id";

/// 앱이 읽고 확장에는 넘기지 않는 블록 인자.
pub const RESERVED_ARGS: &[&str] = &[SCALE_ARG, ALIGN_ARG, EXPORT_ARG, ID_ARG];

impl Block {
    /// 블록의 배율(`scale:`). 숫자가 아니면 1이다.
    pub fn scale(&self) -> f32 {
        scale_of(&self.args)
    }

    /// 확장에 넘길 인자(예약 인자 뺌).
    pub fn extension_args(&self) -> Vec<(String, String)> {
        without_reserved(&self.args)
    }
}

/// 같은 인자가 여럿이면 마지막 값.
fn last_arg<'a>(args: &'a [(String, String)], key: &str) -> Option<&'a str> {
    args.iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.trim())
}

/// 블록의 이름(`id:`). 없거나 비었으면 `None`.
pub fn id_of(args: &[(String, String)]) -> Option<&str> {
    last_arg(args, ID_ARG).filter(|id| !id.is_empty())
}

/// 인자의 정렬(`align:`). 없거나 모르는 값이면 가운데다.
pub fn align_of(args: &[(String, String)]) -> Align {
    match last_arg(args, ALIGN_ARG) {
        Some("left") => Align::Left,
        Some("right") => Align::Right,
        _ => Align::Center,
    }
}

/// 인자의 내보내기 설정(`export:`). 없거나 `true`·`false`가 아니면 `None`(확장을 따른다).
pub fn export_arg(args: &[(String, String)]) -> Option<bool> {
    match last_arg(args, EXPORT_ARG) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// 인자의 배율(`scale:`). 없거나 숫자가 아니면 1이고, 0.1~4로 자른다.
pub fn scale_of(args: &[(String, String)]) -> f32 {
    last_arg(args, SCALE_ARG)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|scale| scale.is_finite())
        .map_or(1.0, |scale| scale.clamp(0.1, 4.0))
}

/// 예약 인자(`scale`·`align`·`export`)를 뺀 인자. 확장에 넘기는 것이다.
pub fn without_reserved(args: &[(String, String)]) -> Vec<(String, String)> {
    args.iter()
        .filter(|(key, _)| !RESERVED_ARGS.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// 본문에서 확장 블록을 찾는다. 닫지 않은 펜스는 블록이 아니다(쓰는 중이다).
/// 다른 코드 펜스 안의 `` ```ext ``는 글자일 뿐이다.
pub fn blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut found = Vec::new();
    let mut row = 0;
    while row < lines.len() {
        let Some((fence, info)) = open_fence(lines[row]) else {
            row += 1;
            continue;
        };
        let close = (row + 1..lines.len()).find(|&ix| closes(lines[ix], fence));
        let Some(close) = close else {
            break;
        };
        let mut words = info.split_whitespace();
        if words.next() == Some(FENCE_TAG)
            && let Some(name) = words.next().filter(|name| valid_name(name))
        {
            found.push(Block {
                rows: row..close + 1,
                name: name.to_string(),
                args: parse_args(&lines[row + 1..close]),
            });
        }
        row = close + 1;
    }
    found
}

/// 펜스를 여는 줄이면 (펜스 글자와 길이, info string).
fn open_fence(line: &str) -> Option<((char, usize), &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let mark = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let count = trimmed.chars().take_while(|c| *c == mark).count();
    if count < 3 {
        return None;
    }
    let info = &trimmed[count..];
    // 백틱 펜스의 info string에는 백틱이 없어야 한다(CommonMark).
    if mark == '`' && info.contains('`') {
        return None;
    }
    Some(((mark, count), info.trim()))
}

fn closes(line: &str, (mark, count): (char, usize)) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let run = trimmed.chars().take_while(|c| *c == mark).count();
    run >= count && trimmed[run..].trim().is_empty()
}

/// 확장 이름: 글자·숫자·`-`·`_`만(파일 이름이 되므로 경로 문자는 안 된다).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
}

/// 펜스 info string(`ext <이름>`)이 확장 블록이면 그 이름.
pub fn fence_name(info: &str) -> Option<&str> {
    let mut words = info.split_whitespace();
    (words.next() == Some(FENCE_TAG))
        .then(|| words.next())
        .flatten()
        .filter(|name| valid_name(name))
}

/// 문단 안의 인라인 확장 하나(`{{name key=value}}`, ADR-0036). `range`는 줄 안의 바이트 범위다(`{{`·`}}` 포함).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineCall {
    pub range: Range<usize>,
    pub name: String,
    pub args: Vec<(String, String)>,
}

/// 줄의 인라인 확장. `` ` ``로 감싼 코드 안은 글자다. 이름이 글자로 시작하는 확장 이름 꼴이 아니면 확장이 아니다
/// (`{{ }}`를 쓰는 다른 글이 그대로 남는다).
pub fn inline_calls(line: &str) -> Vec<InlineCall> {
    let mut found = Vec::new();
    if !line.contains("{{") {
        return found;
    }
    // 인라인 코드(같은 길이의 백틱 사이) 범위.
    let mut code: Vec<Range<usize>> = Vec::new();
    let bytes = line.as_bytes();
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] != b'`' {
            ix += 1;
            continue;
        }
        let run = bytes[ix..].iter().take_while(|&&b| b == b'`').count();
        let open = ix;
        let mut close = None;
        let mut jx = ix + run;
        while jx < bytes.len() {
            if bytes[jx] == b'`' {
                let other = bytes[jx..].iter().take_while(|&&b| b == b'`').count();
                if other == run {
                    close = Some(jx + other);
                    break;
                }
                jx += other;
            } else {
                jx += 1;
            }
        }
        match close {
            Some(end) => {
                code.push(open..end);
                ix = end;
            }
            None => ix += run,
        }
    }
    let mut at = 0;
    while let Some(start) = line[at..].find("{{").map(|found| at + found) {
        let Some(end) = line[start + 2..]
            .find("}}")
            .map(|found| start + 2 + found + 2)
        else {
            break;
        };
        at = end;
        if code.iter().any(|code| code.start < end && start < code.end) {
            continue;
        }
        let inner = line[start + 2..end - 2].trim();
        let mut words = split_words(inner).into_iter();
        // 숫자로 시작하는 이름은 확장으로 보지 않는다(`{{ 3 }}` 같은 글이 남게).
        let Some(name) = words
            .next()
            .filter(|name| valid_name(name) && name.starts_with(char::is_alphabetic))
        else {
            continue;
        };
        let args = words
            .filter_map(|word| {
                let (key, value) = word.split_once('=')?;
                let key = key.trim();
                (!key.is_empty()).then(|| (key.to_string(), value.to_string()))
            })
            .collect();
        found.push(InlineCall {
            range: start..end,
            name,
            args,
        });
    }
    found
}

/// 빈칸으로 나눈 낱말. `"…"` 안의 빈칸은 나누지 않고, 따옴표는 뺀다(`\"`는 따옴표 글자).
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted && chars.peek() == Some(&'"') => {
                word.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// 인라인 확장의 원문 `{{name key=value}}`. 빈칸·따옴표·`}`가 있는 값은 따옴표로 감싼다.
pub fn inline_text(name: &str, args: &[(String, String)]) -> String {
    let mut text = format!("{{{{{name}");
    for (key, value) in args {
        let plain = !value.is_empty()
            && !value.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '}' | '{'));
        if plain {
            text.push_str(&format!(" {key}={value}"));
        } else {
            text.push_str(&format!(" {key}=\"{}\"", value.replace('"', "\\\"")));
        }
    }
    text.push_str("}}");
    text
}

/// 책 미리보기·내보내기의 인라인 확장(ADR-0036): 원고 글자의 `{{…}}`를 그 확장의 글(`value`)로, 글이 없는
/// 그림 확장은 그림 조각(`Inline::object`)으로 바꾼다. 내보내기에서 빼는 것(`export:`·확장의 `export`)은 지운다.
/// 확장을 읽지 못하거나 실패하면 원문 그대로 둔다(무엇이 잘못됐는지 보이게). 코드 글자 안은 건드리지 않는다.
pub fn resolve_inline(blocks: &mut [crate::markdown::Block], root: &Path, sheet: &str) {
    use crate::markdown::{Inline, InlineObject};
    /// 호출 하나가 바뀌는 것.
    enum Resolved {
        Text(String),
        Object(InlineObject),
    }
    let mut loaded: std::collections::HashMap<String, Option<Extension>> =
        std::collections::HashMap::new();
    crate::markdown::for_each_rich_mut(blocks, &mut |rich| {
        if !rich.iter().any(|inline| inline.text.contains("{{")) {
            return;
        }
        let mut out: Vec<Inline> = Vec::with_capacity(rich.len());
        for inline in rich.drain(..) {
            if inline.marks.code || inline.footnote.is_some() || !inline.text.contains("{{") {
                out.push(inline);
                continue;
            }
            let calls = inline_calls(&inline.text);
            if calls.is_empty() {
                out.push(inline);
                continue;
            }
            let piece = |text: String| Inline {
                text,
                ..inline.clone()
            };
            let mut text = String::with_capacity(inline.text.len());
            let mut at = 0;
            for call in calls {
                text.push_str(&inline.text[at..call.range.start]);
                let source = &inline.text[call.range.clone()];
                let chosen = export_arg(&call.args);
                let resolved = if chosen == Some(false) {
                    Some(Resolved::Text(String::new()))
                } else {
                    let extension = loaded
                        .entry(call.name.clone())
                        .or_insert_with(|| Extension::open(root, &call.name).ok());
                    match extension {
                        None => None,
                        Some(extension) if !chosen.unwrap_or(extension.export) => {
                            Some(Resolved::Text(String::new()))
                        }
                        Some(extension) if extension.values => extension
                            .value(&without_reserved(&call.args), sheet)
                            .ok()
                            .flatten()
                            .map(Resolved::Text),
                        Some(extension) if extension.draws => {
                            Some(Resolved::Object(InlineObject {
                                name: call.name.clone(),
                                args: call.args.clone(),
                            }))
                        }
                        Some(_) => Some(Resolved::Text(String::new())),
                    }
                };
                match resolved {
                    Some(Resolved::Text(value)) => text.push_str(&value),
                    Some(Resolved::Object(object)) => {
                        if !text.is_empty() {
                            out.push(piece(std::mem::take(&mut text)));
                        }
                        out.push(Inline {
                            text: String::new(),
                            object: Some(object),
                            ..inline.clone()
                        });
                    }
                    None => text.push_str(source),
                }
                at = call.range.end;
            }
            text.push_str(&inline.text[at..]);
            if !text.is_empty() {
                out.push(piece(text));
            }
        }
        *rich = out;
    });
}

/// 블록 본문의 `key: value` 줄.
pub fn parse_args(lines: &[&str]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// 블록 본문 줄에 인자 변경을 적는다(`click`의 결과, ADR-0032). 있는 인자는 마지막 줄의 값만 바꾸고(들여쓰기와
/// `key:` 모양은 그대로), 없는 인자는 끝에 `key: value` 줄로 더한다.
pub fn set_args(body: &[&str], changes: &[(String, String)]) -> Vec<String> {
    let mut lines: Vec<String> = body.iter().map(ToString::to_string).collect();
    for (key, value) in changes {
        let found = lines.iter().rposition(|line| {
            line.split_once(':')
                .is_some_and(|(name, _)| name.trim() == key)
        });
        match found {
            Some(ix) => {
                let (name, _) = lines[ix].split_once(':').unwrap_or_default();
                lines[ix] = format!("{name}: {value}").trim_end().to_string();
            }
            None => lines.push(format!("{key}: {value}").trim_end().to_string()),
        }
    }
    lines
}

/// 내보내기·책 미리보기에 넣는 확장 블록 그림: 밝은 테마, `t = 0` 한 장.
#[derive(Clone, Debug)]
pub struct Still {
    /// 캔버스 크기(캔버스 좌표).
    pub canvas: (f32, f32),
    /// 블록의 `scale:`.
    pub scale: f32,
    /// 블록의 `align:`.
    pub align: Align,
    pub shapes: std::sync::Arc<Vec<Shape>>,
}

/// 확장 블록(이름과 인자, 예약 인자 포함)을 내보내기용으로 그린다. 내보내기에서 빼는 블록이면 `Ok(None)`이다
/// (블록의 `export: false`, 또는 `export:`가 없고 확장이 `export = false`). 그리지 못하면 `Err`(코드 블록으로 둔다).
pub fn still(root: &Path, name: &str, args: &[(String, String)]) -> Result<Option<Still>, String> {
    let chosen = export_arg(args);
    if chosen == Some(false) {
        return Ok(None);
    }
    let extension = Extension::open(root, name)?;
    if !chosen.unwrap_or(extension.export) {
        return Ok(None);
    }
    if !extension.draws {
        return Err(format!("{name}: no draw function"));
    }
    let own = without_reserved(args);
    let canvas = extension.size_for(&own)?;
    let shapes = extension.draw(0.0, &own, &Theme::default())?;
    Ok(Some(Still {
        canvas,
        scale: scale_of(args),
        align: align_of(args),
        shapes: std::sync::Arc::new(shapes),
    }))
}

/// 확장의 진입 파일 경로: 폴더 확장이면 `<이름>/init.lua`, 아니면 `<이름>.lua`(있든 없든).
pub fn path(root: &Path, name: &str) -> PathBuf {
    if package::is_folder(root, name) {
        package::folder(root, name).join(package::ENTRY_FILE)
    } else {
        package::single_file(root, name)
    }
}

/// 확장이 차지하는 항목: 폴더 확장이면 폴더, 아니면 파일(지우기·이름 바꾸기에 쓴다).
pub fn location(root: &Path, name: &str) -> PathBuf {
    if package::is_folder(root, name) {
        package::folder(root, name)
    } else {
        package::single_file(root, name)
    }
}

/// 라이브러리를 열 때 확장 폴더를 만들어 둔다(모델이 바로 파일을 쓸 수 있게).
pub fn ensure_dir(root: &Path) -> std::io::Result<()> {
    let dir = root.join(EXTENSIONS_DIR);
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Ok(());
    }
    std::fs::create_dir(dir)
}

/// 확장 폴더의 확장 이름과 수정 시각(이름순). 파일 하나(`<이름>.lua`)와 폴더(`<이름>/init.lua`)를 모두
/// 센다. 바뀌었는지 비교할 때 쓴다(폴더는 안의 파일이 바뀌어도 바뀐다).
pub fn scan(root: &Path) -> Vec<(String, Option<SystemTime>)> {
    let Ok(entries) = std::fs::read_dir(root.join(EXTENSIONS_DIR)) else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).ok()?;
            let name = if meta.is_dir() {
                path.file_name()?.to_str()?.to_string()
            } else if meta.is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some(EXTENSION_FILE)
            {
                path.file_stem()?.to_str()?.to_string()
            } else {
                return None;
            };
            if !valid_name(&name) || (meta.is_dir() && !package::is_folder(root, &name)) {
                return None;
            }
            Some((name.clone(), package::modified(root, &name)))
        })
        .collect();
    found.sort();
    // 파일과 폴더가 같은 이름이면 하나만 센다(읽을 때 알린다).
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

/// 확장 폴더의 모든 명령: (확장 이름, 명령 번호, 제목). 읽지 못한 확장은 건너뛴다.
pub fn commands(root: &Path) -> Vec<(String, usize, String)> {
    scan(root)
        .into_iter()
        .filter_map(|(name, _)| Some((Extension::open(root, &name).ok()?, name)))
        .flat_map(|(extension, name)| {
            extension
                .commands
                .iter()
                .enumerate()
                .map(|(ix, title)| (name.clone(), ix, title.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 확장 파일 맨 위의 앞머리(`--[[---` … `---]]` 블록 주석). Lua를 돌리지 않고 읽는다.
///
/// 목록·Insert 대화상자·시스템 프롬프트에 쓴다. 사람에게 보이는 글이라 설명은 사용자의 언어로 쓴다
/// (모델이 읽는 글을 영어로 쓰는 규칙의 예외).
///
/// ```lua
/// --[[---
/// name: archer
/// description: 궁수가 과녁에 화살을 쏜다.
/// args:
///   speed: 화살 속도. 기본값 1.
/// example:
///   speed: 1.5
/// ---]]
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// 확장 이름(파일 이름). 앞머리의 `name`이 다르면 파일 이름을 따른다(블록은 파일 이름으로 찾는다).
    pub name: String,
    pub description: String,
    /// 블록 인자와 그 설명. 순서대로.
    pub args: Vec<(String, String)>,
    /// 미리보기와 블록 넣기에 쓰는 인자 값.
    pub example: Vec<(String, String)>,
    /// `click`이 돌려줄 수 있는 동작(`actions: set, command` 한 줄, ADR-0035).
    pub actions: Vec<String>,
}

const META_OPEN: &str = "--[[---";
const META_CLOSE: &str = "---]]";

/// 앞머리를 읽는다. 파일이 앞머리로 시작하지 않으면(앞의 빈 줄은 괜찮다) `None`.
pub fn parse_meta(name: &str, source: &str) -> Option<Meta> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let rest = source.trim_start().strip_prefix(META_OPEN)?;
    let (body, _) = rest.split_once(META_CLOSE)?;
    let mut meta = Meta {
        name: name.to_string(),
        ..Meta::default()
    };
    let mut section: Option<&mut Vec<(String, String)>> = None;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if indented {
            if let Some(section) = section.as_deref_mut()
                && !key.is_empty()
            {
                section.push((key.to_string(), value.to_string()));
            }
            continue;
        }
        section = None;
        match key {
            "description" => meta.description = value.to_string(),
            "actions" => {
                meta.actions = value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(ToString::to_string)
                    .collect();
            }
            "args" => section = Some(&mut meta.args),
            "example" => section = Some(&mut meta.example),
            _ => {}
        }
    }
    Some(meta)
}

/// 확장 폴더의 앞머리 목록(이름순). 앞머리가 없는 확장은 이름만 있다.
pub fn catalog(root: &Path) -> Vec<Meta> {
    scan(root)
        .into_iter()
        .map(|(name, _)| {
            std::fs::read_to_string(path(root, &name))
                .ok()
                .and_then(|source| parse_meta(&name, &source))
                .unwrap_or(Meta {
                    name,
                    ..Meta::default()
                })
        })
        .collect()
}

/// 시트에 넣을 블록 글(끝 줄바꿈 없음).
pub fn block_text(name: &str, args: &[(String, String)]) -> String {
    let mut text = format!("```{FENCE_TAG} {name}\n");
    for (key, value) in args {
        text.push_str(&format!("{key}: {value}\n"));
    }
    text.push_str("```");
    text
}

/// 그릴 때 넘기는 마우스 상태(`draw`의 `ui`, ADR-0032). 내보내기·미리보기처럼 마우스가 없으면 비어 있다.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Ui {
    /// 포인터가 올라가 있는 영역.
    pub hover: Option<String>,
    /// 누르고 있는 영역.
    pub pressed: Option<String>,
}

/// 한 프레임의 그림: 모양과 누를 수 있는 영역.
pub type Drawing = (Vec<Shape>, Vec<Region>);

/// `click`이 앱에 맡기는 동작(ADR-0035). 확장은 앱을 직접 만지지 않고, 앱이 받은 동작을 실행한다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// 같은 시트에서 `id:`가 `block`인 블록들의 인자를 바꾼다.
    Set {
        block: String,
        args: Vec<(String, String)>,
    },
    /// 다른 확장의 메뉴 명령을 지금 선택 영역에 실행한다.
    Command {
        extension: String,
        command: CommandRef,
    },
    /// 커서 자리에 글을 넣는다.
    Insert(String),
    /// 선택 영역을 글로 바꾼다(선택이 없으면 커서 자리에 넣는다).
    ReplaceSelection(String),
    /// 라이브러리의 시트를 연다(라이브러리 루트 기준 경로).
    Open(String),
    /// 앱 메뉴 동작([`MENU_ACTIONS`]).
    Menu(String),
    /// 이 확장의 `play`를 창에서 실행한다(인자는 `play.load(args)`로, ADR-0038).
    Play(Vec<(String, String)>),
}

/// 명령을 가리키는 번호(0부터) 또는 제목.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandRef {
    Index(usize),
    Title(String),
}

/// `menu` 동작으로 부를 수 있는 앱 메뉴.
pub const MENU_ACTIONS: &[&str] = &["preview", "compile"];

/// 동작 이름(앞머리 `actions:`에 적는 이름).
pub const ACTION_NAMES: &[&str] = &[
    "set",
    "command",
    "editor.insert",
    "editor.replaceSelection",
    "open",
    "menu",
    "play",
];

/// 한 번 누를 때 돌려줄 수 있는 동작 수.
const MAX_ACTIONS: usize = 16;

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Set { .. } => "set",
            Action::Command { .. } => "command",
            Action::Insert(_) => "editor.insert",
            Action::ReplaceSelection(_) => "editor.replaceSelection",
            Action::Open(_) => "open",
            Action::Menu(_) => "menu",
            Action::Play(_) => "play",
        }
    }

    /// 원고를 바꾸거나 다른 확장을 부르는 동작. 확장마다 처음 쓸 때 사용자에게 허락받는다. 자기 시트 블록의 인자를
    /// 바꾸는 `set`은 되돌리기로 돌아오는 블록 안의 일이라 묻지 않는다.
    pub fn needs_consent(&self) -> bool {
        matches!(
            self,
            Action::Command { .. } | Action::Insert(_) | Action::ReplaceSelection(_)
        )
    }

    /// 보고·허락 창에 보일 한 줄(모델이 읽는 영어).
    pub fn describe(&self) -> String {
        let pairs = |args: &[(String, String)]| {
            args.iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Action::Set { block, args } => format!("set block {block} ({})", pairs(args)),
            Action::Command {
                extension,
                command: CommandRef::Index(ix),
            } => format!("command {} of {extension}", ix + 1),
            Action::Command {
                extension,
                command: CommandRef::Title(title),
            } => format!("command \"{title}\" of {extension}"),
            Action::Insert(text) => format!("editor.insert {text:?}"),
            Action::ReplaceSelection(text) => format!("editor.replaceSelection {text:?}"),
            Action::Open(sheet) => format!("open {sheet}"),
            Action::Menu(menu) => format!("menu {menu}"),
            Action::Play(args) => format!("play ({})", pairs(args)),
        }
    }
}

/// 누른 결과: 자기 블록의 인자 변경과 앱에 맡기는 동작.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clicked {
    pub args: Vec<(String, String)>,
    pub actions: Vec<Action>,
}

impl Clicked {
    pub fn is_empty(&self) -> bool {
        self.args.is_empty() && self.actions.is_empty()
    }
}

/// 명령에 넘기는 정보.
#[derive(Clone, Debug, Default)]
pub struct CommandContext {
    /// 선택한 글(없으면 빈 글).
    pub selection: String,
    /// 시트 본문 전체.
    pub text: String,
    /// 시트 경로(라이브러리 루트 기준).
    pub sheet: String,
}

/// 읽은 확장 하나. Lua 상태를 들고 있어 만든 스레드에서만 쓴다.
pub struct Extension {
    lua: Lua,
    module: Table,
    g: Table,
    recorder: Rc<RefCell<canvas::Recorder>>,
    deadline: Rc<Cell<Instant>>,
    /// `sound`가 재생할 묶음(플레이 창에서만 채운다, ADR-0040).
    sound: sound::Slot,
    /// 캔버스 크기(논리 단위). 블록 인자로 정하는 확장(`size = function(args)`)이면 `None`이고
    /// [`Self::size_for`]로 묻는다.
    pub size: Option<(f32, f32)>,
    size_fn: Option<Function>,
    pub animated: bool,
    /// 애니메이션이 되풀이되는 주기(초). 있으면 `t`가 그 주기를 되풀이한다.
    pub period: Option<f32>,
    pub draws: bool,
    /// 영역을 누르면 부르는 `click(id, args)`가 있는지.
    pub clicks: bool,
    /// 인라인 확장으로 글을 보이는 `value(args, ctx)`가 있는지(ADR-0036).
    pub values: bool,
    /// 창에서 실행하는 `play` 표가 있는지(ADR-0038).
    pub plays: bool,
    /// 책 미리보기·내보내기에 넣는지(반환 표의 `export`, 기본 `true`). 블록의 `export:`가 이긴다.
    pub export: bool,
    /// 앞머리 `actions:`에 적은 동작. `click`은 이 동작만 돌려줄 수 있다.
    pub actions: Vec<String>,
    /// 명령 제목(순서대로).
    pub commands: Vec<String>,
}

impl std::fmt::Debug for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extension")
            .field("size", &self.size)
            .field("animated", &self.animated)
            .field("draws", &self.draws)
            .field("commands", &self.commands)
            .finish_non_exhaustive()
    }
}

/// 확장의 소스(폴더면 모듈까지). 이름·파일 종류·크기를 검사한다.
pub fn read_source(root: &Path, name: &str) -> Result<Package, String> {
    let mut package = read_package(root, name)?;
    package.images = Some(image::ImageRoots {
        library: root.to_path_buf(),
        folder: package::is_folder(root, name).then(|| package::folder(root, name)),
    });
    Ok(package)
}

fn read_package(root: &Path, name: &str) -> Result<Package, String> {
    if !valid_name(name) {
        return Err(format!(
            "Invalid extension name \"{name}\" (use letters, digits, - and _)."
        ));
    }
    if package::is_folder(root, name) {
        if package::single_file(root, name).exists() {
            return Err(format!(
                "Both {EXTENSIONS_DIR}/{name}.{EXTENSION_FILE} and {EXTENSIONS_DIR}/{name}/ exist; keep one."
            ));
        }
        return package::read_folder(&package::folder(root, name))
            .map_err(|err| format!("{EXTENSIONS_DIR}/{name}/: {err}"));
    }
    let file = package::single_file(root, name);
    let meta = std::fs::symlink_metadata(&file).map_err(|_| {
        format!("No extension named \"{name}\" ({EXTENSIONS_DIR}/{name}.{EXTENSION_FILE}).")
    })?;
    if !meta.is_file() {
        return Err(format!(
            "{EXTENSIONS_DIR}/{name}.{EXTENSION_FILE} is not a file."
        ));
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "The extension file is too large (limit {MAX_SOURCE_BYTES} bytes)."
        ));
    }
    let source = std::fs::read_to_string(&file).map_err(|err| err.to_string())?;
    Ok(source.into())
}

/// Lua 오류를 모델이 읽을 한 줄로.
fn message(err: mlua::Error) -> String {
    match err {
        mlua::Error::CallbackError { cause, traceback } => {
            let cause = message((*cause).clone());
            let line = traceback
                .lines()
                .map(str::trim)
                .find(|line| line.contains(".lua"))
                .map(|line| format!(" (at {line})"))
                .unwrap_or_default();
            format!("{cause}{line}")
        }
        mlua::Error::RuntimeError(text) | mlua::Error::SyntaxError { message: text, .. } => text,
        mlua::Error::MemoryError(_) => "out of memory (limit 64 MB)".into(),
        other => other.to_string(),
    }
}

impl Extension {
    /// `root`(라이브러리)의 확장 `name`을 읽는다.
    pub fn open(root: &Path, name: &str) -> Result<Self, String> {
        Self::load_package(name, &read_source(root, name)?)
    }

    /// 플레이 창에서 `sound`가 소리 묶음 `group`으로 재생하게 한다(ADR-0040).
    pub fn enable_sound(&self, group: u64) -> Result<(), String> {
        self.sound.set(Some(group));
        Ok(())
    }

    /// 앱이 알아야 할 것(작업 스레드에서 UI 스레드로 넘긴다).
    pub fn info(&self) -> Info {
        Info {
            size: self.size,
            animated: self.animated,
            period: self.period,
            draws: self.draws,
            clicks: self.clicks,
            values: self.values,
            plays: self.plays,
            export: self.export,
            commands: self.commands.clone(),
        }
    }

    /// 파일 하나의 소스에서 확장을 읽는다.
    #[cfg(test)]
    pub fn load(name: &str, source: &str) -> Result<Self, String> {
        Self::load_package(name, &Package::from(source))
    }

    /// 소스(폴더면 모듈까지)에서 확장을 읽는다.
    pub fn load_package(name: &str, package: &Package) -> Result<Self, String> {
        let entry = if package.is_folder() {
            format!("{name}/{}", package::ENTRY_FILE)
        } else {
            format!("{name}.{EXTENSION_FILE}")
        };
        Self::try_load(name, &entry, package).map_err(|err| format!("{entry}: {err}"))
    }

    fn try_load(name: &str, entry: &str, package: &Package) -> Result<Self, String> {
        let source = package.main.as_str();
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY_LIMIT).map_err(message)?;
        // `require`는 이 확장의 모듈만 부른다(파일·다른 확장은 못 읽는다).
        package::install_require(&lua, name, package).map_err(message)?;
        // 캔버스 `g`는 파일 맨 위에서도 쓸 수 있다(경로·그라디언트를 한 번 만들어 두고 draw에서 다시 쓴다).
        // 크기는 그릴 때마다 다시 정한다.
        let (width, height) = DEFAULT_SIZE;
        let recorder = Rc::new(RefCell::new(canvas::Recorder::new(width, height)));
        let deadline = Rc::new(Cell::new(Instant::now() + LOAD_TIMEOUT));
        let g = canvas::create(
            &lua,
            recorder.clone(),
            package.images.clone(),
            deadline.clone(),
        )
        .map_err(message)?;
        lua.sandbox(true).map_err(message)?;
        // sandbox가 그 전 전역 표를 읽기 전용으로 얼리므로(`g.theme`을 그릴 때마다 바꾼다) 그 뒤에 넣는다.
        lua.globals().raw_set("g", g.clone()).map_err(message)?;
        // 소리는 플레이 창에서만 난다([`Extension::enable_sound`]). 그 밖에서는 부르면 알린다.
        let sound = sound::install(&lua).map_err(message)?;
        let check = deadline.clone();
        lua.set_interrupt(move |_| {
            if Instant::now() > check.get() {
                Err(mlua::Error::runtime(
                    "took too long (an endless loop?); each call has a time limit",
                ))
            } else {
                Ok(VmState::Continue)
            }
        });
        let value: Value = lua
            .load(source)
            .set_name(format!("@{entry}"))
            .eval()
            .map_err(message)?;
        let Value::Table(module) = value else {
            return Err(format!(
                "the file must return a table (return {{ draw = ..., commands = ... }}), got {}",
                value.type_name()
            ));
        };
        let draws = matches!(module.get::<Value>("draw"), Ok(Value::Function(_)));
        let plays = match module.get::<Value>("play").map_err(message)? {
            Value::Nil => false,
            Value::Table(play) => {
                if !matches!(play.get::<Value>("draw"), Ok(Value::Function(_))) {
                    return Err(
                        "play needs a draw function: play = { draw = function(g) ... end }".into(),
                    );
                }
                true
            }
            other => {
                return Err(format!(
                    "play must be a table of functions (load, update, draw, keypressed, ...), got {}",
                    other.type_name()
                ));
            }
        };
        let values = match module.get::<Value>("value").map_err(message)? {
            Value::Nil => false,
            Value::Function(_) => true,
            other => {
                return Err(format!(
                    "value must be a function(args, ctx), got {}",
                    other.type_name()
                ));
            }
        };
        let clicks = match module.get::<Value>("click").map_err(message)? {
            Value::Nil => false,
            Value::Function(_) if draws => true,
            Value::Function(_) => return Err("click needs a draw function with regions".into()),
            other => {
                return Err(format!(
                    "click must be a function(id, args), got {}",
                    other.type_name()
                ));
            }
        };
        let commands = command_titles(&module)?;
        if !draws && !values && !plays && commands.is_empty() {
            return Err("the returned table has none of draw, value, play, or commands".into());
        }
        let (size, size_fn) = match module.get::<Value>("size").map_err(message)? {
            Value::Function(size_fn) => (None, Some(size_fn)),
            value => (Some(canvas_size(value)?), None),
        };
        let animated = match module.get::<Value>("animated").map_err(message)? {
            Value::Nil => false,
            Value::Boolean(animated) => animated,
            other => {
                return Err(format!(
                    "animated must be true or false, got {}",
                    other.type_name()
                ));
            }
        };
        let export = match module.get::<Value>("export").map_err(message)? {
            Value::Nil => true,
            Value::Boolean(export) => export,
            other => {
                return Err(format!(
                    "export must be true or false, got {}",
                    other.type_name()
                ));
            }
        };
        let period = match module.get::<Value>("period").map_err(message)? {
            Value::Nil => None,
            Value::Integer(n) => Some(n as f32),
            Value::Number(n) => Some(n as f32),
            other => {
                return Err(format!(
                    "period must be a number of seconds, got {}",
                    other.type_name()
                ));
            }
        };
        if let Some(period) = period
            && !(period > 0.0 && period <= MAX_PERIOD)
        {
            return Err(format!("period must be between 0 and {MAX_PERIOD} seconds"));
        }
        Ok(Self {
            lua,
            sound,
            module,
            g,
            recorder,
            deadline,
            size,
            size_fn,
            animated,
            period,
            draws,
            clicks,
            values,
            plays,
            export,
            actions: parse_meta(name, source)
                .map(|meta| meta.actions)
                .unwrap_or_default(),
            commands,
        })
    }

    /// `t`초의 그림(마우스 없이). `args`는 블록 본문이다.
    pub fn draw(
        &self,
        t: f32,
        args: &[(String, String)],
        theme: &Theme,
    ) -> Result<Vec<Shape>, String> {
        self.draw_ui(t, args, theme, &Ui::default())
            .map(|(shapes, _)| shapes)
    }

    /// `t`초의 그림과 영역. `ui`는 마우스 상태다.
    pub fn draw_ui(
        &self,
        t: f32,
        args: &[(String, String)],
        theme: &Theme,
        ui: &Ui,
    ) -> Result<Drawing, String> {
        if !self.draws {
            return Err("this extension has no draw function".into());
        }
        let draw: Function = self.module.get("draw").map_err(message)?;
        let (width, height) = self.size_for(args)?;
        self.recorder.borrow_mut().reset(
            width,
            height,
            theme.foreground,
            if self.animated {
                canvas::ANIMATED_SHAPE_LIMIT
            } else {
                canvas::STILL_SHAPE_LIMIT
            },
        );
        let run = || -> mlua::Result<()> {
            self.g
                .raw_set("theme", canvas::theme_table(&self.lua, theme)?)?;
            let math: Table = self.lua.globals().get("math")?;
            math.get::<Function>("randomseed")?
                .call::<()>(RANDOM_SEED)?;
            let ui_table = self.lua.create_table()?;
            ui_table.raw_set("hover", ui.hover.as_deref())?;
            ui_table.raw_set("pressed", ui.pressed.as_deref())?;
            self.deadline.set(Instant::now() + DRAW_TIMEOUT);
            draw.call::<()>((self.g.clone(), t, self.args_table(args)?, ui_table))
        };
        run().map_err(message)?;
        let mut recorder = self.recorder.borrow_mut();
        Ok((
            std::mem::take(&mut recorder.shapes),
            std::mem::take(&mut recorder.regions),
        ))
    }

    /// 영역 `id`를 눌렀다. `click(id, args)`가 돌려준 것: 인자 표(자기 블록의 인자 변경), 또는
    /// `{ args = …, actions = { … } }`(동작은 앞머리 `actions:`에 적은 것만).
    pub fn click(&self, id: &str, args: &[(String, String)]) -> Result<Clicked, String> {
        if !self.clicks {
            return Ok(Clicked::default());
        }
        let click: Function = self.module.get("click").map_err(message)?;
        let call = || -> mlua::Result<Value> {
            self.deadline.set(Instant::now() + DRAW_TIMEOUT);
            click.call((id, self.args_table(args)?))
        };
        let result = match call().map_err(message)? {
            Value::Nil => return Ok(Clicked::default()),
            Value::Table(result) => result,
            other => {
                return Err(format!(
                    "click must return a table (argument values, or {{ args = ..., actions = ... }}) or nil, got {}",
                    other.type_name()
                ));
            }
        };
        let field = |key: &str| result.raw_get::<Value>(key).unwrap_or(Value::Nil);
        let (own, actions) = (field("args"), field("actions"));
        if own.is_nil() && actions.is_nil() {
            return Ok(Clicked {
                args: arg_changes(&result)?,
                actions: Vec::new(),
            });
        }
        let args = match own {
            Value::Nil => Vec::new(),
            Value::Table(own) => arg_changes(&own)?,
            other => {
                return Err(format!(
                    "click: args must be a table, got {}",
                    other.type_name()
                ));
            }
        };
        let actions = match actions {
            Value::Nil => Vec::new(),
            Value::Table(list) => {
                let mut out = Vec::new();
                for entry in list.sequence_values::<Value>() {
                    let entry = entry.map_err(message)?;
                    let Value::Table(entry) = entry else {
                        return Err("click: each action must be a table like { \"set\", block = ..., args = ... }".into());
                    };
                    let action = parse_action(&entry)?;
                    if !self
                        .actions
                        .iter()
                        .any(|declared| declared == action.name())
                    {
                        return Err(format!(
                            "click: the action \"{}\" is not declared; list it on the header's actions: line",
                            action.name()
                        ));
                    }
                    out.push(action);
                }
                if out.len() > MAX_ACTIONS {
                    return Err(format!("click: at most {MAX_ACTIONS} actions per click"));
                }
                out
            }
            other => {
                return Err(format!(
                    "click: actions must be a list, got {}",
                    other.type_name()
                ));
            }
        };
        Ok(Clicked { args, actions })
    }

    /// 인라인 확장의 글: `value(args, ctx)`가 돌려준 글(숫자는 글로). `value`가 없으면 `None`.
    /// `ctx`는 `{ sheet = 시트 경로 }`다.
    pub fn value(&self, args: &[(String, String)], sheet: &str) -> Result<Option<String>, String> {
        if !self.values {
            return Ok(None);
        }
        let value: Function = self.module.get("value").map_err(message)?;
        let call = || -> mlua::Result<Value> {
            let ctx = self.lua.create_table()?;
            ctx.raw_set("sheet", sheet)?;
            self.deadline.set(Instant::now() + DRAW_TIMEOUT);
            value.call((self.args_table(args)?, ctx))
        };
        match call().map_err(message)? {
            Value::Nil => Ok(Some(String::new())),
            Value::String(text) => Ok(Some(
                text.to_string_lossy()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            )),
            Value::Integer(n) => Ok(Some(n.to_string())),
            Value::Number(n) => Ok(Some(n.to_string())),
            other => Err(format!(
                "value must return a string or number, got {}",
                other.type_name()
            )),
        }
    }

    /// 플레이 창의 캔버스 크기: `play.size`, 없으면 블록 크기(`size`).
    pub fn play_size(&self, args: &[(String, String)]) -> Result<(f32, f32), String> {
        let play: Table = self.module.get("play").map_err(message)?;
        match play.get::<Value>("size").map_err(message)? {
            Value::Nil => self.size_for(args),
            value => canvas_size(value).map_err(|err| format!("play.size: {err}")),
        }
    }

    /// 플레이 콜백 `name`을 부른다(없으면 아무것도 하지 않는다).
    pub fn play_call(&self, name: &str, args: impl mlua::IntoLuaMulti) -> Result<(), String> {
        let play: Table = self.module.get("play").map_err(message)?;
        let Ok(Value::Function(function)) = play.get::<Value>(name) else {
            return Ok(());
        };
        self.deadline.set(Instant::now() + DRAW_TIMEOUT);
        function
            .call::<()>(args)
            .map_err(|err| format!("play.{name}: {}", message(err)))
    }

    /// 플레이를 시작한다: `play.load(args)`.
    pub fn play_load(&self, args: &[(String, String)]) -> Result<(), String> {
        let table = self.args_table(args).map_err(message)?;
        self.play_call("load", table)
    }

    /// 플레이 화면 한 장: `play.draw(g)`. 그 사이의 상태는 확장(Lua)이 들고 있다.
    pub fn play_draw(&self, size: (f32, f32), theme: &Theme) -> Result<Vec<Shape>, String> {
        self.recorder.borrow_mut().reset(
            size.0,
            size.1,
            theme.foreground,
            canvas::ANIMATED_SHAPE_LIMIT,
        );
        self.g
            .raw_set(
                "theme",
                canvas::theme_table(&self.lua, theme).map_err(message)?,
            )
            .map_err(message)?;
        self.play_call("draw", self.g.clone())?;
        let mut recorder = self.recorder.borrow_mut();
        recorder.regions.clear();
        Ok(std::mem::take(&mut recorder.shapes))
    }

    /// 블록 인자 `args`일 때의 캔버스 크기.
    pub fn size_for(&self, args: &[(String, String)]) -> Result<(f32, f32), String> {
        let Some(size_fn) = &self.size_fn else {
            return Ok(self.size.unwrap_or(DEFAULT_SIZE));
        };
        let call = || -> mlua::Result<Value> {
            self.deadline.set(Instant::now() + DRAW_TIMEOUT);
            size_fn.call(self.args_table(args)?)
        };
        canvas_size(call().map_err(message)?).map_err(|err| format!("size(args): {err}"))
    }

    /// 명령 `index`를 부른다. 넣을 글을 돌려준다(`nil`이면 `None`).
    pub fn run_command(
        &self,
        index: usize,
        ctx: &CommandContext,
    ) -> Result<Option<String>, String> {
        let commands: Table = self.module.get("commands").map_err(message)?;
        let command: Table = commands.get(index + 1).map_err(message)?;
        let run: Function = command.get("run").map_err(message)?;
        let call = || -> mlua::Result<Value> {
            let table = self.lua.create_table()?;
            table.raw_set("selection", ctx.selection.as_str())?;
            table.raw_set("text", ctx.text.as_str())?;
            table.raw_set("sheet", ctx.sheet.as_str())?;
            self.deadline.set(Instant::now() + COMMAND_TIMEOUT);
            run.call(table)
        };
        match call().map_err(message)? {
            Value::Nil => Ok(None),
            Value::String(text) => Ok(Some(text.to_string_lossy())),
            Value::Integer(n) => Ok(Some(n.to_string())),
            Value::Number(n) => Ok(Some(n.to_string())),
            other => Err(format!(
                "run must return a string or nil, got {}",
                other.type_name()
            )),
        }
    }

    /// 블록 본문을 Lua 표로. 숫자·`true`·`false`로 읽히면 그 값으로 넘긴다.
    fn args_table(&self, args: &[(String, String)]) -> mlua::Result<Table> {
        let table = self.lua.create_table()?;
        for (key, value) in args {
            let value = if let Ok(n) = value.parse::<f64>() {
                Value::Number(n)
            } else if value == "true" || value == "false" {
                Value::Boolean(value == "true")
            } else {
                Value::String(self.lua.create_string(value)?)
            };
            table.raw_set(key.as_str(), value)?;
        }
        Ok(table)
    }
}

/// `click`이 돌려준 인자 표 `{ key = value }`를 블록에 쓸 (키, 값)으로. 값은 한 줄 글이다.
fn arg_changes(changes: &Table) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for pair in changes.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(message)?;
        let Value::String(key) = key else {
            return Err("click: argument names must be strings".into());
        };
        let key = key.to_string_lossy();
        if key.trim().is_empty() || key.contains([':', '\n']) || key.trim() != key {
            return Err(format!("click: invalid argument name \"{key}\""));
        }
        let value = match value {
            Value::String(text) => text.to_string_lossy(),
            Value::Boolean(flag) => flag.to_string(),
            Value::Integer(n) => n.to_string(),
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", n as i64),
            Value::Number(n) => n.to_string(),
            other => {
                return Err(format!(
                    "click: the value of {key} must be a string, number, or boolean, got {}",
                    other.type_name()
                ));
            }
        };
        if value.contains('\n') {
            return Err(format!("click: the value of {key} must be one line"));
        }
        out.push((key, value.trim().to_string()));
    }
    out.sort();
    Ok(out)
}

/// 동작 하나(`{ "set", block = …, args = … }` 꼴).
fn parse_action(entry: &Table) -> Result<Action, String> {
    let name: String = entry
        .raw_get::<Option<String>>(1)
        .ok()
        .flatten()
        .ok_or("click: an action needs its name first, like { \"set\", block = \"chart\", args = { ... } }")?;
    let text = |key: &str| -> Result<String, String> {
        match entry.raw_get::<Value>(key).unwrap_or(Value::Nil) {
            Value::String(text) => Ok(text.to_string_lossy()),
            Value::Integer(n) => Ok(n.to_string()),
            Value::Number(n) => Ok(n.to_string()),
            _ => Err(format!("click: the {name} action needs {key} (a string)")),
        }
    };
    match name.as_str() {
        "set" => {
            let block = text("block")?;
            let Value::Table(args) = entry.raw_get::<Value>("args").unwrap_or(Value::Nil) else {
                return Err("click: the set action needs args (a table)".into());
            };
            Ok(Action::Set {
                block,
                args: arg_changes(&args)?,
            })
        }
        "command" => {
            let extension = text("extension")?;
            if !valid_name(&extension) {
                return Err(format!("click: invalid extension name \"{extension}\""));
            }
            let command = match (
                entry.raw_get::<Value>("index").unwrap_or(Value::Nil),
                entry.raw_get::<Value>("title").unwrap_or(Value::Nil),
            ) {
                (Value::Integer(n), _) if n >= 1 => CommandRef::Index(n as usize - 1),
                (Value::Number(n), _) if n >= 1.0 && n.fract() == 0.0 => {
                    CommandRef::Index(n as usize - 1)
                }
                (_, Value::String(title)) => CommandRef::Title(title.to_string_lossy()),
                _ => return Err("click: the command action needs index (from 1) or title".into()),
            };
            Ok(Action::Command { extension, command })
        }
        "editor.insert" => Ok(Action::Insert(text("text")?)),
        "editor.replaceSelection" => Ok(Action::ReplaceSelection(text("text")?)),
        "open" => Ok(Action::Open(text("sheet")?)),
        "menu" => {
            let menu = text("action")?;
            if !MENU_ACTIONS.contains(&menu.as_str()) {
                return Err(format!(
                    "click: unknown menu action \"{menu}\" (available: {})",
                    MENU_ACTIONS.join(", ")
                ));
            }
            Ok(Action::Menu(menu))
        }
        "play" => match entry.raw_get::<Value>("args").unwrap_or(Value::Nil) {
            Value::Nil => Ok(Action::Play(Vec::new())),
            Value::Table(args) => Ok(Action::Play(arg_changes(&args)?)),
            other => Err(format!(
                "click: play args must be a table, got {}",
                other.type_name()
            )),
        },
        other => Err(format!(
            "click: unknown action \"{other}\" (available: {})",
            ACTION_NAMES.join(", ")
        )),
    }
}

fn command_titles(module: &Table) -> Result<Vec<String>, String> {
    let commands = match module.get::<Value>("commands").map_err(message)? {
        Value::Nil => return Ok(Vec::new()),
        Value::Table(commands) => commands,
        other => {
            return Err(format!(
                "commands must be a list of {{ title = ..., run = function(ctx) ... end }}, got {}",
                other.type_name()
            ));
        }
    };
    commands
        .sequence_values::<Value>()
        .enumerate()
        .map(|(ix, command)| {
            let Ok(Value::Table(command)) = command else {
                return Err(format!("commands[{}] must be a table", ix + 1));
            };
            let title: Option<String> = command.get("title").ok();
            let title = title
                .filter(|title| !title.trim().is_empty())
                .ok_or_else(|| format!("commands[{}] needs a title string", ix + 1))?;
            if !matches!(command.get::<Value>("run"), Ok(Value::Function(_))) {
                return Err(format!("commands[{}] needs a run function", ix + 1));
            }
            Ok(title)
        })
        .collect()
}

/// `size` 값(또는 `size(args)`가 돌려준 값): `{ width, height }`. 없으면 기본 크기.
fn canvas_size(value: Value) -> Result<(f32, f32), String> {
    let size = match value {
        Value::Nil => return Ok(DEFAULT_SIZE),
        Value::Table(size) => size,
        other => {
            return Err(format!(
                "size must be {{ width, height }} or function(args) returning it, got {}",
                other.type_name()
            ));
        }
    };
    let side = |ix: usize| -> Result<f32, String> {
        let value: f32 = size
            .get::<Option<f32>>(ix)
            .ok()
            .flatten()
            .ok_or("size must be { width, height } (two numbers)")?;
        if !(1.0..=MAX_SIDE).contains(&value) {
            return Err(format!("size values must be between 1 and {MAX_SIDE}"));
        }
        Ok(value)
    };
    Ok((side(1)?, side(2)?))
}

/// 작업 스레드에 띄운 확장(또는 띄우지 못한 까닭).
pub type Loaded = Result<Worker, String>;

/// 열린 시트(또는 대화상자)의 확장 캐시: 확장마다 작업 스레드 하나. 파일 수정 시각이 바뀌면 새로 띄우고
/// 옛 스레드는 요청 통로가 끊겨 끝난다. UI 스레드에서만 쓴다.
pub struct Host {
    root: PathBuf,
    /// 이름 → (띄울 때의 파일 수정 시각, 띄운 확장).
    cache: RefCell<HashMap<String, Cached>>,
}

type Cached = (Option<SystemTime>, Rc<Loaded>);

impl Host {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache: RefCell::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 확장 `name`. 처음이거나 파일(폴더면 안의 파일)이 바뀌었으면 띄운다(읽기가 끝날 때까지 기다린다).
    pub fn get(&self, name: &str) -> Rc<Loaded> {
        let modified = package::modified(&self.root, name);
        if let Some((cached, loaded)) = self.cache.borrow().get(name)
            && *cached == modified
        {
            return loaded.clone();
        }
        let loaded =
            Rc::new(read_source(&self.root, name).and_then(|source| Worker::spawn(name, source)));
        self.cache
            .borrow_mut()
            .insert(name.to_string(), (modified, loaded.clone()));
        loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ext_fences_with_args() {
        let text = "앞\n```ext archer\npower: 3\ntarget: 오크\n```\n```rust\n```ext inner\n```\n뒤";
        assert_eq!(
            blocks(text),
            [Block {
                rows: 1..5,
                name: "archer".into(),
                args: vec![
                    ("power".into(), "3".into()),
                    ("target".into(), "오크".into())
                ],
            }]
        );
        // 닫지 않은 펜스, 이름이 없거나 경로 문자가 든 이름은 블록이 아니다.
        assert!(blocks("```ext archer\npower: 3").is_empty());
        assert!(blocks("```ext\n```").is_empty());
        assert!(blocks("```ext ../x\n```").is_empty());
        // 물결 펜스와 긴 펜스도 된다.
        assert_eq!(blocks("~~~~ ext 별\n~~~~").len(), 1);
    }

    #[test]
    fn reads_meta_without_running_lua() {
        let source = "\n--[[---\nname: other\ndescription: 궁수가 쏜다.\nargs:\n  speed: 화살 속도. 기본값 1.\n  label: 글\nexample:\n  speed: 1.5\nunknown: x\n---]]\nreturn {}";
        assert_eq!(
            parse_meta("archer", source),
            Some(Meta {
                name: "archer".into(),
                description: "궁수가 쏜다.".into(),
                args: vec![
                    ("speed".into(), "화살 속도. 기본값 1.".into()),
                    ("label".into(), "글".into())
                ],
                example: vec![("speed".into(), "1.5".into())],
                actions: Vec::new(),
            })
        );
        // 앞머리가 맨 위에 없거나 닫히지 않으면 없는 것이다.
        assert_eq!(
            parse_meta("x", "return {}\n--[[---\ndescription: d\n---]]"),
            None
        );
        assert_eq!(parse_meta("x", "--[[---\ndescription: d\n"), None);
        // 앞머리는 Lua 주석이라 실행에 영향이 없다.
        assert!(Extension::load("archer", source).is_err_and(|err| err.contains("none of draw")));
        // 넣은 블록은 다시 블록으로 읽힌다.
        let text = block_text("archer", &[("speed".into(), "1.5".into())]);
        assert_eq!(
            blocks(&text)[0].args,
            [("speed".to_string(), "1.5".to_string())]
        );
    }

    const ARCHER: &str = r#"
return {
  size = { 200, 100 },
  animated = true,
  draw = function(g, t, args)
    g.setColor(1, 0, 0)
    g.circle("fill", 10, 10, 5)
    g.push()
    g.translate(100, 0)
    g.rectangle("line", 0, 0, 10 * args.power, 10)
    g.pop()
    g.print(args.label, 0, 50)
  end,
  commands = {
    { title = "Upper", run = function(ctx) return string.upper(ctx.selection) end },
    { title = "Nothing", run = function(ctx) end },
  },
}
"#;

    #[test]
    fn draws_gif_frames_by_time() {
        use ::image::codecs::gif::{GifEncoder, Repeat};
        let root = std::env::temp_dir().join(format!("hanji-gif-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Notes")).unwrap();
        std::fs::create_dir_all(root.join(EXTENSIONS_DIR)).unwrap();
        // 빨강(0.1초) → 초록(0.2초) → 파랑(0.3초), 무한 반복.
        let mut gif = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut gif);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for (color, ms) in [
                ([255, 0, 0, 255], 100),
                ([0, 255, 0, 255], 200),
                ([0, 0, 255, 255], 300),
            ] {
                let frame = ::image::Frame::from_parts(
                    ::image::RgbaImage::from_pixel(2, 2, ::image::Rgba(color)),
                    0,
                    0,
                    ::image::Delay::from_numer_denom_ms(ms, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        std::fs::write(root.join("Notes/spin.gif"), gif).unwrap();
        std::fs::write(
            root.join(EXTENSIONS_DIR).join("spin.lua"),
            r#"
local gif = g.newImage("Notes/spin.gif")
return {
  size = { 2, 2 },
  draw = function(g, t, args)
    if args.n then
      g.draw(gif:frame(tonumber(args.n)), 0, 0)
    else
      g.draw(gif:frameAt(t), 0, 0)
    end
    g.print(gif:getFrameCount() .. " " .. gif:getDuration(), 9, 9)
  end,
}"#,
        )
        .unwrap();
        let extension = Extension::open(&root, "spin").unwrap();
        let color = |t: f32, args: &[(String, String)]| {
            let shapes = extension.draw(t, args, &Theme::default()).unwrap();
            let pixmap = render::Renderer::new().render(&shapes, 1.0, 2, 2, None);
            let p = pixmap.sample(1, 1);
            ([p.r, p.g, p.b], shapes)
        };
        let (first, shapes) = color(0.0, &[]);
        assert_eq!(first, [255, 0, 0]);
        assert!(matches!(&shapes[1], Shape::Text { text, .. } if text == "3 0.6"));
        assert_eq!(color(0.15, &[]).0, [0, 255, 0]);
        assert_eq!(color(0.45, &[]).0, [0, 0, 255]);
        // 반복하고, 뒤로 가도 처음부터 다시 푼다.
        assert_eq!(color(0.65, &[]).0, [255, 0, 0]);
        assert_eq!(color(0.25, &[]).0, [0, 255, 0]);
        assert_eq!(color(0.0, &[("n".into(), "3".into())]).0, [0, 0, 255]);
        let err = extension
            .draw(0.0, &[("n".into(), "4".into())], &Theme::default())
            .unwrap_err();
        assert!(err.contains("frame must be 1 to 3"), "{err}");
        // 내보내기는 그 프레임을 PNG로 넣는다(움직이는 GIF 원본이 아니다).
        let still = Still {
            canvas: (2., 2.),
            scale: 1.,
            align: Align::Center,
            shapes: std::sync::Arc::new(color(0.15, &[]).1),
        };
        assert!(svg::to_svg(&still).contains("data:image/png;base64,"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn draws_library_images_with_quads() {
        let root = std::env::temp_dir().join(format!("hanji-images-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Notes/Attachments")).unwrap();
        std::fs::create_dir_all(root.join(EXTENSIONS_DIR)).unwrap();
        // 왼쪽 반은 빨강, 오른쪽 반은 파랑인 4×2 그림.
        let mut pixels = ::image::RgbaImage::new(4, 2);
        for (x, _, pixel) in pixels.enumerate_pixels_mut() {
            *pixel = if x < 2 {
                ::image::Rgba([255, 0, 0, 255])
            } else {
                ::image::Rgba([0, 0, 255, 255])
            };
        }
        pixels.save(root.join("Notes/Attachments/pic.png")).unwrap();
        std::fs::write(
            root.join(EXTENSIONS_DIR).join("frame.lua"),
            r#"
local pic = g.newImage("Notes/Attachments/pic.png")
pic:setFilter("nearest")
local right = g.newQuad(2, 0, 2, 2, pic:getDimensions())
return {
  size = { 40, 20 },
  draw = function(g, t, args)
    g.draw(pic, 0, 0, 0, 5)
    g.draw(pic, right, 40, 0, math.pi / 2, 5)
    local w, h = pic:getDimensions()
    g.print(w .. "x" .. h, 100, 100)
    if args.missing then g.newImage(args.missing) end
  end,
}"#,
        )
        .unwrap();
        let extension = Extension::open(&root, "frame").unwrap();
        let shapes = extension.draw(0.0, &[], &Theme::default()).unwrap();
        assert!(matches!(&shapes[2], Shape::Text { text, .. } if text == "4x2"));
        let pixmap = render::Renderer::new().render(&shapes, 1.0, 40, 20, None);
        let at = |x, y| {
            let p = pixmap.sample(x, y);
            [p.r, p.g, p.b, p.a]
        };
        // 5배: 0..10은 빨강, 10..20은 파랑. 오른쪽 칸(파랑)을 90도 돌려 x 30..40, y 0..10에 둔다.
        assert_eq!(at(5, 5), [255, 0, 0, 255]);
        assert_eq!(at(15, 5), [0, 0, 255, 255]);
        assert_eq!(at(35, 5), [0, 0, 255, 255]);
        assert_eq!(at(25, 15)[3], 0);
        // 내보내기: SVG는 원래 파일을 data URI로, PDF는 그림으로 넣는다.
        let still = Still {
            canvas: (40., 20.),
            scale: 1.,
            align: Align::Center,
            shapes: std::sync::Arc::new(shapes.clone()),
        };
        let svg = svg::to_svg(&still);
        assert!(
            svg.contains(r#"image-rendering="pixelated" href="data:image/png;base64,"#),
            "{svg}"
        );
        let document = crate::export::pdf::PdfDocument {
            title: "그림".into(),
            authors: Vec::new(),
            attachments: Vec::new(),
            pages: vec![crate::export::pdf::PdfPage {
                width: 100.,
                height: 100.,
                items: vec![crate::export::pdf::PdfItem::Drawing {
                    x: 0.,
                    y: 0.,
                    width: 40.,
                    height: 20.,
                    canvas: (40., 20.),
                    shapes: std::sync::Arc::new(shapes.clone()),
                }],
            }],
        };
        let pdf = crate::export::pdf::render(&document).unwrap();
        assert!(pdf.windows(6).any(|w| w == b"/Image"));
        // 라이브러리 밖이나 없는 그림은 알린다.
        for (missing, expected) in [
            ("../x.png", "without \"..\""),
            ("Notes/none.png", "not found"),
        ] {
            let err = extension
                .draw(
                    0.0,
                    &[("missing".into(), missing.into())],
                    &Theme::default(),
                )
                .unwrap_err();
            assert!(err.contains(expected), "{err}");
        }
        // 스킬의 예제(photo-frame)는 모든 모양새로 그리고, src가 없으면 안내만 그린다.
        let example =
            include_str!("../../resources/skills/extension/references/examples/photo-frame.md");
        let source = example
            .split("```lua\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap();
        std::fs::write(root.join(EXTENSIONS_DIR).join("photo-frame.lua"), source).unwrap();
        std::fs::copy(
            root.join("Notes/Attachments/pic.png"),
            root.join("Notes/Attachments/beach.jpg"),
        )
        .unwrap();
        let frame = Extension::open(&root, "photo-frame").unwrap();
        for style in ["polaroid", "round", "circle"] {
            let args = [
                ("src".to_string(), "Notes/Attachments/beach.jpg".to_string()),
                ("caption".to_string(), "여름".to_string()),
                ("style".to_string(), style.to_string()),
            ];
            let shapes = frame.draw(0.0, &args, &Theme::default()).unwrap();
            assert!(
                shapes
                    .iter()
                    .any(|shape| matches!(shape, Shape::Image { .. })),
                "{style}"
            );
        }
        let hint = frame.draw(0.0, &[], &Theme::default()).unwrap();
        assert!(matches!(&hint[..], [Shape::Text { .. }]), "{hint:?}");
        // 라이브러리에서 읽지 않은 확장은 그림을 못 읽는다.
        let err = Extension::load("x", "local p = g.newImage('a.png') return {}").unwrap_err();
        assert!(err.contains("only be read from a library"), "{err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn effects_and_rotated_text() {
        let source = r#"
return {
  size = { 100, 100 },
  draw = function(g)
    g.scale(2)
    g.setShadow({ 0, 0, 0, 0.5 }, 3, 1, 2)
    g.setBlendMode("multiply")
    g.rectangle("fill", 0, 0, 10, 10)
    g.setShadow()
    g.setBlendMode("alpha")
    g.setBlendMode("alpha")
    g.rotate(math.pi / 2)
    g.print("hi", 0, 0)
    g.setBlur(2)
    g.setBlendMode(g.getBlendMode())
  end,
}"#;
        let extension = Extension::load("fx", source).unwrap();
        let shapes = extension.draw(0.0, &[], &Theme::default()).unwrap();
        let effects: Vec<&canvas::Effects> = shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Effects(effects) => Some(effects),
                _ => None,
            })
            .collect();
        // 바뀔 때만 넣는다(같은 섞기를 두 번 골라도 하나).
        assert_eq!(effects.len(), 5, "{shapes:?}");
        let shadow = effects[0].shadow.unwrap();
        // 그림자는 그때의 변환 배율을 따른다.
        assert_eq!((shadow.blur, shadow.offset), (6.0, [2.0, 4.0]));
        assert_eq!(effects[1].blend, Blend::Multiply);
        assert!(effects[1].shadow.is_some());
        assert_eq!(*effects[3], canvas::Effects::default());
        assert_eq!(effects[4].blur, 4.0);
        let angle = shapes.iter().find_map(|shape| match shape {
            Shape::Text { angle, .. } => Some(*angle),
            _ => None,
        });
        assert!((angle.unwrap() - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let err = Extension::load(
            "bad",
            "return { size = {1, 1}, draw = function(g) g.setBlendMode('subtract') end }",
        )
        .unwrap()
        .draw(0.0, &[], &Theme::default())
        .unwrap_err();
        assert!(err.contains("\"multiply\""), "{err}");
    }

    #[test]
    fn draws_shapes_in_canvas_space() {
        let extension = Extension::load("archer", ARCHER).unwrap();
        assert_eq!(extension.size, Some((200.0, 100.0)));
        assert!(extension.animated && extension.draws);
        assert_eq!(extension.commands, ["Upper", "Nothing"]);
        let args = [
            ("power".into(), "2".into()),
            ("label".into(), "명중".into()),
        ];
        let shapes = extension.draw(0.5, &args, &Theme::default()).unwrap();
        assert_eq!(shapes.len(), 3);
        assert!(
            matches!(&shapes[0], Shape::Fill { paint, .. } if paint.color() == [1.0, 0.0, 0.0, 1.0])
        );
        let Shape::Stroke { path, .. } = &shapes[1] else {
            panic!("{:?}", shapes[1]);
        };
        assert_eq!(path[0], Segment::Move([100.0, 0.0]));
        assert_eq!(path[1], Segment::Line([120.0, 0.0]));
        assert!(matches!(&shapes[2], Shape::Text { text, .. } if text == "명중"));
        // 다시 그리면 변환과 모양이 처음부터다.
        assert_eq!(
            extension.draw(0.5, &args, &Theme::default()).unwrap(),
            shapes
        );
    }

    const VECTOR: &str = r##"
local leaf = g.newPath():moveTo(0, 0):curveTo(10, -10, 30, -10, 40, 0):quadTo(20, 10, 0, 0):close()
local open = g.newPath("M0 0 C 0 20 40 20 40 0 Z")
local shut = g.newPath("M0 0 C 0 0 40 0 40 0 Z")
local road = g.newPath("M0 50 L 100 50")
return {
  size = {100, 100},
  draw = function(g, t)
    g.setColor(g.newLinearGradient(0, 0, 100, 0, {{0, "#ff0000"}, {0.5, "#00ff00"}, {1, "#0000ff"}}))
    g.push()
    g.translate(10, 10)
    g.fill(leaf)
    g.pop()
    g.setColor(g.newRadialGradient(50, 50, 30, {"#ffffff", {1, 1, 1, 0}}))
    g.circle("fill", 50, 50, 30)
    g.setColor(1, 0, 0)
    g.setLineJoin("round"); g.setLineCap("round"); g.setLineWidth(4)
    g.setDash({10, 10}, 5)
    g.stroke(road)
    g.setDash()
    g.setScissor(0, 0, 50, 100)
    g.fill(g.lerpPath(open, shut, g.ease.inOutSine(0.5)))
    g.setScissor()
    local x, y, angle = road:pointAt(0.25)
    g.stroke(road:trim(0, 0.5):translate(0, 10))
    g.print(string.format("%d %d %.2f %d", x, y, angle, road:length()), 0, 0)
  end,
}
"##;

    #[test]
    fn draws_vector_paths_and_gradients() {
        let extension = Extension::load("vector", VECTOR).unwrap();
        let shapes = extension.draw(0.0, &[], &Theme::default()).unwrap();
        let Shape::Fill { path, paint, .. } = &shapes[0] else {
            panic!("{:?}", shapes[0]);
        };
        // 변환을 따라 옮겨지고, 2차 곡선은 3차로 바뀐다.
        assert_eq!(path[0], Segment::Move([10.0, 10.0]));
        assert!(matches!(path[2], Segment::Cubic(..)));
        assert!(matches!(paint, Paint::Linear { stops, .. } if stops.len() == 3));
        assert!(
            matches!(&shapes[1], Shape::Fill { paint: Paint::Radial { radius, .. }, .. } if *radius == 30.0)
        );
        // 점선: 길이 100을 10/10 무늬로 5만큼 밀면 조각 여섯.
        let Shape::Stroke { path, style, .. } = &shapes[2] else {
            panic!("{:?}", shapes[2]);
        };
        assert_eq!(style.join, LineJoin::Round);
        assert_eq!(style.cap, LineCap::Round);
        assert_eq!(
            path.iter()
                .filter(|s| matches!(s, Segment::Move(_)))
                .count(),
            6
        );
        assert_eq!(shapes[3], Shape::Clip(Some([0.0, 0.0, 50.0, 100.0])));
        // 모핑: 가운데는 높이 10의 곡선.
        let Shape::Fill { path, .. } = &shapes[4] else {
            panic!("{:?}", shapes[4]);
        };
        assert_eq!(
            path[1],
            Segment::Cubic([0.0, 10.0], [40.0, 10.0], [40.0, 0.0])
        );
        assert_eq!(shapes[5], Shape::Clip(None));
        let Shape::Stroke { path, .. } = &shapes[6] else {
            panic!("{:?}", shapes[6]);
        };
        assert_eq!(path.last(), Some(&Segment::Line([50.0, 60.0])));
        assert!(matches!(&shapes[7], Shape::Text { text, .. } if text == "25 50 0.00 100"));
    }

    #[test]
    fn clips_and_combines_paths() {
        let source = r##"
local A = g.newPath():rect(0, 0, 10, 10)
local B = g.newPath():rect(5, 0, 10, 10)
return {
  size = { 100, 100 },
  draw = function(g)
    g.translate(20, 0)
    g.setClip(g.newPath():circle(5, 5, 5))
    g.rectangle("fill", 0, 0, 10, 10)
    g.setClip()
    g.origin()
    g.fill(A:union(B))
    g.fill(A:subtract(B))
    g.fill(g.newPath("M0 0 L 50 0"):outline(4, "round", "round"))
    g.fill(A:offset(2))
    local x, y, w, h = A:intersect(B):bounds()
    g.print(string.format("%g %g %g %g", x, y, w, h), 0, 0)
  end,
}
"##;
        let extension = Extension::load("clip", source).unwrap();
        let shapes = extension.draw(0.0, &[], &Theme::default()).unwrap();
        // 자르는 경로도 변환을 따른다.
        let Shape::ClipPath(Some((path, FillRule::NonZero))) = &shapes[0] else {
            panic!("{:?}", shapes[0]);
        };
        assert_eq!(path[0], Segment::Move([30.0, 5.0]));
        assert_eq!(shapes[2], Shape::ClipPath(None));
        let points = |ix: usize| match &shapes[ix] {
            Shape::Fill { path, .. } => path.len(),
            other => panic!("{other:?}"),
        };
        assert!(points(3) >= 5 && points(4) >= 5);
        assert!(points(5) > 10);
        assert!(
            matches!(&shapes[7], Shape::Text { text, .. } if text == "5 0 5 10"),
            "{:?}",
            shapes[7]
        );
    }

    #[test]
    fn vector_errors_name_what_is_wrong() {
        let draw = |body: &str| {
            let source = format!("return {{ size = {{10, 10}}, draw = function(g) {body} end }}");
            Extension::load("bad", &source)
                .unwrap()
                .draw(0.0, &[], &Theme::default())
                .unwrap_err()
        };
        assert!(draw("g.fill(g.newPath('M0 0 X'))").contains("unknown path command"));
        assert!(draw("g.newPath():bend()").contains("path:bend does not exist"));
        assert!(
            draw("g.lerpPath(g.newPath('M0 0 L1 1'), g.newPath('M0 0 C1 1 1 1 1 1'), 0.5)")
                .contains("same structure")
        );
        assert!(draw("g.setLineJoin('sharp')").contains("line join"));
        assert!(draw("g.newLinearGradient(0, 0, 1, 1, {})").contains("at least one color stop"));
        assert!(draw("g.fill(3)").contains("must be a path"));
        assert!(draw("g.newPath('M0 0 L1 0'):outline(0)").contains("more than 0"));
        assert!(draw("g.setClip('M0 0 L1 0 L1 1 Z', 'odd')").contains("fill rule"));
    }

    #[test]
    fn runs_commands() {
        let extension = Extension::load("archer", ARCHER).unwrap();
        let ctx = CommandContext {
            selection: "abc".into(),
            ..CommandContext::default()
        };
        assert_eq!(extension.run_command(0, &ctx), Ok(Some("ABC".into())));
        assert_eq!(extension.run_command(1, &ctx), Ok(None));
    }

    #[test]
    fn reports_errors_the_model_can_fix() {
        let err = Extension::load("x", "return 1").unwrap_err();
        assert!(err.contains("must return a table"), "{err}");
        let err = Extension::load("x", "return {}").unwrap_err();
        assert!(
            err.contains("none of draw, value, play, or commands"),
            "{err}"
        );
        let err = Extension::load("x", "return { draw = function(g) g.fooBar() end }")
            .unwrap()
            .draw(0.0, &[], &Theme::default())
            .unwrap_err();
        assert!(err.contains("g.fooBar does not exist"), "{err}");
        let err = Extension::load("x", "return { draw = function(g) g.circle('fill', 1) end }")
            .unwrap()
            .draw(0.0, &[], &Theme::default())
            .unwrap_err();
        assert!(
            err.contains("g.circle: argument #3 must be a number"),
            "{err}"
        );
    }

    #[test]
    fn sandbox_blocks_files_and_endless_loops() {
        for source in [
            "return { draw = function(g) io.open('x') end }",
            "return { draw = function(g) os.execute('ls') end }",
            "return { draw = function(g) require('x') end }",
        ] {
            let extension = Extension::load("x", source).unwrap();
            assert!(
                extension.draw(0.0, &[], &Theme::default()).is_err(),
                "{source}"
            );
        }
        let extension =
            Extension::load("x", "return { draw = function(g) while true do end end }").unwrap();
        let err = extension.draw(0.0, &[], &Theme::default()).unwrap_err();
        assert!(err.contains("took too long"), "{err}");
        // 시간 함수는 쓸 수 있다.
        let extension = Extension::load(
            "x",
            "return { commands = { { title = 'd', run = function() return os.date('%Y') end } } }",
        )
        .unwrap();
        assert_eq!(
            extension
                .run_command(0, &CommandContext::default())
                .unwrap()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn random_is_the_same_every_frame() {
        let extension = Extension::load(
            "x",
            "return { draw = function(g) g.circle('fill', math.random() * 100, 0, 1) end }",
        )
        .unwrap();
        let theme = Theme::default();
        assert_eq!(
            extension.draw(0.0, &[], &theme).unwrap(),
            extension.draw(1.0, &[], &theme).unwrap()
        );
    }

    #[test]
    fn scale_is_read_by_the_app() {
        let block = &blocks("```ext dot\nr: 3\nscale: 2\n```")[0];
        assert_eq!(block.scale(), 2.0);
        assert_eq!(block.extension_args(), [("r".to_string(), "3".to_string())]);
        assert_eq!(scale_of(&[("scale".into(), "x".into())]), 1.0);
        assert_eq!(scale_of(&[("scale".into(), "100".into())]), 4.0);
        assert_eq!(scale_of(&[]), 1.0);
    }

    #[test]
    fn size_can_follow_block_args() {
        let extension = Extension::load(
            "x",
            "return { size = function(args) return { 100 * (args.scale or 1), 50 } end, \
             draw = function(g) g.rectangle('fill', 0, 0, g.getWidth(), g.getHeight()) end }",
        )
        .unwrap();
        assert_eq!(extension.size, None);
        let args = [("scale".to_string(), "2".to_string())];
        assert_eq!(extension.size_for(&args), Ok((200.0, 50.0)));
        assert_eq!(extension.size_for(&[]), Ok((100.0, 50.0)));
        // 캔버스(g.getWidth)도 그 크기다.
        let shapes = extension.draw(0.0, &args, &Theme::default()).unwrap();
        let Shape::Fill { path, .. } = &shapes[0] else {
            panic!()
        };
        assert!(path.contains(&Segment::Line([200.0, 0.0])));
        let bad = Extension::load(
            "x",
            "return { size = function() return { 0, 1 } end, draw = function() end }",
        )
        .unwrap();
        assert!(bad.size_for(&[]).unwrap_err().contains("between 1 and"));
    }

    #[test]
    fn finds_inline_calls_outside_code() {
        let line = "오늘은 {{today}}, 진행 {{progress value=0.6 label=\"반 넘음\"}} `{{code}}` {{ 3 }} {{bad name}}";
        let calls = inline_calls(line);
        let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
        assert_eq!(names, ["today", "progress", "bad"]);
        assert_eq!(&line[calls[0].range.clone()], "{{today}}");
        assert_eq!(
            calls[1].args,
            [
                ("value".to_string(), "0.6".to_string()),
                ("label".to_string(), "반 넘음".to_string())
            ]
        );
        // `name`만 낱말이고 `=`가 없는 낱말은 인자가 아니다.
        assert!(calls[2].args.is_empty());
        assert!(inline_calls("{{ 열고 닫지 않음").is_empty());
        // 원문으로 되돌리면 같은 인자로 읽힌다.
        let text = inline_text("progress", &calls[1].args);
        assert_eq!(text, "{{progress value=0.6 label=\"반 넘음\"}}");
        assert_eq!(inline_calls(&text)[0].args, calls[1].args);
        let tricky = vec![("q".to_string(), "say \"hi\" }".to_string())];
        assert_eq!(inline_calls(&inline_text("x", &tricky))[0].args, tricky);
    }

    #[test]
    fn folder_extensions_require_their_modules() {
        let root = std::env::temp_dir().join(format!("hanji-package-{}", std::process::id()));
        let dir = root.join(EXTENSIONS_DIR).join("game");
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(
            dir.join("init.lua"),
            "local hud = require('ui.hud')\nlocal again = require('ui.hud')\n\
             return { value = function() return hud.label .. tostring(hud == again) end }",
        )
        .unwrap();
        std::fs::write(dir.join("ui/hud.lua"), "return { label = 'hp ' }").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a module").unwrap();
        assert!(scan(&root).iter().any(|(name, _)| name == "game"));
        assert_eq!(path(&root, "game"), dir.join("init.lua"));
        // 모듈은 한 번만 돈다.
        let extension = Extension::open(&root, "game").unwrap();
        assert_eq!(
            extension.value(&[], "a.md").unwrap().as_deref(),
            Some("hp true")
        );
        // 모듈을 고치면 확장이 바뀐 것으로 본다.
        let before = package::modified(&root, "game");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("ui/hud.lua"), "return { label = 'mp ' }").unwrap();
        assert_ne!(package::modified(&root, "game"), before);
        // 없는 모듈과 서로 부르는 모듈은 알린다.
        let missing = Package::from_files([("init.lua", "require('nope') return {}")]).unwrap();
        let err = Extension::load_package("x", &missing).unwrap_err();
        assert!(err.contains("no module \"nope\""), "{err}");
        let circular = Package::from_files([
            ("init.lua", "require('a') return {}"),
            ("a.lua", "require('b') return {}"),
            ("b.lua", "require('a') return {}"),
        ])
        .unwrap();
        let err = Extension::load_package("x", &circular).unwrap_err();
        assert!(err.contains("circular require: a -> b -> a"), "{err}");
        // 오류는 모듈 파일을 가리킨다.
        let broken = Package::from_files([
            ("init.lua", "require('a') return {}"),
            ("a.lua", "error('boom')"),
        ])
        .unwrap();
        let err = Extension::load_package("x", &broken).unwrap_err();
        assert!(err.contains("x/a.lua"), "{err}");
        // 파일 하나 확장에는 모듈이 없다.
        let err = Extension::load("solo", "require('a') return {}").unwrap_err();
        assert!(err.contains("this extension is one file"), "{err}");
        // 같은 이름의 파일과 폴더가 함께 있으면 읽지 않는다.
        std::fs::write(root.join(EXTENSIONS_DIR).join("game.lua"), "return {}").unwrap();
        assert!(
            Extension::open(&root, "game")
                .unwrap_err()
                .contains("keep one")
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn exports_inline_values_as_text() {
        let root = std::env::temp_dir().join(format!("hanji-inline-{}", std::process::id()));
        std::fs::create_dir_all(root.join(EXTENSIONS_DIR)).unwrap();
        std::fs::write(
            path(&root, "greet"),
            "return { value = function(args, ctx) return 'hi ' .. (args.who or ctx.sheet) end }",
        )
        .unwrap();
        std::fs::write(
            path(&root, "secret"),
            "return { export = false, value = function() return 'x' end }",
        )
        .unwrap();
        std::fs::write(path(&root, "dot"), "return { draw = function(g) end }").unwrap();
        let mut blocks = crate::markdown::manuscript_blocks(
            "# {{greet who=책}}\n\n앞 {{greet}} {{secret}}{{dot}} {{missing}} `{{greet}}` {{greet export=false}}끝\n",
            false,
        );
        resolve_inline(&mut blocks, &root, "a.md");
        let text = |block: &crate::markdown::Block| match block {
            crate::markdown::Block::Heading { text, .. }
            | crate::markdown::Block::Paragraph(text) => crate::markdown::rich_text(text),
            _ => String::new(),
        };
        assert_eq!(text(&blocks[0]), "hi 책");
        assert_eq!(text(&blocks[1]), "앞 hi a.md  {{missing}} {{greet}} 끝");
        // 그림 확장은 그 자리의 그림 조각이 된다.
        let crate::markdown::Block::Paragraph(rich) = &blocks[1] else {
            panic!("paragraph");
        };
        let objects: Vec<&str> = rich
            .iter()
            .filter_map(|inline| inline.object.as_ref())
            .map(|object| object.name.as_str())
            .collect();
        assert_eq!(objects, ["dot"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn value_returns_one_line() {
        let extension = Extension::load(
            "today",
            "return { value = function(args, ctx) return (args.prefix or '') .. ctx.sheet .. '\\nmore' end }",
        )
        .unwrap();
        assert!(extension.values && !extension.draws);
        let args = [("prefix".to_string(), "@".to_string())];
        assert_eq!(
            extension.value(&args, "a.md").unwrap(),
            Some("@a.md".into())
        );
        let none = Extension::load("d", "return { draw = function() end }").unwrap();
        assert_eq!(none.value(&[], "a.md").unwrap(), None);
    }

    #[test]
    fn reserved_args_choose_align_and_export() {
        let args = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        let block = args(&[
            ("r", "5"),
            ("align", "left"),
            ("export", "false"),
            ("scale", "2"),
        ]);
        assert_eq!(without_reserved(&block), args(&[("r", "5")]));
        assert_eq!(align_of(&block), Align::Left);
        assert_eq!(align_of(&args(&[("align", "middle")])), Align::Center);
        assert_eq!(export_arg(&block), Some(false));
        assert_eq!(export_arg(&args(&[("export", "maybe")])), None);

        let root = std::env::temp_dir().join(format!("hanji-export-{}", std::process::id()));
        std::fs::create_dir_all(root.join(EXTENSIONS_DIR)).unwrap();
        let dot = "return { size = { 10, 10 }, draw = function(g) g.circle('fill', 5, 5, 2) end }";
        std::fs::write(path(&root, "dot"), dot).unwrap();
        std::fs::write(
            path(&root, "game"),
            dot.replace("return {", "return { export = false,"),
        )
        .unwrap();
        let drawn = still(&root, "dot", &args(&[("align", "right")]))
            .unwrap()
            .unwrap();
        assert_eq!(drawn.align, Align::Right);
        assert!(
            still(&root, "dot", &args(&[("export", "false")]))
                .unwrap()
                .is_none()
        );
        assert!(still(&root, "game", &[]).unwrap().is_none());
        assert!(
            still(&root, "game", &args(&[("export", "true")]))
                .unwrap()
                .is_some()
        );
        // 블록이 빼면 확장 파일이 없어도 코드 블록으로 남기지 않는다.
        assert!(
            still(&root, "missing", &args(&[("export", "false")]))
                .unwrap()
                .is_none()
        );
        assert!(still(&root, "missing", &[]).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn click_returns_declared_actions() {
        let source = |actions: &str, body: &str| {
            format!(
                "--[[---\ndescription: 버튼\nactions: {actions}\n---]]\n\
                 return {{ draw = function(g) g.region('b', 0, 0, 10, 10) end, \
                 click = function(id, args) return {body} end }}"
            )
        };
        let extension = Extension::load(
            "buttons",
            &source(
                "set, editor.insert, command",
                "{ args = { page = 2 }, actions = { { 'set', block = 'chart', args = { selected = 3 } }, \
                 { 'editor.insert', text = '안녕' }, { 'command', extension = 'text-tools', index = 2 } } }",
            ),
        )
        .unwrap();
        let clicked = extension.click("b", &[]).unwrap();
        assert_eq!(clicked.args, [("page".to_string(), "2".to_string())]);
        assert_eq!(
            clicked.actions,
            [
                Action::Set {
                    block: "chart".into(),
                    args: vec![("selected".into(), "3".into())]
                },
                Action::Insert("안녕".into()),
                Action::Command {
                    extension: "text-tools".into(),
                    command: CommandRef::Index(1)
                },
            ]
        );
        assert!(!clicked.actions[0].needs_consent() && clicked.actions[1].needs_consent());
        // 앞머리에 없는 동작, 모르는 동작, 잘못된 메뉴는 오류다.
        let undeclared = Extension::load(
            "x",
            &source("set", "{ actions = { { 'open', sheet = 'a.md' } } }"),
        )
        .unwrap();
        assert!(
            undeclared
                .click("b", &[])
                .unwrap_err()
                .contains("not declared")
        );
        let unknown =
            Extension::load("x", &source("set", "{ actions = { { 'delete' } } }")).unwrap();
        assert!(
            unknown
                .click("b", &[])
                .unwrap_err()
                .contains("unknown action")
        );
        let menu = Extension::load(
            "x",
            &source("menu", "{ actions = { { 'menu', action = 'quit' } } }"),
        )
        .unwrap();
        assert!(
            menu.click("b", &[])
                .unwrap_err()
                .contains("unknown menu action")
        );
    }

    #[test]
    fn click_changes_block_args() {
        let body = ["  mode: a", "count: 1", "note: x: y"];
        assert_eq!(
            set_args(
                &body,
                &[
                    ("count".into(), "2".into()),
                    ("note".into(), "z".into()),
                    ("new".into(), "yes".into())
                ]
            ),
            ["  mode: a", "count: 2", "note: z", "new: yes"]
        );
        let extension = Extension::load(
            "toggle",
            "return { draw = function(g, t, args, ui) g.region('x', 0, 0, 1, 1) end, \
             click = function(id, args) return { on = not args.on, label = 'a\\nb' } end }",
        )
        .unwrap();
        assert!(extension.click("x", &[]).unwrap_err().contains("one line"));
        let bad = Extension::load("bad", "return { click = function() end, commands = {} }");
        assert!(bad.unwrap_err().contains("draw"));
        // 변환을 적용한 테두리 상자로 모은다.
        let rotated = Extension::load(
            "r",
            "return { draw = function(g) g.translate(10, 0) g.rotate(math.pi / 2) \
             g.region('r', 0, 0, 4, 2) end }",
        )
        .unwrap();
        let (_, regions) = rotated
            .draw_ui(0.0, &[], &Theme::default(), &Ui::default())
            .unwrap();
        let [x, y, w, h] = regions[0].rect;
        assert!(
            (x - 8.0).abs() < 1e-4
                && y.abs() < 1e-4
                && (w - 2.0).abs() < 1e-4
                && (h - 4.0).abs() < 1e-4
        );
        assert_eq!(region_at(&regions, [9.0, 1.0]), Some("r"));
        assert_eq!(region_at(&regions, [11.0, 1.0]), None);
    }

    /// `extension` 스킬의 예제는 모델이 그대로 베끼므로 모두 실제로 돌아야 한다.
    #[test]
    fn skill_examples_run() {
        let doc = [
            include_str!("../../resources/skills/extension/references/examples/archer.md"),
            include_str!("../../resources/skills/extension/references/examples/bar-chart.md"),
            include_str!("../../resources/skills/extension/references/examples/night-sky.md"),
            include_str!("../../resources/skills/extension/references/examples/text-tools.md"),
            include_str!("../../resources/skills/extension/references/examples/turntable.md"),
            include_str!("../../resources/skills/extension/references/examples/seedling.md"),
            include_str!("../../resources/skills/extension/references/examples/route.md"),
            include_str!("../../resources/skills/extension/references/examples/pick-chart.md"),
            include_str!("../../resources/skills/extension/references/examples/chart-remote.md"),
            include_str!("../../resources/skills/extension/references/examples/today.md"),
            include_str!("../../resources/skills/extension/references/examples/dodge.md"),
        ]
        .concat();
        let sources: Vec<&str> = doc
            .split("```lua\n")
            .skip(1)
            .filter_map(|rest| rest.split("\n```").next())
            .collect();
        assert_eq!(sources.len(), 11);
        let theme = Theme::default();
        let args = [
            ("values".to_string(), "12, 30, 21, 8".to_string()),
            ("labels".to_string(), "봄, 여름, 가을, 겨울".to_string()),
            ("label".to_string(), "명중!".to_string()),
            ("sway".to_string(), "1".to_string()),
            ("from".to_string(), "서울".to_string()),
        ];
        for source in sources {
            let extension = Extension::load("example", source).unwrap();
            // 앞머리가 있고, 그림을 그리면 example 값이 모두 args에 설명되어 있다.
            let meta = parse_meta("example", source).unwrap();
            assert!(!meta.description.is_empty());
            for (key, _) in &meta.example {
                assert!(meta.args.iter().any(|(arg, _)| arg == key), "{key}");
            }
            if extension.draws {
                assert!(!meta.example.is_empty());
                extension.draw(0.0, &meta.example, &theme).unwrap();
            }
            if extension.values {
                assert!(extension.value(&meta.example, "a.md").unwrap().is_some());
            }
            // 플레이는 몇 초 동안 입력과 함께 돌려 본다.
            if extension.plays {
                let size = extension.play_size(&meta.example).unwrap();
                extension.enable_sound(sound::new_group()).unwrap();
                extension.play_load(&meta.example).unwrap();
                for (frame, key) in ["left", "right", "space", "", "", "left"]
                    .iter()
                    .cycle()
                    .take(300)
                    .enumerate()
                {
                    if frame % 7 == 0 && !key.is_empty() {
                        extension.play_call("keypressed", key.to_string()).unwrap();
                    }
                    if frame % 11 == 0 && !key.is_empty() {
                        extension.play_call("keyreleased", key.to_string()).unwrap();
                    }
                    extension.play_call("update", 1.0_f32 / 60.0).unwrap();
                    extension.play_draw(size, &theme).unwrap();
                }
            }
            // 누를 수 있으면 영역마다 눌러 보고, 돌려준 인자로도 그린다. 바꾸는 인자는 앞머리에 있다.
            if extension.clicks {
                let (_, regions) = extension
                    .draw_ui(0.0, &meta.example, &theme, &Ui::default())
                    .unwrap();
                assert!(!regions.is_empty());
                for region in &regions {
                    let changes = extension.click(&region.id, &meta.example).unwrap().args;
                    for (key, _) in &changes {
                        assert!(meta.args.iter().any(|(arg, _)| arg == key), "{key}");
                    }
                    let body: Vec<String> = meta
                        .example
                        .iter()
                        .map(|(key, value)| format!("{key}: {value}"))
                        .collect();
                    let body: Vec<&str> = body.iter().map(String::as_str).collect();
                    let clicked = parse_args(
                        &set_args(&body, &changes)
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    );
                    let hover = Ui {
                        hover: Some(region.id.clone()),
                        pressed: None,
                    };
                    extension.draw_ui(0.0, &clicked, &theme, &hover).unwrap();
                }
            }
            if extension.draws {
                for t in [0.0, 0.7, 1.9, 2.5] {
                    for args in [&args[..], &[]] {
                        let shapes = extension.draw(t, args, &theme).unwrap();
                        assert!(!shapes.is_empty());
                        let (w, h) = extension.size_for(args).unwrap();
                        let inside = |[x, y]: [f32; 2]| {
                            (-1.0..=w + 1.0).contains(&x) && (-1.0..=h + 1.0).contains(&y)
                        };
                        for shape in &shapes {
                            let points: Vec<[f32; 2]> = match shape {
                                Shape::Fill { path, .. } | Shape::Stroke { path, .. } => path
                                    .iter()
                                    .filter_map(|segment| match *segment {
                                        Segment::Move(p) | Segment::Line(p) => Some(p),
                                        Segment::Cubic(_, _, p) => Some(p),
                                        Segment::Close => None,
                                    })
                                    .collect(),
                                Shape::Text { origin, .. } => vec![*origin],
                                // 장면은 칸(뷰포트) 안만 그린다.
                                Shape::Scene3D(mesh) => mesh.clip.to_vec(),
                                Shape::Clip(_)
                                | Shape::ClipPath(_)
                                | Shape::Effects(_)
                                | Shape::Image { .. } => Vec::new(),
                            };
                            assert!(points.into_iter().all(inside), "t = {t}: {shape:?}");
                        }
                    }
                }
            }
            for ix in 0..extension.commands.len() {
                for selection in ["", "가나다\nabc"] {
                    let ctx = CommandContext {
                        selection: selection.into(),
                        ..CommandContext::default()
                    };
                    extension.run_command(ix, &ctx).unwrap();
                }
            }
        }
        // 글자 단위 명령은 한글을 깨뜨리지 않는다.
        let tools = Extension::load(
            "text-tools",
            doc.split("```lua\n")
                .nth(4)
                .unwrap()
                .split("\n```")
                .next()
                .unwrap(),
        )
        .unwrap();
        let ctx = CommandContext {
            selection: "가나다".into(),
            ..CommandContext::default()
        };
        assert_eq!(tools.run_command(2, &ctx), Ok(Some("다나가".into())));
        assert_eq!(tools.run_command(1, &ctx), Ok(Some("「가나다」".into())));
    }

    #[test]
    fn draws_a_3d_scene() {
        let extension = Extension::load(
            "x",
            r#"return { size = { 200, 100 }, draw = function(g)
                local s = g.newScene()
                s:camera(0, 0, 5, 0, 0, 0)
                s:setColor(1, 0, 0)
                s:box("fill", 1)
                s:line(-1, 0, 1, 1, 0, 1)
                s:draw()
                local x, y = s:project(0, 0, 0)
                g.print(string.format("%d,%d", x, y), 0, 0)
            end }"#,
        )
        .unwrap();
        let shapes = extension.draw(0.0, &[], &Theme::default()).unwrap();
        // 장면 하나가 모양 하나다. 정면에서 본 상자는 앞면(삼각형 둘)만 남고, 선은 굵기만큼의 사각형이다.
        assert_eq!(shapes.len(), 2, "{shapes:?}");
        let Shape::Scene3D(mesh) = &shapes[0] else {
            panic!("{shapes:?}");
        };
        assert_eq!(mesh.tris.iter().filter(|tri| tri.lit).count(), 2);
        assert_eq!(mesh.tris.iter().filter(|tri| !tri.lit).count(), 2);
        assert_eq!(mesh.bounds, [0.0, 0.0, 200.0, 100.0]);
        assert!(matches!(&shapes[1], Shape::Text { text, .. } if text == "100,50"));
        // 그리면 선이 상자보다 앞(위)에 보이고, 상자는 빛을 받은 빨강이다.
        let pixmap = render::Renderer::new().render(&shapes, 1.0, 200, 100, None);
        let at = |x, y| {
            let p = pixmap.sample(x, y);
            [p.r, p.g, p.b, p.a]
        };
        let [r, g, b, a] = at(100, 40);
        assert!(r > 100 && g == 0 && b == 0 && a == 255, "{:?}", at(100, 40));
        // 굵기 1인 선이 픽셀 경계에 걸쳐 반쯤 덮는다(색은 칠한 빨강 그대로).
        assert_eq!(at(120, 50), [128, 0, 0, 128]);
        assert_eq!(at(20, 20)[3], 0);

        let err = Extension::load(
            "x",
            "return { draw = function(g) g.newScene():teapot('fill') end }",
        )
        .unwrap()
        .draw(0.0, &[], &Theme::default())
        .unwrap_err();
        assert!(err.contains("scene:teapot does not exist"), "{err}");
        let err = Extension::load(
            "x",
            "return { draw = function(g) g.newScene():sphere('fill', 'big') end }",
        )
        .unwrap()
        .draw(0.0, &[], &Theme::default())
        .unwrap_err();
        assert!(err.contains("scene:sphere: argument #2"), "{err}");
    }

    #[test]
    fn colon_calls_and_255_colors_work() {
        let extension = Extension::load(
            "x",
            "return { draw = function(g) g:setColor(255, 0, 0) g:circle('fill', 1, 1, 1) end }",
        )
        .unwrap();
        let shapes = extension.draw(0.0, &[], &Theme::default()).unwrap();
        assert!(
            matches!(&shapes[0], Shape::Fill { paint, .. } if paint.color() == [1.0, 0.0, 0.0, 1.0])
        );
    }
}
