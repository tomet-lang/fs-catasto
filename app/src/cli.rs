use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "catasto",
    version,
    about = "Filesystem schema, manifest, and lockfile tool",
    long_about = "catasto keeps the files a workspace takes from elsewhere -- a vocabulary, a \
                  shared schema -- where its manifest says, each pinned to a commit and a \
                  SHA-256.\n\n\
                  Start with `catasto init`, name a place with `catasto upstream add`, take a \
                  file from it with `catasto add`, and let `catasto check` say whether anything \
                  has drifted."
)]
pub struct Cli {
    /// The workspace root. Defaults to the current directory.
    ///
    /// `check`, `lock` and `sync` also take it as a trailing path, like the
    /// other tools of the ecosystem. (`upstream add --root` is something else:
    /// a directory inside the repository it names.)
    #[arg(short = 'C', long = "workspace", global = true, value_name = "DIR")]
    pub workspace: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create catasto.ctmt, a manifest that declares nothing yet.
    Init,
    /// Take one file from a place: declare it, fetch it and lock it.
    ///
    /// The other files stay where they are. If the fetch fails the manifest is
    /// put back, so a failed `add` leaves nothing behind.
    ///
    /// Refuses with exit code 3 if DEST is already declared; use --force to
    /// replace its source.
    #[command(after_help = "\
Examples:
  catasto add .envrc --upstream commonfiles
  catasto add docs/terms.tmt --upstream shared terms.tmt
  catasto add schema/writ.tmt --git https://example.com/v.git --rev v1 writ/writ.tmt
  catasto add notes/a.tmt --local ../shared/a.tmt")]
    Add(AddArgs),
    /// Stop managing a file: undeclare it and forget what the lock says.
    ///
    /// The file stays on disk unless --delete is given.
    Remove {
        /// The file, as the manifest writes its path.
        dest: String,
        /// Delete the file too.
        #[arg(long)]
        delete: bool,
    },
    /// Name the places files are taken from.
    #[command(subcommand)]
    Upstream(UpstreamCommand),
    /// Show every declared file: where it comes from, and how it stands.
    List {
        /// Print JSON, for tools.
        #[arg(long)]
        json: bool,
    },
    /// Check the directory tree against its schema and lockfile.
    ///
    /// Exits 1 when a file has drifted, and says which command puts it right.
    /// It never touches the network.
    Check {
        /// Workspace root. Defaults to the current directory.
        path: Option<PathBuf>,
    },
    /// Record the sha256 of every managed file in catasto.lock.
    Lock {
        /// Workspace root. Defaults to the current directory.
        path: Option<PathBuf>,
    },
    /// Re-fetch managed files from their sources and rewrite catasto.lock.
    ///
    /// Git sources are read at the commit recorded in catasto.lock.
    Sync {
        /// Workspace root. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Resolve every git rev again and lock the commits they point at now.
        #[arg(long)]
        update: bool,
        /// Only this file (as the manifest writes its path); repeat for more.
        /// The others, and what the lock says about them, stay as they are.
        #[arg(long, value_name = "FILE")]
        only: Vec<PathBuf>,
        /// Compute what sync would write and print the diff and summary without
        /// writing any files or rewriting the lock. May fetch new git commits
        /// into the cache.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show differences between the workspace files and what sync would write.
    ///
    /// Reads git sources at the commit recorded in catasto.lock and may touch the
    /// network when a commit is not cached yet (unlike check, which is offline).
    Diff {
        /// Workspace root. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Only this file (as the manifest writes its path); repeat for more.
        #[arg(long, value_name = "FILE")]
        only: Vec<PathBuf>,
    },
    /// Generate automated .tmt specifications and reference documentation.
    Spec {
        /// Documentation target to generate (`cli` | `manifest`).
        #[arg(value_name = "TARGET")]
        target: Option<crate::commands::spec::SpecTarget>,
        /// Generate all declared specification targets. Requires `--out <DIR>`.
        #[arg(long, conflicts_with = "target", requires = "out")]
        all: bool,
        /// Write to this file or directory instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Fail if output files are missing or out of date, without writing.
        #[arg(long, requires = "out")]
        check: bool,
    },
}

#[derive(Args)]
#[command(group(
    clap::ArgGroup::new("kind")
        .required(true)
        .args(["upstream", "git", "local"])
))]
pub struct AddArgs {
    /// Where the file goes, relative to the workspace root.
    pub dest: String,
    /// Where the file is in the place it comes from: a path inside the
    /// upstream, inside the repository, or on disk with --local. Defaults to
    /// DEST (except with --local, which requires the path).
    pub from: Option<String>,
    /// Take it from a place named in `::upstreams`.
    #[arg(long, value_name = "NAME")]
    pub upstream: Option<String>,
    /// Take it from this git repository (`https://` or a local path).
    #[arg(long, value_name = "URL")]
    pub git: Option<String>,
    /// Take it from a file on disk, relative to the workspace root.
    #[arg(long)]
    pub local: bool,
    /// The tag, branch or commit to read, with --git.
    #[arg(long, default_value = "main", requires = "git")]
    pub rev: String,
    /// Place an upstream symlink as a symlink rather than following it.
    #[arg(long)]
    pub keep_symlink: bool,
    /// Declare DEST with the given source whether or not it is declared.
    #[arg(long)]
    pub force: bool,
}

#[derive(Subcommand)]
pub enum UpstreamCommand {
    /// Name a git repository, so entries can say `upstream: NAME`.
    Add {
        /// The name, letters, digits, `_` and `-`.
        name: String,
        /// An `https://` URL, or a local path relative to the workspace root.
        git: String,
        /// The tag, branch or commit to read.
        #[arg(long, default_value = "main")]
        rev: String,
        /// The directory of the repository that the entries' paths start from.
        #[arg(long)]
        root: Option<String>,
        /// Declare NAME with the given definition whether or not it is declared.
        #[arg(long)]
        force: bool,
    },
    /// Drop a name; refused while a file still uses it.
    Remove {
        /// The name.
        name: String,
    },
    /// List the names and what they point at.
    List,
}
