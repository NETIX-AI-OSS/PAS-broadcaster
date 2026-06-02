mod app;
mod audio;
mod broadcast;
mod config;
mod converter;
mod log;
mod network;
mod rtp;
mod validation;

fn main() -> iced::Result {
    app::PasBroadcaster::run()
}
