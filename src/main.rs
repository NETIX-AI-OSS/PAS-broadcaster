mod app;
mod audio;
mod broadcast;
mod config;
mod network;
mod rtp;
mod validation;

use iced::{Application, Settings};

fn main() -> iced::Result {
    app::FasBroadcaster::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
}
