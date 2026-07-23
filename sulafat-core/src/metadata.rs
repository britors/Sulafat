//! UI-only metadata (group, color, notes) that has no place in `~/.ssh/config` itself, keyed by
//! host alias and stored separately in `~/.config/sulafat/metadata.toml`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl HostMeta {
    pub fn is_empty(&self) -> bool {
        self.group.is_none() && self.color.is_none() && self.notes.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    hosts: HashMap<String, HostMeta>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("falha de E/S ao acessar {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("metadados inválidos: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("falha ao serializar metadados: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("não foi possível determinar o diretório de configuração do usuário")]
    NoConfigDir,
}

impl MetadataError {
    fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}

/// `~/.config/sulafat` (or the platform equivalent) — shared with `sulafat-gtk`'s own
/// `settings.toml`, so the XDG lookup only happens in one place.
pub fn config_dir() -> Result<PathBuf, MetadataError> {
    let dirs = directories::ProjectDirs::from("org", "lyraos", "sulafat").ok_or(MetadataError::NoConfigDir)?;
    Ok(dirs.config_dir().to_path_buf())
}

fn metadata_path() -> Result<PathBuf, MetadataError> {
    Ok(config_dir()?.join("metadata.toml"))
}

impl Metadata {
    /// Load `~/.config/sulafat/metadata.toml`. A missing file means "no metadata yet", not an
    /// error.
    pub fn load() -> Result<Self, MetadataError> {
        let path = metadata_path()?;
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(MetadataError::io(path, source)),
        };
        Ok(toml::from_str(&contents)?)
    }

    pub fn get(&self, alias: &str) -> Option<&HostMeta> {
        self.hosts.get(alias)
    }

    pub fn set(&mut self, alias: impl Into<String>, meta: HostMeta) {
        let alias = alias.into();
        if meta.is_empty() {
            self.hosts.remove(&alias);
        } else {
            self.hosts.insert(alias, meta);
        }
    }

    pub fn groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self.hosts.values().filter_map(|m| m.group.clone()).collect();
        groups.sort();
        groups.dedup();
        groups
    }

    /// Drop any entry whose alias is no longer present in `known_aliases` — hosts removed from
    /// `~/.ssh/config` outside the app don't leave orphaned metadata behind.
    fn prune(&mut self, known_aliases: &[String]) {
        self.hosts.retain(|alias, _| known_aliases.contains(alias));
    }

    /// Persist metadata, pruning orphans first (see [`Self::prune`]).
    pub fn save(&mut self, known_aliases: &[String]) -> Result<(), MetadataError> {
        self.prune(known_aliases);
        let path = metadata_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| MetadataError::io(parent, e))?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents).map_err(|e| MetadataError::io(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_round_trip_through_toml() {
        let mut meta = Metadata::default();
        meta.set("prod", HostMeta { group: Some("Produção".into()), color: Some("#e01b24".into()), notes: None });
        let toml_text = toml::to_string_pretty(&meta).expect("serialize");
        let parsed: Metadata = toml::from_str(&toml_text).expect("deserialize");
        assert_eq!(parsed.get("prod").unwrap().color.as_deref(), Some("#e01b24"));
    }

    #[test]
    fn setting_empty_meta_removes_the_entry() {
        let mut meta = Metadata::default();
        meta.set("prod", HostMeta { group: Some("X".into()), ..Default::default() });
        assert!(meta.get("prod").is_some());
        meta.set("prod", HostMeta::default());
        assert!(meta.get("prod").is_none());
    }

    #[test]
    fn groups_are_sorted_and_deduplicated() {
        let mut meta = Metadata::default();
        meta.set("a", HostMeta { group: Some("Prod".into()), ..Default::default() });
        meta.set("b", HostMeta { group: Some("Homolog".into()), ..Default::default() });
        meta.set("c", HostMeta { group: Some("Prod".into()), ..Default::default() });
        assert_eq!(meta.groups(), vec!["Homolog".to_string(), "Prod".to_string()]);
    }

    #[test]
    fn prune_removes_aliases_missing_from_known_list() {
        let mut meta = Metadata::default();
        meta.set("kept", HostMeta { group: Some("X".into()), ..Default::default() });
        meta.set("orphan", HostMeta { group: Some("Y".into()), ..Default::default() });
        meta.prune(&["kept".to_string()]);
        assert!(meta.get("orphan").is_none());
        assert!(meta.get("kept").is_some());
    }
}
