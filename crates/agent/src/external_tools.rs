//! Typed web lookups (chantier 15, contract C6). The model chooses a lookup and a measured
//! reference; the query is composed here from local measurements, so no text written by the model,
//! no user name, path, command line, file content, secret, serial number or machine identifier
//! can leave the PC. Results are web snippets, cited `[W1]`, `[W2]`…: they never prove a number
//! measured on this PC, and a PC measurement never proves them.
use crate::{
    tavily::{WebError, WebQuery, WebSearcher},
    tools::{Args, Param, ParamKind, ToolClass, ToolOutput, ToolSpec},
};
use pcagent_inspect::{Inspection, Query};
use pcagent_shared::Status;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Web searches allowed in one turn.
pub const MAX_WEB_SEARCHES: usize = 3;

const DEVICE_REF: Param = Param {
    name: "device_ref",
    description: "A device reference returned by get_drivers in this turn.",
    kind: ParamKind::OpaqueRef { prefix: "device-" },
    required: true,
};

pub const LOOKUP_DRIVER_NOTES: ToolSpec = ToolSpec {
    name: "lookup_driver_notes",
    class: ToolClass::External,
    description: "Search the driver maker's website for notes on one installed driver: latest version, open and fixed issues, security bulletins. The query is composed locally from the device's provider, model and installed version (call get_drivers first). Returns web snippets cited [W1], [W2]…",
    params: &[DEVICE_REF],
};
pub const LOOKUP_KNOWN_ISSUE: ToolSpec = ToolSpec {
    name: "lookup_known_issue",
    class: ToolClass::External,
    description: "Search Microsoft's documentation for a known issue: give either kb (an update number like KB5034441) or code (a stop or error code like 0x0000009F), as measured by get_recent_events, get_system_state or get_drivers. Returns web snippets cited [W1], [W2]…",
    params: &[
        Param {
            name: "kb",
            description: "A Windows update number from a measurement, like KB5034441.",
            kind: ParamKind::OpaqueRef { prefix: "KB" },
            required: false,
        },
        Param {
            name: "code",
            description: "A stop or error code from a measurement, like 0x0000009F.",
            kind: ParamKind::OpaqueRef { prefix: "0x" },
            required: false,
        },
    ],
};
pub const LOOKUP_UPGRADE_PRICE: ToolSpec = ToolSpec {
    name: "lookup_upgrade_price",
    class: ToolClass::External,
    description: "Search shops of the user's country for the price of a compatible memory module, composed locally from the measured memory type, form factor, speed and module size. Returns dated web snippets cited [W1], [W2]…",
    params: &[Param {
        name: "component",
        description: "The part to price.",
        kind: ParamKind::OneOf(&["memory"]),
        required: true,
    }],
};
pub const LOOKUP_PC_SPECS: ToolSpec = ToolSpec {
    name: "lookup_pc_specs",
    class: ToolClass::External,
    description: "Search the PC maker's website for this model's specification sheet: maximum memory, soldered memory, free slots. The query is composed locally from the measured maker and model. Returns web snippets cited [W1], [W2]…",
    params: &[],
};
pub const TOOLS: &[ToolSpec] = &[
    LOOKUP_DRIVER_NOTES,
    LOOKUP_KNOWN_ISSUE,
    LOOKUP_UPGRADE_PRICE,
    LOOKUP_PC_SPECS,
];

pub fn is_external(tool: &str) -> bool {
    TOOLS.iter().any(|spec| spec.name == tool)
}

/// Whether a host belongs to a site the lookups can search: only such a source may be opened.
pub fn searchable(host: &str) -> bool {
    let lists = [
        VENDORS.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
        MAKERS.iter().map(|(_, d)| *d).collect(),
        MICROSOFT.to_vec(),
        SHOPS
            .iter()
            .flat_map(|(_, d, _)| d.iter().copied())
            .collect(),
        DEFAULT_SHOPS.to_vec(),
    ];
    lists
        .iter()
        .flatten()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

/// A driver as `get_drivers` measured it, with its local names.
#[derive(Debug, Clone, Default, PartialEq)]
struct Driver {
    provider: Option<String>,
    model: Option<String>,
    version: Option<String>,
}

/// The memory and model facts that compose price and specification lookups.
#[derive(Debug, Clone, Default, PartialEq)]
struct Hardware {
    manufacturer: Option<String>,
    model: Option<String>,
    memory_type: Option<String>,
    form_factor: Option<String>,
    speed_mt_s: Option<u64>,
    module_bytes: Option<u64>,
}

pub struct ExternalTools {
    searcher: Box<dyn WebSearcher + Send>,
    /// The two-letter country of the Windows region, for prices.
    country: Option<String>,
    searches: usize,
    sources: usize,
    drivers: BTreeMap<String, Driver>,
    hardware: Option<Hardware>,
    /// Tavily credits used in this turn, when reported.
    pub credits: f64,
}

impl ExternalTools {
    pub fn new(searcher: Box<dyn WebSearcher + Send>, country: Option<String>) -> Self {
        Self {
            searcher,
            country,
            searches: 0,
            sources: 0,
            drivers: BTreeMap::new(),
            hardware: None,
            credits: 0.0,
        }
    }

    /// Keeps the local facts of a measurement the lookups may build on; names stay on the PC.
    pub fn retain(&mut self, tool: &str, inspection: &Inspection) {
        match tool {
            "get_drivers" => {
                for observation in &inspection.observations {
                    if !observation.entity_ref.starts_with("device-") {
                        continue;
                    }
                    let driver = self
                        .drivers
                        .entry(observation.entity_ref.clone())
                        .or_default();
                    match observation.metric.as_str() {
                        "driver_provider_ref" => {
                            driver.provider = name(inspection, &observation.value)
                        }
                        "driver_model_ref" => driver.model = name(inspection, &observation.value),
                        "driver_version" => {
                            driver.version = observation.value.as_str().map(str::to_owned)
                        }
                        _ => {}
                    }
                }
            }
            "get_hardware_inventory" => self.hardware = Some(hardware(inspection)),
            _ => {}
        }
    }

    /// Runs a lookup; `None` for any other tool.
    pub fn run(&mut self, spec: &ToolSpec, args: &Args) -> Option<ToolOutput> {
        let query = match spec.name {
            "lookup_driver_notes" => self.driver_query(args.text("device_ref").unwrap_or("")),
            "lookup_known_issue" => known_issue_query(args.text("kb"), args.text("code")),
            "lookup_upgrade_price" => self.price_query(),
            "lookup_pc_specs" => self.specs_query(),
            _ => return None,
        };
        let query = match query {
            Ok(query) => query,
            Err(reason) => return Some(unavailable(spec.name, Status::Partial, reason, None)),
        };
        if self.searches >= MAX_WEB_SEARCHES {
            return Some(unavailable(
                spec.name,
                Status::Partial,
                "The web search limit for this turn is reached; answer with the sources already found.",
                Some(&query),
            ));
        }
        self.searches += 1;
        let searched_at = unix_ms();
        let search = match self.searcher.search(&query) {
            Ok(search) => search,
            Err(error) => {
                return Some(unavailable(
                    spec.name,
                    Status::Error,
                    match error {
                        WebError::Unauthorized => "The web search key was refused.",
                        WebError::Limited => "The web search plan's limit is reached.",
                        WebError::Timeout => "The web search did not answer in time.",
                        _ => "The web search failed.",
                    },
                    Some(&query),
                ))
            }
        };
        self.credits += search.credits.unwrap_or(0.0);
        let sources: Vec<Value> = search
            .results
            .iter()
            .map(|result| {
                self.sources += 1;
                json!({"source_id":format!("W{}", self.sources),"title":result.title,
                    "domain":result.domain,"url":result.url,
                    "published_date":result.published_date,"snippet":result.snippet})
            })
            .collect();
        let status = if sources.is_empty() {
            Status::Partial
        } else {
            Status::Ok
        };
        let mut cloud = json!({"status":status,"origin":"web","tool":spec.name,"query":query.query,
            "domains":query.domains,"country":query.country,"searched_at_unix_ms":searched_at,
            "sources":sources,
            "limitations":["Web snippets are not measurements of this PC and may be outdated or wrong.",
                "published_date is the source's own date when known; it can be missing."]});
        if sources.is_empty() {
            cloud["limitations"]
                .as_array_mut()
                .expect("array")
                .push(json!("No result from the allowed sites."));
        }
        Some(ToolOutput {
            status,
            inspection_complete: false,
            cloud,
            evidence: Vec::new(),
            findings: Vec::new(),
            processes: Vec::new(),
            storage_items: Vec::new(),
            limitations: Vec::new(),
        })
    }

    fn driver_query(&self, device: &str) -> Result<WebQuery, &'static str> {
        let driver = self
            .drivers
            .get(device)
            .ok_or("Unknown device_ref: call get_drivers in this turn and use one of its device references.")?;
        let model = driver
            .model
            .as_deref()
            .ok_or("This device's model was not reported, so no query can be composed.")?;
        let provider = driver.provider.as_deref().unwrap_or_default();
        let domains = vendor_domains(&format!("{provider} {model}"))
            .or_else(|| {
                self.hardware
                    .as_ref()
                    .and_then(|h| h.manufacturer.as_deref())
                    .and_then(maker_domain)
                    .map(|d| vec![d])
            })
            .ok_or("This driver's maker has no known official website to search.")?;
        let mut query = if model
            .to_ascii_lowercase()
            .contains(&provider.to_ascii_lowercase())
        {
            model.to_owned()
        } else {
            format!("{provider} {model}")
        };
        query.push_str(" driver");
        if let Some(version) = &driver.version {
            query.push_str(&format!(" {version}"));
        }
        query.push_str(" release notes known issues");
        Ok(WebQuery {
            query: bounded(&query),
            domains,
            country: None,
        })
    }

    fn price_query(&mut self) -> Result<WebQuery, &'static str> {
        let hardware = self.hardware()?;
        let kind = hardware
            .memory_type
            .as_deref()
            .ok_or("The memory type was not reported, so no compatible module can be priced.")?
            .to_ascii_uppercase();
        let mut query = String::new();
        if let Some(bytes) = hardware.module_bytes {
            query.push_str(&format!("{} GB ", bytes / (1 << 30)));
        }
        query.push_str(&kind);
        match hardware.form_factor.as_deref() {
            Some("so_dimm") => query.push_str(" SO-DIMM"),
            Some("dimm") => query.push_str(" DIMM"),
            _ => {}
        }
        if let Some(speed) = hardware.speed_mt_s {
            query.push_str(&format!(" {speed} MT/s"));
        }
        query.push_str(" RAM price");
        let (domains, country) = shops(self.country.as_deref());
        Ok(WebQuery {
            query: bounded(&query),
            domains,
            country,
        })
    }

    fn specs_query(&mut self) -> Result<WebQuery, &'static str> {
        let hardware = self.hardware()?;
        let manufacturer = hardware
            .manufacturer
            .as_deref()
            .ok_or("The PC maker was not reported by the firmware.")?;
        let model = hardware
            .model
            .as_deref()
            .ok_or("The PC model was not reported by the firmware.")?;
        let domain =
            maker_domain(manufacturer).ok_or("This PC maker has no known website to search.")?;
        Ok(WebQuery {
            query: bounded(&format!(
                "{manufacturer} {model} specifications maximum memory slots"
            )),
            domains: vec![domain],
            country: None,
        })
    }

    /// The memory and model facts, measured now when no inventory was read in this turn.
    fn hardware(&mut self) -> Result<Hardware, &'static str> {
        if self.hardware.is_none() {
            self.hardware = Some(hardware(&pcagent_inspect::inspect(Query::Hardware)));
        }
        self.hardware
            .clone()
            .ok_or("The hardware inventory is unavailable.")
    }
}

const MICROSOFT: [&str; 2] = ["learn.microsoft.com", "support.microsoft.com"];

fn known_issue_query(kb: Option<&str>, code: Option<&str>) -> Result<WebQuery, &'static str> {
    let query = match (kb, code) {
        (Some(kb), None) => {
            let digits = &kb[2..];
            if !(6..=8).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("kb must look like KB5034441.");
            }
            format!("{kb} known issues")
        }
        (None, Some(code)) => {
            let hex = &code[2..];
            if !(1..=8).contains(&hex.len()) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("code must look like 0x0000009F.");
            }
            format!("Windows stop error {code} cause")
        }
        _ => return Err("Give exactly one of kb or code."),
    };
    Ok(WebQuery {
        query,
        domains: MICROSOFT.iter().map(|d| (*d).to_owned()).collect(),
        country: None,
    })
}

/// The local name behind an opaque name reference.
fn name(inspection: &Inspection, value: &Value) -> Option<String> {
    let reference = value.as_str()?;
    inspection
        .names
        .get(reference)
        .map(|name| bounded(name.trim()))
        .filter(|name| !name.is_empty())
}

fn hardware(inspection: &Inspection) -> Hardware {
    let mut facts = Hardware::default();
    let mut modules: Vec<u64> = Vec::new();
    for observation in &inspection.observations {
        match observation.metric.as_str() {
            "pc_manufacturer_ref" => facts.manufacturer = name(inspection, &observation.value),
            "pc_model_ref" => facts.model = name(inspection, &observation.value),
            "memory_type" if facts.memory_type.is_none() => {
                facts.memory_type = observation.value.as_str().map(str::to_owned)
            }
            "memory_form_factor" if facts.form_factor.is_none() => {
                facts.form_factor = observation.value.as_str().map(str::to_owned)
            }
            "memory_configured_speed_megatransfers_per_second"
            | "memory_speed_megatransfers_per_second" => {
                if let Some(speed) = observation.value.as_u64().filter(|s| *s > 0) {
                    facts.speed_mt_s = Some(facts.speed_mt_s.map_or(speed, |s| s.max(speed)));
                }
            }
            "memory_capacity_bytes" => modules.extend(observation.value.as_u64()),
            _ => {}
        }
    }
    facts.module_bytes = modules.into_iter().filter(|b| *b > 0).max();
    facts
}

/// Official sites of common chip and driver makers, matched on the provider and model text.
const VENDORS: [(&str, &str); 11] = [
    ("nvidia", "nvidia.com"),
    ("intel", "intel.com"),
    ("advanced micro devices", "amd.com"),
    ("amd", "amd.com"),
    ("radeon", "amd.com"),
    ("realtek", "realtek.com"),
    ("qualcomm", "qualcomm.com"),
    ("mediatek", "mediatek.com"),
    ("broadcom", "broadcom.com"),
    ("marvell", "marvell.com"),
    ("samsung", "samsung.com"),
];

fn vendor_domains(text: &str) -> Option<Vec<String>> {
    let text = text.to_ascii_lowercase();
    let domain = VENDORS
        .iter()
        .find(|(word, _)| {
            text.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == *word)
                || (word.contains(' ') && text.contains(word))
        })
        .map(|(_, domain)| (*domain).to_owned())?;
    Some(vec![domain])
}

/// The website of a PC maker, as the firmware names it.
const MAKERS: [(&str, &str); 17] = [
    ("dell", "dell.com"),
    ("hewlett", "hp.com"),
    ("hp", "hp.com"),
    ("lenovo", "lenovo.com"),
    ("asus", "asus.com"),
    ("acer", "acer.com"),
    ("micro-star", "msi.com"),
    ("msi", "msi.com"),
    ("microsoft", "microsoft.com"),
    ("samsung", "samsung.com"),
    ("gigabyte", "gigabyte.com"),
    ("razer", "razer.com"),
    ("huawei", "consumer.huawei.com"),
    ("dynabook", "dynabook.com"),
    ("toshiba", "dynabook.com"),
    ("lg electronics", "lg.com"),
    ("framework", "frame.work"),
];

fn maker_domain(manufacturer: &str) -> Option<String> {
    let maker = manufacturer.to_ascii_lowercase();
    MAKERS
        .iter()
        .find(|(word, _)| {
            maker
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w == *word)
                || ((word.contains(' ') || word.contains('-')) && maker.contains(word))
        })
        .map(|(_, domain)| (*domain).to_owned())
}

/// Shops and price comparators by Windows region, with Tavily's country boost.
const SHOPS: [(&str, &[&str], &str); 7] = [
    (
        "FR",
        &[
            "ldlc.com",
            "materiel.net",
            "amazon.fr",
            "idealo.fr",
            "ledenicheur.fr",
        ],
        "france",
    ),
    (
        "BE",
        &["coolblue.be", "amazon.com.be", "alternate.be"],
        "belgium",
    ),
    (
        "CH",
        &["digitec.ch", "galaxus.ch", "toppreise.ch"],
        "switzerland",
    ),
    (
        "CA",
        &["newegg.ca", "amazon.ca", "canadacomputers.com"],
        "canada",
    ),
    (
        "DE",
        &["geizhals.de", "idealo.de", "alternate.de", "amazon.de"],
        "germany",
    ),
    (
        "GB",
        &[
            "scan.co.uk",
            "overclockers.co.uk",
            "amazon.co.uk",
            "pricespy.co.uk",
        ],
        "united kingdom",
    ),
    (
        "US",
        &[
            "newegg.com",
            "bestbuy.com",
            "amazon.com",
            "pcpartpicker.com",
        ],
        "united states",
    ),
];
const DEFAULT_SHOPS: [&str; 3] = ["amazon.com", "newegg.com", "pcpartpicker.com"];

/// Shops and price comparators for the region's country, and Tavily's country boost.
fn shops(country: Option<&str>) -> (Vec<String>, Option<String>) {
    match SHOPS.iter().find(|(code, _, _)| Some(*code) == country) {
        Some((_, domains, name)) => (
            domains.iter().map(|d| (*d).to_owned()).collect(),
            Some((*name).to_owned()),
        ),
        None => (
            DEFAULT_SHOPS.iter().map(|d| (*d).to_owned()).collect(),
            None,
        ),
    }
}

fn unavailable(tool: &str, status: Status, reason: &str, query: Option<&WebQuery>) -> ToolOutput {
    let mut cloud = json!({"status":status,"origin":"web","tool":tool,"sources":[],"error":reason,
        "limitations":[reason]});
    if let Some(query) = query {
        cloud["query"] = json!(query.query);
        cloud["domains"] = json!(query.domains);
    }
    ToolOutput {
        status,
        inspection_complete: false,
        cloud,
        evidence: Vec::new(),
        findings: Vec::new(),
        processes: Vec::new(),
        storage_items: Vec::new(),
        limitations: vec![reason.to_owned()],
    }
}

/// Names come from this PC's firmware and drivers: bounded, and only letters, digits and simple
/// punctuation reach the query.
fn bounded(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| c.is_alphanumeric() || " .-+/()_#".contains(*c))
        .take(200)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tavily::{WebResult, WebSearch};
    use std::sync::{Arc, Mutex};

    struct Scripted(Arc<Mutex<Vec<WebQuery>>>);
    impl WebSearcher for Scripted {
        fn search(&mut self, query: &WebQuery) -> Result<WebSearch, WebError> {
            self.0.lock().unwrap().push(query.clone());
            Ok(WebSearch {
                results: vec![WebResult {
                    title: "GeForce driver 566.36".into(),
                    url: "https://www.nvidia.com/en-us/drivers/details/237728/".into(),
                    domain: "nvidia.com".into(),
                    snippet: "Version 566.36 fixes stutter in some games.".into(),
                    published_date: Some("2024-12-05".into()),
                }],
                credits: Some(1.0),
                duration_ms: 400,
            })
        }
    }

    fn tools() -> (ExternalTools, Arc<Mutex<Vec<WebQuery>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            ExternalTools::new(Box::new(Scripted(seen.clone())), Some("FR".into())),
            seen,
        )
    }

    fn inspection(rows: &[(&str, &str, Value)], names: &[(&str, &str)]) -> Inspection {
        let mut inspection: Inspection = serde_json::from_value(json!({
            "status":"ok","measured_at_unix_ms":1,"window":{"started_at_unix_ms":1,"finished_at_unix_ms":1,"duration_ms":0},
            "origin":"live","observations":[],"coverage":[]}))
        .unwrap();
        for (entity, metric, value) in rows {
            inspection.observations.push(
                serde_json::from_value(json!({"observation_id":"m","measured_at_unix_ms":1,
                    "window":{"started_at_unix_ms":1,"finished_at_unix_ms":1,"duration_ms":0},"entity_ref":entity,
                    "metric":metric,"value":value,"unit":"x","status":"ok","source":"t","origin":"live"}))
                .unwrap(),
            );
        }
        for (reference, name) in names {
            inspection.names.insert((*reference).into(), (*name).into());
        }
        inspection
    }

    fn args(pairs: &[(&'static str, &str)]) -> Args {
        let spec = TOOLS
            .iter()
            .find(|s| {
                pairs
                    .iter()
                    .all(|(k, _)| s.params.iter().any(|p| p.name == *k))
            })
            .unwrap();
        let object: serde_json::Map<String, Value> =
            pairs.iter().map(|(k, v)| ((*k).into(), json!(v))).collect();
        spec.validate(&Value::Object(object).to_string()).unwrap()
    }

    #[test]
    fn driver_notes_compose_the_query_from_the_measured_driver_only() {
        let (mut tools, seen) = tools();
        let measured = inspection(
            &[
                ("device-1", "driver_provider_ref", json!("name-1")),
                ("device-1", "driver_model_ref", json!("name-2")),
                ("device-1", "driver_version", json!("32.0.15.6636")),
            ],
            &[("name-1", "NVIDIA"), ("name-2", "NVIDIA GeForce RTX 3060")],
        );
        tools.retain("get_drivers", &measured);
        let out = tools
            .run(&LOOKUP_DRIVER_NOTES, &args(&[("device_ref", "device-1")]))
            .unwrap();
        assert_eq!(out.status, Status::Ok);
        let query = seen.lock().unwrap()[0].clone();
        assert_eq!(
            query.query,
            "NVIDIA GeForce RTX 3060 driver 32.0.15.6636 release notes known issues"
        );
        assert_eq!(query.domains, ["nvidia.com"]);
        assert_eq!(out.cloud["sources"][0]["source_id"], "W1");
        assert_eq!(out.cloud["sources"][0]["domain"], "nvidia.com");
        assert_eq!(out.cloud["origin"], "web");
        assert!(!out.inspection_complete);
        // An unknown device never reaches the web.
        let out = tools
            .run(&LOOKUP_DRIVER_NOTES, &args(&[("device_ref", "device-9")]))
            .unwrap();
        assert_eq!(out.status, Status::Partial);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn known_issues_accept_only_update_numbers_and_codes() {
        let (mut tools, seen) = tools();
        let out = tools
            .run(&LOOKUP_KNOWN_ISSUE, &args(&[("kb", "KB5034441")]))
            .unwrap();
        assert_eq!(out.status, Status::Ok);
        assert_eq!(seen.lock().unwrap()[0].query, "KB5034441 known issues");
        assert_eq!(
            seen.lock().unwrap()[0].domains,
            ["learn.microsoft.com", "support.microsoft.com"]
        );
        for bad in [("kb", "KBabc"), ("code", "0xZZ"), ("kb", "KB12")] {
            let out = tools.run(&LOOKUP_KNOWN_ISSUE, &args(&[bad])).unwrap();
            assert_eq!(out.status, Status::Partial, "{bad:?}");
        }
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn prices_use_the_measured_memory_and_the_region_shops() {
        let (mut tools, seen) = tools();
        let measured = inspection(
            &[
                ("system", "pc_manufacturer_ref", json!("name-1")),
                ("system", "pc_model_ref", json!("name-2")),
                ("memory-slot-1", "memory_type", json!("ddr5")),
                ("memory-slot-1", "memory_form_factor", json!("so_dimm")),
                ("memory-slot-1", "memory_capacity_bytes", json!(8_u64 << 30)),
                (
                    "memory-slot-1",
                    "memory_configured_speed_megatransfers_per_second",
                    json!(5600),
                ),
            ],
            &[("name-1", "LENOVO"), ("name-2", "Legion 5 15ARH7H")],
        );
        tools.retain("get_hardware_inventory", &measured);
        tools
            .run(&LOOKUP_UPGRADE_PRICE, &args(&[("component", "memory")]))
            .unwrap();
        tools.run(&LOOKUP_PC_SPECS, &Args::default()).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].query, "8 GB DDR5 SO-DIMM 5600 MT/s RAM price");
        assert!(seen[0].domains.contains(&"ldlc.com".to_owned()));
        assert_eq!(seen[0].country.as_deref(), Some("france"));
        assert_eq!(
            seen[1].query,
            "LENOVO Legion 5 15ARH7H specifications maximum memory slots"
        );
        assert_eq!(seen[1].domains, ["lenovo.com"]);
    }

    #[test]
    fn a_turn_searches_at_most_three_times_and_numbers_sources_across_lookups() {
        let (mut tools, seen) = tools();
        for i in 0..MAX_WEB_SEARCHES {
            let out = tools
                .run(&LOOKUP_KNOWN_ISSUE, &args(&[("kb", "KB5034441")]))
                .unwrap();
            assert_eq!(out.cloud["sources"][0]["source_id"], format!("W{}", i + 1));
        }
        let out = tools
            .run(&LOOKUP_KNOWN_ISSUE, &args(&[("kb", "KB5034441")]))
            .unwrap();
        assert_eq!(out.status, Status::Partial);
        assert_eq!(seen.lock().unwrap().len(), MAX_WEB_SEARCHES);
        assert_eq!(tools.credits, MAX_WEB_SEARCHES as f64);
    }

    #[test]
    fn local_names_are_reduced_to_plain_words_before_a_query() {
        assert_eq!(bounded("ACME\u{0} \"Pro\"  15\n<x>"), "ACME Pro 15 x");
        assert_eq!(maker_domain("Hewlett-Packard").as_deref(), Some("hp.com"));
        assert_eq!(
            maker_domain("Micro-Star International Co., Ltd.").as_deref(),
            Some("msi.com")
        );
        assert_eq!(maker_domain("Unknown Inc"), None);
        assert_eq!(
            vendor_domains("Advanced Micro Devices, Inc. AMD Radeon(TM) Graphics"),
            Some(vec!["amd.com".to_owned()])
        );
        assert_eq!(
            vendor_domains("Intel Corporation Intel(R) Wi-Fi 6"),
            Some(vec!["intel.com".to_owned()])
        );
        assert!(
            searchable("support.microsoft.com")
                && searchable("www.nvidia.com".trim_start_matches("www."))
        );
        assert!(searchable("ldlc.com") && !searchable("evil.com") && !searchable("evilnvidia.com"));
    }
}
