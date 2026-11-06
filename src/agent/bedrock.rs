//! 어시스턴트 대화를 모델에 보내고 답을 조각으로 받는다. 모델은 Amazon Bedrock(rig-bedrock,
//! Converse API) 또는 Alibaba Token Plan([`super::token_plan`])이고, 도구 루프는 같다.
//!
//! rig·AWS SDK는 tokio 위에서 돈다. 앱(gpui) 실행기와 섞지 않도록 tokio 런타임을 따로 두고,
//! 답 조각은 채널로 화면에 넘긴다. 받는 쪽이 채널을 버리면 다음 조각에서 요청을 멈춘다.

use futures::StreamExt as _;
use futures::channel::mpsc;
use rig_bedrock::client::BedrockRuntime;
use rig_core::completion::CompletionRequest;
use rig_core::driver::DynModel;
use rig_core::message::{
    DocumentSourceKind, Image, ImageMediaType, Message, ToolResultContent, UserContent,
};
use rig_core::operation::Completion;
use rig_core::streaming::{Item, StreamEvent};

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::channel::oneshot;

use super::tools::{PendingChange, Planned};
use super::{
    AllowedSites, Approval, Backend, ChangeCard, ChangeState, ChatEvent, FetchApproval, FetchCard,
    ModelChoice, Turn, Workspace,
};
use super::{model_tool, review, token_plan, web};

/// 설정에서 모델을 고르지 않았을 때 쓰는 모델(Bedrock 전역 교차 리전 추론 프로필).
pub const DEFAULT_MODEL: &str = "global.anthropic.claude-opus-5-5";

/// 고를 수 있는 Bedrock 모델(모델 고르기 순서). 기본은 Claude Opus 5.5.
pub const BEDROCK_MODELS: &[ModelChoice] = &[
    ModelChoice {
        name: Cow::Borrowed("GPT-6.1 Sol"),
        base: Cow::Borrowed("openai.gpt-6.1-sol"),
    },
    ModelChoice {
        name: Cow::Borrowed("GPT-6 Astra"),
        base: Cow::Borrowed("openai.gpt-6-astra"),
    },
    ModelChoice {
        name: Cow::Borrowed("Claude Sonnet 5.5"),
        base: Cow::Borrowed("anthropic.claude-sonnet-5-5"),
    },
    ModelChoice {
        name: Cow::Borrowed("Claude Opus 5.5"),
        base: Cow::Borrowed("anthropic.claude-opus-5-5"),
    },
];

/// 리전에서 쓸 수 있는 추론 프로필 중 이 모델의 것. 어느 리전으로든 보낼 수 있는 `global.`을
/// 먼저 고르고, 없으면 지역 프로필(`us.`, `eu.`, `apac.` 등)을 고른다.
fn pick<'a>(model: &ModelChoice, ids: &'a [String]) -> Option<&'a String> {
    let mut candidates = ids
        .iter()
        .filter(|id| model.matches(id) && **id != model.base);
    let first = candidates.clone().next();
    candidates.find(|id| id.starts_with("global.")).or(first)
}

/// 이 리전에서 쓸 수 있는 모델과 부를 ID([`BEDROCK_MODELS`] 순서). 추론 프로필 목록으로 판단한다.
pub(super) async fn list_models(
    profile: &str,
    region: Option<String>,
) -> Result<Vec<(ModelChoice, String)>, String> {
    let mut loader =
        aws_config::defaults(aws_config::BehaviorVersion::latest()).profile_name(profile);
    if let Some(region) = region {
        loader = loader.region(aws_config::Region::new(region));
    }
    let client = aws_sdk_bedrock::Client::new(&loader.load().await);
    let mut pages = client
        .list_inference_profiles()
        .into_paginator()
        .items()
        .send();
    let mut ids = Vec::new();
    while let Some(summary) = pages.next().await {
        let summary =
            summary.map_err(|err| aws_sdk_bedrock::error::DisplayErrorContext(err).to_string())?;
        if *summary.status() == aws_sdk_bedrock::types::InferenceProfileStatus::Active {
            ids.push(summary.inference_profile_id().to_string());
        }
    }
    Ok(models_in(&ids))
}

/// 추론 프로필 ID 목록에서 고를 수 있는 모델과 부를 ID.
fn models_in(ids: &[String]) -> Vec<(ModelChoice, String)> {
    BEDROCK_MODELS
        .iter()
        .filter_map(|model| pick(model, ids).map(|id| (model.clone(), id.clone())))
        .collect()
}

/// 한 번의 답에 쓸 수 있는 최대 출력 토큰. 스트리밍이라 넉넉히 둔다(모델의 상한이 더 낮으면 그것).
const MAX_TOKENS: u64 = 64_000;

/// 모델에 보낼 대화.
#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub backend: Backend,
    /// 도구가 읽고 쓸 수 있는 폴더(라이브러리). 없으면 도구 없이 답만 받는다.
    pub workspace: Option<Workspace>,
    /// 시스템 프롬프트.
    pub system: String,
    /// 지금까지의 대화. 마지막은 이번에 보낼 사용자 말이다.
    pub turns: Vec<Turn>,
    /// 이 대화에서 읽기를 허락한 사이트(`web_fetch`).
    pub allowed_sites: AllowedSites,
}

/// 이미지를 볼 수 없는 모델에 이미지 대신 넣는 글.
const NO_IMAGES: &str = "The image was not attached because this model did not accept it. \
Tell the user you could not look at the image itself.";

/// 도구 결과 뒤에 따로 붙인 이미지를 가리키는 글.
const IMAGE_AFTER: &str = "The image is attached right after the tool results.";

/// 도구 결과의 이미지(web_fetch가 가져온 것, run_extension이 그린 것)를 모델에 넣는 자리.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImagePlacement {
    /// 도구 결과 안(Claude).
    InToolResult,
    /// 도구 결과는 글만 두고, 같은 사용자 메시지의 도구 결과 뒤에 이미지 블록으로. 도구 결과 안의 이미지를
    /// 거절하는 모델(Bedrock의 GPT: "doesn't support the image field for user messages")도 이것은 받는다.
    AfterToolResults,
    /// 넣지 않는다(모델이 이미지를 거절했다).
    None,
}

/// 모델에 맞는 이미지 자리. 도구 결과 안의 이미지를 확실히 받는 것은 Bedrock의 Claude다.
/// Token Plan은 그림을 보는 모델만 넣는다.
fn image_placement(backend: &Backend) -> ImagePlacement {
    match backend {
        Backend::Bedrock { model, .. } if model.contains("anthropic.claude") => {
            ImagePlacement::InToolResult
        }
        Backend::Bedrock { .. } => ImagePlacement::AfterToolResults,
        Backend::TokenPlan { model, .. } => match token_plan::find(model) {
            Some(model) if !model.vision => ImagePlacement::None,
            _ => ImagePlacement::AfterToolResults,
        },
        Backend::ClaudeCode { .. } | Backend::Codex { .. } => ImagePlacement::None,
    }
}

/// 한 번의 답에 쓸 최대 출력 토큰. 모델의 상한을 넘겨 부르면 거절한다.
pub(super) fn max_tokens(backend: &Backend) -> u64 {
    match backend {
        Backend::Bedrock { .. } | Backend::ClaudeCode { .. } | Backend::Codex { .. } => MAX_TOKENS,
        Backend::TokenPlan { model, .. } => {
            token_plan::find(model).map_or(MAX_TOKENS, |model| model.max_output.min(MAX_TOKENS))
        }
    }
}

/// 이 곳에서 부를 rig 모델.
pub(super) fn completion_model(backend: &Backend) -> DynModel<Completion> {
    match backend {
        Backend::Bedrock {
            profile,
            region,
            model,
        } => {
            let mut runtime = BedrockRuntime::builder().profile_name(profile);
            if let Some(region) = region {
                runtime = runtime.region(region);
            }
            runtime.build().completion(model).erase()
        }
        Backend::TokenPlan { api_key, model } => token_plan::completion_model(api_key, model),
        // 에이전트는 rig로 부르지 않는다: 어시스턴트는 ACP로 가고(`stream`), 모델 한 번 부르기는
        // `Backend::llm_from_prefs`로 에이전트를 거른다.
        Backend::ClaudeCode { .. } | Backend::Codex { .. } => {
            unreachable!("에이전트 제공자는 rig 모델이 없다")
        }
    }
}

/// 모델 오류가 이미지 때문인지.
fn mentions_image(error: &str) -> bool {
    error.to_ascii_lowercase().contains("image")
}

/// 대화에서 이미지(도구 결과 안과 따로 붙인 것)를 빼고 그 자리에 알림 글을 둔다. 뺀 것이 있으면 `true`.
fn strip_images(messages: &mut [Message]) -> bool {
    let mut stripped = false;
    for message in messages {
        let Message::User { content } = message else {
            continue;
        };
        for item in content.iter_mut() {
            match item {
                UserContent::ToolResult(result) => {
                    let before = result.content.len();
                    result
                        .content
                        .retain(|part| !matches!(part, ToolResultContent::Image(_)));
                    if result.content.len() != before {
                        result.content.push(ToolResultContent::text(NO_IMAGES));
                        stripped = true;
                    }
                }
                UserContent::Image(_) => {
                    *item = UserContent::text(NO_IMAGES);
                    stripped = true;
                }
                _ => {}
            }
        }
    }
    stripped
}

fn fetched_image(image: web::FetchedImage) -> Image {
    Image {
        data: DocumentSourceKind::Raw(image.bytes),
        media_type: Some(match image.format {
            web::ImageFormat::Png => ImageMediaType::PNG,
            web::ImageFormat::Jpeg => ImageMediaType::JPEG,
            web::ImageFormat::Gif => ImageMediaType::GIF,
            web::ImageFormat::Webp => ImageMediaType::WEBP,
        }),
        ..Image::default()
    }
}

/// `web_fetch`: 처음 보는 사이트면 허락을 묻고, 읽어서 모델에 줄 도구 결과와 (도구 결과 뒤에 붙일) 이미지를
/// 돌려준다. 사용자가 답을 멈췄으면 `None`.
async fn web_fetch(
    arguments: &serde_json::Map<String, serde_json::Value>,
    allowed: &AllowedSites,
    images: ImagePlacement,
    tx: &mpsc::UnboundedSender<ChatEvent>,
) -> Option<(Vec<ToolResultContent>, Option<Image>)> {
    let text = |text: String| Some((vec![ToolResultContent::text(text)], None));
    let fetch = match web::parse(arguments) {
        Ok(parsed) => parsed,
        Err(err) => return text(format!("Error: {err}")),
    };
    let url = &fetch.url;
    let site = web::site(url);
    let known = allowed.lock().is_ok_and(|sites| sites.contains(&site));
    if !known {
        let id = next_change_id();
        let (reply, answer) = oneshot::channel();
        let card = FetchCard {
            id,
            url: url.to_string(),
            site: site.clone(),
            state: ChangeState::Pending,
        };
        tx.unbounded_send(ChatEvent::ApproveFetch(FetchApproval { card, reply }))
            .ok()?;
        match answer.await {
            Ok(true) => {
                if let Ok(mut sites) = allowed.lock() {
                    sites.insert(site);
                }
            }
            Ok(false) => return text(FETCH_DECLINED.into()),
            Err(_) => return None,
        }
    }
    let sites = allowed
        .lock()
        .map(|sites| sites.clone())
        .unwrap_or_default();
    let shown = web::shown(url);
    let fetched = web::fetch(fetch.clone(), &sites).await;
    let summary = if fetched.ok {
        format!("{} · {shown}", t!("assistant.tool.fetched"))
    } else {
        t!("assistant.tool.fetch_failed", url = shown).into()
    };
    tx.unbounded_send(ChatEvent::Tool(summary)).ok()?;
    let mut content = vec![ToolResultContent::text(fetched.text)];
    let after = fetched
        .image
        .and_then(|image| attach(&mut content, fetched_image(image), images));
    Some((content, after))
}

/// 도구 결과에 이미지를 모델이 받는 자리로 넣는다. 도구 결과 뒤에 붙여야 하면 그 이미지를 돌려준다.
fn attach(
    content: &mut Vec<ToolResultContent>,
    image: Image,
    placement: ImagePlacement,
) -> Option<Image> {
    match placement {
        ImagePlacement::InToolResult => content.push(ToolResultContent::Image(image)),
        ImagePlacement::AfterToolResults => {
            content.push(ToolResultContent::text(IMAGE_AFTER));
            return Some(image);
        }
        ImagePlacement::None => content.push(ToolResultContent::text(NO_IMAGES)),
    }
    None
}

/// 확장 그림을 대화 맥락 없이 검토받는다(ADR-0030). 검토 글을 돌려준다.
async fn critique(
    model: &DynModel<Completion>,
    prompt: String,
    png: Vec<u8>,
) -> Result<String, String> {
    let image = Image {
        data: DocumentSourceKind::Raw(png),
        media_type: Some(ImageMediaType::PNG),
        ..Image::default()
    };
    let message = Message::User {
        content: vec![UserContent::text(prompt), UserContent::Image(image)],
    };
    let completion = CompletionRequest::new(message)
        .preamble(review::SYSTEM)
        .max_tokens(REVIEW_MAX_TOKENS);
    let mut stream = model.stream(completion).map_err(|err| err.to_string())?;
    while let Some(item) = stream.next().await {
        item.map_err(|err| err.to_string())?;
    }
    let response = stream.finish().await.map_err(|err| err.to_string())?;
    Ok(response.text())
}

/// 검토 답의 최대 출력 토큰.
const REVIEW_MAX_TOKENS: u64 = 4_000;

/// 변경을 승인 카드로 묻고, 승인하면 쓴다. 모델에 돌려줄 글, 사용자가 답을 멈췄으면 `None`.
async fn approve(
    workspace: &Workspace,
    change: PendingChange,
    tx: &mpsc::UnboundedSender<ChatEvent>,
) -> Option<String> {
    let id = next_change_id();
    let (reply, answer) = oneshot::channel();
    let card = ChangeCard {
        id,
        path: change.path.clone(),
        new_file: change.is_new(),
        lines: change.diff(),
        state: ChangeState::Pending,
    };
    tx.unbounded_send(ChatEvent::Approve(Approval { card, reply }))
        .ok()?;
    match answer.await.ok()? {
        true => {
            let output = workspace.commit(change);
            let event = if output.text.starts_with("Error:") {
                ChatEvent::ChangeFailed(id)
            } else {
                ChatEvent::Changed
            };
            tx.unbounded_send(event).ok();
            Some(output.text)
        }
        false => Some(DECLINED.to_string()),
    }
}

/// 답이 끝난 뒤 이번 답의 확장 초안(ADR-0029)을 확장마다 한 번 묻는다. 사용자가 멈추면 남은 초안은 버린다.
async fn approve_drafts(workspace: &Workspace, tx: &mpsc::UnboundedSender<ChatEvent>) {
    for draft in workspace.take_drafts() {
        if approve(workspace, draft, tx).await.is_none() {
            return;
        }
    }
}

/// tokio 런타임. 처음 대화를 보낼 때 만든다.
pub(super) fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("hanji-ai")
            .enable_all()
            .build()
            .expect("tokio 런타임을 만들 수 없습니다")
    })
}

/// 대화를 보내고, 답 조각을 받을 채널을 돌려준다. 마지막 일은 언제나 `Done` 또는 `Failed`다.
pub fn stream(request: ChatRequest) -> mpsc::UnboundedReceiver<ChatEvent> {
    let (tx, rx) = mpsc::unbounded();
    runtime().spawn(async move {
        let result = match &request.backend {
            Backend::ClaudeCode { model } => run_claude_code(&request, model, &tx).await,
            Backend::Codex { model } => run_codex(&request, model, &tx).await,
            _ => run(&request, &tx).await,
        };
        if let Some(workspace) = &request.workspace {
            approve_drafts(workspace, &tx).await;
        }
        let last = match result {
            Ok(()) => ChatEvent::Done,
            Err(message) => ChatEvent::Failed(message),
        };
        tx.unbounded_send(last).ok();
    });
    rx
}

/// 에이전트가 일할 라이브러리 루트와 보낼 말.
fn agent_prompt(request: &ChatRequest) -> Result<super::acp::Prompt, String> {
    let cwd = request
        .workspace
        .as_ref()
        .map(|workspace| workspace.root().to_path_buf())
        .ok_or_else(|| t!("assistant.error.agent_needs_library").to_string())?;
    Ok(super::acp::Prompt {
        cwd,
        turns: request.turns.clone(),
    })
}

/// Claude Code(ACP 어댑터, ADR-0057)로 답을 받는다. 라이브러리 루트에서 일한다.
async fn run_claude_code(
    request: &ChatRequest,
    model: &str,
    tx: &mpsc::UnboundedSender<ChatEvent>,
) -> Result<(), String> {
    let prompt = agent_prompt(request)?;
    let agent = super::acp::claude::ClaudeCode {
        path: crate::config::Config::load().claude_code_path,
        model: model.to_string(),
        system: request.system.clone(),
    };
    super::acp::run(agent.into_agent(), prompt, tx).await
}

/// Codex(ACP 어댑터, ADR-0057)로 답을 받는다. 라이브러리 루트에서 일한다.
async fn run_codex(
    request: &ChatRequest,
    model: &str,
    tx: &mpsc::UnboundedSender<ChatEvent>,
) -> Result<(), String> {
    let prompt = agent_prompt(request)?;
    let agent = super::acp::codex::Codex {
        path: crate::config::Config::load().codex_path,
        model: (!model.is_empty()).then(|| model.to_string()),
        system: request.system.clone(),
    };
    super::acp::run(agent.into_agent(), prompt, tx).await
}

/// 사용자가 변경을 거절했을 때 모델에 돌려줄 글.
const DECLINED: &str = "The user declined this change. Do not retry it unless they ask; you may ask what they would like instead.";

/// 사용자가 사이트 읽기를 거절했을 때 모델에 돌려줄 글.
const FETCH_DECLINED: &str = "The user did not allow reading this site. Do not try it again unless they ask; answer without it or ask for another source.";

/// 승인 카드 번호. 대화가 달라도 겹치지 않게 앱 전체에서 센다.
static NEXT_CHANGE: AtomicU64 = AtomicU64::new(1);

/// 새 승인 카드 번호.
pub(super) fn next_change_id() -> u64 {
    NEXT_CHANGE.fetch_add(1, Ordering::Relaxed)
}

/// 도구를 부르고 다시 묻기를 이만큼까지만 되풀이한다(끝없이 도는 것을 막는다).
/// 확장은 초안을 고치고 그려 보기를 여러 번 되풀이하므로 넉넉히 둔다.
const MAX_TOOL_ROUNDS: usize = 60;

async fn run(request: &ChatRequest, tx: &mpsc::UnboundedSender<ChatEvent>) -> Result<(), String> {
    if request.turns.is_empty() {
        return Err(t!("assistant.error.nothing_to_send").into());
    }
    let backend = &request.backend;
    let model = completion_model(backend);
    let max_tokens = max_tokens(backend);
    let tools = request
        .workspace
        .as_ref()
        .map(Workspace::definitions)
        .unwrap_or_default();

    // 이번 답 동안 쌓이는 대화: 지난 말들 + 도구 호출과 그 결과.
    let mut messages: Vec<Message> = request
        .turns
        .iter()
        .map(|turn| {
            if turn.user {
                Message::user(&turn.text)
            } else {
                Message::assistant(&turn.text)
            }
        })
        .collect();

    // 도구 결과 이미지를 넣을 자리. 그래도 모델이 거절하면 이미지를 빼고 한 번 다시 보낸다.
    let mut tool_images = image_placement(backend);
    let mut retried_without_images = false;
    // 확장 이름 → (마지막으로 검토한 소스, 점수), 검토를 통과하지 못해 돌려보낸 횟수(ADR-0030).
    let mut reviews: HashMap<String, (String, Option<u8>)> = HashMap::new();
    let mut send_backs = 0;
    let user_request = request
        .turns
        .iter()
        .rev()
        .find(|turn| turn.user)
        .map_or("", |turn| turn.text.as_str());
    for _ in 0..MAX_TOOL_ROUNDS {
        let response = loop {
            let Some((prompt, history)) = messages.split_last() else {
                return Ok(());
            };
            let completion = CompletionRequest::new(prompt.clone())
                .preamble(&request.system)
                .messages(history.iter().cloned())
                .tools(tools.iter().cloned())
                .max_tokens(max_tokens);
            let result: Result<Option<_>, String> = async {
                let mut stream = model.stream(completion).map_err(|err| err.to_string())?;
                while let Some(item) = stream.next().await {
                    // 글 조각만 화면에 넘긴다(도구 호출은 다 받은 뒤 응답에서 읽는다).
                    if let Item::Event(StreamEvent::Text { text, .. }) =
                        item.map_err(|err| err.to_string())?
                        // 받는 쪽(화면)이 대화를 버렸으면 그만 받는다.
                        && tx.unbounded_send(ChatEvent::Text(text)).is_err()
                    {
                        return Ok(None);
                    }
                }
                stream
                    .finish()
                    .await
                    .map(Some)
                    .map_err(|err| err.to_string())
            }
            .await;
            match result {
                Ok(Some(response)) => break response,
                Ok(None) => return Ok(()),
                Err(err)
                    if !retried_without_images
                        && mentions_image(&err)
                        && strip_images(&mut messages) =>
                {
                    retried_without_images = true;
                    tool_images = ImagePlacement::None;
                }
                Err(err) => return Err(err),
            }
        };
        let calls: Vec<_> = response.tool_calls().cloned().collect();
        let Some(workspace) = &request.workspace else {
            return Ok(());
        };
        if calls.is_empty() {
            // 끝내려는데 검토를 통과하지 못한 확장 초안이 있으면 몇 번은 돌려보낸다.
            let problems = workspace.draft_problems(&reviews);
            let Some(back) =
                review::send_back(&problems).filter(|_| send_backs < review::MAX_SEND_BACKS)
            else {
                return Ok(());
            };
            send_backs += 1;
            messages.extend(response.message());
            messages.push(Message::user(back));
            if tx.unbounded_send(ChatEvent::Text("\n\n".into())).is_err()
                || tx
                    .unbounded_send(ChatEvent::Tool(t!("assistant.tool.sent_back").into()))
                    .is_err()
            {
                return Ok(());
            }
            continue;
        }
        messages.extend(response.message());
        let mut results = Vec::with_capacity(calls.len());
        // 도구 결과 뒤에 붙일 이미지(`ImagePlacement::AfterToolResults`).
        let mut after_results: Vec<Image> = Vec::new();
        for call in &calls {
            if call.function.name == web::WEB_FETCH {
                let Some((content, image)) = web_fetch(
                    &call.function.arguments,
                    &request.allowed_sites,
                    tool_images,
                    tx,
                )
                .await
                else {
                    return Ok(());
                };
                results.push(call.result(content));
                after_results.extend(image);
                continue;
            }
            if call.function.name == model_tool::SET_MODEL {
                let outcome = model_tool::run(backend, &call.function.arguments).await;
                let summary = match &outcome.model {
                    Some(model) => t!("assistant.tool.model_set", model = model),
                    None => t!("assistant.tool.models_listed"),
                };
                if tx.unbounded_send(ChatEvent::Tool(summary.into())).is_err()
                    || outcome
                        .model
                        .is_some_and(|model| tx.unbounded_send(ChatEvent::SetModel(model)).is_err())
                {
                    return Ok(());
                }
                results.push(call.result(vec![ToolResultContent::text(outcome.text)]));
                continue;
            }
            let text = match workspace.plan(&call.function.name, &call.function.arguments) {
                Planned::Done(output) => {
                    if tx.unbounded_send(ChatEvent::Tool(output.summary)).is_err() {
                        return Ok(());
                    }
                    // run_extension이 그린 그림. 볼 수 있는 모델이면 따로 검토도 받는다(ADR-0030).
                    if let Some(drawing) = output.image {
                        let name = call
                            .function
                            .arguments
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let mut text = output.text;
                        let score = if tool_images == ImagePlacement::None {
                            None
                        } else {
                            let goal = call
                                .function
                                .arguments
                                .get("goal")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let prompt = review::request(
                                user_request,
                                goal,
                                name,
                                drawing.canvas,
                                drawing.frames,
                            );
                            let shown = workspace.extension_shown(name);
                            match critique(&model, prompt, drawing.png.clone()).await {
                                Ok(review) => {
                                    let score = review::score(&review);
                                    let event = ChatEvent::Review {
                                        file: shown.clone(),
                                        score,
                                    };
                                    if tx.unbounded_send(event).is_err() {
                                        return Ok(());
                                    }
                                    text.push_str(&review::report(&review, score));
                                    score
                                }
                                Err(err) => {
                                    let summary =
                                        format!("{} · {shown}", t!("assistant.tool.review_failed"));
                                    tx.unbounded_send(ChatEvent::Tool(summary)).ok();
                                    text.push_str(&format!(
                                        "\n\nThe independent review failed: {err}"
                                    ));
                                    None
                                }
                            }
                        };
                        reviews.insert(name.to_string(), (drawing.source, score));
                        let mut content = vec![ToolResultContent::text(text)];
                        let image = Image {
                            data: DocumentSourceKind::Raw(drawing.png),
                            media_type: Some(ImageMediaType::PNG),
                            ..Image::default()
                        };
                        after_results.extend(attach(&mut content, image, tool_images));
                        results.push(call.result(content));
                        continue;
                    }
                    output.text
                }
                // 파일을 바꾸는 도구는 승인 카드를 띄우고 사용자의 답을 기다린다.
                Planned::Change(change) => match approve(workspace, change, tx).await {
                    Some(text) => text,
                    None => return Ok(()),
                },
            };
            results.push(call.result(vec![ToolResultContent::text(text)]));
        }
        let mut content: Vec<UserContent> =
            results.into_iter().map(UserContent::ToolResult).collect();
        content.extend(after_results.into_iter().map(UserContent::Image));
        messages.push(Message::User { content });
    }
    Err(t!(
        "assistant.error.too_many_tool_rounds",
        count = MAX_TOOL_ROUNDS
    )
    .into())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn strips_images_for_models_that_refuse_them() {
        let bedrock = |model: &str| Backend::Bedrock {
            profile: "default".into(),
            region: None,
            model: model.into(),
        };
        let token_plan = |model: &str| Backend::TokenPlan {
            api_key: "sk-sp-test".into(),
            model: model.into(),
        };
        assert_eq!(
            image_placement(&bedrock("global.anthropic.claude-opus-5-5")),
            ImagePlacement::InToolResult
        );
        assert_eq!(
            image_placement(&bedrock("global.openai.gpt-6.1-sol")),
            ImagePlacement::AfterToolResults
        );
        assert_eq!(
            image_placement(&token_plan("qwen3.8-max")),
            ImagePlacement::AfterToolResults
        );
        assert_eq!(image_placement(&token_plan("glm-5")), ImagePlacement::None);
        assert_eq!(max_tokens(&token_plan("glm-5")), 16_384);
        assert_eq!(max_tokens(&token_plan("qwen3.8-max")), MAX_TOKENS);
        assert!(mentions_image(
            "ValidationException: The model does not support image content"
        ));

        let image = ToolResultContent::Image(Image {
            data: DocumentSourceKind::Raw(vec![1, 2, 3]),
            media_type: Some(ImageMediaType::PNG),
            ..Image::default()
        });
        let name = rig_core::message::ToolName::new("web_fetch").unwrap();
        let mut messages = vec![
            Message::user("보여 줘"),
            Message::User {
                content: vec![UserContent::tool_result(
                    rig_core::message::CallId::from(rig_core::message::LocalCallId::new()),
                    name,
                    vec![ToolResultContent::text("Fetched the image"), image],
                )],
            },
        ];
        assert_eq!(messages.iter().flat_map(Message::images).count(), 1);
        assert!(strip_images(&mut messages));
        assert_eq!(messages.iter().flat_map(Message::images).count(), 0);
        let Message::User { content } = &messages[1] else {
            panic!()
        };
        let UserContent::ToolResult(result) = &content[0] else {
            panic!()
        };
        assert_eq!(result.content.len(), 2);
        assert!(!strip_images(&mut messages));
    }

    #[test]
    fn picks_global_profile_first_and_skips_missing_models() {
        let ids: Vec<String> = [
            "us.anthropic.claude-opus-5-5",
            "global.anthropic.claude-opus-5-5",
            "eu.anthropic.claude-sonnet-5-5",
            "global.openai.gpt-6-sol",
            "us.openai.gpt-6-astra",
        ]
        .map(String::from)
        .into();
        let models = models_in(&ids);
        let picked: Vec<_> = models
            .iter()
            .map(|(m, id)| (m.name.as_ref(), id.as_str()))
            .collect();
        assert_eq!(
            picked,
            [
                ("GPT-6 Astra", "us.openai.gpt-6-astra"),
                ("Claude Sonnet 5.5", "eu.anthropic.claude-sonnet-5-5"),
                ("Claude Opus 5.5", "global.anthropic.claude-opus-5-5"),
            ]
        );
        // gpt-6-sol 프로필은 gpt-6.1-sol이 아니다.
        assert!(!BEDROCK_MODELS[0].matches("global.openai.gpt-6-sol"));
    }

    /// 실제 모델 목록을 읽는다([`live_backend`]).
    /// `WRITER_AI_PROFILE=<프로필> WRITER_AI_REGION=<리전> cargo test -- --ignored live_models`
    /// (Token Plan은 `WRITER_TOKEN_PLAN_KEY=sk-sp-…`)
    #[test]
    #[ignore]
    fn live_models() {
        let rx = live_backend().available_models();
        let models = futures::executor::block_on(rx).expect("응답 없음");
        println!("models={models:?}");
        assert!(models.is_ok());
    }

    /// 실제 Bedrock으로 확장 그림을 검토받는다.
    /// `WRITER_AI_PROFILE=<프로필> WRITER_AI_REGION=<리전> WRITER_REVIEW_PNG=<그림> cargo test -- --ignored live_review --nocapture`
    #[test]
    #[ignore]
    fn live_review() {
        let profile = std::env::var("WRITER_AI_PROFILE").expect("WRITER_AI_PROFILE");
        let png =
            std::fs::read(std::env::var("WRITER_REVIEW_PNG").expect("WRITER_REVIEW_PNG")).unwrap();
        let mut builder = BedrockRuntime::builder().profile_name(&profile);
        if let Ok(region) = std::env::var("WRITER_AI_REGION") {
            builder = builder.region(&region);
        }
        let model = builder.build().completion(DEFAULT_MODEL).erase();
        let prompt = review::request("빨간 머리 여자를 그려 줘", "", "test", (400.0, 400.0), 1);
        let review = runtime().block_on(critique(&model, prompt, png)).unwrap();
        println!("{review}");
        assert!(review::score(&review).is_some());
    }

    /// 실제 모델을 부를 곳. `WRITER_TOKEN_PLAN_KEY`가 있으면 Token Plan,
    /// 없으면 `WRITER_AI_PROFILE`의 Bedrock. 모델은 `WRITER_AI_MODEL`, 없으면 그 제공자의 기본.
    pub(in crate::agent) fn live_backend() -> Backend {
        let model = std::env::var("WRITER_AI_MODEL").ok();
        if let Ok(api_key) = std::env::var("WRITER_TOKEN_PLAN_KEY") {
            return Backend::TokenPlan {
                api_key,
                model: model.unwrap_or_else(|| token_plan::DEFAULT_MODEL.into()),
            };
        }
        Backend::Bedrock {
            profile: std::env::var("WRITER_AI_PROFILE").expect("WRITER_AI_PROFILE"),
            region: std::env::var("WRITER_AI_REGION").ok(),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
        }
    }

    /// 실제 모델을 부른다([`live_backend`]). `WRITER_AI_PROFILE=<프로필> cargo test -- --ignored live_stream`
    #[test]
    #[ignore]
    fn live_stream() {
        let mut rx = stream(ChatRequest {
            backend: live_backend(),
            workspace: None,
            system: "짧게 답하세요.".into(),
            turns: vec![Turn {
                user: true,
                text: "한 문장으로 인사해 줘.".into(),
                ..Turn::default()
            }],
            allowed_sites: AllowedSites::default(),
        });
        let mut chunks = 0;
        let mut text = String::new();
        let last = futures::executor::block_on(async {
            loop {
                match rx.next().await {
                    Some(ChatEvent::Text(piece)) => {
                        chunks += 1;
                        text.push_str(&piece);
                    }
                    Some(other) => return Some(other),
                    None => return None,
                }
            }
        });
        println!("chunks={chunks} text={text:?} last={last:?}");
        assert!(matches!(last, Some(ChatEvent::Done)), "{last:?}");
        assert!(!text.is_empty());
    }
}
/// 실제 모델로 도구 루프를 돈다(도구 호출이 사라지지 않는지, web_fetch 이미지).
/// `WRITER_AI_PROFILE=<프로필> [WRITER_AI_REGION=…] [WRITER_AI_MODEL=…] [Q=질문] cargo test -- --ignored live_tools --nocapture`
/// (Token Plan은 `WRITER_AI_PROFILE` 대신 `WRITER_TOKEN_PLAN_KEY=sk-sp-…`)
#[cfg(test)]
mod live_tools {
    use super::*;
    #[test]
    #[ignore]
    fn live_tools() {
        let root = std::env::temp_dir().join("hanji-live-tools");
        let _ = std::fs::remove_dir_all(&root);
        crate::library::initialize(&root).unwrap();
        crate::agent::skills::sync_builtin(&root).unwrap();
        crate::extension::ensure_dir(&root).unwrap();
        std::fs::write(
            root.join("Notes/Inbox/a.md"),
            "# 첫 메모\n\n오늘은 비가 왔다. 우산을 잃어버렸다.\n",
        )
        .unwrap();
        let ws = Workspace::new(&root);
        let system = crate::agent::system_prompt(None, true, ws.skills(), ws.extensions());
        let question = std::env::var("Q")
            .unwrap_or_else(|_| "Notes/Inbox에 있는 메모를 한 줄로 요약해 줘.".into());
        let mut rx = stream(ChatRequest {
            backend: tests::live_backend(),
            workspace: Some(ws),
            system,
            turns: vec![Turn {
                user: true,
                text: question,
                ..Turn::default()
            }],
            allowed_sites: AllowedSites::default(),
        });
        let start = std::time::Instant::now();
        futures::executor::block_on(async {
            while let Some(event) = rx.next().await {
                let t = start.elapsed().as_secs_f32();
                let finished = matches!(event, ChatEvent::Done | ChatEvent::Failed(_));
                match event {
                    ChatEvent::Text(piece) => println!("{t:6.2} text {piece:?}"),
                    ChatEvent::ApproveFetch(approval) => {
                        println!("{t:6.2} approve {}", approval.card.url);
                        approval.reply.send(true).ok();
                    }
                    other => println!("{t:6.2} {other:?}"),
                }
                if finished {
                    break;
                }
            }
        });
        println!("total {:?}", start.elapsed());
        std::fs::remove_dir_all(&root).ok();
    }
}
