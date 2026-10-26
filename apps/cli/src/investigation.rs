//! Local and cloud investigations, conversation turns and explicit extensions.
use crate::actions::review_actions;
use crate::history::cli_sampler;
use crate::local::{local_data_dir, write_json};
use crate::provider::cloud_setup_error;
use crate::storage::authorize_folder;
use crate::{args as options, model_routing, storage_view, text, Result};
use pcagent_actions::{recover, Journal};
use pcagent_agent::capabilities::{Capabilities, Flows, Probes};
use pcagent_agent::credentials::Credentials;
use pcagent_agent::{
    converse_with_privacy, initial_message_with_privacy, local_report, ExecutionStatus,
    ExtensionRequest, Investigation, PastTurn, PrivacyContext, Sharing, MAX_DURATION,
    MAX_QUESTION_BYTES, MAX_ROUNDS, MAX_TOOLS, PREPARE_ACTION_PLAN,
};
use pcagent_nebius::Config;
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::Snapshot;
use pcagent_storage::StorageService;
use pcagent_telemetry::Collector;
use pcagent_tools::{ActionDesk, PolicyProtection, Workstation};
use serde_json::json;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::{fs, io};

/// The tools a cloud diagnosis declares on this PC (`capabilities`): the history sampler starts
/// with the run; the CLI has no helper flow and no web lookups.
pub(super) fn declared_tools(
    initial: &Snapshot,
    scopes: bool,
    proposals: bool,
) -> Vec<&'static str> {
    let mut specs = Workstation::tool_specs();
    if proposals {
        specs.push(PREPARE_ACTION_PLAN);
    }
    let flows = Flows {
        history_sampler: true,
        storage_scopes: scopes,
        web_search: false,
        action_proposals: proposals,
    };
    let capabilities = Capabilities::of_turn(cfg!(windows), flows, Probes::without_helper(initial));
    pcagent_agent::capabilities::catalog(&specs, &capabilities)
        .iter()
        .map(|spec| spec.name)
        .collect()
}

/// No reading stdin or emitting an extra report in machine-readable/batch mode.
pub(super) fn review_extension(
    report: &Investigation,
    request: &ExtensionRequest,
    json_mode: bool,
    interactive: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<bool> {
    if json_mode || !interactive || !request.available {
        return Ok(false);
    }
    text::report(output, report, false)?;
    write!(output, "Continue with up to {} additional model requests, {} tools and {} seconds? This can consume more provider tokens. Type 'yes' to continue [no]: ",
        request.additional_rounds, request.additional_tool_calls, request.additional_seconds)?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}
pub(super) fn diagnosis_json(
    report: &pcagent_agent::Investigation,
    actions: &[serde_json::Value],
    recovery: Option<&pcagent_actions::RecoveryReport>,
) -> serde_json::Value {
    let mut value = serde_json::to_value(report).expect("serializable report");
    value["actions"] = json!(actions);
    value["recovery"] = json!(recovery);
    value
}

pub(super) fn diagnose(
    options: options::Diagnose,
    json: bool,
    routing: &model_routing::Routing,
) -> Result<u8> {
    let options::Diagnose {
        question,
        offline: _,
        allow_cloud,
        preview_cloud,
        verbose,
        scan_root,
        compare_content,
        share_file_names,
        anonymize,
        preferences,
        propose_actions,
        journal_dir,
    } = options;
    let sharing = if anonymize {
        Sharing::Anonymized
    } else {
        Sharing::Complete
    };
    if share_file_names && !allow_cloud && !preview_cloud {
        return Err("--share-file-names requires --preview-cloud or --allow-cloud.".into());
    }
    let prefs = match &preferences {
        Some(path) => fs::read_to_string(path)
            .map_err(|e| format!("Cannot read the preferences file {}: {e}", text::path(path)))
            .and_then(|json| {
                Preferences::from_json(&json)
                    .map_err(|e| format!("Invalid preferences file {}: {e}", text::path(path)))
            })?,
        None => Preferences::default(),
    };
    if propose_actions && !allow_cloud {
        return Err(
            "--propose-actions requires --allow-cloud: only the model proposes plans.".into(),
        );
    }
    if journal_dir.is_some() && !propose_actions {
        return Err("--journal-dir requires --propose-actions.".into());
    }
    if allow_cloud && preferences.is_some() && !prefs.permissions.cloud_allowed {
        return Err("The preferences file does not allow cloud requests (permissions.cloud_allowed is false).".into());
    }
    let storage = Arc::new(StorageService::new());
    for root in &scan_root {
        authorize_folder(&storage, root)?;
    }
    if question.trim().is_empty() {
        return Err("The question is empty; ask something like \"Why is my PC slow?\"".into());
    }
    if question.len() > MAX_QUESTION_BYTES {
        return Err(
            format!("The question is longer than {MAX_QUESTION_BYTES} bytes; shorten it.").into(),
        );
    }
    if allow_cloud && !cfg!(windows) {
        return Err("Cloud diagnostics require the native Windows collector; no synthetic or other-platform telemetry is substituted.".into());
    }
    let creds = Credentials::load();
    let mut privacy = PrivacyContext::local();
    if let Some(secret) = creds.get("NEBIUS_API_KEY") {
        privacy.protect_secret(&secret);
    }
    let mut model = if allow_cloud {
        let config = Config::from_lookup(|name| creds.get(name))
            .map_err(|e| cloud_setup_error(e, &creds))?;
        Some(routing.model(&config)?)
    } else {
        None
    };
    let mut collector = Collector::new();
    let initial = collector.snapshot(true, 10);
    let policy = Policy::from_env();
    let prepared = storage_view::prepare_with_privacy(
        &storage,
        &policy,
        &prefs,
        share_file_names,
        compare_content,
        &privacy,
    )?;
    if preview_cloud {
        write_json(
            &json!({"sent":false,"notice":"The sanitized question is sent only with --allow-cloud. Later tool samples may differ; the same allowlist applies.",
            "message":initial_message_with_privacy(&question, &initial, sharing, &privacy),
            "model_routing":routing.preview(),
            "tools":declared_tools(&initial, !storage.scopes().is_empty(), propose_actions),
            "authorized_scopes":storage.scopes().len(),
            "file_name_sharing":prepared.context.sharing_preview(),
            "storage_tool_previews":prepared.preview}),
        )?;
        return Ok(0);
    }
    if share_file_names {
        // The flag is the explicit permission. Show this run's exact finite allowlist
        // before the first request, including in batch mode (stderr preserves JSON).
        eprintln!(
            "File names authorized for this run (before any cloud request):\n{}",
            serde_json::to_string_pretty(&prepared.context.sharing_preview())?
        );
    }
    let mut workstation = Workstation::new(&mut collector, storage.clone(), &policy, &prefs);
    workstation.set_storage_context(prepared.context.clone());
    workstation.set_sharing(sharing);
    if allow_cloud {
        workstation.set_history(cli_sampler(None)?, Arc::new(AtomicBool::new(false)));
    }
    let mut recovery = None;
    if propose_actions {
        let dir = journal_dir.unwrap_or_else(|| local_data_dir().join("journal"));
        let journal = Journal::open(dir)?;
        let recovered = recover(&journal)?;
        if !recovered.needs_inspection.is_empty()
            || recovered.journals_unreadable > 0
            || recovered.malformed_lines > 0
            || recovered.truncated
        {
            eprintln!("Action journal: {} interrupted items need manual inspection; {} unreadable journals, {} malformed lines. No action was replayed.",
                recovered.needs_inspection.len(), recovered.journals_unreadable, recovered.malformed_lines);
        }
        recovery = Some(recovered);
        workstation.enable_action_proposals(ActionDesk::native(journal));
    }
    // The model is offered only what works on this PC and in this run.
    workstation.offer_available(Probes::without_helper(&initial));
    let mut report = match &mut model {
        Some(model) => {
            if io::stderr().is_terminal() {
                eprintln!(
                    "Investigating with Nebius; initial budget: {MAX_ROUNDS} requests, {MAX_TOOLS} tools, {} seconds. An extension requires your confirmation.",
                    MAX_DURATION.as_secs()
                );
            }
            converse_with_privacy(
                model,
                &[],
                &question,
                &initial,
                &mut workstation,
                &privacy,
                |event| {
                    if verbose {
                        eprintln!("{}", text::tool_event(event));
                    }
                },
                |partial, request| {
                    review_extension(
                        partial,
                        request,
                        json,
                        io::stdin().is_terminal() && io::stderr().is_terminal(),
                        &mut io::stdin().lock(),
                        &mut io::stderr().lock(),
                    )
                    .unwrap_or(false)
                },
            )
        }
        None => local_report(&initial),
    };
    prepared.record_uninspected_scopes(&mut report);
    let jobs = workstation.take_jobs();
    for job in &jobs {
        job.cancel();
    }
    for job in jobs {
        job.wait()?;
    }
    let actions = workstation.take_actions();
    drop(workstation);
    if !json {
        text::report(&mut io::stdout().lock(), &report, verbose)?;
        if !prepared.local_items.is_empty() {
            println!("Authorized folder observations (prepared locally):");
            text::storage_context(&mut io::stdout().lock(), &prepared.local_items)?;
        }
        text::content_comparisons(&mut io::stdout().lock(), prepared.context.comparisons())?;
    }
    let mut results = Vec::new();
    if let Some((desk, pending)) = actions {
        let protection = PolicyProtection {
            policy: &policy,
            prefs: &prefs,
        };
        results = review_actions(&desk, pending, &protection, &mut collector, json)?;
    }
    if json {
        let mut json = diagnosis_json(&report, &results, recovery.as_ref());
        json["storage_preparation"] = prepared.local_report();
        write_json(&json)?;
    }
    let action_failed = results.iter().any(|r| {
        r.get("error").is_some() || r.get("result").is_some_and(|v| v["overall"] != "completed")
    });
    Ok(
        if report.execution_status == ExecutionStatus::Completed && !action_failed {
            0
        } else {
            3
        },
    )
}

pub(super) fn chat(
    options: options::Chat,
    json: bool,
    routing: &model_routing::Routing,
) -> Result<u8> {
    let options::Chat {
        allow_cloud,
        anonymize,
        verbose,
    } = options;
    if !allow_cloud {
        return Err("chat talks with Nebius: add --allow-cloud to send your questions and minimized measurements.".into());
    }
    if json {
        return Err("chat is interactive and has no --json mode; use diagnose --json.".into());
    }
    if !cfg!(windows) {
        return Err("Cloud diagnostics require the native Windows collector; no synthetic or other-platform telemetry is substituted.".into());
    }
    let creds = Credentials::load();
    let config =
        Config::from_lookup(|name| creds.get(name)).map_err(|e| cloud_setup_error(e, &creds))?;
    let mut privacy = PrivacyContext::local();
    if let Some(secret) = creds.get("NEBIUS_API_KEY") {
        privacy.protect_secret(&secret);
    }
    let sampler = cli_sampler(None)?;
    let sharing = if anonymize {
        Sharing::Anonymized
    } else {
        Sharing::Complete
    };
    let (policy, prefs) = (Policy::from_env(), Preferences::default());
    let storage = Arc::new(StorageService::new());
    let mut collector = Collector::new();
    let mut past: Vec<PastTurn> = Vec::new();
    println!("Ask about this PC; an empty line or `exit` ends the conversation.");
    loop {
        print!("\n> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            break;
        }
        let question = line.trim();
        if question.is_empty() || ["exit", "quit"].contains(&question) {
            break;
        }
        if question.len() > MAX_QUESTION_BYTES {
            eprintln!("The question is longer than {MAX_QUESTION_BYTES} bytes; shorten it.");
            continue;
        }
        let initial = collector.snapshot(true, 10);
        // Routing is per turn: a previous escalation never changes the next turn's
        // starting model or resets/extends any investigation budget.
        let mut model = routing.model(&config)?;
        let mut workstation = Workstation::new(&mut collector, storage.clone(), &policy, &prefs);
        workstation.set_sharing(sharing);
        workstation.set_history(sampler.clone(), Arc::new(AtomicBool::new(false)));
        workstation.offer_available(Probes::without_helper(&initial));
        let report = converse_with_privacy(
            &mut model,
            &past,
            question,
            &initial,
            &mut workstation,
            &privacy,
            |event| {
                if verbose {
                    eprintln!("{}", text::tool_event(event));
                }
            },
            |partial, request| {
                review_extension(
                    partial,
                    request,
                    false,
                    io::stdin().is_terminal() && io::stderr().is_terminal(),
                    &mut io::stdin().lock(),
                    &mut io::stderr().lock(),
                )
                .unwrap_or(false)
            },
        );
        let jobs = workstation.take_jobs();
        for job in &jobs {
            job.cancel();
        }
        for job in jobs {
            job.wait()?;
        }
        drop(workstation);
        println!();
        text::report(&mut io::stdout().lock(), &report, verbose)?;
        // Only answered turns are read again; a failed turn adds nothing to follow up on.
        if report.answer.is_some() {
            past.push(PastTurn::from_report(question, &report));
        }
    }
    Ok(0)
}
