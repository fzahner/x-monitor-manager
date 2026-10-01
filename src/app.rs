//! Editor state and key handling.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};

use crate::layout::{Dir, Layout};
use crate::{ui, xrandr};

pub const REVERT_AFTER: Duration = Duration::from_secs(15);

pub enum Phase {
    Editing,
    /// A layout was applied; revert to `App::applied` unless confirmed by `deadline`.
    Confirming { deadline: Instant },
}

pub struct Status {
    pub text: String,
    pub error: bool,
}

pub struct App {
    /// The staged layout being edited.
    pub layout: Layout,
    /// What was on screen before the last apply; the revert target.
    pub applied: Layout,
    pub focus: usize,
    pub phase: Phase,
    pub status: Option<Status>,
    pub dry_run: bool,
    /// Last command built in dry-run mode, printed on exit.
    pub last_command: Option<String>,
    undo: Vec<Layout>,
    quit: bool,
}

impl App {
    pub fn new(layout: Layout, dry_run: bool) -> Self {
        let focus = layout.monitors.iter().position(|m| m.primary).unwrap_or(0);
        Self {
            applied: layout.clone(),
            layout,
            focus,
            phase: Phase::Editing,
            status: None,
            dry_run,
            last_command: None,
            undo: Vec::new(),
            quit: false,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<Self> {
        while !self.quit {
            terminal.draw(|f| ui::draw(f, &self))?;
            if event::poll(Duration::from_millis(200))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key);
            }
            self.tick();
        }
        Ok(self)
    }

    fn tick(&mut self) {
        if let Phase::Confirming { deadline } = self.phase
            && Instant::now() >= deadline
        {
            self.revert();
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if let Phase::Confirming { .. } = self.phase {
            if key.code == KeyCode::Char('y') {
                self.keep();
            } else {
                self.revert();
            }
            return;
        }

        self.status = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Tab => self.focus = (self.focus + 1) % self.layout.monitors.len(),
            KeyCode::BackTab => {
                let n = self.layout.monitors.len();
                self.focus = (self.focus + n - 1) % n;
            }
            KeyCode::Char('h') | KeyCode::Left => self.focus_dir(Dir::Left),
            KeyCode::Char('j') | KeyCode::Down => self.focus_dir(Dir::Down),
            KeyCode::Char('k') | KeyCode::Up => self.focus_dir(Dir::Up),
            KeyCode::Char('l') | KeyCode::Right => self.focus_dir(Dir::Right),
            KeyCode::Char('H') => self.move_dir(Dir::Left),
            KeyCode::Char('J') => self.move_dir(Dir::Down),
            KeyCode::Char('K') => self.move_dir(Dir::Up),
            KeyCode::Char('L') => self.move_dir(Dir::Right),
            KeyCode::Char(c @ ('m' | 'M')) => {
                let step = if c == 'm' { 1 } else { -1 };
                self.edit_enabled(|l, i| {
                    let mode = l.monitors[i].step_resolution(step);
                    l.resize(i, |m| m.mode = mode);
                });
            }
            KeyCode::Char(c @ ('r' | 'R')) => {
                let step = if c == 'r' { 1 } else { -1 };
                self.edit_enabled(|l, i| l.monitors[i].mode = l.monitors[i].step_refresh(step));
            }
            KeyCode::Char('o') => {
                self.edit_enabled(|l, i| l.resize(i, |m| m.rotation = m.rotation.next()));
            }
            KeyCode::Char('s') => {
                self.edit_enabled(|l, i| l.resize(i, |m| m.scale = m.next_scale()));
            }
            KeyCode::Char('p') => self.edit(|l, i| l.set_primary(i)),
            KeyCode::Char(' ') => self.edit(|l, i| l.toggle_enabled(i)),
            KeyCode::Char('c') => self.edit(|l, i| l.cycle_mirror(i)),
            KeyCode::Char('u') => match self.undo.pop() {
                Some(prev) => self.layout = prev,
                None => self.error("nothing to undo"),
            },
            KeyCode::Char('e') => match self.reload() {
                Ok(()) => self.info("reloaded from xrandr; staged edits discarded"),
                Err(e) => self.error(format!("reload failed: {e:#}")),
            },
            KeyCode::Enter => self.apply(),
            _ => {}
        }
    }

    fn focus_dir(&mut self, dir: Dir) {
        if let Some(j) = self.layout.neighbor(self.focus, dir) {
            self.focus = j;
        }
    }

    fn move_dir(&mut self, dir: Dir) {
        self.edit(|l, i| {
            if l.move_monitor(i, dir) {
                Ok(())
            } else {
                Err(format!("no snap stop {}", match dir {
                    Dir::Left => "to the left",
                    Dir::Right => "to the right",
                    Dir::Up => "above",
                    Dir::Down => "below",
                }))
            }
        });
    }

    /// Runs an edit on the focused monitor, recording undo history, or
    /// rolls it back and shows the error.
    fn edit(&mut self, f: impl FnOnce(&mut Layout, usize) -> Result<(), String>) {
        let before = self.layout.clone();
        match f(&mut self.layout, self.focus) {
            Ok(()) if self.layout != before => self.undo.push(before),
            Ok(()) => {}
            Err(e) => {
                self.layout = before;
                self.error(e);
            }
        }
    }

    fn edit_enabled(&mut self, f: impl FnOnce(&mut Layout, usize)) {
        self.edit(|l, i| {
            if !l.monitors[i].enabled {
                return Err("output is off (space to enable)".into());
            }
            f(l, i);
            Ok(())
        });
    }

    fn apply(&mut self) {
        if let Err(e) = self.layout.validate() {
            return self.error(e);
        }
        let args = xrandr::build_args(&self.layout);
        let cmd = xrandr::format_command(&args);
        if self.dry_run {
            self.info(format!("dry run: {cmd}"));
            self.last_command = Some(cmd);
            return;
        }
        match xrandr::apply(&args) {
            Ok(()) => {
                self.phase = Phase::Confirming {
                    deadline: Instant::now() + REVERT_AFTER,
                }
            }
            Err(e) => {
                // xrandr may have applied part of the command before failing.
                let _ = xrandr::apply(&xrandr::build_args(&self.applied));
                self.error(format!("{e:#}"));
            }
        }
    }

    fn keep(&mut self) {
        self.phase = Phase::Editing;
        // Re-read so the editor reflects exactly what X ended up with.
        match self.reload() {
            Ok(()) => self.info("configuration kept"),
            Err(e) => self.error(format!("kept, but re-reading state failed: {e:#}")),
        }
    }

    /// Replaces the staged layout with what xrandr currently reports,
    /// keeping focus on the same output if it still exists.
    fn reload(&mut self) -> Result<()> {
        let layout = Layout::from_outputs(&xrandr::query()?);
        if layout.monitors.is_empty() {
            bail!("xrandr reports no connected outputs");
        }
        let focused = &self.layout.monitors[self.focus].name;
        self.focus = layout
            .index_of(focused)
            .or_else(|| layout.monitors.iter().position(|m| m.primary))
            .unwrap_or(0);
        self.applied = layout.clone();
        self.layout = layout;
        self.undo.clear();
        Ok(())
    }

    fn revert(&mut self) {
        self.phase = Phase::Editing;
        match xrandr::apply(&xrandr::build_args(&self.applied)) {
            Ok(()) => self.info("reverted; your edits are still staged"),
            Err(e) => self.error(format!("revert failed: {e:#}")),
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: true });
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::xrandr::parse_verbose;

    const FIXTURE: &str = include_str!("../tests/fixtures/verbose_edp_dp.txt");

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let code = if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) };
            app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    fn fixture_app() -> App {
        App::new(Layout::from_outputs(&parse_verbose(FIXTURE).unwrap()), true)
    }

    #[test]
    fn move_external_next_to_laptop_and_dry_run_apply() {
        let mut app = fixture_app();
        assert_eq!(app.focus, 0, "starts on the primary (eDP)");
        press(&mut app, "k");
        assert_eq!(app.layout.monitors[app.focus].name, "DisplayPort-1");
        // DP sits above eDP, left-aligned. L walks it along eDP's top edge
        // (center, right-aligned), then onto eDP's right side.
        press(&mut app, "LLL");
        assert!(app.layout.validate().is_ok());
        let dp = &app.layout.monitors[1];
        assert_eq!((dp.x, dp.y), (2560, 0), "right of eDP, top-aligned");
        press(&mut app, "\n");
        assert_eq!(
            app.last_command.as_deref(),
            Some(
                "xrandr --output eDP --mode 0x5c --pos 0x0 --rotate normal --scale 1x1 --primary \
                 --output DisplayPort-1 --mode 0x3ec --pos 2560x0 --rotate normal --scale 1x1"
            )
        );
        press(&mut app, "uuu");
        assert_eq!(app.layout, app.applied, "undo restores the original");
    }

    #[test]
    fn errors_are_reported_and_rolled_back() {
        let mut app = fixture_app();
        press(&mut app, " k ");
        assert!(app.status.as_ref().is_some_and(|s| s.error), "last output can't go off");
        assert!(app.layout.monitors[1].enabled);
        assert!(!app.layout.monitors[0].enabled);
    }
}
