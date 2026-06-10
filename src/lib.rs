pub mod cli;
pub mod config;
pub mod generate;
pub mod generators;
pub mod introspection;
pub mod lifecycle;
pub mod mcp;
pub mod output;
pub mod resolver;
pub mod scenario;
pub mod semantic;

pub use generate::{
    generate, GenerateConfig, GenerateError, GenerationResult, OutputMode, TableResult,
};
pub use scenario::ScenarioConfig;
