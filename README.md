# Hanji

Hanji는 macOS와 Windows용 마크다운 글쓰기 앱입니다. 메모와 원고를 관리하고, PDF, ePub, DOCX로 내보낼 수 있습니다.

글은 선택한 폴더에 `.md` 파일로 저장됩니다. 다른 편집기에서 수정한 내용도 앱에 반영됩니다.

## 설치

[Releases](https://github.com/drskur-com/hanji/releases/latest)에서 설치 파일을 내려받습니다.

- macOS (Apple Silicon): `Hanji-<버전>-arm64.dmg` — 서명·공증 완료
- Windows (x64): `Hanji-<버전>-x64.msi`

Windows에서는 winget으로도 설치할 수 있습니다. 브라우저로 내려받은 설치 파일과 달리 SmartScreen 경고가 표시되지 않습니다.

```powershell
winget install drskur.Hanji
```

앱을 시작할 때 새 버전을 확인합니다. 알림이나 메뉴의 **업데이트 확인…**에서 업데이트할 수 있습니다.

## 시작하기

1. 첫 실행에서 라이브러리 폴더를 선택합니다.
2. Notes에서 메모를 작성하거나 Projects에서 프로젝트를 만듭니다.
3. 원고를 나누어 작성하고 순서를 정합니다.
4. 책 스타일을 설정하고, 미리보기를 확인한 뒤 내보냅니다.

## 주요 기능

### 라이브러리

문서 하나를 **시트**, 시트를 모은 폴더를 **그룹**이라고 부릅니다.

- **Notes**: 메모를 그룹별로 정리하거나 최근 순으로 조회합니다. 기본 그룹은 Inbox입니다. 메모의 할 일 목록을 모아 볼 수 있습니다.
- **Projects**: 원고를 시트와 그룹으로 나누고 순서를 정합니다. 그룹 안에 하위 그룹을 만들 수 있습니다.
- 시트를 끌어서 옮기거나 순서를 바꿉니다. 시트를 연결하면 미리보기와 내보내기에서 원고가 이어집니다.
- 라이브러리 전체에서 제목과 본문을 검색합니다.

### 편집기

- 마크다운 기호는 흐리게 표시하고, 제목·강조·링크에 서식을 적용합니다.
- 책 스타일의 문단 정렬, 들여쓰기, 간격을 편집 화면에 반영합니다.
- 이미지, GIF, 표, 각주, 할 일 목록을 삽입합니다. 오디오 재생과 동영상·YouTube 카드도 지원합니다.
- OCR로 이미지에서 텍스트를 추출해 본문에 넣습니다. 특수 문자와 이모지도 삽입할 수 있습니다.

본문 내 동영상 재생은 macOS에서 지원합니다. YouTube는 macOS에서 별도 창으로, Windows에서 기본 브라우저로 열립니다.

### 책 미리보기와 내보내기

- 프로젝트 전체나 선택한 그룹·시트를 PDF·ePub 미리보기로 확인합니다. 원고를 수정하면 미리보기도 갱신됩니다.
- 프로젝트마다 판형, 부·장·절의 배치와 번호, 쪽 나눔, 표지, 차례, 쪽 번호를 설정합니다.
- PDF, ePub, DOCX로 내보냅니다.

### AI 어시스턴트

오른쪽 패널에서 교정, 문장 수정, 아이디어 구상을 요청합니다. 수정 전후를 비교하고 변경을 적용할 수 있습니다.

- 교정, 다듬기, 브레인스토밍 스킬을 제공합니다. 작업 지침을 담은 스킬 파일은 라이브러리의 `Skills` 폴더에서 수정하거나 추가합니다.
- 웹 페이지를 읽을 수 있습니다. 처음 접근하는 사이트는 사용자 승인이 필요합니다.
- **설정 → AI**에서 제공자를 선택합니다. **Amazon Bedrock**은 AWS 프로필과 리전, **Alibaba Cloud Token Plan**은 전용 API 키를 설정합니다.

AI 사용에는 제공자의 계정과 이용 권한이 필요합니다. 요청에 필요한 원고 내용이 제공자로 전송되며, 해당 서비스의 요금 정책이 적용됩니다.

### 확장과 테마

- `Extensions` 폴더에 Lua(Luau) 스크립트를 추가합니다. 그림, 애니메이션, 클릭·키 입력에 반응하는 블록, 메뉴 명령을 만들 수 있습니다. AI 어시스턴트에게 확장 작성을 요청할 수도 있습니다.
- 확장은 파일 시스템과 네트워크에 직접 접근할 수 없습니다.
- Writer와 Gruvbox의 라이트·다크 테마를 제공합니다. `Themes` 폴더에 JSON 테마를 추가할 수 있습니다.

## 소스에서 빌드하기

Rust와 [gpui](https://www.gpui.rs/)를 사용합니다. [mise](https://mise.jdx.dev/)로 Rust, sccache, cmake를 설치합니다.

```sh
mise install
mise dev      # 개발 모드로 실행
mise check    # 타입 체크
mise bundle   # 설치용 번들 만들기 (macOS .app, Windows .msi)
```

<details>
<summary>개발자용 릴리스 절차</summary>

릴리스는 `mise notarize`(macOS 공증) → `mise release`(플랫폼별 설치 파일을 초안 릴리스에 업로드) → `mise publish`(공개) 순서로 진행합니다. Windows에서 `mise publish`를 실행하면 winget 매니페스트 업데이트 PR도 올립니다([wingetcreate](https://github.com/microsoft/winget-create) 필요, 첫 등록은 `wingetcreate new <MSI URL>`).

</details>

## 문서

- [`docs/adr/`](docs/adr/README.md) — 설계 결정 기록
- [`docs/PLAN.md`](docs/PLAN.md), [`docs/feature-map.md`](docs/feature-map.md) — 초기 계획과 기능 목록
