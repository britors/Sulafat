//! Fidelity-preserving model of `~/.ssh/config`: [`SshConfig`] is the single entry point a
//! frontend drives — [`SshConfig::load`]/[`list_hosts`](SshConfig::list_hosts) to read,
//! [`upsert_host`](SshConfig::upsert_host)/[`remove_host`](SshConfig::remove_host) plus
//! [`save`](SshConfig::save) to write.

mod parser;
mod writer;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// One of the directives this app has a dedicated UI field for. Anything else is preserved
/// verbatim in [`SshHost::extra`] and never interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownDirective {
    HostName,
    User,
    Port,
    IdentityFile,
    ProxyJump,
}

impl KnownDirective {
    pub(crate) const ALL: [KnownDirective; 5] = [
        Self::HostName,
        Self::User,
        Self::Port,
        Self::IdentityFile,
        Self::ProxyJump,
    ];

    fn keyword(self) -> &'static str {
        match self {
            Self::HostName => "HostName",
            Self::User => "User",
            Self::Port => "Port",
            Self::IdentityFile => "IdentityFile",
            Self::ProxyJump => "ProxyJump",
        }
    }

    fn from_keyword(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|d| d.keyword().eq_ignore_ascii_case(s))
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|d| *d == self)
            .expect("KnownDirective::ALL is exhaustive")
    }
}

/// A single original line, verbatim, including its own line terminator (or none, for a final
/// line with no trailing newline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawLine(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockLine {
    Known {
        directive: KnownDirective,
        line: RawLine,
    },
    Other(RawLine),
}

/// A single-pattern, non-wildcard `Host <alias>` block: the only shape this app edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedBlock {
    pub alias: String,
    pub header: RawLine,
    pub lines: Vec<BlockLine>,
}

/// One chunk of the file, in original order. `Raw` covers comments, blank lines, global
/// directives, `Include` lines, `Match` blocks, and any `Host` block with multiple patterns or a
/// glob/negation pattern — none of that is ever decomposed or rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    Raw(Vec<RawLine>),
    Managed(ManagedBlock),
}

/// A host entry as shown to the UI, whether it came from a rewritable `Host <alias>` block, a
/// read-only wildcard/multi-pattern block, or an `Include`d file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    /// Every directive not mapped to a dedicated field above, verbatim, one per line, in
    /// original order — this is exactly what "Opções avançadas" shows and edits.
    pub extra: String,
    /// True for wildcard/multi-pattern `Host` blocks and anything coming from an `Include`d
    /// file: never rewritten, editing is disabled in the UI.
    pub read_only: bool,
}

impl SshHost {
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            ..Default::default()
        }
    }

    fn set_known(&mut self, directive: KnownDirective, value: String) {
        match directive {
            KnownDirective::HostName => self.host_name = Some(value),
            KnownDirective::User => self.user = Some(value),
            KnownDirective::Port => self.port = value.parse().ok(),
            KnownDirective::IdentityFile => self.identity_file = Some(value),
            KnownDirective::ProxyJump => self.proxy_jump = Some(value),
        }
    }

    /// Like [`set_known`](Self::set_known), but only if the field isn't already populated — used
    /// for best-effort, first-occurrence-wins display parsing of blocks we never rewrite.
    fn set_known_if_absent(&mut self, directive: KnownDirective, value: &str) {
        let already_set = match directive {
            KnownDirective::HostName => self.host_name.is_some(),
            KnownDirective::User => self.user.is_some(),
            KnownDirective::Port => self.port.is_some(),
            KnownDirective::IdentityFile => self.identity_file.is_some(),
            KnownDirective::ProxyJump => self.proxy_jump.is_some(),
        };
        if !already_set {
            self.set_known(directive, value.to_string());
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("falha de E/S ao acessar {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("não foi possível determinar o diretório home do usuário")]
    NoHomeDir,
    #[error("campo {field} inválido: {reason}")]
    Validation {
        field: &'static str,
        reason: &'static str,
    },
    #[error("o arquivo foi alterado externamente; recarregue antes de salvar")]
    Conflict,
    #[error("o OpenSSH recusou a configuração: {0}")]
    InvalidSshConfig(String),
}

impl ConfigError {
    fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

fn value_of(content: &str) -> &str {
    content
        .trim_start()
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .trim()
}

fn host_from_managed(block: &ManagedBlock) -> SshHost {
    let mut host = SshHost::new(block.alias.clone());
    let mut extra_lines = Vec::new();
    for line in &block.lines {
        match line {
            BlockLine::Known { directive, line } => {
                let content = parser::strip_terminator(&line.0);
                host.set_known(*directive, value_of(content).to_string());
            }
            BlockLine::Other(line) => {
                extra_lines.push(parser::strip_terminator(&line.0).to_string());
            }
        }
    }
    host.extra = extra_lines.join("\n");
    host
}

/// Best-effort, read-only scan of a wildcard/multi-pattern raw `Host` block, for display only.
fn host_from_raw_host_block(lines: &[RawLine]) -> Option<SshHost> {
    let first = lines.first()?;
    let content = parser::strip_terminator(&first.0);
    let parser::LineKind::HostHeader(patterns) = parser::classify_top(content) else {
        return None;
    };
    if parser::is_single_plain_pattern(&patterns) {
        return None;
    }
    let mut host = SshHost {
        alias: patterns.join(" "),
        read_only: true,
        ..Default::default()
    };
    for raw in &lines[1..] {
        let content = parser::strip_terminator(&raw.0);
        let trimmed = content.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let keyword = trimmed.split(char::is_whitespace).next().unwrap_or("");
        if let Some(directive) = KnownDirective::from_keyword(keyword) {
            host.set_known_if_absent(directive, value_of(content));
        }
    }
    Some(host)
}

fn extract_include_patterns(segments: &[Segment]) -> Vec<String> {
    let mut patterns = Vec::new();
    for seg in segments {
        let Segment::Raw(lines) = seg else { continue };
        for raw in lines {
            let content = parser::strip_terminator(&raw.0);
            let trimmed = content.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut parts = trimmed.splitn(2, char::is_whitespace);
            if parts.next().unwrap_or("").eq_ignore_ascii_case("include") {
                let rest = parts.next().unwrap_or("").trim();
                patterns.extend(rest.split_whitespace().map(str::to_string));
            }
        }
    }
    patterns
}

/// Load and flatten every `Host` entry out of the files an `Include` pattern matches. Never
/// followed recursively (an included file's own `Include` lines are ignored) — good enough for a
/// read-only view.
fn resolve_includes(base_dir: &Path, patterns: &[String]) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    for pattern in patterns {
        let full_pattern = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base_dir.join(pattern).to_string_lossy().into_owned()
        };
        let Ok(paths) = glob::glob(&full_pattern) else {
            continue;
        };
        for entry in paths.flatten() {
            let Ok(contents) = fs::read_to_string(&entry) else {
                continue;
            };
            for seg in parser::parse(&contents) {
                let host = match &seg {
                    Segment::Managed(block) => Some(host_from_managed(block)),
                    Segment::Raw(lines) => host_from_raw_host_block(lines),
                };
                if let Some(mut host) = host {
                    host.read_only = true;
                    hosts.push(host);
                }
            }
        }
    }
    hosts
}

fn default_path() -> Result<PathBuf, ConfigError> {
    let base = directories::BaseDirs::new().ok_or(ConfigError::NoHomeDir)?;
    Ok(base.home_dir().join(".ssh").join("config"))
}

/// A parsed `~/.ssh/config` (or an arbitrary path, for tests), plus every host flattened out of
/// its `Include`d files.
#[derive(Clone)]
pub struct SshConfig {
    path: PathBuf,
    segments: Vec<Segment>,
    included_hosts: Vec<SshHost>,
    loaded_contents: Option<Vec<u8>>,
}

fn validate_single_line(
    field: &'static str,
    value: &str,
    allow_spaces: bool,
) -> Result<(), ConfigError> {
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control) {
        return Err(ConfigError::Validation {
            field,
            reason: "vazio, iniciado por hífen ou com caractere de controle",
        });
    }
    if !allow_spaces && value.chars().any(char::is_whitespace) {
        return Err(ConfigError::Validation {
            field,
            reason: "espaços não são permitidos",
        });
    }
    Ok(())
}

pub fn validate_host(host: &SshHost) -> Result<(), ConfigError> {
    validate_single_line("Alias", &host.alias, false)?;
    for (field, value, spaces) in [
        ("HostName", host.host_name.as_deref(), false),
        ("User", host.user.as_deref(), false),
        ("IdentityFile", host.identity_file.as_deref(), true),
        ("ProxyJump", host.proxy_jump.as_deref(), false),
    ] {
        if let Some(value) = value {
            validate_single_line(field, value, spaces)?;
        }
    }
    if host.extra.as_bytes().contains(&0) {
        return Err(ConfigError::Validation {
            field: "Opções avançadas",
            reason: "NUL não é permitido",
        });
    }
    Ok(())
}

impl SshConfig {
    /// Load `~/.ssh/config`. A missing file is treated as an empty config, not an error.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(default_path()?)
    }

    /// Empty editable model at the real default path, used by frontends only to preserve the
    /// destination while a load error is being shown. It must not be saved until reloaded.
    pub fn empty_at_default_path() -> Result<Self, ConfigError> {
        let path = default_path()?;
        Ok(Self {
            path,
            segments: Vec::new(),
            included_hosts: Vec::new(),
            loaded_contents: None,
        })
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(ConfigError::io(&path, e)),
        };
        let segments = parser::parse(&contents);
        let base_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let included_hosts = resolve_includes(&base_dir, &extract_include_patterns(&segments));
        Ok(Self {
            path,
            segments,
            included_hosts,
            loaded_contents: Some(contents.into_bytes()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every host known to this config: rewritable `Host <alias>` blocks, read-only
    /// wildcard/multi-pattern blocks, and read-only hosts from `Include`d files.
    pub fn list_hosts(&self) -> Vec<SshHost> {
        let mut hosts: Vec<SshHost> = self
            .segments
            .iter()
            .filter_map(|seg| match seg {
                Segment::Managed(block) => Some(host_from_managed(block)),
                Segment::Raw(lines) => host_from_raw_host_block(lines),
            })
            .collect();
        hosts.extend(self.included_hosts.iter().cloned());
        hosts
    }

    /// Create or update the `Host <alias>` block matching `host.alias`, appending a new block at
    /// the end of the file if none exists yet.
    pub fn upsert_host(&mut self, host: SshHost) {
        writer::upsert(&mut self.segments, &host);
    }

    /// Same as [`upsert_host`](Self::upsert_host), but locates the block via `previous_alias`
    /// (the alias before the edit) so renaming an alias updates the existing block in place
    /// instead of appending a duplicate.
    pub fn upsert_host_renaming(&mut self, previous_alias: &str, host: SshHost) {
        writer::upsert_renaming(&mut self.segments, Some(previous_alias), &host);
    }

    /// Remove the `Host <alias>` block entirely. Returns `false` if no such managed block exists
    /// (wildcard/multi-pattern/`Include`d hosts can never be removed this way).
    pub fn remove_host(&mut self, alias: &str) -> bool {
        writer::remove(&mut self.segments, alias)
    }

    /// Write the file back atomically (temp file + rename), backing up the previous contents to
    /// `<path>.sulafat.bak` and enforcing 0600 permissions on the result. Creates the parent
    /// directory with 0700 permissions if it didn't already exist.
    pub fn save(&mut self) -> Result<(), ConfigError> {
        let contents = parser::render(&self.segments);

        for host in self.list_hosts().iter().filter(|h| !h.read_only) {
            validate_host(host)?;
        }

        if let Some(parent) = self.path.parent() {
            let dir_existed = parent.exists();
            fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
            #[cfg(unix)]
            if !dir_existed {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                    .map_err(|e| ConfigError::io(parent, e))?;
            }
        }

        let current = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Vec::new()),
            Err(e) => return Err(ConfigError::io(&self.path, e)),
        };
        if current.as_deref() != self.loaded_contents.as_deref() {
            return Err(ConfigError::Conflict);
        }

        self.validate_candidate(contents.as_bytes())?;
        if let Some(old) = current.filter(|b| !b.is_empty()) {
            self.rotate_backups(&old)?;
        }
        crate::atomic::write(&self.path, contents.as_bytes())
            .map_err(|e| ConfigError::io(&self.path, e))?;
        self.loaded_contents = Some(contents.into_bytes());
        Ok(())
    }

    fn backup_path(&self, generation: usize) -> PathBuf {
        PathBuf::from(format!("{}.sulafat.bak.{generation}", self.path.display()))
    }

    fn rotate_backups(&self, current: &[u8]) -> Result<(), ConfigError> {
        for generation in (1..3).rev() {
            let from = self.backup_path(generation);
            let to = self.backup_path(generation + 1);
            if from.exists() {
                fs::rename(&from, &to).map_err(|e| ConfigError::io(&to, e))?;
            }
        }
        let first = self.backup_path(1);
        crate::atomic::write(&first, current).map_err(|e| ConfigError::io(&first, e))
    }

    fn validate_candidate(&self, contents: &[u8]) -> Result<(), ConfigError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::Builder::new()
            .prefix(".sulafat-validate-")
            .tempfile_in(parent)
            .map_err(|e| ConfigError::io(parent, e))?;
        use std::io::Write;
        tmp.write_all(contents)
            .map_err(|e| ConfigError::io(tmp.path(), e))?;
        let output = Command::new("ssh")
            .args(["-G", "-F"])
            .arg(tmp.path())
            .arg("sulafat-validation.invalid")
            .output()
            .map_err(|e| ConfigError::io("ssh", e))?;
        if output.status.success() {
            return Ok(());
        }
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(ConfigError::InvalidSshConfig(message))
    }

    pub fn restore_backup(&mut self, generation: usize) -> Result<(), ConfigError> {
        if !(1..=3).contains(&generation) {
            return Err(ConfigError::Validation {
                field: "backup",
                reason: "geração deve estar entre 1 e 3",
            });
        }
        let backup = self.backup_path(generation);
        let contents = fs::read(&backup).map_err(|e| ConfigError::io(&backup, e))?;
        self.validate_candidate(&contents)?;
        if let Ok(current) = fs::read(&self.path) {
            if !current.is_empty() {
                self.rotate_backups(&current)?;
            }
        }
        crate::atomic::write(&self.path, &contents).map_err(|e| ConfigError::io(&self.path, e))?;
        *self = Self::load_from(self.path.clone())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config");
        fs::write(&path, contents).expect("write fixture");
        (dir, path)
    }

    #[test]
    fn list_hosts_reports_managed_and_wildcard_entries() {
        let (_dir, path) =
            write_temp("Host prod\n    HostName 10.0.0.1\n    User admin\n\nHost *\n    ServerAliveInterval 60\n");
        let cfg = SshConfig::load_from(&path).expect("load");
        let hosts = cfg.list_hosts();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].alias, "prod");
        assert!(!hosts[0].read_only);
        assert_eq!(hosts[0].user.as_deref(), Some("admin"));
        assert_eq!(hosts[1].alias, "*");
        assert!(hosts[1].read_only);
    }

    #[test]
    fn save_roundtrips_when_nothing_changed() {
        let original = "# comment\nHost prod\n    HostName 10.0.0.1\n";
        let (_dir, path) = write_temp(original);
        let mut cfg = SshConfig::load_from(&path).expect("load");
        cfg.save().expect("save");
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn save_creates_backup_and_enforces_permissions() {
        let (_dir, path) = write_temp("Host prod\n    HostName 10.0.0.1\n");
        let mut cfg = SshConfig::load_from(&path).expect("load");
        cfg.upsert_host(SshHost {
            alias: "prod".into(),
            host_name: Some("10.0.0.2".into()),
            ..Default::default()
        });
        cfg.save().expect("save");

        let backup_path = PathBuf::from(format!("{}.sulafat.bak.1", path.display()));
        assert_eq!(
            fs::read_to_string(&backup_path).unwrap(),
            "Host prod\n    HostName 10.0.0.1\n"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "Host prod\n    HostName 10.0.0.2\n"
        );

        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn included_files_are_listed_but_never_written_to() {
        let dir = tempfile::tempdir().expect("tempdir");
        let confd = dir.path().join("conf.d");
        fs::create_dir_all(&confd).unwrap();
        let mut included = fs::File::create(confd.join("extra.conf")).unwrap();
        writeln!(included, "Host included-host\n    HostName 10.0.0.9").unwrap();

        let main_path = dir.path().join("config");
        fs::write(
            &main_path,
            "Include conf.d/*.conf\n\nHost prod\n    HostName 10.0.0.1\n",
        )
        .unwrap();

        let mut cfg = SshConfig::load_from(&main_path).expect("load");
        let hosts = cfg.list_hosts();
        let inc = hosts
            .iter()
            .find(|h| h.alias == "included-host")
            .expect("included host present");
        assert!(inc.read_only);
        assert_eq!(inc.host_name.as_deref(), Some("10.0.0.9"));

        cfg.save().expect("save");
        assert_eq!(
            fs::read_to_string(&main_path).unwrap(),
            "Include conf.d/*.conf\n\nHost prod\n    HostName 10.0.0.1\n"
        );
    }

    #[test]
    fn external_change_is_not_overwritten_and_own_saves_do_not_conflict() {
        let (_dir, path) = write_temp("Host one\n");
        let mut cfg = SshConfig::load_from(&path).unwrap();
        fs::write(&path, "Host external\n").unwrap();
        assert!(matches!(cfg.save(), Err(ConfigError::Conflict)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "Host external\n");

        let mut cfg = SshConfig::load_from(&path).unwrap();
        cfg.save().unwrap();
        cfg.save().unwrap();
    }

    #[test]
    fn structured_fields_cannot_inject_directives() {
        for host in [
            SshHost::new("-V"),
            SshHost::new("prod\nProxyCommand evil"),
            SshHost {
                alias: "prod".into(),
                host_name: Some("server\rUser root".into()),
                ..Default::default()
            },
        ] {
            assert!(validate_host(&host).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn config_save_refuses_destination_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "intacto").unwrap();
        let path = dir.path().join("config");
        symlink(&victim, &path).unwrap();
        let mut cfg = SshConfig::empty_at_default_path().unwrap();
        cfg.path = path;
        assert!(cfg.save().is_err());
        assert_eq!(fs::read_to_string(victim).unwrap(), "intacto");
    }

    #[test]
    fn backups_rotate_three_generations_and_can_be_restored() {
        let (_dir, path) = write_temp("Host version-0\n");
        let mut cfg = SshConfig::load_from(&path).unwrap();
        for version in 1..=4 {
            cfg.segments = parser::parse(&format!("Host version-{version}\n"));
            cfg.save().unwrap();
        }
        assert_eq!(
            fs::read_to_string(cfg.backup_path(1)).unwrap(),
            "Host version-3\n"
        );
        assert_eq!(
            fs::read_to_string(cfg.backup_path(2)).unwrap(),
            "Host version-2\n"
        );
        assert_eq!(
            fs::read_to_string(cfg.backup_path(3)).unwrap(),
            "Host version-1\n"
        );
        cfg.restore_backup(2).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "Host version-2\n");
    }
}
