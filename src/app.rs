use crate::event::AppEvent;
use crate::modules::{
    ai::{client::ask, context::build_context},
    bitcoin::{
        multisig::{MultisigInfo, build_multisig},
        psbt::{PsbtSummary, parse_psbt},
    },
    config::{Config, load_config, parse_network},
    ui::{ai_overlay, builder, inspector},
};
use crate::tui::Tui;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Tabs};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, PartialEq)]
pub enum Tab {
    Inspector,
    Builder,
}

/// Four-state enum — explicit and exhaustive for widget match arms.
#[derive(Debug)]
pub enum PsbtState {
    Empty,
    Loading,
    Ok(PsbtSummary),
    Err(String),
}

#[derive(Debug)]
pub enum MultisigState {
    Empty,
    Loading,
    Ok(MultisigInfo),
    Err(String),
}

#[derive(Debug, PartialEq)]
pub enum BuilderFocus {
    Pubkey1,
    Pubkey2,
}

pub struct AppState {
    active_tab: Tab,
    // Inspector
    pub psbt_input: String,
    pub psbt_state: PsbtState,
    // Builder
    pub pubkey1_input: String,
    pub pubkey2_input: String,
    pub builder_focus: BuilderFocus,
    pub multisig_state: MultisigState,
    // Config
    pub config: Config,
    // AI overlay
    pub ai_open: bool,
    pub ai_question_input: String,
    pub ai_response: String,
    pub ai_loading: bool,
    pub ai_error: Option<String>,
    // Channel
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        AppState {
            active_tab: Tab::Inspector,
            psbt_input: String::new(),
            psbt_state: PsbtState::Empty,
            pubkey1_input: String::new(),
            pubkey2_input: String::new(),
            builder_focus: BuilderFocus::Pubkey1,
            multisig_state: MultisigState::Empty,
            config: load_config().unwrap_or_default(),
            ai_open: false,
            ai_question_input: String::new(),
            ai_response: String::new(),
            ai_loading: false,
            ai_error: None,
            tx,
            rx,
        }
    }

    pub async fn run(&mut self, tui: &mut Tui) -> Result<()> {
        loop {
            while let Ok(event) = self.rx.try_recv() {
                self.handle_event(event);
            }

            tui.terminal.draw(|f| self.draw(f))?;

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && !self.handle_key(key)
            {
                break;
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::PsbtParsed(result) => {
                self.psbt_state = match result {
                    Ok(summary) => PsbtState::Ok(summary),
                    Err(e) => PsbtState::Err(e.to_string()),
                };
            }
            AppEvent::MultisigBuilt(result) => {
                self.multisig_state = match result {
                    Ok(info) => MultisigState::Ok(info),
                    Err(e) => MultisigState::Err(e.to_string()),
                };
            }
            AppEvent::AiChunk(text) => {
                self.ai_response.push_str(&text);
            }
            AppEvent::AiDone => {
                self.ai_loading = false;
            }
            AppEvent::AiError(msg) => {
                self.ai_loading = false;
                self.ai_error = Some(msg);
            }
        }
    }

    /// Returns `true` to keep running, `false` to quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Overlay intercepts all keys while open.
        if self.ai_open {
            return self.handle_ai_key(key);
        }
        // '?' opens AI overlay from any panel.
        if key.code == KeyCode::Char('?') {
            self.ai_open = true;
            return true;
        }
        if key.code == KeyCode::Esc {
            return false;
        }
        if key.code == KeyCode::Tab {
            self.toggle_tab();
            return true;
        }

        match self.active_tab {
            Tab::Inspector => match (key.code, key.modifiers) {
                (KeyCode::Enter, _) => self.start_parse(),
                (KeyCode::Backspace, _) => {
                    self.psbt_input.pop();
                }
                (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                    self.psbt_input.clear();
                    self.psbt_state = PsbtState::Empty;
                }
                (KeyCode::Char(c), _) => self.psbt_input.push(c),
                _ => {}
            },
            Tab::Builder => match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) => return false,
                (KeyCode::Enter, _) => match self.builder_focus {
                    BuilderFocus::Pubkey1 => self.builder_focus = BuilderFocus::Pubkey2,
                    BuilderFocus::Pubkey2 => self.start_multisig_build(),
                },
                (KeyCode::Backspace, _) => match self.builder_focus {
                    BuilderFocus::Pubkey1 => {
                        self.pubkey1_input.pop();
                    }
                    BuilderFocus::Pubkey2 => {
                        self.pubkey2_input.pop();
                    }
                },
                (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                    match self.builder_focus {
                        BuilderFocus::Pubkey1 => self.pubkey1_input.clear(),
                        BuilderFocus::Pubkey2 => self.pubkey2_input.clear(),
                    }
                    self.multisig_state = MultisigState::Empty;
                }
                (KeyCode::Char(c), _) => match self.builder_focus {
                    BuilderFocus::Pubkey1 => self.pubkey1_input.push(c),
                    BuilderFocus::Pubkey2 => self.pubkey2_input.push(c),
                },
                _ => {}
            },
        }
        true
    }

    fn handle_ai_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.ai_open = false;
                self.ai_question_input.clear();
            }
            (KeyCode::Enter, _) => self.start_ai_query(),
            (KeyCode::Backspace, _) => {
                self.ai_question_input.pop();
            }
            (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.ai_question_input.clear();
            }
            (KeyCode::Char(c), _) => self.ai_question_input.push(c),
            _ => {}
        }
        true
    }

    fn toggle_tab(&mut self) {
        self.active_tab = match self.active_tab {
            Tab::Inspector => Tab::Builder,
            Tab::Builder => Tab::Inspector,
        };
    }

    fn start_parse(&mut self) {
        let input = self.psbt_input.trim().to_string();
        if input.is_empty() {
            self.psbt_state = PsbtState::Err("Input is empty — paste a PSBT first".into());
            return;
        }
        self.psbt_state = PsbtState::Loading;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = parse_psbt(&input);
            let _ = tx.send(AppEvent::PsbtParsed(result));
        });
    }

    fn start_multisig_build(&mut self) {
        let pk1 = self.pubkey1_input.trim().to_string();
        let pk2 = self.pubkey2_input.trim().to_string();
        if pk1.is_empty() || pk2.is_empty() {
            self.multisig_state = MultisigState::Err("Both public keys are required".into());
            return;
        }
        self.multisig_state = MultisigState::Loading;
        let network = parse_network(&self.config.network).unwrap_or(bitcoin::Network::Testnet);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = build_multisig(&pk1, &pk2, network, true);
            let _ = tx.send(AppEvent::MultisigBuilt(result));
        });
    }

    fn start_ai_query(&mut self) {
        let question = self.ai_question_input.trim().to_string();
        if question.is_empty() {
            return;
        }

        let psbt_ref = match &self.psbt_state {
            PsbtState::Ok(s) => Some(s),
            _ => None,
        };
        let multisig_ref = match &self.multisig_state {
            MultisigState::Ok(s) => Some(s),
            _ => None,
        };
        let context = build_context(psbt_ref, multisig_ref);

        self.ai_response.clear();
        self.ai_error = None;
        self.ai_loading = true;

        let api_key = self.config.api_key.clone();
        let model = self.config.ai_model.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match ask(&api_key, &model, &context, &question).await {
                Ok(text) => {
                    let _ = tx.send(AppEvent::AiChunk(text));
                    let _ = tx.send(AppEvent::AiDone);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::AiError(e.to_string()));
                }
            }
        });
    }

    fn draw(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        let selected = match self.active_tab {
            Tab::Inspector => 0,
            Tab::Builder => 1,
        };
        let tabs = Tabs::new(vec!["Inspector", "Builder"])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("PSBT Inspector  [Tab] switch  [?] AI  [Esc] quit"),
            )
            .select(selected)
            .highlight_style(Style::default().fg(Color::Yellow));
        frame.render_widget(tabs, chunks[0]);

        match self.active_tab {
            Tab::Inspector => inspector::draw(frame, chunks[1], &self.psbt_state, &self.psbt_input),
            Tab::Builder => builder::draw(
                frame,
                chunks[1],
                &self.pubkey1_input,
                &self.pubkey2_input,
                &self.builder_focus,
                &self.multisig_state,
            ),
        }

        if self.ai_open {
            let psbt_ref = match &self.psbt_state {
                PsbtState::Ok(s) => Some(s),
                _ => None,
            };
            let multisig_ref = match &self.multisig_state {
                MultisigState::Ok(s) => Some(s),
                _ => None,
            };
            let context = build_context(psbt_ref, multisig_ref);
            ai_overlay::draw(
                frame,
                area,
                &self.ai_question_input,
                &self.ai_response,
                self.ai_loading,
                self.ai_error.as_deref(),
                &context,
            );
        }
    }
}
