//! 설정 창: 메인 창과 같은 떠 있는 왼쪽 패널(페이지 목록) + 오른쪽 가운데 정렬 폼.
//!
//! 설정 값은 전역 [`Prefs`]에 두고, 바꾸면 `config.toml`에 저장한다.
//! 메인 창은 [`Prefs`]를 관찰해 다시 그린다.

use gpui_kit::assets::IconName as AssetIcon;
#[cfg(not(target_os = "macos"))]
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, FocusableExt as _, Icon, IndexPath, Sizable as _,
    StyledExt as _, Theme, WindowExt as _,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::*;

use crate::agent_cli::{self, Cli};
use crate::app::style::{PANEL_INSET, Palette, TOOLBAR_HEIGHT, window_drag_area};
use crate::aws_profiles::{self, AwsProfile};
use crate::config::{Config, DictationConfig};
use crate::dictation::{self, ModelStore};
use crate::i18n::{self, Language};
use crate::{icons, theme};

/// 설정 창 크기.
const WINDOW_SIZE: Size<Pixels> = Size {
    width: px(720.0),
    height: px(480.0),
};

/// 왼쪽 페이지 패널 폭.
const PAGE_PANEL_WIDTH: f32 = 180.0;

/// 오른쪽 폼 최대 폭.
const FORM_MAX_WIDTH: f32 = 400.0;

/// 사용자 설정(앱 전역). 시작할 때 `config.toml`에서 읽는다.
#[derive(Clone, Default)]
pub struct Prefs {
    /// 에디터 서체. 없으면 테마 서체(시스템 서체).
    pub editor_font: Option<SharedString>,
    pub editor_font_size: Option<u16>,
    /// 어시스턴트가 쓸 AI 제공자. 없으면 쓰지 않는다.
    pub ai_provider: Option<AiProvider>,
    /// Bedrock에 쓸 AWS 프로필 이름.
    pub aws_profile: Option<SharedString>,
    /// Bedrock을 부를 리전. 없으면 프로필의 리전을 따른다.
    pub aws_region: Option<SharedString>,
    /// 어시스턴트가 부를 Bedrock 모델(추론 프로필) ID. 어시스턴트 패널에서 고른다.
    pub ai_model: Option<SharedString>,
    /// Token Plan API 키.
    pub token_plan_key: Option<SharedString>,
    /// 어시스턴트가 부를 Token Plan 모델 ID. 어시스턴트 패널에서 고른다.
    pub token_plan_model: Option<SharedString>,
    /// 어시스턴트가 부를 Claude Code 모델 별칭. 어시스턴트 패널에서 고른다.
    pub claude_code_model: Option<SharedString>,
    /// 어시스턴트가 부를 Codex 모델 ID. 없으면 Codex의 기본 모델. 어시스턴트 패널에서 고른다.
    pub codex_model: Option<SharedString>,
    /// UI 언어(ADR-0027).
    pub language: Language,
}

impl Global for Prefs {}

/// 어시스턴트가 쓸 AI 제공자.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiProvider {
    /// Amazon Bedrock(AWS 프로필의 자격 증명으로 부른다).
    Bedrock,
    /// Alibaba Cloud Model Studio Token Plan(API 키로 부른다).
    TokenPlan,
    /// 이 머신에 설치된 Claude Code CLI(`claude`, 그 로그인으로 부른다).
    ClaudeCode,
    /// 이 머신에 설치된 Codex CLI(`codex`, 그 로그인·제공자 설정으로 부른다).
    Codex,
}

impl AiProvider {
    const ALL: [AiProvider; 4] = [
        AiProvider::ClaudeCode,
        AiProvider::Codex,
        AiProvider::Bedrock,
        AiProvider::TokenPlan,
    ];

    /// `config.toml`에 적는 이름.
    fn key(self) -> &'static str {
        match self {
            AiProvider::Bedrock => "bedrock",
            AiProvider::TokenPlan => "alibaba-token-plan",
            AiProvider::ClaudeCode => "claude-code",
            AiProvider::Codex => "codex",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
    }

    fn title(self) -> &'static str {
        match self {
            AiProvider::Bedrock => "Amazon Bedrock",
            AiProvider::TokenPlan => "Alibaba Token Plan",
            AiProvider::ClaudeCode => "Claude Code",
            AiProvider::Codex => "Codex",
        }
    }

    /// 에이전트 CLI 제공자면 그 CLI(ADR-0057).
    pub fn cli(self) -> Option<Cli> {
        match self {
            AiProvider::ClaudeCode => Some(Cli::ClaudeCode),
            AiProvider::Codex => Some(Cli::Codex),
            _ => None,
        }
    }
}

impl Prefs {
    /// 설정 파일 값으로 전역 설정을 만든다.
    pub fn init(config: &Config, cx: &mut App) {
        let ai_provider = config.ai_provider.as_deref().and_then(AiProvider::from_key);
        cx.set_global(Prefs {
            editor_font: config.editor_font.clone().map(SharedString::from),
            editor_font_size: config
                .editor_font_size
                .filter(|size| (12..=32).contains(size)),
            ai_provider,
            aws_profile: config.aws_profile.clone().map(SharedString::from),
            aws_region: config.aws_region.clone().map(SharedString::from),
            ai_model: config.ai_model.clone().map(SharedString::from),
            token_plan_key: config.token_plan_api_key.clone().map(SharedString::from),
            token_plan_model: config.token_plan_model.clone().map(SharedString::from),
            claude_code_model: config.claude_code_model.clone().map(SharedString::from),
            codex_model: config.codex_model.clone().map(SharedString::from),
            language: Language::from_key(config.language.as_deref()),
        });
        rust_i18n::set_locale(cx.global::<Prefs>().language.locale());
    }

    /// UI 언어를 바꾸고 저장한다. 메뉴와 창은 바로 바뀐다.
    fn set_language(language: Language, cx: &mut App) {
        if cx.global::<Prefs>().language == language {
            return;
        }
        cx.global_mut::<Prefs>().language = language;
        let mut config = Config::load();
        config.language = language.key().map(str::to_string);
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
        i18n::apply(language, cx);
    }

    /// 에디터에 쓸 서체: 고른 서체, 없으면 테마 서체.
    pub fn editor_font_family(cx: &App) -> SharedString {
        cx.global::<Prefs>()
            .editor_font
            .clone()
            .unwrap_or_else(|| cx.theme().font_family.clone())
    }

    pub fn editor_font_size(cx: &App) -> f32 {
        f32::from(cx.global::<Prefs>().editor_font_size.unwrap_or(16))
    }

    fn set_editor_font_size(size: u16, cx: &mut App) {
        if Self::editor_font_size(cx) == f32::from(size) {
            return;
        }
        cx.global_mut::<Prefs>().editor_font_size = Some(size);
        let mut config = Config::load();
        config.editor_font_size = Some(size);
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// AI 제공자를 바꾸고 저장한다.
    fn set_ai_provider(provider: Option<AiProvider>, cx: &mut App) {
        if cx.global::<Prefs>().ai_provider == provider {
            return;
        }
        cx.global_mut::<Prefs>().ai_provider = provider;
        let mut config = Config::load();
        config.ai_provider = provider.map(|provider| provider.key().to_string());
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// Bedrock에 쓸 AWS 프로필을 바꾸고 저장한다.
    fn set_aws_profile(profile: Option<SharedString>, cx: &mut App) {
        if cx.global::<Prefs>().aws_profile == profile {
            return;
        }
        cx.global_mut::<Prefs>().aws_profile = profile.clone();
        let mut config = Config::load();
        config.aws_profile = profile.map(|profile| profile.to_string());
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// Bedrock 리전을 바꾸고 저장한다. `None`이면 프로필의 리전을 따른다.
    fn set_aws_region(region: Option<SharedString>, cx: &mut App) {
        if cx.global::<Prefs>().aws_region == region {
            return;
        }
        cx.global_mut::<Prefs>().aws_region = region.clone();
        let mut config = Config::load();
        config.aws_region = region.map(|region| region.to_string());
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// 어시스턴트가 부를 모델 ID를 지금 제공자의 것으로 바꾸고 저장한다(Bedrock이면 추론 프로필 ID).
    pub(crate) fn set_ai_model(model: Option<SharedString>, cx: &mut App) {
        let provider = cx.global::<Prefs>().ai_provider;
        let prefs = cx.global_mut::<Prefs>();
        let saved = match provider {
            Some(AiProvider::TokenPlan) => &mut prefs.token_plan_model,
            Some(AiProvider::ClaudeCode) => &mut prefs.claude_code_model,
            Some(AiProvider::Codex) => &mut prefs.codex_model,
            _ => &mut prefs.ai_model,
        };
        if *saved == model {
            return;
        }
        *saved = model.clone();
        let mut config = Config::load();
        let model = model.map(|model| model.to_string());
        match provider {
            Some(AiProvider::TokenPlan) => config.token_plan_model = model,
            Some(AiProvider::ClaudeCode) => config.claude_code_model = model,
            Some(AiProvider::Codex) => config.codex_model = model,
            _ => config.ai_model = model,
        }
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// Token Plan API 키를 바꾸고 저장한다. 빈 글이면 지운다.
    fn set_token_plan_key(key: &str, cx: &mut App) {
        let key = key.trim();
        let key = (!key.is_empty()).then(|| key.to_string());
        if cx.global::<Prefs>().token_plan_key.as_deref() == key.as_deref() {
            return;
        }
        cx.global_mut::<Prefs>().token_plan_key = key.clone().map(SharedString::from);
        let mut config = Config::load();
        config.token_plan_api_key = key;
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }

    /// 에디터 서체를 바꾸고 저장한다.
    fn set_editor_font(font: Option<SharedString>, cx: &mut App) {
        if cx.global::<Prefs>().editor_font == font {
            return;
        }
        cx.global_mut::<Prefs>().editor_font = font.clone();
        let mut config = Config::load();
        config.editor_font = font.map(|font| font.to_string());
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
    }
}

/// 열려 있는 설정 창. 다시 열기 요청이 오면 새로 만들지 않고 앞으로 가져온다.
struct OpenSettingsWindow(AnyWindowHandle);

impl Global for OpenSettingsWindow {}

/// 설정 창을 연다(이미 열려 있으면 앞으로 가져온다).
pub fn open(cx: &mut App) {
    if let Some(OpenSettingsWindow(handle)) = cx.try_global::<OpenSettingsWindow>()
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }

    let window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
        None,
        WINDOW_SIZE,
        cx,
    )));
    #[cfg(not(target_os = "macos"))]
    let options = WindowOptions {
        window_bounds,
        window_min_size: Some(size(px(560.0), px(360.0))),
        ..TitleBar::window_options()
    };
    // 메인 창과 같은 투명 타이틀바. 신호등 버튼은 왼쪽 패널 위쪽 줄 안에 둔다.
    #[cfg(target_os = "macos")]
    let options = WindowOptions {
        window_bounds,
        window_min_size: Some(size(px(560.0), px(360.0))),
        titlebar: Some(TitlebarOptions {
            title: Some(t!("settings.title").into()),
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(PANEL_INSET + 12.0),
                px(TOOLBAR_HEIGHT / 2.0 - 7.0),
            )),
        }),
        ..Default::default()
    };

    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| SettingsWindow::new(window, cx))
    }) {
        Ok((handle, _)) => cx.set_global(OpenSettingsWindow(handle)),
        Err(err) => eprintln!("설정 창을 열 수 없습니다: {err}"),
    }
}

/// 설정 페이지.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Ai,
    Dictation,
}

impl Page {
    const ALL: [Page; 3] = [Page::General, Page::Ai, Page::Dictation];

    fn id(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Ai => "ai",
            Page::Dictation => "dictation",
        }
    }

    fn title(self) -> SharedString {
        match self {
            Page::General => t!("settings.page.general").into(),
            Page::Ai => t!("settings.page.ai").into(),
            Page::Dictation => t!("settings.page.dictation").into(),
        }
    }

    fn icon(self) -> AssetIcon {
        match self {
            Page::General => icons::settings(),
            Page::Ai => icons::ai_settings(),
            Page::Dictation => icons::dictation(),
        }
    }
}

/// 서체 선택지 하나. 이름을 그 서체로 그린다.
#[derive(Clone)]
pub(crate) struct FontItem {
    /// 표시 이름.
    title: SharedString,
    /// 서체 이름. 시스템 서체는 `None`.
    pub(crate) value: Option<SharedString>,
    /// 그릴 때 쓰는 서체(시스템 서체는 테마 서체).
    family: SharedString,
}

impl SearchableListItem for FontItem {
    type Value = Option<SharedString>;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(
            div()
                .text_lg()
                .font_family(self.family.clone())
                .child(self.title.clone())
                .into_any_element(),
        )
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .font_family(self.family.clone())
            .child(self.title.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

/// 서체 선택지: 맨 앞의 기본 항목(`value`가 `None`, `default_family`로 그림) + 설치된 서체.
pub(crate) fn font_items(
    default_title: impl Into<SharedString>,
    default_family: SharedString,
    cx: &App,
) -> Vec<FontItem> {
    // `.`으로 시작하는 이름은 시스템 내부 서체라 고를 수 없게 숨긴다.
    let fonts = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|name| !name.starts_with('.'))
        .map(|name| {
            let name = SharedString::from(name);
            FontItem {
                title: name.clone(),
                value: Some(name.clone()),
                family: name,
            }
        });
    std::iter::once(FontItem {
        title: default_title.into(),
        value: None,
        family: default_family,
    })
    .chain(fonts)
    .collect()
}

#[derive(Clone)]
struct FontSizeItem(u16);

impl SearchableListItem for FontSizeItem {
    type Value = u16;

    fn title(&self) -> SharedString {
        format!("{} px", self.0).into()
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().child(self.title())
    }

    fn value(&self) -> &u16 {
        &self.0
    }
}

/// 이름만 있는 선택지(AI 제공자, AWS 프로필). 값이 `None`이면 "사용 안 함" 같은 비움 항목.
#[derive(Clone)]
struct ChoiceItem {
    title: SharedString,
    value: Option<SharedString>,
}

impl SearchableListItem for ChoiceItem {
    type Value = Option<SharedString>;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().child(self.title.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

/// 설정 창 화면.
struct SettingsWindow {
    page: Page,
    /// 선택지를 만들 때의 언어. 바뀌면 선택지를 다시 만든다(이름이 그 언어로 들어 있다).
    language: Language,
    language_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    /// 테마 선택지를 만들 때의 (테마 이름들, 지금 테마). 바뀌면 선택지를 다시 넣는다.
    themes: (Vec<SharedString>, SharedString),
    theme_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    font_picker: Entity<SelectState<SearchableVec<FontItem>>>,
    font_size_picker: Entity<SelectState<SearchableVec<FontSizeItem>>>,
    provider_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    profile_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    region_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    /// Token Plan API 키 입력(가려 보인다).
    token_plan_key: Entity<InputState>,
    /// 창을 열 때 읽은 AWS 프로필.
    aws_profiles: Vec<AwsProfile>,
    /// 에이전트 CLI를 찾은 결과(어느 CLI인지와 상태). 에이전트 제공자를 고르면 찾는다.
    cli: Option<(Cli, CliStatus)>,
    _cli_lookup: Option<Task<()>>,
    /// 받아쓰기 모델 캐시와 내려받기(앱 전역).
    model_store: Entity<ModelStore>,
    /// 받아쓰기의 LLM 옵션(ADR-0055). 바꾸면 `config.toml`에 저장한다.
    dictation: DictationConfig,
    summary_language_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    /// 받아쓰는 언어(ADR-0056).
    dictation_language_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    /// 실시간 받아쓰기 엔진(ADR-0056).
    engine_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    dictation_model_picker: Entity<SelectState<SearchableVec<ChoiceItem>>>,
    dictation_models: DictationModels,
    /// 모델 목록을 읽은 곳(`Backend::models_key`). 바뀌면 다시 읽는다.
    dictation_models_key: Option<String>,
    _dictation_model_lookup: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// 에이전트 CLI 찾기 상태.
enum CliStatus {
    Detecting,
    Found(agent_cli::Install),
    NotFound,
}

/// 받아쓰기 LLM 모델 확인 상태.
enum DictationModels {
    /// 아직 읽지 않았거나 AI를 설정하지 않았다.
    Unavailable,
    Loading,
    Ready,
    Failed(SharedString),
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 언어: 시스템, English, 한국어.
        let language = cx.global::<Prefs>().language;
        let languages: Vec<_> = Language::ALL
            .into_iter()
            .map(|language| ChoiceItem {
                title: language.title().into(),
                value: language.key().map(SharedString::from),
            })
            .collect();
        let selected_language = Language::ALL
            .iter()
            .position(|item| *item == language)
            .unwrap_or(0);
        let language_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(languages),
                Some(IndexPath::new(selected_language)),
                window,
                cx,
            )
        });
        let themes = (theme::theme_names(cx), theme::active_name(cx));
        let (theme_items, selected_theme) = Self::theme_items(&themes);
        let theme_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(theme_items),
                selected_theme.map(IndexPath::new),
                window,
                cx,
            )
        });
        let system_family = cx.theme().font_family.clone();
        let items = font_items(t!("settings.font.system"), system_family, cx);
        let current = cx.global::<Prefs>().editor_font.clone();
        let selected = items
            .iter()
            .position(|item| item.value == current)
            .unwrap_or(0);
        let font_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(items),
                Some(IndexPath::new(selected)),
                window,
                cx,
            )
            .searchable(true)
        });

        let current_size = Prefs::editor_font_size(cx) as usize;
        let font_size_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new((12..=32).map(FontSizeItem).collect::<Vec<_>>()),
                Some(IndexPath::new(current_size - 12)),
                window,
                cx,
            )
        });
        // AI 제공자: 맨 앞은 "사용 안 함".
        let providers: Vec<_> = std::iter::once(ChoiceItem {
            title: t!("settings.ai.provider.none").into(),
            value: None,
        })
        .chain(AiProvider::ALL.into_iter().map(|provider| ChoiceItem {
            title: provider.title().into(),
            value: Some(provider.key().into()),
        }))
        .collect();
        let current_provider = cx.global::<Prefs>().ai_provider.map(|p| p.key());
        let selected_provider = providers
            .iter()
            .position(|item| item.value.as_deref() == current_provider)
            .unwrap_or(0);
        let provider_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(providers),
                Some(IndexPath::new(selected_provider)),
                window,
                cx,
            )
        });

        // AWS 프로필: 저장한 프로필이 목록에 없으면 고르지 않은 상태로 둔다.
        let aws_profiles = aws_profiles::list();
        let current_profile = cx.global::<Prefs>().aws_profile.clone();
        let selected_profile = aws_profiles
            .iter()
            .position(|profile| Some(profile.name.as_str()) == current_profile.as_deref());
        let profile_items: Vec<_> = aws_profiles
            .iter()
            .map(|profile| ChoiceItem {
                title: profile.name.clone().into(),
                value: Some(profile.name.clone().into()),
            })
            .collect();
        let profile_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(profile_items),
                selected_profile.map(IndexPath::new),
                window,
                cx,
            )
            .searchable(true)
        });

        // 리전: 맨 앞은 "프로필 리전 따르기". 저장한 리전이 목록에 없으면(직접 고친 설정) 끝에 더한다.
        let current_region = cx.global::<Prefs>().aws_region.clone();
        let mut regions: Vec<_> = std::iter::once(ChoiceItem {
            title: t!("settings.ai.region.follow_profile").into(),
            value: None,
        })
        .chain(
            aws_profiles::BEDROCK_REGIONS
                .iter()
                .map(|(code, name)| ChoiceItem {
                    title: format!("{name} · {code}").into(),
                    value: Some((*code).into()),
                }),
        )
        .collect();
        if let Some(region) = &current_region
            && !regions
                .iter()
                .any(|item| item.value.as_ref() == Some(region))
        {
            regions.push(ChoiceItem {
                title: region.clone(),
                value: Some(region.clone()),
            });
        }
        let selected_region = regions
            .iter()
            .position(|item| item.value == current_region)
            .unwrap_or(0);
        let region_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(regions),
                Some(IndexPath::new(selected_region)),
                window,
                cx,
            )
            .searchable(true)
        });

        // Token Plan API 키.
        let saved_key = cx.global::<Prefs>().token_plan_key.clone();
        let token_plan_key = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .masked(true)
                .placeholder(t!("settings.ai.token_plan.key_placeholder"));
            if let Some(key) = saved_key {
                input.set_value(key, window, cx);
            }
            input
        });

        // 받아쓰기 모델: 창을 열 때 캐시를 다시 살핀다(밖에서 받거나 지웠을 수 있다).
        let model_store = ModelStore::global(cx);
        model_store.update(cx, |store, cx| store.refresh(cx));

        // 받아쓰기 요약 언어: 맨 앞은 "기록의 언어".
        let dictation = Config::load().dictation;
        let language_items: Vec<_> = std::iter::once(ChoiceItem {
            title: t!("settings.dictation.summary_language.none").into(),
            value: None,
        })
        .chain(
            crate::agent::dictation::LANGUAGES
                .iter()
                .map(|language| ChoiceItem {
                    title: language.native.into(),
                    value: Some(language.code.into()),
                }),
        )
        .collect();
        let selected_language = language_items
            .iter()
            .position(|item| item.value.as_deref() == dictation.summary_language.as_deref())
            .unwrap_or(0);
        let summary_language_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(language_items),
                Some(IndexPath::new(selected_language)),
                window,
                cx,
            )
        });
        // 받아쓰는 언어(macOS 받아쓰기·Cohere)와 실시간 받아쓰기 엔진(ADR-0056).
        let dictation_language_items: Vec<_> = dictation::DICTATION_LANGUAGES
            .iter()
            .map(|language| ChoiceItem {
                title: language.native.into(),
                value: Some(language.code.into()),
            })
            .collect();
        let selected_dictation_language = dictation_language_items
            .iter()
            .position(|item| item.value.as_deref() == Some(dictation.language().code))
            .unwrap_or(0);
        let dictation_language_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(dictation_language_items),
                Some(IndexPath::new(selected_dictation_language)),
                window,
                cx,
            )
        });
        let engine_items: Vec<_> = dictation::Engine::available()
            .iter()
            .map(|engine| ChoiceItem {
                title: engine_title(*engine),
                value: Some(engine_key(*engine).into()),
            })
            .collect();
        let selected_engine = dictation::Engine::available()
            .iter()
            .position(|engine| *engine == dictation.engine)
            .unwrap_or(0);
        let engine_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(engine_items),
                Some(IndexPath::new(selected_engine)),
                window,
                cx,
            )
        });
        let dictation_model_picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<ChoiceItem>::new()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });

        let subscriptions = vec![
            cx.observe(&model_store, |_, _, cx| cx.notify()),
            cx.subscribe(
                &summary_language_picker,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(Some(code)) = event else {
                        return;
                    };
                    this.dictation.summary_language = code.as_ref().map(ToString::to_string);
                    this.save_dictation(cx);
                },
            ),
            cx.subscribe(
                &dictation_language_picker,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(Some(Some(code))) = event else {
                        return;
                    };
                    this.dictation.language = Some(code.to_string());
                    this.save_dictation(cx);
                },
            ),
            cx.subscribe(
                &engine_picker,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(Some(Some(key))) = event else {
                        return;
                    };
                    if let Some(engine) = dictation::Engine::available()
                        .iter()
                        .find(|engine| engine_key(**engine) == key.as_ref())
                    {
                        this.dictation.engine = *engine;
                        this.save_dictation(cx);
                    }
                },
            ),
            cx.subscribe(
                &dictation_model_picker,
                |this, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(Some(Some(id))) = event else {
                        return;
                    };
                    let id = Some(id.to_string());
                    match cx.global::<Prefs>().ai_provider {
                        Some(AiProvider::TokenPlan) => this.dictation.token_plan_model = id,
                        _ => this.dictation.bedrock_model = id,
                    }
                    this.save_dictation(cx);
                },
            ),
            cx.subscribe(
                &language_picker,
                |_, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(key) = value {
                        Prefs::set_language(Language::from_key(key.as_deref()), cx);
                    }
                },
            ),
            cx.subscribe(
                &theme_picker,
                |_, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(Some(name)) = value {
                        theme::choose(name, cx);
                    }
                },
            ),
            // 메뉴·단축키로 바꾸거나 라이브러리 테마 파일이 바뀌면 선택지를 맞춘다.
            cx.observe_global_in::<Theme>(window, |this, window, cx| {
                let themes = (theme::theme_names(cx), theme::active_name(cx));
                if themes != this.themes {
                    let (items, selected) = Self::theme_items(&themes);
                    this.theme_picker.update(cx, |picker, cx| {
                        picker.set_items(SearchableVec::new(items), window, cx);
                        picker.set_selected_index(selected.map(IndexPath::new), window, cx);
                    });
                    this.themes = themes;
                }
                cx.notify();
            }),
            cx.subscribe(
                &font_picker,
                |_, _, event: &SelectEvent<SearchableVec<FontItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(font) = value {
                        Prefs::set_editor_font(font.clone(), cx);
                    }
                },
            ),
            cx.subscribe(
                &font_size_picker,
                |_, _, event: &SelectEvent<SearchableVec<FontSizeItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(size) = value {
                        Prefs::set_editor_font_size(*size, cx);
                    }
                },
            ),
            cx.subscribe(
                &provider_picker,
                |_, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(key) = value {
                        Prefs::set_ai_provider(key.as_deref().and_then(AiProvider::from_key), cx);
                    }
                },
            ),
            cx.subscribe(
                &profile_picker,
                |_, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(profile) = value {
                        Prefs::set_aws_profile(profile.clone(), cx);
                    }
                },
            ),
            cx.subscribe(
                &region_picker,
                |_, _, event: &SelectEvent<SearchableVec<ChoiceItem>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(region) = value {
                        Prefs::set_aws_region(region.clone(), cx);
                    }
                },
            ),
            cx.subscribe(&token_plan_key, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let key = input.read(cx).value().to_string();
                    Prefs::set_token_plan_key(&key, cx);
                }
            }),
            // 언어가 바뀌면 선택지 이름도 그 언어로 다시 만든다. 보던 페이지는 그대로 둔다.
            cx.observe_global_in::<Prefs>(window, |this, window, cx| {
                if cx.global::<Prefs>().language != this.language {
                    let page = this.page;
                    *this = Self::new(window, cx);
                    this.page = page;
                }
                this.detect_cli(false, window, cx);
                // AI 제공자·프로필이 바뀌면 받아쓰기 모델 목록을 다시 읽는다.
                if this.page == Page::Dictation {
                    this.load_dictation_models(window, cx);
                }
                cx.notify();
            }),
        ];

        let mut this = Self {
            page: Page::General,
            language,
            language_picker,
            themes,
            theme_picker,
            font_picker,
            font_size_picker,
            provider_picker,
            profile_picker,
            region_picker,
            token_plan_key,
            aws_profiles,
            cli: None,
            _cli_lookup: None,
            model_store,
            dictation,
            summary_language_picker,
            dictation_language_picker,
            engine_picker,
            dictation_model_picker,
            dictation_models: DictationModels::Unavailable,
            dictation_models_key: None,
            _dictation_model_lookup: None,
            _subscriptions: subscriptions,
        };
        this.detect_cli(false, window, cx);
        this
    }

    /// 에이전트 제공자를 고른 상태면 그 CLI를 찾는다. `force`가 아니면 이미 그 CLI를 찾았을 때 그대로 둔다.
    fn detect_cli(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cli) = cx.global::<Prefs>().ai_provider.and_then(AiProvider::cli) else {
            return;
        };
        if !force && self.cli.as_ref().is_some_and(|(found, _)| *found == cli) {
            return;
        }
        self.cli = Some((cli, CliStatus::Detecting));
        let override_path = cli.configured_path(&Config::load());
        let lookup = cx
            .background_executor()
            .spawn(async move { agent_cli::detect(cli, override_path.as_deref()) });
        self._cli_lookup = Some(cx.spawn_in(window, async move |this, cx| {
            let found = lookup.await;
            this.update(cx, |this, cx| {
                let status = match found {
                    Some(install) => CliStatus::Found(install),
                    None => CliStatus::NotFound,
                };
                this.cli = Some((cli, status));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// 받아쓰기 LLM 옵션을 저장한다(다음 녹음부터).
    fn save_dictation(&mut self, cx: &mut Context<Self>) {
        let mut config = Config::load();
        config.dictation.auto_gain = self.dictation.auto_gain;
        config.dictation.save_audio = self.dictation.save_audio;
        config.dictation.cohere = self.dictation.cohere;
        config.dictation.engine = self.dictation.engine;
        config.dictation.language = self.dictation.language.clone();
        config.dictation.summarize = self.dictation.summarize;
        config.dictation.summary_language = self.dictation.summary_language.clone();
        config.dictation.bedrock_model = self.dictation.bedrock_model.clone();
        config.dictation.token_plan_model = self.dictation.token_plan_model.clone();
        if let Err(err) = config.save() {
            eprintln!("설정 저장 실패: {err}");
        }
        cx.notify();
    }

    /// 받아쓰기에 쓸 수 있는 모델을 읽어 고르기를 채운다. 같은 곳을 이미 읽었으면 그대로 둔다.
    fn load_dictation_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = crate::agent::Backend::llm_from_prefs(cx.global::<Prefs>()) else {
            self.dictation_models = DictationModels::Unavailable;
            self.dictation_models_key = None;
            return;
        };
        let key = backend.models_key();
        if self.dictation_models_key.as_ref() == Some(&key) {
            return;
        }
        self.dictation_models_key = Some(key);
        self.dictation_models = DictationModels::Loading;
        let lookup = backend.available_models();
        self._dictation_model_lookup = Some(cx.spawn_in(window, async move |this, cx| {
            let result = lookup
                .await
                .unwrap_or_else(|_| Err(t!("assistant.model.lookup_stopped").into()));
            this.update_in(cx, |this, window, cx| {
                let models = match result {
                    Ok(models) => models,
                    Err(err) => {
                        this.dictation_models = DictationModels::Failed(err.into());
                        cx.notify();
                        return;
                    }
                };
                // 저장한 모델이 없으면 어시스턴트의 모델을 고른다.
                let saved = match cx.global::<Prefs>().ai_provider {
                    Some(AiProvider::TokenPlan) => this.dictation.token_plan_model.clone(),
                    _ => this.dictation.bedrock_model.clone(),
                };
                let wanted = saved.unwrap_or_else(|| backend.model().to_string());
                let selected = models
                    .iter()
                    .position(|(model, _)| model.matches(&wanted))
                    .or_else(|| (!models.is_empty()).then_some(0));
                let items: Vec<_> = models
                    .iter()
                    .map(|(model, id)| ChoiceItem {
                        title: model.name.to_string().into(),
                        value: Some(id.clone().into()),
                    })
                    .collect();
                this.dictation_model_picker.update(cx, |picker, cx| {
                    picker.set_items(SearchableVec::new(items), window, cx);
                    picker.set_selected_index(selected.map(IndexPath::new), window, cx);
                });
                this.dictation_models = DictationModels::Ready;
                cx.notify();
            })
            .ok();
        }));
    }

    /// 왼쪽 떠 있는 페이지 패널(메인 창의 라이브러리 패널과 같은 모양).
    fn render_page_panel(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        let rows = Page::ALL
            .into_iter()
            .filter(|page| *page != Page::Dictation || crate::dictation::ENABLED)
            .map(|page| {
                let selected = page == self.page;
                let (text_color, icon_color) = if selected {
                    (pal.accent_foreground, pal.accent_foreground)
                } else {
                    (pal.foreground, pal.foreground.opacity(0.7))
                };
                div()
                    .id(page.id())
                    .mx_2()
                    .px_2()
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_lg()
                    .text_sm()
                    .text_color(text_color)
                    .when(selected, |this| this.bg(pal.accent).font_semibold())
                    .when(!selected, |this| {
                        this.hover(move |style| style.bg(pal.hover))
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.page = page;
                        if page == Page::Dictation {
                            this.model_store.update(cx, |store, cx| store.refresh(cx));
                            this.load_dictation_models(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(Icon::new(page.icon()).small().text_color(icon_color))
                    .child(page.title())
            });

        div().h_full().flex_shrink_0().p(px(PANEL_INSET)).child(
            div()
                .w(px(PAGE_PANEL_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .rounded_xl()
                .bg(pal.panel)
                .border_1()
                .border_color(pal.panel_border)
                .shadow_md()
                // 위쪽 줄: (macOS) 신호등 버튼 자리.
                .child(window_drag_area(
                    div()
                        .h(px(TOOLBAR_HEIGHT - PANEL_INSET * 2.0))
                        .flex_shrink_0(),
                ))
                .child(div().pt_1().flex().flex_col().gap_1().children(rows)),
        )
    }

    /// 테마 선택지와 지금 테마의 위치.
    fn theme_items(
        (names, active): &(Vec<SharedString>, SharedString),
    ) -> (Vec<ChoiceItem>, Option<usize>) {
        let items = names
            .iter()
            .map(|name| ChoiceItem {
                title: name.clone(),
                value: Some(name.clone()),
            })
            .collect();
        (items, names.iter().position(|name| name == active))
    }

    /// 확인을 받고 라이브러리 테마 파일 `file`을 휴지통으로 옮긴다.
    fn confirm_delete_theme(file: std::path::PathBuf, window: &mut Window, cx: &mut App) {
        let file_name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let themes = theme::themes_in(&file, cx).join(", ");
        let description = t!(
            "settings.theme.delete_description",
            file = file_name,
            themes = themes
        )
        .to_string();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let file = file.clone();
            alert
                .confirm()
                .title(t!("settings.theme.delete_title"))
                .description(description.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("common.delete"))
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("common.cancel")),
                )
                .on_ok(move |_, window, cx| {
                    if let Err(err) = theme::delete_file(&file, cx) {
                        window.push_notification(
                            t!("settings.theme.delete_failed", err = err).to_string(),
                            cx,
                        );
                    }
                    true
                })
        });
    }

    /// 테마 선택과, 라이브러리 테마면 그 파일(보기·삭제). 시스템 테마는 지울 수 없다.
    fn render_theme(&self, cx: &App, pal: Palette) -> impl IntoElement {
        let file = theme::library_file(&self.themes.1, cx);
        let hint = match &file {
            Some(file) => t!(
                "settings.theme.hint_library",
                file = format!(
                    "{}/{}",
                    theme::THEMES_DIR,
                    file.file_name().unwrap_or_default().to_string_lossy()
                )
            )
            .to_string(),
            None => t!("settings.theme.hint_system").to_string(),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(t!("settings.theme"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div().flex_1().min_w_0().child(
                            Select::new(&self.theme_picker)
                                .large()
                                .focus_ring(false)
                                .menu_max_h(px(320.0)),
                        ),
                    )
                    .when_some(file, |this, file| {
                        let reveal = file.clone();
                        this.child(
                            Button::new("theme-reveal")
                                .ghost()
                                .icon(icons::reveal())
                                .tooltip(t!("common.reveal_in_finder"))
                                .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                        )
                        .child(
                            Button::new("theme-delete")
                                .ghost()
                                .icon(Icon::new(icons::trash()).text_color(pal.danger))
                                .tooltip(t!("common.move_to_trash"))
                                .on_click(move |_, window, cx| {
                                    Self::confirm_delete_theme(file.clone(), window, cx)
                                }),
                        )
                    }),
            )
            .child(div().text_sm().text_color(pal.muted).child(hint))
    }

    /// 일반: 언어, 테마, 에디터 서체와 글자 크기.
    fn render_general(&self, cx: &App, pal: Palette) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.language"))
                    .child(Select::new(&self.language_picker).large().focus_ring(false)),
            )
            .child(self.render_theme(cx, pal))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.font"))
                    .child(
                        Select::new(&self.font_picker)
                            .large()
                            .focus_ring(false)
                            .menu_max_h(px(320.0))
                            .search_placeholder(t!("settings.font.search")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.font.size"))
                    .child(
                        Select::new(&self.font_size_picker)
                            .large()
                            .focus_ring(false)
                            .menu_max_h(px(240.0)),
                    ),
            )
    }
}

impl SettingsWindow {
    /// AI: 어시스턴트가 쓸 제공자와, Bedrock이면 AWS 프로필.
    fn render_ai(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        let prefs = cx.global::<Prefs>().clone();
        let saved_profile = prefs.aws_profile.clone();
        let profile = saved_profile.as_ref().and_then(|name| {
            self.aws_profiles
                .iter()
                .find(|profile| profile.name == name.as_ref())
        });
        // 프로필 아래 안내: 프로필이 없거나 사라졌다는 안내, 아니면 프로필을 읽는 곳.
        let hint: Option<SharedString> = if self.aws_profiles.is_empty() {
            Some(t!("settings.ai.profile.hint_none").into())
        } else if profile.is_some() {
            None
        } else if let Some(name) = &saved_profile {
            Some(t!("settings.ai.profile.hint_missing", name = name).into())
        } else {
            Some(t!("settings.ai.profile.hint_source").into())
        };
        // 리전 아래 안내: 프로필 리전을 따를 때 실제로 쓰일 리전.
        let region_hint: Option<SharedString> = match (&prefs.aws_region, profile) {
            (Some(_), _) => None,
            (None, Some(profile)) => Some(match &profile.region {
                Some(region) => t!("settings.ai.region.hint_profile", region = region).into(),
                None => t!("settings.ai.region.hint_env").into(),
            }),
            (None, None) => Some(t!("settings.ai.region.hint_pick").into()),
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.ai.provider"))
                    .child(Select::new(&self.provider_picker).large().focus_ring(false)),
            )
            .when(prefs.ai_provider == Some(AiProvider::Bedrock), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(t!("settings.ai.profile"))
                        .child(
                            Select::new(&self.profile_picker)
                                .large()
                                .focus_ring(false)
                                .disabled(self.aws_profiles.is_empty())
                                .placeholder(t!("settings.ai.profile.placeholder"))
                                .menu_max_h(px(240.0))
                                .search_placeholder(t!("settings.ai.profile.search")),
                        )
                        .children(
                            hint.map(|hint| div().text_sm().text_color(pal.muted).child(hint)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(t!("settings.ai.region"))
                        .child(
                            Select::new(&self.region_picker)
                                .large()
                                .focus_ring(false)
                                .menu_max_h(px(240.0))
                                .search_placeholder(t!("settings.ai.region.search")),
                        )
                        .children(
                            region_hint
                                .map(|hint| div().text_sm().text_color(pal.muted).child(hint)),
                        ),
                )
            })
            .when(prefs.ai_provider == Some(AiProvider::TokenPlan), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(t!("settings.ai.token_plan.key"))
                        .child(
                            Input::new(&self.token_plan_key)
                                .large()
                                .mask_toggle()
                                .focus_ring(false),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(pal.muted)
                                .child(t!("settings.ai.token_plan.key_hint")),
                        ),
                )
            })
            .when_some(prefs.ai_provider.and_then(AiProvider::cli), |this, cli| {
                this.child(self.render_cli(cli, cx, pal))
            })
    }

    /// 에이전트 CLI: 찾은 실행 파일의 경로와 버전, 못 찾았으면 설치 안내.
    fn render_cli(&self, cli: Cli, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        let hint = |text: SharedString| div().text_sm().text_color(pal.muted).child(text);
        let name = cli.title();
        let program = cli.program();
        let state = self
            .cli
            .as_ref()
            .filter(|(found, _)| *found == cli)
            .map(|(_, state)| state);
        // 찾기 결과: 색(성공·실패·찾는 중), 아이콘, 제목, 자세한 줄.
        let (color, icon, title, details): (Hsla, AssetIcon, SharedString, Vec<SharedString>) =
            match state {
                None | Some(CliStatus::Detecting) => (
                    pal.muted,
                    icons::detecting(),
                    t!("settings.ai.cli.detecting", name = name).into(),
                    Vec::new(),
                ),
                Some(CliStatus::Found(install)) => (
                    pal.success,
                    icons::found(),
                    t!("settings.ai.cli.found", name = name).into(),
                    vec![
                        install.path.display().to_string().into(),
                        match &install.version {
                            Some(version) => {
                                t!("settings.ai.cli.version", version = version).into()
                            }
                            None => t!("settings.ai.cli.version_unknown", program = program).into(),
                        },
                    ],
                ),
                Some(CliStatus::NotFound) => (
                    pal.danger,
                    icons::not_found(),
                    t!("settings.ai.cli.not_found", name = name).into(),
                    vec![t!("settings.ai.cli.not_found_hint", program = program).into()],
                ),
            };
        let status = div()
            .flex()
            .gap_3()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(color.opacity(0.5))
            .bg(color.opacity(0.1))
            .child(Icon::new(icon).text_color(color).flex_none().mt_0p5())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_semibold().text_color(color).child(title))
                    .children(details.into_iter().map(|line| {
                        div()
                            .text_sm()
                            .text_color(pal.muted)
                            .overflow_x_hidden()
                            .text_ellipsis()
                            .child(line)
                    })),
            );
        let detecting = matches!(state, Some(CliStatus::Detecting));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(name)
                    .child(
                        Button::new("cli-detect")
                            .ghost()
                            .small()
                            .label(t!("settings.ai.cli.detect"))
                            .disabled(detecting)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.detect_cli(true, window, cx);
                            })),
                    ),
            )
            .child(status)
            .child(hint(match cli {
                Cli::ClaudeCode => t!("settings.ai.cli.hint_claude_code").into(),
                Cli::Codex => t!("settings.ai.cli.hint_codex").into(),
            }))
    }
}

/// 실시간 받아쓰기 엔진의 이름.
fn engine_title(engine: dictation::Engine) -> SharedString {
    match engine {
        dictation::Engine::Apple => t!("settings.dictation.engine.apple").into(),
        dictation::Engine::Nemotron => "Nemotron Streaming 3.5".into(),
    }
}

/// 엔진 고르기의 값(`config.toml`의 이름과 같다).
fn engine_key(engine: dictation::Engine) -> &'static str {
    match engine {
        dictation::Engine::Apple => "apple",
        dictation::Engine::Nemotron => "nemotron",
    }
}

impl SettingsWindow {
    /// 받아쓰기의 소리 입력: 자동 음량 맞춤, 녹음 소리 남기기.
    fn render_dictation_input(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        div()
            .pt_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Checkbox::new("dictation-auto-gain")
                    .label(t!("settings.dictation.auto_gain").to_string())
                    .checked(self.dictation.auto_gain)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.dictation.auto_gain = *checked;
                        this.save_dictation(cx);
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(pal.muted)
                    .child(t!("settings.dictation.auto_gain_hint")),
            )
            .child(
                div().pt_2().child(
                    Checkbox::new("dictation-save-audio")
                        .label(t!("settings.dictation.save_audio").to_string())
                        .checked(self.dictation.save_audio)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.dictation.save_audio = *checked;
                            this.save_dictation(cx);
                        })),
                ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(pal.muted)
                    .child(t!("settings.dictation.save_audio_hint")),
            )
    }

    /// 받아쓰기의 AI(ADR-0055): 녹음 중 번역 언어, 녹음 중 요약, 모델, 글이 어디로 가는지.
    fn render_dictation_ai(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        let provider = cx.global::<Prefs>().ai_provider;
        let configured = crate::agent::Backend::llm_from_prefs(cx.global::<Prefs>()).is_some();
        let hint = |text: SharedString| div().text_sm().text_color(pal.muted).child(text);
        let section = div().pt_4().flex().flex_col().gap_4().child(
            div()
                .text_lg()
                .font_semibold()
                .child(t!("settings.dictation.ai")),
        );
        if let Some(cli) = provider.and_then(AiProvider::cli) {
            return section.child(hint(
                t!("settings.dictation.ai_hint_agent", name = cli.title()).into(),
            ));
        }
        let Some(provider) = provider.filter(|_| configured) else {
            return section.child(hint(t!("settings.dictation.ai_hint_none").into()));
        };
        let model: AnyElement = match &self.dictation_models {
            DictationModels::Unavailable => div().into_any_element(),
            DictationModels::Loading => {
                hint(t!("settings.dictation.models_loading").into()).into_any_element()
            }
            DictationModels::Failed(err) => div()
                .text_sm()
                .text_color(pal.danger)
                .child(t!("settings.dictation.models_failed", err = err))
                .into_any_element(),
            DictationModels::Ready => Select::new(&self.dictation_model_picker)
                .large()
                .focus_ring(false)
                .menu_max_h(px(240.0))
                .into_any_element(),
        };
        let minutes = crate::config::SUMMARY_MINUTES;
        section
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Checkbox::new("dictation-summarize")
                            .label(t!("settings.dictation.summarize").to_string())
                            .checked(self.dictation.summarize)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.dictation.summarize = *checked;
                                this.save_dictation(cx);
                            })),
                    )
                    .child(hint(
                        t!("settings.dictation.summarize_hint", minutes = minutes).into(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.dictation.summary_language"))
                    .child(
                        Select::new(&self.summary_language_picker)
                            .large()
                            .focus_ring(false)
                            .menu_max_h(px(240.0)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.dictation.ai_model"))
                    .child(model)
                    .child(hint(t!("settings.dictation.ai_model_hint").into())),
            )
            .child(hint(
                t!("settings.dictation.ai_hint", provider = provider.title()).into(),
            ))
    }

    /// 받아쓰기의 엔진(ADR-0056): 실시간 받아쓰기(macOS 받아쓰기·Nemotron)와 받아쓰는 언어.
    fn render_dictation_engine(&self, pal: Palette) -> impl IntoElement {
        let hint = |text: SharedString| div().text_sm().text_color(pal.muted).child(text);
        let engine_hint = match self.dictation.engine {
            dictation::Engine::Apple => t!("settings.dictation.engine.apple_hint"),
            dictation::Engine::Nemotron => t!("settings.dictation.engine.nemotron_hint"),
        };
        div()
            .pb_4()
            .flex()
            .flex_col()
            .gap_4()
            .when(dictation::Engine::available().len() > 1, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(t!("settings.dictation.engine"))
                        .child(Select::new(&self.engine_picker).large().focus_ring(false))
                        .child(hint(engine_hint.into())),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(t!("settings.dictation.language"))
                    .child(
                        Select::new(&self.dictation_language_picker)
                            .large()
                            .focus_ring(false)
                            .menu_max_h(px(240.0)),
                    )
                    .child(hint(t!("settings.dictation.language_hint").into())),
            )
    }

    /// 받아쓰기: 음성 인식 모델 카드(Nemotron, Cohere)와 Hugging Face 로그인 안내.
    fn render_dictation(&self, cx: &mut Context<Self>, pal: Palette) -> impl IntoElement {
        let cohere_ready = self
            .model_store
            .read(cx)
            .downloaded(&dictation::COHERE)
            .is_some();
        let cohere_options = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Checkbox::new("dictation-cohere")
                    .label(t!("settings.dictation.cohere.use").to_string())
                    .checked(self.dictation.cohere)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.dictation.cohere = *checked;
                        this.save_dictation(cx);
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(pal.muted)
                    .child(t!("settings.dictation.cohere.use_hint")),
            );

        let login_hint = if dictation::hf_logged_in() {
            t!("settings.dictation.hint_logged_in")
        } else {
            t!("settings.dictation.hint_login")
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(t!("settings.dictation.model"))
            .child(
                div()
                    .text_sm()
                    .text_color(pal.muted)
                    .child(t!("settings.dictation.model_hint")),
            )
            .child(self.model_card(
                &dictation::NEMOTRON,
                "Nemotron Streaming 3.5",
                t!("settings.dictation.model_description").into(),
                true,
                None,
                cx,
                pal,
            ))
            .child(self.model_card(
                &dictation::COHERE,
                "Cohere Transcribe",
                t!("settings.dictation.cohere.description").into(),
                false,
                // 내려받은 뒤에 켤 수 있다.
                cohere_ready.then(|| cohere_options.into_any_element()),
                cx,
                pal,
            ))
            .child(div().text_sm().text_color(pal.muted).child(login_hint))
    }

    /// 음성 인식 모델 카드 하나: 이름·설명, 언어 수·스트리밍, 내려받기·진행률·내려받음, 그 아래 `extra`.
    #[allow(clippy::too_many_arguments)]
    fn model_card(
        &self,
        spec: &'static dictation::ModelSpec,
        name: &'static str,
        description: SharedString,
        streaming: bool,
        extra: Option<AnyElement>,
        cx: &App,
        pal: Palette,
    ) -> impl IntoElement {
        let store = self.model_store.read(cx);
        let downloaded = store.downloaded(spec).cloned();
        let size = crate::library::format_size(spec.size);
        let id = |name: &str| SharedString::from(format!("{name}-{}", spec.file));
        let meta = |icon: AssetIcon, label: SharedString| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Icon::new(icon).small())
                .child(label)
        };

        let action = if let Some(active) = store.active(spec) {
            let percent = active.done as f32 * 100.0 / active.total.max(1) as f32;
            let store = self.model_store.clone();
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(120.0))
                        .child(Progress::new(id("model-progress")).value(percent)),
                )
                .child(div().w(px(36.0)).child(format!("{percent:.0}%")))
                .child(
                    Button::new(id("model-cancel"))
                        .ghost()
                        .small()
                        .label(t!("common.cancel"))
                        .on_click(move |_, _, cx| {
                            store.update(cx, |store, cx| store.cancel(spec, cx))
                        }),
                )
                .into_any_element()
        } else if let Some(file) = downloaded.clone() {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(meta(icons::disk(), size.into()))
                .child(
                    Button::new(id("model-reveal"))
                        .ghost()
                        .small()
                        .icon(icons::reveal())
                        .tooltip(t!("common.reveal_in_finder"))
                        .on_click(move |_, _, cx| cx.reveal_path(&file)),
                )
                .into_any_element()
        } else {
            let store = self.model_store.clone();
            Button::new(id("model-download"))
                .ghost()
                .small()
                .icon(icons::download())
                .label(size)
                .tooltip(t!("settings.dictation.download"))
                .on_click(move |_, _, cx| store.update(cx, |store, cx| store.start(spec, cx)))
                .into_any_element()
        };

        div()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_xl()
            .border_1()
            .border_color(if downloaded.is_some() {
                pal.accent
            } else {
                pal.panel_border
            })
            .bg(pal.panel)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_lg().font_semibold().child(name))
                            .when(downloaded.is_some(), |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .rounded_full()
                                        .bg(pal.accent)
                                        .text_xs()
                                        .text_color(pal.accent_foreground)
                                        .child(Icon::new(icons::downloaded()).xsmall())
                                        .child(t!("settings.dictation.downloaded")),
                                )
                            }),
                    )
                    .child(div().text_sm().text_color(pal.muted).child(description)),
            )
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(pal.panel_border)
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_sm()
                    .text_color(pal.muted)
                    .child(meta(
                        icons::languages(),
                        t!("settings.dictation.languages", count = spec.languages).into(),
                    ))
                    .when(streaming, |this| {
                        this.child(meta(
                            icons::streaming(),
                            t!("settings.dictation.streaming").into(),
                        ))
                    })
                    .child(div().flex_1())
                    .child(action),
            )
            .when_some(store.failure(spec).cloned(), |this, reason| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(pal.danger)
                        .child(t!("settings.dictation.failed", err = reason)),
                )
            })
            .when_some(extra, |this, extra| {
                this.child(
                    div()
                        .pt_3()
                        .border_t_1()
                        .border_color(pal.panel_border)
                        .child(extra),
                )
            })
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = Palette::from_theme(cx.theme());
        let content = match self.page {
            Page::General => self.render_general(cx, pal).into_any_element(),
            Page::Ai => self.render_ai(cx, pal).into_any_element(),
            Page::Dictation => div()
                .flex()
                .flex_col()
                .child(self.render_dictation_engine(pal))
                .child(self.render_dictation(cx, pal))
                .child(self.render_dictation_input(cx, pal))
                .child(self.render_dictation_ai(cx, pal))
                .into_any_element(),
        };

        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.render_page_panel(cx, pal))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    // 위쪽 빈 줄은 창을 끄는 자리(메인 창 툴바 높이와 맞춘다).
                    .child(window_drag_area(
                        div().h(px(TOOLBAR_HEIGHT)).flex_shrink_0(),
                    ))
                    .child(
                        // 폼이 창보다 길면(AI 페이지) 세로로 스크롤한다.
                        div()
                            .id("settings-form")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_8()
                            .pb_8()
                            .flex()
                            .justify_center()
                            // 기본(stretch)이면 폼이 칸 높이로 늘었다 잘려 넘치는 만큼 스크롤되지 않는다.
                            .items_start()
                            .child(div().w_full().max_w(px(FORM_MAX_WIDTH)).child(content)),
                    ),
            );

        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(pal.background)
            .text_color(pal.foreground);

        #[cfg(not(target_os = "macos"))]
        let root = {
            let theme = cx.theme();
            root.child(
                TitleBar::new()
                    .bg(theme.title_bar)
                    .border_color(theme.title_bar_border)
                    .child(div().text_sm().child(t!("settings.title"))),
            )
        };

        root.child(body)
    }
}
