use super::{last_error, last_status, sysproc, times, OwnedHandle, BOOT_NOTE};
use crate::Collector;
use pcagent_shared::{
    new_id, unix_ms, ProcessTree, ProcessTreeNode, Status, Window, SCHEMA_VERSION,
};
use std::{collections::HashSet, mem::size_of, time::Instant};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    },
};

const MAX_NODES: usize = 16384;
type Entry = (u32, u32, String);

fn entries() -> Result<(Vec<Entry>, bool), Status> {
    // SAFETY: flags request a process list only; a valid result is owned by one wrapper.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return Err(last_status());
    }
    let snap = OwnedHandle(snap);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut rows = Vec::new();
    // SAFETY: the snapshot handle is valid and entry.dwSize matches the structure.
    let mut more = unsafe { Process32FirstW(snap.0, &mut entry) } != 0;
    if !more && last_error() != ERROR_NO_MORE_FILES {
        return Err(last_status());
    }
    while more && rows.len() < MAX_NODES {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        rows.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..len]),
        ));
        // SAFETY: same handle and structure as above.
        more = unsafe { Process32NextW(snap.0, &mut entry) } != 0;
    }
    Ok((rows, more))
}

pub(crate) fn process_tree(collector: &mut Collector) -> ProcessTree {
    let started = unix_ms();
    let clock = Instant::now();
    let mut tree = ProcessTree {
        schema_version: SCHEMA_VERSION,
        id: new_id("tree"),
        session_id: collector.session_id.clone(),
        origin: pcagent_shared::Origin::Live,
        status: Status::Ok,
        window: Window::instant(),
        nodes: Vec::new(),
        edges: Vec::new(),
        truncated: false,
        limitations: vec![
            "Parent PIDs are recorded at child creation and are not updated when the parent exits; relations are validated by creation time.".into(),
            "The system process list supplies creation times without opening processes. Toolhelp32 plus query-only handles is used only if that list is unavailable.".into(),
            "Observation only: a relation never authorizes acting on a process or its descendants.".into(),
            BOOT_NOTE.into(),
        ],
    };
    let (rows, truncated, creation_times) = if let Ok(list) = sysproc::query() {
        let times = list
            .iter()
            .map(|p| (p.pid, p.created))
            .collect::<std::collections::HashMap<_, _>>();
        let rows = list
            .into_iter()
            .filter(|p| p.pid != 0)
            .map(|p| (p.pid, p.parent_pid, p.name.unwrap_or_default()))
            .collect::<Vec<_>>();
        tree.limitations.push("System Idle Process is a pseudo-process and is excluded from parent/application relations.".into());
        (rows, false, Some(times))
    } else {
        match entries() {
            Ok((rows, truncated)) => (rows, truncated, None),
            Err(status) => {
                tree.status = status;
                tree.limitations
                    .push("Both native process-list collectors failed.".into());
                tree.window = Window::finish(started, clock);
                return tree;
            }
        }
    };
    let boot = collector.boot_id.clone();
    let mut seen = HashSet::new();
    for (pid, parent_pid, name) in rows.iter().cloned() {
        let created = match &creation_times {
            Some(times) => times
                .get(&pid)
                .copied()
                .filter(|&created| created != 0)
                .map(|created| (created, None))
                .ok_or(Status::Partial),
            None => OwnedHandle::open(pid).and_then(|h| times(&h)),
        };
        let (entity_ref, creation, status) = match created {
            Ok((created, _)) => {
                seen.insert((pid, created));
                let t = collector.registry.entry(pid, created, boot.as_deref());
                if t.instance.executable_name.is_none() {
                    t.instance.executable_name = (!name.is_empty()).then(|| name.clone());
                }
                (
                    Some(t.instance.entity_ref.clone()),
                    Some(created),
                    Status::Ok,
                )
            }
            Err(status) => (None, None, status),
        };
        tree.nodes.push(ProcessTreeNode {
            pid,
            parent_pid,
            entity_ref,
            creation_time_filetime: creation,
            executable_name: (!name.is_empty()).then_some(name),
            status,
        });
    }
    if !truncated {
        collector
            .registry
            .sweep(&seen, &rows.iter().map(|r| r.0).collect());
    }
    tree.edges = crate::tree::link(&tree.nodes);
    tree.truncated = truncated;
    if truncated || tree.nodes.iter().any(|n| n.status != Status::Ok) {
        tree.status = Status::Partial;
    }
    tree.window = Window::finish(started, clock);
    tree
}
