# Yapper on SteamOS — first-run checklist

SteamOS (Arch-based) is a first-class target that has never been exercised. Everything below is a user-assisted pass on the Steam Machine; the code paths marked ✅ are already prepared.

## Build prerequisites (in a distrobox/dev container or after `steamos-readonly disable` — distrobox recommended so updates don't wipe the toolchain)

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
# Tauri v2 Linux deps (Arch names)
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl \
  appmenu-gtk-module libappindicator-gtk3 librsvg cmake
# Node (via pacman or fnm/volta)
sudo pacman -S --needed nodejs npm
```

Then: `git clone git@github.com:Ninjaruss/yapper && cd yapper && npm install && npm run tauri dev`.

## What to verify, in order

1. **App boots** — Candlelit UI renders (CSP + webkit2gtk quirks would show here first).
2. **Mic capture** — devices list (PipeWire → cpal/ALSA); record a short take; level meter moves.
   - ✅ i16 input formats are handled (common on Linux) — `capture.rs` converts via `i16_to_f32`; unsupported formats produce a clear error naming the format. If you hit one, file the format name.
3. **Transcription** — first run downloads the Moonshine model (~250 MB, resumable); segments appear while speaking. CPU-only inference is expected to keep up (Moonshine was ~6× real-time on CPU-class hardware).
4. **Insight** — LLM model downloads (~2 GB, resumable). llama.cpp builds with Vulkan on Linux only if the crate's feature is enabled — we ship **CPU-only on non-macOS** (llama-cpp-2 without the metal feature). A 3B q4 model on the Steam Machine's CPU should manage the 45 s cadence; if passes feel slow (watch the SO FAR panel lag), note timings — enabling the Vulkan feature is the follow-up.
5. **The wisp** — states animate; if the desktop session forces reduced motion, states still swap statically (by design).
6. **End-of-session** — recap renders; WAV converts to FLAC in the background (check `~/Music/Yapper` — XDG music dir — after a minute).
7. **Paths** — models land under `~/.local/share/net.ninjaruss.yapper/models/` (XDG data dir), recordings under the XDG music dir. Both are handled by Tauri's path resolver; verify they exist where expected.

## Known-unknowns to report back
- PipeWire default-device naming oddities in the mic dropdown
- Whether `audio_dir()` resolves on SteamOS (fallback to app-data/audio is coded if not)
- llama.cpp CPU thread count / cadence timing
- Any webkit2gtk rendering glitches in the Candlelit theme
