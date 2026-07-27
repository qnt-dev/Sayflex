use rust_protocol::TargetSnapshot;
use thiserror::Error;
use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextEditPattern,
            IUIAutomationTextPattern, IUIAutomationTextPattern2, UIA_TextEditPatternId,
            UIA_TextPattern2Id, UIA_TextPatternId,
        },
    },
    core::{BSTR, Interface},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};

const MAX_SELECTION_CHARACTERS: i32 = 1_048_576;
const MAX_PARENT_DEPTH: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionOutcome {
    Text(String),
    Unavailable,
    Protected,
}

#[derive(Debug, Error)]
pub enum SelectionError {
    #[error("UI Automation initialization failed: {0}")]
    Initialize(#[source] windows::core::Error),
    #[error("UI Automation call failed: {0}")]
    Automation(#[source] windows::core::Error),
}

pub fn selected_text(target: TargetSnapshot) -> Result<SelectionOutcome, SelectionError> {
    let _apartment = ComApartment::initialize()?;
    // SAFETY: COM is initialized for this worker thread and CUIAutomation is
    // an in-process COM class with no aggregation requested.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(SelectionError::Automation)?;
    // SAFETY: automation is a live COM interface owned by this stack frame.
    let walker = unsafe { automation.ControlViewWalker() }.map_err(SelectionError::Automation)?;
    for focused in focused_elements(&automation, target) {
        // SAFETY: focused is a live UI Automation element. A provider failure
        // is treated as protected so clipboard fallback never probes a field
        // whose privacy status could not be established.
        if unsafe { focused.CurrentIsPassword() }.map_or(true, |value| value.as_bool()) {
            return Ok(SelectionOutcome::Protected);
        }
        let mut current = Some(focused);
        for _ in 0..MAX_PARENT_DEPTH {
            let Some(element) = current.take() else {
                break;
            };
            if let Some(text) = text_from_element(&element)? {
                return Ok(SelectionOutcome::Text(text));
            }
            // SAFETY: element and walker are live COM interfaces. Failure to
            // navigate is equivalent to reaching the bounded search root.
            current = unsafe { walker.GetParentElement(&element) }.ok();
        }
    }
    Ok(SelectionOutcome::Unavailable)
}

fn focused_elements(
    automation: &IUIAutomation,
    target: TargetSnapshot,
) -> Vec<IUIAutomationElement> {
    let mut elements = Vec::with_capacity(2);
    // SAFETY: automation is a live COM interface for the current apartment.
    if let Ok(element) = unsafe { automation.GetFocusedElement() }
        // SAFETY: element is the live interface returned above.
        && unsafe { element.CurrentProcessId() }.ok() == Some(target.process_id as i32)
    {
        elements.push(element);
    }

    let mut information = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: information has the required cbSize and remains writable for the
    // call; target.thread_id was captured from the foreground target window.
    if unsafe { GetGUIThreadInfo(target.thread_id, &mut information) } != 0
        && !information.hwndFocus.is_null()
        // SAFETY: hwndFocus was returned by GetGUIThreadInfo and automation is
        // live for this apartment.
        && let Ok(element) = unsafe { automation.ElementFromHandle(HWND(information.hwndFocus)) }
        // SAFETY: element is the live interface returned above.
        && unsafe { element.CurrentProcessId() }.ok() == Some(target.process_id as i32)
    {
        elements.push(element);
    }
    elements
}

fn text_from_element(element: &IUIAutomationElement) -> Result<Option<String>, SelectionError> {
    // Some providers return the base pattern through GetCurrentPattern but do
    // not implement GetCurrentPatternAs consistently. Query the native pattern
    // object first, then ask COM for the interface we need.
    // SAFETY: element is a live UI Automation interface; all returned COM
    // values remain reference-counted by their Rust wrappers.
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
    // SAFETY: pattern is a live UI Automation text-pattern interface.
    let ranges = unsafe { pattern.GetSelection() }.map_err(SelectionError::Automation)?;
    // SAFETY: ranges is the live array returned by GetSelection.
    let length = unsafe { ranges.Length() }.map_err(SelectionError::Automation)?;
    if length <= 0 {
        return Ok(None);
    }

    let mut selections = Vec::with_capacity(length as usize);
    for index in 0..length {
        // SAFETY: index is bounded by the Length value returned above.
        let range = unsafe { ranges.GetElement(index) }.map_err(SelectionError::Automation)?;
        // SAFETY: range is a live text-range interface and the requested
        // character count is finite.
        let text: BSTR = unsafe { range.GetText(MAX_SELECTION_CHARACTERS) }
            .map_err(SelectionError::Automation)?;
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

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, SelectionError> {
        // SAFETY: initializes COM exactly once on this short-lived capture
        // thread; Drop balances every successful initialization.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(SelectionError::Initialize)?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: initialize succeeded for this same thread.
            CoUninitialize();
        }
    }
}
