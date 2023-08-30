mod config;
mod state;
mod template;
mod ui;
mod util;

use crate::{
    config::RequestCollection,
    state::{AppState, Message},
    ui::draw_main,
    util::{initialize_panic_handler, log_error},
};
use anyhow::Context;
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
use ratatui::{prelude::CrosstermBackend, Terminal};
use reqwest::{Client, Request, Response};
use std::{
    io::{self, Stdout},
    ops::ControlFlow,
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_panic_handler();
    let collection = dbg!(RequestCollection::load(None).await?);
    App::start(collection)?;
    Ok(())
}

/// Main controller struct. The app uses an MVC architecture, and this is the C
#[derive(Debug)]
pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    http_client: Client,
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
            http_client: Client::new(),
            state: collection.into(),
        };

        app.run()
    }

    /// Run the main TUI update loop
    fn run(&mut self) -> anyhow::Result<()> {
        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();
        loop {
            self.terminal.draw(|f| draw_main(f, &mut self.state))?;

            // Handle all messages in the queue before accepting new input
            // TODO can we get away without a collect here
            for message in
                self.state.message_queue.drain(..).collect::<Vec<_>>()
            {
                self.handle_message(message)?;
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
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => return ControlFlow::Break(()),
                    KeyCode::Up => self.state.enqueue(Message::SelectPrevious),
                    KeyCode::Down => self.state.enqueue(Message::SelectNext),
                    KeyCode::Char(' ') => {
                        self.state.enqueue(Message::SendRequest)
                    }
                    _ => {}
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::SendRequest => {
                self.state.active_request = Some(self.build_request()?);
            }
            Message::SelectPrevious => self.state.recipes.previous(),
            Message::SelectNext => self.state.recipes.next(),
        }
        Ok(())
    }

    fn build_request(&self) -> anyhow::Result<Request> {
        // TODO add error contexts
        let environment = self.state.environments.selected().unwrap();
        let recipe = self.state.recipes.selected().unwrap();
        let method = recipe.method.render(&environment.data)?.parse()?;
        let url = recipe.url.render(&environment.data)?;
        self.http_client
            .request(method, url)
            .build()
            .context("TODO")
    }

    async fn execute_request(
        &self,
        request: Request,
    ) -> reqwest::Result<Response> {
        self.http_client.execute(request).await
    }
}

/// Restore terminal on app exit
impl Drop for App {
    fn drop(&mut self) {
        log_error(disable_raw_mode());
        log_error(execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        ));
        log_error(self.terminal.show_cursor());
    }
}
