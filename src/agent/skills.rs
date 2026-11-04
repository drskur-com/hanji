//! 스킬: 필요할 때 읽어 따르는 지시 묶음([AgentSkills](https://agentskills.io) 형식).
//!
//! 스킬 하나는 `SKILL.md` 하나다. 앞머리(`---`로 감싼 YAML)에 `name`·`description`을 적고, 본문에
//! 지시를 쓴다. 시스템 프롬프트에는 이름과 설명만 넣고, 모델이 `load_skill`을 부를 때 본문을 준다.
//!
//! 스킬은 `<라이브러리>/Skills/<이름>/SKILL.md`에서만 읽는다. 폴더 안의 다른 파일은 `read_file`로
//! 읽을 수 있다.
//!
//! 빌트인 스킬(`resources/skills/<이름>/`의 `SKILL.md`와 참고 파일)은 바이너리에 넣어 두고, 라이브러리를
//! 열 때 `Skills/`에 깔아 준다([`sync_builtin`]). 무엇을 깔았는지는 `.hanji/skills.toml`에 파일마다
//! 해시로 적어, 사용자가 고친 파일은 덮지 않고 지운 스킬은 다시 만들지 않는다.
//!
//! 모델이 읽는 글이라 스킬은 영어로 쓴다(답은 사용자의 언어로 하라고 적는다). 언어에 묶인 규칙은
//! `references/<언어>.md`로 나눠 필요할 때 읽게 한다.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// 라이브러리 루트 안의 스킬 폴더.
pub const SKILLS_DIR: &str = "Skills";

/// 스킬 파일 이름.
const SKILL_FILE: &str = "SKILL.md";

/// 이보다 큰 `SKILL.md`는 읽지 않는다.
const MAX_SKILL_BYTES: u64 = 64 * 1024;

/// 빌트인 스킬 하나: 폴더 이름과 그 안의 파일들((스킬 폴더 기준 경로, 내용)). 첫 파일은 `SKILL.md`다.
struct Builtin {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
}

macro_rules! builtin {
    ($name:literal: $($file:literal),+) => {
        Builtin {
            name: $name,
            files: &[$(($file, include_str!(concat!("../../resources/skills/", $name, "/", $file)))),+],
        }
    };
}

const BUILTIN: &[Builtin] = &[
    builtin!("proofread": "SKILL.md", "references/ko.md"),
    builtin!("revise": "SKILL.md"),
    builtin!("brainstorm": "SKILL.md"),
    builtin!("settings": "SKILL.md"),
    builtin!("sheet-format": "SKILL.md"),
    builtin!("theme": "SKILL.md"),
    builtin!(
        "extension": "SKILL.md",
        "references/draw.md",
        "references/draw-3d.md",
        "references/commands.md",
        "references/interactive.md",
        "references/actions.md",
        "references/inline.md",
        "references/play.md",
        "references/modules.md",
        "references/examples/archer.md",
        "references/examples/bar-chart.md",
        "references/examples/chart-remote.md",
        "references/examples/dodge.md",
        "references/examples/night-sky.md",
        "references/examples/pick-chart.md",
        "references/examples/route.md",
        "references/examples/seedling.md",
        "references/examples/text-tools.md",
        "references/examples/today.md",
        "references/examples/turntable.md"
    ),
];

/// 예전 빌트인에 있다가 빠진 파일(`<스킬>/<파일>`). 사용자가 고치지 않았으면 지운다(남겨 두면 모델이 옛 내용을 읽는다).
const RETIRED: &[&str] = &[
    "extension/references/api.md",
    "extension/references/examples.md",
];

/// 깐 빌트인 스킬의 기록(`.hanji/skills.toml`).
const RECORD_FILE: &str = "skills.toml";

/// 스킬 하나.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// 언제 쓰는지. 모델이 이것을 보고 스킬을 고른다.
    pub description: String,
    /// 지시(앞머리를 뺀 본문).
    pub body: String,
    /// 워크스페이스 스킬이면 그 폴더(라이브러리 루트에서의 상대 경로). 빌트인이면 `None`.
    pub dir: Option<String>,
}

/// `root`(라이브러리)의 `Skills/<이름>/SKILL.md`를 모두 읽는다. 이름순이고, 같은 이름이 둘이면
/// 앞의 것만 쓴다. 읽지 못하거나 설명이 없는 스킬은 건너뛴다. 심볼릭 링크는 따라가지 않는다
/// (라이브러리 밖을 가리킬 수 있다).
pub fn load(root: &Path) -> Vec<Skill> {
    let dir = root.join(SKILLS_DIR);
    if !is_real_dir(&dir) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut skills: Vec<Skill> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            if dir_name.starts_with('.') {
                return None;
            }
            let file = entry.path().join(SKILL_FILE);
            let meta = std::fs::symlink_metadata(&file).ok()?;
            if !meta.is_file() || meta.len() > MAX_SKILL_BYTES {
                return None;
            }
            let text = std::fs::read_to_string(&file).ok()?;
            parse(
                &text,
                Some(&dir_name),
                Some(format!("{SKILLS_DIR}/{dir_name}")),
            )
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.dir.cmp(&b.dir)));
    skills.dedup_by(|later, first| later.name == first.name);
    skills
}

/// 빌트인 스킬을 `root`의 `Skills/`에 깔거나 새 버전으로 바꾼다. 파일마다 판단한다.
///
/// | 기록 | 파일 | 하는 일 |
/// |---|---|---|
/// | 없음 | 없음 | 깔고 기록한다 |
/// | 없음 | 있음 | 사용자 파일이다. 그대로 둔다(빌트인과 같으면 기록만 한다) |
/// | 있음 | 없음 | 사용자가 지웠다. 다시 만들지 않는다 |
/// | 있음 | 기록과 같음 | 고치지 않았다. 새 버전으로 바꾼다 |
/// | 있음 | 기록과 다름 | 사용자가 고쳤다. 그대로 둔다 |
///
/// 스킬의 `SKILL.md`를 사용자가 지웠거나 사용자 것이면, 그 스킬의 다른 파일도 깔지 않는다.
/// 빌트인에서 빠진 파일([`RETIRED`])은 고치지 않았으면 지운다.
pub fn sync_builtin(root: &Path) -> io::Result<()> {
    let skills_dir = root.join(SKILLS_DIR);
    if skills_dir.exists() && !is_real_dir(&skills_dir) {
        return Ok(());
    }
    let record_path = root.join(crate::library::MARKER_DIR).join(RECORD_FILE);
    let mut record = read_record(&record_path);
    let before = record.clone();
    for skill in BUILTIN {
        let dir = skills_dir.join(skill.name);
        if dir.exists() && !is_real_dir(&dir) {
            continue;
        }
        for (ix, (file_name, text)) in skill.files.iter().enumerate() {
            let key = format!("{}/{file_name}", skill.name);
            let file = dir.join(file_name);
            let current = std::fs::symlink_metadata(&file).ok().map(|meta| {
                meta.is_file()
                    .then(|| std::fs::read_to_string(&file).ok())
                    .flatten()
            });
            let builtin = hash(text);
            let install = match (record.get(&key), &current) {
                (None, None) => true,
                (None, Some(Some(current))) if hash(current) == builtin => {
                    record.insert(key.clone(), builtin.clone());
                    false
                }
                (Some(installed), Some(Some(current))) if hash(current) == *installed => {
                    *installed != builtin
                }
                _ => false,
            };
            if install {
                if let Some(parent) = file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                crate::library::write_sheet(&file, text)?;
                record.insert(key, builtin);
            } else if ix == 0 && !record.contains_key(&key) {
                // SKILL.md가 사용자 것이다. 빌트인의 다른 파일을 섞지 않는다.
                break;
            } else if ix == 0 && current.is_none() {
                // 사용자가 스킬을 지웠다.
                break;
            }
        }
    }
    // 고치지 않은 옛 빌트인 파일은 지운다. 사용자가 고쳤으면 사용자 파일로 두고 기록만 지운다.
    for key in RETIRED {
        let Some(installed) = record.remove(*key) else {
            continue;
        };
        let file = skills_dir.join(key);
        if std::fs::read_to_string(&file).is_ok_and(|current| hash(&current) == installed) {
            std::fs::remove_file(&file)?;
        }
    }
    if record != before {
        let text = toml::to_string(&Record { installed: record })
            .map_err(|err| io::Error::other(err.to_string()))?;
        crate::library::write_sheet(&record_path, &text)?;
    }
    Ok(())
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
struct Record {
    /// `<스킬>/<파일>` → 깐 내용의 해시.
    #[serde(default)]
    installed: BTreeMap<String, String>,
}

fn read_record(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<Record>(&text).ok())
        .unwrap_or_default()
        .installed
}

/// 심볼릭 링크가 아닌 폴더.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// 내용 해시(FNV-1a 64비트). Rust 버전이 바뀌어도 같은 값이어야 해서 직접 계산한다.
fn hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// `SKILL.md`를 읽는다. 이름이 없으면 폴더 이름을 쓰고, 설명이 없으면 스킬로 치지 않는다.
fn parse(text: &str, dir_name: Option<&str>, dir: Option<String>) -> Option<Skill> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (front, body) = split_front_matter(text)?;
    let mut name = None;
    let mut description = None;
    // 앞머리는 `key: value` 한 줄씩만 읽는다(name·description에는 이것으로 충분하다).
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "name" if !value.is_empty() => name = Some(value.to_string()),
            "description" if !value.is_empty() => description = Some(value.to_string()),
            _ => {}
        }
    }
    Some(Skill {
        name: name.or_else(|| dir_name.map(str::to_string))?,
        description: description?,
        body: body.trim().to_string(),
        dir,
    })
}

/// `---`로 감싼 앞머리와 본문.
fn split_front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---")?.trim_start_matches([' ', '\t']);
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|value| value.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// `load_skill`의 답: 스킬 본문. 워크스페이스 스킬이면 폴더 위치도 알린다.
pub fn instructions(skill: &Skill) -> String {
    match &skill.dir {
        Some(dir) => format!(
            "{}\n\n(Skill folder: {dir}/ — read files the instructions refer to with read_file, \
             using paths relative to this folder.)",
            skill.body
        ),
        None => skill.body.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::temp_dir;

    fn builtin(name: &str) -> &'static Builtin {
        BUILTIN.iter().find(|skill| skill.name == name).unwrap()
    }

    #[test]
    fn builtin_skills_parse() {
        for skill in BUILTIN {
            let (file, text) = skill.files[0];
            assert_eq!(file, SKILL_FILE);
            let parsed = parse(text, Some(skill.name), None).unwrap();
            assert_eq!(parsed.name, skill.name);
            assert!(!parsed.description.is_empty() && !parsed.body.is_empty());
            // 함께 까는 참고 파일은 본문이나 다른 참고 파일이 가리킨다(참고 파일 안에서는 `references/` 기준).
            for (file, _) in &skill.files[1..] {
                let relative = file.trim_start_matches("references/");
                let mentioned = parsed.body.contains(file)
                    || skill
                        .files
                        .iter()
                        .any(|(other, text)| other != file && text.contains(relative));
                assert!(mentioned, "{}: {file}", skill.name);
            }
        }
    }

    #[test]
    fn parses_front_matter() {
        let skill = parse(
            "---\nname: \"tone\"\ndescription: 어조를 맞춘다: 존댓말\nlicense: MIT\n---\n\n# 어조\n본문\n",
            Some("dir"),
            None,
        )
        .unwrap();
        assert_eq!(skill.name, "tone");
        assert_eq!(skill.description, "어조를 맞춘다: 존댓말");
        assert_eq!(skill.body, "# 어조\n본문");
        // 이름이 없으면 폴더 이름, 설명이 없으면 스킬이 아니다.
        assert_eq!(
            parse("---\ndescription: d\n---\nb", Some("dir"), None)
                .unwrap()
                .name,
            "dir"
        );
        assert!(parse("---\nname: x\n---\nb", None, None).is_none());
        assert!(parse("앞머리 없음", Some("dir"), None).is_none());
    }

    /// 병렬 테스트끼리 폴더가 겹치지 않게 테스트마다 다른 `tag`를 준다.
    fn library(tag: &str) -> std::path::PathBuf {
        let root = temp_dir(&format!("agent-skills-{tag}"));
        std::fs::create_dir_all(root.join(crate::library::MARKER_DIR)).unwrap();
        root
    }

    fn names(root: &Path) -> Vec<String> {
        load(root).into_iter().map(|skill| skill.name).collect()
    }

    #[test]
    fn loads_workspace_skills_only() {
        let root = library("load");
        assert!(load(&root).is_empty());
        let skills_dir = root.join(SKILLS_DIR);
        std::fs::create_dir_all(skills_dir.join("glossary")).unwrap();
        std::fs::write(
            skills_dir.join("glossary/SKILL.md"),
            "---\ndescription: 용어집을 따른다\n---\nterms.md를 본다",
        )
        .unwrap();
        // 설명 없는 스킬과 SKILL.md 없는 폴더는 건너뛴다.
        std::fs::create_dir_all(skills_dir.join("broken")).unwrap();
        std::fs::create_dir_all(skills_dir.join("empty")).unwrap();
        std::fs::write(
            skills_dir.join("broken/SKILL.md"),
            "---\nname: broken\n---\n",
        )
        .unwrap();

        let skills = load(&root);
        assert_eq!(skills.len(), 1);
        assert_eq!(
            instructions(&skills[0]),
            "terms.md를 본다\n\n(Skill folder: Skills/glossary/ — read files the instructions refer to with read_file, using paths relative to this folder.)"
        );
    }

    #[test]
    fn sync_installs_updates_and_respects_user_changes() {
        let root = library("sync");
        sync_builtin(&root).unwrap();
        assert_eq!(
            names(&root),
            [
                "brainstorm",
                "extension",
                "proofread",
                "revise",
                "settings",
                "sheet-format",
                "theme"
            ]
        );
        let file = |name: &str| root.join(SKILLS_DIR).join(name).join(SKILL_FILE);

        let reference = root.join(SKILLS_DIR).join("proofread/references/ko.md");
        assert_eq!(
            std::fs::read_to_string(&reference).unwrap(),
            builtin("proofread").files[1].1
        );

        // 사용자가 고친 것과 지운 것은 그대로 둔다(지운 스킬은 참고 파일도 다시 만들지 않는다).
        std::fs::write(file("proofread"), "---\ndescription: 우리 규칙\n---\n규칙").unwrap();
        std::fs::remove_file(&reference).unwrap();
        std::fs::remove_dir_all(root.join(SKILLS_DIR).join("brainstorm")).unwrap();
        sync_builtin(&root).unwrap();
        assert_eq!(
            names(&root),
            [
                "extension",
                "proofread",
                "revise",
                "settings",
                "sheet-format",
                "theme"
            ]
        );
        assert!(!reference.exists());
        assert!(!root.join(SKILLS_DIR).join("brainstorm").exists());
        assert!(
            std::fs::read_to_string(file("proofread"))
                .unwrap()
                .contains("우리 규칙")
        );

        // 고치지 않은 스킬은 앱의 새 버전으로 바뀐다(옛 버전을 깐 것으로 기록을 꾸민다).
        let old = "---\ndescription: 옛 버전\n---\n옛";
        std::fs::write(file("revise"), old).unwrap();
        let record_path = root.join(crate::library::MARKER_DIR).join(RECORD_FILE);
        let mut record = read_record(&record_path);
        record.insert("revise/SKILL.md".into(), hash(old));
        std::fs::write(
            &record_path,
            toml::to_string(&Record { installed: record }).unwrap(),
        )
        .unwrap();
        sync_builtin(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(file("revise")).unwrap(),
            builtin("revise").files[0].1
        );
    }

    #[test]
    fn sync_removes_retired_files_unless_changed() {
        let root = library("retired");
        let references = root.join(SKILLS_DIR).join("extension/references");
        std::fs::create_dir_all(&references).unwrap();
        // 옛 버전이 깐 두 파일 중 하나는 사용자가 고쳤다.
        let (old, mine) = ("old api", "my examples");
        std::fs::write(references.join("api.md"), old).unwrap();
        std::fs::write(references.join("examples.md"), mine).unwrap();
        let record_path = root.join(crate::library::MARKER_DIR).join(RECORD_FILE);
        std::fs::create_dir_all(record_path.parent().unwrap()).unwrap();
        let record: BTreeMap<String, String> = [
            ("extension/references/api.md".into(), hash(old)),
            ("extension/references/examples.md".into(), hash("installed")),
        ]
        .into();
        std::fs::write(
            &record_path,
            toml::to_string(&Record { installed: record }).unwrap(),
        )
        .unwrap();
        sync_builtin(&root).unwrap();
        assert!(!references.join("api.md").exists());
        assert_eq!(
            std::fs::read_to_string(references.join("examples.md")).unwrap(),
            mine
        );
        assert!(references.join("draw.md").exists());
        let record = read_record(&record_path);
        assert!(!record.keys().any(|key| RETIRED.contains(&key.as_str())));
    }

    #[test]
    fn sync_keeps_user_skill_with_builtin_name() {
        let root = library("user");
        std::fs::create_dir_all(root.join(SKILLS_DIR).join("proofread")).unwrap();
        let mine = "---\ndescription: 내 교정\n---\n내 것";
        std::fs::write(
            root.join(SKILLS_DIR).join("proofread").join(SKILL_FILE),
            mine,
        )
        .unwrap();
        sync_builtin(&root).unwrap();
        sync_builtin(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(SKILLS_DIR).join("proofread").join(SKILL_FILE))
                .unwrap(),
            mine
        );
        // 사용자 스킬에 빌트인 참고 파일을 섞지 않는다.
        assert!(!root.join(SKILLS_DIR).join("proofread/references").exists());
    }
}
