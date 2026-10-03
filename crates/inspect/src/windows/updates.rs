use super::com_error;
use crate::{CoverageStatus as C, Inspection};
use ::windows::Win32::System::{Com::*, UpdateAgent::*};
use serde_json::Value;

fn ole_date_ms(days: f64) -> Option<u64> {
    let millis = (days - 25_569.0) * 86_400_000.0;
    (millis.is_finite() && millis >= 0.0 && millis <= u64::MAX as f64)
        .then_some(millis.round() as u64)
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
