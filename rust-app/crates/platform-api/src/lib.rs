use rust_protocol::SessionId;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    Available,
    PermissionRequired,
    ClipboardOnly,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformCapabilities {
    pub global_shortcut: CapabilityState,
    pub selection_read: CapabilityState,
    pub selection_replace: CapabilityState,
    pub direct_text_insertion: CapabilityState,
    pub non_activating_overlay: CapabilityState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertionRequest {
    pub session_id: SessionId,
    pub text: String,
    pub restore_captured_target: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionOutcome {
    Inserted,
    CopiedToClipboard,
    TargetChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutEvent {
    Pressed,
    Released,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlatformError {
    #[error("permission is required: {0}")]
    PermissionRequired(&'static str),
    #[error("capability is unavailable: {0}")]
    Unavailable(&'static str),
    #[error("the target application changed during the operation")]
    TargetChanged,
    #[error("platform operation failed: {0}")]
    Operation(String),
}

pub trait CapabilityProvider: Send + Sync {
    fn capabilities(&self) -> PlatformCapabilities;
}

pub trait GlobalShortcutService: Send {
    fn start(&mut self, event_sink: Box<dyn Fn(ShortcutEvent) + Send>)
    -> Result<(), PlatformError>;

    fn stop(&mut self);
}

pub trait TextInsertionService: Send + Sync {
    fn insert(&self, request: &InsertionRequest) -> Result<InsertionOutcome, PlatformError>;
}

pub trait SelectionService: Send + Sync {
    fn read_selected_text(&self) -> Result<Option<String>, PlatformError>;

    fn replace_selected_text(&self, text: &str) -> Result<InsertionOutcome, PlatformError>;
}
