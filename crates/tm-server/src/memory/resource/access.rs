use tm_host::{HostError, Result as HostResult};

use crate::ServerError;

pub(super) fn ensure_authorized_subject(
    authority: &tm_host::MemoryAuthority,
    subject: &str,
    uri: &str,
) -> HostResult<()> {
    if authority.subject == subject {
        Ok(())
    } else {
        Err(unauthorized_memory_resource(uri))
    }
}

pub(super) fn ensure_authorized_scope(
    authority: &tm_host::MemoryAuthority,
    scope: &str,
    uri: &str,
) -> HostResult<()> {
    if authority.scope == scope {
        Ok(())
    } else {
        Err(unauthorized_memory_resource(uri))
    }
}

pub(super) fn ensure_authorized_record(
    authority: &tm_host::MemoryAuthority,
    subject: &str,
    scope: &str,
    uri: &str,
) -> HostResult<()> {
    ensure_authorized_subject(authority, subject, uri)?;
    ensure_authorized_scope(authority, scope, uri)
}

pub(super) fn unauthorized_memory_resource(uri: &str) -> HostError {
    HostError::NotFound(format!("memory resource {uri}"))
}

pub(super) fn map_memory_store_error(err: ServerError) -> HostError {
    match err {
        ServerError::NotFound(target) => HostError::NotFound(target),
        ServerError::Policy(message) => HostError::InvalidPath(message),
        ServerError::Forbidden => HostError::InvalidPath("forbidden memory resource".to_string()),
        ServerError::InvalidRequest(message) | ServerError::Conflict(message) => {
            HostError::InvalidArgs(message)
        }
        ServerError::Unauthorized => {
            HostError::CapabilityDenied("resources.read:memory".to_string())
        }
        ServerError::Store(message) | ServerError::Backend(message) => HostError::HostCall(message),
    }
}
