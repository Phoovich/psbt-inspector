use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    question: &str,
    response: &str,
    loading: bool,
    error: Option<&str>,
    context: &str,
) {
    let popup = centered_rect(88, 84, area);
    frame.render_widget(Clear, popup);

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .title(" AI Assistant  [Enter] send  [Esc] close  [Ctrl+U] clear ")
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(outer_block, popup);

    let inner = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    // Cap context block height so it never overwhelms the response area.
    let context_lines = context.lines().count().max(1) as u16;
    let context_height = (context_lines + 2).min(7); // +2 border, max 5 content lines

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(context_height),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(inner);

    // Context (what will be sent to the AI)
    let context_widget = Paragraph::new(context)
        .style(Style::default().fg(Color::DarkGray))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Context  (sent with every query)"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(context_widget, chunks[0]);

    // Question input — always shows cursor
    let question_widget = Paragraph::new(question).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Question")
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(question_widget, chunks[1]);

    let q_inner = chunks[1].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let cursor_x = (q_inner.x + question.chars().count() as u16).min(q_inner.x + q_inner.width);
    frame.set_cursor_position(Position {
        x: cursor_x,
        y: q_inner.y,
    });

    // Response area
    let response_title = if loading {
        "Response  [generating…]"
    } else {
        "Response"
    };
    let response_widget = Paragraph::new(response)
        .block(Block::default().borders(Borders::ALL).title(response_title))
        .wrap(Wrap { trim: false });
    frame.render_widget(response_widget, chunks[2]);

    // Status line
    if let Some(err) = error {
        let err_widget =
            Paragraph::new(format!(" Error: {}", err)).style(Style::default().fg(Color::Red));
        frame.render_widget(err_widget, chunks[3]);
    } else if loading {
        let loading_widget =
            Paragraph::new(" ▓▒░ generating…").style(Style::default().fg(Color::Cyan));
        frame.render_widget(loading_widget, chunks[3]);
    }
}

/// Return a centered `Rect` that is `percent_x`% wide and `percent_y`% tall.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
