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
    /// Start the TUI
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
            self.terminal.draw(|f| draw_main(f, &mut self.state))?;

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

fn draw_main(f: &mut Frame<impl Backend>, state: &mut AppState) {
    // Create layout
    let [left_chunk, right_chunk] = layout(
        f.size(),
        Direction::Horizontal,
        [Constraint::Max(40), Constraint::Percentage(50)],
    );

    let [environments_chunk, requests_chunk] = layout(
        left_chunk,
        Direction::Vertical,
        [Constraint::Max(16), Constraint::Min(0)],
    );

    let [request_chunk, response_chunk] = layout(
        right_chunk,
        Direction::Vertical,
        [Constraint::Percentage(50), Constraint::Percentage(50)],
    );

    draw_environment_list(f, environments_chunk, state);
    draw_request_list(f, requests_chunk, state);
    draw_request(f, request_chunk, state);
    draw_response(f, response_chunk, state);
}

fn layout<const N: usize>(
    area: Rect,
    direction: Direction,
    constraints: [Constraint; N],
) -> [Rect; N] {
    Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area)
        .as_ref()
        .try_into()
        // Should be unreachable
        .expect("Chunk length does not match constraint length")
}

fn draw_environment_list(
    f: &mut Frame<impl Backend>,
    chunk: Rect,
    state: &mut AppState,
) {
    let environments = do_list_thing(&state.environments);
    let items = List::new(environments)
        .block(Block::default().borders(Borders::ALL).title("Environments"))
        .highlight_style(
            Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_stateful_widget(items, chunk, &mut state.environments.state);
}

fn draw_request_list(
    f: &mut Frame<impl Backend>,
    chunk: Rect,
    state: &mut AppState,
) {
    // Create list
    let requests = do_list_thing(&state.recipes);

    // Render list
    let items = List::new(requests)
        .block(Block::default().borders(Borders::ALL).title("Requests"))
        .highlight_style(
            Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_stateful_widget(items, chunk, &mut state.recipes.state);
}

fn draw_request(
    f: &mut Frame<impl Backend>,
    chunk: Rect,
    state: &mut AppState,
) {
    let block = Block::default().borders(Borders::ALL).title("Request");
    f.render_widget(block, chunk);
}

fn draw_response(
    f: &mut Frame<impl Backend>,
    chunk: Rect,
    state: &mut AppState,
) {
    let block = Block::default().borders(Borders::ALL).title("Response");
    f.render_widget(block, chunk);
}

fn do_list_thing<'a, T: ToLines>(
    list: &StatefulList<&'a T>,
) -> Vec<ListItem<'a>> {
    list.items
        .iter()
        .map(|element| {
            let lines = element.to_lines();
            ListItem::new(lines)
                .style(Style::default().fg(Color::Black).bg(Color::White))
        })
        .collect()
}

trait ToLines {
    fn to_lines(&self) -> Vec<Line>;
}

impl ToLines for Environment {
    fn to_lines(&self) -> Vec<Line> {
        vec![self.name.clone().into()]
    }
}

impl ToLines for RequestRecipe {
    fn to_lines(&self) -> Vec<Line> {
        vec![
            self.name.clone().into(),
            format!("{} {}", self.method, self.url).into(),
        ]
    }
}
