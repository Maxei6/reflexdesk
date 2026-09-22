use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Booting,
    SetupRequired,
    Preparing,
    Ready,
    Listening,
    Transcribing,
    Routing,
    Executing,
    ConfirmationRequired,
    Degraded,
    Error,
    Updating,
    ShuttingDown,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub phase: Phase,
    pub ready: bool,
    pub listening: bool,
    pub last_error: Option<String>,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            phase: Phase::Booting,
            ready: false,
            listening: false,
            last_error: None,
        }
    }
}

pub struct RuntimeState(pub Mutex<RuntimeSnapshot>);

impl Default for RuntimeState {
    fn default() -> Self {
        Self(Mutex::new(RuntimeSnapshot::default()))
    }
}

impl RuntimeState {
    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.0
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    pub fn transition(
        &self,
        phase: Phase,
        last_error: Option<String>,
    ) -> Result<RuntimeSnapshot, String> {
        let mut state = self.0.lock().map_err(|_| "runtime lock poisoned")?;
        state.phase = phase;
        state.ready = matches!(
            phase,
            Phase::Ready
                | Phase::Listening
                | Phase::Transcribing
                | Phase::Routing
                | Phase::Executing
                | Phase::ConfirmationRequired
        );
        state.listening = matches!(
            phase,
            Phase::Listening
                | Phase::Transcribing
                | Phase::Routing
                | Phase::Executing
                | Phase::ConfirmationRequired
        );
        state.last_error = last_error;
        Ok(state.clone())
    }
}
