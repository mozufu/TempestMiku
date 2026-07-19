use super::*;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCatalogEntry {
    pub id: String,
    pub memory_scope: String,
    pub project_uri: String,
    pub linked_folders_uri: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCatalogResponse {
    pub projects: Vec<ProjectCatalogEntry>,
}

pub(crate) async fn list_projects<S, M, C>(
    State(state): State<AppState<S, M, C>>,
) -> Json<ProjectCatalogResponse>
where
    S: Store,
    M: MemoryProvider,
    C: ChatRunner,
{
    Json(ProjectCatalogResponse {
        projects: project_catalog_entries(&state.linked_folders),
    })
}

fn project_catalog_entries(linked_folders: &LinkedFolders) -> Vec<ProjectCatalogEntry> {
    linked_folders
        .aliases()
        .into_iter()
        .map(|id| ProjectCatalogEntry {
            memory_scope: format!("project:{id}"),
            project_uri: format!("project://{id}"),
            linked_folders_uri: format!("project://{id}/linked-folders"),
            id,
        })
        .collect()
}

pub(crate) fn project_resource_entries(linked_folders: &LinkedFolders) -> Vec<ResourceEntry> {
    project_catalog_entries(linked_folders)
        .into_iter()
        .map(|project| ResourceEntry {
            uri: project.project_uri,
            name: project.id.clone(),
            kind: "project".to_string(),
            title: Some(project.id),
            size_bytes: None,
            modified_at: None,
        })
        .collect()
}
