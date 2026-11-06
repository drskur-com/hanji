//! 어시스턴트 에이전트: 대화 상태, 시스템 프롬프트, 모델 호출. 화면(gpui)과 떨어져 있어
//! 패널은 [`Conversation`]을 들고 그리기만 한다.

mod acp;
mod bedrock;
pub mod dictation;
mod diff;
mod edit;
mod history;
mod model_tool;
mod prompt;
mod review;
pub mod skills;
pub mod token_plan;
mod tools;
// OCR 텍스트 삽입(macOS·Windows)에서만 쓴다.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod transcribe;
mod web;

pub use bedrock::{ChatRequest, stream};
pub use diff::{DiffLine, LineKind};
pub use history::{History, Summary, now_millis};
pub use prompt::{AgentTools, Sheet, agent_system_prompt, system_prompt};
pub use review::PASS_SCORE;
pub use tools::Workspace;

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use serde::{Deserialize, Serialize};

use crate::settings::{AiProvider, Prefs};

/// 대화의 말 한 마디.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// 사용자가 한 말이면 `true`, 어시스턴트 답이면 `false`.
    pub user: bool,
    pub text: String,
    /// 어시스턴트가 이 답을 쓰며 한 일(부른 도구, 파일 변경). 화면에만 보이고 모델에는 다시
    /// 보내지 않는다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<Activity>,
    /// 사용자가 이 말을 할 때 열려 있던 시트(라이브러리 루트 기준). 시트가 없었으면 `None`.
    /// 대화 기록을 시트로 거르고, 시트가 바뀐 것을 모델에 알리는 데 쓴다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
}

/// 답 하나에서 어시스턴트가 한 일.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activity {
    /// 도구를 불렀다. 화면에 보일 한 줄.
    Tool(String),
    /// 확장 그림을 따로 검토했다(ADR-0030): 확장 파일(라이브러리 기준)과 점수(1~10, 없으면 검토자가 매기지 않음).
    Review { file: String, score: Option<u8> },
    /// 파일을 바꾸려 했다(승인 카드).
    Change(ChangeCard),
    /// 처음 보는 사이트의 웹 페이지를 읽으려 했다(허락 카드).
    Fetch(FetchCard),
}

/// 웹 페이지 읽기 허락 카드. 허락하면 그 사이트는 이 대화에서 다시 묻지 않는다.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchCard {
    pub id: u64,
    pub url: String,
    /// 허락하는 사이트(호스트).
    pub site: String,
    /// `Applied`면 허락했다.
    pub state: ChangeState,
}

/// 승인 카드: 바꿀 파일과 변경 내용.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeCard {
    /// 대화 안에서 카드를 가리키는 번호.
    pub id: u64,
    /// 바꿀 파일(라이브러리 루트 기준). 새 시트면 그룹 폴더.
    pub path: String,
    /// 새 파일(새 시트 포함)을 만드는지.
    pub new_file: bool,
    pub lines: Vec<DiffLine>,
    pub state: ChangeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeState {
    /// 사용자의 승인을 기다린다.
    Pending,
    Applied,
    Declined,
    /// 승인했지만 쓰지 못했다(그사이 파일이 바뀌었거나 쓰기 실패).
    Failed,
}

/// 승인 요청: 카드와, 사용자의 답(`true`면 적용)을 돌려보낼 곳.
#[derive(Debug)]
pub struct Approval {
    pub card: ChangeCard,
    pub reply: oneshot::Sender<bool>,
}

/// 웹 페이지를 읽어도 되는지 묻는다.
#[derive(Debug)]
pub struct FetchApproval {
    pub card: FetchCard,
    pub reply: oneshot::Sender<bool>,
}

/// 이 대화에서 읽기를 허락한 사이트. 요청과 함께 모델 호출 쪽에 넘긴다.
pub type AllowedSites = Arc<Mutex<HashSet<String>>>;

/// 답을 받는 동안 화면에 넘기는 일.
#[derive(Debug)]
pub enum ChatEvent {
    /// 답 글이 이만큼 더 왔다.
    Text(String),
    /// 도구를 불렀다. 화면에 보일 한 줄.
    Tool(String),
    /// 확장 그림 검토 결과([`Activity::Review`]).
    Review { file: String, score: Option<u8> },
    /// 파일을 바꾸려 한다. 사용자가 답할 때까지 도구가 기다린다.
    Approve(Approval),
    /// 처음 보는 사이트를 읽으려 한다. 사용자가 답할 때까지 도구가 기다린다.
    ApproveFetch(FetchApproval),
    /// 승인한 변경을 쓰지 못했다(카드 번호).
    ChangeFailed(u64),
    /// 승인한 변경을 썼다. 화면은 폴더 감시를 기다리지 않고 열린 시트를 곧바로 맞춘다.
    Changed,
    /// 어시스턴트가 부를 모델을 이 ID로 바꿨다(`set_model`). 다음 말부터 쓴다.
    SetModel(String),
    /// 에이전트가 모델 별칭(`requested`)을 이 모델 ID(`resolved`)로 불렀다(ADR-0057). 모델 이름에 버전을 보인다.
    ModelResolved { requested: String, resolved: String },
    /// 답이 끝났다.
    Done,
    /// 요청이 실패했다(자격 증명, 권한, 모델 ID 등). 사람이 읽을 설명.
    Failed(String),
}

/// 모델 고르기의 선택지 하나.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    /// 표시 이름.
    pub name: Cow<'static, str>,
    /// 모델 ID. Bedrock은 기본 모델 ID이고, 실제로 부를 때는 이 ID로 끝나는 추론 프로필
    /// (`global.…`, `us.…` 등)을 쓴다.
    pub base: Cow<'static, str>,
}

impl ModelChoice {
    /// 부를 ID(Bedrock이면 추론 프로필 `global.openai.gpt-6.1-sol`)가 이 모델의 것인지.
    pub fn matches(&self, id: &str) -> bool {
        id == self.base
            || id
                .strip_suffix(self.base.as_ref())
                .is_some_and(|prefix| prefix.ends_with('.'))
    }
}

/// 모델을 부를 곳.
#[derive(Clone, PartialEq, Eq)]
pub enum Backend {
    /// Amazon Bedrock.
    Bedrock {
        /// 자격 증명을 읽을 AWS 프로필(`~/.aws/config`).
        profile: String,
        /// 부를 리전. 없으면 프로필의 리전을 따른다.
        region: Option<String>,
        /// Bedrock 모델 ID 또는 추론 프로필 ID.
        model: String,
    },
    /// Alibaba Cloud Model Studio Token Plan.
    TokenPlan {
        api_key: String,
        /// 모델 ID.
        model: String,
    },
    /// 설치된 Claude Code CLI(에이전트, ADR-0057). 모델 한 번 부르기에는 쓰지 않는다.
    ClaudeCode {
        /// 모델 별칭(`sonnet`·`opus`·`haiku`).
        model: String,
    },
    /// 설치된 Codex CLI(에이전트, ADR-0057). 모델 한 번 부르기에는 쓰지 않는다.
    Codex {
        /// 모델 ID. 비었으면 Codex 설정의 기본 모델.
        model: String,
    },
}

/// API 키는 기록(로그)에 남기지 않는다.
impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Bedrock {
                profile,
                region,
                model,
            } => f
                .debug_struct("Bedrock")
                .field("profile", profile)
                .field("region", region)
                .field("model", model)
                .finish(),
            Backend::TokenPlan { model, .. } => f
                .debug_struct("TokenPlan")
                .field("model", model)
                .finish_non_exhaustive(),
            Backend::ClaudeCode { model } => {
                f.debug_struct("ClaudeCode").field("model", model).finish()
            }
            Backend::Codex { model } => f.debug_struct("Codex").field("model", model).finish(),
        }
    }
}

impl Backend {
    /// 설정에서 고른 곳. AI를 설정하지 않았으면(제공자가 없거나, Bedrock에 AWS 프로필이,
    /// Token Plan에 API 키가 없으면) `None`. Claude Code(에이전트)도 돌려준다. 모델 한 번 부르기만 하는
    /// 기능(받아쓰기 요약·OCR)은 [`Self::llm_from_prefs`]를 쓴다.
    pub fn from_prefs(prefs: &Prefs) -> Option<Self> {
        match prefs.ai_provider? {
            AiProvider::Bedrock => Some(Self::Bedrock {
                profile: prefs.aws_profile.as_ref()?.to_string(),
                region: prefs.aws_region.as_ref().map(ToString::to_string),
                model: prefs
                    .ai_model
                    .as_ref()
                    .map_or_else(|| bedrock::DEFAULT_MODEL.to_string(), ToString::to_string),
            }),
            AiProvider::TokenPlan => Some(Self::TokenPlan {
                api_key: prefs.token_plan_key.as_ref()?.to_string(),
                model: prefs.token_plan_model.as_ref().map_or_else(
                    || token_plan::DEFAULT_MODEL.to_string(),
                    ToString::to_string,
                ),
            }),
            AiProvider::ClaudeCode => Some(Self::ClaudeCode {
                model: prefs
                    .claude_code_model
                    .as_ref()
                    .map_or_else(|| acp::claude::MODELS[0].0.to_string(), ToString::to_string),
            }),
            AiProvider::Codex => Some(Self::Codex {
                model: prefs
                    .codex_model
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            }),
        }
    }

    /// 모델 한 번 부르기(받아쓰기 요약·OCR)에 쓸 곳. 에이전트 제공자면 `None`.
    pub fn llm_from_prefs(prefs: &Prefs) -> Option<Self> {
        Self::from_prefs(prefs).filter(|backend| !backend.is_agent())
    }

    /// 스스로 도구를 부르는 에이전트(ACP, ADR-0057)인지.
    pub fn is_agent(&self) -> bool {
        self.agent_tools().is_some()
    }

    /// 에이전트면 그 에이전트가 쓰는 도구(지시가 다르다).
    pub fn agent_tools(&self) -> Option<AgentTools> {
        match self {
            Backend::ClaudeCode { .. } => Some(AgentTools::ClaudeCode),
            Backend::Codex { .. } => Some(AgentTools::Codex),
            _ => None,
        }
    }

    /// 같은 곳에서 다른 모델(`model` ID)을 부른다.
    pub fn with_model(mut self, model: String) -> Self {
        match &mut self {
            Backend::Bedrock { model: current, .. }
            | Backend::TokenPlan { model: current, .. }
            | Backend::ClaudeCode { model: current }
            | Backend::Codex { model: current } => {
                *current = model;
            }
        }
        self
    }

    /// 이 곳의 모델(`id`)이 그림을 볼 수 있는지. Bedrock에서 고를 수 있는 모델은 모두 보고, Token Plan은 아는
    /// 모델의 표를 따른다(표에 없으면 보낸 뒤 모델이 정한다).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub fn reads_images(&self, id: &str) -> bool {
        match self {
            Backend::Bedrock { .. } => true,
            Backend::TokenPlan { .. } => token_plan::find(id).is_none_or(|model| model.vision),
            Backend::ClaudeCode { .. } | Backend::Codex { .. } => true,
        }
    }

    /// 부를 모델 ID.
    pub fn model(&self) -> &str {
        match self {
            Backend::Bedrock { model, .. }
            | Backend::TokenPlan { model, .. }
            | Backend::ClaudeCode { model }
            | Backend::Codex { model } => model,
        }
    }

    /// 쓸 수 있는 모델이 달라지는 설정. 바뀌면 모델 목록을 다시 확인한다.
    pub fn models_key(&self) -> String {
        match self {
            Backend::Bedrock {
                profile, region, ..
            } => format!("bedrock/{profile}/{}", region.as_deref().unwrap_or("")),
            Backend::TokenPlan { .. } => "token-plan".to_string(),
            Backend::ClaudeCode { .. } => "claude-code".to_string(),
            Backend::Codex { .. } => "codex".to_string(),
        }
    }

    /// 쓸 수 있는 모델과 부를 ID. Bedrock은 리전의 추론 프로필을, Token Plan은 플랜의 모델 목록
    /// (`/models`)을 읽어 판단한다.
    pub fn available_models(
        &self,
    ) -> oneshot::Receiver<Result<Vec<(ModelChoice, String)>, String>> {
        let (tx, rx) = oneshot::channel();
        let backend = self.clone();
        bedrock::runtime().spawn(async move {
            tx.send(backend.list_models().await).ok();
        });
        rx
    }

    /// [`Self::available_models`]를 tokio 런타임 안에서 읽는다.
    async fn list_models(&self) -> Result<Vec<(ModelChoice, String)>, String> {
        match self {
            Backend::Bedrock {
                profile, region, ..
            } => bedrock::list_models(profile, region.clone()).await,
            Backend::TokenPlan { api_key, .. } => token_plan::list_models(api_key).await,
            Backend::Codex { .. } => acp::codex::list_models().await,
            Backend::ClaudeCode { .. } => {
                // 한 번 써 본 별칭은 실제 모델의 이름(버전 포함)으로 보인다.
                let resolved = crate::config::Config::load().claude_code_models;
                Ok(acp::claude::MODELS
                    .iter()
                    .map(|(id, name)| {
                        let name = resolved
                            .get(*id)
                            .and_then(|model| acp::claude::display_name(model))
                            .map_or(Cow::Borrowed(*name), Cow::Owned);
                        (
                            ModelChoice {
                                name,
                                base: Cow::Borrowed(*id),
                            },
                            (*id).to_string(),
                        )
                    })
                    .collect())
            }
        }
    }
}

/// 모델과 주고받는 대화 하나. 답을 받는 중이면 마지막 말은 자라는 어시스턴트 답이다.
#[derive(Debug, Default)]
pub struct Conversation {
    turns: Vec<Turn>,
    streaming: bool,
    /// 마지막 요청이 실패한 까닭.
    error: Option<String>,
    /// 답을 기다리는 승인 카드와 답을 돌려보낼 곳. 버리면 도구는 거절로 알고 멈춘다.
    pending: Option<(u64, oneshot::Sender<bool>)>,
    /// 읽기를 허락한 사이트.
    allowed_sites: AllowedSites,
}

impl Conversation {
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    /// 읽기를 허락한 사이트(모델 호출 쪽과 함께 쓴다).
    pub fn allowed_sites(&self) -> AllowedSites {
        self.allowed_sites.clone()
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 주고받은 말도, 실패도 없다.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.error.is_none()
    }

    /// 첫 사용자 말(대화 제목으로 쓴다).
    pub fn title(&self) -> Option<&str> {
        self.turns.first().map(|turn| turn.text.as_str())
    }

    /// 저장해 둔 대화를 다시 연다.
    pub fn restore(turns: Vec<Turn>, sites: Vec<String>) -> Self {
        Self {
            turns,
            allowed_sites: Arc::new(Mutex::new(sites.into_iter().collect())),
            ..Self::default()
        }
    }

    /// 읽기를 허락한 사이트(저장용, 이름 순).
    pub fn sites(&self) -> Vec<String> {
        let mut sites: Vec<String> = self
            .allowed_sites
            .lock()
            .map(|sites| sites.iter().cloned().collect())
            .unwrap_or_default();
        sites.sort();
        sites
    }

    /// 사용자 말을 더하고 받을 답 자리를 연다. `sheet`는 지금 열린 시트다. 모델에 보낼
    /// 대화(이번 말이 마지막)를 돌려준다. 답을 받는 중이거나 빈 말이면 아무것도 하지 않고 `None`.
    pub fn send(&mut self, text: &str, sheet: Option<&str>) -> Option<Vec<Turn>> {
        let text = text.trim();
        if text.is_empty() || self.streaming {
            return None;
        }
        self.turns.push(Turn {
            user: true,
            text: text.to_string(),
            sheet: sheet.map(ToString::to_string),
            ..Turn::default()
        });
        let turns = request_turns(&self.turns);
        self.turns.push(Turn::default());
        self.error = None;
        self.streaming = true;
        Some(turns)
    }

    /// 받은 일을 대화에 반영한다. 답이 한 글자도 오지 않고 실패했으면 보낸 말을 대화에서 빼고
    /// 돌려준다(입력창에 되돌려 다시 보낼 수 있게).
    pub fn apply(&mut self, event: ChatEvent) -> Option<String> {
        match event {
            ChatEvent::Text(piece) => {
                if let Some(turn) = self.reply_mut() {
                    turn.text.push_str(&piece);
                }
                None
            }
            ChatEvent::Tool(summary) => {
                self.push_activity(Activity::Tool(summary));
                None
            }
            ChatEvent::Review { file, score } => {
                self.push_activity(Activity::Review { file, score });
                None
            }
            ChatEvent::Approve(Approval { card, reply }) => {
                self.pending = Some((card.id, reply));
                self.push_activity(Activity::Change(card));
                None
            }
            ChatEvent::ApproveFetch(FetchApproval { card, reply }) => {
                self.pending = Some((card.id, reply));
                self.push_activity(Activity::Fetch(card));
                None
            }
            ChatEvent::ChangeFailed(id) => {
                self.set_change_state(id, ChangeState::Failed);
                None
            }
            // 설정·열린 시트는 화면이 맞춘다. 대화에는 도구 줄(`Tool`)만 남는다.
            ChatEvent::SetModel(_) | ChatEvent::ModelResolved { .. } | ChatEvent::Changed => None,
            ChatEvent::Done => {
                self.streaming = false;
                self.drop_pending();
                None
            }
            ChatEvent::Failed(message) => {
                self.streaming = false;
                self.drop_pending();
                self.error = Some(message);
                if !self.ends_with_empty_reply() {
                    return None;
                }
                self.turns.pop();
                self.turns
                    .pop()
                    .filter(|turn| turn.user)
                    .map(|turn| turn.text)
            }
        }
    }

    /// 답을 기다리는 승인 카드의 번호.
    pub fn pending_change(&self) -> Option<u64> {
        self.pending.as_ref().map(|(id, _)| *id)
    }

    /// 승인 카드에 답한다(`approve`면 적용). 기다리는 카드가 아니면 아무것도 하지 않는다.
    pub fn decide(&mut self, id: u64, approve: bool) {
        if self.pending_change() != Some(id) {
            return;
        }
        if let Some((_, reply)) = self.pending.take() {
            reply.send(approve).ok();
        }
        let state = if approve {
            ChangeState::Applied
        } else {
            ChangeState::Declined
        };
        self.set_change_state(id, state);
    }

    /// 받는 중인 답을 멈춘다. 받은 데까지는 남긴다. 기다리던 승인은 거절한 것으로 본다.
    pub fn stop(&mut self) {
        self.streaming = false;
        self.drop_pending();
        if self.ends_with_empty_reply() {
            self.turns.pop();
        }
    }

    /// 대화를 비운다.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// 받는 중인 답.
    fn reply_mut(&mut self) -> Option<&mut Turn> {
        self.turns.last_mut().filter(|turn| !turn.user)
    }

    fn push_activity(&mut self, activity: Activity) {
        if let Some(turn) = self.reply_mut() {
            // 도구 앞뒤의 글이 한 문단으로 붙지 않게 한다.
            if !turn.text.is_empty() && !turn.text.ends_with("\n\n") {
                turn.text.push_str("\n\n");
            }
            turn.activity.push(activity);
        }
    }

    fn set_change_state(&mut self, id: u64, state: ChangeState) {
        let card = self
            .turns
            .iter_mut()
            .flat_map(|turn| &mut turn.activity)
            .find_map(|activity| match activity {
                Activity::Change(card) if card.id == id => Some(&mut card.state),
                Activity::Fetch(card) if card.id == id => Some(&mut card.state),
                _ => None,
            });
        if let Some(card_state) = card {
            *card_state = state;
        }
    }

    /// 기다리던 승인을 거둔다(답할 곳을 버리면 도구는 멈춘다).
    fn drop_pending(&mut self) {
        if let Some((id, _)) = self.pending.take() {
            self.set_change_state(id, ChangeState::Declined);
        }
    }

    /// 마지막 답이 글도 한 일도 없이 비어 있다.
    fn ends_with_empty_reply(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| !turn.user && turn.text.is_empty() && turn.activity.is_empty())
    }
}

/// 모델에 보낼 대화. 비어 있는 답(멈춘 답)은 빼고, 그래서 같은 쪽 말이 잇달으면 한 마디로
/// 합친다(Bedrock Converse는 사용자와 어시스턴트가 번갈아 말해야 받는다).
fn request_turns(turns: &[Turn]) -> Vec<Turn> {
    let mut merged: Vec<Turn> = Vec::new();
    // 직전 사용자 말의 시트. 시트가 바뀌었으면 사용자 말 앞에 적는다(앞의 말은 다른 시트를 두고 한 것이다).
    let mut sheet: Option<Option<&str>> = None;
    for turn in turns.iter() {
        let moved = if turn.user {
            let now = turn.sheet.as_deref();
            let moved = sheet
                .is_some_and(|before| before != now)
                .then(|| match now {
                    Some(path) => format!("[The user switched to the sheet {path}.]"),
                    None => "[The user closed the sheet; no sheet is open now.]".to_string(),
                });
            sheet = Some(now);
            moved
        } else {
            None
        };
        // 거절했거나 쓰지 못한 변경은 다음 답에서도 알도록 답 끝에 적는다(도구 결과는 다시 보내지 않는다).
        let notes: Vec<String> = turn
            .activity
            .iter()
            .filter_map(|activity| match activity {
                Activity::Change(card) if card.state == ChangeState::Declined => Some(format!(
                    "[Not applied: the user declined the change to {}.]",
                    card.path
                )),
                Activity::Change(card) if card.state == ChangeState::Failed => {
                    Some(format!("[Not applied: writing {} failed.]", card.path))
                }
                _ => None,
            })
            .collect();
        let text = moved
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(turn.text.trim_end()))
            .chain(notes.iter().map(String::as_str))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let text = text.as_str();
        if text.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some(last) if last.user == turn.user => {
                last.text.push_str("\n\n");
                last.text.push_str(text);
            }
            _ => merged.push(Turn {
                user: turn.user,
                text: text.to_string(),
                ..Turn::default()
            }),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user: bool, text: &str) -> Turn {
        Turn {
            user,
            text: text.into(),
            ..Turn::default()
        }
    }

    #[test]
    fn send_opens_reply_and_returns_history() {
        let mut chat = Conversation::default();
        assert_eq!(chat.send("  안녕  ", None), Some(vec![turn(true, "안녕")]));
        assert_eq!(chat.turns(), [turn(true, "안녕"), turn(false, "")]);
        assert!(chat.is_streaming());
        // 받는 중이거나 빈 말이면 보내지 않는다.
        assert_eq!(chat.send("또", None), None);
        chat.apply(ChatEvent::Text("반가".into()));
        chat.apply(ChatEvent::Text("워요".into()));
        chat.apply(ChatEvent::Done);
        assert_eq!(chat.send("   ", None), None);
        assert_eq!(
            chat.send("다음", None),
            Some(vec![
                turn(true, "안녕"),
                turn(false, "반가워요"),
                turn(true, "다음")
            ])
        );
    }

    #[test]
    fn failure_before_any_text_gives_back_sent_text() {
        let mut chat = Conversation::default();
        chat.send("질문", None);
        assert_eq!(
            chat.apply(ChatEvent::Failed("권한 없음".into())),
            Some("질문".into())
        );
        assert!(chat.turns().is_empty());
        assert_eq!(chat.error(), Some("권한 없음"));
        assert!(!chat.is_streaming());
        // 다시 보내면 실패를 지운다.
        chat.send("질문", None);
        assert_eq!(chat.error(), None);
    }

    #[test]
    fn failure_after_text_keeps_partial_reply() {
        let mut chat = Conversation::default();
        chat.send("질문", None);
        chat.apply(ChatEvent::Text("답의 앞".into()));
        assert_eq!(chat.apply(ChatEvent::Failed("끊김".into())), None);
        assert_eq!(chat.turns(), [turn(true, "질문"), turn(false, "답의 앞")]);
    }

    #[test]
    fn tool_calls_stay_on_reply_and_split_paragraphs() {
        let mut chat = Conversation::default();
        chat.send("1장 고쳐 줘", None);
        chat.apply(ChatEvent::Text("읽어 볼게요.".into()));
        chat.apply(ChatEvent::Tool("읽음 · 1장.md".into()));
        chat.apply(ChatEvent::Tool("고침 · 1장.md".into()));
        chat.apply(ChatEvent::Text("고쳤어요.".into()));
        chat.apply(ChatEvent::Done);
        let reply = chat.turns().last().unwrap();
        assert_eq!(reply.text, "읽어 볼게요.\n\n고쳤어요.");
        assert_eq!(
            reply.activity,
            [
                Activity::Tool("읽음 · 1장.md".into()),
                Activity::Tool("고침 · 1장.md".into())
            ]
        );
        // 도구만 부르고 실패하면(파일은 이미 바뀌었을 수 있다) 보낸 말을 되돌리지 않는다.
        chat.send("또", None);
        chat.apply(ChatEvent::Tool("고침 · 2장.md".into()));
        assert_eq!(chat.apply(ChatEvent::Failed("끊김".into())), None);
        assert_eq!(
            chat.turns().last().unwrap().activity,
            [Activity::Tool("고침 · 2장.md".into())]
        );
    }

    fn card(id: u64) -> ChangeCard {
        ChangeCard {
            id,
            path: "Projects/소설/1장.md".into(),
            new_file: false,
            lines: Vec::new(),
            state: ChangeState::Pending,
        }
    }

    fn state_of(chat: &Conversation, id: u64) -> ChangeState {
        chat.turns()
            .iter()
            .flat_map(|turn| &turn.activity)
            .find_map(|activity| match activity {
                Activity::Change(card) if card.id == id => Some(card.state),
                Activity::Fetch(card) if card.id == id => Some(card.state),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn fetch_approval_waits_and_updates_card() {
        let mut chat = Conversation::default();
        chat.send("이 링크 요약해 줘", None).unwrap();
        let (reply, answer) = oneshot::channel();
        chat.apply(ChatEvent::ApproveFetch(FetchApproval {
            card: FetchCard {
                id: 7,
                url: "https://example.com/a".into(),
                site: "example.com".into(),
                state: ChangeState::Pending,
            },
            reply,
        }));
        assert_eq!(chat.pending_change(), Some(7));
        chat.decide(7, true);
        assert_eq!(state_of(&chat, 7), ChangeState::Applied);
        assert_eq!(futures::executor::block_on(answer), Ok(true));
        // 허락한 사이트는 대화가 들고 있고, 비우면 잊는다.
        chat.allowed_sites()
            .lock()
            .unwrap()
            .insert("example.com".into());
        chat.clear();
        assert!(chat.allowed_sites().lock().unwrap().is_empty());
    }

    #[test]
    fn approval_sends_answer_and_updates_card() {
        let mut chat = Conversation::default();
        chat.send("고쳐 줘", None);
        let (reply, mut answer) = oneshot::channel();
        chat.apply(ChatEvent::Approve(Approval {
            card: card(1),
            reply,
        }));
        assert_eq!(chat.pending_change(), Some(1));
        // 다른 카드 번호에는 답하지 않는다.
        chat.decide(2, true);
        assert_eq!(answer.try_recv().unwrap(), None);
        chat.decide(1, true);
        assert_eq!(answer.try_recv().unwrap(), Some(true));
        assert_eq!(state_of(&chat, 1), ChangeState::Applied);
        assert_eq!(chat.pending_change(), None);
        // 쓰기에 실패했다고 오면 카드에 보인다.
        chat.apply(ChatEvent::ChangeFailed(1));
        assert_eq!(state_of(&chat, 1), ChangeState::Failed);

        // 멈추면 기다리던 카드는 거절한 것이 되고, 도구는 답할 곳이 사라진 것을 안다.
        let (reply, answer) = oneshot::channel();
        chat.apply(ChatEvent::Approve(Approval {
            card: card(2),
            reply,
        }));
        chat.stop();
        assert_eq!(state_of(&chat, 2), ChangeState::Declined);
        assert!(futures::executor::block_on(answer).is_err());
    }

    #[test]
    fn tells_the_model_when_the_sheet_changes() {
        let mut chat = Conversation::default();
        chat.send("하나", Some("Projects/소설/1장.md"));
        chat.apply(ChatEvent::Done);
        chat.send("둘", Some("Projects/소설/1장.md"));
        chat.apply(ChatEvent::Text("답".into()));
        chat.apply(ChatEvent::Done);
        let sent = chat.send("셋", Some("Projects/소설/2장.md")).unwrap();
        assert_eq!(
            sent.last().unwrap().text,
            "[The user switched to the sheet Projects/소설/2장.md.]\n\n셋"
        );
        chat.apply(ChatEvent::Text("답".into()));
        chat.apply(ChatEvent::Done);
        let sent = chat.send("넷", None).unwrap();
        assert!(
            sent.last()
                .unwrap()
                .text
                .starts_with("[The user closed the sheet")
        );
        assert_eq!(
            chat.turns()[0].sheet.as_deref(),
            Some("Projects/소설/1장.md")
        );
    }

    #[test]
    fn restores_turns_and_sites() {
        let chat = Conversation::restore(
            vec![turn(true, "질문"), turn(false, "답")],
            vec!["b.com".into(), "a.com".into()],
        );
        assert_eq!(chat.title(), Some("질문"));
        assert_eq!(chat.sites(), ["a.com", "b.com"]);
        assert!(!chat.is_streaming());
    }

    #[test]
    fn stop_drops_empty_reply_only() {
        let mut chat = Conversation::default();
        chat.send("하나", None);
        chat.stop();
        assert_eq!(chat.turns(), [turn(true, "하나")]);
        // 답 없이 멈춘 말은 다음 말과 합쳐 보낸다.
        assert_eq!(chat.send("둘", None), Some(vec![turn(true, "하나\n\n둘")]));
        chat.apply(ChatEvent::Text("받은 데까지".into()));
        chat.stop();
        assert_eq!(chat.turns().last(), Some(&turn(false, "받은 데까지")));
        assert!(!chat.is_streaming());
    }
}
