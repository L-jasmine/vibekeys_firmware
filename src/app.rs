use crate::{
    lcd,
    protocol::{self},
};

pub enum Event {
    MicAudioChunk(Vec<i16>),
    MicAudioChunkEnd,
    Accept,
    Esc,
    RotateUp,
    RotateDown,
    RotatePush,
    Swap,
    K0,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::MicAudioChunk(_) => write!(f, "MicAudioChunk(...)"),
            Event::MicAudioChunkEnd => write!(f, "MicAudioChunkEnd"),
            Event::Accept => write!(f, "Accept"),
            Event::Esc => write!(f, "Esc"),
            Event::RotateUp => write!(f, "RotateUp"),
            Event::RotateDown => write!(f, "RotateDown"),
            Event::RotatePush => write!(f, "RotatePush"),
            Event::Swap => write!(f, "Swap"),
            Event::K0 => write!(f, "K0"),
        }
    }
}

enum SelectResult {
    Event(Event),
    ServerMessage(protocol::ServerMessage),
}

async fn select_event(
    server: &mut crate::ws::Server,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
) -> Option<SelectResult> {
    tokio::select! {
        Some(evt) = rx.recv() => {
            Some(SelectResult::Event(evt))
        },
        Some(msg) = server.recv() => {
            Some(SelectResult::ServerMessage(msg))
        },
        else => None,
    }
}

pub async fn run(
    uri: String,
    ui: &mut crate::lcd::UI,
    mut rx: tokio::sync::mpsc::Receiver<Event>,
) -> anyhow::Result<()> {
    let mut server = crate::ws::Server::new(uri).await?;
    let mut start_submit_audio = false;

    ui.show_notification(
        lcd::NotificationLevel::Info,
        "Server Connected",
        Some("Success"),
    )?;
    ui.start_input("Ready for input")?;

    while let Some(evt) = select_event(&mut server, &mut rx).await {
        match evt {
            SelectResult::Event(e) => {
                match e {
                    Event::MicAudioChunk(chunk) => {
                        if !start_submit_audio {
                            start_submit_audio = true;
                            log::info!("Starting to submit audio chunks to server");
                            server
                                .send(protocol::ClientMessage::voice_input_start(Some(16000)))
                                .await?;
                        }
                        let audio_buffer_u8 = unsafe {
                            std::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 2)
                        };
                        server
                            .send(protocol::ClientMessage::voice_input_chunk(
                                audio_buffer_u8.to_vec(),
                            ))
                            .await?;
                    }
                    Event::MicAudioChunkEnd => {
                        start_submit_audio = false;
                        server
                            .send(protocol::ClientMessage::voice_input_end())
                            .await?;
                    }
                    evt => {
                        log::info!("Received event: {:?}", evt);

                        match ui.state() {
                            lcd::UiState::WaitingInput { .. } => {
                                ui.handle_key_event_on_waiting_input(evt, &mut server)
                                    .await?;
                            }
                            lcd::UiState::WaitingChoice { .. } => {
                                ui.handle_key_event_on_choice_selection(evt, &mut server)
                                    .await?;
                            }
                            lcd::UiState::ShowingNotification { .. } => {
                                ui.handle_key_event_on_displaying_text(evt, &mut server)
                                    .await?;
                            }
                            lcd::UiState::ShowingText { .. } => {
                                ui.handle_key_event_on_displaying_text(evt, &mut server)
                                    .await?;
                            }
                            _ => {
                                log::info!("Received event {:?} in state {:?}, handling with default handler", evt, ui.state());
                            }
                        }
                    }
                }
            }
            SelectResult::ServerMessage(msg) => match msg {
                protocol::ServerMessage::PtyOutput(..) => {
                    log::debug!("Received PTY output, ignoring for now");
                    continue;
                }
                msg => {
                    let ui_msg = lcd::UiMessage::from(msg);
                    ui.handle_message(ui_msg)?;
                }
            },
        }
    }

    Ok(())
}

impl lcd::UI {
    async fn handle_key_event(
        &mut self,
        evt: Event,
        server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        todo!()
    }

    async fn handle_key_event_on_waiting_input(
        &mut self,
        evt: Event,
        server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        match evt {
            Event::K0 => {
                self.remove_input_char()?;
            }
            Event::Esc => {
                self.clear_input()?;
            }
            Event::RotateDown => {
                self.move_cursor_right()?;
            }
            Event::RotateUp => {
                self.move_cursor_left()?;
            }
            Event::Swap => {
                self.remove_input_char()?;
            }
            Event::Accept => {
                let input = self.get_input().unwrap_or_default();
                if input.is_empty() {
                    log::info!("Input is empty, ignoring submit");
                    return Ok(());
                }
                log::info!("Submitting input: {}", input);
                server.send(protocol::ClientMessage::input(input)).await?;
            }
            _ => {
                log::warn!("Unexpected event in WaitingInput state");
            }
        }

        Ok(())
    }

    async fn handle_key_event_on_choice_selection(
        &mut self,
        evt: Event,
        server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        match evt {
            Event::RotateDown => {
                self.scroll_down()?;
            }
            Event::RotateUp => {
                self.scroll_up()?;
            }
            Event::RotatePush => {
                self.reset_scroll()?;
            }
            Event::K0 => self.next_choice()?,
            Event::Accept => {
                if self.is_confirm_dialog() {
                    server
                        .send(protocol::ClientMessage::pty_input(b"\r".to_vec()))
                        .await?;
                } else {
                    let choice = self.confirm_choice().unwrap_or(0);
                    log::info!("Selected choice index: {}", choice);
                    server.send(protocol::ClientMessage::choice(choice)).await?;
                }
            }
            Event::Esc => {
                if self.is_confirm_dialog() {
                    server
                        .send(protocol::ClientMessage::pty_input(b"\x1b".to_vec()))
                        .await?;
                }
            }
            _ => {
                log::warn!("Unexpected event in ChoiceSelection state");
            }
        }

        Ok(())
    }

    async fn handle_key_event_on_displaying_text(
        &mut self,
        evt: Event,
        _server: &mut crate::ws::Server,
    ) -> anyhow::Result<()> {
        match evt {
            Event::RotateDown => {
                self.scroll_down()?;
            }
            Event::RotateUp => {
                self.scroll_up()?;
            }
            Event::RotatePush => {
                self.reset_scroll()?;
            }
            Event::Accept => {
                self.scroll_up()?;
            }
            _ => {
                log::warn!("Unexpected event in DisplayingText state");
            }
        }

        Ok(())
    }
}
