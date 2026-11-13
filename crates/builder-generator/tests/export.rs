#![cfg(any(windows, target_os = "linux"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use builder_generator::{
    ConflictDecision, ConflictResolution, ExportError, ExportRequest, FileOwner, GenerationRecord,
    plan_export, publish_export, read_resolutions,
};
use builder_project::{MANIFEST_NAME, ManifestRevision, ProjectDocument, validate_document};
use serde_json::json;

const EDITABLE_SOURCE: &str = "src/handlers.rs";
const RECORD: &str = "generation.json";

fn project(root: &Path) {
    fs::create_dir(root).unwrap();
    let manifest = validate_document(ProjectDocument::new("Standalone fixture".into())).unwrap();
    fs::write(root.join(MANIFEST_NAME), manifest.bytes()).unwrap();
}

fn request(root: &Path, name: &str) -> ExportRequest {
    ExportRequest {
        project_root: root.join("project"),
        destination: root.join(name),
        previous_export: None,
        resolutions: Vec::new(),
    }
}

fn inventory(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(directory: &Path, prefix: &str, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let relative = format!("{prefix}{name}");
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), &format!("{relative}/"), files);
            } else {
                files.insert(relative, fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, "", &mut files);
    files
}

#[test]
fn preview_and_repeat_publish_use_the_same_normalized_inputs_and_record() {
    let scratch = tempfile::tempdir().unwrap();
    let first = request(scratch.path(), "first");
    project(&first.project_root);
    let preview = plan_export(&first).unwrap();
    assert!(preview.conflicts.is_empty());
    assert!(!first.destination.exists());
    assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
    let record = publish_export(&first).unwrap();
    assert_eq!(preview.generation.as_ref(), Some(&record));

    // An edit to whitespace alone does not change the exported snapshot.
    let source = first.project_root.join(MANIFEST_NAME);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    fs::write(source, serde_json::to_vec(&value).unwrap()).unwrap();
    let second = request(scratch.path(), "second");
    assert_eq!(publish_export(&second).unwrap(), record);
    let first_files = inventory(&first.destination);
    assert_eq!(first_files, inventory(&second.destination));
    assert_eq!(
        serde_json::from_slice::<GenerationRecord>(&first_files[RECORD]).unwrap(),
        record
    );
    assert_eq!(first_files.len(), record.files.len() + 1);
    assert!(
        record
            .files
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    );
    for file in &record.files {
        assert_eq!(ManifestRevision::of(&first_files[&file.path]), file.digest);
    }
    assert_eq!(
        record.cargo_lock_digest,
        ManifestRevision::of(&first_files["Cargo.lock"])
    );
    assert!(
        record
            .files
            .iter()
            .any(|file| file.path == EDITABLE_SOURCE && file.owner == FileOwner::Project)
    );
    assert!(
        first_files
            .keys()
            .all(|path| !path.contains("https_trust_fixtures") && !path.starts_with("apps/"))
    );
    assert!(matches!(
        publish_export(&first),
        Err(ExportError::DestinationExists)
    ));
    assert_eq!(first_files, inventory(&first.destination));
}

#[test]
fn regeneration_preserves_reviewed_edits_and_refuses_stale_choices() {
    let scratch = tempfile::tempdir().unwrap();
    let first = request(scratch.path(), "first");
    project(&first.project_root);
    let original = publish_export(&first).unwrap();
    let edit = b"// User-owned handler edit\n";
    fs::write(first.destination.join(EDITABLE_SOURCE), edit).unwrap();
    fs::write(first.destination.join("README.md"), b"edited readme\n").unwrap();
    fs::write(first.destination.join("notes.txt"), b"user notes\n").unwrap();
    for name in [
        "runtime-config.json",
        "runtime-config.local.json",
        ".env",
        ".env.dev",
    ] {
        fs::write(first.destination.join(name), b"local-secret-canary").unwrap();
    }
    fs::create_dir(first.destination.join("target")).unwrap();
    fs::write(
        first.destination.join("target/scratch.txt"),
        b"local build output",
    )
    .unwrap();

    let mut next = request(scratch.path(), "next");
    next.previous_export = Some(first.destination.clone());
    let plan = plan_export(&next).unwrap();
    assert!(plan.generation.is_none());
    assert_eq!(
        plan.conflicts
            .iter()
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>(),
        ["README.md", "notes.txt"]
    );
    assert!(matches!(
        publish_export(&next),
        Err(ExportError::Conflict(_))
    ));
    assert!(!next.destination.exists());
    let readme = &plan.conflicts[0];
    assert_eq!(
        readme.previous_digest,
        original
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .map(|file| file.digest)
    );
    let resolution_path = scratch.path().join("resolutions.json");
    fs::write(
        &resolution_path,
        serde_json::to_vec(&json!({
            "version": 1,
            "resolutions": plan.conflicts.iter().map(|item| ConflictResolution {
                path: item.path.clone(),
                observed_digest: item.observed_digest,
                replacement_digest: item.replacement_digest,
                decision: ConflictDecision::Keep,
            }).collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();
    next.resolutions = read_resolutions(&resolution_path).unwrap();
    fs::write(
        first.destination.join("notes.txt"),
        b"changed after review\n",
    )
    .unwrap();
    assert!(matches!(
        publish_export(&next),
        Err(ExportError::InvalidResolution)
    ));
    assert!(!next.destination.exists());
    fs::write(first.destination.join("notes.txt"), b"user notes\n").unwrap();
    let record = publish_export(&next).unwrap();
    let files = inventory(&next.destination);
    assert_eq!(files[EDITABLE_SOURCE], edit);
    assert_eq!(files["README.md"], b"edited readme\n");
    assert_eq!(files["notes.txt"], b"user notes\n");
    assert!(files.values().all(|bytes| {
        !bytes
            .windows(18)
            .any(|window| window == b"local-secret-canary")
    }));
    assert!(!next.destination.join("target").exists());
    assert!(
        record
            .files
            .iter()
            .any(|file| file.path == "README.md" && file.owner == FileOwner::Project)
    );
    let mut original_bytes = serde_json::to_vec_pretty(&original).unwrap();
    original_bytes.push(b'\n');
    assert_eq!(
        fs::read(first.destination.join(RECORD)).unwrap(),
        original_bytes
    );

    let mut third = request(scratch.path(), "third");
    third.previous_export = Some(next.destination);
    assert_eq!(publish_export(&third).unwrap(), record);
    assert_eq!(inventory(&third.destination), files);
}

#[test]
fn regeneration_can_explicitly_restore_a_missing_generated_file() {
    let scratch = tempfile::tempdir().unwrap();
    let first = request(scratch.path(), "first");
    project(&first.project_root);
    publish_export(&first).unwrap();
    fs::remove_file(first.destination.join("README.md")).unwrap();
    let mut next = request(scratch.path(), "next");
    next.previous_export = Some(first.destination.clone());
    let preview = plan_export(&next).unwrap();
    let [conflict] = preview.conflicts.as_slice() else {
        panic!("expected one removal conflict")
    };
    assert!(!conflict.can_keep);
    assert_eq!(conflict.observed_digest, None);
    assert!(conflict.replacement_digest.is_some());
    next.resolutions.push(ConflictResolution {
        path: conflict.path.clone(),
        observed_digest: None,
        replacement_digest: conflict.replacement_digest,
        decision: ConflictDecision::Regenerate,
    });
    publish_export(&next).unwrap();
    assert!(next.destination.join("README.md").is_file());
    assert!(!first.destination.join("README.md").exists());
}

#[test]
fn regeneration_refuses_a_foreign_generation_record() {
    let scratch = tempfile::tempdir().unwrap();
    let first = request(scratch.path(), "first");
    project(&first.project_root);
    publish_export(&first).unwrap();
    let mut next = request(scratch.path(), "next");
    next.project_root = scratch.path().join("other-project");
    project(&next.project_root);
    next.previous_export = Some(first.destination.clone());
    assert!(matches!(
        publish_export(&next),
        Err(ExportError::InvalidPreviousExport)
    ));
    assert!(!next.destination.exists());
    assert!(first.destination.join(RECORD).exists());
}
