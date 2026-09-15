<div align="center">

<img src="public/media/orivo-ring-icon.png" width="88" alt="" />

# Orivo

**Every game you own, in one place that looks like it was made for them.**

A local-first game library for macOS, Windows and Linux. In a beautiful
full-screen selector built to be read across a room and driven with a
controller.

[![Version](https://img.shields.io/badge/version-0.3.0-7D54F4)](https://github.com/justeozan/orivo/releases)
[![Runtime](https://img.shields.io/badge/runtime-Tauri%20v2-24C8DB)](https://tauri.app)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20·%20Windows%20·%20Linux-eeeeee)](#install)
[![Licence](https://img.shields.io/badge/licence-PolyForm%20Noncommercial%201.0.0-f0b429)](LICENSE)

</div>

<p align="center">
  <img src="https://img.shields.io/badge/Steam-171A21?logo=steam&logoColor=white" alt="Steam" />
  <img src="https://img.shields.io/badge/Epic%20Games-313131?logo=epicgames&logoColor=white" alt="Epic Games" />
  <img src="https://img.shields.io/badge/GOG-86328A?logo=gogdotcom&logoColor=white" alt="GOG" />
  <img src="https://img.shields.io/badge/Ubisoft%20Connect-0F0F0F?logo=ubisoft&logoColor=white" alt="Ubisoft Connect" />
  <img src="https://img.shields.io/badge/Xbox-107C10?logo=xbox&logoColor=white" alt="Xbox" />
  <img src="https://img.shields.io/badge/Microsoft%20Store-0067B8?logo=windows&logoColor=white" alt="Microsoft Store" />
  <img src="https://img.shields.io/badge/Instant%20Gaming-C4122E?logo=instant&logoColor=white" alt="Instant Gaming" />
</p>

---

![The Orivo library](docs/screenshots/library.png)
![The Orivo store](docs/screenshots/store.png)


## Install

Latest build on the [releases page](https://github.com/justeozan/orivo/releases).

| Platform | File |
| --- | --- |
| macOS (Apple Silicon) | Orivo_aarch64.dmg |
| macOS (Intel) | Orivo_x64.dmg |
| Windows | Orivo_x64-setup.exe |
| Linux | .AppImage, .deb, .rpm |

Not signed yet. macOS: right-click → Open (if "damaged": `xattr -dr com.apple.quarantine /Applications/Orivo.app`). Windows: More info → Run anyway. Self-updating afterwards.

## Build

```
pnpm install
pnpm tauri dev
```

Node 22, pnpm 11, stable Rust. Linux also needs `libwebkit2gtk-4.1-dev` and `libgtk-3-dev`.

## Licence

Source-available, [PolyForm Noncommercial 1.0.0](https://polyformproject.org/licenses/noncommercial/1.0.0/) — free for any noncommercial use, no commercial use. Commercial licences: contact@oneiby.com. © 2026 Ozan Sahin.

Steam, Epic, GOG, Ubisoft, Xbox, Microsoft Store and Instant Gaming are trademarks of their owners; Orivo is independent and unaffiliated.
