//! Portable parent-relation validation for observed process trees.
use pcagent_shared::{ParentRelation, ProcessEdge, ProcessTreeNode};
use std::collections::HashMap;

/// Windows keeps the creator's PID after the creator exits, so a parent created
/// after its child is a reused PID, not the real parent.
pub fn relation(child_created: Option<u64>, parent: Option<Option<u64>>) -> ParentRelation {
    match parent {
        None => ParentRelation::ParentNotObserved,
        Some(parent_created) => match (child_created, parent_created) {
            (Some(child), Some(parent)) if parent <= child => ParentRelation::Observed,
            (Some(_), Some(_)) => ParentRelation::PidReused,
            _ => ParentRelation::Unknown,
        },
    }
}

/// Builds one edge per node with a non-root parent PID.
pub fn link(nodes: &[ProcessTreeNode]) -> Vec<ProcessEdge> {
    let by_pid: HashMap<u32, &ProcessTreeNode> = nodes.iter().map(|n| (n.pid, n)).collect();
    nodes
        .iter()
        .filter(|n| n.parent_pid != 0 && n.parent_pid != n.pid)
        .map(|child| {
            let parent = by_pid.get(&child.parent_pid);
            let relation = relation(
                child.creation_time_filetime,
                parent.map(|p| p.creation_time_filetime),
            );
            ProcessEdge {
                parent_pid: child.parent_pid,
                child_pid: child.pid,
                parent_ref: match relation {
                    ParentRelation::Observed => parent.and_then(|p| p.entity_ref.clone()),
                    _ => None,
                },
                child_ref: child.entity_ref.clone(),
                relation,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::Status;
    fn node(pid: u32, parent_pid: u32, created: Option<u64>) -> ProcessTreeNode {
        ProcessTreeNode {
            pid,
            parent_pid,
            entity_ref: created.map(|c| format!("proc-{pid}-{c}")),
            creation_time_filetime: created,
            executable_name: None,
            status: if created.is_some() {
                Status::Ok
            } else {
                Status::PermissionDenied
            },
        }
    }
    #[test]
    fn relations_are_validated_by_creation_time() {
        let nodes = [
            node(4, 0, Some(1)),
            node(100, 4, Some(10)),
            node(200, 100, Some(20)),
            node(300, 999, Some(30)),
            node(400, 500, Some(40)),
            node(500, 4, Some(50)),
            node(600, 700, Some(60)),
            node(700, 4, None),
        ];
        let edges = link(&nodes);
        let rel = |pid| edges.iter().find(|e| e.child_pid == pid).unwrap();
        assert!(edges.iter().all(|e| e.child_pid != 4));
        assert_eq!(rel(100).relation, ParentRelation::Observed);
        assert_eq!(rel(200).parent_ref.as_deref(), Some("proc-100-10"));
        assert_eq!(rel(300).relation, ParentRelation::ParentNotObserved);
        assert_eq!(rel(400).relation, ParentRelation::PidReused);
        assert_eq!(rel(400).parent_ref, None);
        assert_eq!(rel(600).relation, ParentRelation::Unknown);
        assert_eq!(rel(600).parent_ref, None);
    }
    #[test]
    fn self_parent_is_treated_as_root() {
        assert!(link(&[node(8, 8, Some(1))]).is_empty());
    }
}
