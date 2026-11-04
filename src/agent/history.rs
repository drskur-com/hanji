//! 어시스턴트 대화 기록(redb, ADR-0031).
//!
//! 대화는 다시 만들 수 없으므로 지워도 되는 캐시(`crate::cache`)와 따로, 동기화하지 않는 OS 데이터 폴더
//! (`<data_dir>/Hanji/<라이브러리 키>/conversations.redb`)에 둔다. 깨졌어도 지우지 않는다.
//! 기록 목록은 요약(`META`)만 읽고, 말(`TURNS`)은 대화를 열 때 읽는다. 말은 대화마다 순번을 키로 두어
//! 답이 끝날 때 바뀐 말만 쓴다.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use redb::{Database, ReadableDatabase as _, ReadableTable as _, TableDefinition};
use serde::{Deserialize, Serialize};

use super::Turn;

/// 기록 파일 이름.
const FILE_NAME: &str = "conversations.redb";

/// 대화 id → 요약(JSON).
const META: TableDefinition<u64, &[u8]> = TableDefinition::new("conversations");
/// (대화 id, 순번) → 말(JSON).
const TURNS: TableDefinition<(u64, u32), &[u8]> = TableDefinition::new("turns");

/// 기록 목록에 보일 대화 요약.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// 만든 때(유닉스 밀리초)를 바탕으로 한 번호. 대화마다 다르다.
    pub id: u64,
    /// 첫 사용자 말의 첫 줄.
    pub title: String,
    /// 마지막 말의 첫 줄.
    pub preview: String,
    /// 만든 때와 마지막으로 말을 주고받은 때(유닉스 밀리초).
    pub created: u64,
    pub updated: u64,
    /// 이 대화에서 말할 때 열려 있던 시트들(라이브러리 루트 기준, 처음 나온 순서).
    #[serde(default)]
    pub sheets: Vec<String>,
    /// 읽기를 허락한 사이트.
    #[serde(default)]
    pub sites: Vec<String>,
}

impl Summary {
    /// 말들에서 제목·미리보기·시트를 채운다.
    pub fn describe(&mut self, turns: &[Turn]) {
        self.title = turns
            .iter()
            .find(|turn| turn.user)
            .map(|turn| first_line(&turn.text))
            .unwrap_or_default();
        self.preview = turns
            .iter()
            .rev()
            .find(|turn| !turn.text.trim().is_empty())
            .map(|turn| first_line(&turn.text))
            .unwrap_or_default();
        self.sheets.clear();
        for sheet in turns.iter().filter_map(|turn| turn.sheet.as_ref()) {
            if !self.sheets.contains(sheet) {
                self.sheets.push(sheet.clone());
            }
        }
    }
}

/// 라이브러리 하나의 대화 기록.
pub struct History {
    db: Database,
    /// 마지막으로 준 대화 번호. 같은 밀리초에 만들어도 겹치지 않게 한다.
    last_id: Mutex<u64>,
}

impl History {
    /// 라이브러리의 대화 기록을 연다(없으면 만든다).
    pub fn open(library_root: &Path) -> Result<Self, String> {
        let path = history_path(library_root).ok_or("no data folder")?;
        Self::open_at(&path)
    }

    /// `path`의 기록 파일을 연다. 열지 못해도 파일은 그대로 둔다.
    pub fn open_at(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        }
        let db = Database::create(path).map_err(|err| err.to_string())?;
        let history = Self {
            db,
            last_id: Mutex::new(0),
        };
        let last = history.read(|txn| {
            Ok(txn
                .open_table(META)?
                .last()?
                .map_or(0, |(id, _)| id.value()))
        });
        *history
            .last_id
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = last.unwrap_or(0);
        Ok(history)
    }

    /// 새 대화 번호.
    pub fn next_id(&self) -> u64 {
        let mut last = self.last_id.lock().unwrap_or_else(|err| err.into_inner());
        *last = now_millis().max(*last + 1);
        *last
    }

    /// 모든 대화의 요약. 마지막으로 주고받은 때가 늦은 것부터.
    pub fn list(&self) -> Vec<Summary> {
        let mut summaries = self
            .read(|txn| {
                let table = txn.open_table(META)?;
                let mut summaries = Vec::new();
                for entry in table.iter()? {
                    let (_, value) = entry?;
                    if let Ok(summary) = serde_json::from_slice::<Summary>(value.value()) {
                        summaries.push(summary);
                    }
                }
                Ok(summaries)
            })
            .unwrap_or_default();
        summaries.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
        summaries
    }

    /// 대화의 말들. 없으면 `None`.
    pub fn load(&self, id: u64) -> Option<Vec<Turn>> {
        self.read(|txn| {
            if txn.open_table(META)?.get(id)?.is_none() {
                return Ok(None);
            }
            let table = txn.open_table(TURNS)?;
            let mut turns = Vec::new();
            for entry in table.range((id, 0)..=(id, u32::MAX))? {
                let (_, value) = entry?;
                turns.push(serde_json::from_slice(value.value()).unwrap_or_default());
            }
            Ok(Some(turns))
        })
        .ok()
        .flatten()
    }

    /// 대화를 쓴다. `turns[from..]`만 다시 쓰고, `turns`보다 뒤에 남은 옛 말은 지운다.
    pub fn save(&self, summary: &Summary, turns: &[Turn], from: usize) -> Result<(), String> {
        let write = || -> Result<(), redb::Error> {
            let txn = self.db.begin_write()?;
            {
                let mut meta = txn.open_table(META)?;
                let json = serde_json::to_vec(summary).unwrap_or_default();
                meta.insert(summary.id, json.as_slice())?;
                let mut table = txn.open_table(TURNS)?;
                for (ix, turn) in turns.iter().enumerate().skip(from) {
                    let json = serde_json::to_vec(turn).unwrap_or_default();
                    table.insert((summary.id, index(ix)), json.as_slice())?;
                }
                table.retain_in(
                    (summary.id, index(turns.len()))..=(summary.id, u32::MAX),
                    |_, _| false,
                )?;
            }
            txn.commit()?;
            Ok(())
        };
        write().map_err(|err| err.to_string())
    }

    /// 대화를 지운다.
    pub fn delete(&self, id: u64) -> Result<(), String> {
        let write = || -> Result<(), redb::Error> {
            let txn = self.db.begin_write()?;
            {
                txn.open_table(META)?.remove(id)?;
                txn.open_table(TURNS)?
                    .retain_in((id, 0)..=(id, u32::MAX), |_, _| false)?;
            }
            txn.commit()?;
            Ok(())
        };
        write().map_err(|err| err.to_string())
    }

    /// 읽기 트랜잭션. 아직 쓴 적이 없어 테이블이 없으면 기본값을 돌려준다.
    fn read<T: Default>(
        &self,
        f: impl FnOnce(&redb::ReadTransaction) -> Result<T, redb::Error>,
    ) -> Result<T, redb::Error> {
        let txn = self.db.begin_read()?;
        match f(&txn) {
            Err(redb::Error::TableDoesNotExist(_)) => Ok(T::default()),
            result => result,
        }
    }
}

/// 라이브러리 기록 파일 경로. 라이브러리 경로마다 따로 둔다.
fn history_path(library_root: &Path) -> Option<PathBuf> {
    let root = library_root
        .canonicalize()
        .unwrap_or_else(|_| library_root.to_path_buf());
    Some(
        dirs::data_dir()?
            .join("Hanji")
            .join(crate::cache::library_key(&root))
            .join(FILE_NAME),
    )
}

fn index(ix: usize) -> u32 {
    u32::try_from(ix).unwrap_or(u32::MAX)
}

/// 비어 있지 않은 첫 줄(목록 한 줄에 들어갈 만큼).
fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    line.unwrap_or("").chars().take(120).collect()
}

/// 지금(유닉스 밀리초).
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user: bool, text: &str, sheet: Option<&str>) -> Turn {
        Turn {
            user,
            text: text.into(),
            sheet: sheet.map(Into::into),
            ..Turn::default()
        }
    }

    fn open(name: &str) -> (History, PathBuf) {
        let dir = std::env::temp_dir().join(format!("hanji-history-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let history = History::open_at(&dir.join(FILE_NAME)).unwrap();
        (history, dir)
    }

    #[test]
    fn empty_history_lists_nothing() {
        let (history, dir) = open("empty");
        assert!(history.list().is_empty());
        assert_eq!(history.load(1), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saves_appends_and_trims_turns() {
        let (history, dir) = open("save");
        let mut turns = vec![
            turn(true, "\n1장 줄여 줘\n자세히", Some("Projects/a/1.md")),
            turn(false, "줄였어요.", None),
        ];
        let mut summary = Summary {
            id: history.next_id(),
            created: 1,
            updated: 1,
            ..Summary::default()
        };
        summary.describe(&turns);
        assert_eq!(summary.title, "1장 줄여 줘");
        assert_eq!(summary.preview, "줄였어요.");
        assert_eq!(summary.sheets, ["Projects/a/1.md"]);
        history.save(&summary, &turns, 0).unwrap();

        turns.push(turn(true, "또", Some("Projects/a/2.md")));
        turns.push(turn(false, "네", None));
        history.save(&summary, &turns, 2).unwrap();
        assert_eq!(history.load(summary.id).unwrap(), turns);

        // 줄어든 대화는 뒤에 남은 옛 말을 지운다.
        turns.truncate(3);
        history.save(&summary, &turns, 3).unwrap();
        assert_eq!(history.load(summary.id).unwrap(), turns);

        // 다른 대화는 따로 두고, 최근 것부터 보인다.
        let other = Summary {
            id: history.next_id(),
            updated: 2,
            ..Summary::default()
        };
        assert!(other.id > summary.id);
        history
            .save(&other, &[turn(true, "다른", None)], 0)
            .unwrap();
        let ids: Vec<u64> = history.list().iter().map(|s| s.id).collect();
        assert_eq!(ids, [other.id, summary.id]);

        history.delete(summary.id).unwrap();
        assert_eq!(history.load(summary.id), None);
        assert_eq!(history.load(other.id).unwrap().len(), 1);
        drop(history);

        // 다시 열어도 남아 있고, 번호는 이어서 준다.
        let reopened = History::open_at(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(reopened.list().len(), 1);
        assert!(reopened.next_id() > other.id);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_file_is_kept() {
        let (history, dir) = open("broken");
        drop(history);
        let path = dir.join("bad.redb");
        std::fs::write(&path, b"not a database").unwrap();
        assert!(History::open_at(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not a database");
        std::fs::remove_dir_all(&dir).ok();
    }
}
