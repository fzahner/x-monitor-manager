//! Everything that talks to the `xrandr` binary: reading state and applying layouts.

mod apply;
mod parse;

pub use apply::{apply, build_args, format_command};
pub use parse::parse_verbose;

use std::process::Command;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    /// XID as printed by xrandr, e.g. `0x5c`. Passed to `--mode` so that
    /// modes sharing a resolution but differing in refresh stay distinct.
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
    pub current: bool,
    pub preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    Normal,
    Left,
    Inverted,
    Right,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [Self::Normal, Self::Left, Self::Inverted, Self::Right];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Left => "left",
            Self::Inverted => "inverted",
            Self::Right => "right",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// Whether width and height are swapped on screen.
    pub fn is_sideways(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % 4]
    }
}

/// Where an enabled output currently sits in the framebuffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub rotation: Rotation,
    pub scale: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub connected: bool,
    pub primary: bool,
    pub modes: Vec<Mode>,
    /// `None` when the output is disabled.
    pub placement: Option<Placement>,
}

impl Output {
    pub fn current_mode(&self) -> Option<usize> {
        self.modes.iter().position(|m| m.current)
    }
}

pub fn query() -> Result<Vec<Output>> {
    let out = Command::new("xrandr")
        .arg("--verbose")
        .output()
        .context("failed to run `xrandr --verbose` (is xrandr installed and DISPLAY set?)")?;
    if !out.status.success() {
        bail!(
            "`xrandr --verbose` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_verbose(&String::from_utf8_lossy(&out.stdout))
}
