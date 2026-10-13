#[derive(Debug)]
pub enum Launchpad {
  MiniMk1,
  MiniMk2,
  MiniMk3,
  X
}

impl Launchpad {
  /// The 64 grid notes. MK1 uses `16*row + col` (so the top-left pad is
  /// note 0), MK2/MK3/X use 11..88 skipping every 9th value.
  pub(crate) fn grid_notes(&self) -> Vec<u8> {
    match self {
      Launchpad::MiniMk1 => {
        let mut notes = Vec::with_capacity(64);
        for row in 0..8u8 {
          for col in 0..8u8 {
            notes.push(16 * row + col);
          }
        }
        notes
      }
      _ => (11..=88u8).filter(|n| n % 10 != 9).collect(),
    }
  }

  pub(crate) fn first_page_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 | Launchpad::MiniMk2 => 104,
      Launchpad::MiniMk3 | Launchpad::X => 91
    }
  }

  pub(crate) fn last_page_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 | Launchpad::MiniMk2 => 105,
      Launchpad::MiniMk3 | Launchpad::X => 92
    }
  }

  pub(crate) fn prev_page_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 | Launchpad::MiniMk2 => 106,
      Launchpad::MiniMk3 | Launchpad::X => 93
    }
  }

  pub(crate) fn next_page_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 | Launchpad::MiniMk2 => 107,
      Launchpad::MiniMk3 | Launchpad::X => 94
    }
  }

  pub(crate) fn end_session_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 | Launchpad::MiniMk2 => 108,
      Launchpad::MiniMk3 | Launchpad::X => 95
    }
  }

  /// MK1: top-row CC 109 (104-108 and 111 are already taken by the page
  /// controls). MK2/MK3/X: note 19.
  pub(crate) fn stop_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 => 109,
      _ => 19
    }
  }

  /// MK1 right-hand column is column 8 of the X-Y layout, i.e. notes
  /// 8, 24, 40, 56, 72, 88, 104, 120. Note 104 collides with the top-row
  /// CC 104 (first page), so it is left unbound.
  pub(crate) fn bookmark_notes<'array>(&self) -> &'array[u8] {
    match self {
      Launchpad::MiniMk1 => &[8, 24, 40, 56, 72, 88, 120],
      _ => &[89, 79, 69, 59, 49, 39, 29]
    }
  }

  pub(crate) fn programmer_mode_command<'array>(&self) -> &'array[u8] {
    match self {
      // MK1 has no SysEx programmer mode. B0 00 01 just re-selects the
      // default X-Y grid mapping, which is what the probe observed.
      Launchpad::MiniMk1 => &[176, 0, 1],
      Launchpad::MiniMk2 => &[240, 0, 32, 41, 2, 24, 34, 0, 247],
      Launchpad::MiniMk3 => &[240, 0, 32, 41, 2, 13, 14, 1, 247],
      Launchpad::X => &[240, 0, 32, 41, 2, 12, 127, 247]
    }
  }

  pub(crate) fn swap_hold_mode_note(&self) -> u8 {
    match self {
      Launchpad::MiniMk1 | Launchpad::MiniMk2 => 111,
      Launchpad::MiniMk3 | Launchpad::X => 98
    }
  }

  /// Build the raw MIDI message that lights `note_id`.
  ///
  /// MK2/MK3/X address an RGB palette by index and use channel 2 for the
  /// blinking "now playing" state. MK1 is a bi-colour pad whose velocity
  /// encodes `16*green + red + 12`, so the palette indices used internally
  /// have to be translated, and there is no blink channel, so "now playing"
  /// is solid amber instead.
  pub fn led_message(&self, note_id: u8, color: u8, should_blink: bool) -> [u8; 3] {
    match self {
      Launchpad::MiniMk1 => {
        let velocity = if should_blink {
          63 // amber full
        } else {
          match color {
            0 => 12,                  // off
            3 => 60,                  // white  -> green full
            5 => 15,                  // red
            13 => 62,                 // yellow
            87 => 60,                 // green
            other => other.min(127),  // page-specified MK1 velocity, pass through
          }
        };
        [144, note_id, velocity]
      }
      _ => {
        let channel = if should_blink { 145 } else { 144 };
        let color = if should_blink { 3 } else { color };
        [channel, note_id, color]
      }
    }
  }

  pub fn get_launchpad(name: &str) -> Launchpad {
    if name.contains("LPMiniMK2") {
      Launchpad::MiniMk2
    } else if name.contains("LPMiniMK3") {
      Launchpad::MiniMk3
    } else if name.contains("LPX") {
      Launchpad::X
    } else if name.contains("Launchpad Mini") || name.contains("LPMini") {
      Launchpad::MiniMk1
    } else {
      panic!("Launchpad not found")
    }
  }
}
