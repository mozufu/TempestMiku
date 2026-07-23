use super::*;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCatalogEntry {
    pub id: String,
    pub title: String,
    pub status: String,
    pub memory_scope: String,
    pub default_memory_policy: crate::MemoryPolicy,
    pub project_uri: String,
    pub linked_folders_uri: String,
    /// 0..n linked folders currently attached to this project (§30). Empty for a folderless project.
    pub linked_folder_uris: Vec<String>,
    /// The memory pool this project currently belongs to (§30.7), if any.
    pub pool_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCatalogResponse {
    pub projects: Vec<ProjectCatalogEntry>,
}

pub(crate) async fn list_projects<S, M, C>(
    State(state): State<AppState<S, M, C>>,
) -> Result<Json<ProjectCatalogResponse>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let records = state.store.projects(false).await?;
    Ok(Json(ProjectCatalogResponse {
        projects: records
            .into_iter()
            .map(|record| catalog_entry(record, &state.linked_folders))
            .collect(),
    }))
}

/// Build a catalog entry for a project entity, resolving any attached linked folders (§30). The
/// attachment is resolved by alias-matching the project id, matching the linked-folder resource view.
fn catalog_entry(record: ProjectRecord, linked_folders: &LinkedFolders) -> ProjectCatalogEntry {
    let id = record.id;
    let key = linked_project_key(&id);
    let linked_folder_uris = linked_folders
        .aliases()
        .into_iter()
        .filter(|alias| linked_project_key(alias) == key)
        .map(|alias| format!("project://{id}/linked-folders/{alias}/"))
        .collect();
    ProjectCatalogEntry {
        title: record.title,
        status: record.status.as_str().to_string(),
        memory_scope: format!("project:{id}"),
        default_memory_policy: record.default_memory_policy,
        project_uri: format!("project://{id}"),
        linked_folders_uri: format!("project://{id}/linked-folders"),
        linked_folder_uris,
        pool_id: record.pool_id,
        id,
    }
}

fn linked_project_key(value: &str) -> String {
    tm_drive::slug(value).replace('-', "")
}

pub(crate) async fn project_resource_entries<S, M, C>(
    state: &AppState<S, M, C>,
) -> Result<Vec<ResourceEntry>>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    let records = state.store.projects(false).await?;
    Ok(records
        .into_iter()
        .map(|record| {
            let entry = catalog_entry(record, &state.linked_folders);
            ResourceEntry {
                uri: entry.project_uri,
                name: entry.id.clone(),
                kind: "project".to_string(),
                title: Some(entry.title),
                size_bytes: None,
                modified_at: None,
            }
        })
        .collect())
}
