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
- RTP dynamic payload carrying big-endian linear PCM: L16 (16-bit, payload type 96) or L24 (24-bit, payload type 97, RFC 3190).
- Default audio profile: 16 kHz, mono, 16-bit PCM.
- WAV and MP3 file playback.
- Lazy FFmpeg executable setup plus WAV conversion with editable PAS-safe tunables, including optional highpass/lowpass band-limiting.
- Microphone live broadcast using `cpal`.
- Selectable IPv4 network interface for multicast egress.
- **Target hardware device profiles** (see below).

## Device Profiles

A device profile bundles everything needed to feed a particular receiver: the
broadcast audio format (sample rate, channels, bit depth, packet duration), the
FFmpeg re-encode settings (codec, band-limiting, output suffix), and network
defaults (RTP payload type, suggested multicast group/port). Selecting a profile
aligns both the live RTP stream and the file re-encode to what the device
expects, so the hardware "just works".

- Built-in profiles ship in `assets/device_profiles.toml` and are merged with
  your own profiles at every launch. The first built-in targets the **ATEIS
  BOUTIQUE BTQ-VM4/VM8** (48 kHz / 24-bit, 50 Hz–18 kHz band-limited, L24).
- Built-in profiles are read-only; clone one to customize it. User profiles are
  saved to `config.toml`; built-ins are never persisted, so updates to the
  bundled asset reach you automatically unless you cloned that profile id.
- Apply a profile from the **Profiles** page, or from the selector on the
  **Broadcast** and **Converter** pages.

> Note: the BTQ-VM datasheet confirms a 48 kHz / 24-bit DSP and IP streaming, but
> not the exact RTP payload type it expects on the wire. The payload type is
> user-editable per profile (placeholder `97`); confirm it against the device's
> protocol documentation before relying on live L24 streaming.

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

Receivers must be statically configured to join the selected multicast group and port and decode the RTP PCM stream using the same sample rate, channels, and bit depth. Applying a matching device profile is the easiest way to keep the broadcaster and receiver in sync.
