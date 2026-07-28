use std::{
    sync::{
        OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use rust_protocol::TargetSnapshot;
use thiserror::Error;
use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::Accessibility::{
            CUIAutomation8, IUIAutomation2, IUIAutomationElement, IUIAutomationTextEditPattern,
            IUIAutomationTextPattern, IUIAutomationTextPattern2, IUIAutomationTreeWalker,
            UIA_TextEditPatternId, UIA_TextPattern2Id, UIA_TextPatternId,
        },
    },
    core::{BSTR, Interface},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};

use crate::inject;

const MAX_SELECTION_CHARACTERS: i32 = 1_048_576;
const MAX_PARENT_DEPTH: usize = 12;
const CONNECTION_TIMEOUT_MS: u32 = 120;
const TRANSACTION_TIMEOUT_MS: u32 = 220;
const ACTOR_RESPONSE_TIMEOUT: Duration = Duration::from_millis(450);

static ACTOR: OnceLock<Result<UiAutomationActor, String>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionOutcome {
    Text(String),
    Unavailable,
    Protected,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SelectionError {
    #[error("UI Automation actor is unavailable: {0}")]
    ActorUnavailable(String),
    #[error("UI Automation request exceeded its latency budget")]
    Timeout,
    #[error("UI Automation initialization failed: {0}")]
    Initialize(String),
    #[error("UI Automation call `{operation}` failed with HRESULT {code:#010x}")]
    Automation { operation: &'static str, code: i32 },
}

pub fn selected_text(target: TargetSnapshot) -> Result<SelectionOutcome, SelectionError> {
    let actor = ACTOR.get_or_init(UiAutomationActor::start);
    match actor {
        Ok(actor) => actor.selected_text(target),
        Err(error) => Err(SelectionError::ActorUnavailable(error.clone())),
    }
}

struct UiAutomationActor {
    requests: Sender<SelectionRequest>,
}

struct SelectionRequest {
    target: TargetSnapshot,
    response: Sender<Result<SelectionOutcome, SelectionError>>,
}

impl UiAutomationActor {
    fn start() -> Result<Self, String> {
        let (requests, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("uia-selection-mta".to_owned())
            .spawn(move || run_actor(receiver, ready_tx))
            .map_err(|error| error.to_string())?;
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| format!("UI Automation actor did not initialize: {error}"))??;
        Ok(Self { requests })
    }

    fn selected_text(&self, target: TargetSnapshot) -> Result<SelectionOutcome, SelectionError> {
        let (response, receiver) = mpsc::channel();
        self.requests
            .send(SelectionRequest { target, response })
            .map_err(|error| SelectionError::ActorUnavailable(error.to_string()))?;
        receiver
            .recv_timeout(ACTOR_RESPONSE_TIMEOUT)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => SelectionError::Timeout,
                mpsc::RecvTimeoutError::Disconnected => {
                    SelectionError::ActorUnavailable("UI Automation actor stopped".to_owned())
                }
            })?
    }
}

fn run_actor(receiver: Receiver<SelectionRequest>, ready: mpsc::SyncSender<Result<(), String>>) {
    let apartment = match ComApartment::initialize() {
        Ok(apartment) => apartment,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    let mut client = match UiAutomationClient::new() {
        Ok(client) => {
            let _ = ready.send(Ok(()));
            Some(client)
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };

    for request in receiver {
        let current = match client.as_ref() {
            Some(client) => client.selected_text(request.target),
            None => match UiAutomationClient::new() {
                Ok(new_client) => {
                    let result = new_client.selected_text(request.target);
                    client = Some(new_client);
                    result
                }
                Err(error) => Err(error),
            },
        };
        if current.is_err() {
            client = None;
        }
        let _ = request.response.send(current);
    }
    drop(apartment);
}

struct UiAutomationClient {
    automation: IUIAutomation2,
    walker: IUIAutomationTreeWalker,
}

impl UiAutomationClient {
    fn new() -> Result<Self, SelectionError> {
        // SAFETY: COM is initialized as MTA on this dedicated actor thread and
        // CUIAutomation8 is an in-process COM class with no aggregation.
        let automation: IUIAutomation2 =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
                .map_err(|error| automation_error("CoCreateInstance", error))?;
        // Bound provider calls so a broken accessibility implementation cannot
        // block selection capture or the global input service indefinitely.
        unsafe {
            automation
                .SetConnectionTimeout(CONNECTION_TIMEOUT_MS)
                .map_err(|error| automation_error("SetConnectionTimeout", error))?;
            automation
                .SetTransactionTimeout(TRANSACTION_TIMEOUT_MS)
                .map_err(|error| automation_error("SetTransactionTimeout", error))?;
            automation
                .SetAutoSetFocus(false)
                .map_err(|error| automation_error("SetAutoSetFocus", error))?;
        }
        // SAFETY: automation is a live COM interface owned by this MTA actor.
        let walker = unsafe { automation.ControlViewWalker() }
            .map_err(|error| automation_error("ControlViewWalker", error))?;
        Ok(Self { automation, walker })
    }

    fn selected_text(&self, target: TargetSnapshot) -> Result<SelectionOutcome, SelectionError> {
        if !inject::is_foreground(target) {
            return Ok(SelectionOutcome::Unavailable);
        }

        let mut password_status_unknown = false;
        for focused in focused_elements(&self.automation, target) {
            // Only a positive password result blocks capture. Provider errors
            // mean that UIA is unavailable for this element; the explicit,
            // user-triggered clipboard path remains eligible as a fallback.
            match unsafe { focused.CurrentIsPassword() } {
                Ok(value) if value.as_bool() => return Ok(SelectionOutcome::Protected),
                Ok(_) => {}
                Err(_) => {
                    password_status_unknown = true;
                    continue;
                }
            }

            let mut current = Some(focused);
            for _ in 0..MAX_PARENT_DEPTH {
                let Some(element) = current.take() else {
                    break;
                };
                if let Some(text) = text_from_element(&element)? {
                    if inject::is_foreground(target) {
                        return Ok(SelectionOutcome::Text(text));
                    }
                    return Ok(SelectionOutcome::Unavailable);
                }
                // SAFETY: element and walker are live COM interfaces. Failure
                // to navigate is equivalent to reaching the bounded root.
                current = unsafe { self.walker.GetParentElement(&element) }.ok();
            }
        }
        if password_status_unknown {
            crate::diagnostic_line(&format!(
                "uia_selection target_pid={} result=unavailable protection_status=unknown",
                target.process_id
            ));
        }
        Ok(SelectionOutcome::Unavailable)
    }
}

fn focused_elements(
    automation: &IUIAutomation2,
    target: TargetSnapshot,
) -> Vec<IUIAutomationElement> {
    let mut elements = Vec::with_capacity(2);
    // Foreground ownership is validated before and after extraction. Do not
    // reject a focused element merely because a multi-process application
    // exposes accessibility through a different provider process.
    if let Ok(element) = unsafe { automation.GetFocusedElement() } {
        elements.push(element);
    }

    let mut information = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: information has the required cbSize and target.thread_id was
    // captured from the foreground target window.
    if unsafe { GetGUIThreadInfo(target.thread_id, &mut information) } != 0
        && !information.hwndFocus.is_null()
        && let Ok(element) = unsafe { automation.ElementFromHandle(HWND(information.hwndFocus)) }
    {
        elements.push(element);
    }
    elements
}

fn text_from_element(element: &IUIAutomationElement) -> Result<Option<String>, SelectionError> {
    // Some providers expose only a derived text interface. Query each native
    // pattern object and let COM perform the interface conversion.
    let pattern = unsafe { element.GetCurrentPattern(UIA_TextPatternId) }
        .and_then(|pattern| pattern.cast::<IUIAutomationTextPattern>())
        .or_else(|_| unsafe {
            element
                .GetCurrentPattern(UIA_TextPattern2Id)?
                .cast::<IUIAutomationTextPattern2>()?
                .cast::<IUIAutomationTextPattern>()
        })
        .or_else(|_| unsafe {
            element
                .GetCurrentPattern(UIA_TextEditPatternId)?
                .cast::<IUIAutomationTextEditPattern>()?
                .cast::<IUIAutomationTextPattern>()
        });
    let pattern = match pattern {
        Ok(pattern) => pattern,
        Err(_) => return Ok(None),
    };
    let ranges = unsafe { pattern.GetSelection() }
        .map_err(|error| automation_error("GetSelection", error))?;
    let length =
        unsafe { ranges.Length() }.map_err(|error| automation_error("Selection.Length", error))?;
    if length <= 0 {
        return Ok(None);
    }

    let mut selections = Vec::with_capacity(length as usize);
    for index in 0..length {
        let range = unsafe { ranges.GetElement(index) }
            .map_err(|error| automation_error("Selection.GetElement", error))?;
        let text: BSTR = unsafe { range.GetText(MAX_SELECTION_CHARACTERS) }
            .map_err(|error| automation_error("TextRange.GetText", error))?;
        let text = String::from_utf16_lossy(&text);
        if !text.is_empty() {
            selections.push(text);
        }
    }
    if selections.is_empty() {
        Ok(None)
    } else {
        Ok(Some(selections.join("\n")))
    }
}

fn automation_error(operation: &'static str, error: windows::core::Error) -> SelectionError {
    SelectionError::Automation {
        operation,
        code: error.code().0,
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, SelectionError> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| SelectionError::Initialize(error.to_string()))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
