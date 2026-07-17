use crate::{HostError, Result};

#[cfg(unix)]
use super::open::host_call;
use super::open::stat_entry;
#[cfg(not(unix))]
use super::open::unsupported;
use super::{EntrySnapshot, SecureParent};

pub(in crate::linked) fn rename_entry(
    source: &SecureParent,
    source_expected: EntrySnapshot,
    dest: &SecureParent,
    dest_expected: Option<EntrySnapshot>,
    display: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
        let fresh_source = stat_entry(source, display)?.ok_or_else(|| {
            HostError::InvalidArgs(format!(
                "stale filesystem entry for {display}; retry from a fresh read"
            ))
        })?;
        if fresh_source.identity != source_expected.identity
            || fresh_source.kind != source_expected.kind
        {
            return Err(HostError::InvalidArgs(format!(
                "stale filesystem entry for {display}; retry from a fresh read"
            )));
        }
        let fresh_dest = stat_entry(dest, display)?;
        if fresh_dest.map(|entry| (entry.identity, entry.kind))
            != dest_expected.map(|entry| (entry.identity, entry.kind))
        {
            return Err(HostError::InvalidArgs(format!(
                "stale destination for {display}; retry from a fresh read"
            )));
        }
        if dest_expected.is_none() {
            #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
            {
                return rustix::fs::renameat_with(
                    &source.dir,
                    source.name.as_os_str(),
                    &dest.dir,
                    dest.name.as_os_str(),
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(host_call);
            }
        }
        rustix::fs::renameat(
            &source.dir,
            source.name.as_os_str(),
            &dest.dir,
            dest.name.as_os_str(),
        )
        .map_err(host_call)
    }
    #[cfg(not(unix))]
    {
        let _ = (source, source_expected, dest, dest_expected, display);
        Err(unsupported())
    }
}

pub(in crate::linked) fn remove_entry(
    parent: &SecureParent,
    expected: EntrySnapshot,
    display: &str,
) -> Result<()> {
    #[cfg(unix)]
    {
        let fresh = stat_entry(parent, display)?.ok_or_else(|| {
            HostError::InvalidArgs(format!(
                "stale filesystem entry for {display}; retry from a fresh read"
            ))
        })?;
        if fresh.identity != expected.identity || fresh.kind != expected.kind {
            return Err(HostError::InvalidArgs(format!(
                "stale filesystem entry for {display}; retry from a fresh read"
            )));
        }
        rustix::fs::unlinkat(
            &parent.dir,
            parent.name.as_os_str(),
            rustix::fs::AtFlags::empty(),
        )
        .map_err(host_call)
    }
    #[cfg(not(unix))]
    {
        let _ = (parent, expected, display);
        Err(unsupported())
    }
}
