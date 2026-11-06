//! 아이콘 팩 매핑: 도메인 개념 → [`IconName`].
//!
//! 아이콘 세트는 gpui-kit-assets 의 **Lucide** 카탈로그를 사용한다(에셋 소스는
//! `AllAssets`). 화면 코드는 여기 정의된 함수만 호출하고, 아이콘을 바꾸려면
//! 이 파일만 수정하면 된다. (도메인/뷰가 특정 아이콘에 직접 의존하지 않도록 분리)

use gpui_kit::assets::IconName;

/// 사용 중인 아이콘 팩 이름 (설정/About 표기용).
pub const PACK: &str = "Lucide";

/// 문자열 아이콘 이름을 [`IconName`] 으로 해석한다. 모르면 폴더.
///
/// 짧은 별칭(inbox, notebook, book, file, folder)을 먼저 보고, 아니면 Lucide 파일 이름
/// (예: `book-open`)으로 카탈로그에서 찾는다.
pub fn by_name(name: &str) -> IconName {
    match name {
        "inbox" => IconName::Inbox,
        "notebook" => IconName::NotebookPen,
        "book" => IconName::BookOpen,
        "file" => IconName::FileText,
        "folder" => IconName::Folder,
        _ => catalog()
            .binary_search_by(|(stem, _)| stem.as_str().cmp(name))
            .map(|ix| catalog()[ix].1)
            .unwrap_or(IconName::Folder),
    }
}

/// 번들된 아이콘 전체: (Lucide 파일 이름, 아이콘), 이름순. 아이콘 고르기·이름 찾기에 쓴다.
pub fn catalog() -> &'static [(String, IconName)] {
    static CATALOG: std::sync::OnceLock<Vec<(String, IconName)>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut icons: Vec<_> = IconName::ALL
            .iter()
            .map(|&icon| {
                let path = icon.path();
                let stem = path
                    .trim_start_matches("icons/")
                    .trim_end_matches(".svg")
                    .to_string();
                (stem, icon)
            })
            .collect();
        icons.sort_by(|a, b| a.0.cmp(&b.0));
        icons
    })
}

/// 편집(프로젝트 우클릭 메뉴).
pub fn edit() -> IconName {
    IconName::Pencil
}

/// 책 정보(프로젝트 더 보기 메뉴).
pub fn book_info() -> IconName {
    IconName::BookText
}

/// 프로젝트 미리보기(프로젝트 도구 줄).
pub fn book_preview() -> IconName {
    IconName::Eye
}

/// 미리보기 차례 메뉴.
pub fn toc() -> IconName {
    IconName::TableOfContents
}

/// 미리보기 확대·축소.
pub fn zoom_in() -> IconName {
    IconName::ZoomIn
}

pub fn zoom_out() -> IconName {
    IconName::ZoomOut
}

/// 책 스타일(프로젝트 더 보기 메뉴).
pub fn book_style() -> IconName {
    IconName::Type
}

/// 목록에서 들어가기·나오기(책 스타일 대상 카드, 스타일 목록으로 돌아가기).
pub fn enter() -> IconName {
    IconName::ChevronRight
}

pub fn leave() -> IconName {
    IconName::ChevronLeft
}

/// 이미지(표지 미리보기 자리).
pub fn image() -> IconName {
    IconName::Image
}

/// 동영상(삽입 대화상자·에디터 안내).
pub fn video() -> IconName {
    IconName::Video
}

/// 이미지의 글자 인식(OCR 텍스트 삽입 대화상자).
pub fn scan_text() -> IconName {
    IconName::ScanText
}

/// 오디오(삽입 대화상자·에디터의 오디오 카드).
pub fn audio() -> IconName {
    IconName::Music
}

/// 재생(동영상·오디오 재생 단추).
pub fn play() -> IconName {
    IconName::Play
}

/// 일시 정지.
pub fn pause() -> IconName {
    IconName::Pause
}

/// 첨부파일 그룹(이미지 삽입 대화상자의 "첨부파일" 탭). 그룹 메타 아이콘과 같다.
pub fn attachments() -> IconName {
    by_name("paperclip")
}

/// 파일 그룹의 파일 종류.
pub fn file_kind(kind: crate::library::FileKind) -> IconName {
    use crate::library::FileKind;
    match kind {
        FileKind::Image => IconName::FileImage,
        FileKind::Video => IconName::FileVideoCamera,
        FileKind::Audio => IconName::FileVolume,
        FileKind::Text => IconName::FileText,
        FileKind::Pdf => IconName::FileText,
        FileKind::Other => IconName::File,
    }
}

/// 기본 앱으로 열기(파일 뷰어 툴바).
pub fn open_external() -> IconName {
    IconName::SquareArrowOutUpRight
}

/// Finder에서 보기(파일 뷰어 툴바).
pub fn reveal() -> IconName {
    IconName::FolderSearch
}

/// 파일 정보(파일 뷰어 툴바).
pub fn info() -> IconName {
    IconName::Info
}

/// 휴지통으로 이동(프로젝트 우클릭 메뉴).
pub fn trash() -> IconName {
    IconName::Trash
}

/// 아이콘 더 보기(…).
pub fn more() -> IconName {
    IconName::Ellipsis
}

/// 검색.
pub fn search_field() -> IconName {
    IconName::Search
}

/// 새 프로젝트에서 고를 수 있는 아이콘(메타 이름). 첫 항목이 기본.
pub const PROJECT_ICONS: [&str; 4] = ["book", "notebook", "file", "folder"];

/// 그룹 아이콘: 메타 이름이 있으면 그걸, 없으면 기본 폴더.
pub fn group_icon(name: Option<&str>) -> IconName {
    name.map(by_name).unwrap_or(IconName::Folder)
}

/// 설정 창의 "일반" 페이지.
pub fn settings() -> IconName {
    IconName::Settings
}

/// 사이드바 토글 아이콘(패널).
pub fn panel() -> IconName {
    IconName::PanelLeft
}

/// 어시스턴트(LLM) 표시와 오른쪽 어시스턴트 패널 토글.
pub fn assistant() -> IconName {
    IconName::Sparkles
}

/// 설정 창의 "AI" 페이지.
pub fn ai_settings() -> IconName {
    IconName::Sparkles
}

/// 설정 창의 "받아쓰기" 페이지.
pub fn dictation() -> IconName {
    IconName::Mic
}

/// 받아쓰기 소리: 마이크.
pub fn microphone() -> IconName {
    IconName::Mic
}

/// 받아쓰기 소리: 시스템 소리(다른 앱이 내는 소리).
pub fn system_audio() -> IconName {
    IconName::Volume2
}

/// 닫기.
pub fn close() -> IconName {
    IconName::X
}

/// 받아쓰기 요약(ADR-0055).
pub fn summary() -> IconName {
    IconName::ListChecks
}

/// 내려받기.
pub fn download() -> IconName {
    IconName::Download
}

/// 받아쓰기 모델 카드: 내려받음 표시.
pub fn downloaded() -> IconName {
    IconName::Check
}

/// 설정 → AI → Claude Code: 찾음.
pub fn found() -> IconName {
    IconName::CircleCheck
}

/// 설정 → AI → Claude Code: 찾지 못함.
pub fn not_found() -> IconName {
    IconName::CircleX
}

/// 설정 → AI → Claude Code: 찾는 중.
pub fn detecting() -> IconName {
    IconName::LoaderCircle
}

/// 받아쓰기 모델 카드: 언어 수.
pub fn languages() -> IconName {
    IconName::Globe
}

/// 받아쓰기 모델 카드: 스트리밍(말하는 대로 받아씀).
pub fn streaming() -> IconName {
    IconName::AudioLines
}

/// 받아쓰기 모델 카드: 디스크에 차지하는 크기.
pub fn disk() -> IconName {
    IconName::HardDrive
}

/// 검토 점수의 별: 가득, 반, 빈 별.
pub fn star_full() -> IconName {
    IconName::StarFill
}

pub fn star_half() -> IconName {
    IconName::StarHalf
}

pub fn star_empty() -> IconName {
    IconName::Star
}

/// 어시스턴트 대화 기록.
pub fn history() -> IconName {
    IconName::Clock
}

/// 어시스턴트 답 받기 멈추기.
pub fn stop() -> IconName {
    IconName::Square
}

/// 어시스턴트에게 보내기.
pub fn send() -> IconName {
    IconName::ArrowUp
}

/// 메모 "모두" 보기.
pub fn all_sheets() -> IconName {
    IconName::Layers
}

/// 메모의 할 일을 모은 보기(ADR-0024).
pub fn todos() -> IconName {
    IconName::ListTodo
}

/// 프로젝트 화면 → 라이브러리로 돌아가기.
pub fn back_to_library() -> IconName {
    IconName::ChevronsLeft
}

/// 프로젝트 빌드(PDF/ePub).
pub fn build() -> IconName {
    IconName::FileOutput
}

/// 하위 그룹 펼침/접힘 표시.
pub fn disclosure(expanded: bool) -> IconName {
    if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    }
}

/// 프로젝트 원고 그룹·추가자료 그룹의 기본 아이콘(메타 이름).
pub const MANUSCRIPT_GROUP_ICON: &str = "files";
pub const EXTRAS_GROUP_ICON: &str = "folder";

/// 새 그룹.
pub fn new_group() -> IconName {
    IconName::FolderPlus
}

/// 시트 목록 필터/정렬.
pub fn filter() -> IconName {
    IconName::ListFilter
}

/// 시트 정렬 방법 고르기.
pub fn sort() -> IconName {
    IconName::ArrowDownUp
}

/// 새 시트 작성.
pub fn compose() -> IconName {
    IconName::SquarePen
}

/// 탐색: 뒤로/앞으로.
pub fn back() -> IconName {
    IconName::ChevronLeft
}

pub fn forward() -> IconName {
    IconName::ChevronRight
}

/// 검색.
pub fn search() -> IconName {
    IconName::Search
}

pub fn link() -> IconName {
    IconName::Link
}

/// 시트 접합.
pub fn glue() -> IconName {
    IconName::Link2
}

/// 시트 접합 해제.
pub fn unglue() -> IconName {
    IconName::Unlink2
}

pub fn paragraph() -> IconName {
    IconName::Pilcrow
}

/// 에디터 할 일 항목: 완료한 체크 상자 안 표시.
pub fn task_done() -> IconName {
    IconName::Check
}

pub fn export() -> IconName {
    IconName::FileUp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_resolves_aliases_and_lucide_names() {
        assert_eq!(by_name("book"), IconName::BookOpen);
        assert_eq!(by_name("newspaper"), IconName::Newspaper);
        assert_eq!(by_name("없는-아이콘"), IconName::Folder);
        assert_eq!(by_name(MANUSCRIPT_GROUP_ICON), IconName::Files);
        assert_eq!(by_name("layout-template"), IconName::LayoutTemplate);
        assert!(catalog().len() > 1000);
    }
}
