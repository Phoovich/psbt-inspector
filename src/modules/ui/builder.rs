use crate::app::{BuilderFocus, MultisigState};
use crate::modules::bitcoin::multisig::MultisigInfo;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    pubkey1: &str,
    pubkey2: &str,
    focus: &BuilderFocus,
    state: &MultisigState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    draw_pubkey_box(
        frame,
        chunks[0],
        "Public Key 1  [Enter] next field",
        pubkey1,
        *focus == BuilderFocus::Pubkey1,
    );
    draw_pubkey_box(
        frame,
        chunks[1],
        "Public Key 2  [Enter] build  [q] quit  [Ctrl+U] clear",
        pubkey2,
        *focus == BuilderFocus::Pubkey2,
    );
    draw_result(frame, chunks[2], state);
}

fn draw_pubkey_box(frame: &mut Frame, area: Rect, title: &str, text: &str, is_focused: bool) {
    let border_style = if is_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let width = inner.width as usize;
    let char_count = text.chars().count();
    let visible: String = if char_count > width {
        text.chars().skip(char_count - width).collect()
    } else {
        text.to_string()
    };

    let p = Paragraph::new(visible).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(border_style),
    );
    frame.render_widget(p, area);

    if is_focused {
        let cursor_x = (inner.x + char_count.min(width) as u16).min(inner.x + inner.width);
        frame.set_cursor_position(Position {
            x: cursor_x,
            y: inner.y,
        });
    }
}

fn draw_result(frame: &mut Frame, area: Rect, state: &MultisigState) {
    match state {
        MultisigState::Empty => {
            let p = Paragraph::new(
                "Enter two compressed public keys (33-byte hex, 02… or 03… prefix)\nand press Enter to build the 2-of-2 P2WSH multisig address.",
            )
            .block(Block::default().borders(Borders::ALL).title("Result"))
            .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        MultisigState::Loading => {
            let p = Paragraph::new("Building…")
                .block(Block::default().borders(Borders::ALL).title("Result"));
            frame.render_widget(p, area);
        }
        MultisigState::Err(msg) => {
            let p = Paragraph::new(msg.as_str())
                .style(Style::default().fg(Color::Red))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Result — Error"),
                )
                .wrap(Wrap { trim: false });
            frame.render_widget(p, area);
        }
        MultisigState::Ok(info) => draw_multisig_result(frame, area, info),
    }
}

fn draw_multisig_result(frame: &mut Frame, area: Rect, info: &MultisigInfo) {
    let network_str = match info.network {
        bitcoin::Network::Bitcoin => "mainnet",
        bitcoin::Network::Testnet => "testnet",
        bitcoin::Network::Signet => "signet",
        bitcoin::Network::Regtest => "regtest",
        _ => "other",
    };

    let label = Style::default().fg(Color::Yellow);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Address:    ", label),
            Span::styled(&info.address, Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("Network:    ", label),
            Span::raw(network_str),
        ]),
        Line::raw(""),
        Line::from(vec![Span::styled("Descriptor:", label)]),
        Line::from(format!("  {}", info.descriptor)),
        Line::raw(""),
        Line::from(vec![Span::styled("Witness script:", label)]),
        Line::from(format!("  {}", info.witness_script_hex)),
    ];

    if info.keys_were_sorted {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("[i] ", Style::default().fg(Color::Cyan)),
            Span::raw("Keys were reordered per BIP67"),
        ]));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Result"))
        .wrap(Wrap { trim: false });
    frame.render_widget(p, area);
}
