# gpui-base 패치

writer는 gpui-base 0.9.1을 `vendor/gpui-base`에 두고 `[patch.crates-io]`로 연결한다(ADR-0006).
에디터의 줄 스타일·줄 블록·접힌 줄·줄 오버레이·스크롤 보정이 이 패치다.
## 지금 상태: `gpui-base-0.9.1-writer-all.patch`

원본 0.9.1(`a/`) → 지금 vendor(`b/`)의 전체 차이다. 깨끗한 0.9.1의 최상위 폴더에서 `patch -p1`로
적용하면 vendor와 같아진다. `gpui-base-0.8.0-writer.patch`는 0.8.0 때의 같은 차이로, 기록으로 남긴다.

우리 코드는 되도록 **원본 파일 밖**에 둔다. 원본 파일에는 연결부(구조체 필드, 호출 몇 줄, `mod` 선언)만
남기고, 기능은 원본 파일의 자식 모듈(`#[path = "…_writer.rs"] mod writer;`)에 둔다. 자식 모듈은 부모의
비공개 필드에 접근할 수 있으므로 원본 코드를 고치지 않고 `impl`을 더할 수 있다.

| 원본 파일 | 우리 코드 |
| --- | --- |
| `input/base/element.rs` | `input/base/element_writer.rs`(줄 블록·오버레이 배치, 행 정렬), `input/base/tests/element_writer.rs`(테스트) |
| `input/base/state.rs` | `input/base/state_writer.rs`(입력·붙여넣기 변환, 접힌 줄 펼치기) |
| `input/editor/mod.rs` | `input/editor/writer.rs`(공개 API: `set_line_style`·`set_line_blocks`·`set_line_overlays`·`set_inline_objects`·`replace_range_keeping_selections`·`set_secondary_click`) |
| `input/editor/display_map/display_map.rs` | `…/display_map_writer.rs`(행 높이 기하) |
| `input/editor/display_map/text_wrapper.rs` | `…/text_wrapper_writer.rs`(`LineStyle` 등 타입, 줄 스타일·블록·펼치기) |
| `input/editor/display_map/wrap_map.rs` | `…/wrap_map_writer.rs` |

원본 파일 안에 남은 변경: `text_wrapper.rs`의 줄바꿈 클로저(줄별 글자 크기·첫 줄 들여쓰기, 인라인 객체를 쪼개지 않는
측정 `measured_wrap_boundaries_with`), `inline_tokens.rs`·`token_presentation.rs`의 파생 객체(문서에 저장하지 않는 토큰),
`element.rs`의 레이아웃·그리기 연결부, `state.rs`의 필드와 붙여넣기·입력 변환 호출,
`overscroll.rs`·`scroll_bounce.rs`의 스크롤 보정(ADR 없음, 아래 두 패치).

### 0.9.1에서 달라진 점

- `overscroll.rs`: `overscroll-tolerance` 패치의 고침(float 오차를 끝으로 읽지 않기, 글 가운데서 뗀
  손가락은 끝을 던지지 않기)이 업스트림에 들어갔다(#3632, #3631). 업스트림 코드를 쓰고 허용치
  `ROUNDING`만 0.5px로 둔다(업스트림 0.01px). 긴 글은 오프셋이 수백만 px라 `f32` 한 칸이 0.25~0.5px다.
  `scroll-reverse`(되돌릴 때 끝이 손가락을 따라감)는 업스트림에 없어 그대로 남긴다.
- `text_wrapper.rs`: 업스트림이 너비 측정 캐시(`WidthMeasurements`)와 크기 조절 중 줄바꿈 재사용
  (`MeasuredWrap`·`WidthRange`, `update_lines`)을 넣었다. 우리 줄바꿈 클로저는 이 구조에 맞췄다.
  보통 줄(글자 크기 1배, 첫 줄 들여쓰기·인라인 객체 없음)은 업스트림의 `measured_wrap`과 캐시를 쓴다.
  나머지 줄은 `measured_wrap_boundaries_with`로 재고 `MeasuredWrap::uncached`로 낸다. 캐시가 글자로만
  찾으므로 글자 크기가 다른 줄에 쓰면 틀린다. 너비로 재는 줄 블록(`line_block`)이 높이를 가진 줄은
  다른 너비에서 재사용하지 않는다. `measured_wrap_boundaries`는 업스트림처럼 테스트용 기준 구현이다.
- `element.rs`: 업스트림의 탭 정렬(`expand_tabs`)·접힌 줄 표시(`folded_lines`)와 우리 줄 블록·오버레이를
  함께 둔다. 레이아웃 기록이 paint에서 prepaint 끝으로 옮겨졌다.
- API: `Modifiers::secondary(cx)`·`secondary_key(cx)`가 `&App`을 받는다.

## gpui-base를 올릴 때

1. 새 버전을 받아 `gpui-base-<지금 버전>-writer-all.patch`를 `patch -p1`로 적용한다.
   충돌이 많으면 원본(지금 버전)을 공통 조상으로 두고 git으로 3-way 병합하면 연결부만 남는다. 새 파일(`*_writer.rs`)은 그대로
   들어가고, 충돌은 원본 파일의 연결부에서만 난다.
2. 충돌을 고친 뒤 `README.md`(저장소 루트, gpui-base 테스트의 `include_str!`가 읽는다)를 잠시 두고
   `cargo test --lib --features test-support`로 gpui-base 테스트를 돌린다.
3. 새 차이로 이 패치를 다시 만든다.

```sh
t=$(mktemp -d); ln -s <원본 gpui-base> $t/a; ln -s "$PWD/vendor/gpui-base" $t/b
(cd $t && diff -ruN a/src b/src) > docs/patches/gpui-base-<버전>-writer-all.patch
```

## 기능별 패치(기록)

아래 파일은 기능을 처음 넣을 때의 차이다. ADR이 가리키는 기록으로 남기며, 지금 코드와는 위치가 다르다
(위 구조로 옮기기 전). 업그레이드에는 쓰지 않는다.

| 패치 | ADR |
| --- | --- |
| `gpui-base-0.8.0-line-style.patch` | ADR-0006 줄 스타일(제목 확대) |
| `gpui-base-0.8.0-line-block.patch` | ADR-0008 줄 블록·접힌 줄 |
| `gpui-base-0.8.0-line-layout.patch`, `gpui-base-0.8.0-line-block-style.patch` | ADR-0010 줄 배치(정렬·양쪽 정렬·첫 줄 들여쓰기·줄 아래 여백) |
| `gpui-base-0.8.0-line-row.patch` | ADR-0016 줄 스타일의 행 번호 |
| `gpui-base-0.8.0-line-block-row.patch` | ADR-0017 표 격자 |
| `gpui-base-0.8.0-line-overlay.patch` | ADR-0023 줄 오버레이 |
| `gpui-base-0.8.0-inline-objects.patch` | ADR-0036 인라인 객체(에디터의 파생 토큰, 객체 행 배치·줄바꿈) |
| `gpui-base-0.8.0-overscroll-tolerance.patch`, `gpui-base-0.8.0-scroll-reverse.patch` | 스크롤 보정 |

# gpui-component 패치

writer는 gpui-component 0.9.1도 `vendor/gpui-component`에 두고 `[patch.crates-io]`로 연결한다.
지금은 찾기(ctrl+F) 검색창의 한글 입력 버그 하나만 고친다. 고친 판이 나오면 지운다.

## 지금 상태: `gpui-component-0.9.1-writer-all.patch`

원본 0.9.1(`a/`) → 지금 vendor(`b/`)의 전체 차이다. 깨끗한 0.9.1의 최상위 폴더에서 `patch -p1`로
적용하면 vendor와 같아진다. `gpui-component-0.8.0-search-echo-ime.patch`는 0.8.0 때의 같은 차이다.

0.9.1에서 업스트림이 앵커 비교를 뺐다(#3772, 아래 원인 1). 검색창 값 비교(원인 2)는 남아 있어,
그 자리를 `wrote_query`로 바꾸는 부분만 패치로 남는다.

- **증상**: 시트 편집기에서 찾기(ctrl+F) 검색창에 한글을 치면 입력한 글자가 사라진다.
  첫 글자를 완성하고 둘째 글자로 넘어갈 때 검색창이 전체 선택되고, 이어서 치면 확정된
  글자까지 지워진다.
- **원인**: 검색창이 자기가 쓴 쿼리를 세션에 메아리로 돌려보내는데, `input/overlay.rs`의
  메아리 판별이 그것을 바깥 변경으로 잘못 읽어 검색창을 다시 연다(`set_value` +
  `select_all` + 포커스). 조합 중인 한글은 이 전체 선택을 다음 조합 글자가 대체해 버려
  입력한 글자가 지워진다. 잘못 읽는 이유는 두 가지다.
  1. **앵커**: 쿼리가 바뀔 때마다 세션의 앵커(`anchor_offset`)가 다시 잡히므로(첫 글자 확정
     `None → Some`, 찾기 이동으로 에디터가 스크롤된 뒤 값 변화) 앵커까지 같아야 메아리로
     보는 판별은 자기 입력을 곧바로 놓친다.
  2. **검색창 값**: 판별이 "검색창이 지금 가진 값 == 세션 쿼리"를 메아리의 근거로 쓰는데,
     조합 중인 글자(marked text)는 `InputEvent::Change`를 내지 않아 세션에 가지 않는다.
     한글 IME는 한 키에서 앞 글자를 확정하고 다음 글자를 조합하므로, 확정("하")이 세션에
     들어가고 곧바로 검색창에는 조합 중 글자("하ㄴ")가 뜬다. 다음 프레임에서 두 값이
     달라 바깥 변경으로 읽히고, `set_value`가 조합 중 글자를 지운 뒤 "하"를 전체 선택한다.
- **고침**: 메아리 판별에서 앵커 비교와 검색창 값 비교를 모두 빼고, 검색창이 세션에
  마지막으로 쓴 쿼리(`SearchPanel::last_written_query`, `wrote_query`)를 대신 본다. 이것이
  조합 중 글자와 무관하게 자기 메아리를 알아보는 유일한 값이다. 되부름(단축키 두 번)은
  여전히 활성화 개정(`activation_revision`)이 잡고, 바깥에서 쿼리를 바꾼 경우는 검색창이
  쓴 적이 없는 쿼리라 메아리가 아니다.
  회귀 테스트 두 개를 함께 넣었다: `typing_in_the_search_field_keeps_the_field_untouched`
  (앵커·찾기 이동), `a_preedit_after_a_commit_is_not_an_external_query_change`(확정 뒤 조합).
  패치는 `input/overlay.rs`(판별)와 `input/search.rs`(마지막으로 쓴 쿼리) 두 파일을 고친다.

## gpui-component를 올릴 때

1. 새 버전을 `vendor/gpui-component`에 받고 이 패치를 `patch -p1`로 적용한다. 충돌은
   `input/overlay.rs`의 메아리 판별과 `input/search.rs`의 쿼리 쓰기에서만 난다.
2. 테스트는 `vendor/gpui-component` 안에서 `cargo test --lib`로 돌린다. 더 간단하게는 gpui-kit 저장소를
   해당 판의 커밋(crate의 `.cargo_vcs_info.json`)으로 받아 `crates/component/src/input/`에 두 파일을
   덮어쓰고 `cargo test -p gpui-component --lib -- input::overlay input::search`로 돌려도 된다.
   패키지 안에서 돌린다면 원본 패키지에는
   모노리포의 테스트 자료가 빠져 있어 그대로는 컴파일이 안 된다. gpui-kit 저장소(버전 맞는
   태그)에서 잠시 받아 와 다음 자리에 두면 된다(끝나면 지운다).
   - `assets/assets/icons/arrow-up.svg`, `assets/assets/icons/search.svg` → `vendor/assets/assets/icons/`
   - `base/src/dock/mod.rs` → `vendor/base/src/dock/`
   - `examples/fixtures/test.md` → 저장소 루트 `examples/fixtures/`
   - `themes/aurora.json` → 저장소 루트 `themes/`(**앱이 사용자 테마로 읽는 자리라 꼭 지운다**)
3. 새 차이로 이 패치를 다시 만든다.

```sh
t=$(mktemp -d); ln -s <원본 gpui-component> $t/a; ln -s "$PWD/vendor/gpui-component" $t/b
(cd $t && diff -ruN a/src b/src) > docs/patches/gpui-component-<버전>-writer-all.patch
```
