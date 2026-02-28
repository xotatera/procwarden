//! Hash verification service with warning logging.

use super::domain::Exemption;
use super::hash::{Blake3Hasher, HashAlgorithm, Sha256Hasher};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationResult {
    Valid,
    Sha256Mismatch {
        expected: String,
        actual: String,
    },
    Blake3Mismatch {
        expected: String,
        actual: String,
    },
    BothMismatch {
        sha256_expected: String,
        sha256_actual: String,
        blake3_expected: String,
        blake3_actual: String,
    },
    IoError(String),
}

impl VerificationResult {
    pub fn is_valid(&self) -> bool {
        matches!(self, VerificationResult::Valid)
    }

    pub fn has_mismatch(&self) -> bool {
        matches!(
            self,
            VerificationResult::Sha256Mismatch { .. }
                | VerificationResult::Blake3Mismatch { .. }
                | VerificationResult::BothMismatch { .. }
        )
    }
}

pub struct VerificationService {
    sha256_hasher: Sha256Hasher,
    blake3_hasher: Blake3Hasher,
}

impl VerificationService {
    pub fn new() -> Self {
        Self {
            sha256_hasher: Sha256Hasher,
            blake3_hasher: Blake3Hasher,
        }
    }

    /// Verify exemption hash against actual file.
    /// Logs warnings on mismatch but does NOT block exemption (user decision).
    pub fn verify(&self, exemption: &Exemption) -> VerificationResult {
        let path = &exemption.path;

        // Compute both hashes
        let sha256_result = self.sha256_hasher.hash_file(path);
        let blake3_result = self.blake3_hasher.hash_file(path);

        match (sha256_result, blake3_result) {
            (Ok(sha256), Ok(blake3)) => {
                let sha256_match = sha256.eq_ignore_ascii_case(&exemption.sha256);
                let blake3_match = blake3.eq_ignore_ascii_case(&exemption.blake3);

                match (sha256_match, blake3_match) {
                    (true, true) => VerificationResult::Valid,
                    (false, true) => {
                        log::warn!(
                            "Hash mismatch for {}: SHA256 expected {}, got {}",
                            path.display(),
                            &exemption.sha256[..16],
                            &sha256[..16]
                        );
                        VerificationResult::Sha256Mismatch {
                            expected: exemption.sha256.clone(),
                            actual: sha256,
                        }
                    }
                    (true, false) => {
                        log::warn!(
                            "Hash mismatch for {}: Blake3 expected {}, got {}",
                            path.display(),
                            &exemption.blake3[..16],
                            &blake3[..16]
                        );
                        VerificationResult::Blake3Mismatch {
                            expected: exemption.blake3.clone(),
                            actual: blake3,
                        }
                    }
                    (false, false) => {
                        log::warn!(
                            "Both hashes mismatch for {}: SHA256 expected {}, got {}; Blake3 expected {}, got {}",
                            path.display(),
                            &exemption.sha256[..16],
                            &sha256[..16],
                            &exemption.blake3[..16],
                            &blake3[..16]
                        );
                        VerificationResult::BothMismatch {
                            sha256_expected: exemption.sha256.clone(),
                            sha256_actual: sha256,
                            blake3_expected: exemption.blake3.clone(),
                            blake3_actual: blake3,
                        }
                    }
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                log::warn!("Failed to verify {}: {}", path.display(), e);
                VerificationResult::IoError(e.to_string())
            }
        }
    }

    /// Verify all exemptions in a list, return results.
    pub fn verify_all(&self, exemptions: &[&Exemption]) -> Vec<(usize, VerificationResult)> {
        exemptions
            .iter()
            .enumerate()
            .map(|(i, ex)| (i, self.verify(ex)))
            .collect()
    }
}

impl Default for VerificationService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn create_test_file_with_content(content: &[u8]) -> PathBuf {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("verify_test_{}.bin", rand::random::<u32>()));
        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(content).unwrap();
        file_path
    }

    fn cleanup_test_file(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn valid_hash_returns_valid() {
        let content = b"test data";
        let file_path = create_test_file_with_content(content);

        // Compute actual hashes
        let (sha256, blake3) = super::super::hash::hash_file_dual(&file_path).unwrap();

        let exemption = Exemption::new(file_path.clone(), sha256, blake3);

        let service = VerificationService::new();
        let result = service.verify(&exemption);

        assert_eq!(result, VerificationResult::Valid);
        assert!(result.is_valid());
        assert!(!result.has_mismatch());

        cleanup_test_file(&file_path);
    }

    #[test]
    fn sha256_mismatch_returns_sha256_mismatch() {
        let content = b"test data";
        let file_path = create_test_file_with_content(content);

        let (_, blake3) = super::super::hash::hash_file_dual(&file_path).unwrap();
        let wrong_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        let exemption = Exemption::new(file_path.clone(), wrong_sha256.clone(), blake3);

        let service = VerificationService::new();
        let result = service.verify(&exemption);

        match &result {
            VerificationResult::Sha256Mismatch { expected, actual } => {
                assert_eq!(expected, &wrong_sha256);
                assert_ne!(actual, &wrong_sha256);
            }
            _ => panic!("Expected Sha256Mismatch, got {:?}", result),
        }

        assert!(!result.is_valid());
        assert!(result.has_mismatch());

        cleanup_test_file(&file_path);
    }

    #[test]
    fn blake3_mismatch_returns_blake3_mismatch() {
        let content = b"test data";
        let file_path = create_test_file_with_content(content);

        let (sha256, _) = super::super::hash::hash_file_dual(&file_path).unwrap();
        let wrong_blake3 =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        let exemption = Exemption::new(file_path.clone(), sha256, wrong_blake3.clone());

        let service = VerificationService::new();
        let result = service.verify(&exemption);

        match &result {
            VerificationResult::Blake3Mismatch { expected, actual } => {
                assert_eq!(expected, &wrong_blake3);
                assert_ne!(actual, &wrong_blake3);
            }
            _ => panic!("Expected Blake3Mismatch, got {:?}", result),
        }

        assert!(!result.is_valid());
        assert!(result.has_mismatch());

        cleanup_test_file(&file_path);
    }

    #[test]
    fn both_mismatch_returns_both_mismatch() {
        let content = b"test data";
        let file_path = create_test_file_with_content(content);

        let wrong_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        let wrong_blake3 =
            "1111111111111111111111111111111111111111111111111111111111111111".to_string();

        let exemption = Exemption::new(
            file_path.clone(),
            wrong_sha256.clone(),
            wrong_blake3.clone(),
        );

        let service = VerificationService::new();
        let result = service.verify(&exemption);

        match &result {
            VerificationResult::BothMismatch {
                sha256_expected,
                sha256_actual,
                blake3_expected,
                blake3_actual,
            } => {
                assert_eq!(sha256_expected, &wrong_sha256);
                assert_eq!(blake3_expected, &wrong_blake3);
                assert_ne!(sha256_actual, &wrong_sha256);
                assert_ne!(blake3_actual, &wrong_blake3);
            }
            _ => panic!("Expected BothMismatch, got {:?}", result),
        }

        assert!(!result.is_valid());
        assert!(result.has_mismatch());

        cleanup_test_file(&file_path);
    }

    #[test]
    fn missing_file_returns_io_error() {
        let exemption = Exemption::new(
            PathBuf::from("nonexistent_file_12345.txt"),
            "sha".to_string(),
            "blake".to_string(),
        );

        let service = VerificationService::new();
        let result = service.verify(&exemption);

        match result {
            VerificationResult::IoError(_) => {}
            _ => panic!("Expected IoError, got {:?}", result),
        }

        assert!(!result.is_valid());
        assert!(!result.has_mismatch());
    }

    #[test]
    fn verify_all_returns_correct_results() {
        let content1 = b"data1";
        let content2 = b"data2";
        let file1 = create_test_file_with_content(content1);
        let file2 = create_test_file_with_content(content2);

        let (sha1, blake1) = super::super::hash::hash_file_dual(&file1).unwrap();
        let (sha2, blake2) = super::super::hash::hash_file_dual(&file2).unwrap();

        let ex1 = Exemption::new(file1.clone(), sha1, blake1);
        let ex2 = Exemption::new(file2.clone(), sha2, blake2);

        let service = VerificationService::new();
        let results = service.verify_all(&[&ex1, &ex2]);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 0);
        assert_eq!(results[1].0, 1);
        assert!(results[0].1.is_valid());
        assert!(results[1].1.is_valid());

        cleanup_test_file(&file1);
        cleanup_test_file(&file2);
    }

    #[test]
    fn verify_all_with_mixed_results() {
        let content = b"test data";
        let file_path = create_test_file_with_content(content);

        let (sha, blake) = super::super::hash::hash_file_dual(&file_path).unwrap();
        let wrong_sha =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        let ex1 = Exemption::new(file_path.clone(), sha, blake.clone());
        let ex2 = Exemption::new(file_path.clone(), wrong_sha, blake);

        let service = VerificationService::new();
        let results = service.verify_all(&[&ex1, &ex2]);

        assert_eq!(results.len(), 2);
        assert!(results[0].1.is_valid());
        assert!(results[1].1.has_mismatch());

        cleanup_test_file(&file_path);
    }
}
