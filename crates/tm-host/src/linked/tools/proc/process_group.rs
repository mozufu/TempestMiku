pub(super) struct ProcessGroupGuard {
    #[cfg(unix)]
    process_group: Option<i32>,
}

impl ProcessGroupGuard {
    pub(super) fn new(child_id: Option<u32>) -> Self {
        #[cfg(not(unix))]
        let _ = child_id;
        Self {
            #[cfg(unix)]
            process_group: child_id.and_then(|id| i32::try_from(id).ok()),
        }
    }

    pub(super) fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.process_group = None;
        }
    }

    #[cfg(unix)]
    fn kill(&self) -> std::io::Result<()> {
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        // SAFETY: `process_group` comes from the spawned child PID and the command was placed in a
        // new process group before spawning. A negative PID targets that entire group.
        let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = self.kill();
    }
}

pub(super) async fn stop_process_tree(
    child: &mut tokio::process::Child,
    process_group: &mut ProcessGroupGuard,
) -> std::io::Result<()> {
    #[cfg(unix)]
    process_group.kill()?;
    #[cfg(not(unix))]
    child.kill().await?;

    match child.wait().await {
        Ok(_) => {
            process_group.disarm();
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            process_group.disarm();
            Ok(())
        }
        Err(error) => Err(error),
    }
}
