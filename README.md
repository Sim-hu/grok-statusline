# grok-statusline

[English](README.md) | [日本語](README.ja.md)

Claude-style status line at the bottom of [Grok Build](https://github.com/xai-org/grok). Written in Rust.

Grok has no `/statusline` hook. Install leaves the official `~/.grok/bin/grok` alone, puts a wrapper at `~/.local/bin/grok`, and prepends that directory on `PATH` after the Grok installer block. Grok can update its own binary in place; the next `grok` launch wraps the new build. **No alias.**

```
[Grok 4.6]  resources  main +2 ~1  high
████░░░░░░ 24% 124k/500k │ use 4% wk 6d │ 14m │ +709/-21
```

## Install

### From source

Rust 1.74+.

```bash
git clone https://github.com/Sim-hu/grok-statusline.git
cd grok-statusline
cargo install --path .
grok-statusline install
```

Then launch `grok` as usual.

### Nix

```bash
nix profile install github:Sim-hu/grok-statusline
grok-statusline install

nix run github:Sim-hu/grok-statusline -- install
nix develop github:Sim-hu/grok-statusline
```

`default.nix` works with `nix-build` if you are not using flakes.

To remove:

```bash
grok-statusline uninstall
```

Grok auto-updates do not drop the bar. The official binary stays at `~/.grok/bin/grok`. Re-run `grok-statusline install` only if Grok's installer appends another `PATH` block at the end of your shell rc (that moves our block back to the end).

This shell still has the old `PATH` until you open a new terminal, or:

```bash
export PATH="$HOME/.local/bin:$PATH"
hash -r
```

## Usage

```bash
grok                            # normal launch, with the bar
grok-statusline once            # print the bar once
grok-statusline dump-json       # session JSON for custom scripts
grok-statusline once --no-usage
grok-statusline once --height 1
```

## Config

`~/.grok/grok-statusline.json`, or `<cwd>/.grok/grok-statusline.json` for a project.

```json
{
  "statusLine": {
    "type": "builtin",
    "padding": 0,
    "refreshInterval": 1,
    "height": 2,
    "barWidth": 10
  },
  "usage": { "enabled": true },
  "git": { "enabled": true, "cacheSeconds": 5, "untracked": false }
}
```

Same contract as Claude Code for a custom command:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/src/grok-statusline/examples/statusline.sh"
  }
}
```

JSON on stdin, bar on stdout. `COLUMNS` / `LINES` are set. After 200ms or a non-zero exit, the builtin renderer is used.

## JSON

See `grok-statusline dump-json`.

| Field | Meaning |
|------|------|
| `model.id` / `model.display_name` | `grok-4.6` / `Grok 4.6` |
| `workspace.current_dir` | cwd |
| `workspace.repo` | origin host / owner / name |
| `context_window.used_percentage` | context fill |
| `cost.total_duration_ms` | session age |
| `cost.total_lines_added` / `removed` | +/- |
| `rate_limits.seven_day` | weekly credits |
| `effort.level` | e.g. `high` |
| `git.*` | branch, staged, modified |

## Development

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

CI on `main` and pull requests runs rustfmt, clippy, tests, a release build, and shellcheck.

## License

MIT
