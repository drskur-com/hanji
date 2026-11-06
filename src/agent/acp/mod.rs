//! ACP(Agent Client Protocol) 클라이언트(ADR-0057). 에이전트가 보내는 알림과 권한 질문을 어시스턴트 패널의
//! [`ChatEvent`]로 바꾼다. 에이전트는 앱 안의 어댑터([`claude`])이거나, 나중에 stdio로 띄운 ACP 에이전트다.

mod adapter;
pub mod claude;
pub mod codex;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCallContent, ToolCallStatus, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectTo};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, pin_mut};

use super::{Approval, ChangeCard, ChangeState, ChatEvent, Turn, diff};

/// 에이전트에 보낼 말 한 번.
pub struct Prompt {
    /// 작업 폴더(라이브러리 루트).
    pub cwd: PathBuf,
    /// 지금까지의 대화. 마지막은 이번에 보낼 사용자 말이다.
    pub turns: Vec<Turn>,
}

/// `agent`에 말을 보내고 끝날 때까지 [`ChatEvent`]를 `tx`로 넘긴다. 받는 쪽(화면)이 대화를 버리면 멈춘다.
pub async fn run(
    agent: impl ConnectTo<Client> + 'static,
    prompt: Prompt,
    tx: &mpsc::UnboundedSender<ChatEvent>,
) -> Result<(), String> {
    let text = prompt_text(&prompt.turns).ok_or_else(|| t!("assistant.error.nothing_to_send"))?;
    let cwd = prompt.cwd;
    // 승인 카드를 보인 도구 호출 → 카드 번호. 다 쓰였다는 알림이 오면 `Changed`·`ChangeFailed`로 바꾼다.
    let cards: Arc<Mutex<HashMap<String, u64>>> = Arc::default();

    let updates_tx = tx.clone();
    let updates_cards = cards.clone();
    let permission_tx = tx.clone();
    let permission_cards = cards;
    let permission_root = cwd.clone();
    let watch_tx = tx.clone();

    let connection = Client
        .v1()
        .name("hanji")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let event = match notification.update {
                    SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                        ContentBlock::Text(text) => Some(ChatEvent::Text(text.text)),
                        _ => None,
                    },
                    SessionUpdate::ToolCall(call) => {
                        tool_line(call.kind, &call.title).map(ChatEvent::Tool)
                    }
                    SessionUpdate::ToolCallUpdate(update) => {
                        let card = updates_cards
                            .lock()
                            .expect("cards")
                            .get(update.tool_call_id.0.as_ref())
                            .copied();
                        match (card, update.fields.status) {
                            (Some(_), Some(ToolCallStatus::Completed)) => Some(ChatEvent::Changed),
                            (Some(id), Some(ToolCallStatus::Failed)) => {
                                Some(ChatEvent::ChangeFailed(id))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(event) = event {
                    // 화면이 대화를 버렸으면 연결을 끝낸다(에이전트 프로세스도 끝난다).
                    updates_tx
                        .unbounded_send(event)
                        .map_err(|_| agent_client_protocol::Error::request_cancelled())?;
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, cx: ConnectionTo<Agent>| {
                let tx = permission_tx.clone();
                let cards = permission_cards.clone();
                let root = permission_root.clone();
                // 사용자의 답을 기다리는 동안 다른 메시지를 막지 않게 따로 돈다.
                cx.spawn(async move {
                    let outcome = ask_permission(&request, &root, &tx, &cards).await;
                    responder.respond(RequestPermissionResponse::new(outcome))
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = cx
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            let session_id = session.session_id;
            let prompt = cx
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new(text))],
                ))
                .block_task();
            pin_mut!(prompt);
            // 화면이 대화를 버리면(멈춤) 에이전트에 그만하라고 알린다.
            let closed = closed(&watch_tx).fuse();
            pin_mut!(closed);
            let response = futures::select! {
                response = prompt.as_mut().fuse() => response?,
                () = closed => {
                    cx.send_notification(CancelNotification::new(session_id))?;
                    return Ok(());
                }
            };
            match response.stop_reason {
                StopReason::MaxTokens | StopReason::MaxTurnRequests => {
                    Err(agent_client_protocol::Error::new(
                        -32603,
                        t!("assistant.error.agent_stopped_early").to_string(),
                    ))
                }
                StopReason::Refusal => Err(agent_client_protocol::Error::new(
                    -32603,
                    t!("assistant.error.agent_refused").to_string(),
                )),
                _ => {
                    if let Some(event) = resolved_model(response.meta.as_ref()) {
                        watch_tx.unbounded_send(event).ok();
                    }
                    Ok(())
                }
            }
        });
    connection.await.map_err(|err| err.message)
}

/// 답의 `_meta`에 담긴 실제 모델([`claude::MODEL_META`]).
fn resolved_model(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> Option<ChatEvent> {
    let model = meta?.get(claude::MODEL_META)?;
    Some(ChatEvent::ModelResolved {
        requested: model["requested"].as_str()?.to_string(),
        resolved: model["resolved"].as_str()?.to_string(),
    })
}

/// 화면이 받는 쪽을 버릴 때까지 기다린다.
async fn closed(tx: &mpsc::UnboundedSender<ChatEvent>) {
    while !tx.is_closed() {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 권한 질문을 승인 카드로 보이고 사용자의 답을 고른 선택지로 돌려준다. 바꿀 내용(diff)이 없으면 거절한다.
async fn ask_permission(
    request: &RequestPermissionRequest,
    root: &Path,
    tx: &mpsc::UnboundedSender<ChatEvent>,
    cards: &Mutex<HashMap<String, u64>>,
) -> RequestPermissionOutcome {
    let pick = |kinds: &[PermissionOptionKind]| {
        kinds.iter().find_map(|kind| {
            request
                .options
                .iter()
                .find(|option| option.kind == *kind)
                .map(|option| {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        option.option_id.clone(),
                    ))
                })
        })
    };
    let reject = || {
        pick(&[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ])
        .unwrap_or(RequestPermissionOutcome::Cancelled)
    };
    let diffs: Vec<_> = request
        .tool_call
        .fields
        .content
        .iter()
        .flatten()
        .filter_map(|content| match content {
            ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        })
        .collect();
    let Some(first) = diffs.first() else {
        return reject();
    };
    let lines_of = |diff: &agent_client_protocol::v1::schema::Diff| match &diff.old_text {
        Some(before) => diff::diff(before, &diff.new_text),
        None => diff::added(&diff.new_text),
    };
    let id = super::bedrock::next_change_id();
    let card = ChangeCard {
        id,
        path: relative(root, &first.path),
        new_file: diffs.iter().all(|diff| diff.old_text.is_none()),
        // 파일이 여럿이면(Codex의 패치 하나) 파일마다 머리줄을 둔다.
        lines: if diffs.len() == 1 {
            lines_of(first)
        } else {
            diffs
                .iter()
                .flat_map(|diff| {
                    std::iter::once(diff::file(&relative(root, &diff.path))).chain(lines_of(diff))
                })
                .collect()
        },
        state: ChangeState::Pending,
    };
    let (reply, answer) = oneshot::channel();
    if tx
        .unbounded_send(ChatEvent::Approve(Approval { card, reply }))
        .is_err()
    {
        return RequestPermissionOutcome::Cancelled;
    }
    match answer.await {
        Ok(true) => {
            cards
                .lock()
                .expect("cards")
                .insert(request.tool_call.tool_call_id.0.to_string(), id);
            pick(&[
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
            ])
            .unwrap_or_else(reject)
        }
        Ok(false) => reject(),
        // 화면이 카드를 버렸다(멈춤·새 대화).
        Err(_) => RequestPermissionOutcome::Cancelled,
    }
}

/// 읽기·찾기 도구 호출의 화면 한 줄. 스킬 파일을 읽으면 스킬 이름으로 보인다.
fn tool_line(kind: ToolKind, target: &str) -> Option<String> {
    match kind {
        ToolKind::Read => Some(match skill_name(target) {
            Some(name) => t!("assistant.tool.skill", name = name).to_string(),
            None => format!("{} · {target}", t!("assistant.tool.read")),
        }),
        ToolKind::Search if target.is_empty() => Some(t!("assistant.tool.listed").to_string()),
        ToolKind::Search => Some(format!("{} · {target}", t!("assistant.tool.listed"))),
        _ => None,
    }
}

/// `Skills/<이름>/SKILL.md`면 스킬 이름.
fn skill_name(path: &str) -> Option<&str> {
    let rest = path
        .strip_prefix(super::skills::SKILLS_DIR)?
        .strip_prefix('/')?;
    rest.strip_suffix("/SKILL.md")
        .filter(|name| !name.contains('/'))
}

/// 라이브러리 루트 기준 경로(`/`로 잇는다). 루트 밖이면 받은 그대로.
pub(super) fn relative(root: &Path, path: &Path) -> String {
    let strip = |root: &Path, path: &Path| {
        path.strip_prefix(root).ok().map(|rest| {
            rest.components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
    };
    if let Some(rest) = strip(root, path) {
        return rest;
    }
    // macOS의 `/tmp` → `/private/tmp`처럼 링크로 갈라진 경로. 새 파일은 폴더만 있다.
    let canonical = |path: &Path| {
        path.canonicalize().ok().or_else(|| {
            let parent = path.parent()?.canonicalize().ok()?;
            Some(parent.join(path.file_name()?))
        })
    };
    canonical(root)
        .zip(canonical(path))
        .and_then(|(root, path)| strip(&root, &path))
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// 에이전트에 보낼 글. 앞의 말이 있으면 `<conversation>`에 묶어 앞에 둔다(에이전트는 세션을 남기지 않는다).
fn prompt_text(turns: &[Turn]) -> Option<String> {
    let (last, earlier) = turns.split_last()?;
    if !last.user || last.text.trim().is_empty() {
        return None;
    }
    let earlier: Vec<&Turn> = earlier
        .iter()
        .filter(|turn| !turn.text.trim().is_empty())
        .collect();
    if earlier.is_empty() {
        return Some(last.text.clone());
    }
    let mut text = String::from("<conversation>\n");
    for turn in earlier {
        let role = if turn.user { "user" } else { "assistant" };
        text.push_str(&format!(
            "<{role}>\n{}\n</{role}>\n",
            turn.text.replace("</conversation", "&lt;/conversation")
        ));
    }
    text.push_str("</conversation>\n\n");
    text.push_str(&last.text);
    Some(text)
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
    fn prompt_wraps_earlier_turns() {
        assert_eq!(prompt_text(&[turn(true, "hi")]).as_deref(), Some("hi"));
        assert_eq!(prompt_text(&[turn(false, "x")]), None);
        let text = prompt_text(&[turn(true, "a"), turn(false, "b"), turn(true, "c")]).unwrap();
        assert_eq!(
            text,
            "<conversation>\n<user>\na\n</user>\n<assistant>\nb\n</assistant>\n</conversation>\n\nc"
        );
    }

    #[test]
    fn relative_paths_and_skill_names() {
        let root = Path::new("/lib");
        assert_eq!(
            relative(root, Path::new("/lib/Projects/a.md")),
            "Projects/a.md"
        );
        assert_eq!(
            relative(root, Path::new("/elsewhere/a.md")),
            "/elsewhere/a.md"
        );
        assert_eq!(skill_name("Skills/proofread/SKILL.md"), Some("proofread"));
        assert_eq!(skill_name("Skills/proofread/references/ko.md"), None);
    }
}

/// 실제 `claude`로 끝까지 돌려 본다(로그인·네트워크가 든다): `cargo test --bin hanji claude_code_end_to_end -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;
    use futures::StreamExt as _;

    /// 임시 라이브러리에서 말을 보내고 승인을 모두 허락한다. 시트가 고쳐졌는지와 앞의 말을 기억했는지 본다.
    fn end_to_end(backend: crate::agent::Backend, tools: crate::agent::AgentTools) {
        let root = std::env::temp_dir().join(format!(
            "hanji-acp-live-{}-{}",
            backend.models_key(),
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("Skills/shout")).unwrap();
        std::fs::write(root.join("sheet.md"), "# Sheet\n\nhello world\n").unwrap();
        std::fs::write(
            root.join("Skills/shout/SKILL.md"),
            "---\nname: shout\ndescription: Use when the user asks to shout.\n---\nEnd the reply with SHOUTED.\n",
        )
        .unwrap();
        let workspace = crate::agent::Workspace::new(&root);
        let system = crate::agent::agent_system_prompt(None, tools, workspace.skills());
        let turn = |user: bool, text: &str| Turn {
            user,
            text: text.into(),
            ..Turn::default()
        };
        let request = crate::agent::ChatRequest {
            backend,
            workspace: Some(workspace),
            system,
            turns: vec![
                turn(true, "My name is Mina."),
                turn(false, "Hi Mina."),
                turn(true, "Shout my name, and change hello to hi in sheet.md."),
            ],
            allowed_sites: Default::default(),
        };
        let mut events = crate::agent::stream(request);
        let mut text = String::new();
        futures::executor::block_on(async {
            while let Some(event) = events.next().await {
                match event {
                    ChatEvent::Text(chunk) => text.push_str(&chunk),
                    ChatEvent::Approve(approval) => {
                        eprintln!(
                            "APPROVE {} new={} lines={}",
                            approval.card.path,
                            approval.card.new_file,
                            approval.card.lines.len()
                        );
                        approval.reply.send(true).ok();
                    }
                    ChatEvent::Done => break,
                    ChatEvent::Failed(err) => panic!("failed: {err}"),
                    other => eprintln!("EVENT {other:?}"),
                }
            }
        });
        eprintln!("TEXT {text}");
        let sheet = std::fs::read_to_string(root.join("sheet.md")).unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(sheet.contains("hi world"), "{sheet}");
        assert!(text.to_lowercase().contains("mina"), "{text}");
    }

    #[test]
    #[ignore]
    fn claude_code_end_to_end() {
        end_to_end(
            crate::agent::Backend::ClaudeCode {
                model: "haiku".into(),
            },
            crate::agent::AgentTools::ClaudeCode,
        );
    }

    #[test]
    #[ignore]
    fn codex_models() {
        let models = crate::agent::bedrock::runtime()
            .block_on(codex::list_models())
            .unwrap();
        eprintln!("MODELS {models:?}");
        assert!(!models.is_empty());
    }

    /// `cargo test --bin hanji codex_end_to_end -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn codex_end_to_end() {
        end_to_end(
            crate::agent::Backend::Codex {
                model: String::new(),
            },
            crate::agent::AgentTools::Codex,
        );
    }
}
