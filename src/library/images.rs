//! 마크다운 이미지 경로(`![캡션](경로)`, ADR-0007)를 디스크의 파일로 찾는다.
//!
//! 미리보기·PDF·DOCX·ePub과 에디터가 같은 규칙을 써야 같은 그림이 보인다. 상대 경로는
//! 시트 폴더 → 프로젝트 폴더 → 첨부파일 그룹 순으로 찾는다.
//!
//! 프로젝트 시트에 넣는 그림은 프로젝트 폴더 기준 경로(`Extras/Attachments/…`)로 적는다
//! (ADR-0022). 시트나 그룹을 옮겨도 경로가 바뀌지 않는다.
//!
//! 메모에 넣는 그림은 `Notes/` 기준 경로(`Attachments/…`)로 적는다(ADR-0041). 메모를 다른
//! 그룹으로 옮겨도 경로가 바뀌지 않는다.
//!
//! 예전에 넣은 시트 기준 경로(`../Extras/…`)는 옮기면 `../` 깊이가 어긋난다(ADR-0021).
//! 그런 경로도 찾을 수 있도록 `../` 뒤의 꼬리를 시트의 위 폴더들(프로젝트 폴더까지)에서
//! 한 번 더 찾는다.

use std::path::{Component, Path, PathBuf};

use super::is_notes_dir;
use super::layout::has_marker;
use super::{GROUP_META_FILE, SECTIONS, SectionKind, attachments_dir};

/// 웹 주소인지. 웹 이미지는 파일이 아니라 주소를 그대로 쓴다.
pub fn is_web_address(destination: &str) -> bool {
    destination.starts_with("http://") || destination.starts_with("https://")
}

/// 이미지 경로를 디스크의 파일로 찾는다. 웹 주소이거나 파일이 없으면 `None`.
/// 찾은 경로는 `..`를 푼 실제 경로라 같은 파일이면 같은 값이다(내보내기에서 한 번만 넣는다).
///
/// `project_dir`는 프로젝트 폴더(원고·추가자료의 기준)다. 모르면 `None`을 준다.
/// 시트 폴더가 든 라이브러리 프로젝트 폴더나 메모 섹션 폴더(`Notes/`, `base_dir_of`)도 함께 본다.
/// 선택한 시트만 내보낼 때는 `project_dir`가 시트 폴더라서 이것으로 첨부파일을 찾는다.
pub fn resolve_image_path(
    sheet_dir: &Path,
    project_dir: Option<&Path>,
    destination: &str,
) -> Option<PathBuf> {
    if is_web_address(destination) {
        return None;
    }
    let path = Path::new(destination.strip_prefix("file://").unwrap_or(destination));
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }

    let library_base = base_dir_of(sheet_dir);
    let project_dir = project_dir.or(library_base);
    let mut bases: Vec<&Path> = project_dir.into_iter().collect();
    bases.extend(library_base.filter(|base| Some(*base) != project_dir));
    let mut candidates = vec![sheet_dir.join(path)];
    // 기준 폴더 기준 경로는 그 안을 가리킨다. `..`가 있으면 밖으로 나가므로 보지 않는다.
    if !path.components().any(|part| part == Component::ParentDir) {
        for base in bases {
            candidates.push(base.join(path));
            candidates.push(attachments_dir(base).join(path));
        }
    }
    let found = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .or_else(|| {
            // `../`가 어긋난 경로: `../`를 뗀 꼬리를 시트의 위 폴더들에서 다시 찾는다.
            let tail = without_leading_parents(path)?;
            if tail.as_path() == path {
                return None;
            }
            search_dirs(sheet_dir, project_dir)
                .into_iter()
                .map(|dir| dir.join(&tail))
                .find(|candidate| candidate.is_file())
        })?;
    Some(found.canonicalize().unwrap_or(found))
}

/// 폴더가 든 첨부 경로의 기준 폴더: 라이브러리 프로젝트 폴더(`<라이브러리>/Projects/<프로젝트>`)나
/// 메모 섹션 폴더(`<라이브러리>/Notes`). 라이브러리 밖이면 `None`.
fn base_dir_of(dir: &Path) -> Option<&Path> {
    dir.ancestors().find(|dir| {
        is_notes_dir(dir)
            || dir.parent().is_some_and(|section| {
                section.ends_with(SectionKind::Projects.dir())
                    && section.parent().is_some_and(has_marker)
            })
    })
}

/// 어긋난 경로를 다시 찾을 시트 폴더와 그 위 폴더들(프로젝트 폴더까지).
///
/// 프로젝트 폴더를 알면 거기까지 올라간다. 모르면 라이브러리 섹션(`Projects/`·`Notes/`)
/// 바로 아래 폴더까지 올라가고, 라이브러리 밖이면 `.hanji.toml`이 있는 그룹 폴더까지만 본다.
/// 그룹 폴더에 `.hanji.toml`이 꼭 있지는 않으므로 라이브러리 안에서는 그것에 기대지 않는다.
fn search_dirs<'a>(sheet_dir: &'a Path, project_dir: Option<&Path>) -> Vec<&'a Path> {
    let ancestors: Vec<&Path> = sheet_dir.ancestors().collect();
    let end = match project_dir {
        Some(project_dir) => ancestors.iter().position(|dir| *dir == project_dir),
        None => ancestors.iter().position(|dir| {
            dir.parent().is_some_and(|section| {
                SECTIONS.iter().any(|kind| section.ends_with(kind.dir()))
                    && section.parent().is_some_and(has_marker)
            })
        }),
    };
    match end {
        Some(end) => ancestors[..=end].to_vec(),
        None => ancestors
            .into_iter()
            .take_while(|dir| dir.join(GROUP_META_FILE).is_file())
            .collect(),
    }
}

/// 앞머리의 `.`·`..`를 뗀 경로. 뗄 것이 없거나 남는 것이 없으면 `None`.
fn without_leading_parents(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components().peekable();
    while matches!(parts.peek(), Some(Component::CurDir | Component::ParentDir)) {
        parts.next();
    }
    parts.peek()?;
    Some(parts.collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;
    use crate::library::{ATTACHMENTS_GROUP, EXTRAS_DIR};

    /// `Projects/책/본문/1장` 아래 시트, `Projects/책/Extras/Attachments`의 이미지.
    fn project() -> (PathBuf, PathBuf) {
        let root = temp_dir("images");
        let project = root.join("Projects").join("책");
        let attachments = project.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::create_dir_all(project.join("본문").join("1장")).unwrap();
        for group in [
            &project,
            &project.join("본문"),
            &project.join("본문").join("1장"),
        ] {
            std::fs::write(group.join(GROUP_META_FILE), "").unwrap();
        }
        std::fs::write(attachments.join("a.png"), "png").unwrap();
        let sheet_dir = project.join("본문").join("1장");
        (project, sheet_dir)
    }

    /// `Extras/Attachments/a.png`의 실제 경로(`..`·심볼릭 링크를 푼 경로).
    fn attachment(project: &Path) -> PathBuf {
        project
            .join(EXTRAS_DIR)
            .join(ATTACHMENTS_GROUP)
            .join("a.png")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn finds_relative_path_from_sheet_folder() {
        let (project, sheet_dir) = project();
        assert_eq!(
            resolve_image_path(&sheet_dir, Some(&project), "../../Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
    }

    #[test]
    fn finds_stale_parent_path_in_groups_without_meta_file() {
        // 라이브러리 안에서는 `.hanji.toml`이 없는 그룹이어도 프로젝트 폴더까지 올라간다.
        let root = temp_dir("images-library");
        crate::library::initialize(&root).unwrap();
        let project = root.join("Projects").join("책");
        let attachments = project.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);
        let sheet_dir = project.join("본문").join("1장");
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::create_dir_all(&sheet_dir).unwrap();
        std::fs::write(attachments.join("a.png"), "png").unwrap();
        assert_eq!(
            resolve_image_path(&sheet_dir, None, "../Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
        // 프로젝트 폴더 밖(`Projects/`)까지는 올라가지 않는다.
        std::fs::write(root.join("Projects").join("b.png"), "png").unwrap();
        assert_eq!(resolve_image_path(&sheet_dir, None, "../b.png"), None);
    }

    #[test]
    fn finds_stale_parent_path_from_upper_folders() {
        // 그룹을 옮겨 `../` 깊이가 어긋난 경로도 같은 파일을 찾는다.
        let (project, sheet_dir) = project();
        assert_eq!(
            resolve_image_path(&sheet_dir, Some(&project), "../Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
        // 프로젝트 폴더를 몰라도(에디터·미리보기) 위 폴더를 훑어 찾는다.
        assert_eq!(
            resolve_image_path(&sheet_dir, None, "../Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
        // 시트를 프로젝트 폴더 맨 위로 옮겨 `..`가 하나 더 붙은 경우.
        assert_eq!(
            resolve_image_path(&project, None, "../../Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
    }

    #[test]
    fn finds_project_relative_path() {
        let (project, sheet_dir) = project();
        assert_eq!(
            resolve_image_path(&sheet_dir, Some(&project), "Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
    }

    #[test]
    fn finds_project_relative_path_without_project_dir() {
        // 에디터·미리보기는 프로젝트 폴더를 넘기지 않는다. 라이브러리 안이면 시트 폴더에서 찾는다.
        let root = temp_dir("images-project-relative");
        crate::library::initialize(&root).unwrap();
        let project = root.join("Projects").join("책");
        let attachments = project.join(EXTRAS_DIR).join(ATTACHMENTS_GROUP);
        let sheet_dir = project.join("본문").join("1장");
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::create_dir_all(&sheet_dir).unwrap();
        std::fs::write(attachments.join("a.png"), "png").unwrap();
        assert_eq!(base_dir_of(&sheet_dir), Some(project.as_path()));
        assert_eq!(base_dir_of(&root.join("Projects")), None);
        assert_eq!(
            resolve_image_path(&sheet_dir, None, "Extras/Attachments/a.png"),
            Some(attachment(&project))
        );
    }

    #[test]
    fn finds_note_relative_path_from_any_note_group() {
        // 메모 그림(`Attachments/…`)은 메모가 어느 그룹에 있든 `Notes/Attachments`에서 찾는다.
        let root = temp_dir("images-notes");
        crate::library::initialize(&root).unwrap();
        let notes = root.join("Notes");
        let attachments = notes.join("Attachments");
        let group = notes.join("일기").join("2026");
        std::fs::create_dir_all(&attachments).unwrap();
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(attachments.join("a.png"), "png").unwrap();
        let found = Some(attachments.join("a.png").canonicalize().unwrap());
        assert_eq!(base_dir_of(&group), Some(notes.as_path()));
        assert_eq!(resolve_image_path(&group, None, "Attachments/a.png"), found);
        assert_eq!(
            resolve_image_path(&notes.join("Inbox"), None, "Attachments/a.png"),
            found
        );
        assert_eq!(resolve_image_path(&notes, None, "Attachments/a.png"), found);
        // 선택한 메모만 내보내면(ePub·DOCX) 시트 폴더가 기준 폴더로 들어온다.
        assert_eq!(
            resolve_image_path(&group, Some(&group), "Attachments/a.png"),
            found
        );
    }

    #[test]
    fn leaves_web_addresses_and_missing_files() {
        let (project, sheet_dir) = project();
        assert_eq!(
            resolve_image_path(&sheet_dir, Some(&project), "https://x.io/a.png"),
            None
        );
        assert_eq!(
            resolve_image_path(&sheet_dir, Some(&project), "../Extras/Attachments/b.png"),
            None
        );
        assert_eq!(
            resolve_image_path(
                &sheet_dir,
                Some(&project),
                attachment(&project).to_str().unwrap()
            ),
            Some(attachment(&project))
        );
    }
}
