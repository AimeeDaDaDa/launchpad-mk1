use std::sync::mpsc::Sender;
use log::warn;
use midir::{MidiInputConnection, MidiInputPort};
use crate::{MidiOutputEvent, NoteEvent};
use crate::config::{Config, HoldMode};
use crate::launchpad::Launchpad;
use crate::referential::{Note, Referential};

pub fn get_midi_input_devices() -> Vec<String> {
    let midi_in = midir::MidiInput::new("launchpad-soundpad").unwrap();
    let mut devices = vec![];
    for port in midi_in.ports() {
        let port_name = midi_in.port_name(&port).unwrap();
        devices.push(port_name);
    }
    devices
}

pub fn get_midi_output_devices() -> Vec<String> {
    let midi_out = midir::MidiOutput::new("launchpad-soundpad").unwrap();
    let mut devices = vec![];
    for port in midi_out.ports() {
        let port_name = midi_out.port_name(&port).unwrap();
        devices.push(port_name);
    }
    devices
}

pub fn select_midi_output_device(name: &str) -> Option<midir::MidiOutputConnection> {
    let midi_out = midir::MidiOutput::new("launchpad-soundpad").unwrap();
    for port in midi_out.ports() {
        let port_name = midi_out.port_name(&port).unwrap();
        if port_name == name || port_name.contains(name) {
            return midi_out.connect(&port, "launchpad-soundpad-output").ok();
        }
    }
    None
}

pub fn listen_midi_input(name: &str, tx_on: Sender<NoteEvent>) -> Option<MidiInputConnection<()>> {
    let midi_in = midir::MidiInput::new("launchpad-soundpad").unwrap();
    let mut found_port: Option<MidiInputPort> = None;
    for port in midi_in.ports() {
        let port_name = midi_in.port_name(&port).unwrap();
        if port_name == name || port_name.contains(name) {
            found_port = Some(port);
            break;
        }
    }
    if let Some(port) = found_port {
        let conn = midi_in.connect(&port, "launchpad-soundpad-input", move |_stamp: u64, message: &[u8], _| {
            let tx_on = tx_on.clone();
            let is_on = message[2] == 127;
            if let Err(error) = tx_on.send((message[1], is_on)) {
                warn!("Error sending midi message: {}", error);
            }
        }, ()).ok();
        return conn;
    }
    None
}

pub fn refresh_grid(launchpad: &Launchpad, config: &Config, referential: &mut Referential, tx_midi: &Sender<MidiOutputEvent>, _with_header: bool) {
    let thread_tx_midi = tx_midi.clone();
    // Only the 64 grid pads are wiped; the top row and right column are
    // redrawn below on every call so they survive page switches.
    clear_grid(launchpad, &thread_tx_midi);

    for note in referential.current_page().get_notes().iter() {
        thread_tx_midi.send(note.into()).unwrap();
    }

    if referential.get_nb_pages() > 1 {
        thread_tx_midi.send(Note::white(launchpad.first_page_note()).into()).unwrap();
        thread_tx_midi.send(Note::white(launchpad.last_page_note()).into()).unwrap();
        thread_tx_midi.send(Note::white(launchpad.prev_page_note()).into()).unwrap();
        thread_tx_midi.send(Note::white(launchpad.next_page_note()).into()).unwrap();
    }
    thread_tx_midi.send(Note::white(launchpad.end_session_note()).into()).unwrap();
    match config.get_hold_to_mode() {
        HoldMode::Normal => {
            thread_tx_midi.send(Note::white(launchpad.swap_hold_mode_note()).into()).unwrap();
        }
        HoldMode::Pause => {
            thread_tx_midi.send(Note::yellow(launchpad.swap_hold_mode_note()).into()).unwrap();
        }
        HoldMode::Stop => {
            thread_tx_midi.send(Note::red(launchpad.swap_hold_mode_note()).into()).unwrap();
        }
    }

    thread_tx_midi.send(Note::white(launchpad.stop_note()).into()).unwrap();

    for (i, bookmark_note) in launchpad.bookmark_notes().iter().enumerate() {
        if !config.bookmark_exists(i) {
            continue;
        }
        if referential.is_current_bookmark(*bookmark_note) {
            thread_tx_midi.send(Note::green(*bookmark_note).into()).unwrap();
        } else {
            thread_tx_midi.send(Note::white(*bookmark_note).into()).unwrap();
        }
    }
}

pub fn clear_grid(launchpad: &Launchpad, thread_tx_midi: &Sender<MidiOutputEvent>) {
    for note in launchpad.grid_notes() {
        thread_tx_midi.send(Note::off(note).into()).unwrap();
    }
}

pub mod actions {
    use std::collections::HashMap;
    use std::sync::mpsc::Sender;
    use std::thread;
    use std::time::Duration;
    use rodio::Sink;
    use crate::launchpad::Launchpad;
    use crate::midi::clear_grid;
    use crate::MidiOutputEvent;

    pub fn end_session(launchpad: &Launchpad, tx_midi: &Sender<MidiOutputEvent>) {
        clear_grid(launchpad, tx_midi);
        thread::sleep(Duration::from_millis(100));
    }

    pub fn stop_note(sinks: &mut HashMap<u8, (Option<Sink>, Sink)>) {
        for (audio_sink, virtual_sink) in sinks.values() {
            if let Some(sink) = audio_sink {
                sink.stop();
            }
            virtual_sink.stop();
        }
        sinks.clear();
    }
}