use std::fs::File;
use std::io::BufReader;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{OutputStream, OutputStreamBuilder, Sink, Source};

pub fn get_output_devices() -> Vec<String> {
  let host = cpal::default_host();
  host.devices().unwrap().map(|device| device.name().unwrap()).collect()
}

pub fn select_output_device(name: &str) -> Option<OutputStream> {
  let host = cpal::default_host();
  // An empty name means "system default"; otherwise match by substring so
  // that names like "Voicemeeter In 3 (VB-Audio Voicemeeter VAIO)" resolve
  // even when the trailing part differs between audio backends.
  let output_device = if name.is_empty() {
    host.default_output_device()
  } else {
    host.devices().unwrap().find(|device| {
      let device_name = device.name().unwrap_or_default();
      device_name == name || device_name.contains(name)
    })
  };
  if let Some(device) = output_device {
    OutputStreamBuilder::from_device(device).unwrap().open_stream().ok()
  } else {
    None
  }
}

pub fn play_sound(handle: &OutputStream, path: &str, volume: f32) -> (Sink, Option<Duration>) {
  let reader = BufReader::new(File::open(path).unwrap());
  let source = rodio::Decoder::new(reader).unwrap()
    .amplify(volume);

  let sink = Sink::connect_new(handle.mixer());
  let duration = source.total_duration();
  sink.append(source);

  (sink, duration)
}