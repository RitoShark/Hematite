//! Big "HEMATITE" splash banner printed when the CLI is invoked in
//! interactive / drag-drop mode.
//!
//! Uses ANSI 256-colour escapes via the `colored` crate (already a CLI
//! dep). Windows ANSI virtual-terminal mode is flipped on in
//! [`crate::main`] before any output, so the escapes render correctly
//! on cmd.exe / Windows Terminal.

use colored::Colorize;

/// Block-letter "HEMATITE" — generated once, embedded verbatim.
///
/// Rows kept as separate `&str`s and joined with explicit `\n` so the
/// banner is invariant against editor / formatter trailing-whitespace
/// stripping. Each row is the same printable width — the trailing
/// double-space on rows 3 and 4 fills out the last `E` so the banner
/// is a clean rectangle.
const BANNER_ROWS: &[&str] = &[
    " ██╗  ██╗███████╗███╗   ███╗ █████╗ ████████╗██╗████████╗███████╗",
    " ██║  ██║██╔════╝████╗ ████║██╔══██╗╚══██╔══╝██║╚══██╔══╝██╔════╝",
    " ███████║█████╗  ██╔████╔██║███████║   ██║   ██║   ██║   █████╗  ",
    " ██╔══██║██╔══╝  ██║╚██╔╝██║██╔══██║   ██║   ██║   ██║   ██╔══╝  ",
    " ██║  ██║███████╗██║ ╚═╝ ██║██║  ██║   ██║   ██║   ██║   ███████╗",
    " ╚═╝  ╚═╝╚══════╝╚═╝     ╚═╝╚═╝  ╚═╝   ╚═╝   ╚═╝   ╚═╝   ╚══════╝",
];

const TAGLINE: &str = "League of Legends custom-skin fixer";

/// Print the splash to stderr (so it doesn't pollute JSON / piped stdout).
pub fn print() {
    eprintln!();
    for row in BANNER_ROWS {
        eprintln!("{}", row.bright_red().bold());
    }
    eprintln!();
    eprintln!(
        "  {}    {}",
        TAGLINE.bright_white(),
        format!("v{}", env!("CARGO_PKG_VERSION")).bright_black()
    );
    eprintln!(
        "  {} {}",
        "tip:".bright_black(),
        "drag a mod onto this exe to fix it instantly"
            .bright_black()
            .italic()
    );
    eprintln!();
}

/// Print a slim divider — useful between the banner and a prompt, or
/// between two prompts.
pub fn divider() {
    eprintln!("  {}", "─".repeat(64).bright_black());
}
