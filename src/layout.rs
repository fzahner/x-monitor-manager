//! Pure geometry model of the monitor arrangement being edited.
//!
//! Positions are only ever changed through snap stops (see [`Layout::move_monitor`]),
//! so the arrangement stays gap- and overlap-free as long as it started that way.

use crate::xrandr::{Mode, Output, Rotation};

pub const SCALES: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w
    }

    fn bottom(&self) -> i32 {
        self.y + self.h
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// Overlapping, or sharing an edge segment of non-zero length.
    /// Touching only at a corner does not count.
    fn adjacent(&self, o: &Rect) -> bool {
        let closed = self.x <= o.right()
            && o.x <= self.right()
            && self.y <= o.bottom()
            && o.y <= self.bottom();
        let corner_x = self.right() == o.x || o.right() == self.x;
        let corner_y = self.bottom() == o.y || o.bottom() == self.y;
        closed && !(corner_x && corner_y)
    }

    /// Doubled center, to stay in integers.
    fn center2(&self) -> (i64, i64) {
        (
            2 * self.x as i64 + self.w as i64,
            2 * self.y as i64 + self.h as i64,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub modes: Vec<Mode>,
    pub enabled: bool,
    pub primary: bool,
    /// Index into `modes`.
    pub mode: usize,
    pub rotation: Rotation,
    pub scale: f64,
    pub x: i32,
    pub y: i32,
    /// When set, this monitor shows the same content as the named one and
    /// takes no part in the geometry.
    pub mirror_of: Option<String>,
}

impl Monitor {
    pub fn current_mode(&self) -> &Mode {
        &self.modes[self.mode]
    }

    pub fn size(&self) -> (i32, i32) {
        let m = self.current_mode();
        let (w, h) = if self.rotation.is_sideways() {
            (m.height, m.width)
        } else {
            (m.width, m.height)
        };
        (
            (w as f64 * self.scale).round() as i32,
            (h as f64 * self.scale).round() as i32,
        )
    }

    pub fn rect(&self) -> Rect {
        let (w, h) = self.size();
        Rect { x: self.x, y: self.y, w, h }
    }

    /// Distinct resolutions in xrandr's order (largest first, in practice).
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut res: Vec<(u32, u32)> = Vec::new();
        for m in &self.modes {
            if !res.contains(&(m.width, m.height)) {
                res.push((m.width, m.height));
            }
        }
        res
    }

    /// Mode indices with the current resolution, highest refresh first.
    pub fn refresh_options(&self) -> Vec<usize> {
        let cur = self.current_mode();
        self.modes_at(cur.width, cur.height)
    }

    fn modes_at(&self, w: u32, h: u32) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.modes.len())
            .filter(|&i| self.modes[i].width == w && self.modes[i].height == h)
            .collect();
        idx.sort_by(|&a, &b| self.modes[b].refresh.total_cmp(&self.modes[a].refresh));
        idx
    }

    /// Mode index for the resolution `step` places away, keeping the refresh
    /// rate as close as possible to the current one.
    pub fn step_resolution(&self, step: isize) -> usize {
        let res = self.resolutions();
        let cur = self.current_mode();
        let pos = res
            .iter()
            .position(|&r| r == (cur.width, cur.height))
            .unwrap_or(0);
        let (w, h) = res[(pos as isize + step).rem_euclid(res.len() as isize) as usize];
        let refresh = cur.refresh;
        self.modes_at(w, h)
            .into_iter()
            .min_by(|&a, &b| {
                let da = (self.modes[a].refresh - refresh).abs();
                let db = (self.modes[b].refresh - refresh).abs();
                da.total_cmp(&db)
            })
            .expect("resolution list comes from modes")
    }

    pub fn step_refresh(&self, step: isize) -> usize {
        let opts = self.refresh_options();
        let pos = opts.iter().position(|&i| i == self.mode).unwrap_or(0);
        opts[(pos as isize + step).rem_euclid(opts.len() as isize) as usize]
    }

    pub fn next_scale(&self) -> f64 {
        SCALES
            .iter()
            .copied()
            .find(|&s| s > self.scale + 1e-9)
            .unwrap_or(SCALES[0])
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub monitors: Vec<Monitor>,
    /// Disconnected outputs that are still enabled; always turned off on apply.
    pub stale: Vec<String>,
}

impl Layout {
    pub fn from_outputs(outputs: &[Output]) -> Self {
        let mut layout = Layout::default();
        for o in outputs {
            if !o.connected || o.modes.is_empty() {
                if o.placement.is_some() {
                    layout.stale.push(o.name.clone());
                }
                continue;
            }
            let mode = o
                .current_mode()
                .or_else(|| o.modes.iter().position(|m| m.preferred))
                .unwrap_or(0);
            let (x, y, rotation, scale) = match &o.placement {
                Some(p) => (p.x, p.y, p.rotation, p.scale),
                None => (0, 0, Rotation::Normal, 1.0),
            };
            layout.monitors.push(Monitor {
                name: o.name.clone(),
                modes: o.modes.clone(),
                enabled: o.placement.is_some(),
                primary: o.primary,
                mode,
                rotation,
                scale,
                x,
                y,
                mirror_of: None,
            });
        }

        // An output exactly on top of an earlier one is a mirror of it.
        for i in 0..layout.monitors.len() {
            if !layout.monitors[i].enabled {
                continue;
            }
            let r = layout.monitors[i].rect();
            let src = (0..i).find(|&j| {
                let m = &layout.monitors[j];
                m.enabled && m.mirror_of.is_none() && m.rect() == r
            });
            if let Some(j) = src {
                layout.monitors[i].mirror_of = Some(layout.monitors[j].name.clone());
            }
        }
        layout
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.monitors.iter().position(|m| m.name == name)
    }

    fn is_participant(&self, i: usize) -> bool {
        let m = &self.monitors[i];
        m.enabled && m.mirror_of.is_none()
    }

    /// Enabled, non-mirror monitors: the ones that make up the geometry.
    pub fn participants(&self) -> Vec<usize> {
        (0..self.monitors.len())
            .filter(|&i| self.is_participant(i))
            .collect()
    }

    fn rects_except(&self, skip: usize) -> Vec<Rect> {
        self.participants()
            .into_iter()
            .filter(|&j| j != skip)
            .map(|j| self.monitors[j].rect())
            .collect()
    }

    /// Moves monitor `i` to the nearest snap stop in `dir`. Returns false if
    /// there is none.
    pub fn move_monitor(&mut self, i: usize, dir: Dir) -> bool {
        if !self.is_participant(i) {
            return false;
        }
        let others = self.rects_except(i);
        if others.is_empty() {
            return false;
        }
        let cur = self.monitors[i].rect();
        let (w, h) = (cur.w, cur.h);
        let components_now = {
            let mut all = others.clone();
            all.push(cur);
            components(&all)
        };

        let mut xs = vec![cur.x];
        let mut ys = vec![cur.y];
        for o in &others {
            xs.extend([o.x - w, o.right(), o.x, o.right() - w, o.x + o.w / 2 - w / 2]);
            ys.extend([o.y - h, o.bottom(), o.y, o.bottom() - h, o.y + o.h / 2 - h / 2]);
        }
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();

        let mut best: Option<((i32, i32), (i32, i32))> = None;
        for &x in &xs {
            for &y in &ys {
                let (primary, cross) = match dir {
                    Dir::Right => (x - cur.x, (y - cur.y).abs()),
                    Dir::Left => (cur.x - x, (y - cur.y).abs()),
                    Dir::Down => (y - cur.y, (x - cur.x).abs()),
                    Dir::Up => (cur.y - y, (x - cur.x).abs()),
                };
                if primary <= 0 {
                    continue;
                }
                if best.is_some_and(|(score, _)| score <= (primary, cross)) {
                    continue;
                }
                let r = Rect { x, y, w, h };
                if others.iter().any(|o| o.overlaps(&r)) || !others.iter().any(|o| o.adjacent(&r)) {
                    continue;
                }
                let mut all = others.clone();
                all.push(r);
                if components(&all) > components_now {
                    continue;
                }
                best = Some(((primary, cross), (x, y)));
            }
        }

        let Some((_, (x, y))) = best else {
            return false;
        };
        self.monitors[i].x = x;
        self.monitors[i].y = y;
        self.normalize();
        true
    }

    /// Nearest enabled monitor in `dir`, by center distance.
    pub fn neighbor(&self, i: usize, dir: Dir) -> Option<usize> {
        let (cx, cy) = self.monitors[i].rect().center2();
        self.participants()
            .into_iter()
            .filter(|&j| j != i)
            .filter_map(|j| {
                let (jx, jy) = self.monitors[j].rect().center2();
                let (dx, dy) = (jx - cx, jy - cy);
                let (primary, cross) = match dir {
                    Dir::Right => (dx, dy.abs()),
                    Dir::Left => (-dx, dy.abs()),
                    Dir::Down => (dy, dx.abs()),
                    Dir::Up => (-dy, dx.abs()),
                };
                (primary > 0).then_some((primary + 2 * cross, j))
            })
            .min()
            .map(|(_, j)| j)
    }

    /// Applies a size-changing edit to monitor `i`, keeping its top-left fixed
    /// and pushing monitors that lie entirely right of / below it.
    pub fn resize(&mut self, i: usize, edit: impl FnOnce(&mut Monitor)) {
        let old = self.monitors[i].rect();
        edit(&mut self.monitors[i]);
        let new = self.monitors[i].rect();
        if self.is_participant(i) {
            let (dw, dh) = (new.w - old.w, new.h - old.h);
            for j in self.participants() {
                if j == i {
                    continue;
                }
                let r = self.monitors[j].rect();
                if r.x >= old.right() {
                    self.monitors[j].x += dw;
                }
                if r.y >= old.bottom() {
                    self.monitors[j].y += dh;
                }
            }
        }
        self.normalize();
    }

    pub fn toggle_enabled(&mut self, i: usize) -> Result<(), String> {
        if self.monitors[i].enabled {
            if self.monitors.iter().filter(|m| m.enabled).count() == 1 {
                return Err("can't disable the only enabled output".into());
            }
            self.monitors[i].enabled = false;
            self.monitors[i].mirror_of = None;
            self.monitors[i].primary = false;
            let name = self.monitors[i].name.clone();
            for j in 0..self.monitors.len() {
                if self.monitors[j].mirror_of.as_deref() == Some(name.as_str()) {
                    self.monitors[j].mirror_of = None;
                    self.place_at_end(j);
                }
            }
        } else {
            self.monitors[i].enabled = true;
            self.monitors[i].mirror_of = None;
            self.place_at_end(i);
        }
        self.normalize();
        Ok(())
    }

    /// Puts monitor `i` right of the rightmost other monitor, top-aligned.
    fn place_at_end(&mut self, i: usize) {
        let rightmost = self
            .rects_except(i)
            .into_iter()
            .max_by_key(|r| (r.right(), std::cmp::Reverse(r.y)));
        let (x, y) = rightmost.map_or((0, 0), |r| (r.right(), r.y));
        self.monitors[i].x = x;
        self.monitors[i].y = y;
    }

    pub fn set_primary(&mut self, i: usize) -> Result<(), String> {
        if !self.monitors[i].enabled {
            return Err("only an enabled output can be primary".into());
        }
        for (j, m) in self.monitors.iter_mut().enumerate() {
            m.primary = j == i;
        }
        Ok(())
    }

    /// Cycles monitor `i` through: not mirroring → mirror of each other
    /// enabled monitor → not mirroring.
    pub fn cycle_mirror(&mut self, i: usize) -> Result<(), String> {
        let name = self.monitors[i].name.clone();
        if !self.monitors[i].enabled {
            return Err("enable the output first".into());
        }
        if let Some(j) = self
            .monitors
            .iter()
            .position(|m| m.mirror_of.as_deref() == Some(name.as_str()))
        {
            return Err(format!("{} is mirroring {name}", self.monitors[j].name));
        }

        let sources: Vec<usize> = self
            .participants()
            .into_iter()
            .filter(|&j| j != i)
            .collect();
        let next = match &self.monitors[i].mirror_of {
            None => sources.first().copied(),
            Some(cur) => {
                let pos = sources.iter().position(|&j| &self.monitors[j].name == cur);
                pos.and_then(|p| sources.get(p + 1).copied())
            }
        };

        match next {
            None => {
                if self.monitors[i].mirror_of.take().is_some() {
                    self.place_at_end(i);
                    self.normalize();
                }
                Ok(())
            }
            Some(src) => self.mirror(i, src),
        }
    }

    /// Makes `i` mirror `src`, switching both to their largest shared resolution.
    fn mirror(&mut self, i: usize, src: usize) -> Result<(), String> {
        let theirs = self.monitors[src].resolutions();
        let (w, h) = self.monitors[i]
            .resolutions()
            .into_iter()
            .filter(|r| theirs.contains(r))
            .max_by_key(|&(w, h)| w as u64 * h as u64)
            .ok_or_else(|| {
                format!(
                    "{} and {} share no resolution",
                    self.monitors[i].name, self.monitors[src].name
                )
            })?;

        let src_mode = self.monitors[src].current_mode();
        if (src_mode.width, src_mode.height) != (w, h) {
            let m = self.monitors[src].modes_at(w, h)[0];
            self.resize(src, |mon| mon.mode = m);
        }
        let (rotation, scale) = (self.monitors[src].rotation, self.monitors[src].scale);
        let src_name = self.monitors[src].name.clone();
        let mon = &mut self.monitors[i];
        mon.mode = mon.modes_at(w, h)[0];
        mon.rotation = rotation;
        mon.scale = scale;
        mon.mirror_of = Some(src_name);
        mon.primary = false;
        self.normalize();
        Ok(())
    }

    /// Checks the layout is something we're willing to hand to xrandr.
    pub fn validate(&self) -> Result<(), String> {
        if !self.monitors.iter().any(|m| m.enabled) {
            return Err("at least one output must be enabled".into());
        }
        let parts = self.participants();
        for (a, &i) in parts.iter().enumerate() {
            for &j in &parts[a + 1..] {
                if self.monitors[i].rect().overlaps(&self.monitors[j].rect()) {
                    return Err(format!(
                        "{} overlaps {}",
                        self.monitors[i].name, self.monitors[j].name
                    ));
                }
            }
        }
        let rects: Vec<Rect> = parts.iter().map(|&i| self.monitors[i].rect()).collect();
        if components(&rects) > 1 {
            return Err("monitors must touch each other (there's a gap)".into());
        }
        Ok(())
    }

    /// Shifts the geometry so it starts at (0, 0) and syncs mirror positions.
    fn normalize(&mut self) {
        let parts = self.participants();
        let min_x = parts.iter().map(|&i| self.monitors[i].x).min().unwrap_or(0);
        let min_y = parts.iter().map(|&i| self.monitors[i].y).min().unwrap_or(0);
        for &i in &parts {
            self.monitors[i].x -= min_x;
            self.monitors[i].y -= min_y;
        }
        for i in 0..self.monitors.len() {
            let Some(src) = self.monitors[i].mirror_of.clone() else {
                continue;
            };
            if let Some(j) = self.index_of(&src) {
                self.monitors[i].x = self.monitors[j].x;
                self.monitors[i].y = self.monitors[j].y;
            }
        }
    }
}

/// Number of connected groups, where rects connect through [`Rect::adjacent`].
fn components(rects: &[Rect]) -> usize {
    let mut seen = vec![false; rects.len()];
    let mut count = 0;
    for start in 0..rects.len() {
        if seen[start] {
            continue;
        }
        count += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(a) = stack.pop() {
            for b in 0..rects.len() {
                if !seen[b] && rects[a].adjacent(&rects[b]) {
                    seen[b] = true;
                    stack.push(b);
                }
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(id: &str, w: u32, h: u32, refresh: f64) -> Mode {
        Mode {
            id: id.into(),
            width: w,
            height: h,
            refresh,
            current: false,
            preferred: false,
        }
    }

    fn mon(name: &str, modes: Vec<Mode>, x: i32, y: i32) -> Monitor {
        Monitor {
            name: name.into(),
            modes,
            enabled: true,
            primary: false,
            mode: 0,
            rotation: Rotation::Normal,
            scale: 1.0,
            x,
            y,
            mirror_of: None,
        }
    }

    fn pos(l: &Layout, name: &str) -> (i32, i32) {
        let m = &l.monitors[l.index_of(name).unwrap()];
        (m.x, m.y)
    }

    /// A: 2560x1440 at origin, B: 1920x1080 to its right, top-aligned.
    fn a_b() -> Layout {
        Layout {
            monitors: vec![
                mon("A", vec![mode("0x1", 2560, 1440, 60.0)], 0, 0),
                mon(
                    "B",
                    vec![mode("0x2", 1920, 1080, 60.0), mode("0x3", 1280, 720, 60.0)],
                    2560,
                    0,
                ),
            ],
            stale: vec![],
        }
    }

    #[test]
    fn j_steps_through_alignments_then_below() {
        let mut l = a_b();
        assert!(l.move_monitor(1, Dir::Down));
        assert_eq!(pos(&l, "B"), (2560, 180), "center-aligned");
        assert!(l.move_monitor(1, Dir::Down));
        assert_eq!(pos(&l, "B"), (2560, 360), "bottom-aligned");
        assert!(l.move_monitor(1, Dir::Down));
        assert_eq!(pos(&l, "B"), (640, 1440), "below A, right-aligned");
        assert!(!l.move_monitor(1, Dir::Down));
        assert!(l.validate().is_ok());
    }

    #[test]
    fn h_steps_along_bottom_then_to_left_side() {
        let mut l = a_b();
        for _ in 0..3 {
            l.move_monitor(1, Dir::Down);
        }
        assert!(l.move_monitor(1, Dir::Left));
        assert_eq!(pos(&l, "B"), (320, 1440), "centered below A");
        assert!(l.move_monitor(1, Dir::Left));
        assert_eq!(pos(&l, "B"), (0, 1440), "left-aligned below A");
        assert!(l.move_monitor(1, Dir::Left));
        // B moved left of A (bottom-aligned, closest to where it was);
        // normalization shifts everything so B starts at x=0.
        assert_eq!(pos(&l, "B"), (0, 360));
        assert_eq!(pos(&l, "A"), (1920, 0));
        assert!(l.validate().is_ok());
    }

    #[test]
    fn moving_a_single_monitor_does_nothing() {
        let mut l = a_b();
        l.monitors[1].enabled = false;
        assert!(!l.move_monitor(0, Dir::Right));
    }

    #[test]
    fn move_never_disconnects_a_bridge() {
        // A | B | C in a row; B is the bridge between A and C.
        let m = |n, x| mon(n, vec![mode("0x1", 1000, 1000, 60.0)], x, 0);
        let mut l = Layout {
            monitors: vec![m("A", 0), m("B", 1000), m("C", 2000)],
            stale: vec![],
        };
        // Every stop below would leave A and C apart, so the move is refused.
        assert!(!l.move_monitor(1, Dir::Down));
        // C itself is not a bridge and can move.
        assert!(l.move_monitor(2, Dir::Left));
        assert!(l.validate().is_ok());
    }

    #[test]
    fn neighbor_focus_is_spatial() {
        let mut l = a_b();
        assert_eq!(l.neighbor(0, Dir::Right), Some(1));
        assert_eq!(l.neighbor(0, Dir::Left), None);
        for _ in 0..3 {
            l.move_monitor(1, Dir::Down);
        }
        assert_eq!(l.neighbor(0, Dir::Down), Some(1));
        assert_eq!(l.neighbor(1, Dir::Up), Some(0));
    }

    #[test]
    fn resize_pushes_monitors_to_the_right() {
        let mut l = a_b();
        l.monitors.push(mon("C", vec![mode("0x9", 1000, 1000, 60.0)], 4480, 0));
        l.resize(1, |m| m.mode = 1); // B: 1920 -> 1280 wide
        assert_eq!(pos(&l, "B"), (2560, 0));
        assert_eq!(pos(&l, "C"), (3840, 0));
        assert!(l.validate().is_ok());
    }

    #[test]
    fn rotation_swaps_size_and_pushes() {
        let mut l = a_b();
        l.resize(0, |m| m.rotation = Rotation::Left);
        assert_eq!(l.monitors[0].size(), (1440, 2560));
        assert_eq!(pos(&l, "B"), (1440, 0));
    }

    #[test]
    fn scale_affects_size() {
        let mut l = a_b();
        l.resize(1, |m| m.scale = 1.5);
        assert_eq!(l.monitors[1].size(), (2880, 1620));
    }

    #[test]
    fn cannot_disable_last_output() {
        let mut l = a_b();
        l.toggle_enabled(1).unwrap();
        assert!(l.toggle_enabled(0).is_err());
        l.toggle_enabled(1).unwrap();
        assert_eq!(pos(&l, "B"), (2560, 0));
    }

    #[test]
    fn mirror_picks_largest_common_resolution() {
        let mut l = Layout {
            monitors: vec![
                mon(
                    "eDP",
                    vec![mode("0x1", 2560, 1600, 60.0), mode("0x2", 1920, 1080, 60.0)],
                    0,
                    0,
                ),
                mon(
                    "DP",
                    vec![
                        mode("0x3", 3840, 2160, 60.0),
                        mode("0x4", 1920, 1080, 30.0),
                        mode("0x5", 1920, 1080, 60.0),
                    ],
                    2560,
                    0,
                ),
            ],
            stale: vec![],
        };
        l.cycle_mirror(1).unwrap();
        assert_eq!(l.monitors[1].mirror_of.as_deref(), Some("eDP"));
        assert_eq!(l.monitors[0].current_mode().id, "0x2");
        assert_eq!(l.monitors[1].current_mode().id, "0x5", "highest refresh");
        assert_eq!(pos(&l, "DP"), (0, 0));
        assert!(l.cycle_mirror(0).is_err(), "source can't mirror its mirror");
        l.cycle_mirror(1).unwrap();
        assert_eq!(l.monitors[1].mirror_of, None);
        assert_eq!(pos(&l, "DP"), (1920, 0));
    }

    #[test]
    fn from_outputs_detects_mirrors_and_stale() {
        use crate::xrandr::Placement;
        let p = |x| Some(Placement { x, y: 0, rotation: Rotation::Normal, scale: 1.0 });
        let mut m = mode("0x1", 1920, 1080, 60.0);
        m.current = true;
        let out = |name: &str, connected, placement| Output {
            name: name.into(),
            connected,
            primary: false,
            modes: if connected { vec![m.clone()] } else { vec![] },
            placement,
        };
        let l = Layout::from_outputs(&[
            out("A", true, p(0)),
            out("B", true, p(0)),
            out("C", false, p(1920)),
            out("D", false, None),
        ]);
        assert_eq!(l.monitors.len(), 2);
        assert_eq!(l.monitors[1].mirror_of.as_deref(), Some("A"));
        assert_eq!(l.stale, ["C"]);
    }

    #[test]
    fn step_resolution_keeps_refresh_close() {
        let m = mon(
            "X",
            vec![
                mode("0x1", 2560, 1600, 165.0),
                mode("0x2", 2560, 1600, 60.0),
                mode("0x3", 1920, 1080, 60.0),
                mode("0x4", 1920, 1080, 144.0),
            ],
            0,
            0,
        );
        assert_eq!(m.step_resolution(1), 3);
        assert_eq!(m.step_resolution(-1), 3, "wraps around");
        assert_eq!(m.step_refresh(1), 1);
        assert_eq!(m.refresh_options(), [0, 1]);
    }
}
