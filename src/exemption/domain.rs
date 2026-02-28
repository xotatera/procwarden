//! Domain models for process exemptions.

use std::path::{Path, PathBuf};

/// An exemption entry (path + hashes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exemption {
    pub path: PathBuf,
    pub sha256: String,
    pub blake3: String,
    pub display_name: String, // Cached filename for UI
}

impl Exemption {
    /// Create a new exemption.
    pub fn new(path: PathBuf, sha256: String, blake3: String) -> Self {
        let display_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        Self {
            path,
            sha256,
            blake3,
            display_name,
        }
    }

    /// Check if this exemption matches the given candidate path.
    /// Uses case-insensitive comparison on Windows.
    pub fn matches_path(&self, candidate_path: &Path) -> bool {
        #[cfg(windows)]
        {
            // Windows paths are case-insensitive
            self.path
                .to_string_lossy()
                .eq_ignore_ascii_case(&candidate_path.to_string_lossy())
        }
        #[cfg(not(windows))]
        {
            self.path == candidate_path
        }
    }
}

/// Collection of exemptions with fast lookup.
#[derive(Debug, Clone, Default)]
pub struct ExemptionList {
    exemptions: Vec<Exemption>,
}

impl ExemptionList {
    pub fn new() -> Self {
        Self {
            exemptions: Vec::new(),
        }
    }

    pub fn add(&mut self, exemption: Exemption) {
        self.exemptions.push(exemption);
    }

    pub fn remove(&mut self, index: usize) -> Option<Exemption> {
        if index < self.exemptions.len() {
            Some(self.exemptions.remove(index))
        } else {
            None
        }
    }

    pub fn get(&self, index: usize) -> Option<&Exemption> {
        self.exemptions.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Exemption> {
        self.exemptions.iter()
    }

    pub fn len(&self) -> usize {
        self.exemptions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.exemptions.is_empty()
    }

    pub fn clear(&mut self) {
        self.exemptions.clear();
    }
}

/// Matches processes against exemptions.
pub struct ExemptionMatcher {
    exemptions: ExemptionList,
}

impl ExemptionMatcher {
    pub fn new(exemptions: ExemptionList) -> Self {
        Self { exemptions }
    }

    /// Check if the given path matches any exemption.
    pub fn is_exempt(&self, path: &Path) -> bool {
        self.exemptions
            .iter()
            .any(|exemption| exemption.matches_path(path))
    }

    /// Get the exemption that matches the given path, if any.
    pub fn find_match(&self, path: &Path) -> Option<&Exemption> {
        self.exemptions
            .iter()
            .find(|exemption| exemption.matches_path(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemption_new_initializes_correctly() {
        let path = PathBuf::from("C:\\test\\app.exe");
        let sha256 = "abc123".to_string();
        let blake3 = "def456".to_string();

        let exemption = Exemption::new(path.clone(), sha256.clone(), blake3.clone());

        assert_eq!(exemption.path, path);
        assert_eq!(exemption.sha256, sha256);
        assert_eq!(exemption.blake3, blake3);
        assert_eq!(exemption.display_name, "app.exe");
    }

    #[test]
    fn exemption_new_handles_no_filename() {
        let path = PathBuf::from("C:\\");
        let exemption = Exemption::new(path, "sha".to_string(), "blake".to_string());
        assert_eq!(exemption.display_name, "unknown");
    }

    #[test]
    fn exemption_matches_path_case_insensitive_on_windows() {
        let exemption = Exemption::new(
            PathBuf::from("C:\\Test\\App.exe"),
            "sha".to_string(),
            "blake".to_string(),
        );

        #[cfg(windows)]
        {
            assert!(exemption.matches_path(&PathBuf::from("C:\\Test\\App.exe")));
            assert!(exemption.matches_path(&PathBuf::from("c:\\test\\app.exe")));
            assert!(exemption.matches_path(&PathBuf::from("C:\\TEST\\APP.EXE")));
        }

        assert!(!exemption.matches_path(&PathBuf::from("C:\\Other\\App.exe")));
    }

    #[test]
    fn exemption_list_add_remove() {
        let mut list = ExemptionList::new();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);

        let exemption = Exemption::new(
            PathBuf::from("C:\\app.exe"),
            "sha".to_string(),
            "blake".to_string(),
        );
        list.add(exemption.clone());

        assert!(!list.is_empty());
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap().path, exemption.path);

        let removed = list.remove(0);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().path, exemption.path);
        assert!(list.is_empty());
    }

    #[test]
    fn exemption_list_remove_out_of_bounds() {
        let mut list = ExemptionList::new();
        assert!(list.remove(0).is_none());
        assert!(list.remove(99).is_none());
    }

    #[test]
    fn exemption_list_get() {
        let mut list = ExemptionList::new();
        list.add(Exemption::new(
            PathBuf::from("C:\\app.exe"),
            "sha".to_string(),
            "blake".to_string(),
        ));

        assert!(list.get(0).is_some());
        assert!(list.get(1).is_none());
    }

    #[test]
    fn exemption_list_iter() {
        let mut list = ExemptionList::new();
        list.add(Exemption::new(
            PathBuf::from("C:\\app1.exe"),
            "sha1".to_string(),
            "blake1".to_string(),
        ));
        list.add(Exemption::new(
            PathBuf::from("C:\\app2.exe"),
            "sha2".to_string(),
            "blake2".to_string(),
        ));

        let collected: Vec<_> = list.iter().collect();
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn exemption_list_clear() {
        let mut list = ExemptionList::new();
        list.add(Exemption::new(
            PathBuf::from("C:\\app.exe"),
            "sha".to_string(),
            "blake".to_string(),
        ));
        assert!(!list.is_empty());

        list.clear();
        assert!(list.is_empty());
    }

    #[test]
    fn exemption_matcher_is_exempt_returns_true_for_match() {
        let mut list = ExemptionList::new();
        list.add(Exemption::new(
            PathBuf::from("C:\\app.exe"),
            "sha".to_string(),
            "blake".to_string(),
        ));

        let matcher = ExemptionMatcher::new(list);
        assert!(matcher.is_exempt(&PathBuf::from("C:\\app.exe")));

        #[cfg(windows)]
        assert!(matcher.is_exempt(&PathBuf::from("c:\\app.exe")));
    }

    #[test]
    fn exemption_matcher_is_exempt_returns_false_for_non_match() {
        let mut list = ExemptionList::new();
        list.add(Exemption::new(
            PathBuf::from("C:\\app.exe"),
            "sha".to_string(),
            "blake".to_string(),
        ));

        let matcher = ExemptionMatcher::new(list);
        assert!(!matcher.is_exempt(&PathBuf::from("C:\\other.exe")));
    }

    #[test]
    fn exemption_matcher_find_match_returns_some_for_match() {
        let mut list = ExemptionList::new();
        let exemption = Exemption::new(
            PathBuf::from("C:\\app.exe"),
            "sha".to_string(),
            "blake".to_string(),
        );
        list.add(exemption.clone());

        let matcher = ExemptionMatcher::new(list);
        let found = matcher.find_match(&PathBuf::from("C:\\app.exe"));
        assert!(found.is_some());
        assert_eq!(found.unwrap().path, exemption.path);
    }

    #[test]
    fn exemption_matcher_find_match_returns_none_for_non_match() {
        let list = ExemptionList::new();
        let matcher = ExemptionMatcher::new(list);
        assert!(matcher.find_match(&PathBuf::from("C:\\app.exe")).is_none());
    }
}
