use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    question: &str,
    response: &str,
    loading: bool,
    error: Option<&str>,
    context: &str,
    consent_pending: bool,
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
    let q_inner = chunks[1].inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let q_width = q_inner.width as usize;
    let q_show = q_width.saturating_sub(1);
    let q_len = question.len();
    let tail_start = if q_len > q_show {
        let byte_start = q_len - q_show;
        (byte_start..=q_len)
            .find(|&i| question.is_char_boundary(i))
            .unwrap_or(q_len)
    } else {
        0
    };
    let q_visible = &question[tail_start..];
    let q_char_count = q_visible.chars().count(); // O(width), not O(n)

    let question_widget = Paragraph::new(q_visible).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Question")
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(question_widget, chunks[1]);

    let cursor_x = q_inner.x + q_char_count as u16;
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

    // Consent prompt overlays the response area while pending.
    if consent_pending {
        let prompt = Paragraph::new(
            "Send PSBT/multisig context (txid, value, address, pubkey) to the AI? [y/n]",
        )
        .style(Style::default().fg(Color::Yellow))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Consent required"),
        )
        .wrap(Wrap { trim: false });
        frame.render_widget(prompt, chunks[2]);
    }

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
