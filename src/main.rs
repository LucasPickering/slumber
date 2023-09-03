mod config;
mod http;
mod state;
mod template;
mod theme;
mod ui;
mod util;

use crate::{
    config::RequestCollection,
    http::HttpEngine,
    state::{AppState, Message},
    ui::Renderer,
    util::{initialize_panic_handler, restore_terminal},
};
use anyhow::{anyhow, Context};
use crossterm::{
    event::{
        self, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{enable_raw_mode, EnterAlternateScreen},
};
use log::error;
use ratatui::{prelude::CrosstermBackend, Terminal};
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    iterator::Signals,
};
use std::{
    io::{self, Stdout},
    ops::ControlFlow,
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_panic_handler();
    let collection = RequestCollection::load(None).await?;
    App::start(collection)
}

/// Main controller struct. The app uses an MVC architecture, and this is the C
#[derive(Debug)]
pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    renderer: Renderer,
    http_engine: HttpEngine,
    state: AppState,
}

impl App {
    /// Start the TUI
    pub fn start(collection: RequestCollection) -> anyhow::Result<()> {
        // Set up terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut app = App {
            terminal,
            renderer: Renderer::new(),
            http_engine: HttpEngine::new(),
            state: collection.into(),
        };

        app.run()
    }

    /// Run the main TUI update loop
    fn run(&mut self) -> anyhow::Result<()> {
        // Listen for signals to stop the program
        let mut quit_signals = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT])
            .context("Error creating signal handler")?;

        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();

        loop {
            if quit_signals.pending().next().is_some() {
                return Ok(());
            }

            self.terminal
                .draw(|f| self.renderer.draw_main(f, &mut self.state))?;

            // Handle all messages in the queue before accepting new input.
            // Can't use a for loop because that maintains a mutable ref to self
            while let Some(message) = self.state.message_queue.pop_front() {
                // If an error occurs, store it so we can show the user
                if let Err(err) = self.handle_message(message) {
                    error!("Error handling message: {err}");
                    self.state.error = Some(err);
                }
            }

            // Check for any new events
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if crossterm::event::poll(timeout)? {
                // If the user asked to quit, exit immediately
                if let ControlFlow::Break(()) =
                    self.handle_event(event::read()?)
                {
                    return Ok(());
                }
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }
    }

    /// Handle a single input event. If the event triggers a Quit, we return
    /// that so it can be done immediately.
    fn handle_event(&mut self, event: Event) -> ControlFlow<()> {
        if let Event::Key(
            key @ KeyEvent {
                kind: KeyEventKind::Press,
                ..
            },
        ) = event
        {
            match key.code {
                // q or ctrl-c both quit
                KeyCode::Char('q') => return ControlFlow::Break(()),
                KeyCode::Char('c')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return ControlFlow::Break(())
                }

                // Normal events
                KeyCode::Up => self.state.enqueue(Message::SelectPrevious),
                KeyCode::Down => self.state.enqueue(Message::SelectNext),
                KeyCode::Char(' ') => self.state.enqueue(Message::SendRequest),
                _ => {}
            }
        }
        ControlFlow::Continue(())
    }

    /// Handle an incoming message. Any error here will be fatal!
    /// TODO render these errors in a popup instead
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::SendRequest => {
                let recipe =
                    self.state.recipes.selected().ok_or_else(|| {
                        anyhow!("Cannot send request with no recipe selected")
                    })?;

                // Build the request, then launch it
                self.state.active_request = Some(
                    self.http_engine
                        .build_request(recipe, &(&self.state).into())?,
                );
                // Unwrap is safe because we *just* populated it
                self.http_engine
                    .send_request(self.state.active_request.as_ref().unwrap());
            }
            Message::SelectPrevious => self.state.recipes.previous(),
            Message::SelectNext => self.state.recipes.next(),
        }
        Ok(())
    }
}

/// Restore terminal on app exit
impl Drop for App {
    fn drop(&mut self) {
        if let Err(err) = restore_terminal() {
            error!("Error restoring terminal, sorry! {}", err);
        }
    }
}
