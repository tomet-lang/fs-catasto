//! Shared configuration and formatting utilities for specification generators.

use std::path::{Path, PathBuf};

use tomet_spec::PrinterConfig;

/// Resolves the [`PrinterConfig`] governing the output target.
pub fn config_for_output(out: Option<&Path>) -> PrinterConfig {
    let probe: PathBuf = out.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    tomet_load_config::find_config_file(&probe)
        .map_or_else(PrinterConfig::default, |(config, _, _)| config)
}
