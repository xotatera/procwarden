//! File hashing with SHA256 and Blake3 algorithms.

use anyhow::Result;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HashAlgorithmType {
    Sha256,
    Blake3,
}

/// Trait for file hashing algorithms.
pub trait HashAlgorithm: Send + Sync {
    fn hash_file(&self, path: &Path) -> Result<String>;
    fn algorithm_type(&self) -> HashAlgorithmType;
}

/// SHA256 hasher implementation.
pub struct Sha256Hasher;

impl HashAlgorithm for Sha256Hasher {
    fn hash_file(&self, path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};

        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];

        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        Ok(hex::encode(hasher.finalize()))
    }

    fn algorithm_type(&self) -> HashAlgorithmType {
        HashAlgorithmType::Sha256
    }
}

/// Blake3 hasher implementation.
pub struct Blake3Hasher;

impl HashAlgorithm for Blake3Hasher {
    fn hash_file(&self, path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0u8; 8192];

        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        Ok(hasher.finalize().to_hex().to_string())
    }

    fn algorithm_type(&self) -> HashAlgorithmType {
        HashAlgorithmType::Blake3
    }
}

/// Hash a file with both SHA256 and Blake3.
/// Returns (sha256_hex, blake3_hex).
pub fn hash_file_dual(path: &Path) -> Result<(String, String)> {
    let sha256 = Sha256Hasher.hash_file(path)?;
    let blake3 = Blake3Hasher.hash_file(path)?;
    Ok((sha256, blake3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn create_test_file(content: &[u8]) -> std::path::PathBuf {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("test_hash_{}.bin", rand::random::<u32>()));
        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(content).unwrap();
        file_path
    }

    fn cleanup_test_file(path: &std::path::PathBuf) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sha256_hasher_returns_64_char_hex() {
        let content = b"test data";
        let file_path = create_test_file(content);

        let hasher = Sha256Hasher;
        let hash = hasher.hash_file(&file_path).unwrap();

        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        cleanup_test_file(&file_path);
    }

    #[test]
    fn blake3_hasher_returns_64_char_hex() {
        let content = b"test data";
        let file_path = create_test_file(content);

        let hasher = Blake3Hasher;
        let hash = hasher.hash_file(&file_path).unwrap();

        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        cleanup_test_file(&file_path);
    }

    #[test]
    fn hash_file_dual_returns_both_hashes() {
        let content = b"test data";
        let file_path = create_test_file(content);

        let (sha256, blake3) = hash_file_dual(&file_path).unwrap();

        assert_eq!(sha256.len(), 64);
        assert_eq!(blake3.len(), 64);
        assert_ne!(sha256, blake3); // Different algorithms should produce different hashes

        cleanup_test_file(&file_path);
    }

    #[test]
    fn hashing_same_file_twice_yields_same_result() {
        let content = b"test data";
        let file_path = create_test_file(content);

        let hash1 = Sha256Hasher.hash_file(&file_path).unwrap();
        let hash2 = Sha256Hasher.hash_file(&file_path).unwrap();

        assert_eq!(hash1, hash2);

        cleanup_test_file(&file_path);
    }

    #[test]
    fn hashing_different_files_yields_different_hashes() {
        let file1_path = create_test_file(b"data1");
        let file2_path = create_test_file(b"data2");

        let hash1 = Sha256Hasher.hash_file(&file1_path).unwrap();
        let hash2 = Sha256Hasher.hash_file(&file2_path).unwrap();

        assert_ne!(hash1, hash2);

        cleanup_test_file(&file1_path);
        cleanup_test_file(&file2_path);
    }

    #[test]
    fn error_handling_for_missing_file() {
        let hasher = Sha256Hasher;
        let result = hasher.hash_file(&PathBuf::from("nonexistent_file_12345.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn sha256_algorithm_type_correct() {
        let hasher = Sha256Hasher;
        assert_eq!(hasher.algorithm_type(), HashAlgorithmType::Sha256);
    }

    #[test]
    fn blake3_algorithm_type_correct() {
        let hasher = Blake3Hasher;
        assert_eq!(hasher.algorithm_type(), HashAlgorithmType::Blake3);
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn fuzz_hash_file_dual_no_panic(content in prop::collection::vec(any::<u8>(), 0..1024)) {
            let temp_dir = std::env::temp_dir();
            let file_path = temp_dir.join(format!("fuzz_test_{}.bin", rand::random::<u32>()));

            if let Ok(mut file) = std::fs::File::create(&file_path) {
                use std::io::Write;
                let _ = file.write_all(&content);
                drop(file);

                // Should not panic
                let _ = hash_file_dual(&file_path);

                let _ = std::fs::remove_file(&file_path);
            }
        }
    }
}
