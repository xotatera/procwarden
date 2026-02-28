//! Serialization adapters for exemption persistence.

use super::domain::Exemption;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// JSON-serializable exemption data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExemptionData {
    pub path: PathBuf,
    pub sha256: String,
    pub blake3: String,
}

impl From<&Exemption> for ExemptionData {
    fn from(ex: &Exemption) -> Self {
        Self {
            path: ex.path.clone(),
            sha256: ex.sha256.clone(),
            blake3: ex.blake3.clone(),
        }
    }
}

impl From<ExemptionData> for Exemption {
    fn from(data: ExemptionData) -> Self {
        Exemption::new(data.path, data.sha256, data.blake3)
    }
}

/// Convert exemptions to persisted format.
pub fn to_persisted_format(exemptions: &[Exemption]) -> Vec<ExemptionData> {
    exemptions.iter().map(ExemptionData::from).collect()
}

/// Convert persisted format to exemptions.
pub fn from_persisted_format(data: Vec<ExemptionData>) -> Vec<Exemption> {
    data.into_iter().map(Exemption::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemption_data_round_trip() {
        let exemption = Exemption::new(
            PathBuf::from("C:\\test\\app.exe"),
            "abc123".to_string(),
            "def456".to_string(),
        );

        let data = ExemptionData::from(&exemption);
        assert_eq!(data.path, exemption.path);
        assert_eq!(data.sha256, exemption.sha256);
        assert_eq!(data.blake3, exemption.blake3);

        let restored = Exemption::from(data);
        assert_eq!(restored.path, exemption.path);
        assert_eq!(restored.sha256, exemption.sha256);
        assert_eq!(restored.blake3, exemption.blake3);
        assert_eq!(restored.display_name, "app.exe");
    }

    #[test]
    fn to_persisted_format_converts_list() {
        let exemptions = vec![
            Exemption::new(
                PathBuf::from("C:\\app1.exe"),
                "sha1".to_string(),
                "blake1".to_string(),
            ),
            Exemption::new(
                PathBuf::from("C:\\app2.exe"),
                "sha2".to_string(),
                "blake2".to_string(),
            ),
        ];

        let data = to_persisted_format(&exemptions);
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].path, PathBuf::from("C:\\app1.exe"));
        assert_eq!(data[1].path, PathBuf::from("C:\\app2.exe"));
    }

    #[test]
    fn from_persisted_format_converts_list() {
        let data = vec![
            ExemptionData {
                path: PathBuf::from("C:\\app1.exe"),
                sha256: "sha1".to_string(),
                blake3: "blake1".to_string(),
            },
            ExemptionData {
                path: PathBuf::from("C:\\app2.exe"),
                sha256: "sha2".to_string(),
                blake3: "blake2".to_string(),
            },
        ];

        let exemptions = from_persisted_format(data);
        assert_eq!(exemptions.len(), 2);
        assert_eq!(exemptions[0].path, PathBuf::from("C:\\app1.exe"));
        assert_eq!(exemptions[0].display_name, "app1.exe");
        assert_eq!(exemptions[1].path, PathBuf::from("C:\\app2.exe"));
        assert_eq!(exemptions[1].display_name, "app2.exe");
    }

    #[test]
    fn json_serialization_roundtrip() {
        let data = ExemptionData {
            path: PathBuf::from("C:\\test\\app.exe"),
            sha256: "abc123".to_string(),
            blake3: "def456".to_string(),
        };

        let json = serde_json::to_string(&data).unwrap();
        let loaded: ExemptionData = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, data);
    }

    #[test]
    fn json_serialization_list() {
        let data = vec![
            ExemptionData {
                path: PathBuf::from("C:\\app1.exe"),
                sha256: "sha1".to_string(),
                blake3: "blake1".to_string(),
            },
            ExemptionData {
                path: PathBuf::from("C:\\app2.exe"),
                sha256: "sha2".to_string(),
                blake3: "blake2".to_string(),
            },
        ];

        let json = serde_json::to_string(&data).unwrap();
        let loaded: Vec<ExemptionData> = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded, data);
    }
}
