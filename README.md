# FAS Broadcaster

Native Rust desktop broadcaster for RTP L16 PCM audio over administratively scoped IPv4 multicast groups.

## Features

- Cross-platform `iced` desktop UI for Windows, Linux, and macOS.
- Four default broadcast channels:
  - `239.10.10.1:5004` - General Announcement
  - `239.10.10.2:5004` - Platform Area
  - `239.10.10.3:5004` - Concourse Area
  - `239.10.10.4:5004` - Emergency Broadcast
- Add, edit, delete, enable, and persist extra channels.
- RTP dynamic payload type 96 carrying big-endian L16 PCM.
- Default audio profile: 16 kHz, mono, 16-bit PCM.
- WAV and MP3 file playback.
- Microphone live broadcast using `cpal`.
- Selectable IPv4 network interface for multicast egress.

## Run

```sh
cargo run
```

## Test

```sh
cargo test
```

Receivers must be statically configured to join the selected multicast group and port and decode RTP L16 PCM using the same sample rate, channels, and bit depth settings.
