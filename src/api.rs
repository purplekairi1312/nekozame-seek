//! slskd's API: the searches, their results, the downloads, and who's logged in.

use serde_json::{json, Value};
use std::time::Duration;

pub struct Slskd {
    url: String,
    key: String,
    agent: ureq::Agent,
}

impl Slskd {
    pub fn new(url: &str, key: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Slskd { url: url.trim_end_matches('/').to_string(), key: key.to_string(), agent }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        self.agent
            .get(&format!("{}/api/v0{}", self.url, path))
            .header("X-API-Key", &self.key)
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_json::<Value>()
            .map_err(|e| e.to_string())
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let mut r = self
            .agent
            .post(&format!("{}/api/v0{}", self.url, path))
            .header("X-API-Key", &self.key)
            .send_json(&body)
            .map_err(|e| e.to_string())?;
        let text = r.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// who you're logged in as and what you share, or why not
    pub fn status(&self) -> String {
        let app = match self.get("/application") {
            Ok(a) => a,
            Err(e) => return format!("slskd isn't answering at {} ({e})", self.url),
        };
        if app["server"]["isLoggedIn"].as_bool().unwrap_or(false) {
            format!(
                "logged in as {} · sharing {} files",
                app["user"]["username"].as_str().unwrap_or("?"),
                thousands(app["shares"]["files"].as_u64().unwrap_or(0))
            )
        } else {
            format!(
                "not logged in to Soulseek ({}): slskd needs your login",
                app["server"]["state"].as_str().unwrap_or("no login yet")
            )
        }
    }

    pub fn search(&self, text: &str) -> Result<String, String> {
        let r = self.post("/searches", json!({ "searchText": text }))?;
        r["id"].as_str().map(str::to_string).ok_or_else(|| "slskd didn't start the search".into())
    }

    pub fn search_state(&self, id: &str) -> Result<Value, String> {
        self.get(&format!("/searches/{id}"))
    }

    pub fn responses(&self, id: &str) -> Result<Vec<Value>, String> {
        Ok(self.get(&format!("/searches/{id}/responses"))?.as_array().cloned().unwrap_or_default())
    }

    /// the searches slskd keeps (made from anywhere), newest first
    pub fn searches(&self) -> Result<Vec<Value>, String> {
        let mut all = self.get("/searches")?.as_array().cloned().unwrap_or_default();
        all.sort_by(|a, b| b["startedAt"].as_str().cmp(&a["startedAt"].as_str()));
        Ok(all)
    }

    pub fn download(&self, user: &str, files: &[(String, u64)]) -> Result<(), String> {
        let body: Vec<Value> = files.iter().map(|(f, s)| json!({ "filename": f, "size": s })).collect();
        self.post(&format!("/transfers/downloads/{}", encode(user)), Value::Array(body)).map(|_| ())
    }

    /// slskd looks through the shared folders again
    pub fn rescan(&self) -> Result<(), String> {
        self.agent
            .put(&format!("{}/api/v0/shares", self.url))
            .header("X-API-Key", &self.key)
            .send_empty()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn downloads(&self) -> Result<Vec<Value>, String> {
        Ok(self.get("/transfers/downloads")?.as_array().cloned().unwrap_or_default())
    }
}

/// a username in a URL
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
