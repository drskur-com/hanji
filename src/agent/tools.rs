//! 어시스턴트가 부르는 파일 도구. 라이브러리 폴더(작업 공간) 안만 읽고 쓴다.
//!
//! - 경로는 라이브러리 루트에서의 상대 경로다. 절대 경로, `..`, 루트 밖을 가리키는 심볼릭 링크는
//!   거부한다.
//! - 숨김 항목(`.hanji/` 등 앱 상태)은 보이지도, 읽지도, 쓰지도 않는다. 그룹 메타(`.hanji.toml`)만은
//!   읽고 `edit_file`로 고칠 수 있다(통째로 쓰지는 못한다).
//! - 새로 쓰기(`write_file`)는 글·설정·스크립트 파일만, 이미 있는 폴더 안에만 한다. 고치기
//!   (`edit_file`)는 있는 UTF-8 파일이면 된다([`super::edit`]). 둘 다 원자적으로 쓴다
//!   ([`crate::library::write_sheet`]).
//! - TOML 파일은 쓰기 전에 문법을, 그룹 메타는 앱이 읽는 모양까지 검사한다. 깨진 글은 쓰지 않는다.
//!
//! 라이브러리에 스킬(`Skills/`)이 있으면 스킬 본문을 읽는 `load_skill`도 준다([`super::skills`]).
//! `run_extension`은 확장(`Extensions/<이름>.lua` 또는 폴더 `Extensions/<이름>/`, ADR-0026·0039)을 실행해 오류와
//! 그린 결과를 알려 준다. 모델이 쓴 확장을 스스로 확인하고 고치게 한다. 확장 파일의 변경은 초안으로 모았다가
//! 답이 끝나면 확장마다(폴더면 그 안의 파일을 함께) 한 번 승인받는다(ADR-0029).
//!
//! 도구 설명과 결과(오류 포함)는 모델이 읽으므로 영어로 쓴다. 화면에 보이는 요약 줄(`summary`)만
//! 사용자 언어다.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use rig_core::completion::ToolDefinition;
use rig_core::message::ToolName;
use serde_json::{Map, Value, json};

use super::diff::{self, DiffLine};
use super::edit::{self, Edit};
use super::skills::{self, Skill};
use crate::library::GROUP_META_FILE;

/// 한 번에 읽을 수 있는 가장 큰 파일.
const MAX_READ_BYTES: u64 = 1024 * 1024;

/// `write_file`로 쓸 수 있는 파일 확장자: 글, 설정, 스킬 스크립트.
const WRITABLE_EXTENSIONS: &[&str] = &[
    "md", "markdown", "txt", "toml", "json", "yaml", "yml", "csv", "py", "js", "mjs", "ts", "sh",
    "rb", "lua",
];

const LIST: &str = "list_files";
const READ: &str = "read_file";
const WRITE: &str = "write_file";
const EDIT: &str = "edit_file";
const CREATE_SHEET: &str = "create_sheet";
const LOAD_SKILL: &str = "load_skill";
const RUN_EXTENSION: &str = "run_extension";

/// 도구가 다룰 수 있는 폴더와 그 안의 스킬.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    skills: Vec<Skill>,
    /// 확장 앞머리(ADR-0026). 시스템 프롬프트에 목록으로 넣는다.
    extensions: Vec<crate::extension::Meta>,
    /// 확장 초안(ADR-0029): 이번 답에서 고친 확장 파일(경로 → 처음 내용과 지금 내용). 디스크에는 쓰지 않고,
    /// 답이 끝나면 [`Workspace::take_drafts`]로 꺼내 확장마다 한 번 승인받는다.
    drafts: Arc<Mutex<BTreeMap<String, PendingChange>>>,
}

/// 파일을 바꾸는 도구의 계획. 사용자가 승인하면 [`Workspace::commit`]으로 쓴다.
#[derive(Clone, Debug)]
pub struct PendingChange {
    /// 바꿀 파일(라이브러리 루트 기준). 새 시트면 그룹 폴더.
    pub path: String,
    /// 지금 내용. 새 파일이면 `None`.
    pub before: Option<String>,
    pub after: String,
    target: Target,
    /// 쓴 뒤 모델에 돌려줄 글.
    done: String,
    /// 화면 요약 줄의 동사 번역 키(`assistant.tool.edited` 등).
    verb: &'static str,
}

#[derive(Clone, Debug)]
enum Target {
    File(PathBuf),
    /// 폴더 확장 하나의 파일들(ADR-0039). 함께 승인하고 함께 쓴다.
    Files(Vec<FileChange>),
    /// `dir`에 새 시트를 만든다. `group`은 루트 기준 그룹 경로.
    NewSheet {
        dir: PathBuf,
        group: String,
    },
}

/// 폴더 확장의 파일 하나의 변경. `after`가 `None`이면 휴지통으로 옮긴다(파일 하나 확장을 폴더로 바꿀 때).
#[derive(Clone, Debug)]
struct FileChange {
    /// 라이브러리 루트 기준 경로.
    path: String,
    file: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

impl PendingChange {
    /// 승인 카드에 보일 변경 내용. 파일이 여럿이면 파일마다 머리줄을 둔다.
    pub fn diff(&self) -> Vec<DiffLine> {
        if let Target::Files(files) = &self.target {
            return files
                .iter()
                .flat_map(|change| {
                    let lines = match (&change.before, &change.after) {
                        (Some(before), Some(after)) => diff::diff(before, after),
                        (None, Some(after)) => diff::added(after),
                        (Some(before), None) => diff::removed(before),
                        (None, None) => Vec::new(),
                    };
                    std::iter::once(diff::file(&change.path)).chain(lines)
                })
                .collect();
        }
        match &self.before {
            Some(before) => diff::diff(before, &self.after),
            None => diff::added(&self.after),
        }
    }

    /// 새 파일(새 시트·새 폴더 확장 포함)을 만드는지.
    pub fn is_new(&self) -> bool {
        match &self.target {
            Target::Files(files) => files.iter().all(|change| change.before.is_none()),
            _ => self.before.is_none(),
        }
    }
}

/// 도구를 부른 계획: 곧바로 끝난 결과, 또는 승인을 기다리는 변경.
#[derive(Debug)]
pub enum Planned {
    Done(ToolOutput),
    Change(PendingChange),
}

/// 도구를 부른 결과.
#[derive(Debug)]
pub struct ToolOutput {
    /// 모델에 돌려줄 글(영어).
    pub text: String,
    /// 화면에 보일 한 줄(예: `읽음 · Projects/소설/1장.md`). 실패해도 까닭은 넣지 않는다(모델용 영어다).
    pub summary: String,
    /// 모델에 함께 보여 줄 그림. `run_extension`이 그린 결과다.
    pub image: Option<Drawing>,
}

/// `run_extension`이 그린 그림과, 검토(ADR-0030)에 넘길 것.
#[derive(Debug)]
pub struct Drawing {
    pub png: Vec<u8>,
    /// 캔버스 크기(캔버스 좌표).
    pub canvas: (f32, f32),
    /// 격자에 넣은 프레임 수.
    pub frames: usize,
    /// 그린 소스(폴더면 모든 파일, [`fingerprint`]). 그 뒤에 초안을 고쳤는지 알아보는 데 쓴다.
    pub source: String,
}

impl Workspace {
    /// `root`의 작업 공간. 스킬과 확장 목록은 지금 읽어 둔다(보낼 때마다 만들므로 늘 최신이다).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let skills = skills::load(&root);
        let extensions = crate::extension::catalog(&root);
        Self {
            root,
            skills,
            extensions,
            drafts: Arc::default(),
        }
    }

    /// 라이브러리의 확장(이름순).
    /// 라이브러리 루트.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn extensions(&self) -> &[crate::extension::Meta] {
        &self.extensions
    }

    /// 쓸 수 있는 스킬(이름순).
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    /// 모델에 알릴 도구 정의.
    pub fn definitions(&self) -> Vec<Arc<ToolDefinition>> {
        let path = |description: &str| json!({ "type": "string", "description": description });
        let load_skill = (!self.skills.is_empty()).then(|| {
            (
                LOAD_SKILL,
                "Read a skill's instructions. Call it before answering when the request matches a skill's description, then follow the instructions.",
                json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "enum": self.skills.iter().map(|skill| skill.name.as_str()).collect::<Vec<_>>(),
                            "description": "Skill name.",
                        },
                    },
                    "required": ["name"],
                }),
            )
        });
        [
            (
                LIST,
                "List the entries of one folder in the library. Folder names end with `/`.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": path("Path relative to the library root. Empty for the root."),
                    },
                }),
            ),
            (
                READ,
                "Read one text file in the library and return its full content.",
                json!({
                    "type": "object",
                    "properties": { "path": path("Path relative to the library root.") },
                    "required": ["path"],
                }),
            ),
            (
                EDIT,
                "Change part of an existing file in the library (a sheet, a settings TOML, a skill, a script). \
                 For each change give old (a snippet of the current text, exactly as it is) and new (the replacement). \
                 old must occur exactly once in the file, so include enough surrounding text. Pass several changes \
                 in one call. If any change fails, nothing is changed.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": path("Path relative to the library root."),
                        "edits": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old": { "type": "string", "description": "The text to replace, exactly as in the file, including line breaks and spaces." },
                                    "new": { "type": "string", "description": "The replacement text. Empty to delete." },
                                    "all": { "type": "boolean", "description": "Replace every occurrence of old. Default false." },
                                },
                                "required": ["old", "new"],
                            },
                        },
                    },
                    "required": ["path", "edits"],
                }),
            ),
            (
                CREATE_SHEET,
                "Create a new sheet in a notes group or a project (manuscript) group. The app names the file \
                 (<ULID>.md) and, in manually ordered groups, appends it to the end of the sheet order. Returns \
                 the new sheet's path. A sheet's title is its first line, so content usually starts with a title \
                 line (# Title).",
                json!({
                    "type": "object",
                    "properties": {
                        "group": path("Group folder to put the sheet in, relative to the library root. For example Projects/Novel, Notes/Inbox."),
                        "content": { "type": "string", "description": "Sheet content (Markdown). Empty for a blank sheet." },
                    },
                    "required": ["group"],
                }),
            ),
            (
                RUN_EXTENSION,
                "Test an extension (Extensions/<name>.lua, or the folder Extensions/<name>/ with init.lua and its modules), \
                 using your drafts. Without command, calls draw(g, t, args) and reports \
                 errors, the number of shapes, and the bounds of what was drawn compared with the canvas, and attaches \
                 a picture of the canvas so you can see the drawing. Pass times to see several animation frames in one \
                 picture. For an extension with value(args, ctx) (inline {{name ...}} text), also reports the text it returns. \
                 Lists the regions (g.region) it declared. Pass hover to draw with the pointer over a region, \
                 or click to call click(id, args) on a region and draw with the arguments it returns. With command, \
                 runs that command (1-based index or title) with the given selection and returns its result. Nothing \
                 in the library changes.",
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Extension name (the file name without .lua, or the folder name)." },
                        "t": { "type": "number", "description": "Time in seconds for draw. Default 0." },
                        "times": {
                            "type": "array",
                            "items": { "type": "number" },
                            "maxItems": MAX_FRAMES,
                            "description": "Instead of t: several times in seconds (at most 8), drawn as a grid of frames in one picture.",
                        },
                        "image": { "type": "boolean", "description": "Attach a picture of the drawing. Default true." },
                        "goal": {
                            "type": "string",
                            "description": "What the drawing should look like, in concrete visual terms (subject, pose, composition, \
                                            style, colors). An independent reviewer compares the picture with this and the user's request.",
                        },
                        "args": {
                            "type": "object",
                            "description": "Block arguments for draw, as in the ```ext block's key: value lines.",
                            "additionalProperties": { "type": ["string", "number", "boolean"] },
                        },
                        "hover": { "type": "string", "description": "Region id under the pointer (ui.hover) while drawing." },
                        "click": { "type": "string", "description": "Region id to click: calls click(id, args), then draws with the changed arguments and the pointer over that region." },
                        "command": { "type": ["integer", "string"], "description": "Command to run instead of draw: 1-based index or title." },
                        "selection": { "type": "string", "description": "Selected text passed to the command as ctx.selection." },
                    },
                    "required": ["name"],
                }),
            ),
            (
                WRITE,
                "Write one file that is not a sheet (settings, skills, scripts, and so on) in the library with the \
                 given content. An existing file is replaced entirely. The folder must already exist, except inside a folder \
                 extension (Extensions/<name>/...), whose folders are made on approval. Create new sheets with create_sheet, and change part of an existing file with edit_file.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": path("Path relative to the library root."),
                        "content": { "type": "string", "description": "The file's complete new content." },
                    },
                    "required": ["path", "content"],
                }),
            ),
            (super::web::WEB_FETCH, super::web::DESCRIPTION, super::web::parameters()),
            (
                super::model_tool::SET_MODEL,
                super::model_tool::DESCRIPTION,
                super::model_tool::parameters(),
            ),
        ]
        .into_iter()
        .chain(load_skill)
        .filter_map(|(name, description, parameters)| {
            let name = ToolName::new(name).ok()?;
            Some(Arc::new(ToolDefinition::new(name, description, parameters)))
        })
        .collect()
    }

    /// 도구를 승인 없이 부른다(변경도 바로 쓴다).
    #[cfg(test)]
    pub fn call(&self, name: &str, arguments: &Map<String, Value>) -> ToolOutput {
        match self.plan(name, arguments) {
            Planned::Done(output) => output,
            Planned::Change(change) => self.commit(change),
        }
    }

    /// 도구를 부른다. 읽기는 바로 끝내고, 파일을 바꾸는 도구는 쓰지 않고 계획만 돌려준다.
    /// 실패도 모델이 읽고 고칠 수 있게 글로 돌려준다.
    pub fn plan(&self, name: &str, arguments: &Map<String, Value>) -> Planned {
        if name == RUN_EXTENSION {
            let extension = arguments.get("name").and_then(Value::as_str).unwrap_or("");
            let path = self.extension_shown(extension);
            let (result, image) = match self.run_extension(arguments) {
                Ok((report, image)) => (Ok(report), image),
                Err(err) => (Err(err), None),
            };
            return Planned::Done(ToolOutput {
                image,
                ..output(&path, "assistant.tool.ran", result)
            });
        }
        if name == LOAD_SKILL {
            return Planned::Done(
                self.load_skill(arguments.get("name").and_then(Value::as_str).unwrap_or("")),
            );
        }
        let path = arguments
            .get(if name == CREATE_SHEET {
                "group"
            } else {
                "path"
            })
            .and_then(Value::as_str)
            .unwrap_or("");
        let change = match name {
            CREATE_SHEET => {
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                self.plan_sheet(path, content)
            }
            WRITE => match arguments.get("content").and_then(Value::as_str) {
                Some(content) if draft_key(path).is_some() => {
                    self.plan_extension_write(path, content)
                }
                Some(content) => self.plan_write(path, content),
                None => Err("content is missing.".into()),
            },
            EDIT => parse_edits(arguments).and_then(|edits| self.plan_edit(path, &edits)),
            LIST => return Planned::Done(output(path, "assistant.tool.listed", self.list(path))),
            READ => return Planned::Done(output(path, "assistant.tool.read", self.read(path))),
            _ => Err(format!("Unknown tool: {name}")),
        };
        match change {
            Ok(change) => match draft_key(path) {
                Some(key) if name != CREATE_SHEET => Planned::Done(self.save_draft(key, change)),
                _ => Planned::Change(change),
            },
            Err(err) => Planned::Done(output(path, "", Err(err))),
        }
    }

    /// 확장 파일의 변경을 초안에 담는다. 처음 내용은 첫 변경 때의 것을 지킨다.
    fn save_draft(&self, key: String, change: PendingChange) -> ToolOutput {
        let Ok(mut drafts) = self.drafts.lock() else {
            return output(&key, "", Err("The draft could not be saved.".into()));
        };
        let before = drafts
            .get(&key)
            .map_or_else(|| change.before.clone(), |draft| draft.before.clone());
        let text = if before.as_ref() == Some(&change.after) {
            drafts.remove(&key);
            format!(
                "{} The draft now matches the file, so there is nothing to approve.",
                change.done
            )
        } else {
            let done = format!(
                "{} Saved as a draft: read_file and run_extension use it, the sheets do not yet. After your \
                 reply the user reviews each extension's drafts once, as one change, so test and improve freely \
                 without asking.",
                change.done
            );
            drafts.insert(
                key.clone(),
                PendingChange {
                    path: key.clone(),
                    before,
                    ..change
                },
            );
            done
        };
        ToolOutput {
            text,
            summary: format!("{} · {key}", t!("assistant.tool.drafted")),
            image: None,
        }
    }

    /// 승인을 묻기 전에 다시 볼 확장 초안의 문제(ADR-0030). `reviews`는 확장 이름 → (마지막으로 검토한 소스, 점수).
    /// 점수가 없으면(검토하지 못했으면) 통과로 본다.
    pub fn draft_problems(
        &self,
        reviews: &std::collections::HashMap<String, (String, Option<u8>)>,
    ) -> Vec<String> {
        let names: std::collections::BTreeSet<String> = match self.drafts.lock() {
            Ok(drafts) => drafts
                .keys()
                .filter_map(|key| draft_extension(key))
                .map(ToString::to_string)
                .collect(),
            Err(_) => return Vec::new(),
        };
        names
            .iter()
            .filter_map(|name| {
                let key = self.extension_shown(name);
                let source = match self.extension_source(name) {
                    Ok(source) => source,
                    Err(err) => return Some(format!("{key} does not load: {err}")),
                };
                let extension = match crate::extension::Extension::load_package(name, &source) {
                    Ok(extension) => extension,
                    Err(err) => return Some(format!("{key} does not load: {err}")),
                };
                if !extension.draws {
                    return None;
                }
                match reviews.get(name.as_str()) {
                    Some((reviewed, _)) if *reviewed != fingerprint(&source) => Some(format!(
                        "{key} changed after its last run_extension, so nobody has looked at the current picture."
                    )),
                    None => Some(format!(
                        "{key} was never drawn with run_extension, so nobody has looked at the picture."
                    )),
                    Some((_, Some(score))) if *score < super::review::PASS_SCORE => Some(format!(
                        "{key} scored {score}/10 in its last review (passing is {}).",
                        super::review::PASS_SCORE
                    )),
                    Some(_) => None,
                }
            })
            .collect()
    }

    /// 이번 답의 확장 초안을 확장마다 하나의 변경으로 꺼낸다(답이 끝날 때 승인받는다). 폴더 확장은 그 안의
    /// 파일들을 함께 담고, 파일 하나 확장을 폴더로 바꿨으면 옛 파일을 휴지통으로 옮기는 것도 담는다.
    pub fn take_drafts(&self) -> Vec<PendingChange> {
        let drafts = self
            .drafts
            .lock()
            .map(|mut drafts| std::mem::take(&mut *drafts))
            .unwrap_or_default();
        let mut by_extension: BTreeMap<String, Vec<PendingChange>> = BTreeMap::new();
        for (key, draft) in drafts {
            if let Some(name) = draft_extension(&key) {
                by_extension
                    .entry(name.to_string())
                    .or_default()
                    .push(draft);
            }
        }
        by_extension
            .into_iter()
            .map(|(name, mut drafts)| {
                let single = format!("{}/{name}.lua", crate::extension::EXTENSIONS_DIR);
                let folder = drafts.iter().any(|draft| draft.path != single)
                    || crate::extension::package::is_folder(&self.root, &name);
                if !folder {
                    return drafts.remove(0);
                }
                let mut files: Vec<FileChange> = drafts
                    .into_iter()
                    .filter(|draft| draft.path != single)
                    .filter_map(|draft| match draft.target {
                        Target::File(file) => Some(FileChange {
                            path: draft.path,
                            file,
                            before: draft.before,
                            after: Some(draft.after),
                        }),
                        _ => None,
                    })
                    .collect();
                let old = crate::extension::package::single_file(&self.root, &name);
                if let Ok(before) = crate::library::read_sheet(&old) {
                    files.push(FileChange {
                        path: single.clone(),
                        file: old,
                        before: Some(before),
                        after: None,
                    });
                }
                PendingChange {
                    path: format!("{}/{name}/", crate::extension::EXTENSIONS_DIR),
                    before: None,
                    after: String::new(),
                    target: Target::Files(files),
                    done: format!("Wrote the extension folder {name}/."),
                    verb: "assistant.tool.wrote",
                }
            })
            .collect()
    }

    /// 화면과 모델에 보일 확장 자리: 폴더 확장(디스크 또는 초안)이면 `Extensions/<이름>/`.
    pub fn extension_shown(&self, name: &str) -> String {
        let prefix = format!("{}/{name}/", crate::extension::EXTENSIONS_DIR);
        let drafted = self
            .drafts
            .lock()
            .is_ok_and(|drafts| drafts.keys().any(|key| key.starts_with(&prefix)));
        if drafted || crate::extension::package::is_folder(&self.root, name) {
            prefix
        } else {
            format!("{}/{name}.lua", crate::extension::EXTENSIONS_DIR)
        }
    }

    fn draft(&self, path: &str) -> Option<PendingChange> {
        let key = draft_key(path)?;
        self.drafts.lock().ok()?.get(&key).cloned()
    }

    /// 확장 소스: 파일(폴더면 그 안의 파일들) 위에 초안을 덮은 것.
    fn extension_source(&self, name: &str) -> Result<crate::extension::Package, String> {
        use crate::extension::{EXTENSIONS_DIR, Package, package};
        if !crate::extension::valid_name(name) {
            return crate::extension::read_source(&self.root, name);
        }
        let prefix = format!("{EXTENSIONS_DIR}/{name}/");
        let drafted: Vec<(String, String)> = self
            .drafts
            .lock()
            .map(|drafts| {
                drafts
                    .iter()
                    .filter_map(|(key, draft)| {
                        Some((key.strip_prefix(&prefix)?.to_string(), draft.after.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let on_disk = package::is_folder(&self.root, name);
        if drafted.is_empty() && !on_disk {
            return match self.draft(&format!("{EXTENSIONS_DIR}/{name}.lua")) {
                Some(draft) => Ok(draft.after.into()),
                None => crate::extension::read_source(&self.root, name),
            };
        }
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        if on_disk {
            let disk = package::read_folder(&package::folder(&self.root, name))
                .map_err(|err| format!("{prefix}: {err}"))?;
            files.insert(package::ENTRY_FILE.to_string(), disk.main);
            files.extend(disk.modules.into_values());
        }
        files.extend(drafted);
        Package::from_files(
            files
                .iter()
                .map(|(path, source)| (path.as_str(), source.as_str())),
        )
        .map_err(|err| format!("{prefix}: {err}"))
    }

    /// 승인한 변경을 쓴다. 기다리는 동안 파일이 바뀌었으면 쓰지 않는다.
    pub fn commit(&self, change: PendingChange) -> ToolOutput {
        let (path, verb) = (change.path.clone(), change.verb);
        output(&path, verb, self.write_change(change))
    }

    fn write_change(&self, change: PendingChange) -> Result<String, String> {
        match change.target {
            Target::File(file) => {
                let current = match std::fs::symlink_metadata(&file) {
                    Ok(_) => {
                        Some(crate::library::read_sheet(&file).map_err(|err| err.to_string())?)
                    }
                    Err(_) => None,
                };
                if current != change.before {
                    return Err(
                        "The file changed while waiting for approval. Read it again \
                                before retrying."
                            .into(),
                    );
                }
                // 첫 라이브러리 테마면 테마 폴더가 아직 없다(`plan_write`가 허락한 경우만 여기 온다).
                if let Some(parent) = file.parent()
                    && parent.file_name() == Some(std::ffi::OsStr::new(crate::theme::THEMES_DIR))
                    && std::fs::symlink_metadata(parent).is_err()
                {
                    std::fs::create_dir(parent).map_err(|err| err.to_string())?;
                }
                crate::library::write_sheet(&file, &change.after).map_err(|err| err.to_string())?;
                Ok(change.done)
            }
            Target::NewSheet { dir, group } => self.create_sheet(&dir, &group, &change.after),
            Target::Files(files) => {
                // 하나라도 그사이 바뀌었으면 아무것도 쓰지 않는다.
                for file in &files {
                    let current = crate::library::read_sheet(&file.file).ok();
                    if current != file.before {
                        return Err(format!(
                            "{} changed while waiting for approval. Read it again before retrying.",
                            file.path
                        ));
                    }
                }
                for file in &files {
                    match &file.after {
                        Some(after) => {
                            if let Some(parent) = file.file.parent() {
                                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
                            }
                            crate::library::write_sheet(&file.file, after)
                                .map_err(|err| format!("{}: {err}", file.path))?;
                        }
                        None => crate::library::move_to_trash(&file.file)
                            .map_err(|err| format!("{}: {err}", file.path))?,
                    }
                }
                Ok(change.done)
            }
        }
    }

    fn load_skill(&self, name: &str) -> ToolOutput {
        match self.skills.iter().find(|skill| skill.name == name) {
            Some(skill) => ToolOutput {
                text: skills::instructions(skill),
                summary: t!("assistant.tool.skill", name = name).into(),
                image: None,
            },
            None => ToolOutput {
                text: format!("Error: no such skill: {name}"),
                summary: t!("assistant.tool.skill_failed", name = name).into(),
                image: None,
            },
        }
    }

    /// 확장을 시험하고, 성공하면 앞머리에서 빠진 것을 덧붙인다(목록·Insert 대화상자가 앞머리를 쓴다).
    fn run_extension(
        &self,
        arguments: &Map<String, Value>,
    ) -> Result<(String, Option<Drawing>), String> {
        let (report, image) = self.try_extension(arguments)?;
        let name = arguments.get("name").and_then(Value::as_str).unwrap_or("");
        let source = self.extension_source(name).ok();
        let draws = source
            .as_ref()
            .and_then(|source| crate::extension::Extension::load_package(name, source).ok())
            .is_some_and(|ext| ext.draws);
        let meta = source.and_then(|source| crate::extension::parse_meta(name, &source.main));
        let mut notes = Vec::new();
        match &meta {
            None => notes.push(
                "The file has no header. Start it with a --[[--- ... ---]] block holding description \
                 (in the user's language), args, and example; see the extension skill."
                    .to_string(),
            ),
            Some(meta) => {
                if meta.description.trim().is_empty() {
                    notes.push("The header has no description.".to_string());
                }
                let reserved = crate::extension::RESERVED_ARGS;
                for name in reserved {
                    if meta.args.iter().any(|(arg, _)| arg == name) {
                        notes.push(format!(
                            "`{name}` is reserved: the app reads it for every block (scale, align, export) and \
                             does not pass it to the extension. Rename that argument."
                        ));
                    }
                }
                let described = |key: &str| {
                    reserved.contains(&key) || meta.args.iter().any(|(arg, _)| arg == key)
                };
                let undocumented: Vec<&str> = arguments
                    .get("args")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|args| args.keys())
                    .map(String::as_str)
                    .chain(meta.example.iter().map(|(key, _)| key.as_str()))
                    .filter(|key| !described(key))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                if !undocumented.is_empty() {
                    notes.push(format!(
                        "Arguments not described under args in the header: {}.",
                        undocumented.join(", ")
                    ));
                }
                if draws && !meta.args.is_empty() && meta.example.is_empty() {
                    notes.push(
                        "The header has no example (the values used for the preview and when inserting the block)."
                            .to_string(),
                    );
                }
            }
        }
        let report = if notes.is_empty() {
            report
        } else {
            format!("{report}\nNote: {}", notes.join(" "))
        };
        Ok((report, image))
    }

    /// 확장을 실행한 보고와, 그렸으면 그 그림(PNG).
    fn try_extension(
        &self,
        arguments: &Map<String, Value>,
    ) -> Result<(String, Option<Drawing>), String> {
        use crate::extension::{CommandContext, Extension, Theme, Ui};
        let name = arguments.get("name").and_then(Value::as_str).unwrap_or("");
        let source = self.extension_source(name)?;
        let extension = Extension::load_package(name, &source)?;
        let commands = if extension.commands.is_empty() {
            "none".to_string()
        } else {
            extension
                .commands
                .iter()
                .enumerate()
                .map(|(ix, title)| format!("{}. {title}", ix + 1))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(command) = arguments.get("command") {
            let index = match command {
                Value::Number(n) => n.as_u64().and_then(|n| (n as usize).checked_sub(1)),
                Value::String(title) => extension.commands.iter().position(|t| t == title),
                _ => None,
            }
            .filter(|ix| *ix < extension.commands.len())
            .ok_or(format!("No such command. Commands: {commands}"))?;
            let ctx = CommandContext {
                selection: arguments
                    .get("selection")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                ..CommandContext::default()
            };
            let report = match extension.run_command(index, &ctx)? {
                Some(text) => format!("The command returned: {text:?}"),
                None => "The command returned nil (nothing is inserted).".into(),
            };
            return Ok((report, None));
        }
        let times: Vec<f32> = match arguments.get("times").and_then(Value::as_array) {
            Some(times) => times
                .iter()
                .map(|t| t.as_f64().map(|t| t as f32).ok_or("times must be numbers."))
                .collect::<Result<_, _>>()?,
            None => vec![arguments.get("t").and_then(Value::as_f64).unwrap_or(0.0) as f32],
        };
        if times.is_empty() || times.len() > MAX_FRAMES {
            return Err(format!("times must hold 1 to {MAX_FRAMES} numbers."));
        }
        let args: Vec<(String, String)> = arguments
            .get("args")
            .and_then(Value::as_object)
            .map(|args| {
                args.iter()
                    .map(|(key, value)| {
                        let value = match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        (key.clone(), value)
                    })
                    .collect()
            })
            .unwrap_or_default();
        // 배율은 앱이 적용하고 확장에는 넘기지 않는다.
        let mut args = crate::extension::without_reserved(&args);
        let mut ui = Ui {
            hover: arguments
                .get("hover")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            pressed: None,
        };
        // 누르면 돌려준 인자로 바꿔 그린다(에디터가 블록 본문에 쓰는 것과 같다).
        let mut clicked = String::new();
        if let Some(region) = arguments.get("click").and_then(Value::as_str) {
            let result = extension.click(region, &args)?;
            clicked = if !extension.clicks {
                " The extension has no click function, so clicking changes nothing.".to_string()
            } else if result.is_empty() {
                format!(" click(\"{region}\") returned nothing to change.")
            } else {
                let mut parts = Vec::new();
                if !result.args.is_empty() {
                    let set: Vec<String> = result
                        .args
                        .iter()
                        .map(|(key, value)| format!("{key}: {value}"))
                        .collect();
                    parts.push(format!(
                        "set {}; drawn with the changed arguments",
                        set.join(", ")
                    ));
                }
                if !result.actions.is_empty() {
                    let actions: Vec<String> = result
                        .actions
                        .iter()
                        .map(|action| action.describe())
                        .collect();
                    parts.push(format!(
                        "returned actions (not run here; the app runs them in the editor): {}",
                        actions.join("; ")
                    ));
                }
                format!(" click(\"{region}\") {}.", parts.join("; it "))
            };
            for (key, value) in result.args {
                match args.iter_mut().rev().find(|(name, _)| *name == key) {
                    Some(arg) => arg.1 = value,
                    None => args.push((key, value)),
                }
            }
            ui.hover = Some(region.to_string());
        }
        let (width, height) = extension.size_for(&args)?;
        let header = format!(
            "Canvas {width} x {height}, animated: {}, commands: {commands}.",
            extension.animated
        );
        // 인라인 확장의 글(ADR-0036).
        let value = if extension.values {
            match extension.value(&args, "Notes/Example.md") {
                Ok(Some(value)) => format!(
                    " value(args, ctx) returned {value:?} (shown inline as {{{{{name} ...}}}})."
                ),
                Ok(None) => String::new(),
                Err(err) => format!(" value(args, ctx) failed: {err}"),
            }
        } else {
            String::new()
        };
        if !extension.draws {
            return Ok((format!("{header}{value} No draw function."), None));
        }
        let theme = Theme::default();
        let mut frames = Vec::with_capacity(times.len());
        let mut regions = Vec::new();
        for &t in &times {
            let (shapes, drawn) = extension.draw_ui(t, &args, &theme, &ui)?;
            if frames.is_empty() {
                regions = drawn;
            }
            frames.push((t, shapes));
        }
        let lines: Vec<String> = frames
            .iter()
            .map(|(t, shapes)| describe(*t, shapes, width, height))
            .collect();
        let mut report = format!("{header}{value}{clicked} {}", lines.join(" "));
        report.push_str(&describe_regions(&regions, ui.hover.as_deref()));
        let image = if arguments.get("image").and_then(Value::as_bool) == Some(false) {
            None
        } else {
            let frames: Vec<_> = frames
                .iter()
                .map(|(t, shapes)| crate::extension::raster::Frame {
                    label: format!("t = {t}"),
                    shapes,
                })
                .collect();
            match crate::extension::raster::render_png(&frames, width, height, theme.background) {
                Ok(png) => {
                    report.push_str(if frames.len() > 1 {
                        " The attached image shows the frames in order, each labeled with its t, on the light theme. \
                         Text uses a fallback font, so its width can differ slightly from the editor."
                    } else {
                        " The attached image shows the canvas on the light theme. Text uses a fallback font, so its \
                         width can differ slightly from the editor."
                    });
                    Some(Drawing {
                        png,
                        canvas: (width, height),
                        frames: frames.len(),
                        source: fingerprint(&source),
                    })
                }
                Err(err) => {
                    report.push_str(&format!(" The picture could not be rendered: {err}"));
                    None
                }
            }
        };
        Ok((report, image))
    }

    fn list(&self, path: &str) -> Result<String, String> {
        let relative = relative_path(path)?;
        // 아직 파일이 없는 확장 초안(폴더 확장의 새 폴더 포함)도 보인다.
        let drafted: Vec<String> = self
            .drafts
            .lock()
            .map(|drafts| {
                drafts
                    .keys()
                    .filter_map(|key| {
                        let rest = Path::new(key).strip_prefix(&relative).ok()?;
                        let mut parts = rest.components();
                        let first = parts.next()?.as_os_str().to_str()?.to_string();
                        Some(if parts.next().is_some() {
                            format!("{first}/")
                        } else {
                            first
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let dir = match self.resolve_existing(path) {
            Ok(dir) => dir,
            Err(_) if !drafted.is_empty() => {
                let mut entries = drafted;
                entries.sort();
                entries.dedup();
                return Ok(entries.join("\n"));
            }
            Err(err) => return Err(err),
        };
        if !dir.is_dir() {
            return Err("Not a folder.".into());
        }
        let mut entries: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|err| err.to_string())?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') && name != GROUP_META_FILE {
                    return None;
                }
                let is_dir = entry.file_type().ok()?.is_dir();
                Some(if is_dir { format!("{name}/") } else { name })
            })
            .collect();
        for name in drafted {
            if !entries.contains(&name) {
                entries.push(name);
            }
        }
        entries.sort();
        Ok(if entries.is_empty() {
            "(empty)".into()
        } else {
            entries.join("\n")
        })
    }

    fn read(&self, path: &str) -> Result<String, String> {
        if let Some(draft) = self.draft(path) {
            return Ok(draft.after);
        }
        let file = self.resolve_existing(path)?;
        let meta = std::fs::metadata(&file).map_err(|err| err.to_string())?;
        if !meta.is_file() {
            return Err("Not a file.".into());
        }
        if meta.len() > MAX_READ_BYTES {
            return Err(format!(
                "File too large ({} bytes, limit {MAX_READ_BYTES} bytes).",
                meta.len()
            ));
        }
        crate::library::read_sheet(&file).map_err(|err| match err.kind() {
            std::io::ErrorKind::InvalidData => "Not a text file (not UTF-8).".into(),
            _ => err.to_string(),
        })
    }

    fn plan_write(&self, path: &str, content: &str) -> Result<PendingChange, String> {
        let relative = relative_path(path)?;
        let name = relative.file_name().ok_or("The path has no file name.")?;
        let writable = Path::new(name)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| WRITABLE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
        if name == GROUP_META_FILE {
            return Err(
                "Group settings cannot be written whole. Change them with edit_file.".into(),
            );
        }
        if !writable {
            return Err(format!(
                "This file type cannot be written (allowed: {}).",
                WRITABLE_EXTENSIONS.join(", ")
            ));
        }
        check_content(&relative, content)?;
        let parent = relative
            .parent()
            .map_or("", |parent| parent.to_str().unwrap_or(""));
        // 테마 폴더는 첫 테마를 승인할 때 만든다(ADR-0047).
        let dir = match self.resolve_existing(parent) {
            Err(_) if parent == crate::theme::THEMES_DIR => {
                let themes = self.resolve_existing("")?.join(parent);
                if std::fs::symlink_metadata(&themes).is_ok() {
                    return Err("Cannot write through a symbolic link.".into());
                }
                themes
            }
            dir => dir?,
        };
        if !dir.is_dir() && parent != crate::theme::THEMES_DIR {
            return Err("The parent folder does not exist.".into());
        }
        let file = dir.join(name);
        // 새 시트는 이름 규칙(<ULID>.md)과 순서를 지키도록 create_sheet로만 만든다.
        if !file.exists() && is_markdown(name) && self.holds_sheets(parent) {
            return Err("Create new sheets with create_sheet.".into());
        }
        // 있는 파일이 심볼릭 링크면 루트 밖을 가리킬 수 있다.
        if file.is_symlink() {
            return Err("Cannot write to a symbolic link.".into());
        }
        if file.is_dir() {
            return Err("The path is a folder.".into());
        }
        let before = if file.exists() {
            Some(
                crate::library::read_sheet(&file)
                    .map_err(|_| "The existing file is not a text file (not UTF-8).".to_string())?,
            )
        } else {
            None
        };
        Ok(PendingChange {
            path: shown(path),
            before,
            after: content.to_string(),
            target: Target::File(file),
            done: format!("Wrote {} characters.", content.chars().count()),
            verb: "assistant.tool.wrote",
        })
    }

    /// 확장 파일 쓰기(초안이 된다). 폴더 확장의 파일은 폴더가 아직 없어도 된다(승인하면 만든다).
    fn plan_extension_write(&self, path: &str, content: &str) -> Result<PendingChange, String> {
        use crate::extension::{EXTENSIONS_DIR, package};
        let relative = relative_path(path)?;
        check_content(&relative, content)?;
        let key = draft_key(path).ok_or("Not an extension file.")?;
        let name = draft_extension(&key).ok_or("Not an extension file.")?;
        let in_folder = key.starts_with(&format!("{EXTENSIONS_DIR}/{name}/"));
        if !in_folder && package::folder(&self.root, name).is_dir() {
            return Err(format!(
                "{name} is a folder extension: write {EXTENSIONS_DIR}/{name}/{} and its modules instead.",
                package::ENTRY_FILE
            ));
        }
        if !self.root.join(EXTENSIONS_DIR).is_dir() {
            return Err(format!("The {EXTENSIONS_DIR} folder does not exist."));
        }
        // 길 위의 심볼릭 링크는 루트 밖을 가리킬 수 있다.
        let mut at = self.root.clone();
        for part in relative.components() {
            at.push(part);
            if at.is_symlink() {
                return Err("Cannot write through a symbolic link.".into());
            }
        }
        let file = self.root.join(&relative);
        if file.is_dir() {
            return Err("The path is a folder.".into());
        }
        let before = match std::fs::symlink_metadata(&file) {
            Ok(_) => Some(
                crate::library::read_sheet(&file)
                    .map_err(|_| "The existing file is not a text file (not UTF-8).".to_string())?,
            ),
            Err(_) => None,
        };
        let mut done = format!("Wrote {} characters.", content.chars().count());
        if in_folder && package::single_file(&self.root, name).is_file() {
            done.push_str(&format!(
                " When the user approves, the folder replaces {EXTENSIONS_DIR}/{name}.lua, which moves to the trash: \
                 put everything the extension needs into the folder."
            ));
        }
        Ok(PendingChange {
            path: shown(path),
            before,
            after: content.to_string(),
            target: Target::File(file),
            done,
            verb: "assistant.tool.wrote",
        })
    }

    fn plan_sheet(&self, group: &str, content: &str) -> Result<PendingChange, String> {
        let dir = self.resolve_existing(group)?;
        if !dir.is_dir() {
            return Err("Not a group folder.".into());
        }
        if !self.holds_sheets(group) {
            return Err(
                "Sheets cannot be created in this folder. Choose a sheet group under Notes/ or Projects/."
                    .into(),
            );
        }
        Ok(PendingChange {
            path: shown(group),
            before: None,
            after: content.to_string(),
            target: Target::NewSheet {
                dir,
                group: group.to_string(),
            },
            done: String::new(),
            verb: "assistant.tool.created",
        })
    }

    fn create_sheet(&self, dir: &Path, group: &str, content: &str) -> Result<String, String> {
        let mut library = crate::library::scan(&self.root).map_err(|err| err.to_string())?;
        // 스캔한 경로는 정규화하지 않은 루트 기준이다.
        let group_dir = self.root.join(relative_path(group)?);
        let path = crate::library::create_sheet(dir).map_err(|err| err.to_string())?;
        if !content.is_empty()
            && let Err(err) = crate::library::write_sheet(&path, content)
        {
            std::fs::remove_file(&path).ok();
            return Err(err.to_string());
        }
        let manual = library.sort_of(&group_dir).0 == crate::library::SortMode::Manual;
        let mut note = String::new();
        if manual && let Some(group) = library.find_group_mut(&group_dir) {
            let sheet = group_dir.join(path.file_name().unwrap_or_default());
            if let Err(err) = crate::library::append_sheet(group, &sheet) {
                note = format!(" (could not save the sheet order: {err})");
            }
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let group = group.trim().trim_matches('/');
        Ok(format!("Created: {group}/{name}{note}"))
    }

    /// `path`(상대 경로)가 시트를 담는 폴더인지: `Notes/`·`Projects/` 아래이고 파일 그룹이 아니다.
    fn holds_sheets(&self, path: &str) -> bool {
        let Ok(relative) = relative_path(path) else {
            return false;
        };
        let in_section = relative.components().next().is_some_and(|part| {
            [
                crate::library::SectionKind::Notes,
                crate::library::SectionKind::Projects,
            ]
            .iter()
            .any(|kind| part.as_os_str() == kind.dir())
        });
        if !in_section {
            return false;
        }
        let kind = std::fs::read_to_string(self.root.join(&relative).join(GROUP_META_FILE))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|table| {
                table
                    .get("kind")
                    .and_then(|kind| kind.as_str().map(str::to_string))
            });
        kind.as_deref() != Some(crate::library::GroupKind::Files.key())
    }

    fn plan_edit(&self, path: &str, edits: &[Edit]) -> Result<PendingChange, String> {
        let (file, text) = match self.draft(path) {
            Some(PendingChange {
                target: Target::File(file),
                after,
                ..
            }) => (file, after),
            _ => (self.resolve_existing(path)?, self.read(path)?),
        };
        let edited = edit::apply(&text, edits)?;
        check_content(&file, &edited.text)?;
        let places: Vec<String> = edited
            .places
            .iter()
            .map(|(line, count)| match count {
                1 => format!("line {line}"),
                _ => format!("{count} places from line {line}"),
            })
            .collect();
        Ok(PendingChange {
            path: shown(path),
            before: Some(text),
            after: edited.text,
            target: Target::File(file),
            done: format!("Changed: {}", places.join(", ")),
            verb: "assistant.tool.edited",
        })
    }

    /// 있는 항목의 실제 경로. 루트 밖이거나 숨김 항목이면 거부한다.
    fn resolve_existing(&self, path: &str) -> Result<PathBuf, String> {
        let relative = relative_path(path)?;
        let root = self
            .root
            .canonicalize()
            .map_err(|err| format!("Cannot open the library folder: {err}"))?;
        let resolved = root
            .join(relative)
            .canonicalize()
            .map_err(|_| "No such path.".to_string())?;
        let inside = resolved
            .strip_prefix(&root)
            .map_err(|_| OUTSIDE.to_string())?;
        if is_hidden_path(inside) {
            return Err(HIDDEN.into());
        }
        Ok(resolved)
    }
}

/// 초안으로 고치는 확장 파일이면 그 경로(초안의 열쇠): `Extensions/<이름>.lua`, 또는 폴더 확장의
/// `Extensions/<이름>/init.lua`와 모듈(`Extensions/<이름>/ui/hud.lua`, ADR-0039).
fn draft_key(path: &str) -> Option<String> {
    let relative = relative_path(path).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let (dir, rest) = parts.split_first()?;
    if *dir != crate::extension::EXTENSIONS_DIR {
        return None;
    }
    match rest {
        [file] => {
            let name = file.strip_suffix(".lua")?;
            crate::extension::valid_name(name).then(|| format!("{dir}/{file}"))
        }
        [name, inner @ ..] if !inner.is_empty() => {
            let inner = inner.join("/");
            let module = inner == crate::extension::package::ENTRY_FILE
                || crate::extension::package::module_name(&inner).is_some();
            (crate::extension::valid_name(name) && module).then(|| format!("{dir}/{name}/{inner}"))
        }
        _ => None,
    }
}

/// 초안 열쇠의 확장 이름.
fn draft_extension(key: &str) -> Option<&str> {
    let rest = key
        .strip_prefix(crate::extension::EXTENSIONS_DIR)?
        .strip_prefix('/')?;
    match rest.split_once('/') {
        Some((name, _)) => Some(name),
        None => rest.strip_suffix(".lua"),
    }
}

/// 확장 소스 전체를 비교할 글(검토한 뒤 고쳤는지 알아본다).
fn fingerprint(source: &crate::extension::Package) -> String {
    let mut text = source.main.clone();
    for (path, module) in source.modules.values() {
        text.push_str(&format!("\n\u{0}{path}\u{0}\n{module}"));
    }
    text
}

/// 화면과 모델에 보일 경로. 비면 루트.
fn shown(path: &str) -> String {
    let path = path.trim().trim_matches('/');
    if path.is_empty() {
        "/".into()
    } else {
        path.into()
    }
}

/// 도구 결과. `verb`는 화면 요약 줄 동사의 번역 키다. 실패해도 요약 줄에는 까닭을 넣지 않는다(모델용 영어다).
/// `run_extension`이 한 번에 그려 보는 프레임 수의 상한(이미지 한 장에 격자로 넣는다).
const MAX_FRAMES: usize = 8;

/// 그린 결과 한 프레임의 요약: 모양 수와 범위, 캔버스를 벗어나는지.
/// `g.region`으로 둔 영역(첫 프레임). `hover`가 영역에 없으면 알린다.
fn describe_regions(regions: &[crate::extension::Region], hover: Option<&str>) -> String {
    const SHOWN: usize = 20;
    let mut text = if regions.is_empty() {
        " No regions (g.region), so the block does not react to the mouse.".to_string()
    } else {
        let listed: Vec<String> = regions
            .iter()
            .take(SHOWN)
            .map(|region| {
                let [x, y, w, h] = region.rect.map(|value| value.round());
                format!("{} [{x}, {y}, {w}, {h}]", region.id)
            })
            .collect();
        let more = regions.len().saturating_sub(SHOWN);
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        format!(" Regions [x, y, w, h]: {}{more}.", listed.join(", "))
    };
    if let Some(hover) = hover
        && !regions.iter().any(|region| region.id == hover)
    {
        text.push_str(&format!(
            " Note: no region is named \"{hover}\", so nothing is under the pointer."
        ));
    }
    text
}

fn describe(t: f32, shapes: &[crate::extension::Shape], width: f32, height: f32) -> String {
    use crate::extension::{Paint, Segment, Shape};
    let (mut fills, mut strokes, mut texts, mut gradients, mut images) = (0, 0, 0, 0, 0);
    let mut scenes = 0;
    let mut bounds: Option<[f32; 4]> = None;
    let mut extend = |[x, y]: [f32; 2]| {
        let b = bounds.get_or_insert([x, y, x, y]);
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    };
    for shape in shapes {
        match shape {
            Shape::Fill { path, paint, .. } | Shape::Stroke { path, paint, .. } => {
                if !matches!(paint, Paint::Solid(_)) {
                    gradients += 1;
                }
                if matches!(shape, Shape::Fill { .. }) {
                    fills += 1;
                } else {
                    strokes += 1;
                }
                for segment in path {
                    match *segment {
                        Segment::Move(p) | Segment::Line(p) => extend(p),
                        Segment::Cubic(a, b, p) => {
                            extend(a);
                            extend(b);
                            extend(p);
                        }
                        Segment::Close => {}
                    }
                }
            }
            Shape::Text { origin, .. } => {
                texts += 1;
                extend(*origin);
            }
            Shape::Image { src, transform, .. } => {
                images += 1;
                let [a, b, c, d, e, f] = *transform;
                let [x, y, w, h] = *src;
                for [px, py] in [[x, y], [x + w, y], [x, y + h], [x + w, y + h]] {
                    extend([a * px + c * py + e, b * px + d * py + f]);
                }
            }
            Shape::Scene3D(mesh) => {
                scenes += 1;
                let [x, y, w, h] = mesh.bounds;
                extend([x, y]);
                extend([x + w, y + h]);
            }
            Shape::Clip(_) | Shape::ClipPath(_) | Shape::Effects(_) => {}
        }
    }
    let Some([x0, y0, x1, y1]) = bounds else {
        return format!("At t = {t}: drew nothing.");
    };
    let outside = x0 < 0.0 || y0 < 0.0 || x1 > width || y1 > height;
    format!(
        "At t = {t}: {} shapes ({fills} fills, {strokes} strokes, {texts} texts, {images} images, {scenes} 3D scenes, {gradients} with gradients). \
         Drawing bounds x {x0:.1}..{x1:.1}, y {y0:.1}..{y1:.1} (text counts its top-left corner only).{}",
        fills + strokes + texts + images + scenes,
        if outside {
            " Part of the drawing is outside the canvas and will be cut off."
        } else {
            ""
        }
    )
}

fn output(path: &str, verb: &str, result: Result<String, String>) -> ToolOutput {
    let path = shown(path);
    match result {
        Ok(text) => ToolOutput {
            text,
            summary: format!("{} · {path}", t!(verb)),
            image: None,
        },
        Err(err) => ToolOutput {
            text: format!("Error: {err}"),
            summary: t!("assistant.tool.failed", path = path).into(),
            image: None,
        },
    }
}

fn is_markdown(name: &std::ffi::OsStr) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

/// `edit_file`의 `edits` 인자.
fn parse_edits(arguments: &Map<String, Value>) -> Result<Vec<Edit>, String> {
    let Some(items) = arguments.get("edits").and_then(Value::as_array) else {
        return Err("edits is missing (it must be an array).".into());
    };
    items
        .iter()
        .enumerate()
        .map(|(ix, item)| {
            let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
            Ok(Edit {
                old: text("old").ok_or(format!("Edit {} has no old.", ix + 1))?,
                new: text("new").ok_or(format!("Edit {} has no new.", ix + 1))?,
                all: item.get("all").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// 쓰기 전에 설정 파일이 깨지지 않았는지 본다.
fn check_content(path: &Path, content: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name == GROUP_META_FILE {
        return crate::library::validate_group_meta(content)
            .map_err(|err| format!("Not written. {err}"));
    }
    if is_theme_file(path) {
        return crate::theme::validate_library_file(content)
            .map_err(|err| format!("Not written. Theme file error: {err}"));
    }
    let is_toml = Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if is_toml {
        content
            .parse::<toml::Table>()
            .map_err(|err| format!("Not written. TOML syntax error: {err}"))?;
    }
    Ok(())
}

/// 라이브러리 테마 파일(`Themes/<이름>.json`)인지.
fn is_theme_file(path: &Path) -> bool {
    path.parent() == Some(Path::new(crate::theme::THEMES_DIR))
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

const OUTSIDE: &str = "Paths outside the library folder are not allowed.";
const HIDDEN: &str = "Hidden entries are not allowed.";

/// 모델이 준 경로를 검사한 상대 경로. 앞뒤의 `/`는 무시한다.
fn relative_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path.trim().trim_start_matches('/'));
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(OUTSIDE.into());
            }
        }
    }
    if is_hidden_path(&relative) {
        return Err(HIDDEN.into());
    }
    Ok(relative)
}

/// 숨김 항목이 든 경로인지. 마지막의 그룹 메타(`.hanji.toml`)는 숨김으로 치지 않는다.
fn is_hidden_path(path: &Path) -> bool {
    let parts: Vec<_> = path.components().collect();
    parts.iter().enumerate().any(|(ix, part)| {
        let name = part.as_os_str().to_string_lossy();
        name.starts_with('.') && !(ix + 1 == parts.len() && name == GROUP_META_FILE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;

    fn args(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), Value::from(*value)))
            .collect()
    }

    fn workspace() -> (Workspace, PathBuf) {
        let root = temp_dir("agent-tools");
        std::fs::create_dir_all(root.join(".hanji")).unwrap();
        std::fs::write(root.join(".hanji/library.toml"), "").unwrap();
        std::fs::create_dir_all(root.join("Projects/소설")).unwrap();
        std::fs::write(root.join("Projects/소설/1장.md"), "첫 장.").unwrap();
        std::fs::write(root.join("Projects/소설/.hanji.toml"), "").unwrap();
        (Workspace::new(&root), root)
    }

    #[test]
    fn loads_skills_from_workspace() {
        let (_, root) = workspace();
        std::fs::create_dir_all(root.join("Skills/tone")).unwrap();
        std::fs::write(
            root.join("Skills/tone/SKILL.md"),
            "---\nname: tone\ndescription: 어조\n---\n존댓말로",
        )
        .unwrap();
        let ws = Workspace::new(&root);
        let names: Vec<_> = ws
            .definitions()
            .iter()
            .map(|def| def.name.to_string())
            .collect();
        assert_eq!(
            names,
            [
                LIST,
                READ,
                EDIT,
                CREATE_SHEET,
                RUN_EXTENSION,
                WRITE,
                super::super::web::WEB_FETCH,
                super::super::model_tool::SET_MODEL,
                LOAD_SKILL
            ]
        );
        let out = ws.call(LOAD_SKILL, &args(&[("name", "tone")]));
        assert!(out.text.starts_with("존댓말로"));
        assert_eq!(out.summary, t!("assistant.tool.skill", name = "tone"));
        assert!(
            ws.call(LOAD_SKILL, &args(&[("name", "없음")]))
                .text
                .starts_with("Error:")
        );
        // 스킬이 없으면 load_skill을 주지 않는다.
        let (bare, _) = workspace();
        assert_eq!(bare.definitions().len(), 8);
    }

    #[test]
    fn lists_without_hidden_entries() {
        let (ws, _) = workspace();
        assert_eq!(ws.call(LIST, &args(&[])).text, "Projects/");
        // 그룹 메타만은 보인다.
        assert_eq!(
            ws.call(LIST, &args(&[("path", "Projects/소설")])).text,
            ".hanji.toml\n1장.md"
        );
    }

    #[test]
    fn reads_and_writes_inside_root() {
        let (ws, root) = workspace();
        let read = ws.call(READ, &args(&[("path", "Projects/소설/1장.md")]));
        assert_eq!(read.text, "첫 장.");
        assert_eq!(
            read.summary,
            format!("{} · Projects/소설/1장.md", t!("assistant.tool.read"))
        );

        let wrote = ws.call(
            WRITE,
            &args(&[("path", "/Projects/소설/메모.txt"), ("content", "둘째 장.")]),
        );
        assert_eq!(
            wrote.summary,
            format!("{} · Projects/소설/메모.txt", t!("assistant.tool.wrote"))
        );
        assert_eq!(
            std::fs::read_to_string(root.join("Projects/소설/메모.txt")).unwrap(),
            "둘째 장."
        );
        // 있는 시트는 통째로 쓸 수 있지만, 새 시트는 create_sheet로만 만든다.
        let out = ws.call(
            WRITE,
            &args(&[("path", "Projects/소설/1장.md"), ("content", "새 첫 장.")]),
        );
        assert_eq!(
            out.summary,
            format!("{} · Projects/소설/1장.md", t!("assistant.tool.wrote"))
        );
        let out = ws.call(
            WRITE,
            &args(&[("path", "Projects/소설/2장.md"), ("content", "")]),
        );
        assert_eq!(out.text, "Error: Create new sheets with create_sheet.");
    }

    #[test]
    fn refuses_paths_outside_root_or_hidden() {
        let (ws, _) = workspace();
        for path in [
            "../outside.md",
            "Projects/../../outside.md",
            ".hanji/library.toml",
        ] {
            let out = ws.call(READ, &args(&[("path", path)]));
            assert!(out.text.starts_with("Error:"), "{path}: {}", out.text);
        }
        let out = ws.call(
            WRITE,
            &args(&[("path", "Projects/소설/.hanji.toml"), ("content", "")]),
        );
        assert!(
            out.text
                .starts_with("Error: Group settings cannot be written whole"),
            "{}",
            out.text
        );
        // 다른 숨김 항목은 고칠 수도 없다.
        let out = ws.call(EDIT, &edit_args(".hanji/library.toml", "a", "b"));
        assert_eq!(out.text, format!("Error: {HIDDEN}"));
    }

    fn edit_args(path: &str, old: &str, new: &str) -> Map<String, Value> {
        json!({ "path": path, "edits": [{ "old": old, "new": new }] })
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn edits_sheet_in_place() {
        let (ws, root) = workspace();
        let out = ws.call(EDIT, &edit_args("Projects/소설/1장.md", "첫 장", "처음 장"));
        assert_eq!(out.text, "Changed: line 1");
        assert_eq!(
            out.summary,
            format!("{} · Projects/소설/1장.md", t!("assistant.tool.edited"))
        );
        assert_eq!(
            std::fs::read_to_string(root.join("Projects/소설/1장.md")).unwrap(),
            "처음 장."
        );
        let out = ws.call(EDIT, &edit_args("Projects/소설/1장.md", "없는 글", "x"));
        assert!(out.text.starts_with("Error:"), "{}", out.text);
        let out = ws.call(EDIT, &args(&[("path", "Projects/소설/1장.md")]));
        assert!(out.text.starts_with("Error: edits is missing"));
    }

    #[test]
    fn plans_without_writing_and_refuses_stale_commit() {
        let (ws, root) = workspace();
        let file = root.join("Projects/소설/1장.md");
        let Planned::Change(change) =
            ws.plan(EDIT, &edit_args("Projects/소설/1장.md", "첫 장", "처음 장"))
        else {
            panic!("edit_file must wait for approval");
        };
        assert_eq!(change.path, "Projects/소설/1장.md");
        assert!(!change.is_new());
        assert!(!change.diff().is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "첫 장.");
        // 기다리는 동안 사용자가 고쳤으면 쓰지 않는다.
        std::fs::write(&file, "사용자가 고친 장.").unwrap();
        let out = ws.commit(change);
        assert!(
            out.text.starts_with("Error: The file changed"),
            "{}",
            out.text
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "사용자가 고친 장.");
        // 읽기 도구는 승인 없이 끝난다.
        assert!(matches!(
            ws.plan(READ, &args(&[("path", "Projects/소설/1장.md")])),
            Planned::Done(_)
        ));
        let Planned::Change(change) = ws.plan(
            CREATE_SHEET,
            &args(&[("group", "Projects/소설"), ("content", "# 새")]),
        ) else {
            panic!("create_sheet must wait for approval");
        };
        assert!(change.is_new());
        assert_eq!(
            std::fs::read_dir(root.join("Projects/소설"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn creates_sheet_and_appends_to_manual_order() {
        let (ws, root) = workspace();
        std::fs::write(
            root.join("Projects/소설/.hanji.toml"),
            "sheets = [\"1장.md\"]\n",
        )
        .unwrap();
        let out = ws.call(
            CREATE_SHEET,
            &args(&[("group", "Projects/소설"), ("content", "# 2장\n\n둘째.")]),
        );
        assert_eq!(
            out.summary,
            format!("{} · Projects/소설", t!("assistant.tool.created"))
        );
        let created = out.text.strip_prefix("Created: ").unwrap();
        let name = created.strip_prefix("Projects/소설/").unwrap();
        assert_eq!(name.len(), "01JB0000000000000000000000.md".len());
        assert_eq!(
            std::fs::read_to_string(root.join(created)).unwrap(),
            "# 2장\n\n둘째."
        );
        let meta = std::fs::read_to_string(root.join("Projects/소설/.hanji.toml")).unwrap();
        let order: toml::Table = meta.parse().unwrap();
        let sheets: Vec<_> = order["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sheet| sheet.as_str().unwrap())
            .collect();
        assert_eq!(sheets, ["1장.md", name]);

        // 파일 그룹과 스킬 폴더에는 시트를 만들지 않는다.
        std::fs::create_dir_all(root.join("Projects/소설/Extras/Attachments")).unwrap();
        std::fs::write(
            root.join("Projects/소설/Extras/Attachments/.hanji.toml"),
            "kind = \"files\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("Skills/tone")).unwrap();
        for group in ["Projects/소설/Extras/Attachments", "Skills/tone"] {
            let out = ws.call(CREATE_SHEET, &args(&[("group", group)]));
            assert!(
                out.text.starts_with("Error: Sheets cannot be created"),
                "{group}: {}",
                out.text
            );
        }
    }

    #[test]
    fn edits_group_meta_only_when_app_can_read_it() {
        let (ws, root) = workspace();
        let meta = root.join("Projects/소설/.hanji.toml");
        std::fs::write(&meta, "title = \"소설\"\nsheets = [\"1장.md\"]\n").unwrap();
        let out = ws.call(
            EDIT,
            &edit_args("Projects/소설/.hanji.toml", "\"소설\"", "\"장편\""),
        );
        assert_eq!(out.text, "Changed: line 1");
        // 시트 순서를 배열이 아닌 값으로 바꾸면 앱이 순서를 잃으므로 쓰지 않는다.
        let out = ws.call(
            EDIT,
            &edit_args("Projects/소설/.hanji.toml", "[\"1장.md\"]", "\"1장.md\""),
        );
        assert!(out.text.starts_with("Error: Not written."), "{}", out.text);
        assert!(
            std::fs::read_to_string(&meta)
                .unwrap()
                .contains("[\"1장.md\"]")
        );
    }

    #[test]
    fn writes_skill_scripts_and_checks_toml() {
        let (ws, root) = workspace();
        std::fs::create_dir_all(root.join("Skills/count")).unwrap();
        let out = ws.call(
            WRITE,
            &args(&[("path", "Skills/count/count.py"), ("content", "print(1)\n")]),
        );
        assert_eq!(
            out.summary,
            format!("{} · Skills/count/count.py", t!("assistant.tool.wrote"))
        );
        let out = ws.call(
            WRITE,
            &args(&[("path", "Skills/count/config.toml"), ("content", "a = ")]),
        );
        assert!(out.text.contains("TOML syntax error"), "{}", out.text);
        assert!(!root.join("Skills/count/config.toml").exists());
    }

    #[test]
    fn writes_theme_files_and_makes_the_folder_on_approval() {
        let (ws, root) = workspace();
        let theme = |name: &str| {
            format!(r#"{{"name":"x","themes":[{{"name":"{name}","mode":"dark","colors":{{}}}}]}}"#)
        };
        // 테마 폴더가 없어도 계획은 되고, 승인 전에는 아무것도 만들지 않는다.
        let Planned::Change(change) = ws.plan(
            WRITE,
            &args(&[
                ("path", "Themes/nord.json"),
                ("content", &theme("Nord Dark")),
            ]),
        ) else {
            panic!("테마 쓰기가 계획되지 않았다");
        };
        assert!(!root.join("Themes").exists());
        ws.commit(change);
        assert!(root.join("Themes/nord.json").is_file());

        let out = ws.call(
            WRITE,
            &args(&[
                ("path", "Themes/mine.json"),
                ("content", &theme("Hanji Dark")),
            ]),
        );
        assert!(out.text.contains("built-in theme name"), "{}", out.text);
        let out = ws.call(
            WRITE,
            &args(&[("path", "Themes/broken.json"), ("content", "{")]),
        );
        assert!(out.text.contains("Theme file error"), "{}", out.text);
        assert!(!root.join("Themes/mine.json").exists());
        assert!(!root.join("Themes/broken.json").exists());
    }

    #[test]
    fn runs_extensions_for_the_model() {
        let (ws, root) = workspace();
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        std::fs::write(
            root.join("Extensions/dot.lua"),
            "--[[---\ndescription: 점\nargs:\n  r: 반지름\nexample:\n  r: 5\n---]]\n\
             return { size = { 100, 50 }, draw = function(g, t, args) g.circle('fill', 10 + t, 10, args.r) end,\n\
             commands = { { title = 'Shout', run = function(ctx) return ctx.selection .. '!' end } } }",
        )
        .unwrap();
        let run = |arguments: Value| ws.call(RUN_EXTENSION, arguments.as_object().unwrap());
        let out = run(json!({ "name": "dot", "args": { "r": 5 } }));
        assert_eq!(
            out.summary,
            format!("{} · Extensions/dot.lua", t!("assistant.tool.ran"))
        );
        assert!(out.text.contains("1 shapes (1 fills"), "{}", out.text);
        assert!(!out.text.contains("outside"), "{}", out.text);
        assert!(
            out.image
                .as_ref()
                .is_some_and(|drawing| drawing.png.starts_with(b"\x89PNG"))
        );
        // 여러 시간은 한 그림에 격자로 넣는다.
        let out = run(json!({ "name": "dot", "times": [0, 1, 2], "args": { "r": 5 } }));
        assert_eq!(out.text.matches("1 shapes").count(), 3, "{}", out.text);
        assert!(out.image.is_some());
        assert!(
            run(json!({ "name": "dot", "args": { "r": 5 }, "image": false }))
                .image
                .is_none()
        );
        let out = run(json!({ "name": "dot", "t": 100, "args": { "r": 5 } }));
        assert!(out.text.contains("outside the canvas"), "{}", out.text);
        let out = run(json!({ "name": "dot", "command": "Shout", "selection": "hi" }));
        assert_eq!(out.text, "The command returned: \"hi!\"");
        assert!(out.image.is_none());
        let out = run(json!({ "name": "dot", "command": 2 }));
        assert!(
            out.text.starts_with("Error: No such command"),
            "{}",
            out.text
        );
        // 오류는 모델이 고칠 수 있게 그대로 돌려준다.
        let out = run(json!({ "name": "dot" }));
        assert!(out.text.starts_with("Error: "), "{}", out.text);
        let out = run(json!({ "name": "none" }));
        assert!(out.text.contains("No extension named"), "{}", out.text);
        assert!(
            !run(json!({ "name": "dot", "args": { "r": 5 } }))
                .text
                .contains("Note:")
        );
        // 앞머리에 없는 인자, 앞머리가 없는 확장은 알린다.
        let out = run(json!({ "name": "dot", "args": { "r": 5, "color": "red" } }));
        assert!(
            out.text
                .contains("not described under args in the header: color."),
            "{}",
            out.text
        );
        std::fs::write(
            root.join("Extensions/bare.lua"),
            "return { draw = function(g) g.circle('fill', 1, 1, 1) end }",
        )
        .unwrap();
        let out = run(json!({ "name": "bare" }));
        assert!(
            out.text.contains("Note: The file has no header."),
            "{}",
            out.text
        );
    }

    #[test]
    fn runs_extension_hover_and_click() {
        let (ws, root) = workspace();
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        std::fs::write(
            root.join("Extensions/toggle.lua"),
            "--[[---\ndescription: 켜고 끄기\nargs:\n  on: 켜짐\nexample:\n  on: false\n---]]\n\
             return { size = { 40, 20 }, \
             draw = function(g, t, args, ui) \
               if args.on then g.circle('fill', 30, 10, 5) end \
               if ui.hover == 'switch' then g.rectangle('line', 0, 0, 20, 20) end \
               g.rectangle('fill', 2, 2, 16, 16) g.region('switch', 0, 0, 20, 20) end, \
             click = function(id, args) return { on = not args.on } end }",
        )
        .unwrap();
        let run = |arguments: Value| ws.call(RUN_EXTENSION, arguments.as_object().unwrap());
        let out = run(json!({ "name": "toggle", "args": { "on": false } }));
        assert!(
            out.text
                .contains("Regions [x, y, w, h]: switch [0, 0, 20, 20]."),
            "{}",
            out.text
        );
        assert!(out.text.contains("1 shapes"), "{}", out.text);
        let out = run(json!({ "name": "toggle", "args": { "on": false }, "hover": "switch" }));
        assert!(out.text.contains("2 shapes"), "{}", out.text);
        let out = run(json!({ "name": "toggle", "args": { "on": false }, "click": "switch" }));
        assert!(
            out.text.contains("click(\"switch\") set on: true"),
            "{}",
            out.text
        );
        assert!(out.text.contains("3 shapes"), "{}", out.text);
        let out = run(json!({ "name": "toggle", "hover": "nope" }));
        assert!(
            out.text.contains("no region is named \"nope\""),
            "{}",
            out.text
        );
    }

    #[test]
    fn keeps_extension_changes_as_drafts_until_the_reply_ends() {
        let (ws, root) = workspace();
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        let file = root.join("Extensions/dot.lua");
        let source =
            "return { size = { 10, 10 }, draw = function(g) g.circle('fill', 5, 5, 1) end }";
        std::fs::write(&file, source).unwrap();
        let call = |name: &str, arguments: Value| ws.plan(name, arguments.as_object().unwrap());
        // 고치기는 승인 없이 초안에 담기고, 파일은 그대로다.
        let edit = json!({ "path": "Extensions/dot.lua", "edits": [{ "old": "5, 5, 1", "new": "5, 5, 3" }] });
        let Planned::Done(out) = call(EDIT, edit) else {
            panic!("an extension edit must not wait for approval");
        };
        assert!(out.text.contains("draft"), "{}", out.text);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        // 읽기와 실행은 초안을 쓰고, 초안을 또 고칠 수 있다.
        let Planned::Done(read) = call(READ, json!({ "path": "Extensions/dot.lua" })) else {
            panic!()
        };
        assert!(read.text.contains("5, 5, 3"));
        let edit = json!({ "path": "Extensions/dot.lua", "edits": [{ "old": "5, 5, 3", "new": "5, 5, 4" }] });
        assert!(matches!(call(EDIT, edit), Planned::Done(_)));
        // 새 확장 파일도 초안이고 목록에 보인다.
        let write = json!({ "path": "Extensions/new.lua", "content": "return { commands = { { title = 'X', run = function() return nil end } } }" });
        assert!(matches!(call(WRITE, write), Planned::Done(_)));
        let Planned::Done(list) = call(LIST, json!({ "path": "Extensions" })) else {
            panic!()
        };
        assert_eq!(list.text, "dot.lua\nnew.lua");
        // 다른 파일은 지금처럼 승인을 기다린다.
        let other = json!({ "path": "Extensions/notes.txt", "content": "x" });
        assert!(matches!(call(WRITE, other), Planned::Change(_)));
        // 그리는 확장은 지금 초안을 검토해 통과해야 문제가 없다(ADR-0030). 그리지 않는 확장은 보지 않는다.
        let mut reviews = std::collections::HashMap::new();
        let problems = ws.draft_problems(&reviews);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("never drawn"), "{problems:?}");
        let draft = ws.draft("Extensions/dot.lua").unwrap().after;
        reviews.insert("dot".to_string(), (draft.clone(), Some(4)));
        assert!(ws.draft_problems(&reviews)[0].contains("scored 4/10"));
        reviews.insert("dot".to_string(), ("old".to_string(), Some(9)));
        assert!(ws.draft_problems(&reviews)[0].contains("changed after"));
        reviews.insert("dot".to_string(), (draft, Some(8)));
        assert!(ws.draft_problems(&reviews).is_empty());
        // 답이 끝나면 확장마다 하나의 변경(처음 내용 → 마지막 내용)으로 꺼낸다.
        let drafts = ws.take_drafts();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].before.as_deref(), Some(source));
        assert!(drafts[0].after.contains("5, 5, 4"));
        assert!(drafts[1].is_new());
        assert!(ws.take_drafts().is_empty());
        let out = ws.commit(drafts.into_iter().next().unwrap());
        assert!(!out.text.starts_with("Error"), "{}", out.text);
        assert!(std::fs::read_to_string(&file).unwrap().contains("5, 5, 4"));
    }

    #[test]
    fn drafts_folder_extensions_and_approves_them_together() {
        let (ws, root) = workspace();
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        // 파일 하나 확장을 폴더로 바꾼다.
        let old = "return { size = { 10, 10 }, draw = function(g) end }";
        std::fs::write(root.join("Extensions/game.lua"), old).unwrap();
        let call = |name: &str, arguments: Value| ws.plan(name, arguments.as_object().unwrap());
        let init = "local hud = require('ui.hud')\nreturn { size = { 10, 10 }, draw = function(g) hud.draw(g) end }";
        let hud = "return { draw = function(g) g.circle('fill', 5, 5, 2) end }";
        let Planned::Done(out) = call(
            WRITE,
            json!({ "path": "Extensions/game/init.lua", "content": init }),
        ) else {
            panic!("a folder extension file must be a draft");
        };
        assert!(out.text.contains("moves to the trash"), "{}", out.text);
        assert!(matches!(
            call(
                WRITE,
                json!({ "path": "Extensions/game/ui/hud.lua", "content": hud })
            ),
            Planned::Done(_)
        ));
        // 모듈 이름이 될 수 없는 경로는 초안이 아니다(보통 쓰기처럼 폴더가 있어야 한다).
        assert!(draft_key("Extensions/game/my hud.lua").is_none());
        // 목록·실행은 아직 없는 폴더도 초안으로 본다.
        let Planned::Done(list) = call(LIST, json!({ "path": "Extensions/game" })) else {
            panic!()
        };
        assert_eq!(list.text, "init.lua\nui/");
        assert_eq!(ws.extension_shown("game"), "Extensions/game/");
        let Planned::Done(run) = call(RUN_EXTENSION, json!({ "name": "game" })) else {
            panic!()
        };
        assert!(!run.text.starts_with("Error"), "{}", run.text);
        assert!(run.summary.ends_with("Extensions/game"), "{}", run.summary);
        // 확장 하나의 승인 카드: 두 파일과, 휴지통으로 갈 옛 파일.
        let drafts = ws.take_drafts();
        assert_eq!(drafts.len(), 1);
        let headers: Vec<String> = drafts[0]
            .diff()
            .into_iter()
            .filter(|line| line.kind == diff::LineKind::File)
            .map(|line| line.text)
            .collect();
        assert_eq!(
            headers,
            [
                "Extensions/game/init.lua",
                "Extensions/game/ui/hud.lua",
                "Extensions/game.lua"
            ]
        );
        assert!(!drafts[0].is_new());
    }

    #[test]
    fn commits_a_new_folder_extension() {
        let (ws, root) = workspace();
        std::fs::create_dir_all(root.join("Extensions")).unwrap();
        let call = |name: &str, arguments: Value| ws.plan(name, arguments.as_object().unwrap());
        for (path, content) in [
            (
                "Extensions/kit/init.lua",
                "return { value = function() return require('words').hi end }",
            ),
            ("Extensions/kit/words.lua", "return { hi = 'hi' }"),
        ] {
            assert!(matches!(
                call(WRITE, json!({ "path": path, "content": content })),
                Planned::Done(_)
            ));
        }
        let drafts = ws.take_drafts();
        assert!(drafts[0].is_new());
        let out = ws.commit(drafts.into_iter().next().unwrap());
        assert!(!out.text.starts_with("Error"), "{}", out.text);
        let extension = crate::extension::Extension::open(&root, "kit").unwrap();
        assert_eq!(extension.value(&[], "a.md").unwrap().as_deref(), Some("hi"));
        // 폴더 확장 옆에 같은 이름의 파일은 쓰지 못한다.
        let Planned::Done(out) = call(
            WRITE,
            json!({ "path": "Extensions/kit.lua", "content": "return {}" }),
        ) else {
            panic!()
        };
        assert!(out.text.contains("folder extension"), "{}", out.text);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_out_of_root() {
        let (ws, root) = workspace();
        let outside = temp_dir("agent-tools-outside");
        std::fs::write(outside.join("secret.md"), "비밀").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Projects/link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.md"), root.join("Projects/s.md")).unwrap();
        let read = ws.call(READ, &args(&[("path", "Projects/link/secret.md")]));
        assert_eq!(read.text, format!("Error: {OUTSIDE}"));
        let write = ws.call(
            WRITE,
            &args(&[("path", "Projects/link/new.md"), ("content", "x")]),
        );
        assert_eq!(write.text, format!("Error: {OUTSIDE}"));
        let write = ws.call(WRITE, &args(&[("path", "Projects/s.md"), ("content", "x")]));
        assert!(write.text.starts_with("Error:"));
        assert_eq!(
            std::fs::read_to_string(outside.join("secret.md")).unwrap(),
            "비밀"
        );
    }

    #[test]
    fn writes_only_text_files_into_existing_folders() {
        let (ws, _) = workspace();
        let out = ws.call(
            WRITE,
            &args(&[("path", "Projects/cover.png"), ("content", "")]),
        );
        assert!(
            out.text.starts_with("Error: This file type"),
            "{}",
            out.text
        );
        let out = ws.call(
            WRITE,
            &args(&[("path", "Projects/없음/a.md"), ("content", "")]),
        );
        assert!(out.text.starts_with("Error:"));
        let out = ws.call(WRITE, &args(&[("path", "Projects/소설/1장.md")]));
        assert_eq!(out.text, "Error: content is missing.");
    }
}
