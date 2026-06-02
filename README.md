# PAS Broadcaster

Native Rust desktop broadcaster for Public Address System RTP L16 PCM audio over administratively scoped IPv4 multicast groups.

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
- Lazy FFmpeg executable setup plus WAV conversion with editable PAS-safe tunables.
- Microphone live broadcast using `cpal`.
- Selectable IPv4 network interface for multicast egress.

## FFmpeg Converter

The converter downloads or reuses an FFmpeg executable only when conversion is invoked. The default
conversion preset is equivalent to:

```sh
ffmpeg -y \
  -i "$IN" \
  -map 0:a:0 \
  -vn -sn -dn \
  -af "adelay=150:all=1,volume=-6dB,afade=t=in:st=0.15:d=0.10" \
  -ar 44100 \
  -ac 2 \
  -c:a pcm_s16le \
  -f wav \
  "$OUT"
```

The app can convert only, convert and immediately broadcast on the selected channel, or save a copy
of the last converted WAV through a desktop save dialog.

## Run

```sh
cargo run
```

## Test

```sh
cargo test
```

## Tagged Release Builds

Pushing a tag that matches `v*` runs the GitHub Actions release workflow for Windows and Linux.
The workflow builds portable x64 zip artifacts and publishes them to the matching GitHub Release.

Receivers must be statically configured to join the selected multicast group and port and decode RTP L16 PCM using the same sample rate, channels, and bit depth settings.
