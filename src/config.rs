use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    midi_in_device: Option<String>,
    midi_out_device: Option<String>,
    output_device: Option<String>,
    virtual_device: Option<String>,
    /// 本地监听开关：false/0/no/off 时完全不建本地输出流，声音只送 virtual_device
    local_monitor: Option<String>,
    /// 送 virtual_device（VoiceMeeter）的音量，缺省 0.10（与上游一致）
    virtual_volume: Option<String>,
    bookmark_1: Option<String>,
    bookmark_2: Option<String>,
    bookmark_3: Option<String>,
    bookmark_4: Option<String>,
    bookmark_5: Option<String>,
    bookmark_6: Option<String>,
    bookmark_7: Option<String>,
    hold_to: HoldMode,
}

#[derive(Deserialize, Debug, PartialEq)]
pub(crate) enum HoldMode {
    Normal,
    Pause,
    Stop
}

impl Config {
    // TODO: Create an init function

    pub fn get_midi_in_device(&self) -> Option<&str> {
        self.midi_in_device.as_deref()
    }

    pub fn get_midi_out_device(&self) -> Option<&str> {
        self.midi_out_device.as_deref()
    }

    pub fn get_output_device(&self) -> Option<&str> {
        self.output_device.as_deref()
    }

    pub fn get_virtual_device(&self) -> Option<String> {
        self.virtual_device.clone()
    }

    pub fn get_bookmark(&self, index: usize) -> Option<String> {
        match index {
            0 => self.bookmark_1.clone(),
            1 => self.bookmark_2.clone(),
            2 => self.bookmark_3.clone(),
            3 => self.bookmark_4.clone(),
            4 => self.bookmark_5.clone(),
            5 => self.bookmark_6.clone(),
            6 => self.bookmark_7.clone(),
            _ => None
        }
    }

    pub fn bookmark_exists(&self, index: usize) -> bool {
        match index {
            0 => self.bookmark_1.is_some(),
            1 => self.bookmark_2.is_some(),
            2 => self.bookmark_3.is_some(),
            3 => self.bookmark_4.is_some(),
            4 => self.bookmark_5.is_some(),
            5 => self.bookmark_6.is_some(),
            6 => self.bookmark_7.is_some(),
            _ => false
        }
    }

    pub fn get_hold_to_mode(&self) -> &HoldMode {
        &self.hold_to
    }

    /// 本地监听是否开启。缺省 true（保持上游行为）；写成 false/0/no/off 则只走虚拟设备。
    pub fn local_monitor_enabled(&self) -> bool {
        match self.local_monitor.as_deref() {
            None => true,
            Some(raw) => !matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off" | "none" | "disabled" | ""
            ),
        }
    }

    /// 送虚拟设备（VoiceMeeter）的音量。缺省 0.10，可写成 0.0 ~ 2.0。
    pub fn virtual_volume(&self) -> f32 {
        self.virtual_volume
            .as_deref()
            .and_then(|raw| raw.trim().parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 2.0))
            .unwrap_or(0.10)
    }

    pub fn swap_hold_to_play(&mut self) {
        self.hold_to = match self.hold_to {
            HoldMode::Normal => HoldMode::Stop,
            HoldMode::Stop => HoldMode::Pause,
            HoldMode::Pause => HoldMode::Normal,
        }
    }
}