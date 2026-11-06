# ADR-0057: 어시스턴트는 ACP(Agent Client Protocol) 클라이언트로 에이전트 CLI를 부르고, Claude Code는 앱 안의 ACP 어댑터가 `claude` stream-json으로 잇는다

- 상태: Proposed
- 날짜: 2026-11-06
- 관련: ADR-0029(확장 초안), ADR-0030(확장 검토), ADR-0031(대화 기록) · `src/agent/acp/mod.rs`(ACP 클라이언트 → `ChatEvent`), `src/agent/acp/claude.rs`(Claude Code 어댑터), `src/claude_code.rs`(설치 찾기), `src/agent/prompt.rs`(에이전트용 지시), `src/settings.rs`(제공자·찾기 상자)

## 배경 (Context)

- 사용자 컴퓨터에 설치된 에이전트 CLI(Claude Code, 나중에 Codex)를 어시스턴트 제공자로 쓰고 싶다. 사용자의 구독 로그인을 그대로 쓰고, API 키를 따로 받지 않는다.
- 지금 어시스턴트(Bedrock·Token Plan)는 앱이 모델 호출과 도구 실행을 되풀이한다(`agent/bedrock.rs`의 `run`). 에이전트 CLI는 그 되풀이를 스스로 한다. 그래서 모델 하나를 바꿔 끼우듯 붙일 수 없다.
- `claude`와 `codex`는 ACP를 직접 말하지 않는다(`--help`에 없음). Zed의 Claude 어댑터는 Node(TypeScript)라 사용자가 Node와 어댑터를 따로 깔아야 한다.
- `claude -p --input-format stream-json --output-format stream-json`을 시험했다(2.1.318):
  - `--restricted`는 코드 실행 도구와 WebFetch를 빼고 파일 도구를 작업 폴더 안으로 가두며, 사용자·프로젝트 설정 파일을 읽지 않는다. `--system-prompt`와 함께 쓰면 CLAUDE.md·사용자 규칙이 맥락에 들어오지 않았다.
  - `--permission-prompt-tool stdio`면 파일을 쓰기 전에 `control_request`(`can_use_tool`, 도구 이름과 입력)를 내보내고 `control_response`(`allow`·`deny`)를 기다린다. Read·Glob·Grep은 작업 폴더 안이면 묻지 않는다.
  - 입력으로 넣은 어시스턴트 메시지는 무시한다(사용자 메시지마다 답한다).
  - `--bare`는 인증을 `ANTHROPIC_API_KEY`로만 받아 구독 로그인을 쓸 수 없다.

## 결정 (Decision)

**어시스턴트 패널은 그대로 두고, 에이전트 제공자는 ACP v1 클라이언트(`agent-client-protocol` 5.x)로 부른다.** 클라이언트는 ACP 알림을 지금의 `ChatEvent`로 바꾼다.

| ACP | `ChatEvent` |
| --- | --- |
| `agent_message_chunk` | `Text` |
| `tool_call`(읽기·찾기) | `Tool`(한 줄) |
| `session/request_permission`(diff가 든 도구 호출) | `Approve`(승인 카드). 답은 `allow_once`·`reject_once` |
| 승인한 도구 호출의 `completed`·`failed` | `Changed`·`ChangeFailed` |
| `session/prompt` 응답·오류 | `Done`·`Failed` |

**Claude Code는 앱 안의 ACP 에이전트(어댑터)로 잇는다.** 같은 프로세스 안에서 `Channel`로 이어 따로 깔 것이 없다. 어댑터는 말 한 번마다 라이브러리 루트에서 다음처럼 `claude`를 띄운다.

```
claude -p --input-format stream-json --output-format stream-json --include-partial-messages --verbose
       --restricted --strict-mcp-config --no-session-persistence
       --tools Read,Edit,Write,Glob,Grep --permission-prompt-tool stdio
       --model <sonnet|opus|haiku> --system-prompt <앱 지시>
```

- `text_delta`는 글 조각으로, `tool_use`는 도구 호출 알림으로 바꾼다.
- `can_use_tool`(Edit·Write)은 파일 전체의 앞뒤 내용으로 diff를 만들어 권한을 묻는다. 허락하면 Claude Code가 쓴다(그 사이 바뀐 파일은 Claude Code가 막는다).
- 멈추면(`session/cancel` 또는 연결이 끊기면) 프로세스를 끝낸다.

**대화 기록은 앱의 것을 쓴다(ADR-0031).** 세션을 남기지 않고(`--no-session-persistence`), 앞의 말들은 사용자 메시지 하나에 글로 묶어 넘긴다. 지금 제공자들도 말 글만 다시 보낸다.

**스킬은 Read로 읽게 한다.** 스킬은 라이브러리 안(`Skills/<이름>/SKILL.md`)이라 작업 폴더 제한 안에 있다. 지시에 이름·설명·경로를 넣는다.

**모델은 `sonnet`·`opus`·`haiku`·`fable` 별칭을 고정 목록으로 둔다.** 기본은 `sonnet`. 별칭이 실제로 어느 모델로 가는지는 `claude`가 정하고 시작할 때(`init`)만 알려 준다(메시지를 보내야 나온다). 그래서 답마다 그 모델 ID를 `session/prompt` 응답의 `_meta`(`hanji/model`)로 넘기고, 앱은 `config.toml`의 `claude_code_models`에 적어 그 뒤로 "Claude Sonnet 5.5"처럼 버전을 붙여 보인다. 한 번 쓰기 전에는 별칭 이름만 보인다.

**받아쓰기 요약·OCR은 에이전트 제공자를 쓰지 않는다.** 모델 한 번 부르기라 에이전트가 필요 없다. 그 기능에는 "AI를 설정하지 않음"으로 보인다.

## 결과 (Consequences)

- 좋아지는 점(+): 설치된 `claude`와 그 로그인만 있으면 된다. 패널·승인 카드·기록을 그대로 쓴다. Codex나 ACP를 직접 말하는 에이전트는 같은 클라이언트에 어댑터나 stdio 연결을 더하면 된다. `claude`가 ACP를 직접 말하게 되면 어댑터를 지운다.
- 감수하는 점(-): 확장 그려 보기·검토(ADR-0030), 웹 읽기, 새 시트 만들기 도구, 대화 중 모델 바꾸기가 없다. 확장 파일도 초안 없이 바로 승인 카드로 묻는다. 말마다 프로세스를 띄워 첫 글자가 1~2초 늦다. `can_use_tool` 제어 메시지는 공개 문서가 아닌 SDK 규약이라 CLI 버전에 따라 바뀔 수 있다. 앞의 답에서 읽은 파일 내용은 넘어가지 않는다.
- 후속 작업(TODO): Codex 어댑터. 앱 도구(확장 그려 보기 등)를 ACP의 MCP로 넘기기. 저장하지 않은 에디터 내용을 fs 위임으로 보이기.

## 대안 (Alternatives)

| 대안 | 기각 사유 |
| --- | --- |
| CLI마다 `ChatEvent`로 바로 잇기 | 코드량은 비슷하고, 외부 ACP 에이전트와 CLI의 ACP 직접 지원을 받으려면 다시 만들어야 한다 |
| Zed의 Node 어댑터를 띄우기 | 사용자가 Node·어댑터를 깔아야 한다 |
| 앱 도구를 MCP 서버(로컬 소켓)로 넘기기 | 앱이 접속을 받아야 하고, 1차에는 기본 파일 도구로 충분하다 |
| `--resume`으로 세션 잇기 | `~/.claude`에 세션이 따로 쌓이고, 앱 기록을 지워도 남고, 다른 컴퓨터에서 이어지지 않는다 |
| `--bare` | 구독 로그인을 쓸 수 없다 |
