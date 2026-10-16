# Tidash

A fast, native desktop client for TIDAL, built in Rust with egui on [fastframe](https://github.com/crmne/fastframe). It has no browser engine, and it aims for low memory and zero CPU while idle or paused.

## Legal stance

Using an official client ID breaches TIDAL's ToS and can be revoked at any time. One is built in; you can enter a replacement client ID and redirect URL in Settings › Account, which the login page also leads to. The name "Tidash" (TIDAL plus dash, for fast) plays on TIDAL's trademark, as Spotifast does on Spotify's; rename if challenged.

## Install

Download a release from [the releases page](https://github.com/rojnwa/tidash/releases). Builds are not code-signed yet.

- **Windows** (x64 or ARM64): the installer (`…-setup.exe`) installs Tidash for your user, with a Start menu entry and `tidal://` links; the zip is portable, so unpack it anywhere and run `tidash.exe`. SmartScreen warns about an unsigned program: More info › Run anyway.
- **macOS** (11 or later, Apple silicon or Intel): open the DMG and drag Tidash to Applications, where it can update itself. macOS refuses the first open of an unsigned app: open it once anyway in System Settings › Privacy & Security › Open Anyway.
- **Linux** (x64 or ARM64, glibc 2.39 or later): the deb, the rpm, the AppImage, or the tar.gz (unpack it anywhere and run `tidash`).

## Updates

Tidash checks for a new version at start and once a day, downloads it in the background, and offers Restart to update in Settings › Updates; it never installs on its own, not even on quit. Turn off Automatic updates there to stop the checks; Check now works either way. A release candidate is offered the next candidate and the release; a release is never offered a candidate.

Copies from the installer, the zip, the tar.gz or the DMG (in Applications) update themselves. A deb or rpm is your package manager's to update: install the new package from the releases page. An AppImage does not update itself: download the new one.

## TIDAL links

Tidash opens `tidal://track/<id>`, `tidal://album/<id>`, `tidal://artist/<id>`, `tidal://playlist/<uuid>` and `tidal://mix/<id>` links from a browser or another app; on the command line it also takes the same pages' links on tidal.com (`tidash https://tidal.com/album/<id>`). A link opens in the running Tidash, if there is one, once you are logged in; a track opens its album. The Windows installer, the deb and rpm, and the macOS app register the `tidal://` scheme. The official TIDAL app registers it too: on Windows the one installed last has the links, and Settings › Apps › Default apps hands them to the other.

## Bug reports

Attach `tidash.log` from the data directory: `~/Library/Application Support/tidash/` on macOS, `%APPDATA%\tidash\data\` on Windows, `~/.local/share/tidash/` on Linux (`$XDG_DATA_HOME/tidash/` if that is set). Each start replaces it, so copy it before starting Tidash again. After a crash, attach `panic.log` from the same directory too.

## Building and releasing

```sh
cargo run -p tidash
```

On Linux, install the build dependencies first:

```sh
sudo apt-get install -y libasound2-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

Packages, signing, the performance budget and each release: [docs/releasing.md](docs/releasing.md).

### Build-time keys

Last.fm scrobbling and the Discord status need keys compiled in. Set these environment variables when building; without them, those integrations are hidden in Settings. ListenBrainz needs none.

- `TIDASH_LASTFM_API_KEY` and `TIDASH_LASTFM_SECRET`: a Last.fm API account ([last.fm/api/account/create](https://www.last.fm/api/account/create)).
- `TIDASH_DISCORD_APP_ID`: the application ID of a Discord application named `Tidash` (its name is what "Listening to …" shows).

## Themes

Settings › Appearance offers Follow system, Dark (TIDAL's look, the default), Light and every palette in the `themes` folder beside `settings.json` in the config directory (Open themes folder opens it). The eight presets from fastframe (Nord, Catppuccin, Rose Pine, Tokyo Night, …) are installed there on the first run, and a palette file you drop in (a preset copied and edited is the easiest start) is listed the next time Settings opens. On Linux, Follow system uses Dark where the desktop reports no theme to the window (often on X11, and on some Wayland compositors); pick Light there explicitly.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
