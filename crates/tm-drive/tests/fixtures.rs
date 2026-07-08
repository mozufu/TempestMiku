use tm_artifacts::ArtifactStore;
use tm_drive::{DrivePutOptions, DriveSearchOptions, InMemoryDriveStore};

fn store() -> (tempfile::TempDir, InMemoryDriveStore) {
    let dir = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::open(dir.path(), "drive-fixtures").unwrap();
    (dir, InMemoryDriveStore::new(artifacts))
}

#[test]
fn deterministic_fixtures_file_into_expected_views() {
    let (_dir, store) = store();
    for (name, bytes, project) in [
        (
            "note.md",
            include_bytes!("fixtures/note.md").as_slice(),
            Some("TempestMiku"),
        ),
        (
            "invoice.txt",
            include_bytes!("fixtures/invoice.txt").as_slice(),
            None,
        ),
        (
            "data.json",
            include_bytes!("fixtures/data.json").as_slice(),
            Some("TempestMiku"),
        ),
        (
            "blob.bin",
            include_bytes!("fixtures/blob.bin").as_slice(),
            None,
        ),
    ] {
        store
            .put_bytes(
                bytes,
                DrivePutOptions {
                    auto: true,
                    source_uri: Some(format!("fixture://{name}")),
                    suggested_path: Some(format!("fixtures/{name}")),
                    project: project.map(str::to_string),
                    ..DrivePutOptions::default()
                },
            )
            .unwrap();
    }

    let invoice_hits = store
        .search(DriveSearchOptions {
            doc_kind: Some("invoice".to_string()),
            ..DriveSearchOptions::default()
        })
        .unwrap();
    assert_eq!(invoice_hits.len(), 1);
    assert_eq!(invoice_hits[0].path, "fixtures/invoice.txt");

    let project_hits = store
        .search(DriveSearchOptions {
            project: Some("TempestMiku".to_string()),
            ..DriveSearchOptions::default()
        })
        .unwrap();
    assert_eq!(project_hits.len(), 2);
}
