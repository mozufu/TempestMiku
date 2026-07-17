use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tm_host::{HostFn, HostRegistry, InvocationCtx, LinkedFolders, ResourceRegistry, ToolDocs};

use super::docs::drive_docs;
use crate::{IntoSharedDriveStore, SharedDriveStore, resources::DriveResourceHandler};

mod authority;
mod linked;
mod operations;
mod organizer;
mod research;

pub(crate) use authority::drive_authority;
use linked::{DriveLinkFn, DriveUnlinkFn};
use research::ResearchDriveFn;

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

macro_rules! drive_fn {
    ($name:ident, $cap:literal, $summary:literal, $approval:literal, $sensitive:expr) => {
        struct $name {
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
