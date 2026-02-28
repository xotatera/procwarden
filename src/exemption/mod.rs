//! PriorityGuard exemption system with path+hash verification.
//!
//! This module provides a clean architecture for managing process exemptions
//! with cryptographic hash verification (SHA256 + Blake3).

pub mod domain;
pub mod hash;
pub mod persistence;
pub mod picker_state;
pub mod verification;

// Re-export public API
pub use domain::{Exemption, ExemptionList, ExemptionMatcher};
pub use hash::{hash_file_dual, Blake3Hasher, HashAlgorithm, HashAlgorithmType, Sha256Hasher};
pub use persistence::{from_persisted_format, to_persisted_format, ExemptionData};
pub use picker_state::{ExemptionPickerState, PickerTab, ProcessCandidate};
pub use verification::{VerificationResult, VerificationService};
