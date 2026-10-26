//! CLI argument, consent and helper-lifecycle regression tests.
use crate::args as options;
use crate::args::{Args, Command};
use crate::elevation::{with_elevation_cleanup, with_elevation_interrupt, CliElevationCompletion};
use crate::investigation::review_extension;
use pcagent_agent::{local_report, ExtensionRequest};
use pcagent_shared::Snapshot;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::{CommandFactory, Parser};
#[test]
fn extension_prompt_requires_interactive_explicit_yes_and_never_reads_json_or_piped_input() {
    let report = local_report(&Snapshot::empty("fixture"));
    let request = ExtensionRequest {
        reason: pcagent_agent::BudgetLimit::Rounds,
        available: true,
        additional_rounds: 6,
        additional_tool_calls: 12,
        additional_seconds: 90,
    };
    for (json_mode, interactive) in [(true, true), (false, false)] {
        let mut input = io::Cursor::new(b"yes\n");
        let mut output = Vec::new();
        assert!(!review_extension(
            &report,
            &request,
            json_mode,
            interactive,
            &mut input,
            &mut output
        )
        .unwrap());
        assert_eq!(input.position(), 0);
        assert!(output.is_empty());
    }
    for (answer, expected) in [
        ("yes\n", true),
        ("YES\n", true),
        ("no\n", false),
        ("\n", false),
        ("", false),
    ] {
        let mut output = Vec::new();
        assert_eq!(
            review_extension(
                &report,
                &request,
                false,
                true,
                &mut io::Cursor::new(answer.as_bytes()),
                &mut output
            )
            .unwrap(),
            expected
        );
        let shown = String::from_utf8(output).unwrap();
        assert!(shown.contains("Execution:") && shown.contains("provider tokens"));
    }
}
#[cfg(windows)]
#[test]
fn json_report_retains_pending_manifest_without_approving_or_executing() {
    use crate::actions::review_actions;
    use crate::investigation::diagnosis_json;
    use pcagent_actions::{
        ActionIntent, ActionKind, Broker, Journal, NativeEffects, NativeGuard, DEFAULT_PLAN_TTL,
    };
    use pcagent_policy::{Policy, Preferences};
    use pcagent_telemetry::Collector;
    use pcagent_tools::{ActionDesk, PendingPlan, PolicyProtection};
    use std::fs;
    let dir = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-cli-plan-test"));
    let journal = Journal::open(&dir).unwrap();
    let policy = Policy::default();
    let prefs = Preferences::default();
    let protection = PolicyProtection {
        policy: &policy,
        prefs: &prefs,
    };
    let targets = pcagent_tools::FixedTargets::default();
    let effects = NativeEffects::default();
    let plan = Broker {
        resolver: &targets,
        guard: &NativeGuard,
        protection: &protection,
        effects: &effects,
        journal: &journal,
        synthetic_cache: None,
        plan_ttl: DEFAULT_PLAN_TTL,
    }
    .prepare_plan(&ActionIntent {
        action_kind: ActionKind::OpenSettingsPage,
        target_refs: vec!["storagesense".into()],
        evidence_refs: vec!["ev-fixture".into()],
    })
    .unwrap();
    let desk = ActionDesk::native(journal);
    let actions = review_actions(
        &desk,
        vec![PendingPlan {
            plan,
            targets,
            synthetic_cache: None,
        }],
        &protection,
        &mut Collector::new(),
        true,
    )
    .unwrap();
    let report = local_report(&Snapshot::empty("fixture"));
    let text = serde_json::to_string(&diagnosis_json(&report, &actions, None)).unwrap();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(document["actions"][0]["decision"], "not_reviewed");
    assert_eq!(document["actions"][0]["plan"]["state"], "awaiting_approval");
    assert_eq!(
        document["actions"][0]["plan"]["items"][0]["target_ref"],
        "storagesense"
    );
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "no action journal without approval"
    );
    fs::remove_dir(&dir).unwrap();
}
#[test]
fn cli_schema_is_consistent() {
    Args::command().debug_assert();
}
#[test]
fn an_interrupt_already_pending_reaches_elevation_before_launch() {
    let signal = AtomicBool::new(true);
    let cancelled = with_elevation_interrupt(&signal, |cancel| cancel.is_cancelled()).unwrap();
    assert!(cancelled);
}

#[test]
fn a_scoped_console_signal_reaches_an_in_flight_elevation() {
    use std::sync::mpsc;
    let signal = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let (started_tx, started_rx) = mpsc::channel();
        let worker_signal = signal.clone();
        let worker = scope.spawn(move || {
            with_elevation_interrupt(&worker_signal, |cancel| {
                assert!(!cancel.is_cancelled());
                started_tx.send(()).unwrap();
                let started = std::time::Instant::now();
                while !cancel.is_cancelled() && started.elapsed() < Duration::from_secs(3) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                cancel.is_cancelled()
            })
            .unwrap()
        });
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        signal.store(true, Ordering::SeqCst);
        assert!(worker.join().unwrap());
    });
}

struct FixtureCliHandler(Arc<AtomicBool>);
impl Drop for FixtureCliHandler {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
struct FixtureCliPrepared(Arc<AtomicBool>);
impl Drop for FixtureCliPrepared {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct FixtureCliCompletion {
    entered: std::sync::mpsc::Sender<()>,
    entered_once: AtomicBool,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    cleaned: Arc<AtomicBool>,
}
impl CliElevationCompletion for FixtureCliCompletion {
    fn wait(&self, timeout: Duration) -> bool {
        if !self.entered_once.swap(true, Ordering::SeqCst) {
            let _ = self.entered.send(());
        }
        match self.release.lock().unwrap().recv_timeout(timeout) {
            Ok(()) => {
                self.cleaned.store(true, Ordering::SeqCst);
                true
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
            // Release a failed fixture's worker while unwinding; it never counts as cleanup.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => true,
        }
    }
}

#[test]
fn preview_approval_error_and_cancelled_run_keep_the_handler_until_confirmed_cleanup() {
    use std::sync::mpsc;
    for outcome in [Ok(0_u8), Err("approval expired"), Err("user cancelled")] {
        let signal = Arc::new(AtomicBool::new(false));
        let handler_active = Arc::new(AtomicBool::new(true));
        let prepared_dropped = Arc::new(AtomicBool::new(false));
        let cleaned = Arc::new(AtomicBool::new(false));
        std::thread::scope(|scope| {
            // Owned here so a failed assertion releases the synthetic cleanup wait.
            let (release_tx, release_rx) = mpsc::channel();
            let (waiting_tx, waiting_rx) = mpsc::channel();
            let (token_tx, token_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let worker_signal = signal.clone();
            let worker_handler = handler_active.clone();
            let dropped = prepared_dropped.clone();
            let confirmed = cleaned.clone();
            let worker = scope.spawn(move || {
                let result = {
                    // This fixture installs no real console handler and launches no helper.
                    let _handler = FixtureCliHandler(worker_handler);
                    with_elevation_interrupt(&worker_signal, |cancel| {
                        let prepared = FixtureCliPrepared(dropped);
                        let completion = FixtureCliCompletion {
                            entered: waiting_tx,
                            entered_once: AtomicBool::new(false),
                            release: std::sync::Mutex::new(release_rx),
                            cleaned: confirmed,
                        };
                        with_elevation_cleanup(completion, move || {
                            let _prepared = prepared;
                            token_tx.send(cancel.clone()).unwrap();
                            outcome
                        })
                    })
                };
                let _ = result_tx.send(result);
            });
            waiting_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            let cancel = token_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(prepared_dropped.load(Ordering::SeqCst));
            assert!(handler_active.load(Ordering::SeqCst));
            assert!(!cleaned.load(Ordering::SeqCst));
            assert!(matches!(
                result_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            signal.store(true, Ordering::SeqCst);
            let started = std::time::Instant::now();
            while !cancel.is_cancelled() && started.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                cancel.is_cancelled(),
                "relay stopped before cleanup completed"
            );
            release_tx.send(()).unwrap();
            assert_eq!(
                result_rx
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .unwrap(),
                outcome
            );
            worker.join().unwrap();
            assert!(cleaned.load(Ordering::SeqCst));
            assert!(!handler_active.load(Ordering::SeqCst));
        });
    }
}

#[test]
fn cloud_requires_an_explicit_flag() {
    let args = Args::try_parse_from(["pcagent", "diagnose", "Why is my PC slow?"]).unwrap();
    assert!(matches!(
        args.command,
        Command::Diagnose(options::Diagnose {
            allow_cloud: false,
            ..
        })
    ));
}
#[test]
fn conflicting_permissions_and_unbounded_limits_are_rejected() {
    assert!(
        Args::try_parse_from(["pcagent", "diagnose", "test", "--offline", "--allow-cloud"])
            .is_err()
    );
    assert!(Args::try_parse_from([
        "pcagent",
        "diagnose",
        "test",
        "--preview-cloud",
        "--allow-cloud"
    ])
    .is_err());
    assert!(Args::try_parse_from(["pcagent", "processes", "--limit", "0"]).is_err());
    assert!(Args::try_parse_from(["pcagent", "snapshot", "--limit", "51"]).is_err());
}
