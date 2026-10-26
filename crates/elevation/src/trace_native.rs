//! Fixed private SystemTraceProvider collection. Native activation fails closed until
//! cleanup across forced helper termination has been proven for this operation.
use super::{
    native::is_elevated,
    protocol::{ElevationError, ElevationRequest, ElevationStatus},
    trace::{
        validate_duration, Accumulator, NumericEvent, TraceClass, TraceLimit, TraceSummary,
        TRACE_BUFFER_KIB, TRACE_MAX_BUFFERS,
    },
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    mem::{offset_of, size_of},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_SUCCESS,
            ERROR_WMI_INSTANCE_NOT_FOUND,
        },
        System::{
            Diagnostics::Etw::*,
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        },
    },
};

const PERF_INFO: GUID = GUID::from_u128(0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc);
const DISK_IO: GUID = GUID::from_u128(0x3d6fa8d4_fe05_11d0_9dda_00c04fd7ba7c);
const MAX_METADATA_BYTES: usize = 32 * 1024;
const MAX_SCHEMA_PROPERTIES: usize = 32;
const MAX_PAYLOAD_BYTES: usize = 4096;

fn same_guid(left: &GUID, right: &GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}
fn random_ref(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_prefix('-'))
        .is_some_and(|suffix| {
            suffix.len() == 36
                && suffix.bytes().enumerate().all(|(index, byte)| {
                    if [8, 13, 18, 23].contains(&index) {
                        byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
        })
}

struct Identity {
    name: Vec<u16>,
    guid: GUID,
}
impl Identity {
    fn request(request: &ElevationRequest) -> Result<Self, ElevationError> {
        if !random_ref(&request.job_id, "elevjob") || !random_ref(&request.nonce, "nonce") {
            return Err(ElevationError::InvalidRequest);
        }
        let mut hash = Sha256::new();
        hash.update(b"pcagent-private-etw-v1\0");
        hash.update(request.job_id.as_bytes());
        hash.update([0]);
        hash.update(request.nonce.as_bytes());
        let bytes = hash.finalize();
        let mut guid = GUID::from_u128(u128::from_be_bytes(bytes[..16].try_into().unwrap()));
        guid.data3 = (guid.data3 & 0x0fff) | 0x4000;
        guid.data4[0] = (guid.data4[0] & 0x3f) | 0x80;
        let name = format!("pcagent-trace-{}", &request.job_id[8..])
            .encode_utf16()
            .chain(Some(0))
            .collect();
        Ok(Self { name, guid })
    }
    fn matches(&self, properties: &EVENT_TRACE_PROPERTIES) -> bool {
        same_guid(&self.guid, &properties.Wnode.Guid)
            && properties.LogFileMode & EVENT_TRACE_SYSTEM_LOGGER_MODE != 0
            && properties.LogFileMode & EVENT_TRACE_REAL_TIME_MODE != 0
    }
}

/// Non-constructible outside this module; no environment/configuration switch bypasses preflight.
struct CleanupProof;
fn proven_cleanup_for_operation(
    _request: &ElevationRequest,
    _owner_sid: &str,
) -> Option<CleanupProof> {
    // Pending native acceptance: QUERY/STOP/permission removal by an unelevated parent,
    // plus an armed privileged guardian before StartTrace to close the start/proof kill gap.
    // Neither permission changes nor a session may be started while that proof is absent.
    None
}

struct Properties {
    words: Vec<u64>,
}
impl Properties {
    fn new(identity: &Identity) -> Self {
        let length = size_of::<EVENT_TRACE_PROPERTIES>() + 2 * 1024 * size_of::<u16>();
        let mut result = Self {
            words: vec![0; length.div_ceil(size_of::<u64>())],
        };
        let bytes = result.words.len() * size_of::<u64>();
        let properties = result.get_mut();
        properties.Wnode.BufferSize = bytes as u32;
        properties.Wnode.Guid = identity.guid;
        properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        properties.Wnode.ClientContext = 1; // QPC, consumed without timestamp conversion.
        properties.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        properties.BufferSize = TRACE_BUFFER_KIB;
        properties.MinimumBuffers = 4;
        properties.MaximumBuffers = TRACE_MAX_BUFFERS;
        properties.LogFileMode = EVENT_TRACE_SYSTEM_LOGGER_MODE
            | EVENT_TRACE_REAL_TIME_MODE
            | EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING;
        properties.FlushTimer = 1;
        properties.EnableFlags = EVENT_TRACE_FLAG_DPC
            | EVENT_TRACE_FLAG_INTERRUPT
            | EVENT_TRACE_FLAG_DISK_IO
            | EVENT_TRACE_FLAG_NO_SYSCONFIG;
        result
    }
    fn get_mut(&mut self) -> &mut EVENT_TRACE_PROPERTIES {
        // SAFETY: u64 storage provides native alignment and enough initialized bytes; never resized.
        unsafe { &mut *self.words.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>() }
    }
}

struct OwnedSession {
    identity: Identity,
    handle: CONTROLTRACE_HANDLE,
}
static ACTIVE: Mutex<Option<OwnedSession>> = Mutex::new(None);

fn stop_session(session: &OwnedSession) -> Result<Option<(u64, u64)>, ElevationError> {
    let mut properties = Properties::new(&session.identity);
    // SAFETY: query only the handle/name returned for our successful StartTrace; bounded output.
    let queried = unsafe {
        ControlTraceW(
            session.handle,
            session.identity.name.as_ptr(),
            properties.get_mut(),
            EVENT_TRACE_CONTROL_QUERY,
        )
    };
    if queried == ERROR_WMI_INSTANCE_NOT_FOUND {
        return Ok(None);
    }
    if queried != ERROR_SUCCESS {
        return Err(ElevationError::LaunchFailed(queried));
    }
    if !session.identity.matches(properties.get_mut()) {
        return Err(ElevationError::InsecureJob);
    }
    let mut final_properties = Properties::new(&session.identity);
    // SAFETY: the successful creation handle and exact queried private identity are owned here;
    // never use NT Kernel Logger, SystemTraceControlGuid or an unrelated queried session.
    let stopped = unsafe {
        ControlTraceW(
            session.handle,
            session.identity.name.as_ptr(),
            final_properties.get_mut(),
            EVENT_TRACE_CONTROL_STOP,
        )
    };
    if stopped == ERROR_WMI_INSTANCE_NOT_FOUND || stopped == ERROR_MORE_DATA {
        // STOP succeeded for MORE_DATA, but its incomplete output does not prove zero losses.
        return Ok(None);
    }
    if stopped != ERROR_SUCCESS {
        return Err(ElevationError::LaunchFailed(stopped));
    }
    let properties = final_properties.get_mut();
    Ok(Some((
        u64::from(properties.EventsLost),
        u64::from(properties.RealTimeBuffersLost),
    )))
}

/// Root calls this before every watchdog/process::exit path; a no-session call has no native effect.
pub(crate) fn abort_owned_session() -> Result<(), ElevationError> {
    finish_owned().map(|_| ())
}
fn finish_owned() -> Result<Option<(u64, u64)>, ElevationError> {
    let mut active = ACTIVE.lock().map_err(|_| ElevationError::InsecureJob)?;
    let Some(session) = active.as_ref() else {
        return Ok(None);
    };
    let lost = stop_session(session)?;
    active.take();
    Ok(lost)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceCleanupStatus {
    NotRunning,
    Stopped,
    IdentityMismatch,
    Unvalidated,
}

/// Local-only parent recovery seam. It is deliberately inactive until forced-exit cleanup is certified.
pub(crate) fn cleanup_for_request(
    request: &ElevationRequest,
    owner_sid: &str,
) -> Result<TraceCleanupStatus, ElevationError> {
    let identity = Identity::request(request)?;
    if proven_cleanup_for_operation(request, owner_sid).is_none() {
        return Ok(TraceCleanupStatus::Unvalidated);
    }
    let mut properties = Properties::new(&identity);
    // SAFETY: only the fixed name derived from this private approved job is queried, never enumeration.
    let queried = unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            identity.name.as_ptr(),
            properties.get_mut(),
            EVENT_TRACE_CONTROL_QUERY,
        )
    };
    if queried == ERROR_WMI_INSTANCE_NOT_FOUND {
        return Ok(TraceCleanupStatus::NotRunning);
    }
    if queried != ERROR_SUCCESS {
        return Err(ElevationError::LaunchFailed(queried));
    }
    if !identity.matches(properties.get_mut()) {
        return Ok(TraceCleanupStatus::IdentityMismatch);
    }
    let mut final_properties = Properties::new(&identity);
    // SAFETY: matching secret-derived GUID/private name after certified successful-creation proof only.
    let stopped = unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            identity.name.as_ptr(),
            final_properties.get_mut(),
            EVENT_TRACE_CONTROL_STOP,
        )
    };
    match stopped {
        ERROR_SUCCESS | ERROR_MORE_DATA => Ok(TraceCleanupStatus::Stopped),
        ERROR_WMI_INSTANCE_NOT_FOUND => Ok(TraceCleanupStatus::NotRunning),
        error => Err(ElevationError::LaunchFailed(error)),
    }
}

fn classify(provider: &GUID, opcode: u8) -> Option<TraceClass> {
    if same_guid(provider, &PERF_INFO) {
        match opcode {
            66 | 68 | 69 => Some(TraceClass::Dpc),
            67 => Some(TraceClass::Isr),
            _ => None,
        }
    } else if same_guid(provider, &DISK_IO) {
        match opcode {
            10 => Some(TraceClass::DiskRead),
            11 => Some(TraceClass::DiskWrite),
            _ => None,
        }
    } else {
        None
    }
}
fn required_properties(class: TraceClass) -> &'static [(&'static str, usize)] {
    match class {
        TraceClass::Dpc | TraceClass::Isr => &[("InitialTime", 8)],
        TraceClass::DiskRead | TraceClass::DiskWrite => {
            &[("TransferSize", 4), ("HighResResponseTime", 8)]
        }
    }
}

fn property_name(words: &[u64], offset: u32, byte_length: usize) -> Option<String> {
    let offset = offset as usize;
    if !offset.is_multiple_of(2) || offset >= byte_length {
        return None;
    }
    let maximum = ((byte_length - offset) / 2).min(65);
    // SAFETY: metadata offsets/length are checked against the allocated initialized TDH buffer.
    let units = unsafe {
        std::slice::from_raw_parts(
            words.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
            maximum,
        )
    };
    let end = units.iter().position(|unit| *unit == 0)?;
    String::from_utf16(&units[..end]).ok()
}
fn schema_valid(record: &EVENT_RECORD, class: TraceClass) -> bool {
    if record.EventHeader.EventDescriptor.Version != 2
        || u32::from(record.EventHeader.Flags) & EVENT_HEADER_FLAG_CLASSIC_HEADER == 0
        || usize::from(record.UserDataLength) > MAX_PAYLOAD_BYTES
        || record.UserData.is_null()
    {
        return false;
    }
    let mut bytes = 0;
    // SAFETY: callback's live record; first call queries bounded required metadata size only.
    let queried = unsafe { TdhGetEventInformation(record, 0, null(), null_mut(), &mut bytes) };
    if queried != ERROR_INSUFFICIENT_BUFFER
        || bytes as usize > MAX_METADATA_BYTES
        || (bytes as usize) < size_of::<TRACE_EVENT_INFO>()
    {
        return false;
    }
    let mut words = vec![0_u64; (bytes as usize).div_ceil(size_of::<u64>())];
    let capacity = words.len() * size_of::<u64>();
    // SAFETY: aligned writable buffer with exactly the requested bounded metadata capacity.
    let status =
        unsafe { TdhGetEventInformation(record, 0, null(), words.as_mut_ptr().cast(), &mut bytes) };
    if status != ERROR_SUCCESS
        || bytes as usize > capacity
        || (bytes as usize) < size_of::<TRACE_EVENT_INFO>()
    {
        return false;
    }
    metadata_valid(record, class, &words, bytes as usize)
}
fn metadata_valid(
    record: &EVENT_RECORD,
    class: TraceClass,
    words: &[u64],
    byte_length: usize,
) -> bool {
    if byte_length < size_of::<TRACE_EVENT_INFO>()
        || byte_length > MAX_METADATA_BYTES
        || byte_length > std::mem::size_of_val(words)
    {
        return false;
    }
    // SAFETY: the successful TDH write contains the checked fixed header and remains live below.
    let info = unsafe { &*words.as_ptr().cast::<TRACE_EVENT_INFO>() };
    if !same_guid(&info.ProviderGuid, &record.EventHeader.ProviderId)
        || info.EventDescriptor.Version != record.EventHeader.EventDescriptor.Version
        || info.EventDescriptor.Opcode != record.EventHeader.EventDescriptor.Opcode
        || info.DecodingSource != DecodingSourceWbem
        || info.TopLevelPropertyCount as usize > MAX_SCHEMA_PROPERTIES
        || info.PropertyCount != info.TopLevelPropertyCount
    {
        return false;
    }
    let count = info.TopLevelPropertyCount as usize;
    let offset = offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
    let Some(end) = count
        .checked_mul(size_of::<EVENT_PROPERTY_INFO>())
        .and_then(|length| offset.checked_add(length))
    else {
        return false;
    };
    if end > byte_length {
        return false;
    }
    // SAFETY: the flexible property array length/offset was checked against the successful TDH output.
    let properties = unsafe {
        std::slice::from_raw_parts(
            words
                .as_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<EVENT_PROPERTY_INFO>(),
            count,
        )
    };
    required_properties(class).iter().all(|(name, length)| {
        let matches: Vec<_> = properties
            .iter()
            .filter(|property| {
                property_name(words, property.NameOffset, byte_length).as_deref() == Some(*name)
            })
            .collect();
        if matches.len() != 1 {
            return false;
        }
        let property = matches[0];
        if property.Flags != 0 {
            return false;
        }
        // SAFETY: zero property flags select the scalar non-struct/count members; numeric types only.
        let (kind, count) = unsafe {
            (
                property.Anonymous1.nonStructType.InType,
                property.Anonymous2.count,
            )
        };
        count == 1
            && if *length == 4 {
                kind == TDH_INTYPE_UINT32 as u16
            } else {
                kind == TDH_INTYPE_UINT64 as u16
                    || (*name == "InitialTime" && kind == TDH_INTYPE_FILETIME as u16)
            }
    })
}
fn numeric_property(record: &EVENT_RECORD, name: &str, bytes: usize) -> Option<u64> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let descriptor = PROPERTY_DATA_DESCRIPTOR {
        PropertyName: name.as_ptr() as u64,
        ArrayIndex: u32::MAX,
        Reserved: 0,
    };
    let mut length = 0;
    // SAFETY: fixed allowlisted property name; live record and bounded scalar size output.
    if unsafe { TdhGetPropertySize(record, 0, null(), 1, &descriptor, &mut length) }
        != ERROR_SUCCESS
        || length as usize != bytes
        || !matches!(bytes, 4 | 8)
    {
        return None;
    }
    let mut value = [0_u8; 8];
    // SAFETY: exact checked 4/8-byte scalar output; routine/IRP/file pointers are never requested.
    if unsafe {
        TdhGetProperty(
            record,
            0,
            null(),
            1,
            &descriptor,
            length,
            value.as_mut_ptr(),
        )
    } != ERROR_SUCCESS
    {
        return None;
    }
    Some(u64::from_le_bytes(value))
}

struct Consumer {
    accumulator: Mutex<Option<Accumulator>>,
    schemas: Mutex<BTreeMap<(u8, u8, u8), bool>>,
    failed: AtomicBool,
    trace_handle: AtomicU64,
    completed: AtomicBool,
}
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // SAFETY: ProcessTrace owns this live callback record; null is refused before dereference.
    let Some(record) = (unsafe { record.as_ref() }) else {
        return;
    };
    // SAFETY: Context is an Arc-held Consumer pointer installed by our consumer thread until ProcessTrace returns.
    let Some(consumer) = (unsafe { record.UserContext.cast::<Consumer>().as_ref() }) else {
        return;
    };
    let processed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(mut guard) = consumer.accumulator.lock() else {
            consumer.failed.store(true, Ordering::Release);
            return;
        };
        let Some(accumulator) = guard.as_mut() else {
            return;
        };
        let Some(class) = classify(
            &record.EventHeader.ProviderId,
            record.EventHeader.EventDescriptor.Opcode,
        ) else {
            accumulator.ignored_event();
            return;
        };
        if accumulator.limit_reached() {
            return;
        }
        let provider = if same_guid(&record.EventHeader.ProviderId, &PERF_INFO) {
            0
        } else {
            1
        };
        let key = (
            provider,
            record.EventHeader.EventDescriptor.Opcode,
            record.EventHeader.EventDescriptor.Version,
        );
        let Ok(mut schemas) = consumer.schemas.lock() else {
            consumer.failed.store(true, Ordering::Release);
            return;
        };
        // At most six recognized version-2 schemas; other versions are refused without TDH calls/cache growth.
        let valid = record.EventHeader.EventDescriptor.Version == 2
            && u32::from(record.EventHeader.Flags) & EVENT_HEADER_FLAG_CLASSIC_HEADER != 0
            && usize::from(record.UserDataLength) <= MAX_PAYLOAD_BYTES
            && !record.UserData.is_null()
            && *schemas
                .entry(key)
                .or_insert_with(|| schema_valid(record, class));
        let at_qpc = u64::try_from(record.EventHeader.TimeStamp).ok();
        let disk = matches!(class, TraceClass::DiskRead | TraceClass::DiskWrite);
        let initial = (valid && !disk)
            .then(|| numeric_property(record, "InitialTime", 8))
            .flatten();
        let response = (valid && disk)
            .then(|| numeric_property(record, "HighResResponseTime", 8))
            .flatten();
        let transfer = (valid && disk)
            .then(|| numeric_property(record, "TransferSize", 4))
            .flatten()
            .and_then(|value| u32::try_from(value).ok());
        accumulator.observe(NumericEvent {
            class,
            schema_supported: valid
                && at_qpc.is_some()
                && if matches!(class, TraceClass::Dpc | TraceClass::Isr) {
                    initial.is_some()
                } else {
                    response.is_some() && transfer.is_some()
                },
            at_qpc: at_qpc.unwrap_or(0),
            initial_qpc: initial,
            response_ticks: response,
            transfer_bytes: transfer,
        });
    }));
    if processed.is_err() {
        consumer.failed.store(true, Ordering::Release);
    }
}
fn qpc(frequency: bool) -> Result<u64, ElevationError> {
    let mut value = 0;
    // SAFETY: exact initialized signed counter output; only QPC or its conversion frequency is queried.
    let status = unsafe {
        if frequency {
            QueryPerformanceFrequency(&mut value)
        } else {
            QueryPerformanceCounter(&mut value)
        }
    };
    if status == 0 || value < 0 || (frequency && value == 0) {
        return Err(ElevationError::Unsupported);
    }
    u64::try_from(value).map_err(|_| ElevationError::Unsupported)
}
fn close_consumer(consumer: &Consumer) {
    let value = consumer.trace_handle.swap(u64::MAX, Ordering::AcqRel);
    if value != u64::MAX {
        // SAFETY: atomic swap claims this owned OpenTrace consumer handle once; never a controller session.
        unsafe {
            let _ = CloseTrace(PROCESSTRACE_HANDLE { Value: value });
        }
    }
}

pub(crate) fn collect(
    request: &ElevationRequest,
    owner_sid: &str,
    duration: Duration,
    cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<TraceSummary, ElevationError> {
    validate_duration(duration)?;
    Identity::request(request)?;
    if cancel()? {
        return Ok(TraceSummary::unavailable(
            duration,
            ElevationStatus::Cancelled,
            TraceLimit::Cancelled,
        ));
    }
    let Some(proof) = proven_cleanup_for_operation(request, owner_sid) else {
        return Ok(TraceSummary::unavailable(
            duration,
            ElevationStatus::Unsupported,
            TraceLimit::CleanupUnvalidated,
        ));
    };
    collect_prevalidated(request, duration, cancel, proof)
}
fn collect_prevalidated(
    request: &ElevationRequest,
    duration: Duration,
    cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
    _proof: CleanupProof,
) -> Result<TraceSummary, ElevationError> {
    if !is_elevated()? {
        return Ok(TraceSummary::unavailable(
            duration,
            ElevationStatus::Denied,
            TraceLimit::ProviderUnavailable,
        ));
    }
    let identity = Identity::request(request)?;
    let name = identity.name.clone();
    let mut properties = Properties::new(&identity);
    let mut handle = CONTROLTRACE_HANDLE::default();
    // Include creation/consumer setup in the fixed duration instead of recording an extra window.
    let started = Instant::now();
    {
        let mut active = ACTIVE.lock().map_err(|_| ElevationError::InsecureJob)?;
        if active.is_some() {
            return Err(ElevationError::InsecureJob);
        }
        // SAFETY: only a new private system logger with fixed numeric flags and certified cleanup proof.
        let status =
            unsafe { StartTraceW(&mut handle, identity.name.as_ptr(), properties.get_mut()) };
        if status != ERROR_SUCCESS {
            return Ok(TraceSummary::unavailable(
                duration,
                ElevationStatus::Unsupported,
                if status == ERROR_ALREADY_EXISTS {
                    TraceLimit::SessionConflict
                } else {
                    TraceLimit::ProviderUnavailable
                },
            ));
        }
        *active = Some(OwnedSession { identity, handle });
    }
    // Every subsequent return uses the guard, in addition to root's process::exit abort hook.
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = abort_owned_session();
        }
    }
    let _guard = Guard;
    if properties.get_mut().BufferSize > TRACE_BUFFER_KIB
        || properties.get_mut().NumberOfBuffers > TRACE_MAX_BUFFERS
        || properties.get_mut().MaximumBuffers > TRACE_MAX_BUFFERS
    {
        abort_owned_session()?;
        return Ok(TraceSummary::unavailable(
            duration,
            ElevationStatus::Unsupported,
            TraceLimit::BufferLimit,
        ));
    }
    let start = qpc(false)?;
    let frequency = qpc(true)?;
    let consumer = Arc::new(Consumer {
        accumulator: Mutex::new(Some(Accumulator::new(duration, start, frequency)?)),
        schemas: Mutex::new(BTreeMap::new()),
        failed: AtomicBool::new(false),
        trace_handle: AtomicU64::new(u64::MAX),
        completed: AtomicBool::new(false),
    });
    let worker = Arc::clone(&consumer);
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut name = name;
        let mut logfile = EVENT_TRACE_LOGFILEW {
            LoggerName: name.as_mut_ptr(),
            Context: Arc::as_ptr(&worker) as *mut _,
            ..Default::default()
        };
        logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME
            | PROCESS_TRACE_MODE_EVENT_RECORD
            | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
        logfile.Anonymous2.EventRecordCallback = Some(on_event);
        // SAFETY: initialized real-time consumer for our private name; Arc keeps callback context alive.
        let trace = unsafe { OpenTraceW(&mut logfile) };
        if trace.Value == u64::MAX {
            worker.completed.store(true, Ordering::Release);
            let _ = sender.send(false);
            return;
        }
        worker.trace_handle.store(trace.Value, Ordering::Release);
        if sender.send(true).is_err() {
            close_consumer(&worker);
            worker.completed.store(true, Ordering::Release);
            return;
        }
        // SAFETY: one owned consumer handle and raw QPC mode; all callbacks finish before Arc is dropped.
        let result = unsafe { ProcessTrace(&trace, 1, null(), null()) };
        if result != ERROR_SUCCESS && result != 1223 {
            worker.failed.store(true, Ordering::Release);
        }
        close_consumer(&worker);
        worker.completed.store(true, Ordering::Release);
    });
    if receiver.recv_timeout(Duration::from_secs(1)) != Ok(true) {
        abort_owned_session()?;
        close_consumer(&consumer);
        return Ok(TraceSummary::unavailable(
            duration,
            ElevationStatus::Unsupported,
            TraceLimit::ConsumerUnavailable,
        ));
    }
    let mut terminal = None;
    loop {
        if cancel()? {
            terminal = Some(TraceLimit::Cancelled);
            break;
        }
        if consumer.failed.load(Ordering::Acquire) {
            terminal = Some(TraceLimit::ConsumerFailed);
            break;
        }
        let limited = consumer
            .accumulator
            .lock()
            .map_err(|_| ElevationError::InsecureJob)?
            .as_ref()
            .is_some_and(Accumulator::limit_reached);
        if limited {
            terminal = Some(TraceLimit::EventLimit);
            break;
        }
        if started.elapsed() >= duration {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let end = qpc(false)?;
    let stopped = finish_owned();
    let lost = stopped?;
    let drain_started = Instant::now();
    while !consumer.completed.load(Ordering::Acquire)
        && drain_started.elapsed() < Duration::from_secs(2)
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let incomplete_drain = !consumer.completed.load(Ordering::Acquire);
    close_consumer(&consumer);
    // A blocked TDH callback can still own this lock; never turn the drain deadline into an unbounded wait.
    let mut accumulator = match consumer.accumulator.try_lock() {
        Ok(mut guard) => guard.take().ok_or(ElevationError::InsecureJob)?,
        Err(std::sync::TryLockError::WouldBlock) => {
            return Ok(TraceSummary::unavailable(
                duration,
                ElevationStatus::Unsupported,
                TraceLimit::ConsumerDrainIncomplete,
            ));
        }
        Err(std::sync::TryLockError::Poisoned(_)) => return Err(ElevationError::InsecureJob),
    };
    if incomplete_drain {
        accumulator.limit(TraceLimit::ConsumerDrainIncomplete);
    }
    let summary = accumulator.finish(end, lost, true, terminal);
    summary.validate()?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{ElevatedTask, ElevationRequest};
    use super::*;
    #[test]
    fn request_identity_is_local_secret_bound_and_never_a_global_logger() {
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(60)).unwrap();
        let first = Identity::request(&request).unwrap();
        let mut changed = request.clone();
        changed.nonce = pcagent_shared::new_id("nonce");
        let second = Identity::request(&changed).unwrap();
        assert!(!same_guid(&first.guid, &second.guid));
        assert!(!same_guid(&first.guid, &SystemTraceControlGuid));
        let name = String::from_utf16(&first.name[..first.name.len() - 1]).unwrap();
        assert!(name.starts_with("pcagent-trace-"));
        assert!(!name.contains(&request.nonce));
        let mut properties = Properties::new(&first);
        assert!(first.matches(properties.get_mut()));
        properties.get_mut().Wnode.Guid = second.guid;
        assert!(!first.matches(properties.get_mut()));
        assert_eq!(
            properties.get_mut().EnableFlags
                & (EVENT_TRACE_FLAG_DISK_FILE_IO
                    | EVENT_TRACE_FLAG_IMAGE_LOAD
                    | EVENT_TRACE_FLAG_FILE_IO),
            0
        );
    }
    #[test]
    fn safe_default_refuses_before_etw_or_permission_effects() {
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(60)).unwrap();
        assert!(proven_cleanup_for_operation(&request, "S-1-5-21-1").is_none());
        let summary = collect(&request, "S-1-5-21-1", Duration::from_secs(30), &mut || {
            Ok(false)
        })
        .unwrap();
        summary.validate().unwrap();
        assert_eq!(summary.limitations, [TraceLimit::CleanupUnvalidated]);
        assert!(ACTIVE.lock().unwrap().is_none());
        assert_eq!(
            cleanup_for_request(&request, "S-1-5-21-1").unwrap(),
            TraceCleanupStatus::Unvalidated
        );
        abort_owned_session().unwrap();
    }
    #[test]
    fn decode_allowlist_has_no_file_or_routine_addresses() {
        assert_eq!(classify(&PERF_INFO, 66), Some(TraceClass::Dpc));
        assert_eq!(classify(&PERF_INFO, 68), Some(TraceClass::Dpc));
        assert_eq!(classify(&PERF_INFO, 69), Some(TraceClass::Dpc));
        assert_eq!(classify(&PERF_INFO, 67), Some(TraceClass::Isr));
        assert_eq!(classify(&DISK_IO, 10), Some(TraceClass::DiskRead));
        assert_eq!(classify(&DISK_IO, 11), Some(TraceClass::DiskWrite));
        assert_eq!(classify(&DISK_IO, 12), None);
        for class in TraceClass::ALL {
            for (name, length) in required_properties(class) {
                assert!(["InitialTime", "TransferSize", "HighResResponseTime"].contains(name));
                assert!([4, 8].contains(length));
            }
        }
    }
    #[test]
    fn reviewed_metadata_refuses_type_identity_count_and_offset_mismatches() {
        let mut record = EVENT_RECORD::default();
        record.EventHeader.ProviderId = PERF_INFO;
        record.EventHeader.EventDescriptor.Version = 2;
        record.EventHeader.EventDescriptor.Opcode = 66;
        let property_offset = offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
        let name_offset = property_offset + size_of::<EVENT_PROPERTY_INFO>();
        let name: Vec<u16> = "InitialTime".encode_utf16().chain(Some(0)).collect();
        let byte_length = name_offset + name.len() * 2;
        let mut words = vec![0_u64; byte_length.div_ceil(size_of::<u64>())];
        // SAFETY: aligned fixture allocation contains the fixed header, one property and bounded UTF-16 name.
        unsafe {
            let info = words.as_mut_ptr().cast::<TRACE_EVENT_INFO>();
            (*info).ProviderGuid = PERF_INFO;
            (*info).EventDescriptor = record.EventHeader.EventDescriptor;
            (*info).DecodingSource = DecodingSourceWbem;
            (*info).PropertyCount = 1;
            (*info).TopLevelPropertyCount = 1;
            let property = words
                .as_mut_ptr()
                .cast::<u8>()
                .add(property_offset)
                .cast::<EVENT_PROPERTY_INFO>();
            (*property).NameOffset = name_offset as u32;
            (*property).Anonymous1.nonStructType.InType = TDH_INTYPE_UINT64 as u16;
            (*property).Anonymous2.count = 1;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                words
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(name_offset)
                    .cast::<u16>(),
                name.len(),
            );
        }
        assert!(metadata_valid(
            &record,
            TraceClass::Dpc,
            &words,
            byte_length
        ));
        assert!(!metadata_valid(
            &record,
            TraceClass::DiskRead,
            &words,
            byte_length
        ));
        let mut mismatched = record;
        mismatched.EventHeader.EventDescriptor.Version = 3;
        assert!(!metadata_valid(
            &mismatched,
            TraceClass::Dpc,
            &words,
            byte_length
        ));
        mismatched.EventHeader.EventDescriptor = record.EventHeader.EventDescriptor;
        mismatched.EventHeader.ProviderId = DISK_IO;
        assert!(!metadata_valid(
            &mismatched,
            TraceClass::Dpc,
            &words,
            byte_length
        ));
        let mut invalid = words.clone();
        // SAFETY: fixture property offset is inside this independently allocated aligned copy.
        unsafe {
            let property = invalid
                .as_mut_ptr()
                .cast::<u8>()
                .add(property_offset)
                .cast::<EVENT_PROPERTY_INFO>();
            (*property).Anonymous1.nonStructType.InType = TDH_INTYPE_POINTER as u16;
        }
        assert!(!metadata_valid(
            &record,
            TraceClass::Dpc,
            &invalid,
            byte_length
        ));
        // SAFETY: same checked fixture property; replacing count does not dereference payload pointers.
        unsafe {
            let property = invalid
                .as_mut_ptr()
                .cast::<u8>()
                .add(property_offset)
                .cast::<EVENT_PROPERTY_INFO>();
            (*property).Anonymous1.nonStructType.InType = TDH_INTYPE_UINT64 as u16;
            (*property).Anonymous2.count = 2;
        }
        assert!(!metadata_valid(
            &record,
            TraceClass::Dpc,
            &invalid,
            byte_length
        ));
        // SAFETY: same checked fixture property; invalid name offset is then tested without dereference.
        unsafe {
            let property = invalid
                .as_mut_ptr()
                .cast::<u8>()
                .add(property_offset)
                .cast::<EVENT_PROPERTY_INFO>();
            (*property).Anonymous2.count = 1;
            (*property).NameOffset = byte_length as u32;
        }
        assert!(!metadata_valid(
            &record,
            TraceClass::Dpc,
            &invalid,
            byte_length
        ));
        assert!(!metadata_valid(
            &record,
            TraceClass::Dpc,
            &words,
            property_offset
        ));
    }
}
