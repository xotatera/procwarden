use anyhow::Result;
use crate::common::ProcessInfo;

pub mod polling;

#[cfg(feature = "etw")]
pub mod etw;

pub use polling::PollingDataSource;
#[cfg(feature = "etw")]
pub use etw::EtwDataSource;

/// Trait for process data sources (polling, ETW, etc.)
pub trait DataSource {
    /// Fetch current list of processes
    fn get_processes(&mut self) -> Result<Vec<ProcessInfo>>;
}
