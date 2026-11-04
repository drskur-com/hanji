//! 라이브러리별 로컬 캐시(redb, ADR-0024).
//!
//! 원문에서 다시 만들 수 있는 파생 데이터만 둔다. 동기화하지 않도록 라이브러리 폴더가 아니라 OS 캐시 폴더
//! (`<cache_dir>/Hanji/<라이브러리 키>/cache.redb`)에 두고, 지워지거나 깨지면 새로 만든다.
//! 쓰는 곳마다 자기 테이블을 정의한다(예: `library::TodoIndex`).

use std::path::{Path, PathBuf};

use redb::{Database, DatabaseError};

/// 캐시 파일 이름.
const FILE_NAME: &str = "cache.redb";

pub struct Cache {
    db: Database,
}

impl Cache {
    /// 라이브러리의 캐시를 연다. 캐시 폴더를 모르거나 다른 창이 이미 열었으면 `None`
    /// (캐시 없이 원문만 읽는다).
    pub fn open(library_root: &Path) -> Option<Self> {
        Self::open_at(&cache_path(library_root)?)
    }

    /// `path`의 캐시 파일을 연다. 옛 형식이거나 깨졌으면 지우고 새로 만든다.
    pub fn open_at(path: &Path) -> Option<Self> {
        std::fs::create_dir_all(path.parent()?).ok()?;
        match Database::create(path) {
            Ok(db) => Some(Self { db }),
            Err(DatabaseError::DatabaseAlreadyOpen) => None,
            Err(_) => {
                std::fs::remove_file(path).ok()?;
                Database::create(path).ok().map(|db| Self { db })
            }
        }
    }

    pub fn db(&self) -> &Database {
        &self.db
    }
}

/// 라이브러리 캐시 파일 경로. 라이브러리 경로마다 따로 둔다.
fn cache_path(library_root: &Path) -> Option<PathBuf> {
    let root = library_root
        .canonicalize()
        .unwrap_or_else(|_| library_root.to_path_buf());
    Some(
        dirs::cache_dir()?
            .join("Hanji")
            .join(library_key(&root))
            .join(FILE_NAME),
    )
}

/// 경로의 안정적인 키(FNV-1a 64비트, 16진수). 표준 해셔는 Rust 버전마다 달라질 수 있어 쓰지 않는다.
pub(crate) fn library_key(root: &Path) -> String {
    let hash = root
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_key_is_stable() {
        assert_eq!(library_key(Path::new("")), "cbf29ce484222325");
        assert_eq!(library_key(Path::new("a")), "af63dc4c8601ec8c");
        assert_ne!(library_key(Path::new("/a")), library_key(Path::new("/b")));
    }

    #[test]
    fn broken_cache_file_is_replaced() {
        let dir = std::env::temp_dir().join(format!("hanji-cache-{}", std::process::id()));
        let path = dir.join("broken").join(FILE_NAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a database").unwrap();
        assert!(Cache::open_at(&path).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
