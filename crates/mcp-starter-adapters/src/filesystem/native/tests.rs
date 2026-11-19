#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use std::{fs, io::Read};

use super::Root;

#[test]
fn root_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Root>();
}

#[test]
fn bounds_native_path_work() {
    use super::{MAX_COMPONENTS, MAX_PATH_BYTES, validate_relative};
    use std::io::ErrorKind;

    let deepest = vec!["x"; MAX_COMPONENTS].join("/");
    assert!(validate_relative(&deepest).is_ok());
    assert_eq!(
        validate_relative(&format!("{deepest}/x"))
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        validate_relative(&"x".repeat(MAX_PATH_BYTES + 1))
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidInput
    );
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[test]
fn reads_exact_nested_name_and_rejects_unsupported_paths() {
    let temp = tempfile::tempdir().unwrap();
    let approved = temp.path().join("MCP súbory");
    let folder = approved.join("Veľký priečinok");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("Príklad.txt"), "approved").unwrap();
    fs::write(temp.path().join("outside.txt"), "outside").unwrap();
    let root = Root::open(&approved).unwrap();
    let mut text = String::new();
    root.open_file("Veľký priečinok/Príklad.txt")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "approved");

    for path in [
        "",
        "/outside.txt",
        "../outside.txt",
        "./Veľký priečinok/Príklad.txt",
        "Veľký priečinok//Príklad.txt",
        "Veľký priečinok/Príklad.txt/",
        "Veľký priečinok\\Príklad.txt",
        "Veľký priečinok/Príklad.txt:stream",
        "Veľký priečinok/Príklad.txt.",
        "Veľký priečinok/Príklad.txt ",
        "Veľký priečinok/Príklad.txt\0",
        "Veľký priečinok/príklad.txt",
        "veľký priečinok/Príklad.txt",
        "Veľký priečinok",
        "NUL",
    ] {
        assert!(root.open_file(path).is_err(), "{path:?}");
    }
    assert!(Root::open(&temp.path().join("outside.txt")).is_err());
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[test]
fn refuses_hard_links_within_and_outside_the_approved_root() {
    let temp = tempfile::tempdir().unwrap();
    let approved = temp.path().join("approved");
    fs::create_dir(&approved).unwrap();
    let original = temp.path().join("original.txt");
    fs::write(&original, "synthetic outside content").unwrap();
    fs::hard_link(&original, approved.join("link.txt")).unwrap();
    fs::write(approved.join("first.txt"), "synthetic approved content").unwrap();
    fs::hard_link(approved.join("first.txt"), approved.join("second.txt")).unwrap();
    let root = Root::open(&approved).unwrap();
    for name in ["link.txt", "first.txt", "second.txt"] {
        assert!(root.open_file(name).is_err(), "{name}");
    }
}

#[cfg(windows)]
#[test]
fn opens_verbatim_drive_roots_and_pins_open_directories() {
    let temp = tempfile::tempdir().unwrap();
    let approved = temp.path().join("approved");
    let nested = approved.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("data.txt"), "approved").unwrap();
    let verbatim = fs::canonicalize(&approved).unwrap();
    let root = Root::open(&verbatim).unwrap();
    let mut file = root.open_file("nested/data.txt").unwrap();
    assert!(fs::rename(&approved, temp.path().join("moved")).is_err());
    assert!(fs::rename(&nested, temp.path().join("moved")).is_err());
    assert!(fs::rename(nested.join("data.txt"), temp.path().join("moved")).is_err());
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();
    assert_eq!(content, "approved");
    drop(file);
    fs::rename(&nested, temp.path().join("moved")).unwrap();
    drop(root);
    fs::rename(&approved, temp.path().join("moved root")).unwrap();
}

#[cfg(windows)]
#[test]
fn refuses_a_rename_above_the_mapped_root_during_a_read() {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};

    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    let approved = parent.join("approved");
    let moved = temp.path().join("moved");
    fs::create_dir_all(&approved).unwrap();
    fs::write(approved.join("data.txt"), "original").unwrap();
    let root = Root::open(&approved).unwrap();
    let mut file = root.open_file("data.txt").unwrap();

    // NTFS normally refuses to rename a directory containing an open file.
    match fs::rename(&parent, &moved) {
        Ok(()) => assert!(file.read(&mut [0u8; 64]).is_err()),
        Err(error) => {
            assert!(
                error.raw_os_error().is_some_and(|code| matches!(
                    code as u32,
                    ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION
                )),
                "{error:?}"
            );
            let mut content = String::new();
            file.read_to_string(&mut content).unwrap();
            assert_eq!(content, "original");
        }
    }
    drop(file);
    drop(root);
    if parent.exists() {
        fs::rename(&parent, &moved).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn refuses_real_ntfs_short_names() {
    use std::fs::OpenOptions;
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_NOT_SUPPORTED, ERROR_PRIVILEGE_NOT_HELD,
        ERROR_SHORT_NAMES_NOT_ENABLED_ON_VOLUME,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, SetFileShortNameW,
    };

    let temp = tempfile::tempdir().unwrap();
    let long_name = "Fixture with a separate short alias.txt";
    let short_name = "FIXALIAS.TXT";
    let file = temp.path().join(long_name);
    fs::write(&file, "approved").unwrap();
    let handle = OpenOptions::new()
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&file)
        .unwrap();
    let name: Vec<u16> = short_name.encode_utf16().chain([0]).collect();
    // SAFETY: The handle belongs to the test fixture and the short name is NUL-terminated.
    let result = unsafe { SetFileShortNameW(handle.as_raw_handle(), name.as_ptr()) };
    let setup_error = (result == 0).then(std::io::Error::last_os_error);
    drop(handle);
    let aliased_path = temp.path().join(short_name);
    if let Some(error) = setup_error {
        assert!(
            error.raw_os_error().is_some_and(|code| matches!(
                code as u32,
                ERROR_ACCESS_DENIED
                    | ERROR_NOT_SUPPORTED
                    | ERROR_PRIVILEGE_NOT_HELD
                    | ERROR_SHORT_NAMES_NOT_ENABLED_ON_VOLUME
            )),
            "{error:?}"
        );
        eprintln!("Could not install the NTFS 8.3 fixture name on this host: {error}");
        assert!(!aliased_path.exists());
    } else {
        assert_eq!(fs::read_to_string(aliased_path).unwrap(), "approved");
    }

    let root = Root::open(temp.path()).unwrap();
    assert!(root.open_file(long_name).is_ok());
    assert!(root.open_file(&long_name.to_ascii_lowercase()).is_err());
    assert!(root.open_file(short_name).is_err());
}

#[cfg(windows)]
#[test]
fn rejects_a_second_link_created_after_open() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("original.txt");
    let link = temp.path().join("link.txt");
    fs::write(&file, "approved").unwrap();
    let root = Root::open(temp.path()).unwrap();
    let mut opened = root.open_file("original.txt").unwrap();
    if fs::hard_link(&file, &link).is_ok() {
        assert!(opened.read(&mut [0u8; 64]).is_err());
    } else {
        assert_eq!(opened.read(&mut [0u8; 64]).unwrap(), 8);
        drop(opened);
        drop(root);
        fs::hard_link(&file, &link).unwrap();
        let root = Root::open(temp.path()).unwrap();
        assert!(root.open_file("original.txt").is_err());
    }
}

#[cfg(windows)]
#[test]
fn refuses_junctions_within_a_binding_or_in_its_path() {
    use std::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let approved = temp.path().join("approved");
    let outside = temp.path().join("outside");
    let nested = outside.join("nested");
    let junction = approved.join("junction");
    fs::create_dir(&approved).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("data.txt"), "outside").unwrap();
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/j"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = Root::open(&approved).unwrap();
    assert!(root.open_file("junction/nested/data.txt").is_err());
    assert!(Root::open(&junction).is_err());
    assert!(Root::open(&junction.join("nested")).is_err());
}

#[cfg(windows)]
#[test]
fn refuses_non_disk_root_forms() {
    use std::path::Path;

    for path in [
        r"C:relative",
        r"\relative",
        r"\\example.invalid\share",
        r"\\.\C:\folder",
        r"\\?\UNC\example.invalid\share",
        r"C:\test\..\folder",
        r"C:\test\folder. ",
    ] {
        assert!(Root::open(Path::new(path)).is_err(), "{path}");
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn refuses_symlinks_and_special_files_without_opening_them_for_data() {
    use std::ffi::CString;
    use std::os::unix::{ffi::OsStrExt, fs::symlink, net::UnixListener};

    let temp = tempfile::tempdir().unwrap();
    let approved = temp.path().join("approved");
    let outside = temp.path().join("outside");
    fs::create_dir(&approved).unwrap();
    fs::create_dir(&outside).unwrap();
    let original = outside.join("data.txt");
    fs::write(&original, "outside").unwrap();
    symlink(&original, approved.join("file-link")).unwrap();
    symlink(&outside, approved.join("dir-link")).unwrap();
    let socket = approved.join("socket");
    let _listener = UnixListener::bind(&socket).unwrap();
    let fifo = CString::new(approved.join("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: The unique tempfile path is NUL-terminated and remains live.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let root = Root::open(&approved).unwrap();
    for path in ["file-link", "dir-link/data.txt", "socket", "fifo"] {
        assert!(root.open_file(path).is_err(), "{path}");
    }
    assert!(Root::open(&approved.join("dir-link")).is_err());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn refuses_a_file_moved_away_before_its_first_read() {
    let temp = tempfile::tempdir().unwrap();
    let approved = temp.path().join("approved");
    let nested = approved.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("data.txt"), "original").unwrap();
    let root = Root::open(&approved).unwrap();
    let mut file = root.open_file("nested/data.txt").unwrap();
    fs::rename(&nested, temp.path().join("outside")).unwrap();
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("data.txt"), "replacement").unwrap();
    assert!(file.read(&mut [0u8; 64]).is_err());
    let mut content = String::new();
    root.open_file("nested/data.txt")
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "replacement");
}

#[cfg(target_os = "macos")]
#[test]
fn refuses_normalization_aliases_on_apfs() {
    let scratch = tempfile::tempdir().unwrap();
    let file = scratch.path().join("Café.txt");
    fs::write(&file, b"test").unwrap();
    let alias = "Cafe\u{301}.txt";
    let root = Root::open(scratch.path()).unwrap();
    assert!(root.open_file("Café.txt").is_ok());
    assert!(root.open_file(alias).is_err());

    let nested = scratch.path().join("MappedRoot");
    fs::create_dir(&nested).unwrap();
    assert!(Root::open(&nested).is_ok());
    assert!(Root::open(&scratch.path().join("mappedroot")).is_err());
}
