//! Lifecycle simulation — time-travel data generation.
//!
//! This module is an additive layer over the existing generation core: it is
//! active only when a scenario declares a `lifecycle:` block. See `LIFECYCLE.md`
//! for the full specification.

pub mod churn;
pub mod config;
pub mod engine;
pub mod growth;
pub mod pool;
pub mod seasonality;
pub mod temporal;
pub mod timeline;

pub use churn::{ChurnEvent, ChurnModel};
pub use config::{BucketGranularity, LifecycleConfig, TimeBucket, TrackedEntity};
pub use engine::{BucketPlan, LifecycleEngine, SimulationReport, TableBucketStat, TableLifecycle};
pub use growth::GrowthModel;
pub use pool::EntityPool;
pub use seasonality::{SeasonalityKind, SeasonalityModel};
pub use temporal::{DurationRange, TemporalConstraint};
pub use timeline::TimelineDistribution;
