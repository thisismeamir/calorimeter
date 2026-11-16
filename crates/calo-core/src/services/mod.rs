//! Built-in and expression-defined analysis services (`calo-core::services`, spec §12).

pub mod builtins;
pub mod error;
pub mod frame;
pub mod registry;

pub use builtins::{
    CacheMissRateService, ImbalanceService, IpcService, ParallelEfficiencyService, SpeedupService,
    StatsSummaryService,
};
pub use error::ServiceError;
pub use frame::{Column, Frame};
pub use registry::{Options, ServiceLayer, ServiceRegistry};

use calo_schema::Tier;

/// Core transformation trait for Calorimeter post-processing services (spec §12.2).
pub trait Service: Send + Sync {
    /// Unique service identifier (e.g. `"stats.summary"`, `"ipc"`).
    fn name(&self) -> &'static str;
    /// Input column/metric names consumed by this service.
    fn inputs(&self) -> &'static [&'static str];
    /// Output column/metric names produced by this service.
    fn produces(&self) -> &'static [&'static str];
    /// Capability tiers required by this service.
    fn requires(&self) -> &'static [Tier];
    /// Executes the service transformation on `table` using `opts`.
    fn run(&self, table: &Frame, opts: &Options) -> Result<Frame, ServiceError>;
}
