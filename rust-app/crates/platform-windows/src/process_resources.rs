use std::{
    collections::{BTreeSet, HashMap},
    ptr,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Performance::{
            PDH_COUNTER_INFO_W, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE,
            PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddCounterW,
            PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhExpandWildCardPathW,
            PdhGetCounterInfoW, PdhGetFormattedCounterValue, PdhRemoveCounter,
        },
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{
            ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetProcessTimes, OpenProcess,
            PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        },
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcessResourceUsage {
    pub cpu_percent: Option<f32>,
    pub ram_bytes: u64,
    pub gpu_percent: Option<f32>,
    pub vram_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessNode {
    pub pid: u32,
    pub parent_pid: u32,
}

#[must_use]
pub fn aggregate_resource_usage(samples: &[ProcessResourceUsage]) -> ProcessResourceUsage {
    let cpu_values = samples
        .iter()
        .filter_map(|sample| sample.cpu_percent)
        .collect::<Vec<_>>();
    let gpu_values = samples
        .iter()
        .filter_map(|sample| sample.gpu_percent)
        .collect::<Vec<_>>();
    let vram_values = samples
        .iter()
        .filter_map(|sample| sample.vram_bytes)
        .collect::<Vec<_>>();
    ProcessResourceUsage {
        cpu_percent: (!cpu_values.is_empty())
            .then(|| cpu_values.into_iter().sum::<f32>().clamp(0.0, 100.0)),
        ram_bytes: samples.iter().map(|sample| sample.ram_bytes).sum(),
        gpu_percent: (!gpu_values.is_empty())
            .then(|| gpu_values.into_iter().sum::<f32>().clamp(0.0, 100.0)),
        vram_bytes: (!vram_values.is_empty()).then(|| vram_values.into_iter().sum()),
    }
}

#[must_use]
pub fn descendant_process_ids(root_pid: u32, processes: &[ProcessNode]) -> Vec<u32> {
    let mut result = BTreeSet::from([root_pid]);
    loop {
        let before = result.len();
        for process in processes {
            if result.contains(&process.parent_pid) {
                result.insert(process.pid);
            }
        }
        if result.len() == before {
            break;
        }
    }
    result.into_iter().collect()
}

#[must_use]
pub fn cpu_percent_between(
    previous_ticks: u64,
    current_ticks: u64,
    elapsed: Duration,
    logical_processors: u32,
) -> Option<f32> {
    let delta_ticks = current_ticks.checked_sub(previous_ticks)?;
    if elapsed.is_zero() || logical_processors == 0 {
        return None;
    }
    let cpu_seconds = delta_ticks as f64 / 10_000_000.0;
    let percent = cpu_seconds / elapsed.as_secs_f64() / f64::from(logical_processors) * 100.0;
    Some(percent.clamp(0.0, 100.0) as f32)
}

#[derive(Clone, Copy)]
struct PreviousCpuSample {
    ticks: u64,
    sampled_at: Instant,
}

pub struct ProcessResourceSampler {
    previous_cpu: HashMap<u32, PreviousCpuSample>,
    logical_processors: u32,
    gpu: Option<GpuQuery>,
}

const GPU_COUNTER_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

impl Default for ProcessResourceSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessResourceSampler {
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: The call has no pointer arguments and only reports the active
        // logical processor count across all processor groups.
        let logical_processors = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1);
        Self {
            previous_cpu: HashMap::new(),
            logical_processors,
            gpu: None,
        }
    }

    /// Samples each root and every process descended from it. This is required
    /// for local translation, whose `llama-server.exe` is a child of the worker.
    pub fn sample_groups(
        &mut self,
        root_pids: &[u32],
    ) -> Result<Vec<ProcessResourceUsage>, String> {
        let processes = process_snapshot()?;
        let groups = root_pids
            .iter()
            .map(|root| descendant_process_ids(*root, &processes))
            .collect::<Vec<_>>();
        let all_pids = groups
            .iter()
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let gpu = self.sample_gpu(&all_pids);
        let now = Instant::now();
        let mut per_process = HashMap::new();
        for pid in all_pids {
            let (ticks, ram_bytes) = sample_cpu_and_ram(pid).unwrap_or((0, 0));
            let cpu_percent = self.previous_cpu.get(&pid).and_then(|previous| {
                cpu_percent_between(
                    previous.ticks,
                    ticks,
                    now.saturating_duration_since(previous.sampled_at),
                    self.logical_processors,
                )
            });
            if ticks > 0 {
                self.previous_cpu.insert(
                    pid,
                    PreviousCpuSample {
                        ticks,
                        sampled_at: now,
                    },
                );
            }
            let (gpu_percent, vram_bytes) = gpu.get(&pid).copied().unwrap_or((None, None));
            per_process.insert(
                pid,
                ProcessResourceUsage {
                    cpu_percent,
                    ram_bytes,
                    gpu_percent,
                    vram_bytes,
                },
            );
        }
        self.previous_cpu
            .retain(|pid, _| per_process.contains_key(pid));
        Ok(groups
            .iter()
            .map(|group| {
                aggregate_resource_usage(
                    &group
                        .iter()
                        .filter_map(|pid| per_process.get(pid).copied())
                        .collect::<Vec<_>>(),
                )
            })
            .collect())
    }

    fn sample_gpu(&mut self, pids: &[u32]) -> HashMap<u32, (Option<f32>, Option<u64>)> {
        if pids.is_empty() {
            self.gpu = None;
            return HashMap::new();
        }
        let rebuild = self.gpu.as_ref().is_none_or(|query| {
            gpu_query_needs_refresh(
                &query.pids,
                pids,
                query.counters.is_empty(),
                query.created_at.elapsed(),
            )
        });
        if rebuild {
            self.gpu = GpuQuery::new(pids).ok();
        }
        self.gpu.as_mut().map(GpuQuery::sample).unwrap_or_default()
    }
}

fn filetime_ticks(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn sample_cpu_and_ram(pid: u32) -> Result<(u64, u64), String> {
    // SAFETY: The handle is opened read-only for a PID obtained from a process
    // snapshot and closed on every return path below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if process.is_null() {
        return Err(format!("could not open process {pid}"));
    }
    let sampled = (|| {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: all pointers refer to initialized writable FILETIME values
        // and the process handle remains open through the call.
        if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
            == 0
        {
            return Err(format!("could not read CPU time for process {pid}"));
        }
        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: `memory` has the documented structure size and the process
        // handle remains valid for the duration of the call.
        if unsafe {
            K32GetProcessMemoryInfo(
                process,
                (&raw mut memory).cast::<PROCESS_MEMORY_COUNTERS>(),
                size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        } == 0
        {
            return Err(format!("could not read memory for process {pid}"));
        }
        Ok((
            filetime_ticks(kernel).saturating_add(filetime_ticks(user)),
            memory.WorkingSetSize as u64,
        ))
    })();
    // SAFETY: `process` is a live handle returned by OpenProcess and is closed
    // exactly once here.
    unsafe {
        CloseHandle(process);
    }
    sampled
}

fn process_snapshot() -> Result<Vec<ProcessNode>, String> {
    // SAFETY: A system-wide process snapshot ignores the PID argument.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err("could not enumerate processes".to_owned());
    }
    let result = (|| {
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: entry has the required size and remains writable throughout
        // the Process32 iteration.
        if unsafe { Process32FirstW(snapshot, &mut entry) } == 0 {
            return Err("process enumeration returned no first item".to_owned());
        }
        let mut processes = Vec::new();
        loop {
            processes.push(ProcessNode {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
            });
            // SAFETY: same initialized snapshot and entry as above.
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                break;
            }
        }
        Ok(processes)
    })();
    // SAFETY: snapshot is a live ToolHelp handle and is closed exactly once.
    unsafe {
        CloseHandle(snapshot);
    }
    result
}

#[derive(Clone, Copy)]
enum GpuCounterKind {
    Utilization,
    DedicatedBytes,
}

struct GpuCounter {
    pid: u32,
    kind: GpuCounterKind,
    handle: PDH_HCOUNTER,
}

struct GpuQuery {
    handle: PDH_HQUERY,
    counters: Vec<GpuCounter>,
    pids: Vec<u32>,
    primed: bool,
    created_at: Instant,
}

impl GpuQuery {
    fn new(pids: &[u32]) -> Result<Self, String> {
        let mut handle: PDH_HQUERY = ptr::null_mut();
        // SAFETY: null data source selects the current machine; `handle` is a
        // valid out pointer.
        let status = unsafe {
            windows_sys::Win32::System::Performance::PdhOpenQueryW(ptr::null(), 0, &mut handle)
        };
        if status != 0 {
            return Err(format!("PDH query open failed: 0x{status:08X}"));
        }
        let mut query = Self {
            handle,
            counters: Vec::new(),
            pids: pids.to_vec(),
            primed: false,
            created_at: Instant::now(),
        };
        for &pid in pids {
            query.add_wildcard(
                pid,
                GpuCounterKind::DedicatedBytes,
                &format!(r"\GPU Process Memory(pid_{pid}_*)\Dedicated Usage"),
            );
            query.add_wildcard(
                pid,
                GpuCounterKind::Utilization,
                &format!(r"\GPU Engine(pid_{pid}_*)\Utilization Percentage"),
            );
        }
        Ok(query)
    }

    fn add_wildcard(&mut self, pid: u32, kind: GpuCounterKind, pattern: &str) {
        let mut api = WindowsPdhApi { query: self.handle };
        for counter in install_localized_wildcard(&mut api, pattern, pid as usize) {
            self.counters.push(GpuCounter {
                pid,
                kind,
                handle: counter,
            });
        }
    }

    fn sample(&mut self) -> HashMap<u32, (Option<f32>, Option<u64>)> {
        // SAFETY: handle is owned by self until Drop.
        let mut values = self
            .pids
            .iter()
            .copied()
            .map(|pid| (pid, (None, None)))
            .collect::<HashMap<_, _>>();
        if unsafe { PdhCollectQueryData(self.handle) } != 0 {
            return values;
        }
        let utilization_ready = std::mem::replace(&mut self.primed, true);
        for counter in &self.counters {
            let entry = values.entry(counter.pid).or_insert((None, None));
            match counter.kind {
                GpuCounterKind::Utilization if utilization_ready => {
                    if let Some(value) = formatted_double(counter.handle) {
                        entry.0 = Some(entry.0.unwrap_or(0.0).max(value as f32));
                    }
                }
                GpuCounterKind::DedicatedBytes => {
                    entry.1 = merge_optional_total(entry.1, formatted_large(counter.handle));
                }
                GpuCounterKind::Utilization => {}
            }
        }
        values
    }
}

fn gpu_query_needs_refresh(
    current_pids: &[u32],
    requested_pids: &[u32],
    counters_empty: bool,
    age: Duration,
) -> bool {
    current_pids != requested_pids || counters_empty || age >= GPU_COUNTER_REFRESH_INTERVAL
}

fn merge_optional_total(current: Option<u64>, sample: Option<u64>) -> Option<u64> {
    match (current, sample) {
        (None, None) => None,
        (Some(current), None) => Some(current),
        (None, Some(sample)) => Some(sample),
        (Some(current), Some(sample)) => Some(current.saturating_add(sample)),
    }
}

trait PdhWildcardApi {
    type Counter: Copy;

    fn add_english(&mut self, path: &str, user_data: usize) -> Option<Self::Counter>;
    fn localized_path(&mut self, counter: Self::Counter) -> Option<String>;
    fn remove(&mut self, counter: Self::Counter);
    fn expand_localized(&mut self, path: &str) -> Vec<String>;
    fn add_localized(&mut self, path: &str, user_data: usize) -> Option<Self::Counter>;
}

fn install_localized_wildcard<A: PdhWildcardApi>(
    api: &mut A,
    english_pattern: &str,
    user_data: usize,
) -> Vec<A::Counter> {
    let Some(probe) = api.add_english(english_pattern, user_data) else {
        return Vec::new();
    };
    let localized_pattern = api.localized_path(probe);
    api.remove(probe);
    let Some(localized_pattern) = localized_pattern else {
        return Vec::new();
    };
    api.expand_localized(&localized_pattern)
        .into_iter()
        .filter_map(|path| api.add_localized(&path, user_data))
        .collect()
}

struct WindowsPdhApi {
    query: PDH_HQUERY,
}

impl PdhWildcardApi for WindowsPdhApi {
    type Counter = PDH_HCOUNTER;

    fn add_english(&mut self, path: &str, user_data: usize) -> Option<Self::Counter> {
        let path = wide(path);
        let mut counter = ptr::null_mut();
        // SAFETY: the query is live, the path is NUL-terminated, and the out
        // pointer remains writable for the duration of the call.
        (unsafe { PdhAddEnglishCounterW(self.query, path.as_ptr(), user_data, &mut counter) } == 0)
            .then_some(counter)
    }

    fn localized_path(&mut self, counter: Self::Counter) -> Option<String> {
        let mut bytes = 0_u32;
        // SAFETY: the first call intentionally requests the required byte
        // count without providing a destination buffer.
        if unsafe { PdhGetCounterInfoW(counter, false, &mut bytes, ptr::null_mut()) }
            != PDH_MORE_DATA
            || bytes == 0
        {
            return None;
        }
        let words = (bytes as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let info = storage.as_mut_ptr().cast::<PDH_COUNTER_INFO_W>();
        // SAFETY: `storage` is pointer-aligned and at least the byte count PDH
        // requested. PDH owns no part of the buffer after the call returns.
        if unsafe { PdhGetCounterInfoW(counter, false, &mut bytes, info) } != 0 {
            return None;
        }
        // SAFETY: a successful call populated `info`; `szFullPath` points into
        // `storage` and is NUL-terminated by PDH.
        let path = unsafe { (*info).szFullPath };
        if path.is_null() {
            return None;
        }
        let maximum_units = bytes as usize / size_of::<u16>();
        let length = (0..maximum_units).position(|index| unsafe { *path.add(index) } == 0)?;
        // SAFETY: the terminator search above bounded the slice to this PDH
        // output allocation.
        Some(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(path, length)
        }))
    }

    fn remove(&mut self, counter: Self::Counter) {
        // SAFETY: the probe belongs to this query and is removed exactly once.
        unsafe {
            PdhRemoveCounter(counter);
        }
    }

    fn expand_localized(&mut self, path: &str) -> Vec<String> {
        expand_pdh_path(path)
    }

    fn add_localized(&mut self, path: &str, user_data: usize) -> Option<Self::Counter> {
        let path = wide(path);
        let mut counter = ptr::null_mut();
        // SAFETY: this path is already localized and fully expanded, and the
        // query and out pointer remain valid through the call.
        (unsafe { PdhAddCounterW(self.query, path.as_ptr(), user_data, &mut counter) } == 0)
            .then_some(counter)
    }
}

impl Drop for GpuQuery {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the query is owned by self and closed exactly once.
            unsafe {
                PdhCloseQuery(self.handle);
            }
        }
    }
}

fn expand_pdh_path(pattern: &str) -> Vec<String> {
    let pattern = wide(pattern);
    let mut length = 0u32;
    // SAFETY: the first call intentionally supplies no output buffer and asks
    // PDH for the number of UTF-16 code units required.
    let first = unsafe {
        PdhExpandWildCardPathW(
            ptr::null(),
            pattern.as_ptr(),
            ptr::null_mut(),
            &mut length,
            0,
        )
    };
    if first != PDH_MORE_DATA || length == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: buffer contains the exact capacity returned by the sizing call.
    if unsafe {
        PdhExpandWildCardPathW(
            ptr::null(),
            pattern.as_ptr(),
            buffer.as_mut_ptr(),
            &mut length,
            0,
        )
    } != 0
    {
        return Vec::new();
    }
    parse_multi_string(&buffer)
}

fn parse_multi_string(buffer: &[u16]) -> Vec<String> {
    buffer
        .split(|unit| *unit == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

fn formatted_double(counter: PDH_HCOUNTER) -> Option<f64> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: counter belongs to a live query and value is a writable output.
    if unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, ptr::null_mut(), &mut value) }
        != 0
        || !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
    {
        return None;
    }
    // SAFETY: PDH wrote the union member selected by PDH_FMT_DOUBLE.
    Some(unsafe { value.Anonymous.doubleValue })
}

fn formatted_large(counter: PDH_HCOUNTER) -> Option<u64> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: counter belongs to a live query and value is a writable output.
    if unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_LARGE, ptr::null_mut(), &mut value) }
        != 0
        || !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
    {
        return None;
    }
    // SAFETY: PDH wrote the union member selected by PDH_FMT_LARGE.
    u64::try_from(unsafe { value.Anonymous.largeValue }).ok()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        GPU_COUNTER_REFRESH_INTERVAL, GpuQuery, PdhWildcardApi, gpu_query_needs_refresh,
        install_localized_wildcard, merge_optional_total,
    };

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        AddEnglish(String),
        LocalizedInfo,
        RemoveEnglish,
        LocalizedExpand(String),
        AddLocalized(String),
    }

    struct FakePdh {
        calls: Vec<Call>,
    }

    impl PdhWildcardApi for FakePdh {
        type Counter = u64;

        fn add_english(&mut self, path: &str, _: usize) -> Option<Self::Counter> {
            self.calls.push(Call::AddEnglish(path.to_owned()));
            Some(1)
        }

        fn localized_path(&mut self, _: Self::Counter) -> Option<String> {
            self.calls.push(Call::LocalizedInfo);
            Some(r"\GPU Engine(pid_42_*)\Utilisation".to_owned())
        }

        fn remove(&mut self, _: Self::Counter) {
            self.calls.push(Call::RemoveEnglish);
        }

        fn expand_localized(&mut self, path: &str) -> Vec<String> {
            self.calls.push(Call::LocalizedExpand(path.to_owned()));
            vec!["counter-a".to_owned(), "counter-b".to_owned()]
        }

        fn add_localized(&mut self, path: &str, _: usize) -> Option<Self::Counter> {
            self.calls.push(Call::AddLocalized(path.to_owned()));
            Some(self.calls.len() as u64)
        }
    }

    #[test]
    fn localized_wildcard_setup_uses_the_documented_pdh_order() {
        let mut api = FakePdh { calls: Vec::new() };

        let counters = install_localized_wildcard(
            &mut api,
            r"\GPU Engine(pid_42_*)\Utilization Percentage",
            42,
        );

        assert_eq!(counters.len(), 2);
        assert_eq!(
            api.calls,
            vec![
                Call::AddEnglish(r"\GPU Engine(pid_42_*)\Utilization Percentage".to_owned()),
                Call::LocalizedInfo,
                Call::RemoveEnglish,
                Call::LocalizedExpand(r"\GPU Engine(pid_42_*)\Utilisation".to_owned()),
                Call::AddLocalized("counter-a".to_owned()),
                Call::AddLocalized("counter-b".to_owned()),
            ]
        );
    }

    #[test]
    fn unchanged_pids_refresh_empty_and_late_gpu_instances() {
        let pids = [42];
        assert!(gpu_query_needs_refresh(&pids, &pids, true, Duration::ZERO));
        assert!(!gpu_query_needs_refresh(
            &pids,
            &pids,
            false,
            GPU_COUNTER_REFRESH_INTERVAL - Duration::from_millis(1)
        ));
        assert!(gpu_query_needs_refresh(
            &pids,
            &pids,
            false,
            GPU_COUNTER_REFRESH_INTERVAL
        ));
    }

    #[test]
    fn an_unavailable_counter_is_not_reported_as_zero() {
        assert_eq!(merge_optional_total(None, None), None);
        assert_eq!(merge_optional_total(None, Some(0)), Some(0));
        assert_eq!(merge_optional_total(Some(7), None), Some(7));
        assert_eq!(merge_optional_total(Some(7), Some(5)), Some(12));
    }

    #[test]
    #[ignore = "requires an interactive Windows session and a PID with a GPU counter instance"]
    fn live_pdh_discovers_gpu_instances_for_a_known_pid() {
        let pid = std::env::var("SAYFLEX_GPU_TEST_PID")
            .expect("set SAYFLEX_GPU_TEST_PID to a PID listed by the GPU Engine counter set")
            .parse::<u32>()
            .expect("SAYFLEX_GPU_TEST_PID must be an unsigned integer");
        let mut query = GpuQuery::new(&[pid]).expect("the live PDH query must open");

        assert!(
            !query.counters.is_empty(),
            "the localized wildcard path must discover live GPU counters for PID {pid}"
        );
        let _ = query.sample();
        std::thread::sleep(Duration::from_millis(100));
        let sampled = query.sample();
        let (gpu, vram) = sampled
            .get(&pid)
            .expect("the requested PID must remain explicit in the sample");
        assert!(
            gpu.is_some() || vram.is_some(),
            "at least one live GPU metric must be available for PID {pid}"
        );
    }
}
