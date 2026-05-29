use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let temp_path = temp_path_for(path);
    let write_result =
        write_temp_file(&temp_path, text.as_bytes()).and_then(|()| replace_file(&temp_path, path));
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn write_temp_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("failed to create temp config {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("failed to write temp config {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("failed to sync temp config {}: {error}", path.display()))
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()))
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(format!(
            "failed to replace config {}: {}",
            String::from_utf16_lossy(&to[..to.len().saturating_sub(1)]),
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|error| format!("failed to replace {}: {error}", to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("kmsync-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn atomic_write_creates_parent_directories_and_file() {
        let root = unique_test_dir("atomic-create");
        let path = root.join("nested").join("profile.json");

        write_text_atomic(&path, "{\"ok\":true}").expect("write config");

        assert_eq!(
            fs::read_to_string(&path).expect("read config"),
            "{\"ok\":true}"
        );
        assert!(fs::read_dir(path.parent().expect("parent"))
            .expect("read parent")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp")));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let root = unique_test_dir("atomic-replace");
        let path = root.join("profile.json");
        fs::create_dir_all(&root).expect("create root");
        fs::write(&path, "old").expect("write old");

        write_text_atomic(&path, "new").expect("replace config");

        assert_eq!(fs::read_to_string(&path).expect("read config"), "new");
        assert!(fs::read_dir(&root).expect("read root").all(|entry| !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .contains(".tmp")));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
