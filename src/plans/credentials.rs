//! Provider credentials this Console keeps: the environment variables a
//! Docker execution's phases receive. They live in `credentials.env` of the
//! data directory, mode 600: never in the database, an execution's folder or
//! its evidence. Nothing here answers with a value or logs one. The worker's
//! `provider_env_file`, when configured, adds its entries under them: where
//! both name a variable, this store's value wins.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where this Console keeps them, in its data directory.
pub(crate) const FILE: &str = "credentials.env";

/// What every phase sets itself: a credential never takes its place.
const RESERVED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "PWD",
    "TMPDIR",
    "LANG",
    "CI",
    "EXECUTION_KEY",
    "GITHUB_TOKEN",
];
const RESERVED_PREFIXES: &[&str] = &[
    "HARNESS_E2E_",
    "DISPATCH_",
    "III_",
    "GIT_",
    "DOCKER_",
    "LC_",
];

/// One read-modify-write at a time.
static WRITES: Mutex<()> = Mutex::new(());

/// The key each provider reads and the other credentials workers read, as
/// the scripts read them.
#[derive(Deserialize)]
struct Catalog {
    providers: BTreeMap<String, String>,
    others: Vec<String>,
}

fn catalog() -> Catalog {
    serde_json::from_str(include_str!("../../config/provider-credentials.json"))
        .expect("config/provider-credentials.json is the catalog")
}

/// The key `provider` reads, when the catalog knows it.
pub(crate) fn provider_key(provider: &str) -> Option<String> {
    catalog().providers.remove(provider)
}

/// A credential by name; never its value.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub(crate) struct CredentialView {
    pub name: String,
    pub set: bool,
    /// `console` (set here), `provider_env_file` (only the worker's file
    /// sets it), or absent when it is not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The providers that read it, as the catalog says.
    pub providers: Vec<String>,
}

/// What an import from this machine's environment found.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub(crate) struct Imported {
    pub found: Vec<String>,
    pub not_found: Vec<String>,
}

/// An environment variable's name that no phase sets itself.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    ensure!(
        chars.next().is_some_and(|first| first.is_ascii_uppercase())
            && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
        "Name a credential as an environment variable: capital letters, digits and _, starting with a letter (OPENAI_API_KEY)."
    );
    ensure!(
        !RESERVED.contains(&name) && !RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)),
        "{name} is set by the executor itself; name the credential otherwise."
    );
    Ok(())
}

/// One line an env file can carry, without the spaces a paste brings.
fn clean_value(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && !value.contains(['\n', '\r', '\0'])).then_some(value)
}

/// `NAME=value` lines, as `docker run --env-file` reads them.
fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.trim_start().split_once('='))
        .filter(|(name, value)| validate_name(name).is_ok() && !value.is_empty())
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

fn render(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(name, value)| format!("{name}={value}\n"))
        .collect()
}

/// The credentials of a data directory, with the worker's
/// `provider_env_file` under them.
pub(crate) struct Credentials {
    path: PathBuf,
    extra: Option<PathBuf>,
}

impl Credentials {
    pub(crate) fn new(data_dir: &Path, provider_env_file: Option<PathBuf>) -> Self {
        Self {
            path: data_dir.join(FILE),
            extra: provider_env_file,
        }
    }

    fn read(path: &Path) -> Result<BTreeMap<String, String>> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(parse(&text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    fn stored(&self) -> Result<BTreeMap<String, String>> {
        Self::read(&self.path)
    }

    fn worker_file(&self) -> Result<BTreeMap<String, String>> {
        self.extra
            .as_deref()
            .map_or_else(|| Ok(BTreeMap::new()), Self::read)
    }

    /// Replaced whole, through a file only its owner reads.
    fn write(&self, values: &BTreeMap<String, String>) -> Result<()> {
        let directory = self.path.parent().context("credentials path")?;
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(render(values).as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&self.path)
            .with_context(|| format!("replace {}", self.path.display()))?;
        Ok(())
    }

    /// Every credential the catalog knows or either source sets, by name.
    pub(crate) fn list(&self) -> Result<Vec<CredentialView>> {
        let (stored, from_file, catalog) = (self.stored()?, self.worker_file()?, catalog());
        let names = catalog
            .providers
            .values()
            .chain(&catalog.others)
            .chain(stored.keys())
            .chain(from_file.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        Ok(names
            .into_iter()
            .map(|name| {
                let source = if stored.contains_key(&name) {
                    Some("console")
                } else if from_file.contains_key(&name) {
                    Some("provider_env_file")
                } else {
                    None
                };
                CredentialView {
                    set: source.is_some(),
                    source: source.map(str::to_owned),
                    providers: catalog
                        .providers
                        .iter()
                        .filter(|(_, key)| **key == name)
                        .map(|(provider, _)| provider.clone())
                        .collect(),
                    name,
                }
            })
            .collect())
    }

    pub(crate) fn set(&self, name: &str, value: &str) -> Result<()> {
        validate_name(name)?;
        let Some(value) = clean_value(value) else {
            bail!("Give {name} a value on one line.");
        };
        let _guard = WRITES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stored = self.stored()?;
        stored.insert(name.to_owned(), value.to_owned());
        self.write(&stored)
    }

    pub(crate) fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let _guard = WRITES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stored = self.stored()?;
        if stored.remove(name).is_none() {
            if self.worker_file()?.contains_key(name) {
                bail!("{name} comes from the worker's provider_env_file; remove it there.");
            }
            bail!("No credential {name} is set in this Console.");
        }
        self.write(&stored)
    }

    /// Set each credential the catalog knows that `lookup` (this worker's
    /// environment) holds, and say which it did not.
    pub(crate) fn import(&self, lookup: impl Fn(&str) -> Option<String>) -> Result<Imported> {
        let catalog = catalog();
        let names = catalog
            .providers
            .values()
            .chain(&catalog.others)
            .cloned()
            .collect::<BTreeSet<_>>();
        let _guard = WRITES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stored = self.stored()?;
        let mut imported = Imported {
            found: Vec::new(),
            not_found: Vec::new(),
        };
        for name in names {
            match lookup(&name).as_deref().and_then(clean_value) {
                Some(value) => {
                    stored.insert(name.clone(), value.to_owned());
                    imported.found.push(name);
                }
                None => imported.not_found.push(name),
            }
        }
        if !imported.found.is_empty() {
            self.write(&stored)?;
        }
        Ok(imported)
    }

    /// What a phase receives: the worker's file, then this store over it.
    pub(crate) fn merged(&self) -> Result<BTreeMap<String, String>> {
        let mut values = self.worker_file()?;
        values.extend(self.stored()?);
        Ok(values)
    }

    /// The merged credentials in a private temporary file (mode 600) that
    /// goes when it is dropped; none when there are none.
    pub(crate) fn env_file(&self) -> Result<Option<tempfile::NamedTempFile>> {
        let values = self.merged()?;
        if values.is_empty() {
            return Ok(None);
        }
        let mut file = tempfile::Builder::new()
            .prefix("harness-e2e-credentials.")
            .suffix(".env")
            .tempfile()
            .context("create the credentials file for a phase")?;
        file.write_all(render(&values).as_bytes())?;
        file.flush()?;
        Ok(Some(file))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_credential_is_kept_private_and_listed_by_name_only() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(root.path(), None);
        // Before anything is set: the catalog's names, none set.
        let listed = credentials.list().unwrap();
        let openai = listed.iter().find(|c| c.name == "OPENAI_API_KEY").unwrap();
        assert_eq!(
            openai,
            &CredentialView {
                name: "OPENAI_API_KEY".into(),
                set: false,
                source: None,
                providers: vec!["openai".into()],
            }
        );
        credentials
            .set("OPENAI_API_KEY", "  sk-openai-secret\n")
            .unwrap();
        credentials.set("MY_GATEWAY_TOKEN", "gw-secret").unwrap();
        let file = root.path().join(FILE);
        assert_eq!(mode(&file), 0o600);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "MY_GATEWAY_TOKEN=gw-secret\nOPENAI_API_KEY=sk-openai-secret\n"
        );
        let listed = credentials.list().unwrap();
        let shown = serde_json::to_string(&listed).unwrap();
        assert!(!shown.contains("secret"), "{shown}");
        let custom = listed
            .iter()
            .find(|c| c.name == "MY_GATEWAY_TOKEN")
            .unwrap();
        assert!(custom.set && custom.providers.is_empty());
        assert_eq!(custom.source.as_deref(), Some("console"));
        // Replaced, then deleted; still private.
        credentials
            .set("OPENAI_API_KEY", "sk-other-secret")
            .unwrap();
        credentials.delete("MY_GATEWAY_TOKEN").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "OPENAI_API_KEY=sk-other-secret\n"
        );
        assert!(!credentials
            .list()
            .unwrap()
            .iter()
            .any(|c| c.name == "MY_GATEWAY_TOKEN"));
        let error = credentials.delete("MY_GATEWAY_TOKEN").unwrap_err();
        assert!(error.to_string().contains("No credential"), "{error}");
    }

    #[test]
    fn names_are_environment_variables_no_phase_sets_itself_and_values_one_line() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(root.path(), None);
        for name in [
            "",
            "openai_api_key",
            "1KEY",
            "OPENAI-API-KEY",
            "KEY WITH SPACE",
            "A=B",
        ] {
            let error = credentials.set(name, "value").unwrap_err().to_string();
            assert!(error.contains("environment variable"), "{name}: {error}");
        }
        for name in [
            "PATH",
            "GITHUB_TOKEN",
            "HARNESS_E2E_CREDENTIALS",
            "III_TELEMETRY_ENABLED",
            "DISPATCH_SUITE",
        ] {
            let error = credentials.set(name, "value").unwrap_err().to_string();
            assert!(error.contains("executor itself"), "{name}: {error}");
        }
        for value in ["", "   ", "two\nlines", "nul\0"] {
            let error = credentials
                .set("OPENAI_API_KEY", value)
                .unwrap_err()
                .to_string();
            assert!(error.contains("one line"), "{value:?}: {error}");
            assert!(!error.contains(value.trim()) || value.trim().is_empty());
        }
        assert!(!root.path().join(FILE).exists());
    }

    #[test]
    fn the_worker_file_is_under_the_store_and_a_phase_gets_both_in_a_private_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("providers.env");
        fs::write(
            &file,
            "# the worker's\nDEEPSEEK_API_KEY=from-file\nZAI_API_KEY=zai-from-file\nlower=x\n",
        )
        .unwrap();
        let credentials = Credentials::new(&root.path().join("data"), Some(file));
        assert!(credentials.env_file().unwrap().is_some());
        credentials.set("DEEPSEEK_API_KEY", "from-console").unwrap();
        let listed = credentials.list().unwrap();
        let source = |name: &str| {
            listed
                .iter()
                .find(|c| c.name == name)
                .unwrap()
                .source
                .clone()
        };
        assert_eq!(source("DEEPSEEK_API_KEY").as_deref(), Some("console"));
        assert_eq!(source("ZAI_API_KEY").as_deref(), Some("provider_env_file"));
        assert_eq!(source("OPENAI_API_KEY"), None);
        let error = credentials.delete("ZAI_API_KEY").unwrap_err().to_string();
        assert!(error.contains("provider_env_file"), "{error}");

        let phase = credentials.env_file().unwrap().unwrap();
        let path = phase.path().to_owned();
        assert_eq!(mode(&path), 0o600);
        assert!(!path.starts_with(root.path()));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "DEEPSEEK_API_KEY=from-console\nZAI_API_KEY=zai-from-file\n"
        );
        drop(phase);
        assert!(!path.exists());
        // None at all: no file.
        assert!(Credentials::new(root.path(), None)
            .env_file()
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_import_takes_the_catalogs_names_from_this_machine_and_says_which() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(root.path(), None);
        let environment = BTreeMap::from([
            ("DEEPSEEK_API_KEY", "sk-deepseek"),
            ("ZAI_API_KEY", "  "),
            ("GITHUB_TOKEN", "never-imported"),
            ("CHOCOLATEY_API_KEY", "never-imported"),
        ]);
        let imported = credentials
            .import(|name| environment.get(name).map(|value| (*value).to_owned()))
            .unwrap();
        assert_eq!(imported.found, vec!["DEEPSEEK_API_KEY"]);
        assert!(imported.not_found.contains(&"ZAI_API_KEY".to_owned()));
        assert!(imported.not_found.contains(&"OPENAI_API_KEY".to_owned()));
        assert_eq!(
            fs::read_to_string(root.path().join(FILE)).unwrap(),
            "DEEPSEEK_API_KEY=sk-deepseek\n"
        );
        assert_eq!(
            provider_key("deepseek").as_deref(),
            Some("DEEPSEEK_API_KEY")
        );
        assert_eq!(provider_key("openai-codex"), None);
    }
}
