//! Rendering. The left pane draws the arrangement to scale; the right pane
//! lists outputs and the focused one's settings.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::app::{App, Phase};
use crate::layout::Monitor;

const FOCUS: Color = Color::Yellow;

pub fn draw(f: &mut Frame, app: &App) {
    let [main, footer] = Split::vertical([Constraint::Min(8), Constraint::Length(4)]).areas(f.area());
    let [canvas, panel] =
        Split::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(main);

    draw_canvas(f, app, canvas);
    draw_panel(f, app, panel);
    draw_footer(f, app, footer);
    if let Phase::Confirming { deadline } = app.phase {
        draw_confirm(f, deadline);
    }
}

fn draw_canvas(f: &mut Frame, app: &App, area: Rect) {
    let title = if app.dry_run { " Layout (dry run) " } else { " Layout " };
    let block = Block::bordered().title(title);
    let inner = block.inner(area).inner(ratatui::layout::Margin::new(1, 0));
    f.render_widget(block, area);

    let l = &app.layout;
    let parts = l.participants();
    if parts.is_empty() || inner.width < 4 || inner.height < 3 {
        return;
    }
    let rects: Vec<_> = parts.iter().map(|&i| l.monitors[i].rect()).collect();
    let bw = rects.iter().map(|r| r.x + r.w).max().unwrap_or(1).max(1) as f64;
    let bh = rects.iter().map(|r| r.y + r.h).max().unwrap_or(1).max(1) as f64;
    // Terminal cells are roughly twice as tall as wide.
    let s = (inner.width as f64 / bw).min(2.0 * inner.height as f64 / bh);
    let ox = (inner.width as f64 - bw * s) / 2.0;
    let oy = (inner.height as f64 - bh * s / 2.0) / 2.0;
    let col = |x: i32| (ox + x as f64 * s).round() as u16;
    let row = |y: i32| (oy + y as f64 * s / 2.0).round() as u16;

    // The focused monitor, or the source it mirrors, gets the highlight.
    let focused = &l.monitors[app.focus];
    let highlight = match &focused.mirror_of {
        Some(src) => l.index_of(src),
        None => focused.enabled.then_some(app.focus),
    };

    for (&i, r) in parts.iter().zip(&rects) {
        let m = &l.monitors[i];
        let (x0, x1, y0, y1) = (col(r.x), col(r.x + r.w), row(r.y), row(r.y + r.h));
        let cell = Rect::new(
            inner.x + x0,
            inner.y + y0,
            (x1 - x0).max(2),
            (y1 - y0).max(2),
        )
        .intersection(inner);

        let mut name = m.name.clone();
        for mirror in l.monitors.iter().filter(|o| o.mirror_of.as_deref() == Some(&m.name)) {
            name.push_str(&format!(" = {}", mirror.name));
        }
        let mut block = Block::bordered().title(Span::raw(name).bold());
        if highlight == Some(i) {
            block = block.border_type(BorderType::Thick).border_style(Style::new().fg(FOCUS));
        }
        let mut lines = vec![Line::raw(mode_label(m))];
        if m.rotation.as_str() != "normal" || m.scale != 1.0 {
            lines.push(Line::raw(format!("{} ×{}", m.rotation.as_str(), m.scale)).dim());
        }
        if m.primary {
            lines.push(Line::raw("★ primary").fg(Color::Cyan));
        }
        f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), cell);
    }
}

fn draw_panel(f: &mut Frame, app: &App, area: Rect) {
    let l = &app.layout;
    let mut lines: Vec<Line> = Vec::new();
    for (i, m) in l.monitors.iter().enumerate() {
        let state = match (&m.mirror_of, m.enabled) {
            (_, false) => Span::raw("off").fg(Color::DarkGray),
            (Some(src), _) => Span::raw(format!("= {src}")).fg(Color::Magenta),
            (None, true) => Span::raw("on").fg(Color::Green),
        };
        let marker = if i == app.focus { "▸ " } else { "  " };
        let mut line = Line::from(vec![
            Span::raw(marker).fg(FOCUS),
            Span::raw(format!("{:<14} ", m.name)),
            state,
        ]);
        if m.primary {
            line.push_span(Span::raw(" ★").fg(Color::Cyan));
        }
        if i == app.focus {
            line = line.add_modifier(Modifier::BOLD);
        }
        lines.push(line);
    }

    let m = &l.monitors[app.focus];
    let refreshes: Vec<String> = m
        .refresh_options()
        .into_iter()
        .map(|i| {
            let r = format!("{:.2}", m.modes[i].refresh);
            if i == m.mode { format!("[{r}]") } else { r }
        })
        .collect();
    let row = |key: &str, label: &str, value: String| {
        Line::from(vec![
            Span::raw(format!("{key:>3} ")).fg(FOCUS),
            Span::raw(format!("{label:<11}")).dim(),
            Span::raw(value),
        ])
    };
    lines.push(Line::raw(""));
    lines.push(Line::raw(m.name.clone()).bold().underlined());
    let cur = m.current_mode();
    lines.extend([
        row("m/M", "resolution", format!("{}x{}", cur.width, cur.height)),
        row("r/R", "refresh", refreshes.join(" ")),
        row("o", "rotation", m.rotation.as_str().into()),
        row("s", "scale", format!("{}", m.scale)),
        row("c", "mirror of", m.mirror_of.clone().unwrap_or_else(|| "—".into())),
        row("", "position", if m.enabled { format!("{},{}", m.x, m.y) } else { "off".into() }),
    ]);

    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Outputs "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let key = |k: &'static str| Span::raw(k).fg(FOCUS);
    let hints = Line::from(vec![
        key("hjkl"), Span::raw(" focus  "),
        key("tab"), Span::raw(" next output (incl. off)  "),
        key("HJKL"), Span::raw(" move  "),
        key("space"), Span::raw(" on/off  "),
        key("p"), Span::raw(" primary  "),
        key("u"), Span::raw(" undo  "),
        key("e"), Span::raw(" reload  "),
        key("enter"), Span::raw(" apply  "),
        key("q"), Span::raw(" quit"),
    ]);
    let status = match &app.status {
        Some(s) if s.error => Line::raw(s.text.clone()).fg(Color::Red),
        Some(s) => Line::raw(s.text.clone()).fg(Color::Green),
        None => Line::raw(""),
    };
    f.render_widget(
        Paragraph::new(vec![hints, status])
            .block(Block::bordered())
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_confirm(f: &mut Frame, deadline: Instant) {
    let secs = deadline.saturating_duration_since(Instant::now()).as_secs() + 1;
    let area = f.area();
    let (w, h) = (44.min(area.width), 5.min(area.height));
    let popup = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    let text = vec![
        Line::raw(format!("Reverting in {secs}s")).bold().centered(),
        Line::from(vec![
            Span::raw("y").fg(FOCUS).bold(),
            Span::raw(" keep    "),
            Span::raw("any other key").fg(FOCUS),
            Span::raw(" revert"),
        ])
        .centered(),
    ];
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(text).block(
            Block::bordered()
                .border_type(BorderType::Double)
                .border_style(Style::new().fg(FOCUS))
                .title(" Keep this configuration? "),
        ),
        popup,
    );
}

fn mode_label(m: &Monitor) -> String {
    let mode = m.current_mode();
    format!("{}x{}@{:.0}", mode.width, mode.height, mode.refresh)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::layout::Layout;
    use crate::xrandr::parse_verbose;

    const FIXTURE: &str = include_str!("../tests/fixtures/verbose_edp_dp.txt");

    fn render(app: &App) -> String {
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_fixture_layout() {
        let layout = Layout::from_outputs(&parse_verbose(FIXTURE).unwrap());
        let screen = render(&App::new(layout, true));
        println!("{screen}");
        assert!(screen.contains("2560x1600@60"));
        assert!(screen.contains("1920x1080@60"));
        assert!(screen.contains("165.00 [60.00]"), "eDP refresh options");
    }
}
