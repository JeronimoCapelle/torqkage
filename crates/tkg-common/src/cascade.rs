//! 4-tier cascaded TOML configuration loader and deep-merge engine.
//!
//! Tiers (lowest to highest priority):
//! 1. Hardcoded / inferred workspace invariants
//! 2. User-global configuration (`~/.config/tkg/project.toml`,
//!    `~/.tkg/project.toml`, `~/.config/tkg/config.toml`)
//! 3. Workspace-local configuration (`<ws>/plugins/torqkage/project.toml`,
//!    `<ws>/project.toml`, `<ws>/.tkg/project.toml`, `<ws>/.tkg/config.toml`)
//! 4. Runtime environment and CLI overrides

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::error::TkgError;

/// Strategy applied when merging two TOML arrays at the same table path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArrayMergeStrategy {
    /// Higher-priority array replaces lower-priority array cleanly (default).
    #[default]
    Replace,
    /// Higher-priority array elements are appended after lower-priority ones.
    Append,
    /// Higher-priority array elements are prepended before lower-priority ones.
    Prepend,
    /// Higher-priority array elements are appended only if not already present.
    UniqueAppend,
}

impl ArrayMergeStrategy {
    fn from_value(val: Option<&toml::Value>) -> Self {
        match val.and_then(toml::Value::as_str).map(str::trim) {
            Some("append" | "concat") => Self::Append,
            Some("prepend") => Self::Prepend,
            Some("unique" | "dedup") => Self::UniqueAppend,
            _ => Self::Replace,
        }
    }
}

/// Normalizes shorthand top-level `default_harness` into `runner.harness` so
/// global/local `config.toml` overrides compose uniformly with `[runner]`.
pub fn normalize_project_layer(table: &mut toml::Table) {
    if let Some(toml::Value::String(harness_name)) = table.remove("default_harness") {
        let runner_entry = table
            .entry("runner")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(runner_tbl) = runner_entry {
            runner_tbl
                .entry("harness")
                .or_insert_with(|| toml::Value::String(harness_name));
        }
    }
}

/// Deep-merges `overlay` TOML table into `base`, respecting optional
/// `merge_strategy = "append" | "prepend" | "unique" | "replace"` on overlay
/// tables (and stripping the `merge_strategy` meta key).
pub fn deep_merge_toml(base: &mut toml::Table, mut overlay: toml::Table) {
    let strategy = ArrayMergeStrategy::from_value(overlay.remove("merge_strategy").as_ref());

    for (key, val) in overlay {
        match (base.get_mut(&key), val) {
            (Some(toml::Value::Table(base_tbl)), toml::Value::Table(over_tbl)) => {
                deep_merge_toml(base_tbl, over_tbl);
            },
            (Some(toml::Value::Array(base_arr)), toml::Value::Array(over_arr)) => {
                merge_arrays(base_arr, over_arr, strategy);
            },
            (_, mut new_val) => {
                if let toml::Value::Table(ref mut tbl) = new_val {
                    strip_merge_strategy_recursive(tbl);
                }
                base.insert(key, new_val);
            },
        }
    }
}

fn strip_merge_strategy_recursive(table: &mut toml::Table) {
    table.remove("merge_strategy");
    for val in table.iter_mut().map(|(_, v)| v) {
        if let toml::Value::Table(inner) = val {
            strip_merge_strategy_recursive(inner);
        }
    }
}

fn merge_arrays(
    base: &mut Vec<toml::Value>,
    overlay: Vec<toml::Value>,
    strategy: ArrayMergeStrategy,
) {
    match strategy {
        ArrayMergeStrategy::Replace => *base = overlay,
        ArrayMergeStrategy::Append => base.extend(overlay),
        ArrayMergeStrategy::Prepend => {
            let mut combined = overlay;
            combined.append(base);
            *base = combined;
        },
        ArrayMergeStrategy::UniqueAppend => {
            for item in overlay {
                if !base.contains(&item) {
                    base.push(item);
                }
            }
        },
    }
}

/// Inserts a dotted key (e.g. `runner.harness`) with `value` into `table`,
/// creating intermediate tables cleanly.
pub fn insert_dotted_override(table: &mut toml::Table, dotted_key: &str, value: toml::Value) {
    let mut segments: Vec<&str> = dotted_key
        .split('.')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let Some(leaf) = segments.pop() else {
        return;
    };

    let mut current = table;
    for seg in segments {
        if !current.get(seg).is_some_and(toml::Value::is_table) {
            current.insert(seg.to_owned(), toml::Value::Table(toml::Table::new()));
        }
        let Some(toml::Value::Table(next)) = current.get_mut(seg) else {
            return;
        };
        current = next;
    }
    current.insert(leaf.to_owned(), value);
}

/// Stateful 4-tier cascaded TOML loader.
#[derive(Debug, Default, Clone)]
pub struct CascadedConfigLoader {
    table: toml::Table,
    files_loaded: usize,
}

impl CascadedConfigLoader {
    /// Creates an empty loader ready for layering tiers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deep-merges a parsed [`toml::Table`] onto the accumulated state.
    pub fn layer_table(&mut self, table: toml::Table) -> &mut Self {
        deep_merge_toml(&mut self.table, table);
        self
    }

    /// Parses and deep-merges an inline TOML snippet with caret diagnostics on
    /// error.
    ///
    /// # Errors
    /// Returns [`TkgError::ConfigParse`] if `content` contains invalid TOML.
    pub fn layer_str(&mut self, path: Option<&Path>, content: &str) -> Result<&mut Self, TkgError> {
        let mut tbl: toml::Table =
            toml::from_str(content).map_err(|err| TkgError::from_toml(path, content, &err))?;
        normalize_project_layer(&mut tbl);
        self.layer_table(tbl);
        Ok(self)
    }

    /// Reads and deep-merges a TOML file if it exists on disk.
    ///
    /// # Errors
    /// Returns [`TkgError::Io`] or [`TkgError::ConfigParse`] if an existing
    /// file cannot be read or contains invalid TOML syntax.
    pub fn layer_optional_file(&mut self, path: &Path) -> Result<bool, TkgError> {
        if !path.is_file() {
            return Ok(false);
        }
        let content = std::fs::read_to_string(path)?;
        self.layer_str(Some(path), &content)?;
        self.files_loaded += 1;
        Ok(true)
    }

    /// Applies an explicit dotted-key CLI override (highest precedence).
    pub fn override_kv(&mut self, dotted_key: &str, value: toml::Value) -> &mut Self {
        insert_dotted_override(&mut self.table, dotted_key, value);
        self
    }

    /// Increments the count of loaded configuration files manually when using
    /// [`Self::layer_str`] directly.
    pub const fn mark_file_loaded(&mut self) {
        self.files_loaded += 1;
    }

    /// Returns how many on-disk files contributed to this cascade.
    #[must_use]
    pub const fn files_loaded(&self) -> usize {
        self.files_loaded
    }

    /// Consumes the loader and returns the merged [`toml::Table`].
    #[must_use]
    pub fn into_table(self) -> toml::Table {
        self.table
    }

    /// Deserializes the merged TOML table into `T`.
    ///
    /// # Errors
    /// Returns [`TkgError::Toml`] if the merged configuration does not match
    /// `T`.
    pub fn deserialize<T: DeserializeOwned>(self) -> Result<T, TkgError> {
        Ok(toml::Value::Table(self.table).try_into()?)
    }
}

/// Ordered candidate config files for the user-global and workspace-local
/// cascade tiers (lowest precedence first).
#[must_use]
pub fn project_cascade_paths(workspace_root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // Tier 2: Global user config
    if let Some(h) = home {
        paths.push(h.join(".config/tkg/project.toml"));
        paths.push(h.join(".tkg/project.toml"));
        paths.push(h.join(".config/tkg/config.toml"));
    }
    // Tier 3: Workspace local config (plugin -> root -> .tkg/project ->
    // .tkg/config)
    paths.push(
        workspace_root
            .join(crate::harness::local_plugin_rel_dir())
            .join("project.toml"),
    );
    paths.push(workspace_root.join("project.toml"));
    paths.push(workspace_root.join(".tkg/project.toml"));
    paths.push(workspace_root.join(".tkg/config.toml"));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_four_tier_cascade_preserves_global_default_harness_and_merges_sandbox() {
        let mut loader = CascadedConfigLoader::new();

        // Tier 1: Hardcoded baseline invariants
        loader
            .layer_str(
                None,
                r#"
                [project]
                language = "rust"
                [sandbox]
                bind_mounts = ["target"]
            "#,
            )
            .unwrap();

        // Tier 2: Global ~/.config/tkg/config.toml sets default_harness =
        // "claude"
        loader
            .layer_str(
                None,
                r#"
                default_harness = "claude"
            "#,
            )
            .unwrap();

        // Tier 3: Workspace .tkg/config.toml only overrides [sandbox] with
        // append
        loader
            .layer_str(
                None,
                r#"
                [sandbox]
                merge_strategy = "append"
                bind_mounts = [".tkg-cache"]
            "#,
            )
            .unwrap();

        // Tier 4: CLI override sets extra arg cleanly without clobbering
        // runner.harness
        loader.override_kv("project.version", toml::Value::String("0.2.0".to_owned()));

        let tbl = loader.into_table();
        assert_eq!(
            tbl.get("runner")
                .and_then(|r| r.get("harness"))
                .and_then(toml::Value::as_str),
            Some("claude")
        );
        assert_eq!(
            tbl.get("project")
                .and_then(|p| p.get("language"))
                .and_then(toml::Value::as_str),
            Some("rust")
        );
        assert_eq!(
            tbl.get("project")
                .and_then(|p| p.get("version"))
                .and_then(toml::Value::as_str),
            Some("0.2.0")
        );
        let mounts: Vec<&str> = tbl
            .get("sandbox")
            .and_then(|s| s.get("bind_mounts"))
            .and_then(toml::Value::as_array)
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect();
        assert_eq!(mounts, vec!["target", ".tkg-cache"]);
        assert!(
            tbl.get("sandbox")
                .and_then(|s| s.get("merge_strategy"))
                .is_none()
        );
    }

    #[test]
    fn test_array_merge_strategies_replace_prepend_unique() {
        let mut base: toml::Table = toml::from_str(
            r#"
            [a]
            items = ["one", "two"]
            [b]
            items = ["two"]
            [c]
            items = ["alpha"]
        "#,
        )
        .unwrap();

        let overlay: toml::Table = toml::from_str(
            r#"
            [a]
            items = ["three"]
            [b]
            merge_strategy = "prepend"
            items = ["one"]
            [c]
            merge_strategy = "unique"
            items = ["alpha", "beta"]
        "#,
        )
        .unwrap();

        deep_merge_toml(&mut base, overlay);

        let arr_strs = |sec: &str| -> Vec<String> {
            base.get(sec)
                .and_then(|s| s.get("items"))
                .and_then(toml::Value::as_array)
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(arr_strs("a"), vec!["three"]);
        assert_eq!(arr_strs("b"), vec!["one", "two"]);
        assert_eq!(arr_strs("c"), vec!["alpha", "beta"]);
    }
}
