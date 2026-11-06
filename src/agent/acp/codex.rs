//! Codex 어댑터(ADR-0057): 앱 안의 ACP 에이전트. 말 한 번마다 `codex app-server`(stdio JSON-RPC)를 띄워
//! 스레드 하나에 말을 보내고, 그 알림을 ACP 알림으로, 파일 변경 승인 요청을 `session/request_permission`으로
//! 바꾼다.
//!
//! 사용자의 Codex 설정(로그인·모델 제공자)은 그대로 쓰되, 설정한 MCP 서버·플러그인·브라우저·웹 검색 등은 끄고
//! 읽기 전용 샌드박스에서 돌린다. 명령 실행은 라이브러리 안을 읽기만 하는 것(Codex가 읽기·목록·찾기로 푼 것)만
//! 허락하고 나머지는 거절한다.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use agent_client_protocol::v1::schema::{
    ContentBlock, ContentChunk, Diff, PromptResponse, SessionUpdate, StopReason, TextContent,
    ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};
use agent_client_protocol::{Client, ConnectTo};
use futures::channel::oneshot;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::adapter::{self, Runner, write_line};
use crate::agent::ModelChoice;
use crate::agent_cli::{self, Cli};

/// 끄는 Codex 기능. 없는 이름은 Codex가 무시한다(`-c features.<이름>=false`).
const DISABLED_FEATURES: &[&str] = &[
    "apps",
    "plugins",
    "remote_plugin",
    "browser_use",
    "browser_use_external",
    "in_app_browser",
    "computer_use",
    "image_generation",
    "multi_agent",
    "hooks",
    "goals",
    "tool_suggest",
    "skill_search",
];

/// 명령 실행을 거절할 때의 까닭(Codex가 모델에 돌려준다).
const COMMAND_DECLINED: &str = "decline";

/// 한 대화에 쓰는 Codex 설정.
#[derive(Clone, Debug)]
pub struct Codex {
    /// `config.toml`의 `codex_path`. 없으면 찾는다.
    pub path: Option<PathBuf>,
    /// 모델 ID. 없으면 Codex 설정의 기본 모델.
    pub model: Option<String>,
    /// 앱이 만든 지시(`developerInstructions`).
    pub system: String,
}

impl Codex {
    /// ACP 에이전트로 만든다. 같은 프로세스 안에서 클라이언트에 바로 잇는다.
    pub fn into_agent(self) -> impl ConnectTo<Client> {
        adapter::into_agent(self)
    }
}

impl Runner for Codex {
    fn name(&self) -> &'static str {
        "hanji-codex"
    }

    fn run(
        self: Arc<Self>,
        turn: adapter::Turn,
        text: String,
        cancel: oneshot::Receiver<()>,
    ) -> BoxFuture<'static, Result<PromptResponse, String>> {
        Box::pin(async move {
            let stop = run_turn(&self, &turn, &text, cancel).await?;
            Ok(PromptResponse::new(stop))
        })
    }
}

/// 쓸 수 있는 모델(표시 이름, ID). Codex의 기본 모델이 맨 앞이다.
pub async fn list_models() -> Result<Vec<(ModelChoice, String)>, String> {
    let path = crate::config::Config::load().codex_path;
    let cwd = std::env::temp_dir();
    let (mut child, mut rpc) = AppServer::start(path, &cwd).await?;
    let result = async {
        rpc.initialize().await?;
        let models = rpc.call("model/list", json!({})).await?;
        let mut models: Vec<&Value> = models["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| model["hidden"].as_bool() != Some(true))
            .collect();
        models.sort_by_key(|model| model["isDefault"].as_bool() != Some(true));
        Ok(models
            .into_iter()
            .filter_map(|model| {
                let id = model["id"].as_str()?.to_string();
                let name = model["displayName"].as_str().unwrap_or(&id).to_string();
                Some((
                    ModelChoice {
                        name: Cow::Owned(name),
                        base: Cow::Owned(id.clone()),
                    },
                    id,
                ))
            })
            .collect())
    }
    .await;
    child.kill().await.ok();
    result
}

/// 띄운 `codex app-server`와 주고받는 JSON-RPC(줄마다 메시지 하나).
struct AppServer {
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: i64,
    /// 끝났을 때 까닭을 보일 stderr.
    stderr: Option<tokio::task::JoinHandle<String>>,
}

impl AppServer {
    async fn start(path: Option<PathBuf>, cwd: &Path) -> Result<(Child, Self), String> {
        let binary =
            tokio::task::spawn_blocking(move || agent_cli::find(Cli::Codex, path.as_deref()))
                .await
                .ok()
                .flatten()
                .ok_or_else(|| t!("assistant.error.cli_missing", name = "Codex").to_string())?;
        let mut command = std::process::Command::new(&binary);
        command
            .args(launch_args(&codex_config()))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        agent_cli::hide_console(&mut command);
        let mut child = Command::from(command)
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| {
                t!("assistant.error.cli_start", name = "Codex", err = err).to_string()
            })?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let mut stderr = child.stderr.take().ok_or("no stderr")?;
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            stderr.read_to_string(&mut text).await.ok();
            text
        });
        Ok((
            child,
            Self {
                stdin,
                lines: BufReader::new(stdout).lines(),
                next_id: 0,
                stderr: Some(stderr),
            },
        ))
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<i64, String> {
        self.next_id += 1;
        let id = self.next_id;
        write_line(
            &mut self.stdin,
            &json!({ "id": id, "method": method, "params": params }),
        )
        .await?;
        Ok(id)
    }

    async fn respond(&mut self, id: &Value, result: Value) -> Result<(), String> {
        write_line(&mut self.stdin, &json!({ "id": id, "result": result })).await
    }

    async fn respond_error(&mut self, id: &Value, message: &str) -> Result<(), String> {
        write_line(
            &mut self.stdin,
            &json!({ "id": id, "error": { "code": -32601, "message": message } }),
        )
        .await
    }

    /// 다음 메시지. 끝났으면 stderr의 까닭으로 실패한다.
    async fn next(&mut self) -> Result<Value, String> {
        loop {
            match self.lines.next_line().await {
                Ok(Some(line)) => {
                    if let Ok(message) = serde_json::from_str(&line) {
                        return Ok(message);
                    }
                }
                Ok(None) => return Err(self.exit_reason().await),
                Err(err) => return Err(err.to_string()),
            }
        }
    }

    async fn exit_reason(&mut self) -> String {
        let text = match self.stderr.take() {
            Some(stderr) => stderr.await.unwrap_or_default(),
            None => String::new(),
        };
        // 마지막 오류 줄(`Error: …`)이 가장 쓸모 있다.
        text.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_else(|| {
                t!(
                    "assistant.error.cli_exited",
                    name = "Codex",
                    program = "codex"
                )
                .to_string()
            })
    }

    /// 요청을 보내고 그 답을 기다린다. 그사이 온 알림은 버리고, 서버 요청은 거절한다.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.request(method, params).await?;
        loop {
            let message = self.next().await?;
            if message.get("method").is_some() {
                if let Some(request_id) = message.get("id") {
                    self.respond_error(request_id, "not supported").await?;
                }
                continue;
            }
            if message["id"].as_i64() == Some(id) {
                return response_result(message);
            }
        }
    }

    async fn initialize(&mut self) -> Result<(), String> {
        self.call(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "hanji",
                    "title": "Hanji",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await?;
        write_line(&mut self.stdin, &json!({ "method": "initialized" })).await
    }
}

/// JSON-RPC 답의 결과, 오류면 그 설명.
fn response_result(mut message: Value) -> Result<Value, String> {
    if let Some(error) = message.get("error") {
        return Err(error["message"]
            .as_str()
            .unwrap_or("Codex request failed")
            .to_string());
    }
    Ok(message["result"].take())
}

/// 사용자의 Codex 설정 파일(`$CODEX_HOME/config.toml`, 기본 `~/.codex/config.toml`). 없으면 빈 표.
fn codex_config() -> toml::Table {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    home.and_then(|home| std::fs::read_to_string(home.join("config.toml")).ok())
        .and_then(|text| text.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

/// `codex app-server`에 줄 인자: 사용자가 설정한 MCP 서버를 모두 끄고, 도구를 늘리는 기능·웹 검색·
/// 프로젝트 지시(AGENTS.md)를 끈다.
fn launch_args(config: &toml::Table) -> Vec<String> {
    let mut args = vec!["app-server".to_string()];
    let servers = config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|servers| servers.keys());
    for name in servers {
        // 점·공백이 든 이름은 `-c` 경로로 가리킬 수 없다. 그런 서버는 켜진 채 남는다.
        if name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            args.extend([
                "-c".to_string(),
                format!("mcp_servers.{name}.enabled=false"),
            ]);
        }
    }
    for feature in DISABLED_FEATURES {
        args.extend(["-c".to_string(), format!("features.{feature}=false")]);
    }
    args.extend([
        "-c".to_string(),
        "project_doc_max_bytes=0".to_string(),
        "-c".to_string(),
        "web_search=\"disabled\"".to_string(),
    ]);
    args
}

/// 말 한 번.
async fn run_turn(
    config: &Codex,
    turn: &adapter::Turn,
    text: &str,
    mut cancel: oneshot::Receiver<()>,
) -> Result<StopReason, String> {
    let (mut child, mut rpc) = AppServer::start(config.path.clone(), &turn.cwd).await?;
    rpc.initialize().await?;
    let mut params = json!({
        "cwd": turn.cwd,
        "sandbox": "read-only",
        "approvalPolicy": "untrusted",
        "approvalsReviewer": "user",
        "ephemeral": true,
        "developerInstructions": config.system,
    });
    if let Some(model) = &config.model {
        params["model"] = json!(model);
    }
    let thread = rpc.call("thread/start", params).await?;
    let thread_id = thread["thread"]["id"]
        .as_str()
        .ok_or("no thread id")?
        .to_string();
    let turn_request = rpc
        .request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": text }],
            }),
        )
        .await?;

    let mut state = TurnState::default();
    let result = loop {
        let message = tokio::select! {
            message = rpc.next() => message?,
            _ = &mut cancel => {
                child.kill().await.ok();
                return Ok(StopReason::Cancelled);
            }
        };
        let Some(method) = message["method"].as_str() else {
            // 요청의 답. `turn/start`가 실패했으면 끝낸다.
            if message["id"].as_i64() == Some(turn_request) {
                response_result(message)?;
            }
            continue;
        };
        if let Some(id) = message.get("id") {
            let reply = state.on_request(turn, method, &message["params"]).await?;
            match reply {
                Some(result) => rpc.respond(id, result).await?,
                None => rpc.respond_error(id, "not supported in Hanji").await?,
            }
            continue;
        }
        if let Some(stop) = state.on_notification(turn, method, &message["params"])? {
            break stop;
        }
    };
    child.kill().await.ok();
    result
}

/// 말 한 번 동안 쌓는 것.
#[derive(Default)]
struct TurnState {
    /// 마지막으로 글을 받은 메시지(`itemId`). 메시지가 바뀌면 문단을 띄운다.
    message: Option<String>,
    /// 글 조각을 받은 메시지. 조각 없이 끝난 메시지는 끝날 때 통째로 넘긴다.
    streamed: HashSet<String>,
    /// 마지막으로 넘긴 글이 줄바꿈으로 끝났는지(문단을 띄울 때 줄바꿈을 겹치지 않는다).
    ends_with_newline: bool,
    /// 파일 변경(`itemId` → 바꿀 파일들). 승인 요청은 `itemId`만 주므로 여기서 찾는다.
    changes: HashMap<String, Value>,
    /// 승인을 거쳐 쓰기로 한 파일 변경.
    approved: HashSet<String>,
    /// 다시 시도하지 않는 마지막 오류.
    error: Option<String>,
}

impl TurnState {
    /// 서버 요청에 줄 결과. `None`이면 지원하지 않는다고 답한다.
    async fn on_request(
        &mut self,
        turn: &adapter::Turn,
        method: &str,
        params: &Value,
    ) -> Result<Option<Value>, String> {
        match method {
            "item/commandExecution/requestApproval" => {
                let decision = if read_only_in(&turn.cwd, &params["commandActions"])
                    || read_only_argv(&turn.cwd, &params["proposedExecpolicyAmendment"])
                {
                    "accept"
                } else {
                    COMMAND_DECLINED
                };
                Ok(Some(json!({ "decision": decision })))
            }
            "item/fileChange/requestApproval" => {
                let item = params["itemId"].as_str().unwrap_or_default().to_string();
                let Some(diffs) = self.diffs(&item).await else {
                    return Ok(Some(json!({ "decision": "decline" })));
                };
                let title = diffs
                    .first()
                    .map(|diff| turn.relative(&diff.path.to_string_lossy()))
                    .unwrap_or_default();
                let tool_call = ToolCallUpdate::new(
                    ToolCallId::new(item.as_str()),
                    ToolCallUpdateFields::new()
                        .kind(ToolKind::Edit)
                        .title(title)
                        .content(
                            diffs
                                .into_iter()
                                .map(ToolCallContent::Diff)
                                .collect::<Vec<_>>(),
                        ),
                );
                let allowed = turn.ask(tool_call).await?;
                if allowed {
                    self.approved.insert(item);
                }
                Ok(Some(
                    json!({ "decision": if allowed { "accept" } else { "decline" } }),
                ))
            }
            _ => Ok(None),
        }
    }

    /// 알림을 ACP 알림으로 넘긴다. 말이 끝났으면 끝난 까닭.
    fn on_notification(
        &mut self,
        turn: &adapter::Turn,
        method: &str,
        params: &Value,
    ) -> Result<Option<Result<StopReason, String>>, String> {
        match method {
            "item/agentMessage/delta" => {
                let item = params["itemId"].as_str().unwrap_or_default();
                self.streamed.insert(item.to_string());
                self.text(turn, item, params["delta"].as_str().unwrap_or_default())?;
            }
            "item/started" if params["item"]["type"] == "fileChange" => {
                let item = &params["item"];
                if let Some(id) = item["id"].as_str() {
                    self.changes.insert(id.to_string(), item["changes"].clone());
                }
            }
            "item/completed" => self.on_item_completed(turn, &params["item"])?,
            "error" if params["willRetry"].as_bool() != Some(true) => {
                self.error = params["error"]["message"].as_str().map(str::to_string);
            }
            "turn/completed" => {
                let status = params["turn"]["status"].as_str().unwrap_or_default();
                let error = params["turn"]["error"]["message"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| self.error.take());
                return Ok(Some(match status {
                    "interrupted" => Ok(StopReason::Cancelled),
                    "failed" => Err(error.unwrap_or_else(|| {
                        t!(
                            "assistant.error.cli_exited",
                            name = "Codex",
                            program = "codex"
                        )
                        .to_string()
                    })),
                    _ => Ok(StopReason::EndTurn),
                }));
            }
            _ => {}
        }
        Ok(None)
    }

    /// 끝난 일: 읽기·찾기 명령은 도구 줄로, 승인한 파일 변경은 됐는지로 알린다.
    fn on_item_completed(&mut self, turn: &adapter::Turn, item: &Value) -> Result<(), String> {
        let Some(id) = item["id"].as_str() else {
            return Ok(());
        };
        match item["type"].as_str() {
            Some("agentMessage") if !self.streamed.contains(id) => {
                self.text(turn, id, item["text"].as_str().unwrap_or_default())?;
            }
            Some("commandExecution") if item["status"] == "completed" => {
                for (index, action) in item["commandActions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let (kind, target) = match action["type"].as_str() {
                        Some("read") => (ToolKind::Read, action["path"].as_str()),
                        Some("listFiles" | "search") => (
                            ToolKind::Search,
                            action["path"].as_str().or(action["query"].as_str()),
                        ),
                        _ => continue,
                    };
                    let title = target
                        .map(|target| turn.relative(target))
                        .unwrap_or_default();
                    turn.notify(SessionUpdate::ToolCall(
                        ToolCall::new(ToolCallId::new(format!("{id}#{index}")), title)
                            .kind(kind)
                            .status(ToolCallStatus::Completed),
                    ))?;
                }
            }
            Some("fileChange") if self.approved.contains(id) => {
                let status = if item["status"] == "completed" {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };
                turn.notify(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new(id),
                    ToolCallUpdateFields::new().status(status),
                )))?;
            }
            _ => {}
        }
        Ok(())
    }

    /// 메시지(`item`)의 글을 넘긴다. 앞 메시지와 다르면 문단을 띄운다.
    fn text(&mut self, turn: &adapter::Turn, item: &str, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let mut text = text.to_string();
        if self.message.as_deref().is_some_and(|last| last != item) {
            text.insert_str(0, if self.ends_with_newline { "\n" } else { "\n\n" });
        }
        self.message = Some(item.to_string());
        self.ends_with_newline = text.ends_with('\n');
        turn.notify(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
        )))
    }

    /// 파일 변경 하나의 앞뒤 내용. 패치를 지금 파일에 대 보지 못하면 `None`.
    async fn diffs(&self, item: &str) -> Option<Vec<Diff>> {
        let mut diffs = Vec::new();
        for change in self.changes.get(item)?.as_array()? {
            let path = PathBuf::from(change["path"].as_str()?);
            let patch = change["diff"].as_str().unwrap_or_default();
            let before = tokio::fs::read_to_string(&path).await.ok();
            let diff = match change["kind"]["type"].as_str()? {
                "add" => {
                    let after = if patch.starts_with("@@") {
                        apply_unified("", patch)?
                    } else {
                        patch.to_string()
                    };
                    Diff::new(path, after)
                }
                "delete" => Diff::new(path, "").old_text(Some(before?)),
                _ => {
                    let before = before?;
                    let after = apply_unified(&before, patch)?;
                    Diff::new(path, after).old_text(Some(before))
                }
            };
            diffs.push(diff);
        }
        (!diffs.is_empty()).then_some(diffs)
    }
}

/// Codex가 푼 명령이 모두 라이브러리 안을 읽기·목록·찾기만 하는지.
fn read_only_in(root: &Path, actions: &Value) -> bool {
    let Some(actions) = actions.as_array().filter(|actions| !actions.is_empty()) else {
        return false;
    };
    actions.iter().all(|action| {
        matches!(
            action["type"].as_str(),
            Some("read" | "listFiles" | "search")
        ) && action["path"]
            .as_str()
            .is_none_or(|path| inside(root, Path::new(path)))
    })
}

/// Codex가 풀지 못한 명령(`unknown`)도, 셸 없이 나눈 인자(`proposedExecpolicyAmendment`)가 읽기만 하는
/// 프로그램이고 경로가 모두 라이브러리 안이면 허락한다. 샌드박스가 읽기 전용이라 쓰지는 못한다.
fn read_only_argv(root: &Path, argv: &Value) -> bool {
    let Some(argv) = argv
        .as_array()
        .and_then(|argv| argv.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
    else {
        return false;
    };
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    let operands: Vec<&str> = args
        .iter()
        .copied()
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let paths: &[&str] = match *program {
        "cat" | "head" | "tail" | "wc" | "nl" | "ls" => &operands,
        // 첫 낱말은 패턴이다. rg의 `--pre`는 다른 프로그램을 돌린다.
        "rg" | "grep" if !args.iter().any(|arg| arg.starts_with("--pre")) => {
            operands.get(1..).unwrap_or_default()
        }
        // `sed -n '1,80p' 파일`: 줄을 보이기만 하는 스크립트.
        "sed" if args.contains(&"-n") => match operands.split_first() {
            Some((script, paths))
                if script
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ',' | 'p' | '$' | ' ')) =>
            {
                paths
            }
            _ => return false,
        },
        _ => return false,
    };
    paths
        .iter()
        .all(|path| !path.starts_with('~') && !path.contains('$') && inside(root, Path::new(path)))
}

/// `path`(상대면 `root` 기준)가 `root` 안인지.
fn inside(root: &Path, path: &Path) -> bool {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if path
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return false;
    }
    !Path::new(&super::relative(root, &path)).is_absolute()
}

/// 통합 diff(`@@ -a,b +c,d @@` 덩어리들)를 `before`에 대 고친 내용. 문맥이 맞지 않으면 `None`.
fn apply_unified(before: &str, patch: &str) -> Option<String> {
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut pos = 0;
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("@@ -") else {
            continue;
        };
        let range = header.split(' ').next()?;
        let mut parts = range.split(',');
        let start: usize = parts.next()?.parse().ok()?;
        let count: usize = parts.next().map_or(Some(1), |count| count.parse().ok())?;
        // 줄 번호는 1부터. 지울 줄이 없으면 그 줄 뒤에 넣는다.
        let at = if count == 0 {
            start
        } else {
            start.checked_sub(1)?
        };
        if at < pos || at > old.len() {
            return None;
        }
        out.extend(old[pos..at].iter().copied());
        pos = at;
        while let Some(line) = lines.peek() {
            if line.starts_with("@@") {
                break;
            }
            let line = lines.next()?;
            if line.starts_with('\\') {
                // "\ No newline at end of file": 바로 앞 줄의 줄바꿈을 뗀다.
                if out.ends_with('\n') {
                    out.pop();
                }
                continue;
            }
            let (mark, text) = line.split_at(line.len().min(1));
            match mark {
                " " | "" => {
                    let current = old.get(pos)?;
                    if current.trim_end_matches(['\n', '\r']) != text {
                        return None;
                    }
                    out.push_str(current);
                    pos += 1;
                }
                "-" => {
                    if old.get(pos)?.trim_end_matches(['\n', '\r']) != text {
                        return None;
                    }
                    pos += 1;
                }
                "+" => {
                    out.push_str(text);
                    out.push('\n');
                }
                _ => return None,
            }
        }
    }
    out.extend(old[pos..].iter().copied());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_unified_hunks() {
        let before = "# Sheet\n\nhello world\nend\n";
        let patch = "@@ -2,2 +2,2 @@\n \n-hello world\n+hi world\n";
        assert_eq!(
            apply_unified(before, patch).as_deref(),
            Some("# Sheet\n\nhi world\nend\n")
        );
        // 두 덩어리, 줄 더하기.
        let before = "a\nb\nc\nd\ne\n";
        let patch = "@@ -1,1 +1,2 @@\n a\n+a2\n@@ -5 +6 @@\n-e\n+E\n";
        assert_eq!(
            apply_unified(before, patch).as_deref(),
            Some("a\na2\nb\nc\nd\nE\n")
        );
        // 문맥이 다르면 대지 않는다.
        assert_eq!(apply_unified("x\n", "@@ -1 +1 @@\n-y\n+z\n"), None);
        // 끝 줄바꿈 없음.
        assert_eq!(
            apply_unified("a\n", "@@ -1 +1 @@\n-a\n+b\n\\ No newline at end of file\n").as_deref(),
            Some("b")
        );
    }

    #[test]
    fn allows_only_reads_inside_library() {
        let root = Path::new("/lib");
        let read =
            |path: &str| json!([{ "type": "read", "command": "cat", "name": "x", "path": path }]);
        assert!(read_only_in(root, &read("/lib/a.md")));
        assert!(read_only_in(root, &read("Projects/a.md")));
        assert!(!read_only_in(root, &read("/etc/passwd")));
        assert!(!read_only_in(root, &read("../secret")));
        assert!(read_only_in(
            root,
            &json!([{ "type": "search", "command": "rg x", "path": null }])
        ));
        assert!(!read_only_in(
            root,
            &json!([{ "type": "unknown", "command": "rm -rf /" }])
        ));
        assert!(!read_only_in(root, &json!([])));
    }

    #[test]
    fn allows_simple_read_commands() {
        let root = Path::new("/lib");
        let argv = |args: &[&str]| json!(args);
        assert!(read_only_argv(
            root,
            &argv(&["cat", "Skills/a/SKILL.md", "sheet.md"])
        ));
        assert!(read_only_argv(root, &argv(&["sed", "-n", "1,80p", "a.md"])));
        assert!(read_only_argv(
            root,
            &argv(&["rg", "-n", "hello", "Projects"])
        ));
        assert!(!read_only_argv(root, &argv(&["cat", "/etc/passwd"])));
        assert!(!read_only_argv(root, &argv(&["ls", "~"])));
        assert!(!read_only_argv(
            root,
            &argv(&["sed", "-n", "1w out", "a.md"])
        ));
        assert!(!read_only_argv(root, &argv(&["sed", "s/a/b/", "a.md"])));
        assert!(!read_only_argv(
            root,
            &argv(&["rg", "--pre", "sh", "x", "."])
        ));
        assert!(!read_only_argv(
            root,
            &argv(&["/bin/zsh", "-lc", "ls -la ~"])
        ));
        assert!(!read_only_argv(root, &argv(&["python3", "x.py"])));
        assert!(!read_only_argv(root, &Value::Null));
    }

    #[test]
    fn launch_args_disable_user_servers() {
        let config: toml::Table = "[mcp_servers.node_repl]\ncommand = \"x\"\n[mcp_servers.\"bad name\"]\ncommand = \"y\"\n"
            .parse()
            .unwrap();
        let args = launch_args(&config);
        assert!(args.contains(&"mcp_servers.node_repl.enabled=false".to_string()));
        assert!(!args.iter().any(|arg| arg.contains("bad name")));
        assert!(args.contains(&"features.apps=false".to_string()));
        assert_eq!(args[0], "app-server");
    }
}
