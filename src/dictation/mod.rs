//! 받아쓰기(ADR-0054): 음성 인식 모델 내려받기, Nemotron 인식기(`recognizer`), 받아 적을 소리(`capture`).
//! 화면은 메인 창의 받아쓰기 그룹이다(`app::dictation`).
//!
//! 말하는 동안 받아 적고 문단을 나누는 엔진([`Engine`])은 macOS 받아쓰기(`apple`, 기기 안) 또는
//! [`NEMOTRON`](NVIDIA Nemotron 3.5 ASR Streaming 0.6B)이다. [`COHERE`](Cohere Transcribe 03-2026)는
//! 스트리밍이 없는 대신 정확해, 켜면 엔진이 나눈 문단의 소리를 다시 받아 적는다(ADR-0056). 녹음 한 번의
//! 소리 남기기·다시 받아쓰기·기록 쓰기는 `pipeline`이 맡는다.
//!
//! 모델은 handy-computer가 transcribe.cpp용으로 바꾼 GGUF다.
//! Hugging Face 캐시(`HF_HUB_CACHE` → `$HF_HOME/hub` → `~/.cache/huggingface/hub`)에 내려받아
//! `hf` CLI·Handy 같은 다른 앱과 같은 파일을 함께 쓴다. 이미 캐시에 있으면 내려받지 않는다.
//!
//! 내려받기는 앱 전역 [`ModelStore`]가 맡아 설정 창을 닫아도 이어진다.

#[cfg(target_os = "macos")]
/// 받아쓰기를 화면에 보일지. 끄면 파일 메뉴의 받아쓰기, 사이드바의 받아쓰기 줄, 설정의 받아쓰기 페이지,
/// 시트 찾기의 받아쓰기 기록이 숨는다. 코드와 라이브러리의 `STT/` 폴더는 그대로 둔다.
pub const ENABLED: bool = false;

mod apple;
mod capture;
mod output;
mod pipeline;
mod recognizer;
mod wav;

#[cfg(target_os = "macos")]
pub use apple::AppleRecognizer;
pub use capture::{AudioSource, SourceInput};
pub use pipeline::{Event as RecordEvent, EventHandler, RecordingSetup, Refiner};
pub use recognizer::{NemotronRecognizer, create_record, ensure_record_title};
pub use wav::audio_path;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::gpui::*;
use hf_hub::progress::{DownloadEvent as HubEvent, Progress, ProgressEvent, ProgressHandler};
use serde::{Deserialize, Serialize};

/// 내려받는 음성 인식 모델 하나. 양자화는 고르게 하지 않고 하나만 쓴다.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ModelSpec {
    /// Hugging Face 저장소(`owner/name`).
    pub owner: &'static str,
    pub repo: &'static str,
    /// 내려받는 파일.
    pub file: &'static str,
    /// 파일 크기(바이트). 내려받기 전에 보이고, 받는 중 전체 크기를 모를 때 쓴다.
    pub size: u64,
    /// 받아쓰는 언어 수(한국어 포함).
    pub languages: usize,
}

/// 말하는 대로 받아 적는 스트리밍 모델. 받아쓰기에 꼭 있어야 한다. Q8_0은 정확도가 원본(F32)과 같다.
pub const NEMOTRON: ModelSpec = ModelSpec {
    owner: "handy-computer",
    repo: "nemotron-3.5-asr-streaming-0.6b-gguf",
    file: "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf",
    size: 751_094_240,
    languages: 32,
};

/// 문단을 다시 받아 적는 모델(스트리밍 없음, 언어를 정해 준다).
pub const COHERE: ModelSpec = ModelSpec {
    owner: "handy-computer",
    repo: "cohere-transcribe-03-2026-gguf",
    file: "cohere-transcribe-03-2026-Q5_K_M.gguf",
    size: 1_770_270_208,
    languages: DICTATION_LANGUAGES.len(),
};

/// 받아쓰는 언어 하나(ADR-0056). macOS 받아쓰기와 Cohere가 쓴다(Nemotron은 언어를 스스로 알아낸다).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DictationLanguage {
    /// 설정에 저장하는 코드(Cohere의 언어 코드).
    pub code: &'static str,
    /// 고르기에 보이는 이름.
    pub native: &'static str,
    /// macOS 받아쓰기(`SFSpeechRecognizer`)의 로캘.
    pub locale: &'static str,
}

const fn language(
    code: &'static str,
    native: &'static str,
    locale: &'static str,
) -> DictationLanguage {
    DictationLanguage {
        code,
        native,
        locale,
    }
}

/// 받아쓰는 언어: Cohere가 받아쓰는 14개 언어.
pub const DICTATION_LANGUAGES: &[DictationLanguage] = &[
    language("ko", "한국어", "ko-KR"),
    language("en", "English", "en-US"),
    language("ja", "日本語", "ja-JP"),
    language("zh", "中文", "zh-CN"),
    language("vi", "Tiếng Việt", "vi-VN"),
    language("es", "Español", "es-ES"),
    language("fr", "Français", "fr-FR"),
    language("de", "Deutsch", "de-DE"),
    language("it", "Italiano", "it-IT"),
    language("pt", "Português", "pt-BR"),
    language("nl", "Nederlands", "nl-NL"),
    language("pl", "Polski", "pl-PL"),
    language("el", "Ελληνικά", "el-GR"),
    language("ar", "العربية", "ar-SA"),
];

impl DictationLanguage {
    pub fn find(code: &str) -> Option<&'static DictationLanguage> {
        DICTATION_LANGUAGES
            .iter()
            .find(|language| language.code == code)
    }
}

/// 말하는 동안 받아 적고 문단을 나누는 엔진(ADR-0056).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// macOS 받아쓰기(`SFSpeechRecognizer`, 기기 안). 내려받을 모델이 없다.
    #[cfg_attr(target_os = "macos", default)]
    Apple,
    /// Nemotron 3.5 ASR Streaming(모델을 내려받는다).
    #[cfg_attr(not(target_os = "macos"), default)]
    Nemotron,
}

impl Engine {
    /// 이 기기에서 고를 수 있는 엔진.
    pub fn available() -> &'static [Engine] {
        if cfg!(target_os = "macos") {
            &[Engine::Apple, Engine::Nemotron]
        } else {
            &[Engine::Nemotron]
        }
    }
}

/// transcribe.cpp의 연산 백엔드를 등록한다. Windows는 백엔드를 실행 중에 불러오는 빌드라(Vulkan이 없으면 그
/// 모듈만 건너뛰고 CPU로 돈다) 첫 모델을 불러오기 전에 메인 스레드에서 한 번 불러야 한다(등록과 모델 불러오기가
/// 겹치면 안 된다). 정적 빌드(macOS)에서는 아무 일도 하지 않는다.
pub fn init_backends() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        if let Err(err) = transcribe_cpp::init_backends_default() {
            eprintln!("음성 인식 백엔드를 불러오지 못했습니다: {err}");
        }
    });
}

/// Hugging Face 캐시 폴더.
pub fn cache_dir() -> PathBuf {
    hf_hub::resolve_cache_dir()
}

/// 캐시의 어느 스냅샷에든 모델 파일이 있으면 그 경로. 예전 커밋으로 받은 것도 쓴다.
/// 받다 만 파일은 스냅샷에 링크되지 않아(`blobs/*.incomplete`) 걸리지 않는다.
pub fn cached(spec: &ModelSpec) -> Option<PathBuf> {
    let repo = cache_dir().join(format!("models--{}--{}", spec.owner, spec.repo));
    std::fs::read_dir(repo.join("snapshots"))
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(spec.file))
        .find(|path| path.is_file())
}

/// Hugging Face에 로그인했는지(`hf auth login`의 토큰 파일 또는 `HF_TOKEN`). 로그인하면 더 빨리 받는다.
pub fn hf_logged_in() -> bool {
    if std::env::var("HF_TOKEN").is_ok_and(|token| !token.trim().is_empty()) {
        return true;
    }
    let path = std::env::var_os("HF_TOKEN_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| hf_hub::hf_home().join("token"));
    std::fs::read_to_string(path).is_ok_and(|token| !token.trim().is_empty())
}

/// 내려받기 소식.
enum Event {
    Progress { done: u64, total: u64 },
    Finished(Result<PathBuf, String>),
}

/// tokio 런타임. 처음 내려받을 때 만든다.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("hanji-hf")
            .enable_all()
            .build()
            .expect("tokio 런타임을 만들 수 없습니다")
    })
}

/// 버리면 내려받기를 멈춘다.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 모델을 캐시에 내려받는다. 마지막 소식은 언제나 `Finished`다(멈추면 오지 않는다).
fn download(spec: &'static ModelSpec) -> (mpsc::UnboundedReceiver<Event>, AbortOnDrop) {
    let (tx, rx) = mpsc::unbounded();
    let handle = runtime().spawn(async move {
        let reporter = Reporter {
            tx: tx.clone(),
            file: spec.file,
            size: spec.size,
            last_permille: AtomicU64::new(u64::MAX),
        };
        let result = async {
            let client = hf_hub::HFClient::builder()
                .user_agent(concat!("hanji/", env!("CARGO_PKG_VERSION")))
                .build()?;
            client
                .model(spec.owner, spec.repo)
                .download_file()
                .filename(spec.file)
                .progress(Progress::new(reporter))
                .send()
                .await
        }
        .await
        .map_err(|err| err.to_string());
        let _ = tx.unbounded_send(Event::Finished(result));
    });
    (rx, AbortOnDrop(handle.abort_handle()))
}

/// hf-hub의 진행 소식을 받은 바이트로 바꿔 보낸다. 화면이 넘치지 않게 0.1%가 바뀔 때만 보낸다.
struct Reporter {
    tx: mpsc::UnboundedSender<Event>,
    file: &'static str,
    size: u64,
    last_permille: AtomicU64,
}

impl Reporter {
    fn send(&self, done: u64, total: u64) {
        let total = if total == 0 { self.size } else { total };
        let permille = done.saturating_mul(1000) / total.max(1);
        if self.last_permille.swap(permille, Ordering::Relaxed) != permille {
            let _ = self.tx.unbounded_send(Event::Progress { done, total });
        }
    }
}

impl ProgressHandler for Reporter {
    fn on_progress(&self, event: &ProgressEvent) {
        let ProgressEvent::Download(event) = event else {
            return;
        };
        match event {
            HubEvent::Start { total_bytes, .. } => self.send(0, *total_bytes),
            // Xet 저장소는 묶음 전체의 바이트만 알린다(파일 하나만 받으니 같은 뜻이다).
            HubEvent::AggregateProgress {
                bytes_completed,
                total_bytes,
                ..
            } => self.send(*bytes_completed, *total_bytes),
            HubEvent::Progress { files } => {
                for file in files.iter().filter(|file| file.filename == self.file) {
                    self.send(file.bytes_completed, file.total_bytes);
                }
            }
            HubEvent::Complete => {}
        }
    }
}

/// 받는 중인 모델.
pub struct ActiveDownload {
    pub done: u64,
    pub total: u64,
    _abort: AbortOnDrop,
    _task: Task<()>,
}

/// 모델 하나의 캐시 상태와 내려받기.
#[derive(Default)]
struct Entry {
    /// 캐시에 있는 모델 파일.
    downloaded: Option<PathBuf>,
    active: Option<ActiveDownload>,
    /// 마지막 내려받기가 실패한 이유.
    failed: Option<SharedString>,
}

/// 모델 캐시 상태와 내려받기(앱 전역). 모델마다 따로 받는다.
pub struct ModelStore {
    entries: HashMap<&'static str, Entry>,
}

struct GlobalModelStore(Entity<ModelStore>);

impl Global for GlobalModelStore {}

impl ModelStore {
    /// 앱 전역 저장소. 처음 부르면 캐시를 살펴 만든다.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(GlobalModelStore(store)) = cx.try_global::<GlobalModelStore>() {
            return store.clone();
        }
        let entries = [&NEMOTRON, &COHERE]
            .into_iter()
            .map(|spec| {
                let entry = Entry {
                    downloaded: cached(spec),
                    ..Default::default()
                };
                (spec.file, entry)
            })
            .collect();
        let store = cx.new(|_| Self { entries });
        cx.set_global(GlobalModelStore(store.clone()));
        store
    }

    fn entry(&self, spec: &ModelSpec) -> Option<&Entry> {
        self.entries.get(spec.file)
    }

    /// 캐시를 다시 살핀다(`hf` CLI 등 밖에서 받거나 지웠을 수 있다).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for spec in [&NEMOTRON, &COHERE] {
            let downloaded = cached(spec);
            let entry = self.entries.entry(spec.file).or_default();
            if downloaded != entry.downloaded {
                entry.downloaded = downloaded;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// 캐시에 있는 모델 파일.
    pub fn downloaded(&self, spec: &ModelSpec) -> Option<&PathBuf> {
        self.entry(spec)?.downloaded.as_ref()
    }

    pub fn active(&self, spec: &ModelSpec) -> Option<&ActiveDownload> {
        self.entry(spec)?.active.as_ref()
    }

    pub fn failure(&self, spec: &ModelSpec) -> Option<&SharedString> {
        self.entry(spec)?.failed.as_ref()
    }

    /// 내려받기 시작한다. 이미 받는 중이면 아무것도 하지 않는다.
    pub fn start(&mut self, spec: &'static ModelSpec, cx: &mut Context<Self>) {
        if self.active(spec).is_some() {
            return;
        }
        let (mut events, abort) = download(spec);
        let task = cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let finished = matches!(event, Event::Finished(_));
                let updated = this.update(cx, |this, cx| this.on_event(spec, event, cx));
                if finished || updated.is_err() {
                    break;
                }
            }
        });
        let entry = self.entries.entry(spec.file).or_default();
        entry.failed = None;
        entry.active = Some(ActiveDownload {
            done: 0,
            total: spec.size,
            _abort: abort,
            _task: task,
        });
        cx.notify();
    }

    /// 받던 것을 멈춘다. 다시 받으면 처음부터 받는다.
    pub fn cancel(&mut self, spec: &ModelSpec, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries.get_mut(spec.file)
            && entry.active.take().is_some()
        {
            cx.notify();
        }
    }

    fn on_event(&mut self, spec: &ModelSpec, event: Event, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get_mut(spec.file) else {
            return;
        };
        let Some(active) = entry.active.as_mut() else {
            return;
        };
        match event {
            Event::Progress { done, total } => {
                active.done = done;
                active.total = total;
            }
            Event::Finished(result) => {
                entry.active = None;
                match result {
                    Ok(path) => entry.downloaded = Some(path),
                    Err(reason) => entry.failed = Some(reason.into()),
                }
            }
        }
        cx.notify();
    }
}
