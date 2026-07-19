use std::sync::{Arc, Barrier};

use super::*;

#[test]
fn storage_identifiers_accept_only_canonical_forms() {
    assert_eq!(SessionId::parse("session-1").unwrap().as_str(), "session-1");
    for invalid in ["", ".", "..", "/absolute", "a/b", "a\\b", "bad id"] {
        assert!(SessionId::parse(invalid).is_err(), "accepted {invalid:?}");
    }

    assert_eq!(ArtifactId::parse_uri("artifact://42").unwrap().get(), 42);
    for invalid in [
        "artifact://",
        "artifact://01",
        "artifact://-1",
        "artifact://1/x",
    ] {
        assert!(
            ArtifactId::parse_uri(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }

    let hash = "a".repeat(64);
    assert_eq!(
        BlobId::parse_uri(&format!("blob:sha256:{hash}"))
            .unwrap()
            .hash(),
        hash
    );
    assert!(BlobId::parse_uri(&format!("blob:sha256:{}", "A".repeat(64))).is_err());
    assert!(BlobId::parse_uri(&format!("blob:sha256:{}", "a".repeat(63))).is_err());
}

#[test]
fn stores_artifact_and_resolves_by_uri() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    let artifact = store
        .put_text("one\ntwo\nthree", Some("test".into()), "text/plain")
        .unwrap();

    assert_eq!(artifact.uri, "artifact://0");
    let content = store.read(&artifact.uri, Some("2-2")).unwrap();
    assert_eq!(content.content, "two");
    assert!(content.has_more);
}

#[test]
fn trusted_full_text_read_preserves_exact_bounded_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "transport").unwrap();
    let expected = format!("{}\r\nlast-line\n", "x".repeat(300 * 1024));
    let artifact = store
        .put_text(&expected, Some("transport".into()), "text/plain")
        .unwrap();

    let (resolved, content) = store.read_all_text(&artifact.uri).unwrap();
    assert_eq!(resolved, artifact);
    assert_eq!(content, expected);
}

#[test]
fn blobs_are_content_addressed() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    let one = store.put_blob(b"same").unwrap();
    let two = store.put_blob(b"same").unwrap();
    assert_eq!(one, two);
    assert!(one.starts_with("blob:sha256:"));
    assert_eq!(store.read_blob(&one).unwrap(), b"same");
}

#[test]
fn concurrent_store_instances_allocate_distinct_artifact_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();

    for label in ["one", "two"] {
        let root = root.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let store = ArtifactStore::open(root, "default").unwrap();
            barrier.wait();
            store
                .put_text(label, Some(label.to_string()), "text/plain")
                .unwrap()
                .uri
        }));
    }

    let mut uris = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    uris.sort();
    assert_eq!(uris, ["artifact://0", "artifact://1"]);
}

#[test]
fn aggregate_session_quota_is_shared_across_store_handles() {
    let dir = tempfile::tempdir().unwrap();
    let limits = ArtifactLimits {
        max_artifact_bytes: 8,
        max_session_bytes: 10,
        ..ArtifactLimits::default()
    };
    let first = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();
    let second = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();

    first.put_text("12345678", None, "text/plain").unwrap();
    assert!(matches!(
        second.put_text("abcd", None, "text/plain"),
        Err(ArtifactError::QuotaExceeded {
            resource: "session",
            attempted: 12,
            limit: 10,
        })
    ));
}

#[test]
fn aggregate_session_quota_counts_blob_references_once_per_session() {
    let dir = tempfile::tempdir().unwrap();
    let limits = ArtifactLimits {
        max_artifact_bytes: 8,
        max_blob_bytes: 8,
        max_session_bytes: 10,
        ..ArtifactLimits::default()
    };
    let first = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();
    let second = ArtifactStore::open_with_limits(dir.path(), "shared", limits).unwrap();

    let uri = first.put_blob(b"12345678").unwrap();
    assert_eq!(second.put_blob(b"12345678").unwrap(), uri);
    second.put_text("12", None, "text/plain").unwrap();
    assert!(matches!(
        first.put_blob(b"abcd"),
        Err(ArtifactError::QuotaExceeded {
            resource: "session",
            attempted: 14,
            limit: 10,
        })
    ));

    let other = ArtifactStore::open_with_limits(dir.path(), "other", limits).unwrap();
    assert_eq!(other.put_blob(b"12345678").unwrap(), uri);
    other.put_text("12", None, "text/plain").unwrap();
}

#[test]
fn zero_byte_artifacts_still_consume_count_quota() {
    let dir = tempfile::tempdir().unwrap();
    let limits = ArtifactLimits {
        max_artifact_count: 2,
        ..ArtifactLimits::default()
    };
    let store = ArtifactStore::open_with_limits(dir.path(), "counted", limits).unwrap();
    store.put_text("", None, "text/plain").unwrap();
    store.put_text("", None, "text/plain").unwrap();
    assert!(matches!(
        store.put_text("", None, "text/plain"),
        Err(ArtifactError::QuotaExceeded {
            resource: "artifact count",
            attempted: 3,
            limit: 2,
        })
    ));
    drop(store);

    let too_small = ArtifactLimits {
        max_artifact_count: 1,
        ..ArtifactLimits::default()
    };
    assert!(matches!(
        ArtifactStore::open_with_limits(dir.path(), "counted", too_small),
        Err(ArtifactError::QuotaExceeded {
            resource: "artifact count",
            attempted: 2,
            limit: 1,
        })
    ));
}

#[test]
fn aggregate_artifact_metadata_is_bounded_before_write() {
    let dir = tempfile::tempdir().unwrap();
    let first = ArtifactStore::open(dir.path(), "metadata").unwrap();
    let artifact = first.put_text("", None, "text/plain").unwrap();
    drop(first);
    let first_metadata_bytes = fs::metadata(
        dir.path()
            .join("sessions/metadata/artifacts")
            .join(format!("{}.meta", artifact.id)),
    )
    .unwrap()
    .len() as usize;
    let limits = ArtifactLimits {
        max_session_metadata_bytes: first_metadata_bytes + 64,
        ..ArtifactLimits::default()
    };
    let store = ArtifactStore::open_with_limits(dir.path(), "metadata", limits).unwrap();
    assert!(matches!(
        store.put_text("", Some("x".repeat(256)), "text/plain"),
        Err(ArtifactError::QuotaExceeded {
            resource: "session artifact metadata",
            ..
        })
    ));
    assert_eq!(
        ArtifactStore::open(dir.path(), "metadata")
            .unwrap()
            .list()
            .len(),
        1
    );
}

#[test]
fn list_page_is_bounded_and_refreshes_cross_handle_writes() {
    let dir = tempfile::tempdir().unwrap();
    let first = ArtifactStore::open(dir.path(), "listed").unwrap();
    let second = ArtifactStore::open(dir.path(), "listed").unwrap();
    for value in ["zero", "one", "two"] {
        second.put_text(value, None, "text/plain").unwrap();
    }

    let (page, has_more) = first.list_page(1, 1).unwrap();
    assert_eq!(
        page.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        ["1"]
    );
    assert!(has_more);
    let (page, has_more) = first.list_page(2, 1).unwrap();
    assert_eq!(
        page.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        ["2"]
    );
    assert!(!has_more);
    assert!(matches!(
        first.list_page(0, 0),
        Err(ArtifactError::InvalidSelector(_))
    ));
}

#[test]
fn blob_references_have_count_and_metadata_quotas() {
    let dir = tempfile::tempdir().unwrap();
    let count_limits = ArtifactLimits {
        max_blob_count: 1,
        ..ArtifactLimits::default()
    };
    let store = ArtifactStore::open_with_limits(dir.path(), "blob-count", count_limits).unwrap();
    store.put_blob(b"one").unwrap();
    assert!(matches!(
        store.put_blob(b"two"),
        Err(ArtifactError::QuotaExceeded {
            resource: "blob reference count",
            attempted: 2,
            limit: 1,
        })
    ));

    let metadata_limits = ArtifactLimits {
        max_session_blob_ref_bytes: 1,
        ..ArtifactLimits::default()
    };
    let store =
        ArtifactStore::open_with_limits(dir.path(), "blob-metadata", metadata_limits).unwrap();
    assert!(matches!(
        store.put_blob(&[0; 10]),
        Err(ArtifactError::QuotaExceeded {
            resource: "session blob-reference metadata",
            attempted: 2,
            limit: 1,
        })
    ));
}

#[test]
fn rejects_unsafe_storage_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    for session_id in ["", ".", "..", "../escape", "/tmp/escape", "nested/id"] {
        assert!(
            matches!(
                ArtifactStore::open(dir.path(), session_id),
                Err(ArtifactError::InvalidUri(_))
            ),
            "session id should be rejected: {session_id:?}"
        );
    }

    let store = ArtifactStore::open(dir.path(), "safe").unwrap();
    for uri in [
        "blob:sha256:/etc/passwd",
        "blob:sha256:../../outside",
        "blob:sha256:ABCDEF",
        "blob:sha256:abc",
    ] {
        assert!(matches!(
            store.read_blob(uri),
            Err(ArtifactError::InvalidUri(_))
        ));
    }
    assert!(matches!(
        store.read("artifact://../0", None),
        Err(ArtifactError::InvalidUri(_))
    ));
}

#[test]
fn detects_tampered_blob_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    let uri = store.put_blob(b"trusted").unwrap();
    let hash = uri.strip_prefix("blob:sha256:").unwrap();
    fs::write(dir.path().join("blobs").join(hash), b"tampered").unwrap();

    assert!(matches!(
        store.read_blob(&uri),
        Err(ArtifactError::Integrity(_))
    ));
}

#[test]
fn rejects_oversized_artifact_metadata_without_loading_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    let artifact = store.put_text("trusted", None, "text/plain").unwrap();
    drop(store);
    let metadata_path = dir
        .path()
        .join("sessions/default/artifacts")
        .join(format!("{}.meta", artifact.id));
    fs::write(metadata_path, vec![b'x'; MAX_ARTIFACT_METADATA_BYTES + 1]).unwrap();

    assert!(matches!(
        ArtifactStore::open(dir.path(), "default"),
        Err(ArtifactError::Integrity(_))
    ));
}

#[test]
fn rejects_tampered_blob_quota_references() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    let uri = store.put_blob(b"trusted").unwrap();
    let hash = uri.strip_prefix("blob:sha256:").unwrap();
    drop(store);
    fs::write(
        dir.path()
            .join("sessions/default/blob_refs")
            .join(format!("{hash}.ref")),
        b"1",
    )
    .unwrap();

    assert!(matches!(
        ArtifactStore::open(dir.path(), "default"),
        Err(ArtifactError::Integrity(_))
    ));
}

#[test]
fn rejects_oversized_artifact_title_before_writing_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();

    assert!(matches!(
        store.put_text(
            "small content",
            Some("x".repeat(MAX_ARTIFACT_TITLE_BYTES + 1)),
            "text/plain",
        ),
        Err(ArtifactError::QuotaExceeded {
            resource: "artifact title",
            ..
        })
    ));
    assert!(store.list().is_empty());
    assert!(
        ArtifactStore::open(dir.path(), "default")
            .unwrap()
            .list()
            .is_empty()
    );
}

#[test]
fn missing_artifact_returns_without_relocking() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();
    store.put_text("present", None, "text/plain").unwrap();

    let err = store.read("artifact://999", None).unwrap_err();
    assert!(matches!(err, ArtifactError::NotFound { .. }));
    assert!(err.to_string().contains("artifact://0"));
}

#[test]
fn enforces_item_session_and_page_limits() {
    let dir = tempfile::tempdir().unwrap();
    let limits = ArtifactLimits {
        max_artifact_bytes: 16,
        max_blob_bytes: 32,
        max_session_bytes: 24,
        max_artifact_count: 8,
        max_session_metadata_bytes: 64 * 1024,
        max_blob_count: 8,
        max_session_blob_ref_bytes: 64 * 1024,
        default_page_lines: 2,
        default_page_bytes: 8,
        max_page_lines: 3,
        max_page_bytes: 8,
    };
    let store = ArtifactStore::open_with_limits(dir.path(), "default", limits).unwrap();
    let artifact = store
        .put_text("one\ntwo\nthree", None, "text/plain")
        .unwrap();

    let first = store.read(&artifact.uri, None).unwrap();
    assert_eq!(first.content, "one\ntwo");
    assert!(first.has_more);
    let third = store.read(&artifact.uri, Some("3-3")).unwrap();
    assert_eq!(third.content, "three");
    assert!(!third.has_more);
    assert!(matches!(
        store.read(&artifact.uri, Some("1-4")),
        Err(ArtifactError::InvalidSelector(_))
    ));
    assert!(matches!(
        store.put_text("x".repeat(17), None, "text/plain"),
        Err(ArtifactError::QuotaExceeded {
            resource: "artifact",
            ..
        })
    ));
    assert!(matches!(
        store.put_text("y".repeat(12), None, "text/plain"),
        Err(ArtifactError::QuotaExceeded {
            resource: "session",
            ..
        })
    ));
}

#[test]
fn byte_limited_pages_end_on_valid_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let store = ArtifactStore::open_with_limits(
        dir.path(),
        "default",
        ArtifactLimits {
            max_artifact_bytes: 64,
            max_blob_bytes: 64,
            max_session_bytes: 64,
            max_artifact_count: 8,
            max_session_metadata_bytes: 64 * 1024,
            max_blob_count: 8,
            max_session_blob_ref_bytes: 64 * 1024,
            default_page_lines: 2,
            default_page_bytes: 3,
            max_page_lines: 2,
            max_page_bytes: 3,
        },
    )
    .unwrap();
    let artifact = store.put_text("ééé", None, "text/plain").unwrap();
    let page = store.read(&artifact.uri, None).unwrap();
    assert_eq!(page.content, "é");
    assert!(page.has_more);
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_blob_and_artifact_content() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.txt");
    fs::write(&outside, "outside").unwrap();
    let store = ArtifactStore::open(dir.path(), "default").unwrap();

    let blob_uri = store.put_blob(b"blob").unwrap();
    let hash = blob_uri.strip_prefix("blob:sha256:").unwrap();
    let blob_path = dir.path().join("blobs").join(hash);
    fs::remove_file(&blob_path).unwrap();
    symlink(&outside, &blob_path).unwrap();
    assert!(matches!(
        store.read_blob(&blob_uri),
        Err(ArtifactError::Integrity(_))
    ));

    // Use a fresh store for the artifact case: once the blob is tampered, every
    // quota-sensitive write for that session must fail closed on the corrupt ref.
    let artifact_dir = tempfile::tempdir().unwrap();
    let artifact_store = ArtifactStore::open(artifact_dir.path(), "default").unwrap();
    let artifact = artifact_store
        .put_text("inside", None, "text/plain")
        .unwrap();
    let artifact_path = artifact_dir
        .path()
        .join("sessions/default/artifacts")
        .join(format!("{}.txt", artifact.id));
    fs::remove_file(&artifact_path).unwrap();
    symlink(&outside, &artifact_path).unwrap();
    assert!(matches!(
        artifact_store.read(&artifact.uri, None),
        Err(ArtifactError::Integrity(_))
    ));
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_managed_storage_directories() {
    use std::os::unix::fs::symlink;

    let blob_root = tempfile::tempdir().unwrap();
    let blob_outside = tempfile::tempdir().unwrap();
    let blob_store = ArtifactStore::open(blob_root.path(), "default").unwrap();
    fs::remove_dir(blob_root.path().join("blobs")).unwrap();
    symlink(blob_outside.path(), blob_root.path().join("blobs")).unwrap();
    assert!(matches!(
        blob_store.put_blob(b"must stay inside"),
        Err(ArtifactError::Integrity(_))
    ));

    let artifact_root = tempfile::tempdir().unwrap();
    let artifact_outside = tempfile::tempdir().unwrap();
    let artifact_store = ArtifactStore::open(artifact_root.path(), "default").unwrap();
    let artifact_dir = artifact_root.path().join("sessions/default/artifacts");
    fs::remove_dir(&artifact_dir).unwrap();
    symlink(artifact_outside.path(), &artifact_dir).unwrap();
    assert!(matches!(
        artifact_store.put_text("must stay inside", None, "text/plain"),
        Err(ArtifactError::Integrity(_))
    ));
}
