//! Turning a [`Layout`] into a single `xrandr` invocation.

use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::layout::Layout;

pub fn build_args(layout: &Layout) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    for m in &layout.monitors {
        args.extend(["--output".into(), m.name.clone()]);
        if !m.enabled {
            args.push("--off".into());
            continue;
        }
        args.extend(["--mode".into(), m.current_mode().id.clone()]);
        match &m.mirror_of {
            Some(src) => args.extend(["--same-as".into(), src.clone()]),
            None => args.extend(["--pos".into(), format!("{}x{}", m.x, m.y)]),
        }
        args.extend([
            "--rotate".into(),
            m.rotation.as_str().into(),
            "--scale".into(),
            format!("{0}x{0}", m.scale),
        ]);
        if m.primary {
            args.push("--primary".into());
        }
    }
    for name in &layout.stale {
        args.extend(["--output".into(), name.clone(), "--off".into()]);
    }
    args
}

pub fn format_command(args: &[String]) -> String {
    let mut cmd = String::from("xrandr");
    for a in args {
        cmd.push(' ');
        if a.starts_with("--") {
            cmd.push_str(a);
        } else {
            cmd.push_str(&a.replace(' ', "\\ "));
        }
    }
    cmd
}

pub fn apply(args: &[String]) -> Result<()> {
    let out = Command::new("xrandr")
        .args(args)
        .output()
        .context("failed to run xrandr")?;
    if !out.status.success() {
        bail!("xrandr failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xrandr::parse_verbose;

    const FIXTURE: &str = include_str!("../../tests/fixtures/verbose_edp_dp.txt");

    #[test]
    fn fixture_roundtrips_to_current_state() {
        let layout = Layout::from_outputs(&parse_verbose(FIXTURE).unwrap());
        assert_eq!(
            format_command(&build_args(&layout)),
            "xrandr \
             --output eDP --mode 0x5c --pos 0x1080 --rotate normal --scale 1x1 --primary \
             --output DisplayPort-1 --mode 0x3ec --pos 0x0 --rotate normal --scale 1x1"
        );
    }

    #[test]
    fn off_mirror_scale_and_stale() {
        let mut layout = Layout::from_outputs(&parse_verbose(FIXTURE).unwrap());
        layout.monitors[0].scale = 1.25;
        layout.monitors[1].mirror_of = Some("eDP".into());
        layout.stale.push("HDMI-A-0".into());
        let cmd = format_command(&build_args(&layout));
        assert!(cmd.contains("--output eDP --mode 0x5c --pos 0x1080 --rotate normal --scale 1.25x1.25"));
        assert!(cmd.contains("--output DisplayPort-1 --mode 0x3ec --same-as eDP --rotate normal"));
        assert!(cmd.ends_with("--output HDMI-A-0 --off"));

        layout.monitors[1].enabled = false;
        assert!(format_command(&build_args(&layout)).contains("--output DisplayPort-1 --off"));
    }
}
