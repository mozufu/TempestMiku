#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectState {
    Running,
    Suspended,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeToken {
    cell_id: String,
    node_id: String,
    origin: String,
    generation: u64,
}

#[derive(Debug, Clone)]
pub struct EffectMachine {
    cell_id: String,
    node_id: String,
    origin: String,
    generation: u64,
    state: EffectState,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EffectMachineError {
    #[error("effect transition {operation} is invalid from {state:?}")]
    InvalidState {
        operation: &'static str,
        state: EffectState,
    },
    #[error("stale or foreign effect continuation")]
    StaleContinuation,
}

impl EffectMachine {
    pub fn new(
        cell_id: impl Into<String>,
        node_id: impl Into<String>,
        origin: impl Into<String>,
    ) -> Self {
        Self {
            cell_id: cell_id.into(),
            node_id: node_id.into(),
            origin: origin.into(),
            generation: 0,
            state: EffectState::Running,
        }
    }
    pub fn state(&self) -> EffectState {
        self.state
    }
    pub fn suspend(&mut self) -> Result<ResumeToken, EffectMachineError> {
        if self.state != EffectState::Running {
            return Err(self.invalid("suspend"));
        }
        self.generation += 1;
        self.state = EffectState::Suspended;
        Ok(ResumeToken {
            cell_id: self.cell_id.clone(),
            node_id: self.node_id.clone(),
            origin: self.origin.clone(),
            generation: self.generation,
        })
    }
    pub fn resume(&mut self, token: &ResumeToken) -> Result<(), EffectMachineError> {
        if self.state != EffectState::Suspended {
            return Err(self.invalid("resume"));
        }
        if token.cell_id != self.cell_id
            || token.node_id != self.node_id
            || token.origin != self.origin
            || token.generation != self.generation
        {
            return Err(EffectMachineError::StaleContinuation);
        }
        self.state = EffectState::Running;
        Ok(())
    }
    pub fn complete(&mut self) -> Result<(), EffectMachineError> {
        self.terminal("complete", EffectState::Completed)
    }
    pub fn fail(&mut self) -> Result<(), EffectMachineError> {
        self.terminal("fail", EffectState::Failed)
    }
    pub fn cancel(&mut self) -> Result<(), EffectMachineError> {
        self.terminal("cancel", EffectState::Cancelled)
    }
    fn terminal(
        &mut self,
        operation: &'static str,
        next: EffectState,
    ) -> Result<(), EffectMachineError> {
        if !matches!(self.state, EffectState::Running | EffectState::Suspended) {
            return Err(self.invalid(operation));
        }
        self.state = next;
        Ok(())
    }
    fn invalid(&self, operation: &'static str) -> EffectMachineError {
        EffectMachineError::InvalidState {
            operation,
            state: self.state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuation_is_origin_generation_and_terminal_bound() {
        let mut machine = EffectMachine::new("cell-1", "node-1", "session-a");
        let token = machine.suspend().unwrap();
        let foreign = EffectMachine::new("cell-1", "node-1", "session-b")
            .suspend()
            .unwrap();
        assert_eq!(
            machine.resume(&foreign),
            Err(EffectMachineError::StaleContinuation)
        );
        machine.resume(&token).unwrap();
        machine.complete().unwrap();
        assert!(matches!(
            machine.resume(&token),
            Err(EffectMachineError::InvalidState { .. })
        ));
        assert!(matches!(
            machine.complete(),
            Err(EffectMachineError::InvalidState { .. })
        ));
    }

    #[test]
    fn cancellation_is_terminal_from_running_or_suspended() {
        let mut running = EffectMachine::new("cell-1", "node-1", "session-a");
        running.cancel().unwrap();
        assert_eq!(running.state(), EffectState::Cancelled);
        assert!(matches!(
            running.complete(),
            Err(EffectMachineError::InvalidState { .. })
        ));

        let mut suspended = EffectMachine::new("cell-2", "node-2", "session-a");
        let token = suspended.suspend().unwrap();
        suspended.cancel().unwrap();
        assert_eq!(suspended.state(), EffectState::Cancelled);
        assert!(matches!(
            suspended.resume(&token),
            Err(EffectMachineError::InvalidState { .. })
        ));
    }
}
