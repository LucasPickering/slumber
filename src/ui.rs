use std::{
    fmt::Display,
    io::{self, Stdout},
    time::{Duration, Instant},
};

use crate::config::{Environment, RequestRecipe};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode,
        KeyEventKind,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use log::error;
use ratatui::{prelude::*, widgets::*};

struct StatefulList<T> {
    state: ListState,
    items: Vec<T>,
}

impl<T> StatefulList<T> {
    fn with_items(items: Vec<T>) -> StatefulList<T> {
        let mut state = ListState::default();
        // Pre-select the first item if possible
        if !items.is_empty() {
            state.select(Some(0));
        }
        StatefulList { state, items }
    }

    fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn unselect(&mut self) {
        self.state.select(None);
    }
}

/// This struct holds the current state of the app. In particular, it has the
/// `items` field which is a wrapper around `ListState`. Keeping track of the
/// items state let us render the associated widget with its state and have
/// access to features such as natural scrolling.
///
/// Check the event handling at the bottom to see how to change the state on
/// incoming events. Check the drawing logic for items on how to specify the
/// highlighting style for selected items.
pub struct App<'a> {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: AppState<'a>,
}

struct AppState<'a> {
    environments: StatefulList<&'a Environment>,
    recipes: StatefulList<&'a RequestRecipe>,
}

impl<'a> App<'a> {
    pub fn start(
        environments: &'a [Environment],
        recipes: &'a [RequestRecipe],
    ) -> anyhow::Result<()> {
        // Set up terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let app = App {
            terminal,
            state: AppState {
                environments: StatefulList::with_items(
                    environments.iter().collect(),
                ),
                recipes: StatefulList::with_items(recipes.iter().collect()),
            },
        };

        app.run()
    }

    /// Run the main TUI update loop
    fn run(mut self) -> anyhow::Result<()> {
        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();
        loop {
            self.terminal.draw(|f| ui(f, &mut self.state))?;

            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if crossterm::event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') => return Ok(()),
                            KeyCode::Left => self.state.recipes.unselect(),
                            KeyCode::Down => self.state.recipes.next(),
                            KeyCode::Up => self.state.recipes.previous(),
                            _ => {}
                        }
                    }
                }
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }
    }
}

impl<'a> Drop for App<'a> {
    fn drop(&mut self) {
        // Restore terminal
        log_error(disable_raw_mode());
        log_error(execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        ));
        log_error(self.terminal.show_cursor());
    }
}

/// If a result is an error, log it. Useful for handling errors in situations
/// where we can't panic or exit.
fn log_error<T, E: Display>(result: Result<T, E>) {
    if let Err(err) = result {
        error!("{err}");
    }
}

fn ui<B: Backend>(f: &mut Frame<B>, state: &mut AppState) {
    // Create layout
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Max(40), Constraint::Percentage(50)].as_ref())
        .split(f.size());

    // Create request list
    let requests: Vec<ListItem> = state
        .recipes
        .items
        .iter()
        .map(|recipe| {
            let lines = recipe.to_lines();
            ListItem::new(lines)
                .style(Style::default().fg(Color::Black).bg(Color::White))
        })
        .collect();

    // Render request list
    let items = List::new(requests)
        .block(Block::default().borders(Borders::ALL).title("Requests"))
        .highlight_style(
            Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // We can now render the item list
    f.render_stateful_widget(items, chunks[0], &mut state.recipes.state);
}

trait ToLines {
    fn to_lines(&self) -> Vec<Line>;
}

impl ToLines for RequestRecipe {
    fn to_lines(&self) -> Vec<Line> {
        vec![
            self.name.clone().into(),
            format!("{} {}", self.method, self.url).into(),
        ]
    }
}
