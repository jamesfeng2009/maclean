//! CLI 契约（strategy §21/§47 + split `05_PUBLIC_CORE_CONTRACTS.md`）。

pub mod envelope;
pub mod exit_code;
pub mod output;

pub use envelope::CliEnvelope;
pub use exit_code::{
    EXIT_CANCELLED, EXIT_CONFIRM_REQUIRED, EXIT_FAILURE, EXIT_OK, EXIT_SERIALIZE, EXIT_WARNINGS,
};
pub use output::{ColorMode, OutputFormat};
