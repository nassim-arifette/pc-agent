//! Session-scoped process identity registry keyed by PID plus creation time.
#![cfg_attr(not(windows), allow(dead_code))]
use pcagent_shared::{new_id, Evidence, InstanceState, ProcessInstance};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};

pub(crate) type Key = (u32, u64);
const RETIRED_CAP: usize = 256;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CounterSample {
    pub cpu_100ns: Option<u64>,
    pub io_bytes: Option<[u64; 3]>,
    pub at: Duration,
    pub unix_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Tracked {
    pub instance: ProcessInstance,
    pub evidence: Vec<Evidence>,
    pub observed_unix_ms: Option<u64>,
    pub counters: Option<CounterSample>,
}

#[derive(Debug, Clone)]
pub(crate) struct Retired {
    pub tracked: Tracked,
    pub state: InstanceState,
}

pub(crate) enum Lookup<'a> {
    Live(Key, &'a Tracked),
    Retired(&'a Retired),
    NotFound,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    live: HashMap<Key, Tracked>,
    refs: HashMap<String, Key>,
    retired: VecDeque<Retired>,
}
impl Registry {
    pub fn entry(&mut self, pid: u32, created: u64, boot_id: Option<&str>) -> &mut Tracked {
        let refs = &mut self.refs;
        self.live.entry((pid, created)).or_insert_with(|| {
            let entity_ref = new_id("proc");
            refs.insert(entity_ref.clone(), (pid, created));
            Tracked {
                instance: ProcessInstance {
                    entity_ref,
                    pid,
                    creation_time_filetime: created,
                    boot_id: boot_id.map(str::to_owned),
                    executable_name: None,
                },
                evidence: Vec::new(),
                observed_unix_ms: None,
                counters: None,
            }
        })
    }
    /// Retires instances contradicted by a complete enumeration: PID absent, or PID present
    /// with a different readable creation time. Unreadable PIDs keep their identity.
    pub fn sweep(&mut self, seen: &HashSet<Key>, enumerated: &HashSet<u32>) {
        let readable: HashSet<u32> = seen.iter().map(|k| k.0).collect();
        let gone: Vec<(Key, InstanceState)> = self
            .live
            .keys()
            .filter(|k| !seen.contains(k))
            .filter_map(|&k| {
                if !enumerated.contains(&k.0) {
                    Some((k, InstanceState::Exited))
                } else if readable.contains(&k.0) {
                    Some((k, InstanceState::PidReused))
                } else {
                    None
                }
            })
            .collect();
        for (key, state) in gone {
            self.retire(key, state);
        }
    }
    pub fn retire(&mut self, key: Key, state: InstanceState) {
        if let Some(tracked) = self.live.remove(&key) {
            self.refs.remove(&tracked.instance.entity_ref);
            if self.retired.len() == RETIRED_CAP {
                self.retired.pop_front();
            }
            self.retired.push_back(Retired { tracked, state });
        }
    }
    pub fn lookup(&self, entity_ref: &str) -> Lookup<'_> {
        if let Some((key, tracked)) = self
            .refs
            .get(entity_ref)
            .and_then(|k| self.live.get(k).map(|t| (*k, t)))
        {
            return Lookup::Live(key, tracked);
        }
        self.retired
            .iter()
            .rev()
            .find(|r| r.tracked.instance.entity_ref == entity_ref)
            .map_or(Lookup::NotFound, Lookup::Retired)
    }
    pub fn get(&self, key: Key) -> Option<&Tracked> {
        self.live.get(&key)
    }
    #[cfg(test)]
    pub fn live_len(&self) -> usize {
        self.live.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sweep(r: &mut Registry, keys: &[Key]) {
        let seen = keys.iter().copied().collect();
        let pids = keys.iter().map(|k| k.0).collect();
        r.sweep(&seen, &pids);
    }
    #[test]
    fn same_identity_keeps_its_ref() {
        let mut r = Registry::default();
        let a = r.entry(10, 100, Some("b")).instance.entity_ref.clone();
        let b = r.entry(10, 100, Some("b")).instance.entity_ref.clone();
        assert_eq!(a, b);
        assert_eq!(
            r.entry(10, 100, None).instance.boot_id.as_deref(),
            Some("b")
        );
    }
    #[test]
    fn reused_pid_gets_a_new_ref_and_old_ref_is_retired() {
        let mut r = Registry::default();
        let old = r.entry(10, 100, None).instance.entity_ref.clone();
        let new = r.entry(10, 200, None).instance.entity_ref.clone();
        assert_ne!(old, new);
        sweep(&mut r, &[(10, 200)]);
        match r.lookup(&old) {
            Lookup::Retired(x) => assert_eq!(x.state, InstanceState::PidReused),
            _ => panic!("old instance must be retired"),
        }
        assert!(matches!(r.lookup(&new), Lookup::Live((10, 200), _)));
    }
    #[test]
    fn exited_and_unknown_refs_are_distinct() {
        let mut r = Registry::default();
        let gone = r.entry(7, 1, None).instance.entity_ref.clone();
        sweep(&mut r, &[]);
        assert!(matches!(r.lookup(&gone), Lookup::Retired(x) if x.state == InstanceState::Exited));
        assert!(matches!(r.lookup("7"), Lookup::NotFound));
        assert!(matches!(r.lookup("proc-unknown"), Lookup::NotFound));
    }
    #[test]
    fn unreadable_pid_keeps_its_identity() {
        let mut r = Registry::default();
        let a = r.entry(9, 1, None).instance.entity_ref.clone();
        r.sweep(&HashSet::new(), &[9].into_iter().collect());
        assert!(matches!(r.lookup(&a), Lookup::Live(..)));
    }
    #[test]
    fn retired_history_is_bounded() {
        let mut r = Registry::default();
        let first = r.entry(1, 1, None).instance.entity_ref.clone();
        sweep(&mut r, &[]);
        for i in 0..RETIRED_CAP as u32 {
            r.entry(i + 2, 1, None);
            sweep(&mut r, &[]);
        }
        assert_eq!(r.retired.len(), RETIRED_CAP);
        assert_eq!(r.live_len(), 0);
        assert!(matches!(r.lookup(&first), Lookup::NotFound));
    }
}
