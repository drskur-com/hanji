//! 그룹 메타(`<Group>/.hanji.toml`) 읽기/쓰기, 프로젝트·그룹·시트 파일 만들기와 순서 바꾸기.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::{
    ATTACHMENTS_GROUP, EXTRAS_DIR, GROUP_META_FILE, Group, GroupKind, Sheet, SortMode,
    TEMPLATES_GROUP, file_name, is_notes_dir, is_reserved_group, sheet_file_name,
};

/// 그룹 폴더의 `.hanji.toml` 스키마.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub(super) struct GroupMeta {
    /// 아이콘 이름(예: inbox, folder, book).
    pub(super) icon: Option<String>,
    /// 색 이름(예: blue). 화면에서 테마 색으로 바꾼다.
    pub(super) color: Option<String>,
    /// 표시 이름(없으면 폴더 이름).
    pub(super) title: Option<String>,
    /// 시트 파일명 순서. 나열되지 않은 파일은 뒤에 이름순으로 붙는다.
    pub(super) sheets: Vec<String>,
    /// 앞 시트에 접합한 시트 파일명.
    pub(super) glued: Vec<String>,
    /// 하위 그룹 폴더 이름 순서. 나열되지 않은 그룹은 뒤에 이름순으로 붙는다.
    pub(super) groups: Vec<String>,
    /// 시트 정렬 방법(`SortMode::key`).
    pub(super) sort: Option<String>,
    /// 정렬 방향. 없으면 정렬 방법의 기본 방향.
    pub(super) descending: Option<bool>,
    /// 그룹이 담는 것(`GroupKind::key`). 없으면 시트.
    pub(super) kind: Option<String>,
    /// 파일 그룹의 파일 이름 순서. 나열되지 않은 파일은 뒤에 이름순으로 붙는다.
    pub(super) files: Vec<String>,
}

/// 그룹 폴더의 `.hanji.toml` 을 읽는다. 없거나 깨지면 기본값.
pub(super) fn read_group_meta(dir: &Path) -> GroupMeta {
    std::fs::read_to_string(dir.join(GROUP_META_FILE))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/// 그룹 메타 파일이 없으면 기본값으로 만든다(멱등).
pub(super) fn write_group_meta_if_absent(
    dir: &Path,
    icon: Option<&str>,
    title: Option<&str>,
) -> io::Result<()> {
    let path = dir.join(GROUP_META_FILE);
    if path.exists() {
        return Ok(());
    }
    let mut table = toml::Table::new();
    if let Some(icon) = icon {
        table.insert("icon".to_string(), toml::Value::String(icon.to_string()));
    }
    if let Some(title) = title {
        table.insert("title".to_string(), toml::Value::String(title.to_string()));
    }
    let text = toml::to_string_pretty(&table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    std::fs::write(path, text)
}

/// 그룹 안에서 시트를 `from` 위치에서 `to` 위치로 옮기고, 새 순서를 `.hanji.toml`에 쓴다.
///
/// 범위를 벗어나면 아무것도 하지 않는다. 쓰기에 실패하면 메모리의 순서도 되돌린다.
/// 개별 시트를 옮긴다(ADR-0014). 같은 접합 안에서는 접합을 유지하고, 밖으로 빼면
/// 옮긴 시트만 해제한다. 다른 접합의 두 시트 사이에 넣으면 그 접합에 들어간다.
/// `to`는 이동 후 시트 위치다. 위로 옮기면 대상 앞, 아래로 옮기면 대상 뒤에 놓는다.
pub fn move_sheet(group: &mut Group, from: usize, to: usize) -> io::Result<()> {
    move_selected_sheets(group, &[from], from, to)
}

/// 그룹 안에서 선택한 시트들을 한 번에 옮긴다(ADR-0014, ADR-0019).
///
/// `indices`는 그룹 목록에서의 위치(정렬 전)이고 `anchor`는 끈 카드의 위치다.
/// `to`는 놓을 대상 카드의 위치이며, 위로 옮기면 그 앞, 아래로 옮기면 그 뒤에 놓는다.
/// 선택한 시트는 목록 순서대로 한 묶음으로 움직이고, 서로 이어져 있던 접합은 유지한다.
pub fn move_selected_sheets(
    group: &mut Group,
    indices: &[usize],
    anchor: usize,
    to: usize,
) -> io::Result<()> {
    let len = group.sheets.len();
    if anchor >= len || to >= len || anchor == to {
        return Ok(());
    }
    let mut selected: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|index| *index < len)
        .collect();
    selected.sort_unstable();
    selected.dedup();
    if !selected.contains(&anchor) {
        selected.push(anchor);
        selected.sort_unstable();
    }
    // 대상 카드가 선택 안에 있으면(선택을 자기 위에 놓으면) 그대로 둔다.
    if selected.is_empty() || selected.contains(&to) {
        return Ok(());
    }

    // 이동 전 접합 소속을 임시 번호로 보존한다. 메타에는 앞 시트에 이어짐만 저장한다.
    let mut membership = Vec::with_capacity(len);
    let mut block = 0;
    for (index, sheet) in group.sheets.iter().enumerate() {
        if index > 0 && !sheet.glued {
            block += 1;
        }
        membership.push(block);
    }
    let source = membership[anchor];
    let same_block = source == membership[to];

    let remaining: Vec<(Sheet, usize)> = group
        .sheets
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected.contains(index))
        .map(|(index, sheet)| (sheet.clone(), membership[index]))
        .collect();
    // 선택을 뺀 목록에서의 삽입 위치. 아래로 옮기면 대상 뒤, 위로 옮기면 대상 앞이다.
    let before_target = selected.iter().filter(|index| **index < to).count();
    let insert = if anchor < to {
        to - before_target + 1
    } else {
        to - before_target
    };

    let mut next_block = membership.iter().max().copied().unwrap_or(0) + 1;
    let destination = if same_block {
        source
    } else if insert > 0
        && insert < remaining.len()
        && remaining[insert - 1].1 == remaining[insert].1
    {
        // 대상 접합의 두 시트 사이에 놓이면 그 접합에 합류한다.
        remaining[insert - 1].1
    } else {
        next_block
    };

    // 선택한 시트를 목록 순서대로 한 묶음으로 만든다. 원래 붙어 있던 시트만 이어 붙인다.
    let mut moved: Vec<(Sheet, usize)> = Vec::with_capacity(selected.len());
    let mut previous: Option<usize> = None;
    let mut run_block = destination;
    for &index in &selected {
        let continues = previous.is_some_and(|previous| previous + 1 == index)
            && membership[previous.unwrap()] == membership[index];
        if previous.is_some() && !continues {
            next_block += 1;
            run_block = next_block;
        }
        moved.push((group.sheets[index].clone(), run_block));
        previous = Some(index);
    }

    let mut sheets = Vec::with_capacity(len);
    let mut blocks = Vec::with_capacity(len);
    for (sheet, block) in remaining[..insert].iter().cloned() {
        sheets.push(sheet);
        blocks.push(block);
    }
    for (sheet, block) in moved {
        sheets.push(sheet);
        blocks.push(block);
    }
    for (sheet, block) in remaining[insert..].iter().cloned() {
        sheets.push(sheet);
        blocks.push(block);
    }
    for (index, sheet) in sheets.iter_mut().enumerate() {
        sheet.glued = index > 0 && blocks[index - 1] == blocks[index];
    }

    let before = group.sheets.clone();
    group.sheets = sheets;
    write_order_and_glue(group, before)
}

/// 선택한 시트를 다른 그룹의 끝으로 옮긴다(ADR-0019).
///
/// 파일을 실제로 옮기고 두 그룹의 순서·접합을 저장한다. 원본에 남은 시트의 접합은 유지하고,
/// 옮긴 묶음 안에서 원래 붙어 있던 시트의 접합도 유지한다. 대상 그룹에 같은 파일명이 있으면
/// `이름 2.md`처럼 비어 있는 이름을 쓴다. 성공하면 (옛 경로, 새 경로) 목록을 돌려준다.
///
/// 쓰기에 실패하면 옮긴 파일과 대상 메타를 되돌린다. 원본·대상 모델은 성공했을 때만 바뀐다.
pub fn move_sheets_between(
    source: &mut Group,
    target: &mut Group,
    indices: &[usize],
) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    if source.path == target.path || target.kind == GroupKind::Files {
        return Ok(Vec::new());
    }
    let mut selected: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|index| *index < source.sheets.len())
        .collect();
    // 넘어온 순서를 대상 그룹의 순서로 쓴다. 표시 목록 순서와 그룹의 실제 순서가 다를 수 있다.
    selected.dedup();
    if selected.is_empty() {
        return Ok(Vec::new());
    }

    let original_source = source.sheets.clone();
    let original_target = target.sheets.clone();

    // 원본의 접합 소속. 선택을 빼도 같은 묶음에 남은 시트끼리는 이어진다(ADR-0014).
    let mut membership = Vec::with_capacity(original_source.len());
    let mut block = 0;
    for (index, sheet) in original_source.iter().enumerate() {
        if index > 0 && !sheet.glued {
            block += 1;
        }
        membership.push(block);
    }
    let mut remaining: Vec<(Sheet, usize)> = original_source
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected.contains(index))
        .map(|(index, sheet)| (sheet.clone(), membership[index]))
        .collect();
    for index in 1..remaining.len() {
        remaining[index].0.glued = remaining[index - 1].1 == remaining[index].1;
    }
    if let Some((first, _)) = remaining.first_mut() {
        first.glued = false;
    }
    let remaining: Vec<Sheet> = remaining.into_iter().map(|(sheet, _)| sheet).collect();

    let mut moved: Vec<Sheet> = Vec::with_capacity(selected.len());
    let mut previous: Option<usize> = None;
    for &index in &selected {
        let mut sheet = original_source[index].clone();
        sheet.glued = previous.is_some_and(|previous| {
            previous + 1 == index && membership[previous] == membership[index]
        });
        moved.push(sheet);
        previous = Some(index);
    }

    // 파일명 충돌을 피해 새 경로를 정하고, 실제 파일을 옮긴다.
    let mut taken = std::collections::HashSet::new();
    let mut moves = Vec::with_capacity(moved.len());
    for sheet in &mut moved {
        let to = unique_sheet_path(&target.path, &sheet.path, &mut taken);
        moves.push((sheet.path.clone(), to.clone()));
        sheet.path = to;
    }
    let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (from, to) in &moves {
        if let Err(err) = std::fs::rename(from, to) {
            for (from, to) in renamed.iter().rev() {
                let _ = std::fs::rename(to, from);
            }
            return Err(err);
        }
        renamed.push((from.clone(), to.clone()));
    }

    let mut destination = original_target;
    destination.extend(moved);
    if let Err(err) = write_sheet_order_and_glue(&target.path, &destination) {
        for (from, to) in renamed.iter().rev() {
            let _ = std::fs::rename(to, from);
        }
        return Err(err);
    }
    if let Err(err) = write_sheet_order_and_glue(&source.path, &remaining) {
        let _ = write_sheet_order_and_glue(&target.path, &target.sheets);
        for (from, to) in renamed.iter().rev() {
            let _ = std::fs::rename(to, from);
        }
        return Err(err);
    }
    source.sheets = remaining;
    target.sheets = destination;
    Ok(moves)
}

/// `dir` 안에서 `source`와 겹치지 않는 시트 파일 경로를 찾는다.
fn unique_sheet_path(
    dir: &Path,
    source: &Path,
    taken: &mut std::collections::HashSet<String>,
) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "무제".to_string());
    let extension = source
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let mut name = format!("{stem}{extension}");
    let mut n = 2;
    while taken.contains(&name) || dir.join(&name).exists() {
        name = format!("{stem} {n}{extension}");
        n += 1;
    }
    taken.insert(name.clone());
    dir.join(name)
}

/// `range`의 시트를 하나로 잇는다: 첫 시트 뒤의 시트를 모두 앞 시트에 접합한다.
///
/// 지금 순서를 메타 `sheets`에도 적는다. 순서가 정해지지 않은 시트는 수정할 때마다 자리가 바뀌어
/// 접합한 이웃이 달라질 수 있어서다.
pub fn glue_sheets(group: &mut Group, range: Range<usize>) -> io::Result<()> {
    let end = range.end.min(group.sheets.len());
    if range.start + 1 >= end {
        return Ok(());
    }
    let before = group.sheets.clone();
    for sheet in &mut group.sheets[range.start + 1..end] {
        sheet.glued = true;
    }
    write_order_and_glue(group, before)
}

/// `range`의 시트를 앞뒤 시트에서 떼어 낸다: 범위의 시트와 범위 바로 뒤 시트의 접합을 푼다.
pub fn unglue_sheets(group: &mut Group, range: Range<usize>) -> io::Result<()> {
    let end = (range.end + 1).min(group.sheets.len());
    if range.start >= end {
        return Ok(());
    }
    let before = group.sheets.clone();
    for sheet in &mut group.sheets[range.start..end] {
        sheet.glued = false;
    }
    write_order_and_glue(group, before)
}

/// 시트 순서와 접합을 메타에 쓴다. 실패하면 `before`로 되돌린다.
fn write_order_and_glue(group: &mut Group, before: Vec<super::Sheet>) -> io::Result<()> {
    if let Err(err) = write_sheet_order_and_glue(&group.path, &group.sheets) {
        group.sheets = before;
        return Err(err);
    }
    Ok(())
}

/// 그룹의 시트 `paths`를 휴지통으로 옮긴다. 옮긴 시트의 경로를 돌려주고, 처음 난 오류는 함께 돌려준다.
///
/// 접합한 시트 묶음의 첫 시트를 지우면 남은 다음 시트가 묶음의 처음이 된다. 그대로 두면 지운 시트
/// 앞의 다른 시트에 이어지기 때문이다. 지운 시트 사이의 시트끼리는 그대로 잇는다.
pub fn trash_sheets(group: &mut Group, paths: &[PathBuf]) -> (Vec<PathBuf>, Option<io::Error>) {
    let mut trashed = Vec::new();
    let mut error = None;
    for path in paths {
        match move_to_trash(path) {
            Ok(()) => trashed.push(path.clone()),
            Err(err) => {
                error.get_or_insert(err);
            }
        }
    }
    if let Err(err) = forget_sheets(group, &trashed) {
        error.get_or_insert(err);
    }
    (trashed, error)
}

/// 지운 시트 `removed`를 그룹 목록에서 빼고, 첫 시트를 잃은 접합 묶음은 다음 시트부터 다시 시작한다.
/// 접합이 바뀌었을 때만 메타에 쓴다.
fn forget_sheets(group: &mut Group, removed: &[PathBuf]) -> io::Result<()> {
    let before = group.sheets.clone();
    let mut head_removed = false;
    let mut changed = false;
    for (index, sheet) in group.sheets.iter_mut().enumerate() {
        let glued = index > 0 && sheet.glued;
        if removed.contains(&sheet.path) {
            head_removed = !glued || head_removed;
        } else {
            if glued && head_removed {
                sheet.glued = false;
                changed = true;
            }
            head_removed = false;
        }
    }
    group.sheets.retain(|sheet| !removed.contains(&sheet.path));
    if changed {
        write_order_and_glue(group, before)?;
    }
    Ok(())
}

/// 시트 목록의 순서와 접합을 그룹 메타에 쓴다.
fn write_sheet_order_and_glue(dir: &Path, sheets: &[Sheet]) -> io::Result<()> {
    let names = |glued_only: bool| -> Vec<toml::Value> {
        sheets
            .iter()
            .enumerate()
            .filter(|(index, sheet)| !glued_only || (*index > 0 && sheet.glued))
            .map(|(_, sheet)| toml::Value::String(sheet_file_name(sheet)))
            .collect()
    };
    let order = names(false);
    let glued = names(true);
    let glued = (!glued.is_empty()).then_some(toml::Value::Array(glued));
    write_meta_values(
        dir,
        [
            ("sheets", Some(toml::Value::Array(order))),
            ("glued", glued),
        ],
    )
}

/// 그룹의 정렬 방법을 바꾸고 `.hanji.toml`의 `sort`에 쓴다. 쓰기에 실패하면 메모리 값도 그대로 둔다.
///
/// 방향은 정렬 방법마다 기본이 달라서(날짜는 최신부터, 제목은 ㄱ부터) 방법을 바꾸면 지운다.
pub fn set_sort(group: &mut Group, mode: SortMode) -> io::Result<()> {
    write_meta_values(
        &group.path,
        [
            ("sort", Some(toml::Value::String(mode.key().to_string()))),
            ("descending", None),
        ],
    )?;
    group.sort = Some(mode);
    group.descending = None;
    Ok(())
}

/// 그룹의 정렬 방향을 바꾸고 `.hanji.toml`의 `descending`에 쓴다.
pub fn set_sort_descending(group: &mut Group, descending: bool) -> io::Result<()> {
    write_meta_values(
        &group.path,
        [("descending", Some(toml::Value::Boolean(descending)))],
    )?;
    group.descending = Some(descending);
    Ok(())
}

/// 그룹 `.hanji.toml`의 `key`(`sheets`·`groups`)를 `order`(이름 목록)로 바꾼다. 다른 키는 그대로 둔다.
pub(super) fn write_meta_list(group_dir: &Path, key: &str, order: &[String]) -> io::Result<()> {
    let order = order.iter().cloned().map(toml::Value::String).collect();
    write_meta_values(group_dir, [(key, Some(toml::Value::Array(order)))])
}

/// 그룹 `.hanji.toml`의 키들을 한 번에 바꾼다(`None`이면 지운다). 다른 키는 그대로 둔다.
///
/// 메타 파일이 깨져 있으면 덮어쓰지 않고 오류를 돌려준다. 임시 파일에 쓴 뒤 바꿔치기한다.
pub(super) fn write_meta_values<'a>(
    group_dir: &Path,
    values: impl IntoIterator<Item = (&'a str, Option<toml::Value>)>,
) -> io::Result<()> {
    let path = group_dir.join(GROUP_META_FILE);
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => toml::Table::new(),
        Err(err) => return Err(err),
    };
    for (key, value) in values {
        match value {
            Some(value) => table.insert(key.to_string(), value),
            None => table.remove(key),
        };
    }
    let text = toml::to_string_pretty(&table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let temp = group_dir.join(format!("{GROUP_META_FILE}.tmp"));
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, &path)
}

/// `Projects/` 아래에 새 프로젝트(그룹 폴더 + `.hanji.toml` + 기본 추가자료)를 만들고 경로를 돌려준다.
///
/// 폴더 이름은 `name`에서 경로 구분자를 뺀 것이고, 비어 있으면 "무제"다.
/// 같은 이름이 있으면 " 2", " 3"…을 붙인다. 표시 이름은 폴더 이름과 같다.
pub fn create_project(
    projects_dir: &Path,
    name: &str,
    icon: Option<&str>,
    color: Option<&str>,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(projects_dir)?;
    let dir = create_unique_dir(projects_dir, &project_dir_name(name))?;
    write_group_meta(&dir, icon, color)?;
    ensure_project_layout(&dir)?;
    Ok(dir)
}

/// 새 그룹 폴더에 아이콘·색만 담은 `.hanji.toml`을 쓴다.
fn write_group_meta(dir: &Path, icon: Option<&str>, color: Option<&str>) -> io::Result<()> {
    let mut table = toml::Table::new();
    for (key, value) in [("icon", icon), ("color", color)] {
        if let Some(value) = value {
            table.insert(key.to_string(), toml::Value::String(value.to_string()));
        }
    }
    let text = toml::to_string_pretty(&table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    std::fs::write(dir.join(GROUP_META_FILE), text)
}

/// 프로젝트에 추가자료 폴더가 없으면 기본 그룹(템플릿)과 함께 만든다(멱등).
///
/// 이미 있으면 건드리지 않는다. 사용자가 지운 기본 그룹을 다시 만들지 않기 위해서다.
pub(super) fn ensure_project_layout(project_dir: &Path) -> io::Result<()> {
    let extras = project_dir.join(EXTRAS_DIR);
    if extras.exists() {
        return Ok(());
    }
    let templates = extras.join(TEMPLATES_GROUP);
    std::fs::create_dir_all(&templates)?;
    write_group_meta_if_absent(&templates, Some("layout-template"), Some("템플릿"))
}

/// 메모 첨부파일 자리(`Notes/Attachments`)의 이름인지. 시트 그룹이 이 이름을 쓰면 첨부할 때
/// 파일 그룹으로 바뀌어 메모가 목록에서 사라진다.
fn is_note_attachments_name(parent: &Path, base: &str) -> bool {
    base == ATTACHMENTS_GROUP && is_notes_dir(parent)
}

/// `parent` 아래에 새 그룹 폴더(+ `.hanji.toml`)를 만들고 경로를 돌려준다.
///
/// 이름 규칙은 프로젝트와 같고, `Extras`는 추가자료 자리라, 메모의 `Attachments`는 첨부파일
/// 자리라 피한다.
/// 새 그룹은 `parent`의 그룹 순서(`groups`) 맨 뒤에 붙는다.
pub fn create_group(
    parent: &Path,
    name: &str,
    icon: Option<&str>,
    color: Option<&str>,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let mut base = project_dir_name(name);
    if base == EXTRAS_DIR || is_note_attachments_name(parent, &base) {
        base.push_str(" 2");
    }
    let dir = create_unique_dir(parent, &base)?;
    let order = write_group_meta(&dir, icon, color).and_then(|()| {
        if is_section_dir(parent) {
            return Ok(());
        }
        let mut order = group_order(parent)?;
        order.retain(|name| *name != file_name(&dir));
        order.push(file_name(&dir));
        write_meta_list(parent, "groups", &order)
    });
    if let Err(err) = order {
        // 방금 만든 폴더라 메타 파일 말고는 비어 있다.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(dir)
}

/// 섹션 폴더(`Notes/`·`Projects/`)인지. 섹션 바로 아래 그룹은 이름순이라 순서를 적지 않는다.
fn is_section_dir(dir: &Path) -> bool {
    dir.parent().is_some_and(super::layout::has_marker)
}

/// 그룹을 다른 그룹의 하위로 옮기고 새 경로를 돌려준다.
///
/// 폴더를 통째로 옮기므로 시트·파일·하위 그룹이 함께 간다. 원래 부모의 `groups`에서 빼고
/// 대상 그룹의 `groups` 맨 뒤에 붙인다. 같은 이름이 있으면 " 2", " 3"…을 붙인다.
/// 대상이 원본의 하위이거나 원본의 조상이면 옮기지 않는다(순환·제자리 이동 방지).
pub fn move_group(source: &Path, target: &Path) -> io::Result<PathBuf> {
    if !source.is_dir() || !target.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            t!("core.error.group_not_found").into_owned(),
        ));
    }
    if source == target || target.starts_with(source) || source.starts_with(target) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.group_into_itself").into_owned(),
        ));
    }
    let parent = source.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.no_parent").into_owned(),
        )
    })?;

    let source_name = file_name(source);
    let mut source_order = group_order(parent)?;
    source_order.retain(|name| *name != source_name);
    let mut target_order = group_order(target)?;
    let dest = unique_dir_path(target, &source_name);
    let dest_name = file_name(&dest);
    target_order.retain(|name| *name != dest_name);
    target_order.push(dest_name);

    // 메타 쓰기가 실패하면 폴더와 두 메타 파일을 원래대로 되돌린다.
    let source_meta = std::fs::read(parent.join(GROUP_META_FILE)).ok();
    let target_meta = std::fs::read(target.join(GROUP_META_FILE)).ok();
    std::fs::rename(source, &dest)?;
    let written = write_meta_list(target, "groups", &target_order)
        .and_then(|()| write_meta_list(parent, "groups", &source_order));
    if let Err(err) = written {
        let _ = std::fs::rename(&dest, source);
        restore_meta(parent, source_meta);
        restore_meta(target, target_meta);
        return Err(err);
    }
    Ok(dest)
}

/// `source` 그룹을 `anchor` 그룹과 같은 레벨(같은 부모)로 옮겨 `anchor` 바로 앞(`after`면 뒤)에 둔다.
/// 새 경로를 돌려준다.
///
/// 부모가 같으면 폴더는 그대로 두고 부모 메타의 `groups` 순서만 바꾼다. 부모가 다르면 폴더를
/// `anchor`의 부모로 옮기고(이름이 겹치면 " 2"…), 두 부모의 `groups`를 함께 쓴다. 메타 쓰기가
/// 실패하면 폴더와 두 메타 파일을 원래대로 되돌린다.
pub fn move_group_beside(source: &Path, anchor: &Path, after: bool) -> io::Result<PathBuf> {
    if !source.is_dir() || !anchor.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            t!("core.error.group_not_found").into_owned(),
        ));
    }
    if source == anchor || anchor.starts_with(source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.group_into_itself").into_owned(),
        ));
    }
    let no_parent = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.no_parent").into_owned(),
        )
    };
    let parent = anchor.parent().ok_or_else(no_parent)?;
    let source_parent = source.parent().ok_or_else(no_parent)?;
    let source_name = file_name(source);
    let anchor_name = file_name(anchor);

    let insert = |order: &mut Vec<String>, name: String| {
        order.retain(|existing| *existing != name);
        let index = order
            .iter()
            .position(|existing| *existing == anchor_name)
            .map_or(order.len(), |index| index + usize::from(after));
        order.insert(index, name);
    };

    if source_parent == parent {
        let mut order = group_order(parent)?;
        insert(&mut order, source_name);
        write_meta_list(parent, "groups", &order)?;
        return Ok(source.to_path_buf());
    }

    let mut source_order = group_order(source_parent)?;
    source_order.retain(|name| *name != source_name);
    let dest = unique_dir_path(parent, &source_name);
    let mut target_order = group_order(parent)?;
    insert(&mut target_order, file_name(&dest));

    let source_meta = std::fs::read(source_parent.join(GROUP_META_FILE)).ok();
    let target_meta = std::fs::read(parent.join(GROUP_META_FILE)).ok();
    std::fs::rename(source, &dest)?;
    let written = write_meta_list(parent, "groups", &target_order)
        .and_then(|()| write_meta_list(source_parent, "groups", &source_order));
    if let Err(err) = written {
        let _ = std::fs::rename(&dest, source);
        restore_meta(source_parent, source_meta);
        restore_meta(parent, target_meta);
        return Err(err);
    }
    Ok(dest)
}

/// `parent/base` 중 아직 없는 경로. 같은 이름이 있으면 " 2", " 3"…을 붙인다.
fn unique_dir_path(parent: &Path, base: &str) -> PathBuf {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            parent.join(base)
        } else {
            parent.join(format!("{base} {n}"))
        };
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// 메타 파일을 미리 읽어 둔 내용으로 되돌린다(`None`이면 원래 없던 파일).
fn restore_meta(dir: &Path, contents: Option<Vec<u8>>) {
    let path = dir.join(GROUP_META_FILE);
    match contents {
        Some(contents) => {
            let _ = std::fs::write(&path, contents);
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// `parent` 바로 아래 그룹 폴더 이름을 화면 순서(메타 `groups` → 이름순)로 돌려준다.
pub(super) fn group_order(parent: &Path) -> io::Result<Vec<String>> {
    let meta = read_group_meta(parent);
    let mut names = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let path = entry?.path();
        if super::is_group_dir(&path) && file_name(&path) != EXTRAS_DIR {
            names.push(file_name(&path));
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    super::order_by_names(&mut names, &meta.groups, String::clone);
    Ok(names)
}

/// `parent/base`를 만든다. 이미 있으면 " 2", " 3"…을 붙여 비어 있는 이름을 찾는다.
fn create_unique_dir(parent: &Path, base: &str) -> io::Result<PathBuf> {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            parent.join(base)
        } else {
            parent.join(format!("{base} {n}"))
        };
        // create_dir는 이미 있으면 실패하므로 동시에 만들어도 덮어쓰지 않는다.
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(err) => return Err(err),
        }
    }
}

/// 따로 풀어 둔 프로젝트 폴더(`staged`)를 `projects_dir` 아래 `name`(겹치면 " 2"…)으로 옮긴다.
///
/// 예전 이름(`.writer.toml`)의 그룹 메타는 지금 이름으로 바꾸고, 추가자료 폴더가 없으면 만든다.
/// `staged`는 `projects_dir`와 같은 볼륨에 있어야 한다(이름 바꾸기로 옮긴다).
pub fn adopt_project_dir(projects_dir: &Path, staged: &Path, name: &str) -> io::Result<PathBuf> {
    super::layout::rename_legacy_group_meta(staged)?;
    ensure_project_layout(staged)?;
    let base = project_dir_name(name);
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            projects_dir.join(&base)
        } else {
            projects_dir.join(format!("{base} {n}"))
        };
        if !candidate.exists() {
            std::fs::rename(staged, &candidate)?;
            return Ok(candidate);
        }
        n += 1;
    }
}

/// 프로젝트 이름 → 폴더 이름: 경로 구분자를 `-`로 바꾸고, 앞의 `.`(숨김)을 떼고, 비면 "무제".
fn project_dir_name(name: &str) -> String {
    let base: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let base = base.trim_start_matches('.').trim();
    if base.is_empty() {
        "무제".to_string()
    } else {
        base.to_string()
    }
}

/// 프로젝트의 아이콘·색을 바꾸고, 이름이 바뀌었으면 폴더 이름도 바꾼다. 새 경로를 돌려준다.
///
/// 메타의 다른 키(시트 순서 등)는 그대로 둔다. 표시 이름은 폴더 이름을 따르므로 `title`은 지운다.
/// 바꿀 이름이 이미 있으면 " 2", " 3"…을 붙인다.
pub fn update_project(
    dir: &Path,
    name: &str,
    icon: Option<&str>,
    color: Option<&str>,
) -> io::Result<PathBuf> {
    let path = dir.join(GROUP_META_FILE);
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => toml::Table::new(),
        Err(err) => return Err(err),
    };
    for (key, value) in [("icon", icon), ("color", color)] {
        match value {
            Some(value) => table.insert(key.to_string(), toml::Value::String(value.to_string())),
            None => table.remove(key),
        };
    }
    table.remove("title");
    let text = toml::to_string_pretty(&table)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let temp = dir.join(format!("{GROUP_META_FILE}.tmp"));
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, &path)?;

    let base = project_dir_name(name);
    if dir
        .file_name()
        .is_some_and(|current| current == base.as_str())
    {
        return Ok(dir.to_path_buf());
    }
    let parent = dir.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.no_parent").into_owned(),
        )
    })?;
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            parent.join(&base)
        } else {
            parent.join(format!("{base} {n}"))
        };
        // rename은 빈 폴더를 덮어쓸 수 있으므로 먼저 확인한다.
        if candidate.exists() {
            n += 1;
            continue;
        }
        std::fs::rename(dir, &candidate)?;
        return Ok(candidate);
    }
}

/// 그룹의 이름·아이콘·색을 바꾸고 새 경로를 돌려준다.
///
/// 보통 그룹은 프로젝트처럼 폴더 이름을 바꾸고 상위 그룹 순서(`groups`)의 이름도 고친다.
/// `Extras`는 추가자료 자리라 피한다. 앱이 폴더 이름으로 찾는 기본 그룹(템플릿·첨부 파일)은
/// 폴더를 그대로 두고 표시 이름을 `title`에 쓴다.
pub fn update_group(
    dir: &Path,
    name: &str,
    icon: Option<&str>,
    color: Option<&str>,
) -> io::Result<PathBuf> {
    let parent = dir.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("core.error.no_parent").into_owned(),
        )
    })?;
    if is_reserved_group(dir) {
        let text = |value: &str| Some(toml::Value::String(value.to_string()));
        write_meta_values(
            dir,
            [
                ("title", text(name.trim())),
                ("icon", icon.and_then(text)),
                ("color", color.and_then(text)),
            ],
        )?;
        return Ok(dir.to_path_buf());
    }

    let mut name = name.to_string();
    let base = project_dir_name(&name);
    if base == EXTRAS_DIR || is_note_attachments_name(parent, &base) {
        name.push_str(" 2");
    }
    let mut order = group_order(parent)?;
    let new_dir = update_project(dir, &name, icon, color)?;
    if new_dir != dir && !is_section_dir(parent) {
        let old = file_name(dir);
        for entry in &mut order {
            if *entry == old {
                *entry = file_name(&new_dir);
            }
        }
        write_meta_list(parent, "groups", &order)?;
    }
    Ok(new_dir)
}

/// 프로젝트·그룹 폴더를 운영체제 휴지통으로 옮긴다(복원 가능).
///
/// macOS는 Finder 대신 `NSFileManager`로 옮긴다. Finder 방식은 자동화 권한을 묻느라
/// 응답을 기다리는 동안 UI가 멈춘다. 대신 Finder의 "되돌려 놓기"는 없을 수 있다.
pub fn move_to_trash(dir: &Path) -> io::Result<()> {
    #[allow(unused_mut)]
    let mut trash = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos as _};
        trash.set_delete_method(DeleteMethod::NsFileManager);
    }
    trash.delete(dir).map_err(io::Error::other)
}

/// `dir` 안에 빈 시트 파일(`<ULID>.md`)을 만들고 경로를 돌려준다.
///
/// 파일명은 바뀌지 않는 식별자다. 표시 제목은 본문에서 얻는다.
/// `create_new`로 만들어 기존 파일을 덮어쓰지 않는다.
pub fn create_sheet(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.md", ulid::Ulid::generate()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(path)
}

/// 새 시트를 그룹의 수동 순서 마지막에 붙이고 `.hanji.toml`의 `sheets`에 쓴다.
///
/// 수동 정렬 그룹(프로젝트 원고 등)은 순서가 곧 원고 구조라서 새 시트가 늘 끝에 와야 한다.
/// 메타에 아직 없는 시트는 최근 수정순으로 뒤에 붙으므로, 그것들을 포함한 지금 목록을 그대로
/// 적어 새 시트만 맨 뒤로 옮긴다. 쓰기에 실패하면 메모리의 순서도 되돌린다.
pub fn append_sheet(group: &mut Group, path: &Path) -> io::Result<()> {
    let before = group.sheets.clone();
    group.sheets.retain(|sheet| sheet.path != path);
    group.sheets.push(super::sheet_from_path(path));
    write_order_and_glue(group, before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_util::{stem, temp_dir};
    use crate::library::*;

    #[test]
    fn note_group_is_created_in_notes_section() {
        let root = temp_dir("note-group");
        initialize(&root).unwrap();
        let notes = root.join(SectionKind::Notes.dir());
        let dir = create_group(&notes, "독서", Some("files"), Some("blue")).unwrap();
        assert_eq!(dir, notes.join("독서"));
        // 섹션 바로 아래 그룹은 이름순이라 섹션 폴더에 순서 메타를 쓰지 않는다.
        assert!(!notes.join(GROUP_META_FILE).exists());
        let lib = scan(&root).unwrap();
        let section = lib
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::Notes)
            .unwrap();
        let found = section.entries.iter().any(|entry| match entry {
            Entry::Group(group) => group.path == dir && group.icon.as_deref() == Some("files"),
            Entry::Sheet(_) => false,
        });
        assert!(found);

        // 이름을 바꿔도 섹션 폴더에 순서 메타를 쓰지 않는다.
        let renamed = update_group(&dir, "책", Some("book"), Some("red")).unwrap();
        assert_eq!(renamed, notes.join("책"));
        assert!(!dir.exists());
        assert!(!notes.join(GROUP_META_FILE).exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn update_project_keeps_sheet_order_and_renames() {
        let root = temp_dir("update-project");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", Some("book"), Some("blue")).unwrap();
        let sheet = create_sheet(&dir).unwrap();
        let file = sheet.file_name().unwrap().to_string_lossy().into_owned();
        write_meta_list(&dir, "sheets", std::slice::from_ref(&file)).unwrap();
        create_project(&projects, "에세이", None, None).unwrap();

        // 아이콘·색만 바꾸면 폴더는 그대로
        let same = update_project(&dir, "소설", Some("newspaper"), Some("red")).unwrap();
        assert_eq!(same, dir);
        // 이미 있는 이름으로 바꾸면 번호를 붙인다
        let renamed = update_project(&dir, "에세이", Some("newspaper"), Some("red")).unwrap();
        assert_eq!(renamed, projects.join("에세이 2"));
        assert!(!dir.exists());
        assert!(renamed.join(&file).is_file());

        let meta = read_group_meta(&renamed);
        assert_eq!(meta.icon.as_deref(), Some("newspaper"));
        assert_eq!(meta.color.as_deref(), Some("red"));
        assert_eq!(meta.sheets, vec![file]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn update_group_renames_in_place_and_keeps_reserved_folders() {
        let root = temp_dir("update-group");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();
        let first = create_group(&dir, "가", None, None).unwrap();
        create_group(&dir, "나", None, None).unwrap();

        // 이름을 바꿔도 순서 자리는 그대로다. `Extras`는 피한다.
        let renamed = update_group(&first, "하", Some("files"), Some("red")).unwrap();
        assert_eq!(renamed, dir.join("하"));
        assert_eq!(group_order(&dir).unwrap(), vec!["하", "나"]);
        let clash = update_group(&renamed, EXTRAS_DIR, None, None).unwrap();
        assert_eq!(clash, dir.join("Extras 2"));

        // 기본 그룹은 폴더를 두고 표시 이름만 바꾼다.
        let templates = dir.join(EXTRAS_DIR).join(TEMPLATES_GROUP);
        assert_eq!(
            update_group(&templates, "양식", None, Some("blue")).unwrap(),
            templates
        );
        let meta = read_group_meta(&templates);
        assert_eq!(meta.title.as_deref(), Some("양식"));
        assert_eq!(meta.icon, None);
        assert_eq!(meta.color.as_deref(), Some("blue"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn note_groups_avoid_attachments_name() {
        let root = temp_dir("note-attachments-name");
        initialize(&root).unwrap();
        let notes = root.join(SectionKind::Notes.dir());

        // 메모의 `Attachments`는 첨부파일 자리라 시트 그룹 이름으로 쓸 수 없다.
        let clash = create_group(&notes, ATTACHMENTS_GROUP, None, None).unwrap();
        assert_eq!(clash, notes.join("Attachments 2"));
        let diary = create_group(&notes, "일기", None, None).unwrap();
        let renamed = update_group(&diary, ATTACHMENTS_GROUP, None, None).unwrap();
        assert_eq!(renamed, notes.join("Attachments 2 2"));
        // 메모 그룹 아래나 프로젝트에서는 보통 이름이다.
        let nested = create_group(&renamed, ATTACHMENTS_GROUP, None, None).unwrap();
        assert_eq!(nested, renamed.join(ATTACHMENTS_GROUP));

        // 첨부파일 그룹은 폴더를 두고 표시 이름만 바꾼다.
        let attachments = notes.join(ATTACHMENTS_GROUP);
        crate::library::attach_bytes(&notes, b"png", "png").unwrap();
        assert!(is_reserved_group(&attachments));
        assert_eq!(
            update_group(&attachments, "그림", None, None).unwrap(),
            attachments
        );
        assert_eq!(read_group_meta(&attachments).title.as_deref(), Some("그림"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn project_groups_keep_creation_order_and_extras_stay_apart() {
        let root = temp_dir("project-groups");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let dir = create_project(&projects, "소설", None, None).unwrap();

        // 이름순과 다른 순서로 만들어도 만든 순서를 지킨다.
        let prologue = create_group(&dir, "프롤로그", Some("files"), None).unwrap();
        let front = create_group(&dir, "앞부속", Some("files"), None).unwrap();
        let part = create_group(&dir, "제1부", None, None).unwrap();
        let chapter = create_group(&part, "1장", None, None).unwrap();
        create_sheet(&chapter).unwrap();
        // 추가자료 자리 이름은 원고 그룹으로 쓸 수 없다.
        let clash = create_group(&dir, EXTRAS_DIR, None, None).unwrap();
        assert_eq!(clash, dir.join("Extras 2"));
        let people = create_group(
            &dir.join(EXTRAS_DIR),
            "등장인물",
            Some("folder"),
            Some("blue"),
        )
        .unwrap();

        let library = scan(&root).unwrap();
        let project = library.find_project(&dir).unwrap();
        let names: Vec<_> = project.groups.iter().map(|g| g.path.clone()).collect();
        assert_eq!(names, vec![prologue, front, part.clone(), clash]);
        assert_eq!(project.groups[2].groups[0].path, chapter);
        assert_eq!(library.find_group(&chapter).unwrap().sheets.len(), 1);

        // 새 프로젝트는 기본 추가자료(템플릿)를 갖고, 새 추가자료는 그 뒤에 붙는다.
        let extras: Vec<_> = project.extras.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(extras, vec!["템플릿", "등장인물"]);
        assert_eq!(
            library.find_group(&people).unwrap().icon.as_deref(),
            Some("folder")
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_adds_extras_to_existing_project_once() {
        let root = temp_dir("project-extras");
        initialize(&root).unwrap();
        let dir = root.join(SectionKind::Projects.dir()).join("옛 프로젝트");
        std::fs::create_dir_all(&dir).unwrap();

        open(&root).unwrap();
        let templates = dir.join(EXTRAS_DIR).join(TEMPLATES_GROUP);
        assert!(templates.is_dir());

        // 사용자가 지운 기본 그룹은 다시 만들지 않는다.
        std::fs::remove_dir_all(&templates).unwrap();
        let library = open(&root).unwrap();
        assert!(!templates.exists());
        assert!(library.find_project(&dir).unwrap().extras.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn create_project_writes_meta_and_avoids_name_clash() {
        let root = temp_dir("project");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());

        let first = create_project(&projects, " 소설/1부 ", Some("book"), Some("blue")).unwrap();
        assert_eq!(first, projects.join("소설-1부"));
        let second = create_project(&projects, "소설-1부", None, None).unwrap();
        assert_eq!(second, projects.join("소설-1부 2"));
        assert_eq!(
            create_project(&projects, "  ", None, None).unwrap(),
            projects.join("무제")
        );

        let library = scan(&root).unwrap();
        let groups: Vec<_> = library
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::Projects)
            .unwrap()
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Group(group) => Some(group),
                _ => None,
            })
            .collect();
        let novel = groups.iter().find(|group| group.path == first).unwrap();
        assert_eq!(novel.icon.as_deref(), Some("book"));
        assert_eq!(novel.color.as_deref(), Some("blue"));
        assert_eq!(groups.len(), 3);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_reads_group_icon_from_meta() {
        let root = temp_dir("icon");
        initialize(&root).unwrap();

        let library = scan(&root).unwrap();
        let notes = library
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::Notes)
            .unwrap();
        let inbox = notes
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Group(group) if group.name == DEFAULT_INBOX => Some(group),
                _ => None,
            })
            .expect("Inbox 그룹이 있어야 한다");
        assert_eq!(inbox.icon.as_deref(), Some("inbox"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_applies_group_meta_sheet_order() {
        let root = temp_dir("order");
        initialize(&root).unwrap();

        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        for stem in ["a", "b", "c", "d"] {
            std::fs::write(inbox.join(format!("{stem}.md")), stem).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // 메타 순서: c, a (b·d는 미나열 -> 뒤에 최근 수정순: d, b)
        std::fs::write(
            inbox.join(GROUP_META_FILE),
            "icon = \"inbox\"\nsheets = [\"c.md\", \"a.md\"]\n",
        )
        .unwrap();

        let library = scan(&root).unwrap();
        let notes = library
            .sections
            .iter()
            .find(|section| section.kind == SectionKind::Notes)
            .unwrap();
        let group = notes
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Group(group) if group.name == DEFAULT_INBOX => Some(group),
                _ => None,
            })
            .unwrap();
        let titles: Vec<_> = group.sheets.iter().map(stem).collect();
        assert_eq!(titles, vec!["c", "a", "d", "b"]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_sheet_writes_order_and_keeps_meta() {
        let root = temp_dir("move");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        for stem in ["a", "b", "c"] {
            std::fs::write(inbox.join(format!("{stem}.md")), stem).unwrap();
        }
        std::fs::write(
            inbox.join(GROUP_META_FILE),
            "icon = \"inbox\"\nsheets = [\"a.md\", \"b.md\", \"c.md\"]\n",
        )
        .unwrap();

        let mut group = scan_group(&inbox, false).unwrap();
        move_sheet(&mut group, 0, 2).unwrap();
        let moved: Vec<_> = group.sheets.iter().map(stem).collect();
        assert_eq!(moved, vec!["b", "c", "a"]);

        // 다시 읽어도 같은 순서, 아이콘은 그대로
        let rescanned = scan_group(&inbox, false).unwrap();
        let titles: Vec<_> = rescanned.sheets.iter().map(stem).collect();
        assert_eq!(titles, vec!["b", "c", "a"]);
        assert_eq!(rescanned.icon.as_deref(), Some("inbox"));
        assert!(!inbox.join(format!("{GROUP_META_FILE}.tmp")).exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn glue_and_unglue_sheets() {
        let root = temp_dir("glue");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        for stem in ["a", "b", "c", "d"] {
            std::fs::write(inbox.join(format!("{stem}.md")), stem).unwrap();
        }
        std::fs::write(
            inbox.join(GROUP_META_FILE),
            "sheets = [\"a.md\", \"b.md\", \"c.md\", \"d.md\"]\n",
        )
        .unwrap();
        let glued =
            |group: &Group| -> Vec<bool> { group.sheets.iter().map(|sheet| sheet.glued).collect() };

        let mut group = scan_group(&inbox, false).unwrap();
        glue_sheets(&mut group, 0..3).unwrap();
        assert_eq!(glued(&group), vec![false, true, true, false]);
        assert_eq!(read_group_meta(&inbox).glued, vec!["b.md", "c.md"]);
        assert_eq!(glued(&scan_group(&inbox, false).unwrap()), glued(&group));

        // 가운데 시트를 떼면 앞뒤 모두 떨어진다.
        unglue_sheets(&mut group, 1..2).unwrap();
        assert_eq!(glued(&group), vec![false, false, false, false]);
        let text = std::fs::read_to_string(inbox.join(GROUP_META_FILE)).unwrap();
        assert!(!text.contains("glued"), "{text}");

        // 접합 내 이동은 유지, 외부로 빼면 해당 시트만 해제, 내부로 넣으면 접합한다.
        glue_sheets(&mut group, 0..3).unwrap(); // [a b c] d
        let initial = group.sheets.clone();
        let order = |group: &Group| -> Vec<String> { group.sheets.iter().map(stem).collect() };
        for (from, to, expected_order, expected_glue) in [
            (0, 2, ["b", "c", "a", "d"], [false, true, true, false]),
            (1, 0, ["b", "a", "c", "d"], [false, true, true, false]),
            (1, 3, ["a", "c", "d", "b"], [false, true, false, false]),
            (0, 3, ["b", "c", "d", "a"], [false, true, false, false]),
            (2, 3, ["a", "b", "d", "c"], [false, true, false, false]),
            (3, 0, ["d", "a", "b", "c"], [false, false, true, true]),
            (3, 1, ["a", "d", "b", "c"], [false, true, true, true]),
        ] {
            group.sheets = initial.clone();
            move_sheet(&mut group, from, to).unwrap();
            assert_eq!(order(&group), expected_order, "{from} → {to}");
            assert_eq!(glued(&group), expected_glue, "{from} → {to}");
            let rescanned = scan_group(&inbox, false).unwrap();
            assert_eq!(order(&rescanned), order(&group));
            assert_eq!(glued(&rescanned), glued(&group));
        }

        // 다른 접합으로 옮겨도 남은 시트들의 접합은 유지된다.
        group.sheets = initial;
        unglue_sheets(&mut group, 0..4).unwrap();
        glue_sheets(&mut group, 0..2).unwrap(); // [a b] c d
        glue_sheets(&mut group, 2..4).unwrap(); // [a b] [c d]
        move_sheet(&mut group, 1, 2).unwrap(); // a [c b d]
        assert_eq!(order(&group), vec!["a", "c", "b", "d"]);
        assert_eq!(glued(&group), vec![false, false, true, true]);
        let rescanned = scan_group(&inbox, false).unwrap();
        assert_eq!(order(&rescanned), order(&group));
        assert_eq!(glued(&rescanned), glued(&group));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn forgetting_a_glue_head_starts_the_group_at_the_next_sheet() {
        let root = temp_dir("forget-sheets");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        for stem in ["a", "b", "c", "d", "e"] {
            std::fs::write(inbox.join(format!("{stem}.md")), stem).unwrap();
        }
        std::fs::write(
            inbox.join(GROUP_META_FILE),
            "sheets = [\"a.md\", \"b.md\", \"c.md\", \"d.md\", \"e.md\"]\n",
        )
        .unwrap();
        let mut group = scan_group(&inbox, false).unwrap();
        glue_sheets(&mut group, 1..5).unwrap(); // a [b c d e]
        let glued = |group: &Group| {
            group
                .sheets
                .iter()
                .map(|sheet| sheet.glued)
                .collect::<Vec<_>>()
        };
        let remove = |group: &mut Group, stems: &[&str]| {
            let paths: Vec<PathBuf> = stems
                .iter()
                .map(|stem| inbox.join(format!("{stem}.md")))
                .collect();
            for path in &paths {
                std::fs::remove_file(path).unwrap();
            }
            forget_sheets(group, &paths).unwrap();
        };

        // 묶음 가운데를 지우면 앞뒤가 그대로 이어진다: a [b d e]
        remove(&mut group, &["c"]);
        assert_eq!(glued(&group), [false, false, true, true]);
        // 묶음의 처음(과 그다음)을 지우면 남은 시트부터 묶음이다. e는 a에 붙지 않는다: a e
        remove(&mut group, &["b", "d"]);
        assert_eq!(glued(&group), [false, false]);
        let rescanned = scan_group(&inbox, false).unwrap();
        assert_eq!(glued(&rescanned), glued(&group));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_selected_sheets_keeps_glue_and_writes_order() {
        let root = temp_dir("move-selected");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        for stem in ["a", "b", "c", "d"] {
            std::fs::write(inbox.join(format!("{stem}.md")), stem).unwrap();
        }
        std::fs::write(
            inbox.join(GROUP_META_FILE),
            "sheets = [\"a.md\", \"b.md\", \"c.md\", \"d.md\"]\n",
        )
        .unwrap();

        let mut group = scan_group(&inbox, false).unwrap();
        glue_sheets(&mut group, 0..3).unwrap(); // [a b c] d
        move_selected_sheets(&mut group, &[0, 1], 0, 3).unwrap();

        let titles: Vec<_> = group.sheets.iter().map(stem).collect();
        assert_eq!(titles, vec!["c", "d", "a", "b"]);
        let glued: Vec<_> = group.sheets.iter().map(|sheet| sheet.glued).collect();
        assert_eq!(glued, vec![false, false, false, true]);

        let rescanned = scan_group(&inbox, false).unwrap();
        assert_eq!(
            rescanned.sheets.iter().map(stem).collect::<Vec<_>>(),
            titles
        );
        assert_eq!(
            rescanned
                .sheets
                .iter()
                .map(|sheet| sheet.glued)
                .collect::<Vec<_>>(),
            glued
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_sheets_between_groups_moves_files_and_keeps_glue() {
        let root = temp_dir("move-between");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        for stem in ["a", "b", "c", "d"] {
            std::fs::write(inbox.join(format!("{stem}.md")), stem).unwrap();
        }
        std::fs::write(
            inbox.join(GROUP_META_FILE),
            "sheets = [\"a.md\", \"b.md\", \"c.md\", \"d.md\"]\n",
        )
        .unwrap();

        let other = root.join("Notes").join("Other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("x.md"), "x").unwrap();
        // 같은 이름의 파일이 있으면 번호를 붙여 옮긴다.
        std::fs::write(other.join("b.md"), "다른 b").unwrap();
        std::fs::write(
            other.join(GROUP_META_FILE),
            "sheets = [\"x.md\", \"b.md\"]\n",
        )
        .unwrap();

        let library = scan(&root).unwrap();
        let mut source = library.find_group(&inbox).unwrap().clone();
        let mut target = library.find_group(&other).unwrap().clone();
        // [a b c] d 에서 b, c를 옮긴다.
        source.sheets[1].glued = true;
        source.sheets[2].glued = true;

        let moves = move_sheets_between(&mut source, &mut target, &[1, 2]).unwrap();
        assert_eq!(moves.len(), 2);
        assert_eq!(
            source.sheets.iter().map(stem).collect::<Vec<_>>(),
            vec!["a", "d"]
        );
        assert_eq!(
            target.sheets.iter().map(stem).collect::<Vec<_>>(),
            vec!["x", "b", "b 2", "c"]
        );
        assert!(source.sheets.iter().all(|sheet| !sheet.glued));
        assert_eq!(
            target
                .sheets
                .iter()
                .map(|sheet| sheet.glued)
                .collect::<Vec<_>>(),
            vec![false, false, false, true]
        );
        assert!(!inbox.join("b.md").exists());
        assert!(!inbox.join("c.md").exists());
        assert!(other.join("b 2.md").is_file());
        assert!(other.join("c.md").is_file());

        // 다시 읽어도 파일과 메타가 같은 순서·접합을 가리킨다.
        let rescanned = scan(&root).unwrap();
        let source = rescanned.find_group(&inbox).unwrap();
        let target = rescanned.find_group(&other).unwrap();
        assert_eq!(
            source.sheets.iter().map(stem).collect::<Vec<_>>(),
            vec!["a", "d"]
        );
        assert_eq!(
            target.sheets.iter().map(stem).collect::<Vec<_>>(),
            vec!["x", "b", "b 2", "c"]
        );
        assert_eq!(
            target
                .sheets
                .iter()
                .map(|sheet| sheet.glued)
                .collect::<Vec<_>>(),
            vec![false, false, false, true]
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn set_sort_persists_and_keeps_other_keys() {
        let root = temp_dir("sort");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);

        let mut group = scan_group(&inbox, false).unwrap();
        assert_eq!(group.sort, None);
        set_sort(&mut group, SortMode::Title).unwrap();
        assert_eq!(group.sort, Some(SortMode::Title));

        set_sort_descending(&mut group, true).unwrap();

        let rescanned = scan_group(&inbox, false).unwrap();
        assert_eq!(rescanned.sort, Some(SortMode::Title));
        assert_eq!(rescanned.descending, Some(true));
        assert_eq!(rescanned.icon.as_deref(), Some("inbox"));

        // 방법을 바꾸면 방향은 그 방법의 기본으로 돌아간다.
        set_sort(&mut group, SortMode::Created).unwrap();
        assert_eq!(scan_group(&inbox, false).unwrap().descending, None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_group_moves_folder_and_updates_orders() {
        let root = temp_dir("move-group");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let project = create_project(&projects, "소설", None, None).unwrap();
        let target = create_group(&project, "1부", None, None).unwrap();
        let source = create_group(&project, "장면", None, None).unwrap();
        let sheet = create_sheet(&source).unwrap();
        std::fs::write(&sheet, "본문").unwrap();

        let moved = move_group(&source, &target).unwrap();
        assert_eq!(moved, target.join("장면"));
        assert!(!source.exists());
        assert!(moved.join(sheet.file_name().unwrap()).is_file());

        // 원래 부모의 순서에서 빠지고, 대상 그룹의 순서 끝에 붙는다.
        assert_eq!(read_group_meta(&project).groups, vec!["1부"]);
        assert_eq!(read_group_meta(&target).groups, vec!["장면"]);

        let library = scan(&root).unwrap();
        let project = library.find_project(&project).unwrap();
        assert_eq!(project.groups.len(), 1);
        let target = library.find_group(&target).unwrap();
        assert_eq!(target.groups.len(), 1);
        assert_eq!(target.groups[0].name, "장면");
        assert_eq!(target.groups[0].sheets.len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_group_beside_reorders_and_moves_up_a_level() {
        let root = temp_dir("move-group-beside");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let project = create_project(&projects, "소설", None, None).unwrap();
        let a = create_group(&project, "가", None, None).unwrap();
        let b = create_group(&project, "나", None, None).unwrap();
        let c = create_group(&project, "다", None, None).unwrap();
        let child = create_group(&a, "하위", None, None).unwrap();

        // 같은 부모: 폴더는 그대로, 순서만 바뀐다.
        assert_eq!(move_group_beside(&c, &a, false).unwrap(), c);
        assert_eq!(read_group_meta(&project).groups, vec!["다", "가", "나"]);
        assert_eq!(move_group_beside(&c, &b, true).unwrap(), c);
        assert_eq!(read_group_meta(&project).groups, vec!["가", "나", "다"]);

        // 하위 그룹을 위 레벨로 꺼내 `나` 뒤에 둔다.
        let moved = move_group_beside(&child, &b, true).unwrap();
        assert_eq!(moved, project.join("하위"));
        assert!(!child.exists());
        assert_eq!(
            read_group_meta(&project).groups,
            vec!["가", "나", "하위", "다"]
        );
        assert!(read_group_meta(&a).groups.is_empty());

        let library = scan(&root).unwrap();
        let names: Vec<_> = library
            .find_project(&project)
            .unwrap()
            .groups
            .iter()
            .map(|group| group.name.clone())
            .collect();
        assert_eq!(names, vec!["가", "나", "하위", "다"]);

        // 자기 하위 옆으로는 옮길 수 없다.
        let inner = create_group(&b, "속", None, None).unwrap();
        assert!(move_group_beside(&b, &inner, false).is_err());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn projects_follow_section_meta_order() {
        let root = temp_dir("project-order");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let first = create_project(&projects, "가", None, None).unwrap();
        create_project(&projects, "나", None, None).unwrap();
        move_group_beside(&first, &projects.join("나"), true).unwrap();

        let library = scan(&root).unwrap();
        let names: Vec<_> = library
            .section(SectionKind::Projects)
            .unwrap()
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Group(group) => Some(group.name.clone()),
                _ => None,
            })
            .collect();
        let at = |name: &str| names.iter().position(|n| n == name).unwrap();
        assert!(at("나") < at("가"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn move_group_avoids_name_clash_and_cycles() {
        let root = temp_dir("move-group-clash");
        initialize(&root).unwrap();
        let projects = root.join(SectionKind::Projects.dir());
        let project = create_project(&projects, "소설", None, None).unwrap();
        let target = create_group(&project, "1부", None, None).unwrap();
        let source = create_group(&project, "장면", None, None).unwrap();
        create_group(&target, "장면", None, None).unwrap();

        let moved = move_group(&source, &target).unwrap();
        assert_eq!(moved, target.join("장면 2"));

        // 자기 자신이나 자기 하위로는 옮길 수 없다.
        assert!(move_group(&target, &target).is_err());
        assert!(move_group(&target, &moved).is_err());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn append_sheet_puts_new_sheet_last_and_keeps_glue() {
        let root = temp_dir("append-sheet");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);
        let name = |path: &Path| path.file_stem().unwrap().to_string_lossy().into_owned();

        // 메타 순서가 없는 수동 그룹은 최근 수정순이라 나중에 만든 시트가 앞에 온다.
        // append_sheet는 그 시트를 목록 끝으로 옮기고 순서를 메타에 적는다.
        let first = create_sheet(&inbox).unwrap();
        let second = create_sheet(&inbox).unwrap();
        let mut group = scan_group(&inbox, false).unwrap();
        assert_eq!(group.sheets[0].path, second);
        append_sheet(&mut group, &second).unwrap();
        let titles: Vec<_> = group.sheets.iter().map(stem).collect();
        assert_eq!(titles, vec![name(&first), name(&second)]);

        // 접합이 있는 그룹에 붙인 시트는 맨 뒤에서 새 묶음으로 시작한다.
        glue_sheets(&mut group, 0..2).unwrap();
        let third = create_sheet(&inbox).unwrap();
        let mut group = scan_group(&inbox, false).unwrap();
        append_sheet(&mut group, &third).unwrap();
        let titles: Vec<_> = group.sheets.iter().map(stem).collect();
        assert_eq!(titles, vec![name(&first), name(&second), name(&third)]);
        assert_eq!(
            group
                .sheets
                .iter()
                .map(|sheet| sheet.glued)
                .collect::<Vec<_>>(),
            vec![false, true, false]
        );

        // 다시 읽어도 같은 순서·접합이고, 다른 메타 키는 그대로다.
        let rescanned = scan_group(&inbox, false).unwrap();
        let titles: Vec<_> = rescanned.sheets.iter().map(stem).collect();
        assert_eq!(titles, vec![name(&first), name(&second), name(&third)]);
        assert_eq!(rescanned.icon.as_deref(), Some("inbox"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn create_sheet_picks_unique_empty_file() {
        let root = temp_dir("create");
        initialize(&root).unwrap();
        let inbox = root.join("Notes").join(DEFAULT_INBOX);

        let first = create_sheet(&inbox).unwrap();
        let second = create_sheet(&inbox).unwrap();
        assert_ne!(first, second);
        for path in [&first, &second] {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            assert!(stem.parse::<ulid::Ulid>().is_ok(), "{stem}");
        }

        assert!(read_preview(&first).is_empty());

        std::fs::remove_dir_all(&root).ok();
    }
}
