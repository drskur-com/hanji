//! 책 정보(제목·지은이·언어·소개·표지): 프로젝트 `.hanji.toml`의 `[book]` 표.
//!
//! ePub(OPF)·DOCX(`core.xml`)·PDF·HTML 내보내기가 같은 값을 형식에 맞게 옮겨 쓴다.
//! 첨부 파일은 `<Project>/Extras/Attachments/`에 두고 프로젝트 기준 상대 경로로 가리킨다.
//! 메모의 첨부 파일은 `Notes/Attachments/`에 두고 `Notes/` 기준 상대 경로로 가리킨다(ADR-0041).
//! 표지는 `cover.<확장자>`로 고정하고, 다른 첨부는 `<ULID>.<확장자>`로 둔다.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::meta::{
    group_order, read_group_meta, write_group_meta_if_absent, write_meta_list, write_meta_values,
};
use super::{ATTACHMENTS_GROUP, EXTRAS_DIR, GROUP_META_FILE, GroupKind, file_name, is_notes_dir};

/// 표지로 쓸 수 있는 이미지 확장자(ePub 핵심 미디어 형식).
pub const COVER_EXTENSIONS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "webp", "svg"];

/// 프로젝트의 책 정보. 빈 값은 파일에 쓰지 않는다.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct BookMeta {
    /// 책 제목(비면 내보낼 때 프로젝트 이름).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// 지은이. 공저자가 있을 수 있어 목록이다.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// 언어 태그(BCP 47, 예: `ko`). ePub에서 필수라 비면 내보낼 때 `ko`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    /// 소개(ePub `dc:description`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// 표지 이미지, 프로젝트 기준 상대 경로(`/` 구분).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// 책 식별자(`urn:uuid:…`). 처음 저장할 때 만들고 바꾸지 않는다(ePub `dc:identifier`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub identifier: String,
}

impl BookMeta {
    /// 표지 이미지의 절대 경로.
    pub fn cover_path(&self, project_dir: &Path) -> Option<PathBuf> {
        self.cover.as_deref().map(|cover| project_dir.join(cover))
    }
}

/// 프로젝트의 책 정보를 읽는다. 없거나 깨지면 기본값.
pub fn read_book(project_dir: &Path) -> BookMeta {
    std::fs::read_to_string(project_dir.join(GROUP_META_FILE))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|mut table| table.remove("book"))
        .and_then(|book| book.try_into().ok())
        .unwrap_or_default()
}

/// 책 정보를 `.hanji.toml`의 `[book]`에 쓴다. 다른 키는 그대로 둔다.
///
/// 식별자가 없으면 새로 만들어 `book`에도 채운다.
pub fn write_book(project_dir: &Path, book: &mut BookMeta) -> io::Result<()> {
    if book.identifier.is_empty() {
        book.identifier = new_identifier();
    }
    let value = toml::Value::try_from(&*book)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    write_meta_values(project_dir, [("book", Some(value))])
}

/// 표지 파일 이름(확장자 제외). 표지는 첨부파일 그룹에 늘 이 이름으로 하나만 둔다.
pub const COVER_STEM: &str = "cover";

/// 이미지를 표지(`Extras/Attachments/cover.<확장자>`)로 넣고 프로젝트 기준 상대 경로를 돌려준다.
///
/// 이미 있는 표지는 덮어쓰고, 확장자가 다른 옛 표지(`cover.png` → `cover.jpg`)는 지운다.
/// 확장자는 소문자로 맞추고 `jpeg`는 `jpg`로 쓴다. 지금 표지를 다시 고르면 아무것도 하지 않는다.
pub fn set_cover(project_dir: &Path, source: &Path) -> io::Result<String> {
    let extension = match normalized_extension(source).as_str() {
        "jpeg" => "jpg".to_string(),
        other => other.to_string(),
    };
    if !COVER_EXTENSIONS.contains(&extension.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("book.cover.unsupported_format").into_owned(),
        ));
    }
    let dir = ensure_attachments(project_dir)?;
    let target = dir.join(format!("{COVER_STEM}.{extension}"));
    if source != target {
        // 임시 파일에 복사한 뒤 바꿔치기해서, 복사가 끊겨도 옛 표지가 남는다.
        let temp = dir.join(format!(".{COVER_STEM}.tmp"));
        std::fs::copy(source, &temp)?;
        std::fs::rename(&temp, &target)?;
    }
    for other in COVER_EXTENSIONS.iter().filter(|&&ext| ext != extension) {
        let old = dir.join(format!("{COVER_STEM}.{other}"));
        if old.is_file() {
            std::fs::remove_file(old)?;
        }
    }
    Ok(relative_string(
        target.strip_prefix(project_dir).unwrap_or(&target),
    ))
}

/// 첨부파일 그룹 폴더. `base_dir`가 프로젝트 폴더면 `<Project>/Extras/Attachments`,
/// 메모 섹션 폴더(`Notes/`)면 `Notes/Attachments`다(ADR-0041).
pub fn attachments_dir(base_dir: &Path) -> PathBuf {
    if is_notes_dir(base_dir) {
        base_dir.join(ATTACHMENTS_GROUP)
    } else {
        base_dir.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP)
    }
}

/// 파일을 첨부파일 그룹에 `<ULID>.<확장자>`로 넣고 기준 폴더(`base_dir`: 프로젝트 폴더나 `Notes/`)
/// 기준 상대 경로를 돌려준다.
///
/// 파일 이름은 시트처럼 바뀌지 않는 식별자다. 이미 기준 폴더 안에 있는 파일이면 복사하지 않고
/// 그 경로를 쓴다. 첨부파일 그룹이 없으면 만들고, 프로젝트면 추가자료 순서 맨 뒤에 붙인다.
pub fn add_attachment(base_dir: &Path, source: &Path) -> io::Result<String> {
    if let Ok(relative) = source.strip_prefix(base_dir) {
        return Ok(relative_string(relative));
    }
    let dir = ensure_attachments(base_dir)?;
    let extension = normalized_extension(source);
    let name = ulid::Ulid::generate().to_string();
    let target = if extension.is_empty() {
        dir.join(name)
    } else {
        dir.join(format!("{name}.{extension}"))
    };
    std::fs::copy(source, &target)?;
    Ok(relative_string(
        target.strip_prefix(base_dir).unwrap_or(&target),
    ))
}

/// 내려받은 이미지 바이트를 첨부파일 그룹에 `<ULID>.<확장자>`로 쓰고, 기준 폴더(`add_attachment`와
/// 같다) 기준 상대 경로를 돌려준다. 웹 주소 이미지도 파일로 두어 미리보기·PDF·ePub이 오프라인에서
/// 같게 한다.
pub fn attach_bytes(base_dir: &Path, bytes: &[u8], extension: &str) -> io::Result<String> {
    let dir = ensure_attachments(base_dir)?;
    let target = dir.join(format!("{}.{extension}", ulid::Ulid::generate()));
    std::fs::write(&target, bytes)?;
    Ok(relative_string(
        target.strip_prefix(base_dir).unwrap_or(&target),
    ))
}

/// 바이트의 앞부분으로 이미지 형식을 알아내 확장자를 돌려준다. 넣을 수 있는 형식(JPEG·PNG·GIF·
/// WebP·SVG)이 아니면 `None`.
pub fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    // SVG는 텍스트라 앞부분에 `<svg`가 있는지 본다(XML 선언·주석이 앞설 수 있다).
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with('<') && head.contains("<svg") {
        return Some("svg");
    }
    None
}

/// 소문자 확장자(없으면 빈 문자열).
fn normalized_extension(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 첨부파일 그룹(`attachments_dir`)이 없으면 메타(아이콘·표시 이름)와 함께 만든다(멱등).
fn ensure_attachments(base_dir: &Path) -> io::Result<PathBuf> {
    let dir = attachments_dir(base_dir);
    let files = GroupKind::Files.key();
    if dir.is_dir() {
        // 파일 그룹 표시가 없는 옛 폴더도 파일 그룹으로 맞춘다.
        if read_group_meta(&dir).kind.as_deref() != Some(files) {
            write_meta_values(&dir, [("kind", Some(toml::Value::String(files.into())))])?;
        }
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    write_group_meta_if_absent(&dir, Some("paperclip"), None)?;
    write_meta_values(&dir, [("kind", Some(toml::Value::String(files.into())))])?;
    // 메모 섹션 폴더는 그룹 순서를 두지 않는다(이름순).
    if is_notes_dir(base_dir) {
        return Ok(dir);
    }
    let extras = base_dir.join(EXTRAS_DIR);
    let mut order = group_order(&extras)?;
    order.retain(|name| *name != file_name(&dir));
    order.push(file_name(&dir));
    write_meta_list(&extras, "groups", &order)?;
    Ok(dir)
}

/// 상대 경로를 운영체제와 상관없이 `/`로 이어 쓴다.
fn relative_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 무작위 UUID(v4 형식) 식별자.
fn new_identifier() -> String {
    let mut bytes = ulid::Ulid::generate().0.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;
    use crate::library::*;

    #[test]
    fn book_round_trips_and_keeps_group_meta() {
        let root = temp_dir("book");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", Some("book"), Some("blue")).unwrap();
        assert_eq!(read_book(&dir), BookMeta::default());

        let mut book = BookMeta {
            title: "긴 이야기".into(),
            authors: vec!["홍길동".into(), "임꺽정".into()],
            language: "ko".into(),
            description: "첫 줄\n둘째 줄".into(),
            ..Default::default()
        };
        write_book(&dir, &mut book).unwrap();
        assert!(book.identifier.starts_with("urn:uuid:"));
        assert_eq!(read_book(&dir), book);

        // 다시 써도 식별자는 그대로, 그룹 메타도 그대로.
        let identifier = book.identifier.clone();
        book.title = "짧은 이야기".into();
        write_book(&dir, &mut book).unwrap();
        assert_eq!(read_book(&dir).identifier, identifier);
        let project = scan(&root).unwrap().find_project(&dir).cloned().unwrap();
        assert_eq!(project.icon.as_deref(), Some("book"));
        assert_eq!(project.name, "소설");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cover_has_fixed_name_and_replaces_old_one() {
        let root = temp_dir("cover");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();
        let png = root.join("표지 시안.PNG");
        let jpeg = root.join("사진.jpeg");
        std::fs::write(&png, "png").unwrap();
        std::fs::write(&jpeg, "jpeg").unwrap();
        let attachments = dir.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);

        assert_eq!(
            set_cover(&dir, &png).unwrap(),
            "Extras/Attachments/cover.png"
        );
        // 다시 골라도(같은 파일이어도) 그대로.
        let cover = attachments.join("cover.png");
        assert_eq!(
            set_cover(&dir, &cover).unwrap(),
            "Extras/Attachments/cover.png"
        );
        assert_eq!(std::fs::read_to_string(&cover).unwrap(), "png");

        // 확장자가 바뀌면 옛 표지는 지운다.
        assert_eq!(
            set_cover(&dir, &jpeg).unwrap(),
            "Extras/Attachments/cover.jpg"
        );
        assert!(!cover.exists());
        assert!(!attachments.join(".cover.tmp").exists());
        assert!(set_cover(&dir, &root.join("메모.txt")).is_err());

        // 추가자료에서 템플릿 뒤에 첨부파일로 보인다(이름은 UI 언어, 테스트는 영어).
        let library = scan(&root).unwrap();
        let project = library.find_project(&dir).unwrap();
        let extras: Vec<_> = project.extras.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(extras, vec!["Templates", "Attachments"]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn downloaded_bytes_become_attachments() {
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(image_extension(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(
            image_extension(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"\"></svg>"),
            Some("svg")
        );
        assert_eq!(image_extension(b"<!doctype html><html>"), None);

        let root = temp_dir("attach-bytes");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();
        let relative = attach_bytes(&dir, b"png", "png").unwrap();
        let name = relative.strip_prefix("Extras/Attachments/").unwrap();
        assert!(name.ends_with(".png"), "{relative}");
        assert_eq!(std::fs::read(dir.join(&relative)).unwrap(), b"png");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn note_attachments_go_to_shared_notes_group() {
        let root = temp_dir("note-attachments");
        initialize(&root).unwrap();
        let notes = root.join(SectionKind::Notes.dir());
        let source = root.join("사진.JPG");
        std::fs::write(&source, "jpg").unwrap();

        let relative = add_attachment(&notes, &source).unwrap();
        let name = relative.strip_prefix("Attachments/").unwrap();
        assert!(name.ends_with(".jpg"), "{relative}");
        assert_eq!(std::fs::read(notes.join(&relative)).unwrap(), b"jpg");
        let bytes = attach_bytes(&notes, b"png", "png").unwrap();
        assert!(bytes.starts_with("Attachments/"), "{bytes}");

        // 메모 섹션에는 파일 그룹으로 보인다.
        let library = crate::library::scan(&root).unwrap();
        let group = library.find_group(&notes.join(ATTACHMENTS_GROUP)).unwrap();
        assert_eq!(group.kind, GroupKind::Files);
        assert_eq!(group.name, "Attachments");
        assert_eq!(group.files.len(), 2);
        // 첨부파일 그룹은 색을 고르지 않아도 다른 그룹과 다른 색으로 보인다.
        assert_eq!(group.display_color(), Some("cyan"));
        let inbox = library.find_group(&notes.join("Inbox")).unwrap();
        assert_eq!(inbox.display_color(), None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn attachments_get_id_names() {
        let root = temp_dir("attachments");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();
        let source = root.join("지도.PNG");
        std::fs::write(&source, "png").unwrap();

        let first = add_attachment(&dir, &source).unwrap();
        let second = add_attachment(&dir, &source).unwrap();
        assert_ne!(first, second);
        let name = first.strip_prefix("Extras/Attachments/").unwrap();
        let id = name.strip_suffix(".png").unwrap();
        assert!(id.parse::<ulid::Ulid>().is_ok(), "{id}");
        // 프로젝트 안의 파일은 복사하지 않는다.
        assert_eq!(add_attachment(&dir, &dir.join(&first)).unwrap(), first);

        std::fs::remove_dir_all(&root).ok();
    }
}
