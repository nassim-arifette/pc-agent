//! The one daily task that runs `pcagent report --scheduled` while the app is closed.
//! Created for the current user only, at the lowest run level, from fixed arguments.
use crate::args::ScheduleAction;
use crate::Result;

#[cfg_attr(not(windows), allow(dead_code))]
pub(super) const TASK_NAME: &str = "PCAgent\\DailyReport";

pub(super) fn parse_time(value: &str) -> std::result::Result<(u8, u8), String> {
    let (h, m) = value
        .split_once(':')
        .ok_or("Use HH:MM, for example 09:00.")?;
    let (h, m): (u8, u8) = (
        h.parse().map_err(|_| "Use HH:MM, for example 09:00.")?,
        m.parse().map_err(|_| "Use HH:MM, for example 09:00.")?,
    );
    if h > 23 || m > 59 || value.len() != 5 {
        return Err("Use HH:MM, for example 09:00.".into());
    }
    Ok((h, m))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Task Scheduler definition: daily, catch up when missed, interactive session only, no
/// elevation, five-minute limit, one instance, no wake, no network requirement.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn task_xml(exe: &str, hour: u8, minute: u8, start_date: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>PC Agent daily local report. Reads this PC only; sends nothing.</Description></RegistrationInfo>
  <Triggers><CalendarTrigger><StartBoundary>{start_date}T{hour:02}:{minute:02}:00</StartBoundary><Enabled>true</Enabled><ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT5M</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>report --scheduled</Arguments></Exec></Actions>
</Task>
"#,
        escape(exe)
    )
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::process::Command;

    fn schtasks(args: &[&str]) -> Result<bool> {
        Ok(Command::new("schtasks.exe")
            .args(args)
            .output()?
            .status
            .success())
    }

    pub(super) fn exists() -> Result<bool> {
        schtasks(&["/Query", "/TN", TASK_NAME])
    }

    pub(super) fn enable(hour: u8, minute: u8) -> Result<()> {
        let exe = std::env::current_exe()?;
        let xml = task_xml(&exe.to_string_lossy(), hour, minute, "2026-01-01");
        let path = std::env::temp_dir().join(format!("pcagent-task-{}.xml", std::process::id()));
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&path, bytes)?;
        let created = schtasks(&[
            "/Create",
            "/F",
            "/TN",
            TASK_NAME,
            "/XML",
            &path.to_string_lossy(),
        ]);
        let _ = std::fs::remove_file(&path);
        if created? {
            Ok(())
        } else {
            Err("Windows refused to create the scheduled task.".into())
        }
    }

    pub(super) fn disable() -> Result<()> {
        if exists()? && !schtasks(&["/Delete", "/F", "/TN", TASK_NAME])? {
            return Err("Windows refused to delete the scheduled task.".into());
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod native {
    use super::*;
    const UNSUPPORTED: &str = "The daily task is available on Windows only.";
    pub(super) fn exists() -> Result<bool> {
        Ok(false)
    }
    pub(super) fn enable(_: u8, _: u8) -> Result<()> {
        Err(UNSUPPORTED.into())
    }
    pub(super) fn disable() -> Result<()> {
        Err(UNSUPPORTED.into())
    }
}

pub(super) fn run(action: ScheduleAction) -> Result<u8> {
    match action {
        ScheduleAction::Status => {
            println!(
                "Daily report task: {}",
                if native::exists()? {
                    "enabled"
                } else {
                    "not installed"
                }
            );
        }
        ScheduleAction::Enable { time } => {
            let (hour, minute) = parse_time(&time)?;
            native::enable(hour, minute)?;
            println!("Daily report scheduled at {hour:02}:{minute:02}. Remove it with `schedule disable`.");
        }
        ScheduleAction::Disable => {
            native::disable()?;
            println!("Daily report task removed.");
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_must_be_a_clock_time() {
        assert_eq!(parse_time("09:00"), Ok((9, 0)));
        assert_eq!(parse_time("23:59"), Ok((23, 59)));
        for bad in ["9:00", "24:00", "12:60", "noon", "", "09:0"] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn definition_is_unelevated_bounded_and_escaped() {
        let xml = task_xml(r"C:\Program Files\A&B\pcagent.exe", 9, 5, "2026-01-01");
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT5M</ExecutionTimeLimit>"));
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
        assert!(xml.contains("<WakeToRun>false</WakeToRun>"));
        assert!(xml.contains("2026-01-01T09:05:00"));
        assert!(xml.contains("A&amp;B"));
        assert!(xml.contains("<Arguments>report --scheduled</Arguments>"));
    }
}
