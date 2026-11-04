//! 라이브러리 폴더 감시.
//!
//! 라이브러리 루트를 재귀로 감시하고, 변경이 생기면 바뀐 경로들을 보낸다. 받는 쪽이 짧게 모았다가
//! (디바운스) 다시 스캔한다. 목록은 전부 다시 스캔하고, 할 일 색인(ADR-0024)은 바뀐 경로만 다시 읽는다.

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

use crate::library::{MARKER_DIR, STT_DIR};

/// `root` 감시를 시작한다. 반환한 watcher를 버리면 감시가 멈춘다.
///
/// 보내는 경로는 `root` 아래 경로로 고쳐 보낸다(이벤트는 정규화된 경로로 온다). 경로가 없는 이벤트는
/// 빈 목록이다.
pub fn watch(root: &Path) -> notify::Result<(RecommendedWatcher, UnboundedReceiver<Vec<PathBuf>>)> {
    let (tx, rx) = unbounded();
    // 앱 내부 상태(.hanji/)와 받아쓰기 기록(STT/, 녹음 중 계속 덧붙는다)의 변경은 목록과 무관하므로 무시한다.
    // 이벤트 경로는 정규화된 경로(macOS: /private/var …)로 오므로 같은 형태로 비교한다.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let ignored = [canonical.join(MARKER_DIR), canonical.join(STT_DIR)];
    let given = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            return;
        };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if !event.paths.is_empty()
            && event
                .paths
                .iter()
                .all(|path| ignored.iter().any(|dir| path.starts_with(dir)))
        {
            return;
        }
        let paths = event
            .paths
            .into_iter()
            .map(|path| match path.strip_prefix(&canonical) {
                Ok(rest) => given.join(rest),
                Err(_) => path,
            })
            .collect();
        let _ = tx.unbounded_send(paths);
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok((watcher, rx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// `path`가 담긴 신호가 올 때까지 최대 `timeout` 동안 기다린다.
    fn wait_path(rx: &mut UnboundedReceiver<Vec<PathBuf>>, path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            while let Ok(paths) = rx.try_recv() {
                if paths.iter().any(|changed| changed == path) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn reports_deleted_sheet_under_given_root() {
        // macOS의 임시 폴더(/var → /private/var)처럼 정규화하면 바뀌는 경로를 그대로 준다.
        // 이벤트는 정규화된 경로로 오지만, 받는 쪽에는 준 루트 아래 경로로 와야 한다.
        let root = std::env::temp_dir().join(format!("hanji-watch-{}", std::process::id()));
        let group = root.join("Notes").join("Inbox");
        std::fs::create_dir_all(&group).unwrap();
        let sheet = group.join("a.md");
        std::fs::write(&sheet, "a").unwrap();

        let (_watcher, mut rx) = watch(&root).unwrap();
        // 감시 시작 직후의 잔여 이벤트를 비운다.
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}

        std::fs::remove_file(&sheet).unwrap();
        assert!(wait_path(&mut rx, &sheet, Duration::from_secs(5)));

        std::fs::remove_dir_all(&root).ok();
    }
}
