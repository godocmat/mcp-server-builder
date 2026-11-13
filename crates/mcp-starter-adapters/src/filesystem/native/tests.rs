use std::{fs, io::Read};

use super::Root;

#[test]
fn root_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Root>();
}

#[cfg(any(windows, target_os = "linux"))]
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

#[cfg(any(windows, target_os = "linux"))]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
