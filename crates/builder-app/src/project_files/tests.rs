use super::*;

fn fixture() -> (tempfile::TempDir, ValidatedManifest, ValidatedManifest) {
    let directory = tempfile::tempdir().unwrap();
    let mut root = LockedRoot::open(directory.path()).unwrap();
    let original = builder_project::ProjectDocument::new("Original".into());
    let mut draft = original.clone();
    draft.name = "Draft".into();
    let original = builder_project::validate_document(original).unwrap();
    let draft = builder_project::validate_document(draft).unwrap();
    root.create(&original).unwrap();
    (directory, original, draft)
}

#[test]
fn failure_after_staging_preserves_valid_manifest_and_backup() {
    let (directory, original, draft) = fixture();
    let mut root = LockedRoot::open(directory.path()).unwrap();
    let current = root.read().unwrap();
    root.before_publish = Some(Box::new(|| {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "fixture"))
    }));
    assert!(matches!(
        root.replace(&current, Some(&original), &draft),
        Err(FileError::Unavailable)
    ));
    assert_eq!(root.read().unwrap().revision, original.revision());
    assert_eq!(
        read_backup(&root.path, original.revision())
            .unwrap()
            .bytes(),
        original.bytes()
    );
    let entries = fs::read_dir(&root.path).unwrap().filter_map(Result::ok);
    assert!(!entries.into_iter().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGE_PREFIX)
    }));
    assert!(root.replace(&current, Some(&original), &draft).unwrap());
    assert_eq!(root.read().unwrap().revision, draft.revision());
}

#[test]
fn external_edit_while_staged_is_rechecked_before_replace() {
    let (directory, original, draft) = fixture();
    let mut root = LockedRoot::open(directory.path()).unwrap();
    let current = root.read().unwrap();
    let path = root.path.join(MANIFEST_NAME);
    let mut external = original.bytes().to_vec();
    external.push(b' ');
    let revision = ManifestRevision::of(&external);
    root.before_publish = Some(Box::new(move || fs::write(path, external)));
    let result = root.replace(&current, Some(&original), &draft);
    assert!(matches!(result, Err(FileError::Conflict(Some(actual))) if actual == revision));
    assert_eq!(root.read().unwrap().revision, revision);
    assert_eq!(
        read_backup(&root.path, original.revision())
            .unwrap()
            .bytes(),
        original.bytes()
    );
}

#[test]
fn native_lock_excludes_a_second_process_and_is_released_on_exit() {
    const CHILD_ROOT: &str = "MCP_BUILDER_M1_LOCK_FIXTURE";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let path = PathBuf::from(root).join(LOCK_NAME);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        assert!(matches!(
            file.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        return;
    }
    let (directory, _, _) = fixture();
    let held = LockedRoot::open(directory.path()).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "project_files::tests::native_lock_excludes_a_second_process_and_is_released_on_exit",
        ])
        .env(CHILD_ROOT, &held.path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    drop(held);
    assert!(LockedRoot::open(directory.path()).is_ok());
}

#[cfg(windows)]
#[test]
fn replacement_denied_by_an_open_windows_handle_preserves_both_revisions() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let (directory, original, draft) = fixture();
    let mut root = LockedRoot::open(directory.path()).unwrap();
    let current = root.read().unwrap();
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(root.path.join(MANIFEST_NAME))
        .unwrap();
    assert!(matches!(
        root.replace(&current, Some(&original), &draft),
        Err(FileError::Unavailable)
    ));
    assert_eq!(root.read().unwrap().revision, original.revision());
    assert_eq!(
        read_backup(&root.path, original.revision())
            .unwrap()
            .bytes(),
        original.bytes()
    );
    drop(handle);
    assert!(root.replace(&current, Some(&original), &draft).unwrap());
}
