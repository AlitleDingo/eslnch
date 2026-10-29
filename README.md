# eslnch

Fast, cross-platform app launcher for the terminal. Type a name, the app opens detached, and your terminal stays free.

```
 ___  ___  _      _  _    ___  _  _
| __|/ __|| |    | \| |  / __|| || |
| _| \__ \| |__  | .` | | (__ | __ |
|___||___/|____| |_|\_|  \___||_||_|
```

## Features

- **Direct launch:** `eslnch blender` starts the app in the background and returns your prompt.
- **Interactive menu:** run `eslnch` with no arguments for a two-column picker with fuzzy search.
- **Auto-detect:** `eslnch scan` finds installed apps on Linux, macOS and Windows.
- **Simple config:** one TOML file; add, update and remove aliases from the CLI.
- **Smart matching:** exact name, unique prefix, or unique substring.

## Install

Requires [Rust](https://rustup.rs) (stable).

```sh
git clone https://github.com/<your-username>/eslnch.git
cd eslnch
cargo build --release
```

The binary is at `target/release/eslnch` (`eslnch.exe` on Windows).

### Add to PATH

**Linux / macOS**

```sh
mkdir -p ~/.local/bin
cp target/release/eslnch ~/.local/bin/
```

If `~/.local/bin` isn't on your PATH, append it (use `~/.zshrc` on macOS) and open a new terminal:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

**Windows**

Copy `eslnch.exe` to `%USERPROFILE%\.cargo\bin` (already on PATH with a normal Rust install), or add its folder under Settings → System → About → Advanced system settings → Environment Variables → User `Path` → Edit → New.

**Any platform:** `cargo install --path .` installs to `~/.cargo/bin`.

## Usage

| Command | Description |
|---|---|
| `eslnch` | Open the interactive menu |
| `eslnch <name>` | Launch an app (exact, prefix or unique substring match) |
| `eslnch scan` | Detect installed apps and register new ones (never overwrites existing entries) |
| `eslnch ls` | List registered apps and their paths |
| `eslnch add <name> <path>` | Add or update an app |
| `eslnch remove <name>` | Remove an app |
| `eslnch run <name>` | Launch an app whose name clashes with a subcommand |
| `eslnch help` | Show all commands |

### Examples

```sh
eslnch scan
eslnch blender
eslnch add code "code --new-window"
eslnch add notes /usr/bin/gedit
eslnch remove notes
```

`<path>` can be an executable path or a command with arguments.

### Menu controls

| Key | Action |
|---|---|
| Arrow keys | Move through the grid |
| Type | Filter apps (fuzzy) |
| Enter | Launch |
| Esc / Ctrl+C | Quit |

## How it works

- **Config:** saved as TOML in your user config directory:
  - Linux: `~/.config/eslnch/config.toml`
  - macOS: `~/Library/Application Support/eslnch/config.toml`
  - Windows: `%APPDATA%\eslnch\config.toml`
- **Scan sources:**
  - Linux: `.desktop` files (system, user, Flatpak, Snap)
  - macOS: `.app` bundles in `/Applications`, `~/Applications` and system folders
  - Windows: Start Menu shortcuts (`.lnk`)
- **Detached launch:** `setsid` on Unix, `DETACHED_PROCESS` on Windows, and `open -a` for macOS apps. Output is discarded, so closing the terminal won't kill the app.
- `eslnch` never deletes files or runs shell commands on your behalf; `remove` only edits the config entry.

## License

MIT, or choose your own. Add a `LICENSE` file to match.
