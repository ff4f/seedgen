//! Serialization for [`DatabaseProfile`]: YAML (default) and JSON, plus
//! path-based load/save that picks the format from the file extension.

use std::path::Path;

use crate::profile::errors::ProfileError;
use crate::profile::stats::DatabaseProfile;

/// The only profile format version this build understands.
pub const SUPPORTED_VERSION: &str = "1.0";

pub fn to_yaml(profile: &DatabaseProfile) -> Result<String, ProfileError> {
    Ok(serde_yaml::to_string(profile)?)
}

pub fn from_yaml(s: &str) -> Result<DatabaseProfile, ProfileError> {
    let profile: DatabaseProfile = serde_yaml::from_str(s)?;
    check_version(&profile)?;
    Ok(profile)
}

pub fn to_json(profile: &DatabaseProfile) -> Result<String, ProfileError> {
    Ok(serde_json::to_string_pretty(profile)?)
}

pub fn from_json(s: &str) -> Result<DatabaseProfile, ProfileError> {
    let profile: DatabaseProfile = serde_json::from_str(s)?;
    check_version(&profile)?;
    Ok(profile)
}

/// Load a profile, choosing the format by extension (`.json` → JSON, else YAML).
pub fn load_profile(path: &Path) -> Result<DatabaseProfile, ProfileError> {
    let content = std::fs::read_to_string(path)?;
    if is_json_path(path) {
        from_json(&content)
    } else {
        from_yaml(&content)
    }
}

/// Save a profile, choosing the format by extension.
pub fn save_profile(profile: &DatabaseProfile, path: &Path) -> Result<(), ProfileError> {
    let serialized = if is_json_path(path) {
        to_json(profile)?
    } else {
        to_yaml(profile)?
    };
    std::fs::write(path, serialized)?;
    Ok(())
}

fn is_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

fn check_version(profile: &DatabaseProfile) -> Result<(), ProfileError> {
    if profile.version != SUPPORTED_VERSION {
        return Err(ProfileError::UnsupportedVersion {
            found: profile.version.clone(),
            expected: SUPPORTED_VERSION.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::profile::config::ProfileOptionsSummary;
    use crate::profile::stats::{ColumnProfile, TableProfile};

    fn sample() -> DatabaseProfile {
        let mut columns = BTreeMap::new();
        columns.insert("id".into(), ColumnProfile::Serial);
        columns.insert(
            "status".into(),
            ColumnProfile::Categorical {
                distribution: BTreeMap::from([("paid".into(), 60.0), ("pending".into(), 40.0)]),
                null_rate: 0.0,
            },
        );
        let mut tables = BTreeMap::new();
        tables.insert(
            "orders".into(),
            TableProfile {
                row_count: 100,
                parent_ratios: BTreeMap::new(),
                columns,
            },
        );
        DatabaseProfile {
            version: "1.0".into(),
            profiled_at: "2026-06-29T10:00:00Z".into(),
            source_hash: "sha256:test".into(),
            seedgen_version: "0.3.0".into(),
            options: ProfileOptionsSummary {
                cardinality_threshold: 50,
                skipped_sensitive: vec![],
            },
            tables,
        }
    }

    #[test]
    fn test_yaml_roundtrip() {
        let p = sample();
        let yaml = to_yaml(&p).unwrap();
        assert_eq!(from_yaml(&yaml).unwrap(), p);
    }

    #[test]
    fn test_json_roundtrip() {
        let p = sample();
        let json = to_json(&p).unwrap();
        assert_eq!(from_json(&json).unwrap(), p);
    }

    #[test]
    fn test_unsupported_version_is_rejected() {
        let mut p = sample();
        p.version = "9.9".into();
        let yaml = to_yaml(&p).unwrap();
        assert!(matches!(
            from_yaml(&yaml),
            Err(ProfileError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn test_save_and_load_by_extension() {
        let p = sample();
        let dir = std::env::temp_dir();
        let pid = std::process::id();

        let yaml_path = dir.join(format!("seedgen-profile-{pid}.yaml"));
        save_profile(&p, &yaml_path).unwrap();
        assert_eq!(load_profile(&yaml_path).unwrap(), p);
        let _ = std::fs::remove_file(&yaml_path);

        let json_path = dir.join(format!("seedgen-profile-{pid}.json"));
        save_profile(&p, &json_path).unwrap();
        let raw = std::fs::read_to_string(&json_path).unwrap();
        assert!(
            raw.trim_start().starts_with('{'),
            "expected JSON, got: {raw}"
        );
        assert_eq!(load_profile(&json_path).unwrap(), p);
        let _ = std::fs::remove_file(&json_path);
    }
}
