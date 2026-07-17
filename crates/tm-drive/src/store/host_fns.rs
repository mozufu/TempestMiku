use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use tm_host::{
    HostError, HostFn, HostRegistry, InvocationCtx, LinkedFolders, ResourceRegistry, ToolDocs,
};
use uuid::Uuid;

use super::{
    core::{
        drive_error_to_host, drive_put_requires_approval, host_drive_put_options,
        linked_alias_from_target, normalize_canonical_path, validate_host_organizer_config,
    },
    docs::{content_to_bytes, drive_docs, research_drive_docs},
    payloads::{
        drive_entry_event_payload, drive_linked_payload, drive_moved_payload,
        drive_unlinked_payload, drive_write_proposal_payload, organizer_completed_payload,
        organizer_failed_payload, organizer_failed_payload_with_proposals,
        organizer_started_payload,
    },
};
use crate::{
    DriveCollisionStrategy, DriveListOptions, DriveOrganizerConfig, DrivePutOptions,
    DriveSearchOptions, DriveUnlinkResult, IntoSharedDriveStore, OrganizerProposal, ProposalStatus,
    SharedDriveStore, drive_link_policy, memory_scope_for_project, resources::DriveResourceHandler,
};

pub fn register_drive_functions(
    host_registry: &mut HostRegistry,
    resource_registry: &mut ResourceRegistry,
    store: impl IntoSharedDriveStore,
    linked_folders: Option<LinkedFolders>,
) {
    let store = store.into_shared_drive_store();
    let linked_for_unlink = linked_folders.clone();
    host_registry.register(Arc::new(DrivePutFn::new(store.clone())));
    host_registry.register(Arc::new(DriveGetFn::new(store.clone())));
    host_registry.register(Arc::new(DriveLsFn::new(store.clone())));
    host_registry.register(Arc::new(DriveMoveFn::new(store.clone())));
    host_registry.register(Arc::new(DriveSearchFn::new(store.clone())));
    host_registry.register(Arc::new(DriveTagFn::new(store.clone())));
    host_registry.register(Arc::new(DriveLinkFn::new(
        Arc::clone(&store),
        linked_folders,
    )));
    host_registry.register(Arc::new(DriveUnlinkFn::new(
        Arc::clone(&store),
        linked_for_unlink,
    )));
    host_registry.register(Arc::new(DriveOrganizeFn::new(store.clone())));
    host_registry.register(Arc::new(ResearchDriveFn::new(store.clone())));
    resource_registry.register(Arc::new(DriveResourceHandler::new(store)));
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePutArgs {
    content: Value,
    #[serde(default)]
    options: DrivePutOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DrivePathArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveMoveArgs {
    from: String,
    to: String,
    #[serde(default)]
    collision: DriveCollisionStrategy,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveTagArgs {
    path: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveLinkArgs {
    host_path: String,
    #[serde(default = "default_link_mode")]
    mode: String,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveUnlinkArgs {
    alias: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DriveOrganizeArgs {
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    config: DriveOrganizerConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResearchDriveArgs {
    #[serde(default)]
    query: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    doc_kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    max_docs: Option<usize>,
    #[serde(default)]
    max_snippets: Option<usize>,
    #[serde(default)]
    max_bytes_per_doc: Option<usize>,
    #[serde(default)]
    max_digest_bytes: Option<usize>,
    #[serde(default)]
    max_workers: Option<usize>,
    #[serde(default)]
    worker_timeout_ms: Option<u64>,
    #[serde(default)]
    total_timeout_ms: Option<u64>,
}

fn default_link_mode() -> String {
    "ro".to_string()
}

macro_rules! drive_fn {
    ($name:ident, $cap:literal, $summary:literal, $approval:literal, $sensitive:expr) => {
        pub struct $name {
            docs: ToolDocs,
            store: SharedDriveStore,
        }

        impl $name {
            fn new(store: SharedDriveStore) -> Self {
                Self {
                    docs: drive_docs($cap, $summary, $approval, $sensitive),
                    store,
                }
            }
        }

        #[async_trait]
        impl HostFn for $name {
            fn docs(&self) -> &ToolDocs {
                &self.docs
            }

            async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
                self.call_drive(args, ctx).await
            }
        }
    };
}

drive_fn!(
    DrivePutFn,
    "drive.put",
    "Store a document in the local-first drive",
    "policy",
    true
);

pub struct ResearchDriveFn {
    docs: ToolDocs,
    store: SharedDriveStore,
}

impl ResearchDriveFn {
    fn new(store: SharedDriveStore) -> Self {
        Self {
            docs: research_drive_docs(),
            store,
        }
    }
}

#[async_trait]
impl HostFn for ResearchDriveFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: ResearchDriveArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(ctx)?;
        let max_docs = args.max_docs.or(args.limit).unwrap_or(5).clamp(1, 10);
        let max_snippets = args.max_snippets.unwrap_or(max_docs).clamp(1, max_docs);
        let max_bytes_per_doc = args.max_bytes_per_doc.unwrap_or(2_000).clamp(1, 8_000);
        let max_digest_bytes = args.max_digest_bytes.unwrap_or(600).clamp(32, 2_000);
        let max_workers = args.max_workers.unwrap_or(max_docs).min(max_docs);
        let requested_worker_timeout_ms =
            args.worker_timeout_ms.unwrap_or(30_000).clamp(100, 120_000);
        let default_total_timeout_ms = requested_worker_timeout_ms
            .saturating_mul(u64::try_from(max_workers.max(max_docs)).unwrap_or(u64::MAX));
        let total_timeout_ms = args
            .total_timeout_ms
            .unwrap_or(default_total_timeout_ms)
            .clamp(100, 300_000);
        let worker_timeout_ms = requested_worker_timeout_ms.min(total_timeout_ms);

        let mut project = args.project;
        match &authority {
            DriveAuthority::Project(authorized) => {
                if project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(authorized))
                {
                    return Err(cross_project_error("research.drive", authorized));
                }
                project = Some(authorized.clone());
            }
            DriveAuthority::Global if project.is_some() => {
                return Err(global_project_error("research.drive"));
            }
            DriveAuthority::Global | DriveAuthority::Trusted => {}
        }

        let mut hits = self
            .store
            .search(DriveSearchOptions {
                query: Some(args.query.clone()),
                project,
                doc_kind: args.doc_kind,
                tags: args.tags,
                limit: max_docs,
                return_snippets: true,
                ..DriveSearchOptions::default()
            })
            .await
            .map_err(drive_error_to_host)?;
        hits.retain(|hit| authority.permits_project(hit.project.as_deref()));
        hits.truncate(max_docs.min(max_snippets));

        let mut corpus = Vec::with_capacity(hits.len());
        let mut digests = Vec::with_capacity(hits.len());
        let mut citations = Vec::with_capacity(hits.len());
        for hit in hits {
            let selector = args
                .selector
                .clone()
                .or(hit.selector.clone())
                .unwrap_or_else(|| "1-20".to_string());
            let read = self
                .store
                .resource_content(&hit.uri, Some(&selector))
                .await
                .map_err(drive_error_to_host)?;
            let content = truncate_utf8(&read.content, max_bytes_per_doc);
            let fallback = hit
                .snippet
                .as_deref()
                .or(hit.title.as_deref())
                .unwrap_or(&hit.uri);
            let summary_source = first_nonempty_lines(&content, 3);
            let summary = truncate_utf8(
                if summary_source.is_empty() {
                    fallback
                } else {
                    &summary_source
                },
                max_digest_bytes,
            );
            let citation = json!({
                "uri": hit.uri,
                "sourceKind": "drive",
                "selector": selector,
                "contentHash": hit.content_hash,
            });
            corpus.push(json!({
                "uri": hit.uri,
                "sourceKind": "drive",
                "selector": selector,
                "contentHash": hit.content_hash,
                "title": hit.title.or(Some(hit.path)),
                "snippet": hit.snippet.unwrap_or_else(|| first_nonempty_lines(&content, 3)),
                "sizeBytes": read.size_bytes,
            }));
            digests.push(json!({
                "uri": hit.uri,
                "selector": selector,
                "contentHash": hit.content_hash,
                "summary": summary,
                "actorId": Value::Null,
                "artifactUri": Value::Null,
                "historyUri": Value::Null,
                "citations": [citation.clone()],
            }));
            citations.push(citation);
        }
        let answer = digests
            .iter()
            .map(|digest| {
                format!(
                    "[{}#{}] {}",
                    digest["uri"].as_str().unwrap_or_default(),
                    digest["selector"].as_str().unwrap_or_default(),
                    digest["summary"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let selected_docs = corpus.len();
        Ok(json!({
            "query": args.query,
            "corpus": corpus,
            "digests": digests,
            "citations": citations,
            "workerFailures": [],
            "answer": answer,
            "budget": {
                "maxDocs": max_docs,
                "maxSnippets": max_snippets,
                "maxBytesPerDoc": max_bytes_per_doc,
                "maxDigestBytes": max_digest_bytes,
                "maxWorkers": max_workers,
                "workerTimeoutMs": worker_timeout_ms,
                "totalTimeoutMs": total_timeout_ms,
                "selectedDocs": selected_docs,
                "agentDocs": 0,
                "agentDocsCompleted": 0,
                "workerFailures": 0,
            }
        }))
    }
}

fn first_nonempty_lines(text: &str, max_lines: usize) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}
drive_fn!(
    DriveGetFn,
    "drive.get",
    "Read a drive document by path or drive:// URI",
    "none",
    false
);
drive_fn!(
    DriveLsFn,
    "drive.ls",
    "List canonical drive paths or virtual directories",
    "none",
    false
);
drive_fn!(
    DriveMoveFn,
    "drive.move",
    "Move a filed drive document",
    "on-write",
    true
);
drive_fn!(
    DriveSearchFn,
    "drive.search",
    "Search filed drive documents",
    "none",
    false
);
drive_fn!(
    DriveTagFn,
    "drive.tag",
    "Add tags to a filed drive document",
    "on-write",
    true
);
drive_fn!(
    DriveOrganizeFn,
    "drive.organize",
    "Generate organizer proposals for filed documents",
    "policy",
    true
);

pub struct DriveLinkFn {
    docs: ToolDocs,
    store: SharedDriveStore,
    linked_folders: Option<LinkedFolders>,
}

impl DriveLinkFn {
    fn new(store: SharedDriveStore, linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.link",
                "Register an approval-gated linked folder plus project memory scope",
                "always",
                true,
            ),
            store,
            linked_folders,
        }
    }
}

#[async_trait]
impl HostFn for DriveLinkFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveLinkArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(ctx)?;
        let authorized_project = match &authority {
            DriveAuthority::Project(project) => {
                if args
                    .project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(project))
                {
                    return Err(cross_project_error("drive.link", project));
                }
                Some(project.as_str())
            }
            DriveAuthority::Trusted => args.project.as_deref(),
            DriveAuthority::Global => return Err(linked_scope_error("drive.link")),
        };
        ctx.require_approval(&format!("drive.link {}", args.host_path))
            .await?;
        let mode = match args.mode.as_str() {
            "ro" => tm_host::FsMode::Ro,
            "rw" => tm_host::FsMode::Rw,
            other => {
                return Err(HostError::InvalidArgs(format!(
                    "drive.link mode must be ro or rw, got {other}"
                )));
            }
        };
        let (plan, policy) = drive_link_policy(&args.host_path, mode, authorized_project)
            .map_err(drive_error_to_host)?;
        let linked_folders = self
            .linked_folders
            .as_ref()
            .ok_or_else(|| HostError::InvalidPath("no linked folders configured".to_string()))?;
        linked_folders.insert_policy(policy)?;
        if let Err(err) = self.store.record_link(&plan).await {
            let _ = linked_folders.remove_policy(&plan.alias);
            return Err(drive_error_to_host(err));
        }
        ctx.emit_event("drive_linked", drive_linked_payload(&plan))
            .await?;
        serde_json::to_value(plan).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

pub struct DriveUnlinkFn {
    docs: ToolDocs,
    store: SharedDriveStore,
    linked_folders: Option<LinkedFolders>,
}

impl DriveUnlinkFn {
    fn new(store: SharedDriveStore, linked_folders: Option<LinkedFolders>) -> Self {
        Self {
            docs: drive_docs(
                "drive.unlink",
                "Revoke an approval-gated linked folder and its project memory scope",
                "always",
                true,
            ),
            store,
            linked_folders,
        }
    }
}

#[async_trait]
impl HostFn for DriveUnlinkFn {
    fn docs(&self) -> &ToolDocs {
        &self.docs
    }

    async fn call(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveUnlinkArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let alias = linked_alias_from_target(&args.alias)?;
        match drive_authority(ctx)? {
            DriveAuthority::Project(project) if crate::slug(&project) == alias => {}
            DriveAuthority::Project(project) => {
                return Err(cross_project_error("drive.unlink", &project));
            }
            DriveAuthority::Trusted => {}
            DriveAuthority::Global => return Err(linked_scope_error("drive.unlink")),
        }
        ctx.require_approval(&format!("drive.unlink {alias}"))
            .await?;
        self.store
            .revoke_link(&alias)
            .await
            .map_err(drive_error_to_host)?;
        let linked_folders = self
            .linked_folders
            .as_ref()
            .ok_or_else(|| HostError::InvalidPath("no linked folders configured".to_string()))?;
        let policy = linked_folders.remove_policy(&alias)?;
        let result = DriveUnlinkResult {
            alias: alias.clone(),
            canonical_root: policy.root.display().to_string(),
            linked_uri: format!("linked://{alias}/"),
            memory_scope: memory_scope_for_project(&alias),
            revoked_at: Utc::now(),
        };
        ctx.emit_event("drive_unlinked", drive_unlinked_payload(&result))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DrivePutFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DrivePutArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let mut options = host_drive_put_options(args.options);
        match drive_authority(ctx)? {
            DriveAuthority::Project(project) => {
                if options
                    .project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(&project))
                {
                    return Err(cross_project_error("drive.put", &project));
                }
                options.project = Some(project);
            }
            DriveAuthority::Global if options.project.is_some() => {
                return Err(global_project_error("drive.put"));
            }
            DriveAuthority::Global | DriveAuthority::Trusted => {}
        }
        if options.session_id.is_none() && !ctx.session_id.is_empty() {
            options.session_id = Some(ctx.session_id.clone());
        }
        let bytes = content_to_bytes(&self.store, &args.content).await?;
        let path = self
            .store
            .plan_put_path(&bytes, &options)
            .await
            .map_err(drive_error_to_host)?;
        if drive_put_requires_approval(&options) {
            ctx.require_approval(&format!("drive.put {path}")).await?;
        }
        let result = self
            .store
            .put_bytes(&bytes, options)
            .await
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_put", drive_entry_event_payload("put", &result.entry))
            .await?;
        serde_json::to_value(result).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveGetFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DrivePathArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let target = args
            .uri
            .or(args.path)
            .ok_or_else(|| HostError::InvalidArgs("drive.get requires path or uri".to_string()))?;
        let uri = if target.starts_with("drive://") {
            target
        } else {
            format!(
                "drive://{}",
                normalize_canonical_path(&target).map_err(drive_error_to_host)?
            )
        };
        authorize_drive_entry(&self.store, &uri, ctx).await?;
        let content = self
            .store
            .resource_content(&uri, args.selector.as_deref())
            .await
            .map_err(drive_error_to_host)?;
        serde_json::to_value(content).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveLsFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let options: DriveListOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(_ctx)?;
        let mut entries = self
            .store
            .list(options)
            .await
            .map_err(drive_error_to_host)?;
        entries.retain(|entry| authority.permits_entry(entry));
        serde_json::to_value(entries).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveMoveFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveMoveArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        authorize_drive_entry(&self.store, &args.from, ctx).await?;
        ctx.require_approval(&format!("drive.move {} -> {}", args.from, args.to))
            .await?;
        let collision = if args.overwrite {
            DriveCollisionStrategy::Overwrite
        } else {
            args.collision
        };
        let entry = self
            .store
            .move_entry(&args.from, &args.to, collision)
            .await
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_moved", drive_moved_payload(&args.from, &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveSearchFn {
    async fn call_drive(&self, args: Value, _ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let mut options: DriveSearchOptions =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        let authority = drive_authority(_ctx)?;
        match &authority {
            DriveAuthority::Project(project) => {
                if options
                    .project
                    .as_deref()
                    .is_some_and(|requested| crate::slug(requested) != crate::slug(project))
                {
                    return Err(cross_project_error("drive.search", project));
                }
                options.project = Some(project.clone());
            }
            DriveAuthority::Global if options.project.is_some() => {
                return Err(global_project_error("drive.search"));
            }
            DriveAuthority::Global | DriveAuthority::Trusted => {}
        }
        let mut results = self
            .store
            .search(options)
            .await
            .map_err(drive_error_to_host)?;
        results.retain(|result| authority.permits_project(result.project.as_deref()));
        serde_json::to_value(results).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveTagFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveTagArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        authorize_drive_entry(&self.store, &args.path, ctx).await?;
        ctx.require_approval(&format!("drive.tag {}", args.path))
            .await?;
        let entry = self
            .store
            .tag_entry(&args.path, args.tags)
            .await
            .map_err(drive_error_to_host)?;
        ctx.emit_event("drive_tagged", drive_entry_event_payload("tag", &entry))
            .await?;
        serde_json::to_value(entry).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

impl DriveOrganizeFn {
    async fn call_drive(&self, args: Value, ctx: &InvocationCtx) -> tm_host::Result<Value> {
        let args: DriveOrganizeArgs =
            serde_json::from_value(args).map_err(|err| HostError::InvalidArgs(err.to_string()))?;
        validate_host_organizer_config(&args.config)?;
        let store = &self.store;
        let authority = drive_authority(ctx)?;
        ctx.emit_event(
            "drive_organizer_started",
            organizer_started_payload(args.apply, &args.config),
        )
        .await?;
        if !args.apply {
            let proposals = match match &authority {
                DriveAuthority::Trusted => store.organize_with_config(args.config.clone()).await,
                DriveAuthority::Global => {
                    store
                        .organize_scoped_with_config(None, args.config.clone())
                        .await
                }
                DriveAuthority::Project(project) => {
                    store
                        .organize_scoped_with_config(Some(project), args.config.clone())
                        .await
                }
            } {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }

        let mut ids = store
            .pending_proposal_ids()
            .await
            .map_err(drive_error_to_host)?;
        if !matches!(&authority, DriveAuthority::Trusted) {
            ids = authorized_proposal_ids(store, ids, &authority).await?;
        }
        let mut generated = Vec::<OrganizerProposal>::new();
        if ids.is_empty() {
            generated = match match &authority {
                DriveAuthority::Trusted => store.organize_with_config(args.config.clone()).await,
                DriveAuthority::Global => {
                    store
                        .organize_scoped_with_config(None, args.config.clone())
                        .await
                }
                DriveAuthority::Project(project) => {
                    store
                        .organize_scoped_with_config(Some(project), args.config.clone())
                        .await
                }
            } {
                Ok(proposals) => proposals,
                Err(err) => {
                    ctx.emit_event(
                        "drive_organizer_failed",
                        organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                    )
                    .await?;
                    return Err(drive_error_to_host(err));
                }
            };
            ids = generated
                .iter()
                .filter(|proposal| {
                    matches!(
                        proposal.status,
                        ProposalStatus::Pending | ProposalStatus::Approved
                    )
                })
                .map(|proposal| proposal.id)
                .collect();
        }
        if ids.is_empty() {
            emit_drive_write_proposals(ctx, &generated).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &generated),
            )
            .await?;
            return serde_json::to_value(generated)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        if let Err(err) = ctx.require_approval("drive.organize apply").await {
            let status = match err {
                HostError::ApprovalDenied(_) => ProposalStatus::Denied,
                HostError::ApprovalTimeout(_) => ProposalStatus::Failed,
                _ => ProposalStatus::Failed,
            };
            store
                .mark_proposals_status(&ids, status)
                .await
                .map_err(drive_error_to_host)?;
            let proposals = store
                .proposals()
                .await
                .map_err(drive_error_to_host)?
                .into_iter()
                .filter(|proposal| ids.contains(&proposal.id))
                .collect::<Vec<_>>();
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_failed",
                organizer_failed_payload_with_proposals(
                    args.apply,
                    &args.config,
                    &err.to_string(),
                    &proposals,
                ),
            )
            .await?;
            return Err(err);
        }
        let proposals = match store.apply_organizer_proposals(&ids).await {
            Ok(proposals) => proposals,
            Err(err) => {
                ctx.emit_event(
                    "drive_organizer_failed",
                    organizer_failed_payload(args.apply, &args.config, &err.to_string()),
                )
                .await?;
                return Err(drive_error_to_host(err));
            }
        };
        if generated.is_empty() {
            emit_drive_write_proposals(ctx, &proposals).await?;
            ctx.emit_event(
                "drive_organizer_completed",
                organizer_completed_payload(args.apply, &args.config, &proposals),
            )
            .await?;
            return serde_json::to_value(proposals)
                .map_err(|err| HostError::HostCall(err.to_string()));
        }
        for updated in proposals {
            if let Some(proposal) = generated
                .iter_mut()
                .find(|proposal| proposal.id == updated.id)
            {
                *proposal = updated;
            } else {
                generated.push(updated);
            }
        }
        emit_drive_write_proposals(ctx, &generated).await?;
        ctx.emit_event(
            "drive_organizer_completed",
            organizer_completed_payload(args.apply, &args.config, &generated),
        )
        .await?;
        serde_json::to_value(generated).map_err(|err| HostError::HostCall(err.to_string()))
    }
}

async fn emit_drive_write_proposals(
    ctx: &InvocationCtx,
    proposals: &[OrganizerProposal],
) -> tm_host::Result<()> {
    for proposal in proposals {
        ctx.emit_event("write_proposal", drive_write_proposal_payload(proposal))
            .await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) enum DriveAuthority {
    Trusted,
    Global,
    Project(String),
}

impl DriveAuthority {
    pub(crate) fn is_trusted(&self) -> bool {
        matches!(self, Self::Trusted)
    }

    fn permits_project(&self, project: Option<&str>) -> bool {
        match self {
            Self::Trusted => true,
            Self::Global => project.is_none(),
            Self::Project(authorized) => {
                project.is_some_and(|project| crate::slug(project) == crate::slug(authorized))
            }
        }
    }

    pub(crate) fn permits_entry(&self, entry: &crate::DriveEntry) -> bool {
        self.permits_project(entry.project.as_deref())
    }
}

pub(crate) fn drive_authority(ctx: &InvocationCtx) -> tm_host::Result<DriveAuthority> {
    let Some(scope) = ctx.session_scope.as_deref() else {
        if ctx.session_id.is_empty() || ctx.session_id == "default" {
            return Ok(DriveAuthority::Trusted);
        }
        return Err(HostError::CapabilityDenied(
            "drive requires server-authoritative project scope".to_string(),
        ));
    };
    if scope == "global" {
        return Ok(DriveAuthority::Global);
    }
    let Some(project) = scope.strip_prefix("project:") else {
        return Err(HostError::CapabilityDenied(format!(
            "drive is unavailable from non-project session scope {scope}"
        )));
    };
    let project = project.trim();
    if project.is_empty() {
        return Err(HostError::CapabilityDenied(
            "drive requires a non-empty project scope".to_string(),
        ));
    }
    Ok(DriveAuthority::Project(project.to_string()))
}

fn cross_project_error(operation: &str, project: &str) -> HostError {
    HostError::CapabilityDenied(format!(
        "{operation} is restricted to project:{}",
        crate::slug(project)
    ))
}

fn global_project_error(operation: &str) -> HostError {
    HostError::CapabilityDenied(format!(
        "{operation} from global scope is restricted to unprojected drive entries"
    ))
}

fn linked_scope_error(operation: &str) -> HostError {
    HostError::CapabilityDenied(format!("{operation} requires an authorized project scope"))
}

async fn authorize_drive_entry(
    store: &SharedDriveStore,
    path_or_uri: &str,
    ctx: &InvocationCtx,
) -> tm_host::Result<()> {
    let authority = drive_authority(ctx)?;
    let path = normalize_canonical_path(path_or_uri).map_err(drive_error_to_host)?;
    let entry = store
        .list(DriveListOptions {
            path: Some(path.clone()),
            recursive: true,
            limit: 1,
            include_archived: true,
        })
        .await
        .map_err(drive_error_to_host)?
        .into_iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| HostError::NotFound(path_or_uri.to_string()))?;
    if authority.permits_entry(&entry) {
        Ok(())
    } else {
        match authority {
            DriveAuthority::Project(project) => {
                Err(cross_project_error("drive entry access", &project))
            }
            DriveAuthority::Global => Err(global_project_error("drive entry access")),
            DriveAuthority::Trusted => Ok(()),
        }
    }
}

async fn authorized_proposal_ids(
    store: &SharedDriveStore,
    ids: Vec<Uuid>,
    authority: &DriveAuthority,
) -> tm_host::Result<Vec<Uuid>> {
    let entry_ids = store
        .list(DriveListOptions {
            path: None,
            recursive: true,
            limit: usize::MAX,
            include_archived: true,
        })
        .await
        .map_err(drive_error_to_host)?
        .into_iter()
        .filter(|entry| authority.permits_entry(entry))
        .map(|entry| entry.id)
        .collect::<BTreeSet<_>>();
    let allowed = store
        .proposals()
        .await
        .map_err(drive_error_to_host)?
        .into_iter()
        .filter(|proposal| entry_ids.contains(&proposal.entry_id))
        .map(|proposal| proposal.id)
        .collect::<BTreeSet<_>>();
    Ok(ids.into_iter().filter(|id| allowed.contains(id)).collect())
}
