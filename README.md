# GRAFT

**GRAFT - Graphical Robocopy Assured File Transfer Tool**

> **Note:** This application was developed with the assistance of AI (GitHub Copilot / Claude).

![Graft Screenshot](docs/screenshot.png)

## Overview

Graft is a Windows GUI application that provides a user-friendly interface for Microsoft's Robocopy command-line utility. It includes SHA-256 hash verification to ensure file integrity after transfers, making it ideal for critical data migrations and backups.

## Features

- **Intuitive GUI** - No need to memorize Robocopy command-line flags
- **Preset Configurations** - Quick access to common copy scenarios:
  - Large Files over WAN
  - Mirror with Full Metadata
  - Copy All & Preserve Attributes
  - Incremental Backup
  - Quick Copy
- **SHA-256 Hash Verification** - Verify file integrity after copy operations
- **Real-time Console Output** - Watch Robocopy progress live
- **Command History** - Save and reuse frequently used configurations
- **Log Export** - Save operation logs for auditing
- **Transfer Statistics** - Summary of files/dirs copied, skipped, failed, and extras
- **Menu Bar** - File menu (Export Log, Exit) and Help menu (About with version info)
- **Destructive Option Guardrails** - Warnings and confirmation before destructive runs
- **Material Design Dark Theme** - Easy on the eyes during long operations

## Requirements

- Windows 10/11 (Robocopy is included with Windows)
- No installation required - single executable

## Installation

1. Download the latest release from the [Releases](https://github.com/brettrbarker/graft/releases) page
2. Run `graft.exe`

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) (1.70 or later)

### Build

```powershell
git clone https://github.com/brettrbarker/graft.git
cd graft
cargo build --release
```

The executable will be at `target/release/graft.exe`

## Usage

1. **Select Source and Destination** - Use the Browse buttons or type paths directly
2. **Choose a Preset** - Or manually configure options in the collapsible sections
3. **Enable Hash Verification** (optional) - Check "Verify with SHA-256 hashing" for critical transfers
4. **Click Run Robocopy** - Monitor progress in the Console Output panel

### Robocopy Exit Codes

| Code  | Meaning                                              |
| ----- | ---------------------------------------------------- |
| 0     | No files copied - source and destination are in sync |
| 1     | Files copied successfully                            |
| 2     | Extra files or directories detected in destination  |
| 3     | Files copied and extra files detected               |
| 4     | Mismatched files or directories detected            |
| 5-7   | Files copied with some issues                       |
| 8-15  | Some files could not be copied (errors occurred)    |
| 16+   | Serious error - no files were copied                |

## Configuration Options

### Copy Options

- `/S` - Copy subdirectories (excluding empty)
- `/E` - Copy subdirectories (including empty)
- `/Z` - Restartable mode (survives network glitches)
- `/B` - Backup mode (requires backup privileges)
- `/J` - Unbuffered I/O (recommended for large files)

### File Selection

- `/COPY:flags` - What to copy (D=Data, A=Attributes, T=Timestamps, S=Security)
- `/MIR` - Mirror mode (sync source to destination)
- `/PURGE` - Delete destination files not in source

### Performance

- `/MT:n` - Multi-threaded copy with n threads (default 8)
- `/R:n` - Number of retries on failed copies
- `/W:n` - Wait time between retries (seconds)

## Project Structure

```text
graft/
├── Cargo.toml          # Rust dependencies and metadata
├── build.rs            # Build script for Windows icon
├── README.md           # This file
└── src/
    ├── main.rs         # Application entry point and icon
    ├── app.rs          # Main GUI application logic
    ├── robocopy.rs     # Robocopy options and command building
    ├── hasher.rs       # SHA-256 file hashing and verification
    └── history.rs      # Command history management
```

## License

MIT License - See [LICENSE](LICENSE) for details.

## Acknowledgments

- Built with [egui](https://github.com/emilk/egui) and [eframe](https://github.com/emilk/egui/tree/master/crates/eframe)
- File dialogs by [rfd](https://github.com/PolyMeilex/rfd)
- Developed with assistance from GitHub Copilot (Claude/ChatGPT)

## Security Notes

- This project uses `cargo audit` for dependency advisories; run `cargo audit` locally.
- Current advisory status: no findings
