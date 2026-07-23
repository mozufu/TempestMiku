use super::*;

#[tokio::test]
async fn in_memory_join_pool_widens_and_leave_narrows_sibling_scopes() {
    let store = InMemoryStore::default();
    store
        .ensure_memory_pool("slime-ecosystem", "Slime Ecosystem")
        .await
        .unwrap();
    for (id, title) in [
        ("slime-os", "SlimeOS"),
        ("zutai", "Zutai"),
        ("dango", "Dango"),
    ] {
        store
            .ensure_project(id, title, crate::MemoryPolicy::Project)
            .await
            .unwrap();
    }

    assert!(
        store
            .pool_sibling_scopes("slime-os")
            .await
            .unwrap()
            .is_empty()
    );

    for project_id in ["slime-os", "zutai", "dango"] {
        let joined = store
            .join_memory_pool(project_id, "slime-ecosystem")
            .await
            .unwrap();
        assert_eq!(joined.pool_id.as_deref(), Some("slime-ecosystem"));
    }

    let mut siblings = store.pool_sibling_scopes("slime-os").await.unwrap();
    siblings.sort();
    assert_eq!(
        siblings,
        vec!["project:dango".to_string(), "project:zutai".to_string()]
    );

    store.leave_memory_pool("zutai").await.unwrap();
    let siblings = store.pool_sibling_scopes("slime-os").await.unwrap();
    assert_eq!(siblings, vec!["project:dango".to_string()]);

    // Idempotent: leaving again with no pool is a no-op, not an error.
    let left_again = store.leave_memory_pool("zutai").await.unwrap();
    assert_eq!(left_again.pool_id, None);
}

#[tokio::test]
async fn in_memory_join_pool_rejects_second_pool_without_explicit_leave() {
    let store = InMemoryStore::default();
    store.ensure_memory_pool("pool-a", "A").await.unwrap();
    store.ensure_memory_pool("pool-b", "B").await.unwrap();
    store
        .ensure_project("slime-os", "SlimeOS", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store.join_memory_pool("slime-os", "pool-a").await.unwrap();

    let err = store
        .join_memory_pool("slime-os", "pool-b")
        .await
        .unwrap_err();
    assert!(matches!(err, ServerError::Conflict(_)));

    // Leaving first clears the way to join the other pool.
    store.leave_memory_pool("slime-os").await.unwrap();
    let joined = store.join_memory_pool("slime-os", "pool-b").await.unwrap();
    assert_eq!(joined.pool_id.as_deref(), Some("pool-b"));
}

#[tokio::test]
async fn in_memory_join_pool_rejects_archived_project_or_pool() {
    let store = InMemoryStore::default();
    store.ensure_memory_pool("pool-a", "A").await.unwrap();
    store
        .ensure_project("slime-os", "SlimeOS", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store
        .archive_project("brian", "slime-os", "test archive")
        .await
        .unwrap();
    assert!(matches!(
        store
            .join_memory_pool("slime-os", "pool-a")
            .await
            .unwrap_err(),
        ServerError::Conflict(_)
    ));

    store
        .ensure_project("zutai", "Zutai", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store.archive_memory_pool("pool-a").await.unwrap();
    assert!(matches!(
        store.join_memory_pool("zutai", "pool-a").await.unwrap_err(),
        ServerError::Conflict(_)
    ));
}

#[tokio::test]
async fn in_memory_pool_archive_is_a_status_flip_not_a_membership_cleanup() {
    let store = InMemoryStore::default();
    store.ensure_memory_pool("pool-a", "A").await.unwrap();
    store
        .ensure_project("slime-os", "SlimeOS", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store
        .ensure_project("zutai", "Zutai", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store.join_memory_pool("slime-os", "pool-a").await.unwrap();
    store.join_memory_pool("zutai", "pool-a").await.unwrap();

    store.archive_memory_pool("pool-a").await.unwrap();

    // Membership itself is untouched by archive...
    let project = store.project("slime-os").await.unwrap().unwrap();
    assert_eq!(project.pool_id.as_deref(), Some("pool-a"));
    // ...but fan-out is gated on the pool's status, so it reads as empty (a read-time filter, not a
    // tombstone or cleanup pass).
    assert!(
        store
            .pool_sibling_scopes("slime-os")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn in_memory_pool_sibling_scopes_excludes_archived_projects() {
    let store = InMemoryStore::default();
    store.ensure_memory_pool("pool-a", "A").await.unwrap();
    store
        .ensure_project("slime-os", "SlimeOS", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store
        .ensure_project("zutai", "Zutai", crate::MemoryPolicy::Project)
        .await
        .unwrap();
    store.join_memory_pool("slime-os", "pool-a").await.unwrap();
    store.join_memory_pool("zutai", "pool-a").await.unwrap();

    store
        .archive_project("brian", "zutai", "test archive")
        .await
        .unwrap();

    assert!(
        store
            .pool_sibling_scopes("slime-os")
            .await
            .unwrap()
            .is_empty()
    );
}
