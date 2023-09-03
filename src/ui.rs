//! All view-related functions live in here. There isn't a singular "View"
//! struct, but these together constitute the V in MVC

use crate::state::AppState;
use ratatui::{prelude::*, widgets::*};

pub fn draw_main(f: &mut Frame<impl Backend>, state: &mut AppState) {
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

fn draw_environment_list(
    f: &mut Frame<impl Backend>,
    chunk: Rect,
    state: &mut AppState,
) {
    let items = List::new(state.environments.to_items())
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
    let items = List::new(state.recipes.to_items())
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
    if let Some(recipe) = state.recipes.selected() {
        let block = Block::default().borders(Borders::ALL).title("Request");

        let mut lines: Vec<Line> =
            vec![format!("{} {}", recipe.method, recipe.url).into()];

        // Add request body if present
        if let Some(body) = &recipe.body {
            lines.extend(body.lines().map(Line::from));
        }

        let paragraph = Paragraph::new(lines).block(block);
        f.render_widget(paragraph, chunk);
    }
}

fn draw_response(
    f: &mut Frame<impl Backend>,
    chunk: Rect,
    state: &mut AppState,
) {
    let block = Block::default().borders(Borders::ALL).title("Response");

    if let Some(request) = &state.active_request {
        // If we can't acquire the read lock for the response, someone else must
        // be writing - don't show anything until they're done
        let text = match request.response.try_read().as_deref() {
            Ok(None) | Err(_) => String::new(),
            Ok(Some(Ok(response))) => response.content.clone(),
            Ok(Some(Err(err))) => err.to_string(),
        };

        let paragraph = Paragraph::new(text).block(block);
        f.render_widget(paragraph, chunk);
    } else {
        f.render_widget(block, chunk);
    }
}

/// Helper for building a layout with some constraints
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
