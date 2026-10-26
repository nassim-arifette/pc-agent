use super::*;

#[test]
fn the_pid_is_shown_only_when_two_cited_processes_would_read_the_same() {
    let processes = [
        process("proc-0000000a", 40, "browser.exe"),
        process("proc-0000000b", 41, "browser.exe"),
        process("proc-0000000c", 50, "other.exe"),
        process("proc-0000000d", 60, "tool.exe"),
        process("proc-0000000e", 61, "tool.exe"),
        process("proc-00000001", 4242, "editor.exe"),
    ];
    let l = |text: &str| localize(text, &processes, &names);
    // One browser process cited: its name alone, even though another one runs.
    assert_eq!(
        l("proc-0000000a uses 2 GiB [O1]."),
        "Example Browser uses 2 GiB [O1]."
    );
    // Two different processes that would read the same both get their PID; others do not.
    assert_eq!(
        l("browser.exe (proc-0000000a) and browser.exe (proc-0000000b) use 3 GiB; proc-00000001 is idle."),
        "Example Browser (PID 40) and Example Browser (PID 41) use 3 GiB; Example Editor is idle."
    );
    // Names that differ only in case read the same.
    assert_eq!(
        l("proc-0000000a, proc-0000000c"),
        "Example Browser (PID 40), example browser (PID 50)"
    );
    // Without readable names, executable names decide.
    assert_eq!(
        l("proc-0000000d then proc-0000000e"),
        "tool.exe (PID 60) then tool.exe (PID 61)"
    );
    assert_eq!(l("proc-0000000d alone"), "tool.exe alone");
    // The same process cited twice is still one process.
    assert_eq!(
        l("proc-0000000a, again proc-0000000a"),
        "Example Browser, again Example Browser"
    );
}

#[test]
fn references_are_replaced_whole_and_in_one_pass() {
    let readable = |pid: u32, _: u64| match pid {
        1 => Some("proc-00000002".to_owned()),
        2 => Some("Real Program".to_owned()),
        3 => Some("My editor.exe".to_owned()),
        _ => None,
    };
    let processes = [
        process("proc-00000001", 1, "fake.exe"),
        process("proc-00000002", 2, "real.exe"),
        process("proc-00000003", 3, "x.exe"),
        process("proc-00000004", 4, "editor.exe"),
    ];
    let l = |text: &str| localize(text, &processes, &readable);
    // A readable name that looks like a reference is shown as written, never replaced again.
    assert_eq!(
        l("proc-00000001 and proc-00000002"),
        "proc-00000002 and Real Program"
    );
    // An inserted name is never taken for the model's own "name (reference)".
    assert_eq!(
        l("proc-00000003 (proc-00000004)"),
        "My editor.exe (editor.exe)"
    );
    // Whole references only: not inside a longer token, and not a draft's partial reference.
    assert_eq!(
        l("proc-000000011, xproc-00000001, proc-0000000"),
        "proc-000000011, xproc-00000001, proc-0000000"
    );
    assert_eq!(
        l("Évité : **proc-00000002**, déjà fermé."),
        "Évité : **Real Program**, déjà fermé."
    );
    assert_eq!(l("No process here."), "No process here.");
}
