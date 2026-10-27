use super::com_error;
use super::drivers::InstalledDriver;
use crate::{
    parse::driver_version::{self, ole_date_ms},
    CoverageStatus as C, Inspection, WindowsUpdateMode,
};
use ::windows::core::{Interface, BSTR};
use ::windows::Win32::System::{Com::*, UpdateAgent::*};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[::windows::core::implement(ISearchCompletedCallback)]
struct SearchCompletion {
    completed: Arc<AtomicBool>,
}
impl ISearchCompletedCallback_Impl for SearchCompletion_Impl {
    fn Invoke(
        &self,
        _job: ::windows::core::Ref<ISearchJob>,
        _args: ::windows::core::Ref<ISearchCompletedCallbackArgs>,
    ) -> ::windows::core::Result<()> {
        self.completed.store(true, Ordering::Release);
        Ok(())
    }
}
/// Kept on the same native worker until WUA releases the callback. The caller receives usable
/// installed readings first; a slow cancellation cannot discard them or spawn another worker.
pub(super) struct SearchCleanup {
    job: ISearchJob,
    searcher: IUpdateSearcher,
    _session: IUpdateSession,
    _callback: ISearchCompletedCallback,
    completed: Arc<AtomicBool>,
    ended: bool,
}
impl SearchCleanup {
    pub(super) fn finish(self) {
        // SAFETY: all objects remain in the initialized worker COM apartment; CleanUp is called
        // outside the callback and before releasing its/job references. It may wait for abort.
        unsafe {
            if !self.completed.load(Ordering::Acquire) {
                let _ = self.job.RequestAbort();
            }
            let _ = self.job.CleanUp();
            if !self.ended {
                let _ = self.searcher.EndSearch(&self.job);
            }
        }
    }
}

pub(super) fn collect(out: &mut Inspection) {
    const SOURCE: &str = "WindowsUpdateAgent.QueryHistory (local database only)";
    // SAFETY: COM is initialized on the native worker. Creating a local session does not search,
    // download or install; this module calls only history and read-only status getters.
    let session: ::windows::core::Result<IUpdateSession> =
        unsafe { CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER) };
    let session = match session {
        Ok(s) => s,
        Err(e) => {
            out.coverage(
                "windows_update_history",
                com_error(&e),
                SOURCE,
                "Windows Update Agent local session unavailable.",
            );
            return;
        }
    };
    // SAFETY: read-only getter on a local COM object; no Install/Uninstall or search is invoked.
    match unsafe {
        session
            .CreateUpdateInstaller()
            .and_then(|installer| installer.IsBusy())
    } {
        Ok(busy) => {
            out.value(
                "system",
                "windows_update_installation_busy",
                busy.0 != 0,
                "boolean",
                "WindowsUpdateAgent.IUpdateInstaller.IsBusy",
            );
            out.coverage("windows_update_busy",C::Ok,"WindowsUpdateAgent.IUpdateInstaller.IsBusy","Installation/uninstallation state at collection time; this does not indicate download progress or all servicing activities.");
        }
        Err(e) => out.coverage(
            "windows_update_busy",
            com_error(&e),
            "WindowsUpdateAgent.IUpdateInstaller.IsBusy",
            "Installation state unavailable.",
        ),
    }
    let history = (|| -> ::windows::core::Result<(i32, Option<u64>)> {
        // SAFETY: QueryHistory reads the local historical database and is bounded to 100 entries.
        unsafe {
            let searcher = session.CreateUpdateSearcher()?;
            let total = searcher.GetTotalHistoryCount()?.max(0);
            if total == 0 {
                return Ok((0, None));
            }
            let history = searcher.QueryHistory(0, total.min(100))?;
            let count = history.Count()?.clamp(0, 100);
            let mut latest = None;
            for index in 0..count {
                let entry = history.get_Item(index)?;
                if entry.Operation()? == uoInstallation && entry.ResultCode()? == orcSucceeded {
                    if let Some(date) = ole_date_ms(entry.Date()?) {
                        latest = Some(latest.unwrap_or(0_u64).max(date));
                    }
                }
            }
            Ok((total, latest))
        }
    })();
    match history {
        Ok((count, latest)) => {
            out.value(
                "system",
                "windows_update_history_entry_count",
                count as u64,
                "count",
                SOURCE,
            );
            out.value(
                "system",
                "latest_successful_update_install_unix_ms",
                latest.map(Value::from).unwrap_or(Value::Null),
                "unix_ms",
                SOURCE,
            );
            out.coverage("windows_update_history",if latest.is_some()||count==0{C::Ok}else{C::Partial},SOURCE,"Latest successful installation among the newest 100 local update-history events, sorted by Windows in descending date order. Cleared history and non-WUA installations cannot be reconstructed; no update search or network request is performed.");
        }
        Err(e) => out.coverage(
            "windows_update_history",
            com_error(&e),
            SOURCE,
            "Local update history could not be read.",
        ),
    }
}
pub(super) fn driver_offers(
    out: &mut Inspection,
    installed: &[InstalledDriver],
    limit: usize,
    mode: WindowsUpdateMode,
) -> Option<SearchCleanup> {
    const SOURCE: &str =
        "WindowsUpdateAgent.IUpdateSearcher.BeginSearch(IsInstalled=0 and Type='Driver')";
    if mode == WindowsUpdateMode::None {
        out.coverage(
            "windows_update_driver_offers",
            C::Unsupported,
            SOURCE,
            "Windows Update driver search was not requested.",
        );
        return None;
    }
    let search = (|| -> ::windows::core::Result<SearchCleanup> {
        // SAFETY: COM is initialized on the dedicated worker. Session and search are read-only;
        // only the explicit Online enum can enable a service request. No download/install exists.
        unsafe {
            let session: IUpdateSession =
                CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
            session.SetClientApplicationID(&BSTR::from("PC Agent"))?;
            let searcher = session.CreateUpdateSearcher()?;
            searcher.SetOnline(::windows::Win32::Foundation::VARIANT_BOOL(
                if mode == WindowsUpdateMode::Online {
                    -1
                } else {
                    0
                },
            ))?;
            // A read-only inspection must not upgrade Windows Update Agent while searching.
            searcher
                .SetCanAutomaticallyUpgradeService(::windows::Win32::Foundation::VARIANT_BOOL(0))?;
            let completed = Arc::new(AtomicBool::new(false));
            let callback: ISearchCompletedCallback = SearchCompletion {
                completed: completed.clone(),
            }
            .into();
            let job = searcher.BeginSearch(
                &BSTR::from("IsInstalled=0 and Type='Driver'"),
                &callback,
                &::windows::Win32::System::Variant::VARIANT::default(),
            )?;
            Ok(SearchCleanup {
                job,
                searcher,
                _session: session,
                _callback: callback,
                completed,
                ended: false,
            })
        }
    })();
    let mut cleanup = match search {
        Ok(cleanup) => cleanup,
        Err(err) => {
            out.value(
                "system",
                "windows_update_driver_search_error_code",
                format!("0x{:08X}", err.code().0 as u32),
                "hex_code",
                SOURCE,
            );
            out.coverage(
                "windows_update_driver_offers",
                com_error(&err),
                SOURCE,
                "Driver offer search failed; no conclusion about available updates can be made.",
            );
            return None;
        }
    };
    let started = Instant::now();
    while !cleanup.completed.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(7) {
        std::thread::sleep(Duration::from_millis(25));
    }
    if !cleanup.completed.load(Ordering::Acquire) {
        out.coverage("windows_update_driver_offers", C::Partial, SOURCE, "Asynchronous driver search exceeded seven seconds. Installed driver readings are preserved; offers remain unknown. Abort/cleanup continues on the same worker and subsequent inspections stay busy until it finishes.");
        return Some(cleanup);
    }
    // SAFETY: completion callback fired; EndSearch consumes its read-only results.
    let completed_result = unsafe { cleanup.searcher.EndSearch(&cleanup.job) };
    cleanup.ended = true;
    let result = match completed_result {
        Ok(result) => result,
        Err(err) => {
            out.value(
                "system",
                "windows_update_driver_search_error_code",
                format!("0x{:08X}", err.code().0 as u32),
                "hex_code",
                SOURCE,
            );
            out.coverage(
                "windows_update_driver_offers",
                com_error(&err),
                SOURCE,
                "Completed driver search yielded no usable result.",
            );
            return Some(cleanup);
        }
    };
    // SAFETY: read-only result collection getters on the same live COM apartment.
    let values = unsafe {
        result
            .Updates()
            .and_then(|updates| updates.Count().map(|count| (updates, count.max(0))))
    };
    let (updates, total) = match values {
        Ok(values) => values,
        Err(err) => {
            out.coverage(
                "windows_update_driver_offers",
                com_error(&err),
                SOURCE,
                "Driver offer collection unavailable.",
            );
            return Some(cleanup);
        }
    };
    // SAFETY: read-only operation result property; unknown result code remains partial.
    let mut partial = !unsafe { result.ResultCode() }.is_ok_and(|code| code == orcSucceeded);
    let keys = installed
        .iter()
        .map(|driver| {
            (
                driver.class,
                driver.provider.as_deref().unwrap_or(""),
                driver.model.as_deref().unwrap_or(""),
            )
        })
        .collect::<Vec<_>>();
    let mut offered = 0;
    let mut examined = 0;
    for index in 0..total.min(200) {
        if offered >= limit {
            partial = true;
            break;
        }
        examined += 1;
        // SAFETY: bounded index in the result collection, then QueryInterface for typed drivers.
        let driver = match unsafe { updates.get_Item(index) }
            .and_then(|update| update.cast::<IWindowsDriverUpdate>())
        {
            Ok(driver) => driver,
            Err(_) => {
                partial = true;
                continue;
            }
        };
        // SAFETY: only nonpersonal class, provider/model, version date and the local title used
        // for numeric-version extraction are queried. HardwareID and DeviceStatus are not read.
        let (class, provider, model, title, date) = unsafe {
            (
                driver.DriverClass().ok().map(|v| v.to_string()),
                driver.DriverProvider().ok().map(|v| v.to_string()),
                driver.DriverModel().ok().map(|v| v.to_string()),
                driver.Title().ok().map(|v| v.to_string()),
                driver.DriverVerDate().ok().and_then(ole_date_ms),
            )
        };
        let Some(class) = class.as_deref().and_then(driver_version::driver_class) else {
            partial = true;
            continue;
        };
        let provider = provider.as_deref().and_then(driver_version::display_text);
        let model = model.as_deref().and_then(driver_version::display_text);
        let version = title.as_deref().and_then(driver_version::title_version);
        let matched = match (&provider, &model) {
            (Some(provider), Some(model)) => {
                driver_version::unique_match((class, provider, model), &keys)
                    .map(|index| &installed[index])
            }
            _ => None,
        };
        partial |= provider.is_none()
            || model.is_none()
            || version.is_none()
            || date.is_none()
            || matched.is_none();
        let entity = out.entity("driver-offer", None);
        out.value(&entity, "offered_driver_class", class, "category", SOURCE);
        out.name_value(&entity, "offered_driver_provider_ref", provider, SOURCE);
        out.name_value(&entity, "offered_driver_model_ref", model, SOURCE);
        out.value(
            &entity,
            "offered_driver_version",
            version
                .map(driver_version::DriverVersion::text)
                .map(Value::from)
                .unwrap_or(Value::Null),
            "version",
            SOURCE,
        );
        out.value(
            &entity,
            "offered_driver_date_unix_ms",
            date.map(Value::from).unwrap_or(Value::Null),
            "unix_ms",
            SOURCE,
        );
        out.value(
            &entity,
            "matched_device_ref",
            matched
                .map(|driver| Value::from(driver.reference.clone()))
                .unwrap_or(Value::Null),
            "opaque_ref",
            SOURCE,
        );
        out.value(
            &entity,
            "offered_version_relation",
            driver_version::relation(matched.and_then(|driver| driver.version), version),
            "category",
            SOURCE,
        );
        offered += 1;
    }
    partial |= examined < total;
    out.value(
        "system",
        "windows_update_driver_offer_count",
        offered as u64,
        "count",
        SOURCE,
    );
    if partial {
        out.observations
            .last_mut()
            .expect("count just inserted")
            .status = C::Partial;
    }
    out.value(
        "system",
        "windows_update_driver_search_online",
        mode == WindowsUpdateMode::Online,
        "boolean",
        SOURCE,
    );
    out.coverage("windows_update_driver_offers", if partial || mode == WindowsUpdateMode::Offline { C::Partial } else { C::Ok }, SOURCE,
        if mode == WindowsUpdateMode::Offline { "Offline search reads the existing Windows Update cache; its refresh date is unknown. At most 200 offers are examined. Driver versions require an exact numeric title suffix; absent/ambiguous metadata or device matches remain unknown. An older offer is never an upgrade recommendation." }
        else { "Explicit online search can contact the configured update service (Microsoft or a managed server) from the dedicated worker. It never downloads or installs. Offer wait is bounded to seven seconds inside the 15-second inspection wait. Versions and unique device matching remain unknown when metadata is insufficient." });
    Some(cleanup)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automation_dates_preserve_utc_and_reject_invalid_values() {
        assert_eq!(ole_date_ms(25569.0), Some(0));
        assert_eq!(ole_date_ms(25569.5), Some(43_200_000));
        assert_eq!(ole_date_ms(f64::NAN), None);
        assert_eq!(ole_date_ms(f64::INFINITY), None);
        assert_eq!(ole_date_ms(1.0), None);
    }
}
