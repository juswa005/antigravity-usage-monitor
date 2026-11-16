# Antigravity Usage Monitor (`agmonitor`)

[![CI](https://img.shields.io/badge/build-Arch%20%7C%20Debian%20%7C%20NixOS-6c9994)](.github/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-98c379)](LICENSE)

A fast, lightweight, native Linux monitoring utility for **Google Antigravity CLI (`agy`)** built in **Rust + Ratatui** for **Arch Linux, Debian/Ubuntu, NixOS**, Wayland (Sway & Hyprland), Waybar, and Kitty/any terminal.

It gives you:
- **Waybar custom module** showing live remaining `%` + reset countdown (`󰚩 AG 80% · 02h 53m`) with warning/critical colors and rich hover tooltips
- **Ratatui TUI dashboard** with live quota bars for all Antigravity model pools (`Gemini`, `Claude & GPT`, `Gemini 4 Argon`)
- **GitHub-style Green Dots Activity Heatmap** showing daily Antigravity intensity (switchable between Fresh Tokens, Combined Tokens, and Planner Steps) with active streaks and per-day inspection
- **Persistent SQLite history** (`~/.local/share/agmonitor/history.db`) for daily, weekly, and snapshot tracking
- Native packaging for **Arch (`PKGBUILD`)**, **Debian/Ubuntu (`.deb` / `install.sh`)**, and **Nix Flakes / NixOS / Home Manager**

---

## Requirements

- Linux (Wayland optional — CLI & TUI work over SSH/TTY too)
- [Google Antigravity CLI (`agy`)](https://antigravity.google/docs/cli/reference) installed and signed in
- Optional for desktop bar integration: `waybar`, `kitty` (or any terminal), and a Nerd Font (e.g. JetBrainsMono Nerd Font)

---

## What Metrics Are Collected

`agmonitor` strictly uses **official `agy` CLI interfaces and local Antigravity records**. It never modifies Antigravity auth files, never reads/extracts OAuth secret tokens, never probes private `v1internal` endpoints, and never sends AI inference prompts to check usage.

| Metric | Status | Source |
| :--- | :--- | :--- |
| **5-Hour & Weekly Quota % + Reset Countdown (Gemini, Claude/GPT, Argon pools)** | **Actual** | Official `agy` interactive `/usage` panel via bounded headless PTY (`fetch_tui_usage = true`) |
| **Subscription / Plan Tier** | **Actual** | `agy /usage` (e.g. `Google AI Pro`) |
| **Authenticated account email & auth state** | **Actual** | `~/.gemini/antigravity-cli/log/cli-*.log`, `/usage`, + `antigravity-oauth-token` stat |
| **OAuth access expiry timestamp** | **Actual** | `~/.gemini/antigravity-cli/log/cli-*.log` |
| **Live model catalog & availability** | **Actual** | `agy models` + `agy --version` |
| **Active selected model** | **Actual** | `~/.gemini/antigravity-cli/settings.json` |
| **Input / Cache Read / Output / Thinking / Response tokens** | **Actual** | `~/.gemini/antigravity-cli/brain/*/transcript.jsonl` + `conversations/*.db` (`gen_metadata`) |
| **GitHub-style Green Dots Activity Heatmap & Streaks** | **Actual** | SQLite `daily_aggregates` (switchable: Fresh Tokens, Combined Tokens, Planner Steps) |
| **Active session context % (optional)** | **Actual (opt-in)** | `agmonitor statusline-hook` via `agy`'s `/statusline` integration |
| **Remaining token allowance** | **Unavailable** | Antigravity enforces percentage compute quotas via `/usage`, not raw token balances |

> [!NOTE]
> Fetching `/usage` spawns `agy` in a bounded headless PTY (`usage_timeout_secs = 15`), captures `Models & Quota`, and exits cleanly without creating conversations or spending tokens. When offline, `agmonitor` serves the last cached snapshot marked `stale` (`~` or `⚠`) and ticks down reset countdowns from cached `refreshes_at` timestamps.

---

## Installation

### Option 1: Arch Linux

Build & install directly with `makepkg` using [packaging/arch/PKGBUILD](packaging/arch/PKGBUILD):

```bash
sudo pacman -S --needed rust base-devel
git clone https://github.com/juswa005/antigravity-usage-monitor.git
cd antigravity-usage-monitor/packaging/arch
makepkg -si

# Optional: enable 5-minute background refresh timer
systemctl --user enable --now agmonitor-refresh.timer
```

Or install to `~/.local/bin` via `./scripts/install.sh --with-timer`.

### Option 2: Debian / Ubuntu (and derivatives)

SQLite is bundled via `rusqlite`, so only a standard C compiler + Rust (`rustup` or `cargo`) are needed:

```bash
sudo apt update && sudo apt install -y build-essential git curl
# If cargo is not installed yet:
# curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

git clone https://github.com/juswa005/antigravity-usage-monitor.git
cd antigravity-usage-monitor
./scripts/install.sh --with-timer
```

To build a `.deb` package (using `[package.metadata.deb]` in [Cargo.toml](Cargo.toml)):

```bash
cargo install cargo-deb
cargo deb
sudo dpkg -i target/debian/antigravity-usage-monitor_*.deb
```

Make sure `~/.local/bin` is in your `PATH` if using `scripts/install.sh`.

### Option 3: NixOS / Nix Flakes

```bash
# Run directly without installing
nix run github:juswa005/antigravity-usage-monitor -- status

# Install into your user profile
nix profile add github:juswa005/antigravity-usage-monitor
```

Or add to your system `flake.nix` (`nixosModules.default` or `homeManagerModules.default` in [nix/hm-module.nix](nix/hm-module.nix)):

```nix
inputs.antigravity-usage-monitor = {
  url = "github:juswa005/antigravity-usage-monitor";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

```nix
# NixOS configuration
services.antigravity-usage-monitor = {
  enable = true;
  enableUserTimer = true;
};
```

### Option 4: Cargo (Any Linux Distro)

```bash
cargo install --git https://github.com/juswa005/antigravity-usage-monitor.git
```

---

## Configuration

Copy the sample configuration to your XDG config directory:

```bash
mkdir -p ~/.config/agmonitor
cp config/config.toml ~/.config/agmonitor/config.toml
```

XDG Paths used:
- Config: `${XDG_CONFIG_HOME:-~/.config}/agmonitor/config.toml`
- Cache: `${XDG_CACHE_HOME:-~/.cache}/agmonitor/snapshot.json`
- History DB: `${XDG_DATA_HOME:-~/.local/share}/agmonitor/history.db`

Key options in [config/config.toml](config/config.toml):
- `general.agy_binary = "agy"`: Automatically checks `PATH` plus `~/.gemini/antigravity-cli/bin/agy`, `~/.local/bin/agy`, `/usr/local/bin/agy`, and `/usr/bin/agy`.
- `general.refresh_interval_secs = 300`: Live refresh TTL (5 minutes).
- `general.fetch_tui_usage = true`: Collect 5h/weekly `%` and reset timers from `agy /usage`.
- `general.mask_email = true`: Mask email in UI/Waybar (`us***@example.com`).
- `waybar.display_mode = "quota"`:
  - `"quota"`: Active model's limiting `%` + reset countdown (e.g. `󰚩 AG 80% · 02h 53m`)
  - `"pools"`: All 3 pools side-by-side (`󰚩 G:80% C:100% A:24%`)
  - `"tokens_5h"`, `"tokens_today"`, `"model"`, `"compact"`
- `waybar.warning_remaining_pct = 30.0` / `critical_remaining_pct = 10.0`.

---

## Waybar, Sway & Hyprland Setup

Ready-to-merge templates are included in [nix/waybar-snippet.jsonc](nix/waybar-snippet.jsonc), [nix/waybar-style.css](nix/waybar-style.css), and [nix/sway-rules.conf](nix/sway-rules.conf).

1. **Back up your existing Waybar config:**
   ```bash
   cp ~/.config/waybar/config.jsonc ~/.config/waybar/config.jsonc.bak
   cp ~/.config/waybar/style.css ~/.config/waybar/style.css.bak
   ```

2. **Add `"custom/antigravity"` to `"modules-right"` in `~/.config/waybar/config.jsonc`:**
   ```jsonc
   "custom/antigravity": {
     "exec": "agmonitor waybar --non-blocking",
     "return-type": "json",
     "format": "{}",
     "interval": 30,
     "signal": 10,
     "on-click": "kitty --class agmonitor -e agmonitor",
     "on-click-right": "agmonitor waybar --refresh",
     "on-click-middle": "kitty --class waybar-kitty -e agy",
     "tooltip": true
   }
   ```
   *(If `agmonitor` is installed to `~/.local/bin/agmonitor` and your Waybar systemd session doesn't include `~/.local/bin` in `PATH`, use the full path `~/.local/bin/agmonitor` or `/usr/bin/agmonitor`.)*
   - `--non-blocking` reads cached JSON in ~1ms and triggers a detached background refresh (`refresh.lock`-guarded) only when the 5-minute TTL expires, signaling Waybar (`SIGRTMIN+10`) when complete.
   - **Left click**: Opens `agmonitor` TUI in Kitty (replace `kitty --class agmonitor -e` with `alacritty`, `foot`, `wezterm`, etc. if preferred).
   - **Right click**: Refreshes immediately (`agy models` + `/usage`) and updates Waybar.
   - **Middle click**: Opens interactive `agy` in Kitty.

3. **Add CSS states from [nix/waybar-style.css](nix/waybar-style.css) to `~/.config/waybar/style.css`.**

4. **Optional floating rules** (see [nix/sway-rules.conf](nix/sway-rules.conf)):
   - **Sway**:
     ```sway
     for_window [app_id="agmonitor"] floating enable
     for_window [app_id="agmonitor"] resize set 1120 740
     for_window [app_id="agmonitor"] move position center
     ```
   - **Hyprland**:
     ```ini
     windowrulev2 = float, class:^(agmonitor)$
     windowrulev2 = size 1120 740, class:^(agmonitor)$
     windowrulev2 = center, class:^(agmonitor)$
     ```

5. **Reload Waybar & compositor:**
   ```bash
   swaymsg reload   # or hyprctl reload
   ```

---

## CLI Usage

```bash
agmonitor                             # Open interactive Ratatui TUI dashboard
agmonitor status                      # Quota pools + rolling token summary (--json)
agmonitor refresh                     # Force live refresh (`agy models` + `/usage`)
agmonitor models                      # Models list with 5h/Weekly % & token usage (--json)
agmonitor history --weeks 20          # ANSI green-dot heatmap + daily/weekly SQLite summaries
agmonitor history --metric steps      # Heatmap scaled by daily planner steps
agmonitor tokens                      # Detailed token stats & top conversations (--json)
agmonitor waybar --non-blocking       # Emit single-line Waybar JSON immediately (<10ms)
agmonitor doctor                      # Verify agy, auth, PTY /usage, SQLite, Waybar/Kitty
```

### TUI Keyboard Shortcuts

- `1` / `d`: Dashboard (all 3 `/usage` pools + compact green-dot heatmap + models table)
- `2` / `m`: Models breakdown with per-model 5h & weekly limits
- `3` / `h`: Interactive GitHub-style green-dot heatmap + daily/weekly SQLite history
- `4` / `t`: Token & conversation statistics
- `5` / `x`: Doctor checks & data provenance
- `g`: Cycle green-dot intensity metric (`Fresh Tokens` → `Combined Tokens` → `Planner Steps`)
- `[` / `]` (or `p` / `n`): Inspect individual days on the History tab heatmap
- `Tab` / `←` `→`: Cycle tabs
- `↑` `↓` / `j` `k`: Scroll table
- `r`: Force background refresh
- `u`: Open `agy` in Kitty
- `?`: Help overlay
- `q` / `Ctrl+C`: Quit

---

## Troubleshooting

- **`agmonitor doctor` reports `Not Logged In` or `Expired`:**
  Run `agy` interactively and sign in via the OAuth prompt, then run `agmonitor refresh`.
- **Waybar shows `~` or `⚠`:**
  `~` means the cached snapshot is older than `refresh_interval_secs`; `⚠` means the last live check failed (e.g. offline) and `agmonitor` is serving the last known cached snapshot.
- **Waybar doesn't display the module:**
  Verify `agmonitor` is on Waybar's `PATH` (or use the full binary path in `exec`) and ensure `agmonitor waybar --non-blocking` prints single-line valid JSON.

---

## Removal

- **Arch (`pacman`)**: `sudo pacman -R antigravity-usage-monitor`
- **Debian/Ubuntu (`.deb`)**: `sudo dpkg -r antigravity-usage-monitor`
- **Nix profile**: `nix profile remove antigravity-usage-monitor`
- **Manual / `install.sh`**: `rm -f ~/.local/bin/agmonitor ~/.config/systemd/user/agmonitor-refresh.*`
- **Config & data**: `rm -rf ~/.config/agmonitor ~/.cache/agmonitor ~/.local/share/agmonitor`
