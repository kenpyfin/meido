//! Native in-process Gemini ADK multi-agent engine (ADK-style orchestration).

pub mod agent;
pub mod config;
pub mod profile;
mod runner;

pub use config::GeminiAdkSettings;
pub use profile::AdkTopologyProfile;
pub use runner::run_gemini_adk_engine;
