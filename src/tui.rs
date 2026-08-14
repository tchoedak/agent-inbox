//! The reader.
//!
//! Topic-first, per the model: the left pane lists topics, the right shows the
//! selected topic's current edition, and `[` / `]` page backward and forward
//! through that topic's history.
//!
//! Live updates poll the index every two seconds. The selected topic is
//! preserved by slug rather than by index, so a report arriving while you are
//! reading never moves what you are looking at.

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::query::{self, EditionDetail, TopicSummary};
use crate::render;
use crate::sound;
use crate::store::Store;

const POLL: Duration = Duration::from_secs(2);

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Topics,
    Reading,
}

struct App {
    topics: Vec<TopicSummary>,
    topic_state: ListState,
    editions: Vec<EditionDetail>,
    edition_idx: usize,
    content: Vec<Line<'static>>,
    scroll: u16,
    focus: Focus,
    show_help: bool,
    status: Option<String>,
    token: (i64, i64),
    last_poll: Instant,
    sound: bool,
}

impl App {
    fn new(store: &Store, sound: bool) -> Result<Self> {
        let mut app = App {
            topics: Vec::new(),
            topic_state: ListState::default(),
            editions: Vec::new(),
            edition_idx: 0,
            content: Vec::new(),
            scroll: 0,
            focus: Focus::Topics,
            show_help: false,
            status: None,
            token: (0, 0),
            last_poll: Instant::now(),
            sound,
        };
        app.reload(store, None)?;
        if !app.topics.is_empty() {
            app.topic_state.select(Some(0));
            app.load_editions(store)?;
        }
        Ok(app)
    }

    fn selected_topic(&self) -> Option<&TopicSummary> {
        self.topic_state.selected().and_then(|i| self.topics.get(i))
    }

    /// Refresh the topic list, keeping the cursor on the same topic by slug.
    fn reload(&mut self, store: &Store, keep: Option<String>) -> Result<()> {
        let keep = keep.or_else(|| self.selected_topic().map(|t| t.slug.clone()));
        self.topics = query::topics(store)?;
        self.token = query::revision_token(store)?;
        if let Some(slug) = keep {
            let found = self.topics.iter().position(|t| t.slug == slug);
            self.topic_state.select(found.or(Some(0)));
        }
        Ok(())
    }

    fn load_editions(&mut self, store: &Store) -> Result<()> {
        let Some(topic) = self.selected_topic().map(|t| t.slug.clone()) else {
            self.editions.clear();
            self.content.clear();
            return Ok(());
        };
        self.editions = query::edition_details(store, &topic)?;
        self.edition_idx = 0;
        self.scroll = 0;
        self.render_current();
        Ok(())
    }

    fn render_current(&mut self) {
        self.content = match self.editions.get(self.edition_idx) {
            Some(e) => match e.display_artifact() {
                Some(a) => render::artifact_lines(&a.path, &a.role),
                None => vec![Line::from("this edition has no artifacts")],
            },
            None => vec![Line::from("no editions for this topic yet")],
        };
    }

    /// Opening marks read. Paging backward through history deliberately does not.
    fn open_selected(&mut self, store: &Store) -> Result<()> {
        self.focus = Focus::Reading;
        self.scroll = 0;
        if let Some(e) = self.editions.first()
            && self.edition_idx == 0
            && !e.read
        {
            query::mark_read(store, e.id)?;
            self.reload(store, None)?;
            self.editions = query::edition_details(
                store,
                &self
                    .selected_topic()
                    .map(|t| t.slug.clone())
                    .unwrap_or_default(),
            )?;
        }
        self.render_current();
        Ok(())
    }

    fn step_edition(&mut self, delta: isize) {
        if self.editions.is_empty() {
            return;
        }
        let next = self.edition_idx as isize + delta;
        if next < 0 || next as usize >= self.editions.len() {
            return;
        }
        self.edition_idx = next as usize;
        self.scroll = 0;
        self.render_current();
    }

    fn move_topic(&mut self, store: &Store, delta: isize) -> Result<()> {
        if self.topics.is_empty() {
            return Ok(());
        }
        let cur = self.topic_state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.topics.len() as isize - 1) as usize;
        if Some(next) != self.topic_state.selected() {
            self.topic_state.select(Some(next));
            self.load_editions(store)?;
        }
        Ok(())
    }

    fn open_in_browser(&mut self) {
        let Some(art) = self
            .editions
            .get(self.edition_idx)
            .and_then(|e| e.primary_artifact())
        else {
            self.status = Some("nothing to open".into());
            return;
        };
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(opener).arg(&art.path).spawn() {
            Ok(_) => self.status = Some(format!("opened {}", art.filename)),
            Err(e) => self.status = Some(format!("could not open: {e}")),
        }
    }
}

pub fn run(store: &Store, sound: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = event_loop(&mut terminal, store, sound);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>, store: &Store, sound: bool) -> Result<()> {
    let mut app = App::new(store, sound)?;

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.status = None;
            if app.show_help {
                app.show_help = false;
                continue;
            }
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char('?') => app.show_help = true,
                KeyCode::Char('o') => app.open_in_browser(),
                KeyCode::Char('r') => {
                    app.reload(store, None)?;
                    app.load_editions(store)?;
                    app.status = Some("refreshed".into());
                }
                KeyCode::Char('j') | KeyCode::Down => match app.focus {
                    Focus::Topics => app.move_topic(store, 1)?,
                    Focus::Reading => app.scroll = app.scroll.saturating_add(1),
                },
                KeyCode::Char('k') | KeyCode::Up => match app.focus {
                    Focus::Topics => app.move_topic(store, -1)?,
                    Focus::Reading => app.scroll = app.scroll.saturating_sub(1),
                },
                KeyCode::PageDown | KeyCode::Char('d') => {
                    app.scroll = app.scroll.saturating_add(20)
                }
                KeyCode::PageUp | KeyCode::Char('u') => app.scroll = app.scroll.saturating_sub(20),
                KeyCode::Char('g') => app.scroll = 0,
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.open_selected(store)?,
                KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => app.focus = Focus::Topics,
                // History paging. Newest is index 0, so `[` goes back in time.
                KeyCode::Char('[') => app.step_edition(1),
                KeyCode::Char(']') => app.step_edition(-1),
                _ => {}
            }
        }

        if app.last_poll.elapsed() >= POLL {
            app.last_poll = Instant::now();
            let token = query::revision_token(store)?;
            if token != app.token {
                let keep = app.selected_topic().map(|t| t.slug.clone());
                let (idx, scroll, focus) = (app.edition_idx, app.scroll, app.focus);
                app.reload(store, keep)?;
                app.load_editions(store)?;
                // Nothing the poll does may move what the reader is looking at.
                if focus == Focus::Reading {
                    app.edition_idx = idx.min(app.editions.len().saturating_sub(1));
                    app.scroll = scroll;
                    app.focus = focus;
                    app.render_current();
                }
                app.status = Some("new report arrived".into());
                if app.sound {
                    sound::play();
                }
            }
        }
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    // Footer spans the full width: a hint clipped by the pane divider is worse
    // than no hint, since you cannot tell it was truncated.
    let outer = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(f.area());
    let panes = Layout::horizontal([Constraint::Length(36), Constraint::Min(24)]).split(outer[0]);

    draw_topics(f, app, panes[0]);
    draw_reading(f, app, panes[1]);

    let hint = app.status.clone().unwrap_or_else(|| {
        "j/k move · enter read · [ ] older/newer · o browser · r refresh · ? help · q quit".into()
    });
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {hint}"),
            Style::default().fg(Color::DarkGray),
        )),
        outer[1],
    );

    if app.show_help {
        draw_help(f);
    }
}

fn draw_topics(f: &mut Frame, app: &mut App, area: Rect) {
    // Borders, the marker, and a space either side of the date.
    let inner = area.width.saturating_sub(2) as usize;
    let date_w = 10;
    let name_w = inner.saturating_sub(2 + date_w + 1);

    let items: Vec<ListItem> = app
        .topics
        .iter()
        .map(|t| {
            let marker = if t.unread { "●" } else { " " };
            let name = t.title.as_deref().unwrap_or(&t.slug);
            let name = truncate(name, name_w);
            let date = t.latest_bucket.as_deref().unwrap_or("-");
            let style = if t.unread {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), Style::default().fg(Color::Green)),
                Span::styled(format!("{name:<name_w$} "), style),
                Span::styled(
                    format!("{date:>date_w$}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let border = if app.focus == Focus::Topics {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border)
                    .title(" topics "),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        area,
        &mut app.topic_state,
    );
}

fn draw_reading(f: &mut Frame, app: &App, area: Rect) {
    let title = match app.editions.get(app.edition_idx) {
        Some(e) => {
            let pos = if app.editions.len() > 1 {
                format!("  ({} of {})", app.edition_idx + 1, app.editions.len())
            } else {
                String::new()
            };
            let rev = if e.revision > 1 {
                format!("  rev {}", e.revision)
            } else {
                String::new()
            };
            format!(" {}{}{} ", e.bucket, rev, pos)
        }
        None => " no editions ".to_string(),
    };
    let border = if app.focus == Focus::Reading {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(app.content.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border)
                    .title(title),
            )
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        area,
    );
}

/// Truncate to a display width, marking that it happened. A silently cut name
/// reads as a different topic.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    let kept: String = s.chars().take(width - 1).collect();
    format!("{kept}…")
}

fn draw_help(f: &mut Frame) {
    let area = centered(62, 17, f.area());
    f.render_widget(Clear, area);
    let text = vec![
        Line::from(""),
        Line::from("  j / k / ↓ / ↑    move, or scroll while reading"),
        Line::from("  enter / l / →    read the selected topic"),
        Line::from("  esc / h / ←      back to the topic list"),
        Line::from("  [                older edition"),
        Line::from("  ]                newer edition"),
        Line::from("  d / u            page down / up"),
        Line::from("  g                back to the top"),
        Line::from("  o                open the report in a browser"),
        Line::from("  r                refresh now"),
        Line::from("  q                quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  ● marks a topic whose latest edition you have not read.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  New reports appear on their own, every two seconds.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  A sound plays when one arrives (--no-sound to mute).",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" keys "),
        ),
        area,
    );
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// Build a store with two editions of one topic, newest last.
    fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Store) {
        let store_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let store = Store::open(store_dir.path()).unwrap();

        for (bucket, body) in [
            (
                "2026-08-12",
                "# Daily trading performance\n\n- **Total Day P&L:** -197.00\n",
            ),
            (
                "2026-08-13",
                "# Daily trading performance\n\n- **Total Day P&L:** -242.00\n",
            ),
        ] {
            let f = work.path().join(format!("report-{bucket}.md"));
            std::fs::write(&f, body).unwrap();
            crate::emit::emit(
                &store,
                crate::emit::EmitRequest {
                    topic: "trading-perf".into(),
                    artifacts: vec![f.display().to_string().parse().unwrap()],
                    bucket: Some(bucket.into()),
                    timestamp: None,
                    title: Some("Daily trading performance".into()),
                    cadence: Some("daily".into()),
                    summary: None,
                    tags: vec![],
                    run_id: None,
                    source_project: None,
                    stdin_name: None,
                },
            )
            .unwrap();
        }
        (store_dir, work, store)
    }

    fn frame(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(96, 16)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_the_newest_edition_of_the_selected_topic() {
        let (_s, _w, store) = fixture();
        let mut app = App::new(&store, true).unwrap();
        let out = frame(&mut app);
        assert!(out.contains("Daily trading performance"), "{out}");
        assert!(
            out.contains("2026-08-13"),
            "newest edition should be shown:\n{out}"
        );
        assert!(out.contains("-242.00"), "{out}");
        assert!(!out.contains("-197.00"), "older edition leaked in:\n{out}");
    }

    #[test]
    fn an_unread_topic_is_marked_and_reading_clears_it() {
        let (_s, _w, store) = fixture();
        let mut app = App::new(&store, true).unwrap();
        assert!(frame(&mut app).contains('●'), "expected an unread marker");

        app.open_selected(&store).unwrap();
        let out = frame(&mut app);
        assert!(
            !out.contains('●'),
            "opening should clear the marker:\n{out}"
        );
    }

    #[test]
    fn paging_back_shows_history_without_marking_it_read() {
        let (_s, _w, store) = fixture();
        let mut app = App::new(&store, true).unwrap();
        app.open_selected(&store).unwrap();

        app.step_edition(1); // older
        let out = frame(&mut app);
        assert!(out.contains("2026-08-12"), "{out}");
        assert!(out.contains("-197.00"), "{out}");
        assert!(
            out.contains("(2 of 2)"),
            "position should be visible:\n{out}"
        );

        // Skimming backward is not reading: the older edition stays unread.
        let editions = query::edition_details(&store, "trading-perf").unwrap();
        let older = editions.iter().find(|e| e.bucket == "2026-08-12").unwrap();
        assert!(!older.read, "paging back must not mark history read");
    }

    #[test]
    fn a_report_arriving_does_not_move_what_you_are_reading() {
        let (_s, work, store) = fixture();
        let mut app = App::new(&store, true).unwrap();
        app.open_selected(&store).unwrap();
        app.step_edition(1);
        app.scroll = 3;
        let before = app.editions[app.edition_idx].bucket.clone();

        // A new topic arrives while the reader sits on an old edition.
        let f = work.path().join("other.md");
        std::fs::write(&f, "# Something else\n").unwrap();
        crate::emit::emit(
            &store,
            crate::emit::EmitRequest {
                topic: "job-scrape".into(),
                artifacts: vec![f.display().to_string().parse().unwrap()],
                bucket: Some("2026-08-14".into()),
                timestamp: None,
                title: None,
                cadence: None,
                summary: None,
                tags: vec![],
                run_id: None,
                source_project: None,
                stdin_name: None,
            },
        )
        .unwrap();

        let keep = app.selected_topic().map(|t| t.slug.clone());
        let (idx, scroll) = (app.edition_idx, app.scroll);
        app.reload(&store, keep).unwrap();
        app.load_editions(&store).unwrap();
        app.edition_idx = idx.min(app.editions.len().saturating_sub(1));
        app.scroll = scroll;
        app.render_current();

        assert_eq!(app.selected_topic().unwrap().slug, "trading-perf");
        assert_eq!(app.editions[app.edition_idx].bucket, before);
        assert_eq!(app.scroll, 3, "scroll position must survive a refresh");
    }

    #[test]
    fn print_a_real_frame() {
        let (_s, _w, store) = fixture();
        let mut app = App::new(&store, true).unwrap();
        println!("\n{}\n", frame(&mut app));
        app.open_selected(&store).unwrap();
        app.show_help = true;
        println!("{}\n", frame(&mut app));
    }
}
