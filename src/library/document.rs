//! 시트 본문 읽기/저장과 단어 수.

use std::io;
use std::path::Path;

use super::modified_nanos;

/// 시트 본문을 읽는다. 앞의 BOM은 떼어 낸다.
pub fn read_sheet(path: &Path) -> io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    Ok(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    })
}

/// 시트 본문을 원자적으로 저장하고, 저장 후의 수정 시각을 돌려준다.
///
/// 같은 폴더의 숨김 임시 파일(`.<이름>.hanji-tmp`)에 쓰고 디스크에 내린 뒤 바꿔치기한다.
/// 중간에 끊겨도 원래 파일은 온전하다. 임시 파일은 `.md`가 아니라 스캔에 잡히지 않는다.
pub fn write_sheet(path: &Path, text: &str) -> io::Result<u128> {
    use std::io::Write as _;
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.no_parent").into_owned(),
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.no_file_name").into_owned(),
        )
    })?;
    let temp = dir.join(format!(".{}.hanji-tmp", name.to_string_lossy()));

    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result?;
    Ok(modified_nanos(path))
}

/// 단어 수: 공백으로 나눈 조각 중 글자나 숫자가 하나라도 있는 것.
///
/// `#`, `-`, `**` 같은 마크업만 있는 조각은 세지 않는다. 한국어는 어절 단위로 센다.
pub fn count_words(chars: impl IntoIterator<Item = char>) -> usize {
    let mut count = 0;
    let mut in_word = false;
    let mut has_alnum = false;
    for c in chars {
        if c.is_whitespace() {
            if in_word && has_alnum {
                count += 1;
            }
            in_word = false;
            has_alnum = false;
        } else {
            in_word = true;
            has_alnum |= c.is_alphanumeric();
        }
    }
    if in_word && has_alnum {
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;

    #[test]
    fn write_then_read_roundtrip_without_temp_leftover() {
        let dir = temp_dir("doc");
        let path = dir.join("a.md");
        std::fs::write(&path, "old").unwrap();

        let modified = write_sheet(&path, "# 제목\n본문").unwrap();
        assert!(modified > 0);
        assert_eq!(read_sheet(&path).unwrap(), "# 제목\n본문");
        assert!(!dir.join(".a.md.hanji-tmp").exists());

        std::fs::write(&path, "\u{feff}bom").unwrap();
        assert_eq!(read_sheet(&path).unwrap(), "bom");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn count_words_skips_markup_only_tokens() {
        assert_eq!(count_words("".chars()), 0);
        assert_eq!(count_words("  \n ".chars()), 0);
        assert_eq!(count_words("# 안녕 여러분\n\n- 하나 **둘**".chars()), 4);
        assert_eq!(count_words("hello, world! 2026".chars()), 3);
        assert_eq!(count_words("---\n***".chars()), 0);
    }
}
