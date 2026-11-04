//! 프로젝트 내보내기: 프로젝트 폴더 전체를 ZIP으로 묶어 `.hanji` 파일 하나로 만든다.
//!
//! 압축 파일 안에는 프로젝트 폴더 자체가 아니라 그 내용이 루트부터 들어간다. 그래야 나중에
//! 풀었을 때 `<Project>/…`가 바로 생기고, 폴더 이름을 바꿔도 안의 구조가 그대로다.
//! 숨김 파일(`.hanji.toml` 등)도 프로젝트 데이터이므로 함께 담는다.

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use super::ExportError;

/// 프로젝트 폴더 전체를 ZIP으로 묶어 `output_path`에 쓴다.
///
/// 쓰는 도중 실패해도 반쪽짜리 `.hanji` 파일이 남지 않도록 임시 파일에 쓴 뒤 바꿔 넣는다.
/// 내보내는 위치가 프로젝트 폴더 안이어도 임시 파일이 아카이브에 섞이지 않는다.
pub fn export_to_file(project_dir: &Path, output_path: &Path) -> Result<(), ExportError> {
    if !project_dir.is_dir() {
        return Err(ExportError::ProjectNotFound(project_dir.to_path_buf()));
    }

    let parent = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".hanji-export-{}.tmp", ulid::Ulid::generate()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        write_archive(project_dir, &mut writer, Some(&temporary))?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temporary, output_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// 프로젝트 폴더 내용을 ZIP 스트림으로 쓴다. 폴더 자체는 넣지 않고 안의 항목부터 루트로 둔다.
pub fn build<W: Write + Seek>(project_dir: &Path, writer: &mut W) -> Result<(), ExportError> {
    write_archive(project_dir, writer, None)
}

fn write_archive<W: Write + Seek>(
    project_dir: &Path,
    writer: &mut W,
    exclude: Option<&Path>,
) -> Result<(), ExportError> {
    if !project_dir.is_dir() {
        return Err(ExportError::ProjectNotFound(project_dir.to_path_buf()));
    }

    let mut zip = zip::ZipWriter::new(writer);
    let file_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let dir_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755);

    add_dir_contents(
        &mut zip,
        project_dir,
        Path::new(""),
        file_options,
        dir_options,
        exclude,
    )?;
    zip.finish()?;
    Ok(())
}

/// `dir`의 항목을 `prefix` 아래 이름으로 넣는다. 폴더는 먼저 빈 항목으로 넣어 빈 폴더도 남긴다.
fn add_dir_contents<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &Path,
    prefix: &Path,
    file_options: SimpleFileOptions,
    dir_options: SimpleFileOptions,
    exclude: Option<&Path>,
) -> Result<(), ExportError> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if exclude.is_some_and(|exclude| path == exclude) {
            continue;
        }
        let name = prefix.join(entry.file_name());
        let zip_name = zip_name(&name);

        // 심볼릭 링크 폴더는 따라가지 않는다(순환·프로젝트 밖 내용을 막는다).
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            match fs::metadata(&path) {
                Ok(target) if target.is_file() => {
                    zip.start_file(&zip_name, file_options)?;
                    zip.write_all(&fs::read(&path)?)?;
                }
                _ => continue,
            }
            continue;
        }

        if metadata.is_dir() {
            zip.add_directory(format!("{zip_name}/"), dir_options)?;
            add_dir_contents(zip, &path, &name, file_options, dir_options, exclude)?;
        } else if metadata.is_file() {
            zip.start_file(&zip_name, file_options)?;
            zip.write_all(&fs::read(&path)?)?;
        }
    }
    Ok(())
}

/// ZIP 항목 이름은 플랫폼과 상관없이 `/` 구분자를 쓴다.
fn zip_name(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// 내보낼 파일 이름에 `.hanji` 확장자를 붙인다. 이미 붙어 있으면 그대로 둔다.
pub fn with_hanji_extension(path: &Path) -> PathBuf {
    let has_extension = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("hanji"));
    if has_extension {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_os_string();
        name.push(".hanji");
        PathBuf::from(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{Cursor, Read};

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "hanji-export-project-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn archive_contains_project_contents_at_root() {
        let root = temp_dir("build");
        let project = root.join("내 책");
        fs::create_dir_all(project.join("1부").join("빈 폴더")).unwrap();
        fs::create_dir_all(project.join("Extras").join("Attachments")).unwrap();
        fs::write(project.join(".hanji.toml"), "[book]\ntitle = \"내 책\"\n").unwrap();
        fs::write(project.join("1부").join("첫 장.md"), "# 첫 장\n").unwrap();
        fs::write(
            project.join("Extras").join("Attachments").join("cover.png"),
            b"png",
        )
        .unwrap();

        let mut bytes = Vec::new();
        build(&project, &mut Cursor::new(&mut bytes)).unwrap();

        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<_> = (0..zip.len())
            .map(|index| zip.by_index(index).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&".hanji.toml".to_string()));
        assert!(names.contains(&"1부/첫 장.md".to_string()));
        assert!(names.contains(&"1부/".to_string()));
        assert!(names.contains(&"1부/빈 폴더/".to_string()));
        assert!(names.contains(&"Extras/Attachments/cover.png".to_string()));
        // 프로젝트 폴더 이름은 아카이브 루트에 들어가지 않는다.
        assert!(!names.iter().any(|name| name.starts_with("내 책/")));

        let mut content = String::new();
        zip.by_name("1부/첫 장.md")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "# 첫 장\n");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn export_skips_its_temporary_file_inside_project() {
        let root = temp_dir("temporary");
        let project = root.join("Project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("sheet.md"), "글").unwrap();

        let output = project.join("Project.hanji");
        export_to_file(&project, &output).unwrap();
        assert!(output.is_file());

        let file = File::open(&output).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let names: Vec<_> = (0..zip.len())
            .map(|index| zip.by_index(index).unwrap().name().to_string())
            .collect();
        assert_eq!(names, vec!["sheet.md".to_string()]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hanji_extension_is_added_once() {
        assert_eq!(
            with_hanji_extension(Path::new("책")),
            PathBuf::from("책.hanji")
        );
        assert_eq!(
            with_hanji_extension(Path::new("책.hanji")),
            PathBuf::from("책.hanji")
        );
        assert_eq!(
            with_hanji_extension(Path::new("책.HANJI")),
            PathBuf::from("책.HANJI")
        );
        assert_eq!(
            with_hanji_extension(Path::new("책.zip")),
            PathBuf::from("책.zip.hanji")
        );
    }
}
