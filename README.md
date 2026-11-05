# Hanji

마크다운으로 쓰고 책으로 엮는 데스크톱 글쓰기 앱. 메모부터 원고까지 한 라이브러리에 두고, 쓰는 화면에서 바로 책 모양을 보며
PDF·ePub·DOCX로 내보낸다. Rust와 [gpui](https://www.gpui.rs/)로 만들었고 macOS와 Windows에서 돈다.

## 설치

[Releases](https://github.com/drskur-com/hanji/releases/latest)에서 내려받는다.

- macOS (Apple Silicon): `Hanji-<버전>-arm64.dmg` — 서명·공증됨
- Windows (x64): `Hanji-<버전>-x64.msi`

앱이 시작할 때 새 버전을 확인하고, 있으면 물어본 뒤 내려받아 바꾼다(메뉴의 `업데이트 확인…`).

## 무엇을 하나

### 라이브러리

- 글은 고른 폴더 안의 평범한 마크다운 파일이다. 앱 밖에서 고쳐도 폴더를 감시해 바로 반영한다.
- `Inbox`(빠른 메모), `Notes`(최근 순 메모), `Projects`(책) 세 구역. 프로젝트는 그룹을 겹쳐 두고 순서를 손으로 정한다.
- 시트를 끌어 옮기고, 여러 시트를 하나로 잇고(접합), 라이브러리 전체에서 찾는다.

### 편집기

- 마크다운 기호는 흐리게, 제목은 크게, 문단은 책 스타일(정렬·들여쓰기·간격)대로 보여 준다.
- 이미지·GIF·동영상·오디오·YouTube 카드, 표, 각주, 할 일 목록을 본문 안에서 그리고 다룬다.
- 이미지에서 글자 읽어 넣기(OCR), 받아쓰기(기기 안 음성 인식, AI 요약), 특수 문자·이모지.

### 책 만들기

- 프로젝트(또는 고른 그룹·시트)를 별도 창에서 PDF·ePub 쪽 모양으로 미리 본다. 쓰는 대로 다시 그린다.
- 판형, 부·장·절 배치와 번호, 쪽 나눔, 표지·차례·쪽 번호를 프로젝트 스타일로 정한다.
- PDF·ePub·DOCX로 컴파일한다.

### AI 어시스턴트

- 옆 패널에서 시트를 읽고 고치고, 다듬기·교정·브레인스토밍 같은 스킬을 쓴다. 고친 곳은 비교해 보고 받아들인다.
- 제공자: Amazon Bedrock, Alibaba Cloud Token Plan.
- 웹 페이지를 읽을 수 있지만, 처음 보는 사이트는 사용자가 허락해야 한다.

### 확장과 테마

- 확장은 라이브러리의 `Extensions/*.lua`(Luau 샌드박스)다. 시트 안에 그림·애니메이션·상호작용 블록을 그리거나 메뉴 명령을 더한다.
  어시스턴트에게 만들어 달라고 할 수 있다.
- 기본 테마 넷(Writer, Gruvbox의 라이트·다크)에 라이브러리 `Themes/*.json` 테마를 더한다.

## 소스에서 빌드

[mise](https://mise.jdx.dev/)로 도구(Rust, sccache, cmake)를 맞춘다.

```sh
mise install
mise dev      # 개발 모드로 실행
mise check    # 타입 체크
mise bundle   # 번들(macOS .app, Windows .msi)
```

릴리스 순서: `mise notarize`(macOS) → `mise release`(초안에 설치 파일 올리기, 플랫폼마다) → `mise publish`.

## 문서

- [`docs/adr/`](docs/adr/README.md) — 설계 결정 기록
- [`docs/PLAN.md`](docs/PLAN.md), [`docs/feature-map.md`](docs/feature-map.md) — 초기 계획과 기능 표
