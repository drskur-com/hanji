//! 파일 그룹(`kind = "files"`): 시트 대신 아무 파일이나 담는 그룹(첨부파일 등).
//!
//! 파일 종류는 저장하지 않고 확장자로 판단한다. Finder에서 파일을 바꿔도 어긋나지 않는다.

use std::io;
use std::path::{Path, PathBuf};

use super::meta::write_meta_list;
use super::{GROUP_META_FILE, Group, created_nanos, file_name, modified_nanos, order_by_names};

/// 그룹이 담는 것. 그룹 메타 `kind`에 저장하고, 없으면 시트.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GroupKind {
    /// 마크다운 시트(`*.md`).
    #[default]
    Sheets,
    /// 아무 파일(숨김 파일·메타 파일 제외).
    Files,
}

impl GroupKind {
    /// 메타 파일에 쓰는 이름. 시트는 기본값이라 쓰지 않는다.
    pub fn key(self) -> &'static str {
        match self {
            GroupKind::Sheets => "sheets",
            GroupKind::Files => "files",
        }
    }

    pub(super) fn from_key(key: Option<&str>) -> Self {
        match key {
            Some("files") => GroupKind::Files,
            _ => GroupKind::Sheets,
        }
    }
}

/// 뷰어를 고르는 파일 종류.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Image,
    /// 동영상(ADR-0050).
    Video,
    /// 오디오(ADR-0050).
    Audio,
    Text,
    Pdf,
    Other,
}

impl FileKind {
    /// 필터 메뉴에 보이는 종류. PDF는 아직 따로 거르지 않고 기타에 넣는다.
    pub const FILTERS: [FileKind; 5] = [
        FileKind::Image,
        FileKind::Video,
        FileKind::Audio,
        FileKind::Text,
        FileKind::Other,
    ];

    /// 필터에서 묶이는 종류(PDF → 기타).
    pub fn filter_kind(self) -> Self {
        match self {
            FileKind::Pdf => FileKind::Other,
            kind => kind,
        }
    }

    /// 확장자로 종류를 정한다(대소문자 무시).
    pub fn of(path: &Path) -> Self {
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff" => {
                FileKind::Image
            }
            "txt" | "md" | "markdown" | "csv" | "json" | "toml" | "yaml" | "yml" | "xml"
            | "html" | "htm" | "css" => FileKind::Text,
            "pdf" => FileKind::Pdf,
            ext if crate::media::VIDEO_EXTENSIONS.contains(&ext) => FileKind::Video,
            ext if crate::media::AUDIO_EXTENSIONS.contains(&ext) => FileKind::Audio,
            _ => FileKind::Other,
        }
    }
}

/// 파일 그룹 안의 파일. 스캔은 메타데이터만 읽는다.
#[derive(Clone, Debug)]
pub struct FileItem {
    pub path: PathBuf,
    pub kind: FileKind,
    /// 바이트 수.
    pub size: u64,
    /// 수정 시각(나노초).
    pub modified: u128,
    /// 만든 시각(나노초). 시트처럼 ULID 파일명이면 거기서 얻는다.
    pub created: u128,
}

impl FileItem {
    /// 파일 이름(확장자 포함).
    pub fn name(&self) -> String {
        file_name(&self.path)
    }
}

/// 파일 그룹 폴더 바로 아래의 파일을 읽는다. 순서는 메타 `files` → 이름순.
pub(super) fn scan_files(dir: &Path, order: &[String]) -> io::Result<Vec<FileItem>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = file_name(&path);
        if name.starts_with('.') || name == GROUP_META_FILE || !path.is_file() {
            continue;
        }
        let modified = modified_nanos(&path);
        files.push(FileItem {
            kind: FileKind::of(&path),
            size: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            modified,
            created: created_nanos(&path).unwrap_or(modified),
            path,
        });
    }
    files.sort_by_key(|file| file.name().to_lowercase());
    order_by_names(&mut files, order, FileItem::name);
    Ok(files)
}

/// 파일 그룹 안에서 파일을 `from` 위치에서 `to` 위치로 옮기고, 새 순서를 `.hanji.toml`의 `files`에 쓴다.
///
/// 범위를 벗어나면 아무것도 하지 않는다. 쓰기에 실패하면 메모리의 순서도 되돌린다.
pub fn move_file(group: &mut Group, from: usize, to: usize) -> io::Result<()> {
    if from >= group.files.len() || to >= group.files.len() || from == to {
        return Ok(());
    }
    let file = group.files.remove(from);
    group.files.insert(to, file);
    let order: Vec<String> = group.files.iter().map(FileItem::name).collect();
    if let Err(err) = write_meta_list(&group.path, "files", &order) {
        let file = group.files.remove(to);
        group.files.insert(from, file);
        return Err(err);
    }
    Ok(())
}

/// 바이트 수를 읽기 쉽게(예: `1.2 MB`).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;
    use crate::library::*;

    #[test]
    fn file_group_lists_files_instead_of_sheets() {
        let root = temp_dir("files");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();
        let source = root.join("표지.png");
        std::fs::write(&source, "png").unwrap();
        set_cover(&dir, &source).unwrap();
        let attachments = dir.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);
        std::fs::write(attachments.join("메모.md"), "글").unwrap();
        std::fs::write(attachments.join("자료.pdf"), "pdf").unwrap();
        std::fs::write(attachments.join(".DS_Store"), "").unwrap();

        let library = scan(&root).unwrap();
        let group = library.find_group(&attachments).unwrap();
        assert_eq!(group.kind, GroupKind::Files);
        assert!(group.sheets.is_empty());
        let names: Vec<_> = group.files.iter().map(FileItem::name).collect();
        assert_eq!(names, vec!["cover.png", "메모.md", "자료.pdf"]);
        let kinds: Vec<_> = group.files.iter().map(|file| file.kind).collect();
        assert_eq!(kinds, vec![FileKind::Image, FileKind::Text, FileKind::Pdf]);
        assert_eq!(group.files[0].size, 3);

        // 보통 그룹은 그대로 시트 그룹이다.
        let templates = dir.join(EXTRAS_DIR).join(TEMPLATES_GROUP);
        assert_eq!(
            library.find_group(&templates).unwrap().kind,
            GroupKind::Sheets
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_file_writes_order() {
        let root = temp_dir("move-file");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();
        let source = root.join("표지.png");
        std::fs::write(&source, "png").unwrap();
        set_cover(&dir, &source).unwrap();
        let attachments = dir.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);
        for name in ["a.txt", "b.txt"] {
            std::fs::write(attachments.join(name), name).unwrap();
        }

        let mut group = scan(&root)
            .unwrap()
            .find_group(&attachments)
            .cloned()
            .unwrap();
        // 이름순(a, b, cover)에서 표지를 맨 앞으로.
        move_file(&mut group, 2, 0).unwrap();
        let names: Vec<_> = group.files.iter().map(FileItem::name).collect();
        assert_eq!(names, vec!["cover.png", "a.txt", "b.txt"]);
        let rescanned = scan(&root)
            .unwrap()
            .find_group(&attachments)
            .cloned()
            .unwrap();
        let names: Vec<_> = rescanned.files.iter().map(FileItem::name).collect();
        assert_eq!(names, vec!["cover.png", "a.txt", "b.txt"]);
        assert_eq!(rescanned.kind, GroupKind::Files);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(20 * 1024 * 1024), "20 MB");
    }
}
