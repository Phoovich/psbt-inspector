use crate::app::PsbtState;
use crate::modules::bitcoin::psbt::{FeeInfo, PsbtSummary};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph, Row, Table, Wrap},
};

/// Top-level draw entry for the Inspector panel.
/// `input` is the current contents of psbt_input in AppState.
pub fn draw(frame: &mut Frame, area: Rect, state: &PsbtState, input: &str) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    draw_input_box(frame, chunks[0], input);

    match state {
        PsbtState::Empty => {
            let p = Paragraph::new("Paste a PSBT above and press Enter to parse.")
                .block(Block::default().borders(Borders::ALL).title("Inspector"));
            frame.render_widget(p, chunks[1]);
        }
        PsbtState::Loading => {
            let p = Paragraph::new("Parsing…")
                .block(Block::default().borders(Borders::ALL).title("Inspector"));
            frame.render_widget(p, chunks[1]);
        }
        PsbtState::Err(msg) => {
            let p = Paragraph::new(msg.as_str())
                .style(Style::default().fg(Color::Red))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Inspector — Error"),
                );
            frame.render_widget(p, chunks[1]);
        }
        PsbtState::Ok(summary) => draw_summary(frame, chunks[1], summary),
    }
}

fn draw_input_box(frame: &mut Frame, area: Rect, input: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("PSBT Input  [Enter] parse  [Ctrl+U] clear  [Esc] quit");

    let p = Paragraph::new(input)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(p, area);

    // Place cursor after the last character, clamped to the inner width.
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let cursor_x = (inner.x + input.chars().count() as u16).min(inner.x + inner.width);
    frame.set_cursor_position(Position {
        x: cursor_x,
        y: inner.y,
    });
}

fn draw_summary(frame: &mut Frame, area: Rect, s: &PsbtSummary) {
    let info_height = 2 + s.warnings.len() as u16 + 2; // content lines + top/bottom border
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(info_height),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .split(area);

    // --- Summary info block ---
    let fee_str = match &s.fee {
        FeeInfo::Known(sats) => format!("{} sats", sats),
        FeeInfo::Unknown => "unknown (UTXO data missing)".into(),
    };
    let mut lines = vec![
        Line::from(format!("PSBT v{}", s.version)),
        Line::from(format!(
            "Inputs: {}   Outputs: {}",
            s.input_count, s.output_count
        )),
        Line::from(format!(
            "Fee: {}   Signing: {}/{}",
            fee_str, s.signing_progress.signed_inputs, s.signing_progress.total_inputs,
        )),
    ];
    for w in &s.warnings {
        lines.push(Line::styled(
            format!("[!] {}", w),
            Style::default().fg(Color::Yellow),
        ));
    }
    let info = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Summary"));
    frame.render_widget(info, chunks[0]);

    // --- Inputs table ---
    let in_header = Row::new(["#", "Txid:vout", "Value (sats)", "Type", "Sigs", "Address"])
        .style(Style::default().fg(Color::Yellow));
    let in_rows: Vec<Row> = s
        .inputs
        .iter()
        .map(|inp| {
            let txid_short = format!("{}…:{}", inp.txid.get(..8).unwrap_or(&inp.txid), inp.vout);
            let value = inp
                .value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".into());
            let addr = inp.address.as_deref().unwrap_or("n/a").to_string();
            Row::new(vec![
                inp.index.to_string(),
                txid_short,
                value,
                inp.script_type.as_str().to_string(),
                inp.partial_sigs.to_string(),
                addr,
            ])
        })
        .collect();
    let inputs_table = Table::new(
        in_rows,
        [
            Constraint::Length(3),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Min(10),
        ],
    )
    .header(in_header)
    .block(Block::default().borders(Borders::ALL).title("Inputs"));
    frame.render_widget(inputs_table, chunks[1]);

    // --- Outputs table ---
    let out_header = Row::new(["#", "Value (sats)", "Type", "Address"])
        .style(Style::default().fg(Color::Yellow));
    let out_rows: Vec<Row> = s
        .outputs
        .iter()
        .map(|out| {
            let addr = out.address.as_deref().unwrap_or("n/a").to_string();
            Row::new(vec![
                out.index.to_string(),
                out.value.to_string(),
                out.script_type.as_str().to_string(),
                addr,
            ])
        })
        .collect();
    let outputs_table = Table::new(
        out_rows,
        [
            Constraint::Length(3),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Min(10),
        ],
    )
    .header(out_header)
    .block(Block::default().borders(Borders::ALL).title("Outputs"));
    frame.render_widget(outputs_table, chunks[2]);
}
