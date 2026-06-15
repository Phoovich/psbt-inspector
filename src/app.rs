use crate::event::AppEvent;
use crate::modules::{
    ai::{
        client::{ask, build_client},
        context::build_context,
    },
    bitcoin::{
        multisig::{MultisigInfo, build_multisig},
        psbt::{PsbtSummary, parse_psbt},
    },
    config::{Config, load_config, parse_network},
    ui::{ai_overlay, builder, inspector},
};
use crate::tui::Tui;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Tabs};
use tokio::sync::mpsc;

/// Cap on pasted PSBT input (S9) — a multi-MB paste would otherwise be
/// re-wrapped/decoded on every keystroke with no benefit to the user.
const MAX_PSBT_INPUT_LEN: usize = 4 * 1024 * 1024;

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

/// What `start_ai_query` should do about sending PSBT/multisig context,
/// given whether a PSBT/multisig is loaded, the `ai_send_context` config
/// flag, and any prior session consent decision (S6).
#[derive(Debug, PartialEq)]
enum ConsentDecision {
    /// Ask the user for consent before sending anything.
    Prompt,
    /// Send the built context along with the question.
    Send,
    /// Send the question without context (no consent needed).
    Withhold,
}

/// Pure decision table for the S6 consent flow. `consent` is `None` until
/// the user answers the first prompt of the session.
fn consent_decision(
    has_context: bool,
    send_context_cfg: bool,
    consent: Option<bool>,
) -> ConsentDecision {
    if !has_context || !send_context_cfg {
        return ConsentDecision::Withhold;
    }
    match consent {
        None => ConsentDecision::Prompt,
        Some(true) => ConsentDecision::Send,
        Some(false) => ConsentDecision::Withhold,
    }
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
    /// App-level warnings from config load (bad permissions, unrecognised
    /// network) — shown in the title bar. Distinct from per-PSBT warnings.
    pub startup_warnings: Vec<String>,
    // AI overlay
    pub ai_open: bool,
    pub ai_question_input: String,
    pub ai_response: String,
    pub ai_loading: bool,
    pub ai_error: Option<String>,
    ai_client: reqwest::Client,
    /// Incremented on every AI query; tags the spawned task so stale
    /// responses (e.g. after Esc cancels the overlay) are dropped.
    ai_generation: u64,
    /// Session-level consent to send PSBT/multisig context to the AI.
    /// `None` until the user answers the first consent prompt.
    pub ai_consent: Option<bool>,
    /// `true` while the consent prompt is shown, blocking the query.
    pub ai_consent_pending: bool,
    /// Cached `build_context(...)` output — rebuilt only when `psbt_state`
    /// or `multisig_state` changes, instead of every frame (P2).
    ai_context: String,
    // Channel
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (config, startup_warnings) =
            load_config().unwrap_or_else(|_| (Config::default(), Vec::new()));
        let ai_context = build_context(None, None);
        AppState {
            active_tab: Tab::Inspector,
            psbt_input: String::new(),
            psbt_state: PsbtState::Empty,
            pubkey1_input: String::new(),
            pubkey2_input: String::new(),
            builder_focus: BuilderFocus::Pubkey1,
            multisig_state: MultisigState::Empty,
            config,
            startup_warnings,
            ai_open: false,
            ai_question_input: String::new(),
            ai_response: String::new(),
            ai_loading: false,
            ai_error: None,
            ai_client: build_client(),
            ai_generation: 0,
            ai_consent: None,
            ai_consent_pending: false,
            ai_context,
            tx,
            rx,
        }
    }

    pub async fn run(&mut self, tui: &mut Tui) -> Result<()> {
        use crossterm::event::EventStream;
        use futures_util::StreamExt;

        let mut events = EventStream::new();
        tui.terminal.draw(|f| self.draw(f))?;

        loop {
            let mut redraw = false;

            tokio::select! {
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                            if !self.handle_key(key) {
                                break;
                            }
                            redraw = true;
                        }
                        Some(Ok(Event::Resize(_, _))) => redraw = true,
                        Some(Ok(_)) => {}
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    }
                }
                Some(app_event) = self.rx.recv() => {
                    self.handle_event(app_event);
                    while let Ok(event) = self.rx.try_recv() {
                        self.handle_event(event);
                    }
                    redraw = true;
                }
            }

            if redraw {
                tui.terminal.draw(|f| self.draw(f))?;
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
                self.rebuild_context();
            }
            AppEvent::MultisigBuilt(result) => {
                self.multisig_state = match result {
                    Ok(info) => MultisigState::Ok(info),
                    Err(e) => MultisigState::Err(e.to_string()),
                };
                self.rebuild_context();
            }
            AppEvent::AiChunk(generation, text) => {
                if generation == self.ai_generation {
                    self.ai_response.push_str(&text);
                }
            }
            AppEvent::AiDone(generation) => {
                if generation == self.ai_generation {
                    self.ai_loading = false;
                }
            }
            AppEvent::AiError(generation, msg) => {
                if generation == self.ai_generation {
                    self.ai_loading = false;
                    self.ai_error = Some(msg);
                }
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
                    self.rebuild_context();
                }
                (KeyCode::Char(c), _) if self.psbt_input.len() < MAX_PSBT_INPUT_LEN => {
                    self.psbt_input.push(c);
                }
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
                    self.rebuild_context();
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
        if self.ai_consent_pending {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.ai_consent = Some(true);
                    self.ai_consent_pending = false;
                    self.start_ai_query();
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.ai_consent = Some(false);
                    self.ai_consent_pending = false;
                    self.start_ai_query();
                }
                KeyCode::Esc => {
                    self.ai_consent_pending = false;
                }
                _ => {}
            }
            return true;
        }
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

    /// Recompute the cached AI context from the current PSBT/multisig state.
    /// Call whenever `psbt_state` or `multisig_state` changes (P2).
    fn rebuild_context(&mut self) {
        let psbt_ref = match &self.psbt_state {
            PsbtState::Ok(s) => Some(s),
            _ => None,
        };
        let multisig_ref = match &self.multisig_state {
            MultisigState::Ok(s) => Some(s),
            _ => None,
        };
        self.ai_context = build_context(psbt_ref, multisig_ref);
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
        tokio::task::spawn_blocking(move || {
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
        // S7: load_config() already validated `network`, falling back to
        // "testnet" with a startup warning if it was unrecognised — this
        // unwrap is just decoding that already-valid string.
        let network =
            parse_network(&self.config.network).expect("config.network validated at load");
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = build_multisig(&pk1, &pk2, network, true);
            let _ = tx.send(AppEvent::MultisigBuilt(result));
        });
    }

    fn start_ai_query(&mut self) {
        let question = self.ai_question_input.trim().to_string();
        if question.is_empty() {
            return;
        }
        if self.ai_loading {
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
        let has_context = psbt_ref.is_some() || multisig_ref.is_some();

        // S6: ask for session-level consent before the first context-bearing
        // request. `ai_send_context = false` skips straight to "no context".
        let context =
            match consent_decision(has_context, self.config.ai_send_context, self.ai_consent) {
                ConsentDecision::Prompt => {
                    self.ai_consent_pending = true;
                    return;
                }
                ConsentDecision::Send => self.ai_context.clone(),
                ConsentDecision::Withhold => String::new(),
            };

        self.ai_response.clear();
        self.ai_error = None;
        self.ai_loading = true;

        self.ai_generation += 1;
        let generation = self.ai_generation;

        let api_key = self.config.api_key.clone();
        let model = self.config.ai_model.clone();
        let client = self.ai_client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match ask(
                &client, &api_key, &model, &context, &question, &tx, generation,
            )
            .await
            {
                Ok(()) => {
                    let _ = tx.send(AppEvent::AiDone(generation));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::AiError(generation, e.to_string()));
                }
            }
        });
    }

    fn draw(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let title_height = if self.startup_warnings.is_empty() {
            3
        } else {
            4
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(title_height), Constraint::Min(0)])
            .split(area);

        let selected = match self.active_tab {
            Tab::Inspector => 0,
            Tab::Builder => 1,
        };
        let title = format!(
            "PSBT Inspector — network: {}  [Tab] switch  [?] AI  [Esc] quit",
            self.config.network
        );
        let tabs = Tabs::new(vec!["Inspector", "Builder"])
            .block(Block::default().borders(Borders::ALL).title(title))
            .select(selected)
            .highlight_style(Style::default().fg(Color::Yellow));
        frame.render_widget(tabs, chunks[0]);

        if !self.startup_warnings.is_empty() {
            let warning_text = self.startup_warnings.join("; ");
            let warning_area = ratatui::layout::Rect {
                x: chunks[0].x + 1,
                y: chunks[0].y + 2,
                width: chunks[0].width.saturating_sub(2),
                height: 1,
            };
            frame.render_widget(
                ratatui::widgets::Paragraph::new(warning_text)
                    .style(Style::default().fg(Color::Red)),
                warning_area,
            );
        }

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
            ai_overlay::draw(
                frame,
                area,
                &self.ai_question_input,
                &self.ai_response,
                self.ai_loading,
                self.ai_error.as_deref(),
                &self.ai_context,
                self.ai_consent_pending,
            );
        }
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(test)]
impl AppState {
    /// Builds an `AppState` with `Config::default()` and no real config I/O,
    /// for unit-testing event handling and decision logic in isolation.
    fn new_for_test() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        AppState {
            active_tab: Tab::Inspector,
            psbt_input: String::new(),
            psbt_state: PsbtState::Empty,
            pubkey1_input: String::new(),
            pubkey2_input: String::new(),
            builder_focus: BuilderFocus::Pubkey1,
            multisig_state: MultisigState::Empty,
            config: Config::default(),
            startup_warnings: Vec::new(),
            ai_open: false,
            ai_question_input: String::new(),
            ai_response: String::new(),
            ai_loading: false,
            ai_error: None,
            ai_client: build_client(),
            ai_generation: 0,
            ai_consent: None,
            ai_consent_pending: false,
            ai_context: build_context(None, None),
            tx,
            rx,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── H5: S6 consent decision ──────────────────────────────────────────────

    #[test]
    fn consent_decision_prompts_on_first_context_query() {
        assert_eq!(consent_decision(true, true, None), ConsentDecision::Prompt);
    }

    #[test]
    fn consent_decision_sends_when_consented() {
        assert_eq!(
            consent_decision(true, true, Some(true)),
            ConsentDecision::Send
        );
    }

    #[test]
    fn consent_decision_withholds_when_declined() {
        assert_eq!(
            consent_decision(true, true, Some(false)),
            ConsentDecision::Withhold
        );
    }

    #[test]
    fn consent_decision_withholds_when_opted_out() {
        assert_eq!(
            consent_decision(true, false, None),
            ConsentDecision::Withhold
        );
        assert_eq!(
            consent_decision(true, false, Some(true)),
            ConsentDecision::Withhold
        );
    }

    #[test]
    fn consent_decision_withholds_when_no_context() {
        assert_eq!(
            consent_decision(false, true, None),
            ConsentDecision::Withhold
        );
    }

    // ─── H4: S8 generation counter ────────────────────────────────────────────

    #[test]
    fn stale_ai_chunk_is_dropped() {
        let mut app = AppState::new_for_test();
        app.ai_generation = 2;
        app.handle_event(AppEvent::AiChunk(1, "stale".into()));
        assert_eq!(app.ai_response, "");
        app.handle_event(AppEvent::AiChunk(2, "current".into()));
        assert_eq!(app.ai_response, "current");
    }

    #[test]
    fn stale_ai_done_does_not_clear_loading() {
        let mut app = AppState::new_for_test();
        app.ai_generation = 2;
        app.ai_loading = true;
        app.handle_event(AppEvent::AiDone(1));
        assert!(app.ai_loading);
        app.handle_event(AppEvent::AiDone(2));
        assert!(!app.ai_loading);
    }

    #[test]
    fn stale_ai_error_is_dropped() {
        let mut app = AppState::new_for_test();
        app.ai_generation = 2;
        app.ai_loading = true;
        app.handle_event(AppEvent::AiError(1, "stale error".into()));
        assert!(app.ai_loading);
        assert!(app.ai_error.is_none());
        app.handle_event(AppEvent::AiError(2, "real error".into()));
        assert!(!app.ai_loading);
        assert_eq!(app.ai_error, Some("real error".into()));
    }

    // ─── S8: double-spawn guard ────────────────────────────────────────────────

    #[test]
    fn start_ai_query_is_noop_while_loading() {
        let mut app = AppState::new_for_test();
        app.ai_loading = true;
        app.ai_question_input = "question".into();
        let generation_before = app.ai_generation;
        app.start_ai_query();
        assert_eq!(app.ai_generation, generation_before);
    }

    #[test]
    fn start_ai_query_is_noop_for_empty_question() {
        let mut app = AppState::new_for_test();
        app.ai_question_input = "   ".into();
        let generation_before = app.ai_generation;
        app.start_ai_query();
        assert_eq!(app.ai_generation, generation_before);
        assert!(!app.ai_loading);
    }
}
