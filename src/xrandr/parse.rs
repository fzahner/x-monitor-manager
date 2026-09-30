//! Parser for `xrandr --verbose` output.
//!
//! Line shapes we care about (everything else is ignored):
//! ```text
//! eDP connected primary 2560x1600+0+1080 (0x5c) normal (normal left ...) 330mm x 200mm
//!     Transform:  1.000000 0.000000 0.000000
//!   2560x1600 (0x5c) 818.611MHz -HSync +VSync *current +preferred
//!         h: width  2560 start 2568 end 2600 total 2720 skew    0 clock 300.96KHz
//!         v: height 1600 start 4988 end 4996 total 5016           clock  60.00Hz
//! ```

use anyhow::{Context, Result, bail};

use super::{Mode, Output, Placement, Rotation};

pub fn parse_verbose(text: &str) -> Result<Vec<Output>> {
    let mut outputs: Vec<Output> = Vec::new();

    for (lineno, line) in text.lines().enumerate() {
        let ctx = || format!("line {}: {line:?}", lineno + 1);

        if line.starts_with("Screen ") || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            outputs.push(parse_header(line).with_context(ctx)?);
            continue;
        }
        let Some(output) = outputs.last_mut() else {
            continue;
        };
        let trimmed = line.trim_start();

        if let Some(rest) = trimmed.strip_prefix("Transform:") {
            let scale = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .with_context(ctx)?;
            if let Some(p) = output.placement.as_mut() {
                p.scale = scale;
            }
        } else if line.starts_with("  ") && !line.starts_with("   ") {
            output.modes.push(parse_mode_line(trimmed).with_context(ctx)?);
        } else if let Some(rest) = trimmed.strip_prefix("h:") {
            let mode = output.modes.last_mut().with_context(ctx)?;
            mode.width = field_after(rest, "width").with_context(ctx)?;
        } else if let Some(rest) = trimmed.strip_prefix("v:") {
            let mode = output.modes.last_mut().with_context(ctx)?;
            mode.height = field_after(rest, "height").with_context(ctx)?;
            let clock = rest
                .split_whitespace()
                .skip_while(|t| *t != "clock")
                .nth(1)
                .and_then(|t| t.strip_suffix("Hz"))
                .with_context(ctx)?;
            mode.refresh = clock.parse().with_context(ctx)?;
        }
    }

    Ok(outputs)
}

fn parse_header(line: &str) -> Result<Output> {
    let mut tokens = line.split_whitespace();
    let name = tokens.next().context("missing output name")?.to_string();
    let connected = tokens.next() == Some("connected");

    let mut primary = false;
    let mut geometry = None;
    let mut rotation = Rotation::Normal;
    for tok in tokens {
        if tok == "primary" {
            primary = true;
        } else if let Some(g) = parse_geometry(tok) {
            geometry = Some(g);
        } else if tok.starts_with("(0x") {
            // current mode id; the mode list marks it with *current too
        } else if tok.starts_with('(') {
            // start of the supported-rotations list; nothing useful after it
            break;
        } else if let Some(r) = Rotation::parse(tok) {
            rotation = r;
        }
    }

    Ok(Output {
        name,
        connected,
        primary,
        modes: Vec::new(),
        placement: geometry.map(|(x, y)| Placement {
            x,
            y,
            rotation,
            scale: 1.0,
        }),
    })
}

/// Parses `WxH+X+Y`, returning the position (size comes from the mode).
fn parse_geometry(tok: &str) -> Option<(i32, i32)> {
    let (size, pos) = tok.split_once('+')?;
    let (w, h) = size.split_once('x')?;
    w.parse::<u32>().ok()?;
    h.parse::<u32>().ok()?;
    let (x, y) = pos.split_once('+')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn parse_mode_line(line: &str) -> Result<Mode> {
    let id = line
        .split_whitespace()
        .find_map(|t| t.strip_prefix('(')?.strip_suffix(')'))
        .filter(|t| t.starts_with("0x"))
        .context("mode line without (0x..) id")?
        .to_string();
    Ok(Mode {
        id,
        width: 0,
        height: 0,
        refresh: 0.0,
        current: line.contains("*current"),
        preferred: line.contains("+preferred"),
    })
}

fn field_after(rest: &str, key: &str) -> Result<u32> {
    let value = rest
        .split_whitespace()
        .skip_while(|t| *t != key)
        .nth(1)
        .with_context(|| format!("missing `{key}`"))?;
    match value.parse() {
        Ok(v) => Ok(v),
        Err(_) => bail!("bad `{key}` value {value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/verbose_edp_dp.txt");

    #[test]
    fn parses_fixture_outputs() {
        let outs = parse_verbose(FIXTURE).unwrap();
        let names: Vec<_> = outs.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "eDP",
                "HDMI-A-0",
                "DisplayPort-0",
                "DisplayPort-1",
                "DisplayPort-2",
                "DisplayPort-3",
                "DisplayPort-4",
                "DisplayPort-5",
                "DisplayPort-6"
            ]
        );
        let connected: Vec<_> = outs.iter().filter(|o| o.connected).map(|o| &o.name).collect();
        assert_eq!(connected, ["eDP", "DisplayPort-1"]);
    }

    #[test]
    fn parses_edp_details() {
        let outs = parse_verbose(FIXTURE).unwrap();
        let edp = &outs[0];
        assert!(edp.primary);
        assert_eq!(
            edp.placement,
            Some(Placement { x: 0, y: 1080, rotation: Rotation::Normal, scale: 1.0 })
        );
        assert_eq!(edp.modes.len(), 13);
        let cur = &edp.modes[edp.current_mode().unwrap()];
        assert_eq!((cur.id.as_str(), cur.width, cur.height), ("0x5c", 2560, 1600));
        assert!(cur.preferred);
        assert_eq!(cur.refresh, 60.0);
        // same resolution, different refresh, different id
        assert_eq!(edp.modes[1].id, "0x5d");
        assert_eq!(edp.modes[1].refresh, 165.0);
    }

    #[test]
    fn parses_external_and_disconnected() {
        let outs = parse_verbose(FIXTURE).unwrap();
        let dp = outs.iter().find(|o| o.name == "DisplayPort-1").unwrap();
        assert!(!dp.primary);
        assert_eq!(dp.placement.as_ref().map(|p| (p.x, p.y)), Some((0, 0)));
        assert_eq!(dp.modes[1].refresh, 360.0);
        let hdmi = outs.iter().find(|o| o.name == "HDMI-A-0").unwrap();
        assert!(!hdmi.connected);
        assert!(hdmi.placement.is_none());
        assert!(hdmi.modes.is_empty());
    }

    #[test]
    fn parses_rotation_and_scale() {
        let text = "\
HDMI-1 connected 2160x3840+1920+0 (0x80) left (normal left inverted right x axis y axis) 600mm x 340mm
\tTransform:  1.500000 0.000000 0.000000
\t            0.000000 1.500000 0.000000
\t            0.000000 0.000000 1.000000
  2560x1440 (0x80) 241.500MHz +HSync -VSync *current
        h: width  2560 start 2608 end 2640 total 2720 skew    0 clock  88.79KHz
        v: height 1440 start 1443 end 1448 total 1481           clock  59.95Hz
HDMI-2 connected (normal left inverted right x axis y axis)
";
        let outs = parse_verbose(text).unwrap();
        let p = outs[0].placement.as_ref().unwrap();
        assert_eq!((p.x, p.y, p.rotation, p.scale), (1920, 0, Rotation::Left, 1.5));
        assert_eq!(outs[0].modes[0].refresh, 59.95);
        assert!(outs[1].connected);
        assert!(outs[1].placement.is_none());
    }
}
