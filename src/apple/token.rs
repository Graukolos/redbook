use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use super::USER_AGENT;

const HOME: &str = "https://beta.music.apple.com";
const ASSET_PREFIX: &str = "/assets/index-legacy";
const BUNDLE_LIMIT: u64 = 32 * 1024 * 1024;
const EXPIRY_MARGIN: u64 = 300;

pub struct TokenStore {
    agent: ureq::Agent,
    path: Option<PathBuf>,
    cached: Mutex<Option<Token>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Token {
    token: String,
    expires_at: u64,
}

#[derive(Deserialize)]
struct Claims {
    exp: u64,
    #[serde(default)]
    origin: Option<String>,
}

impl Token {
    fn fresh(&self) -> bool {
        self.expires_at.saturating_sub(EXPIRY_MARGIN) > now()
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::new_with_defaults(),
            path: default_cache_path(),
            cached: Mutex::new(None),
        }
    }

    pub fn get(&self) -> Result<String> {
        let mut cached = self.cached.lock().expect("token cache poisoned");

        if let Some(token) = cached.as_ref().filter(|t| t.fresh()) {
            return Ok(token.token.clone());
        }

        if let Some(token) = self.load().filter(Token::fresh) {
            let secret = token.token.clone();
            *cached = Some(token);
            return Ok(secret);
        }

        let token = self.scrape()?;
        self.store(&token);
        let secret = token.token.clone();
        *cached = Some(token);
        Ok(secret)
    }

    pub fn invalidate(&self) {
        *self.cached.lock().expect("token cache poisoned") = None;
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }

    fn scrape(&self) -> Result<Token> {
        let html = self
            .fetch(HOME)
            .with_context(|| format!("apple: fetching {HOME}"))?;
        let asset = find_asset(&html)
            .ok_or_else(|| anyhow!("apple: no {ASSET_PREFIX} bundle referenced by {HOME}"))?;

        let url = format!("{HOME}{asset}");
        let bundle = self
            .fetch(&url)
            .with_context(|| format!("apple: fetching {url}"))?;

        jwts(&bundle)
            .find_map(usable)
            .ok_or_else(|| anyhow!("apple: no usable developer token in {asset}"))
    }

    fn fetch(&self, url: &str) -> Result<String> {
        let body = self
            .agent
            .get(url)
            .header("User-Agent", USER_AGENT)
            .call()?
            .body_mut()
            .with_config()
            .limit(BUNDLE_LIMIT)
            .read_to_string()?;
        Ok(body)
    }

    fn load(&self) -> Option<Token> {
        let path = self.path.as_ref()?;
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
    }

    fn store(&self, token: &Token) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(token) {
            let _ = fs::write(path, json);
        }
    }
}

fn find_asset(html: &str) -> Option<&str> {
    let start = html.find(ASSET_PREFIX)?;
    let tail = &html[start + ASSET_PREFIX.len()..];
    let hash = tail.find(".js").filter(|&end| {
        tail[..end]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'~' | b'-' | b'_'))
    })?;
    Some(&html[start..start + ASSET_PREFIX.len() + hash + ".js".len()])
}

fn jwts(js: &str) -> impl Iterator<Item = &str> {
    let mut from = 0;
    std::iter::from_fn(move || {
        loop {
            let start = from + js[from..].find("eyJ")?;
            let end = js[start..]
                .bytes()
                .position(|b| !is_token_byte(b))
                .map_or(js.len(), |len| start + len);
            from = end;

            let candidate = &js[start..end];
            let mut parts = candidate.split('.');
            if parts.clone().count() == 3 && parts.all(|part| !part.is_empty()) {
                return Some(candidate);
            }
        }
    })
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')
}

fn usable(jwt: &str) -> Option<Token> {
    let payload = jwt.split('.').nth(1)?;
    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    if claims.origin.is_some() || claims.exp <= now() {
        return None;
    }
    Some(Token {
        token: jwt.to_owned(),
        expires_at: claims.exp,
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn default_cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("redbook").join("apple-token.json"))
}
