//! 받아 적은 문단을 기록 파일에 쓰는 스레드(ADR-0054).
//!
//! 녹음 중 기록 파일에 쓰는 것은 이 스레드 하나뿐이다. 받아 적은 원문을 받는 대로 덧붙인다.

use std::io;
use std::path::{Path, PathBuf};

/// 문단이 끝날 때까지(채널이 닫힐 때까지) 받은 문단을 기록 끝에 덧붙이고 `written`으로 알린다.
/// 쓰기에 실패하면 그 까닭을 돌려준다.
pub(super) fn write_out(
    mut record: Record,
    paragraphs: impl IntoIterator<Item = String>,
    mut written: impl FnMut(String),
) -> io::Result<()> {
    for paragraph in paragraphs {
        record.append(&paragraph)?;
        written(paragraph);
    }
    Ok(())
}

/// 기록 파일: 에디터 본문의 줄(줄 하나가 문단, ADR-0011)을 들고 있다가 문단을 덧붙일 때마다 마크다운으로
/// 원자적으로 다시 쓴다. 파일은 표준 마크다운이라 문단 사이에 빈 줄이 들어간다(빈 줄 없이 이으면 다시 열 때 한 문단으로
/// 합쳐진다). 녹음 중에는 이 스레드만 쓰므로 바꿔치기해도 잃는 글이 없다.
pub(super) struct Record {
    path: PathBuf,
    lines: Vec<String>,
}

impl Record {
    /// 기록 파일을 읽어 연다(없으면 빈 기록).
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let text = match crate::library::read_sheet(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        let lines = crate::text_transform::to_editor(&text)
            .lines()
            .map(str::to_string)
            .collect();
        Ok(Self {
            path: path.to_path_buf(),
            lines,
        })
    }

    /// 문단을 끝에 한 줄로 덧붙인다.
    pub(super) fn append(&mut self, paragraph: &str) -> io::Result<()> {
        self.lines.push(paragraph.to_string());
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let body = self.lines.join("\n") + "\n";
        crate::library::write_sheet(&self.path, &crate::text_transform::to_markdown(&body))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hanji-stt-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("a.md")
    }

    #[test]
    fn record_appends_markdown_paragraphs() {
        let path = temp("record");
        std::fs::write(&path, "# 제목\n").unwrap();
        let mut record = Record::open(&path).unwrap();
        record.append("첫 문단.").unwrap();
        record.append("둘째.").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# 제목\n\n첫 문단.\n\n둘째.\n");
        assert_eq!(
            crate::text_transform::to_editor(&text),
            "# 제목\n첫 문단.\n둘째.\n"
        );
        // 다시 열어 이어 써도 문단이 합쳐지지 않는다.
        Record::open(&path).unwrap().append("셋째.").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# 제목\n\n첫 문단.\n\n둘째.\n\n셋째.\n"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn writes_each_paragraph() {
        let path = temp("plain");
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send("하나".to_string()).unwrap();
        tx.send("둘".to_string()).unwrap();
        drop(tx);
        let mut written = Vec::new();
        write_out(Record::open(&path).unwrap(), rx, |text| written.push(text)).unwrap();
        assert_eq!(written, ["하나", "둘"]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "하나\n\n둘\n");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
