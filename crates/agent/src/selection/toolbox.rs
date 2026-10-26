//! Re-read frozen evidence; only Investigate can delegate to the host's ordinary tools.
use super::{projection::unavailable, FrozenSelection, SelectionIntent};
use crate::{Args, Sharing, ToolClass, ToolOutput, ToolSpec, Toolbox};
use pcagent_shared::Status;
use serde_json::json;

pub const GET_SELECTED_EVIDENCE: ToolSpec = ToolSpec {
    name: "get_selected_evidence",
    class: ToolClass::Inspection,
    description: "Read again the exact selected measurements frozen for this turn (initial_observation, O0), with their original dates, windows, coverage and limitations. Starts no collector, scan, recording or action. Historical evidence describes its recorded period, not the current PC: cite its numbers in the past, tied to the measurement.",
    params: &[],
};
pub use GET_SELECTED_EVIDENCE as SELECTED_EVIDENCE;

/// Explain cannot reach any delegate method. Investigate uses the existing read-only tool boundary.
pub struct SelectedTools<'a, T: Toolbox> {
    delegate: &'a mut T,
    selection: &'a FrozenSelection,
    specs: Vec<ToolSpec>,
}

impl<'a, T: Toolbox> SelectedTools<'a, T> {
    pub fn new(delegate: &'a mut T, selection: &'a FrozenSelection) -> Self {
        let mut specs = vec![GET_SELECTED_EVIDENCE];
        if selection.intent() == SelectionIntent::Investigate {
            specs.extend(
                delegate
                    .specs()
                    .iter()
                    .filter(|spec| spec.name != GET_SELECTED_EVIDENCE.name),
            );
        }
        Self {
            delegate,
            selection,
            specs,
        }
    }
}

impl<T: Toolbox> Toolbox for SelectedTools<'_, T> {
    fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }
    fn sharing(&self) -> Sharing {
        self.selection.sharing()
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        if spec.name == GET_SELECTED_EVIDENCE.name {
            return self.selection.read();
        }
        if self.selection.intent() == SelectionIntent::Investigate
            && self
                .delegate
                .specs()
                .iter()
                .any(|allowed| allowed.name == spec.name)
        {
            return self.delegate.run(spec, args);
        }
        let mut output = unavailable("This tool is unavailable for the frozen Explain selection.");
        output.status = Status::PermissionDenied;
        output.cloud["status"] = json!(Status::PermissionDenied);
        output
    }
}
