use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "catasto",
    version,
    about = "Filesystem schema, manifest, and lockfile tool"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check the directory tree against its schema and lockfile.
    Check {
        /// Workspace root. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}
