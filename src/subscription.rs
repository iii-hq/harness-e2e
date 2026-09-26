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
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Environment variables for a group, by name.
pub type Env = Vec<(String, String)>;

/// A provider that signs in with a subscription login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subscription {
    /// `openai-codex`: the `codex` CLI's ChatGPT login.
    Codex,
    /// `claude-code`: the `claude` CLI's Claude Pro/Max login.
    ClaudeCode,
}

/// The refresh endpoints and client ids the CLIs use (`provider-openai-codex`
/// refreshes the same way; the `claude` CLI names Claude's in its OAuth
/// configuration).
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

/// One refresh at a time per login in this worker (its parallel groups);
/// another process waits on the login's lock file.
static CODEX: Mutex<()> = Mutex::const_new(());
static CLAUDE_CODE: Mutex<()> = Mutex::const_new(());

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

    /// The access token a group needs to run for `budget`, as the
    /// environment variables the group reads: `CODEX_ACCESS_TOKEN` and
    /// `CODEX_ACCOUNT_ID`, or `CLAUDE_CODE_ACCESS_TOKEN` and
    /// `CLAUDE_CODE_EXPIRES_AT` (epoch milliseconds). The login is refreshed
    /// first only when its token expires within `budget`.
    pub async fn access(self, budget: Duration) -> Result<Env> {
        let file = self.login_file()?;
        let url = match self {
            Self::Codex => CODEX_TOKEN_URL,
            Self::ClaudeCode => CLAUDE_TOKEN_URL,
        };
        self.access_at(&file, url, budget).await
    }

    /// Where the CLI keeps its login.
    fn login_file(self) -> Result<PathBuf> {
        let (variable, folder, name) = match self {
            Self::Codex => ("CODEX_HOME", ".codex", "auth.json"),
            Self::ClaudeCode => ("CLAUDE_CONFIG_DIR", ".claude", ".credentials.json"),
        };
        let folder = match std::env::var_os(variable).filter(|value| !value.is_empty()) {
            Some(folder) => PathBuf::from(folder),
            None => {
                PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(folder)
            }
        };
        Ok(folder.join(name))
    }

    async fn access_at(self, file: &Path, url: &str, budget: Duration) -> Result<Env> {
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
        // ponytail: this lock orders harness-e2e workers only; the vendor CLI
        // does not take it, so a refresh racing the CLI's own loses one side's
        // rotation. The retry below recovers when the CLI won; a lock the CLI
        // honours would close it.
        let _lock = lock(file).await?;
        // The refresh token the sign-in service refused: tried once more only
        // if the CLI rotated it meanwhile.
        let mut refused: Option<String> = None;
        loop {
            let mut login = read_login(file)?;
            let (env, expires_at) = self.current(&login)?;
            let now = chrono::Utc::now().timestamp();
            if expires_at.is_none_or(|expires_at| expires_at - now >= budget.as_secs() as i64) {
                return Ok(env);
            }
            let refresh_token = self
                .refresh_token(&login)
                .ok_or_else(|| anyhow!(self.expired()))?;
            if refused.as_deref() == Some(refresh_token.as_str()) {
                bail!(self.expired());
            }
            match self.refresh(url, &refresh_token, &login).await? {
                Some(tokens) => {
                    self.apply(&mut login, &tokens)?;
                    self.write(file, &login)?;
                    return Ok(self.current(&login)?.0);
                }
                None if refused.is_none() => refused = Some(refresh_token),
                None => bail!(self.expired()),
            }
        }
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

    /// The group's variables for the login's access token, and when that
    /// token expires (epoch seconds) when the login says.
    fn current(self, login: &Value) -> Result<(Env, Option<i64>)> {
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
                    .map(str::to_owned);
                let mut env = vec![("CODEX_ACCESS_TOKEN".to_owned(), access)];
                env.extend(account.map(|account| ("CODEX_ACCOUNT_ID".to_owned(), account)));
                (env, claims["exp"].as_i64())
            }
            Self::ClaudeCode => {
                let oauth = &login["claudeAiOauth"];
                let access = token(&oauth["accessToken"])?;
                let expires_at = oauth["expiresAt"].as_i64();
                let mut env = vec![("CLAUDE_CODE_ACCESS_TOKEN".to_owned(), access)];
                env.extend(
                    expires_at.map(|ms| ("CLAUDE_CODE_EXPIRES_AT".to_owned(), ms.to_string())),
                );
                (env, expires_at.map(|ms| ms / 1000))
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
        let mut body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": match self {
                Self::Codex => CODEX_CLIENT_ID,
                Self::ClaudeCode => CLAUDE_CLIENT_ID,
            },
        });
        // Claude's CLI asks for the scopes its login holds.
        if let Some(scopes) = login["claudeAiOauth"]["scopes"].as_array() {
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
                match tokens["expires_in"].as_i64() {
                    Some(seconds) => {
                        saved.insert("expiresAt".into(), json!(now + seconds * 1000));
                    }
                    // Its expiry is no longer known.
                    None => {
                        saved.remove("expiresAt");
                    }
                }
                if let Some(seconds) = tokens["refresh_token_expires_in"].as_i64() {
                    saved.insert("refreshTokenExpiresAt".into(), json!(now + seconds * 1000));
                }
            }
        }
        Ok(())
    }

    /// Replace the login file at once, as its CLI writes it: Codex's pretty,
    /// Claude's compact, neither with a trailing newline; mode 600.
    fn write(self, file: &Path, login: &Value) -> Result<()> {
        let bytes = match self {
            Self::Codex => serde_json::to_vec_pretty(login)?,
            Self::ClaudeCode => serde_json::to_vec(login)?,
        };
        let folder = file.parent().context("the login file has no folder")?;
        // Created with mode 600.
        let mut temporary = tempfile::NamedTempFile::new_in(folder)
            .with_context(|| format!("write beside {}", file.display()))?;
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
async fn lock(file: &Path) -> Result<fs::File> {
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
        chrono::Utc::now().timestamp() + hours * 3600
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

    fn names(env: &[(String, String)]) -> Vec<&str> {
        env.iter().map(|(name, _)| name.as_str()).collect()
    }

    #[tokio::test]
    async fn a_token_that_outlasts_the_group_is_handed_over_without_a_refresh() {
        let home = tempfile::tempdir().unwrap();
        let access = codex_token(240);
        let file = codex_login(home.path(), &access, "refresh-1");
        let before = fs::read(&file).unwrap();
        let env = Subscription::Codex
            .access_at(&file, NOWHERE, BUDGET)
            .await
            .unwrap();
        assert_eq!(
            env,
            [
                ("CODEX_ACCESS_TOKEN".to_owned(), access),
                ("CODEX_ACCOUNT_ID".to_owned(), "acct-1".to_owned())
            ]
        );
        assert_eq!(fs::read(&file).unwrap(), before);

        let expires_at = in_hours(5) * 1000;
        let file = claude_login(home.path(), "sk-ant-oat01-access", "refresh-1", expires_at);
        let before = fs::read(&file).unwrap();
        let env = Subscription::ClaudeCode
            .access_at(&file, NOWHERE, BUDGET)
            .await
            .unwrap();
        assert_eq!(
            env,
            [
                (
                    "CLAUDE_CODE_ACCESS_TOKEN".to_owned(),
                    "sk-ant-oat01-access".to_owned()
                ),
                ("CLAUDE_CODE_EXPIRES_AT".to_owned(), expires_at.to_string())
            ]
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
        let env = Subscription::Codex
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(names(&env), ["CODEX_ACCESS_TOKEN", "CODEX_ACCOUNT_ID"]);
        assert_eq!(env[0].1, fresh);
        for (_, value) in &env {
            assert!(!value.contains("refresh-") && !value.contains("id-token-"));
        }
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
    async fn a_claude_token_that_would_expire_is_refreshed_and_only_its_tokens_change() {
        let home = tempfile::tempdir().unwrap();
        let file = claude_login(home.path(), "old-access", "refresh-1", in_hours(1) * 1000);
        let (url, received) = service(
            vec![(
                200,
                json!({"access_token": "new-access", "refresh_token": "refresh-2", "expires_in": 28800}),
            )],
            |_| {},
        )
        .await;
        let env = Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
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
        assert_eq!(
            env,
            [
                (
                    "CLAUDE_CODE_ACCESS_TOKEN".to_owned(),
                    "new-access".to_owned()
                ),
                ("CLAUDE_CODE_EXPIRES_AT".to_owned(), expires_at.to_string())
            ]
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
            // The CLI refreshed first: the service no longer takes refresh-1.
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
        let env = Subscription::ClaudeCode
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap();
        assert_eq!(env[0].1, "new-access");
        assert_eq!(
            received
                .lock()
                .unwrap()
                .iter()
                .map(|request| request["refresh_token"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            ["refresh-1", "refresh-2"]
        );
        assert_eq!(
            read_login(&file).unwrap()["claudeAiOauth"]["refreshToken"],
            "refresh-3"
        );
    }

    #[tokio::test]
    async fn a_refused_login_says_to_sign_in_again_and_a_passing_failure_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let file = codex_login(home.path(), &codex_token(1), "refresh-1");
        let before = fs::read(&file).unwrap();
        let (url, received) = service(
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
        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(fs::read(&file).unwrap(), before);

        let (url, _) = service(vec![(503, json!({"error": "overloaded"}))], |_| {}).await;
        let error = Subscription::Codex
            .access_at(&file, &url, BUDGET)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("answered 503") && error.to_string().contains("unchanged"),
            "{error}"
        );
        assert_eq!(fs::read(&file).unwrap(), before);

        let error = Subscription::ClaudeCode
            .access_at(&home.path().join(".credentials.json"), NOWHERE, BUDGET)
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
