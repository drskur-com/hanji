//! Claude Code 어댑터(ADR-0057): 앱 안의 ACP 에이전트. 말 한 번마다 `claude -p`를 stream-json으로 띄워
//! 그 출력을 ACP 알림으로, 파일 쓰기 권한 질문(`can_use_tool`)을 `session/request_permission`으로 바꾼다.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Diff, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionId, SessionNotification, SessionUpdate, StopReason,
    TextContent, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Agent, Client, ConnectTo, Error};
use futures::channel::oneshot;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{ChildStdin, Command};

/// 고를 수 있는 모델(별칭, 표시 이름). 첫 것이 기본이다.
pub const MODELS: &[(&str, &str)] = &[
    ("sonnet", "Claude Sonnet"),
    ("opus", "Claude Opus"),
    ("haiku", "Claude Haiku"),
    ("fable", "Claude Fable"),
];

/// 별칭이 실제로 간 모델을 [`PromptResponse`]의 `_meta`에 담는 열쇠(`{"requested", "resolved"}`).
pub const MODEL_META: &str = "hanji/model";

/// 모델 ID(`claude-sonnet-5-5`)의 표시 이름(`Claude Sonnet 5.5`). 날짜 꼬리(`-20250929`)는 뺀다.
/// `claude-`로 시작하지 않으면 `None`.
pub fn display_name(id: &str) -> Option<String> {
    let rest = id.strip_prefix("claude-")?;
    let mut words = Vec::new();
    let mut version = Vec::new();
    for part in rest.split('-') {
        if part.chars().all(|c| c.is_ascii_digit()) {
            if part.len() < 8 {
                version.push(part);
            }
        } else if version.is_empty() {
            let mut chars = part.chars();
            let first = chars.next()?;
            words.push(first.to_uppercase().chain(chars).collect::<String>());
        }
    }
    if words.is_empty() {
        return None;
    }
    let mut name = format!("Claude {}", words.join(" "));
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    Some(name)
}

/// `claude`에 줄 파일 도구.
const TOOLS: &str = "Read,Edit,Write,Glob,Grep";

/// 승인 카드에서 거절했을 때 Claude Code에 돌려줄 글.
const DECLINED: &str = "The user declined this change. Do not retry it unless they ask; you may ask what they would like instead.";

/// 묻지 않을 도구 밖의 권한 질문을 거절할 때 돌려줄 글.
const NOT_ALLOWED: &str = "This tool is not available in Hanji.";

const ALLOW: &str = "allow";
const REJECT: &str = "reject";

/// 한 대화에 쓰는 Claude Code 설정.
#[derive(Clone, Debug)]
pub struct ClaudeCode {
    /// `config.toml`의 `claude_code_path`. 없으면 찾는다.
    pub path: Option<PathBuf>,
    pub model: String,
    /// 앱이 만든 시스템 프롬프트(`--system-prompt`).
    pub system: String,
}

/// 세션 하나의 상태: 작업 폴더와, 돌고 있는 말을 멈출 곳.
#[derive(Default)]
struct State {
    cwd: Option<PathBuf>,
    cancel: Option<oneshot::Sender<()>>,
}

impl ClaudeCode {
    /// ACP 에이전트로 만든다. 같은 프로세스 안에서 클라이언트에 바로 잇는다.
    pub fn into_agent(self) -> impl ConnectTo<Client> {
        let state = Arc::new(Mutex::new(State::default()));
        let config = Arc::new(self);
        let session_state = state.clone();
        let prompt_state = state.clone();
        let cancel_state = state;
        Agent
            .v1()
            .name("hanji-claude-code")
            .on_receive_request(
                async move |request: InitializeRequest, responder, _cx| {
                    responder.respond(
                        InitializeResponse::new(request.protocol_version)
                            .agent_capabilities(AgentCapabilities::new()),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: NewSessionRequest, responder, _cx| {
                    session_state.lock().expect("state").cwd = Some(request.cwd);
                    responder.respond(NewSessionResponse::new(SessionId::new("claude-code")))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, cx: ConnectionTo<Client>| {
                    let (cancel_tx, cancel_rx) = oneshot::channel();
                    let cwd = {
                        let mut state = prompt_state.lock().expect("state");
                        state.cancel = Some(cancel_tx);
                        state.cwd.clone()
                    };
                    let Some(cwd) = cwd else {
                        return responder.respond_with_internal_error("no session");
                    };
                    let text = request
                        .prompt
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let config = config.clone();
                    let connection = cx.clone();
                    cx.spawn(async move {
                        let turn = Turn {
                            config: &config,
                            cwd: &cwd,
                            session: request.session_id.clone(),
                            cx: &connection,
                        };
                        match turn.run(&text, cancel_rx).await {
                            Ok((stop, resolved)) => {
                                let mut response = PromptResponse::new(stop);
                                if let Some(resolved) = resolved {
                                    response = response.meta(serde_json::Map::from_iter([(
                                        MODEL_META.to_string(),
                                        json!({ "requested": config.model, "resolved": resolved }),
                                    )]));
                                }
                                responder.respond(response)
                            }
                            Err(message) => {
                                responder.respond_with_error(Error::new(-32603, message))
                            }
                        }
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |_cancel: CancelNotification, _cx| {
                    if let Some(cancel) = cancel_state.lock().expect("state").cancel.take() {
                        cancel.send(()).ok();
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
    }
}

/// 말 한 번.
struct Turn<'a> {
    config: &'a ClaudeCode,
    cwd: &'a Path,
    session: SessionId,
    cx: &'a ConnectionTo<Client>,
}

/// 승인을 거쳐 쓰기로 한 도구 호출(`tool_use_id`).
type Approved = HashSet<String>;

impl Turn<'_> {
    /// 말 한 번을 돌린다. 끝난 까닭과, `claude`가 알린 실제 모델(`init`)을 돌려준다.
    async fn run(
        &self,
        text: &str,
        cancel: oneshot::Receiver<()>,
    ) -> Result<(StopReason, Option<String>), String> {
        let override_path = self.config.path.clone();
        let binary =
            tokio::task::spawn_blocking(move || crate::claude_code::find(override_path.as_deref()))
                .await
                .ok()
                .flatten()
                .ok_or_else(|| t!("assistant.error.claude_code_missing").to_string())?;

        let mut command = std::process::Command::new(&binary);
        command
            .args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--verbose",
                "--restricted",
                "--strict-mcp-config",
                "--no-session-persistence",
                "--permission-prompt-tool",
                "stdio",
                "--tools",
                TOOLS,
                "--model",
                &self.config.model,
                "--system-prompt",
                &self.config.system,
            ])
            .current_dir(self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::claude_code::hide_console(&mut command);
        let mut child = Command::from(command)
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| t!("assistant.error.claude_code_start", err = err).to_string())?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let mut stderr = child.stderr.take().ok_or("no stderr")?;
        let stderr_text = tokio::spawn(async move {
            let mut text = String::new();
            stderr.read_to_string(&mut text).await.ok();
            text
        });

        let message = json!({
            "type": "user",
            "message": { "role": "user", "content": text },
        });
        write_line(&mut stdin, &message).await?;

        let mut lines = BufReader::new(stdout).lines();
        let mut cancel = cancel;
        let mut approved = Approved::new();
        let mut resolved = None;
        let outcome = loop {
            let line = tokio::select! {
                line = lines.next_line() => line,
                _ = &mut cancel => {
                    child.kill().await.ok();
                    return Ok((StopReason::Cancelled, resolved));
                }
            };
            let line = match line {
                Ok(Some(line)) => line,
                Ok(None) => break None,
                Err(err) => return Err(err.to_string()),
            };
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match event["type"].as_str() {
                Some("stream_event") => self.on_stream_event(&event["event"])?,
                Some("assistant") => self.on_assistant(&event["message"])?,
                Some("user") => self.on_tool_results(&event["message"], &approved)?,
                Some("control_request") => {
                    let reply = self.on_control_request(&event, &mut approved).await?;
                    write_line(&mut stdin, &reply).await?;
                }
                Some("result") => break Some(event),
                Some("system") if event["subtype"] == "init" => {
                    resolved = event["model"].as_str().map(str::to_string);
                }
                _ => {}
            }
        };
        drop(stdin);
        child.wait().await.ok();
        let stderr_text = stderr_text.await.unwrap_or_default();
        match outcome {
            Some(result) if result["is_error"].as_bool() == Some(true) => {
                Err(result_error(&result))
            }
            Some(_) => Ok((StopReason::EndTurn, resolved)),
            None => Err(match stderr_text.trim() {
                "" => t!("assistant.error.claude_code_exited").to_string(),
                text => text.to_string(),
            }),
        }
    }

    fn notify(&self, update: SessionUpdate) -> Result<(), String> {
        self.cx
            .send_notification(SessionNotification::new(self.session.clone(), update))
            .map_err(|err| err.message)
    }

    /// 글 조각(`text_delta`)을 넘긴다.
    fn on_stream_event(&self, event: &Value) -> Result<(), String> {
        if event["type"] == "content_block_delta"
            && event["delta"]["type"] == "text_delta"
            && let Some(text) = event["delta"]["text"].as_str()
        {
            self.notify(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::Text(TextContent::new(text)),
            )))?;
        }
        Ok(())
    }

    /// 다 받은 어시스턴트 메시지: 읽기·찾기 도구 호출을 알린다(쓰기는 권한 질문에서 알린다).
    fn on_assistant(&self, message: &Value) -> Result<(), String> {
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_use" {
                continue;
            }
            let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else {
                continue;
            };
            let input = &block["input"];
            let (kind, target) = match name {
                "Read" => (ToolKind::Read, input["file_path"].as_str()),
                "Glob" | "Grep" => (
                    ToolKind::Search,
                    input["path"].as_str().or(input["pattern"].as_str()),
                ),
                _ => continue,
            };
            let target = target
                .map(|target| self.relative(target))
                .unwrap_or_default();
            self.notify(SessionUpdate::ToolCall(
                ToolCall::new(ToolCallId::new(id), target)
                    .kind(kind)
                    .status(ToolCallStatus::InProgress)
                    .raw_input(input.clone()),
            ))?;
        }
        Ok(())
    }

    /// 도구 결과: 승인한 쓰기가 됐는지 알린다.
    fn on_tool_results(&self, message: &Value, approved: &Approved) -> Result<(), String> {
        for block in message["content"].as_array().into_iter().flatten() {
            let Some(id) = block["tool_use_id"].as_str() else {
                continue;
            };
            if !approved.contains(id) {
                continue;
            }
            let status = if block["is_error"].as_bool() == Some(true) {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            };
            self.notify(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new(id),
                ToolCallUpdateFields::new().status(status),
            )))?;
        }
        Ok(())
    }

    /// 권한 질문(`can_use_tool`): Edit·Write면 앞뒤 내용으로 클라이언트에 묻고, 나머지는 거절한다.
    async fn on_control_request(
        &self,
        event: &Value,
        approved: &mut Approved,
    ) -> Result<Value, String> {
        let request_id = event["request_id"].clone();
        let request = &event["request"];
        if request["subtype"] != "can_use_tool" {
            return Ok(control_error(request_id, "unsupported request"));
        }
        let input = request["input"].clone();
        let tool_use_id = request["tool_use_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let Some(diff) = planned_diff(request["tool_name"].as_str(), &input).await else {
            return Ok(deny(request_id, NOT_ALLOWED));
        };
        let title = self.relative(&diff.path.to_string_lossy());
        let tool_call = ToolCallUpdate::new(
            ToolCallId::new(tool_use_id.as_str()),
            ToolCallUpdateFields::new()
                .kind(ToolKind::Edit)
                .title(title)
                .content(vec![ToolCallContent::Diff(diff)])
                .raw_input(input.clone()),
        );
        let options = vec![
            PermissionOption::new(ALLOW, "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new(REJECT, "Reject", PermissionOptionKind::RejectOnce),
        ];
        let response = self
            .cx
            .send_request(RequestPermissionRequest::new(
                self.session.clone(),
                tool_call,
                options,
            ))
            .block_task()
            .await
            .map_err(|err| err.message)?;
        let allowed = matches!(
            &response.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == ALLOW
        );
        if !allowed {
            return Ok(deny(request_id, DECLINED));
        }
        approved.insert(tool_use_id);
        Ok(json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": { "behavior": "allow", "updatedInput": input },
            },
        }))
    }

    /// 화면에 보일 경로: 라이브러리 루트 기준.
    fn relative(&self, path: &str) -> String {
        super::relative(self.cwd, Path::new(path))
    }
}

/// Edit·Write가 쓸 내용: 파일 전체의 앞(없으면 새 파일)과 뒤.
async fn planned_diff(tool: Option<&str>, input: &Value) -> Option<Diff> {
    let path = PathBuf::from(input["file_path"].as_str()?);
    let before = tokio::fs::read_to_string(&path).await.ok();
    let after = match tool? {
        "Write" => input["content"].as_str()?.to_string(),
        "Edit" => {
            let old = input["old_string"].as_str()?;
            let new = input["new_string"].as_str()?;
            let text = before.as_deref().unwrap_or_default();
            if input["replace_all"].as_bool() == Some(true) {
                text.replace(old, new)
            } else {
                text.replacen(old, new, 1)
            }
        }
        _ => return None,
    };
    Some(Diff::new(path, after).old_text(before))
}

fn deny(request_id: Value, message: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "deny", "message": message },
        },
    })
}

fn control_error(request_id: Value, message: &str) -> Value {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": message },
    })
}

/// 실패한 결과의 설명.
fn result_error(result: &Value) -> String {
    if let Some(text) = result["result"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
    {
        return text.to_string();
    }
    let errors: Vec<&str> = result["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if errors.is_empty() {
        t!("assistant.error.claude_code_exited").to_string()
    } else {
        errors.join("\n")
    }
}

async fn write_line(stdin: &mut ChildStdin, value: &Value) -> Result<(), String> {
    let mut line = value.to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    stdin.flush().await.map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        assert_eq!(
            display_name("claude-sonnet-5-5").as_deref(),
            Some("Claude Sonnet 5.5")
        );
        assert_eq!(
            display_name("claude-fable-5").as_deref(),
            Some("Claude Fable 5")
        );
        assert_eq!(
            display_name("claude-sonnet-4-5-20250929").as_deref(),
            Some("Claude Sonnet 4.5")
        );
        // 옛 이름 꼴(버전이 앞)은 이름을 만들지 않는다(별칭 이름을 쓴다).
        assert_eq!(display_name("claude-3-5-haiku"), None);
        assert_eq!(display_name("gpt-5"), None);
    }

    #[tokio::test]
    async fn edit_diff_covers_whole_file() {
        let dir = std::env::temp_dir().join(format!("hanji-claude-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sheet.md");
        std::fs::write(&file, "a hello b hello\n").unwrap();
        let path = file.to_string_lossy().to_string();

        let edit = json!({ "file_path": path, "old_string": "hello", "new_string": "hi" });
        let diff = planned_diff(Some("Edit"), &edit).await.unwrap();
        assert_eq!(diff.old_text.as_deref(), Some("a hello b hello\n"));
        assert_eq!(diff.new_text, "a hi b hello\n");

        let all = json!({ "file_path": path, "old_string": "hello", "new_string": "hi", "replace_all": true });
        assert_eq!(
            planned_diff(Some("Edit"), &all).await.unwrap().new_text,
            "a hi b hi\n"
        );

        let new = dir.join("new.md").to_string_lossy().to_string();
        let write = json!({ "file_path": new, "content": "new\n" });
        let diff = planned_diff(Some("Write"), &write).await.unwrap();
        assert_eq!(diff.old_text, None);

        assert!(planned_diff(Some("Bash"), &edit).await.is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
