use crate::common::ProcessInfo;
use anyhow::Result;

pub mod polling;

#[cfg(feature = "etw")]
pub mod etw;

#[cfg(feature = "etw")]
pub use etw::EtwDataSource;
pub use polling::PollingDataSource;

/// Trait for process data sources (polling, ETW, etc.)
pub trait DataSource {
    /// Fetch current list of processes
    fn get_processes(&mut self) -> Result<Vec<ProcessInfo>>;
}
