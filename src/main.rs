mod app;
mod layout;
mod ui;
mod xrandr;

use anyhow::{Result, bail};

use crate::app::App;
use crate::layout::Layout;

const USAGE: &str = "\
xmm - arrange X11 monitors from the terminal

Usage: xmm [--dry-run]

  --dry-run   show the xrandr command on apply instead of running it
              (the last one is printed on exit)";

fn main() -> Result<()> {
    let mut dry_run = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" | "-n" => dry_run = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown argument {other:?}\n\n{USAGE}"),
        }
    }

    let layout = Layout::from_outputs(&xrandr::query()?);
    if layout.monitors.is_empty() {
        bail!("xrandr reports no connected outputs");
    }

    let app = ratatui::run(|terminal| App::new(layout, dry_run).run(terminal))?;
    if let Some(cmd) = app.last_command {
        println!("{cmd}");
    }
    Ok(())
}
