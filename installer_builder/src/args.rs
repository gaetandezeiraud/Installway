use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about = "Build installer .exe with embedded payload")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate an Ed25519 signing keypair.
    Keygen(KeygenArgs),
    /// Build an installer .exe with an embedded payload.
    Pack(PackArgs),
}

#[derive(clap::Args, Debug)]
pub struct KeygenArgs {
    /// Output directory for `priv.key` + `pub.key` (hex-encoded).
    #[arg(short, long)]
    pub out: PathBuf,
}

#[derive(clap::Args, Debug, Clone)]
pub struct PackArgs {
    /// Product name (key).
    #[arg(short, long)]
    pub product: String,

    /// Publisher / vendor name (mandatory). Used for the per-user uninstall
    /// data folder %LOCALAPPDATA%\<publisher>\Uninstall\<product> and the
    /// Add/Remove Programs "Publisher" field.
    #[arg(long)]
    pub publisher: String,

    /// New version string (e.g. "1.0.1").
    #[arg(long)]
    pub to_version: String,

    /// Source dir containing the new version files.
    #[arg(long)]
    pub input: PathBuf,

    /// Previous version dir (for patch mode).
    #[arg(long)]
    pub from_dir: Option<PathBuf>,

    /// Previous version string (for patch mode).
    #[arg(long)]
    pub from_version: Option<String>,

    /// Main executable path relative to product root (e.g. "game.exe").
    #[arg(short, long)]
    pub exe: String,

    /// Optional path to a UTF-8 license text file shown on the License page.
    /// If omitted, the installer uses a built-in lorem-ipsum placeholder.
    #[arg(long)]
    pub license: Option<PathBuf>,

    /// File association, format `.ext:Description`. Repeatable.
    /// e.g. --assoc ".myx:My App Document" --assoc ".myz:My App Archive"
    #[arg(long = "assoc", value_name = ".ext:Description")]
    pub assoc: Vec<String>,

    /// Minimum installer binary version allowed to install this payload.
    #[arg(long, default_value = "1.0.0")]
    pub min_installer_version: String,

    /// Dev: reinstall from scratch. Skips the patch from-version check,
    /// re-writes every file (no hash-skip), and removes any existing files
    /// that aren't part of this build. Intended for development installers.
    #[arg(long)]
    pub force_reinstall: bool,

    /// Path to the Ed25519 private key file.
    #[arg(long)]
    pub priv_key: PathBuf,

    /// Path to the Ed25519 public key file. Required only in toolchain mode
    /// (it gets compiled into a freshly built stub). Not needed with
    /// `--installer-stub`, since a prebuilt stub already has the key baked in.
    #[arg(long)]
    pub pub_key: Option<PathBuf>,

    /// Prebuilt installer stub (`installer.exe`) with the public key already
    /// compiled in. When given, no Rust toolchain is needed: the builder
    /// reuses this binary instead of running `cargo build`. Requires
    /// `--uninstaller` too.
    #[arg(long)]
    pub installer_stub: Option<PathBuf>,

    /// Prebuilt uninstaller (`uninstall.exe`). Used together with
    /// `--installer-stub` for toolchain-free packaging.
    #[arg(long)]
    pub uninstaller: Option<PathBuf>,

    /// Output installer .exe path.
    #[arg(short, long)]
    pub out: PathBuf,

    /// Skip rebuilding installer crate if the stub already exists
    /// (toolchain mode only).
    #[arg(long)]
    pub reuse_stub: bool,
}
