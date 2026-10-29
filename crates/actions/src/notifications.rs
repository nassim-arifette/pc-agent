//! Local-only Windows notifications. No model, network, arbitrary URI or shell argument.
use serde::Serialize;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub const APP_ID: &str = "dev.pcagent.desktop";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Sent,
    Enabled,
    Disabled,
    Failed,
    Unsupported,
}

#[derive(Clone, Debug)]
pub enum Event {
    Activated(String),
    Failed(String),
}

pub type Callback = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Clone)]
pub struct Notification {
    pub alert_id: String,
    pub watch_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone)]
pub struct Service {
    sender: mpsc::SyncSender<Request>,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Operation {
    Show(Notification),
    Remove(String),
    Clear,
    Status,
}
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct Request {
    pub operation: Operation,
    pub at: Instant,
    pub reply: mpsc::SyncSender<Delivery>,
}

impl Service {
    pub fn start(callback: Callback) -> Self {
        // A bounded queue and bounded native notification handles, including test clicks.
        let (sender, receiver) = mpsc::sync_channel(16);
        #[cfg(windows)]
        let _ = std::thread::Builder::new()
            .name("watch-notifications".into())
            .spawn(move || crate::windows::notifications::run(receiver, callback));
        #[cfg(not(windows))]
        let _ = (receiver, callback);
        Self { sender }
    }

    pub fn show(&self, notification: Notification) -> Delivery {
        if !valid_id(&notification.alert_id)
            || !valid_id(&notification.watch_id)
            || notification.title.len() > 512
            || notification.body.len() > 2048
        {
            return Delivery::Failed;
        }
        self.request(Operation::Show(notification))
    }

    pub fn remove_watch(&self, watch_id: &str) -> Delivery {
        if !valid_id(watch_id) {
            return Delivery::Failed;
        }
        self.request(Operation::Remove(watch_id.into()))
    }

    pub fn clear(&self) -> Delivery {
        self.request(Operation::Clear)
    }

    pub fn status(&self) -> Delivery {
        self.request(Operation::Status)
    }

    fn request(&self, operation: Operation) -> Delivery {
        if !cfg!(windows) {
            return Delivery::Unsupported;
        }
        let (reply, answer) = mpsc::sync_channel(1);
        if self
            .sender
            .try_send(Request {
                operation,
                at: Instant::now(),
                reply,
            })
            .is_err()
        {
            return Delivery::Failed;
        }
        answer
            .recv_timeout(REQUEST_TIMEOUT)
            .unwrap_or(Delivery::Failed)
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn xml(notification: &Notification) -> String {
    fn escape(text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .flat_map(|c| match c {
                '&' => "&amp;".chars().collect::<Vec<_>>(),
                '<' => "&lt;".chars().collect(),
                '>' => "&gt;".chars().collect(),
                '"' => "&quot;".chars().collect(),
                '\'' => "&apos;".chars().collect(),
                c => vec![c],
            })
            .collect()
    }
    format!(
        "<toast launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        escape(&notification.alert_id), escape(&notification.title), escape(&notification.body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_never_accepts_paths_commands_or_urls() {
        for bad in [
            "",
            "C:\\folder",
            "https://example.org",
            "x y",
            "x\" y",
            "watch\nid",
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert!(!valid_id(&"a".repeat(65)));
        assert!(valid_id("watch-alert-132b86e0-e89c-4bd0-88fa-a2ba784d8827"));
    }

    #[test]
    fn toast_text_cannot_inject_xml_actions() {
        let n = Notification {
            alert_id: "test".into(),
            watch_id: "test".into(),
            title: "<actions/>".into(),
            body: "x & \"y\"\0".into(),
        };
        let xml = xml(&n);
        assert!(xml.contains("&lt;actions/&gt;"));
        assert!(xml.contains("x &amp; &quot;y&quot;"));
        assert!(!xml.contains("<actions") && !xml.contains('\0'));
    }
}
