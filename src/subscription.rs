//! Subscription providers sign in with this machine's CLI login, not an API
//! key: `provider-openai-codex` reads `${CODEX_HOME:-~/.codex}/auth.json`
//! and `provider-claude-code` `${CLAUDE_CONFIG_DIR:-~/.claude}/.credentials.json`.
//!
//! A group started in Docker gets the access token alone. When that token
//! would not outlast the group, this host refreshes the login first and
//! writes the rotated tokens back where the CLI keeps them, so the CLI goes on
//! working with them: refresh tokens rotate on every use, so only one holder
//! may refresh. No refresh or id token ever leaves the file.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// A provider that signs in with a subscription login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subscription {
    /// `openai-codex`: the `codex` CLI's ChatGPT login.
    Codex,
    /// `claude-code`: the `claude` CLI's Claude Pro/Max login.
    ClaudeCode,
}

/// What a group signs in with.
#[derive(Clone, PartialEq, Eq)]
pub struct Access {
    /// The access token and the variable it goes in: a credential.
    pub token: (String, String),
    /// What else the provider's login holds, not secret: `CODEX_ACCOUNT_ID`,
    /// `CLAUDE_CODE_EXPIRES_AT` (epoch milliseconds).
    pub env: Vec<(String, String)>,
    /// When the token expires, epoch seconds, when the login says.
    pub expires_at: Option<i64>,
    /// Why the token may not last the group, when it may not.
    pub warning: Option<String>,
}

/// Never with the token's value.
impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access")
            .field("token", &(&self.token.0, "[redacted]"))
            .field("env", &self.env)
            .field("expires_at", &self.expires_at)
            .field("warning", &self.warning)
            .finish()
    }
}

impl Access {
    /// Whether it lasts `budget` from now.
    pub fn covers(&self, budget: Duration) -> bool {
        self.expires_at
            .is_none_or(|expires_at| expires_at - now() >= budget.as_secs() as i64)
    }
}

/// The refresh endpoints and client ids the CLIs use (`provider-openai-codex`
/// refreshes the same way; the `claude` CLI names Claude's in its OAuth
/// configuration, and a login may name its own client).
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// The ChatGPT account id claim of a Codex access token.
const CODEX_AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// What a sign-in service answers when the refresh token is no longer good.
const REJECTED: &[&str] = &[
    "invalid_grant",
    "refresh_token_reused",
    "refresh_token_revoked",
    "refresh_token_expired",
    "token_revoked",
    "revoked",
    "expired_token",
];

/// A token closer than this to its expiry is dead on arrival.
const MARGIN_SECONDS: i64 = 60;

/// The Claude CLI's refresh lock (proper-lockfile): a directory whose mtime
/// its holder renews; one older than this is abandoned.
const CLAUDE_LOCK_STALE: Duration = Duration::from_secs(60);
const CLAUDE_LOCK_RENEW: Duration = Duration::from_secs(5);

/// One refresh at a time per login in this worker (its parallel groups);
/// another process waits on the login's lock file.
static CODEX: Mutex<()> = Mutex::const_new(());
static CLAUDE_CODE: Mutex<()> = Mutex::const_new(());

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Subscription {
    /// The subscription a model's provider signs in with; `None` for the
    /// providers that take an API key.
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            "openai-codex" => Some(Self::Codex),
            "claude-code" => Some(Self::ClaudeCode),
            _ => None,
        }
    }

    pub fn provider(self) -> &'static str {
        match self {
            Self::Codex => "openai-codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    /// The access token a group needs to run for `budget`, refreshed first
    /// only when it expires within `budget`. The login is read below `home`
    /// when given, else where the CLI keeps it (`CODEX_HOME`,
    /// `CLAUDE_CONFIG_DIR`, `HOME`). A refresh that fails while the token
    /// still works hands that token over with a warning.
    pub async fn access(self, home: Option<&Path>, budget: Duration) -> Result<Access> {
        let file = self.login_file(home)?;
        let url = match self {
            Self::Codex => CODEX_TOKEN_URL,
            Self::ClaudeCode => CLAUDE_TOKEN_URL,
        };
        self.access_at(&file, url, budget).await
    }

    /// Where the CLI keeps its login.
    fn login_file(self, home: Option<&Path>) -> Result<PathBuf> {
        let (variable, folder, name) = match self {
            Self::Codex => ("CODEX_HOME", ".codex", "auth.json"),
            Self::ClaudeCode => ("CLAUDE_CONFIG_DIR", ".claude", ".credentials.json"),
        };
        let folder = match (home, std::env::var_os(variable).filter(|v| !v.is_empty())) {
            (Some(home), _) => home.join(folder),
            (None, Some(folder)) => PathBuf::from(folder),
            (None, None) => {
                PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(folder)
            }
        };
        Ok(folder.join(name))
    }

    async fn access_at(self, file: &Path, url: &str, budget: Duration) -> Result<Access> {
        ensure!(
            file.is_file(),
            "there is no {} login on this machine ({}); {}",
            self.name(),
            file.display(),
            self.sign_in()
        );
        let _turn = match self {
            Self::Codex => &CODEX,
            Self::ClaudeCode => &CLAUDE_CODE,
        }
        .lock()
        .await;
        let _lock = flock(file).await?;
        let current = self.current(&read_login(file)?)?;
        if current.covers(budget) {
            return Ok(current);
        }
        let refreshed = async {
            // The Claude CLI refreshes under a lock of its own, which it
            // honours here too.
            // ponytail: the codex CLI takes no lock for its auth.json refresh,
            // so a refresh racing it can lose one side's rotation; the
            // compare-and-swap and the retry below recover when the CLI won.
            let _cli = match self {
                Self::ClaudeCode => {
                    Some(ClaudeLock::acquire(file.parent().context("login folder")?).await?)
                }
                Self::Codex => None,
            };
            self.refresh_login(file, url, budget).await
        }
        .await;
        match refreshed {
            Ok(access) => Ok(access),
            // Warn, never restrict: a token that still works is handed over.
            Err(error) => {
                let mut current = self.current(&read_login(file)?)?;
                if !current
                    .expires_at
                    .is_some_and(|expires_at| expires_at > now() + MARGIN_SECONDS)
                {
                    return Err(error);
                }
                current.warning = Some(format!(
                    "the {} login could not be refreshed ({error:#}); provider-{} starts with a token that expires at {}, which may be before the group ends",
                    self.name(),
                    self.provider(),
                    chrono::DateTime::from_timestamp(current.expires_at.unwrap_or_default(), 0)
                        .map_or_else(String::new, |at| at.to_rfc3339())
                ));
                Ok(current)
            }
        }
    }

    /// Refresh the login, as its CLI would: re-read under the lock, and
    /// write the new tokens only over the login they were refreshed from.
    async fn refresh_login(self, file: &Path, url: &str, budget: Duration) -> Result<Access> {
        // The refresh token the sign-in service refused: tried once more only
        // if the CLI rotated it meanwhile.
        let mut refused: Option<String> = None;
        for _ in 0..3 {
            let login = read_login(file)?;
            let current = self.current(&login)?;
            if current.covers(budget) {
                // The CLI refreshed meanwhile.
                return Ok(current);
            }
            let refresh_token = self
                .refresh_token(&login)
                .ok_or_else(|| anyhow!(self.expired()))?;
            if refused.as_deref() == Some(refresh_token.as_str()) {
                bail!(self.expired());
            }
            // Before the refresh token is spent: a login that cannot be
            // written fails while it is still good.
            let folder = file.parent().context("the login file has no folder")?;
            let temporary = tempfile::NamedTempFile::new_in(folder)
                .with_context(|| format!("write beside {}", file.display()))?;
            match self.refresh(url, &refresh_token, &login).await? {
                None => refused = Some(refresh_token),
                Some(tokens) => {
                    let mut fresh = read_login(file)?;
                    if self.refresh_token(&fresh).as_deref() != Some(refresh_token.as_str()) {
                        // Rotated by someone else meanwhile: theirs stands.
                        continue;
                    }
                    self.apply(&mut fresh, &tokens)?;
                    self.write(temporary, file, &fresh)?;
                    return self.current(&fresh);
                }
            }
        }
        bail!(self.expired())
    }

    fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude",
        }
    }

    fn sign_in(self) -> &'static str {
        match self {
            Self::Codex => "run `codex login`",
            Self::ClaudeCode => "run `claude`",
        }
    }

    fn expired(self) -> String {
        format!(
            "the {} login on this machine expired; {}",
            self.name(),
            self.sign_in()
        )
    }

    /// The login's access token as a group receives it.
    fn current(self, login: &Value) -> Result<Access> {
        let token = |value: &Value| {
            value
                .as_str()
                .filter(|token| !token.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    anyhow!(
                        "the {} login on this machine holds no access token; {}",
                        self.name(),
                        self.sign_in()
                    )
                })
        };
        Ok(match self {
            Self::Codex => {
                ensure!(
                    login["auth_mode"] == "chatgpt",
                    "the Codex login on this machine is not a ChatGPT login; run `codex login`"
                );
                let access = token(&login["tokens"]["access_token"])?;
                let claims = jwt_claims(&access);
                let account = login["tokens"]["account_id"]
                    .as_str()
                    .or_else(|| claims[CODEX_AUTH_CLAIM]["chatgpt_account_id"].as_str())
                    .map(|account| ("CODEX_ACCOUNT_ID".to_owned(), account.to_owned()));
                Access {
                    token: ("CODEX_ACCESS_TOKEN".to_owned(), access),
                    env: account.into_iter().collect(),
                    expires_at: claims["exp"].as_i64(),
                    warning: None,
                }
            }
            Self::ClaudeCode => {
                let oauth = &login["claudeAiOauth"];
                let access = token(&oauth["accessToken"])?;
                let expires_at = oauth["expiresAt"].as_i64();
                Access {
                    token: ("CLAUDE_CODE_ACCESS_TOKEN".to_owned(), access),
                    env: expires_at
                        .map(|ms| ("CLAUDE_CODE_EXPIRES_AT".to_owned(), ms.to_string()))
                        .into_iter()
                        .collect(),
                    expires_at: expires_at.map(|ms| ms / 1000),
                    warning: None,
                }
            }
        })
    }

    fn refresh_token(self, login: &Value) -> Option<String> {
        match self {
            Self::Codex => &login["tokens"]["refresh_token"],
            Self::ClaudeCode => &login["claudeAiOauth"]["refreshToken"],
        }
        .as_str()
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
    }

    /// The sign-in service's new tokens, or `None` when it refused the
    /// refresh token. Anything else leaves the login as it was.
    async fn refresh(self, url: &str, refresh_token: &str, login: &Value) -> Result<Option<Value>> {
        let oauth = &login["claudeAiOauth"];
        let mut body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": match self {
                Self::Codex => CODEX_CLIENT_ID,
                Self::ClaudeCode => oauth["clientId"].as_str().unwrap_or(CLAUDE_CLIENT_ID),
            },
        });
        // Claude's CLI asks for the scopes its login holds.
        if let Some(scopes) = oauth["scopes"].as_array() {
            let scopes = scopes.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            if self == Self::ClaudeCode && !scopes.is_empty() {
                body["scope"] = json!(scopes.join(" "));
            }
        }
        let unchanged = format!("the {} login is unchanged", self.name());
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("harness-e2e/", env!("CARGO_PKG_VERSION")))
            .build()?
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                anyhow!(
                    "could not reach the {} sign-in service ({}); {unchanged}",
                    self.name(),
                    error.without_url()
                )
            })?;
        let status = response.status();
        // The status alone: an error's description may echo what was sent.
        if status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error() {
            bail!(
                "the {} sign-in service answered {status}; {unchanged}",
                self.name()
            );
        }
        let answer = response.json::<Value>().await.unwrap_or(Value::Null);
        if status.is_success() {
            // A login is only ever replaced by a whole one.
            let complete = answer["access_token"]
                .as_str()
                .is_some_and(|token| !token.is_empty())
                && (self == Self::Codex || answer["expires_in"].as_i64().is_some_and(|s| s > 0));
            ensure!(
                complete,
                "the {} sign-in service answered {status} without a new access token and its lifetime; {unchanged}",
                self.name()
            );
            return Ok(Some(answer));
        }
        let code = answer["error"]
            .as_str()
            .or_else(|| answer["error"]["code"].as_str())
            .or_else(|| answer["error"]["type"].as_str())
            .or_else(|| answer["code"].as_str())
            .unwrap_or_default();
        if REJECTED.contains(&code) {
            return Ok(None);
        }
        bail!(
            "the {} sign-in service refused the refresh ({status}); {unchanged}",
            self.name()
        )
    }

    /// Put the new tokens in the login, keeping everything else it holds.
    fn apply(self, login: &mut Value, tokens: &Value) -> Result<()> {
        let token = |name: &str| {
            tokens[name]
                .as_str()
                .filter(|token| !token.is_empty())
                .map(|token| json!(token))
        };
        let now = chrono::Utc::now();
        match self {
            Self::Codex => {
                let saved = login["tokens"]
                    .as_object_mut()
                    .context("the Codex login has no tokens")?;
                for name in ["access_token", "refresh_token", "id_token"] {
                    if let Some(value) = token(name) {
                        saved.insert(name.to_owned(), value);
                    }
                }
                login["last_refresh"] =
                    json!(now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true));
            }
            Self::ClaudeCode => {
                let saved = login["claudeAiOauth"]
                    .as_object_mut()
                    .context("the Claude login has no claudeAiOauth")?;
                let now = now.timestamp_millis();
                for (name, field) in [
                    ("access_token", "accessToken"),
                    ("refresh_token", "refreshToken"),
                ] {
                    if let Some(value) = token(name) {
                        saved.insert(field.to_owned(), value);
                    }
                }
                for (seconds, field) in [
                    ("expires_in", "expiresAt"),
                    ("refresh_token_expires_in", "refreshTokenExpiresAt"),
                ] {
                    if let Some(seconds) = tokens[seconds].as_i64() {
                        saved.insert(field.into(), json!(now + seconds * 1000));
                    }
                }
            }
        }
        Ok(())
    }

    /// Replace the login file at once, as its CLI writes it: Codex's pretty,
    /// Claude's compact, neither with a trailing newline; mode 600 (the
    /// temporary file's). Keys come back in sorted order.
    fn write(
        self,
        mut temporary: tempfile::NamedTempFile,
        file: &Path,
        login: &Value,
    ) -> Result<()> {
        let bytes = match self {
            Self::Codex => serde_json::to_vec_pretty(login)?,
            Self::ClaudeCode => serde_json::to_vec(login)?,
        };
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(file)
            .with_context(|| format!("replace {}", file.display()))?;
        Ok(())
    }
}

fn read_login(file: &Path) -> Result<Value> {
    let bytes = fs::read(file).with_context(|| format!("read {}", file.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{} is not JSON", file.display()))
}

/// Hold `<login>.harness-e2e.lock` until the returned file is dropped.
async fn flock(file: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut name = file.file_name().context("login file name")?.to_owned();
    name.push(".harness-e2e.lock");
    let path = file.with_file_name(name);
    tokio::task::spawn_blocking(move || {
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        lock.lock()
            .with_context(|| format!("lock {}", path.display()))?;
        Ok(lock)
    })
    .await?
}

/// The Claude CLI's refresh lock, held as the CLI holds it (proper-lockfile):
/// the directories `<config>/.oauth_refresh.lock` and the legacy
/// `<config>.lock`, their mtime renewed while held, removed when dropped.
struct ClaudeLock {
    held: Vec<PathBuf>,
    renew: tokio::task::JoinHandle<()>,
}

impl ClaudeLock {
    async fn acquire(config: &Path) -> Result<Self> {
        let config = config.canonicalize().unwrap_or_else(|_| config.to_owned());
        let mut legacy = config.clone().into_os_string();
        legacy.push(".lock");
        let mut held = Vec::new();
        for path in [config.join(".oauth_refresh.lock"), PathBuf::from(legacy)] {
            let mut tries = 0;
            loop {
                match fs::create_dir(&path) {
                    Ok(()) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let age = fs::metadata(&path)
                            .and_then(|metadata| metadata.modified())
                            .ok()
                            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
                        if age.is_some_and(|age| age > CLAUDE_LOCK_STALE) {
                            let _ = fs::remove_dir(&path);
                            continue;
                        }
                        tries += 1;
                        if tries >= 30 {
                            release(&held);
                            bail!("the claude CLI is refreshing its login ({} is held); the Claude login is unchanged", path.display());
                        }
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                    }
                    Err(error) => {
                        release(&held);
                        return Err(error).with_context(|| format!("create {}", path.display()));
                    }
                }
            }
            held.push(path);
        }
        let renewed = held.clone();
        let renew = tokio::spawn(async move {
            loop {
                tokio::time::sleep(CLAUDE_LOCK_RENEW).await;
                for path in &renewed {
                    if let Ok(directory) = fs::File::open(path) {
                        let _ = directory.set_modified(SystemTime::now());
                    }
                }
            }
        });
        Ok(Self { held, renew })
    }
}

impl Drop for ClaudeLock {
    fn drop(&mut self) {
        self.renew.abort();
        release(&self.held);
    }
}

fn release(held: &[PathBuf]) {
    for path in held.iter().rev() {
        let _ = fs::remove_dir(path);
    }
}

/// The claims of a JWT, unverified; `Null` for anything else.
fn jwt_claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|payload| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload.trim_end_matches('='))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex as StdMutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// What a group runs for at most: its timeout and fifteen minutes.
    const BUDGET: Duration = Duration::from_secs(10800 + 900);

    fn jwt(claims: Value) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("eyJhbGciOiJub25lIn0.{payload}.signature")
    }

    fn in_hours(hours: i64) -> i64 {
        now() + hours * 3600
    }

    fn codex_token(hours: i64) -> String {
        jwt(json!({"exp": in_hours(hours), CODEX_AUTH_CLAIM: {"chatgpt_account_id": "acct-1"}}))
    }

    /// A Codex login as the CLI leaves it, with fields no one here knows.
    fn codex_login(folder: &Path, access: &str, refresh: &str) -> PathBuf {
        let file = folder.join("auth.json");
        let login = json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {"id_token": "id-token-1", "access_token": access,
                "refresh_token": refresh, "account_id": "acct-1", "later": {"kept": true}},
            "last_refresh": "2026-09-01T00:00:00.000000001Z",
        });
        fs::write(&file, serde_json::to_vec_pretty(&login).unwrap()).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        file
    }

    fn claude_login(folder: &Path, access: &str, refresh: &str, expires_at: i64) -> PathBuf {
        let file = folder.join(".credentials.json");
        let login = json!({
            "claudeAiOauth": {"accessToken": access, "refreshToken": refresh,
                "expiresAt": expires_at, "refreshTokenExpiresAt": 1_900_000_000_000_i64,
                "scopes": ["user:inference", "user:profile"], "subscriptionType": "max",
                "rateLimitTier": "default_claude_max_20x"},
            "mcpOAuth": {"server|1": {"serverName": "server", "accessToken": ""}},
        });
        fs::write(&file, serde_json::to_vec(&login).unwrap()).unwrap();
        file
    }

    /// A sign-in service on 127.0.0.1: each request gets the next answer, and
    /// `meanwhile` runs with each request before it is answered.
    async fn service(
        answers: Vec<(u16, Value)>,
        meanwhile: impl Fn(&Value) + Send + 'static,
    ) -> (String, Arc<StdMutex<Vec<Value>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/oauth/token", listener.local_addr().unwrap());
        let received = Arc::new(StdMutex::new(Vec::new()));
        let seen = received.clone();
        tokio::spawn(async move {
            for (status, answer) in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                let body = loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break body.to_owned();
                        }
                    }
                };
                let body: Value = serde_json::from_str(&body).unwrap();
                meanwhile(&body);
                seen.lock().unwrap().push(body);
                let answer = answer.to_string();
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (url, received)
    }

    /// No service listens here: a test that reaches it fails.
    const NOWHERE: &str = "http://127.0.0.1:9/oauth/token";

    fn requests(received: &StdMutex<Vec<Value>>) -> Vec<String> {
        received
            .lock()
            .unwrap()
            .iter()
            .map(|request| request["refresh_token"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_token_that_outlasts_the_group_is_handed_over_without_a_refresh() {
        let home = tempfile::tempdir().unwrap();
        let access = codex_token(240);
        let file = codex_login(home.path(), &access, "refresh-1");
        let before = fs::read(&file).unwrap();
        let got = Subscription::Codex
            .access_at(&file, NOWHERE, BUDGET)
            .await
            .unwrap();
        assert_eq!(got.token, ("CODEX_ACCESS_TOKEN".to_owned(), access));
        assert_eq!(
            got.env,
            [("CODEX_ACCOUNT_ID".to_owned(), "acct-1".to_owned())]
        );
        assert_eq!((got.expires_at, got.warning), (Some(in_hours(240)), None));
        assert_eq!(fs::read(&file).unwrap(), before);

        let expires_at = in_hours(5) * 1000;
        let file = claude_login(home.path(), "sk-ant-oat01-access", "refresh-1", expires_at);
        let before = fs::read(&file).unwrap();
        let got = Subscription::ClaudeCode
            .access_at(&file, NOWHERE, BUDGET)
            .await
            .unwrap();
        assert_eq!(
            (got.token.0.as_str(), got.token.1.as_str()),
            ("CLAUDE_CODE_ACCESS_TOKEN", "sk-ant-oat01-access")
        );
        assert_eq!(
            got.env,
            [("CLAUDE_CODE_EXPIRES_AT".to_owned(), expires_at.to_string())]
        );
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[tokio::test]
    async fn a_codex_token_that_would_expire_is_refreshed_and_only_its_tokens_change() {
        let home = tempfile::tempdir().unwrap();
        let file = codex_login(home.path(), &codex_token(1), "refresh-1");
        let fresh = codex_token(240);
        let (url, received) = service(
            vec![(
                200,
                json!({"access_token": fresh, "refresh_token": "refresh-2", "id_token": "id-token-2"}),
            )],
            |_| {},
        )
        .await;
        let got = Subscription::Codex
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(got.token.1, fresh);
        // The account alone besides it; its value never printed.
        assert_eq!(
            got.env,
            [("CODEX_ACCOUNT_ID".to_owned(), "acct-1".to_owned())]
        );
        assert!(!format!("{got:?}").contains(&fresh));
        assert_eq!(
            *received.lock().unwrap(),
            [
                json!({"grant_type": "refresh_token", "client_id": CODEX_CLIENT_ID, "refresh_token": "refresh-1"})
            ]
        );
        let text = fs::read_to_string(&file).unwrap();
        // Pretty, as the CLI writes it, without a trailing newline.
        assert!(text.starts_with("{\n  \"") && !text.ends_with('\n'));
        let mut login: Value = serde_json::from_str(&text).unwrap();
        let refreshed = login["last_refresh"].take();
        let refreshed = refreshed.as_str().unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(refreshed).is_ok()
                && refreshed.ends_with('Z')
                && refreshed.split('.').nth(1).unwrap().len() == 10,
            "{refreshed}"
        );
        assert_eq!(
            login,
            json!({
                "auth_mode": "chatgpt",
                "OPENAI_API_KEY": null,
                "tokens": {"id_token": "id-token-2", "access_token": fresh,
                    "refresh_token": "refresh-2", "account_id": "acct-1", "later": {"kept": true}},
                "last_refresh": null,
            })
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Only the login and its lock are left in the folder.
        let mut left = fs::read_dir(home.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        left.sort();
        assert_eq!(left, ["auth.json", "auth.json.harness-e2e.lock"]);
    }

    #[tokio::test]
    async fn a_claude_refresh_takes_the_clis_lock_and_changes_only_its_tokens() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".claude");
        fs::create_dir(&home).unwrap();
        let file = claude_login(&home, "old-access", "refresh-1", in_hours(1) * 1000);
        // The CLI's lock is held while the service answers.
        let (lock, legacy) = (
            home.join(".oauth_refresh.lock"),
            root.path().canonicalize().unwrap().join(".claude.lock"),
        );
        let (seen_lock, seen_legacy) = (lock.clone(), legacy.clone());
        let (url, received) = service(
            vec![(
                200,
                json!({"access_token": "new-access", "refresh_token": "refresh-2", "expires_in": 28800}),
            )],
            move |_| assert!(seen_lock.is_dir() && seen_legacy.is_dir()),
        )
        .await;
        let got = Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert!(
            !lock.exists() && !legacy.exists(),
            "the CLI's lock is released"
        );
        assert_eq!(
            received.lock().unwrap()[0],
            json!({"grant_type": "refresh_token", "client_id": CLAUDE_CLIENT_ID,
                "refresh_token": "refresh-1", "scope": "user:inference user:profile"})
        );
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains(['\n', ' ']), "compact, as the CLI writes it");
        let mut login: Value = serde_json::from_str(&text).unwrap();
        let expires_at = login["claudeAiOauth"]["expiresAt"].take().as_i64().unwrap();
        assert!((expires_at / 1000 - in_hours(8)).abs() < 60);
        assert_eq!(got.token.1, "new-access");
        assert_eq!(
            got.env,
            [("CLAUDE_CODE_EXPIRES_AT".to_owned(), expires_at.to_string())]
        );
        assert_eq!(
            login,
            json!({
                "claudeAiOauth": {"accessToken": "new-access", "refreshToken": "refresh-2",
                    "expiresAt": null, "refreshTokenExpiresAt": 1_900_000_000_000_i64,
                    "scopes": ["user:inference", "user:profile"], "subscriptionType": "max",
                    "rateLimitTier": "default_claude_max_20x"},
                "mcpOAuth": {"server|1": {"serverName": "server", "accessToken": ""}},
            })
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn a_login_names_its_own_client() {
        let home = tempfile::tempdir().unwrap();
        let file = claude_login(home.path(), "old-access", "refresh-1", in_hours(1) * 1000);
        let mut login = read_login(&file).unwrap();
        login["claudeAiOauth"]["clientId"] = json!("other-client");
        fs::write(&file, login.to_string()).unwrap();
        let (url, received) = service(
            vec![(
                200,
                json!({"access_token": "new-access", "refresh_token": "refresh-2", "expires_in": 28800}),
            )],
            |_| {},
        )
        .await;
        Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(received.lock().unwrap()[0]["client_id"], "other-client");
    }

    #[tokio::test]
    async fn what_the_cli_wrote_meanwhile_is_kept_and_its_own_rotation_wins() {
        // It changed another field while the refresh was out: kept.
        let home = tempfile::tempdir().unwrap();
        let file = claude_login(home.path(), "old-access", "refresh-1", in_hours(1) * 1000);
        let changed = file.clone();
        let (url, _) = service(
            vec![(
                200,
                json!({"access_token": "new-access", "refresh_token": "refresh-2", "expires_in": 28800}),
            )],
            move |_| {
                let mut login = read_login(&changed).unwrap();
                login["mcpOAuth"]["server|1"]["accessToken"] = json!("mcp-rotated");
                fs::write(&changed, login.to_string()).unwrap();
            },
        )
        .await;
        let got = Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        let login = read_login(&file).unwrap();
        assert_eq!(got.token.1, "new-access");
        assert_eq!(login["claudeAiOauth"]["refreshToken"], "refresh-2");
        assert_eq!(login["mcpOAuth"]["server|1"]["accessToken"], "mcp-rotated");

        // It rotated the login itself meanwhile: its tokens stand, ours are
        // not written over them.
        let file = claude_login(home.path(), "old-access", "refresh-1", in_hours(1) * 1000);
        let rotated = file.clone();
        let (url, received) = service(
            vec![(
                200,
                json!({"access_token": "new-access", "refresh_token": "refresh-2", "expires_in": 28800}),
            )],
            move |_| {
                claude_login(
                    rotated.parent().unwrap(),
                    "cli-access",
                    "refresh-cli",
                    in_hours(8) * 1000,
                );
            },
        )
        .await;
        let got = Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(got.token.1, "cli-access");
        assert_eq!(requests(&received), ["refresh-1"]);
        let login = read_login(&file).unwrap();
        assert_eq!(
            (
                &login["claudeAiOauth"]["accessToken"],
                &login["claudeAiOauth"]["refreshToken"]
            ),
            (&json!("cli-access"), &json!("refresh-cli"))
        );
    }

    #[tokio::test]
    async fn a_refused_refresh_is_tried_again_with_the_token_the_cli_rotated_meanwhile() {
        let home = tempfile::tempdir().unwrap();
        let file = claude_login(home.path(), "old-access", "refresh-1", in_hours(1) * 1000);
        let rotated = file.clone();
        let (url, received) = service(
            vec![
                (400, json!({"error": "invalid_grant"})),
                (
                    200,
                    json!({"access_token": "new-access", "refresh_token": "refresh-3", "expires_in": 28800}),
                ),
            ],
            // The CLI refreshed first, its token no longer lasting a group:
            // the service no longer takes refresh-1.
            move |request| {
                if request["refresh_token"] == "refresh-1" {
                    claude_login(
                        rotated.parent().unwrap(),
                        "cli-access",
                        "refresh-2",
                        in_hours(1) * 1000,
                    );
                }
            },
        )
        .await;
        let got = Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(got.token.1, "new-access");
        assert_eq!(requests(&received), ["refresh-1", "refresh-2"]);
        assert_eq!(
            read_login(&file).unwrap()["claudeAiOauth"]["refreshToken"],
            "refresh-3"
        );
    }

    #[tokio::test]
    async fn two_groups_at_once_refresh_the_login_once() {
        let home = tempfile::tempdir().unwrap();
        let file = codex_login(home.path(), &codex_token(1), "refresh-1");
        let fresh = codex_token(240);
        let (url, received) = service(
            vec![(
                200,
                json!({"access_token": fresh, "refresh_token": "refresh-2", "id_token": "id-token-2"}),
            )],
            |_| {},
        )
        .await;
        let (first, second) = tokio::join!(
            Subscription::Codex.access_at(&file, &url, BUDGET),
            Subscription::Codex.access_at(&file, &url, BUDGET),
        );
        assert_eq!(first.unwrap().token.1, fresh);
        assert_eq!(second.unwrap().token.1, fresh);
        assert_eq!(requests(&received), ["refresh-1"]);
    }

    #[tokio::test]
    async fn a_success_without_a_whole_login_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        for (subscription, answer) in [
            (Subscription::Codex, json!({"refresh_token": "refresh-2"})),
            (
                Subscription::ClaudeCode,
                json!({"access_token": "new-access", "refresh_token": "refresh-2"}),
            ),
        ] {
            // Dead already: nothing to hand over instead.
            let file = match subscription {
                Subscription::Codex => codex_login(home.path(), &codex_token(-1), "refresh-1"),
                Subscription::ClaudeCode => {
                    claude_login(home.path(), "old-access", "refresh-1", in_hours(-1) * 1000)
                }
            };
            let before = fs::read(&file).unwrap();
            let (url, _) = service(vec![(200, answer)], |_| {}).await;
            let error = subscription
                .access_at(&file, &url, BUDGET)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("without a new access token"), "{error}");
            assert!(error.ends_with("login is unchanged"), "{error}");
            assert_eq!(fs::read(&file).unwrap(), before);
        }
    }

    #[tokio::test]
    async fn a_refresh_that_fails_hands_over_a_token_that_still_works_with_a_warning() {
        let home = tempfile::tempdir().unwrap();
        let access = codex_token(1);
        let file = codex_login(home.path(), &access, "refresh-1");
        let before = fs::read(&file).unwrap();
        let (url, _) = service(vec![(503, json!({"error": "overloaded"}))], |_| {}).await;
        let got = Subscription::Codex
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(got.token.1, access);
        let warning = got.warning.unwrap();
        assert!(
            warning.starts_with("the Codex login could not be refreshed (")
                && warning.contains("answered 503")
                && warning.contains("which may be before the group ends"),
            "{warning}"
        );
        assert_eq!(fs::read(&file).unwrap(), before);

        // Refused, the same.
        let (url, received) = service(
            vec![(401, json!({"error": {"code": "refresh_token_reused"}}))],
            |_| {},
        )
        .await;
        let got = Subscription::Codex
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert!(got.warning.unwrap().contains("expired; run `codex login`"));
        assert_eq!(requests(&received), ["refresh-1"]);
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[tokio::test]
    async fn a_dead_login_says_to_sign_in_again() {
        let home = tempfile::tempdir().unwrap();
        let file = codex_login(home.path(), &codex_token(-1), "refresh-1");
        let before = fs::read(&file).unwrap();
        let (url, _) = service(
            vec![(401, json!({"error": {"code": "refresh_token_reused"}}))],
            |_| {},
        )
        .await;
        let error = Subscription::Codex
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the Codex login on this machine expired; run `codex login`"
        );
        assert_eq!(fs::read(&file).unwrap(), before);

        let error = Subscription::ClaudeCode
            .access(Some(home.path()), BUDGET)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("there is no Claude login on this machine"),
            "{error}"
        );
    }
}
