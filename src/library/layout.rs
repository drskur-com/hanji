//! 라이브러리 폴더 검증·초기화·레이아웃 보장.

use std::fmt;
use std::io;
use std::path::Path;

use super::meta::{ensure_project_layout, write_group_meta_if_absent};
use super::{
    DEFAULT_INBOX, GROUP_META_FILE, MARKER_DIR, MARKER_FILE, SECTIONS, SectionKind, is_group_dir,
    is_markdown,
};

/// 폴더 검증 결과.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderKind {
    /// 이미 Hanji 라이브러리(마커 존재).
    Library,
    /// 비어 있음.
    Empty,
    /// 마크다운 파일만 있음(확인 후 사용 가능).
    MarkdownOnly,
    /// 다른 내용이 있음(하위 폴더 생성 제안).
    NonEmpty,
}

/// 폴더 검증 실패.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InspectError {
    NotFound,
    NotADirectory,
    /// 선택 경로의 조상에 이미 라이브러리가 있음.
    Nested,
    NotWritable,
    Io(String),
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InspectError::NotFound => f.write_str(&t!("core.error.folder_not_found")),
            InspectError::NotADirectory => f.write_str(&t!("core.error.not_a_folder")),
            InspectError::Nested => f.write_str(&t!("core.error.nested_library")),
            InspectError::NotWritable => f.write_str(&t!("core.error.not_writable")),
            InspectError::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for InspectError {}

/// 경로가 라이브러리 표식(`.hanji/library.toml`)을 가지는지.
pub fn has_marker(path: &Path) -> bool {
    path.join(MARKER_DIR).join(MARKER_FILE).is_file()
}

/// 앱 이름이 Writer였을 때의 라이브러리 표식 디렉터리와 그룹 메타 파일 이름.
const LEGACY_MARKER_DIR: &str = ".writer";
const LEGACY_GROUP_META_FILE: &str = ".writer.toml";

/// 예전 이름(Writer)의 라이브러리면 그룹 메타(`.writer.toml` → `.hanji.toml`)와 표식
/// (`.writer/` → `.hanji/`)을 지금 이름으로 옮긴다. 예전 표식이 없으면 아무것도 하지 않는다(멱등).
///
/// 그룹 메타를 먼저 옮기고 표식을 마지막에 옮기므로, 중간에 실패해도 다음에 열 때 이어서 옮긴다.
pub fn migrate_legacy_names(root: &Path) -> io::Result<()> {
    let legacy_marker = root.join(LEGACY_MARKER_DIR);
    if !legacy_marker.join(MARKER_FILE).is_file() || root.join(MARKER_DIR).exists() {
        return Ok(());
    }
    rename_legacy_group_meta(root)?;
    std::fs::rename(legacy_marker, root.join(MARKER_DIR))
}

pub(super) fn rename_legacy_group_meta(dir: &Path) -> io::Result<()> {
    let legacy = dir.join(LEGACY_GROUP_META_FILE);
    let current = dir.join(GROUP_META_FILE);
    if legacy.is_file() && !current.exists() {
        std::fs::rename(&legacy, &current)?;
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        // 심볼릭 링크는 따라가지 않는다(라이브러리 밖으로 나가거나 순환할 수 있다).
        if !hidden && entry.file_type()?.is_dir() {
            rename_legacy_group_meta(&entry.path())?;
        }
    }
    Ok(())
}

/// 후보 폴더를 검사해 어떤 종류인지 판정한다.
pub fn inspect(path: &Path) -> Result<FolderKind, InspectError> {
    let meta = std::fs::metadata(path).map_err(|_| InspectError::NotFound)?;
    if !meta.is_dir() {
        return Err(InspectError::NotADirectory);
    }
    migrate_legacy_names(path).map_err(|err| InspectError::Io(err.to_string()))?;
    if has_marker(path) {
        return Ok(FolderKind::Library);
    }
    if path.ancestors().skip(1).any(has_marker) {
        return Err(InspectError::Nested);
    }
    if !is_writable(path) {
        return Err(InspectError::NotWritable);
    }

    let mut any = false;
    let mut has_markdown = false;
    for entry in std::fs::read_dir(path).map_err(|err| InspectError::Io(err.to_string()))? {
        let entry = entry.map_err(|err| InspectError::Io(err.to_string()))?;
        any = true;
        if is_markdown(&entry.path()) {
            has_markdown = true;
        }
    }

    Ok(if !any {
        FolderKind::Empty
    } else if has_markdown {
        FolderKind::MarkdownOnly
    } else {
        FolderKind::NonEmpty
    })
}

/// 라이브러리 골격(섹션 폴더 + 마커)을 만든다.
///
/// 반쪽 라이브러리를 피하려고 마커 파일을 마지막에 쓴다.
pub fn initialize(root: &Path) -> io::Result<()> {
    std::fs::create_dir_all(root)?;
    ensure_layout(root)?;
    write_default_meta(root)
}

/// 라이브러리 레이아웃을 최신 상태로 보장한다(멱등).
///
/// - `Notes/`·`Projects/` 폴더를 보장한다.
/// - 구버전 최상위 `Inbox/` 를 `Notes/Inbox/` 로 이관한다.
/// - 추가자료 폴더가 없는 프로젝트에 기본 추가자료(템플릿)를 만든다.
pub fn ensure_layout(root: &Path) -> io::Result<()> {
    for kind in SECTIONS {
        std::fs::create_dir_all(root.join(kind.dir()))?;
    }

    // 구버전 최상위 Inbox -> Notes/Inbox 이관
    let legacy_inbox = root.join("Inbox");
    let notes_inbox = root.join(SectionKind::Notes.dir()).join(DEFAULT_INBOX);
    if legacy_inbox.is_dir() {
        std::fs::create_dir_all(&notes_inbox)?;
        for entry in std::fs::read_dir(&legacy_inbox)? {
            let entry = entry?;
            let dest = notes_inbox.join(entry.file_name());
            if !dest.exists() {
                std::fs::rename(entry.path(), &dest)?;
            }
        }
        let _ = std::fs::remove_dir(&legacy_inbox);
    }
    std::fs::create_dir_all(&notes_inbox)?;
    write_group_meta_if_absent(&notes_inbox, Some("inbox"), None)?;

    for entry in std::fs::read_dir(root.join(SectionKind::Projects.dir()))? {
        let path = entry?.path();
        if is_group_dir(&path) {
            ensure_project_layout(&path)?;
        }
    }

    Ok(())
}

/// `.hanji/library.toml` 마커를 보장한다(멱등).
pub(super) fn write_default_meta(root: &Path) -> io::Result<()> {
    let dir = root.join(MARKER_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(MARKER_FILE);
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, "schema_version = 1\n")
}

/// 임시 파일을 만들어 실제 쓰기 가능 여부를 확인한다.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".hanji-write-probe");
    let result = std::fs::write(&probe, b"");
    if result.is_ok() {
        let _ = std::fs::remove_file(&probe);
    }
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;

    #[test]
    fn initialize_creates_sections_and_marker() {
        let root = temp_dir("init");
        initialize(&root).unwrap();

        assert!(has_marker(&root));
        for kind in SECTIONS {
            assert!(root.join(kind.dir()).is_dir(), "{:?}", kind);
        }
        assert!(root.join("Notes").join(DEFAULT_INBOX).is_dir());
        // 프로젝트는 만들어 두지 않는다.
        assert!(root.join("Projects").is_dir());
        assert_eq!(std::fs::read_dir(root.join("Projects")).unwrap().count(), 0);
        assert_eq!(inspect(&root).unwrap(), FolderKind::Library);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ensure_layout_migrates_legacy_inbox() {
        let root = temp_dir("migrate");
        initialize(&root).unwrap();

        // 구버전 형태로 되돌린다: Notes/Inbox 를 없애고 최상위 Inbox 를 만든다.
        std::fs::remove_dir_all(root.join("Notes").join(DEFAULT_INBOX)).unwrap();
        std::fs::create_dir_all(root.join("Inbox")).unwrap();
        std::fs::write(root.join("Inbox").join("note.md"), "n").unwrap();

        ensure_layout(&root).unwrap();

        assert!(
            root.join("Notes")
                .join(DEFAULT_INBOX)
                .join("note.md")
                .is_file()
        );
        assert!(!root.join("Inbox").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inspect_renames_legacy_writer_library() {
        let root = temp_dir("legacy-name");
        let group = root.join("Projects").join("소설");
        std::fs::create_dir_all(root.join(LEGACY_MARKER_DIR)).unwrap();
        std::fs::write(root.join(LEGACY_MARKER_DIR).join(MARKER_FILE), "").unwrap();
        std::fs::create_dir_all(group.join("Extras")).unwrap();
        std::fs::write(group.join(LEGACY_GROUP_META_FILE), "name = \"소설\"\n").unwrap();
        std::fs::write(group.join("Extras").join(LEGACY_GROUP_META_FILE), "").unwrap();

        assert_eq!(inspect(&root).unwrap(), FolderKind::Library);

        assert!(!root.join(LEGACY_MARKER_DIR).exists());
        assert!(!group.join(LEGACY_GROUP_META_FILE).exists());
        assert_eq!(
            std::fs::read_to_string(group.join(GROUP_META_FILE)).unwrap(),
            "name = \"소설\"\n"
        );
        assert!(group.join("Extras").join(GROUP_META_FILE).is_file());
        // 다시 불러도 그대로다.
        migrate_legacy_names(&root).unwrap();
        assert!(has_marker(&root));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inspect_reports_empty_and_nested() {
        let root = temp_dir("inspect");
        initialize(&root).unwrap();

        // 라이브러리 내부의 폴더는 Nested
        assert_eq!(
            inspect(&root.join("Notes")).unwrap_err(),
            InspectError::Nested
        );

        // 라이브러리 밖의 빈 폴더는 Empty
        let outside = temp_dir("outside");
        assert_eq!(inspect(&outside).unwrap(), FolderKind::Empty);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}
