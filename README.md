# Agent of Empires for Mac

A small personal fork of [Agent of Empires](https://github.com/agent-of-empires/agent-of-empires) for people who use Claude Code and Codex in the terminal.

![macOS](https://img.shields.io/badge/macOS-Apple_Silicon_%26_Intel-000000?logo=apple&logoColor=white)
[![Download](https://img.shields.io/github/v/release/Reeezer/agent-of-empires?label=download)](https://github.com/Reeezer/agent-of-empires/releases/latest)

![The Agent of Empires terminal dashboard](docs/assets/demo.gif)

## ✨ Features

- Keep your Claude Code and Codex sessions in one terminal dashboard.
- See estimated API costs and context usage beside each session.
- Tell when Claude is waiting on a background command or monitor.
- Switch between projects and branches. Sessions keep running when you close the dashboard.

## 📦 Installation

Requires a **Mac**, **Homebrew**, and **Claude Code or Codex** already installed and signed in. Downloads are available for Apple Silicon and Intel; Rust is not needed.

```sh
brew install tmux node
curl -fsSL https://raw.githubusercontent.com/Reeezer/agent-of-empires/main/scripts/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"
aoe
```

The installer also prepares the cost helper, [ccusage](https://github.com/ryoppippi/ccusage). Add the `export PATH` line to `~/.zshrc` to keep `aoe` available in new terminals.

To update, run `aoe update` and reopen the dashboard. This keeps the Mac fork's additions. The install command also works for updates.

For a new official AoE release, ask Claude Code or Codex in this repository: "Update to the latest stable AoE release and publish the Mac build, keeping this fork's additions." The maintenance instructions are in [AGENTS.md](AGENTS.md); the [Mac release action](https://github.com/Reeezer/agent-of-empires/actions/workflows/macos-release.yml) builds and publishes both downloads.

## 🚀 Usage

- Run `aoe` to open the dashboard.
- Press `n` to create a session, choose Claude Code or Codex, and select your project.
- Press `Enter` to open a session and `Ctrl+b d` to return to the dashboard.
- Read `$4.32 46%` as the session's estimated API cost and context usage. Costs refresh every 15 seconds; `?` means unavailable.
- A spinning icon in the favorite color means Claude is waiting on background work. A working agent stays green.
- Press `?` for all shortcuts.

Cost estimates are equivalent API spend, including for subscription accounts. Context percentages are estimates based on the model and the latest reply.

## 🙏 Credits

Built on [Agent of Empires](https://github.com/agent-of-empires/agent-of-empires), with a few additions for my daily terminal workflow. See the [upstream guides](https://www.agent-of-empires.com/docs/quick-start/) for the rest of AoE.

The upstream [MIT license](LICENSE), [third-party notices](THIRD_PARTY_NOTICES.md), and [Apache 2.0 license](licenses/Apache-2.0.txt) apply.
