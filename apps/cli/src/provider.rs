//! Provider configuration diagnostics and explicitly authorized catalog reads.
use crate::local::write_json;
use crate::{args as options, text, Result};
use pcagent_agent::credentials::{Credentials, Source};
use pcagent_agent::{AgentError, Pricing};
use pcagent_nebius::{Config, Nebius};
use serde_json::json;
use std::error::Error;
use std::io;
use std::io::Write;
use std::time::Duration;

pub(super) fn credentials_file_text(creds: &Credentials) -> String {
    creds
        .file()
        .map_or_else(|| "%APPDATA%\\pcagent\\.env".to_owned(), text::path)
}
pub(super) fn cloud_setup_error(e: AgentError, creds: &Credentials) -> Box<dyn Error> {
    match e {
        AgentError::MissingKey | AgentError::MissingModel | AgentError::InvalidModel => {
            let mut message = format!(
                "{e}.\nPut NEBIUS_API_KEY and NEBIUS_MODEL_ID in {} or set them as environment variables (see \"Opt-in cloud investigation\" in the README), or run without --allow-cloud for a local analysis.",
                credentials_file_text(creds)
            );
            if let Some(problem) = &creds.problem {
                message.push_str(&format!("\nThe credentials file was not used: {problem}."));
            }
            message.into()
        }
        other => other.into(),
    }
}
pub(super) fn print_doctor(
    creds: &Credentials,
    cloud: Option<&std::result::Result<Option<Pricing>, AgentError>>,
    limits: &[&str],
) -> Result<()> {
    use crate::text::label as l;
    let origin = |name: &str| match creds.source(name) {
        Some(Source::Environment) => format!(" ({})", l("cli.doctor.from_env")),
        Some(Source::File) => format!(" ({})", l("cli.doctor.from_file")),
        None => String::new(),
    };
    let key = match creds.get("NEBIUS_API_KEY") {
        Some(_) => format!("{}{}", l("cli.doctor.set"), origin("NEBIUS_API_KEY")),
        None => l("cli.doctor.not_set").to_owned(),
    };
    let model = match creds.get("NEBIUS_MODEL_ID") {
        Some(m) => format!("{}{}", text::clean(&m), origin("NEBIUS_MODEL_ID")),
        None => l("cli.doctor.not_set").to_owned(),
    };
    let file = match (&creds.problem, creds.file_found()) {
        (Some(problem), _) => format!("{}: {problem}", l("cli.doctor.unusable")),
        (None, true) => credentials_file_text(creds),
        (None, false) => format!(
            "{} ({})",
            credentials_file_text(creds),
            l("cli.doctor.not_found")
        ),
    };
    let rows = [
        (l("cli.doctor.pc").to_owned(), String::new()),
        (
            format!("  {}", l("cli.doctor.collector")),
            l(if cfg!(windows) {
                "cli.doctor.available"
            } else {
                "cli.doctor.windows_only"
            })
            .to_owned(),
        ),
        (
            format!("  {}", l("cli.doctor.build")),
            format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        (
            format!("  {}", l("cli.doctor.version_check")),
            l("cli.doctor.not_implemented").to_owned(),
        ),
        (l("cli.doctor.cloud").to_owned(), String::new()),
        (format!("  {}", l("cli.doctor.credentials")), file),
        ("  NEBIUS_API_KEY".to_owned(), key),
        ("  NEBIUS_MODEL_ID".to_owned(), model),
        (
            format!("  {}", l("cli.doctor.catalog")),
            match cloud {
                None => l("cli.doctor.catalog.not_requested").to_owned(),
                Some(Ok(Some(pricing))) => format!(
                    "{}; {}",
                    l("cli.doctor.catalog.found"),
                    text::price_text(pricing)
                ),
                Some(Ok(None)) => format!(
                    "{}; {}",
                    l("cli.doctor.catalog.found"),
                    l("cli.doctor.catalog.no_price")
                ),
                Some(Err(e)) => format!("{}: {e}", l("cli.doctor.failed")),
            },
        ),
        (l("cli.doctor.safety").to_owned(), String::new()),
        (
            format!("  {}", l("cli.doctor.mode")),
            l("cli.doctor.mode.value").to_owned(),
        ),
        (
            format!("  {}", l("cli.doctor.actions")),
            l("cli.doctor.actions.value").to_owned(),
        ),
        (
            format!("  {}", l("cli.doctor.history")),
            l("cli.doctor.history.value").to_owned(),
        ),
    ];
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = io::stdout().lock();
    for (label, value) in rows {
        if value.is_empty() {
            writeln!(out, "{label}")?;
        } else {
            writeln!(out, "{label:<width$}  {value}")?;
        }
    }
    writeln!(out, "{}", crate::text::label("cli.notes"))?;
    for (index, limit) in limits.iter().enumerate() {
        // The JSON keeps the English sentence; the human report shows the chosen language.
        let key = format!("cli.doctor.limit.{index}");
        let shown = crate::text::label(&key);
        writeln!(out, "  - {}", if shown == key { limit } else { shown })?;
    }
    Ok(())
}

pub(super) fn models(options: options::Models, json: bool) -> Result<u8> {
    let options::Models { allow_cloud } = options;
    if !allow_cloud {
        return Err("Listing account models requires --allow-cloud; the request sends only the saved credential, never telemetry.".into());
    }
    let credentials = Credentials::load();
    let catalog = Nebius::account_catalog(|name| credentials.get(name), Duration::from_secs(10))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
    } else {
        println!(
            "NVIDIA Nemotron account catalog ({}):",
            pcagent_shared::format_utc_minute(catalog.read_at_unix_ms)
        );
        for model in catalog.models {
            let prices = model
                .pricing
                .as_ref()
                .and_then(Pricing::per_million_usd)
                .map(|(input, output)| {
                    format!("${input} / ${output} per million input / output tokens")
                })
                .unwrap_or_else(|| "prices unknown".into());
            let context = model
                .context_tokens
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".into());
            println!("  {}: {prices}; context {context}", model.id);
        }
    }
    Ok(0)
}

pub(super) fn doctor(options: options::Doctor, json: bool) -> Result<u8> {
    let options::Doctor { check_cloud } = options;
    let creds = Credentials::load();
    let cloud_result = if check_cloud {
        Some(
            Config::from_lookup(|name| creds.get(name))
                .and_then(Nebius::new)
                .and_then(|mut client| {
                    client
                        .check_catalog(Duration::from_secs(10))
                        .map(|()| client.catalog_pricing().cloned())
                }),
        )
    } else {
        None
    };
    let cloud_failed = cloud_result.as_ref().is_some_and(|r| r.is_err());
    let limits = [
        "Doctor does not yet validate Windows build, storage capabilities or a local database.",
        "A catalog check does not validate tool calling, diagnostic quality or latency.",
    ];
    if json {
        let report = json!({"schema_version":1,"native_collector_available":cfg!(windows),
            "compiled_os":std::env::consts::OS,"compiled_arch":std::env::consts::ARCH,
            "product_target":"Windows 11 x64","windows_version_check":"not_implemented",
            "read_only":true,"persistent_history":false,"actions_available":cfg!(windows),
            "nebius_key_configured":creds.get("NEBIUS_API_KEY").is_some(),"nebius_model_configured":creds.get("NEBIUS_MODEL_ID").is_some(),
            "credentials_file_found":creds.file_found(),
            "cloud_check":match &cloud_result { None=>"not_requested".to_string(),Some(Ok(_))=>"catalog_ok".to_string(),Some(Err(e))=>e.to_string() },
            "pricing":cloud_result.as_ref().and_then(|r| r.as_ref().ok()).cloned().flatten(),
            "limits":limits});
        write_json(&report)?;
    } else {
        print_doctor(&creds, cloud_result.as_ref(), &limits)?;
    }
    Ok(if cfg!(windows) && !cloud_failed { 0 } else { 3 })
}
