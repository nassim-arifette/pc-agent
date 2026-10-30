//! Fixed loopback transport, separate from Token Factory and all provider credentials.
use pcagent_agent::specialist::{Backend, Decision, Planned, BASE_MODEL, BASE_REVISION};
use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

pub const ENDPOINT: &str = "http://127.0.0.1:8737";
pub const ADAPTER_SHA256: &str = "b85a03348ebeb27a327bcf53c5b3da3bde931a34337650d0eccb9d5887dbd459";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Identity {
    pub protocol: u32,
    pub model: String,
    pub revision: String,
    pub adapter_sha256: String,
    pub device: String,
    pub precision: String,
    pub synthetic_training_only: bool,
}
impl Identity {
    fn valid(&self) -> bool {
        self.protocol == 1
            && self.model == BASE_MODEL
            && self.revision == BASE_REVISION
            && self.adapter_sha256 == ADAPTER_SHA256
            && self.synthetic_training_only
            && ["cpu", "cuda"].contains(&self.device.as_str())
    }
}

#[derive(Clone, Default)]
pub struct Local;
impl Local {
    fn request(path: &str, body: Option<Value>, timeout: Duration) -> Result<Value, &'static str> {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(1))
            .timeout(timeout)
            .build()
            .map_err(|_| "unavailable")?;
        let url = format!("{ENDPOINT}{path}");
        let request = match body {
            Some(body) => client.post(url).json(&body),
            None => client.get(url),
        };
        let response = request.send().map_err(|_| "unavailable")?;
        if !response.status().is_success() {
            return Err("unsupported_context");
        }
        let mut bytes = Vec::new();
        response
            .take(65_537)
            .read_to_end(&mut bytes)
            .map_err(|_| "unavailable")?;
        if bytes.len() > 65_536 {
            return Err("invalid_decision");
        }
        serde_json::from_slice(&bytes).map_err(|_| "invalid_decision")
    }

    pub fn health() -> Result<Identity, &'static str> {
        let identity: Identity =
            serde_json::from_value(Self::request("/health", None, Duration::from_secs(2))?)
                .map_err(|_| "identity_mismatch")?;
        if !identity.valid() {
            return Err("identity_mismatch");
        }
        Ok(identity)
    }
}
impl Backend for Local {
    fn decide(
        &mut self,
        state: &Value,
        timeout: Duration,
        stop: &Arc<AtomicBool>,
    ) -> Result<Planned, &'static str> {
        let started = Instant::now();
        let body = json!({"state":state,"variant":"adapter"});
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sender.send(Self::request("/decide", Some(body), timeout));
        });
        let value = loop {
            if stop.load(Ordering::SeqCst) {
                return Err("cancelled");
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err("timeout");
            }
            match receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(value) => break value?,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(_) => return Err("unavailable"),
            }
        };
        let identity: Identity =
            serde_json::from_value(value["identity"].clone()).map_err(|_| "identity_mismatch")?;
        if !identity.valid() || value["variant"] != "adapter" {
            return Err("identity_mismatch");
        }
        if value["hit_token_limit"] != false {
            return Err("invalid_decision");
        }
        let decision: Decision =
            serde_json::from_value(value["decision"].clone()).map_err(|_| "invalid_decision")?;
        Ok(Planned {
            decision,
            device: identity.device,
            input_tokens: value["input_tokens"]
                .as_u64()
                .filter(|n| *n <= 1792)
                .ok_or("invalid_decision")?,
            output_tokens: value["output_tokens"]
                .as_u64()
                .filter(|n| *n <= 256)
                .ok_or("invalid_decision")?,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_requires_the_exact_published_adapter_and_base() {
        let mut identity = Identity {
            protocol: 1,
            model: BASE_MODEL.into(),
            revision: BASE_REVISION.into(),
            adapter_sha256: ADAPTER_SHA256.into(),
            device: "cpu".into(),
            precision: "torch.float32".into(),
            synthetic_training_only: true,
        };
        assert!(identity.valid());
        identity.adapter_sha256 = "different".into();
        assert!(!identity.valid());
    }
}
