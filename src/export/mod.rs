//! 프로젝트 내보내기: 형식별 내보내기가 공유하는 원고 데이터와 오류.
//!
//! 책 정보(`.hanji.toml`의 `[book]`)와 책 스타일(`[styles.*]`)을 함께 읽는다.
//! 형식별 생성·저장은 하위 모듈이 맡는다. 현재 구현한 형식은 [`epub`]·[`pdf`]·[`docx`]다(PDF는 미리보기 배치를 그대로 쓴다).
//! [`project`]는 원고가 아니라 프로젝트 폴더 전체를 `.hanji` 파일로 묶는다.

pub mod docx;
pub mod epub;
pub mod pdf;
pub mod project;

use std::io;
use std::path::{Path, PathBuf};

use crate::library::{self, BookMeta, Styles};

/// 내보내기 중 생길 수 있는 오류.
#[derive(Debug)]
pub enum ExportError {
    Io(io::Error),
    Zip(zip::result::ZipError),
    ProjectNotFound(PathBuf),
    InvalidProject(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{}", t!("export.error.io", err = err)),
            Self::Zip(err) => write!(f, "{}", t!("export.error.zip", err = err)),
            Self::ProjectNotFound(path) => {
                let path = path.display();
                write!(f, "{}", t!("export.error.project_not_found", path = path))
            }
            Self::InvalidProject(msg) => {
                write!(f, "{}", t!("export.error.invalid_project", msg = msg))
            }
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Zip(err) => Some(err),
            Self::ProjectNotFound(_) | Self::InvalidProject(_) => None,
        }
    }
}

impl From<io::Error> for ExportError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<zip::result::ZipError> for ExportError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

/// 내보낼 프로젝트 원고 묶음.
pub struct ExportProject {
    pub name: String,
    pub dir: PathBuf,
    /// 라이브러리 루트(확장 블록을 그릴 때 `Extensions/`를 찾는다).
    pub root: PathBuf,
    pub book: BookMeta,
    pub styles: Styles,
    pub sheets: Vec<ExportSheet>,
}

/// 내보내기에 들어가는 시트 하나.
pub struct ExportSheet {
    pub path: PathBuf,
    /// 앞 시트에 이어지는지(ADR-0012). 이어지면 새 장으로 분리하지 않고 이전 파일에 연결된다.
    pub glued: bool,
    /// 시트 원문 텍스트.
    pub content: String,
}

/// 프로젝트 폴더에서 내보내기에 필요한 모든 정보를 읽는다.
pub fn load_project(project_dir: &Path) -> Result<ExportProject, ExportError> {
    let root = project_dir.parent().and_then(Path::parent).ok_or_else(|| {
        ExportError::InvalidProject(t!("export.error.library_not_found").into_owned())
    })?;

    let library = library::scan(root)?;
    let project = library
        .find_project(project_dir)
        .ok_or_else(|| ExportError::ProjectNotFound(project_dir.to_path_buf()))?;

    let book = library::read_book(project_dir);
    let styles = library::read_styles(project_dir);
    let manuscript = library::manuscript_sheets(project);

    let mut sheets = Vec::with_capacity(manuscript.len());
    for ms in manuscript {
        let content = library::read_sheet(&ms.path)?;
        sheets.push(ExportSheet {
            path: ms.path,
            glued: ms.glued,
            content,
        });
    }

    Ok(ExportProject {
        name: project.name.clone(),
        dir: project_dir.to_path_buf(),
        root: root.to_path_buf(),
        book,
        styles,
        sheets,
    })
}

/// 미리보기와 컴파일이 공유하는 선택 범위. 접합 묶음은 선택 순서대로 중복 없이 포함한다.
pub fn load_selection(root: &Path, selected: &[PathBuf]) -> Result<ExportProject, ExportError> {
    let first = selected.first().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("export.error.no_selection").into_owned(),
        )
    })?;
    let lib = library::scan(root)?;
    let mut manuscript = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut title = String::new();
    for path in selected {
        let (name, sheets) = selection_sheets(&lib, path)?;
        if title.is_empty() {
            title = if name.trim().is_empty() {
                "빈 시트".to_string()
            } else {
                name
            };
        }
        for sheet in sheets {
            if seen.insert(sheet.path.clone()) {
                manuscript.push(sheet);
            }
        }
    }
    if selected.len() > 1 && manuscript.len() > 1 {
        title = format!("{title} 외 {}개 시트", manuscript.len() - 1);
    }
    let styles = lib
        .project_of(first)
        .map(|project| library::read_styles(&project))
        .unwrap_or_default();
    let mut sheets = Vec::with_capacity(manuscript.len());
    for sheet in manuscript {
        sheets.push(ExportSheet {
            content: library::read_sheet(&sheet.path)?,
            path: sheet.path,
            glued: sheet.glued,
        });
    }
    Ok(ExportProject {
        root: root.to_path_buf(),
        name: title.clone(),
        dir: if first.is_dir() {
            first.to_path_buf()
        } else {
            first.parent().unwrap_or(root).to_path_buf()
        },
        book: BookMeta {
            title,
            ..Default::default()
        },
        styles,
        sheets,
    })
}

fn selection_sheets(
    lib: &library::Library,
    selected: &Path,
) -> Result<(String, Vec<library::ManuscriptSheet>), ExportError> {
    if let Some(group) = lib.find_group(selected) {
        Ok((group.name.clone(), library::manuscript_sheets(group)))
    } else if let Some(group) = selected.parent().and_then(|dir| lib.find_group(dir)) {
        let index = group
            .sheets
            .iter()
            .position(|sheet| sheet.path == selected)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    t!("export.error.sheet_not_found").into_owned(),
                )
            })?;
        let mut start = index;
        while start > 0 && group.sheets[start].glued {
            start -= 1;
        }
        let mut end = index + 1;
        while end < group.sheets.len() && group.sheets[end].glued {
            end += 1;
        }
        let sheets = group.sheets[start..end]
            .iter()
            .enumerate()
            .map(|(index, sheet)| library::ManuscriptSheet {
                path: sheet.path.clone(),
                glued: index > 0,
            })
            .collect();
        Ok((
            library::read_preview(&group.sheets[start].path).title,
            sheets,
        ))
    } else {
        if !selected.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                t!("export.error.sheet_not_found").into_owned(),
            )
            .into());
        }
        Ok((
            library::read_preview(selected).title,
            vec![library::ManuscriptSheet {
                path: selected.to_path_buf(),
                glued: false,
            }],
        ))
    }
}
