use std::fs::{self, File};

use super::*;

#[test]
fn rejects_unsafe_names_and_collisions_before_staging() {
    for paths in [
        vec!["../outside"],
        vec!["sub\\file"],
        vec!["/rooted"],
        vec!["aux.txt"],
        vec!["notes.txt:alternate"],
        vec!["notes.txt."],
        vec!["sub//file"],
        vec!["foo", "foo/file"],
        vec!["Readme.md", "README.md"],
        vec!["A/x", "a/y"],
        vec!["café/a", "cafe\u{301}/b"],
        vec!["data", "data"],
        vec!["target/debug/out"],
        vec![".GIT"],
        vec![".eNv.DeV"],
        vec!["runtime-config.LOCAL.json"],
        vec![".MCP-Export-Stage-example/a"],
    ] {
        assert!(
            matches!(
                validate_paths(paths.iter().copied()),
                Err(ExportError::UnsafePath)
            ),
            "{paths:?}"
        );
    }
    validate_paths([
        "src/main.rs",
        "src/other.rs",
        ".gitignore",
        "runtime-config.example.json",
    ])
    .unwrap();
}

#[test]
fn stages_every_file_before_recheck_and_publishes_complete_tree() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    let checked = check_destination(&destination).unwrap();
    let files = fixture();
    publish(&checked, &files, || {
        assert!(!destination.exists());
        let entry = fs::read_dir(parent.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert!(
            entry
                .file_name()
                .to_str()
                .unwrap()
                .starts_with(STAGE_PREFIX)
        );
        for (name, bytes) in &files {
            assert_eq!(&fs::read(entry.path().join(name)).unwrap(), bytes);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(read_tree(&destination).unwrap(), files);
    assert_eq!(checked_root(&destination).unwrap(), checked);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[test]
fn existing_file_or_directory_is_a_collision() {
    let parent = tempfile::tempdir().unwrap();
    let file = parent.path().join("file");
    let directory = parent.path().join("directory");
    fs::write(&file, b"prior").unwrap();
    fs::create_dir(&directory).unwrap();
    for destination in [&file, &directory] {
        assert!(matches!(
            check_destination(destination),
            Err(ExportError::DestinationExists)
        ));
        assert!(matches!(
            publish(destination, &fixture(), || panic!("unexpected recheck")),
            Err(ExportError::DestinationExists)
        ));
    }
    assert_eq!(fs::read(file).unwrap(), b"prior");
    assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 2);
}

#[test]
fn a_destination_created_during_recheck_wins_without_losing_any_bytes() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    let result = publish(&destination, &fixture(), || {
        fs::create_dir(&destination).unwrap();
        Ok(())
    });
    assert!(matches!(result, Err(ExportError::DestinationExists)));
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[test]
fn failed_recheck_cleans_up_and_a_retry_publishes() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    let prior = parent.path().join("old");
    fs::create_dir(&prior).unwrap();
    fs::write(prior.join("custom.txt"), b"prior").unwrap();
    let result = publish(&destination, &fixture(), || Err(ExportError::UnsafePath));
    assert!(matches!(result, Err(ExportError::UnsafePath)));
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
    assert!(!destination.exists());
    assert_eq!(fs::read(prior.join("custom.txt")).unwrap(), b"prior");

    let files = fixture();
    publish(&destination, &files, || Ok(())).unwrap();
    assert_eq!(read_tree(&destination).unwrap(), files);
}

#[test]
fn inventory_skips_local_metadata_and_keeps_nested_sources() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    let mut files = fixture();
    files.insert("sub/target/keep.txt".into(), b"source".to_vec());
    files.insert("sub/.env".into(), b"nested fixture".to_vec());
    publish(&destination, &files, || Ok(())).unwrap();
    fs::create_dir(destination.join("target")).unwrap();
    fs::write(destination.join("target/build"), b"ignored").unwrap();
    for path in [".git", ".ENV.Local", "Runtime-Config.json"] {
        fs::write(destination.join(path), b"ignored").unwrap();
    }
    assert_eq!(read_tree(&destination).unwrap(), files);
}

#[test]
fn rejects_linked_files_and_bounds_reads() {
    let parent = tempfile::tempdir().unwrap();
    let destination = parent.path().join("export");
    fs::create_dir(&destination).unwrap();
    let manifest = parent.path().join("project.json");
    fs::write(&manifest, b"synthetic").unwrap();
    assert!(matches!(
        read_file(&manifest, 1),
        Err(ExportError::InputTooLarge)
    ));
    assert_eq!(read_file(&manifest, 9).unwrap(), b"synthetic");
    let large = File::create(destination.join("large")).unwrap();
    large.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
    drop(large);
    assert!(matches!(
        read_tree(&destination),
        Err(ExportError::InputTooLarge)
    ));
    fs::remove_file(destination.join("large")).unwrap();

    fs::hard_link(&manifest, destination.join("alias.json")).unwrap();
    assert!(matches!(
        read_tree(&destination),
        Err(ExportError::UnsafePath)
    ));
    assert!(matches!(
        read_file(&manifest, 9),
        Err(ExportError::UnsafePath)
    ));
}

#[test]
fn refuses_linked_parent_and_does_not_follow_linked_children() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    let export = temp.path().join("export");
    let link = export.join("target");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&export).unwrap();
    fs::write(outside.join("project.json"), b"fixture").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    {
        // A local NTFS junction exercises reparse-point rejection without
        // requiring Developer Mode or a symlink privilege.
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
    }
    assert!(matches!(read_tree(&export), Err(ExportError::UnsafePath)));
    assert!(matches!(read_tree(&link), Err(ExportError::UnsafePath)));
    assert!(matches!(
        read_file(&link.join("project.json"), 8),
        Err(ExportError::UnsafePath)
    ));
    assert!(matches!(
        publish(&link.join("new"), &fixture(), || panic!(
            "unexpected recheck"
        )),
        Err(ExportError::UnsafePath)
    ));
    assert!(!outside.join("new").exists());
    assert_eq!(fs::read(outside.join("project.json")).unwrap(), b"fixture");
}

fn fixture() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("Cargo.toml".to_owned(), b"[workspace]\n".to_vec()),
        ("src/café.rs".to_owned(), b"fn main() {}\n".to_vec()),
    ])
}

fn check_destination(path: &Path) -> Result<PathBuf, ExportError> {
    Ok(Destination::open(path)?.path())
}

fn publish(
    path: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    recheck: impl FnOnce() -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    Destination::open(path)?.publish(files, recheck)
}

#[cfg(target_os = "macos")]
#[test]
fn refuses_apfs_aliases_in_read_inputs() {
    let parent = tempfile::tempdir().unwrap();
    let file = parent.path().join("Café.txt");
    fs::write(&file, b"original").unwrap();
    for alias in ["café.txt", "Cafe\u{301}.txt"] {
        assert!(matches!(
            read_file(&parent.path().join(alias), 64),
            Err(ExportError::UnsafePath | ExportError::Storage)
        ));
    }
    assert_eq!(read_file(&file, 64).unwrap(), b"original");
}

#[cfg(target_os = "macos")]
#[test]
fn does_not_publish_after_the_selected_parent_changes() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let moved = root.path().join("moved");
    fs::create_dir(&parent).unwrap();
    let destination = parent.join("export");
    let result = publish(&destination, &fixture(), || {
        fs::rename(&parent, &moved).unwrap();
        fs::create_dir(&parent).unwrap();
        Ok(())
    });
    assert!(matches!(result, Err(ExportError::UnsafePath)));
    assert!(moved.is_dir(), "the publication must reach the recheck");
    assert!(!destination.exists());
    assert!(!moved.join("export").exists());
}
