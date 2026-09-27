use std::{ffi::OsStr, mem::size_of, path::Path, time::Instant};

use rust_protocol::PhysicalRect;
use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HWND, LPARAM, RECT},
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{CreateRectRgn, DeleteObject, GetRegionData, GetWindowRgn, RGNDATA},
    },
    System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
    },
};

const MAX_SNAPSHOT_WINDOWS: usize = 2_048;
const MAX_WINDOW_TEXT_UNITS: usize = 16 * 1024;
const MAX_REGION_RECTS: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowIdentity {
    pub hwnd: u64,
    pub process_id: u32,
    pub process_started_100ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSnapshot {
    pub identity: WindowIdentity,
    pub z_order: u32,
    pub bounds: PhysicalRect,
    pub region: Vec<PhysicalRect>,
    pub executable_name: Option<String>,
    pub window_title: Option<String>,
    pub application_id: Option<String>,
    pub partial: bool,
}

#[derive(Debug, Clone)]
pub struct WindowInventory {
    generation: u64,
    desktop_bounds: PhysicalRect,
    windows: Vec<WindowSnapshot>,
}

#[derive(Debug, Error)]
pub enum WindowSnapshotError {
    #[error("the source-window snapshot deadline elapsed")]
    DeadlineExceeded,
    #[error("Windows could not enumerate top-level windows: {0}")]
    Enumeration(std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateDisposition {
    Eligible,
    Invisible,
    Cloaked,
    ToolWindow,
    SayFlexOwned,
}

#[derive(Debug, Clone)]
struct SnapshotCandidate {
    snapshot: WindowSnapshot,
    disposition: CandidateDisposition,
}

impl WindowInventory {
    pub fn capture(
        generation: u64,
        desktop_bounds: PhysicalRect,
        deadline: Instant,
    ) -> Result<Self, WindowSnapshotError> {
        if Instant::now() >= deadline {
            return Err(WindowSnapshotError::DeadlineExceeded);
        }
        let mut state = EnumerationState {
            deadline,
            current_process_id: std::process::id(),
            desktop_bounds,
            candidates: Vec::new(),
            timed_out: false,
        };
        // SAFETY: the callback is synchronous. `state` remains alive and
        // exclusively borrowed for the complete EnumWindows call.
        let enumerated = unsafe {
            EnumWindows(
                Some(enumerate_window),
                (&raw mut state).cast::<core::ffi::c_void>() as LPARAM,
            )
        };
        if state.timed_out {
            return Err(WindowSnapshotError::DeadlineExceeded);
        }
        if enumerated == 0 {
            return Err(WindowSnapshotError::Enumeration(
                std::io::Error::last_os_error(),
            ));
        }
        Ok(Self::from_candidates(
            generation,
            desktop_bounds,
            state.candidates,
        ))
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    #[must_use]
    pub fn source_for(&self, selection: PhysicalRect) -> Option<WindowSnapshot> {
        let selection = intersect(selection, self.desktop_bounds)?;
        let center = (
            i64::from(selection.x) + i64::from(selection.width) / 2,
            i64::from(selection.y) + i64::from(selection.height) / 2,
        );
        let mut scored = Vec::with_capacity(self.windows.len());
        let mut higher_regions = Vec::<PhysicalRect>::new();
        for window in &self.windows {
            let mut visible = window
                .region
                .iter()
                .filter_map(|region| intersect(*region, self.desktop_bounds))
                .collect::<Vec<_>>();
            for cover in &higher_regions {
                visible = visible
                    .into_iter()
                    .flat_map(|part| subtract_rect(part, *cover))
                    .collect();
            }
            let inside_selection = visible
                .iter()
                .filter_map(|part| intersect(*part, selection))
                .collect::<Vec<_>>();
            let area = inside_selection.iter().map(|rect| area(*rect)).sum::<u64>();
            let owns_center = inside_selection
                .iter()
                .any(|rect| contains_point(*rect, center.0, center.1));
            if area > 0 {
                scored.push((window, area, owns_center));
            }
            higher_regions.extend(window.region.iter().copied());
        }
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let (winner, winner_area, owns_center) = *scored.first()?;
        let runner_area = scored.get(1).map_or(0, |entry| entry.1);
        if winner_area == runner_area {
            return None;
        }
        let accounted_area = scored.iter().map(|entry| entry.1).sum::<u64>();
        let majority = winner_area.saturating_mul(2) > accounted_area;
        let center_lead =
            owns_center && winner_area.saturating_mul(2) >= runner_area.saturating_mul(3);
        if !majority && !center_lead {
            return None;
        }
        let mut winner = winner.clone();
        winner.partial = scored.len() > 1 || winner_area < area(selection);
        Some(winner)
    }

    fn from_candidates(
        generation: u64,
        desktop_bounds: PhysicalRect,
        candidates: Vec<SnapshotCandidate>,
    ) -> Self {
        let mut windows = candidates
            .into_iter()
            .filter_map(|candidate| {
                (candidate.disposition == CandidateDisposition::Eligible)
                    .then_some(candidate.snapshot)
            })
            .collect::<Vec<_>>();
        windows.sort_by_key(|window| window.z_order);
        Self {
            generation,
            desktop_bounds,
            windows,
        }
    }
}

struct EnumerationState {
    deadline: Instant,
    current_process_id: u32,
    desktop_bounds: PhysicalRect,
    candidates: Vec<SnapshotCandidate>,
    timed_out: bool,
}

unsafe extern "system" fn enumerate_window(hwnd: HWND, state: LPARAM) -> i32 {
    // SAFETY: EnumWindows passes back the exact pointer supplied by capture.
    let state = unsafe { &mut *(state as *mut EnumerationState) };
    if Instant::now() >= state.deadline {
        state.timed_out = true;
        return 0;
    }
    if state.candidates.len() >= MAX_SNAPSHOT_WINDOWS {
        return 1;
    }
    if let Some(candidate) = snapshot_window(hwnd, state) {
        state.candidates.push(candidate);
    }
    1
}

fn snapshot_window(hwnd: HWND, state: &EnumerationState) -> Option<SnapshotCandidate> {
    // SAFETY: HWND is supplied by EnumWindows and is valid for the duration of
    // this callback. Every query tolerates a window disappearing concurrently.
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let mut process_id = 0_u32;
    unsafe { GetWindowThreadProcessId(hwnd, &raw mut process_id) };
    let tool = (unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW) != 0;
    let cloaked = window_is_cloaked(hwnd);
    let disposition = if !visible {
        CandidateDisposition::Invisible
    } else if cloaked {
        CandidateDisposition::Cloaked
    } else if tool {
        CandidateDisposition::ToolWindow
    } else if process_id == state.current_process_id {
        CandidateDisposition::SayFlexOwned
    } else {
        CandidateDisposition::Eligible
    };
    let bounds = window_bounds(hwnd).and_then(|bounds| intersect(bounds, state.desktop_bounds))?;
    let executable_name = process_executable_name(process_id);
    let application_id = executable_name.as_deref().and_then(application_id);
    let identity = WindowIdentity {
        hwnd: hwnd as usize as u64,
        process_id,
        process_started_100ns: process_started_100ns(process_id).unwrap_or_default(),
    };
    Some(SnapshotCandidate {
        snapshot: WindowSnapshot {
            identity,
            z_order: state.candidates.len() as u32,
            bounds,
            region: window_region(hwnd, bounds),
            executable_name,
            window_title: window_text(hwnd),
            application_id,
            partial: false,
        },
        disposition,
    })
}

fn window_is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0_u32;
    // SAFETY: DWM writes one u32 into the provided, correctly sized buffer.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        ) >= 0
            && cloaked != 0
    }
}

fn window_bounds(hwnd: HWND) -> Option<PhysicalRect> {
    let mut bounds = RECT::default();
    // SAFETY: DWM writes one RECT into the provided, correctly sized buffer.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&raw mut bounds).cast(),
            size_of::<RECT>() as u32,
        )
    };
    (result >= 0).then(|| rect_from_win32(bounds)).flatten()
}

fn window_region(hwnd: HWND, bounds: PhysicalRect) -> Vec<PhysicalRect> {
    // SAFETY: owned GDI region; deleted on every path below.
    let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if region.is_null() {
        return vec![bounds];
    }
    let kind = unsafe { GetWindowRgn(hwnd, region) };
    if kind <= 0 {
        unsafe { DeleteObject(region) };
        return vec![bounds];
    }
    let bytes = unsafe { GetRegionData(region, 0, std::ptr::null_mut()) };
    if bytes == 0 {
        unsafe { DeleteObject(region) };
        return vec![bounds];
    }
    let mut storage = vec![0_u8; bytes as usize];
    let read = unsafe { GetRegionData(region, bytes, storage.as_mut_ptr().cast::<RGNDATA>()) };
    unsafe { DeleteObject(region) };
    if read == 0 || storage.len() < size_of::<windows_sys::Win32::Graphics::Gdi::RGNDATAHEADER>() {
        return vec![bounds];
    }
    // SAFETY: GetRegionData filled an RGNDATAHEADER followed by nCount RECTs.
    let data = unsafe { &*(storage.as_ptr().cast::<RGNDATA>()) };
    let count = usize::try_from(data.rdh.nCount)
        .unwrap_or(MAX_REGION_RECTS)
        .min(MAX_REGION_RECTS);
    let rect_bytes = count.saturating_mul(size_of::<RECT>());
    if size_of::<windows_sys::Win32::Graphics::Gdi::RGNDATAHEADER>() + rect_bytes > storage.len() {
        return vec![bounds];
    }
    let rects = unsafe {
        std::slice::from_raw_parts(
            storage
                .as_ptr()
                .add(size_of::<windows_sys::Win32::Graphics::Gdi::RGNDATAHEADER>())
                .cast::<RECT>(),
            count,
        )
    };
    let result = rects
        .iter()
        .filter_map(|rect| {
            let local = rect_from_win32(*rect)?;
            let x = i64::from(bounds.x).checked_add(i64::from(local.x))?;
            let y = i64::from(bounds.y).checked_add(i64::from(local.y))?;
            intersect(
                PhysicalRect {
                    x: i32::try_from(x).ok()?,
                    y: i32::try_from(y).ok()?,
                    width: local.width,
                    height: local.height,
                },
                bounds,
            )
        })
        .collect::<Vec<_>>();
    if result.is_empty() {
        vec![bounds]
    } else {
        result
    }
}

fn window_text(hwnd: HWND) -> Option<String> {
    let length = usize::try_from(unsafe { GetWindowTextLengthW(hwnd) })
        .ok()?
        .min(MAX_WINDOW_TEXT_UNITS.saturating_sub(1));
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0_u16; length + 1];
    let written = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    (written > 0).then(|| String::from_utf16_lossy(&buffer[..written as usize]))
}

fn process_executable_name(process_id: u32) -> Option<String> {
    if process_id == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let queried =
        unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &raw mut length) }
            != 0;
    unsafe { CloseHandle(process) };
    if !queried || length == 0 {
        return None;
    }
    Path::new(&String::from_utf16_lossy(&buffer[..length as usize]))
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
}

fn process_started_100ns(process_id: u32) -> Option<u64> {
    if process_id == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let queried = unsafe {
        GetProcessTimes(
            process,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    } != 0;
    unsafe { CloseHandle(process) };
    queried.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn application_id(executable_name: &str) -> Option<String> {
    let value = Path::new(executable_name)
        .file_stem()
        .and_then(OsStr::to_str)?
        .trim()
        .to_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

fn rect_from_win32(rect: RECT) -> Option<PhysicalRect> {
    let width = i64::from(rect.right).checked_sub(i64::from(rect.left))?;
    let height = i64::from(rect.bottom).checked_sub(i64::from(rect.top))?;
    (width > 0 && height > 0).then_some(PhysicalRect {
        x: rect.left,
        y: rect.top,
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
    })
}

fn intersect(first: PhysicalRect, second: PhysicalRect) -> Option<PhysicalRect> {
    let left = i64::from(first.x).max(i64::from(second.x));
    let top = i64::from(first.y).max(i64::from(second.y));
    let right = (i64::from(first.x) + i64::from(first.width))
        .min(i64::from(second.x) + i64::from(second.width));
    let bottom = (i64::from(first.y) + i64::from(first.height))
        .min(i64::from(second.y) + i64::from(second.height));
    (right > left && bottom > top).then_some(PhysicalRect {
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
        width: u32::try_from(right - left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
    })
}

fn subtract_rect(source: PhysicalRect, cover: PhysicalRect) -> Vec<PhysicalRect> {
    let Some(overlap) = intersect(source, cover) else {
        return vec![source];
    };
    let source_right = i64::from(source.x) + i64::from(source.width);
    let source_bottom = i64::from(source.y) + i64::from(source.height);
    let overlap_right = i64::from(overlap.x) + i64::from(overlap.width);
    let overlap_bottom = i64::from(overlap.y) + i64::from(overlap.height);
    let mut parts = Vec::with_capacity(4);
    push_rect(
        &mut parts,
        i64::from(source.x),
        i64::from(source.y),
        source_right,
        i64::from(overlap.y),
    );
    push_rect(
        &mut parts,
        i64::from(source.x),
        overlap_bottom,
        source_right,
        source_bottom,
    );
    push_rect(
        &mut parts,
        i64::from(source.x),
        i64::from(overlap.y),
        i64::from(overlap.x),
        overlap_bottom,
    );
    push_rect(
        &mut parts,
        overlap_right,
        i64::from(overlap.y),
        source_right,
        overlap_bottom,
    );
    parts
}

fn push_rect(output: &mut Vec<PhysicalRect>, left: i64, top: i64, right: i64, bottom: i64) {
    if right <= left || bottom <= top {
        return;
    }
    if let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
        i32::try_from(left),
        i32::try_from(top),
        u32::try_from(right - left),
        u32::try_from(bottom - top),
    ) {
        output.push(PhysicalRect {
            x,
            y,
            width,
            height,
        });
    }
}

fn area(rect: PhysicalRect) -> u64 {
    u64::from(rect.width).saturating_mul(u64::from(rect.height))
}

fn contains_point(rect: PhysicalRect, x: i64, y: i64) -> bool {
    x >= i64::from(rect.x)
        && y >= i64::from(rect.y)
        && x < i64::from(rect.x) + i64::from(rect.width)
        && y < i64::from(rect.y) + i64::from(rect.height)
}

#[cfg(test)]
mod tests {
    use rust_protocol::PhysicalRect;

    use super::{
        CandidateDisposition, SnapshotCandidate, WindowIdentity, WindowInventory, WindowSnapshot,
    };

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }

    fn candidate(
        hwnd: u64,
        z_order: u32,
        bounds: PhysicalRect,
        region: Vec<PhysicalRect>,
    ) -> SnapshotCandidate {
        SnapshotCandidate {
            snapshot: WindowSnapshot {
                identity: WindowIdentity {
                    hwnd,
                    process_id: hwnd as u32,
                    process_started_100ns: hwnd,
                },
                z_order,
                bounds,
                region,
                executable_name: Some(format!("app-{hwnd}.exe")),
                window_title: Some(format!("window {hwnd}")),
                application_id: Some(format!("app-{hwnd}")),
                partial: false,
            },
            disposition: CandidateDisposition::Eligible,
        }
    }

    fn inventory(candidates: Vec<SnapshotCandidate>) -> WindowInventory {
        WindowInventory::from_candidates(7, rect(-1_920, 0, 3_840, 1_080), candidates)
    }

    #[test]
    fn small_game_above_large_browser_wins_from_visible_pixels() {
        let game = candidate(
            1,
            0,
            rect(200, 150, 400, 300),
            vec![rect(200, 150, 400, 300)],
        );
        let browser = candidate(2, 1, rect(0, 0, 1_200, 900), vec![rect(0, 0, 1_200, 900)]);

        let source = inventory(vec![game, browser])
            .source_for(rect(220, 170, 360, 260))
            .unwrap();

        assert_eq!(source.identity.hwnd, 1);
        assert!(!source.partial);
    }

    #[test]
    fn peer_overlap_counts_only_each_windows_uncovered_region() {
        let top = candidate(10, 0, rect(100, 0, 200, 200), vec![rect(100, 0, 200, 200)]);
        let bottom = candidate(20, 1, rect(0, 0, 250, 200), vec![rect(0, 0, 250, 200)]);

        let source = inventory(vec![top, bottom])
            .source_for(rect(0, 0, 300, 200))
            .unwrap();

        assert_eq!(source.identity.hwnd, 10);
        assert!(source.partial);
    }

    #[test]
    fn shaped_window_does_not_occlude_pixels_outside_its_saved_region() {
        let shaped = candidate(1, 0, rect(0, 0, 200, 100), vec![rect(0, 0, 80, 100)]);
        let behind = candidate(2, 1, rect(0, 0, 200, 100), vec![rect(0, 0, 200, 100)]);

        let source = inventory(vec![shaped, behind])
            .source_for(rect(100, 0, 100, 100))
            .unwrap();

        assert_eq!(source.identity.hwnd, 2);
    }

    #[test]
    fn negative_monitor_coordinates_are_kept_in_physical_pixels() {
        let left = candidate(
            3,
            0,
            rect(-1_900, 20, 500, 400),
            vec![rect(-1_900, 20, 500, 400)],
        );

        let source = inventory(vec![left])
            .source_for(rect(-1_800, 40, 100, 80))
            .unwrap();

        assert_eq!(source.bounds.x, -1_900);
    }

    #[test]
    fn cloaked_tool_and_sayflex_windows_are_excluded_before_occlusion() {
        let excluded = [
            CandidateDisposition::Cloaked,
            CandidateDisposition::ToolWindow,
            CandidateDisposition::SayFlexOwned,
        ];
        for (index, disposition) in excluded.into_iter().enumerate() {
            let mut overlay = candidate(
                100 + index as u64,
                0,
                rect(0, 0, 100, 100),
                vec![rect(0, 0, 100, 100)],
            );
            overlay.disposition = disposition;
            let behind = candidate(9, 1, rect(0, 0, 100, 100), vec![rect(0, 0, 100, 100)]);

            let source = inventory(vec![overlay, behind])
                .source_for(rect(0, 0, 100, 100))
                .unwrap();

            assert_eq!(source.identity.hwnd, 9);
        }
    }

    #[test]
    fn center_owner_needs_a_one_point_five_area_lead() {
        let center_owner = candidate(1, 0, rect(30, 0, 45, 100), vec![rect(30, 0, 45, 100)]);
        let runner_up = candidate(2, 1, rect(0, 0, 30, 100), vec![rect(0, 0, 30, 100)]);
        let third = candidate(3, 2, rect(75, 0, 25, 100), vec![rect(75, 0, 25, 100)]);
        assert_eq!(
            inventory(vec![center_owner.clone(), runner_up, third])
                .source_for(rect(0, 0, 100, 100))
                .unwrap()
                .identity
                .hwnd,
            1
        );

        let ambiguous_runner = candidate(
            4,
            1,
            rect(0, 0, 80, 100),
            vec![rect(0, 0, 30, 100), rect(75, 0, 5, 100)],
        );
        let ambiguous_third = candidate(5, 2, rect(80, 0, 20, 100), vec![rect(80, 0, 20, 100)]);
        assert!(
            inventory(vec![center_owner, ambiguous_runner, ambiguous_third])
                .source_for(rect(0, 0, 100, 100))
                .is_none()
        );
    }

    #[test]
    fn an_even_split_is_ambiguous() {
        let left = candidate(1, 0, rect(0, 0, 50, 100), vec![rect(0, 0, 50, 100)]);
        let right = candidate(2, 1, rect(50, 0, 50, 100), vec![rect(50, 0, 50, 100)]);

        assert!(
            inventory(vec![left, right])
                .source_for(rect(0, 0, 100, 100))
                .is_none()
        );
    }
}
