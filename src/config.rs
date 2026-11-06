//! 앱 설정(머신 로컬 상태) 로드/저장.
//!
//! 라이브러리(사용자 데이터)와 분리해 OS 설정 디렉터리에 둔다.
//! - Windows: `%APPDATA%\Hanji\config.toml`
//! - macOS: `~/Library/Application Support/Hanji/config.toml`
//! - Linux: `~/.config/Hanji/config.toml`

use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// 최근 라이브러리 목록 최대 개수.
const RECENT_LIMIT: usize = 5;

/// `config.toml` 스키마.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 현재 라이브러리 루트. 없으면 첫 실행(온보딩).
    pub library_root: Option<PathBuf>,
    /// 최근에 연 라이브러리(최신이 앞).
    pub recent_libraries: Vec<PathBuf>,
    /// 마지막으로 선택한 테마 이름. 없으면 기본 라이트.
    pub theme: Option<String>,
    /// 에디터 서체 이름. 없으면 시스템 서체.
    pub editor_font: Option<String>,
    /// 에디터 본문 글자 크기(px). 없으면 16.
    pub editor_font_size: Option<u16>,
    /// 어시스턴트가 쓸 AI 제공자(`bedrock`, `alibaba-token-plan`, `claude-code`, `codex`). 없으면 쓰지 않는다.
    pub ai_provider: Option<String>,
    /// Bedrock에 쓸 AWS 프로필 이름(`~/.aws/config`). 없으면 고르지 않은 것.
    pub aws_profile: Option<String>,
    /// Bedrock을 부를 리전. 없으면 프로필의 리전을 따른다.
    pub aws_region: Option<String>,
    /// 어시스턴트가 부를 Bedrock 모델(추론 프로필) ID. 없으면 `agent::DEFAULT_MODEL`.
    pub ai_model: Option<String>,
    /// Alibaba Token Plan API 키(`sk-sp-…`).
    pub token_plan_api_key: Option<String>,
    /// 어시스턴트가 부를 Token Plan 모델 ID. 없으면 `agent::token_plan::DEFAULT_MODEL`.
    pub token_plan_model: Option<String>,
    /// Claude Code CLI(`claude`) 경로. 없으면 `PATH`와 흔한 설치 위치에서 찾는다.
    pub claude_code_path: Option<PathBuf>,
    /// 어시스턴트가 부를 Claude Code 모델 별칭(`sonnet`·`opus`·`haiku`). 없으면 `sonnet`.
    pub claude_code_model: Option<String>,
    /// Codex CLI(`codex`) 경로. 없으면 `PATH`와 흔한 설치 위치에서 찾는다.
    pub codex_path: Option<PathBuf>,
    /// 어시스턴트가 부를 Codex 모델 ID. 없으면 Codex의 기본 모델.
    pub codex_model: Option<String>,
    /// Claude Code 모델 별칭 → 마지막으로 실제 불린 모델 ID(`sonnet` → `claude-sonnet-5-5`). 모델 이름에 버전을 보인다.
    pub claude_code_models: std::collections::BTreeMap<String, String>,
    /// UI 언어(`en`·`ko`). 없으면 시스템 언어를 따른다.
    pub language: Option<String>,
    /// OCR 텍스트 삽입 대화상자의 옵션(ADR-0053). 다음에 열 때도 그대로 쓴다.
    pub ocr: OcrConfig,
    /// 받아쓰기 창의 옵션(ADR-0054). 다음에 열 때도 그대로 쓴다.
    pub dictation: DictationConfig,
    /// 앱을 시작할 때 새 버전을 확인할지. 없으면 확인한다.
    pub check_updates: Option<bool>,
}

/// 녹음 중 요약 주기(분).
pub const SUMMARY_MINUTES: u64 = 1;

/// 받아쓰기 옵션(`[dictation]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    /// 받아 적을 소리(`microphone`·`system`·`both`).
    pub source: crate::dictation::AudioSource,
    /// 작은 소리를 말소리 크기로 키운다(자동 음량 맞춤).
    pub auto_gain: bool,
    /// 녹음한 소리를 기록마다 WAV로 남긴다(`STT/.audio/`). 받아쓰기가 무엇을 들었는지 들어 본다.
    pub save_audio: bool,
    /// 말하는 동안 받아 적고 문단을 나누는 엔진(ADR-0056).
    pub engine: crate::dictation::Engine,
    /// 받아쓰는 언어 코드(`dictation::DICTATION_LANGUAGES`). macOS 받아쓰기와 Cohere가 쓴다.
    /// 없으면 앱 언어(없으면 영어).
    #[serde(alias = "cohere_language")]
    pub language: Option<String>,
    /// 문단을 Cohere로 다시 받아 적는다(ADR-0056). 모델을 내려받았을 때만.
    pub cohere: bool,
    /// 녹음 중 [`SUMMARY_MINUTES`]분마다 요약한다(ADR-0055).
    pub summarize: bool,
    /// 요약을 쓸 언어 코드(`agent::dictation::LANGUAGES`). 없으면 기록의 언어로 쓴다.
    /// 예전 녹음 중 번역 언어(`translate_to`)를 그대로 잇는다.
    #[serde(alias = "translate_to")]
    pub summary_language: Option<String>,
    /// 요약에 쓸 Bedrock 모델(추론 프로필) ID. 없으면 어시스턴트의 모델.
    pub bedrock_model: Option<String>,
    /// 요약에 쓸 Token Plan 모델 ID. 없으면 어시스턴트의 모델.
    pub token_plan_model: Option<String>,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            source: Default::default(),
            auto_gain: true,
            save_audio: false,
            engine: Default::default(),
            language: None,
            cohere: false,
            summarize: false,
            summary_language: None,
            bedrock_model: None,
            token_plan_model: None,
        }
    }
}

impl DictationConfig {
    /// 요약에 부를 곳. AI를 설정하지 않았으면 `None`.
    pub fn backend(&self, prefs: &crate::settings::Prefs) -> Option<crate::agent::Backend> {
        let backend = crate::agent::Backend::llm_from_prefs(prefs)?;
        let model = match backend {
            crate::agent::Backend::Bedrock { .. } => self.bedrock_model.clone(),
            crate::agent::Backend::TokenPlan { .. } => self.token_plan_model.clone(),
            crate::agent::Backend::ClaudeCode { .. } | crate::agent::Backend::Codex { .. } => None,
        };
        Some(match model {
            Some(model) => backend.with_model(model),
            None => backend,
        })
    }

    /// 받아쓰는 언어.
    pub fn language(&self) -> &'static crate::dictation::DictationLanguage {
        use crate::dictation::{DICTATION_LANGUAGES, DictationLanguage};
        self.language
            .as_deref()
            .and_then(DictationLanguage::find)
            .or_else(|| DictationLanguage::find(&rust_i18n::locale()[..]))
            .unwrap_or(&DICTATION_LANGUAGES[1])
    }

    /// 요약 언어.
    pub fn summary_language(&self) -> Option<&'static crate::agent::dictation::Language> {
        crate::agent::dictation::Language::find(self.summary_language.as_deref()?)
    }
}

/// OCR 텍스트 삽입 옵션(`[ocr]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    /// 이미지를 AI(LLM 비전)로 읽는다. 아니면 기기의 문자 인식만 쓴다.
    pub use_ai: bool,
    /// AI로 읽을 Bedrock 모델(추론 프로필) ID. 없으면 어시스턴트의 모델.
    pub bedrock_model: Option<String>,
    /// AI로 읽을 Token Plan 모델 ID. 없으면 어시스턴트의 모델.
    pub token_plan_model: Option<String>,
    /// 기기 문자 인식 결과를 초안으로 함께 보낸다.
    pub send_draft: bool,
    /// AI에 덧붙이는 지시.
    pub instruction: String,
    /// 이미지의 줄바꿈을 그대로 둔다.
    pub keep_breaks: bool,
}

impl Default for OcrConfig {
    /// 초안은 기본으로 함께 보낸다(빠뜨리거나 지어내는 줄이 준다).
    fn default() -> Self {
        Self {
            use_ai: false,
            bedrock_model: None,
            token_plan_model: None,
            send_draft: true,
            instruction: String::new(),
            keep_breaks: false,
        }
    }
}

/// 앱 이름이 Writer였을 때 만든 설정·데이터·캐시 폴더(`<dir>/Writer`)를 `<dir>/Hanji`로 옮긴다.
/// 새 폴더가 이미 있으면 건드리지 않는다. 설정을 읽기 전, 앱 시작에서 한 번 부른다.
pub fn migrate_legacy_dirs() {
    for base in [dirs::config_dir(), dirs::data_dir(), dirs::cache_dir()]
        .into_iter()
        .flatten()
    {
        let legacy = base.join("Writer");
        let current = base.join("Hanji");
        if legacy.is_dir() && !current.exists() {
            if let Err(err) = std::fs::rename(&legacy, &current) {
                eprintln!("예전 폴더를 옮길 수 없습니다({}): {err}", legacy.display());
            }
        }
    }
}

/// 설정 디렉터리 (`<config_dir>/Hanji`).
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("Hanji"))
}

/// 설정 파일 경로 (`<config_dir>/Hanji/config.toml`).
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.toml"))
}

/// 새 라이브러리를 만들 때 제안하는 기본 경로 (`~/Documents/Hanji`).
pub fn default_library_root() -> Option<PathBuf> {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .map(|dir| dir.join("Hanji"))
}

impl Config {
    /// 시작할 때 새 버전을 확인할지(기본: 확인).
    pub fn check_updates(&self) -> bool {
        self.check_updates.unwrap_or(true)
    }

    /// 설정을 읽는다. 파일이 없거나 깨져 있으면 기본값을 돌려준다(앱 시작을 막지 않음).
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 설정을 저장한다(상위 디렉터리 생성 포함).
    pub fn save(&self) -> io::Result<()> {
        let path = config_path().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "설정 디렉터리를 찾을 수 없습니다")
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        std::fs::write(path, text)
    }

    /// 라이브러리를 현재 루트로 설정하고 최근 목록을 갱신한다.
    pub fn set_library(&mut self, root: PathBuf) {
        self.recent_libraries.retain(|path| path != &root);
        self.recent_libraries.insert(0, root.clone());
        self.recent_libraries.truncate(RECENT_LIMIT);
        self.library_root = Some(root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_library_updates_root_and_recents() {
        let mut config = Config::default();
        config.set_library(PathBuf::from("/a"));
        config.set_library(PathBuf::from("/b"));
        // 중복은 최신으로 승격
        config.set_library(PathBuf::from("/a"));

        assert_eq!(config.library_root, Some(PathBuf::from("/a")));
        assert_eq!(
            config.recent_libraries,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }
}
